from __future__ import annotations

import struct
import tempfile
import unittest
from pathlib import Path

from verify_rtl8762e_sdk_v1_5_0 import (
    EXPECTED_ROM_SYMBOLS,
    extract_ascii_strings,
    extract_utf16le_ascii_strings,
    parse_pe_export_names,
    parse_rom_symbols,
    verify_sdk_archive,
)


class Rtl8762eSdkEvidenceTests(unittest.TestCase):
    def test_rom_symbol_parser_normalizes_thumb_bit(self) -> None:
        source = b"\n".join(
            f"{name} = 0x{address | 1:08x} ;".encode("ascii")
            for name, address in EXPECTED_ROM_SYMBOLS.items()
        )
        self.assertEqual(parse_rom_symbols(source), EXPECTED_ROM_SYMBOLS)

    def test_nonmatching_archive_is_rejected_before_opening(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "not-the-sdk.zip"
            path.write_bytes(b"not an SDK")
            with self.assertRaisesRegex(ValueError, "archive SHA-256 mismatch"):
                verify_sdk_archive(path)

    def test_ascii_string_extraction_ignores_short_noise(self) -> None:
        data = b"\x00ReadMPFlashData\x00no\x00GetBtMPFlashSize\x00"
        self.assertEqual(
            extract_ascii_strings(data),
            {"ReadMPFlashData", "GetBtMPFlashSize"},
        )

    def test_utf16le_ascii_string_extraction(self) -> None:
        data = b"\x00" + "Get Flash ID".encode("utf-16le") + b"\x00\x00"
        self.assertEqual(extract_utf16le_ascii_strings(data), {"Get Flash ID"})

    def test_minimal_pe_export_parser(self) -> None:
        image = bytearray(0x400)
        image[:2] = b"MZ"
        struct.pack_into("<I", image, 0x3C, 0x80)
        image[0x80:0x84] = b"PE\0\0"
        struct.pack_into("<H", image, 0x86, 1)
        struct.pack_into("<H", image, 0x94, 0xE0)
        optional = 0x98
        struct.pack_into("<H", image, optional, 0x10B)
        struct.pack_into("<II", image, optional + 96, 0x1000, 0x100)
        section = optional + 0xE0
        image[section : section + 8] = b".rdata\0\0"
        struct.pack_into("<IIII", image, section + 8, 0x200, 0x1000, 0x200, 0x200)
        struct.pack_into("<I", image, 0x200 + 24, 2)
        struct.pack_into("<I", image, 0x200 + 32, 0x1040)
        struct.pack_into("<II", image, 0x240, 0x1050, 0x1060)
        image[0x250:0x25A] = b"ReadFlash\0"
        image[0x260:0x26B] = b"GetFlashID\0"
        self.assertEqual(
            parse_pe_export_names(bytes(image)),
            {"ReadFlash", "GetFlashID"},
        )


if __name__ == "__main__":
    unittest.main()
