#!/usr/bin/env python3
"""Lock the stock RT08 OTA activation call chain to exact image bytes.

This is deliberately a read-only evidence report.  It proves which calls are
present in the downloaded application image; it does not name undocumented ROM
APIs or claim that a staged candidate will boot or roll back safely.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
from pathlib import Path
from typing import Any

from analyze_rt08_thumb import address_to_file_offset, load_image
from inspect_r08_image import (
    KNOWN_HEADER_SIZE,
    REALTEK_IMAGE_HEADER_SIZE,
    RTL8762E_SHA256_OFFSET,
)


EXPECTED_STOCK_SHA256 = (
    "c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b"
)
APP_IMAGE_ID = 0x2793
EXPECTED_CTRL_FLAGS = 0x0981
EXPECTED_STOCK_IMAGE_DIGEST = bytes.fromhex(
    "3e143d383a69b749ed928345ac04d517d7aefb95ecc0f2f4eafbe9fd9b146f8f"
)
RAW_VERSION_FORMAT = bytes.fromhex("41 10 00 00 9e a3 01 12")

# Each anchor is an independently checked instruction sequence from the exact
# stock image. Hash-locked RTL8762E SDK v1.5.0 evidence names the ROM targets.
ANCHORS = (
    (
        "ota_end_passes_app_image_id_to_activation",
        0x0082F182,
        "05 21 41 70 00 21 2f 48 f7 f7 ce fe",
    ),
    (
        "activation_preserves_second_argument_as_optional_offset",
        0x00826F2A,
        "fe b5 0f 46 00 25 62 49 2e 46 88 42 06 d1",
    ),
    (
        "activation_special_id_or_rom_lookup",
        0x00826F38,
        "05 20 e1 f7 bd d8 01 21 09 06 08 43 01 e0 e1 f7 25 de",
    ),
    (
        "activation_rom_gate_8b7a",
        0x00826F4A,
        "04 00 04 d0 e1 f7 14 de 00 28 02 d0 02 e0",
    ),
    (
        "activation_rom_gate_8a5c",
        0x00826F5C,
        "e4 19 20 46 e1 f7 7c dd 06 00 0d d0",
    ),
    (
        "activation_calls_commit_wrapper",
        0x00826F68,
        "20 46 ff f7 d4 ff",
    ),
    (
        "commit_wrapper_calls_rom_3ed1a",
        0x00826F16,
        "10 b5 04 46 ff f7 e1 fe 20 46 17 f4 fb f6 ff f7 f5 fe 10 bd",
    ),
)


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
                f"activation anchor {name} mismatch at 0x{address:08x}: "
                f"{actual.hex(' ')}"
            )
        checked.append(
            {
                "name": name,
                "address": f"0x{address:08X}",
                "bytes": expected.hex(" "),
            }
        )

    payload = data[KNOWN_HEADER_SIZE:]
    if len(payload) < REALTEK_IMAGE_HEADER_SIZE:
        raise ValueError("image payload is shorter than the RTL8762E header")
    ctrl_flags = struct.unpack_from("<H", payload, 2)[0]
    image_id = struct.unpack_from("<H", payload, 4)[0]
    raw_version = payload[0x60:0x68]
    git_version, git_commit_id = struct.unpack("<II", raw_version)
    stored_sha256 = payload[
        RTL8762E_SHA256_OFFSET : RTL8762E_SHA256_OFFSET + 32
    ]
    if ctrl_flags != EXPECTED_CTRL_FLAGS:
        raise ValueError(f"unexpected control flags 0x{ctrl_flags:04x}")
    if image_id != APP_IMAGE_ID:
        raise ValueError(f"unexpected app image id 0x{image_id:04x}")
    if raw_version != RAW_VERSION_FORMAT:
        raise ValueError(f"unexpected raw version bytes: {raw_version.hex(' ')}")
    if stored_sha256 != EXPECTED_STOCK_IMAGE_DIGEST:
        raise ValueError("stock SDK-generated image digest mismatch")

    return {
        "classification": "READ_ONLY_STATIC_ACTIVATION_PATH",
        "stock_sha256": digest,
        "app_image_id": f"0x{image_id:04X}",
        "control_flags": f"0x{ctrl_flags:04X}",
        "integrity_check_en_in_boot": bool(ctrl_flags & (1 << 9)),
        "stored_sha256_all_zero": not any(stored_sha256),
        "stored_sha256": stored_sha256.hex(),
        "stored_sha256_offset": f"0x{RTL8762E_SHA256_OFFSET:03X}",
        "raw_version_format_bytes": raw_version.hex(" "),
        "raw_version_semantics_proven": True,
        "application_git_version": {
            "raw": f"0x{git_version:08X}",
            "major": git_version & 0xF,
            "minor": (git_version >> 4) & 0xFF,
            "revision": (git_version >> 12) & 0x7FFF,
            "reserve": (git_version >> 27) & 0x1F,
            "commit_id": f"0x{git_commit_id:08X}",
        },
        "downloaded_payload_type": "single_rtl8762e_application_image",
        "separate_ota_bank_header_present_in_downloaded_payload": False,
        "packaged_application_flags": {
            "not_ready": bool(ctrl_flags & (1 << 7)),
            "not_obsolete": bool(ctrl_flags & (1 << 8)),
        },
        "installed_application_flags_read_from_device": False,
        "ota_end_reaches_activation_wrapper": True,
        "activation_arguments": {
            "image_id": f"0x{image_id:04X}",
            "second_argument": 0,
        },
        "application_git_version_passed_as_activation_argument": False,
        "activation_address_dataflow": {
            "image_id_resolver_call": (
                "get_temp_ota_bank_addr_by_img_id (0x00008B94)"
            ),
            "resolver_result_saved_in": "r4",
            "second_argument_saved_in": "r7",
            "second_argument_conditionally_added_to_resolved_address": True,
            "bank_switch_query_call": (
                "is_ota_support_bank_switch (0x00008B7A)"
            ),
            "resolved_address_validation_call": (
                "check_image_chksum (0x00008A5C)"
            ),
            "commit_wrapper": "dfu_set_image_ready (0x00826F16)",
            "commit_rom_call": "dfu_set_ready (0x0003ED1A)",
            "validation_success_required_before_commit": True,
        },
        "rom_calls_observed": ["0x00008B94", "0x00008B7A", "0x00008A5C", "0x0003ED1A"],
        "activation_function": "dfu_check_checksum (0x00826F2A)",
        "rom_api_names_proven": True,
        "ota_bank_header_update_proven": False,
        "staged_application_flag_transition_proven": True,
        "application_flag_transition_proven": False,
        "equal_version_bank_selection_proven": False,
        "runtime_crash_rollback_proven": False,
        "power_loss_recovery_proven": False,
        "flash_authorized": False,
        "safety_note": (
            "The stock OTA End handler reaches the hash-locked SDK-named checksum "
            "and staged-image ready path. Bootloader copy completion, OTA-bank-header "
            "selection, installed application state, runtime-crash rollback, and an "
            "independent recovery path remain unproven."
        ),
        "anchors": checked,
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Read-only validation of the stock RT08 activation call chain"
    )
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    print(json.dumps(analyze(load_image(args.image)), ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as error:
        print(f"error: {error}")
        raise SystemExit(2)
