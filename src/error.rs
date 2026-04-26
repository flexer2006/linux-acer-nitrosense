// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum NitroError {
    #[error("EC open failed: {0}")]
    EcOpen(#[source] io::Error),

    #[error("EC refresh failed: {0}")]
    EcRefresh(#[source] io::Error),

    #[error("EC write failed at addr 0x{addr:02X}: {source}")]
    EcWrite {
        addr: u8,
        #[source]
        source: io::Error,
    },

    #[error("MSR open failed for CPU {cpu}: {source}")]
    MsrOpen {
        cpu: i32,
        #[source]
        source: io::Error,
    },

    #[error("MSR read failed for register 0x{msr:08X}: {source}")]
    MsrRead {
        msr: u32,
        #[source]
        source: io::Error,
    },

    #[error("Unsupported device model: {0}")]
    UnsupportedModel(String),

    #[error("Config error: {0}")]
    Config(#[source] anyhow::Error),

    #[error("RGB device not found: {0}")]
    RgbDevice(String),

    #[error("Validation failed: {0}")]
    Validation(String),

    #[error("Telemetry poller failed: {0}")]
    Poller(String),

    #[error("Process command failed: {0}")]
    Process(String),
}
