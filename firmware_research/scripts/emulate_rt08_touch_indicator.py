#!/usr/bin/env python3
"""Instruction-level validation of the v8 RT08 touch-indicator patch.

The emulator executes the exact finalized image from the stock 0x50 command
handler until it reaches the stock indicator engine.  It records the actual
ARM register arguments without connecting to a ring or exposing any DFU path.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any

from analyze_rt08_thumb import (
    APPLICATION_BASE,
    HEADER_SIZE,
    address_to_file_offset,
    load_image,
)


EXPECTED_V8_SHA256 = (
    "4b44c8a82f227e6697e7c5dc2633db5ed478f69ca28684b19d7fb17920d08441"
)
EXPECTED_V9_SHA256 = (
    "681dbb3e7a9112fc85b1d8e546717eb5052ae7a7138b117b6dfff75de7eba1f5"
)
SUPPORTED_IMAGES = {
    EXPECTED_V8_SHA256: "V8",
    EXPECTED_V9_SHA256: "V9",
}
HANDLER_ENTRY = 0x0082C602
INDICATOR_ENGINE = 0x00829B86
TOUCH_REPEAT_ADDRESS = 0x0082C604
EXPECTED_REPEAT_INSTRUCTION = bytes.fromhex("03 23")  # movs r3, #3
OPTICAL_SENSOR_FUNCTIONS = {0x008350BE, 0x008350D8}


def _load_unicorn() -> tuple[Any, Any]:
    try:
        import unicorn
        from unicorn import arm_const
    except ImportError as error:  # pragma: no cover - CLI diagnostic
        raise RuntimeError(
            "Unicorn is required; install firmware_research/requirements-analysis.txt"
        ) from error
    return unicorn, arm_const


def validate_touch_indicator(image: bytes) -> dict[str, Any]:
    image_hash = hashlib.sha256(image).hexdigest()
    version = SUPPORTED_IMAGES.get(image_hash)
    if version is None:
        raise ValueError(f"v8/v9 SHA-256 mismatch: {image_hash}")
    repeat_offset = address_to_file_offset(TOUCH_REPEAT_ADDRESS, len(image))
    repeat_instruction = image[
        repeat_offset : repeat_offset + len(EXPECTED_REPEAT_INSTRUCTION)
    ]
    if repeat_instruction != EXPECTED_REPEAT_INSTRUCTION:
        raise ValueError(
            "touch repeat instruction mismatch: " + repeat_instruction.hex(" ")
        )

    unicorn, arm_const = _load_unicorn()
    machine = unicorn.Uc(
        unicorn.UC_ARCH_ARM,
        unicorn.UC_MODE_THUMB | unicorn.UC_MODE_MCLASS,
    )
    machine.mem_map(0x00800000, 0x00100000)
    machine.mem_write(APPLICATION_BASE, image[HEADER_SIZE:])
    visited: list[int] = []
    invocation: dict[str, int] = {}

    def reg(name: str) -> int:
        return machine.reg_read(getattr(arm_const, f"UC_ARM_REG_{name.upper()}"))

    def on_code(_uc: Any, address: int, _size: int, _user_data: Any) -> None:
        normalized = address & ~1
        visited.append(normalized)
        if normalized == INDICATOR_ENGINE:
            invocation.update({name: reg(name) for name in ("r0", "r1", "r2", "r3")})
            machine.emu_stop()

    machine.hook_add(unicorn.UC_HOOK_CODE, on_code)
    machine.emu_start(HANDLER_ENTRY | 1, 0, count=16)
    if not invocation:
        raise AssertionError("0x50 handler did not reach the stock indicator engine")
    expected = {"r0": 20, "r1": 1, "r2": 1, "r3": 3}
    if invocation != expected:
        raise AssertionError(f"unexpected indicator arguments: {invocation}")
    optical_reached = sorted(set(visited) & OPTICAL_SENSOR_FUNCTIONS)
    if optical_reached:
        raise AssertionError(f"optical sensor function reached: {optical_reached}")

    return {
        "classification": f"INSTRUCTION_LEVEL_{version}_TOUCH_INDICATOR_VALIDATION",
        "firmware_profile": version.lower(),
        "image_sha256": image_hash,
        "handler_entry": f"0x{HANDLER_ENTRY:08X}",
        "indicator_engine": f"0x{INDICATOR_ENGINE:08X}",
        "repeat_instruction_address": f"0x{TOUCH_REPEAT_ADDRESS:08X}",
        "repeat_instruction": repeat_instruction.hex(" "),
        "captured_arguments": invocation,
        "repeat_count": invocation["r3"],
        "stock_touch_indicator_path_reached": True,
        "optical_sensor_function_reached": False,
        "flash_allowed": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    try:
        report = validate_touch_indicator(load_image(args.image))
    except (AssertionError, OSError, RuntimeError, ValueError) as error:
        parser.error(str(error))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
