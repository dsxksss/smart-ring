#!/usr/bin/env python3
"""Execute the v9 mouse-helper entries and prove they return without HID I/O."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any

from analyze_rt08_thumb import APPLICATION_BASE, HEADER_SIZE, address_to_file_offset, load_image
from verify_rt08_hid_mouse_anchors import HELPERS, SERVER_SEND_DATA


EXPECTED_V9_SHA256 = (
    "681dbb3e7a9112fc85b1d8e546717eb5052ae7a7138b117b6dfff75de7eba1f5"
)
RETURN_SENTINEL = 0x008FF000
MOUSE_HELPER_COUNT = 3
BLOCK_INSTRUCTION = bytes.fromhex("70 47")  # bx lr
V9_MARKER_ADDRESS = 0x008280CA
V9_MARKER = bytes.fromhex("fc 21")
TOUCH_REPEAT_ADDRESS = 0x0082C604
TOUCH_REPEAT = bytes.fromhex("03 23")


def _load_unicorn() -> tuple[Any, Any]:
    try:
        import unicorn
        from unicorn import arm_const
    except ImportError as error:  # pragma: no cover - CLI diagnostic
        raise RuntimeError(
            "Unicorn is required; install firmware_research/requirements-analysis.txt"
        ) from error
    return unicorn, arm_const


def validate_hid_mouse_block(image: bytes) -> dict[str, Any]:
    image_hash = hashlib.sha256(image).hexdigest()
    if image_hash != EXPECTED_V9_SHA256:
        raise ValueError(f"v9 SHA-256 mismatch: {image_hash}")

    marker_offset = address_to_file_offset(V9_MARKER_ADDRESS, len(image))
    if image[marker_offset : marker_offset + 2] != V9_MARKER:
        raise ValueError("v9 activation marker mismatch")
    repeat_offset = address_to_file_offset(TOUCH_REPEAT_ADDRESS, len(image))
    if image[repeat_offset : repeat_offset + 2] != TOUCH_REPEAT:
        raise ValueError("v9 touch indicator repeat mismatch")

    helper_records = []
    for index, (name, start, end, _index_instruction, hid_index, expected_hex) in enumerate(
        HELPERS
    ):
        start_offset = address_to_file_offset(start, len(image))
        end_offset = address_to_file_offset(end, len(image))
        actual = image[start_offset:end_offset]
        expected = bytes.fromhex(expected_hex)
        if index < MOUSE_HELPER_COUNT:
            expected = BLOCK_INSTRUCTION + expected[len(BLOCK_INSTRUCTION) :]
            state = "blocked"
        else:
            state = "preserved"
        if actual != expected:
            raise ValueError(f"v9 {name} bytes differ")
        helper_records.append(
            {
                "name": name,
                "entry": f"0x{start:08X}",
                "hid_attribute_index": hid_index,
                "state": state,
                "bytes_sha256": hashlib.sha256(actual).hexdigest(),
            }
        )

    unicorn, arm_const = _load_unicorn()
    executions = []
    for name, start, *_rest in HELPERS[:MOUSE_HELPER_COUNT]:
        machine = unicorn.Uc(
            unicorn.UC_ARCH_ARM,
            unicorn.UC_MODE_THUMB | unicorn.UC_MODE_MCLASS,
        )
        machine.mem_map(0x00800000, 0x00100000)
        machine.mem_write(APPLICATION_BASE, image[HEADER_SIZE:])
        machine.mem_write(RETURN_SENTINEL, b"\x00\xbf")
        visited: list[int] = []

        def on_code(_uc: Any, address: int, _size: int, _user_data: Any) -> None:
            normalized = address & ~1
            visited.append(normalized)
            if normalized == SERVER_SEND_DATA:
                raise AssertionError(f"{name} reached server_send_data")
            if normalized == RETURN_SENTINEL:
                machine.emu_stop()

        machine.hook_add(unicorn.UC_HOOK_CODE, on_code)
        machine.reg_write(arm_const.UC_ARM_REG_LR, RETURN_SENTINEL | 1)
        machine.emu_start(start | 1, 0, count=4)
        if visited != [start, RETURN_SENTINEL]:
            raise AssertionError(f"unexpected {name} execution path: {visited}")
        executions.append(
            {
                "name": name,
                "visited": [f"0x{address:08X}" for address in visited],
                "server_send_data_reached": False,
            }
        )

    return {
        "classification": "INSTRUCTION_LEVEL_V9_HID_MOUSE_BLOCK_VALIDATION",
        "image_sha256": image_hash,
        "activation_marker": "0xFC",
        "touch_indicator_repeat": 3,
        "mouse_helpers": helper_records[:MOUSE_HELPER_COUNT],
        "preserved_keyboard_helpers": helper_records[MOUSE_HELPER_COUNT:],
        "executions": executions,
        "mouse_server_send_data_reached": False,
        "keyboard_helpers_untouched": True,
        "flash_allowed": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    try:
        report = validate_hid_mouse_block(load_image(args.image))
    except (AssertionError, OSError, RuntimeError, ValueError) as error:
        parser.error(str(error))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
