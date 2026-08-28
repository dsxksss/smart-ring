# R08 v11 contact-gated touch wheel

This Thumb thunk is linked at `0x00829FD6`. It retains the v10 pointer and
button suppression, but emits wheel input only when the stock gesture queue
marks the primary button held and `abs(Y) >= 16`.

The two reviewed stock vertical arrays begin with an opposite-direction
calibration sample while the button byte is zero. Their intended-direction
motion is marked with button byte one, with four `abs(Y)=8` samples and two
`abs(Y)=16` samples. The v11 filter therefore removes the reversal and reduces
one swipe from roughly six wheel increments to two.

This is a filter over the stock synthesized HID gesture queue. It does not
claim access to raw electrode weights, absolute upper/lower touch position, or
a stationary-contact stream from the separate touch controller.

Build with PowerShell:

```powershell
.\build.ps1
```

Build outputs are ignored and must not be committed.
