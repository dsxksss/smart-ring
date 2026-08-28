# RT08 custom firmware v10: capacitive swipe to safe native wheel

Date: 2026-08-28. Target is only `R08_9C07 / RT08_V3.1`.

## Why v10 exists

The installed v9 proved that official `3B 02 00 02 01` capacitive wake works:
after sleep, a real double-tap lights the green touch indicator and produces
`73 2A 00`. Repeated up/down swipes produced no `0x1D/2/3` GATT actions and no
scroll input. Offline disassembly then showed that application type 2 consumes
the touch queue as a six-byte HID mouse report: buttons, signed X, signed Y and
signed wheel. v9 returned immediately from all three attribute-4 mouse helpers,
so it also removed the only observed swipe output.

## Exact rewrite

v10 keeps the stock connection guard, attribute index 4 and
`server_send_data` path in the ordinary motion helper at `0x00829F74`.

- At `0x00829F7E`, the original `mov r4,sp; strb r0,[r4,#8]` is replaced by a
  Thumb `BL 0x00829FD6`.
- The 30-byte thunk at `0x00829FD6` maps signed Y to one wheel notch
  (`negative=-1`, `zero=0`, `positive=+1`), clears buttons/X/Y, repeats the two
  overwritten packing instructions and returns to stock code.
- The extended report entry at `0x00829FD4` becomes `BX LR`, preventing a
  second path from emitting pointer movement or buttons.
- The ordinary zero-release helper at `0x00829FAA` remains stock.
- The capability marker is `A1 FB`; the existing IMU hook and three-repeat
  touch indicator remain unchanged.

Patch source: `patches/r08_touch_wheel/`. Reviewed patch SHA-256:
`5275bf3c5afd6a0bdc538ad83bc8ce0ade342110871d04852f8ded129617fb20`.

## Locked candidate

- Outer version: `RT08_3.10.54_260828`
- Inner version: `1.4.9` (`0x00009041`)
- Pre-final SHA-256:
  `7eb94863714b9baa61017838de61074e8eeea4d384ffb0f80b4e935344a1fded`
- Official SDK inner digest:
  `eb9ecc8a8ca60ceba14cbec0e1abb9465770c0e8548376d0e1f16ce5bd977548`
- Finalized SHA-256:
  `6cd256de135ce4290794feebec808cdf4cea2e6fd9dfdd30e675a16fcb7927bb`
- Size: `146812` bytes
- CRC16: `0x50ED`
- sum16: `0xB4D0`
- DFU blocks: `144`

The official `prepend_header.exe` changed exactly the 32 inner digest bytes;
the finalizer then updated only the outer sum32. The public repository does not
contain the pre-final or finalized binary.

## Offline verification

`emulate_rt08_touch_wheel.py` executes the exact finalized machine code with
negative, zero and positive signed Y. It observes wheel `-1/0/+1` respectively,
while buttons/X/Y and the stack-packed button byte are zero in every case. The
extended mouse-report entry is independently checked as `BX LR`.

Rust tests, Clippy, release builds, the hash-locked DFU dry-run and all Python
firmware tests pass. Offline results do not prove direction, sensitivity or
Windows HID behavior on the physical ring.

## Flash and acceptance boundary

The candidate is not authorized or installed. Before DFU, the user must approve
the exact finalized SHA-256 above. After a successful flash, acceptance order is:

1. Confirm `A1 FB`, battery read and continuous `A2 10` without input injection.
2. Run touch-scroll-only with explicit `--inject`.
3. Confirm no pointer movement or mouse-button action under aggressive swipes.
4. Confirm up/down scroll direction and decide whether the unit notch is too
   sensitive, too slow or reversed.
5. Confirm one-minute sleep, double-tap green wake and scrolling after wake.

Any code or binary change creates a new hash and requires a new authorization.
