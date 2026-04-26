// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

//! End-to-end CLI tests (Step 12.5 of ROADMAP).
//!
//! These tests spawn the actual `nitrosense` binary and assert the
//! observable behaviour of the CLI surface that does NOT touch hardware:
//!
//!   * `--help` and `--version` exit before privilege checks.
//!   * Invalid flags are rejected with a non-zero exit.
//!   * Invalid `--set-profile` / `--set-fan-mode` argument values are
//!     rejected by clap before any EC code runs.
//!
//! Code paths that *do* touch hardware (root check + `pkexec` relaunch)
//! are not exercised here because they require interactive auth. The
//! capability- and signal-handling logic is covered by unit tests in
//! `src/main.rs`.

use assert_cmd::Command;
use predicates::prelude::*;

fn nitrosense() -> Command {
    Command::cargo_bin("nitrosense").expect("nitrosense binary must build for CLI tests")
}

#[test]
fn help_flag_succeeds_and_lists_known_options() {
    nitrosense()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Fan control and system monitoring",
        ))
        .stdout(predicate::str::contains("--no-gui"))
        .stdout(predicate::str::contains("--status"))
        .stdout(predicate::str::contains("--set-profile"))
        .stdout(predicate::str::contains("--set-fan-mode"));
}

#[test]
fn version_flag_succeeds_and_reports_cargo_pkg_version() {
    let expected = format!("nitrosense {}", env!("CARGO_PKG_VERSION"));
    nitrosense()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(expected));
}

#[test]
fn unknown_flag_rejected_with_nonzero_exit() {
    nitrosense()
        .arg("--definitely-not-a-flag")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument"));
}

#[test]
fn set_profile_rejects_invalid_value() {
    nitrosense()
        .args(["--set-profile", "balanced"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("balanced"));
}

#[test]
fn set_fan_mode_rejects_invalid_target() {
    nitrosense()
        .args(["--set-fan-mode", "both", "auto"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("both"));
}

#[test]
fn set_fan_mode_rejects_invalid_mode() {
    nitrosense()
        .args(["--set-fan-mode", "cpu", "max"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("max"));
}

#[test]
fn set_fan_mode_requires_two_values() {
    nitrosense()
        .args(["--set-fan-mode", "cpu"])
        .assert()
        .failure();
}
