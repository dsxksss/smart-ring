#!/usr/bin/env python3
"""Audit software-only recovery paths in the exact stock RT08 image.

The report is deliberately fail-closed.  Static references can prove that a
path is present, but their absence cannot prove that no computed indirect call
exists.  Nothing in this script connects to or writes the ring.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any

from analyze_rt08_thumb import (
    address_to_file_offset,
    find_bl_callers,
    find_thumb_ldr_literal_value_sites,
    load_image,
)


EXPECTED_STOCK_SHA256 = (
    "c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b"
)

ROM_SET_HCI_MODE_FLAG = 0x00008A1C
ROM_CHECK_HCI_MODE_FLAG = 0x00008A46
ROM_DFU_CHECK_OTA_MODE_FLAG = 0x0003ED30
ROM_WDG_SYSTEM_RESET = 0x0000029C

LOCAL_DFU_SET_OTA_MODE_FLAG = 0x00826B30
LOCAL_DFU_SWITCH_TO_OTA_MODE = 0x00826B6C
LOCAL_DFU_FW_REBOOT = 0x00826B8A

ANCHORS = (
    (
        "system_init_checks_hci_mode_before_application_setup",
        0x00826664,
        "10 b5 e2 f7 ee d9 00 28 53 d1 4e 48 00 7f c0 07 4f d0",
    ),
    (
        "local_dfu_set_ota_mode_flag_uses_aon_safe_read_write",
        LOCAL_DFU_SET_OTA_MODE_FLAG,
        (
            "38 b5 04 46 02 20 0c f4 2d f2 69 46 08 80 0a 46 20 20 "
            "09 88 00 2c 01 d0 01 43 00 e0 81 43 11 80"
        ),
    ),
    (
        "local_dfu_switch_sets_flag_then_resets_except_aon",
        LOCAL_DFU_SWITCH_TO_OTA_MODE,
        (
            "c7 49 10 b5 00 22 50 31 c6 48 de f7 97 df 01 20 "
            "ff f7 d8 ff d3 21 01 20 d9 f7 8a db 10 bd"
        ),
    ),
    (
        "application_dfu_completion_calls_reboot_wrapper",
        0x0082D6B2,
        "00 20 30 70 01 20 f9 f7 67 fa",
    ),
)


def _checked_anchor(
    data: bytes, name: str, address: int, expected_hex: str
) -> dict[str, Any]:
    expected = bytes.fromhex(expected_hex)
    offset = address_to_file_offset(address, len(data))
    actual = data[offset : offset + len(expected)]
    if actual != expected:
        raise ValueError(
            f"software-recovery anchor {name} mismatch at 0x{address:08x}: "
            f"{actual.hex(' ')}"
        )
    return {
        "name": name,
        "address": f"0x{address:08X}",
        "bytes": expected.hex(" "),
    }


def _call_sites(data: bytes, target: int) -> list[str]:
    return [f"0x{site['address']:08X}" for site in find_bl_callers(data, target)]


def _literal_sites(data: bytes, value: int) -> list[str]:
    return [
        f"0x{site['address']:08X}"
        for site in find_thumb_ldr_literal_value_sites(data, value)
    ]


def analyze(data: bytes) -> dict[str, Any]:
    digest = hashlib.sha256(data).hexdigest()
    if digest != EXPECTED_STOCK_SHA256:
        raise ValueError(f"stock SHA-256 mismatch: {digest}")

    checked = [_checked_anchor(data, *anchor) for anchor in ANCHORS]
    hci_check_callers = _call_sites(data, ROM_CHECK_HCI_MODE_FLAG)
    hci_set_callers = _call_sites(data, ROM_SET_HCI_MODE_FLAG)
    normal_ota_check_callers = _call_sites(data, ROM_DFU_CHECK_OTA_MODE_FLAG)
    switch_callers = _call_sites(data, LOCAL_DFU_SWITCH_TO_OTA_MODE)
    switch_pointer_sites = _literal_sites(data, LOCAL_DFU_SWITCH_TO_OTA_MODE | 1)
    reboot_callers = _call_sites(data, LOCAL_DFU_FW_REBOOT)
    reset_callers = _call_sites(data, ROM_WDG_SYSTEM_RESET)

    if hci_check_callers != ["0x00826666"]:
        raise ValueError(f"unexpected HCI mode check callers: {hci_check_callers}")
    if hci_set_callers:
        raise ValueError(f"unexpected HCI mode setter callers: {hci_set_callers}")
    if normal_ota_check_callers:
        raise ValueError(
            f"unexpected normal OTA pre-main check callers: {normal_ota_check_callers}"
        )
    if switch_callers or switch_pointer_sites:
        raise ValueError(
            "stock image unexpectedly contains a static reference to the local "
            "switch-to-OTA function"
        )
    if reboot_callers != ["0x0082D6B8"]:
        raise ValueError(f"unexpected application DFU reboot callers: {reboot_callers}")

    return {
        "classification": "APPLICATION_DEPENDENT_SOFTWARE_RECOVERY_ONLY",
        "stock_sha256": digest,
        "device_write_performed": False,
        "hci_mode": {
            "early_boot_check_present": True,
            "check_call_sites": hci_check_callers,
            "setter_call_sites": hci_set_callers,
            "application_can_request_hci_mode": False,
            "software_only_entry_proven": False,
            "note": (
                "The early check is present, but the exact application contains no "
                "direct call to the ROM HCI-mode setter. Physical MP/UART entry is "
                "outside the no-disassembly constraint."
            ),
        },
        "normal_pre_main_ble_ota": {
            "rom_check_call_sites": normal_ota_check_callers,
            "compiled_in": False,
            "application_independent_recovery_proven": False,
        },
        "local_switch_to_ota_mode": {
            "function_present": True,
            "direct_call_sites": switch_callers,
            "thumb_pointer_literal_sites": switch_pointer_sites,
            "static_reference_found": False,
            "absence_scope": (
                "No exact Thumb BL or LDR-literal function-pointer reference was found; "
                "this does not mathematically exclude a computed indirect call."
            ),
        },
        "custom_qring_dfu": {
            "application_completion_reboot_call_sites": reboot_callers,
            "requires_running_application": True,
            "can_restore_when_application_and_ble_service_start": True,
            "can_restore_boot_failure": False,
            "can_restore_interrupted_bootloader_copy": False,
        },
        "watchdog_reset": {
            "rom_call_site_count": len(reset_callers),
            "call_sites": reset_callers,
            "proves_runtime_fault_auto_recovery": False,
        },
        "candidate_operational_containment": {
            "hook_address": "0x008280F6",
            "boot_entry_modified": False,
            "feature_default_state": "dormant_until_A1_09_01",
            "hard_stream_timeout_seconds": 12,
            "restores_stock_after_runtime_fault": False,
            "note": (
                "A reset or power cycle should return the feature to its default dormant "
                "state if the patched application still boots. This is containment, not "
                "firmware rollback."
            ),
        },
        "no_disassembly_recovery_options": [
            {
                "option": "borrowed or vendor-supplied same-hardware test ring",
                "covers": (
                    "real install, stock-restore, runtime, disconnect, and controlled "
                    "power-loss testing without risking the only ring"
                ),
                "available_under_current_constraints": False,
            },
            {
                "option": "vendor factory reflash service or signed custom firmware",
                "covers": "a manufacturer-supported recovery or deployment path",
                "available_under_current_constraints": True,
            },
            {
                "option": "application-level trial mode on the only ring",
                "covers": "runtime feature faults only while the application still boots",
                "available_under_current_constraints": True,
            },
        ],
        "remaining_hard_blockers": [
            "no application-independent BLE bootloader recovery is proven",
            "bootloader copy behavior under power loss is unproven",
            "the current ring is the only target device",
            "the user has excluded physical MP/UART/SWD access",
        ],
        "flash_authorized": False,
        "anchors": checked,
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Read-only audit of no-disassembly RT08 recovery options"
    )
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    print(json.dumps(analyze(load_image(args.image)), ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as error:
        print(f"error: {error}")
        raise SystemExit(2)
