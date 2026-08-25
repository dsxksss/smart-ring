from __future__ import annotations

import asyncio
import ctypes
import json
import queue
import re
import threading
import time
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Coroutine

import tkinter as tk
from tkinter import messagebox, ttk
from tkinter.scrolledtext import ScrolledText

try:
    from bleak import BleakClient, BleakScanner
except ImportError as exc:  # The GUI can explain how to install the dependency.
    BleakClient = None  # type: ignore[assignment]
    BleakScanner = None  # type: ignore[assignment]
    BLEAK_IMPORT_ERROR: Exception | None = exc
else:
    BLEAK_IMPORT_ERROR = None


APP_DIR = Path(__file__).resolve().parent
CAPTURE_DIR = APP_DIR / "captures"
COLMI_WRITE_UUID = "6e400002-b5a3-f393-e0a9-e50e24dcca9e"


def parse_hex_payload(value: str) -> bytes:
    """Parse user-entered hex, accepting spaces, colons, commas and 0x prefixes."""
    cleaned = value.strip().lower().replace("0x", "")
    cleaned = re.sub(r"[\s,:;_-]+", "", cleaned)
    if not cleaned:
        raise ValueError("请输入十六进制数据")
    if not re.fullmatch(r"[0-9a-f]+", cleaned):
        raise ValueError("数据中包含非十六进制字符")
    if len(cleaned) % 2:
        raise ValueError("十六进制数据必须由完整字节组成，例如 02 04")
    return bytes.fromhex(cleaned)


def format_packet(data: bytes) -> str:
    return " ".join(f"{byte:02X}" for byte in data)


def build_colmi_packet(payload: bytes) -> bytes:
    """Pad a COLMI command to 16 bytes and append its additive checksum."""
    if not payload:
        raise ValueError("COLMI 命令不能为空")
    if len(payload) > 15:
        raise ValueError("COLMI 命令正文最多 15 字节")
    packet = bytearray(16)
    packet[: len(payload)] = payload
    packet[15] = sum(payload) & 0xFF
    return bytes(packet)


COLMI_RAW_START_PACKET = build_colmi_packet(bytes.fromhex("A1 04 04"))
COLMI_RAW_STOP_PACKET = build_colmi_packet(bytes.fromhex("A1 02"))
R08_TOUCH_ENABLE_PACKET = build_colmi_packet(bytes.fromhex("3B 02 00 01 01"))
R08_TOUCH_VIDEO_PACKET = build_colmi_packet(bytes.fromhex("3B 02 00 02 01"))
R08_TOUCH_DISABLE_PACKET = build_colmi_packet(bytes.fromhex("3B 02 00 00 01"))
R08_TOUCH_READ_PACKET = build_colmi_packet(bytes.fromhex("3B 01 00"))
R08_TAP_FLUSH_MS = 850
WHEEL_DELTA = 120
SMOOTH_SCROLL_STEPS_PER_NOTCH = 3
SMOOTH_SCROLL_MAX_QUEUED_STEPS = 60


def describe_colmi_packet(data: bytes) -> str:
    """Return a cautious human-readable description for known packet shapes."""
    if len(data) != 16:
        return ""
    if (sum(data[:15]) & 0xFF) != data[15]:
        return "校验和不匹配"
    command = data[0]
    if command == 0x73:
        notification = data[1]
        if notification == 0x2A:
            return f"R08 未知状态通知 0x2A={data[2]}"
        return f"设备通知 0x{notification:02X}，值={data[2]}"
    if command == 0xAA and data[1] == 0xEE:
        return "设备返回 AA EE（上一条命令未识别或不受支持）"
    if command == 0x02 and data[1] == 0x02:
        return "R08 相机/长按事件（动作=拍照）"
    if command == 0x1D:
        actions = {
            1: "点击/播放暂停",
            2: "下滑/上一项",
            3: "上滑/下一项",
            4: "音量增加方向",
            5: "音量减少方向",
        }
        return f"R08 触摸动作：{actions.get(data[1], f'未知动作 {data[1]}')}"
    if command == 0x3B:
        operation = data[1]
        if operation == 0x01:
            enabled = data[2] == 0
            if enabled:
                return (
                    "R08 触摸控制状态：已开启，"
                    f"应用类型={data[3]}，休眠={data[4]} 分钟，"
                    f"当前休眠={'是' if data[5] == 1 else '否'}"
                )
            return f"R08 触摸控制状态：已关闭，灵敏度={data[4]}"
        if operation == 0x02:
            return "R08 触摸控制设置应答"
        return f"R08 触摸控制协议操作 0x{operation:02X}"
    if command == 0xA1:
        channel = data[1]
        if channel == 0x01:
            return "光学/血氧原始数据"
        if channel == 0x02:
            return "心率 PPG 原始数据"
        if channel == 0x03:
            def int12(value: int) -> int:
                return value - 0x1000 if value & 0x800 else value

            raw_y = int12(((data[2] << 4) | (data[3] & 0x0F)) & 0xFFF)
            raw_z = int12(((data[4] << 4) | (data[5] & 0x0F)) & 0xFFF)
            raw_x = int12(((data[6] << 4) | (data[7] & 0x0F)) & 0xFFF)
            return f"三轴加速度原始数据 X={raw_x} Y={raw_y} Z={raw_z}"
        return f"传感器原始数据通道 0x{channel:02X}"
    return ""


def build_smooth_scroll_deltas(direction: int, notches: int) -> list[int]:
    """Split wheel notches into high-resolution deltas for gradual scrolling."""
    if direction not in (-1, 1):
        raise ValueError("滚动方向必须是 -1 或 1")
    if notches < 1:
        raise ValueError("滚动格数必须至少为 1")
    step_delta = WHEEL_DELTA // SMOOTH_SCROLL_STEPS_PER_NOTCH
    return [direction * step_delta] * (notches * SMOOTH_SCROLL_STEPS_PER_NOTCH)


@dataclass(slots=True)
class WriteTarget:
    uuid: str
    label: str


class WindowsInputController:
    """Small SendInput-compatible wrapper using built-in Win32 APIs."""

    KEYEVENTF_KEYUP = 0x0002
    MOUSEEVENTF_WHEEL = 0x0800

    VK_CONTROL = 0x11
    VK_BACK = 0x08
    VK_LEFT = 0x25
    VK_RIGHT = 0x27

    def __init__(self) -> None:
        self.user32 = ctypes.windll.user32

    def key(self, virtual_key: int) -> None:
        self.user32.keybd_event(virtual_key, 0, 0, 0)
        self.user32.keybd_event(virtual_key, 0, self.KEYEVENTF_KEYUP, 0)

    def hotkey(self, *virtual_keys: int) -> None:
        for virtual_key in virtual_keys:
            self.user32.keybd_event(virtual_key, 0, 0, 0)
        for virtual_key in reversed(virtual_keys):
            self.user32.keybd_event(virtual_key, 0, self.KEYEVENTF_KEYUP, 0)

    def scroll(self, delta: int) -> None:
        self.user32.mouse_event(self.MOUSEEVENTF_WHEEL, 0, 0, delta, 0)


class BleWorker:
    """Owns the asyncio loop and every Bleak object on one background thread."""

    def __init__(self, events: queue.Queue[dict[str, Any]]) -> None:
        self.events = events
        self.loop = asyncio.new_event_loop()
        self.thread = threading.Thread(target=self._run, daemon=True, name="ble-worker")
        self.devices: dict[str, Any] = {}
        self.client: Any = None
        self.connected_address = ""
        self._disconnect_requested = False
        self.thread.start()

    def _run(self) -> None:
        asyncio.set_event_loop(self.loop)
        self.loop.run_forever()

    def post(self, event_type: str, **values: Any) -> None:
        self.events.put({"type": event_type, **values})

    def submit(self, coroutine: Coroutine[Any, Any, Any]) -> None:
        future = asyncio.run_coroutine_threadsafe(coroutine, self.loop)

        def report_unhandled(done: Any) -> None:
            try:
                error = done.exception()
            except Exception as exc:  # pragma: no cover - defensive shutdown path
                error = exc
            if error is not None:
                self.post("error", message=f"后台任务失败：{error}")

        future.add_done_callback(report_unhandled)

    async def scan(self, timeout: float = 7.0) -> None:
        self.post("state", state="scanning", message=f"正在扫描 BLE 设备（{timeout:.0f} 秒）…")
        try:
            discovered = await BleakScanner.discover(timeout=timeout, return_adv=True)
            rows: list[dict[str, Any]] = []
            self.devices.clear()
            for address, item in discovered.items():
                device, advertisement = item
                key = device.address or address
                self.devices[key] = device
                name = device.name or advertisement.local_name or "（未命名设备）"
                rows.append(
                    {
                        "key": key,
                        "name": name,
                        "address": device.address,
                        "rssi": advertisement.rssi,
                        "service_uuids": list(advertisement.service_uuids or []),
                    }
                )
            rows.sort(key=lambda row: (row["rssi"] is None, -(row["rssi"] or -999)))
            self.post("devices", devices=rows)
            self.post("state", state="idle", message=f"扫描完成：发现 {len(rows)} 个 BLE 设备")
        except Exception as exc:
            self.post("state", state="idle", message="扫描失败")
            self.post("error", message=f"扫描失败：{exc}")

    async def connect(self, key: str) -> None:
        device = self.devices.get(key)
        if device is None:
            self.post("error", message="设备已不在扫描列表中，请重新扫描")
            return

        await self._disconnect(silent=True)
        self.post("state", state="connecting", message=f"正在连接 {device.name or key}…")
        self._disconnect_requested = False
        client: Any = None
        try:
            client = BleakClient(device, disconnected_callback=self._on_disconnected, timeout=25.0)
            await client.connect()
            self.client = client
            self.connected_address = device.address

            services = client.services
            service_lines: list[str] = []
            write_targets: list[dict[str, str]] = []
            notify_count = 0

            for service in services:
                service_lines.append(f"服务 {service.uuid}  {service.description or ''}".rstrip())
                for characteristic in service.characteristics:
                    properties = {str(item).lower() for item in characteristic.properties}
                    props = ", ".join(sorted(properties)) or "无属性"
                    service_lines.append(
                        f"    特征 {characteristic.uuid}  [{props}]  {characteristic.description or ''}".rstrip()
                    )
                    if properties.intersection({"write", "write-without-response"}):
                        write_targets.append(
                            {
                                "uuid": characteristic.uuid,
                                "label": f"{characteristic.uuid}  [{props}]",
                            }
                        )
                    if properties.intersection({"notify", "indicate"}):
                        try:
                            await client.start_notify(characteristic, self._notification_handler)
                            notify_count += 1
                            service_lines.append("        ↳ 已订阅通知")
                        except Exception as exc:
                            service_lines.append(f"        ↳ 订阅失败：{exc}")

            self.post(
                "connected",
                name=device.name or "（未命名设备）",
                address=device.address,
                service_lines=service_lines,
                write_targets=write_targets,
                notify_count=notify_count,
            )
            self.post(
                "state",
                state="connected",
                message=f"已连接；自动订阅 {notify_count} 个通知特征",
            )
        except Exception as exc:
            if client is not None:
                try:
                    if client.is_connected:
                        self._disconnect_requested = True
                        await client.disconnect()
                except Exception:
                    pass
            self.client = None
            self.connected_address = ""
            self.post("state", state="idle", message="连接失败")
            self.post("error", message=f"连接失败：{exc}")

    def _notification_handler(self, sender: Any, data: bytearray) -> None:
        characteristic_uuid = str(getattr(sender, "uuid", sender))
        self.post(
            "packet",
            timestamp=datetime.now().astimezone().isoformat(timespec="milliseconds"),
            characteristic_uuid=characteristic_uuid,
            data=bytes(data),
            address=self.connected_address,
        )

    def _on_disconnected(self, _client: Any) -> None:
        self.client = None
        self.connected_address = ""
        if not self._disconnect_requested:
            self.post("disconnected", unexpected=True)

    async def write(self, characteristic_uuid: str, payload: bytes) -> None:
        client = self.client
        if client is None or not client.is_connected:
            self.post("error", message="请先连接戒指")
            return
        try:
            await client.write_gatt_char(characteristic_uuid, payload)
            self.post(
                "write_ok",
                timestamp=datetime.now().astimezone().isoformat(timespec="milliseconds"),
                characteristic_uuid=characteristic_uuid,
                data=payload,
            )
        except Exception as exc:
            self.post("error", message=f"发送失败：{exc}")

    async def disconnect(self) -> None:
        await self._disconnect(silent=False)

    async def _disconnect(self, silent: bool) -> None:
        client = self.client
        if client is None:
            if not silent:
                self.post("disconnected", unexpected=False)
            return
        self._disconnect_requested = True
        try:
            if client.is_connected:
                try:
                    await client.write_gatt_char(COLMI_WRITE_UUID, COLMI_RAW_STOP_PACKET)
                except Exception:
                    pass
                await client.disconnect()
        finally:
            self.client = None
            self.connected_address = ""
            if not silent:
                self.post("disconnected", unexpected=False)

    def close(self) -> None:
        try:
            future = asyncio.run_coroutine_threadsafe(self._disconnect(silent=True), self.loop)
            future.result(timeout=2.0)
        except RuntimeError:
            pass
        except Exception:
            pass
        self.loop.call_soon_threadsafe(self.loop.stop)
        self.thread.join(timeout=1.0)


class RingDetectorApp:
    CONTROL_ACTIONS = ("无操作", "滚轮上", "滚轮下", "复制", "粘贴", "光标左", "光标右", "退格", "撤销")
    PRESETS = {
        "开启 R08 触摸控制（1 分钟休眠）": format_packet(R08_TOUCH_ENABLE_PACKET),
        "开启 R08 短视频触控（支持双击）": format_packet(R08_TOUCH_VIDEO_PACKET),
        "关闭 R08 触摸控制": format_packet(R08_TOUCH_DISABLE_PACKET),
        "读取 R08 触摸控制状态": format_packet(R08_TOUCH_READ_PACKET),
        "开启 COLMI 遥控模式": format_packet(build_colmi_packet(bytes.fromhex("02 04"))),
        "关闭 COLMI 遥控模式": format_packet(build_colmi_packet(bytes.fromhex("02 06"))),
        "⚠ 开启光学/传感器原始数据": format_packet(COLMI_RAW_START_PACKET),
        "停止 COLMI 原始数据": format_packet(COLMI_RAW_STOP_PACKET),
    }
    GESTURES = ("上滑", "下滑", "左滑", "右滑", "单击", "双击", "三击", "长按", "其他")

    def __init__(self, root: tk.Tk) -> None:
        self.root = root
        self.events: queue.Queue[dict[str, Any]] = queue.Queue()
        self.worker = BleWorker(self.events)
        self.input_controller = WindowsInputController()
        self.device_keys: dict[str, str] = {}
        self.write_targets: dict[str, WriteTarget] = {}
        self.capture_file: Any = None
        self.capture_path: Path | None = None
        self.connected_name = ""
        self.connected_address = ""
        self.active_label = ""
        self.label_deadline = 0.0
        self.packet_count = 0
        self.hidden_sensor_count = 0
        self.tap_count = 0
        self.tap_flush_job: str | None = None
        self.scroll_deltas: list[int] = []
        self.scroll_job: str | None = None

        self.status_var = tk.StringVar(value="准备就绪。请先关闭手机蓝牙，然后扫描。")
        self.target_var = tk.StringVar()
        self.hex_var = tk.StringVar(value=self.PRESETS["开启 R08 触摸控制（1 分钟休眠）"])
        self.preset_var = tk.StringVar(value=next(iter(self.PRESETS)))
        self.capture_var = tk.StringVar(value="尚未开始记录")
        self.packet_var = tk.StringVar(value="收到 0 个数据包")
        self.show_sensor_var = tk.BooleanVar(value=False)
        self.control_enabled_var = tk.BooleanVar(value=False)
        self.smooth_scroll_var = tk.BooleanVar(value=True)
        self.scroll_notches_var = tk.IntVar(value=2)
        self.scroll_duration_var = tk.IntVar(value=360)
        self.action_vars = {
            2: tk.StringVar(value="滚轮下"),
            3: tk.StringVar(value="滚轮上"),
            4: tk.StringVar(value="无操作"),
            5: tk.StringVar(value="无操作"),
            "camera": tk.StringVar(value="无操作"),
        }

        self._build_ui()
        self.root.after(100, self._poll_events)
        self.root.protocol("WM_DELETE_WINDOW", self._on_close)

    def _build_ui(self) -> None:
        self.root.title("智能戒指 BLE 动作检测器")
        self.root.geometry("1120x760")
        self.root.minsize(920, 640)

        style = ttk.Style()
        if "vista" in style.theme_names():
            style.theme_use("vista")

        outer = ttk.Frame(self.root, padding=12)
        outer.pack(fill=tk.BOTH, expand=True)

        toolbar = ttk.Frame(outer)
        toolbar.pack(fill=tk.X, pady=(0, 10))
        self.scan_button = ttk.Button(toolbar, text="1. 扫描 BLE 设备", command=self._scan)
        self.scan_button.pack(side=tk.LEFT)
        self.connect_button = ttk.Button(toolbar, text="2. 连接所选设备", command=self._connect)
        self.connect_button.pack(side=tk.LEFT, padx=8)
        self.disconnect_button = ttk.Button(toolbar, text="断开", command=self._disconnect, state=tk.DISABLED)
        self.disconnect_button.pack(side=tk.LEFT)
        ttk.Label(toolbar, textvariable=self.status_var).pack(side=tk.LEFT, padx=16)

        paned = ttk.Panedwindow(outer, orient=tk.HORIZONTAL)
        paned.pack(fill=tk.BOTH, expand=True)

        device_frame = ttk.LabelFrame(paned, text="附近设备", padding=8)
        paned.add(device_frame, weight=2)
        self.device_tree = ttk.Treeview(
            device_frame,
            columns=("name", "address", "rssi"),
            show="headings",
            selectmode="browse",
        )
        self.device_tree.heading("name", text="名称")
        self.device_tree.heading("address", text="地址/标识")
        self.device_tree.heading("rssi", text="信号")
        self.device_tree.column("name", width=160)
        self.device_tree.column("address", width=260)
        self.device_tree.column("rssi", width=60, anchor=tk.CENTER)
        device_scroll = ttk.Scrollbar(device_frame, orient=tk.VERTICAL, command=self.device_tree.yview)
        self.device_tree.configure(yscrollcommand=device_scroll.set)
        self.device_tree.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)
        device_scroll.pack(side=tk.RIGHT, fill=tk.Y)
        self.device_tree.bind("<Double-1>", lambda _event: self._connect())

        notebook = ttk.Notebook(paned)
        paned.add(notebook, weight=5)

        live_tab = ttk.Frame(notebook, padding=8)
        service_tab = ttk.Frame(notebook, padding=8)
        command_tab = ttk.Frame(notebook, padding=8)
        control_tab = ttk.Frame(notebook, padding=8)
        notebook.add(live_tab, text="实时动作数据")
        notebook.add(control_tab, text="电脑控制")
        notebook.add(service_tab, text="服务与特征")
        notebook.add(command_tab, text="发送命令")

        marker = ttk.LabelFrame(live_tab, text="动作标记（点击后 8 秒内操作戒指）", padding=7)
        marker.pack(fill=tk.X, pady=(0, 8))
        for index, gesture in enumerate(self.GESTURES):
            ttk.Button(marker, text=gesture, command=lambda name=gesture: self._mark_gesture(name)).grid(
                row=index // 5, column=index % 5, padx=3, pady=3, sticky="ew"
            )
        for column in range(5):
            marker.columnconfigure(column, weight=1)

        mode_row = ttk.Frame(live_tab)
        mode_row.pack(fill=tk.X, pady=(0, 8))
        self.quick_touch_enable_button = ttk.Button(
            mode_row,
            text="开启 R08 触摸控制",
            command=lambda: self._send_named_preset("开启 R08 触摸控制（1 分钟休眠）"),
            state=tk.DISABLED,
        )
        self.quick_touch_enable_button.pack(side=tk.LEFT)
        self.quick_touch_video_button = ttk.Button(
            mode_row,
            text="开启短视频触控",
            command=lambda: self._send_named_preset("开启 R08 短视频触控（支持双击）"),
            state=tk.DISABLED,
        )
        self.quick_touch_video_button.pack(side=tk.LEFT, padx=(8, 0))
        self.quick_touch_disable_button = ttk.Button(
            mode_row,
            text="关闭触摸控制",
            command=lambda: self._send_named_preset("关闭 R08 触摸控制"),
            state=tk.DISABLED,
        )
        self.quick_touch_disable_button.pack(side=tk.LEFT, padx=8)
        self.quick_touch_read_button = ttk.Button(
            mode_row,
            text="读取触摸状态",
            command=lambda: self._send_named_preset("读取 R08 触摸控制状态"),
            state=tk.DISABLED,
        )
        self.quick_touch_read_button.pack(side=tk.LEFT)

        diagnostic_row = ttk.Frame(live_tab)
        diagnostic_row.pack(fill=tk.X, pady=(0, 8))
        self.quick_remote_button = ttk.Button(
            diagnostic_row,
            text="开启相机遥控模式",
            command=lambda: self._send_named_preset("开启 COLMI 遥控模式"),
            state=tk.DISABLED,
        )
        self.quick_remote_button.pack(side=tk.LEFT)
        self.quick_stop_raw_button = ttk.Button(
            diagnostic_row,
            text="停止原始健康数据",
            command=lambda: self._send_named_preset("停止 COLMI 原始数据"),
            state=tk.DISABLED,
        )
        self.quick_stop_raw_button.pack(side=tk.LEFT, padx=8)
        ttk.Label(diagnostic_row, text="断开连接时也会自动停止原始光学采样").pack(side=tk.LEFT, padx=8)

        live_header = ttk.Frame(live_tab)
        live_header.pack(fill=tk.X, pady=(0, 5))
        ttk.Label(live_header, textvariable=self.packet_var).pack(side=tk.LEFT)
        ttk.Checkbutton(
            live_header,
            text="显示 A1 原始健康/传感器数据",
            variable=self.show_sensor_var,
        ).pack(side=tk.LEFT, padx=12)
        ttk.Button(live_header, text="清空显示", command=self._clear_live_log).pack(side=tk.RIGHT)
        self.live_log = ScrolledText(live_tab, wrap=tk.NONE, height=20, font=("Consolas", 10), state=tk.DISABLED)
        self.live_log.pack(fill=tk.BOTH, expand=True)
        ttk.Label(live_tab, textvariable=self.capture_var).pack(fill=tk.X, pady=(6, 0))

        ttk.Checkbutton(
            control_tab,
            text="启用戒指控制 Windows",
            variable=self.control_enabled_var,
        ).pack(anchor=tk.W, pady=(0, 12))
        scroll_frame = ttk.LabelFrame(control_tab, text="平滑滚动", padding=10)
        scroll_frame.pack(fill=tk.X, pady=(0, 12))
        ttk.Checkbutton(
            scroll_frame,
            text="将一次上下滑拆成连续小步",
            variable=self.smooth_scroll_var,
        ).grid(row=0, column=0, columnspan=4, sticky="w", pady=(0, 8))
        ttk.Label(scroll_frame, text="每次滑动：").grid(row=1, column=0, sticky="w")
        ttk.Spinbox(
            scroll_frame,
            from_=1,
            to=10,
            textvariable=self.scroll_notches_var,
            width=5,
        ).grid(row=1, column=1, sticky="w")
        ttk.Label(scroll_frame, text="格").grid(row=1, column=2, sticky="w", padx=(4, 20))
        ttk.Label(scroll_frame, text="完成时间：").grid(row=1, column=3, sticky="w")
        ttk.Spinbox(
            scroll_frame,
            from_=100,
            to=1500,
            increment=50,
            textvariable=self.scroll_duration_var,
            width=7,
        ).grid(row=1, column=4, sticky="w")
        ttk.Label(scroll_frame, text="毫秒").grid(row=1, column=5, sticky="w", padx=(4, 0))
        ttk.Label(
            scroll_frame,
            text="连续同向滑动会累加；反向滑动会立即切换方向。",
            foreground="#555555",
        ).grid(row=2, column=0, columnspan=6, sticky="w", pady=(8, 0))
        ttk.Label(
            control_tab,
            text=(
                "点击计数：双击执行 Ctrl+C，较慢三击执行 Ctrl+V；"
                "三击时每下间隔约 400～700 ms，单击不执行电脑操作。"
            ),
        ).pack(anchor=tk.W, pady=(0, 12))
        mapping_frame = ttk.LabelFrame(control_tab, text="R08 动作码映射", padding=10)
        mapping_frame.pack(fill=tk.X)
        mapping_labels = {
            2: "动作 2（实测：下滑/上一项）",
            3: "动作 3（实测：上滑/下一项）",
            4: "动作 4（音量增加方向）",
            5: "动作 5（音量减少方向）",
            "camera": "相机/长按动作 02 02",
        }
        for row, code in enumerate((2, 3, 4, 5, "camera")):
            ttk.Label(mapping_frame, text=mapping_labels[code]).grid(row=row, column=0, sticky="w", pady=4)
            ttk.Combobox(
                mapping_frame,
                textvariable=self.action_vars[code],
                values=self.CONTROL_ACTIONS,
                state="readonly",
                width=16,
            ).grid(row=row, column=1, sticky="w", padx=12, pady=4)
        ttk.Label(
            control_tab,
            text=(
                "R08_9C07 真机录制结果：上滑=动作 3，下滑=动作 2；"
                "左右滑会被当成普通点击或纵向滑动，长按没有稳定 HID 事件。"
                "因此动作 4/5 与长按默认不执行操作，避免误删文字。"
            ),
            wraplength=680,
            foreground="#555555",
        ).pack(anchor=tk.W, pady=(14, 0))

        self.service_log = ScrolledText(service_tab, wrap=tk.NONE, font=("Consolas", 9), state=tk.DISABLED)
        self.service_log.pack(fill=tk.BOTH, expand=True)

        ttk.Label(command_tab, text="可写特征：").pack(anchor=tk.W)
        self.target_combo = ttk.Combobox(command_tab, textvariable=self.target_var, state="readonly")
        self.target_combo.pack(fill=tk.X, pady=(3, 12))

        preset_row = ttk.Frame(command_tab)
        preset_row.pack(fill=tk.X)
        ttk.Label(preset_row, text="常用命令：").pack(side=tk.LEFT)
        preset_combo = ttk.Combobox(
            preset_row,
            textvariable=self.preset_var,
            values=list(self.PRESETS),
            state="readonly",
            width=26,
        )
        preset_combo.pack(side=tk.LEFT, padx=6)
        ttk.Button(preset_row, text="填入", command=self._apply_preset).pack(side=tk.LEFT)

        ttk.Label(command_tab, text="十六进制数据：").pack(anchor=tk.W, pady=(18, 0))
        hex_row = ttk.Frame(command_tab)
        hex_row.pack(fill=tk.X, pady=4)
        ttk.Entry(hex_row, textvariable=self.hex_var, font=("Consolas", 11)).pack(side=tk.LEFT, fill=tk.X, expand=True)
        self.send_button = ttk.Button(hex_row, text="发送", command=self._send, state=tk.DISABLED)
        self.send_button.pack(side=tk.LEFT, padx=(8, 0))

        tip = (
            "说明：程序会自动订阅所有 Notify/Indicate 特征。未知型号通常先直接操作戒指观察数据；"
            "只有没有通知时，才尝试厂商命令。错误命令可能导致设备暂时无响应，请勿发送来源不明的数据。"
        )
        ttk.Label(command_tab, text=tip, wraplength=620, foreground="#555555").pack(anchor=tk.W, pady=(18, 0))

    def _scan(self) -> None:
        self.scan_button.configure(state=tk.DISABLED)
        self.worker.submit(self.worker.scan())

    def _connect(self) -> None:
        selection = self.device_tree.selection()
        if not selection:
            messagebox.showinfo("选择设备", "请先扫描并选择一个设备")
            return
        key = self.device_keys.get(selection[0])
        if not key:
            return
        self.connect_button.configure(state=tk.DISABLED)
        self.worker.submit(self.worker.connect(key))

    def _disconnect(self) -> None:
        self.worker.submit(self.worker.disconnect())

    def _send(self) -> None:
        display_value = self.target_var.get()
        target = self.write_targets.get(display_value)
        if target is None:
            messagebox.showinfo("选择特征", "当前设备没有可用的写入特征，或尚未连接")
            return
        try:
            payload = parse_hex_payload(self.hex_var.get())
        except ValueError as exc:
            messagebox.showerror("数据格式错误", str(exc))
            return
        if payload == COLMI_RAW_START_PACKET:
            confirmed = messagebox.askyesno(
                "开启原始传感器数据？",
                "此命令会开启心率/血氧光学传感器，戒指将持续闪红光或绿光并增加耗电。\n\n"
                "它不用于读取触控滑动。仍要继续吗？",
            )
            if not confirmed:
                return
        self.worker.submit(self.worker.write(target.uuid, payload))

    def _apply_preset(self) -> None:
        self.hex_var.set(self.PRESETS[self.preset_var.get()])

    def _send_named_preset(self, name: str) -> None:
        self.hex_var.set(self.PRESETS[name])
        self._send()

    def _mark_gesture(self, name: str) -> None:
        if not self.connected_address:
            messagebox.showinfo("尚未连接", "请先连接戒指，再标记动作")
            return
        self.active_label = name
        self.label_deadline = time.monotonic() + 8.0
        timestamp = datetime.now().astimezone().isoformat(timespec="milliseconds")
        self._append_live(f"--- 请执行：{name}（8 秒）---\n")
        self._write_capture(
            {
                "type": "marker",
                "timestamp": timestamp,
                "label": name,
                "device_address": self.connected_address,
            }
        )

    def _current_label(self) -> str:
        if self.active_label and time.monotonic() <= self.label_deadline:
            return self.active_label
        self.active_label = ""
        return ""

    def _start_capture(self) -> None:
        self._close_capture()
        CAPTURE_DIR.mkdir(parents=True, exist_ok=True)
        stamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        safe_name = re.sub(r"[^A-Za-z0-9._-]+", "_", self.connected_name).strip("_") or "ring"
        self.capture_path = CAPTURE_DIR / f"{stamp}_{safe_name}.jsonl"
        self.capture_file = self.capture_path.open("a", encoding="utf-8", buffering=1)
        self.capture_var.set(f"自动记录：{self.capture_path}")

    def _write_capture(self, record: dict[str, Any]) -> None:
        if self.capture_file is not None:
            self.capture_file.write(json.dumps(record, ensure_ascii=False) + "\n")

    def _close_capture(self) -> None:
        if self.capture_file is not None:
            self.capture_file.close()
        self.capture_file = None

    def _poll_events(self) -> None:
        try:
            while True:
                self._handle_event(self.events.get_nowait())
        except queue.Empty:
            pass
        self.root.after(100, self._poll_events)

    def _handle_event(self, event: dict[str, Any]) -> None:
        event_type = event["type"]
        if event_type == "state":
            self.status_var.set(event["message"])
            state = event["state"]
            if state in {"idle", "connected"}:
                self.scan_button.configure(state=tk.NORMAL)
            if state == "idle":
                self.connect_button.configure(state=tk.NORMAL)
        elif event_type == "devices":
            self._show_devices(event["devices"])
        elif event_type == "connected":
            self._show_connection(event)
        elif event_type == "packet":
            self._show_packet(event)
        elif event_type == "write_ok":
            packet = format_packet(event["data"])
            self._append_live(f"{event['timestamp']}  TX  {event['characteristic_uuid']}  {packet}\n")
            self._write_capture(
                {
                    "type": "tx",
                    "timestamp": event["timestamp"],
                    "characteristic_uuid": event["characteristic_uuid"],
                    "hex": event["data"].hex(),
                    "device_address": self.connected_address,
                }
            )
        elif event_type == "disconnected":
            self._show_disconnected(event.get("unexpected", False))
        elif event_type == "error":
            self.status_var.set(event["message"])
            self.scan_button.configure(state=tk.NORMAL)
            self.connect_button.configure(state=tk.NORMAL)
            messagebox.showerror("BLE 错误", event["message"])

    def _show_devices(self, devices: list[dict[str, Any]]) -> None:
        self.device_tree.delete(*self.device_tree.get_children())
        self.device_keys.clear()
        likely_item = ""
        likely_priority = -1
        for index, device in enumerate(devices):
            rssi = "" if device["rssi"] is None else f"{device['rssi']} dBm"
            item_id = self.device_tree.insert(
                "",
                tk.END,
                values=(device["name"], device["address"], rssi),
            )
            self.device_keys[item_id] = device["key"]
            name_lower = device["name"].lower()
            if name_lower.startswith("r08_"):
                priority = 3
            elif any(
                token in name_lower
                for token in ("ring", "r02", "r03", "r06", "r07", "r08", "r09", "r10", "r11", "r12")
            ):
                priority = 2
            elif index == 0:
                priority = 1
            else:
                priority = 0
            if priority > likely_priority:
                likely_item = item_id
                likely_priority = priority
        if likely_item:
            self.device_tree.selection_set(likely_item)
            self.device_tree.focus(likely_item)
            self.device_tree.see(likely_item)

    def _show_connection(self, event: dict[str, Any]) -> None:
        self.connected_name = event["name"]
        self.connected_address = event["address"]
        self.packet_count = 0
        self.hidden_sensor_count = 0
        self.packet_var.set("收到 0 个数据包")
        self.disconnect_button.configure(state=tk.NORMAL)
        self.connect_button.configure(state=tk.DISABLED)
        self.send_button.configure(state=tk.NORMAL)
        self.quick_touch_enable_button.configure(state=tk.NORMAL)
        self.quick_touch_video_button.configure(state=tk.NORMAL)
        self.quick_touch_disable_button.configure(state=tk.NORMAL)
        self.quick_touch_read_button.configure(state=tk.NORMAL)
        self.quick_remote_button.configure(state=tk.NORMAL)
        self.quick_stop_raw_button.configure(state=tk.NORMAL)

        self._replace_text(self.service_log, "\n".join(event["service_lines"]))
        self.write_targets.clear()
        target_labels: list[str] = []
        preferred_label = ""
        for item in event["write_targets"]:
            label = item["label"]
            self.write_targets[label] = WriteTarget(uuid=item["uuid"], label=label)
            target_labels.append(label)
            if item["uuid"].lower() == COLMI_WRITE_UUID:
                preferred_label = label
        self.target_combo.configure(values=target_labels)
        if target_labels:
            self.target_var.set(preferred_label or target_labels[0])
        else:
            self.target_var.set("")
            self.send_button.configure(state=tk.DISABLED)

        self._start_capture()
        self._write_capture(
            {
                "type": "session",
                "timestamp": datetime.now().astimezone().isoformat(timespec="milliseconds"),
                "device_name": self.connected_name,
                "device_address": self.connected_address,
                "notify_count": event["notify_count"],
                "write_targets": [item["uuid"] for item in event["write_targets"]],
                "services": event["service_lines"],
            }
        )
        self._append_live(
            f"\n=== 已连接 {self.connected_name} ({self.connected_address})；"
            f"订阅 {event['notify_count']} 个通知特征 ===\n"
        )

    def _show_packet(self, event: dict[str, Any]) -> None:
        self.packet_count += 1
        label = self._current_label()
        is_sensor_raw = len(event["data"]) == 16 and event["data"][0] == 0xA1
        if is_sensor_raw and not self.show_sensor_var.get():
            self.hidden_sensor_count += 1
        else:
            label_text = f"  [{label}]" if label else ""
            packet = format_packet(event["data"])
            description = describe_colmi_packet(event["data"])
            description_text = f"  ⟵ {description}" if description else ""
            self._append_live(
                f"{event['timestamp']}  RX  {event['characteristic_uuid']}  "
                f"({len(event['data'])} B)  {packet}{label_text}{description_text}\n"
            )
        if self.hidden_sensor_count:
            self.packet_var.set(
                f"收到 {self.packet_count} 个数据包（隐藏 {self.hidden_sensor_count} 个 A1 原始包）"
            )
        else:
            self.packet_var.set(f"收到 {self.packet_count} 个数据包")
        self._write_capture(
            {
                "type": "rx",
                "timestamp": event["timestamp"],
                "characteristic_uuid": event["characteristic_uuid"],
                "length": len(event["data"]),
                "hex": event["data"].hex(),
                "label": label or None,
                "device_address": event["address"],
            }
        )
        if self.control_enabled_var.get():
            self._handle_control_packet(event["data"])

    def _show_disconnected(self, unexpected: bool) -> None:
        self._close_capture()
        self.connected_name = ""
        self.connected_address = ""
        self.disconnect_button.configure(state=tk.DISABLED)
        self.connect_button.configure(state=tk.NORMAL)
        self.scan_button.configure(state=tk.NORMAL)
        self.send_button.configure(state=tk.DISABLED)
        self.quick_touch_enable_button.configure(state=tk.DISABLED)
        self.quick_touch_video_button.configure(state=tk.DISABLED)
        self.quick_touch_disable_button.configure(state=tk.DISABLED)
        self.quick_touch_read_button.configure(state=tk.DISABLED)
        self.quick_remote_button.configure(state=tk.DISABLED)
        self.quick_stop_raw_button.configure(state=tk.DISABLED)
        self.status_var.set("连接意外断开" if unexpected else "已断开")
        self._append_live("=== 连接已断开 ===\n")

    def _handle_control_packet(self, data: bytes) -> None:
        if len(data) != 16 or (sum(data[:15]) & 0xFF) != data[15]:
            return
        if data[0] == 0x1D:
            action_code = data[1]
            if action_code == 1:
                self.tap_count += 1
                if self.tap_flush_job is not None:
                    self.root.after_cancel(self.tap_flush_job)
                self.tap_flush_job = self.root.after(R08_TAP_FLUSH_MS, self._flush_taps)
                return
            action_var = self.action_vars.get(action_code)
            if action_var is not None:
                self._perform_windows_action(action_var.get())
        elif data[0] == 0x02 and data[1] == 0x02:
            self._perform_windows_action(self.action_vars["camera"].get())

    def _flush_taps(self) -> None:
        count = self.tap_count
        self.tap_count = 0
        self.tap_flush_job = None
        if count == 2:
            self._perform_windows_action("复制")
        elif count >= 3:
            self._perform_windows_action("粘贴")

    def _perform_windows_action(self, action: str) -> None:
        if action == "无操作":
            return
        if action == "滚轮上":
            self._scroll_windows(1)
        elif action == "滚轮下":
            self._scroll_windows(-1)
        elif action == "复制":
            self.input_controller.hotkey(self.input_controller.VK_CONTROL, ord("C"))
        elif action == "粘贴":
            self.input_controller.hotkey(self.input_controller.VK_CONTROL, ord("V"))
        elif action == "光标左":
            self.input_controller.key(self.input_controller.VK_LEFT)
        elif action == "光标右":
            self.input_controller.key(self.input_controller.VK_RIGHT)
        elif action == "退格":
            self.input_controller.key(self.input_controller.VK_BACK)
        elif action == "撤销":
            self.input_controller.hotkey(self.input_controller.VK_CONTROL, ord("Z"))
        self._append_live(f"    → Windows：{action}\n")

    def _scroll_windows(self, direction: int) -> None:
        if not self.smooth_scroll_var.get():
            self.input_controller.scroll(direction * WHEEL_DELTA)
            return

        try:
            notches = max(1, min(10, int(self.scroll_notches_var.get())))
        except (tk.TclError, ValueError):
            notches = 2
        new_deltas = build_smooth_scroll_deltas(direction, notches)

        if self.scroll_deltas and (self.scroll_deltas[0] > 0) != (direction > 0):
            self.scroll_deltas.clear()
        available = SMOOTH_SCROLL_MAX_QUEUED_STEPS - len(self.scroll_deltas)
        if available > 0:
            self.scroll_deltas.extend(new_deltas[:available])
        if self.scroll_job is None:
            self._run_scroll_step()

    def _run_scroll_step(self) -> None:
        self.scroll_job = None
        if not self.scroll_deltas:
            return
        delta = self.scroll_deltas.pop(0)
        self.input_controller.scroll(delta)
        try:
            duration_ms = max(100, min(1500, int(self.scroll_duration_var.get())))
            notches = max(1, min(10, int(self.scroll_notches_var.get())))
        except (tk.TclError, ValueError):
            duration_ms = 360
            notches = 2
        step_count = notches * SMOOTH_SCROLL_STEPS_PER_NOTCH
        interval_ms = max(15, duration_ms // step_count)
        self.scroll_job = self.root.after(interval_ms, self._run_scroll_step)

    def _append_live(self, value: str) -> None:
        self.live_log.configure(state=tk.NORMAL)
        self.live_log.insert(tk.END, value)
        self.live_log.see(tk.END)
        self.live_log.configure(state=tk.DISABLED)

    def _clear_live_log(self) -> None:
        self._replace_text(self.live_log, "")

    @staticmethod
    def _replace_text(widget: ScrolledText, value: str) -> None:
        widget.configure(state=tk.NORMAL)
        widget.delete("1.0", tk.END)
        widget.insert("1.0", value)
        widget.configure(state=tk.DISABLED)

    def _on_close(self) -> None:
        if self.scroll_job is not None:
            self.root.after_cancel(self.scroll_job)
            self.scroll_job = None
        self.scroll_deltas.clear()
        self._close_capture()
        self.worker.close()
        self.root.destroy()


def main() -> int:
    if BLEAK_IMPORT_ERROR is not None:
        root = tk.Tk()
        root.withdraw()
        messagebox.showerror(
            "缺少依赖",
            "尚未安装 bleak。请先双击 install.bat，或运行：\n\npython -m pip install -r requirements.txt",
        )
        root.destroy()
        return 1

    root = tk.Tk()
    RingDetectorApp(root)
    root.mainloop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
