use anyhow::{bail, Result};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode, KeyCode, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use crate::platform::inject::Injector;

const KEY_C: CGKeyCode = 0x08;
const KEY_V: CGKeyCode = 0x09;

pub struct MacInjector {
    anchor: Option<CGPoint>,
}

impl MacInjector {
    pub fn new() -> Result<Self> {
        Ok(Self { anchor: None })
    }

    fn source() -> Result<CGEventSource> {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| anyhow::anyhow!("无法创建 CGEventSource；请在系统设置中允许辅助功能"))
    }

    fn post_scroll(&self, delta: i32) -> Result<()> {
        let source = Self::source()?;
        let event = CGEvent::new_scroll_event(source, ScrollEventUnit::PIXEL, 1, delta, 0, 0)
            .map_err(|()| anyhow::anyhow!("无法创建滚动事件"))?;
        event.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn key(&self, code: CGKeyCode, down: bool, flags: CGEventFlags) -> Result<()> {
        let source = Self::source()?;
        let event = CGEvent::new_keyboard_event(source, code, down)
            .map_err(|()| anyhow::anyhow!("无法创建按键事件"))?;
        event.set_flags(flags);
        event.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn hotkey(&mut self, key: CGKeyCode) -> Result<()> {
        let cmd = CGEventFlags::CGEventFlagCommand;
        let result = (|| {
            self.key(KeyCode::COMMAND, true, cmd)?;
            self.key(key, true, cmd)?;
            self.key(key, false, cmd)?;
            self.key(KeyCode::COMMAND, false, CGEventFlags::CGEventFlagNull)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.release_all();
        }
        result
    }
}

impl Injector for MacInjector {
    fn wheel(&mut self, delta: i32) -> Result<()> {
        self.post_scroll(delta)
    }

    fn restore_cursor(&mut self) -> Result<()> {
        if let Some(point) = self.anchor {
            if core_graphics::display::CGDisplay::warp_mouse_cursor_position(point).is_err() {
                bail!("无法恢复光标位置");
            }
        }
        Ok(())
    }

    fn release_left_button(&mut self) -> Result<()> {
        Ok(())
    }

    fn copy(&mut self) -> Result<()> {
        self.hotkey(KEY_C)
    }

    fn paste(&mut self) -> Result<()> {
        self.hotkey(KEY_V)
    }

    fn release_all(&mut self) -> Result<()> {
        let _ = self.key(KEY_C, false, CGEventFlags::CGEventFlagNull);
        let _ = self.key(KEY_V, false, CGEventFlags::CGEventFlagNull);
        self.key(KeyCode::COMMAND, false, CGEventFlags::CGEventFlagNull)
    }
}
