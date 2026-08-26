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

use crate::identity::{is_ring_advertisement, RING_MAC, RING_NAME};
use crate::protocol::{
    format_packet, is_dfu_uuid, reject_if_dfu, DIS_SERVICE, NORDIC_UART_NOTIFY, NORDIC_UART_WRITE,
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

async fn connect_with_options(scan_seconds: u64, allow_win32: bool) -> Result<RingConnection> {
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
    let mut matched = None;
    while tokio::time::Instant::now() < deadline && matched.is_none() {
        for adapter in &adapters {
            for candidate in adapter.peripherals().await.unwrap_or_default() {
                if let Some((name, address)) = advertisement_identity(&candidate).await {
                    let name_ref = (!name.is_empty()).then_some(name.as_str());
                    if is_ring_advertisement(name_ref, &address) {
                        matched = Some((candidate, name, address));
                        break;
                    }
                }
            }
            if matched.is_some() {
                break;
            }
        }
        if matched.is_none() {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    for adapter in &adapters {
        let _ = adapter.stop_scan().await;
    }
    let Some((peripheral, name, address)) = matched else {
        bail!("没有找到 {RING_NAME}。请关闭手机蓝牙后唤醒戒指再试。");
    };
    tracing::info!("连接 {name} ({address})");
    timeout(Duration::from_secs(20), peripheral.connect())
        .await
        .context("连接超时")?
        .context("GATT 连接失败")?;
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

    pub async fn read_device_information(&self) -> Result<Vec<(String, String)>> {
        let RingBackend::Btleplug { peripheral, .. } = &self.backend else {
            #[cfg(windows)]
            if let RingBackend::WindowsNative(connection) = &self.backend {
                return connection.read_device_information().await;
            }
            #[cfg(windows)]
            if let RingBackend::WindowsWin32(connection) = &self.backend {
                return connection.read_device_information().await;
            }
            unreachable!("当前平台只存在 btleplug 蓝牙后端");
        };
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
