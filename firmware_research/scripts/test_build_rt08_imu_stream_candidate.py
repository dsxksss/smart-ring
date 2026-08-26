from __future__ import annotations

import struct
import unittest

from analyze_rt08_thumb import HEADER_SIZE, address_to_file_offset, decode_thumb_bl
from build_rt08_imu_stream_candidate import (
    CAVE_ADDRESS,
    CAVE_END,
    HOOK_ADDRESS,
    HOOK_ORIGINAL,
    INNER_SHA256_OFFSET,
    build_candidate,
    validate_patch_binary,
)
from test_analyze_rt08_thumb import synthetic_image


def candidate_fixture() -> bytes:
    minimum_length = address_to_file_offset(CAVE_END - 1, 0x40000) + 1
    data = bytearray(synthetic_image())
    if len(data) < minimum_length:
        payload_growth = minimum_length - len(data)
        data.extend(b"\0" * payload_growth)
        payload_length = len(data) - HEADER_SIZE
        struct.pack_into("<I", data, HEADER_SIZE + 8, payload_length - 0x400)
        struct.pack_into("<II", data, 4, payload_length, payload_length)
    hook_offset = address_to_file_offset(HOOK_ADDRESS, len(data))
    data[hook_offset : hook_offset + 4] = HOOK_ORIGINAL
    struct.pack_into("<I", data, 12, sum(data[HEADER_SIZE:]) & 0xFFFFFFFF)
    return bytes(data)


class ImuStreamCandidateBuilderTests(unittest.TestCase):
    def test_patches_hook_and_cave_and_updates_outer_sum(self) -> None:
        stock = candidate_fixture()
        patch = bytes(range(32))
        candidate, report = build_candidate(
            stock, patch, enforce_stock_hash=False, validate_patch=False
        )
        hook_offset = address_to_file_offset(HOOK_ADDRESS, len(candidate))
        first, second = struct.unpack_from("<HH", candidate, hook_offset)
        self.assertEqual(decode_thumb_bl(HOOK_ADDRESS, first, second), CAVE_ADDRESS)
        cave_offset = address_to_file_offset(CAVE_ADDRESS, len(candidate))
        self.assertEqual(candidate[cave_offset : cave_offset + len(patch)], patch)
        self.assertTrue(report["outer_sum32_valid"])
        self.assertEqual(report["unplanned_differences"], 0)
        self.assertGreater(report["changed_byte_count"], 0)
        self.assertFalse(report["flash_allowed"])

    def test_rejects_oversized_patch(self) -> None:
        with self.assertRaisesRegex(ValueError, "does not fit"):
            build_candidate(
                candidate_fixture(),
                b"x" * (CAVE_END - CAVE_ADDRESS + 1),
                enforce_stock_hash=False,
                validate_patch=False,
            )

    def test_rejects_patch_with_stale_literal_table(self) -> None:
        with self.assertRaisesRegex(ValueError, "exactly match"):
            validate_patch_binary(b"\0" * (CAVE_END - CAVE_ADDRESS))

    def test_rejects_stock_with_enabled_sha_field_assumption(self) -> None:
        stock = bytearray(candidate_fixture())
        stock[INNER_SHA256_OFFSET] = 1
        struct.pack_into("<I", stock, 12, sum(stock[HEADER_SIZE:]) & 0xFFFFFFFF)
        with self.assertRaisesRegex(ValueError, "inner SHA-256 field"):
            build_candidate(
                bytes(stock),
                b"x" * (CAVE_END - CAVE_ADDRESS),
                enforce_stock_hash=False,
                validate_patch=False,
            )


if __name__ == "__main__":
    unittest.main()
