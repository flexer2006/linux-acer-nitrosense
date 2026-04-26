// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use crate::error::NitroError;
use crate::hardware::msr::Msr;
use crate::hardware::platform::CpuVendor;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

/// Tracks voltage readings over time, maintaining current, min, and max values.
#[derive(Debug, Clone)]
pub struct VoltageState {
    pub current: f64,
    pub min: f64,
    pub max: f64,
    pub undervolt_status: String,
}

impl Default for VoltageState {
    fn default() -> Self {
        Self {
            current: 0.0,
            min: f64::MAX,
            max: 0.0,
            undervolt_status: String::new(),
        }
    }
}

impl VoltageState {
    /// Update with a new voltage reading, tracking min/max.
    pub fn update(&mut self, voltage: f64) {
        self.current = voltage;
        if voltage < self.min {
            self.min = voltage;
        }
        if voltage > self.max {
            self.max = voltage;
        }
    }
}

/// Trait for spawning external processes. Implementations can use
/// `tokio::process::Command` in production or a mock in tests.
///
/// The trait is intentionally synchronous so that test mocks can be
/// trivially implemented. Production callers should wrap invocations
/// in `tokio::task::spawn_blocking` to avoid blocking the async runtime.
pub trait ProcessRunner: Send + Sync {
    /// Run a command with arguments and return its stdout as a string.
    fn run(&self, cmd: &str, args: &[&str]) -> Result<String, NitroError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(&self, cmd: &str, args: &[&str]) -> Result<String, NitroError> {
        let executable = resolve_system_command(cmd)?;
        let output = StdCommand::new(executable)
            .args(args)
            .output()
            .map_err(|e| NitroError::Process(format!("failed to run {cmd} {args:?}: {e}")))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            if stderr.is_empty() {
                Err(NitroError::Process(format!(
                    "{cmd} {args:?} exited with {}",
                    output.status
                )))
            } else {
                Err(NitroError::Process(format!(
                    "{cmd} {args:?} exited with {}: {stderr}",
                    output.status
                )))
            }
        }
    }
}

fn resolve_system_command(cmd: &str) -> Result<PathBuf, NitroError> {
    let path = Path::new(cmd);
    if path.is_absolute() || path.components().count() > 1 {
        return path
            .is_file()
            .then_some(path.to_path_buf())
            .ok_or_else(|| NitroError::Process(format!("command '{cmd}' was not found")));
    }

    for dir in [
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
    ] {
        let candidate = Path::new(dir).join(cmd);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(NitroError::Process(format!(
        "command '{cmd}' was not found in standard system paths"
    )))
}

// ---- AMD voltage monitoring ----

/// Parse AMD voltage from `amdctl -g -c0` output.
///
/// Extracts millivolt values from lines containing "mV", converts to volts,
/// and returns the average.
pub fn parse_amd_voltage(output: &str) -> Option<f64> {
    let mut voltages = Vec::new();
    for line in output.lines() {
        if line.contains("mV") {
            for token in line.split_whitespace() {
                if token.contains("mV") {
                    let stripped = token.replace("mV", "");
                    if let Ok(mv) = stripped.parse::<f64>() {
                        voltages.push(mv / 1000.0);
                    }
                }
            }
        }
    }
    if voltages.is_empty() {
        None
    } else {
        Some(voltages.iter().sum::<f64>() / voltages.len() as f64)
    }
}

/// Check AMD voltage by running `amdctl -g -c0`.
///
/// Callers in an async context should wrap this in
/// `tokio::task::spawn_blocking` to avoid blocking the runtime.
pub fn check_amd_voltage(runner: &dyn ProcessRunner) -> Result<f64, NitroError> {
    let output = runner.run("amdctl", &["-g", "-c0"])?;
    parse_amd_voltage(&output)
        .ok_or_else(|| NitroError::Validation("No voltage data from amdctl".to_string()))
}

const INTEL_VOLTAGE_MSR: u32 = 0x198;
const INTEL_ONLINE_CPU_PATH: &str = "/sys/devices/system/cpu/online";

pub trait IntelVoltageReader: Send + Sync {
    fn read_samples(&self) -> Result<Vec<u64>, NitroError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemIntelVoltageReader;

impl IntelVoltageReader for SystemIntelVoltageReader {
    fn read_samples(&self) -> Result<Vec<u64>, NitroError> {
        read_intel_voltage_samples()
    }
}

pub fn check_intel_voltage(reader: &dyn IntelVoltageReader) -> Result<f64, NitroError> {
    let samples = reader.read_samples()?;
    compute_intel_voltage(&samples)
        .ok_or_else(|| NitroError::Validation("No voltage data from MSR".to_string()))
}

fn read_intel_voltage_samples() -> Result<Vec<u64>, NitroError> {
    read_intel_voltage_samples_from(INTEL_ONLINE_CPU_PATH)
}

/// Reads Intel voltage samples using the given online-CPU file path. Exposed
/// for testing so the file-IO error legs and the partial-sampling warning
/// path can be exercised without depending on `/sys`.
pub(crate) fn read_intel_voltage_samples_from(
    online_cpu_path: &str,
) -> Result<Vec<u64>, NitroError> {
    let cpus = read_online_cpu_ids_from(online_cpu_path)?;
    let mut samples = Vec::with_capacity(cpus.len());
    let mut failed_cpus = Vec::new();
    let mut first_error: Option<NitroError> = None;

    for cpu in cpus {
        match Msr::open(cpu) {
            Ok(msr) => match msr.read(INTEL_VOLTAGE_MSR) {
                Ok(value) => samples.push(value),
                Err(err) => {
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                    failed_cpus.push(cpu);
                }
            },
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
                failed_cpus.push(cpu);
            }
        }
    }

    if !failed_cpus.is_empty() {
        tracing::warn!(
            failed_cpus = ?failed_cpus,
            total_cpus = samples.len() + failed_cpus.len(),
            "Incomplete Intel voltage sample set"
        );
    }

    if samples.is_empty() {
        return Err(first_error.unwrap_or_else(|| {
            NitroError::Poller("Intel voltage sampling produced no data".to_string())
        }));
    }

    Ok(samples)
}

/// Read the online-CPU list from the given path and decode the comma-separated
/// range syntax (e.g. `0-3,5,8-11`). Exposed for testing so the file-IO error
/// leg can be reached without remounting `/sys`.
pub(crate) fn read_online_cpu_ids_from(path: &str) -> Result<Vec<i32>, NitroError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        NitroError::Poller(format!("Failed to read online CPU list from {path}: {e}"))
    })?;
    parse_online_cpu_ids(&content)
}

fn parse_online_cpu_ids(content: &str) -> Result<Vec<i32>, NitroError> {
    let mut cpus = Vec::new();

    for part in content.trim().split(',') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }

        if let Some((start, end)) = token.split_once('-') {
            let start = parse_cpu_id(start)?;
            let end = parse_cpu_id(end)?;
            if start > end {
                return Err(NitroError::Poller(format!(
                    "Invalid online CPU range '{token}'"
                )));
            }
            for cpu in start..=end {
                push_unique(&mut cpus, cpu);
            }
        } else {
            push_unique(&mut cpus, parse_cpu_id(token)?);
        }
    }

    if cpus.is_empty() {
        return Err(NitroError::Poller(
            "No online CPUs found for Intel voltage monitoring".to_string(),
        ));
    }

    Ok(cpus)
}

fn parse_cpu_id(value: &str) -> Result<i32, NitroError> {
    let cpu = value.trim().parse::<i32>().map_err(|e| {
        NitroError::Poller(format!("Invalid CPU id '{value}' in online CPU list: {e}"))
    })?;

    if cpu < 0 {
        return Err(NitroError::Poller(format!(
            "CPU id '{value}' in online CPU list must be non-negative"
        )));
    }

    Ok(cpu)
}

fn push_unique(cpus: &mut Vec<i32>, cpu: i32) {
    if !cpus.contains(&cpu) {
        cpus.push(cpu);
    }
}

// ---- Intel voltage monitoring ----

/// Compute Intel voltage from MSR 0x198 bitfield 47:32 values.
///
/// Each value is the raw bitfield reading; voltage = value / 8192.
/// Returns the average voltage across all provided values.
pub fn compute_intel_voltage(msr_values: &[u64]) -> Option<f64> {
    if msr_values.is_empty() {
        return None;
    }
    let extracted: Vec<u64> = msr_values
        .iter()
        .map(|v| (v >> 32) & 0xFFFF) // bits 47:32
        .collect();
    let avg = extracted.iter().sum::<u64>() as f64 / extracted.len() as f64;
    Some(avg / 8192.0)
}

// ---- AMD undervolt ----

/// Parse AMD undervolt status from `amdctl -m -g -c0` output.
///
/// Skips the first 3 lines, then extracts columns 0, 5, 6, 7, 11
/// from each remaining line (matching original Python logic).
pub fn parse_amd_undervolt_status(output: &str) -> String {
    let lines: Vec<&str> = output.lines().skip(3).collect();
    let mut result = String::new();
    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() > 11 {
            let col6 = parts[6].replace(".00", "");
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}",
                parts[0], parts[5], col6, parts[7], parts[11]
            ));
        }
    }
    result
}

/// Check AMD undervolt status by running `amdctl -m -g -c0`.
///
/// Callers in an async context should wrap this in
/// `tokio::task::spawn_blocking` to avoid blocking the runtime.
pub fn check_amd_undervolt_status(runner: &dyn ProcessRunner) -> Result<String, NitroError> {
    let output = runner.run("amdctl", &["-m", "-g", "-c0"])?;
    Ok(parse_amd_undervolt_status(&output))
}

/// Maximum core index for AMD undervolt operations.
const MAX_UNDERVOLT_CORE: u8 = 7;

/// Apply AMD undervolt for the given core index.
///
/// Core must be in `0.=7`. Computes VID = core * 16 (minimum 1),
/// then runs `amdctl -m -v{vid}`, then refreshes undervolt status.
///
/// Callers in an async context should wrap this in
/// `tokio::task::spawn_blocking` to avoid blocking the runtime.
pub fn apply_amd_undervolt(runner: &dyn ProcessRunner, core: u8) -> Result<String, NitroError> {
    if core > MAX_UNDERVOLT_CORE {
        return Err(NitroError::Validation(format!(
            "undervolt core {core} is outside 0..={MAX_UNDERVOLT_CORE}"
        )));
    }
    let vid = if core == 0 { 1 } else { core as u32 * 16 };
    let vid_arg = format!("-v{vid}");
    runner.run("amdctl", &["-m", &vid_arg])?;
    check_amd_undervolt_status(runner)
}

// ---- CPU vendor dispatch ----

/// Get the unsupported CPU type message for voltage operations.
pub fn unsupported_cpu_message() -> &'static str {
    "Voltage not supported for this CPU type."
}

/// Get the unsupported CPU type message for undervolt operations.
pub fn unsupported_undervolt_message() -> &'static str {
    "Undervolt not supported for this CPU type."
}

/// Returns the appropriate voltage check function description for the given CPU vendor.
pub fn voltage_dispatch_description(vendor: CpuVendor) -> &'static str {
    match vendor {
        CpuVendor::Amd => "AMD: amdctl -g -c0",
        CpuVendor::Intel => "Intel: MSR 0x198 bits 47:32",
        CpuVendor::Unknown => "Unknown CPU: voltage monitoring unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- VoltageState tests ----

    #[test]
    fn voltage_state_default_has_max_min_and_zero_current() {
        let state = VoltageState::default();
        assert_eq!(state.current, 0.0);
        assert_eq!(state.min, f64::MAX);
        assert_eq!(state.max, 0.0);
        assert!(state.undervolt_status.is_empty());
    }

    #[test]
    fn voltage_state_update_tracks_min_and_max() {
        let mut state = VoltageState::default();
        state.update(1.2);
        assert_eq!(state.current, 1.2);
        assert_eq!(state.min, 1.2);
        assert_eq!(state.max, 1.2);

        state.update(0.9);
        assert_eq!(state.current, 0.9);
        assert_eq!(state.min, 0.9);
        assert_eq!(state.max, 1.2);

        state.update(1.5);
        assert_eq!(state.current, 1.5);
        assert_eq!(state.min, 0.9);
        assert_eq!(state.max, 1.5);
    }

    // ---- AMD voltage parsing tests ----

    #[test]
    fn parse_amd_voltage_extracts_millivolts_and_averages() {
        let output = "Core 0: 1200mV some noise\nCore 1: 1180mV more noise\n";
        let voltage = parse_amd_voltage(output).expect("should parse voltage");

        let expected = (1200.0 + 1180.0) / 2.0 / 1000.0;
        assert!(
            (voltage - expected).abs() < 0.0001,
            "AMD voltage should average millivolt readings: got {voltage}, expected {expected}"
        );
    }

    #[test]
    fn parse_amd_voltage_returns_none_for_empty_output() {
        assert!(
            parse_amd_voltage("no voltage data here").is_none(),
            "output without mV should return None"
        );
    }

    #[test]
    fn parse_amd_voltage_handles_single_reading() {
        let output = "Core 0: 1150mV\n";
        let voltage = parse_amd_voltage(output).expect("single reading should parse");
        assert!(
            (voltage - 1.15).abs() < 0.0001,
            "single mV reading should convert to volts"
        );
    }

    // ---- Intel voltage computation tests ----

    #[test]
    fn compute_intel_voltage_extracts_bitfield_47_32() {
        // Bit 47:32 = 0x4000 = 16384, voltage = 16384 / 8192 = 2.0
        let msr_value = 0x4000_u64 << 32;
        let voltage = compute_intel_voltage(&[msr_value]).expect("should compute voltage");
        assert!(
            (voltage - 2.0).abs() < 0.0001,
            "Intel voltage should be bitfield 47:32 / 8192"
        );
    }

    #[test]
    fn compute_intel_voltage_averages_multiple_cpus() {
        // CPU 0: 0x2000 << 32 = 8192/8192 = 1.0V
        // CPU 1: 0x2400 << 32 = 9216/8192 = 1.125V
        let v0 = 0x2000_u64 << 32;
        let v1 = 0x2400_u64 << 32;
        let voltage = compute_intel_voltage(&[v0, v1]).expect("should average");
        let expected = (1.0 + 1.125) / 2.0;
        assert!(
            (voltage - expected).abs() < 0.0001,
            "Intel voltage should average across CPUs"
        );
    }

    #[test]
    fn compute_intel_voltage_returns_none_for_empty() {
        assert!(
            compute_intel_voltage(&[]).is_none(),
            "empty MSR values should return None"
        );
    }

    // ---- AMD undervolt status parsing tests ----

    #[test]
    fn parse_amd_undervolt_status_skips_first_three_lines_and_extracts_columns() {
        let output = "\
Header line 1\n\
Header line 2\n\
Header line 3\n\
0  P0  00  00  -00.00  -50  -0.00  00  00  00  00  Stable\n\
1  P1  01  01  -00.01  -60  -0.60  01  01  01  01  Unstable\n";
        let status = parse_amd_undervolt_status(output);

        assert!(
            status.contains("0\t-50\t-0\t00\tStable"),
            "should extract columns 0,5,6,7,11 with .00 stripped from col 6"
        );
        assert!(
            status.contains("1\t-60\t-0.60\t01\tUnstable"),
            "second data row should also be parsed"
        );
        assert!(
            !status.contains("Header"),
            "first 3 lines should be skipped"
        );
    }

    #[test]
    fn parse_amd_undervolt_status_returns_empty_for_insufficient_lines() {
        let output = "Line 1\nLine 2\n";
        let status = parse_amd_undervolt_status(output);
        assert!(
            status.is_empty(),
            "output with fewer than 3+data lines should produce empty status"
        );
    }

    #[test]
    fn parse_amd_undervolt_status_skips_short_lines() {
        let output =
            "A\nB\nC\nshort line\n0  P0  00  00  -00.00  -50  -0.00  00  00  00  00  Stable\n";
        let status = parse_amd_undervolt_status(output);
        assert!(
            !status.contains("short"),
            "lines with fewer than 12 columns should be skipped"
        );
        assert!(
            status.contains("0\t-50\t-0\t00\tStable"),
            "valid data line should still be parsed"
        );
    }

    // ---- CPU vendor dispatch tests ----

    #[test]
    fn voltage_dispatch_description_matches_vendor() {
        assert!(voltage_dispatch_description(CpuVendor::Amd).contains("amdctl"));
        assert!(voltage_dispatch_description(CpuVendor::Intel).contains("MSR"));
        assert!(voltage_dispatch_description(CpuVendor::Unknown).contains("Unknown"));
    }

    #[test]
    fn unsupported_messages_are_non_empty() {
        assert!(!unsupported_cpu_message().is_empty());
        assert!(!unsupported_undervolt_message().is_empty());
    }

    // ---- AMD undervolt bounds tests ----

    #[derive(Debug)]
    struct StubProcessRunner {
        output: String,
    }

    impl ProcessRunner for StubProcessRunner {
        fn run(&self, _cmd: &str, _args: &[&str]) -> Result<String, NitroError> {
            Ok(self.output.clone())
        }
    }

    #[test]
    fn apply_amd_undervolt_rejects_core_above_seven() {
        let runner = StubProcessRunner {
            output: String::new(),
        };
        let result = apply_amd_undervolt(&runner, 8);
        assert!(
            matches!(result, Err(NitroError::Validation(_))),
            "core 8 must be rejected at hardware boundary"
        );
    }

    #[test]
    fn apply_amd_undervolt_accepts_core_seven() {
        let runner = StubProcessRunner {
            output: "header1\nheader2\nheader3\n".to_string(),
        };
        let result = apply_amd_undervolt(&runner, 7);
        assert!(result.is_ok(), "core 7 should be accepted");
    }

    #[test]
    fn apply_amd_undervolt_core_zero_uses_vid_one() {
        // Core 0 → VID=1, so arg should be "-v1"
        #[derive(Debug)]
        struct ArgCapture {
            captured: std::sync::Mutex<Vec<String>>,
        }
        impl ProcessRunner for ArgCapture {
            fn run(&self, _cmd: &str, args: &[&str]) -> Result<String, NitroError> {
                self.captured
                    .lock()
                    .unwrap()
                    .extend(args.iter().map(|s| s.to_string()));
                Ok("h1\nh2\nh3\n".to_string())
            }
        }

        let runner = ArgCapture {
            captured: std::sync::Mutex::new(Vec::new()),
        };
        let _ = apply_amd_undervolt(&runner, 0);
        let args = runner.captured.lock().unwrap();
        assert!(
            args.contains(&"-v1".to_string()),
            "core 0 should compute VID=1, got args: {args:?}"
        );
    }

    #[test]
    fn parse_online_cpu_ids_expands_ranges_and_singletons() {
        let cpus = parse_online_cpu_ids("0-2,4,6-7\n").expect("online CPU list should parse");

        assert_eq!(cpus, vec![0, 1, 2, 4, 6, 7]);
    }

    #[test]
    fn parse_online_cpu_ids_rejects_invalid_ranges() {
        let result = parse_online_cpu_ids("4-2\n");

        assert!(
            matches!(result, Err(NitroError::Poller(_))),
            "descending CPU range should be rejected"
        );
    }

    #[derive(Debug, Default)]
    struct MockIntelVoltageReader {
        samples: Vec<u64>,
    }

    impl IntelVoltageReader for MockIntelVoltageReader {
        fn read_samples(&self) -> Result<Vec<u64>, NitroError> {
            Ok(self.samples.clone())
        }
    }

    #[test]
    fn check_intel_voltage_averages_samples_from_reader() {
        let reader = MockIntelVoltageReader {
            samples: vec![0x2000_u64 << 32, 0x2400_u64 << 32],
        };

        let voltage = check_intel_voltage(&reader).expect("Intel voltage should average");

        assert!(
            (voltage - 1.0625).abs() < 0.0001,
            "Intel voltage should average the MSR bitfield samples"
        );
    }

    #[test]
    fn check_intel_voltage_rejects_empty_sample_sets() {
        let reader = MockIntelVoltageReader { samples: vec![] };

        let result = check_intel_voltage(&reader);

        assert!(
            matches!(result, Err(NitroError::Validation(_))),
            "empty Intel MSR sample sets should be rejected"
        );
    }

    // ---- VoltageState additional coverage ----

    #[test]
    fn voltage_state_clone_preserves_min_max_and_status() {
        let mut state = VoltageState::default();
        state.update(1.05);
        state.update(1.45);
        state.undervolt_status = "active".to_string();
        let cloned = state.clone();

        assert!((cloned.current - 1.45).abs() < f64::EPSILON);
        assert!((cloned.min - 1.05).abs() < f64::EPSILON);
        assert!((cloned.max - 1.45).abs() < f64::EPSILON);
        assert_eq!(cloned.undervolt_status, "active");
    }

    // ---- AMD voltage check (runner integration) ----

    #[test]
    fn check_amd_voltage_returns_average_from_runner_output() {
        let runner = StubProcessRunner {
            output: "Core 0: 1100mV\nCore 1: 1150mV\n".to_string(),
        };

        let voltage = check_amd_voltage(&runner).expect("non-empty voltage data should parse");

        assert!(
            (voltage - 1.125).abs() < 0.001,
            "should average the two readings"
        );
    }

    #[test]
    fn check_amd_voltage_rejects_runner_output_without_voltage_data() {
        let runner = StubProcessRunner {
            output: "no voltage here".to_string(),
        };

        let err =
            check_amd_voltage(&runner).expect_err("missing voltage tokens must surface as error");

        assert!(
            matches!(err, NitroError::Validation(_)),
            "missing data must produce Validation error"
        );
    }

    #[test]
    fn check_amd_voltage_propagates_runner_error() {
        #[derive(Debug)]
        struct ErrRunner;
        impl ProcessRunner for ErrRunner {
            fn run(&self, _: &str, _: &[&str]) -> Result<String, NitroError> {
                Err(NitroError::Process("runner failed".to_string()))
            }
        }
        let err = check_amd_voltage(&ErrRunner).expect_err("runner error must propagate");
        assert!(matches!(err, NitroError::Process(_)));
    }

    #[test]
    fn check_amd_undervolt_status_returns_parsed_status_text() {
        let runner = StubProcessRunner {
            output: "h1\nh2\nh3\n0  P0  00  00  -00.00  -50  -0.00  00  00  00  00  Stable\n"
                .to_string(),
        };

        let status = check_amd_undervolt_status(&runner).expect("4-line output should parse");

        assert!(status.contains("0\t-50\t-0\t00\tStable"));
    }

    #[test]
    fn check_amd_undervolt_status_propagates_runner_error() {
        #[derive(Debug)]
        struct ErrRunner;
        impl ProcessRunner for ErrRunner {
            fn run(&self, _: &str, _: &[&str]) -> Result<String, NitroError> {
                Err(NitroError::Process("amdctl missing".to_string()))
            }
        }
        let err = check_amd_undervolt_status(&ErrRunner).expect_err("runner error must propagate");
        assert!(matches!(err, NitroError::Process(_)));
    }

    // ---- apply_amd_undervolt VID computation across all cores ----

    #[test]
    fn apply_amd_undervolt_uses_vid_core_times_sixteen_for_nonzero_cores() {
        #[derive(Debug, Default)]
        struct ArgCapture {
            captured: std::sync::Mutex<Vec<Vec<String>>>,
        }
        impl ProcessRunner for ArgCapture {
            fn run(&self, _cmd: &str, args: &[&str]) -> Result<String, NitroError> {
                self.captured
                    .lock()
                    .unwrap()
                    .push(args.iter().map(|s| s.to_string()).collect());
                Ok("h1\nh2\nh3\n".to_string())
            }
        }

        for (core, expected_vid) in [(1u8, 16u32), (3, 48), (5, 80), (7, 112)] {
            let runner = ArgCapture::default();
            apply_amd_undervolt(&runner, core).unwrap_or_else(|e| {
                panic!("core {core} should be accepted: {e}");
            });
            let calls = runner.captured.lock().unwrap();
            let arg = format!("-v{expected_vid}");
            assert!(
                calls.iter().any(|args| args.contains(&arg)),
                "core {core} should produce arg `{arg}` (got {calls:?})"
            );
        }
    }

    #[test]
    fn apply_amd_undervolt_propagates_runner_error_before_status_check() {
        #[derive(Debug)]
        struct ErrRunner;
        impl ProcessRunner for ErrRunner {
            fn run(&self, _: &str, _: &[&str]) -> Result<String, NitroError> {
                Err(NitroError::Process("amdctl set failed".to_string()))
            }
        }
        let err = apply_amd_undervolt(&ErrRunner, 4).expect_err("runner error must propagate");
        assert!(matches!(err, NitroError::Process(_)));
    }

    // ---- parse_cpu_id error legs ----

    #[test]
    fn parse_online_cpu_ids_rejects_non_numeric_token() {
        let err =
            parse_online_cpu_ids("0,abc,1\n").expect_err("non-numeric token must produce error");
        assert!(matches!(err, NitroError::Poller(_)));
    }

    #[test]
    fn parse_online_cpu_ids_rejects_negative_cpu_id_via_explicit_range_with_negative_end() {
        // `0--1` parses via `split_once('-')` to (start="0", end="-1"). The end
        // token parses cleanly as -1, which then trips the non-negative check
        // inside `parse_cpu_id`.
        let err = parse_online_cpu_ids("0--1\n").expect_err("negative CPU id must produce error");
        match err {
            NitroError::Poller(msg) => assert!(
                msg.contains("non-negative"),
                "error must explain the non-negative constraint: {msg}"
            ),
            other => panic!("expected Poller error, got {other:?}"),
        }
    }

    #[test]
    fn parse_online_cpu_ids_rejects_negative_range_endpoint() {
        // The single-token "-1" splits into ("", "1") so this exercises the
        // empty-start parse error rather than the non-negative check.
        let err =
            parse_online_cpu_ids("-1-2\n").expect_err("negative range endpoint must produce error");
        assert!(matches!(err, NitroError::Poller(_)));
    }

    #[test]
    fn parse_online_cpu_ids_treats_empty_input_as_no_online_cpus() {
        let err = parse_online_cpu_ids("\n   \n").expect_err("blank-only input must produce error");
        match err {
            NitroError::Poller(msg) => {
                assert!(
                    msg.contains("No online CPUs"),
                    "error must say no CPUs found: {msg}"
                );
            }
            other => panic!("expected Poller error, got {other:?}"),
        }
    }

    #[test]
    fn parse_online_cpu_ids_deduplicates_repeated_cpu_ids() {
        let cpus = parse_online_cpu_ids("0,0,1,1-2,2\n").expect("parse should succeed");
        assert_eq!(cpus, vec![0, 1, 2], "duplicates must be removed in order");
    }

    // ---- read_online_cpu_ids_from + read_intel_voltage_samples_from ----

    #[test]
    fn read_online_cpu_ids_from_reads_cpu_ids_from_file() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("online");
        std::fs::write(&path, "0-3\n").unwrap();

        let cpus = read_online_cpu_ids_from(path.to_str().unwrap())
            .expect("read_online_cpu_ids_from should parse the file");

        assert_eq!(cpus, vec![0, 1, 2, 3]);
    }

    #[test]
    fn read_online_cpu_ids_from_returns_poller_error_when_file_missing() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let missing = dir.path().join("does-not-exist");

        let err = read_online_cpu_ids_from(missing.to_str().unwrap())
            .expect_err("missing file must produce error");
        match err {
            NitroError::Poller(msg) => {
                assert!(
                    msg.contains("Failed to read online CPU list"),
                    "error message: {msg}"
                );
            }
            other => panic!("expected Poller error, got {other:?}"),
        }
    }

    #[test]
    fn read_intel_voltage_samples_from_returns_poller_error_when_no_msr_devices_open() {
        // /dev/cpu/N/msr typically requires root in production, and our test
        // runs as a non-root user. We use a high CPU index so that even on
        // hosts where /dev/cpu/0/msr exists for some reason, the test
        // remains hermetic.
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("online");
        std::fs::write(&path, "9999\n").unwrap();

        let err = read_intel_voltage_samples_from(path.to_str().unwrap())
            .expect_err("non-existent MSR fd opens must surface as error");
        match err {
            NitroError::Poller(_) | NitroError::MsrOpen { .. } => {}
            other => panic!("expected Poller or MsrOpen error from sampling, got {other:?}"),
        }
    }

    #[test]
    fn read_intel_voltage_samples_from_propagates_cpu_list_parse_error() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("online");
        std::fs::write(&path, "garbage\n").unwrap();

        let err = read_intel_voltage_samples_from(path.to_str().unwrap())
            .expect_err("invalid online CPU list must propagate");
        assert!(matches!(err, NitroError::Poller(_)));
    }

    // ---- SystemIntelVoltageReader uses production code path ----

    #[test]
    fn system_intel_voltage_reader_reports_error_when_no_msrs_accessible() {
        // SystemIntelVoltageReader::read_samples reads the real
        // /sys/devices/system/cpu/online file. Since the test process is
        // non-root, opening any /dev/cpu/N/msr will fail with EACCES, so
        // we expect an error here.
        let reader = SystemIntelVoltageReader;
        let result = reader.read_samples();
        match result {
            Ok(samples) => {
                // If for some reason the sandbox grants MSR access we still
                // accept a non-empty sample set as a valid outcome.
                assert!(
                    !samples.is_empty(),
                    "if read_samples succeeds it must return non-empty data"
                );
            }
            Err(NitroError::Poller(_)) | Err(NitroError::MsrOpen { .. }) => {}
            Err(other) => panic!("unexpected error type: {other:?}"),
        }
    }

    // ---- SystemProcessRunner integration with /bin/echo + /bin/false ----

    #[test]
    fn system_process_runner_returns_stdout_for_successful_command() {
        let runner = SystemProcessRunner;

        let out = runner
            .run("echo", &["hello"])
            .expect("/bin/echo hello must succeed");

        assert!(
            out.contains("hello"),
            "echo output should be returned: {out}"
        );
    }

    #[test]
    fn system_process_runner_runs_absolute_path_when_provided() {
        let runner = SystemProcessRunner;

        let out = runner
            .run("/bin/echo", &["abs"])
            .expect("/bin/echo via absolute path must succeed");

        assert!(out.contains("abs"));
    }

    #[test]
    fn system_process_runner_returns_process_error_for_nonzero_exit() {
        let runner = SystemProcessRunner;

        let err = runner
            .run("false", &[])
            .expect_err("/bin/false must produce an error");

        assert!(matches!(err, NitroError::Process(_)));
    }

    #[test]
    fn system_process_runner_returns_process_error_for_nonexistent_command() {
        let runner = SystemProcessRunner;

        let err = runner
            .run("definitely-not-a-real-command", &[])
            .expect_err("missing command must produce error");
        match err {
            NitroError::Process(msg) => {
                assert!(
                    msg.contains("not found"),
                    "missing command error should mention not-found: {msg}"
                );
            }
            other => panic!("expected Process error, got {other:?}"),
        }
    }

    #[test]
    fn system_process_runner_returns_process_error_for_missing_absolute_path() {
        let runner = SystemProcessRunner;

        let err = runner
            .run("/this/path/does/not/exist", &[])
            .expect_err("missing absolute path must produce error");
        assert!(matches!(err, NitroError::Process(_)));
    }

    #[test]
    fn system_process_runner_returns_process_error_for_missing_relative_with_components() {
        let runner = SystemProcessRunner;

        let err = runner
            .run("./doesnt/exist", &[])
            .expect_err("missing relative path with components must produce error");
        assert!(matches!(err, NitroError::Process(_)));
    }

    #[test]
    fn system_process_runner_includes_stderr_in_failure_message_when_available() {
        // /bin/sh -c 'echo oops >&2; exit 1' produces a non-empty stderr that
        // our error path should surface verbatim.
        let runner = SystemProcessRunner;

        let err = runner
            .run("sh", &["-c", "echo nitrostderr >&2; exit 1"])
            .expect_err("nonzero exit must produce error");

        match err {
            NitroError::Process(msg) => {
                assert!(
                    msg.contains("nitrostderr"),
                    "stderr should be appended to error message: {msg}"
                );
            }
            other => panic!("expected Process error, got {other:?}"),
        }
    }

    #[test]
    fn system_process_runner_message_has_no_stderr_section_when_stderr_empty() {
        // /bin/false produces an empty stderr; the error message must still
        // exist but should not reference stderr content.
        let runner = SystemProcessRunner;

        let err = runner.run("false", &[]).expect_err("/bin/false must error");

        match err {
            NitroError::Process(msg) => {
                assert!(
                    msg.contains("exited with"),
                    "empty-stderr error should still mention exit status: {msg}"
                );
            }
            other => panic!("expected Process error, got {other:?}"),
        }
    }

    // ---- resolve_system_command branch coverage ----

    #[test]
    fn resolve_system_command_finds_well_known_binary_in_path() {
        let path =
            resolve_system_command("echo").expect("echo must be findable in PATH directories");
        assert!(
            path.is_absolute(),
            "resolve_system_command must return an absolute path: {path:?}"
        );
        assert!(
            path.is_file(),
            "resolve_system_command result must be an existing file: {path:?}"
        );
    }

    #[test]
    fn resolve_system_command_returns_error_for_unknown_relative_command() {
        let err = resolve_system_command("definitely-not-a-real-command")
            .expect_err("unknown command must produce error");
        match err {
            NitroError::Process(msg) => {
                assert!(msg.contains("not found"), "msg: {msg}");
            }
            other => panic!("expected Process error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_system_command_passes_through_existing_absolute_path() {
        let candidates = ["/bin/echo", "/usr/bin/echo"];
        let pick = candidates
            .into_iter()
            .find(|p| std::path::Path::new(p).is_file())
            .expect("at least one /bin or /usr/bin echo should exist on Linux");

        let path =
            resolve_system_command(pick).expect("existing absolute path must resolve to itself");
        assert_eq!(path, std::path::Path::new(pick));
    }

    #[test]
    fn resolve_system_command_returns_error_for_missing_absolute_path() {
        let err = resolve_system_command("/this/does/not/exist")
            .expect_err("missing absolute path must produce error");
        assert!(matches!(err, NitroError::Process(_)));
    }
}
