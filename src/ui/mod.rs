// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

pub mod dashboard;
pub mod fans;
pub mod fmtbuf;
pub mod keyboard;
pub mod profiles;
pub mod settings;
pub mod theme;
pub mod voltage;

/// Drives one egui frame inside a `CentralPanel`.
///
/// Equivalent to `egui::__run_test_ui`, but accepts `FnMut` so tests can borrow
/// mutable fixtures such as `undervolt_core`.
#[cfg(test)]
pub(crate) fn run_test_ui(mut add_contents: impl FnMut(&mut egui::Ui)) {
    let ctx = egui::Context::default();
    ctx.set_fonts(egui::FontDefinitions::empty());
    let output = ctx.run_ui(Default::default(), |ui| {
        egui::CentralPanel::default().show(ui, |ui| add_contents(ui));
    });
    output.drop_without_applying_deltas();
}
