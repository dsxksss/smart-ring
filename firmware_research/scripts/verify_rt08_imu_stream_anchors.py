#!/usr/bin/env python3
"""Verify read-only RT08 IMU stream research anchors against the official image."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Iterable

from analyze_rt08_thumb import address_to_file_offset, image_summary, load_image


EXPECTED_SHA256 = "c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b"

ANCHORS = (
    ("lis3dh_25hz_ctrl1", 0x00830D32, "37 21 20 20 ff f7 f7 fd"),
    ("lis3dh_fifo_enable", 0x00830D42, "40 21 24 20 ff f7 ef fd"),
    (
        "lis3dh_fifo_stream",
        0x00830D4A,
        "00 21 2e 20 ff f7 eb fd 80 21 2e 20 ff f7 e7 fd",
    ),
    ("lis3dh_50hz_int1", 0x0083168C, "47 21 20 20 ff f7 4a f9"),
    ("read_timer_activity_state", 0x00831822, "60 79 00 28 0e d0"),
    ("read_timer_fifo_drain", 0x0083186A, "ff f7 97 fb"),
    ("shake_remote_action_2", 0x00831BA2, "02 20 f9 f7 30 fc"),
    ("read_and_shake_timer_create", 0x00831CF0, "ff f7 1e f9 01 20 02 46 00 90"),
    ("a1_xyz_ring_consumer", 0x00825E6E, "07 aa 09 a9 08 a8 0b f0 6b fd"),
    ("a1_xyz_subtype", 0x00825E78, "03 20 69 46 48 71"),
    ("a1_xyz_notify", 0x00825ED0, "02 f0 96 fe 69 46 c8 74 01 a8 06 f0 4b fd"),
    ("remote_packet_builder", 0x0082B408, "1f b5 00 21 00 91 01 91"),
    ("a1_common_packer_call", 0x008262E8, "00 20 ff f7 5e fd"),
)


def verify_anchor_bytes(
    data: bytes, anchors: Iterable[tuple[str, int, str]] = ANCHORS
) -> list[dict[str, object]]:
    records: list[dict[str, object]] = []
    for name, address, expected_hex in anchors:
        expected = bytes.fromhex(expected_hex)
        offset = address_to_file_offset(address, len(data))
        actual = data[offset : offset + len(expected)]
        record = {
            "name": name,
            "address": f"0x{address:08x}",
            "file_offset": offset,
            "expected": expected.hex(" "),
            "actual": actual.hex(" "),
            "matches": actual == expected,
        }
        records.append(record)
        if actual != expected:
            raise ValueError(
                f"anchor {name} at 0x{address:08x} differs: "
                f"expected {expected.hex(' ')}, got {actual.hex(' ')}"
            )
    return records


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Verify RT08 IMU research addresses without modifying the image"
    )
    parser.add_argument("image", type=Path)
    args = parser.parse_args()

    data = load_image(args.image)
    sha256 = hashlib.sha256(data).hexdigest()
    if sha256 != EXPECTED_SHA256:
        raise ValueError(
            f"unexpected image SHA-256: expected {EXPECTED_SHA256}, got {sha256}"
        )
    report = {
        "summary": image_summary(data),
        "sha256": sha256,
        "read_only": True,
        "flash_authorized": False,
        "anchors": verify_anchor_bytes(data),
    }
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as error:
        print(f"error: {error}")
        raise SystemExit(2)
