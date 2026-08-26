//! COLMI/R08 packet helpers. No BLE I/O lives here.

use std::fmt::Write as _;

pub const COLMI_PACKET_LEN: usize = 16;
pub const WHEEL_DELTA: i32 = 120;
pub const SMOOTH_SCROLL_STEPS_PER_NOTCH: i32 = 3;
pub const SMOOTH_SCROLL_MAX_QUEUED_STEPS: usize = 60;
pub const R08_TAP_FLUSH_MS: u64 = 850;
pub const R08_TAP_DEBOUNCE_MS: u64 = 60;

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
    #[error("拒绝写入 DFU 特征；当前固件尚未备份")]
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
                format!("R08 未知状态通知 0x2A={}", data[2])
            } else {
                format!("设备通知 0x{:02X}，值={}", data[1], data[2])
            }
        }
        0xAA if data[1] == 0xEE => "设备返回 AA EE（上一条命令未识别或不受支持）".to_string(),
        0x02 if data[1] == 0x02 => "R08 相机/长按事件（动作=拍照）".to_string(),
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
                let raw_y = int12(u16::from(data[2]) << 4 | u16::from(data[3] & 0x0F));
                let raw_z = int12(u16::from(data[4]) << 4 | u16::from(data[5] & 0x0F));
                let raw_x = int12(u16::from(data[6]) << 4 | u16::from(data[7] & 0x0F));
                format!("三轴加速度原始数据 X={raw_x} Y={raw_y} Z={raw_z}")
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
    fn describes_observed_r08_notification() {
        let packet = parse_hex_payload("732a010000000000000000000000009e").unwrap();
        let description = describe_colmi_packet(&packet);
        assert!(
            description.contains("R08 未知状态通知 0x2A=1"),
            "{description}"
        );
    }

    #[test]
    fn describes_unsupported_command_response() {
        let packet = parse_hex_payload("aaee0000000000000000000000000098").unwrap();
        assert!(describe_colmi_packet(&packet).contains("未识别或不受支持"));
    }

    #[test]
    fn describes_observed_remote_event() {
        let packet = parse_hex_payload("02020000000000000000000000000004").unwrap();
        assert!(describe_colmi_packet(&packet).contains("R08 相机/长按事件"));
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
        let description = describe_colmi_packet(&packet);
        assert!(description.contains("X=28"), "{description}");
        assert!(description.contains("Y=500"), "{description}");
        assert!(description.contains("Z=-24"), "{description}");
    }

    #[test]
    fn blocks_dfu_uuids() {
        assert!(reject_if_dfu(DFU_WRITE).is_err());
        assert!(reject_if_dfu(NORDIC_UART_WRITE).is_ok());
    }
}
