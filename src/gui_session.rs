// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

//! Helpers for preserving Wayland/X11 session environment across privilege elevation.
//!
//! `sudo nitrosense` drops the caller's compositor variables while often keeping a stale
//! `DISPLAY` value. On Hyprland that makes winit fall back to X11 and fail with
//! `XOpenDisplayFailed`. These helpers restore the invoking user's session env.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Environment variables required to attach a GUI to the active desktop session.
pub const GUI_ENV_KEYS: &[&str] = &[
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
];

/// Returns true when the current process has enough session state to open a window.
pub fn ready() -> bool {
    wayland_ready() || x11_ready()
}

/// A non-empty `DISPLAY` is not a Wayland session. Restore whenever the
/// Wayland socket is missing, including under `sudo` where `DISPLAY` survives.
pub fn should_restore_wayland(wayland_is_ready: bool) -> bool {
    !wayland_is_ready
}

/// Prefer the invoking user's socket name. Discovery is only the fallback.
pub fn choose_wayland_display<'a>(
    user_value: Option<&'a str>,
    discovered: &'a [String],
) -> Option<&'a str> {
    if let Some(value) = user_value.map(str::trim).filter(|value| !value.is_empty()) {
        return Some(value);
    }
    discovered.first().map(String::as_str)
}

/// Populate missing GUI session variables from the user that invoked `sudo`.
///
/// Returns `true` when at least one variable was restored.
pub fn ensure_from_invoking_user() -> bool {
    let Some(uid) = invoking_session_uid() else {
        return false;
    };

    let mut restored = false;
    let runtime_dir = PathBuf::from(format!("/run/user/{uid}"));

    if env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .is_none()
        && runtime_dir.is_dir()
    {
        restored |= set_env("XDG_RUNTIME_DIR", runtime_dir.to_string_lossy().as_ref());
    }

    let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| runtime_dir.is_dir().then_some(runtime_dir));

    for key in [
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "XAUTHORITY",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        if env::var_os(key).filter(|value| !value.is_empty()).is_none()
            && let Some(value) = read_env_var_from_user_process(uid, key)
        {
            if key == "WAYLAND_DISPLAY"
                && let Some(dir) = runtime_dir.as_ref()
                && !wayland_socket_exists(dir, &value)
            {
                continue;
            }
            restored |= set_env(key, &value);
        }
    }

    if let Some(runtime_dir) = runtime_dir {
        if let Some(name) = env::var("WAYLAND_DISPLAY")
            .ok()
            .filter(|name| !name.is_empty())
            && !wayland_socket_exists(&runtime_dir, &name)
        {
            // SAFETY: `main` calls this before the tokio runtime starts.
            unsafe { env::remove_var("WAYLAND_DISPLAY") };
        }

        if env::var_os("WAYLAND_DISPLAY")
            .filter(|value| !value.is_empty())
            .is_none()
            && let Some(display) = discover_wayland_display(&runtime_dir)
        {
            restored |= set_env("WAYLAND_DISPLAY", &display);
        }

        if env::var_os("DBUS_SESSION_BUS_ADDRESS")
            .filter(|value| !value.is_empty())
            .is_none()
            && runtime_dir.join("bus").exists()
        {
            let address = format!("unix:path={}/bus", runtime_dir.display());
            restored |= set_env("DBUS_SESSION_BUS_ADDRESS", &address);
        }

        if env::var_os("XAUTHORITY")
            .filter(|value| !value.is_empty())
            .is_none()
            && let Some(path) = discover_xauthority(&runtime_dir)
        {
            restored |= set_env("XAUTHORITY", path.to_string_lossy().as_ref());
        }
    }

    restored
}

/// Build `KEY=VALUE` assignments for `pkexec env` wrappers.
pub fn env_assignments() -> Vec<String> {
    GUI_ENV_KEYS
        .iter()
        .filter_map(|key| {
            env::var(key)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| format!("{key}={value}"))
        })
        .collect()
}

pub fn wayland_ready() -> bool {
    let Some(display) = env::var_os("WAYLAND_DISPLAY").filter(|value| !value.is_empty()) else {
        return false;
    };

    let runtime_dir = match env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => return false,
    };

    if !runtime_dir.is_dir() {
        return false;
    }

    runtime_dir.join(display).exists()
}

fn x11_ready() -> bool {
    env::var_os("DISPLAY")
        .filter(|value| !value.is_empty())
        .is_some()
}

fn invoking_session_uid() -> Option<u32> {
    if let Some(uid) = session_uid(
        env::var("PKEXEC_UID").ok().as_deref(),
        env::var("SUDO_UID").ok().as_deref(),
    ) {
        return Some(uid);
    }

    let user = env::var("SUDO_USER").ok()?;
    resolve_user_uid(&user)
}

/// `pkexec` records the caller in `PKEXEC_UID` and drops the desktop
/// environment on a kept authorization. `sudo` uses `SUDO_UID`.
pub fn session_uid(pkexec_uid: Option<&str>, sudo_uid: Option<&str>) -> Option<u32> {
    fn parse(value: Option<&str>) -> Option<u32> {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())?
            .parse()
            .ok()
    }

    parse(pkexec_uid).or_else(|| parse(sudo_uid))
}

fn resolve_user_uid(user: &str) -> Option<u32> {
    let output = std::process::Command::new("id")
        .args(["-u", user])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

fn discover_wayland_display(runtime_dir: &Path) -> Option<String> {
    let mut sockets = fs::read_dir(runtime_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with("wayland-") && !name.ends_with(".lock")) {
                return None;
            }
            let modified = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            Some((modified, name.into_owned()))
        })
        .collect::<Vec<_>>();

    sockets.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let names = sockets
        .into_iter()
        .map(|(_, name)| name)
        .collect::<Vec<_>>();
    choose_wayland_display(None, &names).map(str::to_owned)
}

fn discover_xauthority(runtime_dir: &Path) -> Option<PathBuf> {
    let direct = runtime_dir.join(".Xauthority");
    if direct.is_file() {
        return Some(direct);
    }

    let mut candidates = fs::read_dir(runtime_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| {
                    name.starts_with(".mutter-Xwaylandauth.") || name.starts_with(".xauth_")
                })
        })
        .collect::<Vec<_>>();

    candidates.sort();
    candidates.into_iter().find(|path| path.is_file())
}

fn wayland_socket_exists(runtime_dir: &Path, name: &str) -> bool {
    !name.is_empty() && runtime_dir.join(name).exists()
}

fn read_env_var_from_user_process(uid: u32, key: &str) -> Option<String> {
    let proc_root = fs::read_dir("/proc").ok()?;
    for entry in proc_root.flatten() {
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }

        let Ok(status) = fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        if !status_has_uid(&status, uid) {
            continue;
        }

        let Ok(environ) = fs::read(entry.path().join("environ")) else {
            continue;
        };
        if let Some(value) = environ_key(&environ, key) {
            return Some(value);
        }
    }

    None
}

fn status_has_uid(status: &str, uid: u32) -> bool {
    status.lines().any(|line| {
        line.strip_prefix("Uid:")
            .and_then(|values| values.split_whitespace().next())
            .and_then(|value| value.parse::<u32>().ok())
            .is_some_and(|value| value == uid)
    })
}

fn environ_key(environ: &[u8], key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    environ.split(|byte| *byte == 0).find_map(|entry| {
        let entry = std::str::from_utf8(entry).ok()?;
        entry.strip_prefix(&prefix).map(str::to_owned)
    })
}

fn set_env(key: &str, value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    // SAFETY: `main` calls `ensure_from_invoking_user` before the tokio runtime
    // starts. No other thread exists yet, so the environment mutation is race-free.
    unsafe { env::set_var(key, value) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_has_uid_matches_real_euid_field() {
        let status = "Name:\ttest\nUid:\t1000\t1000\t1000\t1000\n";
        assert!(status_has_uid(status, 1000));
        assert!(!status_has_uid(status, 1001));
    }

    #[test]
    fn environ_key_extracts_requested_entry() {
        let environ = b"HOME=/home/user\0WAYLAND_DISPLAY=wayland-1\0";
        assert_eq!(
            environ_key(environ, "WAYLAND_DISPLAY").as_deref(),
            Some("wayland-1")
        );
        assert_eq!(environ_key(environ, "DISPLAY"), None);
    }

    #[test]
    fn stale_wayland_name_is_not_usable_when_the_socket_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("wayland-1"), []).unwrap();
        assert!(!wayland_socket_exists(dir.path(), "wayland-0"));
        assert!(wayland_socket_exists(dir.path(), "wayland-1"));
    }

    #[test]
    fn pkexec_uid_identifies_the_caller_when_sudo_uid_is_absent() {
        assert_eq!(session_uid(Some("1000"), None), Some(1000));
        assert_eq!(session_uid(None, Some("1000")), Some(1000));
        assert_eq!(session_uid(Some("1000"), Some("0")), Some(1000));
        assert_eq!(session_uid(Some("  "), Some("")), None);
    }

    #[test]
    fn stale_display_does_not_cancel_wayland_restore() {
        assert!(should_restore_wayland(false));
        assert!(!should_restore_wayland(true));
    }

    #[test]
    fn choose_wayland_display_prefers_user_socket_over_discovered_names() {
        let discovered = vec!["wayland-0".to_string(), "wayland-1".to_string()];
        assert_eq!(
            choose_wayland_display(Some("wayland-1"), &discovered),
            Some("wayland-1")
        );
        assert_eq!(
            choose_wayland_display(Some("  "), &discovered),
            Some("wayland-0")
        );
        assert_eq!(choose_wayland_display(None, &discovered), Some("wayland-0"));
    }

    #[test]
    fn discover_wayland_display_prefers_newest_socket() {
        let dir = tempfile::tempdir().unwrap();
        let older = dir.path().join("wayland-0");
        let newer = dir.path().join("wayland-1");
        fs::write(&older, []).unwrap();
        fs::write(&newer, []).unwrap();
        fs::write(dir.path().join("wayland-0.lock"), []).unwrap();
        fs::File::open(&older)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10))
            .unwrap();
        fs::File::open(&newer)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(40))
            .unwrap();

        assert_eq!(
            discover_wayland_display(dir.path()).as_deref(),
            Some("wayland-1")
        );
    }

    #[test]
    fn env_assignments_omit_empty_values() {
        let assignments = env_assignments();
        for assignment in assignments {
            let (_, value) = assignment
                .split_once('=')
                .expect("assignment must contain '='");
            assert!(!value.is_empty());
        }
    }
}
