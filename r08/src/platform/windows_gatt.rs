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
const DFU_SERVICE_UUID: Uuid = Uuid::from_u128(0xde5bf728_d711_4e47_af26_65e3012a5dc7);
const DFU_NOTIFY_UUID: Uuid = Uuid::from_u128(0xde5bf729_d711_4e47_af26_65e3012a5dc7);
const DFU_WRITE_UUID: Uuid = Uuid::from_u128(0xde5bf72a_d711_4e47_af26_65e3012a5dc7);
const HARDWARE_UUID: Uuid = Uuid::from_u128(0x00002a27_0000_1000_8000_00805f9b34fb);
const FIRMWARE_UUID: Uuid = Uuid::from_u128(0x00002a26_0000_1000_8000_00805f9b34fb);
const BATTERY_UUID: Uuid = Uuid::from_u128(0x00002a19_0000_1000_8000_00805f9b34fb);

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

/// Destructive QRing DFU transport used only by the hash-locked sacrificial
/// test command. The ordinary `RingConnection` continues to reject DFU UUIDs.
pub struct WindowsDfuConnection {
    device: Option<BluetoothLEDevice>,
    services: Vec<GattDeviceService>,
    characteristics: Vec<(Uuid, GattCharacteristic)>,
    write: GattCharacteristic,
    notify: GattCharacteristic,
    uart_write: GattCharacteristic,
    uart_notify: GattCharacteristic,
    notify_token: Mutex<Option<EventRegistrationToken>>,
}

impl WindowsDfuConnection {
    pub async fn connect_exact() -> Result<Self> {
        let (device, services) = match (
            open_registered_service(DFU_SERVICE_UUID).await,
            open_registered_service(DIS_SERVICE).await,
            open_registered_service(NORDIC_UART_SERVICE).await,
        ) {
            (Ok(dfu), Ok(dis), Ok(uart)) => (None, vec![dfu, dis, uart]),
            _ => {
                let device = open_exact_paired_device().await?;
                let actual_address = device.BluetoothAddress()?;
                if actual_address != RING_BLUETOOTH_ADDRESS {
                    bail!(
                        "Windows returned wrong BLE address: {actual_address:012X}, expected {RING_BLUETOOTH_ADDRESS:012X}"
                    );
                }
                if !device.Name()?.to_string().eq_ignore_ascii_case(RING_NAME) {
                    bail!("Windows returned wrong BLE name: {}", device.Name()?);
                }
                let service_result = await_operation(
                    device
                        .GetGattServicesWithCacheModeAsync(BluetoothCacheMode::Uncached)
                        .context("request exact R08 GATT services")?,
                )
                .await
                .context("read exact R08 GATT services")?;
                require_success(
                    service_result.Status()?,
                    "enumerate exact R08 GATT services",
                )?;
                let services: Vec<_> = service_result.Services()?.into_iter().collect();
                (Some(device), services)
            }
        };
        let mut characteristics = Vec::new();
        for service in &services {
            let service_uuid = guid_to_uuid(service.Uuid()?);
            let result = await_operation(
                service
                    .GetCharacteristicsWithCacheModeAsync(BluetoothCacheMode::Uncached)
                    .with_context(|| format!("request characteristics for {service_uuid}"))?,
            )
            .await
            .with_context(|| format!("read characteristics for {service_uuid}"))?;
            if result.Status()? != GattCommunicationStatus::Success {
                continue;
            }
            characteristics.extend(
                result
                    .Characteristics()?
                    .into_iter()
                    .map(|item| Ok((guid_to_uuid(item.Uuid()?), item)))
                    .collect::<windows::core::Result<Vec<_>>>()?,
            );
        }
        if !services
            .iter()
            .any(|service| guid_to_uuid(service.Uuid().unwrap_or_default()) == DFU_SERVICE_UUID)
        {
            bail!("exact R08 does not expose the official QRing DFU service");
        }
        let find = |uuid| {
            characteristics
                .iter()
                .find(|(candidate, _)| *candidate == uuid)
                .map(|(_, item)| item.clone())
                .with_context(|| format!("exact R08 is missing characteristic {uuid}"))
        };
        let write = find(DFU_WRITE_UUID)?;
        let notify = find(DFU_NOTIFY_UUID)?;
        let uart_write = find(NORDIC_UART_WRITE)?;
        let uart_notify = find(NORDIC_UART_NOTIFY)?;
        let connection = Self {
            device,
            services,
            characteristics,
            write,
            notify,
            uart_write,
            uart_notify,
            notify_token: Mutex::new(None),
        };
        connection.require_text(HARDWARE_UUID, "RT08_V3.1").await?;
        connection
            .require_text(FIRMWARE_UUID, "RT08_3.10.48_260309")
            .await?;
        Ok(connection)
    }

    pub async fn battery_percent(&self) -> Result<u8> {
        if self
            .characteristics
            .iter()
            .any(|(candidate, _)| *candidate == BATTERY_UUID)
        {
            let bytes = self.read_characteristic(BATTERY_UUID).await?;
            if bytes.len() != 1 || bytes[0] > 100 {
                bail!("invalid battery response: {}", format_packet(&bytes));
            }
            return Ok(bytes[0]);
        }

        // R08 does not register the standard Battery Service in Windows. The
        // official app instead sends SimpleKeyReq(0x03) over its UART service.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handler = TypedEventHandler::new(
            move |_: &Option<GattCharacteristic>, args: &Option<GattValueChangedEventArgs>| {
                if let Some(args) = args {
                    let _ = tx.send(buffer_to_vec(&args.CharacteristicValue()?)?);
                }
                Ok(())
            },
        );
        let token = self.uart_notify.ValueChanged(&handler)?;
        let status = await_operation(
            self.uart_notify
                .WriteClientCharacteristicConfigurationDescriptorAsync(
                    GattClientCharacteristicConfigurationDescriptorValue::Notify,
                )?,
        )
        .await?;
        require_success(status, "enable UART battery notifications")?;

        let mut query = [0_u8; 16];
        query[0] = 0x03;
        query[15] = 0x03;
        let response = async {
            self.write_uart_packet(&query).await?;
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let packet = rx.recv().await.context("UART notification stream ended")?;
                    if packet.first().map(|value| value & 0x7f) == Some(0x03) {
                        return Ok::<_, anyhow::Error>(packet);
                    }
                }
            })
            .await
            .context("timeout waiting for official R08 battery response")?
        }
        .await;
        let _ = self.uart_notify.RemoveValueChanged(token);
        if let Ok(operation) = self
            .uart_notify
            .WriteClientCharacteristicConfigurationDescriptorAsync(
                GattClientCharacteristicConfigurationDescriptorValue::None,
            )
        {
            let _ = await_operation(operation).await;
        }
        let packet = response?;
        if packet.len() != 16
            || packet[..15]
                .iter()
                .fold(0_u8, |sum, value| sum.wrapping_add(*value))
                != packet[15]
            || packet[1] > 100
        {
            bail!(
                "invalid official UART battery response: {}",
                format_packet(&packet)
            );
        }
        Ok(packet[1])
    }

    pub async fn subscribe(&self) -> Result<Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let handler = TypedEventHandler::new(
            move |_: &Option<GattCharacteristic>, args: &Option<GattValueChangedEventArgs>| {
                if let Some(args) = args {
                    let bytes = buffer_to_vec(&args.CharacteristicValue()?)?;
                    let _ = tx.send(bytes);
                }
                Ok(())
            },
        );
        let token = self.notify.ValueChanged(&handler)?;
        let status = await_operation(
            self.notify
                .WriteClientCharacteristicConfigurationDescriptorAsync(
                    GattClientCharacteristicConfigurationDescriptorValue::Notify,
                )?,
        )
        .await?;
        if status != GattCommunicationStatus::Success {
            let _ = self.notify.RemoveValueChanged(token);
            bail!("enable DFU notifications failed: {status:?}");
        }
        *self.notify_token.lock().expect("DFU notify token mutex") = Some(token);
        Ok(Box::pin(UnboundedReceiverStream::new(rx)))
    }

    pub async fn write_frame(&self, frame: &[u8]) -> Result<()> {
        for chunk in frame.chunks(20) {
            let writer = DataWriter::new()?;
            writer.WriteBytes(chunk)?;
            let buffer = writer.DetachBuffer()?;
            let status = await_operation(
                self.write
                    .WriteValueWithOptionAsync(&buffer, GattWriteOption::WriteWithoutResponse)?,
            )
            .await?;
            require_success(status, "write QRing DFU chunk")?;
            tokio::time::sleep(std::time::Duration::from_millis(4)).await;
        }
        Ok(())
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

    async fn require_text(&self, uuid: Uuid, expected: &str) -> Result<()> {
        let bytes = self.read_characteristic(uuid).await?;
        let actual = String::from_utf8(bytes)?.trim_end_matches('\0').to_string();
        if actual != expected {
            bail!("identity mismatch for {uuid}: {actual:?}, expected {expected:?}");
        }
        Ok(())
    }

    async fn read_characteristic(&self, uuid: Uuid) -> Result<Vec<u8>> {
        let characteristic = self
            .characteristics
            .iter()
            .find(|(candidate, _)| *candidate == uuid)
            .map(|(_, item)| item)
            .with_context(|| format!("missing readable characteristic {uuid}"))?;
        let result = await_operation(
            characteristic.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)?,
        )
        .await?;
        require_success(result.Status()?, &format!("read characteristic {uuid}"))?;
        Ok(buffer_to_vec(&result.Value()?)?)
    }

    async fn write_uart_packet(&self, packet: &[u8]) -> Result<()> {
        let writer = DataWriter::new()?;
        writer.WriteBytes(packet)?;
        let buffer = writer.DetachBuffer()?;
        let status = await_operation(
            self.uart_write
                .WriteValueWithOptionAsync(&buffer, GattWriteOption::WriteWithResponse)?,
        )
        .await?;
        require_success(status, "write official R08 battery query")
    }

    fn remove_notification_handler(&self) {
        if let Some(token) = self
            .notify_token
            .lock()
            .expect("DFU notify token mutex")
            .take()
        {
            let _ = self.notify.RemoveValueChanged(token);
        }
    }
}

impl Drop for WindowsDfuConnection {
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

async fn open_exact_paired_device() -> Result<BluetoothLEDevice> {
    let selector = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)?;
    let devices = await_operation(DeviceInformation::FindAllAsyncAqsFilter(&selector)?).await?;
    for info in devices.into_iter() {
        let id = info.Id()?;
        let normalized_id = id.to_string().to_ascii_uppercase().replace([':', '-'], "");
        if !normalized_id.contains("313145379C07") {
            continue;
        }
        let device = await_operation(BluetoothLEDevice::FromIdAsync(&id)?).await?;
        if device.BluetoothAddress()? == RING_BLUETOOTH_ADDRESS {
            return Ok(device);
        }
        let _ = device.Close();
    }
    bail!("Windows paired BLE list does not contain exact {RING_NAME} / {RING_MAC}")
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
