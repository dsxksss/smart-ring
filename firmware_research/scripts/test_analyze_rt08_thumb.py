from __future__ import annotations

import struct
import tempfile
import unittest
from pathlib import Path

from analyze_rt08_thumb import (
    APPLICATION_BASE,
    HEADER_SIZE,
    address_to_file_offset,
    decode_thumb_bl,
    file_offset_to_address,
    find_bl_callers,
    find_thumb_imm8_sites,
    instruction_annotations,
    load_image,
    string_records,
)


def synthetic_image() -> bytes:
    payload = bytearray(512)
    payload[0x20 : 0x20 + len(b"RT08_V3.1\0")] = b"RT08_V3.1\0"
    string_offset = 0x100
    anchor = APPLICATION_BASE + 0x80 + 1
    struct.pack_into("<I", payload, string_offset - 4, anchor)
    payload[string_offset : string_offset + len(b"gsensor_timer\0")] = b"gsensor_timer\0"
    header = bytearray(HEADER_SIZE)
    header[:4] = bytes.fromhex("e5 c3 bd 81")
    struct.pack_into("<III", header, 4, len(payload), len(payload), sum(payload))
    return bytes(header + payload)


class AnalyzerTests(unittest.TestCase):
    def test_address_round_trip(self) -> None:
        data = synthetic_image()
        address = APPLICATION_BASE + 0x80 + 1
        offset = address_to_file_offset(address, len(data))
        self.assertEqual(offset, HEADER_SIZE + 0x80)
        self.assertEqual(file_offset_to_address(offset, len(data)), address & ~1)

    def test_finds_preceding_thumb_anchor(self) -> None:
        records = string_records(synthetic_image(), ["gsensor"])
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0]["text"], "gsensor_timer")
        self.assertEqual(
            records[0]["preceding_thumb_anchor"], APPLICATION_BASE + 0x80 + 1
        )

    def test_rejects_bad_checksum(self) -> None:
        data = bytearray(synthetic_image())
        data[-1] ^= 1
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bad.bin"
            path.write_bytes(data)
            with self.assertRaisesRegex(ValueError, "sum32"):
                load_image(path)

    def test_decodes_known_thumb_bl(self) -> None:
        self.assertEqual(
            decode_thumb_bl(0x00833A8E, 0xF7FF, 0xFF68),
            0x00833962,
        )

    def test_finds_bl_caller(self) -> None:
        data = bytearray(synthetic_image())
        caller_offset = HEADER_SIZE + 0x40
        struct.pack_into("<HH", data, caller_offset, 0xF7FF, 0xFF68)
        target = file_offset_to_address(caller_offset, len(data)) + 4 - 0x130
        self.assertEqual(
            find_bl_callers(bytes(data), target),
            [
                {
                    "address": file_offset_to_address(caller_offset, len(data)),
                    "file_offset": caller_offset,
                    "bytes": "ff f7 68 ff",
                    "target": target,
                }
            ],
        )

    def test_annotates_mapped_ldr_literal(self) -> None:
        data = synthetic_image()
        address = APPLICATION_BASE + 0x40
        literal_address = ((address + 4) & ~3) + 8
        annotations = instruction_annotations(data, address, "ldr", "r0, [pc, #8]")
        self.assertEqual(annotations["literal_address"], literal_address)
        self.assertTrue(annotations["literal_mapped"])

    def test_finds_thumb_imm8_sites(self) -> None:
        data = bytearray(synthetic_image())
        instruction_offset = HEADER_SIZE + 0x60
        data[instruction_offset : instruction_offset + 2] = bytes.fromhex("3b 2a")
        self.assertIn(
            {
                "address": file_offset_to_address(instruction_offset, len(data)),
                "file_offset": instruction_offset,
                "bytes": "3b 2a",
                "mnemonic": "cmp",
                "register": "r2",
                "immediate": 0x3B,
            },
            find_thumb_imm8_sites(bytes(data), 0x3B),
        )


if __name__ == "__main__":
    unittest.main()
