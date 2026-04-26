// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

//! Hardware-gated integration tests (Step 12.4 of ROADMAP).
//!
//! These tests open the *real* EC, refresh telemetry, and assert that
//! values fall inside physically plausible ranges. They MUST NOT run in
//! CI: they require:
//!
//!   * Acer Nitro hardware with a known DMI model.
//!   * Either root (`geteuid() == 0`) or `cap_sys_rawio` + `cap_sys_admin`.
//!   * The `acer-predator-turbo-and-rgb-keyboard-linux-module` kernel
//!     module loaded if RGB-related assertions are enabled.
//!
//! Enable with:
//!   sudo -E cargo test --features hardware-tests --test hardware_integration -- --nocapture

#![cfg(feature = "hardware-tests")]

use nitrosense::ffi::RawEcDevice;
use nitrosense::hardware::ec::Ec;
use nitrosense::hardware::platform::{detect_model, register_map_for_model};

fn require_root_or_skip() -> bool {
    if unsafe { libc::geteuid() } == 0 {
        return true;
    }
    eprintln!("hardware-tests skipped: requires root (got euid != 0)");
    false
}

fn ec_for_current_model() -> Ec<RawEcDevice> {
    let model = detect_model().expect("DMI model must be readable");
    let regs = register_map_for_model(&model)
        .unwrap_or_else(|| panic!("model `{model}` is not in the supported register map"));
    Ec::new(RawEcDevice::new(), regs)
}

#[test]
fn ec_open_and_refresh_returns_sane_temperatures_and_rpms() {
    if !require_root_or_skip() {
        return;
    }
    let mut ec = ec_for_current_model();
    ec.open().expect("ec_open must succeed on real hardware");
    ec.refresh().expect("ec_refresh must succeed");
    let snap = ec.snapshot();
    assert!(
        snap.cpu_temp <= 120,
        "CPU temp out of range: {}°C",
        snap.cpu_temp
    );
    assert!(
        snap.gpu_temp <= 120,
        "GPU temp out of range: {}°C",
        snap.gpu_temp
    );
    assert!(
        snap.sys_temp <= 120,
        "System temp out of range: {}°C",
        snap.sys_temp
    );
    assert!(
        snap.cpu_fan_rpm <= 7000,
        "CPU fan RPM out of range: {}",
        snap.cpu_fan_rpm
    );
    assert!(
        snap.gpu_fan_rpm <= 7000,
        "GPU fan RPM out of range: {}",
        snap.gpu_fan_rpm
    );
}

#[test]
fn ec_write_then_refresh_observes_quiet_mode() {
    if !require_root_or_skip() {
        return;
    }
    let mut ec = ec_for_current_model();
    ec.open().expect("ec_open must succeed");
    ec.refresh().expect("ec_refresh must succeed");
    let regs = ec.regs();
    let before = ec.snapshot().nitro_mode;
    ec.write(regs.nitro_mode, regs.quiet_mode)
        .expect("nitro mode write to quiet must succeed");
    let after = ec.snapshot().nitro_mode;
    assert_eq!(
        after, regs.quiet_mode,
        "EC must readback the value we just wrote"
    );
    ec.write(regs.nitro_mode, before)
        .expect("nitro mode restore must succeed");
}
