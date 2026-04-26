// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NitroConfig {
    pub cpu_mode: u8,
    pub gpu_mode: u8,
    pub kb_30_timeout: u8,
    pub usb_charging: u8,
    pub nitro_mode: u8,
    pub battery_charge_limit: u8,
}

impl Default for NitroConfig {
    fn default() -> Self {
        Self {
            cpu_mode: 0x04,
            gpu_mode: 0x10,
            kb_30_timeout: 0x00,
            usb_charging: 0x0F,
            nitro_mode: 0x01,
            battery_charge_limit: 0x11,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbConfig {
    pub mode: u8,
    pub zone: u8,
    pub speed: u8,
    pub brightness: u8,
    pub direction: u8,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Default for RgbConfig {
    fn default() -> Self {
        Self {
            mode: 0,
            zone: 0,
            speed: 1,
            brightness: 100,
            direction: 1,
            red: 255,
            green: 255,
            blue: 255,
        }
    }
}
