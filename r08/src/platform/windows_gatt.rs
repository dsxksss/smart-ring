use std::pin::Pin;
use std::sync::Mutex;

use anyhow::{anyhow, bail, Context, Result};
use futures::Stream;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;
use windows::core::GUID;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristic, GattClientCharacteristicConfigurationDescriptorValue,
    GattCommunicationStatus, GattDeviceService, GattValueChangedEventArgs, GattWriteOption,
};
use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Foundation::{
    AsyncOperationCompletedHandler, EventRegistrationToken, IAsyncOperation, TypedEventHandler,
};
use windows::Storage::Streams::{DataReader, DataWriter, IBuffer};

use crate::identity::{RING_MAC, RING_NAME};
use crate::protocol::{
    format_packet, is_dfu_uuid, DIS_SERVICE, NORDIC_UART_NOTIFY, NORDIC_UART_SERVICE,
    NORDIC_UART_WRITE,
};

const RING_BLUETOOTH_ADDRESS: u64 = 0x3131_4537_9C07;

pub struct WindowsGattConnection {
    device: Option<BluetoothLEDevice>,
    services: Vec<GattDeviceService>,
    characteristics: Vec<(Uuid, GattCharacteristic)>,
    write: GattCharacteristic,
    notify: GattCharacteristic,
    notify_token: Mutex<Option<EventRegistrationToken>>,
}

impl WindowsGattConnection {
    pub async fn connect_known() -> Result<Self> {
        let (device, services) = match open_registered_service(NORDIC_UART_SERVICE).await {
            Ok(uart_service) => {
                tracing::info!("从 Windows 已注册 GATT 服务接口打开 R08 UART");
                let mut services = vec![uart_service];
                if let Ok(dis_service) = open_registered_service(DIS_SERVICE).await {
                    services.push(dis_service);
                }
                (None, services)
            }
            Err(service_error) => {
                tracing::warn!("按 Windows 已注册 GATT 服务打开 R08 失败：{service_error:#}");
                let device = match open_paired_device().await {
                    Ok(device) => device,
                    Err(paired_error) => {
                        tracing::warn!("按 Windows 已配对设备 ID 打开 R08 失败：{paired_error:#}");
                        await_operation(
                            BluetoothLEDevice::FromBluetoothAddressAsync(RING_BLUETOOTH_ADDRESS)
                                .context("按已知地址打开 Windows 已配对 R08 失败")?,
                        )
                        .await
                        .context("Windows 没有返回已配对 R08 设备")?
                    }
                };
                let service_result = await_operation(
                    device
                        .GetGattServicesWithCacheModeAsync(BluetoothCacheMode::Uncached)
                        .context("请求 Windows GATT 服务失败")?,
                )
                .await
                .context("读取 Windows GATT 服务失败")?;
                require_success(service_result.Status()?, "枚举 GATT 服务")?;
                let services = service_result
                    .Services()
                    .context("Windows 未返回 GATT 服务列表")?
                    .into_iter()
                    .collect();
                (Some(device), services)
            }
        };

        let mut characteristics = Vec::new();
        for service in &services {
            let service_uuid = guid_to_uuid(service.Uuid()?);
            if is_dfu_uuid(service_uuid) {
                tracing::info!("发现官方 DFU 服务，只记录存在，不会写入");
            }
            let result = await_operation(
                service
                    .GetCharacteristicsWithCacheModeAsync(BluetoothCacheMode::Uncached)
                    .with_context(|| format!("请求服务 {service_uuid} 的特征失败"))?,
            )
            .await
            .with_context(|| format!("读取服务 {service_uuid} 的特征失败"))?;
            if result.Status()? != GattCommunicationStatus::Success {
                continue;
            }
            for characteristic in result.Characteristics()?.into_iter() {
                characteristics.push((guid_to_uuid(characteristic.Uuid()?), characteristic));
            }
        }

        let write = characteristics
            .iter()
            .find(|(uuid, _)| *uuid == NORDIC_UART_WRITE)
            .map(|(_, characteristic)| characteristic.clone())
            .with_context(|| format!("Windows 已配对设备缺少写入特征 {NORDIC_UART_WRITE}"))?;
        let notify = characteristics
            .iter()
            .find(|(uuid, _)| *uuid == NORDIC_UART_NOTIFY)
            .map(|(_, characteristic)| characteristic.clone())
            .with_context(|| format!("Windows 已配对设备缺少通知特征 {NORDIC_UART_NOTIFY}"))?;

        tracing::info!("已通过 Windows 系统配对记录直连 {RING_NAME} ({RING_MAC})");
        Ok(Self {
            device,
            services,
            characteristics,
            write,
            notify,
            notify_token: Mutex::new(None),
        })
    }

    pub async fn subscribe(&self) -> Result<Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let handler = TypedEventHandler::new(
            move |_: &Option<GattCharacteristic>, args: &Option<GattValueChangedEventArgs>| {
                if let Some(args) = args {
                    let value = args.CharacteristicValue()?;
                    let bytes = buffer_to_vec(&value)?;
                    let _ = tx.send(bytes);
                }
                Ok(())
            },
        );
        let token = self
            .notify
            .ValueChanged(&handler)
            .context("注册 Windows GATT 通知回调失败")?;
        let status = await_operation(
            self.notify
                .WriteClientCharacteristicConfigurationDescriptorAsync(
                    GattClientCharacteristicConfigurationDescriptorValue::Notify,
                )
                .context("启用 Windows GATT 通知失败")?,
        )
        .await
        .context("Windows GATT 通知配置失败")?;
        if status != GattCommunicationStatus::Success {
            let _ = self.notify.RemoveValueChanged(token);
            bail!("启用 Windows GATT 通知失败：{status:?}");
        }
        *self.notify_token.lock().expect("notify token mutex") = Some(token);
        Ok(Box::pin(UnboundedReceiverStream::new(rx)))
    }

    pub async fn write(&self, packet: &[u8]) -> Result<()> {
        tracing::info!("TX  {}", format_packet(packet));
        let writer = DataWriter::new().context("创建 Windows GATT 写入缓冲区失败")?;
        writer.WriteBytes(packet)?;
        let buffer = writer.DetachBuffer()?;
        let status = await_operation(
            self.write
                .WriteValueWithOptionAsync(&buffer, GattWriteOption::WriteWithResponse)
                .context("发起 Windows GATT 写入失败")?,
        )
        .await
        .context("Windows GATT 写入失败")?;
        require_success(status, "写入 R08 GATT")
    }

    pub async fn read_device_information(&self) -> Result<Vec<(String, String)>> {
        let mut rows = Vec::new();
        for (uuid, characteristic) in &self.characteristics {
            if is_dfu_uuid(*uuid) {
                continue;
            }
            let Some(label) = dis_label(*uuid) else {
                continue;
            };
            let result = await_operation(
                characteristic.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)?,
            )
            .await?;
            if result.Status()? != GattCommunicationStatus::Success {
                rows.push((
                    label.to_string(),
                    format!("读取失败：{:?}", result.Status()?),
                ));
                continue;
            }
            let bytes = buffer_to_vec(&result.Value()?)?;
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
        if self
            .services
            .iter()
            .any(|service| service.Uuid().ok().map(guid_to_uuid) == Some(DIS_SERVICE))
        {
            tracing::info!("DEVICE_INFO_READ_ONLY 发现标准设备信息服务");
        }
        Ok(rows)
    }

    pub async fn disconnect(&self) -> Result<()> {
        self.remove_notification_handler();
        if let Ok(operation) = self
            .notify
            .WriteClientCharacteristicConfigurationDescriptorAsync(
                GattClientCharacteristicConfigurationDescriptorValue::None,
            )
        {
            let _ = await_operation(operation).await;
        }
        for service in &self.services {
            let _ = service.Close();
        }
        if let Some(device) = &self.device {
            let _ = device.Close();
        }
        Ok(())
    }

    fn remove_notification_handler(&self) {
        if let Some(token) = self.notify_token.lock().expect("notify token mutex").take() {
            let _ = self.notify.RemoveValueChanged(token);
        }
    }
}

async fn open_paired_device() -> Result<BluetoothLEDevice> {
    let selector = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)
        .context("生成 Windows 已配对 BLE 查询条件失败")?;
    let devices = await_operation(
        DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .context("查询 Windows 已配对 BLE 设备失败")?,
    )
    .await
    .context("读取 Windows 已配对 BLE 设备失败")?;
    let mut seen = Vec::new();
    for info in devices.into_iter() {
        let name = info.Name()?.to_string();
        let id = info.Id()?;
        let normalized_id = id.to_string().to_ascii_uppercase().replace([':', '-'], "");
        seen.push(name.clone());
        if name.eq_ignore_ascii_case(RING_NAME) || normalized_id.contains("313145379C07") {
            tracing::info!("从 Windows 已配对 BLE 列表找到 {name}");
            return await_operation(
                BluetoothLEDevice::FromIdAsync(&id).context("按 Windows 设备 ID 打开 R08 失败")?,
            )
            .await
            .context("Windows 已配对列表中的 R08 当前不可打开");
        }
    }
    bail!(
        "Windows 已配对 BLE 列表没有 {RING_NAME}（列表：{}）",
        if seen.is_empty() {
            "<空>".to_string()
        } else {
            seen.join(", ")
        }
    )
}

async fn open_registered_service(uuid: Uuid) -> Result<GattDeviceService> {
    let selector = GattDeviceService::GetDeviceSelectorFromUuid(GUID::from_u128(uuid.as_u128()))
        .with_context(|| format!("生成 Windows GATT 服务 {uuid} 查询条件失败"))?;
    let services = await_operation(
        DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .with_context(|| format!("查询 Windows GATT 服务 {uuid} 失败"))?,
    )
    .await
    .with_context(|| format!("读取 Windows GATT 服务 {uuid} 列表失败"))?;
    let mut candidate_count = 0usize;
    for info in services.into_iter() {
        candidate_count += 1;
        let id = info.Id()?;
        if id
            .to_string()
            .to_ascii_uppercase()
            .replace([':', '-'], "")
            .contains("313145379C07")
        {
            return await_operation(
                GattDeviceService::FromIdAsync(&id)
                    .with_context(|| format!("按 Windows 服务 ID 打开 GATT {uuid} 失败"))?,
            )
            .await
            .with_context(|| format!("Windows 已注册 GATT 服务 {uuid} 当前不可打开"));
        }
    }
    bail!("Windows 没有注册 R08 的 GATT 服务 {uuid}（候选数：{candidate_count}）")
}

async fn await_operation<T>(operation: IAsyncOperation<T>) -> Result<T>
where
    T: windows::core::RuntimeType + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    let mut tx = Some(tx);
    operation.SetCompleted(&AsyncOperationCompletedHandler::new(
        move |operation: Option<&IAsyncOperation<T>>, _status| {
            if let Some(tx) = tx.take() {
                let result = operation
                    .ok_or_else(|| anyhow!("Windows 异步操作没有返回结果对象"))
                    .and_then(|operation| operation.GetResults().map_err(Into::into));
                let _ = tx.send(result);
            }
            Ok(())
        },
    ))?;
    rx.await.context("Windows 异步操作回调提前关闭")?
}

impl Drop for WindowsGattConnection {
    fn drop(&mut self) {
        self.remove_notification_handler();
        for service in &self.services {
            let _ = service.Close();
        }
        if let Some(device) = &self.device {
            let _ = device.Close();
        }
    }
}

fn require_success(status: GattCommunicationStatus, action: &str) -> Result<()> {
    if status == GattCommunicationStatus::Success {
        Ok(())
    } else {
        bail!("{action}失败：{status:?}")
    }
}

fn buffer_to_vec(buffer: &IBuffer) -> windows::core::Result<Vec<u8>> {
    let reader = DataReader::FromBuffer(buffer)?;
    let length = reader.UnconsumedBufferLength()? as usize;
    let mut bytes = vec![0u8; length];
    reader.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

fn guid_to_uuid(guid: GUID) -> Uuid {
    Uuid::from_fields(guid.data1, guid.data2, guid.data3, &guid.data4)
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
