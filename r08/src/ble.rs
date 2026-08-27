use std::pin::Pin;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::{Stream, StreamExt};
use tokio::time::timeout;
use uuid::Uuid;

#[cfg(windows)]
use crate::identity::RING_MAC;
use crate::identity::{is_ring_advertisement, RING_NAME};
use crate::protocol::{
    decode_uart_battery_response, format_packet, is_dfu_uuid, reject_if_dfu,
    uart_battery_query_packet, DIS_SERVICE, NORDIC_UART_NOTIFY, NORDIC_UART_WRITE,
};

pub struct RingConnection {
    pub name: String,
    pub address: String,
    backend: RingBackend,
}

enum RingBackend {
    Btleplug {
        peripheral: Peripheral,
        write: Characteristic,
        notify: Characteristic,
    },
    #[cfg(windows)]
    WindowsNative(crate::platform::windows_gatt::WindowsGattConnection),
    #[cfg(windows)]
    WindowsWin32(crate::platform::windows_gatt_win32::WindowsGattWin32Connection),
}

pub async fn adapters() -> Result<Vec<Adapter>> {
    let manager = Manager::new().await.context("初始化蓝牙管理器失败")?;
    let adapters = manager.adapters().await.context("枚举蓝牙适配器失败")?;
    if adapters.is_empty() {
        bail!("没有找到 BLE 适配器");
    }
    Ok(adapters)
}

pub async fn has_adapter() -> bool {
    adapters().await.is_ok()
}

pub async fn scan(scan_seconds: u64) -> Result<Vec<(String, String)>> {
    let adapters = adapters().await?;
    let mut found = Vec::new();
    for adapter in adapters {
        adapter
            .start_scan(ScanFilter::default())
            .await
            .context("开始扫描失败")?;
        tokio::time::sleep(Duration::from_secs(scan_seconds.max(1))).await;
        for peripheral in adapter.peripherals().await.context("读取扫描结果失败")? {
            if let Some((name, address)) = advertisement_identity(&peripheral).await {
                let name_ref = (!name.is_empty()).then_some(name.as_str());
                if is_ring_advertisement(name_ref, &address)
                    || name.to_ascii_uppercase().contains("R08")
                {
                    found.push((name, address));
                }
            }
        }
        let _ = adapter.stop_scan().await;
    }
    found.sort();
    found.dedup();
    Ok(found)
}

pub async fn connect(scan_seconds: u64) -> Result<RingConnection> {
    connect_with_options(scan_seconds, true).await
}

/// Connect without reusing the Win32 system GATT handle.
///
/// This is useful for high-rate notification sessions because a stale Windows
/// service interface can enumerate successfully but time out while enabling
/// the CCCD. WinRT and btleplug establish their own GATT session instead.
pub async fn connect_fresh(scan_seconds: u64) -> Result<RingConnection> {
    connect_with_options(scan_seconds, false).await
}

async fn connect_with_options(
    scan_seconds: u64,
    #[cfg_attr(not(windows), allow(unused_variables))] allow_win32: bool,
) -> Result<RingConnection> {
    #[cfg(windows)]
    if allow_win32 {
        match crate::platform::windows_gatt_win32::WindowsGattWin32Connection::connect_known().await
        {
            Ok(connection) => {
                return Ok(RingConnection {
                    name: RING_NAME.to_string(),
                    address: RING_MAC.to_string(),
                    backend: RingBackend::WindowsWin32(connection),
                });
            }
            Err(error) => {
                tracing::warn!("Windows Win32 GATT 系统连接复用失败，将尝试 WinRT：{error:#}")
            }
        }
    }

    #[cfg(windows)]
    match crate::platform::windows_gatt::WindowsGattConnection::connect_known().await {
        Ok(connection) => {
            return Ok(RingConnection {
                name: RING_NAME.to_string(),
                address: RING_MAC.to_string(),
                backend: RingBackend::WindowsNative(connection),
            });
        }
        Err(error) => tracing::warn!("Windows 已配对设备直连失败，将尝试广播扫描：{error:#}"),
    }

    let adapters = adapters().await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(scan_seconds.max(3));
    for adapter in &adapters {
        adapter
            .start_scan(ScanFilter::default())
            .await
            .context("开始扫描失败")?;
    }
    let mut connected_match = None;
    let mut saw_candidate = false;
    let mut connect_attempt = 0u32;
    let mut last_connect_error = None;
    while tokio::time::Instant::now() < deadline && connected_match.is_none() {
        let mut attempted_candidate = false;
        for adapter in &adapters {
            for candidate in adapter.peripherals().await.unwrap_or_default() {
                if let Some((name, address)) = advertisement_identity(&candidate).await {
                    let name_ref = (!name.is_empty()).then_some(name.as_str());
                    if is_ring_advertisement(name_ref, &address) {
                        saw_candidate = true;
                        attempted_candidate = true;
                        connect_attempt += 1;
                        tracing::info!(attempt = connect_attempt, "连接 {name} ({address})");
                        let remaining =
                            deadline.saturating_duration_since(tokio::time::Instant::now());
                        if remaining.is_zero() {
                            break;
                        }
                        let attempt_timeout = remaining.min(Duration::from_secs(8));
                        match timeout(attempt_timeout, candidate.connect()).await {
                            Ok(Ok(())) => {
                                connected_match = Some((candidate.clone(), name, address));
                            }
                            Ok(Err(error)) => {
                                last_connect_error =
                                    Some(anyhow::Error::new(error).context("GATT 连接失败"));
                            }
                            Err(error) => {
                                last_connect_error =
                                    Some(anyhow::Error::new(error).context("连接超时"));
                            }
                        }
                        if connected_match.is_none() {
                            let _ = candidate.disconnect().await;
                            tracing::warn!(
                                attempt = connect_attempt,
                                "BLE_CONNECT_RETRY 已丢弃本次失败连接；在总等待期限内继续扫描"
                            );
                        }
                        break;
                    }
                }
            }
            if attempted_candidate || connected_match.is_some() {
                break;
            }
        }
        if connected_match.is_none() {
            let delay = if attempted_candidate {
                Duration::from_millis(750)
            } else {
                Duration::from_millis(250)
            };
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            tokio::time::sleep(delay.min(remaining)).await;
        }
    }
    for adapter in &adapters {
        let _ = adapter.stop_scan().await;
    }
    let Some((peripheral, name, address)) = connected_match else {
        if saw_candidate {
            return Err(last_connect_error
                .unwrap_or_else(|| anyhow::anyhow!("GATT 连接失败"))
                .context(format!("在 {scan_seconds} 秒内未能稳定连接 {RING_NAME}")));
        }
        bail!("没有找到 {RING_NAME}。请关闭手机蓝牙后唤醒戒指再试。");
    };
    peripheral
        .discover_services()
        .await
        .context("枚举 GATT 服务失败")?;
    for service in peripheral.services() {
        if is_dfu_uuid(service.uuid) {
            tracing::info!("发现官方 DFU 服务，只记录存在，不会写入");
        }
    }
    let chars: Vec<Characteristic> = peripheral.characteristics().into_iter().collect();
    let write = chars
        .iter()
        .find(|item| item.uuid == NORDIC_UART_WRITE)
        .cloned()
        .with_context(|| {
            format!(
                "缺少写入特征 {NORDIC_UART_WRITE}；实际：{}",
                chars
                    .iter()
                    .map(|item| item.uuid.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let notify = chars
        .iter()
        .find(|item| item.uuid == NORDIC_UART_NOTIFY)
        .cloned()
        .with_context(|| format!("缺少通知特征 {NORDIC_UART_NOTIFY}"))?;
    reject_if_dfu(write.uuid)?;
    Ok(RingConnection {
        name,
        address,
        backend: RingBackend::Btleplug {
            peripheral,
            write,
            notify,
        },
    })
}

async fn advertisement_identity(peripheral: &Peripheral) -> Option<(String, String)> {
    let props = peripheral.properties().await.ok().flatten()?;
    let name = props.local_name.unwrap_or_default();
    let address = props.address.to_string();
    Some((name, address))
}

impl RingConnection {
    pub fn backend_name(&self) -> &'static str {
        match &self.backend {
            RingBackend::Btleplug { .. } => "btleplug",
            #[cfg(windows)]
            RingBackend::WindowsNative(_) => "windows-winrt",
            #[cfg(windows)]
            RingBackend::WindowsWin32(_) => "windows-win32-gatt",
        }
    }

    pub fn supports_v9_touch_imu_combo(&self) -> bool {
        #[cfg(windows)]
        return matches!(&self.backend, RingBackend::WindowsWin32(_));
        #[cfg(not(windows))]
        false
    }

    pub async fn subscribe(&self) -> Result<Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>> {
        match &self.backend {
            RingBackend::Btleplug {
                peripheral, notify, ..
            } => {
                peripheral.subscribe(notify).await.context("启用通知失败")?;
                let stream = peripheral.notifications().await.context("打开通知流失败")?;
                Ok(Box::pin(stream.map(|event| event.value)))
            }
            #[cfg(windows)]
            RingBackend::WindowsNative(connection) => connection.subscribe().await,
            #[cfg(windows)]
            RingBackend::WindowsWin32(connection) => connection.subscribe().await,
        }
    }

    pub async fn write(&self, packet: &[u8]) -> Result<()> {
        match &self.backend {
            RingBackend::Btleplug {
                peripheral, write, ..
            } => {
                reject_if_dfu(write.uuid)?;
                tracing::info!("TX  {}", format_packet(packet));
                peripheral
                    .write(write, packet, WriteType::WithResponse)
                    .await
                    .context("GATT 写入失败")?;
                Ok(())
            }
            #[cfg(windows)]
            RingBackend::WindowsNative(connection) => connection.write(packet).await,
            #[cfg(windows)]
            RingBackend::WindowsWin32(connection) => connection.write(packet).await,
        }
    }

    /// Queue a short Nordic UART command without waiting for an ATT write
    /// response. The IMU stream protocol confirms start/renew through its
    /// sequenced A2/10 notifications, so waiting for a second acknowledgement
    /// only adds WinRT stalls and can close an otherwise healthy GATT object.
    pub async fn write_without_response(&self, packet: &[u8]) -> Result<()> {
        match &self.backend {
            RingBackend::Btleplug {
                peripheral, write, ..
            } => {
                reject_if_dfu(write.uuid)?;
                tracing::info!("TX_NO_RESPONSE  {}", format_packet(packet));
                peripheral
                    .write(write, packet, WriteType::WithoutResponse)
                    .await
                    .context("GATT 无应答写入失败")?;
                Ok(())
            }
            #[cfg(windows)]
            RingBackend::WindowsNative(connection) => {
                connection.write_without_response(packet).await
            }
            #[cfg(windows)]
            RingBackend::WindowsWin32(connection) => connection.write(packet).await,
        }
    }

    pub async fn read_device_information(&self) -> Result<Vec<(String, String)>> {
        match &self.backend {
            RingBackend::Btleplug { peripheral, .. } => {
                read_btleplug_device_information(peripheral).await
            }
            #[cfg(windows)]
            RingBackend::WindowsNative(connection) => connection.read_device_information().await,
            #[cfg(windows)]
            RingBackend::WindowsWin32(connection) => connection.read_device_information().await,
        }
    }

    /// Verify a bidirectional application GATT session without enabling any
    /// sensor, touch, HID, IMU, LED, or DFU function.
    pub async fn verify_uart_round_trip(&self) -> Result<u8> {
        let mut notifications = self.subscribe().await.context("稳定连接检查订阅通知失败")?;
        self.write(&uart_battery_query_packet())
            .await
            .context("稳定连接检查发送只读电量查询失败")?;
        timeout(Duration::from_millis(1_500), async {
            loop {
                let packet = notifications
                    .next()
                    .await
                    .context("稳定连接检查等待通知时连接结束")?;
                if let Some(battery_percent) = decode_uart_battery_response(&packet) {
                    return Ok::<u8, anyhow::Error>(battery_percent);
                }
                tracing::debug!(packet = %format_packet(&packet), "CONNECT_CHECK_IGNORED 忽略非电量通知");
            }
        })
        .await
        .context("稳定连接检查等待电量应答超时")?
    }

    pub async fn disconnect(&self) -> Result<()> {
        match &self.backend {
            RingBackend::Btleplug {
                peripheral, notify, ..
            } => {
                let _ = peripheral.unsubscribe(notify).await;
                peripheral.disconnect().await.ok();
                Ok(())
            }
            #[cfg(windows)]
            RingBackend::WindowsNative(connection) => connection.disconnect().await,
            #[cfg(windows)]
            RingBackend::WindowsWin32(connection) => connection.disconnect().await,
        }
    }
}

async fn read_btleplug_device_information(
    peripheral: &Peripheral,
) -> Result<Vec<(String, String)>> {
    let mut rows = Vec::new();
    for characteristic in peripheral.characteristics() {
        if is_dfu_uuid(characteristic.uuid) {
            continue;
        }
        let Some(label) = dis_label(characteristic.uuid) else {
            continue;
        };
        match peripheral.read(&characteristic).await {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes)
                    .trim_end_matches('\0')
                    .to_string();
                let printable = !text.is_empty() && text.chars().all(|ch| !ch.is_control());
                rows.push((
                    label.to_string(),
                    if printable {
                        format!("{}  [HEX {}]", text, format_packet(&bytes))
                    } else {
                        format!("<二进制>  [HEX {}]", format_packet(&bytes))
                    },
                ));
            }
            Err(error) => rows.push((label.to_string(), format!("读取失败：{error}"))),
        }
    }
    for service in peripheral.services() {
        if service.uuid == DIS_SERVICE {
            tracing::info!("DEVICE_INFO_READ_ONLY 发现标准设备信息服务");
        }
    }
    Ok(rows)
}

fn dis_label(uuid: Uuid) -> Option<&'static str> {
    let short = ((uuid.as_u128() >> 96) & 0xFFFF) as u16;
    match short {
        0x2A23 => Some("System ID"),
        0x2A24 => Some("Model Number"),
        0x2A25 => Some("Serial Number"),
        0x2A26 => Some("Firmware Revision"),
        0x2A27 => Some("Hardware Revision"),
        0x2A28 => Some("Software Revision"),
        0x2A29 => Some("Manufacturer Name"),
        0x2A50 => Some("PnP ID"),
        _ => None,
    }
}
