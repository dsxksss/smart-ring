from __future__ import annotations

import asyncio
import sys
from datetime import datetime

from bleak import BleakClient, BleakScanner

from smart_ring_detector import (
    COLMI_WRITE_UUID,
    R08_TOUCH_ENABLE_PACKET,
    R08_TOUCH_READ_PACKET,
    describe_colmi_packet,
    format_packet,
)


R08_NOTIFY_UUID = "6e400003-b5a3-f393-e0a9-e50e24dcca9e"


async def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    print("正在扫描 R08_9C07（30 秒）……", flush=True)
    device = None
    found = asyncio.Event()

    def on_advertisement(candidate: object, advertisement: object) -> None:
        nonlocal device
        name = candidate.name or advertisement.local_name or ""
        print(f"发现 {name or '(无名称)'}  {candidate.address}")
        if device is None and name.upper() == "R08_9C07":
            device = candidate
            found.set()

    scanner = BleakScanner(on_advertisement)
    await scanner.start()
    try:
        await asyncio.wait_for(found.wait(), timeout=30.0)
    except TimeoutError:
        pass
    finally:
        await scanner.stop()
    if device is None:
        print("没有找到 R08_9C07。请确认手机 QRing 已退出且戒指在附近。")
        return 2

    print(f"连接 {device.name} ({device.address})……", flush=True)
    print(f"Windows BLE 设备信息：{device!r}", flush=True)
    client = None
    last_error: Exception | None = None
    for attempt in range(1, 4):
        candidate_client = BleakClient(device, timeout=20.0)
        try:
            await candidate_client.connect()
            client = candidate_client
            break
        except Exception as exc:
            last_error = exc
            print(f"第 {attempt}/3 次连接失败：{type(exc).__name__}: {exc}", flush=True)
            await asyncio.sleep(2.0)
    if client is None:
        print(f"Windows 未能创建 R08 的 GATT 连接：{last_error}")
        return 4

    try:
        notifications: list[bytes] = []

        def on_notify(_sender: object, data: bytearray) -> None:
            packet = bytes(data)
            notifications.append(packet)
            description = describe_colmi_packet(packet)
            suffix = f"  {description}" if description else ""
            now = datetime.now().astimezone().isoformat(timespec="milliseconds")
            print(f"{now} RX {format_packet(packet)}{suffix}", flush=True)

        await client.start_notify(R08_NOTIFY_UUID, on_notify)
        print(f"TX 开启触摸控制  {format_packet(R08_TOUCH_ENABLE_PACKET)}", flush=True)
        await client.write_gatt_char(COLMI_WRITE_UUID, R08_TOUCH_ENABLE_PACKET, response=True)
        await asyncio.sleep(0.8)
        print(f"TX 读取触摸状态  {format_packet(R08_TOUCH_READ_PACKET)}", flush=True)
        await client.write_gatt_char(COLMI_WRITE_UUID, R08_TOUCH_READ_PACKET, response=True)
        await asyncio.sleep(3.0)
        await client.stop_notify(R08_NOTIFY_UUID)
    finally:
        if client.is_connected:
            await client.disconnect()

    touch_packets = [packet for packet in notifications if len(packet) == 16 and packet[0] == 0x3B]
    if not touch_packets:
        print("命令写入成功，但未收到 0x3B 状态回包。")
        return 3
    print(f"验证成功：收到 {len(touch_packets)} 个 R08 触摸协议回包。")
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
