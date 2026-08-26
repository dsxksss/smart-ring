#!/usr/bin/env python3
"""Conservatively inventory padding runs in an RT08 RTL8762E OTA image.

This tool is read-only.  A run with no detected references is only a candidate;
it is not proof that the linker, bootloader, or runtime never uses the bytes.
"""

from __future__ import annotations

import argparse
import json
import struct
from pathlib import Path
from typing import Any

from analyze_rt08_thumb import (
    APPLICATION_BASE,
    HEADER_SIZE,
    REALTEK_IMAGE_HEADER_SIZE,
    decode_thumb_bl,
    file_offset_to_address,
    image_summary,
    load_image,
)


EXECUTABLE_FILE_OFFSET = HEADER_SIZE + REALTEK_IMAGE_HEADER_SIZE


def sign_extend(value: int, bits: int) -> int:
    sign = 1 << (bits - 1)
    return (value ^ sign) - sign


def decode_thumb16_branch(address: int, halfword: int) -> int | None:
    """Decode common 16-bit B/B<cond> immediates, excluding SVC/UDF data."""
    if halfword & 0xF800 == 0xE000:
        return (address + 4 + sign_extend((halfword & 0x7FF) << 1, 12)) & 0xFFFFFFFF
    if halfword & 0xF000 == 0xD000:
        condition = (halfword >> 8) & 0xF
        if condition < 0xE:
            return (
                address + 4 + sign_extend((halfword & 0xFF) << 1, 9)
            ) & 0xFFFFFFFF
    return None


def padding_runs(data: bytes, minimum: int) -> list[dict[str, int | str]]:
    runs: list[dict[str, int | str]] = []
    cursor = EXECUTABLE_FILE_OFFSET
    while cursor < len(data):
        fill = data[cursor]
        if fill not in (0x00, 0xFF):
            cursor += 1
            continue
        end = cursor + 1
        while end < len(data) and data[end] == fill:
            end += 1
        if end - cursor >= minimum:
            start_address = file_offset_to_address(cursor, len(data))
            end_address = file_offset_to_address(end - 1, len(data)) + 1
            runs.append(
                {
                    "fill": f"0x{fill:02x}",
                    "file_offset": cursor,
                    "file_end": end,
                    "address": start_address,
                    "address_end": end_address,
                    "length": end - cursor,
                }
            )
        cursor = end
    return runs


def reference_candidates(
    data: bytes, start_address: int, end_address: int
) -> dict[str, list[dict[str, int | str]]]:
    pointers: list[dict[str, int | str]] = []
    branches: list[dict[str, int | str]] = []

    for offset in range(EXECUTABLE_FILE_OFFSET, len(data) - 3, 4):
        value = struct.unpack_from("<I", data, offset)[0]
        normalized = value & ~1
        if start_address <= normalized < end_address:
            source = file_offset_to_address(offset, len(data))
            if not start_address <= source < end_address:
                pointers.append(
                    {
                        "source_address": source,
                        "source_file_offset": offset,
                        "value": value,
                    }
                )

    for offset in range(EXECUTABLE_FILE_OFFSET, len(data) - 1, 2):
        source = file_offset_to_address(offset, len(data))
        if start_address <= source < end_address:
            continue
        first = struct.unpack_from("<H", data, offset)[0]
        target = decode_thumb16_branch(source, first)
        size = 2
        if offset + 4 <= len(data):
            second = struct.unpack_from("<H", data, offset + 2)[0]
            bl_target = decode_thumb_bl(source, first, second)
            if bl_target is not None:
                target = bl_target
                size = 4
        if target is not None and start_address <= (target & ~1) < end_address:
            branches.append(
                {
                    "source_address": source,
                    "source_file_offset": offset,
                    "bytes": data[offset : offset + size].hex(" "),
                    "target": target & ~1,
                }
            )

    return {"pointer_candidates": pointers, "branch_candidates": branches}


def cave_report(data: bytes, minimum: int) -> list[dict[str, Any]]:
    report: list[dict[str, Any]] = []
    for run in padding_runs(data, minimum):
        references = reference_candidates(
            data, int(run["address"]), int(run["address_end"])
        )
        report.append(
            {
                **run,
                **references,
                "externally_unreferenced_candidate": not any(references.values()),
            }
        )
    return report


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Read-only conservative padding/code-cave inventory for RT08"
    )
    parser.add_argument("image", type=Path)
    parser.add_argument("--minimum", type=lambda value: int(value, 0), default=32)
    args = parser.parse_args()
    if args.minimum <= 0:
        raise ValueError("minimum run length must be positive")
    data = load_image(args.image)
    print(
        json.dumps(
            {
                "summary": image_summary(data),
                "warning": (
                    "No detected references does not prove a run is safe for executable code"
                ),
                "runs": cave_report(data, args.minimum),
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as error:
        print(f"error: {error}")
        raise SystemExit(2)
