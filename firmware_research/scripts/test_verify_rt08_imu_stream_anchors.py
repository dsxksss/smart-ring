from __future__ import annotations

import struct
import unittest

from analyze_rt08_thumb import APPLICATION_BASE, HEADER_SIZE
from verify_rt08_imu_stream_anchors import verify_anchor_bytes


def synthetic_image() -> bytes:
    payload = bytearray(0x500)
    struct.pack_into("<BBHHHI", payload, 0, 12, 0, 0x181, 0x2793, 0, len(payload) - 0x400)
    struct.pack_into("<III", payload, 0x1C, APPLICATION_BASE + 0x400, APPLICATION_BASE + 0x400, 0)
    struct.pack_into("<I", payload, 0x28, APPLICATION_BASE)
    payload[0x60 : 0x60 + len(b"RT08_V3.1\0")] = b"RT08_V3.1\0"
    header = bytearray(HEADER_SIZE)
    header[:4] = bytes.fromhex("e5 c3 bd 81")
    struct.pack_into("<III", header, 4, len(payload), len(payload), sum(payload))
    return bytes(header + payload)


class AnchorVerifierTests(unittest.TestCase):
    def test_accepts_matching_anchor(self) -> None:
        data = bytearray(synthetic_image())
        address = APPLICATION_BASE + 0x20
        data[HEADER_SIZE + 0x20 : HEADER_SIZE + 0x24] = bytes.fromhex("01 23 45 67")
        records = verify_anchor_bytes(
            bytes(data), (("synthetic", address, "01 23 45 67"),)
        )
        self.assertTrue(records[0]["matches"])

    def test_rejects_mismatched_anchor(self) -> None:
        data = synthetic_image()
        address = APPLICATION_BASE + 0x20
        with self.assertRaisesRegex(ValueError, "anchor synthetic"):
            verify_anchor_bytes(data, (("synthetic", address, "01 23 45 67"),))


if __name__ == "__main__":
    unittest.main()
