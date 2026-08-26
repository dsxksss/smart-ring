pub mod capabilities;
pub mod hid;
pub mod inject;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub mod windows_gatt;
#[cfg(windows)]
pub mod windows_gatt_win32;
#[cfg(windows)]
pub mod windows_pointer;

use crate::mapping::HidMouseEvent;

#[derive(Debug, Clone)]
pub struct PlatformCapabilities {
    pub os: &'static str,
    pub ble_backend: &'static str,
    pub hid_backend: &'static str,
    pub inject_backend: &'static str,
    pub notes: Vec<String>,
}

pub fn capabilities() -> PlatformCapabilities {
    capabilities::detect()
}

pub fn spawn_hid_monitor(tx: std::sync::mpsc::Sender<HidMouseEvent>) -> anyhow::Result<HidMonitor> {
    hid::spawn(tx)
}

pub fn create_injector() -> anyhow::Result<Box<dyn inject::Injector>> {
    inject::create()
}

pub struct PointerSuppression {
    #[cfg(windows)]
    inner: windows_pointer::RingMouseDeviceGuard,
}

impl PointerSuppression {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            inner: windows_pointer::RingMouseDeviceGuard::new(),
        }
    }

    pub fn suppress(&mut self) -> anyhow::Result<()> {
        #[cfg(windows)]
        self.inner.suppress()?;
        Ok(())
    }

    pub fn restore(&mut self) -> anyhow::Result<()> {
        #[cfg(windows)]
        self.inner.restore()?;
        Ok(())
    }
}

impl Default for PointerSuppression {
    fn default() -> Self {
        Self::new()
    }
}

pub struct HidMonitor {
    #[allow(dead_code)]
    pub(crate) shutdown: Option<std::sync::mpsc::Sender<()>>,
    pub(crate) thread: Option<std::thread::JoinHandle<()>>,
}

impl HidMonitor {
    pub(crate) fn new(
        shutdown: Option<std::sync::mpsc::Sender<()>>,
        thread: Option<std::thread::JoinHandle<()>>,
    ) -> Self {
        Self { shutdown, thread }
    }
}

impl Drop for HidMonitor {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
