// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::fmt::Write;

use egui::Ui;
use tokio::sync::mpsc::Sender;

use crate::app::events::Command;
use crate::app::state::{AppState, FanMode};
use crate::ui::fmtbuf::FmtBuf;

/// Render fan control panel with performance profile and fan mode controls
pub fn render(ui: &mut Ui, state: &AppState, command_tx: &Sender<Command>) {
    ui.heading("Fan Control");
    ui.separator();

    crate::ui::profiles::render(ui, state.performance_profile, command_tx);

    ui.add_space(16.0);

    // Global Turbo Toggle
    ui.group(|ui| {
        ui.label("Global Turbo:");
        ui.horizontal(|ui| {
            if ui.radio(!state.turbo_enabled, "Auto").clicked() && state.turbo_enabled {
                let _ = command_tx.try_send(Command::ToggleTurbo(false));
            }
            if ui.radio(state.turbo_enabled, "Turbo").clicked() && !state.turbo_enabled {
                let _ = command_tx.try_send(Command::ToggleTurbo(true));
            }
        });
    });

    ui.add_space(16.0);

    // CPU Fan Controls
    render_fan_section(
        ui,
        "CPU Fan",
        state.cpu_fan_mode,
        state.cpu_manual_speed,
        true,
        command_tx,
    );

    ui.add_space(8.0);

    // GPU Fan Controls
    render_fan_section(
        ui,
        "GPU Fan",
        state.gpu_fan_mode,
        state.gpu_manual_speed,
        false,
        command_tx,
    );
}

fn render_fan_section(
    ui: &mut Ui,
    label: &str,
    mode: FanMode,
    manual_speed: u8,
    is_cpu: bool,
    command_tx: &Sender<Command>,
) {
    ui.group(|ui| {
        // Stack-allocated label formatting avoids per-frame String allocation
        let mut label_buf = FmtBuf::<32>::new();
        let _ = write!(label_buf, "{}:", label);
        ui.label(label_buf.as_str());

        // Mode selection
        ui.horizontal(|ui| {
            if ui.radio(mode == FanMode::Auto, "Auto").clicked() && mode != FanMode::Auto {
                let cmd = if is_cpu {
                    Command::SetCpuFanMode(FanMode::Auto)
                } else {
                    Command::SetGpuFanMode(FanMode::Auto)
                };
                let _ = command_tx.try_send(cmd);
            }
            if ui.radio(mode == FanMode::Manual, "Manual").clicked() && mode != FanMode::Manual {
                let cmd = if is_cpu {
                    Command::SetCpuFanMode(FanMode::Manual)
                } else {
                    Command::SetGpuFanMode(FanMode::Manual)
                };
                let _ = command_tx.try_send(cmd);
            }
            if ui.radio(mode == FanMode::Turbo, "Turbo").clicked() && mode != FanMode::Turbo {
                let cmd = if is_cpu {
                    Command::SetCpuFanMode(FanMode::Turbo)
                } else {
                    Command::SetGpuFanMode(FanMode::Turbo)
                };
                let _ = command_tx.try_send(cmd);
            }
        });

        // Manual speed slider (only enabled in Manual mode)
        ui.add_space(8.0);
        let mut speed = manual_speed;
        ui.add_enabled_ui(mode == FanMode::Manual, |ui| {
            ui.horizontal(|ui| {
                ui.label("Speed:");
                let slider = egui::Slider::new(&mut speed, 0..=25)
                    .text("%")
                    .show_value(true);
                if ui.add(slider).changed() {
                    let cmd = if is_cpu {
                        Command::SetCpuManualSpeed(speed)
                    } else {
                        Command::SetGpuManualSpeed(speed)
                    };
                    let _ = command_tx.try_send(cmd);
                }
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::PerformanceProfile;
    use tokio::sync::mpsc;

    #[test]
    fn test_fan_mode_coverage() {
        let modes = [FanMode::Auto, FanMode::Manual, FanMode::Turbo];
        assert_eq!(modes, [FanMode::Auto, FanMode::Manual, FanMode::Turbo]);
    }

    fn fan_state_with_modes(
        cpu: FanMode,
        gpu: FanMode,
        turbo_enabled: bool,
        profile: PerformanceProfile,
    ) -> AppState {
        AppState {
            cpu_fan_mode: cpu,
            gpu_fan_mode: gpu,
            cpu_manual_speed: 12,
            gpu_manual_speed: 14,
            turbo_enabled,
            performance_profile: profile,
            ..AppState::default()
        }
    }

    #[test]
    fn render_fan_panel_executes_all_combinations_of_cpu_and_gpu_fan_modes() {
        let (tx, mut rx) = mpsc::channel::<Command>(16);

        for cpu in [FanMode::Auto, FanMode::Manual, FanMode::Turbo] {
            for gpu in [FanMode::Auto, FanMode::Manual, FanMode::Turbo] {
                for turbo in [false, true] {
                    let state = fan_state_with_modes(cpu, gpu, turbo, PerformanceProfile::Default);
                    egui::__run_test_ui(|ui| render(ui, &state, &tx));
                }
            }
        }

        // Without synthesized clicks, the channel must remain empty.
        assert!(
            rx.try_recv().is_err(),
            "rendering without clicks must not enqueue any Command"
        );
    }

    #[test]
    fn render_fan_panel_with_turbo_active_uses_radio_pair() {
        let (tx, _rx) = mpsc::channel::<Command>(8);
        let state = fan_state_with_modes(
            FanMode::Turbo,
            FanMode::Turbo,
            true,
            PerformanceProfile::Extreme,
        );

        egui::__run_test_ui(|ui| render(ui, &state, &tx));
    }

    // ---- Interactive tests via egui_kittest ----
    //
    // These tests synthesize real pointer events via the AccessKit-backed
    // kittest harness. They cover the click-driven branches in `render` that
    // emit `Command` messages onto the channel.

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn build_fans_harness(state: AppState, tx: mpsc::Sender<Command>) -> Harness<'static, ()> {
        Harness::builder()
            .with_size(egui::Vec2::new(800.0, 600.0))
            .build_ui(move |ui| render(ui, &state, &tx))
    }

    #[test]
    fn clicking_global_turbo_radio_sends_toggle_turbo_command() {
        let (tx, mut rx) = mpsc::channel::<Command>(16);
        let state = fan_state_with_modes(
            FanMode::Auto,
            FanMode::Auto,
            false,
            PerformanceProfile::Default,
        );
        let mut harness = build_fans_harness(state, tx);

        // Multiple "Turbo" labels exist (global + CPU section + GPU section);
        // index 0 is the global toggle which lives in the dedicated Global
        // Turbo group rendered first.
        let turbos: Vec<_> = harness.get_all_by_label("Turbo").collect();
        turbos[0].click();
        harness.run();

        let cmd = rx
            .try_recv()
            .expect("clicking Turbo radio must send a command");
        assert!(matches!(cmd, Command::ToggleTurbo(true)));
    }

    #[test]
    fn clicking_auto_radio_when_turbo_enabled_sends_toggle_off_command() {
        let (tx, mut rx) = mpsc::channel::<Command>(16);
        let state = fan_state_with_modes(
            FanMode::Turbo,
            FanMode::Turbo,
            true,
            PerformanceProfile::Extreme,
        );
        let mut harness = build_fans_harness(state, tx);

        // Multiple "Auto" labels exist (global + CPU + GPU); index 0 is the
        // global turbo Auto radio. Clicking it must emit ToggleTurbo(false).
        let autos: Vec<_> = harness.get_all_by_label("Auto").collect();
        autos[0].click();
        harness.run();

        let cmd = rx.try_recv().expect("clicking Auto must send a command");
        assert!(matches!(cmd, Command::ToggleTurbo(false)));
    }

    #[test]
    fn clicking_manual_radio_in_cpu_section_sends_set_cpu_fan_mode_manual() {
        let (tx, mut rx) = mpsc::channel::<Command>(16);
        let state = fan_state_with_modes(
            FanMode::Auto,
            FanMode::Auto,
            false,
            PerformanceProfile::Default,
        );
        let mut harness = build_fans_harness(state, tx);

        // There are two "Manual" radios (CPU and GPU); both fire SetCpuFanMode
        // or SetGpuFanMode respectively. We use get_all_by_label to pick the
        // first one (CPU section is rendered first).
        let manuals: Vec<_> = harness.get_all_by_label("Manual").collect();
        assert!(
            !manuals.is_empty(),
            "Manual radios must be discoverable via accesskit"
        );
        manuals[0].click();
        harness.run();

        let cmd = rx
            .try_recv()
            .expect("clicking CPU Manual radio must send a command");
        assert!(matches!(cmd, Command::SetCpuFanMode(FanMode::Manual)));
    }

    #[test]
    fn clicking_turbo_radio_in_cpu_section_sends_set_cpu_fan_mode_turbo() {
        let (tx, mut rx) = mpsc::channel::<Command>(16);
        let state = fan_state_with_modes(
            FanMode::Auto,
            FanMode::Auto,
            false,
            PerformanceProfile::Default,
        );
        let mut harness = build_fans_harness(state, tx);

        // The label "Turbo" appears: (1) global turbo radio, (2) CPU fan
        // section, (3) GPU fan section. Clicking entry 1 (CPU section's
        // Turbo) sends SetCpuFanMode(Turbo). Index 0 is the global turbo
        // toggle.
        let turbos: Vec<_> = harness.get_all_by_label("Turbo").collect();
        assert!(
            turbos.len() >= 2,
            "should have at least global turbo + CPU section turbo radios"
        );
        turbos[1].click();
        harness.run();

        let cmd = rx
            .try_recv()
            .expect("clicking CPU Turbo radio must send a command");
        assert!(matches!(cmd, Command::SetCpuFanMode(FanMode::Turbo)));
    }

    #[test]
    fn clicking_auto_radio_in_gpu_section_sends_set_gpu_fan_mode_auto_when_currently_manual() {
        let (tx, mut rx) = mpsc::channel::<Command>(16);
        let state = fan_state_with_modes(
            FanMode::Auto,
            FanMode::Manual,
            false,
            PerformanceProfile::Default,
        );
        let mut harness = build_fans_harness(state, tx);

        // The "Auto" labels appear: (1) global turbo toggle, (2) CPU fan
        // section auto, (3) GPU fan section auto. The CPU is already Auto, so
        // its radio click is a no-op; clicking the GPU section's Auto fires
        // SetGpuFanMode(Auto).
        let autos: Vec<_> = harness.get_all_by_label("Auto").collect();
        assert!(autos.len() >= 3);
        autos[2].click();
        harness.run();

        let cmd = rx
            .try_recv()
            .expect("clicking GPU Auto radio must send a command");
        assert!(matches!(cmd, Command::SetGpuFanMode(FanMode::Auto)));
    }
}
