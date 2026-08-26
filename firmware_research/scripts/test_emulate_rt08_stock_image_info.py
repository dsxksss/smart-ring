from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

from analyze_rt08_thumb import load_image
from emulate_rt08_stock_image_info import validate_image_info


UNICORN_AVAILABLE = importlib.util.find_spec("unicorn") is not None
STOCK = (
    Path(__file__).resolve().parents[2]
    / "research_artifacts"
    / "firmware"
    / "RT08_3.10.48_260309.bin"
)


@unittest.skipUnless(UNICORN_AVAILABLE and STOCK.exists(), "analysis dependency/image missing")
class StockImageInfoEmulationTests(unittest.TestCase):
    def test_image_id_branching_and_field_offsets(self) -> None:
        report = validate_image_info(load_image(STOCK))
        self.assertEqual(report["scenario_count"], 7)
        self.assertEqual(
            report["observed_descriptor_field_offsets"],
            {
                "ota_header_0x2790": "0x194",
                "image_ids_0x2791_through_0x279A_and_0xFFFE": "0x60",
            },
        )
        self.assertTrue(
            report["ota_header_and_application_use_distinct_descriptor_fields"]
        )
        self.assertFalse(report["ota_header_field_semantics_proven"])
        self.assertFalse(report["application_field_semantics_proven"])
        self.assertFalse(report["rom_api_names_proven"])
        self.assertFalse(report["installed_ota_header_readback_proven"])
        self.assertFalse(report["bank_selection_proven"])
        self.assertFalse(report["runtime_rollback_proven"])
        self.assertFalse(report["flash_authorized"])

    def test_tampered_stock_is_rejected_before_execution(self) -> None:
        stock = bytearray(load_image(STOCK))
        stock[-1] ^= 1
        with self.assertRaisesRegex(ValueError, "stock SHA-256 mismatch"):
            validate_image_info(bytes(stock))


if __name__ == "__main__":
    unittest.main()
