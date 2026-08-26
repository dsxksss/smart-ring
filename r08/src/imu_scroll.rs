//! Fail-closed host support for the experimental A2/10 IMU-only stream.
//!
//! Nothing in this module flashes firmware. `run` transmits the candidate's
//! ordinary UART start/stop command only after the CLI's explicit opt-in.

use std::f64::consts::PI;
use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use tokio::sync::mpsc;

use crate::ble::RingConnection;
use crate::platform::create_injector;
use crate::platform::inject::{Injector, NullInjector};
use crate::protocol::{
    format_packet, imu_stream_start_packet, imu_stream_stop_packet, AccelerometerSample,
    ImuStreamEvaluation, ImuStreamSample, ImuStreamStopReason, ImuStreamTracker,
};

pub const EXPECTED_HARDWARE: &str = "RT08_V3.1";
pub const EXPECTED_FIRMWARE: &str = "RT08_3.10.48_260309";
pub const STREAM_RENEW_MS: u64 = 8_000;
const STREAM_FIRST_SAMPLE_TIMEOUT_MS: u64 = 1_500;
const MAX_STREAM_RECOVERIES: u8 = 2;
const CALIBRATION_SAMPLES: usize = 10;
// v7's first real packets put 1 g near 8192 raw counts (for example
// 8148, -388, 196). Fast hand motion adds linear acceleration and can push the
// vector well above static 1 g; keep a deliberately broad but finite envelope
// so those real transients are accepted while a zeroed or corner-saturated
// vector still stops the stream.
const MIN_GRAVITY_NORM: f64 = 2_000.0;
const MAX_GRAVITY_NORM: f64 = 45_000.0;
const BASELINE_ADAPTATION: f64 = 0.02;
const FILTER_ALPHA: f64 = 0.35;
const MAX_WHEEL_PER_SAMPLE: f64 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationPlane {
    Xy,
    Xz,
    Yz,
}

impl FromStr for RotationPlane {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "xy" => Ok(Self::Xy),
            "xz" => Ok(Self::Xz),
            "yz" => Ok(Self::Yz),
            _ => bail!("旋转平面必须是 xy、xz 或 yz"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ImuWheelConfig {
    pub plane: RotationPlane,
    pub invert: bool,
    pub deadzone_degrees: f64,
    pub full_speed_degrees: f64,
    pub gain: f64,
}

impl Default for ImuWheelConfig {
    fn default() -> Self {
        Self {
            // Existing no-flash recordings showed the clearest rotation in X/Z.
            plane: RotationPlane::Xz,
            invert: false,
            deadzone_degrees: 6.0,
            full_speed_degrees: 40.0,
            gain: 1.0,
        }
    }
}

impl ImuWheelConfig {
    pub fn validate(self) -> Result<Self> {
        if !(1.0..=30.0).contains(&self.deadzone_degrees) {
            bail!("deadzone 必须在 1..=30 度");
        }
        if !(self.deadzone_degrees + 1.0..=120.0).contains(&self.full_speed_degrees) {
            bail!("full-speed 必须大于 deadzone 且不超过 120 度");
        }
        if !(0.1..=4.0).contains(&self.gain) {
            bail!("gain 必须在 0.1..=4.0");
        }
        Ok(self)
    }
}

#[derive(Debug)]
pub struct ImuWheelMapper {
    config: ImuWheelConfig,
    calibration_sin: f64,
    calibration_cos: f64,
    calibration_count: usize,
    baseline_angle: Option<f64>,
    filtered_offset: f64,
    wheel_remainder: f64,
}

impl ImuWheelMapper {
    pub fn new(config: ImuWheelConfig) -> Result<Self> {
        Ok(Self {
            config: config.validate()?,
            calibration_sin: 0.0,
            calibration_cos: 0.0,
            calibration_count: 0,
            baseline_angle: None,
            filtered_offset: 0.0,
            wheel_remainder: 0.0,
        })
    }

    pub fn calibrated(&self) -> bool {
        self.baseline_angle.is_some()
    }

    /// Convert one validated stream sample into a high-resolution wheel delta.
    /// `None` means the sample is physically implausible and the caller must
    /// fail closed. `Some(0)` is a valid stationary/dead-zone sample.
    pub fn update(&mut self, sample: ImuStreamSample) -> Option<i32> {
        let acceleration = sample.acceleration;
        if !plausible_gravity(acceleration) {
            return None;
        }
        let angle = plane_angle(acceleration, self.config.plane);
        if self.baseline_angle.is_none() {
            self.calibration_sin += angle.sin();
            self.calibration_cos += angle.cos();
            self.calibration_count += 1;
            if self.calibration_count >= CALIBRATION_SAMPLES {
                self.baseline_angle = Some(self.calibration_sin.atan2(self.calibration_cos));
            }
            return Some(0);
        }

        let mut offset = wrap_angle(angle - self.baseline_angle.expect("calibrated"));
        if self.config.invert {
            offset = -offset;
        }
        self.filtered_offset += FILTER_ALPHA * (offset - self.filtered_offset);

        let deadzone = self.config.deadzone_degrees.to_radians();
        if self.filtered_offset.abs() <= deadzone {
            // Re-center very slowly only while the ring is already stationary.
            let baseline = self.baseline_angle.expect("calibrated");
            self.baseline_angle = Some(wrap_angle(
                baseline + BASELINE_ADAPTATION * wrap_angle(angle - baseline),
            ));
            self.wheel_remainder = 0.0;
            return Some(0);
        }

        let full_speed = self.config.full_speed_degrees.to_radians();
        let normalized =
            ((self.filtered_offset.abs() - deadzone) / (full_speed - deadzone)).clamp(0.0, 1.0);
        let magnitude = normalized * MAX_WHEEL_PER_SAMPLE * self.config.gain;
        self.wheel_remainder += self.filtered_offset.signum() * magnitude;
        let bounded = self.wheel_remainder.clamp(-120.0, 120.0);
        let delta = bounded.trunc() as i32;
        self.wheel_remainder -= f64::from(delta);
        Some(delta)
    }
}

fn plausible_gravity(sample: AccelerometerSample) -> bool {
    let x = f64::from(sample.x);
    let y = f64::from(sample.y);
    let z = f64::from(sample.z);
    let norm = (x * x + y * y + z * z).sqrt();
    (MIN_GRAVITY_NORM..=MAX_GRAVITY_NORM).contains(&norm)
}

fn plane_angle(sample: AccelerometerSample, plane: RotationPlane) -> f64 {
    let (a, b) = match plane {
        RotationPlane::Xy => (sample.x, sample.y),
        RotationPlane::Xz => (sample.x, sample.z),
        RotationPlane::Yz => (sample.y, sample.z),
    };
    f64::from(b).atan2(f64::from(a))
}

fn wrap_angle(angle: f64) -> f64 {
    (angle + PI).rem_euclid(2.0 * PI) - PI
}

pub struct ImuStreamOptions {
    pub seconds: u64,
    pub inject: bool,
    pub config: ImuWheelConfig,
}

pub async fn run(connection: RingConnection, options: ImuStreamOptions) -> Result<()> {
    let mut mapper = ImuWheelMapper::new(options.config)?;
    if let Err(error) = verify_device_identity(&connection).await {
        let _ = connection.disconnect().await;
        return Err(error);
    }
    let mut notifications = match connection.subscribe().await {
        Ok(notifications) => notifications,
        Err(error) => {
            let _ = connection.disconnect().await;
            return Err(error).context("订阅候选 IMU 通知失败");
        }
    };
    let mut injector: Box<dyn Injector> = if options.inject {
        match create_injector() {
            Ok(injector) => injector,
            Err(error) => {
                let _ = connection.disconnect().await;
                return Err(error).context("创建滚轮注入后端失败");
            }
        }
    } else {
        Box::new(NullInjector)
    };
    let started = Instant::now();
    let mut tracker = ImuStreamTracker::default();
    let mut last_renew_ms = 0u64;
    let mut waiting_for_first_sample = true;
    let mut stream_started_at_ms = 0u64;
    let mut stream_recoveries = 0u8;
    let start_packet = imu_stream_start_packet();
    let stop_packet = imu_stream_stop_packet();

    if let Err(error) = connection.write(&start_packet).await {
        let _ = connection.write(&stop_packet).await;
        let _ = injector.release_all();
        let _ = connection.disconnect().await;
        return Err(error).context("启动候选 IMU 流失败");
    }
    tracker.start(0);
    tracing::info!(
        "IMU_STREAM_ARMED 等待 10 Hz A2/10；首包限时 1.5 秒，进入稳态后 250 ms 无数据将急停"
    );
    if options.inject {
        tracing::info!("IMU_SCROLL_CALIBRATING 请保持正常姿态约 1 秒");
    } else {
        tracing::info!("IMU_LISTEN_ONLY 默认不注入滚轮");
    }

    let (quit_tx, mut quit_rx) = mpsc::channel(1);
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        let _ = quit_tx.blocking_send(());
    });

    let mut tick = tokio::time::interval(Duration::from_millis(25));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut run_result = Ok(());

    loop {
        let now_ms = started.elapsed().as_millis() as u64;
        if options.seconds > 0 && now_ms >= options.seconds.saturating_mul(1_000) {
            break;
        }
        tokio::select! {
            _ = tick.tick() => {
                let now_ms = started.elapsed().as_millis() as u64;
                if waiting_for_first_sample {
                    if now_ms.saturating_sub(stream_started_at_ms) >= STREAM_FIRST_SAMPLE_TIMEOUT_MS {
                        run_result = Err(anyhow::anyhow!("IMU 首包超时急停"));
                        break;
                    }
                } else if let Some(reason) = tracker.check_timeout(now_ms) {
                    if reason == ImuStreamStopReason::NoDataTimeout
                        && stream_recoveries < MAX_STREAM_RECOVERIES
                    {
                        let _ = injector.release_all();
                        if let Err(error) = connection.write(&stop_packet).await {
                            run_result = Err(error.context("IMU 失联后发送急停命令失败"));
                            break;
                        }
                        if let Err(error) = connection.write(&start_packet).await {
                            run_result = Err(error.context("IMU 失联后重启流失败"));
                            break;
                        }
                        stream_recoveries += 1;
                        tracker.start(now_ms);
                        last_renew_ms = now_ms;
                        stream_started_at_ms = now_ms;
                        waiting_for_first_sample = true;
                        tracing::warn!(
                            recovery = stream_recoveries,
                            limit = MAX_STREAM_RECOVERIES,
                            "IMU_GAP_RECOVERY 已先急停并释放输入；正在等待重启后的 sequence=0"
                        );
                        continue;
                    }
                    run_result = Err(anyhow::anyhow!("IMU 流失联急停：{reason:?}"));
                    break;
                }
                if now_ms.saturating_sub(last_renew_ms) >= STREAM_RENEW_MS {
                    if let Err(error) = connection.write(&start_packet).await {
                        run_result = Err(error.context("IMU 流续期失败"));
                        break;
                    }
                    tracker.start(now_ms);
                    last_renew_ms = now_ms;
                    stream_started_at_ms = now_ms;
                    waiting_for_first_sample = true;
                    tracing::info!("IMU_STREAM_RENEW 已在设备 12 秒硬超时前安全续期");
                }
            }
            packet = futures::StreamExt::next(&mut notifications) => {
                let Some(packet) = packet else {
                    run_result = Err(anyhow::anyhow!("IMU 通知流意外结束"));
                    break;
                };
                let now_ms = started.elapsed().as_millis() as u64;
                let evaluation = if waiting_for_first_sample {
                    match crate::protocol::evaluate_imu_stream_packet(&packet, None) {
                        ImuStreamEvaluation::Sample(sample) if sample.sequence != 0 => {
                            tracing::debug!(
                                sequence = sample.sequence,
                                "等待续期后的 sequence=0，丢弃通知队列旧尾包"
                            );
                            continue;
                        }
                        _ => tracker.ingest(&packet, now_ms),
                    }
                } else {
                    tracker.ingest(&packet, now_ms)
                };
                match evaluation {
                    ImuStreamEvaluation::NotStreamPacket => {
                        tracing::debug!(packet = %format_packet(&packet), "忽略非 A2/10 通知");
                    }
                    ImuStreamEvaluation::Stop(reason) => {
                        if reason == ImuStreamStopReason::Stale
                            && stream_recoveries < MAX_STREAM_RECOVERIES
                        {
                            let _ = injector.release_all();
                            if let Err(error) = connection.write(&start_packet).await {
                                run_result = Err(error.context("STALE 后重启 IMU 流失败"));
                                break;
                            }
                            stream_recoveries += 1;
                            tracker.start(now_ms);
                            last_renew_ms = now_ms;
                            stream_started_at_ms = now_ms;
                            waiting_for_first_sample = true;
                            tracing::warn!(
                                recovery = stream_recoveries,
                                limit = MAX_STREAM_RECOVERIES,
                                "IMU_STALE_RECOVERY 固件已急停；正在等待重启后的 sequence=0"
                            );
                            continue;
                        }
                        run_result = Err(anyhow::anyhow!("IMU 数据失效急停：{reason:?}"));
                        break;
                    }
                    ImuStreamEvaluation::Sample(sample) => {
                        waiting_for_first_sample = false;
                        let was_calibrated = mapper.calibrated();
                        let Some(delta) = mapper.update(sample) else {
                            run_result = Err(anyhow::anyhow!("IMU 重力向量超出可信范围，已急停"));
                            break;
                        };
                        if !was_calibrated && mapper.calibrated() {
                            tracing::info!("IMU_SCROLL_READY 零点校准完成；转动戒指控制滚轮，回到零点停止");
                        }
                        if options.inject && delta != 0 {
                            if let Err(error) = injector.wheel(delta) {
                                run_result = Err(error.context("滚轮注入失败"));
                                break;
                            }
                        }
                        tracing::debug!(sequence=sample.sequence, x=sample.acceleration.x, y=sample.acceleration.y, z=sample.acceleration.z, wheel=delta, "IMU_SAMPLE");
                    }
                }
            }
            _ = quit_rx.recv() => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    let stop_error = connection.write(&stop_packet).await.err();
    let release_result = injector.release_all();
    let disconnect_result = connection.disconnect().await;
    if let Some(error) = stop_error.as_ref() {
        tracing::warn!("发送 IMU 停止命令失败：{error:#}");
    }
    match run_result {
        Err(error) => {
            if let Err(cleanup_error) = release_result {
                tracing::warn!("急停后释放输入状态失败：{cleanup_error:#}");
            }
            if let Err(cleanup_error) = disconnect_result {
                tracing::warn!("急停后断开戒指失败：{cleanup_error:#}");
            }
            Err(error)
        }
        Ok(()) => {
            release_result.context("退出时释放输入状态失败")?;
            disconnect_result.context("退出时断开戒指失败")?;
            if let Some(error) = stop_error {
                return Err(error).context("退出时发送 IMU 停止命令失败");
            }
            Ok(())
        }
    }
}

async fn verify_device_identity(connection: &RingConnection) -> Result<()> {
    let rows = connection
        .read_device_information()
        .await
        .context("读取设备身份失败；未发送 IMU 启动命令")?;
    let hardware = rows
        .iter()
        .find(|(label, _)| label == "Hardware Revision")
        .map(|(_, value)| device_information_text(value));
    let firmware = rows
        .iter()
        .find(|(label, _)| label == "Firmware Revision")
        .map(|(_, value)| device_information_text(value));
    if hardware != Some(EXPECTED_HARDWARE) || firmware != Some(EXPECTED_FIRMWARE) {
        bail!("设备身份不匹配，拒绝启动候选 IMU 流：hardware={hardware:?}, firmware={firmware:?}");
    }
    Ok(())
}

fn device_information_text(value: &str) -> &str {
    value
        .split_once("  [HEX ")
        .map_or(value, |(text, _)| text)
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::IMU_STREAM_FLAG_VALID;

    fn sample(sequence: u8, degrees: f64) -> ImuStreamSample {
        let radians = degrees.to_radians();
        ImuStreamSample {
            sequence,
            flags: IMU_STREAM_FLAG_VALID,
            acceleration: AccelerometerSample {
                x: (8_192.0 * radians.cos()).round() as i16,
                y: 0,
                z: (8_192.0 * radians.sin()).round() as i16,
            },
            fifo_level: 1,
            dropped_samples: 0,
            age_ticks: 0,
        }
    }

    fn calibrated_mapper() -> ImuWheelMapper {
        let mut mapper = ImuWheelMapper::new(ImuWheelConfig::default()).unwrap();
        for sequence in 0..CALIBRATION_SAMPLES as u8 {
            assert_eq!(mapper.update(sample(sequence, 0.0)), Some(0));
        }
        assert!(mapper.calibrated());
        mapper
    }

    #[test]
    fn packet_commands_are_exact_and_checksummed() {
        assert_eq!(
            imu_stream_start_packet(),
            [0xA1, 0x09, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xAB]
        );
        assert_eq!(
            imu_stream_stop_packet(),
            [0xA1, 0x09, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xAA]
        );
    }

    #[test]
    fn calibration_and_deadzone_emit_no_wheel() {
        let mut mapper = calibrated_mapper();
        for (sequence, degrees) in [(10, 2.0), (11, -2.0), (12, 4.0)] {
            assert_eq!(mapper.update(sample(sequence, degrees)), Some(0));
        }
    }

    #[test]
    fn rotation_is_proportional_reversible_and_stops_at_zero() {
        let mut mapper = calibrated_mapper();
        let positive: i32 = (10..20)
            .map(|sequence| mapper.update(sample(sequence, 30.0)).unwrap())
            .sum();
        assert!(positive > 0);
        let negative: i32 = (20..35)
            .map(|sequence| mapper.update(sample(sequence, -30.0)).unwrap())
            .sum();
        assert!(negative < 0);
        for sequence in 35..60 {
            let _ = mapper.update(sample(sequence, 0.0));
        }
        assert_eq!(mapper.update(sample(60, 0.0)), Some(0));
    }

    #[test]
    fn inversion_changes_direction() {
        let config = ImuWheelConfig {
            invert: true,
            ..ImuWheelConfig::default()
        };
        let mut mapper = ImuWheelMapper::new(config).unwrap();
        for sequence in 0..CALIBRATION_SAMPLES as u8 {
            mapper.update(sample(sequence, 0.0));
        }
        let total: i32 = (10..20)
            .map(|sequence| mapper.update(sample(sequence, 35.0)).unwrap())
            .sum();
        assert!(total < 0);
    }

    #[test]
    fn invalid_gravity_fails_closed() {
        let mut mapper = calibrated_mapper();
        let mut invalid = sample(10, 0.0);
        invalid.acceleration = AccelerometerSample { x: 0, y: 0, z: 0 };
        assert_eq!(mapper.update(invalid), None);
    }

    #[test]
    fn accepts_observed_v7_gravity_scale() {
        let mut mapper = ImuWheelMapper::new(ImuWheelConfig::default()).unwrap();
        let observed = ImuStreamSample {
            sequence: 0,
            flags: IMU_STREAM_FLAG_VALID,
            acceleration: AccelerometerSample {
                x: 8_148,
                y: -388,
                z: 196,
            },
            fifo_level: 0,
            dropped_samples: 0,
            age_ticks: 0,
        };
        assert_eq!(mapper.update(observed), Some(0));
    }

    #[test]
    fn accepts_observed_dynamic_v7_sample() {
        let mut mapper = ImuWheelMapper::new(ImuWheelConfig::default()).unwrap();
        let observed = ImuStreamSample {
            sequence: 0,
            flags: IMU_STREAM_FLAG_VALID,
            acceleration: AccelerometerSample {
                x: 6_632,
                y: 14_164,
                z: 10_204,
            },
            fifo_level: 0,
            dropped_samples: 0,
            age_ticks: 0,
        };
        assert_eq!(mapper.update(observed), Some(0));
    }

    #[test]
    fn rejects_corner_saturated_vector() {
        let mut mapper = calibrated_mapper();
        let mut saturated = sample(10, 0.0);
        saturated.acceleration = AccelerometerSample {
            x: i16::MAX,
            y: i16::MAX,
            z: i16::MAX,
        };
        assert_eq!(mapper.update(saturated), None);
    }

    #[test]
    fn angle_wrap_does_not_reverse_at_pi_boundary() {
        let mut mapper = ImuWheelMapper::new(ImuWheelConfig::default()).unwrap();
        for sequence in 0..CALIBRATION_SAMPLES as u8 {
            mapper.update(sample(sequence, 179.0));
        }
        for sequence in 10..20 {
            assert_eq!(mapper.update(sample(sequence, -179.0)), Some(0));
        }
    }

    #[test]
    fn bad_configuration_is_rejected() {
        let config = ImuWheelConfig {
            deadzone_degrees: 45.0,
            ..ImuWheelConfig::default()
        };
        assert!(ImuWheelMapper::new(config).is_err());
    }

    #[test]
    fn device_identity_ignores_read_only_hex_annotation() {
        assert_eq!(
            device_information_text("RT08_V3.1  [HEX 52 54 30 38 5F 56 33 2E 31]"),
            EXPECTED_HARDWARE
        );
        assert_eq!(
            device_information_text("RT08_3.10.48_260309  [HEX 52 54 30 38 5F 33 2E 31 30]"),
            EXPECTED_FIRMWARE
        );
    }
}
