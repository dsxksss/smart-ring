from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from verify_r08_readback_pair import compare_readbacks


class ReadbackPairTests(unittest.TestCase):
    def make_pair(self, first: bytes, second: bytes) -> tuple[tempfile.TemporaryDirectory, Path, Path]:
        directory = tempfile.TemporaryDirectory()
        root = Path(directory.name)
        first_path = root / "cold-boot-a.bin"
        second_path = root / "cold-boot-b.bin"
        first_path.write_bytes(first)
        second_path.write_bytes(second)
        return directory, first_path, second_path

    def test_identical_pair_proves_only_region_repeatability(self) -> None:
        directory, first, second = self.make_pair(b"\xA5" * 0x1000, b"\xA5" * 0x1000)
        self.addCleanup(directory.cleanup)
        report = compare_readbacks(
            first,
            second,
            base_address=0x00800000,
            expected_length=0x1000,
            region_name="unknown-first-page",
            encryption_status="encrypted",
        )
        self.assertTrue(report["readback_repeatability_proven"])
        self.assertTrue(report["comparison"]["byte_equal"])
        self.assertEqual(report["comparison"]["differing_byte_count"], 0)
        self.assertFalse(report["writeback_semantics_proven"])
        self.assertFalse(report["restore_proven"])
        self.assertFalse(report["full_device_backup_proven"])
        self.assertFalse(report["flash_authorized"])

    def test_reports_exact_first_difference(self) -> None:
        left = bytearray(b"\x00" * 0x1000)
        right = bytearray(left)
        right[0x123] = 1
        directory, first, second = self.make_pair(bytes(left), bytes(right))
        self.addCleanup(directory.cleanup)
        report = compare_readbacks(
            first,
            second,
            base_address=0x00872000,
            expected_length=0x1000,
            region_name="persistent-page-0",
            encryption_status="unknown",
        )
        self.assertFalse(report["readback_repeatability_proven"])
        self.assertEqual(report["comparison"]["differing_byte_count"], 1)
        self.assertEqual(report["comparison"]["first_difference_offset"], 0x123)
        self.assertEqual(report["comparison"]["first_difference_address"], "0x00872123")

    def test_rejects_wrong_length_even_when_files_match(self) -> None:
        directory, first, second = self.make_pair(b"\x00" * 0x1000, b"\x00" * 0x1000)
        self.addCleanup(directory.cleanup)
        report = compare_readbacks(
            first,
            second,
            base_address=0x00800000,
            expected_length=0x2000,
            region_name="declared-two-pages",
            encryption_status="plaintext",
        )
        self.assertTrue(report["comparison"]["byte_equal"])
        self.assertFalse(report["comparison"]["lengths_match_expected"])
        self.assertFalse(report["readback_repeatability_proven"])

    def test_rejects_same_path_and_unaligned_ranges(self) -> None:
        directory, first, second = self.make_pair(b"\x00" * 0x1000, b"\x00" * 0x1000)
        self.addCleanup(directory.cleanup)
        with self.assertRaisesRegex(ValueError, "different files"):
            compare_readbacks(
                first,
                first,
                base_address=0x00800000,
                expected_length=0x1000,
                region_name="same-file",
                encryption_status="unknown",
            )
        with self.assertRaisesRegex(ValueError, "aligned"):
            compare_readbacks(
                first,
                second,
                base_address=0x00800001,
                expected_length=0x1000,
                region_name="bad-base",
                encryption_status="unknown",
            )


if __name__ == "__main__":
    unittest.main()
