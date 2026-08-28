from __future__ import annotations

import sys
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))

from analyze_rt08_thumb import APPLICATION_BASE, HEADER_SIZE, encode_thumb_bl  # noqa: E402
from analyze_rt08_touch_driver import (  # noqa: E402
    exact_bytes,
    scan_bl_edges,
    scan_thumb_pointers,
)


def synthetic_image() -> bytearray:
    return bytearray(HEADER_SIZE + 0x200)


def test_scans_thumb_call_edge() -> None:
    data = synthetic_image()
    caller = APPLICATION_BASE + 0x20
    target = APPLICATION_BASE + 0xA0
    data[HEADER_SIZE + 0x20 : HEADER_SIZE + 0x24] = encode_thumb_bl(caller, target)
    assert (caller, target) in scan_bl_edges(bytes(data))


def test_scans_aligned_thumb_pointer_and_exact_bytes() -> None:
    data = synthetic_image()
    target = APPLICATION_BASE + 0xA0
    data[HEADER_SIZE + 0x40 : HEADER_SIZE + 0x44] = (target | 1).to_bytes(4, "little")
    records = scan_thumb_pointers(bytes(data), target, target + 2)
    assert records == [
        {
            "pointer_file_offset": HEADER_SIZE + 0x40,
            "pointer_address": APPLICATION_BASE + 0x40,
            "target": target,
        }
    ]
    assert exact_bytes(bytes(data), APPLICATION_BASE + 0x40, 4) == "a1 00 82 00"
