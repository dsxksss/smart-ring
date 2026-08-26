#!/usr/bin/env python3
"""Disassemble a raw Thumb blob for offline firmware-patch review."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from analyze_rt08_thumb import load_disassembler, parse_int


def main() -> int:
    parser = argparse.ArgumentParser(description="Disassemble a raw Thumb binary")
    parser.add_argument("blob", type=Path)
    parser.add_argument("--base", type=parse_int, required=True)
    parser.add_argument("--offset", type=parse_int, default=0)
    parser.add_argument("--code-bytes", type=parse_int)
    parser.add_argument("--engine-path", type=Path)
    args = parser.parse_args()

    data = args.blob.read_bytes()
    if args.offset < 0 or args.offset > len(data):
        parser.error("--offset is outside the input blob")
    available = data[args.offset :]
    code = available if args.code_bytes is None else available[: args.code_bytes]
    disassembler = load_disassembler(args.engine_path)
    instructions = [
        {
            "address": instruction.address,
            "bytes": instruction.bytes.hex(" "),
            "mnemonic": instruction.mnemonic,
            "operands": instruction.op_str,
        }
        for instruction in disassembler.disasm(code, args.base)
    ]
    print(
        json.dumps(
            {
                "blob": str(args.blob),
                "base": args.base,
                "offset": args.offset,
                "size": len(data),
                "code_bytes": len(code),
                "instructions": instructions,
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
