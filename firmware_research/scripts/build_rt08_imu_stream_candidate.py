#!/usr/bin/env python3
"""Build an explicitly non-flashable RT08 IMU-stream candidate for offline QA."""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
from pathlib import Path
from typing import Any

from analyze_rt08_thumb import (
    HEADER_SIZE,
    address_to_file_offset,
    decode_thumb_bl,
    encode_thumb_bl,
    load_image,
)


EXPECTED_STOCK_SHA256 = (
    "c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b"
)
EXPECTED_PATCH_SHA256 = (
    "a157de7d40619ac9efa259e04473f7e7929ed2c82f9a9a77f25b2fc217a55928"
)
EXPECTED_PATCH_SIZE = 280
HOOK_ADDRESS = 0x008280F6
HOOK_ORIGINAL = bytes.fromhex("3e 70 7e 70")
CAVE_ADDRESS = 0x00849B08
CAVE_END = 0x00849C30
INNER_CTRL_FLAGS_OFFSET = HEADER_SIZE + 2
INNER_SHA256_OFFSET = HEADER_SIZE + 0x394
EXPECTED_LITERAL_WORDS = (
    0x00209CB8,
    0x00209CC8,
    0x0020BFA0,
    0x00828115,
    None,  # r08_stream_tick Thumb address, derived below
    0x00829F19,
    0x00829F45,
    0x00832D07,
    0x00832CBD,
    0x008335FD,
    0x0083394F,
    0x0082AC01,
    0x0082E975,
)


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def validate_patch_binary(patch: bytes) -> dict[str, Any]:
    capacity = CAVE_END - CAVE_ADDRESS
    if len(patch) != EXPECTED_PATCH_SIZE:
        raise ValueError(
            f"patch must exactly match the reviewed {EXPECTED_PATCH_SIZE}-byte object"
        )
    tick_signature = bytes.fromhex("f0 b5 87 b0")
    tick_offset = patch.find(tick_signature)
    if tick_offset < 0 or patch.find(tick_signature, tick_offset + 1) >= 0:
        raise ValueError("timer callback prologue is missing or ambiguous")
    tick_pointer = (CAVE_ADDRESS + tick_offset) | 1
    literal_size = len(EXPECTED_LITERAL_WORDS) * 4
    if len(patch) < literal_size:
        raise ValueError("patch is shorter than its required literal table")
    literals = list(struct.unpack_from(f"<{len(EXPECTED_LITERAL_WORDS)}I", patch, len(patch) - literal_size))
    expected = [tick_pointer if value is None else value for value in EXPECTED_LITERAL_WORDS]
    if literals != expected:
        raise ValueError(
            "patch literal table mismatch; refusing an object with shifted or stale addresses"
        )
    forbidden_led_functions = (0x008350BF, 0x008350D9)
    if any(value in literals for value in forbidden_led_functions):
        raise ValueError("patch references an optical/LED raw-sensor function")
    patch_hash = sha256_hex(patch)
    if patch_hash != EXPECTED_PATCH_SHA256:
        raise ValueError(
            "patch SHA-256 differs from the instruction-reviewed build"
        )
    return {
        "capacity": capacity,
        "size": len(patch),
        "remaining": capacity - len(patch),
        "timer_callback": tick_pointer,
        "sha256": patch_hash,
        "instruction_reviewed_sha256": True,
        "literal_table_valid": True,
        "optical_led_functions_absent": True,
    }


def build_candidate(
    stock: bytes,
    patch: bytes,
    *,
    enforce_stock_hash: bool = True,
    validate_patch: bool = True,
) -> tuple[bytes, dict[str, Any]]:
    load_image_bytes = bytearray(stock)
    stock_hash = sha256_hex(stock)
    if enforce_stock_hash and stock_hash != EXPECTED_STOCK_SHA256:
        raise ValueError(
            f"stock SHA-256 mismatch: expected {EXPECTED_STOCK_SHA256}, got {stock_hash}"
        )
    patch_validation = (
        validate_patch_binary(patch)
        if validate_patch
        else {
            "capacity": CAVE_END - CAVE_ADDRESS,
            "size": len(patch),
            "remaining": CAVE_END - CAVE_ADDRESS - len(patch),
        }
    )
    if patch_validation["remaining"] < 0:
        raise ValueError("patch does not fit the conservative zero-run candidate")

    hook_offset = address_to_file_offset(HOOK_ADDRESS, len(stock))
    if stock[hook_offset : hook_offset + 4] != HOOK_ORIGINAL:
        raise ValueError("stock hook bytes do not match A1 invalid-subcommand path")
    cave_offset = address_to_file_offset(CAVE_ADDRESS, len(stock))
    cave_end_offset = address_to_file_offset(CAVE_END - 1, len(stock)) + 1
    if any(stock[cave_offset:cave_end_offset]):
        raise ValueError("candidate cave is not all zero in this image")

    ctrl_flags = struct.unpack_from("<H", stock, INNER_CTRL_FLAGS_OFFSET)[0]
    if ctrl_flags & (1 << 9):
        raise ValueError("boot-time inner integrity checking is enabled")

    hook = encode_thumb_bl(HOOK_ADDRESS, CAVE_ADDRESS)
    first, second = struct.unpack("<HH", hook)
    if decode_thumb_bl(HOOK_ADDRESS, first, second) != CAVE_ADDRESS:
        raise AssertionError("encoded hook does not round-trip")

    stored_inner_sha = stock[INNER_SHA256_OFFSET : INNER_SHA256_OFFSET + 32]
    if any(stored_inner_sha):
        raise ValueError("expected disabled stock inner SHA-256 field to be all zero")
    load_image_bytes[hook_offset : hook_offset + 4] = hook
    load_image_bytes[cave_offset : cave_offset + len(patch)] = patch
    payload = load_image_bytes[HEADER_SIZE:]
    struct.pack_into("<I", load_image_bytes, 12, sum(payload) & 0xFFFFFFFF)

    candidate = bytes(load_image_bytes)
    if candidate[INNER_SHA256_OFFSET : INNER_SHA256_OFFSET + 32] != stored_inner_sha:
        raise AssertionError("builder unexpectedly changed the stored inner SHA-256 field")
    changed_offsets = {
        index for index, (before, after) in enumerate(zip(stock, candidate)) if before != after
    }
    allowed_offsets = set(range(12, 16))
    allowed_offsets.update(range(hook_offset, hook_offset + len(hook)))
    allowed_offsets.update(range(cave_offset, cave_offset + len(patch)))
    unexpected_offsets = changed_offsets - allowed_offsets
    if unexpected_offsets:
        first = min(unexpected_offsets)
        raise AssertionError(f"builder changed unplanned file offset 0x{first:x}")

    report = {
        "classification": "NON_FLASHABLE_OFFLINE_CANDIDATE",
        "stock_sha256": stock_hash,
        "candidate_sha256": sha256_hex(candidate),
        "hook_address": HOOK_ADDRESS,
        "hook_original": HOOK_ORIGINAL.hex(" "),
        "hook_patched": hook.hex(" "),
        "hook_target": CAVE_ADDRESS,
        "cave_address": CAVE_ADDRESS,
        "cave_end": CAVE_END,
        "patch_size": len(patch),
        "remaining_bytes": CAVE_END - CAVE_ADDRESS - len(patch),
        "changed_byte_count": len(changed_offsets),
        "allowed_mutation_ranges": [
            {"kind": "qring_sum32", "file_offset": 12, "length": 4},
            {
                "kind": "thumb_hook",
                "file_offset": hook_offset,
                "address": HOOK_ADDRESS,
                "length": len(hook),
            },
            {
                "kind": "patch_object",
                "file_offset": cave_offset,
                "address": CAVE_ADDRESS,
                "length": len(patch),
            },
        ],
        "unplanned_differences": 0,
        "patch_validation": patch_validation,
        "boot_integrity_check_enabled": bool(ctrl_flags & (1 << 9)),
        "stored_inner_sha256_unchanged": True,
        "stored_inner_sha256_all_zero": True,
        "outer_sum32_valid": struct.unpack_from("<I", candidate, 12)[0]
        == (sum(candidate[HEADER_SIZE:]) & 0xFFFFFFFF),
        "flash_allowed": False,
        "blocking_reasons": [
            "the full R08 flash map outside the two app slots has not been recovered",
            "0x00872000 is adjacent application storage, so the proven OTA slot cannot be expanded",
            "the OTA activation call chain is proven, but ROM API names and equal-version bank selection remain unproven",
            "runtime-crash rollback and power-loss recovery remain unproven",
            "UART/SWD pads and byte-for-byte restore have not been proven on hardware",
            "candidate has not been validated on a sacrificial RT08_V3.1 device",
        ],
    }
    return candidate, report


def validated_stock(path: Path) -> bytes:
    load_image(path)
    return path.read_bytes()


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Build or inspect the non-flashable RT08 IMU-stream candidate"
    )
    parser.add_argument("stock", type=Path)
    parser.add_argument("patch", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--allow-unverified-output", action="store_true")
    args = parser.parse_args()

    candidate, report = build_candidate(
        validated_stock(args.stock), args.patch.read_bytes()
    )
    if args.output:
        if not args.allow_unverified_output:
            raise ValueError(
                "refusing to write candidate without --allow-unverified-output"
            )
        args.output.write_bytes(candidate)
        report["output"] = str(args.output)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as error:
        print(f"error: {error}")
        raise SystemExit(2)
