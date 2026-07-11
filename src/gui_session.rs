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

    if let Some(runtime_dir) = runtime_dir {
        if env::var_os("WAYLAND_DISPLAY")
            .filter(|value| !value.is_empty())
            .is_none()
        {
            if let Some(display) = discover_wayland_display(&runtime_dir) {
                restored |= set_env("WAYLAND_DISPLAY", &display);
            }
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

    for key in ["WAYLAND_DISPLAY", "DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS"] {
        if env::var_os(key)
            .filter(|value| !value.is_empty())
            .is_none()
            && let Some(value) = read_env_var_from_user_process(uid, key)
        {
            restored |= set_env(key, &value);
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

fn wayland_ready() -> bool {
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
    if let Ok(uid) = env::var("SUDO_UID").and_then(|value| value.parse::<u32>().map_err(|_| env::VarError::NotPresent)) {
        return Some(uid);
    }

    let user = env::var("SUDO_USER").ok()?;
    resolve_user_uid(&user)
}

fn resolve_user_uid(user: &str) -> Option<u32> {
    let output = std::process::Command::new("id")
        .args(["-u", user])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn discover_wayland_display(runtime_dir: &Path) -> Option<String> {
    let mut sockets = fs::read_dir(runtime_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name())
        .filter(|name| {
            let name = name.to_string_lossy();
            name.starts_with("wayland-") && !name.ends_with(".lock")
        })
        .map(|name| name.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    sockets.sort();
    sockets.into_iter().next()
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
                .is_some_and(|name| name.starts_with(".mutter-Xwaylandauth.") || name.starts_with(".xauth_"))
        })
        .collect::<Vec<_>>();

    candidates.sort();
    candidates.into_iter().find(|path| path.is_file())
}

fn read_env_var_from_user_process(uid: u32, key: &str) -> Option<String> {
    let proc_root = fs::read_dir("/proc").ok()?;
    for entry in proc_root {
        let entry = entry.ok()?;
        if entry
            .file_name()
            .to_string_lossy()
            .parse::<u32>()
            .is_err()
        {
            continue;
        }

        let status = fs::read_to_string(entry.path().join("status")).ok()?;
        if !status_has_uid(&status, uid) {
            continue;
        }

        let environ = fs::read(entry.path().join("environ")).ok()?;
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
    // SAFETY: called during startup before other threads read the environment.
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
    fn discover_wayland_display_prefers_lowest_socket() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("wayland-1"), []).unwrap();
        fs::write(dir.path().join("wayland-0"), []).unwrap();
        fs::write(dir.path().join("wayland-0.lock"), []).unwrap();

        assert_eq!(
            discover_wayland_display(dir.path()).as_deref(),
            Some("wayland-0")
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
