// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use crate::app::events::Command;
use crate::app::state::{AppState, FanMode, PerformanceProfile};
use crate::error::NitroError;
use crate::hardware::ec::{Ec, EcDevice};
use crate::hardware::power;

/// Execute a UI command against the EC hardware with transactional rollback.
///
/// 1. Snapshot the current app state.
/// 2. Apply the command to the in-memory state.
/// 3. Execute the corresponding EC write(s).
/// 4. If any EC write fails, roll back the state to the pre-command snapshot.
pub fn execute_command<D: EcDevice>(
    ec: &mut Ec<D>,
    state: &mut AppState,
    cmd: Command,
) -> Result<(), NitroError> {
    let snapshot = state.clone();
    state.apply_command(cmd.clone())?;

    let result = execute_ec_writes(ec, &cmd);

    if result.is_err() {
        *state = snapshot;
    }
    result
}

fn fan_mode_to_ec_value(mode: FanMode, regs: &crate::hardware::platform::RegisterMap) -> u8 {
    match mode {
        FanMode::Auto => regs.cpu_auto_mode,
        FanMode::Turbo => regs.cpu_turbo_mode,
        FanMode::Manual => regs.cpu_manual_mode,
    }
}

fn profile_to_ec_value(
    profile: PerformanceProfile,
    regs: &crate::hardware::platform::RegisterMap,
) -> u8 {
    match profile {
        PerformanceProfile::Quiet => regs.quiet_mode,
        PerformanceProfile::Default => regs.default_mode,
        PerformanceProfile::Extreme => regs.extreme_mode,
    }
}

fn execute_ec_writes<D: EcDevice>(ec: &mut Ec<D>, cmd: &Command) -> Result<(), NitroError> {
    let regs = ec.regs();
    match cmd {
        Command::SetCpuFanMode(mode) => {
            ec.write(regs.cpu_fan_mode_control, fan_mode_to_ec_value(*mode, regs))
        }
        Command::SetGpuFanMode(mode) => {
            let value = match mode {
                FanMode::Auto => regs.gpu_auto_mode,
                FanMode::Turbo => regs.gpu_turbo_mode,
                FanMode::Manual => regs.gpu_manual_mode,
            };
            ec.write(regs.gpu_fan_mode_control, value)
        }
        Command::SetCpuManualSpeed(level) => {
            let ec_value = crate::app::state::manual_speed_level_to_ec_value(*level)?;
            ec.write(regs.cpu_manual_speed_control, ec_value)
        }
        Command::SetGpuManualSpeed(level) => {
            let ec_value = crate::app::state::manual_speed_level_to_ec_value(*level)?;
            ec.write(regs.gpu_manual_speed_control, ec_value)
        }
        Command::SetProfile(profile) => {
            ec.write(regs.nitro_mode, profile_to_ec_value(*profile, regs))
        }
        Command::ToggleTurbo(enabled) => {
            if *enabled {
                ec.write(regs.nitro_mode, regs.extreme_mode)?;
                ec.write(regs.cpu_fan_mode_control, regs.cpu_turbo_mode)?;
                ec.write(regs.gpu_fan_mode_control, regs.gpu_turbo_mode)
            } else {
                ec.write(regs.nitro_mode, regs.default_mode)?;
                ec.write(regs.cpu_fan_mode_control, regs.cpu_auto_mode)?;
                ec.write(regs.gpu_fan_mode_control, regs.gpu_auto_mode)
            }
        }
        Command::ToggleKbTimer(enabled) => power::toggle_kb_timer(ec, *enabled),
        Command::ToggleUsbCharging(enabled) => power::toggle_usb_charging(ec, *enabled),
        Command::ToggleBatteryLimit(enabled) => power::toggle_battery_limit(ec, *enabled),
        Command::ApplyRgb(_)
        | Command::ApplyUndervolt(_)
        | Command::SaveRgbConfig
        | Command::LoadRgbConfig
        | Command::SaveConfig
        | Command::Shutdown => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::events::Command;
    use crate::app::state::{AppState, FanMode, PerformanceProfile};
    use crate::error::NitroError;
    use crate::hardware::ec::{Ec, EcDevice};
    use crate::hardware::platform::{AN515_44_REGS, AN515_46_REGS};
    use std::time::Duration;

    // ---- Mock EC device ----

    #[derive(Debug)]
    struct MockEcDevice {
        buffer: [u8; 256],
        writes: Vec<(u8, u8)>,
        fail_on_write: bool,
        fail_after_n_writes: Option<usize>,
    }

    impl Default for MockEcDevice {
        fn default() -> Self {
            Self {
                buffer: [0; 256],
                writes: Vec::new(),
                fail_on_write: false,
                fail_after_n_writes: None,
            }
        }
    }

    impl MockEcDevice {
        fn refresh_into(&self, buffer: &mut [u8]) -> usize {
            buffer.copy_from_slice(&self.buffer);
            self.buffer.len()
        }
    }

    impl EcDevice for MockEcDevice {
        fn open(&mut self) -> Result<(), NitroError> {
            Ok(())
        }

        fn close(&mut self) {}

        fn refresh(&mut self, buffer: &mut [u8]) -> Result<usize, NitroError> {
            Ok(self.refresh_into(buffer))
        }

        fn write_byte(&mut self, addr: u8, val: u8) -> Result<(), NitroError> {
            if self.fail_on_write {
                return Err(NitroError::EcWrite {
                    addr,
                    source: std::io::Error::from_raw_os_error(5),
                });
            }
            if let Some(n) = self.fail_after_n_writes
                && self.writes.len() >= n
            {
                return Err(NitroError::EcWrite {
                    addr,
                    source: std::io::Error::from_raw_os_error(5),
                });
            }
            self.buffer[addr as usize] = val;
            self.writes.push((addr, val));
            Ok(())
        }
    }

    fn ec_with_regs(regs: &'static crate::hardware::platform::RegisterMap) -> Ec<MockEcDevice> {
        Ec::new(MockEcDevice::default(), regs).with_min_write_interval(Duration::ZERO)
    }

    fn ec_46() -> Ec<MockEcDevice> {
        ec_with_regs(&AN515_46_REGS)
    }

    // ---- Toggle tests ----

    #[test]
    fn execute_command_toggle_kb_timer_writes_ec_and_updates_state() {
        let mut ec = ec_46();
        let mut state = AppState::default();

        execute_command(&mut ec, &mut state, Command::ToggleKbTimer(true))
            .expect("KB timer enable should succeed");

        assert!(
            state.kb_timer_enabled,
            "state should reflect KB timer enabled"
        );
        assert!(
            ec.device_ref()
                .writes
                .contains(&(AN515_46_REGS.kb_30_sec_auto, AN515_46_REGS.kb_30_auto_on,)),
            "EC should write KB timer on value"
        );
    }

    #[test]
    fn execute_command_toggle_usb_charging_writes_ec_and_updates_state() {
        let mut ec = ec_46();
        let mut state = AppState::default();

        execute_command(&mut ec, &mut state, Command::ToggleUsbCharging(true))
            .expect("USB charging enable should succeed");

        assert!(
            state.usb_charging_enabled,
            "state should reflect USB charging enabled"
        );
        assert!(
            ec.device_ref()
                .writes
                .contains(&(AN515_46_REGS.usb_charging, AN515_46_REGS.usb_charging_on,)),
            "EC should write USB charging on value"
        );
    }

    #[test]
    fn execute_command_toggle_battery_limit_writes_ec_and_updates_state() {
        let mut ec = ec_46();
        let mut state = AppState::default();

        execute_command(&mut ec, &mut state, Command::ToggleBatteryLimit(true))
            .expect("battery limit enable should succeed");

        assert!(
            state.battery_limit_enabled,
            "state should reflect battery limit enabled"
        );
        assert!(
            ec.device_ref().writes.contains(&(
                AN515_46_REGS.battery_charge_limit,
                AN515_46_REGS.battery_limit_on,
            )),
            "EC should write battery limit on value for AN515-46"
        );
    }

    #[test]
    fn execute_command_toggle_battery_limit_uses_an515_44_values() {
        let mut ec = ec_with_regs(&AN515_44_REGS);
        let mut state = AppState::default();

        execute_command(&mut ec, &mut state, Command::ToggleBatteryLimit(true))
            .expect("battery limit enable on AN515-44 should succeed");

        assert!(
            ec.device_ref().writes.contains(&(
                AN515_44_REGS.battery_charge_limit,
                AN515_44_REGS.battery_limit_on,
            )),
            "EC should write AN515-44 battery limit on value 0x40"
        );
    }

    // ---- Fan mode tests ----

    #[test]
    fn execute_command_set_cpu_fan_mode_writes_ec_and_updates_state() {
        for (mode, ec_val) in [
            (FanMode::Auto, AN515_46_REGS.cpu_auto_mode),
            (FanMode::Turbo, AN515_46_REGS.cpu_turbo_mode),
            (FanMode::Manual, AN515_46_REGS.cpu_manual_mode),
        ] {
            let mut ec = ec_46();
            let mut state = AppState::default();

            execute_command(&mut ec, &mut state, Command::SetCpuFanMode(mode))
                .unwrap_or_else(|e| panic!("CPU fan mode {mode:?} should succeed: {e}"));

            assert_eq!(
                state.cpu_fan_mode, mode,
                "state should reflect CPU fan mode"
            );
            assert!(
                ec.device_ref()
                    .writes
                    .contains(&(AN515_46_REGS.cpu_fan_mode_control, ec_val,)),
                "EC should write correct CPU fan mode value for {mode:?}"
            );
        }
    }

    #[test]
    fn execute_command_set_gpu_fan_mode_writes_ec_and_updates_state() {
        for (mode, ec_val) in [
            (FanMode::Auto, AN515_46_REGS.gpu_auto_mode),
            (FanMode::Turbo, AN515_46_REGS.gpu_turbo_mode),
            (FanMode::Manual, AN515_46_REGS.gpu_manual_mode),
        ] {
            let mut ec = ec_46();
            let mut state = AppState::default();

            execute_command(&mut ec, &mut state, Command::SetGpuFanMode(mode))
                .unwrap_or_else(|e| panic!("GPU fan mode {mode:?} should succeed: {e}"));

            assert_eq!(
                state.gpu_fan_mode, mode,
                "state should reflect GPU fan mode"
            );
            assert!(
                ec.device_ref()
                    .writes
                    .contains(&(AN515_46_REGS.gpu_fan_mode_control, ec_val,)),
                "EC should write correct GPU fan mode value for {mode:?}"
            );
        }
    }

    // ---- Profile tests ----

    #[test]
    fn execute_command_set_profile_writes_ec_and_updates_state() {
        for (profile, ec_val) in [
            (PerformanceProfile::Quiet, AN515_46_REGS.quiet_mode),
            (PerformanceProfile::Default, AN515_46_REGS.default_mode),
            (PerformanceProfile::Extreme, AN515_46_REGS.extreme_mode),
        ] {
            let mut ec = ec_46();
            let mut state = AppState::default();

            execute_command(&mut ec, &mut state, Command::SetProfile(profile))
                .unwrap_or_else(|e| panic!("profile {profile:?} should succeed: {e}"));

            assert_eq!(
                state.performance_profile, profile,
                "state should reflect profile"
            );
            assert!(
                ec.device_ref()
                    .writes
                    .contains(&(AN515_46_REGS.nitro_mode, ec_val)),
                "EC should write correct nitro mode value for {profile:?}"
            );
        }
    }

    // ---- Turbo tests ----

    #[test]
    fn execute_command_toggle_turbo_on_writes_three_ec_registers() {
        let mut ec = ec_46();
        let mut state = AppState::default();

        execute_command(&mut ec, &mut state, Command::ToggleTurbo(true))
            .expect("turbo on should succeed");

        assert!(state.turbo_enabled);
        assert_eq!(state.cpu_fan_mode, FanMode::Turbo);
        assert_eq!(state.gpu_fan_mode, FanMode::Turbo);
        assert_eq!(state.performance_profile, PerformanceProfile::Extreme);

        let writes = &ec.device_ref().writes;
        assert!(
            writes.contains(&(AN515_46_REGS.nitro_mode, AN515_46_REGS.extreme_mode)),
            "turbo on should write extreme mode to nitro_mode"
        );
        assert!(
            writes.contains(&(
                AN515_46_REGS.cpu_fan_mode_control,
                AN515_46_REGS.cpu_turbo_mode
            )),
            "turbo on should write CPU turbo mode"
        );
        assert!(
            writes.contains(&(
                AN515_46_REGS.gpu_fan_mode_control,
                AN515_46_REGS.gpu_turbo_mode
            )),
            "turbo on should write GPU turbo mode"
        );
    }

    #[test]
    fn execute_command_toggle_turbo_off_writes_three_ec_registers() {
        let mut ec = ec_46();
        let mut state = AppState {
            turbo_enabled: true,
            cpu_fan_mode: FanMode::Turbo,
            gpu_fan_mode: FanMode::Turbo,
            performance_profile: PerformanceProfile::Extreme,
            ..AppState::default()
        };

        execute_command(&mut ec, &mut state, Command::ToggleTurbo(false))
            .expect("turbo off should succeed");

        assert!(!state.turbo_enabled);
        assert_eq!(
            state.performance_profile,
            PerformanceProfile::Default,
            "turbo off must set profile to Default to match EC nitro_mode write"
        );
        assert_eq!(state.cpu_fan_mode, FanMode::Auto);
        assert_eq!(state.gpu_fan_mode, FanMode::Auto);

        let writes = &ec.device_ref().writes;
        assert!(
            writes.contains(&(AN515_46_REGS.nitro_mode, AN515_46_REGS.default_mode)),
            "turbo off should write default mode to nitro_mode"
        );
        assert!(
            writes.contains(&(
                AN515_46_REGS.cpu_fan_mode_control,
                AN515_46_REGS.cpu_auto_mode
            )),
            "turbo off should write CPU auto mode"
        );
        assert!(
            writes.contains(&(
                AN515_46_REGS.gpu_fan_mode_control,
                AN515_46_REGS.gpu_auto_mode
            )),
            "turbo off should write GPU auto mode"
        );
    }

    // ---- Manual speed tests ----

    #[test]
    fn execute_command_set_cpu_manual_speed_writes_ec_and_updates_state() {
        let mut ec = ec_46();
        let mut state = AppState {
            cpu_fan_mode: FanMode::Manual,
            ..AppState::default()
        };

        execute_command(&mut ec, &mut state, Command::SetCpuManualSpeed(15))
            .expect("CPU manual speed should succeed");

        assert_eq!(state.cpu_manual_speed, 15);
        assert!(
            ec.device_ref()
                .writes
                .contains(&(AN515_46_REGS.cpu_manual_speed_control, 150,)),
            "EC should write level * 10 = 150"
        );
    }

    #[test]
    fn execute_command_set_gpu_manual_speed_writes_ec_and_updates_state() {
        let mut ec = ec_46();
        let mut state = AppState {
            gpu_fan_mode: FanMode::Manual,
            ..AppState::default()
        };

        execute_command(&mut ec, &mut state, Command::SetGpuManualSpeed(20))
            .expect("GPU manual speed should succeed");

        assert_eq!(state.gpu_manual_speed, 20);
        assert!(
            ec.device_ref()
                .writes
                .contains(&(AN515_46_REGS.gpu_manual_speed_control, 200,)),
            "EC should write level * 10 = 200"
        );
    }

    // ---- Transactional rollback tests ----

    #[test]
    fn execute_command_ec_write_failure_rolls_back_state() {
        let mock = MockEcDevice {
            fail_on_write: true,
            ..MockEcDevice::default()
        };
        let mut ec = Ec::new(mock, &AN515_46_REGS).with_min_write_interval(Duration::ZERO);
        let mut state = AppState::default();
        let original_state = state.clone();

        let result = execute_command(&mut ec, &mut state, Command::ToggleKbTimer(true));

        assert!(
            matches!(result, Err(NitroError::EcWrite { .. })),
            "EC write failure should propagate as error"
        );
        assert_eq!(
            state, original_state,
            "state must roll back to pre-command snapshot on EC write failure"
        );
    }

    #[test]
    fn execute_command_turbo_on_partial_failure_rolls_back_state() {
        // Fail after 1 write — nitro_mode succeeds, first fan mode fails
        let mock = MockEcDevice {
            fail_after_n_writes: Some(1),
            ..MockEcDevice::default()
        };
        let mut ec = Ec::new(mock, &AN515_46_REGS).with_min_write_interval(Duration::ZERO);
        let mut state = AppState::default();
        let original_state = state.clone();

        let result = execute_command(&mut ec, &mut state, Command::ToggleTurbo(true));

        assert!(result.is_err(), "partial turbo failure should return error");
        assert_eq!(
            state, original_state,
            "state must roll back on partial turbo EC write failure"
        );
    }

    // ---- Idempotency test ----

    #[test]
    fn execute_command_idempotent_toggle_does_not_corrupt_state() {
        let mut ec = ec_46();
        let mut state = AppState::default();

        execute_command(&mut ec, &mut state, Command::ToggleKbTimer(true))
            .expect("first KB timer enable should succeed");
        let state_after_first = state.clone();

        execute_command(&mut ec, &mut state, Command::ToggleKbTimer(true))
            .expect("second KB timer enable should succeed");

        assert_eq!(
            state.kb_timer_enabled, state_after_first.kb_timer_enabled,
            "idempotent toggle should not corrupt state"
        );
    }

    // ---- No-op commands ----

    #[test]
    fn execute_command_noop_commands_return_ok_without_ec_writes() {
        let mut ec = ec_46();
        let mut state = AppState::default();

        for cmd in [
            Command::SaveConfig,
            Command::SaveRgbConfig,
            Command::LoadRgbConfig,
            Command::Shutdown,
        ] {
            execute_command(&mut ec, &mut state, cmd.clone())
                .unwrap_or_else(|e| panic!("{cmd:?} should be a no-op: {e}"));
        }

        assert!(
            ec.device_ref().writes.is_empty(),
            "no-op commands must not produce EC writes"
        );
    }
}
