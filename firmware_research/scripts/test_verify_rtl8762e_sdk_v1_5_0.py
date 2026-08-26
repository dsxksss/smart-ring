from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from verify_rtl8762e_sdk_v1_5_0 import (
    EXPECTED_ROM_SYMBOLS,
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


if __name__ == "__main__":
    unittest.main()
