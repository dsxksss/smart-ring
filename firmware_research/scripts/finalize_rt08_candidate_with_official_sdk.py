#!/usr/bin/env python3
"""Regenerate an RT08 candidate's image digest with the exact official SDK tool."""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import subprocess
import tempfile
import zipfile
from pathlib import Path
from typing import Any


HEADER_SIZE = 0x50
INNER_SHA256_OFFSET = 0x174
PROFILES = {
    "imu-v7": {
        "input_sha256": "eac7d909b6cc8226512445a8716e56f2d6a23a49bee5a0a19087759aeff209ee",
        "old_digest": bytes.fromhex(
            "3e143d383a69b749ed928345ac04d517d7aefb95ecc0f2f4eafbe9fd9b146f8f"
        ),
        "new_digest": bytes.fromhex(
            "0a4b55c5f9c74d02adb0cfb4aabc1d6ccd5af55238fcd4443a70ee7a6101019a"
        ),
        "classification": "SDK_FINALIZED_IMU_STREAM_CANDIDATE",
        "activation_marker_status": "0xFD/0xFE",
        "imu_patch_present": True,
    },
}
DEFAULT_PROFILE = PROFILES["imu-v7"]
EXPECTED_INPUT_SHA256 = DEFAULT_PROFILE["input_sha256"]
EXPECTED_SDK_ARCHIVE_SHA256 = (
    "ef1f47b83d60aeb54edb83a34ecc9d92218965ab18ff02256773441d35f7db52"
)
TOOL_MEMBER = "tool/prepend_header/prepend_header.exe"
EXPECTED_TOOL_SIZE = 5_308_928
EXPECTED_TOOL_SHA256 = (
    "9d71cbf180afef5f7e48e0c847277addef91e344b4ceeef91296d3de0b081c22"
)
EXPECTED_OLD_IMAGE_DIGEST = DEFAULT_PROFILE["old_digest"]
EXPECTED_NEW_IMAGE_DIGEST = DEFAULT_PROFILE["new_digest"]


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _validate_container(data: bytes) -> None:
    if len(data) < HEADER_SIZE + 0x400 or data[:4] != bytes.fromhex("e5 c3 bd 81"):
        raise ValueError("not the reviewed QRing 0x50 container")
    length_a, length_b, stored_sum = struct.unpack_from("<III", data, 4)
    payload = data[HEADER_SIZE:]
    if length_a != length_b or length_a != len(payload):
        raise ValueError("QRing payload length mismatch")
    if stored_sum != sum(payload) & 0xFFFFFFFF:
        raise ValueError("QRing payload sum32 mismatch")


def finalize_candidate(
    candidate: bytes,
    processed_inner: bytes,
    *,
    enforce_input_hash: bool = True,
    profile: dict[str, Any] | None = None,
) -> tuple[bytes, dict[str, Any]]:
    selected = DEFAULT_PROFILE if profile is None else profile
    input_hash = sha256_hex(candidate)
    if enforce_input_hash and input_hash != selected["input_sha256"]:
        raise ValueError(f"input candidate SHA-256 mismatch: {input_hash}")
    _validate_container(candidate)
    original_inner = candidate[HEADER_SIZE:]
    if len(processed_inner) != len(original_inner):
        raise ValueError("official tool changed the inner image length")
    digest_slice = slice(INNER_SHA256_OFFSET, INNER_SHA256_OFFSET + 32)
    if original_inner[digest_slice] != selected["old_digest"]:
        raise ValueError("input does not carry the exact stock SDK image digest")
    if processed_inner[digest_slice] != selected["new_digest"]:
        raise ValueError("official tool produced an unexpected image digest")

    inner_differences = {
        index
        for index, (before, after) in enumerate(zip(original_inner, processed_inner))
        if before != after
    }
    expected_differences = set(range(INNER_SHA256_OFFSET, INNER_SHA256_OFFSET + 32))
    if inner_differences != expected_differences:
        unexpected = inner_differences ^ expected_differences
        first = min(unexpected) if unexpected else -1
        raise ValueError(f"official tool changed unexpected inner offset 0x{first:x}")

    output = bytearray(candidate)
    output[HEADER_SIZE:] = processed_inner
    struct.pack_into("<I", output, 12, sum(output[HEADER_SIZE:]) & 0xFFFFFFFF)
    finalized = bytes(output)
    _validate_container(finalized)
    changed = {
        index
        for index, (before, after) in enumerate(zip(candidate, finalized))
        if before != after
    }
    allowed = set(range(12, 16))
    allowed.update(
        range(HEADER_SIZE + INNER_SHA256_OFFSET, HEADER_SIZE + INNER_SHA256_OFFSET + 32)
    )
    if changed - allowed:
        raise AssertionError("finalizer changed bytes outside digest and outer sum32")

    return finalized, {
        "classification": selected["classification"],
        "input_sha256": input_hash,
        "candidate_sha256": sha256_hex(finalized),
        "candidate_size": len(finalized),
        "sdk_image_digest_offset": f"0x{INNER_SHA256_OFFSET:03X}",
        "sdk_image_digest": selected["new_digest"].hex(),
        "official_tool_changed_inner_bytes": len(inner_differences),
        "total_changed_bytes_from_prefinal": len(changed),
        "outer_sum32_valid": True,
        "activation_marker_status": selected["activation_marker_status"],
        "imu_patch_present": selected["imu_patch_present"],
        "flash_allowed": False,
    }


def process_with_official_tool(
    candidate: bytes, sdk_archive: Path, expected_digest: bytes
) -> tuple[bytes, str]:
    archive_hash = sha256_hex(sdk_archive.read_bytes())
    if archive_hash != EXPECTED_SDK_ARCHIVE_SHA256:
        raise ValueError(f"SDK archive SHA-256 mismatch: {archive_hash}")
    with zipfile.ZipFile(sdk_archive) as archive:
        tool = archive.read(TOOL_MEMBER)
    if len(tool) != EXPECTED_TOOL_SIZE or sha256_hex(tool) != EXPECTED_TOOL_SHA256:
        raise ValueError("official prepend_header.exe identity mismatch")

    with tempfile.TemporaryDirectory(prefix="r08-sdk-finalize-") as directory:
        root = Path(directory)
        tool_path = root / "prepend_header.exe"
        inner_path = root / "candidate_inner.bin"
        tool_path.write_bytes(tool)
        inner_path.write_bytes(candidate[HEADER_SIZE:])
        completed = subprocess.run(
            [
                str(tool_path),
                "-t",
                "app_code",
                "-b",
                "12",
                "-p",
                str(inner_path),
                "-m",
                "0",
                "-c",
                "sha256",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        output = (completed.stdout + completed.stderr).strip()
        if completed.returncode != 0:
            raise ValueError(f"official prepend_header.exe failed: {output}")
        expected_line = f"SHA256 = {expected_digest.hex().upper()}"
        if expected_line not in output:
            raise ValueError(f"official tool did not report the reviewed digest: {output}")
        return inner_path.read_bytes(), output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--profile", choices=tuple(PROFILES), default="imu-v7")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--allow-unverified-output", action="store_true")
    args = parser.parse_args()
    profile = PROFILES[args.profile]
    candidate = args.candidate.read_bytes()
    processed, tool_output = process_with_official_tool(
        candidate, args.sdk, profile["new_digest"]
    )
    finalized, report = finalize_candidate(candidate, processed, profile=profile)
    report["profile"] = args.profile
    report["sdk_archive_sha256"] = EXPECTED_SDK_ARCHIVE_SHA256
    report["official_tool_sha256"] = EXPECTED_TOOL_SHA256
    report["official_tool_output"] = tool_output.splitlines()
    if args.output:
        if not args.allow_unverified_output:
            raise ValueError("refusing output without --allow-unverified-output")
        args.output.write_bytes(finalized)
        report["output"] = str(args.output)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(f"error: {error}")
        raise SystemExit(2)
