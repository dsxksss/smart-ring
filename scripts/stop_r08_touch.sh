#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ! -x target/release/r08 ]]; then
  echo "Building r08..."
  cargo build -p r08 --release
fi

echo "Disabling R08 touch HID mode..."
exec ./target/release/r08 disable-touch --sleep-minutes 1
