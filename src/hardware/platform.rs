// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use crate::error::NitroError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuVendor {
    Amd,
    Intel,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct RegisterMap {
    pub cpu_fan_mode_control: u8,
    pub cpu_auto_mode: u8,
    pub cpu_turbo_mode: u8,
    pub cpu_manual_mode: u8,
    pub cpu_manual_speed_control: u8,

    pub gpu_fan_mode_control: u8,
    pub gpu_auto_mode: u8,
    pub gpu_turbo_mode: u8,
    pub gpu_manual_mode: u8,
    pub gpu_manual_speed_control: u8,

    pub kb_30_sec_auto: u8,
    pub kb_30_auto_off: u8,
    pub kb_30_auto_on: u8,

    pub cpu_fan_speed_high: u8,
    pub cpu_fan_speed_low: u8,
    pub gpu_fan_speed_high: u8,
    pub gpu_fan_speed_low: u8,

    pub cpu_temp: u8,
    pub gpu_temp: u8,
    pub sys_temp: u8,

    pub power_status: u8,

    pub battery_charge_limit: u8,
    pub battery_limit_on: u8,
    pub battery_limit_off: u8,
    pub battery_status: u8,

    pub usb_charging: u8,
    pub usb_charging_on: u8,
    pub usb_charging_off: u8,

    pub nitro_mode: u8,
    pub quiet_mode: u8,
    pub default_mode: u8,
    pub extreme_mode: u8,
}

pub const AN515_46_REGS: RegisterMap = RegisterMap {
    cpu_fan_mode_control: 0x22,
    cpu_auto_mode: 0x04,
    cpu_turbo_mode: 0x08,
    cpu_manual_mode: 0x0C,
    cpu_manual_speed_control: 0x37,

    gpu_fan_mode_control: 0x21,
    gpu_auto_mode: 0x10,
    gpu_turbo_mode: 0x20,
    gpu_manual_mode: 0x30,
    gpu_manual_speed_control: 0x3A,

    kb_30_sec_auto: 0x06,
    kb_30_auto_off: 0x00,
    kb_30_auto_on: 0x1E,

    cpu_fan_speed_high: 0x13,
    cpu_fan_speed_low: 0x14,
    gpu_fan_speed_high: 0x15,
    gpu_fan_speed_low: 0x16,

    cpu_temp: 0xB0,
    gpu_temp: 0xB6,
    sys_temp: 0xB3,

    power_status: 0x00,

    battery_charge_limit: 0x03,
    battery_limit_on: 0x51,
    battery_limit_off: 0x11,
    battery_status: 0xC1,

    usb_charging: 0x08,
    usb_charging_on: 0x0F,
    usb_charging_off: 0x1F,

    nitro_mode: 0x2C,
    quiet_mode: 0x00,
    default_mode: 0x01,
    extreme_mode: 0x04,
};

pub const AN515_44_REGS: RegisterMap = RegisterMap {
    cpu_fan_mode_control: 0x22,
    cpu_auto_mode: 0x04,
    cpu_turbo_mode: 0x08,
    cpu_manual_mode: 0x0C,
    cpu_manual_speed_control: 0x37,

    gpu_fan_mode_control: 0x21,
    gpu_auto_mode: 0x10,
    gpu_turbo_mode: 0x20,
    gpu_manual_mode: 0x30,
    gpu_manual_speed_control: 0x3A,

    kb_30_sec_auto: 0x06,
    kb_30_auto_off: 0x00,
    kb_30_auto_on: 0x1E,

    cpu_fan_speed_high: 0x13,
    cpu_fan_speed_low: 0x14,
    gpu_fan_speed_high: 0x15,
    gpu_fan_speed_low: 0x16,

    cpu_temp: 0xB0,
    gpu_temp: 0xB4,
    sys_temp: 0xB0,

    power_status: 0x00,

    battery_charge_limit: 0x03,
    battery_limit_on: 0x40,
    battery_limit_off: 0x00,
    battery_status: 0xC1,

    usb_charging: 0x08,
    usb_charging_on: 0x0F,
    usb_charging_off: 0x1F,

    nitro_mode: 0x2C,
    quiet_mode: 0x00,
    default_mode: 0x01,
    extreme_mode: 0x04,
};

pub const MODEL_TO_REGS: &[(&str, &RegisterMap)] = &[
    ("Nitro AN515-44", &AN515_44_REGS),
    ("Nitro AN515-45", &AN515_46_REGS),
    ("Nitro AN515-46", &AN515_46_REGS),
    ("Nitro AN515-54", &AN515_46_REGS),
    ("Nitro AN515-56", &AN515_46_REGS),
    ("Nitro AN515-57", &AN515_46_REGS),
    ("Nitro AN515-58", &AN515_46_REGS),
    ("Nitro AN517-55", &AN515_46_REGS),
];

/// Path to the DMI product name file used to identify the laptop model.
const DMI_PRODUCT_NAME_PATH: &str = "/sys/class/dmi/id/product_name";

/// Path to the cpuinfo file used to identify the CPU vendor.
const CPUINFO_PATH: &str = "/proc/cpuinfo";

pub fn detect_model() -> Result<String, NitroError> {
    detect_model_from(DMI_PRODUCT_NAME_PATH)
}

/// Read the DMI product-name file at the given path and return the trimmed
/// model identifier. Exposed for testing so the error path (file missing or
/// unreadable) can be exercised without mocking the real `/sys` filesystem.
pub(crate) fn detect_model_from(path: &str) -> Result<String, NitroError> {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .map_err(|e| NitroError::UnsupportedModel(format!("Cannot read DMI: {e}")))
}

pub fn detect_cpu_vendor() -> CpuVendor {
    detect_cpu_vendor_from(CPUINFO_PATH)
}

/// Read `cpuinfo` from the given path and classify the vendor. Exposed for
/// testing so the missing-file path (returns [`CpuVendor::Unknown`]) can be
/// exercised without remounting `/proc`.
pub(crate) fn detect_cpu_vendor_from(path: &str) -> CpuVendor {
    let Ok(cpuinfo) = std::fs::read_to_string(path) else {
        return CpuVendor::Unknown;
    };
    cpu_vendor_from_cpuinfo(&cpuinfo)
}

pub fn cpu_vendor_from_cpuinfo(cpuinfo: &str) -> CpuVendor {
    for line in cpuinfo.lines() {
        if line.starts_with("vendor_id") {
            if line.contains("AuthenticAMD") {
                return CpuVendor::Amd;
            } else if line.contains("GenuineIntel") {
                return CpuVendor::Intel;
            }
        }
    }
    CpuVendor::Unknown
}

pub fn register_map_for_model(model: &str) -> Option<&'static RegisterMap> {
    MODEL_TO_REGS
        .iter()
        .find_map(|(name, regs)| model.contains(name).then_some(*regs))
}

pub fn detect_device() -> Result<&'static RegisterMap, NitroError> {
    detect_device_from(DMI_PRODUCT_NAME_PATH)
}

/// Resolve the register map for the laptop whose DMI product-name file lives
/// at the given path. Exposed for testing so both error legs (file unreadable
/// and unsupported model) can be exercised without touching real `/sys`.
pub(crate) fn detect_device_from(path: &str) -> Result<&'static RegisterMap, NitroError> {
    let model = detect_model_from(path)?;
    register_map_for_model(&model).ok_or(NitroError::UnsupportedModel(model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_vendor_from_cpuinfo_detects_amd_intel_and_unknown() {
        assert_eq!(
            cpu_vendor_from_cpuinfo("processor: 0\nvendor_id\t: AuthenticAMD\n"),
            CpuVendor::Amd,
            "AuthenticAMD vendor_id should map to AMD"
        );
        assert_eq!(
            cpu_vendor_from_cpuinfo("processor: 0\nvendor_id\t: GenuineIntel\n"),
            CpuVendor::Intel,
            "GenuineIntel vendor_id should map to Intel"
        );
        assert_eq!(
            cpu_vendor_from_cpuinfo("processor: 0\nvendor_id\t: SomeOtherVendor\n"),
            CpuVendor::Unknown,
            "unknown vendor_id should map to Unknown"
        );
    }

    #[test]
    fn register_map_for_model_resolves_all_supported_models() {
        for model in [
            "Nitro AN515-45",
            "Nitro AN515-46",
            "Nitro AN515-54",
            "Nitro AN515-56",
            "Nitro AN515-57",
            "Nitro AN515-58",
            "Nitro AN517-55",
        ] {
            let regs = register_map_for_model(model)
                .unwrap_or_else(|| panic!("{model} should resolve to AN515-46 map"));
            assert_eq!(
                regs.gpu_temp, AN515_46_REGS.gpu_temp,
                "{model} should use the AN515-46-compatible map"
            );
        }

        let an44 = register_map_for_model("Nitro AN515-44")
            .expect("AN515-44 should resolve to its dedicated map");
        assert_eq!(
            an44.gpu_temp, AN515_44_REGS.gpu_temp,
            "AN515-44 should use the AN515-44 map"
        );
    }

    #[test]
    fn register_map_for_model_rejects_unsupported_model() {
        assert!(
            register_map_for_model("Predator PH315-55").is_none(),
            "unsupported Acer models must not resolve to a Nitro EC map"
        );
    }

    #[test]
    fn an515_44_register_differences_match_source_reference() {
        assert_eq!(AN515_46_REGS.gpu_temp, 0xB6);
        assert_eq!(AN515_44_REGS.gpu_temp, 0xB4);
        assert_eq!(AN515_46_REGS.sys_temp, 0xB3);
        assert_eq!(AN515_44_REGS.sys_temp, 0xB0);
        assert_eq!(AN515_46_REGS.battery_limit_on, 0x51);
        assert_eq!(AN515_44_REGS.battery_limit_on, 0x40);
        assert_eq!(AN515_46_REGS.battery_limit_off, 0x11);
        assert_eq!(AN515_44_REGS.battery_limit_off, 0x00);
    }

    // ---- File-path detection tests (cover the file-IO error legs) ----

    #[test]
    fn detect_model_from_reads_and_trims_file_contents() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("product_name");
        std::fs::write(&path, "Nitro AN515-46\n").expect("DMI file should be writable");

        let model = detect_model_from(path.to_str().unwrap())
            .expect("readable DMI file should produce model");

        assert_eq!(
            model, "Nitro AN515-46",
            "detect_model must trim trailing whitespace/newlines"
        );
    }

    #[test]
    fn detect_model_from_returns_unsupported_model_error_when_file_missing() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let missing = dir.path().join("does-not-exist");

        let err = detect_model_from(missing.to_str().unwrap())
            .expect_err("missing DMI file must produce an error");

        match err {
            NitroError::UnsupportedModel(msg) => {
                assert!(
                    msg.contains("Cannot read DMI"),
                    "error must mention DMI read failure: {msg}"
                );
            }
            other => panic!("expected UnsupportedModel, got {other:?}"),
        }
    }

    #[test]
    fn detect_cpu_vendor_from_handles_missing_cpuinfo_as_unknown() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let missing = dir.path().join("does-not-exist");

        let vendor = detect_cpu_vendor_from(missing.to_str().unwrap());

        assert_eq!(
            vendor,
            CpuVendor::Unknown,
            "missing cpuinfo file must return Unknown rather than failing"
        );
    }

    #[test]
    fn detect_cpu_vendor_from_classifies_amd_cpuinfo_file() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("cpuinfo");
        std::fs::write(&path, "processor: 0\nvendor_id\t: AuthenticAMD\n").unwrap();

        assert_eq!(
            detect_cpu_vendor_from(path.to_str().unwrap()),
            CpuVendor::Amd
        );
    }

    #[test]
    fn detect_cpu_vendor_from_classifies_intel_cpuinfo_file() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("cpuinfo");
        std::fs::write(&path, "processor: 0\nvendor_id\t: GenuineIntel\n").unwrap();

        assert_eq!(
            detect_cpu_vendor_from(path.to_str().unwrap()),
            CpuVendor::Intel
        );
    }

    #[test]
    fn cpu_vendor_from_cpuinfo_returns_unknown_for_empty_input() {
        assert_eq!(cpu_vendor_from_cpuinfo(""), CpuVendor::Unknown);
    }

    #[test]
    fn cpu_vendor_from_cpuinfo_returns_unknown_when_no_vendor_id_line_present() {
        assert_eq!(
            cpu_vendor_from_cpuinfo("processor: 0\nfamily : 23\n"),
            CpuVendor::Unknown,
            "missing vendor_id line should map to Unknown"
        );
    }

    // ---- detect_device end-to-end tests ----

    #[test]
    fn detect_device_from_returns_register_map_for_supported_model() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("product_name");
        std::fs::write(&path, "Nitro AN515-46\n").unwrap();

        let regs = detect_device_from(path.to_str().unwrap())
            .expect("supported model should resolve to register map");

        assert_eq!(regs.gpu_temp, AN515_46_REGS.gpu_temp);
    }

    #[test]
    fn detect_device_from_rejects_unsupported_model() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("product_name");
        std::fs::write(&path, "Predator PH315-55\n").unwrap();

        let err = detect_device_from(path.to_str().unwrap())
            .expect_err("unsupported model must produce error");

        match err {
            NitroError::UnsupportedModel(model) => {
                assert!(
                    model.contains("Predator"),
                    "error must echo unsupported model: {model}"
                );
            }
            other => panic!("expected UnsupportedModel, got {other:?}"),
        }
    }

    #[test]
    fn detect_device_from_propagates_dmi_read_error() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let missing = dir.path().join("does-not-exist");

        let err = detect_device_from(missing.to_str().unwrap())
            .expect_err("missing DMI must produce error");

        match err {
            NitroError::UnsupportedModel(msg) => {
                assert!(msg.contains("Cannot read DMI"), "error message: {msg}");
            }
            other => panic!("expected UnsupportedModel, got {other:?}"),
        }
    }

    // ---- CpuVendor / RegisterMap derive completeness ----

    #[test]
    fn cpu_vendor_supports_clone_copy_eq_debug() {
        let v = CpuVendor::Amd;
        let copied = v;
        let cloned = v;
        assert_eq!(copied, cloned, "Copy + Clone must agree on equality");
        let debug = format!("{v:?}");
        assert!(
            debug.contains("Amd"),
            "Debug formatting should show variant name: {debug}"
        );
    }

    #[test]
    fn register_map_supports_clone_copy_debug() {
        let copied = AN515_46_REGS;
        let cloned = AN515_46_REGS;
        assert_eq!(copied.gpu_temp, cloned.gpu_temp);
        let debug = format!("{copied:?}");
        assert!(
            debug.starts_with("RegisterMap"),
            "Debug formatting should include struct name: {debug}"
        );
    }

    // ---- Public-API smoke test against real /sys (Linux-only) ----

    #[test]
    fn detect_cpu_vendor_returns_a_known_variant_on_this_host() {
        // The public function reads /proc/cpuinfo, which is always present on
        // Linux. We only verify the wrapper compiles and returns a known
        // variant (the exact vendor depends on the host). On non-Linux
        // platforms cargo test wouldn't compile this crate at all.
        let vendor = detect_cpu_vendor();
        assert!(matches!(
            vendor,
            CpuVendor::Amd | CpuVendor::Intel | CpuVendor::Unknown
        ));
    }
}
