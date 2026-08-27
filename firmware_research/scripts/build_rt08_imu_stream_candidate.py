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
    "736ceb0f70b186c487dc816ced7f637cf9ae4c7b1d4e1d34bd931a1300ebcbd5"
)
EXPECTED_PATCH_SIZE = 292
HOOK_ADDRESS = 0x008280F6
HOOK_ORIGINAL = bytes.fromhex("3e 70 7e 70")
CAVE_ADDRESS = 0x00849B08
CAVE_END = 0x00849C30
INNER_CTRL_FLAGS_OFFSET = HEADER_SIZE + 2
INNER_SHA256_OFFSET = HEADER_SIZE + 0x174
EXPECTED_STOCK_INNER_SHA256 = bytes.fromhex(
    "3e143d383a69b749ed928345ac04d517d7aefb95ecc0f2f4eafbe9fd9b146f8f"
)
INNER_GIT_VERSION_OFFSET = HEADER_SIZE + 0x60
EXPECTED_STOCK_GIT_VERSION = 0x00001041  # 1.4.1 in T_IMAGE_VERSION layout
BUMPED_GIT_VERSION = 0x00006041  # 1.4.6; newer than the activated v6 probe
TOUCH_V8_GIT_VERSION = 0x00007041  # 1.4.7; newer than installed IMU v7
OUTER_FIRMWARE_OFFSET = 0x10
EXPECTED_OUTER_FIRMWARE = b"RT08_3.10.48_260309"
BUMPED_OUTER_FIRMWARE = b"RT08_3.10.51_260827"
TOUCH_V8_OUTER_FIRMWARE = b"RT08_3.10.52_260827"
DEFAULT_STATUS_ADDRESS = 0x008280CA
DEFAULT_STATUS_ORIGINAL = bytes.fromhex("ff 21")  # movs r1, #0xff
DEFAULT_STATUS_MARKER = bytes.fromhex("fd 21")  # movs r1, #0xfd
TOUCH_INDICATOR_REPEAT_ADDRESS = 0x0082C604
TOUCH_INDICATOR_REPEAT_ORIGINAL = bytes.fromhex("19 23")  # movs r3, #25
MIN_TOUCH_INDICATOR_REPEAT = 1
MAX_TOUCH_INDICATOR_REPEAT = 10
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
    0x0082DCFF,
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
    bump_internal_revision: bool = False,
    bump_outer_revision: bool = False,
    add_activation_marker: bool = False,
    touch_indicator_repeat: int | None = None,
    revision_profile: str = "imu-v7",
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
    revision_profiles = {
        "imu-v7": (BUMPED_GIT_VERSION, BUMPED_OUTER_FIRMWARE, "1.4.6"),
        "imu-touch-v8": (
            TOUCH_V8_GIT_VERSION,
            TOUCH_V8_OUTER_FIRMWARE,
            "1.4.7",
        ),
    }
    if revision_profile not in revision_profiles:
        raise ValueError(f"unknown revision profile: {revision_profile}")
    candidate_git_version, candidate_outer_firmware, candidate_semantic_version = (
        revision_profiles[revision_profile]
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
    if stored_inner_sha != EXPECTED_STOCK_INNER_SHA256:
        raise ValueError("unexpected stock SDK-generated inner SHA-256 field")
    load_image_bytes[hook_offset : hook_offset + 4] = hook
    load_image_bytes[cave_offset : cave_offset + len(patch)] = patch
    stock_git_version = struct.unpack_from("<I", stock, INNER_GIT_VERSION_OFFSET)[0]
    if bump_internal_revision:
        if stock_git_version != EXPECTED_STOCK_GIT_VERSION:
            raise ValueError(
                f"unexpected stock internal version 0x{stock_git_version:08x}"
            )
        struct.pack_into(
            "<I", load_image_bytes, INNER_GIT_VERSION_OFFSET, candidate_git_version
        )
    if bump_outer_revision:
        outer = stock[
            OUTER_FIRMWARE_OFFSET : OUTER_FIRMWARE_OFFSET + len(EXPECTED_OUTER_FIRMWARE)
        ]
        if outer != EXPECTED_OUTER_FIRMWARE:
            raise ValueError(f"unexpected outer firmware marker {outer!r}")
        load_image_bytes[
            OUTER_FIRMWARE_OFFSET : OUTER_FIRMWARE_OFFSET
            + len(candidate_outer_firmware)
        ] = candidate_outer_firmware
    if add_activation_marker:
        marker_offset = address_to_file_offset(DEFAULT_STATUS_ADDRESS, len(stock))
        if stock[marker_offset : marker_offset + 2] != DEFAULT_STATUS_ORIGINAL:
            raise ValueError("stock A1 default-status bytes do not match")
        load_image_bytes[marker_offset : marker_offset + 2] = DEFAULT_STATUS_MARKER
    touch_indicator_offset = address_to_file_offset(
        TOUCH_INDICATOR_REPEAT_ADDRESS, len(stock)
    )
    if touch_indicator_repeat is not None:
        if not (
            MIN_TOUCH_INDICATOR_REPEAT
            <= touch_indicator_repeat
            <= MAX_TOUCH_INDICATOR_REPEAT
        ):
            raise ValueError(
                "touch indicator repeat must be between "
                f"{MIN_TOUCH_INDICATOR_REPEAT} and {MAX_TOUCH_INDICATOR_REPEAT}"
            )
        original_repeat = stock[
            touch_indicator_offset : touch_indicator_offset
            + len(TOUCH_INDICATOR_REPEAT_ORIGINAL)
        ]
        if original_repeat != TOUCH_INDICATOR_REPEAT_ORIGINAL:
            raise ValueError("stock touch-indicator repeat instruction does not match")
        load_image_bytes[
            touch_indicator_offset : touch_indicator_offset
            + len(TOUCH_INDICATOR_REPEAT_ORIGINAL)
        ] = bytes((touch_indicator_repeat, 0x23))  # movs r3, #imm8
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
    if bump_internal_revision:
        allowed_offsets.update(
            range(INNER_GIT_VERSION_OFFSET, INNER_GIT_VERSION_OFFSET + 4)
        )
    if bump_outer_revision:
        allowed_offsets.update(
            range(
                OUTER_FIRMWARE_OFFSET,
                OUTER_FIRMWARE_OFFSET + len(candidate_outer_firmware),
            )
        )
    if add_activation_marker:
        marker_offset = address_to_file_offset(DEFAULT_STATUS_ADDRESS, len(stock))
        allowed_offsets.update(range(marker_offset, marker_offset + 2))
    if touch_indicator_repeat is not None:
        allowed_offsets.update(
            range(
                touch_indicator_offset,
                touch_indicator_offset + len(TOUCH_INDICATOR_REPEAT_ORIGINAL),
            )
        )
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
        ]
        + (
            [
                {
                    "kind": "internal_git_version",
                    "file_offset": INNER_GIT_VERSION_OFFSET,
                    "length": 4,
                    "before": f"0x{stock_git_version:08X}",
                    "after": f"0x{candidate_git_version:08X}",
                }
            ]
            if bump_internal_revision
            else []
        )
        + (
            [
                {
                    "kind": "outer_firmware_revision",
                    "file_offset": OUTER_FIRMWARE_OFFSET,
                    "length": len(candidate_outer_firmware),
                    "before": EXPECTED_OUTER_FIRMWARE.decode("ascii"),
                    "after": candidate_outer_firmware.decode("ascii"),
                }
            ]
            if bump_outer_revision
            else []
        )
        + (
            [
                {
                    "kind": "activation_marker",
                    "file_offset": marker_offset,
                    "address": DEFAULT_STATUS_ADDRESS,
                    "length": 2,
                    "before": DEFAULT_STATUS_ORIGINAL.hex(" "),
                    "after": DEFAULT_STATUS_MARKER.hex(" "),
                }
            ]
            if add_activation_marker
            else []
        )
        + (
            [
                {
                    "kind": "touch_indicator_repeat",
                    "file_offset": touch_indicator_offset,
                    "address": TOUCH_INDICATOR_REPEAT_ADDRESS,
                    "length": len(TOUCH_INDICATOR_REPEAT_ORIGINAL),
                    "before": TOUCH_INDICATOR_REPEAT_ORIGINAL.hex(" "),
                    "after": bytes((touch_indicator_repeat, 0x23)).hex(" "),
                }
            ]
            if touch_indicator_repeat is not None
            else []
        ),
        "unplanned_differences": 0,
        "patch_validation": patch_validation,
        "internal_version": {
            "stock_raw": f"0x{stock_git_version:08X}",
            "candidate_raw": f"0x{struct.unpack_from('<I', candidate, INNER_GIT_VERSION_OFFSET)[0]:08X}",
            "candidate_semantic": (
                candidate_semantic_version if bump_internal_revision else "1.4.1"
            ),
            "bumped_for_bank_selection": bump_internal_revision,
            "profile": revision_profile,
        },
        "outer_firmware_revision": {
            "candidate": (
                candidate_outer_firmware.decode("ascii")
                if bump_outer_revision
                else EXPECTED_OUTER_FIRMWARE.decode("ascii")
            ),
            "bumped": bump_outer_revision,
            "profile": revision_profile,
        },
        "activation_marker": {
            "enabled": add_activation_marker,
            "unknown_a1_status": "0xFD" if add_activation_marker else "0xFF",
            "custom_hook_status": "0xFE",
        },
        "touch_indicator": {
            "command": "0x50 0x55 0xAA",
            "stock_repeat": 25,
            "candidate_repeat": touch_indicator_repeat,
            "patched": touch_indicator_repeat is not None,
            "optical_sensor_led_untouched": True,
        },
        "boot_integrity_check_enabled": bool(ctrl_flags & (1 << 9)),
        "stored_inner_sha256_unchanged": True,
        "stored_inner_sha256_all_zero": False,
        "stored_inner_sha256_requires_regeneration": True,
        "outer_sum32_valid": struct.unpack_from("<I", candidate, 12)[0]
        == (sum(candidate[HEADER_SIZE:]) & 0xFFFFFFFF),
        "flash_allowed": False,
        "blocking_reasons": [
            "the full R08 flash map outside the two app slots has not been recovered",
            "0x00872000 is adjacent application storage, so the proven OTA slot cannot be expanded",
            "the OTA activation call chain is proven, but ROM API names and OTA-bank-header update/selection remain unproven",
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
    parser.add_argument("--bump-internal-revision", action="store_true")
    parser.add_argument("--bump-outer-revision", action="store_true")
    parser.add_argument("--activation-marker", action="store_true")
    parser.add_argument(
        "--touch-indicator-repeat",
        type=int,
        metavar="N",
        help="reduce the confirmed touch-area 0x50 indicator from 25 repeats to 1..10",
    )
    parser.add_argument(
        "--revision-profile",
        choices=("imu-v7", "imu-touch-v8"),
        default="imu-v7",
        help="version markers used when revision bump flags are enabled",
    )
    args = parser.parse_args()

    candidate, report = build_candidate(
        validated_stock(args.stock),
        args.patch.read_bytes(),
        bump_internal_revision=args.bump_internal_revision,
        bump_outer_revision=args.bump_outer_revision,
        add_activation_marker=args.activation_marker,
        touch_indicator_repeat=args.touch_indicator_repeat,
        revision_profile=args.revision_profile,
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
