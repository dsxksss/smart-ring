#!/usr/bin/env python3
"""Verify the exact RTL8762E SDK archive used as read-only evidence.

The verifier reads selected files directly from the ZIP.  It does not extract
or execute vendor binaries, connect to a device, build firmware, or authorize
flashing.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import zipfile
from pathlib import Path
from typing import Any


EXPECTED_ARCHIVE_SHA256 = (
    "ef1f47b83d60aeb54edb83a34ecc9d92218965ab18ff02256773441d35f7db52"
)

EXPECTED_FILES = {
    "bin/gcc/rom_symbol_gcc.axf": (
        15182,
        "b3ee1a71a933ef5b20c5107f72826242ef8cbbeb0a3d6208c2641259f278612d",
    ),
    "inc/platform/patch_header_check.h": (
        10489,
        "14a82f325d07b1702fa7acbf7c078662b5131fb4f7ce0fb94e934f5b34c67ba8",
    ),
    "src/platform/dfu_flash.c": (
        29038,
        "0b82a80597740af6bea9f53b7ed6e631bccee30509a3fe9417b905bf1295c906",
    ),
    "src/ble/profile/server/dfu_service.c": (
        49687,
        "19ecefd3e1603c0c69555a248ae43b15679cfecaa8c573a00923179443d04167",
    ),
    "src/mcu/rtl876x/system_rtl876x.c": (
        42412,
        "d784cc2394b0a2bcc4d39f6ffcdcf4efa655091105238394565afc075968ab79",
    ),
    "bin/default_bin/disable_bank_switch/flash map.ini": (
        1950,
        "1c4b4ab65cedac3d8a9a15fae1a910e2849dbee996e29d571bb947b39e0a7249",
    ),
    "bin/default_bin/disable_bank_switch/flash_map.h": (
        6752,
        "0908904c8ab804c5c63848d91aabdf9f0c74485974888b929029595cd6525c83",
    ),
}

EXPECTED_ROM_SYMBOLS = {
    "flash_get_bank_addr": 0x000080B8,
    "check_image_chksum": 0x00008A5C,
    "check_header_valid": 0x00008A82,
    "get_header_addr_by_img_id": 0x00008AE2,
    "is_ota_support_bank_switch": 0x00008B7A,
    "get_temp_ota_bank_addr_by_img_id": 0x00008B94,
    "dfu_set_ready": 0x0003ED1A,
}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def parse_rom_symbols(data: bytes) -> dict[str, int]:
    text = data.decode("ascii")
    result: dict[str, int] = {}
    for name, address in re.findall(
        r"^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*0x([0-9a-fA-F]+)\s*;",
        text,
        flags=re.MULTILINE,
    ):
        # Function symbols carry the Thumb bit.  Reports use normalized code
        # addresses so they can be compared directly with disassembly targets.
        result[name] = int(address, 16) & ~1
    return result


def _require_tokens(text: str, tokens: tuple[str, ...], source: str) -> None:
    missing = [token for token in tokens if token not in text]
    if missing:
        raise ValueError(f"{source} is missing expected evidence token(s): {missing}")


def verify_sdk_archive(path: Path) -> dict[str, Any]:
    archive_hash = sha256_file(path)
    if archive_hash != EXPECTED_ARCHIVE_SHA256:
        raise ValueError(f"SDK archive SHA-256 mismatch: {archive_hash}")

    selected: dict[str, bytes] = {}
    with zipfile.ZipFile(path) as archive:
        for name, (expected_size, expected_hash) in EXPECTED_FILES.items():
            try:
                data = archive.read(name)
            except KeyError as error:
                raise ValueError(f"SDK archive is missing {name}") from error
            digest = hashlib.sha256(data).hexdigest()
            if len(data) != expected_size or digest != expected_hash:
                raise ValueError(
                    f"SDK member mismatch for {name}: size={len(data)}, sha256={digest}"
                )
            selected[name] = data

    symbols = parse_rom_symbols(selected["bin/gcc/rom_symbol_gcc.axf"])
    actual_symbols = {name: symbols.get(name) for name in EXPECTED_ROM_SYMBOLS}
    if actual_symbols != EXPECTED_ROM_SYMBOLS:
        raise ValueError(f"unexpected RTL8762E ROM symbol mapping: {actual_symbols}")

    header = selected["inc/platform/patch_header_check.h"].decode(
        "utf-8", errors="strict"
    )
    dfu_flash = selected["src/platform/dfu_flash.c"].decode(
        "utf-8", errors="strict"
    )
    dfu_service = selected["src/ble/profile/server/dfu_service.c"].decode(
        "utf-8", errors="strict"
    )
    flash_map = selected[
        "bin/default_bin/disable_bank_switch/flash map.ini"
    ].decode("ascii")

    _require_tokens(
        header,
        (
            "OTA         = 0x2790",
            "AppPatch    = 0x2793",
            "uint16_t not_ready : 1",
            "T_VERSION_FORMAT git_ver",
            "uint32_t ver_val",
        ),
        "patch_header_check.h",
    )
    _require_tokens(
        dfu_flash,
        (
            "uint32_t dfu_report_target_fw_info",
            "p_ota_header->ver_val",
            "p_header->git_ver.ver_info.version",
            "bool dfu_check_checksum",
            "get_temp_ota_bank_addr_by_img_id",
            "check_image_chksum(p_header)",
            "dfu_set_image_ready(p_header)",
        ),
        "dfu_flash.c",
    )
    _require_tokens(
        dfu_service,
        (
            "void dfu_service_handle_active_image(void)",
            "if (!is_ota_support_bank_switch())",
            "dfu_set_image_ready(p_header)",
            "case DFU_OPCODE_ACTIVE_IMAGE_RESET",
            "unlock_flash_bp_all()",
        ),
        "dfu_service.c",
    )
    _require_tokens(
        flash_map,
        (
            "OTA_SWITCH=Disable",
            "OTA_BANK1_SIZE=0x00000000",
            "OTA_TMP_SIZE=0x00025000",
            "BANK1_APP_SIZE=0x00000000",
        ),
        "disable-bank-switch flash map",
    )

    return {
        "classification": "HASH_LOCKED_OFFICIAL_RTL8762E_SDK_EVIDENCE",
        "archive_sha256": archive_hash,
        "archive_size": path.stat().st_size,
        "verified_member_count": len(selected),
        "rom_symbols": {
            name: f"0x{address:08X}" for name, address in actual_symbols.items()
        },
        "structure_fields": {
            "ota_image_id": "0x2790",
            "application_image_id": "0x2793",
            "application_version_field": "T_IMG_HEADER_FORMAT.git_ver.ver_info.version",
            "ota_version_field": "T_OTA_HEADER_FORMAT.ver_val",
            "ota_version_field_offset": "0x194",
            "application_version_field_offset": "0x60",
        },
        "disabled_bank_switch_reference": {
            "bank1_size": 0,
            "bank1_application_size": 0,
            "temporary_ota_size": "0x25000",
            "activation_marks_staged_images_ready_before_reset": True,
            "bootloader_copy_path_documented_by_sdk_control_flow": True,
        },
        "target_r08_flash_map_proven": False,
        "installed_r08_header_readback_proven": False,
        "power_loss_recovery_proven": False,
        "runtime_crash_rollback_proven": False,
        "flash_authorized": False,
        "safety_note": (
            "This report authenticates selected SDK source and symbol evidence only. "
            "The SDK reference layout is not the target R08 flash map, and no vendor "
            "program was executed or device operation performed."
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Verify an RTL8762E SDK v1.5.0 archive as read-only evidence"
    )
    parser.add_argument("archive", type=Path)
    args = parser.parse_args()
    if not args.archive.is_file():
        parser.error(f"file not found: {args.archive}")
    try:
        report = verify_sdk_archive(args.archive)
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        parser.error(str(error))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
