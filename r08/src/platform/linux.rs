use std::collections::HashMap;
use std::os::unix::io::AsRawFd;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use evdev::{
    uinput::VirtualDeviceBuilder, AttributeSet, Device, EventType, InputEvent, InputEventKind, Key,
    RelativeAxisType,
};

use crate::identity::is_ring_hid_identity;
use crate::mapping::{
    HidMouseEvent, HORIZONTAL_WHEEL, LEFT_BUTTON_DOWN, LEFT_BUTTON_UP, VERTICAL_WHEEL,
};
use crate::platform::inject::Injector;
use crate::platform::HidMonitor;

const BTN_LEFT: Key = Key::BTN_LEFT;
const KEY_LEFTCTRL: Key = Key::KEY_LEFTCTRL;
const KEY_C: Key = Key::KEY_C;
const KEY_V: Key = Key::KEY_V;

pub fn spawn_hid(tx: Sender<HidMouseEvent>) -> Result<HidMonitor> {
    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("r08-linux-hid".into())
        .spawn(move || hid_loop(tx, shutdown_rx))
        .context("启动 Linux HID 线程失败")?;
    Ok(HidMonitor::new(Some(shutdown_tx), Some(thread)))
}

fn hid_loop(tx: Sender<HidMouseEvent>, shutdown: mpsc::Receiver<()>) {
    let mut known: HashMap<String, Device> = HashMap::new();
    loop {
        if shutdown.try_recv().is_ok() {
            break;
        }
        refresh_devices(&mut known);
        if known.is_empty() {
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        let paths: Vec<String> = known.keys().cloned().collect();
        for path in paths {
            let events = match known.get_mut(&path) {
                Some(device) => match device.fetch_events() {
                    Ok(events) => Ok(events.into_iter().collect::<Vec<_>>()),
                    Err(error) => Err(error),
                },
                None => continue,
            };
            match events {
                Ok(events) => {
                    let mut dx = 0;
                    let mut dy = 0;
                    let mut flags = 0u16;
                    let mut button_data = 0i16;
                    let mut pending = false;
                    for event in events {
                        match event.kind() {
                            InputEventKind::Key(key) if key == BTN_LEFT => {
                                flags |= if event.value() > 0 {
                                    LEFT_BUTTON_DOWN
                                } else {
                                    LEFT_BUTTON_UP
                                };
                                pending = true;
                            }
                            InputEventKind::RelAxis(axis) if axis == RelativeAxisType::REL_X => {
                                dx += event.value();
                                pending = true;
                            }
                            InputEventKind::RelAxis(axis) if axis == RelativeAxisType::REL_Y => {
                                dy += event.value();
                                pending = true;
                            }
                            InputEventKind::RelAxis(axis)
                                if axis == RelativeAxisType::REL_WHEEL =>
                            {
                                flags |= VERTICAL_WHEEL;
                                button_data = event.value() as i16;
                                pending = true;
                            }
                            InputEventKind::RelAxis(axis)
                                if axis == RelativeAxisType::REL_HWHEEL =>
                            {
                                flags |= HORIZONTAL_WHEEL;
                                button_data = event.value() as i16;
                                pending = true;
                            }
                            InputEventKind::Synchronization(_) => {
                                if pending {
                                    let _ = tx.send(HidMouseEvent {
                                        is_ring: true,
                                        button_flags: flags,
                                        button_data,
                                        dx,
                                        dy,
                                    });
                                    dx = 0;
                                    dy = 0;
                                    flags = 0;
                                    button_data = 0;
                                    pending = false;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    tracing::warn!("HID_DEVICE 读取 {path} 失败：{error}");
                    known.remove(&path);
                }
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn set_nonblocking(device: &Device) {
    unsafe {
        let fd = device.as_raw_fd();
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            let _ = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

fn refresh_devices(known: &mut HashMap<String, Device>) {
    for (path, mut device) in evdev::enumerate() {
        let path_text = path.display().to_string();
        if known.contains_key(&path_text) {
            continue;
        }
        let name = device.name().unwrap_or("").to_string();
        let unique = device.unique_name().unwrap_or("").to_string();
        if !is_ring_hid_identity(&name, &unique, &path_text) {
            continue;
        }
        match device.grab() {
            Ok(()) => {
                tracing::info!("HID_DEVICE grabbed {path_text} name={name} uniq={unique}");
                set_nonblocking(&device);
                known.insert(path_text, device);
            }
            Err(error) => tracing::warn!("HID_DEVICE 无法 grab {path_text}：{error}"),
        }
    }
}

pub struct LinuxInjector {
    device: evdev::uinput::VirtualDevice,
    cursor: Option<(i32, i32)>,
}

impl LinuxInjector {
    pub fn new() -> Result<Self> {
        let mut keys = AttributeSet::<Key>::new();
        keys.insert(KEY_LEFTCTRL);
        keys.insert(KEY_C);
        keys.insert(KEY_V);
        let mut axes = AttributeSet::<RelativeAxisType>::new();
        axes.insert(RelativeAxisType::REL_WHEEL);
        axes.insert(RelativeAxisType::REL_WHEEL_HI_RES);
        let device = VirtualDeviceBuilder::new()
            .context("打开 /dev/uinput 失败")?
            .name("R08 Virtual Wheel")
            .with_keys(&keys)
            .context("uinput keys")?
            .with_relative_axes(&axes)
            .context("uinput axes")?
            .build()
            .context("创建虚拟滚轮设备失败")?;
        Ok(Self {
            device,
            cursor: None,
        })
    }

    fn emit(&mut self, events: &[InputEvent]) -> Result<()> {
        self.device.emit(events).context("uinput emit")?;
        Ok(())
    }

    fn key(&mut self, key: Key, value: i32) -> Result<()> {
        self.emit(&[
            InputEvent::new(EventType::KEY, key.code(), value),
            InputEvent::new(EventType::SYNCHRONIZATION, 0, 0),
        ])
    }
}

impl Injector for LinuxInjector {
    fn wheel(&mut self, delta: i32) -> Result<()> {
        let lines = delta / 120;
        let mut events = Vec::new();
        if lines != 0 {
            events.push(InputEvent::new(
                EventType::RELATIVE,
                RelativeAxisType::REL_WHEEL.0,
                lines,
            ));
        }
        events.push(InputEvent::new(
            EventType::RELATIVE,
            RelativeAxisType::REL_WHEEL_HI_RES.0,
            delta,
        ));
        events.push(InputEvent::new(EventType::SYNCHRONIZATION, 0, 0));
        self.emit(&events)
    }

    fn restore_cursor(&mut self) -> Result<()> {
        // evdev grab already swallows ring pointer motion.
        let _ = self.cursor;
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
        let _ = self.key(KEY_C, 0);
        let _ = self.key(KEY_V, 0);
        self.key(KEY_LEFTCTRL, 0)
    }
}

impl LinuxInjector {
    fn hotkey(&mut self, key: Key) -> Result<()> {
        let result = (|| {
            self.key(KEY_LEFTCTRL, 1)?;
            self.key(key, 1)?;
            self.key(key, 0)?;
            self.key(KEY_LEFTCTRL, 0)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.release_all();
        }
        result
    }
}
