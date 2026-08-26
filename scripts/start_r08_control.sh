#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ! -x target/release/r08 ]]; then
  echo "Building r08..."
  cargo build -p r08 --release
fi

echo "Close the phone ring app and turn off phone Bluetooth first."
echo "Control starts automatically. Double-tap the sleeping ring to wake it."
echo "Press Enter or Ctrl+C to exit safely."
exec ./target/release/r08
