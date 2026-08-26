#!/usr/bin/env python3
"""Validate the stock RT08 DFU staging path without sending any BLE command."""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
from pathlib import Path
from typing import Any

from analyze_rt08_thumb import address_to_file_offset, load_image


EXPECTED_STOCK_SHA256 = (
    "c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b"
)
QRING_HEADER_SIZE = 0x50
ACTIVE_APP_BASE = 0x00826000
INACTIVE_APP_BASE = 0x0084E000
MAX_STAGED_IMAGE_SIZE = 0x24000
INACTIVE_APP_END = INACTIVE_APP_BASE + MAX_STAGED_IMAGE_SIZE
ADJACENT_STORAGE_BASE = 0x00872000
ADJACENT_STORAGE_OBSERVED_SPAN = 0x2000
APP_IMAGE_ID = 0x2793
REALTEK_IMAGE_HEADER_SIZE = 0x400

# These anchors cover the destructive receiver path, so every conclusion is
# tied to the one exact stock image rather than inferred from the phone app.
ANCHORS = (
    ("init_accepts_9_bytes", 0x0082EF66, "09 29 01 d0"),
    ("init_accepts_type_1_or_4", 0x0082EF6E, "01 78 01 29 01 d0 04 29 10 d1"),
    ("init_subtracts_qring_overhead", 0x0082EF8C, "05 25 ed 02"),
    ("init_max_file_literal_load", 0x0082EF90, "a6 4e"),
    ("data_first_block_qring_header", 0x0082F0A6, "50 22"),
    ("data_skips_qring_header", 0x0082F114, "22 46 50 3a 39 46 14 46 50 31"),
    ("data_erases_crossed_4k_page", 0x0082F06E, "08 0b 00 03 80 19 fa f7 96 fa"),
    ("data_writes_inactive_slot", 0x0082F080, "80 19 22 46 1c 99 fa f7 e1 fa"),
    ("check_requires_length_minus_header", 0x0082F148, "81 68 c3 68 50 39 8b 42"),
    ("end_activates_app_image", 0x0082F182, "05 21 41 70 00 21 2f 48 f7 f7 ce fe"),
    (
        "adjacent_storage_erases_two_pages",
        0x00831B20,
        "10 b5 00 24 e0 b2 00 f0 57 f9 64 1c 02 2c f9 db",
    ),
    (
        "adjacent_storage_write_path",
        0x00831B86,
        "b8 4b 61 78 60 88 09 03 09 18 b4 48 80 22 c9 18 f7 f7 29 fd",
    ),
    (
        "adjacent_storage_page_address",
        0x00831DD8,
        "23 49 10 b5 00 03 40 18 f7 f7 e0 fb 10 bd",
    ),
    ("storage_descriptor_874000", 0x00847658, "00 40 87 00 00 20 00 00 00 10 00 02"),
    ("storage_descriptor_876800", 0x00847664, "00 68 87 00 00 08 00 00 00 04 80 00"),
    ("storage_descriptor_876000", 0x00847670, "00 60 87 00 00 08 00 00 00 04 80 00"),
    ("storage_descriptor_877000", 0x008476C0, "00 70 87 00 00 10 00 00 00 08 80 00"),
    ("storage_descriptor_878000", 0x008476CC, "00 80 87 00 00 20 00 00 00 10 00 02"),
    ("storage_descriptor_87b000", 0x00847704, "00 b0 87 00 00 10 00 00 00 08 00 01"),
    ("storage_descriptor_87c000", 0x00847710, "00 c0 87 00 00 10 00 00 00 08 00 01"),
    ("storage_descriptor_87e000", 0x0084771C, "00 e0 87 00 00 10 00 00 00 08 00 01"),
    ("storage_descriptor_87d000", 0x00846F88, "00 d0 87 00 00 10 00 00 00 08 00 01"),
    ("storage_descriptor_87f000", 0x00847010, "00 f0 87 00 00 10 00 00 00 08 00 01"),
)

STORAGE_DESCRIPTORS = (
    (0x00847658, 0x00874000, 0x2000),
    (0x00847670, 0x00876000, 0x0800),
    (0x00847664, 0x00876800, 0x0800),
    (0x008476C0, 0x00877000, 0x1000),
    (0x008476CC, 0x00878000, 0x2000),
    (0x00847704, 0x0087B000, 0x1000),
    (0x00847710, 0x0087C000, 0x1000),
    (0x00846F88, 0x0087D000, 0x1000),
    (0x0084771C, 0x0087E000, 0x1000),
    (0x00847010, 0x0087F000, 0x1000),
)


def _read_u32(data: bytes, address: int) -> int:
    return struct.unpack_from("<I", data, address_to_file_offset(address, len(data)))[0]


def analyze(data: bytes) -> dict[str, Any]:
    digest = hashlib.sha256(data).hexdigest()
    if digest != EXPECTED_STOCK_SHA256:
        raise ValueError(f"stock SHA-256 mismatch: {digest}")

    checked: list[dict[str, Any]] = []
    for name, address, expected_hex in ANCHORS:
        expected = bytes.fromhex(expected_hex)
        offset = address_to_file_offset(address, len(data))
        actual = data[offset : offset + len(expected)]
        if actual != expected:
            raise ValueError(
                f"OTA anchor {name} mismatch at 0x{address:08x}: {actual.hex(' ')}"
            )
        checked.append(
            {
                "name": name,
                "address": f"0x{address:08X}",
                "bytes": expected.hex(" "),
            }
        )

    # LDR literals used by the checked instructions above.
    inactive_base = _read_u32(data, 0x0082F230)
    max_delta_literal = _read_u32(data, 0x0082F22C)
    image_id = _read_u32(data, 0x0082F248)
    adjacent_storage_base = _read_u32(data, 0x00831E68)
    max_file_size = 0x2800 + max_delta_literal - 1
    max_staged_size = max_file_size - QRING_HEADER_SIZE
    if inactive_base != INACTIVE_APP_BASE:
        raise ValueError(f"unexpected inactive app base 0x{inactive_base:08x}")
    if max_staged_size != MAX_STAGED_IMAGE_SIZE:
        raise ValueError(f"unexpected inactive app capacity 0x{max_staged_size:x}")
    if image_id != APP_IMAGE_ID:
        raise ValueError(f"unexpected app image id 0x{image_id:04x}")
    if adjacent_storage_base != ADJACENT_STORAGE_BASE:
        raise ValueError(
            f"unexpected adjacent storage base 0x{adjacent_storage_base:08x}"
        )
    if adjacent_storage_base != INACTIVE_APP_END:
        raise ValueError("inactive app slot does not end at adjacent storage base")

    stock_staged_size = len(data) - QRING_HEADER_SIZE
    if stock_staged_size > max_staged_size:
        raise ValueError("stock image does not fit its own receiver's inactive slot")

    payload = data[QRING_HEADER_SIZE:]
    exe_base, load_base = struct.unpack_from("<II", payload, 0x1C)
    image_base_candidate = struct.unpack_from("<I", payload, 0x28)[0]
    expected_active_exe_base = ACTIVE_APP_BASE + REALTEK_IMAGE_HEADER_SIZE
    expected_staging_exe_base = INACTIVE_APP_BASE + REALTEK_IMAGE_HEADER_SIZE
    if image_base_candidate != ACTIVE_APP_BASE:
        raise ValueError(
            f"unexpected application base candidate 0x{image_base_candidate:08x}"
        )
    if (exe_base, load_base) != (expected_active_exe_base, expected_active_exe_base):
        raise ValueError(
            "stock application is not linked to the observed active application base"
        )

    storage_regions: list[dict[str, Any]] = []
    for descriptor_address, expected_base, expected_size in STORAGE_DESCRIPTORS:
        offset = address_to_file_offset(descriptor_address, len(data))
        region_base, region_size, raw_geometry = struct.unpack_from("<III", data, offset)
        if (region_base, region_size) != (expected_base, expected_size):
            raise ValueError(
                f"unexpected storage descriptor at 0x{descriptor_address:08x}: "
                f"base=0x{region_base:08x} size=0x{region_size:x}"
            )
        storage_regions.append(
            {
                "descriptor_address": f"0x{descriptor_address:08X}",
                "base": f"0x{region_base:08X}",
                "size": region_size,
                "end": f"0x{region_base + region_size:08X}",
                "raw_geometry_word": f"0x{raw_geometry:08X}",
                "semantic_name_proven": False,
            }
        )

    return {
        "classification": "READ_ONLY_STATIC_OTA_PATH",
        "stock_sha256": digest,
        "active_app_base": f"0x{ACTIVE_APP_BASE:08X}",
        "inactive_app_base": f"0x{inactive_base:08X}",
        "inactive_app_end": f"0x{INACTIVE_APP_END:08X}",
        "inactive_app_capacity": max_staged_size,
        "adjacent_storage_base": f"0x{adjacent_storage_base:08X}",
        "adjacent_storage_observed_minimum_span": ADJACENT_STORAGE_OBSERVED_SPAN,
        "adjacent_storage_classification": "application_persistent_storage_not_ota",
        "additional_application_storage_descriptors": storage_regions,
        "highest_observed_storage_end": "0x00880000",
        "unclassified_gap": {"base": "0x0087A000", "size": 0x1000},
        "physical_flash_capacity_proven": False,
        "stock_staged_size": stock_staged_size,
        "stock_remaining_bytes": max_staged_size - stock_staged_size,
        "qring_wrapper_bytes_skipped": QRING_HEADER_SIZE,
        "erase_granularity": 0x1000,
        "app_image_id": f"0x{image_id:04X}",
        "packaged_application_addresses": {
            "image_base_candidate": f"0x{image_base_candidate:08X}",
            "exe_base": f"0x{exe_base:08X}",
            "load_base": f"0x{load_base:08X}",
        },
        "active_slot_xip_address_compatible": exe_base == expected_active_exe_base,
        "staging_slot_xip_address_compatible": exe_base == expected_staging_exe_base,
        "separate_bank1_relocated_application_present": False,
        "separate_ota_header_present_in_package": False,
        "ota_layout_assessment": "SINGLE_BANK_COPY_IMAGE_CONSISTENT",
        "ota_layout_assessment_proven_by_runtime_or_rom_symbols": False,
        "address_remap_during_bank_switch_proven": False,
        "old_application_survives_activation_proven": False,
        "whole_file_crc_fields_stored_by_init": True,
        "whole_file_crc_fields_rechecked_in_visible_check_handler": False,
        "per_frame_crc16_is_checked_by_protocol": True,
        "bootloader_rollback_proven": False,
        "flash_authorized": False,
        "anchors": checked,
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Read-only validation of the stock RT08 OTA staging path"
    )
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    report = analyze(load_image(args.image))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as error:
        print(f"error: {error}")
        raise SystemExit(2)
