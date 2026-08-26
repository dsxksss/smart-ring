import sys
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

from analyze_rt08_software_recovery import analyze  # noqa: E402
from analyze_rt08_thumb import address_to_file_offset, load_image  # noqa: E402


STOCK = REPO_ROOT / "research_artifacts" / "firmware" / "RT08_3.10.48_260309.bin"


class SoftwareRecoveryAuditTests(unittest.TestCase):
    def test_exact_stock_is_application_dependent_only(self):
        report = analyze(load_image(STOCK))
        self.assertEqual(
            report["classification"],
            "APPLICATION_DEPENDENT_SOFTWARE_RECOVERY_ONLY",
        )
        self.assertTrue(report["hci_mode"]["early_boot_check_present"])
        self.assertFalse(report["hci_mode"]["application_can_request_hci_mode"])
        self.assertFalse(report["normal_pre_main_ble_ota"]["compiled_in"])
        self.assertTrue(report["local_switch_to_ota_mode"]["function_present"])
        self.assertFalse(report["local_switch_to_ota_mode"]["static_reference_found"])
        self.assertTrue(report["custom_qring_dfu"]["requires_running_application"])
        self.assertTrue(
            report["custom_qring_dfu"][
                "can_restore_when_application_and_ble_service_start"
            ]
        )
        self.assertFalse(report["custom_qring_dfu"]["can_restore_boot_failure"])
        self.assertFalse(
            report["watchdog_reset"]["proves_runtime_fault_auto_recovery"]
        )
        self.assertFalse(report["flash_authorized"])
        self.assertEqual(len(report["anchors"]), 4)

    def test_tampered_stock_is_rejected(self):
        data = bytearray(load_image(STOCK))
        offset = address_to_file_offset(0x00826664, len(data))
        data[offset] ^= 1
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            analyze(bytes(data))


if __name__ == "__main__":
    unittest.main()
