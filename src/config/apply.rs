// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use crate::config::model::{NitroConfig, RgbConfig};
use crate::error::NitroError;
use crate::hardware::ec::{Ec, EcDevice};
use crate::hardware::power;
use crate::hardware::rgb::{self, RgbDeviceWriter};

/// Apply saved `NitroConfig` settings to the EC registers.
///
/// This is called once at startup after EC init. Each config field is written
/// to its corresponding EC register. The order matters: nitro mode (profile)
/// is set first, then fan modes, then toggles.
pub fn apply_config_to_ec<D: EcDevice>(
    ec: &mut Ec<D>,
    config: &NitroConfig,
) -> Result<(), NitroError> {
    // Profile first — this also resets fans to auto in the original Python.
    ec.write(ec.regs().nitro_mode, config.nitro_mode)?;

    // Fan modes
    ec.write(ec.regs().cpu_fan_mode_control, config.cpu_mode)?;
    ec.write(ec.regs().gpu_fan_mode_control, config.gpu_mode)?;

    // Toggles
    power::toggle_kb_timer(ec, config.kb_30_timeout == ec.regs().kb_30_auto_on)?;
    power::toggle_usb_charging(ec, config.usb_charging == ec.regs().usb_charging_on)?;
    power::toggle_battery_limit(
        ec,
        config.battery_charge_limit == ec.regs().battery_limit_on,
    )?;

    Ok(())
}

/// Apply saved `RgbConfig` settings to the RGB character devices.
///
/// Returns `Ok(())` even if RGB devices are unavailable (graceful degradation).
/// Returns `Err` only if the RGB command values fail validation.
pub fn apply_rgb_config(
    writer: &mut impl RgbDeviceWriter,
    config: &RgbConfig,
) -> Result<(), NitroError> {
    let command = rgb::RgbCommand {
        mode: config.mode,
        zone: config.zone,
        speed: config.speed,
        brightness: config.brightness,
        direction: config.direction,
        red: config.red,
        green: config.green,
        blue: config.blue,
    };
    rgb::apply_rgb_command(writer, command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::platform::{AN515_44_REGS, AN515_46_REGS};
    use crate::test_support::{RecordingEcDevice, RecordingRgbWriter as SharedRecordingRgbWriter};
    use std::time::Duration;

    /// Mock EC device that records all writes.
    type MockEcDevice = RecordingEcDevice;

    fn ec(regs: &'static crate::hardware::platform::RegisterMap) -> Ec<MockEcDevice> {
        Ec::new(MockEcDevice::default(), regs).with_min_write_interval(Duration::ZERO)
    }

    /// Mock RGB writer that records all writes.
    type RecordingRgbWriter = SharedRecordingRgbWriter;

    #[test]
    fn apply_config_writes_profile_fan_modes_and_toggles_to_ec() {
        let mut ec = ec(&AN515_46_REGS);
        let config = NitroConfig {
            cpu_mode: AN515_46_REGS.cpu_manual_mode,
            gpu_mode: AN515_46_REGS.gpu_turbo_mode,
            kb_30_timeout: AN515_46_REGS.kb_30_auto_on,
            usb_charging: AN515_46_REGS.usb_charging_on,
            nitro_mode: AN515_46_REGS.extreme_mode,
            battery_charge_limit: AN515_46_REGS.battery_limit_on,
        };

        apply_config_to_ec(&mut ec, &config).expect("applying config should succeed");

        let writes = &ec.device_ref().writes;
        // Profile
        assert!(writes.contains(&(AN515_46_REGS.nitro_mode, AN515_46_REGS.extreme_mode)));
        // Fan modes
        assert!(writes.contains(&(
            AN515_46_REGS.cpu_fan_mode_control,
            AN515_46_REGS.cpu_manual_mode
        )));
        assert!(writes.contains(&(
            AN515_46_REGS.gpu_fan_mode_control,
            AN515_46_REGS.gpu_turbo_mode
        )));
        // KB timer on
        assert!(writes.contains(&(AN515_46_REGS.kb_30_sec_auto, AN515_46_REGS.kb_30_auto_on)));
        // USB charging on
        assert!(writes.contains(&(AN515_46_REGS.usb_charging, AN515_46_REGS.usb_charging_on)));
        // Battery limit on
        assert!(writes.contains(&(
            AN515_46_REGS.battery_charge_limit,
            AN515_46_REGS.battery_limit_on
        )));
    }

    #[test]
    fn apply_config_disables_toggles_when_config_has_off_values() {
        let mut ec = ec(&AN515_46_REGS);
        let config = NitroConfig {
            cpu_mode: AN515_46_REGS.cpu_auto_mode,
            gpu_mode: AN515_46_REGS.gpu_auto_mode,
            kb_30_timeout: AN515_46_REGS.kb_30_auto_off,
            usb_charging: AN515_46_REGS.usb_charging_off,
            nitro_mode: AN515_46_REGS.default_mode,
            battery_charge_limit: AN515_46_REGS.battery_limit_off,
        };

        apply_config_to_ec(&mut ec, &config)
            .expect("applying config with off toggles should succeed");

        let writes = &ec.device_ref().writes;
        assert!(writes.contains(&(AN515_46_REGS.kb_30_sec_auto, AN515_46_REGS.kb_30_auto_off)));
        assert!(writes.contains(&(AN515_46_REGS.usb_charging, AN515_46_REGS.usb_charging_off)));
        assert!(writes.contains(&(
            AN515_46_REGS.battery_charge_limit,
            AN515_46_REGS.battery_limit_off
        )));
    }

    #[test]
    fn apply_config_uses_an515_44_battery_limit_values() {
        let mut ec = ec(&AN515_44_REGS);
        let config = NitroConfig {
            battery_charge_limit: AN515_44_REGS.battery_limit_on,
            ..NitroConfig::default()
        };

        apply_config_to_ec(&mut ec, &config).expect("AN515-44 battery limit should apply");

        let writes = &ec.device_ref().writes;
        assert!(writes.contains(&(
            AN515_44_REGS.battery_charge_limit,
            AN515_44_REGS.battery_limit_on
        )));
    }

    #[test]
    fn apply_rgb_config_writes_dynamic_mode_to_device() {
        let mut writer = RecordingRgbWriter::default();
        let config = RgbConfig {
            mode: 3,
            zone: 2,
            speed: 5,
            brightness: 80,
            direction: 1,
            red: 255,
            green: 0,
            blue: 128,
        };

        apply_rgb_config(&mut writer, &config).expect("dynamic RGB config should apply");

        assert!(
            !writer.writes.is_empty(),
            "dynamic RGB should write at least one payload"
        );
        assert_eq!(writer.writes[0].0, rgb::CHARACTER_DEVICE);
    }

    #[test]
    fn apply_rgb_config_writes_static_mode_to_device() {
        let mut writer = RecordingRgbWriter::default();
        let config = RgbConfig {
            mode: 0,
            zone: 1,
            speed: 1,
            brightness: 100,
            direction: 1,
            red: 100,
            green: 200,
            blue: 50,
        };

        apply_rgb_config(&mut writer, &config).expect("static RGB config should apply");

        // Static mode writes to the static device + brightness to main device
        assert!(
            writer.writes.len() >= 2,
            "static mode should write zone + brightness"
        );
        assert_eq!(writer.writes[0].0, rgb::CHARACTER_DEVICE_STATIC);
    }

    #[test]
    fn apply_rgb_config_rejects_invalid_config() {
        let mut writer = RecordingRgbWriter::default();
        let config = RgbConfig {
            mode: 0,
            zone: 5, // invalid — zone must be 0..=4
            speed: 5,
            brightness: 80,
            direction: 1,
            red: 10,
            green: 20,
            blue: 30,
        };

        let result = apply_rgb_config(&mut writer, &config);

        assert!(
            matches!(result, Err(NitroError::Validation(_))),
            "invalid RGB config must be rejected"
        );
        assert!(
            writer.writes.is_empty(),
            "invalid RGB config must not touch devices"
        );
    }
}
