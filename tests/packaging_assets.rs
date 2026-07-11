// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::fs;
use std::path::Path;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn install_and_uninstall_scripts_are_posix_shell() {
    for script in ["install.sh", "uninstall.sh"] {
        let contents = fs::read_to_string(Path::new(ROOT).join(script)).unwrap();
        assert!(contents.starts_with("#!/bin/sh\n"));
        assert!(contents.contains("id -u"));
        assert!(!contents.contains("#!/bin/bash"));
    }
}

#[test]
fn install_script_installs_phase_14_paths() {
    let contents = fs::read_to_string(Path::new(ROOT).join("install.sh")).unwrap();
    for expected in [
        "/usr/bin/nitrosense",
        "/usr/bin/nitro-sense",
        "/usr/share/applications/nitrosense.desktop",
        "/usr/share/icons/hicolor/256x256/apps/nitrosense.png",
        "/usr/share/polkit-1/actions/com.nitrosense.launch.policy",
        "/usr/local/share/fonts/nitrosense",
        "/etc/nitrosense",
        "fc-cache -f",
        "cargo build --release --manifest-path \"${repo_dir}/Cargo.toml\"",
        "warning: RGB keyboard interface not detected.",
    ] {
        assert!(
            contents.contains(expected),
            "missing install path {expected}"
        );
    }
}

#[test]
fn install_script_builds_repo_manifest_independent_of_caller_cwd() {
    let contents = fs::read_to_string(Path::new(ROOT).join("install.sh")).unwrap();
    assert!(contents.contains("repo_dir=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd -P)"));
    assert!(contents.contains("binary_src=\"${repo_dir}/target/release/nitrosense\""));
    assert!(contents.contains("cargo build --release --manifest-path \"${repo_dir}/Cargo.toml\""));
    assert!(!contents.contains("\n    cargo build --release\n"));
}

#[test]
fn install_script_never_builds_release_binary_as_root() {
    let contents = fs::read_to_string(Path::new(ROOT).join("install.sh")).unwrap();
    let build_dispatch = contents
        .split("if [ ! -x \"$binary_src\" ]; then")
        .nth(1)
        .and_then(|section| section.split("\nif [ ! -x \"$binary_src\" ]; then").next())
        .expect("install.sh should dispatch the missing-binary build");
    assert!(contents.contains("refusing to run cargo build as root"));
    assert!(contents.contains("sudo -H -u \"$SUDO_USER\""));
    assert!(contents.contains("build_as_sudo_user"));
    assert!(contents.contains("build_as_current_user"));
    assert!(!build_dispatch.contains("cargo build --release --manifest-path"));
}

#[test]
fn pkexec_wrapper_preserves_gui_session_environment() {
    let contents = fs::read_to_string(Path::new(ROOT).join("install.sh")).unwrap();
    assert!(contents.contains("pkexec env"));
    for key in [
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        assert!(
            contents.contains(key),
            "nitro-sense wrapper should preserve {key}"
        );
    }
}

#[test]
fn scripts_skip_font_cache_updates_during_destdir_staging() {
    for script in ["install.sh", "uninstall.sh"] {
        let contents = fs::read_to_string(Path::new(ROOT).join(script)).unwrap();
        assert!(contents.contains("Skipping fc-cache for DESTDIR staging."));
        assert!(contents.contains(r#"[ "${DESTDIR:-}" != "" ]"#));
    }
}

#[test]
fn uninstall_script_prompts_before_removing_config_dir() {
    let contents = fs::read_to_string(Path::new(ROOT).join("uninstall.sh")).unwrap();
    assert!(contents.contains("Remove %s and all saved NitroSense settings? [y/N]"));
    assert!(contents.contains("rm -rf \"$config_dir\""));
}

#[test]
fn desktop_entry_matches_linux_launcher_contract() {
    let contents = fs::read_to_string(Path::new(ROOT).join("assets/nitrosense.desktop")).unwrap();
    for expected in [
        "Type=Application",
        "Terminal=false",
        "Exec=/usr/bin/nitro-sense",
        "Name=Linux NitroSense",
        "Icon=nitrosense",
        "Categories=System;Monitor;Settings;",
    ] {
        assert!(
            contents.contains(expected),
            "missing desktop key {expected}"
        );
    }
}

#[test]
fn polkit_policy_targets_installed_binary() {
    let contents =
        fs::read_to_string(Path::new(ROOT).join("assets/com.nitrosense.launch.policy")).unwrap();
    assert!(contents.contains(r#"<action id="com.nitrosense.launch">"#));
    assert!(contents.contains(
        r#"<annotate key="org.freedesktop.policykit.exec.path">/usr/bin/nitrosense</annotate>"#
    ));
    assert!(
        contents.contains(
            r#"<annotate key="org.freedesktop.policykit.exec.allow_gui">true</annotate>"#
        )
    );
}

#[test]
fn polkit_policy_has_required_dtd_elements_in_order() {
    let contents =
        fs::read_to_string(Path::new(ROOT).join("assets/com.nitrosense.launch.policy")).unwrap();
    for expected in [
        r#"<!DOCTYPE policyconfig PUBLIC "-//freedesktop//DTD polkit Policy Configuration 1.0//EN""#,
        "<description>Launch Linux NitroSense</description>",
        "<message>NitroSense requires elevated privileges for hardware access</message>",
    ] {
        assert!(
            contents.contains(expected),
            "missing policy element {expected}"
        );
    }
    let description = contents.find("<description>").unwrap();
    let message = contents.find("<message>").unwrap();
    let defaults = contents.find("<defaults>").unwrap();
    let annotate = contents.find("<annotate").unwrap();
    assert!(description < message);
    assert!(message < defaults);
    assert!(defaults < annotate);
}

#[test]
fn packaged_assets_include_all_fonts_and_desktop_icon() {
    let fonts_dir = Path::new(ROOT).join("assets/fonts");
    for font in [
        "Squares Black Italic.otf",
        "Squares Black.otf",
        "Squares Bold Italic.otf",
        "Squares Bold.otf",
        "Squares Italic.otf",
        "Squares Light italic.otf",
        "Squares Light.otf",
        "Squares Regular.otf",
        "Squares Thin Italic.otf",
        "Squares Thin.otf",
    ] {
        let path = fonts_dir.join(font);
        assert!(path.exists(), "missing font asset {}", path.display());
        assert!(fs::metadata(path).unwrap().len() > 0);
    }
    let icon = Path::new(ROOT).join("assets/nitrosense.png");
    assert!(icon.exists());
    assert!(fs::metadata(icon).unwrap().len() > 0);
}
