# RT08 touch-controller four-channel path

This note records only conclusions checked against the exact
`RT08_3.10.48_260309.bin` application image for hardware `RT08_V3.1`.

## Confirmed read path

- `0x00834A16` is the only four-channel snapshot function.
- It requires the touch-controller state byte at `0x0020C1E8` to equal 1.
- It reads two bytes from each controller register `0x61`, `0x65`, `0x69`, and
  `0x6D` through the existing serialized driver at `0x0083480C`.
- It byte-swaps the four 16-bit results before copying them to the caller.
- Its boolean result is true only when the final transfer succeeds and all four
  values are greater than 1000. This is a validity check, not a confirmed
  contact bit.
- The only caller is `0x00827EE6` in the stock common diagnostic snapshot
  packer.

The returned COLMI notification is:

| Byte | Meaning |
| --- | --- |
| 0..1 | `A1 04` |
| 2..3 | C1, big-endian `u16` |
| 4..5 | C2, big-endian `u16` |
| 6..7 | C3, big-endian `u16` |
| 8..9 | C4, big-endian `u16` |
| 10 | four-channel function validity result |
| 11..14 | zero in the stock packer |
| 15 | additive COLMI checksum |

The incoming one-shot snapshot payload is `A1 03`. This selects case 3 in the
stock A1 dispatcher and emits one common `A1 01..05` snapshot. It is not the
`A1 04 04` recurring optical/raw-sensor start command and does not start host
input injection.

## Host observation command

```powershell
cargo run -p r08 --release --bin r08 -- touch-raw --seconds 30 --interval-ms 500
```

The command prints C1 through C4, the validity result, minimum, maximum, and
spread. For the first hardware comparison, keep the ring untouched for several
samples, then hold the upper half of the touch area, release it, and finally
hold the lower half. Do not slide during this first comparison.

## Still unverified

- Whether contact raises or lowers each channel relative to its own baseline.
- Which physical part of the touch area corresponds to each channel.
- Whether a stationary hold keeps a stable delta until release.
- The safe contact/release hysteresis and required debounce duration.
- Whether 4 Hz observation is sufficient or a firmware-internal 10 Hz read is
  required for responsive scrolling.

Until those values are measured, a v12 firmware that labels C1/C2 as
upper/lower would be guesswork. No v12 candidate or flash authorization exists
at this stage.

## Intended fail-closed state machine after mapping

1. Maintain a per-channel untouched baseline while no contact is present.
2. Require a debounced, sufficiently large delta before entering hold mode.
3. Compare calibrated upper/lower aggregate weights with hysteresis; an
   ambiguous center contact produces no wheel output.
4. Emit only a slow fixed `wheel=+1` or `wheel=-1` report at a bounded interval.
   Mouse X/Y and all buttons remain zero.
5. Release, stale data, failed I2C reads, or invalid channel values immediately
   clears the direction and stops output.

`firmware_research/scripts/analyze_rt08_touch_driver.py` verifies the exact
function, caller, register immediates, response layout, and binary anchors.
