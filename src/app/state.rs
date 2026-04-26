// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use crate::app::events::Command;
use crate::config::model::{NitroConfig, RgbConfig};
use crate::error::NitroError;
use crate::hardware::platform::RegisterMap;
use crate::telemetry::poller::TelemetrySnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanMode {
    Auto,
    Manual,
    Turbo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerformanceProfile {
    Quiet,
    Default,
    Extreme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BatteryStatus {
    Charging,
    Discharging,
    #[default]
    NotInUse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub cpu_fan_mode: FanMode,
    pub gpu_fan_mode: FanMode,
    pub performance_profile: PerformanceProfile,
    pub turbo_enabled: bool,
    pub cpu_manual_speed: u8,
    pub gpu_manual_speed: u8,
    pub cpu_temp: u8,
    pub gpu_temp: u8,
    pub sys_temp: u8,
    pub cpu_fan_rpm: u16,
    pub gpu_fan_rpm: u16,
    pub power_plugged_in: bool,
    pub battery_status: BatteryStatus,
    pub voltage: f64,
    pub min_voltage: f64,
    pub max_voltage: f64,
    pub undervolt_status: String,
    pub kb_timer_enabled: bool,
    pub usb_charging_enabled: bool,
    pub battery_limit_enabled: bool,
    pub rgb_config: RgbConfig,
    pub rgb_available: bool,
    pub last_error: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            cpu_fan_mode: FanMode::Auto,
            gpu_fan_mode: FanMode::Auto,
            performance_profile: PerformanceProfile::Default,
            turbo_enabled: false,
            cpu_manual_speed: 0,
            gpu_manual_speed: 0,
            cpu_temp: 0,
            gpu_temp: 0,
            sys_temp: 0,
            cpu_fan_rpm: 0,
            gpu_fan_rpm: 0,
            power_plugged_in: false,
            battery_status: BatteryStatus::NotInUse,
            voltage: 0.0,
            min_voltage: f64::MAX,
            max_voltage: 0.0,
            undervolt_status: String::new(),
            kb_timer_enabled: false,
            usb_charging_enabled: false,
            battery_limit_enabled: false,
            rgb_config: RgbConfig::default(),
            rgb_available: false,
            last_error: None,
        }
    }
}

impl AppState {
    pub fn to_nitro_config(&self, regs: &RegisterMap) -> NitroConfig {
        NitroConfig {
            cpu_mode: match self.cpu_fan_mode {
                FanMode::Auto => regs.cpu_auto_mode,
                FanMode::Manual => regs.cpu_manual_mode,
                FanMode::Turbo => regs.cpu_turbo_mode,
            },
            gpu_mode: match self.gpu_fan_mode {
                FanMode::Auto => regs.gpu_auto_mode,
                FanMode::Manual => regs.gpu_manual_mode,
                FanMode::Turbo => regs.gpu_turbo_mode,
            },
            kb_30_timeout: if self.kb_timer_enabled {
                regs.kb_30_auto_on
            } else {
                regs.kb_30_auto_off
            },
            usb_charging: if self.usb_charging_enabled {
                regs.usb_charging_on
            } else {
                regs.usb_charging_off
            },
            nitro_mode: match self.performance_profile {
                PerformanceProfile::Quiet => regs.quiet_mode,
                PerformanceProfile::Default => regs.default_mode,
                PerformanceProfile::Extreme => regs.extreme_mode,
            },
            battery_charge_limit: if self.battery_limit_enabled {
                regs.battery_limit_on
            } else {
                regs.battery_limit_off
            },
        }
    }

    pub fn apply_command(&mut self, command: Command) -> Result<(), NitroError> {
        match command {
            Command::SetCpuFanMode(mode) => {
                self.cpu_fan_mode = mode;
                Ok(())
            }
            Command::SetGpuFanMode(mode) => {
                self.gpu_fan_mode = mode;
                Ok(())
            }
            Command::SetCpuManualSpeed(level) => {
                validate_manual_speed_level(level)?;
                if self.cpu_fan_mode != FanMode::Manual {
                    return Err(NitroError::Validation(
                        "CPU manual speed requires CPU fan manual mode".to_string(),
                    ));
                }
                self.cpu_manual_speed = level;
                Ok(())
            }
            Command::SetGpuManualSpeed(level) => {
                validate_manual_speed_level(level)?;
                if self.gpu_fan_mode != FanMode::Manual {
                    return Err(NitroError::Validation(
                        "GPU manual speed requires GPU fan manual mode".to_string(),
                    ));
                }
                self.gpu_manual_speed = level;
                Ok(())
            }
            Command::SetProfile(profile) => {
                self.performance_profile = profile;
                self.turbo_enabled = false;
                self.cpu_fan_mode = FanMode::Auto;
                self.gpu_fan_mode = FanMode::Auto;
                Ok(())
            }
            Command::ToggleTurbo(enabled) => {
                self.turbo_enabled = enabled;
                if enabled {
                    self.performance_profile = PerformanceProfile::Extreme;
                    self.cpu_fan_mode = FanMode::Turbo;
                    self.gpu_fan_mode = FanMode::Turbo;
                } else {
                    self.performance_profile = PerformanceProfile::Default;
                    self.cpu_fan_mode = FanMode::Auto;
                    self.gpu_fan_mode = FanMode::Auto;
                }
                Ok(())
            }
            Command::ToggleKbTimer(enabled) => {
                self.kb_timer_enabled = enabled;
                Ok(())
            }
            Command::ToggleUsbCharging(enabled) => {
                self.usb_charging_enabled = enabled;
                Ok(())
            }
            Command::ToggleBatteryLimit(enabled) => {
                self.battery_limit_enabled = enabled;
                Ok(())
            }
            Command::ApplyRgb(config) => {
                self.rgb_config = config;
                Ok(())
            }
            Command::ApplyUndervolt(core) => {
                self.undervolt_status = format!("Pending undervolt apply for core {core}");
                Ok(())
            }
            Command::SaveRgbConfig
            | Command::LoadRgbConfig
            | Command::SaveConfig
            | Command::Shutdown => Ok(()),
        }
    }

    pub fn apply_telemetry(&mut self, snapshot: &TelemetrySnapshot, regs: &RegisterMap) {
        if let Some(mode) = cpu_fan_mode_from_ec(snapshot.cpu_fan_mode, regs) {
            self.cpu_fan_mode = mode;
        }
        if let Some(mode) = gpu_fan_mode_from_ec(snapshot.gpu_fan_mode, regs) {
            self.gpu_fan_mode = mode;
        }
        if let Some(profile) = performance_profile_from_ec(snapshot.nitro_mode, regs) {
            self.performance_profile = profile;
        }

        if self.cpu_fan_mode == FanMode::Turbo && self.gpu_fan_mode == FanMode::Turbo {
            self.turbo_enabled = true;
            self.performance_profile = PerformanceProfile::Extreme;
        } else if self.turbo_enabled
            && self.cpu_fan_mode == FanMode::Auto
            && self.gpu_fan_mode == FanMode::Auto
        {
            self.turbo_enabled = false;
        }

        self.cpu_temp = snapshot.cpu_temp;
        self.gpu_temp = snapshot.gpu_temp;
        self.sys_temp = snapshot.sys_temp;
        self.cpu_fan_rpm = snapshot.cpu_fan_rpm;
        self.gpu_fan_rpm = snapshot.gpu_fan_rpm;
        self.power_plugged_in = snapshot.power_plugged_in;
        self.battery_status = snapshot.battery_status;
        self.battery_limit_enabled = snapshot.battery_charge_limit == regs.battery_limit_on;
        self.kb_timer_enabled = snapshot.kb_30_timeout == regs.kb_30_auto_on;
        self.usb_charging_enabled = snapshot.usb_charging == regs.usb_charging_on;
        self.cpu_manual_speed = snapshot.cpu_manual_speed / 10;
        self.gpu_manual_speed = snapshot.gpu_manual_speed / 10;
    }
}

pub fn manual_speed_level_to_ec_value(level: u8) -> Result<u8, NitroError> {
    validate_manual_speed_level(level)?;
    Ok(level * 10)
}

fn validate_manual_speed_level(level: u8) -> Result<(), NitroError> {
    if level <= 25 {
        Ok(())
    } else {
        Err(NitroError::Validation(format!(
            "manual speed level {level} is outside 0..=25"
        )))
    }
}

fn cpu_fan_mode_from_ec(value: u8, regs: &RegisterMap) -> Option<FanMode> {
    if value == regs.cpu_auto_mode {
        Some(FanMode::Auto)
    } else if value == regs.cpu_turbo_mode || value == 0xA8 {
        Some(FanMode::Turbo)
    } else if value == regs.cpu_manual_mode {
        Some(FanMode::Manual)
    } else {
        None
    }
}

fn gpu_fan_mode_from_ec(value: u8, regs: &RegisterMap) -> Option<FanMode> {
    if value == regs.gpu_auto_mode || value == 0x00 {
        Some(FanMode::Auto)
    } else if value == regs.gpu_turbo_mode {
        Some(FanMode::Turbo)
    } else if value == regs.gpu_manual_mode {
        Some(FanMode::Manual)
    } else {
        None
    }
}

fn performance_profile_from_ec(value: u8, regs: &RegisterMap) -> Option<PerformanceProfile> {
    if value == regs.quiet_mode {
        Some(PerformanceProfile::Quiet)
    } else if value == regs.default_mode {
        Some(PerformanceProfile::Default)
    } else if value == regs.extreme_mode {
        Some(PerformanceProfile::Extreme)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::platform::AN515_46_REGS;

    #[test]
    fn manual_speed_level_to_ec_value_scales_boundaries() {
        assert_eq!(
            manual_speed_level_to_ec_value(0).expect("0 should be valid"),
            0
        );
        assert_eq!(
            manual_speed_level_to_ec_value(1).expect("1 should be valid"),
            10
        );
        assert_eq!(
            manual_speed_level_to_ec_value(25).expect("25 should be valid"),
            250
        );
        assert!(
            matches!(
                manual_speed_level_to_ec_value(26),
                Err(NitroError::Validation(_))
            ),
            "level 26 must be rejected"
        );
    }

    #[test]
    fn manual_speed_command_requires_manual_fan_mode() {
        let mut state = AppState::default();

        let rejected = state.apply_command(Command::SetCpuManualSpeed(10));
        state
            .apply_command(Command::SetCpuFanMode(FanMode::Manual))
            .expect("switching CPU fan to manual should be valid");
        let accepted = state.apply_command(Command::SetCpuManualSpeed(10));

        assert!(
            matches!(rejected, Err(NitroError::Validation(_))),
            "manual speed should be rejected unless fan is already manual"
        );
        assert!(
            accepted.is_ok(),
            "manual speed should be accepted after entering manual mode"
        );
        assert_eq!(state.cpu_manual_speed, 10);
    }

    #[test]
    fn profile_switch_resets_turbo_and_returns_fans_to_auto() {
        let mut state = AppState {
            turbo_enabled: true,
            cpu_fan_mode: FanMode::Turbo,
            gpu_fan_mode: FanMode::Turbo,
            performance_profile: PerformanceProfile::Extreme,
            ..AppState::default()
        };

        state
            .apply_command(Command::SetProfile(PerformanceProfile::Quiet))
            .expect("profile switch should be valid");

        assert!(
            !state.turbo_enabled,
            "profile switch should clear global turbo"
        );
        assert_eq!(state.cpu_fan_mode, FanMode::Auto);
        assert_eq!(state.gpu_fan_mode, FanMode::Auto);
        assert_eq!(state.performance_profile, PerformanceProfile::Quiet);
    }

    #[test]
    fn turbo_toggle_enables_extreme_profile_and_both_fans() {
        let mut state = AppState::default();

        state
            .apply_command(Command::ToggleTurbo(true))
            .expect("enabling turbo should be valid");

        assert!(state.turbo_enabled);
        assert_eq!(state.performance_profile, PerformanceProfile::Extreme);
        assert_eq!(state.cpu_fan_mode, FanMode::Turbo);
        assert_eq!(state.gpu_fan_mode, FanMode::Turbo);
    }

    #[test]
    fn turbo_off_resets_profile_to_default_and_fans_to_auto() {
        let mut state = AppState {
            turbo_enabled: true,
            cpu_fan_mode: FanMode::Turbo,
            gpu_fan_mode: FanMode::Turbo,
            performance_profile: PerformanceProfile::Extreme,
            ..AppState::default()
        };

        state
            .apply_command(Command::ToggleTurbo(false))
            .expect("disabling turbo should be valid");

        assert!(!state.turbo_enabled);
        assert_eq!(
            state.performance_profile,
            PerformanceProfile::Default,
            "turbo off must set profile to Default to match EC nitro_mode write"
        );
        assert_eq!(state.cpu_fan_mode, FanMode::Auto);
        assert_eq!(state.gpu_fan_mode, FanMode::Auto);
    }

    #[test]
    fn telemetry_detects_external_turbo_with_cpu_alternate_readback() {
        let mut state = AppState::default();
        let snapshot = TelemetrySnapshot {
            cpu_fan_mode: 0xA8,
            gpu_fan_mode: AN515_46_REGS.gpu_turbo_mode,
            nitro_mode: AN515_46_REGS.extreme_mode,
            ..TelemetrySnapshot::default()
        };

        state.apply_telemetry(&snapshot, &AN515_46_REGS);

        assert!(
            state.turbo_enabled,
            "CPU 0xA8 + GPU turbo should auto-enable global turbo"
        );
        assert_eq!(state.cpu_fan_mode, FanMode::Turbo);
        assert_eq!(state.gpu_fan_mode, FanMode::Turbo);
        assert_eq!(state.performance_profile, PerformanceProfile::Extreme);
    }

    #[test]
    fn telemetry_detects_external_gpu_auto_alternate_and_clears_turbo() {
        let mut state = AppState {
            turbo_enabled: true,
            cpu_fan_mode: FanMode::Turbo,
            gpu_fan_mode: FanMode::Turbo,
            ..AppState::default()
        };
        let snapshot = TelemetrySnapshot {
            cpu_fan_mode: AN515_46_REGS.cpu_auto_mode,
            gpu_fan_mode: 0x00,
            nitro_mode: AN515_46_REGS.default_mode,
            ..TelemetrySnapshot::default()
        };

        state.apply_telemetry(&snapshot, &AN515_46_REGS);

        assert!(
            !state.turbo_enabled,
            "both fans auto should clear previously enabled global turbo"
        );
        assert_eq!(state.cpu_fan_mode, FanMode::Auto);
        assert_eq!(state.gpu_fan_mode, FanMode::Auto);
    }

    // ---- Additional coverage ----

    #[test]
    fn to_nitro_config_serializes_all_fields_using_provided_register_map() {
        let state = AppState {
            cpu_fan_mode: FanMode::Manual,
            gpu_fan_mode: FanMode::Turbo,
            performance_profile: PerformanceProfile::Extreme,
            kb_timer_enabled: true,
            usb_charging_enabled: false,
            battery_limit_enabled: true,
            ..AppState::default()
        };

        let config = state.to_nitro_config(&AN515_46_REGS);
        assert_eq!(config.cpu_mode, AN515_46_REGS.cpu_manual_mode);
        assert_eq!(config.gpu_mode, AN515_46_REGS.gpu_turbo_mode);
        assert_eq!(config.kb_30_timeout, AN515_46_REGS.kb_30_auto_on);
        assert_eq!(config.usb_charging, AN515_46_REGS.usb_charging_off);
        assert_eq!(config.nitro_mode, AN515_46_REGS.extreme_mode);
        assert_eq!(config.battery_charge_limit, AN515_46_REGS.battery_limit_on);
    }

    #[test]
    fn to_nitro_config_covers_inverse_branches_for_toggles_and_profiles() {
        let state = AppState {
            cpu_fan_mode: FanMode::Auto,
            gpu_fan_mode: FanMode::Manual,
            performance_profile: PerformanceProfile::Quiet,
            kb_timer_enabled: false,
            usb_charging_enabled: true,
            battery_limit_enabled: false,
            ..AppState::default()
        };

        let config = state.to_nitro_config(&AN515_46_REGS);
        assert_eq!(config.cpu_mode, AN515_46_REGS.cpu_auto_mode);
        assert_eq!(config.gpu_mode, AN515_46_REGS.gpu_manual_mode);
        assert_eq!(config.kb_30_timeout, AN515_46_REGS.kb_30_auto_off);
        assert_eq!(config.usb_charging, AN515_46_REGS.usb_charging_on);
        assert_eq!(config.nitro_mode, AN515_46_REGS.quiet_mode);
        assert_eq!(config.battery_charge_limit, AN515_46_REGS.battery_limit_off);
    }

    #[test]
    fn to_nitro_config_default_profile_uses_default_mode_register() {
        let state = AppState {
            performance_profile: PerformanceProfile::Default,
            ..AppState::default()
        };
        let config = state.to_nitro_config(&AN515_46_REGS);
        assert_eq!(config.nitro_mode, AN515_46_REGS.default_mode);
    }

    #[test]
    fn apply_command_set_gpu_manual_speed_requires_manual_fan_mode() {
        let mut state = AppState::default();
        let err = state
            .apply_command(Command::SetGpuManualSpeed(10))
            .expect_err("setting GPU manual speed in Auto mode must be rejected");
        assert!(matches!(err, NitroError::Validation(_)));

        // After switching to Manual, the same command must succeed.
        state
            .apply_command(Command::SetGpuFanMode(FanMode::Manual))
            .unwrap();
        state
            .apply_command(Command::SetGpuManualSpeed(10))
            .expect("GPU manual speed in Manual mode must succeed");
        assert_eq!(state.gpu_manual_speed, 10);
    }

    #[test]
    fn apply_command_set_cpu_manual_speed_rejects_level_above_max() {
        let mut state = AppState {
            cpu_fan_mode: FanMode::Manual,
            ..AppState::default()
        };
        let err = state
            .apply_command(Command::SetCpuManualSpeed(26))
            .expect_err("level 26 must exceed the 0..=25 range");
        assert!(matches!(err, NitroError::Validation(_)));
    }

    #[test]
    fn apply_command_apply_rgb_replaces_rgb_config_in_state() {
        let mut state = AppState::default();
        let cfg = crate::config::model::RgbConfig {
            mode: 3,
            zone: 2,
            speed: 5,
            brightness: 80,
            direction: 2,
            red: 1,
            green: 2,
            blue: 3,
        };
        state.apply_command(Command::ApplyRgb(cfg.clone())).unwrap();
        assert_eq!(state.rgb_config, cfg);
    }

    #[test]
    fn apply_command_apply_undervolt_records_pending_message_in_status() {
        let mut state = AppState::default();
        state.apply_command(Command::ApplyUndervolt(4)).unwrap();
        assert!(
            state
                .undervolt_status
                .contains("Pending undervolt apply for core 4"),
            "ApplyUndervolt must record a pending status: {:?}",
            state.undervolt_status
        );
    }

    #[test]
    fn apply_command_save_load_and_shutdown_are_no_ops_at_state_level() {
        for cmd in [
            Command::SaveConfig,
            Command::SaveRgbConfig,
            Command::LoadRgbConfig,
            Command::Shutdown,
        ] {
            let mut state = AppState::default();
            let original = state.clone();
            state
                .apply_command(cmd.clone())
                .unwrap_or_else(|e| panic!("{cmd:?} must be a no-op at state level: {e}"));
            assert_eq!(state, original, "{cmd:?} must not mutate state");
        }
    }

    #[test]
    fn apply_telemetry_updates_temperature_and_battery_fields() {
        let mut state = AppState::default();
        let snapshot = TelemetrySnapshot {
            cpu_temp: 75,
            gpu_temp: 60,
            sys_temp: 50,
            cpu_fan_rpm: 2500,
            gpu_fan_rpm: 2200,
            power_plugged_in: true,
            battery_status: BatteryStatus::Discharging,
            cpu_fan_mode: AN515_46_REGS.cpu_auto_mode,
            gpu_fan_mode: AN515_46_REGS.gpu_auto_mode,
            nitro_mode: AN515_46_REGS.default_mode,
            battery_charge_limit: AN515_46_REGS.battery_limit_on,
            kb_30_timeout: AN515_46_REGS.kb_30_auto_on,
            usb_charging: AN515_46_REGS.usb_charging_on,
            cpu_manual_speed: 120,
            gpu_manual_speed: 250,
            ..TelemetrySnapshot::default()
        };

        state.apply_telemetry(&snapshot, &AN515_46_REGS);
        assert_eq!(state.cpu_temp, 75);
        assert_eq!(state.gpu_temp, 60);
        assert_eq!(state.sys_temp, 50);
        assert_eq!(state.cpu_fan_rpm, 2500);
        assert_eq!(state.gpu_fan_rpm, 2200);
        assert!(state.power_plugged_in);
        assert_eq!(state.battery_status, BatteryStatus::Discharging);
        assert!(state.battery_limit_enabled);
        assert!(state.kb_timer_enabled);
        assert!(state.usb_charging_enabled);
        assert_eq!(state.cpu_manual_speed, 12);
        assert_eq!(state.gpu_manual_speed, 25);
    }

    #[test]
    fn apply_telemetry_keeps_existing_modes_when_register_value_is_unrecognized() {
        let mut state = AppState {
            cpu_fan_mode: FanMode::Manual,
            gpu_fan_mode: FanMode::Manual,
            performance_profile: PerformanceProfile::Quiet,
            ..AppState::default()
        };
        let snapshot = TelemetrySnapshot {
            cpu_fan_mode: 0x77,
            gpu_fan_mode: 0x88,
            nitro_mode: 0x99,
            ..TelemetrySnapshot::default()
        };
        state.apply_telemetry(&snapshot, &AN515_46_REGS);
        assert_eq!(state.cpu_fan_mode, FanMode::Manual);
        assert_eq!(state.gpu_fan_mode, FanMode::Manual);
        assert_eq!(state.performance_profile, PerformanceProfile::Quiet);
    }

    #[test]
    fn manual_speed_level_to_ec_value_zero_handled_at_boundary() {
        assert_eq!(manual_speed_level_to_ec_value(0).unwrap(), 0);
        assert_eq!(manual_speed_level_to_ec_value(25).unwrap(), 250);
    }

    #[test]
    fn battery_status_default_is_not_in_use() {
        let bs: BatteryStatus = Default::default();
        assert_eq!(bs, BatteryStatus::NotInUse);
    }

    #[test]
    fn app_state_default_voltages_are_finite_or_max() {
        let state = AppState::default();
        assert_eq!(state.voltage, 0.0);
        assert_eq!(state.min_voltage, f64::MAX);
        assert_eq!(state.max_voltage, 0.0);
        assert!(state.last_error.is_none());
        assert!(!state.rgb_available);
    }

    #[test]
    fn fan_mode_and_profile_derives_provide_clone_eq_debug() {
        let auto = FanMode::Auto;
        let cloned = auto;
        assert_eq!(cloned, FanMode::Auto);
        let debug = format!("{auto:?}");
        assert!(debug.contains("Auto"));

        let profile = PerformanceProfile::Extreme;
        let cloned_p = profile;
        assert_eq!(cloned_p, PerformanceProfile::Extreme);
        let debug_p = format!("{profile:?}");
        assert!(debug_p.contains("Extreme"));
    }
}
