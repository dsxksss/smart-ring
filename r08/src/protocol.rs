//! COLMI/R08 packet helpers. No BLE I/O lives here.

use std::fmt::Write as _;

pub const COLMI_PACKET_LEN: usize = 16;
pub const WHEEL_DELTA: i32 = 120;
pub const SMOOTH_SCROLL_STEPS_PER_NOTCH: i32 = 3;
pub const SMOOTH_SCROLL_MAX_QUEUED_STEPS: usize = 60;
pub const R08_TAP_FLUSH_MS: u64 = 850;
pub const R08_TAP_DEBOUNCE_MS: u64 = 60;
pub const IMU_STREAM_HEADER: u8 = 0xA2;
pub const IMU_STREAM_SAMPLE_V1: u8 = 0x10;
pub const IMU_STREAM_FLAG_VALID: u8 = 1 << 0;
pub const IMU_STREAM_FLAG_STALE: u8 = 1 << 1;
pub const IMU_STREAM_FLAG_FIFO_OVERFLOW: u8 = 1 << 2;
pub const IMU_STREAM_FLAG_ENDING: u8 = 1 << 3;
// The v7 stream is nominally 10 Hz, but the Windows WinRT BLE path has shown
// isolated notification gaps longer than 250 ms. Wheel output is sample-driven
// (there is no host-side inertia), so waiting 750 ms cannot continue scrolling
// without new data while still stopping well before the firmware's 12 s limit.
pub const IMU_STREAM_NO_DATA_TIMEOUT_MS: u64 = 750;
pub const IMU_STREAM_COMMAND: u8 = 0x09;
pub const FIRMWARE_CAPABILITY_PROBE_COMMAND: u8 = 0x0A;
pub const IMU_TOUCH_V9_CAPABILITY_STATUS: u8 = 0xFC;
pub const IMU_TOUCH_V10_CAPABILITY_STATUS: u8 = 0xFB;
pub const IMU_TOUCH_V11_CAPABILITY_STATUS: u8 = 0xFA;

pub const NORDIC_UART_SERVICE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6e40fff0_b5a3_f393_e0a9_e50e24dcca9e);
pub const NORDIC_UART_WRITE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6e400002_b5a3_f393_e0a9_e50e24dcca9e);
pub const NORDIC_UART_NOTIFY: uuid::Uuid =
    uuid::Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);
pub const DIS_SERVICE: uuid::Uuid = uuid::Uuid::from_u128(0x0000180a_0000_1000_8000_00805f9b34fb);
pub const DFU_SERVICE: uuid::Uuid = uuid::Uuid::from_u128(0xde5bf728_d711_4e47_af26_65e3012a5dc7);
pub const DFU_NOTIFY: uuid::Uuid = uuid::Uuid::from_u128(0xde5bf729_d711_4e47_af26_65e3012a5dc7);
pub const DFU_WRITE: uuid::Uuid = uuid::Uuid::from_u128(0xde5bf72a_d711_4e47_af26_65e3012a5dc7);

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("请输入十六进制数据")]
    EmptyHex,
    #[error("数据中包含非十六进制字符")]
    NonHex,
    #[error("十六进制数据必须由完整字节组成，例如 02 04")]
    OddHex,
    #[error("COLMI 命令不能为空")]
    EmptyCommand,
    #[error("COLMI 命令正文最多 15 字节")]
    CommandTooLong,
    #[error("滚动方向必须是 -1 或 1")]
    InvalidScrollDirection,
    #[error("滚动格数必须至少为 1")]
    InvalidScrollNotches,
    #[error("拒绝写入 DFU 特征；本程序不提供固件刷写，且当前恢复路径尚未验证")]
    DfuWriteBlocked,
}

pub fn parse_hex_payload(value: &str) -> Result<Vec<u8>, ProtocolError> {
    let cleaned = value.trim().to_ascii_lowercase().replace("0x", "");
    let cleaned: String = cleaned
        .chars()
        .filter(|ch| !matches!(ch, ' ' | '\t' | '\n' | '\r' | ':' | ',' | ';' | '_' | '-'))
        .collect();
    if cleaned.is_empty() {
        return Err(ProtocolError::EmptyHex);
    }
    if !cleaned.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(ProtocolError::NonHex);
    }
    if cleaned.len() % 2 == 1 {
        return Err(ProtocolError::OddHex);
    }
    Ok((0..cleaned.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&cleaned[i..i + 2], 16).expect("hex digits"))
        .collect())
}

pub fn format_packet(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 3);
    for (index, byte) in data.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{byte:02X}");
    }
    out
}

pub fn build_colmi_packet(payload: &[u8]) -> Result<[u8; COLMI_PACKET_LEN], ProtocolError> {
    if payload.is_empty() {
        return Err(ProtocolError::EmptyCommand);
    }
    if payload.len() > 15 {
        return Err(ProtocolError::CommandTooLong);
    }
    let mut packet = [0u8; COLMI_PACKET_LEN];
    packet[..payload.len()].copy_from_slice(payload);
    packet[15] = payload
        .iter()
        .copied()
        .fold(0u8, |acc, b| acc.wrapping_add(b));
    Ok(packet)
}

pub fn checksum_ok(data: &[u8]) -> bool {
    data.len() == COLMI_PACKET_LEN
        && data[..15]
            .iter()
            .copied()
            .fold(0u8, |acc, b| acc.wrapping_add(b))
            == data[15]
}

pub fn touch_enable_packet(app_type: u8, sleep_minutes: u8) -> [u8; COLMI_PACKET_LEN] {
    build_colmi_packet(&[0x3B, 0x02, 0x00, app_type, sleep_minutes]).expect("touch packet")
}

pub fn touch_read_packet() -> [u8; COLMI_PACKET_LEN] {
    build_colmi_packet(&[0x3B, 0x01, 0x00]).expect("touch read")
}

pub fn touch_disable_packet(sleep_minutes: u8) -> [u8; COLMI_PACKET_LEN] {
    touch_enable_packet(0, sleep_minutes)
}

pub fn raw_sensor_stop_packet() -> [u8; COLMI_PACKET_LEN] {
    build_colmi_packet(&[0xA1, 0x02]).expect("raw stop")
}

pub fn raw_sensor_start_packet() -> [u8; COLMI_PACKET_LEN] {
    build_colmi_packet(&[0xA1, 0x04, 0x04]).expect("raw start")
}

/// Request one snapshot of the stock firmware's five diagnostic channels.
///
/// Unlike [`raw_sensor_start_packet`], this does not enable the recurring
/// optical/raw-sensor mode. The exact RT08_V3.1 firmware handles subcommand
/// `0x03` by emitting one A1 01..05 snapshot; A1 04 contains the four touch
/// controller channel readings.
pub fn touch_electrode_snapshot_packet() -> [u8; COLMI_PACKET_LEN] {
    build_colmi_packet(&[0xA1, 0x03]).expect("touch electrode snapshot")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchElectrodeSample {
    pub channels: [u16; 4],
    pub channels_valid: bool,
}

/// Decode the four touch-controller values returned in the stock A1 04
/// diagnostic channel.
///
/// This packet proves four readable channels exist. Their physical upper/lower
/// mapping and whether touch raises or lowers a value still require a real-ring
/// baseline/hold comparison, so this decoder deliberately does not assign
/// spatial labels.
pub fn decode_touch_electrode_packet(data: &[u8]) -> Option<TouchElectrodeSample> {
    if data.len() != COLMI_PACKET_LEN || !checksum_ok(data) || data[..2] != [0xA1, 0x04] {
        return None;
    }
    Some(TouchElectrodeSample {
        channels: [
            u16::from_be_bytes([data[2], data[3]]),
            u16::from_be_bytes([data[4], data[5]]),
            u16::from_be_bytes([data[6], data[7]]),
            u16::from_be_bytes([data[8], data[9]]),
        ],
        channels_valid: data[10] != 0,
    })
}

pub fn uart_battery_query_packet() -> [u8; COLMI_PACKET_LEN] {
    build_colmi_packet(&[0x03]).expect("battery query")
}

/// Official QRing "find device" request used by the stock app.
///
/// This is deliberately separate from the A1 optical sensor commands. On the
/// RT08_V3.1 stock firmware, command 0x50 has its own indicator-sequence
/// handler. The target ring visibly runs this sequence on the expected touch
/// indicator, with more than five flashes from one request.
pub fn find_device_packet() -> [u8; COLMI_PACKET_LEN] {
    build_colmi_packet(&[0x50, 0x55, 0xAA]).expect("find device")
}

pub fn decode_uart_battery_response(packet: &[u8]) -> Option<u8> {
    if packet.len() != COLMI_PACKET_LEN || packet[0] & 0x7F != 0x03 || packet[1] > 100 {
        return None;
    }
    checksum_ok(packet).then_some(packet[1])
}

/// Start the experimental IMU-only stream implemented by the offline R08
/// candidate. Stock firmware does not implement this command.
pub fn imu_stream_start_packet() -> [u8; COLMI_PACKET_LEN] {
    build_colmi_packet(&[0xA1, IMU_STREAM_COMMAND, 0x01]).expect("imu stream start")
}

/// Stop the experimental IMU-only stream. The candidate also has an on-device
/// watchdog, but the host always sends this on normal and error exits.
pub fn imu_stream_stop_packet() -> [u8; COLMI_PACKET_LEN] {
    build_colmi_packet(&[0xA1, IMU_STREAM_COMMAND, 0x00]).expect("imu stream stop")
}

/// Query the independent A1 status marker embedded in reviewed custom builds.
///
/// Stock replies 0xFF, v7/v8 reply 0xFD, the HID-mouse-blocked v9 build replies
/// 0xFC, native wheel-only v10 replies 0xFB, and filtered v11 replies 0xFA. This command does not
/// start a sensor or change touch state.
pub fn firmware_capability_probe_packet() -> [u8; COLMI_PACKET_LEN] {
    build_colmi_packet(&[0xA1, FIRMWARE_CAPABILITY_PROBE_COMMAND, 0x00])
        .expect("firmware capability probe")
}

pub fn decode_firmware_capability_status(packet: &[u8]) -> Option<u8> {
    if packet.len() != COLMI_PACKET_LEN
        || packet[0] != 0xA1
        || !matches!(packet[1], 0xFA..=0xFF)
        || !checksum_ok(packet)
    {
        return None;
    }
    Some(packet[1])
}

pub fn is_imu_touch_v9_capability(packet: &[u8]) -> bool {
    decode_firmware_capability_status(packet) == Some(IMU_TOUCH_V9_CAPABILITY_STATUS)
}

pub fn is_imu_touch_v10_capability(packet: &[u8]) -> bool {
    decode_firmware_capability_status(packet) == Some(IMU_TOUCH_V10_CAPABILITY_STATUS)
}

pub fn is_imu_touch_v11_capability(packet: &[u8]) -> bool {
    decode_firmware_capability_status(packet) == Some(IMU_TOUCH_V11_CAPABILITY_STATUS)
}

pub fn is_reviewed_touch_capability_status(status: u8) -> bool {
    matches!(
        status,
        IMU_TOUCH_V9_CAPABILITY_STATUS
            | IMU_TOUCH_V10_CAPABILITY_STATUS
            | IMU_TOUCH_V11_CAPABILITY_STATUS
    )
}

pub fn is_dfu_uuid(uuid: uuid::Uuid) -> bool {
    uuid == DFU_SERVICE || uuid == DFU_NOTIFY || uuid == DFU_WRITE
}

pub fn reject_if_dfu(uuid: uuid::Uuid) -> Result<(), ProtocolError> {
    if is_dfu_uuid(uuid) {
        Err(ProtocolError::DfuWriteBlocked)
    } else {
        Ok(())
    }
}

pub fn build_smooth_scroll_deltas(direction: i32, notches: i32) -> Result<Vec<i32>, ProtocolError> {
    if direction != -1 && direction != 1 {
        return Err(ProtocolError::InvalidScrollDirection);
    }
    if notches < 1 {
        return Err(ProtocolError::InvalidScrollNotches);
    }
    let step_delta = WHEEL_DELTA / SMOOTH_SCROLL_STEPS_PER_NOTCH;
    Ok(vec![
        direction * step_delta;
        (notches * SMOOTH_SCROLL_STEPS_PER_NOTCH) as usize
    ])
}

fn int12(value: u16) -> i16 {
    let value = value & 0x0FFF;
    if value & 0x0800 != 0 {
        value as i16 - 0x1000
    } else {
        value as i16
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccelerometerSample {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImuStreamSample {
    pub sequence: u8,
    pub flags: u8,
    pub acceleration: AccelerometerSample,
    pub fifo_level: u8,
    pub dropped_samples: u8,
    pub age_ticks: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImuStreamStopReason {
    BadLength,
    BadChecksum,
    InvalidSample,
    Stale,
    FifoOverflow,
    Ending,
    NoDataTimeout,
    SequenceGap { expected: u8, received: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImuStreamEvaluation {
    NotStreamPacket,
    Sample(ImuStreamSample),
    Stop(ImuStreamStopReason),
}

/// Evaluate the proposed A2/10 IMU-only packet without enabling any injection.
///
/// The stream is deliberately fail-closed: once an A2/10 packet is recognized,
/// malformed, stale, overflowing, ending, or non-contiguous data requests an
/// immediate stop instead of reusing the last motion sample.
pub fn evaluate_imu_stream_packet(data: &[u8], last_sequence: Option<u8>) -> ImuStreamEvaluation {
    if data.get(..2) != Some(&[IMU_STREAM_HEADER, IMU_STREAM_SAMPLE_V1]) {
        return ImuStreamEvaluation::NotStreamPacket;
    }
    if data.len() != COLMI_PACKET_LEN {
        return ImuStreamEvaluation::Stop(ImuStreamStopReason::BadLength);
    }
    if !checksum_ok(data) {
        return ImuStreamEvaluation::Stop(ImuStreamStopReason::BadChecksum);
    }

    let sequence = data[2];
    let flags = data[3];
    if flags & IMU_STREAM_FLAG_ENDING != 0 {
        return ImuStreamEvaluation::Stop(ImuStreamStopReason::Ending);
    }
    if flags & IMU_STREAM_FLAG_STALE != 0 {
        return ImuStreamEvaluation::Stop(ImuStreamStopReason::Stale);
    }
    if flags & IMU_STREAM_FLAG_FIFO_OVERFLOW != 0 {
        return ImuStreamEvaluation::Stop(ImuStreamStopReason::FifoOverflow);
    }
    if flags & IMU_STREAM_FLAG_VALID == 0 {
        return ImuStreamEvaluation::Stop(ImuStreamStopReason::InvalidSample);
    }
    if let Some(last_sequence) = last_sequence {
        let expected = last_sequence.wrapping_add(1);
        if sequence != expected {
            return ImuStreamEvaluation::Stop(ImuStreamStopReason::SequenceGap {
                expected,
                received: sequence,
            });
        }
    }

    ImuStreamEvaluation::Sample(ImuStreamSample {
        sequence,
        flags,
        acceleration: AccelerometerSample {
            x: i16::from_le_bytes([data[4], data[5]]),
            y: i16::from_le_bytes([data[6], data[7]]),
            z: i16::from_le_bytes([data[8], data[9]]),
        },
        fifo_level: data[10],
        dropped_samples: data[11],
        age_ticks: u16::from_be_bytes([data[12], data[13]]),
    })
}

#[derive(Debug, Default)]
pub struct ImuStreamTracker {
    active: bool,
    last_sequence: Option<u8>,
    last_sample_at_ms: Option<u64>,
}

impl ImuStreamTracker {
    pub fn start(&mut self, now_ms: u64) {
        self.active = true;
        self.last_sequence = None;
        self.last_sample_at_ms = Some(now_ms);
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.last_sequence = None;
        self.last_sample_at_ms = None;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn ingest(&mut self, data: &[u8], now_ms: u64) -> ImuStreamEvaluation {
        let evaluation = evaluate_imu_stream_packet(data, self.last_sequence);
        match evaluation {
            ImuStreamEvaluation::Sample(sample) => {
                if self.active {
                    self.last_sequence = Some(sample.sequence);
                    self.last_sample_at_ms = Some(now_ms);
                }
            }
            ImuStreamEvaluation::Stop(_) => self.stop(),
            ImuStreamEvaluation::NotStreamPacket => {}
        }
        evaluation
    }

    pub fn check_timeout(&mut self, now_ms: u64) -> Option<ImuStreamStopReason> {
        if !self.active {
            return None;
        }
        let last_sample_at_ms = self.last_sample_at_ms.unwrap_or(now_ms);
        if now_ms.saturating_sub(last_sample_at_ms) < IMU_STREAM_NO_DATA_TIMEOUT_MS {
            return None;
        }
        self.stop();
        Some(ImuStreamStopReason::NoDataTimeout)
    }
}

pub fn decode_accelerometer_packet(data: &[u8]) -> Option<AccelerometerSample> {
    if data.len() != COLMI_PACKET_LEN || !checksum_ok(data) || data[..2] != [0xA1, 0x03] {
        return None;
    }
    Some(AccelerometerSample {
        y: int12(u16::from(data[2]) << 4 | u16::from(data[3] & 0x0F)),
        z: int12(u16::from(data[4]) << 4 | u16::from(data[5] & 0x0F)),
        x: int12(u16::from(data[6]) << 4 | u16::from(data[7] & 0x0F)),
    })
}

pub fn describe_colmi_packet(data: &[u8]) -> String {
    if data.len() != COLMI_PACKET_LEN {
        return String::new();
    }
    if !checksum_ok(data) {
        return "校验和不匹配".to_string();
    }
    match data[0] {
        0x73 => {
            if data[1] == 0x2A {
                match data[2] {
                    0 => "R08 触控状态：已唤醒".to_string(),
                    1 => "R08 触控状态：已休眠，请双击戒指唤醒".to_string(),
                    value => format!("R08 触控状态：未知值 {value}"),
                }
            } else {
                format!("设备通知 0x{:02X}，值={}", data[1], data[2])
            }
        }
        0xAA if data[1] == 0xEE => "设备返回 AA EE（上一条命令未识别或不受支持）".to_string(),
        0x02 if data[1] == 0x02 => "R08 摇动/相机兼容事件（可由 IMU 产生）".to_string(),
        0x1D => {
            let label = match data[1] {
                1 => "点击/播放暂停",
                2 => "下滑/上一项",
                3 => "上滑/下一项",
                4 => "音量增加方向",
                5 => "音量减少方向",
                other => return format!("R08 触摸动作：未知动作 {other}"),
            };
            format!("R08 触摸动作：{label}")
        }
        0x3B => match data[1] {
            0x01 => {
                let enabled = data[2] == 0;
                if enabled {
                    format!(
                        "R08 触摸控制状态：已开启，应用类型={}，休眠={} 分钟，当前休眠={}",
                        data[3],
                        data[4],
                        if data[5] == 1 { "是" } else { "否" }
                    )
                } else {
                    format!("R08 触摸控制状态：已关闭，灵敏度={}", data[4])
                }
            }
            0x02 => "R08 触摸控制设置应答".to_string(),
            other => format!("R08 触摸控制协议操作 0x{other:02X}"),
        },
        0xA1 => match data[1] {
            0x01 => "光学/血氧原始数据".to_string(),
            0x02 => "心率 PPG 原始数据".to_string(),
            0x03 => {
                let sample = decode_accelerometer_packet(data).expect("validated A1 03 packet");
                format!(
                    "三轴加速度原始数据 X={} Y={} Z={}",
                    sample.x, sample.y, sample.z
                )
            }
            0x04 => {
                let sample = decode_touch_electrode_packet(data)
                    .expect("validated A1 04 touch-electrode packet");
                format!(
                    "触控控制器四通道原始数据 C1={} C2={} C3={} C4={} VALID={}",
                    sample.channels[0],
                    sample.channels[1],
                    sample.channels[2],
                    sample.channels[3],
                    sample.channels_valid
                )
            }
            channel => format!("传感器原始数据通道 0x{channel:02X}"),
        },
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_common_separators() {
        assert_eq!(parse_hex_payload("02 04").unwrap(), vec![0x02, 0x04]);
        assert_eq!(
            parse_hex_payload("0xA1:0x04,04").unwrap(),
            vec![0xA1, 0x04, 0x04]
        );
    }

    #[test]
    fn rejects_empty_odd_and_non_hex() {
        for value in ["", "A", "02 GG"] {
            assert!(parse_hex_payload(value).is_err(), "{value}");
        }
    }

    #[test]
    fn formats_uppercase_bytes() {
        assert_eq!(format_packet(&[0x00, 0xA1, 0xFF]), "00 A1 FF");
    }

    #[test]
    fn builds_padded_colmi_packet_with_checksum() {
        let packet = build_colmi_packet(&[0x02, 0x04]).unwrap();
        assert_eq!(packet.len(), 16);
        assert_eq!(
            format_packet(&packet).replace(' ', "").to_ascii_lowercase(),
            "02040000000000000000000000000006"
        );
    }

    #[test]
    fn builds_official_r08_touch_packets() {
        assert_eq!(
            format_packet(&touch_enable_packet(1, 1))
                .replace(' ', "")
                .to_ascii_lowercase(),
            "3b02000101000000000000000000003f"
        );
        assert_eq!(
            format_packet(&touch_enable_packet(2, 1))
                .replace(' ', "")
                .to_ascii_lowercase(),
            "3b020002010000000000000000000040"
        );
        assert_eq!(
            format_packet(&touch_disable_packet(1))
                .replace(' ', "")
                .to_ascii_lowercase(),
            "3b02000001000000000000000000003e"
        );
        assert_eq!(
            format_packet(&touch_read_packet())
                .replace(' ', "")
                .to_ascii_lowercase(),
            "3b01000000000000000000000000003c"
        );
    }

    #[test]
    fn identifies_only_checksum_valid_reviewed_touch_capability_markers() {
        assert_eq!(
            format_packet(&firmware_capability_probe_packet()),
            "A1 0A 00 00 00 00 00 00 00 00 00 00 00 00 00 AB"
        );
        let v9 = build_colmi_packet(&[0xA1, IMU_TOUCH_V9_CAPABILITY_STATUS]).unwrap();
        assert_eq!(
            decode_firmware_capability_status(&v9),
            Some(IMU_TOUCH_V9_CAPABILITY_STATUS)
        );
        assert!(is_imu_touch_v9_capability(&v9));

        let v10 = build_colmi_packet(&[0xA1, IMU_TOUCH_V10_CAPABILITY_STATUS]).unwrap();
        assert_eq!(
            decode_firmware_capability_status(&v10),
            Some(IMU_TOUCH_V10_CAPABILITY_STATUS)
        );
        assert!(is_imu_touch_v10_capability(&v10));
        assert!(is_reviewed_touch_capability_status(
            IMU_TOUCH_V10_CAPABILITY_STATUS
        ));
        let v11 = build_colmi_packet(&[0xA1, IMU_TOUCH_V11_CAPABILITY_STATUS]).unwrap();
        assert_eq!(
            decode_firmware_capability_status(&v11),
            Some(IMU_TOUCH_V11_CAPABILITY_STATUS)
        );
        assert!(is_imu_touch_v11_capability(&v11));
        assert!(is_reviewed_touch_capability_status(
            IMU_TOUCH_V11_CAPABILITY_STATUS
        ));

        let v8 = build_colmi_packet(&[0xA1, 0xFD]).unwrap();
        assert_eq!(decode_firmware_capability_status(&v8), Some(0xFD));
        assert!(!is_imu_touch_v9_capability(&v8));

        let mut corrupted = v9;
        corrupted[15] ^= 1;
        assert_eq!(decode_firmware_capability_status(&corrupted), None);
        assert!(!is_imu_touch_v9_capability(&corrupted));
    }

    #[test]
    fn describes_observed_r08_notification() {
        let packet = parse_hex_payload("732a010000000000000000000000009e").unwrap();
        let description = describe_colmi_packet(&packet);
        assert!(description.contains("已休眠"), "{description}");
        assert!(description.contains("双击"), "{description}");

        let awake = parse_hex_payload("732a000000000000000000000000009d").unwrap();
        assert!(describe_colmi_packet(&awake).contains("已唤醒"));
    }

    #[test]
    fn builds_known_raw_sensor_packets() {
        assert_eq!(
            format_packet(&raw_sensor_start_packet())
                .replace(' ', "")
                .to_ascii_lowercase(),
            "a10404000000000000000000000000a9"
        );
        assert_eq!(
            format_packet(&raw_sensor_stop_packet())
                .replace(' ', "")
                .to_ascii_lowercase(),
            "a10200000000000000000000000000a3"
        );
    }

    #[test]
    fn builds_official_find_device_packet_without_optical_command() {
        let packet = find_device_packet();
        assert_eq!(
            format_packet(&packet),
            "50 55 AA 00 00 00 00 00 00 00 00 00 00 00 00 4F"
        );
        assert_ne!(&packet[..3], &[0xA1, 0x04, 0x04]);
    }

    #[test]
    fn describes_unsupported_command_response() {
        let packet = parse_hex_payload("aaee0000000000000000000000000098").unwrap();
        assert!(describe_colmi_packet(&packet).contains("未识别或不受支持"));
    }

    #[test]
    fn describes_observed_remote_event() {
        let packet = parse_hex_payload("02020000000000000000000000000004").unwrap();
        assert!(describe_colmi_packet(&packet).contains("可由 IMU 产生"));
    }

    #[test]
    fn describes_music_touch_action() {
        let packet = parse_hex_payload("1d02000000000000000000000000001f").unwrap();
        let description = describe_colmi_packet(&packet);
        assert!(description.contains("下滑"), "{description}");
        assert!(description.contains("上一项"), "{description}");
    }

    #[test]
    fn tap_window_accepts_observed_slow_triple_click() {
        assert_eq!(R08_TAP_FLUSH_MS, 850);
    }

    #[test]
    fn smooth_scroll_preserves_total_wheel_distance() {
        let up = build_smooth_scroll_deltas(1, 2).unwrap();
        let down = build_smooth_scroll_deltas(-1, 3).unwrap();
        assert_eq!(up.len(), 6);
        assert_eq!(up.iter().sum::<i32>(), 2 * WHEEL_DELTA);
        assert_eq!(down.len(), 9);
        assert_eq!(down.iter().sum::<i32>(), -3 * WHEEL_DELTA);
    }

    #[test]
    fn smooth_scroll_rejects_invalid_values() {
        assert!(build_smooth_scroll_deltas(0, 1).is_err());
        assert!(build_smooth_scroll_deltas(1, 0).is_err());
    }

    #[test]
    fn describes_r08_touch_status() {
        let packet = parse_hex_payload("3b01000101000000000000000000003e").unwrap();
        let description = describe_colmi_packet(&packet);
        assert!(description.contains("已开启"), "{description}");
        assert!(description.contains("应用类型=1"), "{description}");
        assert!(description.contains("休眠=1 分钟"), "{description}");
    }

    #[test]
    fn decodes_accelerometer_packet() {
        let packet = parse_hex_payload("a1031fe4fe88010c000000000000003a").unwrap();
        assert_eq!(
            decode_accelerometer_packet(&packet),
            Some(AccelerometerSample {
                x: 28,
                y: 500,
                z: -24,
            })
        );
        let description = describe_colmi_packet(&packet);
        assert!(description.contains("X=28"), "{description}");
        assert!(description.contains("Y=500"), "{description}");
        assert!(description.contains("Z=-24"), "{description}");
    }

    #[test]
    fn rejects_bad_or_non_accelerometer_packets() {
        let mut packet = raw_sensor_start_packet();
        assert_eq!(decode_accelerometer_packet(&packet), None);
        packet[0] = 0xA1;
        packet[1] = 0x03;
        assert_eq!(decode_accelerometer_packet(&packet), None);
    }

    #[test]
    fn builds_one_shot_touch_electrode_query_without_optical_start() {
        let packet = touch_electrode_snapshot_packet();
        assert_eq!(&packet[..3], &[0xA1, 0x03, 0x00]);
        assert_ne!(packet, raw_sensor_start_packet());
        assert_eq!(packet[15], 0xA4);
    }

    #[test]
    fn decodes_touch_electrode_channels_without_inventing_spatial_labels() {
        let packet = build_colmi_packet(&[
            0xA1, 0x04, 0x03, 0xE8, 0x04, 0x4C, 0x12, 0x34, 0xAB, 0xCD, 0x01,
        ])
        .unwrap();
        let sample = decode_touch_electrode_packet(&packet).unwrap();
        assert_eq!(sample.channels, [1000, 1100, 0x1234, 0xABCD]);
        assert!(sample.channels_valid);
        let description = describe_colmi_packet(&packet);
        assert!(description.contains("C1=1000"), "{description}");
        assert!(description.contains("C4=43981"), "{description}");
    }

    #[test]
    fn rejects_bad_touch_electrode_packets() {
        let mut packet = build_colmi_packet(&[0xA1, 0x04, 0, 1, 0, 2, 0, 3, 0, 4, 1]).unwrap();
        packet[15] ^= 1;
        assert_eq!(decode_touch_electrode_packet(&packet), None);
        assert_eq!(
            decode_touch_electrode_packet(&touch_electrode_snapshot_packet()),
            None
        );
    }

    fn imu_stream_packet(
        sequence: u8,
        flags: u8,
        acceleration: AccelerometerSample,
    ) -> [u8; COLMI_PACKET_LEN] {
        let [x_lo, x_hi] = acceleration.x.to_le_bytes();
        let [y_lo, y_hi] = acceleration.y.to_le_bytes();
        let [z_lo, z_hi] = acceleration.z.to_le_bytes();
        build_colmi_packet(&[
            IMU_STREAM_HEADER,
            IMU_STREAM_SAMPLE_V1,
            sequence,
            flags,
            x_lo,
            x_hi,
            y_lo,
            y_hi,
            z_lo,
            z_hi,
            12,
            3,
            0,
            25,
        ])
        .unwrap()
    }

    #[test]
    fn evaluates_proposed_imu_stream_packet_without_injection() {
        let acceleration = AccelerometerSample {
            x: -444,
            y: 264,
            z: 100,
        };
        let packet = imu_stream_packet(7, IMU_STREAM_FLAG_VALID, acceleration);
        assert_eq!(
            evaluate_imu_stream_packet(&packet, Some(6)),
            ImuStreamEvaluation::Sample(ImuStreamSample {
                sequence: 7,
                flags: IMU_STREAM_FLAG_VALID,
                acceleration,
                fifo_level: 12,
                dropped_samples: 3,
                age_ticks: 25,
            })
        );
    }

    #[test]
    fn imu_stream_sequence_wrap_is_contiguous() {
        let packet = imu_stream_packet(
            0,
            IMU_STREAM_FLAG_VALID,
            AccelerometerSample { x: 0, y: 0, z: 0 },
        );
        assert!(matches!(
            evaluate_imu_stream_packet(&packet, Some(255)),
            ImuStreamEvaluation::Sample(_)
        ));
    }

    #[test]
    fn imu_stream_fail_closed_conditions_request_stop() {
        let sample = AccelerometerSample { x: 1, y: 2, z: 3 };
        for (flags, reason) in [
            (0, ImuStreamStopReason::InvalidSample),
            (
                IMU_STREAM_FLAG_VALID | IMU_STREAM_FLAG_STALE,
                ImuStreamStopReason::Stale,
            ),
            (
                IMU_STREAM_FLAG_VALID | IMU_STREAM_FLAG_FIFO_OVERFLOW,
                ImuStreamStopReason::FifoOverflow,
            ),
            (
                IMU_STREAM_FLAG_VALID | IMU_STREAM_FLAG_ENDING,
                ImuStreamStopReason::Ending,
            ),
        ] {
            let packet = imu_stream_packet(4, flags, sample);
            assert_eq!(
                evaluate_imu_stream_packet(&packet, Some(3)),
                ImuStreamEvaluation::Stop(reason)
            );
        }

        let packet = imu_stream_packet(4, IMU_STREAM_FLAG_VALID, sample);
        assert_eq!(
            evaluate_imu_stream_packet(&packet, Some(1)),
            ImuStreamEvaluation::Stop(ImuStreamStopReason::SequenceGap {
                expected: 2,
                received: 4,
            })
        );

        let mut bad_checksum = packet;
        bad_checksum[15] ^= 1;
        assert_eq!(
            evaluate_imu_stream_packet(&bad_checksum, Some(3)),
            ImuStreamEvaluation::Stop(ImuStreamStopReason::BadChecksum)
        );
        assert_eq!(
            evaluate_imu_stream_packet(&packet[..15], Some(3)),
            ImuStreamEvaluation::Stop(ImuStreamStopReason::BadLength)
        );
        assert_eq!(
            evaluate_imu_stream_packet(&[0x02, 0x02], None),
            ImuStreamEvaluation::NotStreamPacket
        );
    }

    #[test]
    fn imu_stream_tracker_stops_after_750ms_without_data() {
        let mut tracker = ImuStreamTracker::default();
        tracker.start(1_000);
        assert!(tracker.is_active());
        assert_eq!(tracker.check_timeout(1_749), None);
        assert_eq!(
            tracker.check_timeout(1_750),
            Some(ImuStreamStopReason::NoDataTimeout)
        );
        assert!(!tracker.is_active());
    }

    #[test]
    fn imu_stream_tracker_resets_deadline_and_sequence_on_valid_sample() {
        let mut tracker = ImuStreamTracker::default();
        tracker.start(1_000);
        let sample = AccelerometerSample { x: 1, y: 2, z: 3 };
        let first = imu_stream_packet(9, IMU_STREAM_FLAG_VALID, sample);
        assert!(matches!(
            tracker.ingest(&first, 1_200),
            ImuStreamEvaluation::Sample(_)
        ));
        assert_eq!(tracker.check_timeout(1_949), None);

        let gap = imu_stream_packet(11, IMU_STREAM_FLAG_VALID, sample);
        assert_eq!(
            tracker.ingest(&gap, 1_950),
            ImuStreamEvaluation::Stop(ImuStreamStopReason::SequenceGap {
                expected: 10,
                received: 11,
            })
        );
        assert!(!tracker.is_active());
    }

    #[test]
    fn blocks_dfu_uuids() {
        assert!(reject_if_dfu(DFU_WRITE).is_err());
        assert!(reject_if_dfu(NORDIC_UART_WRITE).is_ok());
    }
}
