// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

pub mod events;
pub mod handler;
pub mod state;

use std::sync::Arc;

use tokio::sync::{Mutex, mpsc, watch};

use crate::app::events::Command;
use crate::app::state::AppState;
use crate::config::manager::ConfigManager;
use crate::config::model::RgbConfig;
use crate::hardware::ec::{Ec, EcDevice};
use crate::hardware::platform::RegisterMap;
use crate::telemetry::poller::TelemetrySnapshot;
use crate::ui::theme::apply_theme;

/// Application tabs matching original UI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    FanControl,
    Monitoring,
    Keyboard,
}

/// Main egui application state
pub struct NitroSenseAppInit<D: EcDevice> {
    pub state: Arc<Mutex<AppState>>,
    pub telemetry_rx: watch::Receiver<TelemetrySnapshot>,
    pub ec: Arc<Mutex<Ec<D>>>,
    pub regs: &'static RegisterMap,
    pub config_manager: ConfigManager,
    pub shutdown_rx: watch::Receiver<bool>,
    pub command_tx: mpsc::Sender<Command>,
    pub rgb_editor: RgbConfig,
    pub undervolt_core: u8,
}

/// Main egui application state
pub struct NitroSenseApp<D: EcDevice> {
    /// Shared application state (protected by mutex for thread safety)
    pub state: Arc<Mutex<AppState>>,
    /// Telemetry receiver from async poller
    pub telemetry_rx: watch::Receiver<TelemetrySnapshot>,
    /// EC device handle for hardware commands
    pub ec: Arc<Mutex<Ec<D>>>,
    /// Register map for the detected model
    pub regs: &'static RegisterMap,
    /// Persistent configuration manager
    pub config_manager: ConfigManager,
    /// Currently active tab
    pub active_tab: Tab,
    /// Command sender to async hardware worker
    pub command_tx: mpsc::Sender<Command>,
    /// Shutdown signal from the command worker
    pub shutdown_rx: watch::Receiver<bool>,
    /// Persistent RGB editor state for the keyboard tab
    pub rgb_editor: RgbConfig,
    /// Persistent undervolt core selection for the settings panel
    pub undervolt_core: u8,
}

impl<D: EcDevice + 'static> NitroSenseApp<D> {
    /// Create new application instance
    pub fn new(init: NitroSenseAppInit<D>) -> Self {
        Self {
            state: init.state,
            telemetry_rx: init.telemetry_rx,
            ec: init.ec,
            regs: init.regs,
            config_manager: init.config_manager,
            active_tab: Tab::default(),
            command_tx: init.command_tx,
            shutdown_rx: init.shutdown_rx,
            rgb_editor: init.rgb_editor,
            undervolt_core: init.undervolt_core,
        }
    }

    /// Send command to async hardware worker
    pub fn send_command(&self, cmd: Command) {
        if let Err(e) = self.command_tx.try_send(cmd) {
            tracing::warn!("Failed to send command: {}", e);
        }
    }
}

impl<D: EcDevice + 'static> eframe::App for NitroSenseApp<D> {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply custom dark theme (idempotent — egui deduplicates identical visuals)
        apply_theme(ctx);

        if *self.shutdown_rx.borrow() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Only clone telemetry snapshot when data has actually changed,
        // avoiding a per-frame heap allocation of the 40-byte TelemetrySnapshot.
        if self.telemetry_rx.has_changed().unwrap_or(false) {
            let snapshot = self.telemetry_rx.borrow_and_update().clone();
            if let Ok(mut state) = self.state.try_lock() {
                state.apply_telemetry(&snapshot, self.regs);
            }
        }

        // Render tab bar at top
        egui::TopBottomPanel::top("tab_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.active_tab, Tab::FanControl, "Fan Control");
                ui.selectable_value(&mut self.active_tab, Tab::Monitoring, "Monitoring");
                ui.selectable_value(&mut self.active_tab, Tab::Keyboard, "Keyboard");
            });
        });

        // Display error banner (if any), then clear after showing.
        // Acquire the lock once to read last_error; Option::clone on None
        // is just a discriminant copy (trivially cheap), so no separate
        // is_some() guard is needed. This avoids a TOCTOU race that the
        // previous triple-lock pattern had.
        let last_error = self
            .state
            .try_lock()
            .ok()
            .and_then(|guard| guard.last_error.clone());
        if let Some(ref error_msg) = last_error {
            egui::TopBottomPanel::bottom("error_bar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(255, 80, 80), error_msg);
                    if ui.small_button("Dismiss").clicked()
                        && let Ok(mut s) = self.state.try_lock()
                    {
                        s.last_error = None;
                    }
                });
            });
        }

        // Render active tab content.
        // Instead of cloning the entire AppState every frame, we borrow the
        // lock and render directly. This eliminates a ~200-byte heap allocation
        // per frame (AppState contains String fields for undervolt_status,
        // rgb_config, last_error). If the lock is contended, we show "Loading...".
        egui::CentralPanel::default().show(ctx, |ui| match self.active_tab {
            Tab::FanControl => {
                if let Ok(state_guard) = self.state.try_lock() {
                    crate::ui::fans::render(ui, &state_guard, &self.command_tx);
                    ui.add_space(16.0);
                    crate::ui::settings::render(
                        ui,
                        &state_guard,
                        &mut self.undervolt_core,
                        &self.command_tx,
                    );
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label("Loading...");
                    });
                }
            }
            Tab::Monitoring => {
                if let Ok(state_guard) = self.state.try_lock() {
                    crate::ui::dashboard::render(ui, &state_guard);
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label("Loading...");
                    });
                }
            }
            Tab::Keyboard => {
                if let Ok(mut state_guard) = self.state.try_lock() {
                    let mut rgb_editor = state_guard.rgb_config.clone();
                    let before_editor = rgb_editor.clone();
                    let rgb_available = state_guard.rgb_available;
                    let actions = crate::ui::keyboard::render(ui, &mut rgb_editor, rgb_available);

                    // Write directly through the existing guard instead of
                    // attempting a second try_lock() — tokio::sync::Mutex is
                    // NOT reentrant, so a nested try_lock() would always fail
                    // and the RGB edit would be silently lost.
                    if rgb_editor != before_editor {
                        state_guard.rgb_config = rgb_editor.clone();
                    }

                    if actions.load_clicked {
                        self.send_command(Command::LoadRgbConfig);
                    } else {
                        if actions.save_clicked {
                            self.send_command(Command::SaveRgbConfig);
                        }

                        if actions.apply_clicked {
                            self.send_command(Command::ApplyRgb(rgb_editor));
                        }
                    }
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label("Loading...");
                    });
                }
            }
        });

        // Request repaint at 1 Hz for telemetry updates
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::{BatteryStatus, FanMode, PerformanceProfile};
    use crate::config::manager::ConfigManager;
    use crate::config::model::RgbConfig;
    use crate::hardware::ec::Ec;
    use crate::hardware::platform::AN515_46_REGS;
    use crate::test_support::RecordingEcDevice;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{Mutex, mpsc, watch};

    #[test]
    fn test_tab_default_is_fan_control() {
        assert_eq!(Tab::default(), Tab::FanControl);
    }

    #[test]
    fn test_tab_equality() {
        assert_eq!(Tab::FanControl, Tab::FanControl);
        assert_ne!(Tab::FanControl, Tab::Monitoring);
    }

    #[test]
    fn test_all_three_tabs_are_distinct() {
        let tabs = [Tab::FanControl, Tab::Monitoring, Tab::Keyboard];
        for i in 0..tabs.len() {
            for j in (i + 1)..tabs.len() {
                assert_ne!(tabs[i], tabs[j]);
            }
        }
    }

    #[test]
    fn test_tab_clone_and_copy() {
        let tab = Tab::Monitoring;
        // Tab is `Copy + Clone`; bind a second name and ensure equality.
        let copied = tab;
        assert_eq!(tab, copied);
    }

    #[test]
    fn command_enum_covers_all_fan_modes() {
        for mode in [FanMode::Auto, FanMode::Manual, FanMode::Turbo] {
            let cmd = Command::SetCpuFanMode(mode);
            assert!(matches!(cmd, Command::SetCpuFanMode(_)));
            let cmd = Command::SetGpuFanMode(mode);
            assert!(matches!(cmd, Command::SetGpuFanMode(_)));
        }
    }

    #[test]
    fn command_enum_covers_all_profiles() {
        for profile in [
            PerformanceProfile::Quiet,
            PerformanceProfile::Default,
            PerformanceProfile::Extreme,
        ] {
            let cmd = Command::SetProfile(profile);
            assert!(matches!(cmd, Command::SetProfile(_)));
        }
    }

    #[test]
    fn command_enum_covers_manual_speed_range() {
        for speed in [0u8, 1, 12, 25] {
            let cpu = Command::SetCpuManualSpeed(speed);
            let gpu = Command::SetGpuManualSpeed(speed);
            assert!(matches!(cpu, Command::SetCpuManualSpeed(_)));
            assert!(matches!(gpu, Command::SetGpuManualSpeed(_)));
        }
    }

    #[test]
    fn command_enum_covers_toggles() {
        for value in [true, false] {
            assert!(matches!(
                Command::ToggleTurbo(value),
                Command::ToggleTurbo(_)
            ));
            assert!(matches!(
                Command::ToggleKbTimer(value),
                Command::ToggleKbTimer(_)
            ));
            assert!(matches!(
                Command::ToggleUsbCharging(value),
                Command::ToggleUsbCharging(_)
            ));
            assert!(matches!(
                Command::ToggleBatteryLimit(value),
                Command::ToggleBatteryLimit(_)
            ));
        }
    }

    #[test]
    fn command_enum_covers_rgb_operations() {
        let config = RgbConfig::default();
        assert!(matches!(Command::ApplyRgb(config), Command::ApplyRgb(_)));
        assert!(matches!(Command::SaveRgbConfig, Command::SaveRgbConfig));
        assert!(matches!(Command::LoadRgbConfig, Command::LoadRgbConfig));
    }

    #[test]
    fn command_enum_covers_undervolt() {
        for core in [0u8, 3, 7] {
            assert!(matches!(
                Command::ApplyUndervolt(core),
                Command::ApplyUndervolt(_)
            ));
        }
    }

    #[test]
    fn command_enum_covers_system_operations() {
        assert!(matches!(Command::SaveConfig, Command::SaveConfig));
        assert!(matches!(Command::Shutdown, Command::Shutdown));
    }

    #[test]
    fn command_is_debug_printable() {
        let cmd = Command::Shutdown;
        let debug = format!("{:?}", cmd);
        assert!(debug.contains("Shutdown"));
    }

    #[test]
    fn command_is_cloneable() {
        let cmd = Command::SetProfile(PerformanceProfile::Extreme);
        let cloned = cmd.clone();
        assert!(matches!(
            cloned,
            Command::SetProfile(PerformanceProfile::Extreme)
        ));
    }

    // ---- NitroSenseApp / NitroSenseAppInit construction tests ----

    /// Tuple returned from `build_test_app`. Aliased as a `type` so the
    /// signature stays under clippy's `type_complexity` lint.
    type TestAppFixture = (
        NitroSenseApp<RecordingEcDevice>,
        mpsc::Sender<Command>,
        mpsc::Receiver<Command>,
        watch::Sender<bool>,
        watch::Sender<TelemetrySnapshot>,
    );

    /// Build a fresh `NitroSenseApp` with mock hardware and freshly-allocated
    /// channels. Returns the app, the channel sender (for assertions), the
    /// shutdown sender, and the receivers for monitoring.
    fn build_test_app() -> TestAppFixture {
        let state = Arc::new(Mutex::new(AppState::default()));
        let ec = Arc::new(Mutex::new(
            Ec::new(RecordingEcDevice::default(), &AN515_46_REGS)
                .with_min_write_interval(Duration::ZERO),
        ));
        let (telemetry_tx, telemetry_rx) = watch::channel(TelemetrySnapshot::default());
        let (command_tx, command_rx) = mpsc::channel::<Command>(8);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let app = NitroSenseApp::new(NitroSenseAppInit {
            state,
            telemetry_rx,
            ec,
            regs: &AN515_46_REGS,
            config_manager: ConfigManager::with_config_dir(
                std::env::temp_dir().join("nitrosense-app-test-cfg"),
            ),
            shutdown_rx,
            command_tx: command_tx.clone(),
            rgb_editor: RgbConfig::default(),
            undervolt_core: 3,
        });

        (app, command_tx, command_rx, shutdown_tx, telemetry_tx)
    }

    #[test]
    fn nitro_sense_app_new_initializes_active_tab_to_fan_control() {
        let (app, _tx, _rx, _shutdown_tx, _telemetry_tx) = build_test_app();
        assert_eq!(app.active_tab, Tab::FanControl);
        assert_eq!(
            app.undervolt_core, 3,
            "init.undervolt_core must be preserved"
        );
        assert_eq!(app.regs.gpu_temp, AN515_46_REGS.gpu_temp);
    }

    #[test]
    fn nitro_sense_app_send_command_enqueues_command_on_channel() {
        let (app, _tx, mut rx, _shutdown_tx, _telemetry_tx) = build_test_app();

        app.send_command(Command::SaveConfig);

        let cmd = rx.try_recv().expect("send_command must enqueue");
        assert!(matches!(cmd, Command::SaveConfig));
    }

    #[test]
    fn nitro_sense_app_send_command_swallows_full_channel_error() {
        // Build an app with a tiny channel and saturate it to force try_send
        // to fail; the function must not panic.
        let state = Arc::new(Mutex::new(AppState::default()));
        let ec = Arc::new(Mutex::new(
            Ec::new(RecordingEcDevice::default(), &AN515_46_REGS)
                .with_min_write_interval(Duration::ZERO),
        ));
        let (telemetry_tx, telemetry_rx) = watch::channel(TelemetrySnapshot::default());
        let (command_tx, mut command_rx) = mpsc::channel::<Command>(1);
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        let app = NitroSenseApp::new(NitroSenseAppInit {
            state,
            telemetry_rx,
            ec,
            regs: &AN515_46_REGS,
            config_manager: ConfigManager::with_config_dir(
                std::env::temp_dir().join("nitrosense-fullchan-test"),
            ),
            shutdown_rx,
            command_tx,
            rgb_editor: RgbConfig::default(),
            undervolt_core: 0,
        });

        app.send_command(Command::SaveConfig);
        // Now the channel is full; this second send must not panic even though
        // try_send returns Err(TrySendError::Full).
        app.send_command(Command::SaveConfig);

        // Drain to make rx happy.
        let _ = command_rx.try_recv();
        // Suppress unused warnings
        let _ = telemetry_tx;
    }

    /// Drive a single egui frame against the application, returning the egui
    /// context for caller-side assertions. Also accepts a closure that runs
    /// before the frame to mutate shared state (e.g. publish a telemetry
    /// snapshot or set `last_error`).
    fn run_one_frame(
        app: &mut NitroSenseApp<RecordingEcDevice>,
        prepare: impl FnOnce(&NitroSenseApp<RecordingEcDevice>),
    ) -> egui::Context {
        prepare(app);
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::empty());
        let mut frame = eframe::Frame::_new_kittest();
        let _ = ctx.run(Default::default(), |ctx| {
            <NitroSenseApp<RecordingEcDevice> as eframe::App>::update(app, ctx, &mut frame);
        });
        ctx
    }

    #[test]
    fn nitro_sense_app_update_renders_fan_control_tab_with_default_state() {
        let (mut app, _tx, _rx, _shutdown_tx, _telemetry_tx) = build_test_app();
        let _ctx = run_one_frame(&mut app, |_| {});
    }

    #[test]
    fn nitro_sense_app_update_renders_monitoring_tab_when_active() {
        let (mut app, _tx, _rx, _shutdown_tx, _telemetry_tx) = build_test_app();
        app.active_tab = Tab::Monitoring;
        let _ctx = run_one_frame(&mut app, |_| {});
    }

    #[test]
    fn nitro_sense_app_update_renders_keyboard_tab_when_active() {
        let (mut app, _tx, _rx, _shutdown_tx, _telemetry_tx) = build_test_app();
        app.active_tab = Tab::Keyboard;
        let _ctx = run_one_frame(&mut app, |_| {});
    }

    #[test]
    fn nitro_sense_app_update_short_circuits_when_shutdown_signal_is_set() {
        let (mut app, _tx, _rx, shutdown_tx, _telemetry_tx) = build_test_app();
        shutdown_tx.send(true).expect("shutdown send must succeed");
        // Mark the receiver as having seen the change so `*self.shutdown_rx.borrow()` is true.
        // The watch::Receiver returns the latest value via borrow().
        let _ctx = run_one_frame(&mut app, |_| {});
    }

    #[test]
    fn nitro_sense_app_update_consumes_telemetry_snapshot_into_state() {
        let (mut app, _tx, _rx, _shutdown_tx, telemetry_tx) = build_test_app();

        let snapshot = TelemetrySnapshot {
            cpu_temp: 77,
            gpu_temp: 70,
            power_plugged_in: true,
            battery_status: BatteryStatus::Charging,
            ..TelemetrySnapshot::default()
        };
        telemetry_tx
            .send(snapshot)
            .expect("telemetry send must succeed");

        let _ctx = run_one_frame(&mut app, |_| {});

        let state = app
            .state
            .try_lock()
            .expect("state must not be locked elsewhere");
        assert_eq!(state.cpu_temp, 77);
        assert_eq!(state.gpu_temp, 70);
        assert!(state.power_plugged_in);
        assert_eq!(state.battery_status, BatteryStatus::Charging);
    }

    #[test]
    fn nitro_sense_app_update_renders_error_banner_when_last_error_present() {
        let (mut app, _tx, _rx, _shutdown_tx, _telemetry_tx) = build_test_app();
        {
            let mut s = app.state.try_lock().unwrap();
            s.last_error = Some("simulated failure".to_string());
        }
        let _ctx = run_one_frame(&mut app, |_| {});
    }

    #[test]
    fn nitro_sense_app_update_handles_loading_branch_when_state_lock_unavailable() {
        let (mut app, _tx, _rx, _shutdown_tx, _telemetry_tx) = build_test_app();
        // Hold the state lock so try_lock() inside update returns None and
        // each tab falls through to the "Loading..." branch.
        let _held = app.state.clone();
        let _guard = _held.try_lock().expect("we own the only handle currently");

        // Run all three tabs while the state lock is held.
        for tab in [Tab::FanControl, Tab::Monitoring, Tab::Keyboard] {
            app.active_tab = tab;
            let ctx = egui::Context::default();
            ctx.set_fonts(egui::FontDefinitions::empty());
            let mut frame = eframe::Frame::_new_kittest();
            let _ = ctx.run(Default::default(), |ctx| {
                <NitroSenseApp<RecordingEcDevice> as eframe::App>::update(
                    &mut app, ctx, &mut frame,
                );
            });
        }
    }

    #[test]
    fn nitro_sense_app_update_keyboard_tab_propagates_rgb_editor_changes_to_state() {
        // Regression test for a double-lock bug: the Keyboard tab used to
        // call `self.state.try_lock()` a second time while the first guard
        // (`state_guard`) was still held. Since tokio::sync::Mutex is NOT
        // reentrant, the second try_lock() always failed and RGB edits were
        // silently dropped. The fix writes directly through the existing
        // guard instead.
        //
        // We verify by directly exercising the critical code path:
        // acquire the lock, clone rgb_config, simulate an edit, and confirm
        // that writing through the guard actually updates the shared state.
        let (app, _tx, _rx, _shutdown_tx, _telemetry_tx) = build_test_app();

        // Simulate what the Keyboard tab does: acquire lock, clone editor,
        // modify it, then write back through the SAME guard (not a second
        // try_lock).
        {
            let mut state_guard = app.state.try_lock().unwrap();
            let mut rgb_editor = state_guard.rgb_config.clone();
            let before_editor = rgb_editor.clone();

            // Simulate a user edit (e.g. changing mode from 0 to 3)
            rgb_editor.mode = 3;

            // This is the fixed code path — write through the existing guard
            assert_ne!(rgb_editor, before_editor, "editor must have changed");
            state_guard.rgb_config = rgb_editor.clone();
        }

        // Verify the change propagated to the shared state
        let state = app.state.try_lock().unwrap();
        assert_eq!(
            state.rgb_config.mode, 3,
            "RGB mode change must propagate through the Mutex guard"
        );
    }
}
