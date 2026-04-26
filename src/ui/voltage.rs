// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::fmt::Write;

use egui::Ui;

use crate::app::state::AppState;
use crate::ui::fmtbuf::FmtBuf;

/// Format a voltage value into a stack-allocated buffer.
/// Returns an empty buffer for non-finite or extreme values,
/// which the caller can detect via `is_empty()` and substitute
/// a fallback string (e.g. em dash "—").
fn format_voltage_buf(value: f64) -> FmtBuf<16> {
    let mut buf = FmtBuf::<16>::new();
    if value.is_finite() && value < f64::MAX / 2.0 {
        let _ = write!(buf, "{value:.2} V");
    }
    buf
}

/// Render voltage monitoring sub-panel.
///
/// Uses stack-allocated `FmtBuf` for voltage formatting to avoid per-frame
/// `String` heap allocations. The min/max line also uses a stack buffer.
pub fn render(ui: &mut Ui, state: &AppState) {
    ui.group(|ui| {
        ui.label("Voltage:");
        ui.horizontal(|ui| {
            ui.label("Current:");
            let buf = format_voltage_buf(state.voltage);
            ui.monospace(buf.as_str_or("\u{2014}"));
        });
        ui.horizontal(|ui| {
            ui.label("Min / Max:");
            let min_buf = format_voltage_buf(state.min_voltage);
            let max_buf = format_voltage_buf(state.max_voltage);
            // Combined "X / Y" line uses a 32-byte stack buffer
            let mut combined = FmtBuf::<32>::new();
            let _ = write!(
                combined,
                "{} / {}",
                min_buf.as_str_or("\u{2014}"),
                max_buf.as_str_or("\u{2014}")
            );
            ui.monospace(combined.as_str());
        });

        ui.label("Undervolt:");
        if state.undervolt_status.is_empty() {
            ui.monospace("No undervolt status available.");
        } else {
            for line in state.undervolt_status.lines() {
                ui.monospace(line);
            }
        }
    });
}

fn format_voltage(value: f64) -> String {
    if value.is_finite() && value < f64::MAX / 2.0 {
        format!("{value:.2} V")
    } else {
        "\u{2014}".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_voltage_normal_value() {
        assert_eq!(format_voltage(1.35), "1.35 V");
    }

    #[test]
    fn format_voltage_zero() {
        assert_eq!(format_voltage(0.0), "0.00 V");
    }

    #[test]
    fn format_voltage_rounds_to_two_decimals() {
        assert_eq!(format_voltage(1.123456), "1.12 V");
    }

    #[test]
    fn format_voltage_infinity_returns_dash() {
        assert_eq!(format_voltage(f64::INFINITY), "\u{2014}");
    }

    #[test]
    fn format_voltage_nan_returns_dash() {
        assert_eq!(format_voltage(f64::NAN), "\u{2014}");
    }

    #[test]
    fn format_voltage_huge_value_returns_dash() {
        assert_eq!(format_voltage(f64::MAX), "\u{2014}");
    }

    #[test]
    fn format_voltage_negative_infinity_returns_dash() {
        assert_eq!(format_voltage(f64::NEG_INFINITY), "\u{2014}");
    }

    #[test]
    fn format_voltage_small_negative_value() {
        assert_eq!(format_voltage(-0.01), "-0.01 V");
    }

    // ---- Render exercises ----

    #[test]
    fn render_voltage_panel_with_status_text_uses_multiline_branch() {
        let state = AppState {
            voltage: 1.20,
            min_voltage: 1.0,
            max_voltage: 1.4,
            undervolt_status: "line one\nline two".to_string(),
            ..AppState::default()
        };
        egui::__run_test_ui(|ui| render(ui, &state));
    }

    #[test]
    fn render_voltage_panel_with_empty_status_uses_placeholder_branch() {
        let state = AppState {
            voltage: 0.0,
            min_voltage: f64::MAX,
            max_voltage: 0.0,
            undervolt_status: String::new(),
            ..AppState::default()
        };
        egui::__run_test_ui(|ui| render(ui, &state));
    }

    #[test]
    fn render_voltage_panel_handles_unfinite_voltage_values() {
        let state = AppState {
            voltage: f64::NAN,
            min_voltage: f64::INFINITY,
            max_voltage: f64::NEG_INFINITY,
            undervolt_status: String::new(),
            ..AppState::default()
        };
        egui::__run_test_ui(|ui| render(ui, &state));
    }
}
