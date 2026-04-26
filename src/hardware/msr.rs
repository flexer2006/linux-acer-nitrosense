// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::ffi::c_int;
use std::io;

use crate::error::NitroError;
use crate::ffi::bindings;

#[derive(Debug)]
pub struct Msr {
    fd: i32,
    cpu: i32,
}

impl Msr {
    pub fn cpu(&self) -> i32 {
        self.cpu
    }

    pub fn fd(&self) -> i32 {
        self.fd
    }

    pub fn open(cpu: i32) -> Result<Self, NitroError> {
        let fd = unsafe {
            // SAFETY: The C helper returns a raw file descriptor for the requested CPU index.
            bindings::msr_open(cpu as c_int)
        };

        if fd < 0 {
            return Err(NitroError::MsrOpen {
                cpu,
                source: io::Error::from_raw_os_error(-fd),
            });
        }

        Ok(Self { fd, cpu })
    }

    pub fn read(&self, msr: u32) -> Result<u64, NitroError> {
        let mut value = 0u64;
        let rc = unsafe {
            // SAFETY: The MSR handle is owned by `self`; the output pointer is valid for writes.
            bindings::msr_read(self.fd, msr, &mut value as *mut u64)
        };

        if rc < 0 {
            return Err(NitroError::MsrRead {
                msr,
                source: io::Error::from_raw_os_error(-rc),
            });
        }

        Ok(value)
    }
}

impl Drop for Msr {
    fn drop(&mut self) {
        if self.fd >= 0 {
            crate::ffi::msr_close_fd(self.fd);
            self.fd = -1;
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the Rust `Msr` wrapper around the C `msr_*` FFI surface.
    //!
    //! The wrapper itself is thin (open/read/Drop), but each branch carries a
    //! distinct error mapping that we want to exercise:
    //!
    //!   * `Msr::open` success path returns a positive fd via `msr_open`.
    //!   * `Msr::open` failure path maps `-errno` from the C side to
    //!     `NitroError::MsrOpen { cpu, source }` with the original errno.
    //!   * `Msr::read` success path returns the little-endian u64 read by the
    //!     C side.
    //!   * `Msr::read` failure path maps `-errno` to `NitroError::MsrRead
    //!     { msr, source }` with the original errno.
    //!   * `Drop` calls `msr_close_fd` exactly once when fd is non-negative
    //!     and is a no-op for the sentinel.
    //!   * Accessor methods (`cpu`, `fd`) return the values set at open time.
    //!
    //! Mocks are installed via `crate::test_support::ec_syscall_mock` which
    //! shares the same global `struct ec_ops` table as the FFI tests. Every
    //! test must hold the mock lock for the duration of the call.
    use super::*;
    use crate::test_support::ec_syscall_mock::{MOCK_STATE, install_mocks};
    use serial_test::serial;

    #[test]
    #[serial]
    fn msr_open_returns_handle_with_assigned_fd_and_cpu() {
        let _guard = install_mocks();

        let msr = Msr::open(2).expect("Msr::open should succeed against mock");

        assert!(msr.fd() >= 3, "fd should be a positive mock fd");
        assert_eq!(msr.cpu(), 2, "cpu accessor must echo the constructor input");

        let s = MOCK_STATE.lock().unwrap();
        assert_eq!(
            s.opens.len(),
            1,
            "Msr::open must open the MSR device exactly once"
        );
        assert_eq!(s.opens[0].0, "/dev/cpu/2/msr");
    }

    #[test]
    #[serial]
    fn msr_open_propagates_errno_as_msr_open_error() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.open_results.push(-libc::EACCES);
        }

        let err = Msr::open(0).expect_err("Msr::open must surface failure when C msr_open fails");

        match err {
            NitroError::MsrOpen { cpu, source } => {
                assert_eq!(cpu, 0, "MsrOpen must carry the requested CPU index");
                assert_eq!(source.raw_os_error(), Some(libc::EACCES));
            }
            other => panic!("expected NitroError::MsrOpen, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn msr_open_propagates_enoent_when_msr_path_missing() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.open_results.push(-libc::ENOENT);
        }

        let err = Msr::open(7).expect_err("missing /dev/cpu/N/msr must surface as Msr::open error");

        match err {
            NitroError::MsrOpen { cpu, source } => {
                assert_eq!(cpu, 7);
                assert_eq!(source.raw_os_error(), Some(libc::ENOENT));
            }
            other => panic!("expected NitroError::MsrOpen, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn msr_read_returns_msr_value_in_little_endian_byte_order() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.read_payloads
                .push(0xCAFE_F00D_DEAD_BEEFu64.to_le_bytes().to_vec());
        }
        let msr = Msr::open(0).expect("Msr::open should succeed against mock");

        let value = msr
            .read(0x198)
            .expect("Msr::read should succeed for valid mocked payload");

        assert_eq!(value, 0xCAFE_F00D_DEAD_BEEF);
        let s = MOCK_STATE.lock().unwrap();
        assert!(
            s.lseeks.contains(&(msr.fd(), 0x198, libc::SEEK_SET)),
            "Msr::read must seek to msr offset before reading"
        );
    }

    #[test]
    #[serial]
    fn msr_read_propagates_errno_as_msr_read_error() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.read_errors.push(libc::EIO);
        }
        let msr = Msr::open(0).expect("Msr::open should succeed against mock");

        let err = msr
            .read(0x198)
            .expect_err("Msr::read must propagate read errno");

        match err {
            NitroError::MsrRead { msr, source } => {
                assert_eq!(msr, 0x198, "MsrRead must carry the requested register");
                assert_eq!(source.raw_os_error(), Some(libc::EIO));
            }
            other => panic!("expected NitroError::MsrRead, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn msr_read_propagates_short_read_as_eio() {
        let _guard = install_mocks();
        {
            let mut s = MOCK_STATE.lock().unwrap();
            // Provide only 4 of the 8 bytes the C layer expects.
            s.read_payloads.push(vec![0xAA; 4]);
        }
        let msr = Msr::open(0).expect("Msr::open should succeed against mock");

        let err = msr
            .read(0x198)
            .expect_err("short MSR read must surface as Msr::read error");

        match err {
            NitroError::MsrRead { source, .. } => {
                assert_eq!(source.raw_os_error(), Some(libc::EIO));
            }
            other => panic!("expected NitroError::MsrRead, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn msr_read_propagates_lseek_errno() {
        let _guard = install_mocks();
        let msr = Msr::open(0).expect("Msr::open should succeed against mock");
        {
            let mut s = MOCK_STATE.lock().unwrap();
            s.lseek_errors.push(libc::EBADF);
        }

        let err = msr
            .read(0x150)
            .expect_err("lseek failure must surface as Msr::read error");

        match err {
            NitroError::MsrRead { msr, source } => {
                assert_eq!(msr, 0x150);
                assert_eq!(source.raw_os_error(), Some(libc::EBADF));
            }
            other => panic!("expected NitroError::MsrRead, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn msr_drop_calls_msr_close_for_valid_fd() {
        let _guard = install_mocks();
        let msr = Msr::open(0).expect("Msr::open should succeed against mock");
        let fd = msr.fd();

        drop(msr);

        let s = MOCK_STATE.lock().unwrap();
        assert!(s.closes.contains(&fd), "Msr::drop must close the MSR fd");
    }

    #[test]
    #[serial]
    fn msr_drop_skips_close_for_sentinel_fd() {
        let _guard = install_mocks();
        // Construct an Msr in the failed-open shape: fd = -1, cpu = 0.
        // We can't call Msr::open with a forced fd, but we can simulate the
        // post-drop state by manually constructing one.
        let msr = Msr { fd: -1, cpu: 0 };

        drop(msr);

        let s = MOCK_STATE.lock().unwrap();
        assert!(
            s.closes.is_empty(),
            "Msr::drop on sentinel fd must not call C close"
        );
    }

    #[test]
    fn msr_accessors_expose_cpu_and_fd_after_construction() {
        // Construct directly without touching the C side so the test does
        // not need the mock install helpers; it only exercises the simple
        // getters.
        let msr = Msr { fd: 99, cpu: 13 };
        assert_eq!(msr.fd(), 99);
        assert_eq!(msr.cpu(), 13);
        // Prevent Drop from running (fd 99 is fictional).
        std::mem::forget(msr);
    }
}
