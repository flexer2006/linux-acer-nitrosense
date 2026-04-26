// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use egui::Ui;
use tokio::sync::mpsc::Sender;

use crate::app::events::Command;
use crate::app::state::AppState;

/// Render settings panel with toggles and system controls
pub fn render(
    ui: &mut Ui,
    state: &AppState,
    undervolt_core: &mut u8,
    command_tx: &Sender<Command>,
) {
    ui.heading("Settings");
    ui.separator();

    // Keyboard Backlight Timer
    ui.group(|ui| {
        let mut kb_timer = state.kb_timer_enabled;
        if ui
            .checkbox(&mut kb_timer, "Keyboard Backlight Timer (30s auto-off)")
            .changed()
        {
            let _ = command_tx.try_send(Command::ToggleKbTimer(kb_timer));
        }
    });

    ui.add_space(8.0);

    // USB Charging
    ui.group(|ui| {
        let mut usb_charging = state.usb_charging_enabled;
        if ui
            .checkbox(&mut usb_charging, "USB Power-off Charging")
            .changed()
        {
            let _ = command_tx.try_send(Command::ToggleUsbCharging(usb_charging));
        }
    });

    ui.add_space(8.0);

    // Battery Charge Limit
    ui.group(|ui| {
        let mut battery_limit = state.battery_limit_enabled;
        if ui
            .checkbox(&mut battery_limit, "Battery Charge Limit (80%)")
            .changed()
        {
            let _ = command_tx.try_send(Command::ToggleBatteryLimit(battery_limit));
        }
    });

    ui.add_space(16.0);
    ui.separator();

    // Undervolt Section (if available)
    render_undervolt_section(ui, state, undervolt_core, command_tx);

    ui.add_space(16.0);
    ui.separator();

    // Config Management
    ui.group(|ui| {
        ui.label("Configuration:");
        if ui.button("Save Config").clicked() {
            let _ = command_tx.try_send(Command::SaveConfig);
        }
    });

    ui.add_space(16.0);
    ui.separator();

    // Exit Button
    ui.vertical_centered(|ui| {
        if ui.button("Exit").clicked() {
            let _ = command_tx.try_send(Command::Shutdown);
        }
    });
}

fn render_undervolt_section(
    ui: &mut Ui,
    state: &AppState,
    undervolt_core: &mut u8,
    command_tx: &Sender<Command>,
) {
    ui.group(|ui| {
        ui.label("Undervolt:");

        // Display current status
        if !state.undervolt_status.is_empty() {
            ui.label("Status:");
            for line in state.undervolt_status.lines().take(5) {
                ui.monospace(line);
            }
        } else {
            ui.label("No undervolt applied.");
        }

        ui.add_space(8.0);

        // Core selection and apply
        ui.horizontal(|ui| {
            ui.label("Core:");
            ui.add(egui::DragValue::new(undervolt_core).range(0..=7));

            if ui.button("Apply Undervolt").clicked() {
                let _ = command_tx.try_send(Command::ApplyUndervolt(*undervolt_core));
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppState;
    use tokio::sync::mpsc;

    fn create_test_state() -> AppState {
        AppState {
            usb_charging_enabled: true,
            ..AppState::default()
        }
    }

    #[test]
    fn test_settings_state_creation() {
        let state = create_test_state();
        assert!(!state.kb_timer_enabled);
        assert!(state.usb_charging_enabled);
        assert!(!state.battery_limit_enabled);
    }

    // ---- Render exercises ----

    /// Test helper that drives one egui frame against a fresh context inside
    /// a CentralPanel. Equivalent to `egui::__run_test_ui` but accepts FnMut
    /// closures so tests can borrow mutable fixtures (e.g. `undervolt_core`).
    fn run_test_ui(mut add_contents: impl FnMut(&mut egui::Ui)) {
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::empty());
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| add_contents(ui));
        });
    }

    #[test]
    fn render_settings_panel_with_undervolt_status_uses_first_five_status_lines() {
        let (tx, _rx) = mpsc::channel::<Command>(8);
        let state = AppState {
            kb_timer_enabled: true,
            usb_charging_enabled: false,
            battery_limit_enabled: true,
            undervolt_status: (0..7)
                .map(|i| format!("status line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            ..AppState::default()
        };
        let mut undervolt_core = 4u8;
        run_test_ui(|ui| render(ui, &state, &mut undervolt_core, &tx));
    }

    #[test]
    fn render_settings_panel_with_no_undervolt_status_uses_placeholder_branch() {
        let (tx, _rx) = mpsc::channel::<Command>(8);
        let state = AppState::default();
        let mut undervolt_core = 0u8;
        run_test_ui(|ui| render(ui, &state, &mut undervolt_core, &tx));
    }

    #[test]
    fn render_settings_panel_with_all_toggles_enabled_uses_on_branches() {
        let (tx, _rx) = mpsc::channel::<Command>(8);
        let state = AppState {
            kb_timer_enabled: true,
            usb_charging_enabled: true,
            battery_limit_enabled: true,
            ..AppState::default()
        };
        let mut undervolt_core = 7u8;
        run_test_ui(|ui| render(ui, &state, &mut undervolt_core, &tx));
    }

    #[test]
    fn render_settings_panel_does_not_emit_commands_without_synthesized_clicks() {
        let (tx, mut rx) = mpsc::channel::<Command>(16);
        let state = AppState::default();
        let mut undervolt_core = 0u8;
        run_test_ui(|ui| render(ui, &state, &mut undervolt_core, &tx));
        assert!(
            rx.try_recv().is_err(),
            "rendering must not synthesize Command messages"
        );
    }

    // ---- Interactive tests via egui_kittest ----

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn build_settings_harness<'a>(
        state: AppState,
        tx: mpsc::Sender<Command>,
        undervolt_core: u8,
    ) -> Harness<'a, u8> {
        Harness::builder()
            .with_size(egui::Vec2::new(800.0, 600.0))
            .build_ui_state(
                move |ui, core| render(ui, &state, core, &tx),
                undervolt_core,
            )
    }

    #[test]
    fn clicking_kb_timer_checkbox_toggles_and_sends_command() {
        let (tx, mut rx) = mpsc::channel::<Command>(8);
        let state = AppState::default();
        let mut harness = build_settings_harness(state, tx, 0);

        harness
            .get_by_label("Keyboard Backlight Timer (30s auto-off)")
            .click();
        harness.run();

        let cmd = rx.try_recv().expect("kb_timer toggle must send command");
        assert!(matches!(cmd, Command::ToggleKbTimer(true)));
    }

    #[test]
    fn clicking_usb_charging_checkbox_toggles_and_sends_command() {
        let (tx, mut rx) = mpsc::channel::<Command>(8);
        let state = AppState::default();
        let mut harness = build_settings_harness(state, tx, 0);

        harness.get_by_label("USB Power-off Charging").click();
        harness.run();

        let cmd = rx
            .try_recv()
            .expect("usb charging toggle must send command");
        assert!(matches!(cmd, Command::ToggleUsbCharging(true)));
    }

    #[test]
    fn clicking_battery_limit_checkbox_toggles_and_sends_command() {
        let (tx, mut rx) = mpsc::channel::<Command>(8);
        let state = AppState::default();
        let mut harness = build_settings_harness(state, tx, 0);

        harness.get_by_label("Battery Charge Limit (80%)").click();
        harness.run();

        let cmd = rx
            .try_recv()
            .expect("battery limit toggle must send command");
        assert!(matches!(cmd, Command::ToggleBatteryLimit(true)));
    }

    #[test]
    fn clicking_save_config_button_sends_save_config_command() {
        let (tx, mut rx) = mpsc::channel::<Command>(8);
        let state = AppState::default();
        let mut harness = build_settings_harness(state, tx, 0);

        harness.get_by_label("Save Config").click();
        harness.run();

        let cmd = rx.try_recv().expect("save button must send command");
        assert!(matches!(cmd, Command::SaveConfig));
    }

    #[test]
    fn clicking_apply_undervolt_button_sends_apply_undervolt_command() {
        let (tx, mut rx) = mpsc::channel::<Command>(8);
        let state = AppState::default();
        let mut harness = build_settings_harness(state, tx, 5);

        harness.get_by_label("Apply Undervolt").click();
        harness.run();

        let cmd = rx
            .try_recv()
            .expect("apply undervolt button must send command");
        match cmd {
            Command::ApplyUndervolt(core) => assert_eq!(core, 5),
            other => panic!("expected ApplyUndervolt(5), got {other:?}"),
        }
    }

    #[test]
    fn clicking_exit_button_sends_shutdown_command() {
        let (tx, mut rx) = mpsc::channel::<Command>(8);
        let state = AppState::default();
        let mut harness = build_settings_harness(state, tx, 0);

        harness.get_by_label("Exit").click();
        harness.run();

        let cmd = rx.try_recv().expect("exit button must send command");
        assert!(matches!(cmd, Command::Shutdown));
    }
}
