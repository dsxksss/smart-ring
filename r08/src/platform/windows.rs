use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::Console::GetConsoleWindow;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, mouse_event, KEYEVENTF_KEYUP, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_WHEEL, VK_C,
    VK_CONTROL, VK_V,
};
use windows::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, GetRawInputDeviceList, RegisterRawInputDevices,
    HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTDEVICELIST, RAWINPUTHEADER, RIDEV_INPUTSINK,
    RIDI_DEVICENAME, RID_INPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClassNameW,
    GetForegroundWindow, GetMessageW, GetWindowLongPtrW, PostMessageW, PostQuitMessage,
    RegisterClassW, SetWindowLongPtrW, ShowWindowAsync, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    CW_USEDEFAULT, GWLP_USERDATA, MSG, SW_MINIMIZE, WM_CLOSE, WM_DESTROY, WM_INPUT, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_POPUP,
};

use crate::identity::RING_MAC_COMPACT;
use crate::mapping::HidMouseEvent;
use crate::platform::inject::Injector;
use crate::platform::HidMonitor;

const RIM_TYPEMOUSE: u32 = 0;
const RIM_TYPEHID: u32 = 2;

struct HidState {
    tx: Sender<HidMouseEvent>,
    names: HashMap<isize, String>,
}

pub fn spawn_hid(tx: Sender<HidMouseEvent>) -> Result<HidMonitor> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<isize>>();
    let thread = thread::Builder::new()
        .name("r08-windows-hid".into())
        .spawn(move || match create_raw_input_window(tx) {
            Ok(hwnd) => {
                let _ = ready_tx.send(Ok(hwnd.0 as isize));
                message_loop();
            }
            Err(error) => {
                let _ = ready_tx.send(Err(error));
            }
        })
        .context("启动 Windows HID 线程失败")?;
    let hwnd_bits = ready_rx.recv().context("等待 Raw Input 窗口")??;
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _ = shutdown_rx.recv();
        unsafe {
            let hwnd = HWND(hwnd_bits as *mut c_void);
            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    });
    Ok(HidMonitor::new(Some(shutdown_tx), Some(thread)))
}

fn create_raw_input_window(tx: Sender<HidMouseEvent>) -> Result<HWND> {
    unsafe {
        let class_name: Vec<u16> = "R08RawInputWindow\0".encode_utf16().collect();
        let wnd = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: GetModuleHandleW(None)?.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            hbrBackground: HBRUSH::default(),
            ..Default::default()
        };
        RegisterClassW(&wnd);
        let state = Box::into_raw(Box::new(HidState {
            tx,
            names: HashMap::new(),
        }));
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WS_POPUP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            HWND::default(),
            None,
            wnd.hInstance,
            Some(state as *const c_void),
        )?;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        let devices = [
            RAWINPUTDEVICE {
                usUsagePage: 0x01,
                usUsage: 0x02,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
            RAWINPUTDEVICE {
                usUsagePage: 0x0C,
                usUsage: 0x01,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
        ];
        RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32)
            .context("RegisterRawInputDevices")?;
        enumerate_ring_devices();
        tracing::info!("HID_READY 已监听 R08 鼠标与用户控制原始输入");
        Ok(hwnd)
    }
}

fn message_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn enumerate_ring_devices() {
    unsafe {
        let mut count = 0u32;
        let _ = GetRawInputDeviceList(None, &mut count, size_of::<RAWINPUTDEVICELIST>() as u32);
        if count == 0 {
            tracing::info!("HID_DEVICE 未枚举到原始输入设备");
            return;
        }
        let mut devices = vec![RAWINPUTDEVICELIST::default(); count as usize];
        let actual = GetRawInputDeviceList(
            Some(devices.as_mut_ptr()),
            &mut count,
            size_of::<RAWINPUTDEVICELIST>() as u32,
        );
        if actual == u32::MAX {
            return;
        }
        let mut matched = 0;
        for device in devices.iter().take(actual as usize) {
            let name = device_name(device.hDevice);
            if name.to_ascii_uppercase().contains(RING_MAC_COMPACT) {
                matched += 1;
                tracing::info!("HID_DEVICE type={} {name}", device.dwType.0);
            }
        }
        if matched == 0 {
            tracing::info!("HID_DEVICE Windows 已安装 R08 HID，但当前原始输入列表未匹配到其地址");
        }
    }
}

fn device_name(device: HANDLE) -> String {
    unsafe {
        let mut chars = 0u32;
        let _ = GetRawInputDeviceInfoW(device, RIDI_DEVICENAME, None, &mut chars);
        if chars == 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; chars as usize];
        let result = GetRawInputDeviceInfoW(
            device,
            RIDI_DEVICENAME,
            Some(buffer.as_mut_ptr() as *mut c_void),
            &mut chars,
        );
        if result == u32::MAX {
            return String::new();
        }
        String::from_utf16_lossy(&buffer)
            .trim_end_matches('\0')
            .to_string()
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_INPUT => {
            read_raw_input(hwnd, HRAWINPUT(lparam.0 as *mut c_void));
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if ptr != 0 {
                drop(Box::from_raw(ptr as *mut HidState));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn read_raw_input(hwnd: HWND, handle: HRAWINPUT) {
    unsafe {
        let userdata = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if userdata == 0 {
            return;
        }
        let state = &mut *(userdata as *mut HidState);
        let mut size = 0u32;
        let header_size = size_of::<RAWINPUTHEADER>() as u32;
        if GetRawInputData(handle, RID_INPUT, None, &mut size, header_size) != 0 || size == 0 {
            return;
        }
        let mut buffer = vec![0u8; size as usize];
        if GetRawInputData(
            handle,
            RID_INPUT,
            Some(buffer.as_mut_ptr() as *mut c_void),
            &mut size,
            header_size,
        ) != size
        {
            return;
        }
        let raw = &*(buffer.as_ptr() as *const RAWINPUT);
        let name = state
            .names
            .entry(raw.header.hDevice.0 as isize)
            .or_insert_with(|| device_name(raw.header.hDevice));
        let is_ring = name.to_ascii_uppercase().contains(RING_MAC_COMPACT);
        if raw.header.dwType == RIM_TYPEMOUSE {
            let mouse = raw.data.mouse;
            let _ = state.tx.send(HidMouseEvent {
                is_ring,
                button_flags: mouse.Anonymous.Anonymous.usButtonFlags,
                button_data: mouse.Anonymous.Anonymous.usButtonData as i16,
                dx: mouse.lLastX,
                dy: mouse.lLastY,
            });
        } else if raw.header.dwType == RIM_TYPEHID && is_ring {
            tracing::debug!("HID_REPORT consumer collection from R08");
        }
    }
}

pub struct WindowsInjector;

impl WindowsInjector {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    fn hotkey(&mut self, key: u8) -> Result<()> {
        unsafe {
            if foreground_is_terminal() {
                let terminal = GetForegroundWindow();
                tracing::info!("CONTROL_TARGET 正在最小化控制窗口并切回目标软件");
                let _ = ShowWindowAsync(terminal, SW_MINIMIZE);
            }
        }
        if foreground_is_terminal() {
            thread::sleep(Duration::from_millis(180));
        }
        if foreground_is_terminal() {
            tracing::warn!(
                "ACTION_IGNORED Windows 未能切离控制窗口；为防止程序被 Ctrl+C 关闭，本次快捷键未发送"
            );
            return Ok(());
        }
        unsafe {
            keybd_event(VK_CONTROL.0 as u8, 0, Default::default(), 0);
            keybd_event(key, 0, Default::default(), 0);
            keybd_event(key, 0, KEYEVENTF_KEYUP, 0);
            keybd_event(VK_CONTROL.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        }
        Ok(())
    }
}

fn foreground_is_terminal() -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground == GetConsoleWindow() {
            return true;
        }
        if foreground.0.is_null() {
            return false;
        }
        let mut buffer = [0u16; 128];
        let length = GetClassNameW(foreground, &mut buffer);
        if length <= 0 {
            return false;
        }
        is_terminal_window_class(&String::from_utf16_lossy(&buffer[..length as usize]))
    }
}

fn is_terminal_window_class(class_name: &str) -> bool {
    matches!(
        class_name,
        "ConsoleWindowClass" | "CASCADIA_HOSTING_WINDOW_CLASS"
    )
}

impl Injector for WindowsInjector {
    fn wheel(&mut self, delta: i32) -> Result<()> {
        unsafe {
            mouse_event(MOUSEEVENTF_WHEEL, 0, 0, delta, 0);
        }
        Ok(())
    }

    fn capture_cursor_anchor(&mut self) -> Result<()> {
        Ok(())
    }

    fn restore_cursor(&mut self) -> Result<()> {
        Ok(())
    }

    fn release_left_button(&mut self) -> Result<()> {
        unsafe {
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
        }
        Ok(())
    }

    fn copy(&mut self) -> Result<()> {
        self.hotkey(VK_C.0 as u8)
    }

    fn paste(&mut self) -> Result<()> {
        self.hotkey(VK_V.0 as u8)
    }

    fn release_all(&mut self) -> Result<()> {
        unsafe {
            keybd_event(VK_C.0 as u8, 0, KEYEVENTF_KEYUP, 0);
            keybd_event(VK_V.0 as u8, 0, KEYEVENTF_KEYUP, 0);
            keybd_event(VK_CONTROL.0 as u8, 0, KEYEVENTF_KEYUP, 0);
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
        }
        Ok(())
    }
}

#[allow(dead_code)]
fn _hinstance_ty(_: HINSTANCE) {}

#[cfg(test)]
mod tests {
    use super::is_terminal_window_class;

    #[test]
    fn recognizes_legacy_console_and_windows_terminal() {
        assert!(is_terminal_window_class("ConsoleWindowClass"));
        assert!(is_terminal_window_class("CASCADIA_HOSTING_WINDOW_CLASS"));
        assert!(!is_terminal_window_class("Chrome_WidgetWin_1"));
    }
}
