#![no_std]

// This crate deliberately contains assembly only.  It produces a relocatable
// Cortex-M0+ object for offline inspection; it does not write a device or an
// OTA image.
core::arch::global_asm!(include_str!("patch.S"), options(raw));
