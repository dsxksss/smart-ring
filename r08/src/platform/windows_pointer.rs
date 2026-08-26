use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Disable_DevNode, CM_Enable_DevNode, CM_Get_DevNode_Status, CM_Get_Device_ID_ListW,
    CM_Get_Device_ID_List_SizeW, CM_Locate_DevNodeW, CM_GETIDLIST_FILTER_ENUMERATOR,
    CM_GETIDLIST_FILTER_PRESENT, CM_LOCATE_DEVNODE_NORMAL, CR_NEED_RESTART, CR_SUCCESS, DN_STARTED,
};

const RING_HID_MOUSE_MARKER: &str = "{00001812-0000-1000-8000-00805F9B34FB}_313145379C07&COL01";

pub struct RingMouseDeviceGuard {
    disabled_by_us: Option<String>,
}

impl RingMouseDeviceGuard {
    pub fn new() -> Self {
        Self {
            disabled_by_us: None,
        }
    }

    pub fn suppress(&mut self) -> Result<()> {
        if self.disabled_by_us.is_some() {
            return Ok(());
        }
        let instance_id = find_ring_mouse_instance()?
            .context("没有找到 R08_9C07 的 HID 鼠标子设备；为避免光标移动，已拒绝开启触控")?;
        let devinst = locate_devnode(&instance_id)?;
        if !devnode_started(devinst)? {
            tracing::info!("R08_POINTER_BLOCK HID 鼠标子设备已经停用，不会移动光标");
            return Ok(());
        }

        let result = unsafe { CM_Disable_DevNode(devinst, 0) };
        if result == CR_NEED_RESTART {
            bail!("停用 R08 HID 鼠标需要重启，已拒绝开启触控");
        }
        require_config_success(result, "停用 R08 HID 鼠标子设备")?;
        thread::sleep(Duration::from_millis(120));
        if devnode_started(devinst)? {
            bail!("Windows 未真正停用 R08 HID 鼠标子设备，已拒绝开启触控");
        }
        self.disabled_by_us = Some(instance_id);
        tracing::info!(
            "R08_POINTER_BLOCK 已停用戒指 HID 鼠标子设备；普通鼠标不受影响，手势仅走 GATT"
        );
        Ok(())
    }

    pub fn restore(&mut self) -> Result<()> {
        let Some(instance_id) = self.disabled_by_us.take() else {
            return Ok(());
        };
        let devinst = locate_devnode(&instance_id)?;
        let result = unsafe { CM_Enable_DevNode(devinst, 0) };
        if let Err(error) = require_config_success(result, "恢复 R08 HID 鼠标子设备") {
            self.disabled_by_us = Some(instance_id);
            return Err(error);
        }
        thread::sleep(Duration::from_millis(120));
        if !devnode_started(devinst)? {
            self.disabled_by_us = Some(instance_id);
            bail!("Windows 没有恢复 R08 HID 鼠标子设备");
        }
        tracing::info!("R08_POINTER_RESTORE 已恢复戒指 HID 鼠标子设备");
        Ok(())
    }
}

impl Default for RingMouseDeviceGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RingMouseDeviceGuard {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            tracing::warn!("退出时恢复 R08 HID 鼠标子设备失败：{error:#}");
        }
    }
}

fn find_ring_mouse_instance() -> Result<Option<String>> {
    let filter: Vec<u16> = "HID\0".encode_utf16().collect();
    let flags = CM_GETIDLIST_FILTER_ENUMERATOR | CM_GETIDLIST_FILTER_PRESENT;
    let mut length = 0u32;
    let result =
        unsafe { CM_Get_Device_ID_List_SizeW(&mut length, PCWSTR(filter.as_ptr()), flags) };
    require_config_success(result, "读取当前 HID 设备列表长度")?;
    if length < 2 {
        return Ok(None);
    }
    let mut buffer = vec![0u16; length as usize];
    let result = unsafe { CM_Get_Device_ID_ListW(PCWSTR(filter.as_ptr()), &mut buffer, flags) };
    require_config_success(result, "读取当前 HID 设备列表")?;
    Ok(multi_sz(&buffer)
        .into_iter()
        .find(|instance_id| is_ring_mouse_instance(instance_id)))
}

fn locate_devnode(instance_id: &str) -> Result<u32> {
    let wide: Vec<u16> = instance_id
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut devinst = 0u32;
    let result = unsafe {
        CM_Locate_DevNodeW(
            &mut devinst,
            PCWSTR(wide.as_ptr()),
            CM_LOCATE_DEVNODE_NORMAL,
        )
    };
    require_config_success(result, "定位 R08 HID 鼠标子设备")?;
    Ok(devinst)
}

fn devnode_started(devinst: u32) -> Result<bool> {
    let mut status = Default::default();
    let mut problem = Default::default();
    let result = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, devinst, 0) };
    require_config_success(result, "读取 R08 HID 鼠标子设备状态")?;
    Ok(status.0 & DN_STARTED.0 != 0)
}

fn require_config_success(
    result: windows::Win32::Devices::DeviceAndDriverInstallation::CONFIGRET,
    action: &str,
) -> Result<()> {
    if result == CR_SUCCESS {
        Ok(())
    } else {
        bail!(
            "{action}失败：CONFIGRET {}。请以管理员身份运行终端",
            result.0
        )
    }
}

fn multi_sz(buffer: &[u16]) -> Vec<String> {
    let mut values = Vec::new();
    let mut start = 0usize;
    while start < buffer.len() && buffer[start] != 0 {
        let end = buffer[start..]
            .iter()
            .position(|value| *value == 0)
            .map(|offset| start + offset)
            .unwrap_or(buffer.len());
        values.push(String::from_utf16_lossy(&buffer[start..end]));
        start = end.saturating_add(1);
    }
    values
}

fn is_ring_mouse_instance(instance_id: &str) -> bool {
    let upper = instance_id.to_ascii_uppercase();
    upper.starts_with("HID\\") && upper.contains(RING_HID_MOUSE_MARKER)
}

#[cfg(test)]
mod tests {
    use super::{is_ring_mouse_instance, multi_sz};

    #[test]
    fn matches_only_the_r08_mouse_collection() {
        assert!(is_ring_mouse_instance(
            "HID\\{00001812-0000-1000-8000-00805F9B34FB}_313145379C07&COL01\\9&1F7B755D&0&0000"
        ));
        assert!(!is_ring_mouse_instance(
            "HID\\{00001812-0000-1000-8000-00805F9B34FB}_313145379C07&COL02\\9&1F7B755D&0&0001"
        ));
        assert!(!is_ring_mouse_instance(
            "HID\\VID_046D&PID_C548&MI_00\\8&ORDINARY&MOUSE"
        ));
    }

    #[test]
    fn parses_windows_multi_sz() {
        let data: Vec<u16> = "one\0two\0\0".encode_utf16().collect();
        assert_eq!(multi_sz(&data), vec!["one", "two"]);
    }
}
