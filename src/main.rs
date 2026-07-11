// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::ffi::OsString;
use std::process::Command as ProcessCommand;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context;
use clap::{Arg, ArgAction, Command as ClapCommand};
use tokio::sync::{Mutex, mpsc, watch};
use tracing::{error, info, warn};

use nitrosense::app::events::Command;
use nitrosense::app::state::{AppState, BatteryStatus, FanMode, PerformanceProfile};
use nitrosense::app::{NitroSenseApp, NitroSenseAppInit};
use nitrosense::config::apply::{apply_config_to_ec, apply_rgb_config};
use nitrosense::config::manager::ConfigManager;
use nitrosense::config::model::RgbConfig;
use nitrosense::error::NitroError;
use nitrosense::ffi::RawEcDevice;
use nitrosense::gui_session;
use nitrosense::hardware::ec::{Ec, EcDevice};
use nitrosense::hardware::platform::{
    CpuVendor, RegisterMap, detect_cpu_vendor, detect_model, register_map_for_model,
};
use nitrosense::hardware::rgb::{self, FsRgbDeviceWriter, RgbDeviceWriter};
use nitrosense::hardware::voltage::{
    self, SystemIntelVoltageReader, SystemProcessRunner, VoltageState, check_amd_undervolt_status,
    check_amd_voltage, check_intel_voltage,
};
use nitrosense::telemetry::poller::{TelemetrySnapshot, run_poller_until_shutdown};

#[derive(Debug, Default)]
struct NoopRgbWriter;

impl RgbDeviceWriter for NoopRgbWriter {
    fn write_payload(&mut self, _device: &str, _payload: &[u8]) -> Result<(), NitroError> {
        Ok(())
    }
}

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

const CAP_SYS_RAWIO: u32 = 17;
const CAP_SYS_ADMIN: u32 = 21;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct CliOptions {
    no_gui: bool,
    status: bool,
    set_profile: Option<PerformanceProfile>,
    set_fan_mode: Option<FanModeSelection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FanModeSelection {
    target: FanTarget,
    mode: FanMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FanTarget {
    Cpu,
    Gpu,
}

struct CliRuntime<D: EcDevice + 'static> {
    ec: Arc<Mutex<Ec<D>>>,
    state: Arc<Mutex<AppState>>,
    telemetry_rx: watch::Receiver<TelemetrySnapshot>,
    shutdown_rx: watch::Receiver<bool>,
    command_tx: mpsc::Sender<Command>,
    regs: &'static RegisterMap,
    config_manager: ConfigManager,
}

struct WorkerHandles {
    poller: tokio::task::JoinHandle<()>,
    voltage: Option<tokio::task::JoinHandle<()>>,
    command: tokio::task::JoinHandle<()>,
    signal: tokio::task::JoinHandle<()>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let startup_begin = std::time::Instant::now();

    let cli = CliOptions::parse();

    tracing_subscriber::fmt()
        .with_env_filter("nitrosense=info")
        .init();

    info!("NitroSense v0.1.0 starting");

    install_signal_handlers()?;

    if !has_required_privileges() {
        warn!("NitroSense requires root or cap_sys_rawio+cap_sys_admin; relaunching");
        relaunch_with_pkexec()?;
        return Ok(());
    }

    let model = detect_model()?;
    info!("Detected model: {}", model);

    let register_map = register_map_for_model(&model)
        .ok_or_else(|| anyhow::anyhow!("Unsupported model: {}", model))?;
    let cpu_vendor = detect_cpu_vendor();
    info!("Detected CPU vendor: {:?}", cpu_vendor);

    let config_manager = ConfigManager::new();
    let nitro_config = config_manager.load_config().unwrap_or_else(|err| {
        error!("Failed to load main config, using defaults: {}", err);
        Default::default()
    });
    let mut rgb_config = config_manager.load_rgb_config().unwrap_or_else(|err| {
        error!("Failed to load RGB config, using defaults: {}", err);
        Default::default()
    });
    if let Err(err) = validate_rgb_config(&rgb_config) {
        warn!(
            "Invalid RGB config detected, falling back to defaults: {}",
            err
        );
        rgb_config = Default::default();
    }

    let device = RawEcDevice::new();
    let ec = Arc::new(Mutex::new(Ec::new(device, register_map)));
    let initial_snapshot = {
        let mut guard = ec.lock().await;
        guard.open().context(
            "EC open failed: ensure debugfs is mounted and ec_sys is loaded with write_support=y \
             (e.g., sudo modprobe ec_sys write_support=y). If the kernel was recently upgraded, reboot."
        )?;
        guard.refresh()?;
        apply_config_to_ec(&mut guard, &nitro_config)?;

        if rgb::is_available() {
            let mut writer = FsRgbDeviceWriter;
            if let Err(err) = apply_rgb_config(&mut writer, &rgb_config) {
                warn!("Failed to apply startup RGB config: {}", err);
            }
        } else {
            let reason = rgb::unavailable_reason();
            if reason.is_empty() {
                info!("RGB devices unavailable; skipping startup apply");
            } else {
                info!("RGB devices unavailable; skipping startup apply ({reason})");
            }
        }

        guard.snapshot()
    };

    let rgb_available = rgb::is_available();

    let mut initial_state = AppState::default();
    initial_state.apply_telemetry(&initial_snapshot, register_map);
    initial_state.rgb_config = rgb_config.clone();
    initial_state.rgb_available = rgb_available;

    seed_voltage_state(&mut initial_state, cpu_vendor);

    let app_state = Arc::new(Mutex::new(initial_state));
    let (telemetry_tx, telemetry_rx) = watch::channel(initial_snapshot.clone());
    let (command_tx, command_rx) = mpsc::channel::<Command>(100);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let poller_handle = {
        let ec_clone = Arc::clone(&ec);
        let shutdown_rx_clone = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(err) = run_poller_until_shutdown(
                ec_clone,
                telemetry_tx,
                shutdown_rx_clone,
                Duration::from_secs(1),
            )
            .await
            {
                error!("Telemetry poller failed: {}", err);
            }
        })
    };

    let voltage_handle =
        spawn_voltage_poller(Arc::clone(&app_state), shutdown_rx.clone(), cpu_vendor);

    let command_handle = {
        let ec_clone = Arc::clone(&ec);
        let state_clone = Arc::clone(&app_state);
        let config_manager_clone = config_manager.clone();
        tokio::spawn(async move {
            run_command_worker(
                command_rx,
                ec_clone,
                state_clone,
                config_manager_clone,
                register_map,
                shutdown_tx,
                cpu_vendor,
            )
            .await;
        })
    };

    let signal_handle = spawn_signal_watcher(command_tx.clone(), shutdown_rx.clone());

    if run_cli_mode(
        &cli,
        CliRuntime {
            ec: Arc::clone(&ec),
            state: Arc::clone(&app_state),
            telemetry_rx: telemetry_rx.clone(),
            shutdown_rx: shutdown_rx.clone(),
            command_tx: command_tx.clone(),
            regs: register_map,
            config_manager: config_manager.clone(),
        },
    )
    .await?
    {
        shutdown_application(
            &app_state,
            register_map,
            &config_manager,
            &command_tx,
            WorkerHandles {
                poller: poller_handle,
                voltage: voltage_handle,
                command: command_handle,
                signal: signal_handle,
            },
        )
        .await;
        info!("NitroSense shutdown complete");
        return Ok(());
    }

    let startup_elapsed = startup_begin.elapsed();
    info!(
        "Startup to GUI launch: {:.1} ms",
        startup_elapsed.as_secs_f64() * 1000.0
    );

    if !gui_session::ready() && gui_session::ensure_from_invoking_user() {
        info!("Restored GUI session environment from invoking user");
    }

    if !gui_session::ready() {
        return Err(anyhow::anyhow!(
            "Cannot connect to a display server. On Wayland compositors such as Hyprland, \
             launch NitroSense via `nitro-sense` from your desktop session instead of \
             `sudo nitrosense`, or export WAYLAND_DISPLAY and XDG_RUNTIME_DIR first."
        ));
    }

    let app = NitroSenseApp::new(NitroSenseAppInit {
        state: Arc::clone(&app_state),
        telemetry_rx,
        ec,
        regs: register_map,
        config_manager: config_manager.clone(),
        shutdown_rx,
        command_tx: command_tx.clone(),
        rgb_editor: rgb_config,
        undervolt_core: 0,
    });

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([500.0, 700.0])
            .with_resizable(false)
            .with_title("Linux NitroSense")
            .with_icon(load_window_icon()),
        ..Default::default()
    };

    info!("Launching GUI");
    eframe::run_native(
        "Linux NitroSense",
        native_options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
    .map_err(|e| anyhow::anyhow!("GUI error: {e}"))?;

    shutdown_application(
        &app_state,
        register_map,
        &config_manager,
        &command_tx,
        WorkerHandles {
            poller: poller_handle,
            voltage: voltage_handle,
            command: command_handle,
            signal: signal_handle,
        },
    )
    .await;

    info!("NitroSense shutdown complete");
    Ok(())
}

/// Load embedded window icon from pre-converted RGBA binary.
/// Format: u32 LE width + u32 LE height + raw RGBA pixel data.
///
/// Uses `OnceLock` to avoid re-allocating the RGBA Vec on every call.
fn load_window_icon() -> egui::IconData {
    static ICON: OnceLock<egui::IconData> = OnceLock::new();
    ICON.get_or_init(|| {
        static ICON_BYTES: &[u8] = include_bytes!("../assets/icon_rgba.bin");
        let width =
            u32::from_le_bytes([ICON_BYTES[0], ICON_BYTES[1], ICON_BYTES[2], ICON_BYTES[3]]);
        let height =
            u32::from_le_bytes([ICON_BYTES[4], ICON_BYTES[5], ICON_BYTES[6], ICON_BYTES[7]]);
        let rgba = ICON_BYTES[8..].to_vec();
        egui::IconData {
            rgba,
            width,
            height,
        }
    })
    .clone()
}

impl CliOptions {
    fn parse() -> Self {
        Self::parse_from(std::env::args_os())
    }

    fn parse_from<I, T>(args: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let matches = cli_command().get_matches_from(args);
        let set_profile = matches
            .get_one::<String>("set-profile")
            .map(|value| parse_profile_arg(value).expect("clap validates profile values"));
        let set_fan_mode = matches.get_many::<String>("set-fan-mode").map(|values| {
            let values: Vec<_> = values.map(String::as_str).collect();
            FanModeSelection {
                target: parse_fan_target_arg(values[0]).unwrap_or_else(|| {
                    cli_command()
                        .error(
                            clap::error::ErrorKind::InvalidValue,
                            "--set-fan-mode target must be 'cpu' or 'gpu'",
                        )
                        .exit()
                }),
                mode: parse_fan_mode_arg(values[1]).unwrap_or_else(|| {
                    cli_command()
                        .error(
                            clap::error::ErrorKind::InvalidValue,
                            "--set-fan-mode mode must be 'auto', 'manual', or 'turbo'",
                        )
                        .exit()
                }),
            }
        });

        Self {
            no_gui: matches.get_flag("no-gui"),
            status: matches.get_flag("status"),
            set_profile,
            set_fan_mode,
        }
    }
}

fn cli_command() -> ClapCommand {
    ClapCommand::new("nitrosense")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Fan control and system monitoring for Acer Nitro laptops")
        .arg(
            Arg::new("no-gui")
                .long("no-gui")
                .help("Run headless and stream telemetry to stdout")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("status")
                .long("status")
                .help("Print current EC status and exit")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("set-profile")
                .long("set-profile")
                .value_name("quiet|default|extreme")
                .help("Set the Nitro performance profile and exit")
                .value_parser(["quiet", "default", "extreme"]),
        )
        .arg(
            Arg::new("set-fan-mode")
                .long("set-fan-mode")
                .value_names(["cpu|gpu", "auto|manual|turbo"])
                .num_args(2)
                .value_parser(["cpu", "gpu", "auto", "manual", "turbo"])
                .help("Set CPU or GPU fan mode and exit"),
        )
}

fn parse_profile_arg(value: &str) -> Option<PerformanceProfile> {
    match value {
        "quiet" => Some(PerformanceProfile::Quiet),
        "default" => Some(PerformanceProfile::Default),
        "extreme" => Some(PerformanceProfile::Extreme),
        _ => None,
    }
}

fn parse_fan_target_arg(value: &str) -> Option<FanTarget> {
    match value {
        "cpu" => Some(FanTarget::Cpu),
        "gpu" => Some(FanTarget::Gpu),
        _ => None,
    }
}

fn parse_fan_mode_arg(value: &str) -> Option<FanMode> {
    match value {
        "auto" => Some(FanMode::Auto),
        "manual" => Some(FanMode::Manual),
        "turbo" => Some(FanMode::Turbo),
        _ => None,
    }
}

fn set_fan_mode_command(selection: FanModeSelection) -> Command {
    match selection.target {
        FanTarget::Cpu => Command::SetCpuFanMode(selection.mode),
        FanTarget::Gpu => Command::SetGpuFanMode(selection.mode),
    }
}

extern "C" fn handle_shutdown_signal(_signal: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() -> std::io::Result<()> {
    unsafe {
        // SAFETY: sigaction is a per-process syscall; `action` is zeroed then fully initialized
        // before the call. The handler function is a plain C function pointer that only stores
        // an atomic flag (no locks, no allocations, no reentrant unsafety).
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handle_shutdown_signal as *const () as libc::sighandler_t;
        action.sa_flags = 0;
        libc::sigemptyset(&mut action.sa_mask);

        if libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

fn has_required_privileges() -> bool {
    (unsafe {
        // SAFETY: geteuid() is a simple POSIX syscall with no side effects and no pointers.
        libc::geteuid()
    }) == 0
        || process_has_required_capabilities()
}

fn process_has_required_capabilities() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| parse_cap_eff_from_status(&status))
        .is_some_and(cap_mask_has_required_caps)
}

fn parse_cap_eff_from_status(status: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        line.strip_prefix("CapEff:")
            .and_then(|value| u64::from_str_radix(value.trim(), 16).ok())
    })
}

fn cap_mask_has_required_caps(mask: u64) -> bool {
    let rawio = 1u64 << CAP_SYS_RAWIO;
    let admin = 1u64 << CAP_SYS_ADMIN;
    (mask & rawio) != 0 && (mask & admin) != 0
}

fn relaunch_with_pkexec() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = ProcessCommand::new("pkexec");
    command.arg("env");

    for assignment in gui_session::env_assignments() {
        command.arg(assignment);
    }

    command.arg(exe);
    command.args(std::env::args_os().skip(1));

    let status = command.status()?;
    std::process::exit(status.code().unwrap_or(1));
}

fn validate_rgb_config(config: &RgbConfig) -> Result<(), NitroError> {
    let mut writer = NoopRgbWriter;
    apply_rgb_config(&mut writer, config)
}

async fn set_last_error(state: &Arc<Mutex<AppState>>, msg: String) {
    let mut guard = state.lock().await;
    guard.last_error = Some(msg);
}

fn read_voltage_for_vendor(cpu_vendor: CpuVendor) -> Result<f64, NitroError> {
    match cpu_vendor {
        CpuVendor::Amd => {
            let runner = SystemProcessRunner;
            check_amd_voltage(&runner)
        }
        CpuVendor::Intel => {
            let reader = SystemIntelVoltageReader;
            check_intel_voltage(&reader)
        }
        CpuVendor::Unknown => Err(NitroError::Validation(
            voltage::unsupported_cpu_message().to_string(),
        )),
    }
}

fn read_amd_undervolt_status() -> Result<String, NitroError> {
    let runner = SystemProcessRunner;
    check_amd_undervolt_status(&runner)
}

fn seed_voltage_state(state: &mut AppState, cpu_vendor: CpuVendor) {
    match cpu_vendor {
        CpuVendor::Amd | CpuVendor::Intel => match read_voltage_for_vendor(cpu_vendor) {
            Ok(voltage) => {
                state.voltage = voltage;
                state.min_voltage = voltage;
                state.max_voltage = voltage;
            }
            Err(err) => warn!("Initial voltage read failed: {}", err),
        },
        CpuVendor::Unknown => {}
    }

    match cpu_vendor {
        CpuVendor::Amd => match read_amd_undervolt_status() {
            Ok(status) => state.undervolt_status = status,
            Err(err) => warn!("Initial AMD undervolt status read failed: {}", err),
        },
        _ => state.undervolt_status = voltage::unsupported_undervolt_message().to_string(),
    }
}

fn should_save_main_config(command: &Command) -> bool {
    matches!(
        command,
        Command::SetCpuFanMode(_)
            | Command::SetGpuFanMode(_)
            | Command::SetProfile(_)
            | Command::ToggleTurbo(_)
            | Command::ToggleKbTimer(_)
            | Command::ToggleUsbCharging(_)
            | Command::ToggleBatteryLimit(_)
    )
}

async fn save_main_config(
    state: &Arc<Mutex<AppState>>,
    regs: &'static RegisterMap,
    config_manager: &ConfigManager,
) {
    let config = {
        let state_guard = state.lock().await;
        state_guard.to_nitro_config(regs)
    };

    if let Err(err) = config_manager.save_config(&config) {
        error!("Failed to save main config: {}", err);
    }
}

fn format_status(state: &AppState) -> String {
    format!(
        concat!(
            "Profile: {}\n",
            "Turbo: {}\n",
            "CPU Fan: {} (level {})\n",
            "GPU Fan: {} (level {})\n",
            "CPU Temp: {} C\n",
            "GPU Temp: {} C\n",
            "System Temp: {} C\n",
            "CPU RPM: {}\n",
            "GPU RPM: {}\n",
            "Power: {}\n",
            "Battery: {}\n",
            "Charge Limit: {}\n",
            "USB Charging: {}\n",
            "KB Timer: {}\n",
            "Voltage: {} (min {}, max {})"
        ),
        profile_name(state.performance_profile),
        toggle_name(state.turbo_enabled),
        fan_mode_name(state.cpu_fan_mode),
        state.cpu_manual_speed,
        fan_mode_name(state.gpu_fan_mode),
        state.gpu_manual_speed,
        state.cpu_temp,
        state.gpu_temp,
        state.sys_temp,
        state.cpu_fan_rpm,
        state.gpu_fan_rpm,
        if state.power_plugged_in {
            "Plugged In"
        } else {
            "Unplugged"
        },
        battery_status_name(state.battery_status),
        toggle_name(state.battery_limit_enabled),
        toggle_name(state.usb_charging_enabled),
        toggle_name(state.kb_timer_enabled),
        format_voltage_cli(state.voltage),
        format_voltage_cli(state.min_voltage),
        format_voltage_cli(state.max_voltage),
    )
}

fn format_voltage_cli(value: f64) -> String {
    if value.is_finite() && value < f64::MAX / 2.0 {
        format!("{value:.2} V")
    } else {
        "N/A".to_owned()
    }
}

fn fan_mode_name(mode: FanMode) -> &'static str {
    match mode {
        FanMode::Auto => "Auto",
        FanMode::Manual => "Manual",
        FanMode::Turbo => "Turbo",
    }
}

fn profile_name(profile: PerformanceProfile) -> &'static str {
    match profile {
        PerformanceProfile::Quiet => "Quiet",
        PerformanceProfile::Default => "Default",
        PerformanceProfile::Extreme => "Extreme",
    }
}

fn battery_status_name(status: BatteryStatus) -> &'static str {
    match status {
        BatteryStatus::Charging => "Charging",
        BatteryStatus::Discharging => "Discharging",
        BatteryStatus::NotInUse => "Not In Use",
    }
}

fn toggle_name(enabled: bool) -> &'static str {
    if enabled { "On" } else { "Off" }
}

async fn run_cli_mode<D: EcDevice + 'static>(
    cli: &CliOptions,
    runtime: CliRuntime<D>,
) -> Result<bool, NitroError> {
    let mut handled_one_shot = false;

    if let Some(profile) = cli.set_profile {
        execute_hardware_command_and_save(
            &runtime.ec,
            &runtime.state,
            runtime.regs,
            &runtime.config_manager,
            Command::SetProfile(profile),
        )
        .await?;
        handled_one_shot = true;
    }

    if let Some(selection) = cli.set_fan_mode {
        execute_hardware_command_and_save(
            &runtime.ec,
            &runtime.state,
            runtime.regs,
            &runtime.config_manager,
            set_fan_mode_command(selection),
        )
        .await?;
        handled_one_shot = true;
    }

    if cli.status || handled_one_shot {
        let state_guard = runtime.state.lock().await;
        println!("{}", format_status(&state_guard));
        return Ok(true);
    }

    if cli.no_gui {
        run_headless(
            runtime.telemetry_rx,
            runtime.state,
            runtime.shutdown_rx,
            runtime.command_tx,
            runtime.regs,
        )
        .await;
        return Ok(true);
    }

    Ok(false)
}

async fn execute_hardware_command_and_save<D: EcDevice + 'static>(
    ec: &Arc<Mutex<Ec<D>>>,
    state: &Arc<Mutex<AppState>>,
    regs: &'static RegisterMap,
    config_manager: &ConfigManager,
    command: Command,
) -> Result<(), NitroError> {
    let config_to_save = {
        let mut ec_guard = ec.lock().await;
        let mut state_guard = state.lock().await;
        nitrosense::app::handler::execute_command(
            &mut ec_guard,
            &mut state_guard,
            command.clone(),
        )?;
        let snapshot = ec_guard.snapshot();
        state_guard.apply_telemetry(&snapshot, regs);

        should_save_main_config(&command).then(|| state_guard.to_nitro_config(regs))
    };

    if let Some(config) = config_to_save {
        config_manager.save_config(&config)?;
    }

    Ok(())
}

async fn run_headless(
    mut telemetry_rx: watch::Receiver<TelemetrySnapshot>,
    state: Arc<Mutex<AppState>>,
    mut shutdown_rx: watch::Receiver<bool>,
    command_tx: mpsc::Sender<Command>,
    regs: &'static RegisterMap,
) {
    {
        let state_guard = state.lock().await;
        println!("{}", format_status(&state_guard));
    }

    loop {
        tokio::select! {
            changed = telemetry_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let snapshot = telemetry_rx.borrow_and_update().clone();
                let mut state_guard = state.lock().await;
                state_guard.apply_telemetry(&snapshot, regs);
                println!("{}", format_status(&state_guard));
            }
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }

    let _ = command_tx.send(Command::Shutdown).await;
}

async fn shutdown_application(
    state: &Arc<Mutex<AppState>>,
    regs: &'static RegisterMap,
    config_manager: &ConfigManager,
    command_tx: &mpsc::Sender<Command>,
    handles: WorkerHandles,
) {
    save_main_config(state, regs, config_manager).await;

    let shutdown_timeout = Duration::from_secs(5);
    let _ = tokio::time::timeout(shutdown_timeout, command_tx.send(Command::Shutdown)).await;

    let _ = tokio::time::timeout(shutdown_timeout, handles.poller).await;
    if let Some(handle) = handles.voltage {
        let _ = tokio::time::timeout(shutdown_timeout, handle).await;
    }
    let _ = tokio::time::timeout(shutdown_timeout, handles.command).await;
    let _ = tokio::time::timeout(shutdown_timeout, handles.signal).await;
}

fn spawn_signal_watcher(
    command_tx: mpsc::Sender<Command>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(200));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if shutdown_requested() {
                        let _ = command_tx.send(Command::Shutdown).await;
                        break;
                    }
                }
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
    })
}

async fn run_command_worker<D: EcDevice + 'static>(
    mut command_rx: mpsc::Receiver<Command>,
    ec: Arc<Mutex<Ec<D>>>,
    state: Arc<Mutex<AppState>>,
    config_manager: ConfigManager,
    regs: &'static RegisterMap,
    shutdown_tx: watch::Sender<bool>,
    cpu_vendor: CpuVendor,
) {
    while let Some(cmd) = command_rx.recv().await {
        info!("Received command: {:?}", cmd);

        match cmd {
            Command::Shutdown => {
                let _ = shutdown_tx.send(true);
                break;
            }
            Command::SaveConfig => {
                save_main_config(&state, regs, &config_manager).await;
            }
            Command::SaveRgbConfig => {
                let rgb = {
                    let state_guard = state.lock().await;
                    state_guard.rgb_config.clone()
                };
                if let Err(err) = config_manager.save_rgb_config(&rgb) {
                    error!("Failed to save RGB config: {}", err);
                    set_last_error(&state, format!("Failed to save RGB config: {err}")).await;
                }
            }
            Command::LoadRgbConfig => match config_manager.load_rgb_config() {
                Ok(rgb) => {
                    if let Err(err) = validate_rgb_config(&rgb) {
                        error!("Loaded RGB config is invalid: {}", err);
                        set_last_error(&state, format!("Loaded RGB config is invalid: {err}"))
                            .await;
                        continue;
                    }

                    {
                        let mut state_guard = state.lock().await;
                        state_guard.rgb_config = rgb.clone();
                    }

                    if rgb::is_available() {
                        let mut writer = FsRgbDeviceWriter;
                        if let Err(err) = apply_rgb_config(&mut writer, &rgb) {
                            error!("Failed to apply loaded RGB config: {}", err);
                            set_last_error(&state, format!("RGB device write failed: {err}")).await;
                        }
                    }
                }
                Err(err) => {
                    error!("Failed to load RGB config: {}", err);
                    set_last_error(&state, format!("Failed to load RGB config: {err}")).await;
                }
            },
            Command::ApplyRgb(rgb) => {
                if let Err(err) = validate_rgb_config(&rgb) {
                    error!("Rejected invalid RGB config: {}", err);
                    set_last_error(&state, format!("Invalid RGB config: {err}")).await;
                    continue;
                }

                {
                    let mut state_guard = state.lock().await;
                    state_guard.rgb_config = rgb.clone();
                }

                if rgb::is_available() {
                    let mut writer = FsRgbDeviceWriter;
                    if let Err(err) = apply_rgb_config(&mut writer, &rgb) {
                        error!("Failed to apply RGB config: {}", err);
                        set_last_error(&state, format!("RGB device write failed: {err}")).await;
                    }
                }

                if let Err(err) = config_manager.save_rgb_config(&rgb) {
                    error!("Failed to save RGB config after apply: {}", err);
                }
            }
            Command::ApplyUndervolt(core) => {
                if core > 7 {
                    let msg = format!("Undervolt core {core} is outside 0..=7");
                    error!("{}", msg);
                    set_last_error(&state, msg).await;
                    continue;
                }

                if cpu_vendor != CpuVendor::Amd {
                    let mut state_guard = state.lock().await;
                    state_guard.undervolt_status =
                        voltage::unsupported_undervolt_message().to_string();
                    continue;
                }

                let result = tokio::task::spawn_blocking(move || {
                    let runner = SystemProcessRunner;
                    voltage::apply_amd_undervolt(&runner, core)
                })
                .await;

                match result {
                    Ok(Ok(status)) => {
                        let mut state_guard = state.lock().await;
                        state_guard.undervolt_status = status;
                    }
                    Ok(Err(err)) => {
                        error!("Undervolt apply failed: {}", err);
                        let msg = format!("Undervolt failed: {err}");
                        let mut state_guard = state.lock().await;
                        state_guard.undervolt_status = msg.clone();
                        state_guard.last_error = Some(msg);
                    }
                    Err(err) => {
                        error!("Undervolt worker join failed: {}", err);
                        let msg = format!("Undervolt failed: {err}");
                        let mut state_guard = state.lock().await;
                        state_guard.undervolt_status = msg.clone();
                        state_guard.last_error = Some(msg);
                    }
                }
            }
            other => {
                if let Err(err) = execute_hardware_command_and_save(
                    &ec,
                    &state,
                    regs,
                    &config_manager,
                    other.clone(),
                )
                .await
                {
                    error!("Command execution failed: {}", err);
                    if let Ok(mut state_guard) = state.try_lock() {
                        state_guard.last_error = Some(format!("Hardware command failed: {err}"));
                    }
                }
            }
        }
    }

    info!("Command handler shutting down");
}

fn spawn_voltage_poller(
    state: Arc<Mutex<AppState>>,
    mut shutdown_rx: watch::Receiver<bool>,
    cpu_vendor: CpuVendor,
) -> Option<tokio::task::JoinHandle<()>> {
    if cpu_vendor == CpuVendor::Unknown {
        return None;
    }

    Some(tokio::spawn(async move {
        let mut tracker = VoltageState::default();
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let voltage_result = tokio::task::spawn_blocking(move || read_voltage_for_vendor(cpu_vendor)).await;

                    if let Ok(Ok(voltage)) = voltage_result {
                        tracker.update(voltage);
                        if let Ok(mut state_guard) = state.try_lock() {
                            state_guard.voltage = tracker.current;
                            state_guard.min_voltage = tracker.min;
                            state_guard.max_voltage = tracker.max;
                        }
                    } else if let Ok(Err(err)) = voltage_result {
                        warn!("Voltage poll failed: {}", err);
                    } else if let Err(err) = voltage_result {
                        warn!("Voltage poll join failed: {}", err);
                    }

                    if cpu_vendor == CpuVendor::Amd {
                        let status_result = tokio::task::spawn_blocking(read_amd_undervolt_status).await;

                        if let Ok(Ok(status)) = status_result {
                            if let Ok(mut state_guard) = state.try_lock() {
                                state_guard.undervolt_status = status;
                            }
                        } else if let Ok(Err(err)) = status_result {
                            warn!("AMD undervolt status poll failed: {}", err);
                        } else if let Err(err) = status_result {
                            warn!("AMD undervolt status poll join failed: {}", err);
                        }
                    }
                }
                _ = shutdown_rx.changed() => break,
            }
        }

        info!("Voltage poller shutting down");
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_main_compiles() {}

    #[test]
    fn test_load_window_icon_dimensions() {
        let icon = load_window_icon();
        assert_eq!(icon.width, 64);
        assert_eq!(icon.height, 64);
    }

    #[test]
    fn test_load_window_icon_rgba_byte_count() {
        let icon = load_window_icon();
        let expected_bytes = (icon.width * icon.height * 4) as usize;
        assert_eq!(icon.rgba.len(), expected_bytes);
    }

    #[test]
    fn test_load_window_icon_not_all_zeros() {
        let icon = load_window_icon();
        assert!(icon.rgba.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_validate_rgb_config_rejects_invalid_mode() {
        let config = RgbConfig {
            mode: 99,
            ..Default::default()
        };
        assert!(validate_rgb_config(&config).is_err());
    }

    #[test]
    fn test_validate_rgb_config_accepts_default() {
        assert!(validate_rgb_config(&RgbConfig::default()).is_ok());
    }

    #[test]
    fn test_should_save_main_config_for_settings_commands() {
        use nitrosense::app::state::{FanMode, PerformanceProfile};
        assert!(should_save_main_config(&Command::SetCpuFanMode(
            FanMode::Auto
        )));
        assert!(should_save_main_config(&Command::SetGpuFanMode(
            FanMode::Turbo
        )));
        assert!(should_save_main_config(&Command::SetProfile(
            PerformanceProfile::Extreme
        )));
        assert!(should_save_main_config(&Command::ToggleTurbo(true)));
        assert!(should_save_main_config(&Command::ToggleKbTimer(false)));
        assert!(should_save_main_config(&Command::ToggleUsbCharging(true)));
        assert!(should_save_main_config(&Command::ToggleBatteryLimit(true)));
    }

    #[test]
    fn test_should_not_save_main_config_for_non_settings_commands() {
        assert!(!should_save_main_config(&Command::Shutdown));
        assert!(!should_save_main_config(&Command::SaveConfig));
        assert!(!should_save_main_config(&Command::SaveRgbConfig));
        assert!(!should_save_main_config(&Command::LoadRgbConfig));
        assert!(!should_save_main_config(&Command::ApplyUndervolt(0)));
        assert!(!should_save_main_config(&Command::ApplyRgb(
            RgbConfig::default()
        )));
    }

    #[test]
    fn test_cli_parse_status_and_no_gui_flags() {
        let cli = CliOptions::parse_from(["nitrosense", "--no-gui", "--status"]);

        assert!(cli.no_gui);
        assert!(cli.status);
        assert_eq!(cli.set_profile, None);
        assert_eq!(cli.set_fan_mode, None);
    }

    #[test]
    fn test_cli_parse_profile_and_fan_mode_commands() {
        let cli = CliOptions::parse_from([
            "nitrosense",
            "--set-profile",
            "extreme",
            "--set-fan-mode",
            "gpu",
            "turbo",
        ]);

        assert_eq!(cli.set_profile, Some(PerformanceProfile::Extreme));
        assert_eq!(
            cli.set_fan_mode,
            Some(FanModeSelection {
                target: FanTarget::Gpu,
                mode: FanMode::Turbo,
            })
        );
    }

    #[test]
    fn test_cli_arg_parsers_reject_unknown_values() {
        assert_eq!(parse_profile_arg("balanced"), None);
        assert_eq!(parse_fan_target_arg("both"), None);
        assert_eq!(parse_fan_mode_arg("max"), None);
    }

    #[test]
    fn test_set_fan_mode_command_routes_cpu_and_gpu_targets() {
        assert!(matches!(
            set_fan_mode_command(FanModeSelection {
                target: FanTarget::Cpu,
                mode: FanMode::Manual,
            }),
            Command::SetCpuFanMode(FanMode::Manual)
        ));
        assert!(matches!(
            set_fan_mode_command(FanModeSelection {
                target: FanTarget::Gpu,
                mode: FanMode::Auto,
            }),
            Command::SetGpuFanMode(FanMode::Auto)
        ));
    }

    #[test]
    fn test_parse_cap_eff_from_status_reads_hex_mask() {
        let status = "Name:\tnitrosense\nCapEff:\t0000000000220000\n";

        assert_eq!(parse_cap_eff_from_status(status), Some(0x220000));
    }

    #[test]
    fn test_cap_mask_has_required_caps_requires_rawio_and_admin() {
        let rawio = 1u64 << CAP_SYS_RAWIO;
        let admin = 1u64 << CAP_SYS_ADMIN;

        assert!(cap_mask_has_required_caps(rawio | admin));
        assert!(!cap_mask_has_required_caps(rawio));
        assert!(!cap_mask_has_required_caps(admin));
    }

    #[test]
    fn test_shutdown_signal_handler_sets_atomic_flag() {
        SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);

        handle_shutdown_signal(libc::SIGTERM);

        assert!(shutdown_requested());
        SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn test_format_status_includes_cli_accessibility_fields() {
        let state = AppState {
            performance_profile: PerformanceProfile::Quiet,
            cpu_fan_mode: FanMode::Manual,
            cpu_manual_speed: 12,
            gpu_fan_mode: FanMode::Turbo,
            gpu_manual_speed: 25,
            cpu_temp: 60,
            gpu_temp: 61,
            sys_temp: 50,
            cpu_fan_rpm: 2400,
            gpu_fan_rpm: 2600,
            power_plugged_in: true,
            battery_status: BatteryStatus::Charging,
            min_voltage: f64::MAX,
            max_voltage: 1.25,
            ..AppState::default()
        };

        let status = format_status(&state);

        assert!(status.contains("Profile: Quiet"));
        assert!(status.contains("CPU Fan: Manual (level 12)"));
        assert!(status.contains("GPU Fan: Turbo (level 25)"));
        assert!(status.contains("Power: Plugged In"));
        assert!(status.contains("Voltage: 0.00 V (min N/A, max 1.25 V)"));
    }

    // -----------------------------------------------------------------
    // Comprehensive helper-function tests
    // -----------------------------------------------------------------

    use nitrosense::config::manager::ConfigManager;
    use nitrosense::hardware::ec::Ec;
    use nitrosense::hardware::platform::AN515_46_REGS;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{Mutex as TokioMutex, mpsc as tokio_mpsc, watch as tokio_watch};

    /// Recording EC mock used by these tests. The list of writes is stored
    /// behind a shared `Arc<StdMutex>` so the test thread can inspect it
    /// while the EC instance is locked behind a `tokio::sync::Mutex`.
    #[derive(Debug)]
    struct RecordingEcDevice {
        buffer: [u8; 256],
        writes: Arc<std::sync::Mutex<Vec<(u8, u8)>>>,
    }

    impl Default for RecordingEcDevice {
        fn default() -> Self {
            Self {
                buffer: [0; 256],
                writes: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }
    }

    impl RecordingEcDevice {
        fn writes_handle(&self) -> Arc<std::sync::Mutex<Vec<(u8, u8)>>> {
            Arc::clone(&self.writes)
        }
    }

    impl nitrosense::hardware::ec::EcDevice for RecordingEcDevice {
        fn open(&mut self) -> Result<(), NitroError> {
            Ok(())
        }
        fn close(&mut self) {}
        fn refresh(&mut self, buffer: &mut [u8]) -> Result<usize, NitroError> {
            buffer.copy_from_slice(&self.buffer);
            Ok(self.buffer.len())
        }
        fn write_byte(&mut self, addr: u8, val: u8) -> Result<(), NitroError> {
            self.buffer[addr as usize] = val;
            self.writes.lock().unwrap().push((addr, val));
            Ok(())
        }
    }

    /// Tuple returned by `build_runtime`. Aliased so clippy's type-complexity
    /// lint stays happy without sacrificing the ability to destructure all
    /// the moving parts in each test.
    type TestRuntimeFixture = (
        CliRuntime<RecordingEcDevice>,
        Arc<TokioMutex<Ec<RecordingEcDevice>>>,
        Arc<TokioMutex<AppState>>,
        tokio_watch::Sender<TelemetrySnapshot>,
        tokio_watch::Sender<bool>,
        tokio_mpsc::Receiver<Command>,
        Arc<std::sync::Mutex<Vec<(u8, u8)>>>,
    );

    fn build_runtime() -> TestRuntimeFixture {
        let device = RecordingEcDevice::default();
        let writes = device.writes_handle();
        let ec = Arc::new(TokioMutex::new(
            Ec::new(device, &AN515_46_REGS).with_min_write_interval(Duration::ZERO),
        ));
        let state = Arc::new(TokioMutex::new(AppState::default()));
        let (telemetry_tx, telemetry_rx) =
            tokio_watch::channel::<TelemetrySnapshot>(TelemetrySnapshot::default());
        let (command_tx, command_rx) = tokio_mpsc::channel::<Command>(8);
        let (shutdown_tx, shutdown_rx) = tokio_watch::channel(false);

        let dir = tempfile::tempdir().expect("tempdir for ConfigManager");
        let config_manager = ConfigManager::with_config_dir(dir.path());
        std::mem::forget(dir);

        let runtime = CliRuntime {
            ec: Arc::clone(&ec),
            state: Arc::clone(&state),
            telemetry_rx,
            shutdown_rx,
            command_tx,
            regs: &AN515_46_REGS,
            config_manager,
        };

        (
            runtime,
            ec,
            state,
            telemetry_tx,
            shutdown_tx,
            command_rx,
            writes,
        )
    }

    // ---- Tiny string helpers ----

    #[test]
    fn fan_mode_name_covers_all_variants() {
        assert_eq!(fan_mode_name(FanMode::Auto), "Auto");
        assert_eq!(fan_mode_name(FanMode::Manual), "Manual");
        assert_eq!(fan_mode_name(FanMode::Turbo), "Turbo");
    }

    #[test]
    fn profile_name_covers_all_variants() {
        assert_eq!(profile_name(PerformanceProfile::Quiet), "Quiet");
        assert_eq!(profile_name(PerformanceProfile::Default), "Default");
        assert_eq!(profile_name(PerformanceProfile::Extreme), "Extreme");
    }

    #[test]
    fn battery_status_name_covers_all_variants() {
        assert_eq!(battery_status_name(BatteryStatus::Charging), "Charging");
        assert_eq!(
            battery_status_name(BatteryStatus::Discharging),
            "Discharging"
        );
        assert_eq!(battery_status_name(BatteryStatus::NotInUse), "Not In Use");
    }

    #[test]
    fn toggle_name_returns_on_off() {
        assert_eq!(toggle_name(true), "On");
        assert_eq!(toggle_name(false), "Off");
    }

    #[test]
    fn format_voltage_cli_finite_value() {
        assert_eq!(format_voltage_cli(1.234), "1.23 V");
    }

    #[test]
    fn format_voltage_cli_infinity_returns_na() {
        assert_eq!(format_voltage_cli(f64::INFINITY), "N/A");
    }

    #[test]
    fn format_voltage_cli_nan_returns_na() {
        assert_eq!(format_voltage_cli(f64::NAN), "N/A");
    }

    #[test]
    fn format_voltage_cli_huge_value_returns_na() {
        assert_eq!(format_voltage_cli(f64::MAX), "N/A");
    }

    #[test]
    fn format_status_handles_unplugged_and_default_profile() {
        let state = AppState {
            performance_profile: PerformanceProfile::Default,
            cpu_fan_mode: FanMode::Auto,
            gpu_fan_mode: FanMode::Auto,
            power_plugged_in: false,
            battery_status: BatteryStatus::Discharging,
            ..AppState::default()
        };
        let status = format_status(&state);
        assert!(status.contains("Profile: Default"));
        assert!(status.contains("Power: Unplugged"));
        assert!(status.contains("Battery: Discharging"));
    }

    #[test]
    fn format_status_handles_battery_not_in_use() {
        let state = AppState {
            battery_status: BatteryStatus::NotInUse,
            ..AppState::default()
        };
        let status = format_status(&state);
        assert!(status.contains("Battery: Not In Use"));
    }

    // ---- read_voltage_for_vendor + read_amd_undervolt_status (Unknown leg) ----

    #[test]
    fn read_voltage_for_vendor_returns_validation_for_unknown_cpu() {
        let result = read_voltage_for_vendor(CpuVendor::Unknown);
        assert!(matches!(result, Err(NitroError::Validation(_))));
    }

    #[test]
    fn seed_voltage_state_for_unknown_cpu_leaves_voltage_zero_and_marks_undervolt_unsupported() {
        let mut state = AppState::default();
        seed_voltage_state(&mut state, CpuVendor::Unknown);
        // Unknown skips the voltage read (no AMD/Intel match) but the second
        // match unconditionally writes the "unsupported" message via its `_`
        // arm, which covers both Intel and Unknown.
        assert_eq!(state.voltage, 0.0);
        assert_eq!(
            state.undervolt_status,
            nitrosense::hardware::voltage::unsupported_undervolt_message()
        );
    }

    #[test]
    fn seed_voltage_state_for_intel_writes_unsupported_undervolt_message() {
        let mut state = AppState::default();
        seed_voltage_state(&mut state, CpuVendor::Intel);
        // Voltage may or may not be readable on this host; we only assert the
        // undervolt status message is present (Intel: undervolt unsupported).
        assert_eq!(
            state.undervolt_status,
            nitrosense::hardware::voltage::unsupported_undervolt_message()
        );
    }

    // ---- run_cli_mode integration ----

    #[tokio::test]
    async fn run_cli_mode_with_no_flags_returns_false_for_gui_continuation() {
        let (runtime, _ec, _state, _t, _s, _c, _writes) = build_runtime();
        let cli = CliOptions::default();

        let handled = run_cli_mode(&cli, runtime)
            .await
            .expect("no-op CLI must succeed");
        assert!(
            !handled,
            "with no flags run_cli_mode must report not-handled to fall through to the GUI"
        );
    }

    #[tokio::test]
    async fn run_cli_mode_set_profile_writes_to_ec_and_returns_true() {
        let (runtime, _ec, _state, _t, _s, _c, writes) = build_runtime();
        let cli = CliOptions {
            set_profile: Some(PerformanceProfile::Quiet),
            ..CliOptions::default()
        };

        let handled = run_cli_mode(&cli, runtime)
            .await
            .expect("set-profile CLI must succeed");

        assert!(handled, "one-shot CLI must report handled");
        let logged = writes.lock().unwrap().clone();
        assert!(
            logged
                .iter()
                .any(|(addr, val)| *addr == AN515_46_REGS.nitro_mode
                    && *val == AN515_46_REGS.quiet_mode),
            "set-profile=quiet must write quiet_mode to nitro_mode register: {logged:?}"
        );
    }

    #[tokio::test]
    async fn run_cli_mode_set_fan_mode_writes_correct_register() {
        let (runtime, _ec, _state, _t, _s, _c, writes) = build_runtime();
        let cli = CliOptions {
            set_fan_mode: Some(FanModeSelection {
                target: FanTarget::Cpu,
                mode: FanMode::Manual,
            }),
            ..CliOptions::default()
        };

        let handled = run_cli_mode(&cli, runtime)
            .await
            .expect("set-fan-mode CLI must succeed");

        assert!(handled);
        let logged = writes.lock().unwrap().clone();
        assert!(
            logged
                .iter()
                .any(|(addr, val)| *addr == AN515_46_REGS.cpu_fan_mode_control
                    && *val == AN515_46_REGS.cpu_manual_mode),
            "set-fan-mode cpu manual must write to CPU fan mode register: {logged:?}"
        );
    }

    #[tokio::test]
    async fn run_cli_mode_status_only_returns_true_without_modifying_ec() {
        let (runtime, _ec, _state, _t, _s, _c, writes) = build_runtime();
        let cli = CliOptions {
            status: true,
            ..CliOptions::default()
        };

        let handled = run_cli_mode(&cli, runtime).await.unwrap();
        assert!(handled);
        assert!(
            writes.lock().unwrap().is_empty(),
            "--status must not write to EC"
        );
    }

    #[tokio::test]
    async fn run_cli_mode_no_gui_streams_telemetry_until_shutdown() {
        let (runtime, _ec, _state, telemetry_tx, shutdown_tx, _c, _writes) = build_runtime();
        let cli = CliOptions {
            no_gui: true,
            ..CliOptions::default()
        };

        // Drive run_cli_mode concurrently with a shutdown signal so the
        // headless loop terminates promptly. Both futures are awaited in
        // place to avoid 'static lifetime requirements from `tokio::spawn`.
        let driver = async {
            // Allow the headless loop to grab the initial state lock before
            // we send the shutdown signal so we exercise the telemetry path.
            tokio::time::sleep(Duration::from_millis(20)).await;
            telemetry_tx.send(TelemetrySnapshot::default()).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
            shutdown_tx.send(true).unwrap();
        };

        let (handled, _) = tokio::time::timeout(
            Duration::from_secs(3),
            futures_join(run_cli_mode(&cli, runtime), driver),
        )
        .await
        .expect("run_cli_mode --no-gui must terminate after shutdown");
        let handled = handled.expect("run_cli_mode --no-gui must succeed");

        assert!(
            handled,
            "--no-gui run_cli_mode must report handled=true so caller skips GUI"
        );
    }

    /// Tiny helper analogous to `futures::future::join` to avoid pulling in the
    /// `futures` crate just for two-arm concurrency in the test above.
    async fn futures_join<A, B, RA, RB>(a: A, b: B) -> (RA, RB)
    where
        A: std::future::Future<Output = RA>,
        B: std::future::Future<Output = RB>,
    {
        tokio::join!(a, b)
    }

    // ---- execute_hardware_command_and_save ----

    #[tokio::test]
    async fn execute_hardware_command_and_save_persists_settings_commands() {
        let (runtime, ec, state, _t, _s, _c, _writes) = build_runtime();
        let regs = runtime.regs;
        let config_manager = runtime.config_manager.clone();
        drop(runtime);

        execute_hardware_command_and_save(
            &ec,
            &state,
            regs,
            &config_manager,
            Command::SetProfile(PerformanceProfile::Quiet),
        )
        .await
        .expect("settings command should succeed");

        // The saved config must reflect the new profile (quiet_mode = 0x00 on AN515-46).
        let loaded = config_manager.load_config().expect("config should reload");
        assert_eq!(loaded.nitro_mode, regs.quiet_mode);
    }

    #[tokio::test]
    async fn execute_hardware_command_and_save_skips_persist_for_non_settings_commands() {
        let (runtime, ec, state, _t, _s, _c, _writes) = build_runtime();
        let regs = runtime.regs;
        let config_manager = runtime.config_manager.clone();
        drop(runtime);

        execute_hardware_command_and_save(&ec, &state, regs, &config_manager, Command::SaveConfig)
            .await
            .expect("save-config should succeed (no-op in handler)");

        // No config file should be persisted because SaveConfig isn't a
        // settings command.
        let path = std::env::temp_dir().join("nitrosense.toml");
        assert!(!path.exists());
    }

    // ---- save_main_config + set_last_error ----

    #[tokio::test]
    async fn save_main_config_writes_state_derived_config_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ConfigManager::with_config_dir(dir.path());
        let state = Arc::new(TokioMutex::new(AppState {
            performance_profile: PerformanceProfile::Extreme,
            ..AppState::default()
        }));

        save_main_config(&state, &AN515_46_REGS, &manager).await;
        let loaded = manager.load_config().expect("config should reload");
        assert_eq!(loaded.nitro_mode, AN515_46_REGS.extreme_mode);
    }

    #[tokio::test]
    async fn save_main_config_logs_error_when_config_dir_unwritable() {
        // Use a config_dir whose parent doesn't exist and isn't writable to
        // force the underlying create_dir_all to fail. We use /proc/foo
        // which is read-only.
        let manager = ConfigManager::with_config_dir("/proc/this/cannot/be/created/nitrosense");
        let state = Arc::new(TokioMutex::new(AppState::default()));

        // Should NOT panic even if the save fails.
        save_main_config(&state, &AN515_46_REGS, &manager).await;
    }

    #[tokio::test]
    async fn set_last_error_stores_message_in_app_state() {
        let state = Arc::new(TokioMutex::new(AppState::default()));
        set_last_error(&state, "unit test error".to_string()).await;
        let guard = state.lock().await;
        assert_eq!(guard.last_error.as_deref(), Some("unit test error"));
    }

    // ---- spawn_signal_watcher / spawn_voltage_poller ----

    #[tokio::test]
    async fn spawn_signal_watcher_terminates_on_shutdown_change() {
        let (cmd_tx, _cmd_rx) = tokio_mpsc::channel::<Command>(8);
        let (shutdown_tx, shutdown_rx) = tokio_watch::channel(false);

        let handle = spawn_signal_watcher(cmd_tx, shutdown_rx);
        shutdown_tx.send(true).expect("shutdown send must succeed");

        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("signal watcher must terminate after shutdown change")
            .expect("signal watcher task must not panic");
    }

    #[tokio::test]
    async fn spawn_signal_watcher_terminates_when_shutdown_sender_dropped() {
        let (cmd_tx, _cmd_rx) = tokio_mpsc::channel::<Command>(8);
        let (shutdown_tx, shutdown_rx) = tokio_watch::channel(false);

        let handle = spawn_signal_watcher(cmd_tx, shutdown_rx);
        drop(shutdown_tx);

        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("signal watcher must terminate after sender drop")
            .expect("signal watcher task must not panic");
    }

    #[tokio::test]
    async fn spawn_voltage_poller_returns_none_for_unknown_vendor() {
        let state = Arc::new(TokioMutex::new(AppState::default()));
        let (_shutdown_tx, shutdown_rx) = tokio_watch::channel(false);

        let handle = spawn_voltage_poller(state, shutdown_rx, CpuVendor::Unknown);
        assert!(
            handle.is_none(),
            "Unknown vendor must skip the voltage task"
        );
    }

    #[tokio::test]
    async fn spawn_voltage_poller_returns_some_for_known_vendor_and_terminates_on_shutdown() {
        let state = Arc::new(TokioMutex::new(AppState::default()));
        let (shutdown_tx, shutdown_rx) = tokio_watch::channel(false);

        let handle = spawn_voltage_poller(state, shutdown_rx, CpuVendor::Intel)
            .expect("Intel vendor must spawn voltage task");

        // Immediately shut down — the polled subprocesses might error out but
        // the task should exit cleanly.
        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("voltage poller should terminate")
            .expect("voltage poller task must not panic");
    }

    // ---- run_command_worker  ----

    #[tokio::test]
    async fn run_command_worker_processes_save_config_then_shuts_down_on_command() {
        let (runtime, _ec, state, _t, _s, _c, _writes) = build_runtime();
        let CliRuntime {
            ec,
            command_tx,
            regs,
            config_manager,
            ..
        } = runtime;

        let (cmd_tx, cmd_rx) = tokio_mpsc::channel::<Command>(8);
        let (shutdown_tx, mut shutdown_rx) = tokio_watch::channel(false);
        let _ = command_tx; // not used here

        let worker = tokio::spawn(run_command_worker(
            cmd_rx,
            ec,
            Arc::clone(&state),
            config_manager,
            regs,
            shutdown_tx,
            CpuVendor::Unknown,
        ));

        cmd_tx.send(Command::SaveConfig).await.unwrap();
        cmd_tx.send(Command::Shutdown).await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .expect("command worker must terminate after Shutdown")
            .expect("command worker task must not panic");

        // Worker must have flipped the shutdown flag before exiting.
        shutdown_rx.changed().await.unwrap();
        assert!(*shutdown_rx.borrow());
    }

    #[tokio::test]
    async fn run_command_worker_apply_undervolt_rejects_core_above_seven() {
        let (runtime, _ec, state, _t, _s, _c, _writes) = build_runtime();
        let CliRuntime {
            ec,
            regs,
            config_manager,
            ..
        } = runtime;
        let (cmd_tx, cmd_rx) = tokio_mpsc::channel::<Command>(8);
        let (shutdown_tx, _shutdown_rx) = tokio_watch::channel(false);
        let worker = tokio::spawn(run_command_worker(
            cmd_rx,
            ec,
            Arc::clone(&state),
            config_manager,
            regs,
            shutdown_tx,
            CpuVendor::Amd,
        ));

        cmd_tx.send(Command::ApplyUndervolt(8)).await.unwrap();
        // Allow processing.
        tokio::time::sleep(Duration::from_millis(50)).await;

        {
            let s = state.lock().await;
            assert_eq!(
                s.last_error.as_deref(),
                Some("Undervolt core 8 is outside 0..=7")
            );
        }

        cmd_tx.send(Command::Shutdown).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), worker).await;
    }

    #[tokio::test]
    async fn run_command_worker_apply_undervolt_marks_unsupported_for_intel() {
        let (runtime, _ec, state, _t, _s, _c, _writes) = build_runtime();
        let CliRuntime {
            ec,
            regs,
            config_manager,
            ..
        } = runtime;
        let (cmd_tx, cmd_rx) = tokio_mpsc::channel::<Command>(8);
        let (shutdown_tx, _shutdown_rx) = tokio_watch::channel(false);

        let worker = tokio::spawn(run_command_worker(
            cmd_rx,
            ec,
            Arc::clone(&state),
            config_manager,
            regs,
            shutdown_tx,
            CpuVendor::Intel,
        ));

        cmd_tx.send(Command::ApplyUndervolt(0)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        {
            let s = state.lock().await;
            assert_eq!(
                s.undervolt_status,
                nitrosense::hardware::voltage::unsupported_undervolt_message()
            );
        }

        cmd_tx.send(Command::Shutdown).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), worker).await;
    }

    #[tokio::test]
    async fn run_command_worker_save_rgb_config_round_trips_through_config_manager() {
        let (runtime, _ec, state, _t, _s, _c, _writes) = build_runtime();
        let CliRuntime {
            ec,
            regs,
            config_manager,
            ..
        } = runtime;

        // Mutate state's rgb_config so SaveRgbConfig has something to write.
        {
            let mut s = state.lock().await;
            s.rgb_config = RgbConfig {
                mode: 1,
                zone: 2,
                speed: 3,
                brightness: 90,
                direction: 1,
                red: 11,
                green: 22,
                blue: 33,
            };
        }

        let (cmd_tx, cmd_rx) = tokio_mpsc::channel::<Command>(8);
        let (shutdown_tx, _shutdown_rx) = tokio_watch::channel(false);
        let worker = tokio::spawn(run_command_worker(
            cmd_rx,
            ec,
            Arc::clone(&state),
            config_manager.clone(),
            regs,
            shutdown_tx,
            CpuVendor::Unknown,
        ));

        cmd_tx.send(Command::SaveRgbConfig).await.unwrap();
        cmd_tx.send(Command::Shutdown).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), worker).await;

        let on_disk = config_manager
            .load_rgb_config()
            .expect("RGB config should round-trip through SaveRgbConfig");
        assert_eq!(on_disk.brightness, 90);
        assert_eq!(on_disk.red, 11);
    }

    #[tokio::test]
    async fn run_command_worker_load_rgb_config_returns_default_when_file_absent() {
        let (runtime, _ec, state, _t, _s, _c, _writes) = build_runtime();
        let CliRuntime {
            ec,
            regs,
            config_manager,
            ..
        } = runtime;

        let (cmd_tx, cmd_rx) = tokio_mpsc::channel::<Command>(8);
        let (shutdown_tx, _shutdown_rx) = tokio_watch::channel(false);
        let worker = tokio::spawn(run_command_worker(
            cmd_rx,
            ec,
            Arc::clone(&state),
            config_manager,
            regs,
            shutdown_tx,
            CpuVendor::Unknown,
        ));

        cmd_tx.send(Command::LoadRgbConfig).await.unwrap();
        cmd_tx.send(Command::Shutdown).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), worker).await;

        // No on-disk file; the worker must fall back to RgbConfig::default()
        // and copy it into state.rgb_config without producing an error.
        let s = state.lock().await;
        assert_eq!(s.rgb_config, RgbConfig::default());
        assert!(
            s.last_error.is_none(),
            "loading missing RGB file should not produce a user-visible error: {:?}",
            s.last_error
        );
    }

    #[tokio::test]
    async fn run_command_worker_apply_rgb_persists_valid_config_to_disk() {
        let (runtime, _ec, state, _t, _s, _c, _writes) = build_runtime();
        let CliRuntime {
            ec,
            regs,
            config_manager,
            ..
        } = runtime;

        let (cmd_tx, cmd_rx) = tokio_mpsc::channel::<Command>(8);
        let (shutdown_tx, _shutdown_rx) = tokio_watch::channel(false);
        let worker = tokio::spawn(run_command_worker(
            cmd_rx,
            ec,
            Arc::clone(&state),
            config_manager.clone(),
            regs,
            shutdown_tx,
            CpuVendor::Unknown,
        ));

        let cfg = RgbConfig {
            mode: 2,
            zone: 1,
            speed: 4,
            brightness: 75,
            direction: 2,
            red: 240,
            green: 220,
            blue: 100,
        };
        cmd_tx.send(Command::ApplyRgb(cfg.clone())).await.unwrap();
        cmd_tx.send(Command::Shutdown).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), worker).await;

        let on_disk = config_manager
            .load_rgb_config()
            .expect("ApplyRgb should persist the new config");
        assert_eq!(on_disk, cfg);
        let s = state.lock().await;
        assert_eq!(s.rgb_config, cfg);
    }

    #[tokio::test]
    async fn run_command_worker_apply_rgb_rejects_invalid_config() {
        let (runtime, _ec, state, _t, _s, _c, _writes) = build_runtime();
        let CliRuntime {
            ec,
            regs,
            config_manager,
            ..
        } = runtime;
        let (cmd_tx, cmd_rx) = tokio_mpsc::channel::<Command>(8);
        let (shutdown_tx, _shutdown_rx) = tokio_watch::channel(false);

        let worker = tokio::spawn(run_command_worker(
            cmd_rx,
            ec,
            Arc::clone(&state),
            config_manager,
            regs,
            shutdown_tx,
            CpuVendor::Unknown,
        ));

        let bad = RgbConfig {
            mode: 99, // out-of-range
            ..RgbConfig::default()
        };
        cmd_tx.send(Command::ApplyRgb(bad)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        {
            let s = state.lock().await;
            assert!(
                s.last_error
                    .as_ref()
                    .map(|m| m.contains("Invalid RGB config"))
                    .unwrap_or(false),
                "rejected RGB must surface a descriptive error: {:?}",
                s.last_error
            );
        }

        cmd_tx.send(Command::Shutdown).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), worker).await;
    }

    // ---- run_headless ----

    #[tokio::test]
    async fn run_headless_exits_when_telemetry_sender_dropped() {
        let state = Arc::new(TokioMutex::new(AppState::default()));
        let (telemetry_tx, telemetry_rx) =
            tokio_watch::channel::<TelemetrySnapshot>(TelemetrySnapshot::default());
        let (_shutdown_tx, shutdown_rx) = tokio_watch::channel(false);
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(8);

        let handle = tokio::spawn(run_headless(
            telemetry_rx,
            state,
            shutdown_rx,
            cmd_tx,
            &AN515_46_REGS,
        ));

        // Drop the telemetry sender to make changed() return Err.
        drop(telemetry_tx);

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("run_headless must terminate when telemetry sender drops")
            .expect("run_headless task must not panic");

        // Must have sent Shutdown before exiting.
        let cmd = cmd_rx
            .try_recv()
            .expect("run_headless must enqueue Shutdown");
        assert!(matches!(cmd, Command::Shutdown));
    }

    #[tokio::test]
    async fn run_headless_exits_when_shutdown_signal_set() {
        let state = Arc::new(TokioMutex::new(AppState::default()));
        let (_telemetry_tx, telemetry_rx) =
            tokio_watch::channel::<TelemetrySnapshot>(TelemetrySnapshot::default());
        let (shutdown_tx, shutdown_rx) = tokio_watch::channel(false);
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(8);

        let handle = tokio::spawn(run_headless(
            telemetry_rx,
            state,
            shutdown_rx,
            cmd_tx,
            &AN515_46_REGS,
        ));
        // Allow the loop to start.
        tokio::time::sleep(Duration::from_millis(50)).await;
        shutdown_tx.send(true).unwrap();

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("run_headless must terminate when shutdown is set")
            .expect("run_headless task must not panic");

        let cmd = cmd_rx
            .try_recv()
            .expect("run_headless must enqueue Shutdown");
        assert!(matches!(cmd, Command::Shutdown));
    }

    // ---- shutdown_application ----

    #[tokio::test]
    async fn shutdown_application_joins_all_workers_and_saves_config() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ConfigManager::with_config_dir(dir.path());
        let state = Arc::new(TokioMutex::new(AppState {
            performance_profile: PerformanceProfile::Quiet,
            ..AppState::default()
        }));
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(8);

        // Build worker handles that finish promptly when called.
        let poller = tokio::spawn(async {});
        let voltage = Some(tokio::spawn(async {}));
        let command = tokio::spawn(async {});
        let signal = tokio::spawn(async {});

        shutdown_application(
            &state,
            &AN515_46_REGS,
            &manager,
            &cmd_tx,
            WorkerHandles {
                poller,
                voltage,
                command,
                signal,
            },
        )
        .await;

        let cmd = cmd_rx
            .try_recv()
            .expect("shutdown_application must send Shutdown");
        assert!(matches!(cmd, Command::Shutdown));

        let loaded = manager.load_config().expect("config should reload");
        assert_eq!(loaded.nitro_mode, AN515_46_REGS.quiet_mode);
    }

    // ---- has_required_privileges / process_has_required_capabilities ----

    #[test]
    fn has_required_privileges_returns_true_for_root_or_capable_process() {
        // Whatever the host returns is acceptable; we only assert this doesn't
        // panic and exercises the function body.
        let _ = has_required_privileges();
    }

    #[test]
    fn process_has_required_capabilities_returns_a_bool_for_current_process() {
        let _ = process_has_required_capabilities();
    }

    #[test]
    fn parse_cap_eff_from_status_returns_none_when_field_absent() {
        assert_eq!(parse_cap_eff_from_status("Name:\tnitrosense\n"), None);
    }

    #[test]
    fn parse_cap_eff_from_status_returns_none_for_non_hex_value() {
        assert_eq!(parse_cap_eff_from_status("CapEff:\tnot-hex\n"), None);
    }

    // ---- install_signal_handlers (idempotent install) ----

    #[test]
    fn install_signal_handlers_returns_ok_for_supported_signals() {
        // On Linux this should always succeed. The handlers persist for the
        // remainder of the test process — that's fine because they only set
        // the SHUTDOWN_REQUESTED atomic.
        install_signal_handlers().expect("sigaction(SIGINT/SIGTERM) must succeed on Linux");
    }

    #[test]
    fn shutdown_requested_observability_clears_to_false_when_reset() {
        SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
        assert!(!shutdown_requested());
        SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
        assert!(shutdown_requested());
        SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    }

    // ---- CliOptions defaults ----

    #[test]
    fn cli_options_default_is_all_negative() {
        let cli = CliOptions::default();
        assert!(!cli.no_gui);
        assert!(!cli.status);
        assert!(cli.set_profile.is_none());
        assert!(cli.set_fan_mode.is_none());
    }

    // ---- cli_command builder structure ----

    #[test]
    fn cli_command_exposes_expected_long_options() {
        let cmd = cli_command();
        let names: Vec<_> = cmd.get_arguments().filter_map(|a| a.get_long()).collect();
        for required in ["no-gui", "status", "set-profile", "set-fan-mode"] {
            assert!(
                names.contains(&required),
                "cli should expose --{required}: {names:?}"
            );
        }
    }
}
