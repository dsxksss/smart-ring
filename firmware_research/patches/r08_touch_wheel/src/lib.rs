#![no_std]

use core::arch::global_asm;

global_asm!(include_str!("patch.S"));
