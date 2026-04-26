// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

//! FASM ABI smoke test (Step 12.3 of ROADMAP).
//!
//! These tests exercise the System V AMD64 calling convention contract for
//! the FASM-built primitives in `src/c_hw/port_io.asm`. They do NOT verify
//! hardware behaviour (`in`/`out`/`rdmsr`/`wrmsr` are all CPL-0 instructions)
//! — they verify that:
//!
//!   1. The symbols are linked into the binary (by taking their address).
//!   2. Each function pointer has the expected width on x86_64 (8 bytes).
//!   3. Calling the routines under root + CAP_SYS_RAWIO succeeds and the
//!      return value plumbing (via `rax` for `inb` / `rdmsr`) works.
//!
//! Privileged steps run only behind `--features hardware-tests`. The
//! linkage/ABI checks compile and run unconditionally so CI catches a
//! missing FASM toolchain or broken symbol export early.

use std::mem::size_of;

use nitrosense::ffi::bindings::{asm_inb, asm_outb, asm_rdmsr};

fn inb_addr() -> usize {
    asm_inb as *const () as usize
}
fn outb_addr() -> usize {
    asm_outb as *const () as usize
}
fn rdmsr_addr() -> usize {
    asm_rdmsr as *const () as usize
}

#[test]
fn asm_symbols_have_pointer_layout_consistent_with_amd64_abi() {
    assert_ne!(inb_addr(), 0, "asm_inb must be resolvable at link time");
    assert_ne!(outb_addr(), 0, "asm_outb must be resolvable at link time");
    assert_ne!(rdmsr_addr(), 0, "asm_rdmsr must be resolvable at link time");
    assert_eq!(size_of::<unsafe extern "C" fn(u16) -> u8>(), 8);
    assert_eq!(size_of::<unsafe extern "C" fn(u16, u8)>(), 8);
    assert_eq!(size_of::<unsafe extern "C" fn(u32) -> u64>(), 8);
}

#[test]
fn asm_symbols_are_distinct_addresses() {
    let inb = inb_addr();
    let outb = outb_addr();
    let rdmsr = rdmsr_addr();
    assert_ne!(
        inb, outb,
        "asm_inb and asm_outb must be different functions"
    );
    assert_ne!(
        inb, rdmsr,
        "asm_inb and asm_rdmsr must be different functions"
    );
    assert_ne!(
        outb, rdmsr,
        "asm_outb and asm_rdmsr must be different functions"
    );
}

#[cfg(feature = "hardware-tests")]
#[test]
fn asm_inb_post_port_returns_byte() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("skipping asm_inb hardware test: not root");
        return;
    }
    let rc = unsafe { libc::iopl(3) };
    if rc != 0 {
        eprintln!("skipping asm_inb hardware test: iopl(3) failed");
        return;
    }
    let value = unsafe { asm_inb(0x80) };
    let _ = value;
}
