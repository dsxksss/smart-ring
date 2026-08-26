from __future__ import annotations

import struct
import unittest

from analyze_rt08_thumb import APPLICATION_BASE, HEADER_SIZE, file_offset_to_address
from scan_rt08_code_caves import (
    EXECUTABLE_FILE_OFFSET,
    decode_thumb16_branch,
    padding_runs,
    reference_candidates,
)
from test_analyze_rt08_thumb import synthetic_image


class CodeCaveScannerTests(unittest.TestCase):
    def test_decodes_thumb16_branches(self) -> None:
        self.assertEqual(decode_thumb16_branch(0x1000, 0xE001), 0x1006)
        self.assertEqual(decode_thumb16_branch(0x1000, 0xD101), 0x1006)
        self.assertIsNone(decode_thumb16_branch(0x1000, 0xDF01))

    def test_finds_only_executable_padding(self) -> None:
        data = bytearray(synthetic_image())
        data[EXECUTABLE_FILE_OFFSET:] = b"\x55" * (
            len(data) - EXECUTABLE_FILE_OFFSET
        )
        start = EXECUTABLE_FILE_OFFSET + 0x20
        data[start : start + 40] = b"\x00" * 40
        runs = padding_runs(bytes(data), 32)
        self.assertEqual(len(runs), 1)
        self.assertEqual(runs[0]["file_offset"], start)
        self.assertEqual(runs[0]["length"], 40)

    def test_reports_external_pointer_and_branch(self) -> None:
        data = bytearray(synthetic_image())
        data[EXECUTABLE_FILE_OFFSET:] = b"\x55" * (
            len(data) - EXECUTABLE_FILE_OFFSET
        )
        cave_offset = EXECUTABLE_FILE_OFFSET + 0x80
        cave_address = file_offset_to_address(cave_offset, len(data))
        pointer_offset = EXECUTABLE_FILE_OFFSET + 0x10
        struct.pack_into("<I", data, pointer_offset, cave_address | 1)

        branch_offset = EXECUTABLE_FILE_OFFSET + 0x20
        branch_address = file_offset_to_address(branch_offset, len(data))
        displacement = cave_address - (branch_address + 4)
        self.assertGreaterEqual(displacement, 0)
        struct.pack_into("<H", data, branch_offset, 0xE000 | (displacement >> 1))

        refs = reference_candidates(bytes(data), cave_address, cave_address + 32)
        self.assertEqual(refs["pointer_candidates"][0]["value"], cave_address | 1)
        self.assertEqual(refs["branch_candidates"][0]["target"], cave_address)


if __name__ == "__main__":
    unittest.main()
