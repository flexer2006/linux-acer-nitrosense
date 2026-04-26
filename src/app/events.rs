// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use crate::app::state::{FanMode, PerformanceProfile};
use crate::config::model::RgbConfig;

#[derive(Debug, Clone)]
pub enum Command {
    SetCpuFanMode(FanMode),
    SetGpuFanMode(FanMode),
    SetCpuManualSpeed(u8),
    SetGpuManualSpeed(u8),
    SetProfile(PerformanceProfile),
    ToggleTurbo(bool),
    ToggleKbTimer(bool),
    ToggleUsbCharging(bool),
    ToggleBatteryLimit(bool),
    ApplyRgb(RgbConfig),
    SaveRgbConfig,
    LoadRgbConfig,
    ApplyUndervolt(u8),
    SaveConfig,
    Shutdown,
}
