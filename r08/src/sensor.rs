//! Safe, non-DFU recording of the R08 A1 03 accelerometer notification stream.

use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use futures::StreamExt;
use serde::Serialize;
use tokio::time::{Duration, Instant};

use crate::ble::RingConnection;
use crate::protocol::{
    decode_accelerometer_packet, raw_sensor_start_packet, raw_sensor_stop_packet,
    AccelerometerSample,
};

#[derive(Debug, Clone)]
pub struct SensorRecordOptions {
    pub seconds: u64,
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SensorRecordSummary {
    pub output: PathBuf,
    pub requested_seconds: u64,
    pub captured_seconds: f64,
    pub sample_count: usize,
    pub effective_hz: f64,
    pub interval_mean_ms: f64,
    pub interval_min_ms: f64,
    pub interval_max_ms: f64,
    pub x_min: i16,
    pub x_max: i16,
    pub y_min: i16,
    pub y_max: i16,
    pub z_min: i16,
    pub z_max: i16,
    pub interrupted: bool,
    pub dfu_sent: bool,
}

#[derive(Debug, Default)]
struct SampleStats {
    count: usize,
    first_ms: Option<f64>,
    last_ms: Option<f64>,
    interval_sum_ms: f64,
    interval_min_ms: f64,
    interval_max_ms: f64,
    x_min: i16,
    x_max: i16,
    y_min: i16,
    y_max: i16,
    z_min: i16,
    z_max: i16,
}

impl SampleStats {
    fn add(&mut self, elapsed_ms: f64, sample: AccelerometerSample) -> Option<f64> {
        let delta_ms = self.last_ms.map(|last| elapsed_ms - last);
        if self.count == 0 {
            self.first_ms = Some(elapsed_ms);
            self.interval_min_ms = f64::INFINITY;
            self.x_min = sample.x;
            self.x_max = sample.x;
            self.y_min = sample.y;
            self.y_max = sample.y;
            self.z_min = sample.z;
            self.z_max = sample.z;
        } else if let Some(delta) = delta_ms {
            self.interval_sum_ms += delta;
            self.interval_min_ms = self.interval_min_ms.min(delta);
            self.interval_max_ms = self.interval_max_ms.max(delta);
            self.x_min = self.x_min.min(sample.x);
            self.x_max = self.x_max.max(sample.x);
            self.y_min = self.y_min.min(sample.y);
            self.y_max = self.y_max.max(sample.y);
            self.z_min = self.z_min.min(sample.z);
            self.z_max = self.z_max.max(sample.z);
        }
        self.count += 1;
        self.last_ms = Some(elapsed_ms);
        delta_ms
    }

    fn finish(
        self,
        output: PathBuf,
        requested_seconds: u64,
        captured_seconds: f64,
        interrupted: bool,
    ) -> SensorRecordSummary {
        let intervals = self.count.saturating_sub(1);
        let span_ms = match (self.first_ms, self.last_ms) {
            (Some(first), Some(last)) => (last - first).max(0.0),
            _ => 0.0,
        };
        SensorRecordSummary {
            output,
            requested_seconds,
            captured_seconds,
            sample_count: self.count,
            effective_hz: if intervals > 0 && span_ms > 0.0 {
                intervals as f64 * 1000.0 / span_ms
            } else {
                0.0
            },
            interval_mean_ms: if intervals > 0 {
                self.interval_sum_ms / intervals as f64
            } else {
                0.0
            },
            interval_min_ms: if intervals > 0 {
                self.interval_min_ms
            } else {
                0.0
            },
            interval_max_ms: if intervals > 0 {
                self.interval_max_ms
            } else {
                0.0
            },
            x_min: self.x_min,
            x_max: self.x_max,
            y_min: self.y_min,
            y_max: self.y_max,
            z_min: self.z_min,
            z_max: self.z_max,
            interrupted,
            dfu_sent: false,
        }
    }
}

pub async fn record(
    connection: &RingConnection,
    options: SensorRecordOptions,
) -> Result<SensorRecordSummary> {
    if options.seconds == 0 || options.seconds > 600 {
        bail!("采集时间必须在 1 到 600 秒之间");
    }
    let output = options.output.unwrap_or_else(default_output_path);
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建采集目录失败：{}", parent.display()))?;
    }
    let mut notifications = connection.subscribe().await?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .with_context(|| format!("创建采集文件失败（不会覆盖已有文件）：{}", output.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "elapsed_ms,delta_ms,x,y,z,magnitude")?;
    connection.write(&raw_sensor_start_packet()).await?;
    println!("SENSOR_STARTED LED 可能持续闪烁；仅记录 A1 03 三轴加速度");

    let started = Instant::now();
    let deadline = tokio::time::sleep(Duration::from_secs(options.seconds));
    tokio::pin!(deadline);
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let mut stats = SampleStats::default();
    let mut interrupted = false;
    let capture_result: Result<()> = async {
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                signal = &mut ctrl_c => {
                    signal.context("监听 Ctrl+C 失败")?;
                    interrupted = true;
                    break;
                }
                packet = notifications.next() => {
                    let Some(packet) = packet else {
                        bail!("BLE 通知流意外结束");
                    };
                    let Some(sample) = decode_accelerometer_packet(&packet) else {
                        continue;
                    };
                    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                    let delta_ms = stats.add(elapsed_ms, sample);
                    let magnitude = f64::from(sample.x).hypot(f64::from(sample.y)).hypot(f64::from(sample.z));
                    writeln!(
                        writer,
                        "{elapsed_ms:.3},{},{},{},{},{magnitude:.3}",
                        delta_ms.map(|value| format!("{value:.3}")).unwrap_or_default(),
                        sample.x,
                        sample.y,
                        sample.z,
                    )?;
                    writer.flush()?;
                    println!(
                        "SAMPLE t={:.2}s X={} Y={} Z={}",
                        elapsed_ms / 1000.0,
                        sample.x,
                        sample.y,
                        sample.z
                    );
                }
            }
        }
        Ok(())
    }
    .await;

    let stop_result = connection.write(&raw_sensor_stop_packet()).await;
    println!("SENSOR_STOP_REQUESTED 已发送 A1 02，LED 应停止闪烁");
    writer.flush()?;
    if let Err(error) = stop_result {
        return Err(error).context("采集结束后发送 A1 02 停止命令失败");
    }
    capture_result?;

    Ok(stats.finish(
        output,
        options.seconds,
        started.elapsed().as_secs_f64(),
        interrupted,
    ))
}

pub async fn stop(connection: &RingConnection) -> Result<()> {
    connection
        .write(&raw_sensor_stop_packet())
        .await
        .context("发送 A1 02 停止命令失败")
}

fn default_output_path() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    PathBuf::from(format!("captures/r08-sensor-{timestamp}.csv"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_effective_rate_and_ranges() {
        let mut stats = SampleStats::default();
        stats.add(100.0, AccelerometerSample { x: 1, y: 2, z: 3 });
        stats.add(600.0, AccelerometerSample { x: -4, y: 8, z: 1 });
        stats.add(1100.0, AccelerometerSample { x: 7, y: -2, z: 9 });
        let summary = stats.finish(PathBuf::from("test.csv"), 1, 1.1, false);
        assert_eq!(summary.sample_count, 3);
        assert!((summary.effective_hz - 2.0).abs() < 0.001);
        assert_eq!(summary.interval_mean_ms, 500.0);
        assert_eq!((summary.x_min, summary.x_max), (-4, 7));
        assert_eq!((summary.y_min, summary.y_max), (-2, 8));
        assert_eq!((summary.z_min, summary.z_max), (1, 9));
        assert!(!summary.dfu_sent);
    }
}
