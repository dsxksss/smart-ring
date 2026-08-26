#!/usr/bin/env python3
"""Compare two offline R08 Flash readbacks without claiming recoverability."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any


FLASH_READ_ALIGNMENT = 0x1000


def parse_integer(value: str) -> int:
    try:
        parsed = int(value, 0)
    except ValueError as error:
        raise argparse.ArgumentTypeError(f"invalid integer: {value}") from error
    if parsed < 0:
        raise argparse.ArgumentTypeError("value must not be negative")
    return parsed


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def compare_readbacks(
    first_path: Path,
    second_path: Path,
    *,
    base_address: int,
    expected_length: int,
    region_name: str,
    encryption_status: str,
) -> dict[str, Any]:
    first_resolved = first_path.resolve()
    second_resolved = second_path.resolve()
    if first_resolved == second_resolved:
        raise ValueError("the two readbacks must be different files")
    if base_address % FLASH_READ_ALIGNMENT:
        raise ValueError("base address must be aligned to 0x1000")
    if expected_length <= 0 or expected_length % FLASH_READ_ALIGNMENT:
        raise ValueError("expected length must be a positive multiple of 0x1000")
    if encryption_status not in {"unknown", "encrypted", "plaintext"}:
        raise ValueError("invalid encryption status")
    if not region_name.strip():
        raise ValueError("region name must not be empty")

    first = first_path.read_bytes()
    second = second_path.read_bytes()
    lengths_match_expected = len(first) == len(second) == expected_length
    byte_equal = first == second
    differing_byte_count = 0
    first_difference_offset = None
    for offset, (left, right) in enumerate(zip(first, second)):
        if left == right:
            continue
        differing_byte_count += 1
        if first_difference_offset is None:
            first_difference_offset = offset
    if len(first) != len(second):
        if first_difference_offset is None:
            first_difference_offset = min(len(first), len(second))
        differing_byte_count += abs(len(first) - len(second))
    readback_repeatability_proven = lengths_match_expected and byte_equal

    return {
        "classification": "READBACK_REPEATABILITY_ONLY",
        "region": {
            "name": region_name,
            "base_address": f"0x{base_address:08X}",
            "expected_length": expected_length,
            "end_address": f"0x{base_address + expected_length:08X}",
        },
        "inputs": {
            "first": {
                "file_name": first_path.name,
                "length": len(first),
                "sha256": _sha256(first),
            },
            "second": {
                "file_name": second_path.name,
                "length": len(second),
                "sha256": _sha256(second),
            },
            "different_files": True,
        },
        "comparison": {
            "lengths_match_expected": lengths_match_expected,
            "byte_equal": byte_equal,
            "differing_byte_count": differing_byte_count,
            "first_difference_offset": first_difference_offset,
            "first_difference_address": (
                f"0x{base_address + first_difference_offset:08X}"
                if first_difference_offset is not None
                else None
            ),
        },
        "encryption_status": encryption_status,
        "readback_repeatability_proven": readback_repeatability_proven,
        "cold_boot_separation_attested": False,
        "complete_flash_map_proven": False,
        "independent_media_copies_verified": False,
        "writeback_semantics_proven": False,
        "restore_proven": False,
        "full_device_backup_proven": False,
        "flash_authorized": False,
        "safety_note": (
            "Identical offline files prove only repeatable bytes for this declared region. "
            "They do not prove cold-boot separation, complete coverage, plaintext semantics, "
            "same-chip writeback, independent recovery, or authorization to flash."
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Compare two offline RTL8762E readback files byte-for-byte"
    )
    parser.add_argument("first", type=Path)
    parser.add_argument("second", type=Path)
    parser.add_argument("--base-address", required=True, type=parse_integer)
    parser.add_argument("--expected-length", required=True, type=parse_integer)
    parser.add_argument("--region-name", required=True)
    parser.add_argument(
        "--encryption-status",
        choices=("unknown", "encrypted", "plaintext"),
        default="unknown",
    )
    args = parser.parse_args()
    for path in (args.first, args.second):
        if not path.is_file():
            parser.error(f"file not found: {path}")
    try:
        report = compare_readbacks(
            args.first,
            args.second,
            base_address=args.base_address,
            expected_length=args.expected_length,
            region_name=args.region_name,
            encryption_status=args.encryption_status,
        )
    except ValueError as error:
        parser.error(str(error))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["readback_repeatability_proven"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
