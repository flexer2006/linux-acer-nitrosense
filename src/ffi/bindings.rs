// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::ffi::c_int;

#[repr(C)]
pub struct EcHandle {
    pub fd: c_int,
    pub uses_ec_sys: bool,
}

unsafe extern "C" {
    pub fn ec_open(out: *mut EcHandle) -> c_int;
    pub fn ec_close(h: *mut EcHandle);
    pub fn ec_refresh(h: *mut EcHandle, buffer: *mut u8, len: usize) -> c_int;
    pub fn ec_write_byte(h: *mut EcHandle, addr: u8, val: u8) -> c_int;

    pub fn asm_inb(port: u16) -> u8;
    pub fn asm_outb(port: u16, val: u8);
    pub fn asm_rdmsr(msr: u32) -> u64;

    pub fn msr_open(cpu: c_int) -> c_int;
    pub fn msr_close(fd: c_int);
    pub fn msr_read(fd: c_int, msr: u32, out: *mut u64) -> c_int;
}

// NOTE: asm_wrmsr and msr_write are intentionally excluded from the Rust FFI
// surface. The C/ASM layer retains them for potential future use (e.g. Intel
// undervolt via MSR 0x150), but they are not exposed to Rust to minimize
// attack surface in a root-privileged application.
