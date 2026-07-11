// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use egui::Ui;

use crate::config::model::RgbConfig;
use crate::hardware::rgb;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardActions {
    pub apply_clicked: bool,
    pub save_clicked: bool,
    pub load_clicked: bool,
}

/// Render keyboard RGB control panel.
///
/// `rgb_available` is cached in `AppState` at startup to avoid per-frame
/// filesystem probes.
pub fn render(ui: &mut Ui, editor: &mut RgbConfig, rgb_available: bool) -> KeyboardActions {
    let mut actions = KeyboardActions::default();

    ui.heading("Keyboard RGB");
    ui.separator();

    if !rgb_available {
        ui.label(rgb::unavailable_reason());
        return actions;
    }

    // Mode dropdown
    ui.horizontal(|ui| {
        ui.label("Mode:");
        egui::ComboBox::from_id_salt("rgb_mode")
            .selected_text(rgb_mode_name(editor.mode))
            .show_ui(ui, |ui| {
                for mode in 0..=5u8 {
                    if ui
                        .selectable_value(&mut editor.mode, mode, rgb_mode_name(mode))
                        .clicked()
                    {
                        // Mode changed
                    }
                }
            });
    });

    ui.add_space(8.0);

    // Zone dropdown
    ui.horizontal(|ui| {
        ui.label("Zone:");
        egui::ComboBox::from_id_salt("rgb_zone")
            .selected_text(rgb_zone_name(editor.zone))
            .show_ui(ui, |ui| {
                for zone in 0..=4u8 {
                    if ui
                        .selectable_value(&mut editor.zone, zone, rgb_zone_name(zone))
                        .clicked()
                    {
                        // Zone changed
                    }
                }
            });
    });

    ui.add_space(8.0);

    // Speed spinner (0-9)
    ui.horizontal(|ui| {
        ui.label("Speed:");
        ui.add(egui::DragValue::new(&mut editor.speed).range(0..=9));
    });

    ui.add_space(8.0);

    // Brightness spinner (0-100)
    ui.horizontal(|ui| {
        ui.label("Brightness:");
        ui.add(egui::DragValue::new(&mut editor.brightness).range(0..=100));
    });

    ui.add_space(8.0);

    // Direction dropdown
    ui.horizontal(|ui| {
        ui.label("Direction:");
        egui::ComboBox::from_id_salt("rgb_direction")
            .selected_text(direction_name(editor.direction))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut editor.direction, 1u8, "Right");
                ui.selectable_value(&mut editor.direction, 2u8, "Left");
            });
    });

    ui.add_space(8.0);

    // Color picker (only for static mode)
    if editor.mode == 0 {
        ui.horizontal(|ui| {
            ui.label("Color:");
            let mut rgb = [
                editor.red as f32 / 255.0,
                editor.green as f32 / 255.0,
                editor.blue as f32 / 255.0,
            ];
            if egui::color_picker::color_edit_button_rgb(ui, &mut rgb).changed() {
                editor.red = (rgb[0] * 255.0) as u8;
                editor.green = (rgb[1] * 255.0) as u8;
                editor.blue = (rgb[2] * 255.0) as u8;
            }
        });
        ui.add_space(8.0);
    }

    ui.separator();

    // Action buttons
    ui.horizontal(|ui| {
        if ui.button("Apply").clicked() {
            actions.apply_clicked = true;
        }
        if ui.button("Save Config").clicked() {
            actions.save_clicked = true;
        }
        if ui.button("Load Config").clicked() {
            actions.load_clicked = true;
        }
    });

    actions
}

fn rgb_mode_name(mode: u8) -> &'static str {
    match mode {
        0 => "Static",
        1 => "Breathing",
        2 => "Neon",
        3 => "Wave",
        4 => "Shifting",
        5 => "Zoom",
        _ => "Unknown",
    }
}

fn rgb_zone_name(zone: u8) -> &'static str {
    match zone {
        0 => "All",
        1 => "Zone 1",
        2 => "Zone 2",
        3 => "Zone 3",
        4 => "Zone 4",
        _ => "Unknown",
    }
}

fn direction_name(direction: u8) -> &'static str {
    if direction == 1 { "Right" } else { "Left" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rgb_mode_names_all_valid() {
        let expected = [
            (0, "Static"),
            (1, "Breathing"),
            (2, "Neon"),
            (3, "Wave"),
            (4, "Shifting"),
            (5, "Zoom"),
        ];
        for (mode, name) in expected {
            assert_eq!(rgb_mode_name(mode), name, "mode {mode}");
        }
    }

    #[test]
    fn test_rgb_mode_names_out_of_range() {
        for mode in [6, 7, 128, 255] {
            assert_eq!(rgb_mode_name(mode), "Unknown", "mode {mode}");
        }
    }

    #[test]
    fn test_rgb_zone_names_all_valid() {
        let expected = [
            (0, "All"),
            (1, "Zone 1"),
            (2, "Zone 2"),
            (3, "Zone 3"),
            (4, "Zone 4"),
        ];
        for (zone, name) in expected {
            assert_eq!(rgb_zone_name(zone), name, "zone {zone}");
        }
    }

    #[test]
    fn test_rgb_zone_names_out_of_range() {
        for zone in [5, 6, 128, 255] {
            assert_eq!(rgb_zone_name(zone), "Unknown", "zone {zone}");
        }
    }

    #[test]
    fn test_keyboard_actions_default_is_all_false() {
        let actions = KeyboardActions::default();
        assert!(!actions.apply_clicked);
        assert!(!actions.save_clicked);
        assert!(!actions.load_clicked);
    }

    #[test]
    fn test_direction_name_right() {
        assert_eq!(direction_name(1), "Right");
    }

    #[test]
    fn test_direction_name_left_for_any_non_one() {
        for dir in [0u8, 2, 3, 255] {
            assert_eq!(direction_name(dir), "Left", "direction {dir}");
        }
    }

    // ---- Render exercises ----

    /// Run an egui frame inside a CentralPanel with the given (FnMut) builder.
    /// This is the test-only equivalent of `egui::__run_test_ui` that supports
    /// closures borrowing mutable test fixtures.
    fn run_test_ui(mut add_contents: impl FnMut(&mut egui::Ui)) {
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::empty());
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| add_contents(ui));
        });
    }

    #[test]
    fn render_keyboard_panel_unavailable_short_circuits_with_default_actions() {
        let mut editor = RgbConfig::default();
        let mut captured = KeyboardActions::default();
        run_test_ui(|ui| {
            captured = render(ui, &mut editor, false);
        });

        assert_eq!(
            captured,
            KeyboardActions::default(),
            "unavailable RGB device must not register any action clicks"
        );
        assert!(
            !rgb::unavailable_reason().is_empty(),
            "unavailable panel should have a setup hint"
        );
    }

    #[test]
    fn render_keyboard_panel_dynamic_mode_renders_without_color_picker() {
        let mut editor = RgbConfig {
            mode: 3, // dynamic Wave
            ..RgbConfig::default()
        };
        run_test_ui(|ui| {
            let _ = render(ui, &mut editor, true);
        });
    }

    #[test]
    fn render_keyboard_panel_static_mode_includes_color_picker_branch() {
        let mut editor = RgbConfig {
            mode: 0,
            zone: 0,
            ..RgbConfig::default()
        };
        run_test_ui(|ui| {
            let _ = render(ui, &mut editor, true);
        });
    }

    #[test]
    fn render_keyboard_panel_renders_each_zone_and_direction_combo() {
        for zone in 0..=4u8 {
            for direction in [1u8, 2] {
                let mut editor = RgbConfig {
                    mode: 0,
                    zone,
                    direction,
                    ..RgbConfig::default()
                };
                run_test_ui(|ui| {
                    let _ = render(ui, &mut editor, true);
                });
            }
        }
    }

    #[test]
    fn render_keyboard_panel_renders_each_dynamic_mode_label() {
        for mode in 1..=5u8 {
            let mut editor = RgbConfig {
                mode,
                ..RgbConfig::default()
            };
            run_test_ui(|ui| {
                let _ = render(ui, &mut editor, true);
            });
        }
    }

    #[test]
    fn render_keyboard_actions_are_default_until_buttons_clicked() {
        // Without synthesizing button clicks, action flags must remain false.
        let mut editor = RgbConfig::default();
        let mut actions = KeyboardActions::default();
        run_test_ui(|ui| {
            actions = render(ui, &mut editor, true);
        });
        assert_eq!(actions, KeyboardActions::default());
    }

    // ---- Interactive tests via egui_kittest ----

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    /// Holds the editor state and the OR-merged button-click actions across
    /// every frame the harness drives. Egui only reports `clicked()` on the
    /// frame the click is processed; subsequent frames see false. We
    /// accumulate so the test can assert the click was observed regardless
    /// of how many frames the harness ends up running.
    #[derive(Default)]
    struct KeyboardHarnessState {
        editor: RgbConfig,
        latched: KeyboardActions,
    }

    fn build_keyboard_harness<'a>(
        rgb_available: bool,
        editor: RgbConfig,
    ) -> Harness<'a, KeyboardHarnessState> {
        Harness::builder()
            .with_size(egui::Vec2::new(800.0, 600.0))
            .build_ui_state(
                move |ui, state: &mut KeyboardHarnessState| {
                    let frame = render(ui, &mut state.editor, rgb_available);
                    state.latched.apply_clicked |= frame.apply_clicked;
                    state.latched.save_clicked |= frame.save_clicked;
                    state.latched.load_clicked |= frame.load_clicked;
                },
                KeyboardHarnessState {
                    editor,
                    latched: KeyboardActions::default(),
                },
            )
    }

    #[test]
    fn clicking_apply_button_sets_apply_clicked_action() {
        let mut harness = build_keyboard_harness(true, RgbConfig::default());
        harness.get_by_label("Apply").click();
        harness.run();
        assert!(harness.state().latched.apply_clicked);
    }

    #[test]
    fn clicking_save_config_button_sets_save_clicked_action() {
        let mut harness = build_keyboard_harness(true, RgbConfig::default());
        harness.get_by_label("Save Config").click();
        harness.run();
        assert!(harness.state().latched.save_clicked);
    }

    #[test]
    fn clicking_load_config_button_sets_load_clicked_action() {
        let mut harness = build_keyboard_harness(true, RgbConfig::default());
        harness.get_by_label("Load Config").click();
        harness.run();
        assert!(harness.state().latched.load_clicked);
    }

    #[test]
    fn opening_each_combo_box_renders_its_dropdown_options() {
        // Click each ComboBox by `accesskit::Role::ComboBox` to expand its
        // dropdown. While open, egui calls each `selectable_value` closure
        // for the items inside `show_ui`, which is the only way to cover
        // those branches.
        use accesskit::Role;
        let mut harness = build_keyboard_harness(true, RgbConfig::default());

        // The keyboard panel has three ComboBoxes (Mode, Zone, Direction)
        // in that order. Open each, run a frame so the dropdown items
        // render, and verify it doesn't panic. We re-query each iteration
        // because the AccessKit tree may grow new nodes when the dropdown
        // is open.
        let combo_count = harness.query_all_by_role(Role::ComboBox).count();
        assert_eq!(
            combo_count, 3,
            "keyboard tab should expose three ComboBoxes"
        );

        for index in 0..3 {
            let combos: Vec<_> = harness.get_all_by_role(Role::ComboBox).collect();
            combos[index].click();
            harness.run();
        }
    }
}
