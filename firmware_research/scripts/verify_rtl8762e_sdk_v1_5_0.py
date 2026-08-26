#!/usr/bin/env python3
"""Verify the exact RTL8762E SDK archive used as read-only evidence.

The verifier reads selected files directly from the ZIP.  It does not extract
or execute vendor binaries, connect to a device, build firmware, or authorize
flashing.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import struct
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
    "doc/EN/RTL8762E_OTA_User_Manual_EN-v1.3.pdf": (
        1894351,
        "34bc03506e809d8b1dc1e101fdc8c6b963b53b6a71483b1ed89b2c994da9b131",
    ),
    "doc/EN/RTL8762E_Security_Mechanism_User_Guide_EN-v1.1.pdf": (
        646197,
        "94061ead8303656de443262c6d16bfa18114bec70b8da0a277d6133914744824",
    ),
    "tool/BeeMPTool_v1.1.2.1.zip": (
        35509886,
        "57eb7cf9ce3ce7120706f7d7144d9a2cce3b4c33a0409a3784912afadbdfaec4",
    ),
}

EXPECTED_BEE_MP_TOOL_FILES = {
    "BeeMPTool_v1.1.2.1/doc/RTL87x2x MP Tool User Guide-EN.pdf": (
        4872246,
        "c69ec1d66f0a22e42f2e920ba2463de047117adc2acd13b3f9a006b0add6df5f",
    ),
    "BeeMPTool_v1.1.2.1/BeeMPTool/Release Note.txt": (
        36845,
        "3d62305dc8ff334ba7a8528653eb01335e17c91103d468a4dd748f40bdf83de4",
    ),
    "BeeMPTool_v1.1.2.1/BeeMPTool/MPTool.exe": (
        18672640,
        "268177560c6f694aafdd07998c55403cf3fb725159776d41ca02222da25d841d",
    ),
    "BeeMPTool_v1.1.2.1/BeeMPTool/DLL/rtkmp.dll": (
        2117632,
        "be098cec366fecc5e8d6d2d5b3783acda46a9b17fc0c383890ba9b9198127430",
    ),
    "BeeMPTool_v1.1.2.1/BeeMPTool/DLL/RtkSwdMp.dll": (
        1029120,
        "e2cacddcb2f43cf9e39d6bddeeb190e2b5a0c5d326e2527f33d4b4f6fabc6533",
    ),
    "BeeMPTool_v1.1.2.1/BeeMPTool/DLL/EnableButton.switch": (
        56,
        "dee61d07a7a94c9934480cc3791224afd2c2d519c40bc9dcea75aae5e3242d83",
    ),
    "BeeMPTool_v1.1.2.1/BeeMPTool/Image/RTL8762E_FW_B.bin": (
        17664,
        "0bb4649917a58ed3cbb8c24b19f941f7919bb3e6a83c7b9b881f373621ed1eb6",
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


def extract_ascii_strings(data: bytes, minimum_length: int = 5) -> set[str]:
    pattern = rb"[ -~]{" + str(minimum_length).encode("ascii") + rb",}"
    return {match.decode("ascii") for match in re.findall(pattern, data)}


def extract_utf16le_ascii_strings(data: bytes, minimum_length: int = 5) -> set[str]:
    unit = rb"(?:[ -~]\x00)"
    pattern = unit + b"{" + str(minimum_length).encode("ascii") + rb",}"
    return {match.decode("utf-16le") for match in re.findall(pattern, data)}


def _require_binary_tokens(data: bytes, tokens: tuple[str, ...], source: str) -> None:
    strings = extract_ascii_strings(data) | extract_utf16le_ascii_strings(data)
    missing = [token for token in tokens if not any(token in item for item in strings)]
    if missing:
        raise ValueError(f"{source} is missing expected binary token(s): {missing}")


def parse_pe_export_names(data: bytes) -> set[str]:
    """Return named PE exports without loading or executing the image."""

    if len(data) < 0x40 or data[:2] != b"MZ":
        raise ValueError("not a DOS/PE image")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if pe_offset + 24 > len(data) or data[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise ValueError("missing PE signature")

    section_count = struct.unpack_from("<H", data, pe_offset + 6)[0]
    optional_size = struct.unpack_from("<H", data, pe_offset + 20)[0]
    optional_offset = pe_offset + 24
    if optional_offset + optional_size > len(data):
        raise ValueError("truncated PE optional header")
    magic = struct.unpack_from("<H", data, optional_offset)[0]
    if magic == 0x10B:
        data_directory_offset = optional_offset + 96
    elif magic == 0x20B:
        data_directory_offset = optional_offset + 112
    else:
        raise ValueError(f"unsupported PE optional header magic 0x{magic:04X}")
    if data_directory_offset + 8 > optional_offset + optional_size:
        raise ValueError("PE has no export data directory")
    export_rva, export_size = struct.unpack_from("<II", data, data_directory_offset)
    if export_rva == 0 or export_size == 0:
        return set()

    section_offset = optional_offset + optional_size
    sections: list[tuple[int, int, int, int]] = []
    for index in range(section_count):
        offset = section_offset + index * 40
        if offset + 40 > len(data):
            raise ValueError("truncated PE section table")
        virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from(
            "<IIII", data, offset + 8
        )
        sections.append((virtual_address, virtual_size, raw_offset, raw_size))

    def rva_to_offset(rva: int, length: int = 1) -> int:
        for virtual_address, virtual_size, raw_offset, raw_size in sections:
            span = max(virtual_size, raw_size)
            if virtual_address <= rva and rva + length <= virtual_address + span:
                result = raw_offset + (rva - virtual_address)
                if result + length > len(data):
                    break
                return result
        raise ValueError(f"unmapped PE RVA 0x{rva:X}")

    export_offset = rva_to_offset(export_rva, 40)
    name_count = struct.unpack_from("<I", data, export_offset + 24)[0]
    names_rva = struct.unpack_from("<I", data, export_offset + 32)[0]
    if name_count > 100000:
        raise ValueError(f"unreasonable PE export name count {name_count}")
    names_offset = rva_to_offset(names_rva, name_count * 4)

    result: set[str] = set()
    for index in range(name_count):
        name_rva = struct.unpack_from("<I", data, names_offset + index * 4)[0]
        name_offset = rva_to_offset(name_rva)
        end = data.find(b"\0", name_offset)
        if end < 0:
            raise ValueError("unterminated PE export name")
        result.add(data[name_offset:end].decode("ascii"))
    return result


def _require_pe_exports(data: bytes, names: tuple[str, ...], source: str) -> None:
    exports = parse_pe_export_names(data)
    missing = [name for name in names if name not in exports]
    if missing:
        raise ValueError(f"{source} is missing expected PE export(s): {missing}")


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

    bee_mp_selected: dict[str, bytes] = {}
    bee_mp_archive = selected["tool/BeeMPTool_v1.1.2.1.zip"]
    with zipfile.ZipFile(io.BytesIO(bee_mp_archive)) as archive:
        for name, (expected_size, expected_hash) in EXPECTED_BEE_MP_TOOL_FILES.items():
            try:
                data = archive.read(name)
            except KeyError as error:
                raise ValueError(f"BeeMPTool archive is missing {name}") from error
            digest = hashlib.sha256(data).hexdigest()
            if len(data) != expected_size or digest != expected_hash:
                raise ValueError(
                    "BeeMPTool member mismatch for "
                    f"{name}: size={len(data)}, sha256={digest}"
                )
            bee_mp_selected[name] = data

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

    bee_mp_prefix = "BeeMPTool_v1.1.2.1/BeeMPTool/"
    release_note = bee_mp_selected[bee_mp_prefix + "Release Note.txt"].decode(
        "utf-8", errors="replace"
    )
    rd_switch = bee_mp_selected[bee_mp_prefix + "DLL/EnableButton.switch"].decode(
        "ascii"
    )
    _require_tokens(
        release_note,
        (
            "Version 1.1.2.1",
            "save flash when readall is interrupted",
            "support bee3+ plain text dump",
        ),
        "BeeMPTool Release Note.txt",
    )
    _require_tokens(
        rd_switch,
        ("[RdUIEnable]", "ID_RD_UI_SWITCH=1"),
        "BeeMPTool EnableButton.switch",
    )
    _require_pe_exports(
        bee_mp_selected[bee_mp_prefix + "DLL/rtkmp.dll"],
        (
            "OpenBtMPModulePort",
            "ConnectBtMPFlash",
            "GetBtMPFlashSize",
            "ReadBtMPFlashData",
            "ReadBtMPEfuseData",
            "WriteBtMPFlashData",
        ),
        "BeeMPTool rtkmp.dll",
    )
    _require_pe_exports(
        bee_mp_selected[bee_mp_prefix + "DLL/RtkSwdMp.dll"],
        (
            "LoadBootstrap",
            "ReadMPFlashData",
            "ReadMPEfuseData",
            "WriteMPFlashData",
        ),
        "BeeMPTool RtkSwdMp.dll",
    )
    _require_binary_tokens(
        bee_mp_selected[bee_mp_prefix + "MPTool.exe"],
        (
            "Image\\RTL8762E_FW_B.bin",
            "Get Flash ID",
            "FlashReadAllHandle",
            "./ReadbackFlash.bin",
            "./ReadALL",
        ),
        "BeeMPTool MPTool.exe",
    )

    return {
        "classification": "HASH_LOCKED_OFFICIAL_RTL8762E_SDK_EVIDENCE",
        "archive_sha256": archive_hash,
        "archive_size": path.stat().st_size,
        "verified_member_count": len(selected),
        "verified_nested_bee_mp_member_count": len(bee_mp_selected),
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
        "manual_evidence": {
            "ota_manual_sha256": EXPECTED_FILES[
                "doc/EN/RTL8762E_OTA_User_Manual_EN-v1.3.pdf"
            ][1],
            "ota_manual_printed_page_15": (
                "without bank switching, the boot program moves OTA Temp data "
                "to the image area designated by OTA Bank0 and restarts"
            ),
            "power_loss_atomicity_documented": False,
            "security_manual_sha256": EXPECTED_FILES[
                "doc/EN/RTL8762E_Security_Mechanism_User_Guide_EN-v1.1.pdf"
            ][1],
            "application_encryption_optional": True,
            "swd_depends_on_unread_device_security_level": True,
        },
        "recovery_tool_evidence": {
            "bee_mp_tool_archive_sha256": EXPECTED_FILES[
                "tool/BeeMPTool_v1.1.2.1.zip"
            ][1],
            "mp_tool_manual_sha256": EXPECTED_BEE_MP_TOOL_FILES[
                "BeeMPTool_v1.1.2.1/doc/RTL87x2x MP Tool User Guide-EN.pdf"
            ][1],
            "rtl8762e_uart_loader_sha256": EXPECTED_BEE_MP_TOOL_FILES[
                bee_mp_prefix + "Image/RTL8762E_FW_B.bin"
            ][1],
            "rd_mode_enabled_in_bundled_configuration": True,
            "uart_burn_pins_documented": ["P3_0/UART_TX", "P3_1/UART_RX"],
            "mp_mode_trap_documented": "pull P0_3 low while resetting",
            "flash_readback_export_present": True,
            "efuse_read_export_present": True,
            "swd_flash_read_export_present": True,
            "documented_single_read_limit_consistent": False,
            "documented_single_read_limits": ["16 MiB", "32 MiB"],
            "rtl8762e_read_all_support_proven": False,
            "rtl8762e_readback_plaintext_proven": False,
            "documented_cli_present": False,
            "vendor_executables_signed": False,
            "vendor_executables_executed": False,
            "target_device_operation_performed": False,
        },
        "target_r08_security_level_proven": False,
        "target_r08_mp_test_points_proven": False,
        "target_r08_full_flash_readback_proven": False,
        "target_r08_restore_rehearsal_proven": False,
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
