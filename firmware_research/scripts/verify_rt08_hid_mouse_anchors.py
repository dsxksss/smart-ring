#!/usr/bin/env python3
"""Hash-lock the stock R08 HID report helpers before mouse-only suppression.

This analysis is offline.  It proves that three adjacent helpers send HID
attribute index 4 (mouse), while the following two helpers use index 0x18 and
are deliberately left unchanged by the v9 candidate builder.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
from pathlib import Path
from typing import Any

from analyze_rt08_thumb import address_to_file_offset, decode_thumb_bl, load_image


EXPECTED_STOCK_SHA256 = (
    "c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b"
)
SERVER_SEND_DATA = 0x0083D7B2
HELPERS = (
    (
        "hid_mouse_motion_report",
        0x00829F74,
        0x00829FAA,
        0x00829FA0,
        4,
        "1f b5 3f 4c 24 78 00 2c 14 d0 6c 46 20 72 61 72 08 12 a0 72 "
        "e2 72 10 12 20 73 63 73 01 21 06 20 01 91 00 90 37 48 38 49 "
        "80 7c 02 ab 04 22 09 78 13 f0 05 fc 1f bd",
    ),
    (
        "hid_mouse_release_report",
        0x00829FAA,
        0x00829FD4,
        0x00829FCA,
        4,
        "1f b5 31 48 00 78 00 28 0e d0 00 20 02 90 03 90 01 21 06 20 "
        "01 91 00 90 2d 48 2d 49 80 7c 02 ab 04 22 09 78 13 f0 f0 fb "
        "1f bd",
    ),
    (
        "hid_mouse_extended_report",
        0x00829FD4,
        0x0082A022,
        0x0082A018,
        4,
        "1f b5 27 4b 1b 78 00 2b 20 d0 00 28 01 d0 07 23 00 e0 00 23 "
        "6c 46 23 72 06 23 63 72 a1 72 09 12 e1 72 11 12 22 73 61 73 "
        "00 28 00 d0 01 20 a0 73 00 20 e0 73 01 21 08 20 01 91 00 90 "
        "19 48 1a 49 80 7c 02 ab 04 22 09 78 13 f0 c9 fb 1f bd",
    ),
    (
        "hid_keyboard_press_report",
        0x0082A022,
        0x0082A04C,
        0x0082A042,
        0x18,
        "0e b5 16 49 09 78 00 29 0e d0 01 22 11 46 81 40 03 20 01 92 "
        "02 91 00 90 0f 48 0f 49 80 7c 02 ab 18 22 09 78 13 f0 b4 fb "
        "0e bd",
    ),
    (
        "hid_keyboard_release_report",
        0x0082A04C,
        0x0082A074,
        0x0082A06A,
        0x18,
        "0e b5 0c 48 00 78 00 28 0d d0 00 20 02 90 01 21 03 20 01 91 "
        "00 90 05 48 05 49 80 7c 02 ab 18 22 09 78 13 f0 a0 fb 0e bd",
    ),
)


def verify_hid_helpers(stock: bytes) -> dict[str, Any]:
    digest = hashlib.sha256(stock).hexdigest()
    if digest != EXPECTED_STOCK_SHA256:
        raise ValueError(f"stock SHA-256 mismatch: {digest}")

    records = []
    for name, start, end, index_instruction, expected_index, expected_hex in HELPERS:
        expected = bytes.fromhex(expected_hex)
        start_offset = address_to_file_offset(start, len(stock))
        end_offset = address_to_file_offset(end, len(stock))
        actual = stock[start_offset:end_offset]
        if actual != expected:
            raise ValueError(f"{name} bytes differ at 0x{start:08X}")

        index_offset = address_to_file_offset(index_instruction, len(stock))
        index_halfword = struct.unpack_from("<H", stock, index_offset)[0]
        if index_halfword != 0x2200 | expected_index:
            raise ValueError(f"{name} no longer loads HID index {expected_index:#x}")

        call_address = end - 6
        call_offset = address_to_file_offset(call_address, len(stock))
        first, second = struct.unpack_from("<HH", stock, call_offset)
        call_target = decode_thumb_bl(call_address, first, second)
        if call_target != SERVER_SEND_DATA:
            raise ValueError(f"{name} no longer calls server_send_data")
        records.append(
            {
                "name": name,
                "start": f"0x{start:08X}",
                "end": f"0x{end:08X}",
                "hid_attribute_index": expected_index,
                "server_send_data": f"0x{call_target:08X}",
                "bytes_sha256": hashlib.sha256(actual).hexdigest(),
            }
        )

    return {
        "classification": "READ_ONLY_STOCK_HID_REPORT_HELPERS",
        "stock_sha256": digest,
        "mouse_helpers": records[:3],
        "preserved_keyboard_helpers": records[3:],
        "mouse_attribute_index": 4,
        "keyboard_attribute_index": 0x18,
        "mouse_only_suppression_boundary_proven": True,
        "flash_authorized": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    try:
        report = verify_hid_helpers(load_image(args.image))
    except (OSError, ValueError) as error:
        parser.error(str(error))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
