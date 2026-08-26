#!/usr/bin/env python3
"""Read-only first-pass inspection for candidate R08 firmware images."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import struct
from pathlib import Path
from typing import Any


EXPECTED_HARDWARE = "RT08_V3.1"
EXPECTED_FIRMWARE_PREFIX = "RT08_"
KNOWN_CONTAINER_MAGIC = bytes.fromhex("e5 c3 bd 81")
KNOWN_HEADER_SIZE = 0x50
REALTEK_IMAGE_HEADER_SIZE = 0x400
RTL8762E_IC_TYPE = 12
RTL8762E_APPLICATION_BASE = 0x00826000
# T_IMG_HEADER_FORMAT is 1024 bytes and ends with sha256[32], rsvd2[76].
RTL8762E_SHA256_OFFSET = REALTEK_IMAGE_HEADER_SIZE - 76 - 32
RTL8762E_SOURCE_MARKERS = (
    b"qc_code\\app_module\\gsensor\\lis3dh_spi.c",
    b"Error! Please implement your ISR Handler",
)


def shannon_entropy(data: bytes) -> float:
    if not data:
        return 0.0
    counts = [0] * 256
    for value in data:
        counts[value] += 1
    length = len(data)
    return -sum(
        (count / length) * math.log2(count / length)
        for count in counts
        if count
    )


def printable_strings(data: bytes, minimum: int = 4) -> list[str]:
    pattern = rb"[\x20-\x7e]{" + str(minimum).encode("ascii") + rb",}"
    return [match.decode("ascii") for match in re.findall(pattern, data)]


def vector_candidates(data: bytes) -> list[dict[str, int]]:
    candidates: list[dict[str, int]] = []
    offsets = {0, 0x50, 0x80, 0x100, 0x200, 0x400}
    for offset in sorted(offsets):
        if offset + 8 > len(data):
            continue
        stack_pointer, reset_handler = struct.unpack_from("<II", data, offset)
        stack_in_sram = 0x20000000 <= stack_pointer < 0x21000000
        thumb_handler = reset_handler & 1 == 1
        if stack_in_sram and thumb_handler:
            candidates.append(
                {
                    "offset": offset,
                    "initial_stack_pointer": stack_pointer,
                    "reset_handler": reset_handler,
                }
            )
    return candidates


def inspect_known_container(data: bytes) -> dict[str, Any] | None:
    if len(data) < KNOWN_HEADER_SIZE or data[:4] != KNOWN_CONTAINER_MAGIC:
        return None
    length_a, length_b, stored_sum32 = struct.unpack_from("<III", data, 4)
    payload = data[KNOWN_HEADER_SIZE:]
    calculated_sum32 = sum(payload) & 0xFFFFFFFF
    return {
        "format": "qring-0x50-sum32",
        "header_size": KNOWN_HEADER_SIZE,
        "length_a": length_a,
        "length_b": length_b,
        "actual_payload_length": len(payload),
        "lengths_match": length_a == length_b == len(payload),
        "stored_sum32": stored_sum32,
        "calculated_sum32": calculated_sum32,
        "sum32_matches": stored_sum32 == calculated_sum32,
        "header_strings": printable_strings(data[:KNOWN_HEADER_SIZE]),
    }


def inspect_rtl8762e_application(data: bytes) -> dict[str, Any]:
    payload = data[KNOWN_HEADER_SIZE:]
    if len(payload) < REALTEK_IMAGE_HEADER_SIZE:
        return {"candidate": False, "reason": "payload is shorter than 1024-byte image header"}

    ic_type, secure_version, ctrl_flags, image_id, crc16, payload_length = (
        struct.unpack_from("<BBHHHI", payload, 0)
    )
    uuid = payload[0x0C:0x1C]
    exe_base, load_base, load_length = struct.unpack_from("<III", payload, 0x1C)
    # This word is documented as reserved in the public SDK guide, but the stock
    # R08 image stores 0x00826000 here.  The following executable bytes begin at
    # +0x400 and contain a Thumb veneer to 0x00826665, so treat it as an image-base
    # candidate while retaining the distinction from an officially named field.
    image_base_candidate = struct.unpack_from("<I", payload, 0x28)[0]
    application_end = image_base_candidate + len(payload)
    body = payload[REALTEK_IMAGE_HEADER_SIZE:]
    stored_sha256 = payload[
        RTL8762E_SHA256_OFFSET : RTL8762E_SHA256_OFFSET + hashlib.sha256().digest_size
    ]
    calculated_body_sha256 = hashlib.sha256(body).digest()
    thumb_entry_pointers: list[int] = []
    for offset in range(0, len(data) - 3, 4):
        value = struct.unpack_from("<I", data, offset)[0]
        if (
            value & 1
            and image_base_candidate <= (value & ~1) < application_end
        ):
            thumb_entry_pointers.append(value)
    source_markers = [
        marker.decode("ascii") for marker in RTL8762E_SOURCE_MARKERS if marker in data
    ]
    header_consistent = (
        ic_type == RTL8762E_IC_TYPE
        and payload_length == len(body)
        and image_base_candidate == RTL8762E_APPLICATION_BASE
        and exe_base == image_base_candidate + REALTEK_IMAGE_HEADER_SIZE
    )
    candidate = header_consistent and len(thumb_entry_pointers) >= 20 and bool(source_markers)
    return {
        "candidate": candidate,
        "ic_type": ic_type,
        "ic_type_matches_rtl8762e": ic_type == RTL8762E_IC_TYPE,
        "secure_version": secure_version,
        "ctrl_flags": ctrl_flags,
        "ctrl_flag_bits": {
            "xip": bool(ctrl_flags & (1 << 0)),
            "encrypted": bool(ctrl_flags & (1 << 1)),
            "load_when_boot": bool(ctrl_flags & (1 << 2)),
            "encrypted_load": bool(ctrl_flags & (1 << 3)),
            "not_ready": bool(ctrl_flags & (1 << 7)),
            "not_obsolete": bool(ctrl_flags & (1 << 8)),
            "integrity_check_en_in_boot": bool(ctrl_flags & (1 << 9)),
        },
        "image_id": image_id,
        "crc16": crc16,
        "payload_length": payload_length,
        "actual_body_length": len(body),
        "payload_length_matches": payload_length == len(body),
        "uuid": uuid.hex(),
        "exe_base": exe_base,
        "load_base": load_base,
        "load_length": load_length,
        "application_base_candidate": image_base_candidate,
        "application_end_candidate": application_end,
        "body_start_matches_exe_base": (
            exe_base == image_base_candidate + REALTEK_IMAGE_HEADER_SIZE
        ),
        "stored_sha256": stored_sha256.hex(),
        "calculated_body_sha256": calculated_body_sha256.hex(),
        "body_sha256_matches": stored_sha256 == calculated_body_sha256,
        "thumb_entry_pointer_count": len(thumb_entry_pointers),
        "thumb_entry_pointer_examples": thumb_entry_pointers[:12],
        "source_markers": source_markers,
        "standard_vector_table_expected": False if candidate else None,
        "sha256_all_zero": not any(stored_sha256),
        "note": (
            "The outer QRing wrapper contains an RTL8762E-style 1024-byte application "
            "header. The official trailing SHA-256 field is at header offset 0x394; "
            "this stock image leaves it zero while boot integrity checking is disabled."
        ),
    }


def inspect_image(path: Path) -> dict[str, Any]:
    data = path.read_bytes()
    strings = printable_strings(data)
    hardware_strings = [value for value in strings if "RT08" in value or "RY08" in value]
    exact_hardware = any(EXPECTED_HARDWARE in value for value in hardware_strings)
    firmware_string_present = any(
        EXPECTED_FIRMWARE_PREFIX in value for value in hardware_strings
    )
    container = inspect_known_container(data)
    rtl8762e_application = inspect_rtl8762e_application(data)
    result: dict[str, Any] = {
        "path": str(path.resolve()),
        "size": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
        "entropy_bits_per_byte": round(shannon_entropy(data), 4),
        "hardware_strings": hardware_strings,
        "expected_hardware": EXPECTED_HARDWARE,
        "exact_hardware_string_found": exact_hardware,
        "firmware_string_found": firmware_string_present,
        "known_container": container,
        "arm_vector_candidates": vector_candidates(data),
        "rtl8762e_application": rtl8762e_application,
    }
    reasons: list[str] = []
    if not exact_hardware:
        reasons.append(f"missing exact hardware marker {EXPECTED_HARDWARE}")
    if not firmware_string_present:
        reasons.append("missing RT08 firmware version marker")
    if container is None:
        reasons.append("container format is not yet recognized")
    elif not container["lengths_match"] or not container["sum32_matches"]:
        reasons.append("container length or checksum validation failed")
    if not result["arm_vector_candidates"] and not rtl8762e_application["candidate"]:
        reasons.append(
            "neither a Cortex-M vector table nor a consistent RTL8762E application header was recognized"
        )
    result["offline_patch_candidate"] = not reasons
    result["rejection_reasons"] = reasons
    result["flash_authorized"] = False
    result["safety_note"] = (
        "Inspection is read-only. A valid container is not authorization to flash; "
        "MCU, memory map, signature policy, stock backup, and independent recovery "
        "must still be verified."
    )
    return result


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Read-only inspection of a candidate RT08_V3.1 firmware image"
    )
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    if not args.image.is_file():
        parser.error(f"file not found: {args.image}")
    print(json.dumps(inspect_image(args.image), ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
