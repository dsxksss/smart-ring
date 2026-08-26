#!/usr/bin/env python3
"""Record the R08 A1 03 stream through Bleak's Windows WinRT backend."""

from __future__ import annotations

import argparse
import asyncio
import csv
import json
import logging
import math
import sys
import time
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[2]
VENDORED_BLEAK = PROJECT_ROOT / "tmp" / "bleak-runtime"
if VENDORED_BLEAK.is_dir():
    sys.path.insert(0, str(VENDORED_BLEAK))

try:
    from bleak import BleakClient, BleakScanner  # noqa: E402
except ModuleNotFoundError:  # Keep protocol-only unit tests runnable offline.
    BleakClient = None  # type: ignore[assignment,misc]
    BleakScanner = None  # type: ignore[assignment,misc]

from analyze_sensor_csv import analyze_file  # noqa: E402


RING_NAME = "R08_9C07"
RING_ADDRESS = "31:31:45:37:9C:07"
WRITE_UUID = "6e400002-b5a3-f393-e0a9-e50e24dcca9e"
NOTIFY_UUID = "6e400003-b5a3-f393-e0a9-e50e24dcca9e"


def build_packet(payload: bytes) -> bytes:
    packet = bytearray(16)
    packet[: len(payload)] = payload
    packet[15] = sum(payload) & 0xFF
    return bytes(packet)


RAW_START = build_packet(bytes.fromhex("A1 04 04"))
RAW_STOP = build_packet(bytes.fromhex("A1 02"))


def decode_accelerometer(data: bytes) -> tuple[int, int, int] | None:
    if len(data) != 16 or data[:2] != bytes.fromhex("A1 03"):
        return None
    if sum(data[:15]) & 0xFF != data[15]:
        return None

    def int12(value: int) -> int:
        value &= 0xFFF
        return value - 0x1000 if value & 0x800 else value

    y = int12(data[2] << 4 | data[3] & 0x0F)
    z = int12(data[4] << 4 | data[5] & 0x0F)
    x = int12(data[6] << 4 | data[7] & 0x0F)
    return x, y, z


async def find_ring(timeout_seconds: float) -> object | None:
    if BleakScanner is None:
        raise RuntimeError("缺少 bleak；请先安装 requirements.txt 后再连接设备")
    found = asyncio.Event()
    ring = None

    def on_advertisement(device: object, advertisement: object) -> None:
        nonlocal ring
        name = getattr(device, "name", None) or getattr(advertisement, "local_name", None) or ""
        address = getattr(device, "address", "")
        normalized = str(address).replace("-", ":").upper()
        if ring is None and (name.upper() == RING_NAME or normalized == RING_ADDRESS):
            ring = device
            details = getattr(device, "details", None)
            raw_event = getattr(details, "adv", None) or getattr(details, "scan", None)
            print(f"FOUND {name or RING_NAME} {address}", flush=True)
            print(
                "ADVERTISEMENT "
                f"connectable={getattr(advertisement, 'connectable', None)} "
                f"rssi={getattr(advertisement, 'rssi', None)} "
                f"services={getattr(advertisement, 'service_uuids', None)} "
                f"type={getattr(raw_event, 'advertisement_type', None)!r} "
                f"address_type={getattr(raw_event, 'bluetooth_address_type', None)!r} "
                f"details={details!r}",
                flush=True,
            )
            found.set()

    scanner = BleakScanner(on_advertisement)
    await scanner.start()
    try:
        await asyncio.wait_for(found.wait(), timeout=timeout_seconds)
    except TimeoutError:
        return None
    finally:
        await scanner.stop()
    return ring


async def connect_ring(device: object) -> BleakClient:
    if BleakClient is None:
        raise RuntimeError("缺少 bleak；请先安装 requirements.txt 后再连接设备")
    errors: list[str] = []
    for address_type in (None, "random", "public"):
        winrt = {} if address_type is None else {"address_type": address_type}
        label = "auto" if address_type is None else address_type
        client = BleakClient(device, timeout=20.0, winrt=winrt)
        try:
            await client.connect()
            print(f"CONNECTED address_type={label}", flush=True)
            return client
        except Exception as error:  # Bleak exposes backend-specific exception types.
            errors.append(f"{label}: {type(error).__name__}: {error}")
            print(f"CONNECT_RETRY {errors[-1]}", flush=True)
            try:
                await client.disconnect()
            except Exception:
                pass
            await asyncio.sleep(1.0)
    raise RuntimeError("；".join(errors))


async def record(seconds: int, output: Path) -> dict[str, object]:
    if seconds < 1 or seconds > 600:
        raise ValueError("采集时间必须在 1 到 600 秒之间")
    if output.exists():
        raise FileExistsError(f"不会覆盖已有文件：{output}")

    print("SCAN 请保持手机蓝牙关闭，并双击戒指唤醒……", flush=True)
    ring = await find_ring(30.0)
    if ring is None:
        raise RuntimeError(f"30 秒内没有发现 {RING_NAME}")
    client = await connect_ring(ring)
    writer_handle = None
    started_raw = False
    samples = 0
    notifications = 0
    started = time.monotonic()
    last_ms: float | None = None

    try:
        await client.start_notify(NOTIFY_UUID, lambda _sender, data: None)
        output.parent.mkdir(parents=True, exist_ok=True)
        writer_handle = output.open("x", encoding="utf-8", newline="", buffering=1)
        writer = csv.writer(writer_handle)
        writer.writerow(("elapsed_ms", "delta_ms", "x", "y", "z", "magnitude"))

        def on_notify(_sender: object, value: bytearray) -> None:
            nonlocal notifications, samples, last_ms
            data = bytes(value)
            notifications += 1
            print(f"RX {data.hex(' ').upper()}", flush=True)
            decoded = decode_accelerometer(data)
            if decoded is None:
                return
            x, y, z = decoded
            elapsed_ms = (time.monotonic() - started) * 1000.0
            delta_ms = "" if last_ms is None else f"{elapsed_ms - last_ms:.3f}"
            last_ms = elapsed_ms
            magnitude = math.sqrt(x * x + y * y + z * z)
            writer.writerow((f"{elapsed_ms:.3f}", delta_ms, x, y, z, f"{magnitude:.3f}"))
            samples += 1
            print(f"SAMPLE t={elapsed_ms / 1000.0:.2f}s X={x} Y={y} Z={z}", flush=True)

        await client.stop_notify(NOTIFY_UUID)
        await client.start_notify(NOTIFY_UUID, on_notify)
        try:
            await client.write_gatt_char(WRITE_UUID, RAW_START, response=True)
            started_raw = True
        except Exception:
            try:
                await client.write_gatt_char(WRITE_UUID, RAW_STOP, response=True)
            except Exception:
                pass
            raise
        started = time.monotonic()
        print("SENSOR_STARTED LED 可能闪烁；不会发送 DFU", flush=True)
        await asyncio.sleep(seconds)
    finally:
        if client.is_connected and started_raw:
            try:
                await client.write_gatt_char(WRITE_UUID, RAW_STOP, response=True)
                print("SENSOR_STOP_REQUESTED 已发送 A1 02", flush=True)
            except Exception as error:
                print(f"SENSOR_STOP_ERROR {type(error).__name__}: {error}", flush=True)
        if client.is_connected:
            try:
                await client.stop_notify(NOTIFY_UUID)
            except Exception:
                pass
            await client.disconnect()
        if writer_handle is not None:
            writer_handle.close()

    if samples == 0:
        raise RuntimeError(
            "已经连接并发送采集命令，"
            f"共收到 {notifications} 条 UART 通知，但其中没有 A1 03 样本；"
            f"文件保留在 {output}"
        )
    return analyze_file(output)


def main() -> int:
    parser = argparse.ArgumentParser(description="通过 Bleak/WinRT 采集 R08 三轴通知")
    parser.add_argument("--seconds", type=int, default=15)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--debug", action="store_true", help="显示 Bleak/WinRT 调试日志")
    args = parser.parse_args()
    if args.debug:
        logging.basicConfig(level=logging.DEBUG)
    try:
        summary = asyncio.run(record(args.seconds, args.output))
    except KeyboardInterrupt:
        print("已中断；程序已尝试发送 A1 02", file=sys.stderr)
        return 130
    except Exception as error:
        print(f"ERROR {type(error).__name__}: {error}", file=sys.stderr)
        return 1
    print("SENSOR_SUMMARY " + json.dumps(summary, ensure_ascii=False), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
