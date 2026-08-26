#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ! -x target/release/r08 ]]; then
  echo "Building r08..."
  cargo build -p r08 --release
fi

echo "Close the phone ring app and turn off phone Bluetooth first."
echo "The numeric menu starts with touch and computer control disabled."
echo "Choose 2 to start computer control, or 0 to exit safely."
exec ./target/release/r08 interactive --touch-type 2 --sleep-minutes 1 --scroll-gain 4
