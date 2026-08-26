#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ! -x target/release/r08 ]]; then
  echo "Building r08..."
  cargo build -p r08 --release
fi

echo "Close the phone ring app and turn off phone Bluetooth first."
echo "Injection starts after CONTROL_READY. Press Enter in this terminal to stop."
exec ./target/release/r08 control --touch-type 2 --sleep-minutes 1 --scroll-gain 4
