// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use egui::Ui;
use tokio::sync::mpsc::Sender;

use crate::app::events::Command;
use crate::app::state::PerformanceProfile;

/// Profile labels exposed for testing
pub const PROFILE_LABELS: [(PerformanceProfile, &str); 3] = [
    (PerformanceProfile::Quiet, "Quiet"),
    (PerformanceProfile::Default, "Default"),
    (PerformanceProfile::Extreme, "Extreme"),
];

pub fn render(ui: &mut Ui, current: PerformanceProfile, command_tx: &Sender<Command>) {
    ui.group(|ui| {
        ui.label("Performance Profile:");
        ui.horizontal(|ui| {
            for (profile, label) in PROFILE_LABELS {
                if ui.radio(current == profile, label).clicked() {
                    let _ = command_tx.try_send(Command::SetProfile(profile));
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_labels_cover_all_variants() {
        assert_eq!(PROFILE_LABELS.len(), 3);
        assert_eq!(PROFILE_LABELS[0], (PerformanceProfile::Quiet, "Quiet"));
        assert_eq!(PROFILE_LABELS[1], (PerformanceProfile::Default, "Default"));
        assert_eq!(PROFILE_LABELS[2], (PerformanceProfile::Extreme, "Extreme"));
    }

    #[test]
    fn profile_labels_are_unique() {
        let profiles: Vec<_> = PROFILE_LABELS.iter().map(|(p, _)| p).collect();
        let labels: Vec<_> = PROFILE_LABELS.iter().map(|(_, l)| l).collect();
        for i in 0..profiles.len() {
            for j in (i + 1)..profiles.len() {
                assert_ne!(profiles[i], profiles[j]);
                assert_ne!(labels[i], labels[j]);
            }
        }
    }

    // ---- Render exercises ----

    #[test]
    fn render_profile_panel_displays_each_profile_and_emits_no_command_for_unchanged_state() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Command>(8);

        for profile in [
            PerformanceProfile::Quiet,
            PerformanceProfile::Default,
            PerformanceProfile::Extreme,
        ] {
            egui::__run_test_ui(|ui| render(ui, profile, &tx));
        }

        // No clicks were synthesized, so the channel must remain empty.
        assert!(
            rx.try_recv().is_err(),
            "rendering without input must not enqueue any Command"
        );
    }

    // ---- Interactive tests via egui_kittest ----

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use tokio::sync::mpsc;

    #[test]
    fn clicking_each_profile_radio_emits_set_profile_command() {
        for (target, label) in [
            (PerformanceProfile::Quiet, "Quiet"),
            (PerformanceProfile::Default, "Default"),
            (PerformanceProfile::Extreme, "Extreme"),
        ] {
            let (tx, mut rx) = mpsc::channel::<Command>(8);
            // Render with a different profile selected so clicking `target`
            // is a real state change.
            let current = if target == PerformanceProfile::Quiet {
                PerformanceProfile::Default
            } else {
                PerformanceProfile::Quiet
            };
            let mut harness: Harness<()> = Harness::builder()
                .with_size(egui::Vec2::new(800.0, 200.0))
                .build_ui(move |ui| render(ui, current, &tx));

            harness.get_by_label(label).click();
            harness.run();

            let cmd = rx.try_recv().expect("clicking must enqueue a command");
            match cmd {
                Command::SetProfile(p) if p == target => {}
                other => {
                    panic!("clicking `{label}` should send SetProfile({target:?}), got {other:?}")
                }
            }
        }
    }
}
