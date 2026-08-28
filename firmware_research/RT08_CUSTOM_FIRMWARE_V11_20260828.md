# RT08 custom firmware v11: contact-gated, slower monotonic wheel

Date: 2026-08-28. Target is only `R08_9C07 / RT08_V3.1`.

## Physical finding that motivated v11

The installed v10 proved that native touch scrolling reaches Windows while
pointer X/Y and mouse buttons remain suppressed. Physical acceptance also
showed that one swipe could briefly scroll in both directions and felt too
fast.

Read-only disassembly found two stock vertical gesture arrays at
`0x008478E2` and `0x0084794A`. Each starts with an opposite-direction
calibration sample while the stock button byte is zero. The actual contact
phase then contains four `abs(Y)=8` and two `abs(Y)=16` samples in one stable
direction, followed by button-up and unrelated tail entries. v10 converted
every signed Y sample, including the button-up calibration and tail, into a
wheel report.

## Exact rewrite

v11 keeps the v10 hook, connection guard, attribute index 4 sender and blocked
extended report path. Its 42-byte thunk at `0x00829FD6` emits wheel input only
when all of these are true:

- the stock synthesized report marks exactly primary button 1 held;
- `abs(Y) >= 16`;
- the ordinary guarded HID sender is active.

Every output report still clears buttons, X and Y, including the stack-packed
button byte. The two reviewed vertical arrays therefore emit exactly two
same-direction wheel increments per swipe. Calibration, small motion, release
and tail entries emit zero.

This filter does not expose raw electrode weights, absolute upper/lower touch
position, or a stationary-contact stream. The application callback receives
only an event type and two 16-bit relative values after the separate touch
controller has already recognized a gesture. True upper/lower half hold-to-
scroll requires further reverse engineering of that controller interface.

Patch source: `patches/r08_touch_wheel_v11/`. Reviewed patch SHA-256:
`92bcd47df85a56a613a76c50ce6256dfe9deab36dd86b1d9d0615b3b23d09ec7`.

## Locked candidate

- Outer version: `RT08_3.10.55_260828`
- Inner version: `1.5.0` (`0x00000051`)
- Capability marker: `A1 FA`
- Pre-final SHA-256:
  `dd495e46fc4a76d71ad683f842070619d90f8db4414e7bf63f0e338bb755b172`
- Official SDK inner digest:
  `be0ced2e6d3d05b9b4080fb84a29f698b11aff4357a2efa138c745bb606c660a`
- Finalized SHA-256:
  `7b60058f5d4de8246834acf139b059009495e0dc9a811b5ff041ec33e3e00e0f`
- Size: `146812` bytes
- CRC16: `0x0349`
- sum16: `0xAE68`
- DFU blocks: `144`

The official SDK tool changed exactly the 32 inner digest bytes; the finalizer
then updated only the outer sum32. Candidate binaries remain ignored and are
not committed to the public repository.

## Offline verification and boundary

`emulate_rt08_touch_wheel_v11_patch.py` executes the exact 42-byte machine
code against both reviewed stock arrays. It observes only `[-1,-1]` and
`[+1,+1]`; all pointer/button fields remain zero. Boundary tests prove that
button-up samples, non-primary buttons and `abs(Y) <= 15` emit no wheel.

The hash-locked Rust DFU accepts only the finalized identity above. Rust tests,
Clippy, release builds and all Python firmware tests pass. On 2026-08-28 the
user authorized this exact SHA-256; all 144 data blocks, DFU CHECK and DFU END
completed successfully. After reboot the independent capability query returned
`A1 FA`, confirming that v11 is active. Physical swipe feel and direction
acceptance remain pending.
