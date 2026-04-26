// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

pub mod bindings;

use std::ffi::c_int;
use std::io;

use crate::error::NitroError;

pub use bindings::EcHandle;

pub struct RawEcDevice {
    handle: EcHandle,
    opened: bool,
}

impl RawEcDevice {
    pub fn new() -> Self {
        Self {
            handle: EcHandle {
                fd: -1,
                uses_ec_sys: false,
            },
            opened: false,
        }
    }

    pub fn open(&mut self) -> Result<(), NitroError> {
        let rc = unsafe {
            // SAFETY: `self.handle` is a valid, uniquely borrowed repr(C) output pointer.
            bindings::ec_open(&mut self.handle)
        };
        if rc < 0 {
            return Err(NitroError::EcOpen(errno_to_io(rc)));
        }
        self.opened = true;
        Ok(())
    }

    pub fn refresh(&mut self, buffer: &mut [u8]) -> Result<usize, NitroError> {
        let rc = unsafe {
            // SAFETY: `self.handle` is initialized by `ec_open` before production use; `buffer`
            // is a valid mutable byte slice and its pointer/length are passed unchanged.
            bindings::ec_refresh(&mut self.handle, buffer.as_mut_ptr(), buffer.len())
        };
        if rc < 0 {
            return Err(NitroError::EcRefresh(errno_to_io(rc)));
        }
        Ok(rc as usize)
    }

    pub fn write_byte(&mut self, addr: u8, val: u8) -> Result<(), NitroError> {
        let rc = unsafe {
            // SAFETY: `self.handle` is initialized by `ec_open` before production use; the C
            // function copies primitive address/value arguments and does not retain references.
            bindings::ec_write_byte(&mut self.handle, addr, val)
        };
        if rc < 0 {
            return Err(NitroError::EcWrite {
                addr,
                source: errno_to_io(rc),
            });
        }
        Ok(())
    }

    pub fn close(&mut self) {
        if self.opened {
            unsafe {
                // SAFETY: Closing an initialized C handle is idempotently guarded by `opened`;
                // the C side also accepts invalid/closed handles defensively.
                bindings::ec_close(&mut self.handle);
            }
            self.opened = false;
        }
    }
}

impl Default for RawEcDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RawEcDevice {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn msr_close_fd(fd: i32) {
    unsafe {
        // SAFETY: `msr_close` accepts any integer fd and ignores negative fds on the C side.
        bindings::msr_close(fd as c_int);
    }
}

fn errno_to_io(code: c_int) -> io::Error {
    io::Error::from_raw_os_error(-code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ec_syscall_mock::{MOCK_STATE, install_mocks, msr_write};
    use serial_test::serial;
    use std::mem::{align_of, size_of};

    #[test]
    fn ec_handle_layout_matches_c_static_assert_contract() {
        assert_eq!(
            size_of::<EcHandle>(),
            8,
            "Rust EcHandle size must match C struct ec_handle static_assert"
        );
        assert_eq!(
            align_of::<EcHandle>(),
            align_of::<c_int>(),
            "Rust EcHandle alignment should be driven by c_int, matching C layout"
        );
    }

    #[test]
    fn errno_to_io_negates_negative_errno_for_io_error() {
        let err = errno_to_io(-libc::EACCES);
        assert_eq!(
            err.raw_os_error(),
            Some(libc::EACCES),
            "errno_to_io should restore the positive errno"
        );
    }

    #[test]
    fn errno_to_io_zero_returns_success_marker() {
        let err = errno_to_io(0);
        assert_eq!(
            err.raw_os_error(),
            Some(0),
            "errno_to_io(0) should produce an io::Error with raw_os_error = 0"
        );
    }

    #[test]
    fn raw_ec_device_default_matches_new_for_idiomatic_initialization() {
        let from_default = RawEcDevice::default();
        let from_new = RawEcDevice::new();
        assert_eq!(from_default.handle.fd, from_new.handle.fd);
        assert_eq!(from_default.handle.uses_ec_sys, from_new.handle.uses_ec_sys);
        assert_eq!(from_default.opened, from_new.opened);
    }

    #[test]
    fn raw_ec_device_new_initializes_handle_to_sentinel_state() {
        let device = RawEcDevice::new();
        assert_eq!(device.handle.fd, -1);
        assert!(!device.handle.uses_ec_sys);
        assert!(!device.opened);
    }

    #[test]
    #[serial]
    fn raw_ec_device_open_flips_opened_flag_on_mock_success() {
        let _guard = install_mocks();
        let mut device = RawEcDevice::new();

        device
            .open()
            .expect("RawEcDevice::open should succeed via mock ec_sys path");

        assert!(device.opened, "successful open must mark device opened");
        assert!(device.handle.fd >= 3, "fd must be a valid mock fd");
        assert!(device.handle.uses_ec_sys, "ec_sys path should be selected");
    }

    #[test]
    #[serial]
    fn raw_ec_device_open_propagates_negative_errno_as_ec_open_error() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.open_results.push(-libc::ENOENT);
            s.open_results.push(-libc::EACCES);
        }
        let mut device = RawEcDevice::new();

        let err = device
            .open()
            .expect_err("RawEcDevice::open must surface failure when both paths fail");

        match err {
            NitroError::EcOpen(io_err) => {
                assert_eq!(io_err.raw_os_error(), Some(libc::EACCES));
            }
            other => panic!("expected NitroError::EcOpen, got {other:?}"),
        }
        assert!(!device.opened, "failed open must not flip the opened flag");
    }

    #[test]
    #[serial]
    fn raw_ec_device_refresh_returns_byte_count_on_mock_success() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.read_payloads.push(vec![0xCC; 256]);
        }
        let mut device = RawEcDevice {
            handle: EcHandle {
                fd: 17,
                uses_ec_sys: true,
            },
            opened: true,
        };
        let mut buffer = [0u8; 256];

        let n = device
            .refresh(&mut buffer)
            .expect("refresh should succeed against mocked read");

        assert_eq!(n, 256, "refresh must report the bytes read");
        assert!(buffer.iter().all(|&b| b == 0xCC));
    }

    #[test]
    #[serial]
    fn raw_ec_device_refresh_maps_negative_errno_to_ec_refresh_error() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.read_errors.push(libc::EIO);
        }
        let mut device = RawEcDevice {
            handle: EcHandle {
                fd: 17,
                uses_ec_sys: true,
            },
            opened: true,
        };
        let mut buffer = [0u8; 16];

        let err = device
            .refresh(&mut buffer)
            .expect_err("refresh must propagate read errno");

        match err {
            NitroError::EcRefresh(io_err) => {
                assert_eq!(io_err.raw_os_error(), Some(libc::EIO));
            }
            other => panic!("expected NitroError::EcRefresh, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn raw_ec_device_write_byte_succeeds_with_default_mock_short_write_off() {
        let _guard = install_mocks();
        let mut device = RawEcDevice {
            handle: EcHandle {
                fd: 7,
                uses_ec_sys: true,
            },
            opened: true,
        };

        device
            .write_byte(0x22, 0x0C)
            .expect("write_byte should succeed against default mock");

        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(s.lseeks, vec![(7, 0x22, libc::SEEK_SET)]);
        assert_eq!(s.writes, vec![(7, vec![0x0C])]);
    }

    #[test]
    #[serial]
    fn raw_ec_device_write_byte_propagates_negative_errno_as_ec_write_error() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.write_results.push(-libc::EIO as isize);
        }
        let mut device = RawEcDevice {
            handle: EcHandle {
                fd: 7,
                uses_ec_sys: true,
            },
            opened: true,
        };

        let err = device
            .write_byte(0x22, 0xCC)
            .expect_err("write_byte must propagate write errno");

        match err {
            NitroError::EcWrite { addr, source } => {
                assert_eq!(
                    addr, 0x22,
                    "EcWrite must carry the failing register address"
                );
                assert_eq!(source.raw_os_error(), Some(libc::EIO));
            }
            other => panic!("expected NitroError::EcWrite, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn raw_ec_device_close_only_calls_c_close_when_opened_flag_set() {
        let _guard = install_mocks();
        let mut device = RawEcDevice {
            handle: EcHandle {
                fd: 21,
                uses_ec_sys: true,
            },
            opened: true,
        };

        device.close();

        assert!(!device.opened, "close must clear the opened flag");
        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(s.closes, vec![21]);
    }

    #[test]
    #[serial]
    fn raw_ec_device_close_is_idempotent_for_unopened_device() {
        let _guard = install_mocks();
        let mut device = RawEcDevice::new();

        device.close();
        device.close();

        let s = MOCK_STATE.lock().unwrap();
        assert!(
            s.closes.is_empty(),
            "close on never-opened device must not invoke C close"
        );
    }

    #[test]
    #[serial]
    fn raw_ec_device_drop_calls_close_for_opened_device() {
        let _guard = install_mocks();
        {
            let _device = RawEcDevice {
                handle: EcHandle {
                    fd: 33,
                    uses_ec_sys: true,
                },
                opened: true,
            };
            // _device dropped here -> Drop -> close()
        }

        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(
            s.closes,
            vec![33],
            "Drop must close opened devices exactly once"
        );
    }

    #[test]
    #[serial]
    fn raw_ec_device_drop_skips_close_for_never_opened_device() {
        let _guard = install_mocks();
        {
            let _device = RawEcDevice::new();
            // _device dropped here -> Drop -> close() guarded by `opened == false`
        }
        let s = MOCK_STATE.lock().unwrap();
        assert!(
            s.closes.is_empty(),
            "Drop must not close devices that were never opened"
        );
    }

    #[test]
    #[serial]
    fn msr_close_fd_forwards_to_c_close_through_msr_close() {
        let _guard = install_mocks();

        msr_close_fd(42);

        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(
            s.closes,
            vec![42],
            "msr_close_fd must call C close on the fd"
        );
    }

    #[test]
    #[serial]
    fn msr_close_fd_with_negative_fd_is_skipped_by_c_layer() {
        let _guard = install_mocks();

        msr_close_fd(-1);

        let s = MOCK_STATE.lock().unwrap();
        assert!(
            s.closes.is_empty(),
            "msr_close on negative fd must be a no-op per C contract"
        );
    }

    // ----- Direct C-binding mock tests (Step 12.2 — preserved from the
    // original suite to keep the C layer assertions coupled to the FFI
    // layer rather than only to the higher-level Rust wrappers). -----

    #[test]
    #[serial]
    fn ec_open_succeeds_via_ec_sys_path_and_records_modprobe_calls() {
        let _guard = install_mocks();
        let mut handle = EcHandle {
            fd: -1,
            uses_ec_sys: false,
        };

        let rc = unsafe { bindings::ec_open(&mut handle) };

        assert_eq!(rc, 0, "ec_open should succeed when ec_sys path opens");
        assert!(
            handle.uses_ec_sys,
            "ec_open should mark handle as ec_sys-backed"
        );
        assert!(handle.fd >= 3, "ec_open should expose the mock fd");

        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(
            s.opens.len(),
            1,
            "ec_open should try ec_sys before acpi_ec on success"
        );
        assert!(
            s.opens[0].0.contains("ec0/io"),
            "ec_open must request /sys/kernel/debug/ec/ec0/io first"
        );
        assert!(
            s.systems.iter().any(|c| c.contains("modprobe -r ec_sys")),
            "ec_open must unload ec_sys before reload"
        );
        assert!(
            s.systems.iter().any(|c| c.contains("write_support=y")),
            "ec_open must reload ec_sys with write_support=y"
        );
    }

    #[test]
    #[serial]
    fn ec_open_falls_back_to_acpi_ec_when_ec_sys_fails() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.open_results.push(-libc::ENOENT);
        }
        let mut handle = EcHandle {
            fd: -1,
            uses_ec_sys: true,
        };

        let rc = unsafe { bindings::ec_open(&mut handle) };

        assert_eq!(
            rc, 0,
            "ec_open should fall back to /dev/ec when ec_sys is unavailable"
        );
        assert!(!handle.uses_ec_sys, "fallback path must clear uses_ec_sys");

        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(s.opens.len(), 2, "ec_open should try both EC paths");
        assert!(s.opens[1].0.contains("/dev/ec"));
        assert!(
            s.systems.iter().any(|c| c.contains("modprobe acpi_ec")),
            "ec_open must request acpi_ec to be loaded before fallback open"
        );
    }

    #[test]
    #[serial]
    fn ec_open_returns_negative_errno_when_both_paths_fail() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.open_results.push(-libc::ENOENT);
            s.open_results.push(-libc::EACCES);
        }
        let mut handle = EcHandle {
            fd: 99,
            uses_ec_sys: true,
        };

        let rc = unsafe { bindings::ec_open(&mut handle) };

        assert_eq!(
            rc,
            -libc::EACCES,
            "ec_open must propagate -errno of last failure"
        );
    }

    #[test]
    #[serial]
    fn ec_open_with_null_out_returns_einval() {
        let _guard = install_mocks();
        let rc = unsafe { bindings::ec_open(std::ptr::null_mut()) };
        assert_eq!(
            rc,
            -libc::EINVAL,
            "ec_open(NULL) must return -EINVAL per C precondition"
        );
    }

    #[test]
    #[serial]
    fn ec_refresh_reads_full_buffer_via_lseek_then_read() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.read_payloads.push(vec![0xAA; 256]);
        }
        let mut handle = EcHandle {
            fd: 7,
            uses_ec_sys: true,
        };
        let mut buffer = [0u8; 256];

        let rc = unsafe { bindings::ec_refresh(&mut handle, buffer.as_mut_ptr(), buffer.len()) };

        assert_eq!(rc, 256, "ec_refresh should return the byte count read");
        assert!(buffer.iter().all(|&b| b == 0xAA));

        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(
            s.lseeks,
            vec![(7, 0, libc::SEEK_SET)],
            "ec_refresh must seek to offset 0 before reading"
        );
        assert_eq!(s.reads, vec![(7, 256)]);
    }

    #[test]
    #[serial]
    fn ec_refresh_returns_einval_for_null_buffer() {
        let _guard = install_mocks();
        let mut handle = EcHandle {
            fd: 7,
            uses_ec_sys: true,
        };

        let rc = unsafe { bindings::ec_refresh(&mut handle, std::ptr::null_mut(), 16) };

        assert_eq!(rc, -libc::EINVAL);
    }

    #[test]
    #[serial]
    fn ec_refresh_returns_einval_for_zero_length_or_bad_handle() {
        let _guard = install_mocks();
        let mut buffer = [0u8; 16];

        // zero length
        let mut handle = EcHandle {
            fd: 7,
            uses_ec_sys: true,
        };
        let rc_zero = unsafe { bindings::ec_refresh(&mut handle, buffer.as_mut_ptr(), 0) };
        assert_eq!(
            rc_zero,
            -libc::EINVAL,
            "len=0 must short-circuit to -EINVAL"
        );

        // negative fd
        let mut handle_bad = EcHandle {
            fd: -1,
            uses_ec_sys: true,
        };
        let rc_bad = unsafe { bindings::ec_refresh(&mut handle_bad, buffer.as_mut_ptr(), 16) };
        assert_eq!(rc_bad, -libc::EINVAL, "fd<0 must short-circuit to -EINVAL");
    }

    #[test]
    #[serial]
    fn ec_refresh_propagates_lseek_errno_negation() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.lseek_errors.push(libc::ESPIPE);
        }
        let mut handle = EcHandle {
            fd: 7,
            uses_ec_sys: true,
        };
        let mut buffer = [0u8; 16];

        let rc = unsafe { bindings::ec_refresh(&mut handle, buffer.as_mut_ptr(), buffer.len()) };

        assert_eq!(
            rc,
            -libc::ESPIPE,
            "lseek failure must surface as -errno from ec_refresh"
        );
    }

    #[test]
    #[serial]
    fn ec_refresh_propagates_read_errno_negation() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.read_errors.push(libc::EIO);
        }
        let mut handle = EcHandle {
            fd: 7,
            uses_ec_sys: true,
        };
        let mut buffer = [0u8; 16];

        let rc = unsafe { bindings::ec_refresh(&mut handle, buffer.as_mut_ptr(), buffer.len()) };

        assert_eq!(
            rc,
            -libc::EIO,
            "read failure must surface as -errno from ec_refresh"
        );
    }

    #[test]
    #[serial]
    fn ec_write_byte_seeks_then_writes_single_byte() {
        let _guard = install_mocks();
        let mut handle = EcHandle {
            fd: 11,
            uses_ec_sys: true,
        };

        let rc = unsafe { bindings::ec_write_byte(&mut handle, 0x22, 0x0C) };
        assert_eq!(rc, 0);

        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(s.lseeks, vec![(11, 0x22, libc::SEEK_SET)]);
        assert_eq!(s.writes, vec![(11, vec![0x0C])]);
    }

    #[test]
    #[serial]
    fn ec_write_byte_returns_negative_errno_on_short_write() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.write_results.push(0);
        }
        let mut handle = EcHandle {
            fd: 11,
            uses_ec_sys: true,
        };

        let rc = unsafe { bindings::ec_write_byte(&mut handle, 0x22, 0x0C) };
        assert_eq!(rc, -libc::EIO);
    }

    #[test]
    #[serial]
    fn ec_write_byte_propagates_lseek_errno_negation() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.lseek_errors.push(libc::EBADF);
        }
        let mut handle = EcHandle {
            fd: 11,
            uses_ec_sys: true,
        };

        let rc = unsafe { bindings::ec_write_byte(&mut handle, 0x22, 0x0C) };
        assert_eq!(
            rc,
            -libc::EBADF,
            "lseek failure inside ec_write_byte must surface as -errno"
        );
    }

    #[test]
    #[serial]
    fn ec_write_byte_propagates_write_errno_negation() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.write_results.push(-libc::EACCES as isize);
        }
        let mut handle = EcHandle {
            fd: 11,
            uses_ec_sys: true,
        };

        let rc = unsafe { bindings::ec_write_byte(&mut handle, 0x22, 0x0C) };
        assert_eq!(rc, -libc::EACCES);
    }

    #[test]
    #[serial]
    fn ec_write_byte_with_null_handle_returns_einval() {
        let _guard = install_mocks();
        let rc = unsafe { bindings::ec_write_byte(std::ptr::null_mut(), 0x22, 0x0C) };
        assert_eq!(
            rc,
            -libc::EINVAL,
            "ec_write_byte must reject NULL handle with -EINVAL"
        );
    }

    #[test]
    #[serial]
    fn ec_close_closes_fd_and_resets_handle() {
        let _guard = install_mocks();
        let mut handle = EcHandle {
            fd: 42,
            uses_ec_sys: true,
        };

        unsafe { bindings::ec_close(&mut handle) };

        assert_eq!(handle.fd, -1, "ec_close must reset fd to sentinel -1");
        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(s.closes, vec![42]);
    }

    #[test]
    #[serial]
    fn ec_close_with_null_handle_is_no_op() {
        let _guard = install_mocks();

        unsafe { bindings::ec_close(std::ptr::null_mut()) };

        let s = MOCK_STATE.lock().unwrap();
        assert!(s.closes.is_empty(), "ec_close(NULL) must not call C close");
    }

    #[test]
    #[serial]
    fn ec_close_with_negative_fd_resets_struct_without_calling_close() {
        let _guard = install_mocks();
        let mut handle = EcHandle {
            fd: -1,
            uses_ec_sys: true,
        };

        unsafe { bindings::ec_close(&mut handle) };

        assert_eq!(handle.fd, -1);
        let s = MOCK_STATE.lock().unwrap();
        assert!(
            s.closes.is_empty(),
            "ec_close on already-closed fd must not call C close"
        );
    }

    #[test]
    #[serial]
    fn msr_open_formats_path_and_returns_mock_fd() {
        let _guard = install_mocks();

        let fd = unsafe { bindings::msr_open(0) };

        assert!(
            fd >= 3,
            "msr_open should return a positive fd via mock open"
        );
        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(
            s.opens.len(),
            1,
            "msr_open should issue exactly one open call"
        );
        assert_eq!(s.opens[0].0, "/dev/cpu/0/msr");
    }

    #[test]
    #[serial]
    fn msr_open_propagates_errno_on_failure() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.open_results.push(-libc::EACCES);
        }

        let rc = unsafe { bindings::msr_open(0) };

        assert_eq!(rc, -libc::EACCES);
    }

    #[test]
    #[serial]
    fn msr_open_with_max_cpu_fits_in_path_buffer() {
        let _guard = install_mocks();

        let fd = unsafe { bindings::msr_open(99_999) };

        assert!(
            fd >= 3,
            "msr_open should accept a CPU index that still fits in 32 bytes"
        );
        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(s.opens[0].0, "/dev/cpu/99999/msr");
    }

    #[test]
    #[serial]
    fn msr_read_returns_zero_and_loads_msr_value() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.read_payloads
                .push(0xDEAD_BEEF_CAFE_F00Du64.to_le_bytes().to_vec());
        }
        let mut value: u64 = 0;

        let rc = unsafe { bindings::msr_read(5, 0x198, &mut value as *mut u64) };

        assert_eq!(rc, 0);
        assert_eq!(value, 0xDEAD_BEEF_CAFE_F00D);
        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(s.lseeks, vec![(5, 0x198, libc::SEEK_SET)]);
    }

    #[test]
    #[serial]
    fn msr_read_returns_einval_for_null_out_or_bad_fd() {
        let _guard = install_mocks();

        let rc_null = unsafe { bindings::msr_read(5, 0x198, std::ptr::null_mut()) };
        assert_eq!(rc_null, -libc::EINVAL, "NULL out must surface as -EINVAL");

        let mut value: u64 = 0;
        let rc_bad = unsafe { bindings::msr_read(-1, 0x198, &mut value as *mut u64) };
        assert_eq!(rc_bad, -libc::EINVAL, "fd < 0 must surface as -EINVAL");
    }

    #[test]
    #[serial]
    fn msr_read_propagates_lseek_errno_negation() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.lseek_errors.push(libc::EBADF);
        }
        let mut value: u64 = 0;

        let rc = unsafe { bindings::msr_read(5, 0x198, &mut value as *mut u64) };

        assert_eq!(rc, -libc::EBADF);
    }

    #[test]
    #[serial]
    fn msr_read_propagates_short_read_as_eio() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            // Provide only 4 bytes of payload — half the requested 8.
            s.read_payloads.push(vec![0xAA; 4]);
        }
        let mut value: u64 = 0;

        let rc = unsafe { bindings::msr_read(5, 0x198, &mut value as *mut u64) };

        assert_eq!(
            rc,
            -libc::EIO,
            "msr_read must surface short reads as -EIO per C implementation"
        );
    }

    #[test]
    #[serial]
    fn msr_read_propagates_read_errno_negation() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.read_errors.push(libc::EIO);
        }
        let mut value: u64 = 0;

        let rc = unsafe { bindings::msr_read(5, 0x198, &mut value as *mut u64) };

        assert_eq!(rc, -libc::EIO);
    }

    #[test]
    #[serial]
    fn msr_write_serializes_value_as_le_bytes() {
        let _guard = install_mocks();

        let rc = unsafe { msr_write(5, 0x150, 0x0102_0304_0506_0708) };

        assert_eq!(rc, 0);
        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(s.lseeks, vec![(5, 0x150, libc::SEEK_SET)]);
        assert_eq!(
            s.writes,
            vec![(5, 0x0102_0304_0506_0708u64.to_le_bytes().to_vec())]
        );
    }

    #[test]
    #[serial]
    fn msr_write_returns_einval_for_bad_fd() {
        let _guard = install_mocks();

        let rc = unsafe { msr_write(-1, 0x150, 0xCAFE) };

        assert_eq!(rc, -libc::EINVAL, "fd < 0 must short-circuit to -EINVAL");
    }

    #[test]
    #[serial]
    fn msr_write_propagates_lseek_errno_negation() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.lseek_errors.push(libc::EBADF);
        }

        let rc = unsafe { msr_write(5, 0x150, 0xCAFE) };

        assert_eq!(rc, -libc::EBADF);
    }

    #[test]
    #[serial]
    fn msr_write_propagates_write_errno_negation() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.write_results.push(-libc::EACCES as isize);
        }

        let rc = unsafe { msr_write(5, 0x150, 0xCAFE) };

        assert_eq!(rc, -libc::EACCES);
    }

    #[test]
    #[serial]
    fn msr_write_returns_eio_for_short_write() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            // Force the mock to claim only 4 of the requested 8 bytes were written.
            s.write_results.push(4);
        }

        let rc = unsafe { msr_write(5, 0x150, 0xCAFE) };

        assert_eq!(
            rc,
            -libc::EIO,
            "short MSR write must surface as -EIO per C implementation"
        );
    }
}
