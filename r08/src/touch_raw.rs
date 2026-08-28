//! Read-only observation of the RT08 touch controller's four diagnostic channels.

use anyhow::{bail, Context, Result};
use futures::StreamExt;
use tokio::time::{Duration, Instant, MissedTickBehavior};

use crate::ble::RingConnection;
use crate::protocol::{decode_touch_electrode_packet, touch_electrode_snapshot_packet};

pub const MIN_INTERVAL_MS: u64 = 250;
pub const MAX_INTERVAL_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy)]
pub struct TouchRawOptions {
    pub seconds: u64,
    pub interval_ms: u64,
}

pub async fn observe(connection: &RingConnection, options: TouchRawOptions) -> Result<usize> {
    if options.seconds == 0 || options.seconds > 600 {
        bail!("观察时间必须在 1 到 600 秒之间");
    }
    if !(MIN_INTERVAL_MS..=MAX_INTERVAL_MS).contains(&options.interval_ms) {
        bail!("触控原始值查询间隔必须在 {MIN_INTERVAL_MS} 到 {MAX_INTERVAL_MS} 毫秒之间");
    }

    let mut notifications = connection.subscribe().await?;
    let mut ticker = tokio::time::interval(Duration::from_millis(options.interval_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let deadline = tokio::time::sleep(Duration::from_secs(options.seconds));
    tokio::pin!(deadline);
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let started = Instant::now();
    let query = touch_electrode_snapshot_packet();
    let mut sample_count = 0usize;

    loop {
        tokio::select! {
            _ = &mut deadline => break,
            signal = &mut ctrl_c => {
                signal.context("监听 Ctrl+C 失败")?;
                break;
            }
            _ = ticker.tick() => {
                connection
                    .write(&query)
                    .await
                    .context("发送只读 A1 03 触控四通道快照查询失败")?;
            }
            packet = notifications.next() => {
                let Some(packet) = packet else {
                    bail!("BLE 通知流意外结束");
                };
                let Some(sample) = decode_touch_electrode_packet(&packet) else {
                    continue;
                };
                sample_count += 1;
                let (minimum_index, minimum) = sample
                    .channels
                    .iter()
                    .copied()
                    .enumerate()
                    .min_by_key(|(_, value)| *value)
                    .expect("four channels");
                let (maximum_index, maximum) = sample
                    .channels
                    .iter()
                    .copied()
                    .enumerate()
                    .max_by_key(|(_, value)| *value)
                    .expect("four channels");
                println!(
                    "TOUCH_RAW t={:.3}s C1={} C2={} C3={} C4={} VALID={} MIN=C{}:{} MAX=C{}:{} SPREAD={}",
                    started.elapsed().as_secs_f64(),
                    sample.channels[0],
                    sample.channels[1],
                    sample.channels[2],
                    sample.channels[3],
                    sample.channels_valid,
                    minimum_index + 1,
                    minimum,
                    maximum_index + 1,
                    maximum,
                    maximum.saturating_sub(minimum),
                );
            }
        }
    }

    if sample_count == 0 {
        bail!(
            "没有收到 A1 04 触控四通道快照；当前固件可能不支持该只读诊断入口，或通知通道已被其他设备占用"
        );
    }
    Ok(sample_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_bounds_are_conservative() {
        assert_eq!(MIN_INTERVAL_MS, 250);
        assert_eq!(MAX_INTERVAL_MS, 5_000);
    }
}
