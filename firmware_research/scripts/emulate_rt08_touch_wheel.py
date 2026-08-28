#!/usr/bin/env python3
"""Execute the v10 wheel thunk and prove pointer fields are always cleared."""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
from pathlib import Path
from typing import Any

from analyze_rt08_thumb import (
    APPLICATION_BASE,
    HEADER_SIZE,
    address_to_file_offset,
    decode_thumb_bl,
    load_image,
)


EXPECTED_V10_SHA256 = (
    "6cd256de135ce4290794feebec808cdf4cea2e6fd9dfdd30e675a16fcb7927bb"
)
MOTION_HOOK_ADDRESS = 0x00829F7E
WHEEL_PATCH_ADDRESS = 0x00829FD6
EXTENDED_ENTRY_ADDRESS = 0x00829FD4
RETURN_SENTINEL = 0x008FF000
STACK_POINTER = 0x008EF000
V10_MARKER_ADDRESS = 0x008280CA
TOUCH_REPEAT_ADDRESS = 0x0082C604
WHEEL_PATCH = bytes.fromhex(
    "00 2a 05 d0 02 dc 01 23 5b 42 02 e0 01 23 00 e0 "
    "00 23 00 20 00 21 00 22 6c 46 20 72 70 47"
)


def _load_unicorn() -> tuple[Any, Any]:
    try:
        import unicorn
        from unicorn import arm_const
    except ImportError as error:  # pragma: no cover - CLI diagnostic
        raise RuntimeError(
            "Unicorn is required; install firmware_research/requirements-analysis.txt"
        ) from error
    return unicorn, arm_const


def validate_touch_wheel(image: bytes) -> dict[str, Any]:
    image_hash = hashlib.sha256(image).hexdigest()
    if image_hash != EXPECTED_V10_SHA256:
        raise ValueError(f"v10 SHA-256 mismatch: {image_hash}")

    marker_offset = address_to_file_offset(V10_MARKER_ADDRESS, len(image))
    if image[marker_offset : marker_offset + 2] != bytes.fromhex("fb 21"):
        raise ValueError("v10 activation marker mismatch")
    repeat_offset = address_to_file_offset(TOUCH_REPEAT_ADDRESS, len(image))
    if image[repeat_offset : repeat_offset + 2] != bytes.fromhex("03 23"):
        raise ValueError("v10 touch indicator repeat mismatch")

    hook_offset = address_to_file_offset(MOTION_HOOK_ADDRESS, len(image))
    first, second = struct.unpack_from("<HH", image, hook_offset)
    if decode_thumb_bl(MOTION_HOOK_ADDRESS, first, second) != WHEEL_PATCH_ADDRESS:
        raise ValueError("v10 motion hook target mismatch")
    extended_offset = address_to_file_offset(EXTENDED_ENTRY_ADDRESS, len(image))
    if image[extended_offset : extended_offset + 2] != bytes.fromhex("70 47"):
        raise ValueError("v10 extended mouse-report entry is not blocked")
    patch_offset = address_to_file_offset(WHEEL_PATCH_ADDRESS, len(image))
    if image[patch_offset : patch_offset + len(WHEEL_PATCH)] != WHEEL_PATCH:
        raise ValueError("v10 touch-wheel thunk bytes mismatch")

    unicorn, arm_const = _load_unicorn()
    executions = []
    for signed_y, expected_wheel in ((-321, -1), (0, 0), (654, 1)):
        machine = unicorn.Uc(
            unicorn.UC_ARCH_ARM,
            unicorn.UC_MODE_THUMB | unicorn.UC_MODE_MCLASS,
        )
        machine.mem_map(0x00800000, 0x00100000)
        machine.mem_write(APPLICATION_BASE, image[HEADER_SIZE:])
        machine.mem_write(RETURN_SENTINEL, b"\x00\xbf")
        machine.mem_write(STACK_POINTER + 8, b"\xa5")

        def on_code(_uc: Any, address: int, _size: int, _user_data: Any) -> None:
            if address & ~1 == RETURN_SENTINEL:
                machine.emu_stop()

        machine.hook_add(unicorn.UC_HOOK_CODE, on_code)
        machine.reg_write(arm_const.UC_ARM_REG_SP, STACK_POINTER)
        machine.reg_write(arm_const.UC_ARM_REG_LR, RETURN_SENTINEL | 1)
        machine.reg_write(arm_const.UC_ARM_REG_R0, 0x7F)
        machine.reg_write(arm_const.UC_ARM_REG_R1, 1234)
        machine.reg_write(arm_const.UC_ARM_REG_R2, signed_y & 0xFFFFFFFF)
        machine.reg_write(arm_const.UC_ARM_REG_R3, 0x55)
        machine.emu_start(WHEEL_PATCH_ADDRESS | 1, 0, count=32)

        registers = [
            machine.reg_read(register)
            for register in (
                arm_const.UC_ARM_REG_R0,
                arm_const.UC_ARM_REG_R1,
                arm_const.UC_ARM_REG_R2,
                arm_const.UC_ARM_REG_R3,
            )
        ]
        if registers[:3] != [0, 0, 0]:
            raise AssertionError(f"pointer fields not cleared: {registers}")
        if registers[3] != expected_wheel & 0xFFFFFFFF:
            raise AssertionError(f"wrong wheel for Y={signed_y}: {registers[3]}")
        if machine.mem_read(STACK_POINTER + 8, 1) != b"\x00":
            raise AssertionError("stack-packed mouse button byte was not cleared")
        executions.append(
            {
                "signed_y": signed_y,
                "wheel": expected_wheel,
                "buttons": 0,
                "x": 0,
                "y": 0,
            }
        )

    return {
        "classification": "INSTRUCTION_LEVEL_V10_TOUCH_WHEEL_VALIDATION",
        "image_sha256": image_hash,
        "activation_marker": "0xFB",
        "touch_indicator_repeat": 3,
        "motion_hook_target": f"0x{WHEEL_PATCH_ADDRESS:08X}",
        "extended_mouse_report_blocked": True,
        "executions": executions,
        "pointer_axes_always_zero": True,
        "mouse_buttons_always_zero": True,
        "wheel_is_signed_y_unit_direction": True,
        "flash_allowed": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    try:
        report = validate_touch_wheel(load_image(args.image))
    except (AssertionError, OSError, RuntimeError, ValueError) as error:
        parser.error(str(error))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
