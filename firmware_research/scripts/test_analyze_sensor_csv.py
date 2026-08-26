import tempfile
import unittest
from pathlib import Path

from analyze_sensor_csv import analyze_file, percentile


class AnalyzeSensorCsvTests(unittest.TestCase):
    def test_percentile_interpolates(self):
        self.assertEqual(percentile([0.0, 10.0], 0.5), 5.0)

    def test_analyzes_rate_and_axis_ranges(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "sample.csv"
            path.write_text(
                "elapsed_ms,delta_ms,x,y,z,magnitude\n"
                "100,,1,2,3,3.742\n"
                "200,100,-2,4,7,8.307\n"
                "300,100,5,-1,9,10.344\n",
                encoding="utf-8",
            )
            result = analyze_file(path)
        self.assertEqual(result["sample_count"], 3)
        self.assertAlmostEqual(result["effective_hz"], 10.0)
        self.assertEqual(result["axes"]["x"]["range"], 7)
        self.assertEqual(result["axes"]["y"]["range"], 5)
        self.assertEqual(result["axes"]["z"]["range"], 6)


if __name__ == "__main__":
    unittest.main()
