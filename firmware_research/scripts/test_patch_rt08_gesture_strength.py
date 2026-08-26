from __future__ import annotations

import struct
import unittest

from analyze_rt08_thumb import HEADER_SIZE
from patch_rt08_gesture_strength import (
    ORIGINAL_BYTES,
    PATCHED_BYTES,
    PATCH_FILE_OFFSET,
    apply_strength_patch,
)


def synthetic_patchable_image() -> bytes:
    payload_length = PATCH_FILE_OFFSET + len(ORIGINAL_BYTES) - HEADER_SIZE + 32
    payload = bytearray(payload_length)
    payload[0x20 : 0x20 + len(b"RT08_V3.1\0")] = b"RT08_V3.1\0"
    payload_offset = PATCH_FILE_OFFSET - HEADER_SIZE
    payload[payload_offset : payload_offset + len(ORIGINAL_BYTES)] = ORIGINAL_BYTES
    header = bytearray(HEADER_SIZE)
    header[:4] = bytes.fromhex("e5 c3 bd 81")
    struct.pack_into("<III", header, 4, len(payload), len(payload), sum(payload))
    return bytes(header + payload)


class GestureStrengthPatchTests(unittest.TestCase):
    def test_patches_expected_instructions_and_sum32(self) -> None:
        stock = synthetic_patchable_image()
        patched = apply_strength_patch(stock, enforce_stock_hash=False)
        self.assertEqual(
            patched[PATCH_FILE_OFFSET : PATCH_FILE_OFFSET + len(PATCHED_BYTES)],
            PATCHED_BYTES,
        )
        self.assertEqual(
            struct.unpack_from("<I", patched, 12)[0],
            sum(patched[HEADER_SIZE:]) & 0xFFFFFFFF,
        )
        self.assertEqual(
            stock[PATCH_FILE_OFFSET : PATCH_FILE_OFFSET + len(ORIGINAL_BYTES)],
            ORIGINAL_BYTES,
        )

    def test_rejects_unexpected_patch_site(self) -> None:
        stock = bytearray(synthetic_patchable_image())
        stock[PATCH_FILE_OFFSET] ^= 1
        with self.assertRaisesRegex(ValueError, "original instructions"):
            apply_strength_patch(bytes(stock), enforce_stock_hash=False)


if __name__ == "__main__":
    unittest.main()
