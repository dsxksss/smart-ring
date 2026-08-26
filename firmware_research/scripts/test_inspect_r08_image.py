from __future__ import annotations

import hashlib
import struct
import tempfile
import unittest
from pathlib import Path

from inspect_r08_image import (
    KNOWN_HEADER_SIZE,
    RF03_APPLICATION_BASE,
    inspect_image,
    shannon_entropy,
)


def synthetic_image(hardware: str = "RT08_V3.1") -> bytes:
    payload = bytearray(256)
    struct.pack_into("<II", payload, 0, 0x20001000, 0x00824021)
    marker = f"{hardware}\0RT08_3.10.48_260309\0".encode("ascii")
    payload[32 : 32 + len(marker)] = marker
    header = bytearray(KNOWN_HEADER_SIZE)
    header[:4] = bytes.fromhex("e5 c3 bd 81")
    struct.pack_into("<III", header, 4, len(payload), len(payload), sum(payload))
    return bytes(header + payload)


def synthetic_rf03_application() -> bytes:
    payload = bytearray(2048)
    marker = b"RT08_V3.1\0RT08_3.10.48_260309\0"
    payload[32 : 32 + len(marker)] = marker
    source = b"..\\..\\qc_code\\app_module\\gsensor\\lis3dh_spi.c\0"
    payload[96 : 96 + len(source)] = source
    for index in range(24):
        address = RF03_APPLICATION_BASE + 0x400 + index * 2 + 1
        struct.pack_into("<I", payload, 0x200 + index * 4, address)
    header = bytearray(KNOWN_HEADER_SIZE)
    header[:4] = bytes.fromhex("e5 c3 bd 81")
    struct.pack_into("<III", header, 4, len(payload), len(payload), sum(payload))
    return bytes(header + payload)


class InspectorTests(unittest.TestCase):
    def inspect_bytes(self, data: bytes) -> dict:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "candidate.bin"
            path.write_bytes(data)
            return inspect_image(path)

    def test_accepts_structurally_valid_exact_hardware_candidate(self) -> None:
        data = synthetic_image()
        report = self.inspect_bytes(data)
        self.assertEqual(report["sha256"], hashlib.sha256(data).hexdigest())
        self.assertTrue(report["exact_hardware_string_found"])
        self.assertTrue(report["known_container"]["lengths_match"])
        self.assertTrue(report["known_container"]["sum32_matches"])
        self.assertTrue(report["arm_vector_candidates"])
        self.assertTrue(report["offline_patch_candidate"])
        self.assertFalse(report["flash_authorized"])

    def test_rejects_other_hardware_even_with_valid_container(self) -> None:
        report = self.inspect_bytes(synthetic_image("RY02_V3.0"))
        self.assertFalse(report["offline_patch_candidate"])
        self.assertIn(
            "missing exact hardware marker RT08_V3.1", report["rejection_reasons"]
        )

    def test_accepts_rf03_application_without_standard_vector_table(self) -> None:
        report = self.inspect_bytes(synthetic_rf03_application())
        self.assertFalse(report["arm_vector_candidates"])
        self.assertTrue(report["rf03_application"]["candidate"])
        self.assertEqual(
            report["rf03_application"]["application_base_candidate"],
            RF03_APPLICATION_BASE,
        )
        self.assertTrue(report["offline_patch_candidate"])
        self.assertFalse(report["flash_authorized"])

    def test_entropy_boundaries(self) -> None:
        self.assertEqual(shannon_entropy(b""), 0.0)
        self.assertEqual(shannon_entropy(b"\x00" * 16), 0.0)
        self.assertEqual(shannon_entropy(bytes(range(256))), 8.0)


if __name__ == "__main__":
    unittest.main()
