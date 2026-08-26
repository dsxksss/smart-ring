use anyhow::Result;

use super::HidMonitor;
use crate::mapping::HidMouseEvent;

#[cfg(windows)]
pub fn spawn(_tx: std::sync::mpsc::Sender<HidMouseEvent>) -> Result<HidMonitor> {
    tracing::info!("HID_DEVICE Windows 无光标模式不读取戒指鼠标位移；上下滑与点击仅使用 GATT 0x1D");
    Ok(HidMonitor::new(None, None))
}

#[cfg(target_os = "linux")]
pub fn spawn(tx: std::sync::mpsc::Sender<HidMouseEvent>) -> Result<HidMonitor> {
    crate::platform::linux::spawn_hid(tx)
}

#[cfg(target_os = "macos")]
pub fn spawn(_tx: std::sync::mpsc::Sender<HidMouseEvent>) -> Result<HidMonitor> {
    tracing::info!("HID_DEVICE macOS 系统 HID 会占用 BLE 鼠标；精细相对 Y 不可用，改用 GATT 0x1D");
    Ok(HidMonitor::new(None, None))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn spawn(_tx: std::sync::mpsc::Sender<HidMouseEvent>) -> Result<HidMonitor> {
    Ok(HidMonitor::new(None, None))
}
