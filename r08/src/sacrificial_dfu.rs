//! Hash- and identity-locked QRing DFU path for one sacrificial R08.

use std::path::Path;
#[cfg(windows)]
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
#[cfg(windows)]
use futures::{Stream, StreamExt};
use sha2::{Digest, Sha256};

#[cfg(windows)]
use crate::protocol::{
    decode_firmware_capability_status, firmware_capability_probe_packet, format_packet,
    IMU_TOUCH_V11_CAPABILITY_STATUS,
};

pub const EXPECTED_SHA256: &str =
    "7b60058f5d4de8246834acf139b059009495e0dc9a811b5ff041ec33e3e00e0f";
pub const EXPECTED_SIZE: usize = 146_812;
pub const ACK_PHRASE: &str = "I_ACCEPT_PERMANENT_BRICK";
const HEADER_SIZE: usize = 0x50;
const INNER_HEADER_SIZE: usize = 0x400;
const APPLICATION_BASE: u32 = 0x0082_6000;
const OUTER_FIRMWARE: &[u8] = b"RT08_3.10.55_260828";
const DEFAULT_STATUS_ADDRESS: u32 = 0x0082_80CA;
const TOUCH_REPEAT_ADDRESS: u32 = 0x0082_C604;
const TOUCH_WHEEL_MOTION_HOOK_ADDRESS: u32 = 0x0082_9F7E;
const TOUCH_WHEEL_PATCH_ADDRESS: u32 = 0x0082_9FD6;
const TOUCH_WHEEL_EXTENDED_ENTRY_ADDRESS: u32 = 0x0082_9FD4;
const TOUCH_WHEEL_PATCH: &[u8] = &[
    0x01, 0x28, 0x0b, 0xd1, 0x10, 0x2a, 0x07, 0xda, 0x10, 0x24, 0x64, 0x42, 0xa2, 0x42, 0x00, 0xdd,
    0x04, 0xe0, 0x01, 0x23, 0x5b, 0x42, 0x02, 0xe0, 0x01, 0x23, 0x00, 0xe0, 0x00, 0x23, 0x00, 0x20,
    0x00, 0x21, 0x00, 0x22, 0x6c, 0x46, 0x20, 0x72, 0x70, 0x47,
];
const SDK_IMAGE_DIGEST_OFFSET: usize = HEADER_SIZE + 0x174;
const SDK_IMAGE_DIGEST: &[u8] = &[
    0xbe, 0x0c, 0xed, 0x2e, 0x6d, 0x3d, 0x05, 0xb9, 0xb4, 0x08, 0x0f, 0xb8, 0x4a, 0x29, 0xf6, 0x98,
    0xb1, 0x1a, 0xff, 0x43, 0x57, 0xa2, 0xef, 0xa1, 0x38, 0xc7, 0x45, 0xbb, 0x60, 0x6c, 0x66, 0x0a,
];

#[derive(Debug, Clone)]
pub struct Candidate {
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub crc16: u16,
    pub sum16: u16,
    pub blocks: usize,
}

pub fn crc16(data: &[u8]) -> u16 {
    let mut value = 0xffff_u16;
    for byte in data {
        value ^= u16::from(*byte);
        for _ in 0..8 {
            value = if value & 1 != 0 {
                (value >> 1) ^ 0xa001
            } else {
                value >> 1
            };
        }
    }
    value
}

pub fn frame(command: u8, payload: &[u8]) -> Result<Vec<u8>> {
    let length = u16::try_from(payload.len()).context("DFU payload exceeds u16")?;
    let checksum = if payload.is_empty() {
        0xffff
    } else {
        crc16(payload)
    };
    let mut output = Vec::with_capacity(6 + payload.len());
    output.extend_from_slice(&[0xbc, command]);
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(&checksum.to_le_bytes());
    output.extend_from_slice(payload);
    Ok(output)
}

pub fn parse_response(data: &[u8]) -> Result<(u8, u8)> {
    if data.len() < 7 || data[0] != 0xbc {
        bail!("invalid DFU response header: {}", hex(data));
    }
    let length = u16::from_le_bytes([data[2], data[3]]) as usize;
    let stored_crc = u16::from_le_bytes([data[4], data[5]]);
    if length != data.len() - 6 {
        bail!("invalid DFU response length: {}", hex(data));
    }
    if crc16(&data[6..]) != stored_crc {
        bail!("invalid DFU response CRC: {}", hex(data));
    }
    Ok((data[1], data[6]))
}

pub fn load_candidate(path: &Path) -> Result<Candidate> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    if sha256 != EXPECTED_SHA256 {
        bail!("candidate SHA-256 mismatch: expected {EXPECTED_SHA256}, got {sha256}");
    }
    if bytes.len() != EXPECTED_SIZE {
        bail!("candidate size mismatch: {}", bytes.len());
    }
    validate_container(&bytes)?;
    let crc16 = crc16(&bytes);
    let sum16 = bytes
        .iter()
        .fold(0_u16, |sum, value| sum.wrapping_add(u16::from(*value)));
    let blocks = bytes.len().div_ceil(1024);
    Ok(Candidate {
        bytes,
        sha256,
        crc16,
        sum16,
        blocks,
    })
}

fn validate_container(bytes: &[u8]) -> Result<()> {
    if bytes.len() < HEADER_SIZE + INNER_HEADER_SIZE || bytes[..4] != [0xe5, 0xc3, 0xbd, 0x81] {
        bail!("not the reviewed QRing 0x50 OTA container");
    }
    let read_u32 = |offset: usize| {
        u32::from_le_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("validated offset"),
        )
    };
    let payload = &bytes[HEADER_SIZE..];
    let length_a = read_u32(4) as usize;
    let length_b = read_u32(8) as usize;
    let stored_sum = read_u32(12);
    let actual_sum = payload
        .iter()
        .fold(0_u32, |sum, value| sum.wrapping_add(u32::from(*value)));
    if length_a != payload.len() || length_b != payload.len() || stored_sum != actual_sum {
        bail!("QRing outer length/checksum validation failed");
    }
    if !bytes.windows(b"RT08_V3.1".len()).any(|w| w == b"RT08_V3.1") {
        bail!("candidate lacks exact RT08_V3.1 marker");
    }
    if &bytes[0x10..0x10 + OUTER_FIRMWARE.len()] != OUTER_FIRMWARE {
        bail!("candidate outer firmware revision is not the reviewed v11 marker");
    }
    if payload[0] != 12 {
        bail!("candidate is not RTL8762E IC type 12");
    }
    let payload_u32 = |offset: usize| {
        u32::from_le_bytes(
            payload[offset..offset + 4]
                .try_into()
                .expect("validated offset"),
        )
    };
    if payload_u32(8) as usize != payload.len() - INNER_HEADER_SIZE
        || payload_u32(0x1c) != APPLICATION_BASE + INNER_HEADER_SIZE as u32
        || payload_u32(0x28) != APPLICATION_BASE
    {
        bail!("RTL8762E inner length/base validation failed");
    }
    if payload_u32(0x60) != 0x0000_0051 {
        bail!("candidate internal version is not the reviewed 1.5.0 revision");
    }
    if &bytes[SDK_IMAGE_DIGEST_OFFSET..SDK_IMAGE_DIGEST_OFFSET + SDK_IMAGE_DIGEST.len()]
        != SDK_IMAGE_DIGEST
    {
        bail!("candidate lacks the official SDK-generated image digest");
    }
    let marker_offset = HEADER_SIZE + (DEFAULT_STATUS_ADDRESS - APPLICATION_BASE) as usize;
    if bytes[marker_offset..marker_offset + 2] != [0xfa, 0x21] {
        bail!("candidate lacks the independent v11 A1 FA capability marker");
    }
    let touch_repeat_offset = HEADER_SIZE + (TOUCH_REPEAT_ADDRESS - APPLICATION_BASE) as usize;
    if bytes[touch_repeat_offset..touch_repeat_offset + 2] != [0x03, 0x23] {
        bail!("candidate lacks the reviewed three-repeat touch indicator patch");
    }
    let motion_hook_offset =
        HEADER_SIZE + (TOUCH_WHEEL_MOTION_HOOK_ADDRESS - APPLICATION_BASE) as usize;
    if bytes[motion_hook_offset..motion_hook_offset + 4] != [0x00, 0xf0, 0x2a, 0xf8] {
        bail!("candidate lacks the reviewed touch-wheel motion hook");
    }
    let extended_entry_offset =
        HEADER_SIZE + (TOUCH_WHEEL_EXTENDED_ENTRY_ADDRESS - APPLICATION_BASE) as usize;
    if bytes[extended_entry_offset..extended_entry_offset + 2] != [0x70, 0x47] {
        bail!("candidate does not block the extended mouse report path");
    }
    let wheel_patch_offset = HEADER_SIZE + (TOUCH_WHEEL_PATCH_ADDRESS - APPLICATION_BASE) as usize;
    if &bytes[wheel_patch_offset..wheel_patch_offset + TOUCH_WHEEL_PATCH.len()] != TOUCH_WHEEL_PATCH
    {
        bail!("candidate lacks the exact reviewed contact-gated touch-wheel rewrite");
    }
    Ok(())
}

#[cfg(windows)]
fn init_payload(candidate: &Candidate) -> Vec<u8> {
    let mut payload = Vec::with_capacity(9);
    payload.push(1);
    payload.extend_from_slice(&(candidate.bytes.len() as u32).to_le_bytes());
    payload.extend_from_slice(&candidate.crc16.to_le_bytes());
    payload.extend_from_slice(&candidate.sum16.to_le_bytes());
    payload
}

#[cfg(windows)]
fn data_frame(candidate: &Candidate, index: usize) -> Result<Vec<u8>> {
    let start = index * 1024;
    let end = usize::min(start + 1024, candidate.bytes.len());
    let mut payload = Vec::with_capacity(2 + end - start);
    payload.extend_from_slice(&u16::try_from(index + 1)?.to_le_bytes());
    payload.extend_from_slice(&candidate.bytes[start..end]);
    frame(3, &payload)
}

fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|v| format!("{v:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(windows)]
async fn wait_ack<S>(responses: &mut S, expected: u8, wait: Duration) -> Result<()>
where
    S: Stream<Item = Vec<u8>> + Unpin,
{
    let data = tokio::time::timeout(wait, responses.next())
        .await
        .with_context(|| format!("timeout waiting for DFU command {expected} response"))?
        .with_context(|| format!("DFU notification stream ended before command {expected}"))?;
    let (command, status) = parse_response(&data)?;
    if command != expected {
        bail!("unexpected DFU response command {command}, expected {expected}");
    }
    if status != 0 {
        let label = match status {
            1 => "size",
            2 => "content",
            3 => "command-status",
            4 => "format",
            5 => "internal",
            6 => "low-battery",
            _ => "unknown",
        };
        bail!("DFU command {command} rejected: status={status} ({label})");
    }
    Ok(())
}

#[cfg(windows)]
pub async fn probe() -> Result<(u8, String, String)> {
    // The official 0x3B touch-disable command can close the R08's WinRT GATT
    // object while Windows keeps the exact, MAC-qualified service interfaces
    // registered.  Use those Win32 interfaces for the isolated DFU transport.
    let connection =
        crate::platform::windows_gatt_win32::WindowsDfuWin32Connection::connect_exact().await?;
    let battery = connection.battery_percent().await?;
    // `connect_exact` already reads and requires these two exact DIS values.
    let hardware = "RT08_V3.1".to_owned();
    let firmware = "RT08_3.10.48_260309".to_owned();
    connection.disconnect().await?;
    Ok((battery, hardware, firmware))
}

#[cfg(not(windows))]
pub async fn probe() -> Result<(u8, String, String)> {
    bail!("sacrificial DFU is implemented only for Windows")
}

#[cfg(windows)]
pub async fn execute(candidate: &Candidate) -> Result<()> {
    let connection =
        crate::platform::windows_gatt_win32::WindowsDfuWin32Connection::connect_exact().await?;
    let battery = connection.battery_percent().await?;
    if battery < 80 {
        bail!("battery must be at least 80%, current={battery}%");
    }
    let mut responses = connection.subscribe().await?;

    connection.write_frame(&frame(1, &[])?).await?;
    wait_ack(&mut responses, 1, Duration::from_secs(15)).await?;
    println!("DFU START accepted");

    connection
        .write_frame(&frame(2, &init_payload(candidate))?)
        .await?;
    wait_ack(&mut responses, 2, Duration::from_secs(15)).await?;
    println!("DFU INIT accepted");

    let started = Instant::now();
    for index in 0..candidate.blocks {
        connection
            .write_frame(&data_frame(candidate, index)?)
            .await?;
        wait_ack(&mut responses, 3, Duration::from_secs(15)).await?;
        if index == 0 || (index + 1) % 10 == 0 || index + 1 == candidate.blocks {
            println!(
                "DFU DATA {}/{} {}%",
                index + 1,
                candidate.blocks,
                (index + 1) * 100 / candidate.blocks
            );
        }
    }

    connection.write_frame(&frame(4, &[])?).await?;
    wait_ack(&mut responses, 4, Duration::from_secs(30)).await?;
    println!("DFU CHECK accepted");

    connection.write_frame(&frame(5, &[])?).await?;
    println!(
        "DFU END sent elapsed={:.1}s; waiting for reboot",
        started.elapsed().as_secs_f32()
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    let _ = connection.disconnect().await;
    Ok(())
}

#[cfg(windows)]
pub async fn probe_activation_marker() -> Result<Vec<u8>> {
    // After DFU reboot Windows can keep the exact GATT service interfaces
    // available while its WinRT paired-device query returns E_FAIL.  Use the
    // same MAC-qualified Win32 transport that passed the post-flash health
    // check so activation verification does not depend on the stale query.
    let connection =
        crate::platform::windows_gatt_win32::WindowsGattWin32Connection::connect_known().await?;
    let mut notifications = connection.subscribe().await?;
    connection
        .write(&firmware_capability_probe_packet())
        .await?;
    let response_result = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let packet = notifications
                .next()
                .await
                .context("UART notification stream ended")?;
            println!("ACTIVATION_RX {}", format_packet(&packet));
            if let Some(status) = decode_firmware_capability_status(&packet) {
                if status != IMU_TOUCH_V11_CAPABILITY_STATUS {
                    bail!("unexpected activation marker 0x{status:02X}; expected v11 marker 0xFA");
                }
                return Ok::<_, anyhow::Error>(packet);
            }
        }
    })
    .await;
    let _ = connection.disconnect().await;
    response_result.context("timeout waiting for exact v11 A1 FA activation marker")?
}

#[cfg(not(windows))]
pub async fn probe_activation_marker() -> Result<Vec<u8>> {
    bail!("activation marker probe is implemented only for Windows")
}

#[cfg(not(windows))]
pub async fn execute(_candidate: &Candidate) -> Result<()> {
    bail!("sacrificial DFU is implemented only for Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_modbus_reference() {
        assert_eq!(crc16(b"123456789"), 0x4b37);
        assert_eq!(crc16(b""), 0xffff);
    }

    #[test]
    fn empty_command_matches_official_header() {
        assert_eq!(frame(1, &[]).unwrap(), [0xbc, 1, 0, 0, 0xff, 0xff]);
    }

    #[test]
    fn response_is_strictly_validated() {
        let response = frame(3, &[0]).unwrap();
        assert_eq!(parse_response(&response).unwrap(), (3, 0));
        let mut corrupt = response;
        corrupt[4] ^= 1;
        assert!(parse_response(&corrupt).is_err());
    }
}
