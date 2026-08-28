# R08 touch-wheel rewrite

This reviewed Thumb thunk is linked at `0x00829FD6` for the v10 candidate.
The stock touch queue consumer passes mouse buttons, signed X, signed Y and
wheel values in `r0..r3`. The thunk converts only the sign of Y to one wheel
notch, forces buttons/X/Y to zero, then resumes the stock HID attribute 4
packing and connection-guard path.

The v10 builder also changes the extended mouse-report entry at `0x00829FD4`
to `BX LR`; that path cannot emit pointer movement or buttons. The ordinary
zero-release helper remains stock.

Build with PowerShell:

```powershell
.\build.ps1
```

The reviewed binary is exactly 30 bytes with SHA-256
`5275bf3c5afd6a0bdc538ad83bc8ce0ade342110871d04852f8ded129617fb20`.
Build outputs are ignored and must not be committed.
