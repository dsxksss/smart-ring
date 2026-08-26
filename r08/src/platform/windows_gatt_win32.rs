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
    BluetoothGATTGetCharacteristics, BluetoothGATTGetDescriptors, BluetoothGATTGetServices,
    BluetoothGATTRegisterEvent, BluetoothGATTSetCharacteristicValue,
    BluetoothGATTSetDescriptorValue, BluetoothGATTUnregisterEvent, CharacteristicValueChangedEvent,
    ClientCharacteristicConfiguration, BLUETOOTH_GATT_FLAG_NONE,
    BLUETOOTH_GATT_VALUE_CHANGED_EVENT, BLUETOOTH_GATT_VALUE_CHANGED_EVENT_REGISTRATION,
    BTH_LE_GATT_CHARACTERISTIC, BTH_LE_GATT_CHARACTERISTIC_VALUE, BTH_LE_GATT_DESCRIPTOR,
    BTH_LE_GATT_DESCRIPTOR_VALUE, BTH_LE_GATT_DESCRIPTOR_VALUE_0_2, BTH_LE_GATT_EVENT_TYPE,
    BTH_LE_GATT_SERVICE, BTH_LE_UUID, GUID_BLUETOOTHLE_DEVICE_INTERFACE,
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
        Ok(Vec::new())
    }

    pub async fn disconnect(&self) -> Result<()> {
        self.stop_notifications();
        let _ = set_notify_descriptor(self.handle, &self.notify_descriptor, false);
        Ok(())
    }

    fn stop_notifications(&self) {
        if let Some(event_handle) = self.event_handle.lock().expect("event handle mutex").take() {
            let _ = unsafe { BluetoothGATTUnregisterEvent(event_handle, 0) };
        }
        if let Some(context) = self
            .callback_context
            .lock()
            .expect("callback context mutex")
            .take()
        {
            unsafe { drop(Box::from_raw(context as *mut CallbackContext)) };
        }
    }
}

impl Drop for WindowsGattWin32Connection {
    fn drop(&mut self) {
        self.stop_notifications();
        let _ = set_notify_descriptor(self.handle, &self.notify_descriptor, false);
        let _ = unsafe { CloseHandle(self.handle) };
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
