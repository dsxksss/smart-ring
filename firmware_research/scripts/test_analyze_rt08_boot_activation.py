import sys
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

from analyze_rt08_boot_activation import analyze  # noqa: E402
from analyze_rt08_thumb import address_to_file_offset, load_image  # noqa: E402


STOCK = (
    REPO_ROOT
    / "firmware_research"
    / "evidence"
    / "ota"
    / "RT08_3.10.48_260309.bin"
)


@unittest.skipUnless(STOCK.exists(), "exact stock image not present locally")
class BootActivationTests(unittest.TestCase):
    def test_exact_stock_activation_path_remains_non_flashable(self):
        report = analyze(load_image(STOCK))
        self.assertTrue(report["ota_end_reaches_activation_wrapper"])
        self.assertFalse(report["integrity_check_en_in_boot"])
        self.assertFalse(report["stored_sha256_all_zero"])
        self.assertEqual(report["stored_sha256_offset"], "0x174")
        self.assertEqual(
            report["stored_sha256"],
            "3e143d383a69b749ed928345ac04d517d7aefb95ecc0f2f4eafbe9fd9b146f8f",
        )
        self.assertEqual(
            report["downloaded_payload_type"],
            "single_rtl8762e_application_image",
        )
        self.assertFalse(report["separate_ota_bank_header_present_in_downloaded_payload"])
        self.assertEqual(
            report["packaged_application_flags"],
            {"not_ready": True, "not_obsolete": True},
        )
        self.assertFalse(report["installed_application_flags_read_from_device"])
        self.assertEqual(
            report["activation_arguments"],
            {"image_id": "0x2793", "second_argument": 0},
        )
        self.assertFalse(report["application_git_version_passed_as_activation_argument"])
        self.assertTrue(report["raw_version_semantics_proven"])
        self.assertEqual(
            report["application_git_version"],
            {
                "raw": "0x00001041",
                "major": 1,
                "minor": 4,
                "revision": 1,
                "reserve": 0,
                "commit_id": "0x1201A39E",
            },
        )
        self.assertTrue(
            report["activation_address_dataflow"][
                "validation_success_required_before_commit"
            ]
        )
        self.assertTrue(report["rom_api_names_proven"])
        self.assertFalse(report["ota_bank_header_update_proven"])
        self.assertTrue(report["staged_application_flag_transition_proven"])
        self.assertFalse(report["application_flag_transition_proven"])
        self.assertFalse(report["equal_version_bank_selection_proven"])
        self.assertFalse(report["runtime_crash_rollback_proven"])
        self.assertFalse(report["power_loss_recovery_proven"])
        self.assertFalse(report["flash_authorized"])
        self.assertEqual(len(report["anchors"]), 7)

    def test_tampered_stock_is_rejected_before_semantic_claims(self):
        data = bytearray(load_image(STOCK))
        offset = address_to_file_offset(0x00826F68, len(data))
        data[offset] ^= 1
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            analyze(bytes(data))


if __name__ == "__main__":
    unittest.main()
