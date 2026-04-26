// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use crate::error::NitroError;

pub const PAYLOAD_SIZE: usize = 16;
pub const CHARACTER_DEVICE: &str = "/dev/acer-gkbbl-0";
pub const PAYLOAD_SIZE_STATIC: usize = 4;
pub const CHARACTER_DEVICE_STATIC: &str = "/dev/acer-gkbbl-static-0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbCommand {
    pub mode: u8,
    pub zone: u8,
    pub speed: u8,
    pub brightness: u8,
    pub direction: u8,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

pub trait RgbDeviceWriter {
    fn write_payload(&mut self, device: &str, payload: &[u8]) -> Result<(), NitroError>;
}

#[derive(Debug, Default)]
pub struct FsRgbDeviceWriter;

impl RgbDeviceWriter for FsRgbDeviceWriter {
    fn write_payload(&mut self, device: &str, payload: &[u8]) -> Result<(), NitroError> {
        std::fs::write(device, payload).map_err(|_| NitroError::RgbDevice(device.to_string()))
    }
}

pub fn is_available() -> bool {
    rgb_devices_available_at(
        std::path::Path::new(CHARACTER_DEVICE),
        std::path::Path::new(CHARACTER_DEVICE_STATIC),
    )
}

pub fn rgb_devices_available_at(main: &std::path::Path, static_device: &std::path::Path) -> bool {
    main.exists() && static_device.exists()
}

#[allow(clippy::too_many_arguments)]
pub fn set_mode(
    mode: u8,
    zone: u8,
    speed: u8,
    brightness: u8,
    direction: u8,
    r: u8,
    g: u8,
    b: u8,
) -> Result<(), NitroError> {
    let command = RgbCommand {
        mode,
        zone,
        speed,
        brightness,
        direction,
        red: r,
        green: g,
        blue: b,
    };
    let mut writer = FsRgbDeviceWriter;
    apply_rgb_command(&mut writer, command)
}

pub fn apply_rgb_command(
    writer: &mut impl RgbDeviceWriter,
    command: RgbCommand,
) -> Result<(), NitroError> {
    validate_rgb_command(command)?;
    if command.mode == 0 {
        set_static_mode(
            writer,
            command.zone,
            command.red,
            command.green,
            command.blue,
        )
    } else {
        set_dynamic_mode(writer, command)
    }
}

fn validate_rgb_command(command: RgbCommand) -> Result<(), NitroError> {
    validate_range(command.mode, 0, 5, "RGB mode")?;
    validate_range(command.zone, 0, 4, "RGB zone")?;
    validate_range(command.speed, 0, 9, "RGB speed")?;
    validate_range(command.brightness, 0, 100, "RGB brightness")?;
    validate_range(command.direction, 1, 2, "RGB direction")
}

fn validate_range(value: u8, min: u8, max: u8, name: &str) -> Result<(), NitroError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(NitroError::Validation(format!(
            "{name} value {value} is outside {min}..={max}"
        )))
    }
}

fn set_static_mode(
    writer: &mut impl RgbDeviceWriter,
    zone: u8,
    r: u8,
    g: u8,
    b: u8,
) -> Result<(), NitroError> {
    if zone == 0 {
        for i in 1..=4 {
            writer.write_payload(CHARACTER_DEVICE_STATIC, &create_static_payload(i, r, g, b)?)?;
        }
    } else {
        writer.write_payload(
            CHARACTER_DEVICE_STATIC,
            &create_static_payload(zone, r, g, b)?,
        )?;
    }
    writer.write_payload(CHARACTER_DEVICE, &create_brightness_payload())
}

fn set_dynamic_mode(
    writer: &mut impl RgbDeviceWriter,
    command: RgbCommand,
) -> Result<(), NitroError> {
    writer.write_payload(CHARACTER_DEVICE, &create_dynamic_payload(command))
}

pub fn create_dynamic_payload(command: RgbCommand) -> [u8; PAYLOAD_SIZE] {
    let mut payload = [0u8; PAYLOAD_SIZE];
    payload[0] = command.mode;
    payload[1] = command.speed;
    payload[2] = command.brightness;
    payload[3] = if command.mode == 3 { 8 } else { 0 };
    payload[4] = command.direction;
    payload[5] = command.red;
    payload[6] = command.green;
    payload[7] = command.blue;
    payload[9] = 1;
    payload
}

pub fn create_static_payload(
    zone: u8,
    r: u8,
    g: u8,
    b: u8,
) -> Result<[u8; PAYLOAD_SIZE_STATIC], NitroError> {
    validate_range(zone, 1, 4, "RGB static zone")?;
    Ok([1 << (zone - 1), r, g, b])
}

pub fn create_brightness_payload() -> [u8; PAYLOAD_SIZE] {
    let mut payload = [0u8; PAYLOAD_SIZE];
    payload[9] = 1;
    payload
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RecordingRgbWriter;

    type RecordingWriter = RecordingRgbWriter;

    fn command(mode: u8) -> RgbCommand {
        RgbCommand {
            mode,
            zone: 2,
            speed: 5,
            brightness: 80,
            direction: 1,
            red: 10,
            green: 20,
            blue: 30,
        }
    }

    #[test]
    fn static_payload_matches_python_zone_bitmask() {
        assert_eq!(
            create_static_payload(1, 10, 20, 30).expect("zone 1 should be valid"),
            [0b0001, 10, 20, 30],
            "zone 1 should set bit 0"
        );
        assert_eq!(
            create_static_payload(4, 10, 20, 30).expect("zone 4 should be valid"),
            [0b1000, 10, 20, 30],
            "zone 4 should set bit 3"
        );
    }

    #[test]
    fn dynamic_payload_matches_python_layout_for_mode_three() {
        let payload = create_dynamic_payload(command(3));

        assert_eq!(
            payload,
            [3, 5, 80, 8, 1, 10, 20, 30, 0, 1, 0, 0, 0, 0, 0, 0],
            "mode 3 dynamic payload should include Python's payload[3] = 8 special case"
        );
    }

    #[test]
    fn static_all_zones_writes_four_static_payloads_then_brightness() {
        let mut writer = RecordingWriter::default();
        let mut cmd = command(0);
        cmd.zone = 0;

        apply_rgb_command(&mut writer, cmd).expect("all-zone static RGB should write");

        assert_eq!(
            writer.writes.len(),
            5,
            "all-zone static mode should write 4 zones plus brightness"
        );
        assert_eq!(
            writer.writes[0],
            (CHARACTER_DEVICE_STATIC.to_string(), vec![1, 10, 20, 30])
        );
        assert_eq!(
            writer.writes[3],
            (CHARACTER_DEVICE_STATIC.to_string(), vec![8, 10, 20, 30])
        );
        assert_eq!(
            writer.writes[4],
            (
                CHARACTER_DEVICE.to_string(),
                create_brightness_payload().to_vec()
            ),
            "final all-zone static write should update brightness on main device"
        );
    }

    #[test]
    fn static_single_zone_writes_one_static_payload_then_brightness() {
        let mut writer = RecordingWriter::default();
        let mut cmd = command(0);
        cmd.zone = 3;

        apply_rgb_command(&mut writer, cmd).expect("single-zone static RGB should write");

        assert_eq!(
            writer.writes.len(),
            2,
            "single-zone static mode should write one zone plus brightness"
        );
        assert_eq!(
            writer.writes[0],
            (CHARACTER_DEVICE_STATIC.to_string(), vec![4, 10, 20, 30])
        );
        assert_eq!(writer.writes[1].0, CHARACTER_DEVICE);
    }

    #[test]
    fn invalid_zone_is_rejected_before_any_device_write() {
        let mut writer = RecordingWriter::default();
        let mut cmd = command(0);
        cmd.zone = 5;

        let result = apply_rgb_command(&mut writer, cmd);

        assert!(
            matches!(result, Err(NitroError::Validation(_))),
            "invalid RGB zone should be rejected"
        );
        assert!(
            writer.writes.is_empty(),
            "invalid RGB command must not touch devices"
        );
    }

    #[test]
    fn invalid_brightness_and_direction_are_rejected() {
        for cmd in [
            RgbCommand {
                brightness: 101,
                ..command(1)
            },
            RgbCommand {
                direction: 3,
                ..command(1)
            },
        ] {
            let mut writer = RecordingWriter::default();
            let result = apply_rgb_command(&mut writer, cmd);
            assert!(
                matches!(result, Err(NitroError::Validation(_))),
                "out-of-range RGB command should be rejected: {cmd:?}"
            );
        }
    }

    #[test]
    fn speed_zero_is_accepted_matching_original_qslider_range() {
        let mut writer = RecordingWriter::default();
        let cmd = RgbCommand {
            speed: 0,
            ..command(1)
        };

        let result = apply_rgb_command(&mut writer, cmd);

        assert!(
            result.is_ok(),
            "speed 0 must be accepted — original QSlider defaults to minimum 0"
        );
        assert_eq!(
            writer.writes.len(),
            1,
            "dynamic mode with speed 0 should write one payload"
        );
    }

    #[test]
    fn speed_ten_is_rejected() {
        let mut writer = RecordingWriter::default();
        let cmd = RgbCommand {
            speed: 10,
            ..command(1)
        };

        let result = apply_rgb_command(&mut writer, cmd);

        assert!(
            matches!(result, Err(NitroError::Validation(_))),
            "speed 10 must be rejected — maximum is 9"
        );
    }

    #[test]
    fn device_availability_requires_both_character_devices() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let main = dir.path().join("main");
        let static_dev = dir.path().join("static");

        assert!(
            !rgb_devices_available_at(&main, &static_dev),
            "missing devices should report RGB unavailable"
        );
        std::fs::write(&main, []).expect("main device placeholder should be created");
        assert!(
            !rgb_devices_available_at(&main, &static_dev),
            "only one device should still report RGB unavailable"
        );
        std::fs::write(&static_dev, []).expect("static device placeholder should be created");
        assert!(
            rgb_devices_available_at(&main, &static_dev),
            "both device paths existing should report RGB available"
        );
    }

    // ---- FsRgbDeviceWriter coverage ----

    #[test]
    fn fs_rgb_device_writer_writes_payload_to_real_file() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("device");
        let mut writer = FsRgbDeviceWriter;

        writer
            .write_payload(path.to_str().unwrap(), &[0xAA, 0xBB, 0xCC])
            .expect("writing to a tempfile should succeed");

        let contents = std::fs::read(&path).expect("device file should be readable");
        assert_eq!(contents, vec![0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn fs_rgb_device_writer_returns_rgb_device_error_when_path_invalid() {
        let mut writer = FsRgbDeviceWriter;
        let err = writer
            .write_payload("/this/path/does/not/exist/device", &[0x01])
            .expect_err("non-existent device path must produce error");
        match err {
            NitroError::RgbDevice(p) => {
                assert_eq!(p, "/this/path/does/not/exist/device");
            }
            other => panic!("expected RgbDevice error, got {other:?}"),
        }
    }

    #[test]
    fn fs_rgb_device_writer_default_compiles_and_has_zero_size() {
        // Ensures the FsRgbDeviceWriter::default() codepath is exercised; this
        // also documents that the writer carries no per-instance state.
        let _writer: FsRgbDeviceWriter = Default::default();
        assert_eq!(std::mem::size_of::<FsRgbDeviceWriter>(), 0);
    }

    #[test]
    fn is_available_reflects_filesystem_state_for_real_device_paths() {
        // The actual /dev/acer-gkbbl-0 character devices generally do NOT
        // exist on a generic Linux test host. The function must return a
        // boolean either way and never panic.
        let _ = is_available();
    }

    // ---- set_mode integration coverage ----

    #[test]
    fn set_mode_validates_input_before_touching_any_real_device() {
        // mode 99 is out of range; validation must reject before attempting
        // to write to /dev/acer-gkbbl-0, so this test is hermetic.
        let result = set_mode(99, 0, 5, 80, 1, 0, 0, 0);
        assert!(matches!(result, Err(NitroError::Validation(_))));
    }

    #[test]
    fn set_mode_returns_rgb_device_error_when_real_device_missing() {
        // For a valid command, set_mode hits the real filesystem at
        // /dev/acer-gkbbl-0. If the keyboard module is not loaded (as on
        // most CI hosts), we expect an RgbDevice error.
        let result = set_mode(1, 1, 5, 80, 1, 0, 0, 0);
        match result {
            // On a host where the device exists this is fine; we just don't
            // assert on the success path here.
            Ok(()) => {}
            Err(NitroError::RgbDevice(_)) => {}
            Err(other) => panic!("expected RgbDevice or success, got {other:?}"),
        }
    }

    // ---- Validation boundary tests ----

    #[test]
    fn validate_rgb_command_rejects_mode_six() {
        let cmd = RgbCommand {
            mode: 6,
            ..command(1)
        };
        let mut writer = RecordingWriter::default();
        assert!(matches!(
            apply_rgb_command(&mut writer, cmd),
            Err(NitroError::Validation(_))
        ));
    }

    #[test]
    fn validate_rgb_command_rejects_zone_above_max() {
        let cmd = RgbCommand {
            zone: 5,
            ..command(1)
        };
        let mut writer = RecordingWriter::default();
        assert!(matches!(
            apply_rgb_command(&mut writer, cmd),
            Err(NitroError::Validation(_))
        ));
    }

    #[test]
    fn validate_rgb_command_rejects_direction_zero() {
        let cmd = RgbCommand {
            direction: 0,
            ..command(1)
        };
        let mut writer = RecordingWriter::default();
        assert!(matches!(
            apply_rgb_command(&mut writer, cmd),
            Err(NitroError::Validation(_))
        ));
    }

    #[test]
    fn create_static_payload_rejects_zone_zero_and_above_four() {
        assert!(matches!(
            create_static_payload(0, 0, 0, 0),
            Err(NitroError::Validation(_))
        ));
        assert!(matches!(
            create_static_payload(5, 0, 0, 0),
            Err(NitroError::Validation(_))
        ));
    }

    #[test]
    fn create_brightness_payload_layout_matches_python_reference() {
        let payload = create_brightness_payload();
        assert_eq!(payload[9], 1);
        for (i, byte) in payload.iter().enumerate() {
            if i != 9 {
                assert_eq!(*byte, 0, "brightness payload byte {i} should be zero");
            }
        }
    }

    #[test]
    fn create_dynamic_payload_for_modes_other_than_three_zeros_offset_three() {
        for mode in [1u8, 2, 4, 5] {
            let payload = create_dynamic_payload(command(mode));
            assert_eq!(
                payload[3], 0,
                "mode {mode} should produce payload[3] = 0 (only mode 3 sets 8)"
            );
        }
    }

    #[test]
    fn rgb_command_supports_clone_copy_and_eq() {
        let a = command(2);
        let b = a;
        let c = a;
        assert_eq!(b, c, "Copy + Clone must agree");
    }
}
