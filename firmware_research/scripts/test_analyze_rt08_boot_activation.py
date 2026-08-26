import sys
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

from analyze_rt08_boot_activation import analyze  # noqa: E402
from analyze_rt08_thumb import address_to_file_offset, load_image  # noqa: E402


STOCK = REPO_ROOT / "research_artifacts" / "firmware" / "RT08_3.10.48_260309.bin"


class BootActivationTests(unittest.TestCase):
    def test_exact_stock_activation_path_remains_non_flashable(self):
        report = analyze(load_image(STOCK))
        self.assertTrue(report["ota_end_reaches_activation_wrapper"])
        self.assertFalse(report["integrity_check_en_in_boot"])
        self.assertTrue(report["stored_sha256_all_zero"])
        self.assertFalse(report["rom_api_names_proven"])
        self.assertFalse(report["equal_version_bank_selection_proven"])
        self.assertFalse(report["runtime_crash_rollback_proven"])
        self.assertFalse(report["power_loss_recovery_proven"])
        self.assertFalse(report["flash_authorized"])
        self.assertEqual(len(report["anchors"]), 6)

    def test_tampered_stock_is_rejected_before_semantic_claims(self):
        data = bytearray(load_image(STOCK))
        offset = address_to_file_offset(0x00826F68, len(data))
        data[offset] ^= 1
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            analyze(bytes(data))


if __name__ == "__main__":
    unittest.main()
