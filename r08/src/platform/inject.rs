use anyhow::Result;

pub trait Injector: Send {
    fn wheel(&mut self, delta: i32) -> Result<()>;
    fn restore_cursor(&mut self) -> Result<()>;
    fn release_left_button(&mut self) -> Result<()>;
    fn copy(&mut self) -> Result<()>;
    fn paste(&mut self) -> Result<()>;
    fn release_all(&mut self) -> Result<()>;
}

pub struct NullInjector;

impl Injector for NullInjector {
    fn wheel(&mut self, _delta: i32) -> Result<()> {
        Ok(())
    }
    fn restore_cursor(&mut self) -> Result<()> {
        Ok(())
    }
    fn release_left_button(&mut self) -> Result<()> {
        Ok(())
    }
    fn copy(&mut self) -> Result<()> {
        Ok(())
    }
    fn paste(&mut self) -> Result<()> {
        Ok(())
    }
    fn release_all(&mut self) -> Result<()> {
        Ok(())
    }
}

pub struct GuardedInjector {
    inner: Box<dyn Injector>,
}

impl GuardedInjector {
    pub fn new(inner: Box<dyn Injector>) -> Self {
        Self { inner }
    }
}

impl Injector for GuardedInjector {
    fn wheel(&mut self, delta: i32) -> Result<()> {
        self.inner.wheel(delta)
    }
    fn restore_cursor(&mut self) -> Result<()> {
        self.inner.restore_cursor()
    }
    fn release_left_button(&mut self) -> Result<()> {
        self.inner.release_left_button()
    }
    fn copy(&mut self) -> Result<()> {
        let result = self.inner.copy();
        if result.is_err() {
            let _ = self.inner.release_all();
        }
        result
    }
    fn paste(&mut self) -> Result<()> {
        let result = self.inner.paste();
        if result.is_err() {
            let _ = self.inner.release_all();
        }
        result
    }
    fn release_all(&mut self) -> Result<()> {
        self.inner.release_all()
    }
}

impl Drop for GuardedInjector {
    fn drop(&mut self) {
        let _ = self.inner.release_all();
    }
}

pub fn create() -> Result<Box<dyn Injector>> {
    #[cfg(windows)]
    {
        return Ok(Box::new(GuardedInjector::new(Box::new(
            crate::platform::windows::WindowsInjector::new()?,
        ))));
    }
    #[cfg(target_os = "linux")]
    {
        return Ok(Box::new(GuardedInjector::new(Box::new(
            crate::platform::linux::LinuxInjector::new()?,
        ))));
    }
    #[cfg(target_os = "macos")]
    {
        return Ok(Box::new(GuardedInjector::new(Box::new(
            crate::platform::macos::MacInjector::new()?,
        ))));
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        anyhow::bail!("当前操作系统没有输入注入后端");
    }
}
