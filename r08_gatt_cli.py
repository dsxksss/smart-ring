from __future__ import annotations

import argparse
import asyncio
import sys
import traceback
import uuid
from datetime import datetime

from winrt.windows.devices.bluetooth.genericattributeprofile import (
    GattClientCharacteristicConfigurationDescriptorValue,
    GattCommunicationStatus,
    GattDeviceService,
    GattWriteOption,
)
from winrt.windows.devices.enumeration import DeviceInformation
from winrt.windows.storage.streams import Buffer


SERVICE_UUID = uuid.UUID("6e40fff0-b5a3-f393-e0a9-e50e24dcca9e")
WRITE_UUID = uuid.UUID("6e400002-b5a3-f393-e0a9-e50e24dcca9e")
NOTIFY_UUID = uuid.UUID("6e400003-b5a3-f393-e0a9-e50e24dcca9e")
TEST_OPEN = bytes([0xC9, *([0x00] * 14), 0xC9])
TEST_CLOSE = bytes([0xCA, *([0x00] * 14), 0xCA])


def format_hex(data: bytes | bytearray) -> str:
    return " ".join(f"{value:02X}" for value in data)


def require_success(result: object, operation: str) -> object:
    status = getattr(result, "status", None)
    if status != GattCommunicationStatus.SUCCESS:
        protocol_error = getattr(result, "protocol_error", None)
        raise RuntimeError(
            f"{operation}失败：status={status}, protocol_error={protocol_error}"
        )
    return result


def to_buffer(data: bytes) -> Buffer:
    buffer = Buffer(len(data))
    buffer.length = buffer.capacity
    with memoryview(buffer) as view:
        view[:] = data
    return buffer


async def find_service() -> tuple[DeviceInformation, GattDeviceService]:
    selector = GattDeviceService.get_device_selector_from_uuid(SERVICE_UUID)
    devices = await DeviceInformation.find_all_async_aqs_filter(selector)
    matches = [device for device in devices if device.name.upper() == "R08_9C07"]
    if not matches:
        raise RuntimeError("Windows 中没有找到已配对的 R08_9C07 GATT 服务")
    info = matches[0]
    service = await GattDeviceService.from_id_async(info.id)
    if service is None:
        raise RuntimeError("Windows 找到了服务，但无法打开 GATT 服务接口")
    return info, service


async def get_io_characteristics(service: GattDeviceService) -> tuple[object, object]:
    result = require_success(
        await service.get_characteristics_async(), "枚举 GATT 特征"
    )
    characteristics = list(result.characteristics)
    write_char = next((item for item in characteristics if item.uuid == WRITE_UUID), None)
    notify_char = next((item for item in characteristics if item.uuid == NOTIFY_UUID), None)
    if write_char is None or notify_char is None:
        found = ", ".join(str(item.uuid) for item in characteristics)
        raise RuntimeError(f"缺少 R08 写入或通知特征；实际特征：{found}")
    return write_char, notify_char


async def write_packet(characteristic: object, packet: bytes) -> None:
    result = await characteristic.write_value_with_result_and_option_async(
        to_buffer(packet), GattWriteOption.WRITE_WITH_RESPONSE
    )
    require_success(result, f"写入 {format_hex(packet)}")
    now = datetime.now().astimezone().isoformat(timespec="milliseconds")
    print(f"{now} TX  {format_hex(packet)}", flush=True)


async def run_test(seconds: float) -> int:
    info, service = await find_service()
    print(f"已通过 Windows GATT 服务直连：{info.name}", flush=True)
    print(f"设备接口：{info.id}", flush=True)
    notify_char = None
    notify_token = None
    try:
        write_char, notify_char = await get_io_characteristics(service)

        def on_value_changed(_sender: object, args: object) -> None:
            packet = bytes(args.characteristic_value)
            now = datetime.now().astimezone().isoformat(timespec="milliseconds")
            print(f"{now} RX  {format_hex(packet)}", flush=True)

        notify_token = notify_char.add_value_changed(on_value_changed)
        result = await notify_char.write_client_characteristic_configuration_descriptor_with_result_async(
            GattClientCharacteristicConfigurationDescriptorValue.NOTIFY
        )
        require_success(result, "启用通知")
        print("通知已启用", flush=True)
        await write_packet(write_char, TEST_OPEN)
        print(f"TEST_READY 请持续转动戒指；监听 {seconds:g} 秒", flush=True)
        await asyncio.sleep(seconds)
        await write_packet(write_char, TEST_CLOSE)
        print("TEST_DONE 已发送关闭测试命令", flush=True)
        return 0
    finally:
        if notify_char is not None:
            try:
                await notify_char.write_client_characteristic_configuration_descriptor_with_result_async(
                    GattClientCharacteristicConfigurationDescriptorValue.NONE
                )
            except Exception:
                pass
            if notify_token is not None:
                notify_char.remove_value_changed(notify_token)
        service.close()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="R08 Windows GATT CLI 调试器")
    parser.add_argument(
        "--seconds", type=float, default=20.0, help="C9 测试监听时长（默认 20 秒）"
    )
    return parser.parse_args()


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    args = parse_args()
    try:
        return asyncio.run(run_test(max(1.0, args.seconds)))
    except KeyboardInterrupt:
        print("已中断", flush=True)
        return 130
    except Exception as exc:
        print(f"ERROR {type(exc).__name__}: {exc}", flush=True)
        traceback.print_exc()
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
