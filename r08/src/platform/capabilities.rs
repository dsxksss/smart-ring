use super::PlatformCapabilities;

#[cfg(windows)]
pub fn detect() -> PlatformCapabilities {
    PlatformCapabilities {
        os: "windows",
        ble_backend: "Win32 GATT system-connection reuse + WinRT/btleplug fallback",
        hid_backend: "R08 HID mouse child temporarily disabled; gestures use GATT only",
        inject_backend: "mouse_event / keybd_event",
        notes: vec![
            "完全退出手机官方 App 并关闭手机蓝牙".to_string(),
            "无光标模式需要管理员权限，仅临时停用 R08 的 HID 鼠标子设备".to_string(),
            "Windows 设置里蓝牙显示未连接不代表 GATT/HID 不可用".to_string(),
            "默认启动即进入控制；只监听使用 r08 listen".to_string(),
        ],
    }
}

#[cfg(target_os = "linux")]
pub fn detect() -> PlatformCapabilities {
    let mut notes = vec![
        "需要 BlueZ 与可用的 BLE 适配器".to_string(),
        "HID 抓取需要 input 组权限；滚轮注入需要 /dev/uinput".to_string(),
        "对戒指 evdev 设备执行 grab，避免指针漂移".to_string(),
        "默认启动即进入控制；只监听使用 r08 listen".to_string(),
    ];
    if !std::path::Path::new("/dev/uinput").exists() {
        notes.push("/dev/uinput 不存在，连续滚轮注入不可用".to_string());
    } else if std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/uinput")
        .is_err()
    {
        notes.push("/dev/uinput 当前用户不可写，加入 input 组或调整 udev 规则".to_string());
    }
    PlatformCapabilities {
        os: "linux",
        ble_backend: "BlueZ D-Bus (btleplug)",
        hid_backend: "evdev grab, filtered by R08 name/MAC",
        inject_backend: "uinput REL_WHEEL_HI_RES",
        notes,
    }
}

#[cfg(target_os = "macos")]
pub fn detect() -> PlatformCapabilities {
    PlatformCapabilities {
        os: "macos",
        ble_backend: "Core Bluetooth (btleplug)",
        hid_backend: "GATT 0x1D discrete actions; macOS HID stack owns BLE mouse",
        inject_backend: "CGEvent scroll/key",
        notes: vec![
            "首次注入需要辅助功能权限".to_string(),
            "macOS 通常不暴露 BLE MAC，主要按广播名 R08_9C07 匹配".to_string(),
            "系统 HID 会吃掉鼠标报告，因此精细相对 Y 跟踪受限；上下滑以 GATT 离散动作为主"
                .to_string(),
            "这不是逐点触摸跟踪".to_string(),
            "默认启动即进入控制；只监听使用 r08 listen".to_string(),
        ],
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn detect() -> PlatformCapabilities {
    PlatformCapabilities {
        os: std::env::consts::OS,
        ble_backend: "unsupported",
        hid_backend: "unsupported",
        inject_backend: "unsupported",
        notes: vec!["当前操作系统没有输入注入后端".to_string()],
    }
}
