#!/usr/bin/env python3
"""Execute the v11 thunk against the two stock vertical gesture arrays."""

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
    load_image,
)


EXPECTED_STOCK_SHA256 = (
    "c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b"
)
EXPECTED_PATCH_SHA256 = (
    "92bcd47df85a56a613a76c50ce6256dfe9deab36dd86b1d9d0615b3b23d09ec7"
)
PATCH_ADDRESS = 0x00829FD6
RETURN_SENTINEL = 0x008FF000
STACK_POINTER = 0x008EF000
VERTICAL_ARRAYS = (
    (0x008478E2, 13, -1),
    (0x0084794A, 12, 1),
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


def _signed_i16(data: bytes) -> int:
    return struct.unpack("<h", data)[0]


def execute_patch(patch: bytes, buttons: int, signed_y: int) -> dict[str, int]:
    unicorn, arm_const = _load_unicorn()
    machine = unicorn.Uc(
        unicorn.UC_ARCH_ARM,
        unicorn.UC_MODE_THUMB | unicorn.UC_MODE_MCLASS,
    )
    machine.mem_map(0x00800000, 0x00100000)
    machine.mem_write(PATCH_ADDRESS, patch)
    machine.mem_write(RETURN_SENTINEL, b"\x00\xbf")
    machine.mem_write(STACK_POINTER + 8, b"\xa5")

    def on_code(_uc: Any, address: int, _size: int, _user_data: Any) -> None:
        if address & ~1 == RETURN_SENTINEL:
            machine.emu_stop()

    machine.hook_add(unicorn.UC_HOOK_CODE, on_code)
    machine.reg_write(arm_const.UC_ARM_REG_SP, STACK_POINTER)
    machine.reg_write(arm_const.UC_ARM_REG_LR, RETURN_SENTINEL | 1)
    machine.reg_write(arm_const.UC_ARM_REG_R0, buttons)
    machine.reg_write(arm_const.UC_ARM_REG_R1, 1234)
    machine.reg_write(arm_const.UC_ARM_REG_R2, signed_y & 0xFFFFFFFF)
    machine.reg_write(arm_const.UC_ARM_REG_R3, 0x55)
    machine.emu_start(PATCH_ADDRESS | 1, 0, count=40)

    values = {
        "buttons": machine.reg_read(arm_const.UC_ARM_REG_R0),
        "x": machine.reg_read(arm_const.UC_ARM_REG_R1),
        "y": machine.reg_read(arm_const.UC_ARM_REG_R2),
        "wheel_raw": machine.reg_read(arm_const.UC_ARM_REG_R3),
    }
    values["wheel"] = struct.unpack("<i", struct.pack("<I", values["wheel_raw"]))[0]
    if [values["buttons"], values["x"], values["y"]] != [0, 0, 0]:
        raise AssertionError(f"pointer fields not cleared: {values}")
    if machine.mem_read(STACK_POINTER + 8, 1) != b"\x00":
        raise AssertionError("stack-packed mouse button byte was not cleared")
    del values["wheel_raw"]
    return values


def validate_patch(stock: bytes, patch: bytes) -> dict[str, Any]:
    stock_hash = hashlib.sha256(stock).hexdigest()
    if stock_hash != EXPECTED_STOCK_SHA256:
        raise ValueError(f"stock SHA-256 mismatch: {stock_hash}")
    patch_hash = hashlib.sha256(patch).hexdigest()
    if patch_hash != EXPECTED_PATCH_SHA256:
        raise ValueError(f"v11 patch SHA-256 mismatch: {patch_hash}")

    arrays = []
    for address, count, expected_direction in VERTICAL_ARRAYS:
        offset = address_to_file_offset(address, len(stock))
        records = []
        emitted = []
        for index in range(count):
            record = stock[offset + index * 8 : offset + (index + 1) * 8]
            buttons = record[0]
            signed_y = _signed_i16(record[4:6])
            result = execute_patch(patch, buttons, signed_y)
            wheel = result["wheel"]
            if wheel:
                emitted.append(wheel)
            records.append(
                {
                    "index": index,
                    "source_buttons": buttons,
                    "source_y": signed_y,
                    "wheel": wheel,
                }
            )
        if emitted != [expected_direction, expected_direction]:
            raise AssertionError(
                f"array 0x{address:08X} emitted {emitted}, expected two locked steps"
            )
        arrays.append(
            {
                "address": f"0x{address:08X}",
                "records": records,
                "emitted_wheel": emitted,
            }
        )

    boundary_cases = []
    for buttons, signed_y, expected in (
        (0, -1024, 0),
        (0, 1024, 0),
        (1, -15, 0),
        (1, 15, 0),
        (1, -16, -1),
        (1, 16, 1),
        (2, 1024, 0),
    ):
        result = execute_patch(patch, buttons, signed_y)
        if result["wheel"] != expected:
            raise AssertionError(
                f"buttons={buttons} Y={signed_y} emitted {result['wheel']}"
            )
        boundary_cases.append(
            {"source_buttons": buttons, "source_y": signed_y, "wheel": expected}
        )

    return {
        "classification": "INSTRUCTION_LEVEL_V11_CONTACT_GATED_WHEEL_VALIDATION",
        "stock_sha256": stock_hash,
        "patch_sha256": patch_hash,
        "arrays": arrays,
        "boundary_cases": boundary_cases,
        "pointer_axes_always_zero": True,
        "mouse_buttons_always_zero": True,
        "calibration_and_release_samples_suppressed": True,
        "wheel_steps_per_reviewed_vertical_gesture": 2,
        "raw_electrode_weight_available": False,
        "flash_allowed": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stock", type=Path)
    parser.add_argument("patch", type=Path)
    args = parser.parse_args()
    try:
        report = validate_patch(load_image(args.stock), args.patch.read_bytes())
    except (AssertionError, OSError, RuntimeError, ValueError) as error:
        parser.error(str(error))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
