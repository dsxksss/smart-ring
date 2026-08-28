#!/usr/bin/env python3
"""Locate RT08 CAP_Touch entry points and state references without modifying an image."""

from __future__ import annotations

import argparse
import json
import struct
from collections import Counter
from pathlib import Path
from typing import Any

from analyze_rt08_thumb import (
    APPLICATION_BASE,
    HEADER_SIZE,
    address_to_file_offset,
    decode_thumb_bl,
    disassemble,
    file_offset_to_address,
    load_image,
)


TOUCH_MARKER = b"RT08_TP_FILE_0x1FD4_250106_01"
DEFAULT_TOUCH_START = 0x0082_F3B0
DEFAULT_TOUCH_END = 0x0083_0000
RTL8762E_RAM_START = 0x0020_0000
RTL8762E_RAM_END = 0x0022_0000
TOUCH_SNAPSHOT_FUNCTION = 0x0083_4A16
TOUCH_SNAPSHOT_CALLER = 0x0082_7EE6
TOUCH_SNAPSHOT_REGISTERS = (0x61, 0x65, 0x69, 0x6D)
TOUCH_STATE_ADDRESS = 0x0020_C1E8


def scan_bl_edges(data: bytes) -> list[tuple[int, int]]:
    edges: list[tuple[int, int]] = []
    for offset in range(HEADER_SIZE, len(data) - 3, 2):
        first, second = struct.unpack_from("<HH", data, offset)
        caller = file_offset_to_address(offset, len(data))
        target = decode_thumb_bl(caller, first, second)
        if target is not None:
            edges.append((caller, target))
    return edges


def scan_thumb_pointers(
    data: bytes, start: int, end: int
) -> list[dict[str, int]]:
    records: list[dict[str, int]] = []
    for offset in range(HEADER_SIZE, len(data) - 3, 4):
        value = struct.unpack_from("<I", data, offset)[0]
        normalized = value & ~1
        if value & 1 and start <= normalized < end:
            records.append(
                {
                    "pointer_file_offset": offset,
                    "pointer_address": file_offset_to_address(offset, len(data)),
                    "target": normalized,
                }
            )
    return records


def scan_literal_loads(
    data: bytes, start: int, end: int
) -> list[dict[str, int]]:
    records: list[dict[str, int]] = []
    start_offset = address_to_file_offset(start, len(data))
    end_offset = address_to_file_offset(end - 1, len(data)) + 1
    for offset in range(start_offset, end_offset - 1, 2):
        halfword = struct.unpack_from("<H", data, offset)[0]
        if halfword & 0xF800 != 0x4800:
            continue
        address = file_offset_to_address(offset, len(data))
        register = (halfword >> 8) & 7
        literal_address = ((address + 4) & ~3) + (halfword & 0xFF) * 4
        try:
            literal_offset = address_to_file_offset(literal_address, len(data))
        except ValueError:
            continue
        if literal_offset + 4 > len(data):
            continue
        value = struct.unpack_from("<I", data, literal_offset)[0]
        if RTL8762E_RAM_START <= value < RTL8762E_RAM_END:
            kind = 1
        elif APPLICATION_BASE <= (value & ~1) < APPLICATION_BASE + len(data) - HEADER_SIZE:
            kind = 2
        else:
            continue
        records.append(
            {
                "instruction": address,
                "register": register,
                "literal_address": literal_address,
                "value": value,
                "kind": kind,
            }
        )
    return records


def exact_bytes(data: bytes, address: int, size: int) -> str:
    offset = address_to_file_offset(address, len(data))
    return data[offset : offset + size].hex(" ")


def verify_touch_snapshot_path(
    data: bytes, edges: list[tuple[int, int]]
) -> dict[str, Any]:
    callers = [caller for caller, target in edges if target == TOUCH_SNAPSHOT_FUNCTION]
    if callers != [TOUCH_SNAPSHOT_CALLER]:
        raise ValueError(
            "unexpected four-channel touch snapshot callers: "
            + ", ".join(f"0x{caller:08X}" for caller in callers)
        )

    register_sites = (0x0083_4A32, 0x0083_4A3C, 0x0083_4A46, 0x0083_4A50)
    observed_registers: list[int] = []
    for site in register_sites:
        offset = address_to_file_offset(site, len(data))
        instruction = struct.unpack_from("<H", data, offset)[0]
        if instruction & 0xF800 != 0x2000:
            raise ValueError(f"expected MOVS immediate at 0x{site:08X}")
        observed_registers.append(instruction & 0xFF)
    if tuple(observed_registers) != TOUCH_SNAPSHOT_REGISTERS:
        raise ValueError(f"unexpected touch snapshot registers: {observed_registers}")

    return {
        "function": TOUCH_SNAPSHOT_FUNCTION,
        "only_caller": TOUCH_SNAPSHOT_CALLER,
        "controller_state": TOUCH_STATE_ADDRESS,
        "registers": observed_registers,
        "values": "four big-endian uint16 values",
        "response": {
            "header": [0xA1, 0x04],
            "channel_byte_ranges": [[2, 3], [4, 5], [6, 7], [8, 9]],
            "validity_flag_index": 10,
            "checksum_index": 15,
        },
        "query": {
            "payload": [0xA1, 0x03],
            "mode": "one-shot common diagnostic snapshot",
            "does_not_equal_optical_stream_start": [0xA1, 0x04, 0x04],
        },
        "anchors": {
            "function_prefix": exact_bytes(data, TOUCH_SNAPSHOT_FUNCTION, 16),
            "caller": exact_bytes(data, TOUCH_SNAPSHOT_CALLER, 4),
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Read-only CAP_Touch entry/state analysis for the exact RT08 stock image"
    )
    parser.add_argument("image", type=Path)
    parser.add_argument("--engine-path", type=Path)
    parser.add_argument("--start", type=lambda value: int(value, 0), default=DEFAULT_TOUCH_START)
    parser.add_argument("--end", type=lambda value: int(value, 0), default=DEFAULT_TOUCH_END)
    parser.add_argument("--entry-bytes", type=lambda value: int(value, 0), default=0x80)
    args = parser.parse_args()

    data = load_image(args.image)
    marker_offset = data.find(TOUCH_MARKER)
    if marker_offset < HEADER_SIZE:
        raise ValueError("exact RT08 touch file marker is missing")

    edges = scan_bl_edges(data)
    inbound = [
        {"caller": caller, "target": target}
        for caller, target in edges
        if args.start <= target < args.end and not args.start <= caller < args.end
    ]
    internal_targets = Counter(
        target
        for caller, target in edges
        if args.start <= caller < args.end and args.start <= target < args.end
    )
    pointers = scan_thumb_pointers(data, args.start, args.end)
    literals = scan_literal_loads(data, args.start, args.end)
    ram_counts = Counter(
        item["value"] for item in literals if item["kind"] == 1
    )

    entry_addresses = sorted(
        {item["target"] for item in inbound}
        | {item["target"] for item in pointers}
    )
    report: dict[str, Any] = {
        "image": str(args.image.resolve()),
        "touch_marker": {
            "file_offset": marker_offset,
            "address": file_offset_to_address(marker_offset, len(data)),
            "text": TOUCH_MARKER.decode("ascii"),
        },
        "review_window": {"start": args.start, "end": args.end},
        "inbound_bl_edges": inbound,
        "thumb_pointer_references": pointers,
        "internal_call_targets": [
            {"target": target, "count": count}
            for target, count in internal_targets.most_common()
        ],
        "ram_literal_counts": [
            {"address": address, "count": count}
            for address, count in ram_counts.most_common()
        ],
        "literal_loads": literals,
        "verified_touch_snapshot": verify_touch_snapshot_path(data, edges),
        "entry_disassembly": {
            f"0x{address:08x}": disassemble(
                data, address, args.entry_bytes, args.engine_path
            )
            for address in entry_addresses
        },
    }
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print(f"error: {error}")
        raise SystemExit(2)
