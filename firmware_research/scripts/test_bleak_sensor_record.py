import unittest

from bleak_sensor_record import RAW_START, RAW_STOP, decode_accelerometer


class BleakSensorRecordTests(unittest.TestCase):
    def test_packets_have_expected_payload_and_checksum(self):
        self.assertEqual(RAW_START[:3], bytes.fromhex("A1 04 04"))
        self.assertEqual(RAW_STOP[:2], bytes.fromhex("A1 02"))
        self.assertEqual(sum(RAW_START[:15]) & 0xFF, RAW_START[15])
        self.assertEqual(sum(RAW_STOP[:15]) & 0xFF, RAW_STOP[15])

    def test_decodes_signed_12_bit_axes(self):
        packet = bytearray(16)
        packet[:8] = bytes.fromhex("A1 03 00 02 FF 0F 80 00")
        packet[15] = sum(packet[:15]) & 0xFF
        self.assertEqual(decode_accelerometer(bytes(packet)), (-2048, 2, -1))

    def test_rejects_wrong_or_corrupt_packet(self):
        self.assertIsNone(decode_accelerometer(bytes(16)))
        packet = bytearray(RAW_START)
        packet[0] = 0xA1
        packet[1] = 0x03
        self.assertIsNone(decode_accelerometer(bytes(packet)))


if __name__ == "__main__":
    unittest.main()
