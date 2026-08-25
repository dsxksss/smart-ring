from __future__ import annotations

import asyncio
import json
import sys
import time
import traceback
from datetime import datetime

from bleak import BleakClient, BleakScanner

from smart_ring_detector import (
    CAPTURE_DIR,
    COLMI_WRITE_UUID,
    R08_TOUCH_READ_PACKET,
    R08_TOUCH_VIDEO_PACKET,
    describe_colmi_packet,
    format_packet,
)


R08_NOTIFY_UUID = "6e400003-b5a3-f393-e0a9-e50e24dcca9e"
RECORD_SECONDS = 70


async def find_ring() -> object | None:
    found = asyncio.Event()
    ring = None

    def on_advertisement(device: object, advertisement: object) -> None:
        nonlocal ring
        name = device.name or advertisement.local_name or ""
        if ring is None and name.upper() == "R08_9C07":
            ring = device
            print(f"FOUND {name} {device.address}", flush=True)
            found.set()

    scanner = BleakScanner(on_advertisement)
    await scanner.start()
    try:
        await asyncio.wait_for(found.wait(), timeout=60.0)
    except TimeoutError:
        return None
    finally:
        await scanner.stop()
    return ring


async def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    CAPTURE_DIR.mkdir(parents=True, exist_ok=True)
    capture_path = CAPTURE_DIR / f"r08_actions_{datetime.now():%Y%m%d_%H%M%S}.jsonl"
    print("SCAN 请双击触摸区唤醒戒指……", flush=True)
    ring = await find_ring()
    if ring is None:
        print("ERROR 60 秒内没有发现 R08_9C07", flush=True)
        return 2

    client = None
    address_types = ("random", "public", "random")
    for attempt, address_type in enumerate(address_types, start=1):
        candidate = BleakClient(
            ring,
            timeout=20.0,
            winrt={"address_type": address_type},
        )
        try:
            await candidate.connect()
            client = candidate
            break
        except Exception as exc:
            print(
                f"CONNECT_ERROR {attempt}/3 address_type={address_type} "
                f"{type(exc).__name__}: {exc}",
                flush=True,
            )
            traceback.print_exc()
            await asyncio.sleep(2.0)
    if client is None:
        return 3

    started = time.monotonic()
    capture_file = capture_path.open("w", encoding="utf-8", buffering=1)

    def write_record(record: dict[str, object]) -> None:
        capture_file.write(json.dumps(record, ensure_ascii=False) + "\n")

    def on_notify(sender: object, value: bytearray) -> None:
        data = bytes(value)
        elapsed = time.monotonic() - started
        now = datetime.now().astimezone().isoformat(timespec="milliseconds")
        description = describe_colmi_packet(data)
        print(f"RX +{elapsed:07.3f}s {format_packet(data)}  {description}", flush=True)
        write_record(
            {
                "type": "rx",
                "timestamp": now,
                "elapsed": round(elapsed, 3),
                "characteristic_uuid": str(sender),
                "hex": data.hex(),
                "description": description,
            }
        )

    try:
        await client.start_notify(R08_NOTIFY_UUID, on_notify)
        await client.write_gatt_char(COLMI_WRITE_UUID, R08_TOUCH_VIDEO_PACKET, response=True)
        await asyncio.sleep(0.8)
        await client.write_gatt_char(COLMI_WRITE_UUID, R08_TOUCH_READ_PACKET, response=True)
        await asyncio.sleep(1.2)
        started = time.monotonic()
        print(f"READY {RECORD_SECONDS} 秒动作录制开始，文件：{capture_path}", flush=True)
        write_record(
            {
                "type": "session",
                "timestamp": datetime.now().astimezone().isoformat(timespec="milliseconds"),
                "mode": "short_video_app_type_2",
                "record_seconds": RECORD_SECONDS,
            }
        )
        await asyncio.sleep(RECORD_SECONDS)
        print("DONE 动作录制结束", flush=True)
        await client.stop_notify(R08_NOTIFY_UUID)
    finally:
        capture_file.close()
        if client.is_connected:
            await client.disconnect()
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
