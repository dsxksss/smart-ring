#!/usr/bin/env python3
"""Export only R08-relevant lines from a verbose local Bleak capture log."""

from __future__ import annotations

import argparse
from pathlib import Path


ALLOWED_PREFIXES = (
    "FOUND R08_9C07 ",
    "ADVERTISEMENT ",
    "CONNECTED ",
    "SENSOR_STARTED ",
    "RX ",
    "SAMPLE ",
    "SENSOR_STOP_REQUESTED ",
    "SENSOR_SUMMARY ",
)


def sanitize_lines(lines: list[str]) -> list[str]:
    output: list[str] = []
    for line in lines:
        clean = line.rstrip("\r\n")
        if clean.startswith(ALLOWED_PREFIXES):
            output.append(clean)
    return output


def export_log(source: Path, output: Path) -> int:
    if output.exists():
        raise FileExistsError(f"不会覆盖已有文件：{output}")
    raw = source.read_bytes()
    if raw.startswith((b"\xff\xfe", b"\xfe\xff")):
        text = raw.decode("utf-16")
    else:
        text = raw.decode("utf-8", errors="strict")
    lines = text.splitlines()
    sanitized = sanitize_lines(lines)
    if not any(line.startswith("SENSOR_SUMMARY ") for line in sanitized):
        raise ValueError("源日志没有完整 SENSOR_SUMMARY，拒绝导出不完整证据")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n".join(sanitized) + "\n", encoding="utf-8")
    return len(sanitized)


def main() -> int:
    parser = argparse.ArgumentParser(description="导出脱敏后的 R08 传感器证据")
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    count = export_log(args.source, args.output)
    print(f"EXPORTED_LINES={count}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
