from __future__ import annotations

import hashlib
import struct
import tempfile
import unittest
from pathlib import Path

from inspect_r08_image import (
    KNOWN_HEADER_SIZE,
    REALTEK_IMAGE_HEADER_SIZE,
    RTL8762E_APPLICATION_BASE,
    RTL8762E_SHA256_OFFSET,
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


def synthetic_rtl8762e_application() -> bytes:
    payload = bytearray(2048)
    struct.pack_into("<BBHHHI", payload, 0, 12, 0, 0x181, 0x2793, 0, len(payload) - REALTEK_IMAGE_HEADER_SIZE)
    struct.pack_into("<III", payload, 0x1C, RTL8762E_APPLICATION_BASE + 0x400, RTL8762E_APPLICATION_BASE + 0x400, 0)
    struct.pack_into("<I", payload, 0x28, RTL8762E_APPLICATION_BASE)
    marker = b"RT08_V3.1\0RT08_3.10.48_260309\0"
    payload[0x300 : 0x300 + len(marker)] = marker
    source = b"..\\..\\qc_code\\app_module\\gsensor\\lis3dh_spi.c\0"
    payload[0x340 : 0x340 + len(source)] = source
    for index in range(24):
        address = RTL8762E_APPLICATION_BASE + 0x400 + index * 2 + 1
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

    def test_accepts_rtl8762e_application_without_standard_vector_table(self) -> None:
        report = self.inspect_bytes(synthetic_rtl8762e_application())
        self.assertFalse(report["arm_vector_candidates"])
        self.assertTrue(report["rtl8762e_application"]["candidate"])
        self.assertEqual(
            report["rtl8762e_application"]["application_base_candidate"],
            RTL8762E_APPLICATION_BASE,
        )
        self.assertTrue(report["offline_patch_candidate"])
        self.assertFalse(report["flash_authorized"])

    def test_reads_sha256_from_official_header_field(self) -> None:
        data = bytearray(synthetic_rtl8762e_application())
        payload_offset = KNOWN_HEADER_SIZE
        body = data[payload_offset + REALTEK_IMAGE_HEADER_SIZE :]
        expected = hashlib.sha256(body).digest()
        start = payload_offset + RTL8762E_SHA256_OFFSET
        data[start : start + len(expected)] = expected
        struct.pack_into("<I", data, 12, sum(data[KNOWN_HEADER_SIZE:]) & 0xFFFFFFFF)
        report = self.inspect_bytes(bytes(data))["rtl8762e_application"]
        self.assertEqual(report["stored_sha256"], expected.hex())
        self.assertTrue(report["body_sha256_matches"])
        self.assertFalse(report["sha256_all_zero"])

    def test_entropy_boundaries(self) -> None:
        self.assertEqual(shannon_entropy(b""), 0.0)
        self.assertEqual(shannon_entropy(b"\x00" * 16), 0.0)
        self.assertEqual(shannon_entropy(bytes(range(256))), 8.0)


if __name__ == "__main__":
    unittest.main()
