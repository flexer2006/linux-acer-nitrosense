// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use crate::app::state::BatteryStatus;
use crate::error::NitroError;
use crate::hardware::ec::{Ec, EcDevice};

pub fn toggle_battery_limit<D: EcDevice>(ec: &mut Ec<D>, enable: bool) -> Result<(), NitroError> {
    let regs = ec.regs();
    let value = if enable {
        regs.battery_limit_on
    } else {
        regs.battery_limit_off
    };
    ec.write(regs.battery_charge_limit, value)
}

pub fn toggle_usb_charging<D: EcDevice>(ec: &mut Ec<D>, enable: bool) -> Result<(), NitroError> {
    let regs = ec.regs();
    let value = if enable {
        regs.usb_charging_on
    } else {
        regs.usb_charging_off
    };
    ec.write(regs.usb_charging, value)
}

pub fn toggle_kb_timer<D: EcDevice>(ec: &mut Ec<D>, enable: bool) -> Result<(), NitroError> {
    let regs = ec.regs();
    let value = if enable {
        regs.kb_30_auto_on
    } else {
        regs.kb_30_auto_off
    };
    ec.write(regs.kb_30_sec_auto, value)
}

// ---- Display-label helpers ----

/// Returns a human-readable label for power plug status.
pub fn power_status_label(plugged_in: bool) -> &'static str {
    if plugged_in {
        "Plugged In"
    } else {
        "Unplugged"
    }
}

/// Returns a human-readable label for battery status.
pub fn battery_status_label(status: BatteryStatus) -> &'static str {
    match status {
        BatteryStatus::Charging => "Charging",
        BatteryStatus::Discharging => "Discharging",
        BatteryStatus::NotInUse => "Battery Not In Use",
    }
}

/// Returns a human-readable label for a boolean toggle (On/Off).
pub fn toggle_label(enabled: bool) -> &'static str {
    if enabled { "On" } else { "Off" }
}

/// Returns a human-readable label for battery charge limit status.
pub fn battery_limit_label(enabled: bool) -> &'static str {
    toggle_label(enabled)
}

/// Returns a human-readable label for USB charging status.
pub fn usb_charging_label(enabled: bool) -> &'static str {
    toggle_label(enabled)
}

/// Returns a human-readable label for keyboard backlight timer status.
pub fn kb_timer_label(enabled: bool) -> &'static str {
    if enabled {
        "30 sec auto-off"
    } else {
        "Always on"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::platform::{AN515_44_REGS, AN515_46_REGS};
    use crate::test_support::RecordingEcDevice;
    use std::time::Duration;

    #[derive(Debug, Default)]
    struct MockEcDevice(RecordingEcDevice);

    impl std::ops::Deref for MockEcDevice {
        type Target = RecordingEcDevice;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl EcDevice for MockEcDevice {
        fn open(&mut self) -> Result<(), NitroError> {
            self.0.open()
        }

        fn close(&mut self) {
            self.0.close();
        }

        fn refresh(&mut self, buffer: &mut [u8]) -> Result<usize, NitroError> {
            self.0.refresh(buffer)
        }

        fn write_byte(&mut self, addr: u8, val: u8) -> Result<(), NitroError> {
            self.0.write_byte(addr, val)
        }
    }

    fn ec(regs: &'static crate::hardware::platform::RegisterMap) -> Ec<MockEcDevice> {
        Ec::new(MockEcDevice::default(), regs).with_min_write_interval(Duration::ZERO)
    }

    #[test]
    fn battery_limit_toggle_uses_an515_46_values() {
        let mut ec = ec(&AN515_46_REGS);

        toggle_battery_limit(&mut ec, true).expect("enabling battery limit should write");
        toggle_battery_limit(&mut ec, false).expect("disabling battery limit should write");

        assert_eq!(
            ec.device_ref().writes,
            vec![
                (
                    AN515_46_REGS.battery_charge_limit,
                    AN515_46_REGS.battery_limit_on
                ),
                (
                    AN515_46_REGS.battery_charge_limit,
                    AN515_46_REGS.battery_limit_off
                ),
            ],
            "AN515-46 battery limit toggle should use 0x51/0x11"
        );
    }

    #[test]
    fn battery_limit_toggle_uses_an515_44_values() {
        let mut ec = ec(&AN515_44_REGS);

        toggle_battery_limit(&mut ec, true).expect("enabling battery limit should write");
        toggle_battery_limit(&mut ec, false).expect("disabling battery limit should write");

        assert_eq!(
            ec.device_ref().writes,
            vec![
                (
                    AN515_44_REGS.battery_charge_limit,
                    AN515_44_REGS.battery_limit_on
                ),
                (
                    AN515_44_REGS.battery_charge_limit,
                    AN515_44_REGS.battery_limit_off
                ),
            ],
            "AN515-44 battery limit toggle should use 0x40/0x00"
        );
    }

    #[test]
    fn usb_charging_toggle_writes_known_values() {
        let mut ec = ec(&AN515_46_REGS);

        toggle_usb_charging(&mut ec, true).expect("enabling USB charging should write");
        toggle_usb_charging(&mut ec, false).expect("disabling USB charging should write");

        assert_eq!(
            ec.device_ref().writes,
            vec![
                (AN515_46_REGS.usb_charging, AN515_46_REGS.usb_charging_on),
                (AN515_46_REGS.usb_charging, AN515_46_REGS.usb_charging_off),
            ],
            "USB charging toggle should use original 0x0F/0x1F values"
        );
    }

    #[test]
    fn keyboard_timer_toggle_writes_known_values() {
        let mut ec = ec(&AN515_46_REGS);

        toggle_kb_timer(&mut ec, true).expect("enabling keyboard timer should write");
        toggle_kb_timer(&mut ec, false).expect("disabling keyboard timer should write");

        assert_eq!(
            ec.device_ref().writes,
            vec![
                (AN515_46_REGS.kb_30_sec_auto, AN515_46_REGS.kb_30_auto_on),
                (AN515_46_REGS.kb_30_sec_auto, AN515_46_REGS.kb_30_auto_off),
            ],
            "keyboard timer toggle should use original 0x1E/0x00 values"
        );
    }

    // ---- Display-label helper tests ----

    #[test]
    fn power_status_label_plugged_in_returns_correct_string() {
        assert_eq!(power_status_label(true), "Plugged In");
    }

    #[test]
    fn power_status_label_unplugged_returns_correct_string() {
        assert_eq!(power_status_label(false), "Unplugged");
    }

    #[test]
    fn battery_status_label_charging_returns_charging() {
        assert_eq!(battery_status_label(BatteryStatus::Charging), "Charging");
    }

    #[test]
    fn battery_status_label_discharging_returns_discharging() {
        assert_eq!(
            battery_status_label(BatteryStatus::Discharging),
            "Discharging"
        );
    }

    #[test]
    fn battery_status_label_not_in_use_returns_correct_string() {
        assert_eq!(
            battery_status_label(BatteryStatus::NotInUse),
            "Battery Not In Use"
        );
    }

    #[test]
    fn toggle_label_on_returns_on() {
        assert_eq!(toggle_label(true), "On");
    }

    #[test]
    fn toggle_label_off_returns_off() {
        assert_eq!(toggle_label(false), "Off");
    }

    #[test]
    fn battery_limit_label_on_returns_on() {
        assert_eq!(battery_limit_label(true), "On");
    }

    #[test]
    fn battery_limit_label_off_returns_off() {
        assert_eq!(battery_limit_label(false), "Off");
    }

    #[test]
    fn usb_charging_label_on_returns_on() {
        assert_eq!(usb_charging_label(true), "On");
    }

    #[test]
    fn usb_charging_label_off_returns_off() {
        assert_eq!(usb_charging_label(false), "Off");
    }

    #[test]
    fn kb_timer_label_enabled_returns_30_sec_auto_off() {
        assert_eq!(kb_timer_label(true), "30 sec auto-off");
    }

    #[test]
    fn kb_timer_label_disabled_returns_always_on() {
        assert_eq!(kb_timer_label(false), "Always on");
    }
}
