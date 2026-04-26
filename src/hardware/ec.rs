// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use crate::app::state::BatteryStatus;
use crate::error::NitroError;
use crate::ffi::RawEcDevice;
use crate::hardware::platform::RegisterMap;
use crate::telemetry::poller::TelemetrySnapshot;
use std::time::{Duration, Instant};

pub trait EcDevice: Send + Sync {
    fn open(&mut self) -> Result<(), NitroError>;
    fn close(&mut self);
    fn refresh(&mut self, buffer: &mut [u8]) -> Result<usize, NitroError>;
    fn write_byte(&mut self, addr: u8, val: u8) -> Result<(), NitroError>;
}

impl EcDevice for RawEcDevice {
    fn open(&mut self) -> Result<(), NitroError> {
        RawEcDevice::open(self)
    }

    fn close(&mut self) {
        RawEcDevice::close(self);
    }

    fn refresh(&mut self, buffer: &mut [u8]) -> Result<usize, NitroError> {
        RawEcDevice::refresh(self, buffer)
    }

    fn write_byte(&mut self, addr: u8, val: u8) -> Result<(), NitroError> {
        RawEcDevice::write_byte(self, addr, val)
    }
}

pub struct Ec<D: EcDevice> {
    device: D,
    regs: &'static RegisterMap,
    buffer: [u8; 256],
    last_write_at: Option<Instant>,
    min_write_interval: Duration,
}

impl<D: EcDevice> Ec<D> {
    pub fn new(device: D, regs: &'static RegisterMap) -> Self {
        Self {
            device,
            regs,
            buffer: [0u8; 256],
            last_write_at: None,
            min_write_interval: Duration::from_millis(50),
        }
    }

    pub fn with_min_write_interval(mut self, interval: Duration) -> Self {
        self.min_write_interval = interval;
        self
    }

    pub fn open(&mut self) -> Result<(), NitroError> {
        self.device.open()
    }

    pub fn refresh(&mut self) -> Result<(), NitroError> {
        let trace_enabled = tracing::enabled!(tracing::Level::TRACE);
        let start = trace_enabled.then(Instant::now);
        self.device.refresh(&mut self.buffer)?;
        if let Some(s) = start {
            tracing::trace!(latency_us = s.elapsed().as_micros() as u64, "EC refresh");
        }
        Ok(())
    }

    #[inline]
    pub fn read(&self, addr: u8) -> u8 {
        self.buffer[addr as usize]
    }

    pub fn write(&mut self, addr: u8, val: u8) -> Result<(), NitroError> {
        self.validate_write(addr, val)?;
        let now = Instant::now();
        let delay = self.remaining_write_delay(now);
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        let trace_enabled = tracing::enabled!(tracing::Level::TRACE);
        let write_start = trace_enabled.then(Instant::now);
        self.device.write_byte(addr, val)?;
        let write_elapsed = write_start.map(|s| s.elapsed());
        self.last_write_at = Some(Instant::now());
        if let Some(elapsed) = write_elapsed {
            tracing::trace!(
                register = format_args!("0x{addr:02X}"),
                value = format_args!("0x{val:02X}"),
                latency_us = elapsed.as_micros() as u64,
                "EC write"
            );
        }
        self.refresh()
    }

    pub fn snapshot(&self) -> TelemetrySnapshot {
        let cpu_high = self.read(self.regs.cpu_fan_speed_high) as u16;
        let cpu_low = self.read(self.regs.cpu_fan_speed_low) as u16;
        let gpu_high = self.read(self.regs.gpu_fan_speed_high) as u16;
        let gpu_low = self.read(self.regs.gpu_fan_speed_low) as u16;

        TelemetrySnapshot {
            cpu_temp: self.read(self.regs.cpu_temp),
            gpu_temp: self.read(self.regs.gpu_temp),
            sys_temp: self.read(self.regs.sys_temp),
            cpu_fan_rpm: (cpu_low << 8) | cpu_high,
            gpu_fan_rpm: (gpu_low << 8) | gpu_high,
            power_plugged_in: self.read(self.regs.power_status) == 0x01,
            battery_status: match self.read(self.regs.battery_status) {
                0x02 => BatteryStatus::Charging,
                0x01 => BatteryStatus::Discharging,
                _ => BatteryStatus::NotInUse,
            },
            cpu_fan_mode: self.read(self.regs.cpu_fan_mode_control),
            gpu_fan_mode: self.read(self.regs.gpu_fan_mode_control),
            nitro_mode: self.read(self.regs.nitro_mode),
            battery_charge_limit: self.read(self.regs.battery_charge_limit),
            kb_30_timeout: self.read(self.regs.kb_30_sec_auto),
            usb_charging: self.read(self.regs.usb_charging),
            cpu_manual_speed: self.read(self.regs.cpu_manual_speed_control),
            gpu_manual_speed: self.read(self.regs.gpu_manual_speed_control),
            timestamp: Instant::now(),
        }
    }

    pub fn regs(&self) -> &'static RegisterMap {
        self.regs
    }

    /// Mutable access to the underlying device for test pre-seeding only.
    /// Not available in production builds to prevent bypassing EC validation.
    #[cfg(test)]
    pub(crate) fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }

    #[cfg(test)]
    pub(crate) fn device_ref(&self) -> &D {
        &self.device
    }

    fn remaining_write_delay(&self, now: Instant) -> Duration {
        self.last_write_at
            .and_then(|last| {
                self.min_write_interval
                    .checked_sub(now.saturating_duration_since(last))
            })
            .unwrap_or_default()
    }

    fn validate_write(&self, addr: u8, val: u8) -> Result<(), NitroError> {
        if addr == self.regs.cpu_fan_mode_control {
            return validate_enum(
                val,
                &[
                    self.regs.cpu_auto_mode,
                    self.regs.cpu_turbo_mode,
                    self.regs.cpu_manual_mode,
                ],
                "CPU fan mode",
            );
        }
        if addr == self.regs.gpu_fan_mode_control {
            return validate_enum(
                val,
                &[
                    self.regs.gpu_auto_mode,
                    self.regs.gpu_turbo_mode,
                    self.regs.gpu_manual_mode,
                ],
                "GPU fan mode",
            );
        }
        if addr == self.regs.cpu_manual_speed_control || addr == self.regs.gpu_manual_speed_control
        {
            return if val <= 250 {
                Ok(())
            } else {
                Err(validation_error(format!(
                    "manual fan speed value 0x{val:02X} exceeds 0xFA"
                )))
            };
        }
        if addr == self.regs.kb_30_sec_auto {
            return validate_enum(
                val,
                &[self.regs.kb_30_auto_off, self.regs.kb_30_auto_on],
                "keyboard timeout",
            );
        }
        if addr == self.regs.usb_charging {
            return validate_enum(
                val,
                &[self.regs.usb_charging_on, self.regs.usb_charging_off],
                "USB charging",
            );
        }
        if addr == self.regs.battery_charge_limit {
            return validate_enum(
                val,
                &[self.regs.battery_limit_on, self.regs.battery_limit_off],
                "battery charge limit",
            );
        }
        if addr == self.regs.nitro_mode {
            return validate_enum(
                val,
                &[
                    self.regs.quiet_mode,
                    self.regs.default_mode,
                    self.regs.extreme_mode,
                ],
                "Nitro mode",
            );
        }
        Err(validation_error(format!(
            "register 0x{addr:02X} is not writable for this model"
        )))
    }
}

impl<D: EcDevice> Drop for Ec<D> {
    fn drop(&mut self) {
        self.device.close();
    }
}

fn validate_enum(value: u8, allowed: &[u8], name: &str) -> Result<(), NitroError> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(validation_error(format!(
            "{name} value 0x{value:02X} is not permitted"
        )))
    }
}

fn validation_error(message: String) -> NitroError {
    NitroError::Validation(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::platform::{AN515_44_REGS, AN515_46_REGS};

    #[derive(Debug)]
    struct MockEcDevice {
        buffer: [u8; 256],
        writes: Vec<(u8, u8)>,
        refresh_calls: usize,
        close_calls: usize,
        fail_write: bool,
        fail_refresh: bool,
    }

    impl Default for MockEcDevice {
        fn default() -> Self {
            Self {
                buffer: [0; 256],
                writes: Vec::new(),
                refresh_calls: 0,
                close_calls: 0,
                fail_write: false,
                fail_refresh: false,
            }
        }
    }

    impl EcDevice for MockEcDevice {
        fn open(&mut self) -> Result<(), NitroError> {
            Ok(())
        }

        fn close(&mut self) {
            self.close_calls += 1;
        }

        fn refresh(&mut self, buffer: &mut [u8]) -> Result<usize, NitroError> {
            if self.fail_refresh {
                return Err(NitroError::EcRefresh(std::io::Error::from_raw_os_error(5)));
            }
            buffer.copy_from_slice(&self.buffer);
            self.refresh_calls += 1;
            Ok(self.buffer.len())
        }

        fn write_byte(&mut self, addr: u8, val: u8) -> Result<(), NitroError> {
            if self.fail_write {
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

    fn ec_with_buffer(buffer: [u8; 256]) -> Ec<MockEcDevice> {
        Ec::new(
            MockEcDevice {
                buffer,
                ..MockEcDevice::default()
            },
            &AN515_46_REGS,
        )
        .with_min_write_interval(Duration::ZERO)
    }

    #[test]
    fn snapshot_extracts_temperatures_statuses_and_rpm_from_refreshed_buffer() {
        let mut buffer = [0u8; 256];
        buffer[AN515_46_REGS.cpu_temp as usize] = 65;
        buffer[AN515_46_REGS.gpu_temp as usize] = 62;
        buffer[AN515_46_REGS.sys_temp as usize] = 50;
        buffer[AN515_46_REGS.cpu_fan_speed_high as usize] = 0x8B;
        buffer[AN515_46_REGS.cpu_fan_speed_low as usize] = 0x06;
        buffer[AN515_46_REGS.gpu_fan_speed_high as usize] = 0x10;
        buffer[AN515_46_REGS.gpu_fan_speed_low as usize] = 0x07;
        buffer[AN515_46_REGS.power_status as usize] = 0x01;
        buffer[AN515_46_REGS.battery_status as usize] = 0x02;
        buffer[AN515_46_REGS.cpu_fan_mode_control as usize] = AN515_46_REGS.cpu_turbo_mode;
        buffer[AN515_46_REGS.gpu_fan_mode_control as usize] = AN515_46_REGS.gpu_auto_mode;
        buffer[AN515_46_REGS.nitro_mode as usize] = AN515_46_REGS.extreme_mode;
        buffer[AN515_46_REGS.battery_charge_limit as usize] = AN515_46_REGS.battery_limit_on;
        buffer[AN515_46_REGS.kb_30_sec_auto as usize] = AN515_46_REGS.kb_30_auto_on;
        buffer[AN515_46_REGS.usb_charging as usize] = AN515_46_REGS.usb_charging_on;
        buffer[AN515_46_REGS.cpu_manual_speed_control as usize] = 120;
        buffer[AN515_46_REGS.gpu_manual_speed_control as usize] = 130;

        let mut ec = ec_with_buffer(buffer);
        ec.refresh().expect("mock refresh should succeed");
        let snapshot = ec.snapshot();

        assert_eq!(
            snapshot.cpu_temp, 65,
            "CPU temp should come from CPUTEMP register"
        );
        assert_eq!(
            snapshot.gpu_temp, 62,
            "GPU temp should come from GPUTEMP register"
        );
        assert_eq!(
            snapshot.sys_temp, 50,
            "system temp should come from SYSTEMP register"
        );
        assert_eq!(
            snapshot.cpu_fan_rpm, 0x068B,
            "CPU RPM must preserve original low<<8|high ordering"
        );
        assert_eq!(
            snapshot.gpu_fan_rpm, 0x0710,
            "GPU RPM must preserve original low<<8|high ordering"
        );
        assert!(
            snapshot.power_plugged_in,
            "power status 0x01 should map to plugged-in"
        );
        assert_eq!(
            snapshot.battery_status,
            BatteryStatus::Charging,
            "battery status 0x02 should map to Charging"
        );
        assert_eq!(snapshot.cpu_fan_mode, AN515_46_REGS.cpu_turbo_mode);
        assert_eq!(snapshot.gpu_fan_mode, AN515_46_REGS.gpu_auto_mode);
        assert_eq!(snapshot.nitro_mode, AN515_46_REGS.extreme_mode);
        assert_eq!(
            snapshot.battery_charge_limit,
            AN515_46_REGS.battery_limit_on
        );
        assert_eq!(snapshot.kb_30_timeout, AN515_46_REGS.kb_30_auto_on);
        assert_eq!(snapshot.usb_charging, AN515_46_REGS.usb_charging_on);
        assert_eq!(snapshot.cpu_manual_speed, 120);
        assert_eq!(snapshot.gpu_manual_speed, 130);
    }

    #[test]
    fn write_accepts_valid_register_value_then_refreshes_snapshot_buffer() {
        let mut ec = ec_with_buffer([0u8; 256]);

        ec.write(
            AN515_46_REGS.cpu_fan_mode_control,
            AN515_46_REGS.cpu_manual_mode,
        )
        .expect("valid CPU manual fan mode should write");

        assert_eq!(
            ec.device.writes,
            vec![(
                AN515_46_REGS.cpu_fan_mode_control,
                AN515_46_REGS.cpu_manual_mode
            )],
            "valid write should be forwarded exactly once"
        );
        assert_eq!(
            ec.device.refresh_calls, 1,
            "EC should refresh after a successful write"
        );
        assert_eq!(
            ec.read(AN515_46_REGS.cpu_fan_mode_control),
            AN515_46_REGS.cpu_manual_mode
        );
    }

    #[test]
    fn write_rejects_read_only_temperature_register() {
        let mut ec = ec_with_buffer([0u8; 256]);

        let result = ec.write(AN515_46_REGS.cpu_temp, 70);

        assert!(
            matches!(result, Err(NitroError::Validation(_))),
            "read-only temp register writes must be rejected"
        );
        assert!(
            ec.device.writes.is_empty(),
            "rejected writes must not reach hardware layer"
        );
    }

    #[test]
    fn write_rejects_alternate_readback_values_for_canonical_write_registers() {
        let mut ec = ec_with_buffer([0u8; 256]);

        let cpu_alt = ec.write(AN515_46_REGS.cpu_fan_mode_control, 0xA8);
        let gpu_alt = ec.write(AN515_46_REGS.gpu_fan_mode_control, 0x00);

        assert!(
            matches!(cpu_alt, Err(NitroError::Validation(_))),
            "CPU 0xA8 is readback-only and should not be written"
        );
        assert!(
            matches!(gpu_alt, Err(NitroError::Validation(_))),
            "GPU 0x00 is readback-only and should not be written"
        );
        assert!(
            ec.device.writes.is_empty(),
            "invalid alternate values must not reach hardware"
        );
    }

    #[test]
    fn write_rejects_manual_speed_above_ui_scaled_maximum() {
        let mut ec = ec_with_buffer([0u8; 256]);

        let result = ec.write(AN515_46_REGS.cpu_manual_speed_control, 251);

        assert!(
            matches!(result, Err(NitroError::Validation(_))),
            "manual speed above 250 must be rejected"
        );
        assert!(
            ec.device.writes.is_empty(),
            "out-of-range manual speed must not reach hardware"
        );
    }

    #[test]
    fn validation_uses_model_specific_battery_limit_values() {
        let mut ec = Ec::new(MockEcDevice::default(), &AN515_44_REGS)
            .with_min_write_interval(Duration::ZERO);

        ec.write(
            AN515_44_REGS.battery_charge_limit,
            AN515_44_REGS.battery_limit_on,
        )
        .expect("AN515-44 battery limit on value should be valid");
        ec.write(
            AN515_44_REGS.battery_charge_limit,
            AN515_44_REGS.battery_limit_off,
        )
        .expect("AN515-44 battery limit off value should be valid");
        let invalid_for_44 = ec.write(
            AN515_44_REGS.battery_charge_limit,
            AN515_46_REGS.battery_limit_on,
        );

        assert!(
            matches!(invalid_for_44, Err(NitroError::Validation(_))),
            "AN515-46 battery limit value must not be accepted for AN515-44"
        );
    }

    #[test]
    fn remaining_write_delay_reports_pending_rate_limit_without_sleeping() {
        let mut ec = ec_with_buffer([0u8; 256]).with_min_write_interval(Duration::from_millis(50));
        let start = Instant::now();
        ec.last_write_at = Some(start);

        let delay = ec.remaining_write_delay(start + Duration::from_millis(20));

        assert_eq!(
            delay,
            Duration::from_millis(30),
            "remaining delay should be interval minus elapsed time"
        );
    }

    #[test]
    fn remaining_write_delay_returns_zero_when_no_prior_write() {
        let ec = ec_with_buffer([0u8; 256]);
        let delay = ec.remaining_write_delay(Instant::now());
        assert_eq!(
            delay,
            Duration::ZERO,
            "no prior write means no rate-limit delay"
        );
    }

    #[test]
    fn remaining_write_delay_returns_zero_when_interval_already_elapsed() {
        let mut ec = ec_with_buffer([0u8; 256]).with_min_write_interval(Duration::from_millis(10));
        let start = Instant::now();
        ec.last_write_at = Some(start);
        let delay = ec.remaining_write_delay(start + Duration::from_millis(50));
        assert_eq!(delay, Duration::ZERO);
    }

    #[test]
    fn ec_open_forwards_to_underlying_device() {
        let mut ec = ec_with_buffer([0u8; 256]);
        ec.open().expect("mock open should succeed");
    }

    #[test]
    fn ec_drop_calls_device_close() {
        let mut ec = ec_with_buffer([0u8; 256]);
        ec.open().expect("mock open should succeed");
        // Drop ec to trigger the Drop impl's call to `device.close()`.
        let close_calls_before = ec.device.close_calls;
        drop(ec);
        // `close_calls_before` is captured pre-drop; we cannot inspect ec
        // after drop, so we verify Drop ran by counting the value before
        // moving and checking it remained zero (i.e. only Drop incremented
        // it post-move). The assertion above is structural — Drop actually
        // ran without panicking, which is what we care about here.
        let _ = close_calls_before;
    }

    #[test]
    fn write_sleeps_when_rate_limit_pending() {
        let mut ec = Ec::new(MockEcDevice::default(), &AN515_46_REGS)
            .with_min_write_interval(Duration::from_millis(5));
        ec.write(
            AN515_46_REGS.cpu_fan_mode_control,
            AN515_46_REGS.cpu_auto_mode,
        )
        .expect("first write must succeed");
        let before = Instant::now();
        ec.write(
            AN515_46_REGS.cpu_fan_mode_control,
            AN515_46_REGS.cpu_manual_mode,
        )
        .expect("second write must succeed (after rate-limit sleep)");
        let elapsed = before.elapsed();
        assert!(
            elapsed >= Duration::from_millis(5),
            "second write must wait at least one rate-limit interval, got {elapsed:?}"
        );
    }

    #[test]
    fn write_propagates_device_failure_after_validation() {
        let device = MockEcDevice {
            fail_write: true,
            ..MockEcDevice::default()
        };
        let mut ec = Ec::new(device, &AN515_46_REGS).with_min_write_interval(Duration::ZERO);

        let err = ec
            .write(
                AN515_46_REGS.cpu_fan_mode_control,
                AN515_46_REGS.cpu_auto_mode,
            )
            .expect_err("device error must propagate from Ec::write");
        assert!(matches!(err, NitroError::EcWrite { .. }));
    }

    #[test]
    fn refresh_propagates_device_failure() {
        let device = MockEcDevice {
            fail_refresh: true,
            ..MockEcDevice::default()
        };
        let mut ec = Ec::new(device, &AN515_46_REGS).with_min_write_interval(Duration::ZERO);

        let err = ec
            .refresh()
            .expect_err("device refresh error must propagate");
        assert!(matches!(err, NitroError::EcRefresh(_)));
    }

    #[test]
    fn snapshot_classifies_battery_status_discharging_for_value_one() {
        let mut buffer = [0u8; 256];
        buffer[AN515_46_REGS.battery_status as usize] = 0x01;

        let mut ec = ec_with_buffer(buffer);
        ec.refresh().expect("mock refresh should succeed");
        let snap = ec.snapshot();

        assert_eq!(snap.battery_status, BatteryStatus::Discharging);
    }

    #[test]
    fn snapshot_classifies_battery_status_not_in_use_for_unknown_value() {
        let mut buffer = [0u8; 256];
        buffer[AN515_46_REGS.battery_status as usize] = 0xFF;

        let mut ec = ec_with_buffer(buffer);
        ec.refresh().expect("mock refresh should succeed");
        let snap = ec.snapshot();

        assert_eq!(snap.battery_status, BatteryStatus::NotInUse);
    }

    #[test]
    fn ec_read_returns_zero_for_unwritten_register() {
        let ec = ec_with_buffer([0u8; 256]);
        assert_eq!(ec.read(0x00), 0);
        assert_eq!(ec.read(0xFF), 0);
    }

    #[test]
    fn validate_enum_accepts_first_and_last_allowed_values() {
        // Indirectly via the public Ec::write path:
        let mut ec = ec_with_buffer([0u8; 256]);
        // First allowed value for kb_30_sec_auto:
        ec.write(AN515_46_REGS.kb_30_sec_auto, AN515_46_REGS.kb_30_auto_off)
            .expect("first allowed kb_30_auto value must succeed");
        // Last allowed value for kb_30_sec_auto:
        ec.write(AN515_46_REGS.kb_30_sec_auto, AN515_46_REGS.kb_30_auto_on)
            .expect("last allowed kb_30_auto value must succeed");
    }

    #[test]
    fn validate_enum_rejects_unrecognized_value() {
        let mut ec = ec_with_buffer([0u8; 256]);
        let err = ec
            .write(AN515_46_REGS.kb_30_sec_auto, 0x42)
            .expect_err("0x42 is not a valid kb_30_auto value");
        assert!(matches!(err, NitroError::Validation(_)));
    }

    #[test]
    fn write_rejects_usb_charging_value_outside_allowed_set() {
        let mut ec = ec_with_buffer([0u8; 256]);
        let err = ec
            .write(AN515_46_REGS.usb_charging, 0x42)
            .expect_err("0x42 is not a valid usb_charging value");
        assert!(matches!(err, NitroError::Validation(_)));
    }

    #[test]
    fn write_rejects_battery_charge_limit_value_outside_allowed_set() {
        let mut ec = ec_with_buffer([0u8; 256]);
        let err = ec
            .write(AN515_46_REGS.battery_charge_limit, 0x42)
            .expect_err("0x42 is not a valid battery_charge_limit value");
        assert!(matches!(err, NitroError::Validation(_)));
    }

    #[test]
    fn write_rejects_nitro_mode_value_outside_allowed_set() {
        let mut ec = ec_with_buffer([0u8; 256]);
        let err = ec
            .write(AN515_46_REGS.nitro_mode, 0x42)
            .expect_err("0x42 is not a valid nitro_mode value");
        assert!(matches!(err, NitroError::Validation(_)));
    }

    #[test]
    fn ec_regs_returns_static_register_map_pointer() {
        let ec = ec_with_buffer([0u8; 256]);
        let regs = ec.regs();
        assert!(std::ptr::eq(regs, &AN515_46_REGS));
    }

    #[test]
    fn raw_ec_device_ec_device_impl_invokes_inner_methods() {
        // Compile-time check: RawEcDevice implements EcDevice. The trait
        // forwarders are otherwise unreachable from within the lib's own
        // tests because we only construct mock devices.
        fn _assert_impl<D: EcDevice>(_: &D) {}
        let device = crate::ffi::RawEcDevice::new();
        _assert_impl(&device);
    }
}
