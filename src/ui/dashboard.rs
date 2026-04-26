// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::fmt::Write;

use egui::Ui;

use crate::app::state::{AppState, BatteryStatus, PerformanceProfile};
use crate::ui::fmtbuf::FmtBuf;

fn power_status_label(plugged_in: bool) -> &'static str {
    if plugged_in {
        "Plugged In"
    } else {
        "Unplugged"
    }
}

fn battery_status_label(status: BatteryStatus) -> &'static str {
    match status {
        BatteryStatus::Charging => "Charging",
        BatteryStatus::Discharging => "Discharging",
        BatteryStatus::NotInUse => "Not In Use",
    }
}

fn nitro_mode_label(profile: PerformanceProfile) -> &'static str {
    match profile {
        PerformanceProfile::Quiet => "Quiet",
        PerformanceProfile::Default => "Default",
        PerformanceProfile::Extreme => "Extreme",
    }
}

fn toggle_label(enabled: bool) -> &'static str {
    if enabled { "On" } else { "Off" }
}

/// Render monitoring dashboard with temps, fans, and power status.
///
/// Uses stack-allocated `FmtBuf` for dynamic labels to avoid per-frame
/// `String` heap allocations in the render loop.
pub fn render(ui: &mut Ui, state: &AppState) {
    ui.heading("Monitoring");
    ui.separator();

    // Status Group (Power/Battery)
    ui.group(|ui| {
        ui.label("Power Status:");
        ui.horizontal(|ui| {
            ui.label(power_status_label(state.power_plugged_in));
        });

        ui.horizontal(|ui| {
            ui.label("Battery:");
            ui.label(battery_status_label(state.battery_status));
        });

        ui.horizontal(|ui| {
            ui.label("Charge Limit:");
            ui.label(toggle_label(state.battery_limit_enabled));
        });

        ui.horizontal(|ui| {
            ui.label("USB Charging:");
            ui.label(toggle_label(state.usb_charging_enabled));
        });

        ui.horizontal(|ui| {
            ui.label("KB Timer:");
            ui.label(toggle_label(state.kb_timer_enabled));
        });

        ui.horizontal(|ui| {
            ui.label("Nitro Mode:");
            ui.label(nitro_mode_label(state.performance_profile));
        });
    });

    ui.add_space(16.0);

    // Temperature Group — stack-allocated formatting avoids String allocation
    ui.group(|ui| {
        ui.label("Temperatures:");
        ui.horizontal(|ui| {
            let mut buf = FmtBuf::<32>::new();
            let _ = write!(buf, "CPU: {}°C", state.cpu_temp);
            ui.label(buf.as_str());
        });
        ui.horizontal(|ui| {
            let mut buf = FmtBuf::<32>::new();
            let _ = write!(buf, "GPU: {}°C", state.gpu_temp);
            ui.label(buf.as_str());
        });
        ui.horizontal(|ui| {
            let mut buf = FmtBuf::<32>::new();
            let _ = write!(buf, "System: {}°C", state.sys_temp);
            ui.label(buf.as_str());
        });
    });

    ui.add_space(16.0);

    // Fan Speed Group — stack-allocated formatting avoids String allocation
    ui.group(|ui| {
        ui.label("Fan Speeds:");
        ui.horizontal(|ui| {
            let mut buf = FmtBuf::<32>::new();
            let _ = write!(buf, "CPU: {} RPM", state.cpu_fan_rpm);
            ui.label(buf.as_str());
        });
        ui.horizontal(|ui| {
            let mut buf = FmtBuf::<32>::new();
            let _ = write!(buf, "GPU: {} RPM", state.gpu_fan_rpm);
            ui.label(buf.as_str());
        });
    });

    ui.add_space(16.0);
    crate::ui::voltage::render(ui, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_state() -> AppState {
        AppState {
            cpu_temp: 65,
            gpu_temp: 55,
            sys_temp: 45,
            cpu_fan_rpm: 2500,
            gpu_fan_rpm: 2200,
            power_plugged_in: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_state_creation() {
        let state = create_test_state();
        assert_eq!(state.cpu_temp, 65);
        assert_eq!(state.gpu_temp, 55);
        assert!(state.power_plugged_in);
    }

    #[test]
    fn test_power_status_labels() {
        assert_eq!(power_status_label(true), "Plugged In");
        assert_eq!(power_status_label(false), "Unplugged");
    }

    #[test]
    fn test_battery_status_labels() {
        assert_eq!(battery_status_label(BatteryStatus::Charging), "Charging");
        assert_eq!(
            battery_status_label(BatteryStatus::Discharging),
            "Discharging"
        );
        assert_eq!(battery_status_label(BatteryStatus::NotInUse), "Not In Use");
    }

    #[test]
    fn test_nitro_mode_labels() {
        assert_eq!(nitro_mode_label(PerformanceProfile::Quiet), "Quiet");
        assert_eq!(nitro_mode_label(PerformanceProfile::Default), "Default");
        assert_eq!(nitro_mode_label(PerformanceProfile::Extreme), "Extreme");
    }

    #[test]
    fn test_temperature_format_strings() {
        let state = create_test_state();
        assert_eq!(format!("CPU: {}°C", state.cpu_temp), "CPU: 65°C");
        assert_eq!(format!("GPU: {}°C", state.gpu_temp), "GPU: 55°C");
        assert_eq!(format!("System: {}°C", state.sys_temp), "System: 45°C");
    }

    #[test]
    fn test_fan_rpm_format_strings() {
        let state = create_test_state();
        assert_eq!(format!("CPU: {} RPM", state.cpu_fan_rpm), "CPU: 2500 RPM");
        assert_eq!(format!("GPU: {} RPM", state.gpu_fan_rpm), "GPU: 2200 RPM");
    }

    #[test]
    fn test_toggle_labels_on_off() {
        assert_eq!(toggle_label(true), "On");
        assert_eq!(toggle_label(false), "Off");
    }

    // ---- Render exercises (using egui's built-in test harness) ----

    fn fully_populated_state() -> AppState {
        AppState {
            cpu_temp: 65,
            gpu_temp: 55,
            sys_temp: 45,
            cpu_fan_rpm: 2500,
            gpu_fan_rpm: 2200,
            power_plugged_in: true,
            battery_status: BatteryStatus::Charging,
            performance_profile: PerformanceProfile::Extreme,
            kb_timer_enabled: true,
            usb_charging_enabled: true,
            battery_limit_enabled: true,
            voltage: 1.25,
            min_voltage: 1.0,
            max_voltage: 1.4,
            undervolt_status: "0\t-50\t-0\t00\tStable".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn render_executes_all_subgroups_for_fully_populated_state() {
        let state = fully_populated_state();
        // `__run_test_ui` runs the closure inside a CentralPanel against a
        // fresh egui::Context. This exercises every render branch (power,
        // battery, charge limit, USB, KB timer, nitro mode, temps, fans,
        // voltage subpanel) without needing a real display.
        egui::__run_test_ui(|ui| {
            render(ui, &state);
        });
    }

    #[test]
    fn render_handles_default_state_without_panicking() {
        let state = AppState::default();
        egui::__run_test_ui(|ui| {
            render(ui, &state);
        });
    }

    #[test]
    fn render_covers_each_battery_status_and_profile_branch() {
        for status in [
            BatteryStatus::Charging,
            BatteryStatus::Discharging,
            BatteryStatus::NotInUse,
        ] {
            for profile in [
                PerformanceProfile::Quiet,
                PerformanceProfile::Default,
                PerformanceProfile::Extreme,
            ] {
                let state = AppState {
                    battery_status: status,
                    performance_profile: profile,
                    ..fully_populated_state()
                };
                egui::__run_test_ui(|ui| render(ui, &state));
            }
        }
    }

    #[test]
    fn render_handles_unplugged_and_all_toggles_off_branches() {
        let state = AppState {
            power_plugged_in: false,
            kb_timer_enabled: false,
            usb_charging_enabled: false,
            battery_limit_enabled: false,
            ..fully_populated_state()
        };
        egui::__run_test_ui(|ui| render(ui, &state));
    }
}
