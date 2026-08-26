#!/usr/bin/env python3
"""Create a narrowly scoped, offline RT08 gesture-strength patch candidate.

This tool never modifies the stock input image. It only accepts the exact
RT08_3.10.48_260309 image whose SHA-256 is pinned below, verifies the original
Thumb instructions, patches the command 0x3B strength-field mismatch, and
recomputes only the outer QRing container sum32.

The stock image contains an RTL8762E inner application header whose integrity
field/tool policy has not yet been reproduced.  Output from this script is
therefore an offline analysis artifact and MUST NOT be flashed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import sys
from pathlib import Path

from analyze_rt08_thumb import HEADER_SIZE, address_to_file_offset, load_image


EXPECTED_STOCK_SHA256 = (
    "c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b"
)
PATCH_ADDRESS = 0x0082CA54
PATCH_FILE_OFFSET = 0x6AA4
ORIGINAL_BYTES = bytes.fromhex("4b 74 60 79 88 74")
PATCHED_BYTES = bytes.fromhex("20 79 48 74 8e 74")


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def apply_strength_patch(data: bytes, *, enforce_stock_hash: bool = True) -> bytes:
    if address_to_file_offset(PATCH_ADDRESS, len(data)) != PATCH_FILE_OFFSET:
        raise ValueError("patch address/file offset mapping is inconsistent")
    load_image_bytes = bytearray(data)
    if enforce_stock_hash and sha256_hex(data) != EXPECTED_STOCK_SHA256:
        raise ValueError("input SHA-256 is not the pinned RT08 stock image")
    actual = bytes(
        load_image_bytes[PATCH_FILE_OFFSET : PATCH_FILE_OFFSET + len(ORIGINAL_BYTES)]
    )
    if actual != ORIGINAL_BYTES:
        raise ValueError(
            "original instructions do not match at patch site: " + actual.hex(" ")
        )
    load_image_bytes[
        PATCH_FILE_OFFSET : PATCH_FILE_OFFSET + len(PATCHED_BYTES)
    ] = PATCHED_BYTES
    payload = load_image_bytes[HEADER_SIZE:]
    struct.pack_into("<I", load_image_bytes, 12, sum(payload) & 0xFFFFFFFF)
    return bytes(load_image_bytes)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Build an offline RT08 gesture-strength patch candidate"
    )
    parser.add_argument("stock_image", type=Path)
    parser.add_argument("output_image", type=Path)
    args = parser.parse_args()

    if args.output_image.exists():
        raise ValueError(f"refusing to overwrite existing output: {args.output_image}")
    stock = load_image(args.stock_image)
    patched = apply_strength_patch(stock)
    if patched[:4] != stock[:4]:
        raise ValueError("patched container magic changed unexpectedly")
    if struct.unpack_from("<I", patched, 12)[0] != sum(patched[HEADER_SIZE:]) & 0xFFFFFFFF:
        raise ValueError("patched container sum32 verification failed")

    args.output_image.parent.mkdir(parents=True, exist_ok=True)
    args.output_image.write_bytes(patched)
    report = {
        "input": str(args.stock_image.resolve()),
        "output": str(args.output_image.resolve()),
        "stock_sha256": sha256_hex(stock),
        "patched_sha256": sha256_hex(patched),
        "patch_address": f"0x{PATCH_ADDRESS:08x}",
        "patch_file_offset": f"0x{PATCH_FILE_OFFSET:x}",
        "original_bytes": ORIGINAL_BYTES.hex(" "),
        "patched_bytes": PATCHED_BYTES.hex(" "),
        "stored_sum32": struct.unpack_from("<I", patched, 12)[0],
        "flash_authorized": False,
        "flash_blocker": (
            "RTL8762E inner integrity/prepend policy and independent recovery "
            "path are not verified"
        ),
    }
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(2)
