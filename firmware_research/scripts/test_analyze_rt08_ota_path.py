import sys
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

from analyze_rt08_ota_path import (  # noqa: E402
    ADJACENT_STORAGE_BASE,
    ADJACENT_STORAGE_OBSERVED_SPAN,
    INACTIVE_APP_BASE,
    INACTIVE_APP_END,
    analyze,
)
from analyze_rt08_thumb import address_to_file_offset, load_image  # noqa: E402


STOCK = REPO_ROOT / "research_artifacts" / "firmware" / "RT08_3.10.48_260309.bin"


class OtaPathTests(unittest.TestCase):
    def test_exact_stock_path_and_capacity(self):
        report = analyze(load_image(STOCK))
        self.assertEqual(report["inactive_app_base"], f"0x{INACTIVE_APP_BASE:08X}")
        self.assertEqual(report["inactive_app_end"], f"0x{INACTIVE_APP_END:08X}")
        self.assertEqual(report["inactive_app_capacity"], 0x24000)
        self.assertEqual(
            report["adjacent_storage_base"], f"0x{ADJACENT_STORAGE_BASE:08X}"
        )
        self.assertEqual(
            report["adjacent_storage_observed_minimum_span"],
            ADJACENT_STORAGE_OBSERVED_SPAN,
        )
        self.assertEqual(report["stock_remaining_bytes"], 724)
        self.assertEqual(len(report["additional_application_storage_descriptors"]), 10)
        self.assertEqual(report["highest_observed_storage_end"], "0x00880000")
        self.assertEqual(report["unclassified_gap"]["base"], "0x0087A000")
        self.assertEqual(
            report["packaged_application_addresses"],
            {
                "image_base_candidate": "0x00826000",
                "exe_base": "0x00826400",
                "load_base": "0x00826400",
            },
        )
        self.assertTrue(report["active_slot_xip_address_compatible"])
        self.assertFalse(report["staging_slot_xip_address_compatible"])
        self.assertFalse(report["separate_bank1_relocated_application_present"])
        self.assertFalse(report["separate_ota_header_present_in_package"])
        self.assertEqual(
            report["ota_layout_assessment"],
            "SINGLE_BANK_COPY_IMAGE_CONSISTENT",
        )
        self.assertFalse(
            report["ota_layout_assessment_proven_by_runtime_or_rom_symbols"]
        )
        self.assertFalse(report["address_remap_during_bank_switch_proven"])
        self.assertFalse(report["old_application_survives_activation_proven"])
        self.assertFalse(report["physical_flash_capacity_proven"])
        self.assertFalse(report["bootloader_rollback_proven"])
        self.assertFalse(report["flash_authorized"])

    def test_anchor_tamper_is_rejected_even_with_hash_check_bypassed(self):
        data = bytearray(load_image(STOCK))
        offset = address_to_file_offset(0x0082F080, len(data))
        data[offset] ^= 1
        # The exact stock hash check is itself fail-closed and should trigger
        # before any semantic conclusion is returned.
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            analyze(bytes(data))


if __name__ == "__main__":
    unittest.main()
