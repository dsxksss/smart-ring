//! Target ring identity. Ordinary mice and nearby BLE devices must not match.

pub const RING_NAME: &str = "R08_9C07";
pub const RING_MAC: &str = "31:31:45:37:9C:07";
pub const RING_MAC_COMPACT: &str = "313145379C07";

pub fn compact_address(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .map(|ch| ch.to_ascii_uppercase())
        .collect()
}

pub fn is_ring_name(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case(RING_NAME)
}

pub fn address_looks_like_ring(address: &str) -> bool {
    let compact = compact_address(address);
    compact.contains(RING_MAC_COMPACT) || address.to_ascii_uppercase().contains(RING_MAC)
}

pub fn is_ring_advertisement(name: Option<&str>, address: &str) -> bool {
    name.is_some_and(is_ring_name) || address_looks_like_ring(address)
}

pub fn is_ring_hid_identity(name: &str, unique_id: &str, device_path: &str) -> bool {
    let blob = format!("{name} {unique_id} {device_path}");
    is_ring_name(name)
        || blob.to_ascii_uppercase().contains(RING_MAC_COMPACT)
        || blob.to_ascii_uppercase().contains(RING_MAC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_known_ring_and_rejects_neighbors() {
        assert!(is_ring_advertisement(Some("R08_9C07"), ""));
        assert!(is_ring_advertisement(None, "31:31:45:37:9C:07"));
        assert!(is_ring_hid_identity(
            "HID-compliant mouse",
            "",
            r"\\?\HID#BTHLEDEVICE&COL03#313145379C07"
        ));
        assert!(!is_ring_advertisement(Some("LH-FG2C"), "AA:BB:CC:DD:EE:FF"));
        assert!(!is_ring_hid_identity(
            "Logitech USB Receiver",
            "usb-1",
            "/dev/input/event3"
        ));
    }
}
