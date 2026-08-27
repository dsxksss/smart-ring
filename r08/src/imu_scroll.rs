//! Fail-closed host support for the experimental A2/10 IMU-only stream.
//!
//! Nothing in this module flashes firmware. `run` transmits the candidate's
//! ordinary UART start/stop command only after the CLI's explicit opt-in.

use std::f64::consts::PI;
use std::pin::Pin;
use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures::{Stream, StreamExt};
use tokio::sync::mpsc;

use crate::ble::RingConnection;
use crate::mapping::{InputEvent, MappingConfig, MappingEngine, Output};
use crate::platform::inject::{Injector, NullInjector};
use crate::platform::{create_injector, PointerSuppression};
use crate::protocol::{
    decode_firmware_capability_status, decode_uart_battery_response,
    firmware_capability_probe_packet, format_packet, imu_stream_start_packet,
    imu_stream_stop_packet, touch_disable_packet, touch_enable_packet, uart_battery_query_packet,
    AccelerometerSample, ImuStreamEvaluation, ImuStreamSample, ImuStreamStopReason,
    ImuStreamTracker, IMU_TOUCH_V9_CAPABILITY_STATUS,
};

pub const EXPECTED_HARDWARE: &str = "RT08_V3.1";
pub const EXPECTED_FIRMWARE: &str = "RT08_3.10.48_260309";
pub const STREAM_RENEW_MS: u64 = 8_000;
const STREAM_FIRST_SAMPLE_TIMEOUT_MS: u64 = 1_500;
// Windows can deliver a few packets from the previous start command before a
// later idempotent retry takes effect.  During this short post-start window a
// second sequence=0 marks the newer stream epoch; outside the window it remains
// a fail-closed sequence gap.
const STREAM_EPOCH_RESET_GRACE_MS: u64 = STREAM_FIRST_SAMPLE_TIMEOUT_MS;
const STREAM_RESTART_SETTLE_MS: u64 = 150;
const MAX_STREAM_RECOVERIES: u8 = 2;
const RECOVERY_HEALTHY_SAMPLES: u8 = 10;
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
const UART_READY_TIMEOUT_MS: u64 = 1_500;
const TAP_JERK_THRESHOLD: f64 = 3_500.0;
const TAP_SECOND_JERK_THRESHOLD: f64 = 2_500.0;
const TAP_RELEASE_THRESHOLD: f64 = 1_800.0;
const TAP_MIN_INTERVAL_MS: u64 = 250;
const TAP_MAX_INTERVAL_MS: u64 = 850;
const TAP_STREAM_SETTLE_MS: u64 = 1_500;
const STANDBY_RETRY_BACKOFF_MS: u64 = 1_000;
const CAPABILITY_PROBE_TIMEOUT_MS: u64 = 1_500;
const TOUCH_ARM_TIMEOUT_MS: u64 = 1_500;
const TOUCH_ARM_SETTLE_MS: u64 = 1_000;

type NotificationStream = Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>;

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

#[derive(Debug, Default)]
struct StreamRecoveryBudget {
    consecutive_recoveries: u8,
    healthy_samples: u8,
}

impl StreamRecoveryBudget {
    fn can_recover(&self) -> bool {
        self.consecutive_recoveries < MAX_STREAM_RECOVERIES
    }

    fn record_recovery(&mut self) -> u8 {
        self.consecutive_recoveries = self.consecutive_recoveries.saturating_add(1);
        self.healthy_samples = 0;
        self.consecutive_recoveries
    }

    fn record_valid_sample(&mut self) -> Option<u8> {
        if self.consecutive_recoveries == 0 {
            return None;
        }
        self.healthy_samples = self.healthy_samples.saturating_add(1);
        if self.healthy_samples < RECOVERY_HEALTHY_SAMPLES {
            return None;
        }
        let recovered_from = self.consecutive_recoveries;
        self.reset();
        Some(recovered_from)
    }

    fn restart_health_window(&mut self) {
        self.healthy_samples = 0;
    }

    fn reset(&mut self) {
        self.consecutive_recoveries = 0;
        self.healthy_samples = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TapDetection {
    None,
    Candidate {
        jerk: f64,
    },
    DoubleTap {
        interval_ms: u64,
        jerk: f64,
        direction_similarity: f64,
    },
}

#[derive(Debug, Default)]
struct ImuDoubleTapDetector {
    previous: Option<AccelerometerSample>,
    in_impulse: bool,
    first_tap: Option<(u64, [f64; 3])>,
}

impl ImuDoubleTapDetector {
    fn reset(&mut self) {
        self.previous = None;
        self.in_impulse = false;
        self.first_tap = None;
    }

    fn update(&mut self, sample: AccelerometerSample, now_ms: u64) -> TapDetection {
        let Some(previous) = self.previous.replace(sample) else {
            return TapDetection::None;
        };
        let impulse = [
            f64::from(sample.x) - f64::from(previous.x),
            f64::from(sample.y) - f64::from(previous.y),
            f64::from(sample.z) - f64::from(previous.z),
        ];
        let jerk = vector_norm(impulse);

        if self.in_impulse {
            if jerk <= TAP_RELEASE_THRESHOLD {
                self.in_impulse = false;
            }
            if self
                .first_tap
                .is_some_and(|(first_ms, _)| now_ms.saturating_sub(first_ms) > TAP_MAX_INTERVAL_MS)
            {
                self.first_tap = None;
            }
            // A strike and its rebound are one physical impulse.  Require at
            // least one calm sample before a second impulse can wake control.
            return TapDetection::None;
        }
        let threshold = if self.first_tap.is_some() {
            TAP_SECOND_JERK_THRESHOLD
        } else {
            TAP_JERK_THRESHOLD
        };
        if jerk < threshold {
            if self
                .first_tap
                .is_some_and(|(first_ms, _)| now_ms.saturating_sub(first_ms) > TAP_MAX_INTERVAL_MS)
            {
                self.first_tap = None;
            }
            return TapDetection::None;
        }
        self.in_impulse = true;

        let Some((first_ms, first_impulse)) = self.first_tap else {
            self.first_tap = Some((now_ms, impulse));
            return TapDetection::Candidate { jerk };
        };
        let interval_ms = now_ms.saturating_sub(first_ms);
        if interval_ms < TAP_MIN_INTERVAL_MS {
            return TapDetection::None;
        }
        if interval_ms > TAP_MAX_INTERVAL_MS {
            self.first_tap = Some((now_ms, impulse));
            return TapDetection::Candidate { jerk };
        }

        // At 10 Hz the sampled impulse can land on either the strike or rebound,
        // so direction is diagnostic only. Requiring a cosine threshold made a
        // real second knock disappear even though the two impulses were separate.
        let direction_similarity = vector_similarity(first_impulse, impulse).abs();
        self.first_tap = None;
        TapDetection::DoubleTap {
            interval_ms,
            jerk,
            direction_similarity,
        }
    }
}

fn vector_norm(vector: [f64; 3]) -> f64 {
    (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt()
}

fn vector_similarity(left: [f64; 3], right: [f64; 3]) -> f64 {
    let denominator = vector_norm(left) * vector_norm(right);
    if denominator <= f64::EPSILON {
        return 0.0;
    }
    (left[0] * right[0] + left[1] * right[1] + left[2] * right[2]) / denominator
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
    pub double_tap_wake: bool,
    pub config: ImuWheelConfig,
}

async fn probe_v9_touch_capability(
    connection: &RingConnection,
    notifications: &mut NotificationStream,
) -> Result<bool> {
    connection
        .write(&firmware_capability_probe_packet())
        .await
        .context("发送只读固件能力标记查询失败")?;
    let status = tokio::time::timeout(Duration::from_millis(CAPABILITY_PROBE_TIMEOUT_MS), async {
        loop {
            let packet = notifications
                .next()
                .await
                .context("固件能力标记返回前通知流结束")?;
            if let Some(status) = decode_firmware_capability_status(&packet) {
                return Ok::<u8, anyhow::Error>(status);
            }
            tracing::info!(
                packet = %format_packet(&packet),
                "CAPABILITY_PROBE_IGNORED 等待 A1 FC..FF 时忽略其他通知"
            );
        }
    })
    .await
    .context("固件能力标记查询超时")??;
    tracing::info!(
        status = format_args!("0x{status:02X}"),
        "FIRMWARE_CAPABILITY 已读取独立 A1 状态标记"
    );
    Ok(status == IMU_TOUCH_V9_CAPABILITY_STATUS)
}

async fn arm_v9_touch(
    connection: &RingConnection,
    notifications: &mut NotificationStream,
) -> Result<()> {
    let packet = touch_enable_packet(2, 1);
    connection
        .write(&packet)
        .await
        .context("发送 v9 原生触控双击唤醒配置失败")?;
    tokio::time::timeout(Duration::from_millis(TOUCH_ARM_TIMEOUT_MS), async {
        loop {
            let response = notifications
                .next()
                .await
                .context("v9 触控设置应答前通知流结束")?;
            if response.as_slice() == packet {
                return Ok::<(), anyhow::Error>(());
            }
            tracing::info!(
                packet = %format_packet(&response),
                "TOUCH_ARM_IGNORED 等待 3B 精确应答时忽略其他通知"
            );
        }
    })
    .await
    .context("v9 触控设置应答超时；未启动 IMU")??;
    tracing::info!("FIRMWARE_TOUCH_WAKE_ARMED v9 已确认 3B 设置；等待电容触控区双击上报 73 2A 00");
    Ok(())
}

pub async fn run(connection: RingConnection, options: ImuStreamOptions) -> Result<()> {
    let mut mapper = ImuWheelMapper::new(options.config)?;
    if let Err(error) = verify_device_identity(&connection).await {
        let _ = connection.disconnect().await;
        return Err(error);
    }
    let mut pointer_suppression = PointerSuppression::new();
    let mut notifications = match connection.subscribe().await {
        Ok(notifications) => notifications,
        Err(error) => {
            if options.double_tap_wake {
                let _ = pointer_suppression.restore();
            }
            let _ = connection.disconnect().await;
            return Err(error).context("订阅候选 IMU 通知失败");
        }
    };
    let uart_ready = tokio::time::timeout(Duration::from_millis(UART_READY_TIMEOUT_MS), async {
        connection
            .write(&uart_battery_query_packet())
            .await
            .context("发送 UART 通知通道验证查询失败")?;
        loop {
            let packet = notifications
                .next()
                .await
                .context("UART 通知流在验证完成前结束")?;
            if let Some(battery_percent) = decode_uart_battery_response(&packet) {
                return Ok::<u8, anyhow::Error>(battery_percent);
            }
            tracing::info!(
                packet = %format_packet(&packet),
                "UART_READY_IGNORED 验证通知通道时收到其他数据包"
            );
        }
    })
    .await;
    let battery_percent = match uart_ready {
        Ok(Ok(battery_percent)) => battery_percent,
        Ok(Err(error)) => {
            if options.double_tap_wake {
                let _ = pointer_suppression.restore();
            }
            let _ = connection.disconnect().await;
            return Err(error).context("UART 通知通道验证失败；未发送 IMU 启动命令");
        }
        Err(_) => {
            if options.double_tap_wake {
                let _ = pointer_suppression.restore();
            }
            let _ = connection.disconnect().await;
            bail!("UART 通知通道验证超时；未收到只读电量应答，也未发送 IMU 启动命令");
        }
    };
    tracing::info!(
        battery_percent,
        "UART_NOTIFY_READY 已收到只读电量应答；通知通道可用"
    );
    let firmware_touch_wake = if options.double_tap_wake {
        match probe_v9_touch_capability(&connection, &mut notifications).await {
            Ok(true) => true,
            Ok(false) => false,
            Err(error) => {
                tracing::warn!(
                    "FIRMWARE_CAPABILITY_FALLBACK 未确认 v9 A1 FC 标记，将保留主机 IMU 双敲模式：{error:#}"
                );
                false
            }
        }
    } else {
        false
    };
    if firmware_touch_wake {
        if !connection.supports_v9_touch_imu_combo() {
            let backend = connection.backend_name();
            let _ = connection.disconnect().await;
            bail!(
                "v9 原生触控+IMU 组合模式只允许已验证的 Windows Win32 GATT 路径；当前后端={backend}"
            );
        }
        if let Err(error) = arm_v9_touch(&connection, &mut notifications).await {
            let _ = connection
                .write_without_response(&touch_disable_packet(1))
                .await;
            let _ = connection.disconnect().await;
            return Err(error).context("v9 原生触控唤醒未能安全武装；IMU 未启动");
        }
        tracing::info!(
            backend = connection.backend_name(),
            "R08_POINTER_BLOCK_FIRMWARE v9 已在戒指端屏蔽 HID 鼠标报告；不需要管理员停用设备"
        );
    } else if options.double_tap_wake {
        match pointer_suppression.suppress_if_present() {
            Ok(true) => {}
            Ok(false) => tracing::info!(
                "R08_POINTER_BLOCK_NOT_NEEDED Windows 当前没有 R08 HID 鼠标子设备；GATT 组合模式可安全继续"
            ),
            Err(error) => {
                let _ = connection.disconnect().await;
                return Err(error).context("检查 R08 无光标移动保护失败；组合模式未启动");
            }
        }
        tracing::info!("IMU_TAP_WAKE_READY 未检测到 v9 标记；保留主机 IMU 双敲兜底且不发送 3B");
    }
    let mut injector: Box<dyn Injector> = if options.inject {
        match create_injector() {
            Ok(injector) => injector,
            Err(error) => {
                if firmware_touch_wake {
                    let _ = connection
                        .write_without_response(&touch_disable_packet(1))
                        .await;
                }
                if options.double_tap_wake {
                    let _ = pointer_suppression.restore();
                }
                let _ = connection.disconnect().await;
                return Err(error).context("创建滚轮注入后端失败");
            }
        }
    } else {
        Box::new(NullInjector)
    };
    let started = Instant::now();
    let mut tracker = ImuStreamTracker::default();
    let mut waiting_for_first_sample = true;
    let mut recovery_budget = StreamRecoveryBudget::default();
    let mut control_active = !options.double_tap_wake;
    let mut last_pointer_check_ms = 0u64;
    let start_packet = imu_stream_start_packet();
    let stop_packet = imu_stream_stop_packet();
    let mut touch_engine = options.double_tap_wake.then(|| {
        MappingEngine::new(MappingConfig {
            scroll_gain: 4,
            inject: options.inject,
            require_double_tap_wake: true,
        })
    });
    let mut tap_detector = ImuDoubleTapDetector::default();

    if let Err(error) = connection.write_without_response(&start_packet).await {
        let _ = connection.write_without_response(&stop_packet).await;
        if firmware_touch_wake {
            let _ = connection
                .write_without_response(&touch_disable_packet(1))
                .await;
        }
        let _ = injector.release_all();
        let _ = connection.disconnect().await;
        return Err(error).context("启动候选 IMU 流失败");
    }
    let initial_stream_ms = arm_stream_tracking(&mut tracker, &started);
    let mut last_renew_ms = initial_stream_ms;
    let mut stream_started_at_ms = initial_stream_ms;
    tracing::info!(
        "IMU_STREAM_ARMED 等待 10 Hz A2/10；首包限时 1.5 秒，进入稳态后 750 ms 无数据将急停"
    );
    if options.double_tap_wake {
        if firmware_touch_wake {
            tracing::info!(
                "IMU_CONTROL_STANDBY v9 原生触控已武装；只接受电容触控区双击唤醒，不使用 IMU 敲击判断"
            );
            println!("控制待机：请双击戒指电容触控区域；看到 IMU_CONTROL_AWAKE 后再转动戒指。");
            println!("v9 已在戒指端屏蔽 HID 鼠标报告，因此触控过程不会移动电脑光标。");
        } else {
            tracing::info!(
                "IMU_CONTROL_STANDBY 请连续轻敲戒指两次；单次敲击只记录候选，不会启动滚动"
            );
            println!("控制待机：数据流稳定约 1.5 秒后，请间隔约 0.25～0.85 秒连续轻敲戒指两次。");
            println!("说明：未检测到 v9 标记，当前仍通过戒指 IMU 感知敲击，无法判断是否命中电容触控区域。");
        }
        println!("看到 IMU_CONTROL_AWAKE 后保持正常姿态约 1 秒，再转动滚动。60 秒后停止注入；按 Enter 或 Ctrl+C 安全退出。");
    } else if options.inject {
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
                if options.double_tap_wake
                    && !firmware_touch_wake
                    && now_ms.saturating_sub(last_pointer_check_ms) >= 1_000
                {
                    last_pointer_check_ms = now_ms;
                    if let Err(error) = pointer_suppression.suppress_if_present() {
                        run_result = Err(error.context("运行时检查 R08 HID 鼠标子设备失败"));
                        break;
                    }
                }
                if let Some(engine) = touch_engine.as_mut() {
                    if let Err(error) = apply_touch_outputs(&mut injector, engine.tick(now_ms)) {
                        run_result = Err(error.context("触控动作注入失败"));
                        break;
                    }
                    if control_active && !engine.control_awake(now_ms) {
                        if let Err(error) = injector.release_all() {
                            run_result = Err(error.context("IMU 唤醒窗口结束时释放输入失败"));
                            break;
                        }
                        control_active = false;
                        mapper = ImuWheelMapper::new(options.config)
                            .expect("validated IMU wheel configuration");
                        tap_detector.reset();
                        tracing::info!(
                            "IMU_CONTROL_STANDBY 60 秒控制窗口结束；已停止输入注入，等待下一次双击唤醒"
                        );
                    }
                }
                if waiting_for_first_sample {
                    if now_ms.saturating_sub(stream_started_at_ms) >= STREAM_FIRST_SAMPLE_TIMEOUT_MS {
                        let controller_retry = options.double_tap_wake;
                        if should_retry_stream(recovery_budget.can_recover(), controller_retry) {
                            let _ = injector.release_all();
                            let delay_ms = if recovery_budget.can_recover() {
                                STREAM_RESTART_SETTLE_MS
                            } else {
                                recovery_budget.reset();
                                STANDBY_RETRY_BACKOFF_MS
                            };
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            if let Err(error) = connection.write_without_response(&start_packet).await {
                                run_result = Err(error.context("IMU 首包超时后重启流失败"));
                                break;
                            }
                            let recovery = recovery_budget.record_recovery();
                            tap_detector.reset();
                            let restarted_ms = arm_stream_tracking(&mut tracker, &started);
                            last_renew_ms = restarted_ms;
                            stream_started_at_ms = restarted_ms;
                            tracing::warn!(
                                recovery,
                                limit = MAX_STREAM_RECOVERIES,
                                delay_ms,
                                controller_retry,
                                "IMU_FIRST_PACKET_RECOVERY 首包超时后已重发幂等启动命令"
                            );
                            continue;
                        }
                            run_result = Err(anyhow::anyhow!("IMU 首包超时急停"));
                            break;
                    }
                } else if let Some(reason) = tracker.check_timeout(now_ms) {
                    let controller_retry = options.double_tap_wake;
                    if reason == ImuStreamStopReason::NoDataTimeout
                        && should_retry_stream(recovery_budget.can_recover(), controller_retry)
                    {
                        let _ = injector.release_all();
                        let delay_ms = if recovery_budget.can_recover() {
                            STREAM_RESTART_SETTLE_MS
                        } else {
                            recovery_budget.reset();
                            STANDBY_RETRY_BACKOFF_MS
                        };
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        if let Err(error) = connection.write_without_response(&start_packet).await {
                            run_result = Err(error.context("IMU 失联后重启流失败"));
                            break;
                        }
                        let recovery = recovery_budget.record_recovery();
                        tap_detector.reset();
                        let restarted_ms = arm_stream_tracking(&mut tracker, &started);
                        last_renew_ms = restarted_ms;
                        stream_started_at_ms = restarted_ms;
                        waiting_for_first_sample = true;
                        tracing::warn!(
                            recovery,
                            limit = MAX_STREAM_RECOVERIES,
                            delay_ms,
                            controller_retry,
                            "IMU_GAP_RECOVERY 已停止主机输入并重发幂等启动命令；稳定 10 个样本后清零连续失败计数"
                        );
                        continue;
                    }
                    run_result = Err(anyhow::anyhow!("IMU 流失联急停：{reason:?}"));
                    break;
                }
                if now_ms.saturating_sub(last_renew_ms) >= STREAM_RENEW_MS {
                    if let Err(error) = connection.write_without_response(&start_packet).await {
                        run_result = Err(error.context("IMU 流续期失败"));
                        break;
                    }
                    tap_detector.reset();
                    let renewed_ms = arm_stream_tracking(&mut tracker, &started);
                    last_renew_ms = renewed_ms;
                    stream_started_at_ms = renewed_ms;
                    waiting_for_first_sample = true;
                    recovery_budget.restart_health_window();
                    tracing::info!("IMU_STREAM_RENEW 已在设备 12 秒硬超时前安全续期");
                }
            }
            packet = notifications.next() => {
                let Some(packet) = packet else {
                    run_result = Err(anyhow::anyhow!("IMU 通知流意外结束"));
                    break;
                };
                let now_ms = started.elapsed().as_millis() as u64;
                let is_stream_packet = packet.len() >= 2 && packet[0] == 0xA2 && packet[1] == 0x10;
                if !is_stream_packet {
                    if firmware_touch_wake
                        && now_ms < TOUCH_ARM_SETTLE_MS
                        && packet.get(..3) == Some(&[0x73, 0x2A, 0x00])
                    {
                        tracing::info!(
                            "TOUCH_INITIAL_AWAKE_IGNORED 忽略 3B 武装后 1 秒内的初始唤醒状态；仍需真实双击"
                        );
                        continue;
                    }
                    if let Some(engine) = touch_engine.as_mut() {
                        let outputs = engine.handle(InputEvent::GattPacket(packet), now_ms);
                        if let Err(error) = apply_touch_outputs(&mut injector, outputs) {
                            run_result = Err(error.context("触控动作注入失败"));
                            break;
                        }
                        let control_awake = engine.control_awake(now_ms);
                        if !control_active && control_awake {
                            mapper = ImuWheelMapper::new(options.config)
                                .expect("validated IMU wheel configuration");
                            control_active = true;
                            tap_detector.reset();
                            tracing::info!(
                                "IMU_CONTROL_AWAKE 收到戒指被动触控唤醒通知；控制窗口为 60 秒"
                            );
                            if options.inject {
                                tracing::info!("IMU_SCROLL_CALIBRATING 请保持正常姿态约 1 秒");
                            }
                        } else if control_active && !control_awake {
                            if let Err(error) = injector.release_all() {
                                run_result = Err(error.context("戒指休眠时释放输入失败"));
                                break;
                            }
                            control_active = false;
                            mapper = ImuWheelMapper::new(options.config)
                                .expect("validated IMU wheel configuration");
                            tap_detector.reset();
                            tracing::info!("IMU_CONTROL_STANDBY 戒指触控窗口已关闭；等待下一次双击唤醒");
                        }
                        continue;
                    }
                }
                let without_history = crate::protocol::evaluate_imu_stream_packet(&packet, None);
                if should_accept_stream_epoch_reset(
                    &without_history,
                    waiting_for_first_sample,
                    now_ms,
                    stream_started_at_ms,
                ) {
                    tracker.start(now_ms);
                    waiting_for_first_sample = true;
                    tap_detector.reset();
                    tracing::warn!(
                        elapsed_ms = now_ms.saturating_sub(stream_started_at_ms),
                        "IMU_STREAM_EPOCH_RESET 启动命令重叠期间收到新的 sequence=0；已丢弃旧世代序号并继续"
                    );
                }
                let evaluation = if waiting_for_first_sample {
                    match &without_history {
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
                        let controller_retry = options.double_tap_wake;
                        if reason == ImuStreamStopReason::Stale
                            && should_retry_stream(recovery_budget.can_recover(), controller_retry)
                        {
                            let _ = injector.release_all();
                            let delay_ms = if recovery_budget.can_recover() {
                                STREAM_RESTART_SETTLE_MS
                            } else {
                                recovery_budget.reset();
                                STANDBY_RETRY_BACKOFF_MS
                            };
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            if let Err(error) = connection.write_without_response(&start_packet).await {
                                run_result = Err(error.context("STALE 后重启 IMU 流失败"));
                                break;
                            }
                            let recovery = recovery_budget.record_recovery();
                            tap_detector.reset();
                            let restarted_ms = arm_stream_tracking(&mut tracker, &started);
                            last_renew_ms = restarted_ms;
                            stream_started_at_ms = restarted_ms;
                            waiting_for_first_sample = true;
                            tracing::warn!(
                                recovery,
                                limit = MAX_STREAM_RECOVERIES,
                                delay_ms,
                                controller_retry,
                                "IMU_STALE_RECOVERY 固件已急停；这是连续失败计数，稳定 10 个样本后会清零"
                            );
                            continue;
                        }
                        run_result = Err(anyhow::anyhow!("IMU 数据失效急停：{reason:?}"));
                        break;
                    }
                    ImuStreamEvaluation::Sample(sample) => {
                        waiting_for_first_sample = false;
                        if let Some(recovered_from) = recovery_budget.record_valid_sample() {
                            tracing::info!(
                                recovered_from,
                                healthy_samples = RECOVERY_HEALTHY_SAMPLES,
                                "IMU_RECOVERY_BUDGET_RESET 数据流已稳定，连续恢复预算已清零"
                            );
                        }
                        if options.double_tap_wake
                            && !firmware_touch_wake
                            && !control_active
                        {
                            if !tap_detection_ready(now_ms, stream_started_at_ms) {
                                tap_detector.reset();
                                continue;
                            }
                            match tap_detector.update(sample.acceleration, now_ms) {
                                TapDetection::None => {}
                                TapDetection::Candidate { jerk } => tracing::info!(
                                    jerk = format_args!("{jerk:.0}"),
                                    "IMU_TAP_CANDIDATE 已记录第一次敲击；等待 0.25～0.85 秒内第二次敲击"
                                ),
                                TapDetection::DoubleTap {
                                    interval_ms,
                                    jerk,
                                    direction_similarity,
                                } => {
                                    if let Some(engine) = touch_engine.as_mut() {
                                        if let Err(error) = apply_touch_outputs(
                                            &mut injector,
                                            engine.wake_control(now_ms),
                                        ) {
                                            run_result = Err(error.context("双敲唤醒后更新控制状态失败"));
                                            break;
                                        }
                                    }
                                    control_active = true;
                                    mapper = ImuWheelMapper::new(options.config)
                                        .expect("validated IMU wheel configuration");
                                    tracing::info!(
                                        interval_ms,
                                        jerk = format_args!("{jerk:.0}"),
                                        similarity = format_args!("{direction_similarity:.2}"),
                                        "IMU_CONTROL_AWAKE 已确认两次独立敲击；控制窗口为 60 秒"
                                    );
                                    if options.inject {
                                        tracing::info!("IMU_SCROLL_CALIBRATING 请保持正常姿态约 1 秒");
                                    }
                                    continue;
                                }
                            }
                            continue;
                        }
                        if !control_active {
                            continue;
                        }
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

    let stop_error = connection.write_without_response(&stop_packet).await.err();
    let touch_disable_error = if firmware_touch_wake {
        connection
            .write_without_response(&touch_disable_packet(1))
            .await
            .err()
    } else {
        None
    };
    let release_result = injector.release_all();
    let disconnect_result = connection.disconnect().await;
    let pointer_result = if options.double_tap_wake && !firmware_touch_wake {
        pointer_suppression.restore()
    } else {
        Ok(())
    };
    if let Some(error) = stop_error.as_ref() {
        tracing::warn!("发送 IMU 停止命令失败：{error:#}");
    }
    if let Some(error) = touch_disable_error.as_ref() {
        tracing::warn!("退出时关闭 v9 原生触控失败：{error:#}");
    }
    match run_result {
        Err(error) => {
            if let Err(cleanup_error) = release_result {
                tracing::warn!("急停后释放输入状态失败：{cleanup_error:#}");
            }
            if let Err(cleanup_error) = disconnect_result {
                tracing::warn!("急停后断开戒指失败：{cleanup_error:#}");
            }
            if let Err(cleanup_error) = pointer_result {
                tracing::warn!("急停后恢复 R08 HID 鼠标子设备失败：{cleanup_error:#}");
            }
            Err(error)
        }
        Ok(()) => {
            if let Some(error) = touch_disable_error {
                return Err(error).context("退出时关闭 v9 原生触控失败");
            }
            release_result.context("退出时释放输入状态失败")?;
            disconnect_result.context("退出时断开戒指失败")?;
            pointer_result.context("退出时恢复 R08 HID 鼠标子设备失败")?;
            if let Some(error) = stop_error {
                return Err(error).context("退出时发送 IMU 停止命令失败");
            }
            Ok(())
        }
    }
}

fn arm_stream_tracking(tracker: &mut ImuStreamTracker, started: &Instant) -> u64 {
    let now_ms = started.elapsed().as_millis() as u64;
    tracker.start(now_ms);
    now_ms
}

fn should_accept_stream_epoch_reset(
    without_history: &ImuStreamEvaluation,
    waiting_for_first_sample: bool,
    now_ms: u64,
    stream_started_at_ms: u64,
) -> bool {
    !waiting_for_first_sample
        && now_ms.saturating_sub(stream_started_at_ms) <= STREAM_EPOCH_RESET_GRACE_MS
        && matches!(
            without_history,
            ImuStreamEvaluation::Sample(sample) if sample.sequence == 0
        )
}

fn tap_detection_ready(now_ms: u64, stream_started_at_ms: u64) -> bool {
    now_ms.saturating_sub(stream_started_at_ms) >= TAP_STREAM_SETTLE_MS
}

fn should_retry_stream(recovery_budget_available: bool, persistent_controller: bool) -> bool {
    recovery_budget_available || persistent_controller
}

fn apply_touch_outputs(injector: &mut Box<dyn Injector>, outputs: Vec<Output>) -> Result<()> {
    for output in outputs {
        match output {
            Output::Log(text) => tracing::info!("{text}"),
            Output::Wheel(delta) => injector.wheel(delta)?,
            Output::CaptureCursorAnchor => injector.capture_cursor_anchor()?,
            Output::RestoreCursor => injector.restore_cursor()?,
            Output::ReleaseLeftButton => injector.release_left_button()?,
            Output::Copy => injector.copy()?,
            Output::Paste => injector.paste()?,
        }
    }
    Ok(())
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

    fn acceleration(x: i16, y: i16, z: i16) -> AccelerometerSample {
        AccelerometerSample { x, y, z }
    }

    #[test]
    fn imu_double_tap_requires_two_separate_impulses() {
        let mut detector = ImuDoubleTapDetector::default();
        assert_eq!(
            detector.update(acceleration(0, 0, 8_192), 0),
            TapDetection::None
        );
        assert!(matches!(
            detector.update(acceleration(7_000, 0, 8_192), 100),
            TapDetection::Candidate { .. }
        ));
        assert_eq!(
            detector.update(acceleration(0, 0, 8_192), 200),
            TapDetection::None
        );
        assert_eq!(
            detector.update(acceleration(0, 0, 8_192), 300),
            TapDetection::None
        );
        assert!(matches!(
            detector.update(acceleration(7_000, 0, 8_192), 500),
            TapDetection::DoubleTap {
                interval_ms: 400,
                ..
            }
        ));
    }

    #[test]
    fn imu_fast_double_tap_requires_a_calm_sample_after_the_first_rebound() {
        let mut detector = ImuDoubleTapDetector::default();
        detector.update(acceleration(0, 0, 8_192), 0);
        assert!(matches!(
            detector.update(acceleration(7_000, 0, 8_192), 100),
            TapDetection::Candidate { .. }
        ));
        // The first tap rebounds before the detector has seen a calm sample.
        assert_eq!(
            detector.update(acceleration(0, 0, 8_192), 200),
            TapDetection::None
        );
        assert_eq!(
            detector.update(acceleration(0, 0, 8_192), 300),
            TapDetection::None
        );
        assert!(matches!(
            detector.update(acceleration(6_000, 0, 8_192), 400),
            TapDetection::DoubleTap {
                interval_ms: 300,
                ..
            }
        ));
    }

    #[test]
    fn imu_single_tap_and_slow_rotation_never_wake() {
        let mut detector = ImuDoubleTapDetector::default();
        detector.update(acceleration(0, 0, 8_192), 0);
        assert!(matches!(
            detector.update(acceleration(7_000, 0, 8_192), 100),
            TapDetection::Candidate { .. }
        ));
        detector.update(acceleration(0, 0, 8_192), 200);
        detector.update(acceleration(0, 0, 8_192), 300);
        for (index, x) in [1_000, 2_000, 3_000, 4_000, 5_000].into_iter().enumerate() {
            assert_eq!(
                detector.update(acceleration(x, 0, 8_192), 1_100 + index as u64 * 100),
                TapDetection::None
            );
        }
    }

    #[test]
    fn recovery_budget_resets_after_stable_stream() {
        let mut budget = StreamRecoveryBudget::default();
        assert_eq!(budget.record_recovery(), 1);
        for _ in 0..RECOVERY_HEALTHY_SAMPLES - 1 {
            assert_eq!(budget.record_valid_sample(), None);
        }
        assert_eq!(budget.record_valid_sample(), Some(1));
        assert!(budget.can_recover());
        assert_eq!(budget.record_recovery(), 1);
    }

    #[test]
    fn recovery_budget_still_fails_closed_on_consecutive_gaps() {
        let mut budget = StreamRecoveryBudget::default();
        assert_eq!(budget.record_recovery(), 1);
        for _ in 0..RECOVERY_HEALTHY_SAMPLES - 1 {
            budget.record_valid_sample();
        }
        assert_eq!(budget.record_recovery(), 2);
        assert!(!budget.can_recover());
    }

    #[test]
    fn accepts_a_late_sequence_zero_only_inside_the_post_start_grace_window() {
        let sequence_zero = ImuStreamEvaluation::Sample(sample(0, 0.0));
        assert!(should_accept_stream_epoch_reset(
            &sequence_zero,
            false,
            2_200,
            1_000
        ));
        assert!(!should_accept_stream_epoch_reset(
            &sequence_zero,
            false,
            2_501,
            1_000
        ));
        assert!(!should_accept_stream_epoch_reset(
            &sequence_zero,
            true,
            1_200,
            1_000
        ));
    }

    #[test]
    fn never_treats_a_nonzero_sequence_as_a_new_stream_epoch() {
        let sequence_one = ImuStreamEvaluation::Sample(sample(1, 0.0));
        assert!(!should_accept_stream_epoch_reset(
            &sequence_one,
            false,
            1_200,
            1_000
        ));
    }

    #[test]
    fn tap_detection_waits_for_the_stream_to_settle() {
        assert!(!tap_detection_ready(2_499, 1_000));
        assert!(tap_detection_ready(2_500, 1_000));
    }

    #[test]
    fn persistent_double_tap_controller_keeps_retrying_after_recovery_budget_is_used() {
        assert!(should_retry_stream(false, true));
        assert!(should_retry_stream(true, false));
        assert!(!should_retry_stream(false, false));
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
    fn uart_ready_query_and_response_are_strict() {
        assert_eq!(
            uart_battery_query_packet(),
            [0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x03]
        );
        let mut response = [0u8; 16];
        response[0] = 0x83;
        response[1] = 72;
        response[15] = response[..15]
            .iter()
            .fold(0u8, |sum, value| sum.wrapping_add(*value));
        assert_eq!(decode_uart_battery_response(&response), Some(72));
        response[15] ^= 1;
        assert_eq!(decode_uart_battery_response(&response), None);
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
