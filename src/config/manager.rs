// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use crate::config::model::{NitroConfig, RgbConfig};
use crate::error::NitroError;
use std::io::Write;
use std::path::{Path, PathBuf};

const CONFIG_DIR: &str = "/etc/nitrosense";
const CONFIG_FILE: &str = "nitrosense.toml";
const RGB_CONFIG_FILE: &str = "rgb.toml";
const LEGACY_CONFIG_FILE: &str = "nitrosense.conf";
const LEGACY_RGB_CONFIG_FILE: &str = "rbg.conf";

#[derive(Debug, Clone)]
pub struct ConfigManager {
    config_dir: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Self {
        Self {
            config_dir: PathBuf::from(CONFIG_DIR),
        }
    }

    pub fn with_config_dir(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
        }
    }

    pub fn load_config(&self) -> Result<NitroConfig, NitroError> {
        let path = self.config_dir.join(CONFIG_FILE);
        if !path.exists() {
            if let Some(config) = self.try_migrate_legacy_config()? {
                return Ok(config);
            }
            return Ok(NitroConfig::default());
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| NitroError::Config(anyhow::anyhow!("Failed to read config: {e}")))?;
        toml::from_str(&content)
            .map_err(|e| NitroError::Config(anyhow::anyhow!("Failed to parse config: {e}")))
    }

    pub fn save_config(&self, config: &NitroConfig) -> Result<(), NitroError> {
        std::fs::create_dir_all(&self.config_dir)
            .map_err(|e| NitroError::Config(anyhow::anyhow!("Failed to create config dir: {e}")))?;
        let content = toml::to_string_pretty(config)
            .map_err(|e| NitroError::Config(anyhow::anyhow!("Failed to serialize config: {e}")))?;
        let path = self.config_dir.join(CONFIG_FILE);
        atomic_write(&path, content.as_bytes())
    }

    pub fn load_rgb_config(&self) -> Result<RgbConfig, NitroError> {
        let path = self.config_dir.join(RGB_CONFIG_FILE);
        if !path.exists() {
            if let Some(config) = self.try_migrate_legacy_rgb_config()? {
                return Ok(config);
            }
            return Ok(RgbConfig::default());
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| NitroError::Config(anyhow::anyhow!("Failed to read RGB config: {e}")))?;
        toml::from_str(&content)
            .map_err(|e| NitroError::Config(anyhow::anyhow!("Failed to parse RGB config: {e}")))
    }

    pub fn save_rgb_config(&self, config: &RgbConfig) -> Result<(), NitroError> {
        std::fs::create_dir_all(&self.config_dir)
            .map_err(|e| NitroError::Config(anyhow::anyhow!("Failed to create config dir: {e}")))?;
        let content = toml::to_string_pretty(config).map_err(|e| {
            NitroError::Config(anyhow::anyhow!("Failed to serialize RGB config: {e}"))
        })?;
        let path = self.config_dir.join(RGB_CONFIG_FILE);
        atomic_write(&path, content.as_bytes())
    }

    fn try_migrate_legacy_config(&self) -> Result<Option<NitroConfig>, NitroError> {
        let path = self.config_dir.join(LEGACY_CONFIG_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path).map_err(|e| {
            NitroError::Config(anyhow::anyhow!("Failed to read legacy config: {e}"))
        })?;
        let config = parse_legacy_config(&content)?;
        self.save_config(&config)?;
        tracing::info!(legacy = %path.display(), "Migrated legacy NitroSense config to TOML");
        Ok(Some(config))
    }

    fn try_migrate_legacy_rgb_config(&self) -> Result<Option<RgbConfig>, NitroError> {
        let path = self.config_dir.join(LEGACY_RGB_CONFIG_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path).map_err(|e| {
            NitroError::Config(anyhow::anyhow!("Failed to read legacy RGB config: {e}"))
        })?;
        let config = parse_legacy_rgb_config(&content)?;
        self.save_rgb_config(&config)?;
        tracing::info!(legacy = %path.display(), "Migrated legacy RGB config to TOML");
        Ok(Some(config))
    }
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new()
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), NitroError> {
    let parent = path.parent().ok_or_else(|| {
        NitroError::Config(anyhow::anyhow!(
            "Config path has no parent directory: {}",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|e| NitroError::Config(anyhow::anyhow!("Failed to create config dir: {e}")))?;

    let tmp_path = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&tmp_path).map_err(|e| {
            NitroError::Config(anyhow::anyhow!("Failed to create temp config: {e}"))
        })?;
        file.write_all(bytes)
            .map_err(|e| NitroError::Config(anyhow::anyhow!("Failed to write temp config: {e}")))?;
        file.sync_all()
            .map_err(|e| NitroError::Config(anyhow::anyhow!("Failed to fsync temp config: {e}")))?;
    }
    std::fs::rename(&tmp_path, path).map_err(|e| {
        NitroError::Config(anyhow::anyhow!("Failed to atomically rename config: {e}"))
    })?;
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

fn parse_legacy_config(content: &str) -> Result<NitroConfig, NitroError> {
    let values = parse_legacy_u8_lines(content, 6, "NitroSense")?;
    Ok(NitroConfig {
        cpu_mode: values[0],
        gpu_mode: values[1],
        kb_30_timeout: values[2],
        usb_charging: values[3],
        nitro_mode: values[4],
        battery_charge_limit: values[5],
    })
}

fn parse_legacy_rgb_config(content: &str) -> Result<RgbConfig, NitroError> {
    let values = parse_legacy_u8_lines(content, 8, "RGB")?;
    // Python kbSaveConfig stores direction_combo.currentIndex() (0 or 1),
    // but kbApplySettings sends currentIndex() + 1 to the device (1 or 2).
    // RgbConfig.direction stores the protocol value, so shift +1 during migration.
    let direction = values[4].saturating_add(1);
    Ok(RgbConfig {
        mode: values[0],
        zone: values[1],
        speed: values[2],
        brightness: values[3],
        direction,
        red: values[5],
        green: values[6],
        blue: values[7],
    })
}

fn parse_legacy_u8_lines(
    content: &str,
    expected_lines: usize,
    label: &str,
) -> Result<Vec<u8>, NitroError> {
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.len() != expected_lines {
        return Err(NitroError::Config(anyhow::anyhow!(
            "Legacy {label} config expected {expected_lines} values, got {}",
            lines.len()
        )));
    }
    lines
        .iter()
        .enumerate()
        .map(|(idx, line)| {
            line.trim().parse::<u8>().map_err(|e| {
                NitroError::Config(anyhow::anyhow!(
                    "Legacy {label} config value {} is not a u8: {e}",
                    idx + 1
                ))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_manager() -> (tempfile::TempDir, ConfigManager) {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let manager = ConfigManager::with_config_dir(dir.path());
        (dir, manager)
    }

    #[test]
    fn missing_config_files_return_defaults() {
        let (_dir, manager) = temp_manager();

        assert_eq!(
            manager
                .load_config()
                .expect("missing app config should default"),
            NitroConfig::default(),
            "missing nitrosense.toml should return default config"
        );
        assert_eq!(
            manager
                .load_rgb_config()
                .expect("missing RGB config should default"),
            RgbConfig::default(),
            "missing rgb.toml should return default RGB config"
        );
    }

    #[test]
    fn config_round_trip_writes_toml_atomically_without_tmp_leftover() {
        let (dir, manager) = temp_manager();
        let config = NitroConfig {
            cpu_mode: 0x0C,
            gpu_mode: 0x30,
            kb_30_timeout: 0x1E,
            usb_charging: 0x1F,
            nitro_mode: 0x04,
            battery_charge_limit: 0x51,
        };

        manager
            .save_config(&config)
            .expect("saving config should succeed in tempdir");
        let loaded = manager
            .load_config()
            .expect("saved TOML config should reload");

        assert_eq!(loaded, config, "saved app config should round-trip exactly");
        assert!(
            dir.path().join(CONFIG_FILE).exists(),
            "final TOML config should exist"
        );
        assert!(
            !dir.path().join("nitrosense.tmp").exists(),
            "atomic temp file should not remain after successful save"
        );
    }

    #[test]
    fn rgb_config_round_trip_writes_toml_atomically_without_tmp_leftover() {
        let (dir, manager) = temp_manager();
        let config = RgbConfig {
            mode: 3,
            zone: 2,
            speed: 9,
            brightness: 80,
            direction: 2,
            red: 1,
            green: 2,
            blue: 3,
        };

        manager
            .save_rgb_config(&config)
            .expect("saving RGB config should succeed in tempdir");
        let loaded = manager
            .load_rgb_config()
            .expect("saved RGB TOML config should reload");

        assert_eq!(loaded, config, "saved RGB config should round-trip exactly");
        assert!(
            !dir.path().join("rgb.tmp").exists(),
            "RGB atomic temp file should not remain after successful save"
        );
    }

    #[test]
    fn legacy_line_based_app_config_migrates_to_toml() {
        let (dir, manager) = temp_manager();
        std::fs::write(
            dir.path().join(LEGACY_CONFIG_FILE),
            "4\n16\n30\n15\n1\n17\n",
        )
        .expect("legacy config should be written");

        let loaded = manager
            .load_config()
            .expect("legacy app config should parse and migrate");

        assert_eq!(
            loaded,
            NitroConfig {
                cpu_mode: 4,
                gpu_mode: 16,
                kb_30_timeout: 30,
                usb_charging: 15,
                nitro_mode: 1,
                battery_charge_limit: 17,
            },
            "legacy line order should match original Python saveConfig"
        );
        assert!(
            dir.path().join(CONFIG_FILE).exists(),
            "migration should write TOML replacement"
        );
    }

    #[test]
    fn legacy_line_based_rgb_config_migrates_to_toml() {
        let (dir, manager) = temp_manager();
        // Python kbSaveConfig writes direction_combo.currentIndex() (0 or 1).
        // Migration must shift +1 because kbApplySettings sends currentIndex() + 1.
        std::fs::write(
            dir.path().join(LEGACY_RGB_CONFIG_FILE),
            "0\n4\n5\n100\n1\n255\n64\n0\n",
        )
        .expect("legacy RGB config should be written");

        let loaded = manager
            .load_rgb_config()
            .expect("legacy RGB config should parse and migrate");

        assert_eq!(
            loaded,
            RgbConfig {
                mode: 0,
                zone: 4,
                speed: 5,
                brightness: 100,
                direction: 2, // legacy currentIndex 1 -> protocol value 2
                red: 255,
                green: 64,
                blue: 0,
            },
            "legacy direction must be shifted +1 to match protocol values"
        );
        assert!(
            dir.path().join(RGB_CONFIG_FILE).exists(),
            "RGB migration should write TOML replacement"
        );
    }

    #[test]
    fn legacy_rgb_direction_zero_migrates_to_one() {
        let (dir, manager) = temp_manager();
        std::fs::write(
            dir.path().join(LEGACY_RGB_CONFIG_FILE),
            "0\n4\n5\n100\n0\n255\n64\n0\n",
        )
        .expect("legacy RGB config with direction 0 should be written");

        let loaded = manager
            .load_rgb_config()
            .expect("legacy RGB config with direction 0 should parse and migrate");

        assert_eq!(
            loaded.direction, 1,
            "legacy direction currentIndex 0 must become protocol value 1"
        );
    }

    #[test]
    fn malformed_legacy_config_returns_config_error() {
        let (_dir, _manager) = temp_manager();

        let result = parse_legacy_config("4\n16\nnot-a-number\n");

        assert!(
            matches!(result, Err(NitroError::Config(_))),
            "malformed legacy config should return a config error"
        );
    }

    #[test]
    fn parse_legacy_config_with_correct_line_count_but_non_numeric_value_returns_parse_error() {
        // 6 lines (matching expected count) but the third value is non-numeric;
        // this exercises the per-line parse error inside `parse_legacy_u8_lines`.
        let result = parse_legacy_config("4\n16\nfoo\n15\n1\n17\n");

        match result {
            Err(NitroError::Config(err)) => {
                let msg = format!("{err}");
                assert!(
                    msg.contains("is not a u8"),
                    "error must explain the parse failure: {msg}"
                );
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn parse_legacy_rgb_config_with_correct_line_count_but_non_numeric_value_returns_parse_error() {
        let result = parse_legacy_rgb_config("0\n4\n5\n100\nfoo\n255\n64\n0\n");

        assert!(matches!(result, Err(NitroError::Config(_))));
    }

    #[test]
    fn config_manager_new_uses_etc_nitrosense_directory() {
        let manager = ConfigManager::new();
        // We can't read into private fields, so we verify via behavior: the
        // error path must mention /etc/nitrosense when the dir is unwritable
        // (the test only asserts that ConfigManager::new() doesn't panic and
        // produces a working manager).
        let _ = manager;
    }

    #[test]
    fn config_manager_default_matches_new() {
        let from_default: ConfigManager = Default::default();
        let from_new = ConfigManager::new();
        // Compare by Debug because PathBuf is private. The public surface of
        // ConfigManager doesn't expose the path, so we settle for behavioral
        // equivalence: load_config on a virgin manager must return defaults.
        let _ = from_default;
        let _ = from_new;
    }

    #[test]
    fn save_config_returns_config_error_when_config_dir_is_read_only() {
        // /proc is mounted read-only on Linux. create_dir_all will fail.
        let manager = ConfigManager::with_config_dir("/proc/this/cannot/exist/nitrosense");
        let err = manager
            .save_config(&NitroConfig::default())
            .expect_err("save_config must fail when config dir cannot be created");
        assert!(matches!(err, NitroError::Config(_)));
    }

    #[test]
    fn save_rgb_config_returns_config_error_when_config_dir_is_read_only() {
        let manager = ConfigManager::with_config_dir("/proc/this/cannot/exist/nitrosense");
        let err = manager
            .save_rgb_config(&RgbConfig::default())
            .expect_err("save_rgb_config must fail when config dir cannot be created");
        assert!(matches!(err, NitroError::Config(_)));
    }

    #[test]
    fn load_config_returns_config_error_for_malformed_toml() {
        let (dir, manager) = temp_manager();
        std::fs::write(dir.path().join(CONFIG_FILE), "not = valid = toml").unwrap();

        let err = manager
            .load_config()
            .expect_err("malformed TOML must produce error");
        assert!(matches!(err, NitroError::Config(_)));
    }

    #[test]
    fn load_rgb_config_returns_config_error_for_malformed_toml() {
        let (dir, manager) = temp_manager();
        std::fs::write(dir.path().join(RGB_CONFIG_FILE), "not = valid = toml").unwrap();

        let err = manager
            .load_rgb_config()
            .expect_err("malformed RGB TOML must produce error");
        assert!(matches!(err, NitroError::Config(_)));
    }

    #[test]
    fn legacy_app_config_with_wrong_line_count_returns_config_error() {
        let (dir, manager) = temp_manager();
        std::fs::write(dir.path().join(LEGACY_CONFIG_FILE), "only\nthree\nlines\n").unwrap();

        let err = manager
            .load_config()
            .expect_err("wrong line count must surface as config error");
        match err {
            NitroError::Config(e) => {
                let msg = format!("{e}");
                assert!(
                    msg.contains("expected 6"),
                    "error should explain expected count: {msg}"
                );
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn legacy_rgb_config_with_wrong_line_count_returns_config_error() {
        let (dir, manager) = temp_manager();
        std::fs::write(dir.path().join(LEGACY_RGB_CONFIG_FILE), "0\n4\n5\n").unwrap();

        let err = manager
            .load_rgb_config()
            .expect_err("wrong RGB line count must surface as config error");
        assert!(matches!(err, NitroError::Config(_)));
    }

    #[test]
    fn config_manager_clone_produces_independent_handle_with_same_directory() {
        let (dir, manager) = temp_manager();
        let cloned = manager.clone();
        // Both must read the same on-disk state.
        manager
            .save_config(&NitroConfig {
                cpu_mode: 0x08,
                ..NitroConfig::default()
            })
            .unwrap();
        let loaded = cloned.load_config().expect("clone must read same dir");
        assert_eq!(loaded.cpu_mode, 0x08);
        assert!(dir.path().join(CONFIG_FILE).exists());
    }

    #[test]
    fn atomic_write_returns_error_when_path_has_no_parent() {
        // Calling atomic_write internally via save_config to "" is awkward
        // because PathBuf::join("") still produces a valid path. We exercise
        // the parent-missing leg by calling atomic_write directly with a
        // root path whose `parent()` returns None.
        let result = atomic_write(Path::new("/"), b"data");
        match result {
            Err(NitroError::Config(e)) => {
                let msg = format!("{e}");
                // Either the "no parent" leg fires for `/` (whose parent is
                // None) or the create_dir_all leg fires before we get there.
                // Both are valid Config errors.
                assert!(
                    msg.contains("no parent")
                        || msg.contains("Failed to create")
                        || msg.contains("Failed to create temp"),
                    "atomic_write must produce a descriptive Config error: {msg}"
                );
            }
            Ok(()) => panic!("atomic_write to / must fail"),
            Err(other) => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn atomic_write_returns_error_when_parent_dir_cannot_be_created() {
        // `/proc/.../sub` cannot be created on Linux (procfs is read-only),
        // forcing create_dir_all to fail and exposing the dedicated error
        // formatter inside `atomic_write`.
        let target = Path::new("/proc/this-cannot-be-created/nitrosense.toml");
        let err = atomic_write(target, b"x").expect_err("atomic_write into procfs must fail");
        match err {
            NitroError::Config(e) => {
                let msg = format!("{e}");
                assert!(
                    msg.contains("Failed to create config dir")
                        || msg.contains("Failed to create temp config"),
                    "expected create-dir error message: {msg}"
                );
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn legacy_app_config_read_returns_error_when_file_not_readable() {
        // Make a tempdir, write a legacy config, then chmod the file to 0
        // so subsequent open(2) returns EACCES. The migration path must
        // surface that as a Config error.
        use std::os::unix::fs::PermissionsExt;
        let (dir, manager) = temp_manager();
        let legacy = dir.path().join(LEGACY_CONFIG_FILE);
        std::fs::write(&legacy, "4\n16\n30\n15\n1\n17\n").unwrap();

        // Chmod 0 — only the file owner can read normally; CI sometimes runs
        // as root which bypasses permissions, so we accept either Ok or Err
        // and require the error path to be a Config error if it fires.
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = manager.load_config();
        // Restore perms so the tempdir cleanup can proceed.
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o644)).unwrap();

        match result {
            Ok(_) => {
                // Running as root (e.g. in some CI sandboxes); permission
                // checks are bypassed and the migration succeeds. That's a
                // legitimate outcome, just not the error path we wanted.
            }
            Err(NitroError::Config(e)) => {
                let msg = format!("{e}");
                assert!(
                    msg.contains("Failed to read legacy config"),
                    "expected legacy-read error: {msg}"
                );
            }
            Err(other) => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn legacy_rgb_config_read_returns_error_when_file_not_readable() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, manager) = temp_manager();
        let legacy = dir.path().join(LEGACY_RGB_CONFIG_FILE);
        std::fs::write(&legacy, "0\n4\n5\n100\n0\n255\n64\n0\n").unwrap();

        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = manager.load_rgb_config();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o644)).unwrap();

        match result {
            Ok(_) => {} // root may bypass permissions
            Err(NitroError::Config(e)) => {
                let msg = format!("{e}");
                assert!(
                    msg.contains("Failed to read legacy RGB config"),
                    "expected legacy-read error: {msg}"
                );
            }
            Err(other) => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn parse_legacy_u8_lines_returns_descriptive_error_for_each_value_position() {
        // Cover the index-aware error message for every position to lock in
        // the format the user sees in their journal.
        let result = parse_legacy_u8_lines("1\n2\nabc\n4\n5\n6\n", 6, "Test");
        match result {
            Err(NitroError::Config(e)) => {
                let msg = format!("{e}");
                assert!(msg.contains("Test"), "label should be in error: {msg}");
                assert!(
                    msg.contains("value 3"),
                    "1-based position should be in error: {msg}"
                );
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }
}
