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
        "format": "colmi-0x50-sum32-candidate",
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


def inspect_image(path: Path) -> dict[str, Any]:
    data = path.read_bytes()
    strings = printable_strings(data)
    hardware_strings = [value for value in strings if "RT08" in value or "RY08" in value]
    exact_hardware = any(EXPECTED_HARDWARE in value for value in hardware_strings)
    firmware_string_present = any(
        EXPECTED_FIRMWARE_PREFIX in value for value in hardware_strings
    )
    container = inspect_known_container(data)
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
    if not result["arm_vector_candidates"]:
        reasons.append("no plausible Cortex-M vector table at common offsets")
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
