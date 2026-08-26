use std::ffi::c_void;
use std::pin::Pin;
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use futures::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;
use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::Bluetooth::{
    BluetoothGATTGetCharacteristicValue, BluetoothGATTGetCharacteristics,
    BluetoothGATTGetDescriptors, BluetoothGATTGetServices, BluetoothGATTRegisterEvent,
    BluetoothGATTSetCharacteristicValue, BluetoothGATTSetDescriptorValue,
    BluetoothGATTUnregisterEvent, CharacteristicValueChangedEvent,
    ClientCharacteristicConfiguration, BLUETOOTH_GATT_FLAG_NONE,
    BLUETOOTH_GATT_FLAG_WRITE_WITHOUT_RESPONSE, BLUETOOTH_GATT_VALUE_CHANGED_EVENT,
    BLUETOOTH_GATT_VALUE_CHANGED_EVENT_REGISTRATION, BTH_LE_GATT_CHARACTERISTIC,
    BTH_LE_GATT_CHARACTERISTIC_VALUE, BTH_LE_GATT_DESCRIPTOR, BTH_LE_GATT_DESCRIPTOR_VALUE,
    BTH_LE_GATT_DESCRIPTOR_VALUE_0_2, BTH_LE_GATT_EVENT_TYPE, BTH_LE_GATT_SERVICE, BTH_LE_UUID,
    GUID_BLUETOOTHLE_DEVICE_INTERFACE,
};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW,
    CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CR_SUCCESS,
};
use windows::Win32::Foundation::{CloseHandle, BOOLEAN, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};

use crate::identity::{RING_MAC, RING_NAME};
use crate::protocol::{format_packet, NORDIC_UART_NOTIFY, NORDIC_UART_SERVICE, NORDIC_UART_WRITE};

const DFU_SERVICE_UUID: Uuid = Uuid::from_u128(0xde5bf728_d711_4e47_af26_65e3012a5dc7);
const DFU_NOTIFY_UUID: Uuid = Uuid::from_u128(0xde5bf729_d711_4e47_af26_65e3012a5dc7);
const DFU_WRITE_UUID: Uuid = Uuid::from_u128(0xde5bf72a_d711_4e47_af26_65e3012a5dc7);
const DIS_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000180a_0000_1000_8000_00805f9b34fb);
const FIRMWARE_UUID: Uuid = Uuid::from_u128(0x00002a26_0000_1000_8000_00805f9b34fb);
const HARDWARE_UUID: Uuid = Uuid::from_u128(0x00002a27_0000_1000_8000_00805f9b34fb);
const SOFTWARE_UUID: Uuid = Uuid::from_u128(0x00002a28_0000_1000_8000_00805f9b34fb);

pub struct WindowsGattWin32Connection {
    handle: HANDLE,
    write: BTH_LE_GATT_CHARACTERISTIC,
    notify: BTH_LE_GATT_CHARACTERISTIC,
    notify_descriptor: BTH_LE_GATT_DESCRIPTOR,
    event_handle: Mutex<Option<isize>>,
    callback_context: Mutex<Option<usize>>,
}

struct CallbackContext {
    tx: mpsc::UnboundedSender<Vec<u8>>,
}

#[repr(C)]
struct CharacteristicValueBuffer {
    data_size: u32,
    data: [u8; 64],
}

impl WindowsGattWin32Connection {
    pub async fn connect_known() -> Result<Self> {
        let device_path = ring_interface_path(&GUID_BLUETOOTHLE_DEVICE_INTERFACE)?;
        let device_handle = open_interface(&device_path)?;

        let result: Result<Self> = (|| {
            let services = gatt_services(device_handle)?;
            let uart = services
                .iter()
                .find(|service| bth_uuid(&service.ServiceUuid) == NORDIC_UART_SERVICE)
                .copied()
                .context("Windows BLE 设备接口缺少 Nordic UART 服务")?;
            let characteristics = gatt_characteristics(device_handle, &uart)?;
            let write = characteristics
                .iter()
                .find(|item| bth_uuid(&item.CharacteristicUuid) == NORDIC_UART_WRITE)
                .copied()
                .context("Windows BLE 设备接口缺少 UART 写入特征")?;
            let notify = characteristics
                .iter()
                .find(|item| bth_uuid(&item.CharacteristicUuid) == NORDIC_UART_NOTIFY)
                .copied()
                .context("Windows BLE 设备接口缺少 UART 通知特征")?;
            let notify_descriptor = gatt_descriptors(device_handle, &notify)?
                .into_iter()
                .find(|item| item.DescriptorType == ClientCharacteristicConfiguration)
                .context("Windows BLE 设备接口缺少通知配置描述符")?;
            let service_path =
                ring_interface_path(&GUID::from_u128(NORDIC_UART_SERVICE.as_u128()))?;
            let service_handle = open_interface(&service_path)?;
            Ok(Self {
                handle: service_handle,
                write,
                notify,
                notify_descriptor,
                event_handle: Mutex::new(None),
                callback_context: Mutex::new(None),
            })
        })();
        let _ = unsafe { CloseHandle(device_handle) };
        let connection = result?;
        tracing::info!("已通过 Windows Win32 GATT 复用系统连接 {RING_NAME} ({RING_MAC})");
        Ok(connection)
    }

    pub async fn subscribe(&self) -> Result<Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>> {
        self.stop_notifications();
        set_notify_descriptor(self.handle, &self.notify_descriptor, true)?;

        let (tx, rx) = mpsc::unbounded_channel();
        let context = Box::into_raw(Box::new(CallbackContext { tx })) as usize;
        let registration = BLUETOOTH_GATT_VALUE_CHANGED_EVENT_REGISTRATION {
            NumCharacteristics: 1,
            Characteristics: [self.notify],
        };
        let mut event_handle = 0isize;
        let register_result = unsafe {
            BluetoothGATTRegisterEvent(
                self.handle,
                CharacteristicValueChangedEvent,
                (&registration as *const BLUETOOTH_GATT_VALUE_CHANGED_EVENT_REGISTRATION).cast(),
                Some(gatt_event_callback),
                Some(context as *const c_void),
                &mut event_handle,
                BLUETOOTH_GATT_FLAG_NONE,
            )
        };
        if let Err(error) = register_result {
            unsafe { drop(Box::from_raw(context as *mut CallbackContext)) };
            let _ = set_notify_descriptor(self.handle, &self.notify_descriptor, false);
            return Err(error).context("注册 Windows Win32 GATT 通知失败");
        }
        *self.event_handle.lock().expect("event handle mutex") = Some(event_handle);
        *self
            .callback_context
            .lock()
            .expect("callback context mutex") = Some(context);
        Ok(Box::pin(UnboundedReceiverStream::new(rx)))
    }

    pub async fn write(&self, packet: &[u8]) -> Result<()> {
        if packet.len() > 64 {
            bail!("GATT 数据包过长：{} 字节", packet.len());
        }
        tracing::info!("TX  {}", format_packet(packet));
        let mut value = CharacteristicValueBuffer {
            data_size: packet.len() as u32,
            data: [0u8; 64],
        };
        value.data[..packet.len()].copy_from_slice(packet);
        unsafe {
            BluetoothGATTSetCharacteristicValue(
                self.handle,
                &self.write,
                (&value as *const CharacteristicValueBuffer)
                    .cast::<BTH_LE_GATT_CHARACTERISTIC_VALUE>(),
                0,
                BLUETOOTH_GATT_FLAG_NONE,
            )
        }
        .context("Windows Win32 GATT 写入失败")
    }

    pub async fn read_device_information(&self) -> Result<Vec<(String, String)>> {
        let device_path = ring_interface_path(&GUID_BLUETOOTHLE_DEVICE_INTERFACE)?;
        let device_handle = open_interface(&device_path)?;
        let result = (|| {
            let services = gatt_services(device_handle)?;
            let dis = service_by_uuid(&services, DIS_SERVICE_UUID)?;
            let characteristics = gatt_characteristics(device_handle, &dis)?;
            let hardware = characteristic_by_uuid(&characteristics, HARDWARE_UUID)?;
            let firmware = characteristic_by_uuid(&characteristics, FIRMWARE_UUID)?;
            let dis_path = ring_interface_path(&GUID::from_u128(DIS_SERVICE_UUID.as_u128()))?;
            let dis_handle = open_interface(&dis_path)?;
            let values = (|| {
                let mut rows = vec![
                    (
                        "Hardware Revision".to_string(),
                        read_characteristic_text(dis_handle, &hardware)?,
                    ),
                    (
                        "Firmware Revision".to_string(),
                        read_characteristic_text(dis_handle, &firmware)?,
                    ),
                ];
                if let Ok(software) = characteristic_by_uuid(&characteristics, SOFTWARE_UUID) {
                    rows.push((
                        "Software Revision".to_string(),
                        read_characteristic_text(dis_handle, &software)?,
                    ));
                }
                Ok::<_, anyhow::Error>(rows)
            })();
            let _ = unsafe { CloseHandle(dis_handle) };
            values
        })();
        let _ = unsafe { CloseHandle(device_handle) };
        result
    }

    pub async fn disconnect(&self) -> Result<()> {
        self.stop_notifications();
        // The R08 is owned by the Windows BLE stack rather than by an
        // application-level connection. Disabling its cached CCCD here can
        // block indefinitely after A1 09 00 while the driver tears down the
        // stream. Unregistering our event is sufficient; the caller has
        // already sent the firmware stop command, and v7 also has a 12-second
        // hard timeout if the process crashes before cleanup.
        Ok(())
    }

    fn stop_notifications(&self) {
        let event_handle = self.event_handle.lock().expect("event handle mutex").take();
        let context = self
            .callback_context
            .lock()
            .expect("callback context mutex")
            .take();
        if event_handle.is_none() && context.is_none() {
            return;
        }
        // Some Windows Bluetooth drivers block indefinitely inside
        // BluetoothGATTUnregisterEvent after the R08 stream is stopped. Keep
        // the callback context alive until unregister returns, but do that on
        // a detached cleanup thread so the control process can finish. If the
        // driver never returns, process teardown safely reclaims both handles.
        std::thread::spawn(move || {
            if let Some(event_handle) = event_handle {
                let _ = unsafe { BluetoothGATTUnregisterEvent(event_handle, 0) };
            }
            if let Some(context) = context {
                unsafe { drop(Box::from_raw(context as *mut CallbackContext)) };
            }
        });
    }
}

impl Drop for WindowsGattWin32Connection {
    fn drop(&mut self) {
        self.stop_notifications();
        let raw_handle = self.handle.0 as usize;
        std::thread::spawn(move || {
            let handle = HANDLE(raw_handle as *mut c_void);
            let _ = unsafe { CloseHandle(handle) };
        });
    }
}

/// Low-level Win32 transport for the one hash-locked sacrificial DFU command.
/// Keeping this separate prevents ordinary commands from ever reaching DFU.
pub struct WindowsDfuWin32Connection {
    dfu_handle: HANDLE,
    write: BTH_LE_GATT_CHARACTERISTIC,
    notify: BTH_LE_GATT_CHARACTERISTIC,
    notify_descriptor: BTH_LE_GATT_DESCRIPTOR,
    event_handle: Mutex<Option<isize>>,
    callback_context: Mutex<Option<usize>>,
    hardware_text: String,
    firmware_text: String,
}

impl WindowsDfuWin32Connection {
    pub async fn connect_exact() -> Result<Self> {
        let device_path = ring_interface_path(&GUID_BLUETOOTHLE_DEVICE_INTERFACE)?;
        let device_handle = open_interface(&device_path)?;
        let result = (|| {
            let services = gatt_services(device_handle)?;
            let dfu = service_by_uuid(&services, DFU_SERVICE_UUID)?;
            let dis = service_by_uuid(&services, DIS_SERVICE_UUID)?;
            let dfu_characteristics = gatt_characteristics(device_handle, &dfu)?;
            let dis_characteristics = gatt_characteristics(device_handle, &dis)?;

            let hardware = characteristic_by_uuid(&dis_characteristics, HARDWARE_UUID)?;
            let firmware = characteristic_by_uuid(&dis_characteristics, FIRMWARE_UUID)?;
            let dis_path = ring_interface_path(&GUID::from_u128(DIS_SERVICE_UUID.as_u128()))?;
            let dis_handle = open_interface(&dis_path)?;
            let identity_result = (|| {
                Ok::<_, anyhow::Error>((
                    read_characteristic_text(dis_handle, &hardware)?,
                    read_characteristic_text(dis_handle, &firmware)?,
                ))
            })();
            let _ = unsafe { CloseHandle(dis_handle) };
            let (hardware_text, firmware_text) = identity_result?;
            if hardware_text != "RT08_V3.1" {
                bail!("hardware identity mismatch: {hardware_text:?}");
            }
            if firmware_text != "RT08_3.10.48_260309"
                && firmware_text != "RT08_3.10.49_260827"
                && firmware_text != "RT08_3.10.50_260827"
                && firmware_text != "RT08_3.10.51_260827"
            {
                bail!("firmware identity mismatch: {firmware_text:?}");
            }

            let write = characteristic_by_uuid(&dfu_characteristics, DFU_WRITE_UUID)?;
            let notify = characteristic_by_uuid(&dfu_characteristics, DFU_NOTIFY_UUID)?;
            let notify_descriptor = gatt_descriptors(device_handle, &notify)?
                .into_iter()
                .find(|item| item.DescriptorType == ClientCharacteristicConfiguration)
                .context("official DFU notify characteristic lacks CCCD")?;
            let dfu_path = ring_interface_path(&GUID::from_u128(DFU_SERVICE_UUID.as_u128()))?;
            let dfu_handle = open_interface(&dfu_path)?;
            Ok(Self {
                dfu_handle,
                write,
                notify,
                notify_descriptor,
                event_handle: Mutex::new(None),
                callback_context: Mutex::new(None),
                hardware_text,
                firmware_text,
            })
        })();
        let _ = unsafe { CloseHandle(device_handle) };
        result
    }

    pub fn hardware_text(&self) -> &str {
        &self.hardware_text
    }

    pub fn firmware_text(&self) -> &str {
        &self.firmware_text
    }

    pub async fn battery_percent(&self) -> Result<u8> {
        let uart = WindowsGattWin32Connection::connect_known().await?;
        let mut notifications = uart.subscribe().await?;
        let mut query = [0_u8; 16];
        query[0] = 0x03;
        query[15] = 0x03;
        uart.write(&query).await?;
        let packet = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let packet = futures::StreamExt::next(&mut notifications)
                    .await
                    .context("UART battery notification stream ended")?;
                if packet.first().map(|value| value & 0x7f) == Some(0x03) {
                    return Ok::<_, anyhow::Error>(packet);
                }
            }
        })
        .await
        .context("timeout waiting for official R08 battery response")??;
        uart.disconnect().await?;
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
        self.stop_notifications();
        set_notify_descriptor(self.dfu_handle, &self.notify_descriptor, true)?;
        let (tx, rx) = mpsc::unbounded_channel();
        let context = Box::into_raw(Box::new(CallbackContext { tx })) as usize;
        let registration = BLUETOOTH_GATT_VALUE_CHANGED_EVENT_REGISTRATION {
            NumCharacteristics: 1,
            Characteristics: [self.notify],
        };
        let mut event_handle = 0isize;
        let registered = unsafe {
            BluetoothGATTRegisterEvent(
                self.dfu_handle,
                CharacteristicValueChangedEvent,
                (&registration as *const BLUETOOTH_GATT_VALUE_CHANGED_EVENT_REGISTRATION).cast(),
                Some(gatt_event_callback),
                Some(context as *const c_void),
                &mut event_handle,
                BLUETOOTH_GATT_FLAG_NONE,
            )
        };
        if let Err(error) = registered {
            unsafe { drop(Box::from_raw(context as *mut CallbackContext)) };
            let _ = set_notify_descriptor(self.dfu_handle, &self.notify_descriptor, false);
            return Err(error).context("register official DFU notifications");
        }
        *self.event_handle.lock().expect("DFU event handle mutex") = Some(event_handle);
        *self
            .callback_context
            .lock()
            .expect("DFU callback context mutex") = Some(context);
        Ok(Box::pin(UnboundedReceiverStream::new(rx)))
    }

    pub async fn write_frame(&self, frame: &[u8]) -> Result<()> {
        for chunk in frame.chunks(20) {
            set_characteristic_value(
                self.dfu_handle,
                &self.write,
                chunk,
                BLUETOOTH_GATT_FLAG_WRITE_WITHOUT_RESPONSE,
            )?;
            tokio::time::sleep(std::time::Duration::from_millis(4)).await;
        }
        Ok(())
    }

    pub async fn disconnect(&self) -> Result<()> {
        self.stop_notifications();
        let _ = set_notify_descriptor(self.dfu_handle, &self.notify_descriptor, false);
        Ok(())
    }

    fn stop_notifications(&self) {
        if let Some(event_handle) = self.event_handle.lock().expect("DFU event mutex").take() {
            let _ = unsafe { BluetoothGATTUnregisterEvent(event_handle, 0) };
        }
        if let Some(context) = self
            .callback_context
            .lock()
            .expect("DFU callback mutex")
            .take()
        {
            unsafe { drop(Box::from_raw(context as *mut CallbackContext)) };
        }
    }
}

impl Drop for WindowsDfuWin32Connection {
    fn drop(&mut self) {
        self.stop_notifications();
        let _ = set_notify_descriptor(self.dfu_handle, &self.notify_descriptor, false);
        let _ = unsafe { CloseHandle(self.dfu_handle) };
    }
}

unsafe extern "system" fn gatt_event_callback(
    event_type: BTH_LE_GATT_EVENT_TYPE,
    event_parameter: *const c_void,
    context: *const c_void,
) {
    if event_type != CharacteristicValueChangedEvent
        || event_parameter.is_null()
        || context.is_null()
    {
        return;
    }
    let event = unsafe { &*(event_parameter.cast::<BLUETOOTH_GATT_VALUE_CHANGED_EVENT>()) };
    if event.CharacteristicValue.is_null() {
        return;
    }
    let value = unsafe { &*event.CharacteristicValue };
    let bytes = unsafe {
        std::slice::from_raw_parts(value.Data.as_ptr(), value.DataSize as usize).to_vec()
    };
    let callback = unsafe { &*(context.cast::<CallbackContext>()) };
    let _ = callback.tx.send(bytes);
}

fn ring_interface_path(interface_guid: &GUID) -> Result<Vec<u16>> {
    let mut length = 0u32;
    let status = unsafe {
        CM_Get_Device_Interface_List_SizeW(
            &mut length,
            interface_guid,
            PCWSTR::null(),
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        )
    };
    if status != CR_SUCCESS || length < 2 {
        bail!("枚举 Windows BLE 设备接口长度失败：{status:?}");
    }
    let mut buffer = vec![0u16; length as usize];
    let status = unsafe {
        CM_Get_Device_Interface_ListW(
            interface_guid,
            PCWSTR::null(),
            &mut buffer,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        )
    };
    if status != CR_SUCCESS {
        bail!("枚举 Windows BLE 设备接口失败：{status:?}");
    }
    for path in multi_sz(&buffer) {
        let text = String::from_utf16_lossy(&path).to_ascii_uppercase();
        if text.replace([':', '-'], "").contains("313145379C07") {
            return Ok(path);
        }
    }
    bail!("Windows 当前 BLE 设备接口列表没有 {RING_NAME}")
}

fn open_interface(path: &[u16]) -> Result<HANDLE> {
    unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .context("打开 Windows R08 BLE/GATT 设备接口失败")
}

fn multi_sz(buffer: &[u16]) -> Vec<Vec<u16>> {
    let mut values = Vec::new();
    let mut start = 0usize;
    while start < buffer.len() && buffer[start] != 0 {
        let end = buffer[start..]
            .iter()
            .position(|value| *value == 0)
            .map(|offset| start + offset)
            .unwrap_or(buffer.len());
        let mut value = buffer[start..end].to_vec();
        value.push(0);
        values.push(value);
        start = end.saturating_add(1);
    }
    values
}

fn gatt_services(handle: HANDLE) -> Result<Vec<BTH_LE_GATT_SERVICE>> {
    let mut count = 0u16;
    let _ = unsafe { BluetoothGATTGetServices(handle, None, &mut count, BLUETOOTH_GATT_FLAG_NONE) };
    if count == 0 {
        bail!("Windows Win32 GATT 没有返回服务");
    }
    let mut values = vec![BTH_LE_GATT_SERVICE::default(); count as usize];
    unsafe {
        BluetoothGATTGetServices(
            handle,
            Some(&mut values),
            &mut count,
            BLUETOOTH_GATT_FLAG_NONE,
        )
    }
    .context("读取 Windows Win32 GATT 服务失败")?;
    values.truncate(count as usize);
    Ok(values)
}

fn service_by_uuid(services: &[BTH_LE_GATT_SERVICE], uuid: Uuid) -> Result<BTH_LE_GATT_SERVICE> {
    services
        .iter()
        .find(|service| bth_uuid(&service.ServiceUuid) == uuid)
        .copied()
        .with_context(|| format!("exact R08 lacks service {uuid}"))
}

fn characteristic_by_uuid(
    characteristics: &[BTH_LE_GATT_CHARACTERISTIC],
    uuid: Uuid,
) -> Result<BTH_LE_GATT_CHARACTERISTIC> {
    characteristics
        .iter()
        .find(|item| bth_uuid(&item.CharacteristicUuid) == uuid)
        .copied()
        .with_context(|| format!("exact R08 lacks characteristic {uuid}"))
}

fn read_characteristic_text(
    handle: HANDLE,
    characteristic: &BTH_LE_GATT_CHARACTERISTIC,
) -> Result<String> {
    let mut required = 0_u16;
    let mut value = CharacteristicValueBuffer {
        data_size: 0,
        data: [0; 64],
    };
    unsafe {
        BluetoothGATTGetCharacteristicValue(
            handle,
            characteristic,
            std::mem::size_of::<CharacteristicValueBuffer>() as u32,
            Some(
                (&mut value as *mut CharacteristicValueBuffer)
                    .cast::<BTH_LE_GATT_CHARACTERISTIC_VALUE>(),
            ),
            Some(&mut required),
            BLUETOOTH_GATT_FLAG_NONE,
        )
    }
    .context("read exact R08 identity characteristic")?;
    let length = value.data_size as usize;
    if length > value.data.len() {
        bail!("identity characteristic exceeds local read buffer");
    }
    Ok(std::str::from_utf8(&value.data[..length])?
        .trim_end_matches('\0')
        .to_string())
}

fn set_characteristic_value(
    handle: HANDLE,
    characteristic: &BTH_LE_GATT_CHARACTERISTIC,
    bytes: &[u8],
    flags: u32,
) -> Result<()> {
    if bytes.len() > 64 {
        bail!("GATT chunk exceeds 64 bytes");
    }
    let mut value = CharacteristicValueBuffer {
        data_size: bytes.len() as u32,
        data: [0; 64],
    };
    value.data[..bytes.len()].copy_from_slice(bytes);
    unsafe {
        BluetoothGATTSetCharacteristicValue(
            handle,
            characteristic,
            (&value as *const CharacteristicValueBuffer).cast::<BTH_LE_GATT_CHARACTERISTIC_VALUE>(),
            0,
            flags,
        )
    }
    .context("write exact R08 GATT characteristic")
}

fn gatt_characteristics(
    handle: HANDLE,
    service: &BTH_LE_GATT_SERVICE,
) -> Result<Vec<BTH_LE_GATT_CHARACTERISTIC>> {
    let mut count = 0u16;
    let _ = unsafe {
        BluetoothGATTGetCharacteristics(
            handle,
            Some(service),
            None,
            &mut count,
            BLUETOOTH_GATT_FLAG_NONE,
        )
    };
    if count == 0 {
        bail!("Windows Win32 GATT UART 服务没有特征");
    }
    let mut values = vec![BTH_LE_GATT_CHARACTERISTIC::default(); count as usize];
    unsafe {
        BluetoothGATTGetCharacteristics(
            handle,
            Some(service),
            Some(&mut values),
            &mut count,
            BLUETOOTH_GATT_FLAG_NONE,
        )
    }
    .context("读取 Windows Win32 GATT 特征失败")?;
    values.truncate(count as usize);
    Ok(values)
}

fn gatt_descriptors(
    handle: HANDLE,
    characteristic: &BTH_LE_GATT_CHARACTERISTIC,
) -> Result<Vec<BTH_LE_GATT_DESCRIPTOR>> {
    let mut count = 0u16;
    let _ = unsafe {
        BluetoothGATTGetDescriptors(
            handle,
            characteristic,
            None,
            &mut count,
            BLUETOOTH_GATT_FLAG_NONE,
        )
    };
    if count == 0 {
        bail!("Windows Win32 GATT 通知特征没有描述符");
    }
    let mut values = vec![BTH_LE_GATT_DESCRIPTOR::default(); count as usize];
    unsafe {
        BluetoothGATTGetDescriptors(
            handle,
            characteristic,
            Some(&mut values),
            &mut count,
            BLUETOOTH_GATT_FLAG_NONE,
        )
    }
    .context("读取 Windows Win32 GATT 描述符失败")?;
    values.truncate(count as usize);
    Ok(values)
}

fn set_notify_descriptor(
    handle: HANDLE,
    descriptor: &BTH_LE_GATT_DESCRIPTOR,
    enabled: bool,
) -> Result<()> {
    let mut value = BTH_LE_GATT_DESCRIPTOR_VALUE {
        DescriptorType: ClientCharacteristicConfiguration,
        ..Default::default()
    };
    value.Anonymous.ClientCharacteristicConfiguration = BTH_LE_GATT_DESCRIPTOR_VALUE_0_2 {
        IsSubscribeToNotification: BOOLEAN(enabled as u8),
        IsSubscribeToIndication: BOOLEAN(0),
    };
    unsafe { BluetoothGATTSetDescriptorValue(handle, descriptor, &value, BLUETOOTH_GATT_FLAG_NONE) }
        .context("配置 Windows Win32 GATT 通知失败")
}

fn bth_uuid(uuid: &BTH_LE_UUID) -> Uuid {
    unsafe {
        if uuid.IsShortUuid.0 != 0 {
            Uuid::from_u128(
                ((uuid.Value.ShortUuid as u128) << 96) | 0x0000_0000_0000_1000_8000_0080_5F9B_34FB,
            )
        } else {
            let guid = uuid.Value.LongUuid;
            Uuid::from_fields(guid.data1, guid.data2, guid.data3, &guid.data4)
        }
    }
}
