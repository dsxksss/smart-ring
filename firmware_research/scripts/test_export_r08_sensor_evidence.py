import tempfile
import unittest
from pathlib import Path

from export_r08_sensor_evidence import export_log, sanitize_lines


class ExportR08SensorEvidenceTests(unittest.TestCase):
    def test_removes_neighbor_scan_and_windows_adapter_lines(self):
        source = [
            "DEBUG:bleak:Received AA:BB:CC:DD:EE:FF: neighbor",
            "FOUND R08_9C07 31:31:45:37:9C:07",
            "RX A1 03 00 00",
            "SENSOR_SUMMARY {\"sample_count\": 1}",
        ]
        self.assertEqual(
            sanitize_lines(source),
            source[1:],
        )

    def test_exports_complete_log_without_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.log"
            output = root / "public.log"
            source.write_text(
                "DEBUG neighbor\n"
                "FOUND R08_9C07 31:31:45:37:9C:07\n"
                "SENSOR_SUMMARY {\"sample_count\": 1}\n",
                encoding="utf-8",
            )
            self.assertEqual(export_log(source, output), 2)
            self.assertNotIn("neighbor", output.read_text(encoding="utf-8"))
            with self.assertRaises(FileExistsError):
                export_log(source, output)

    def test_reads_powershell_utf16_log(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.log"
            output = root / "public.log"
            source.write_text(
                "FOUND R08_9C07 31:31:45:37:9C:07\r\n"
                "SENSOR_SUMMARY {\"sample_count\": 1}\r\n",
                encoding="utf-16",
            )
            self.assertEqual(export_log(source, output), 2)
            self.assertTrue(output.read_text(encoding="utf-8").startswith("FOUND R08"))


if __name__ == "__main__":
    unittest.main()
