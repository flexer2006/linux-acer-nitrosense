// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use crate::error::NitroError;
use crate::hardware::ec::EcDevice;
use crate::hardware::rgb::RgbDeviceWriter;

#[derive(Debug)]
pub(crate) struct RecordingEcDevice {
    pub(crate) buffer: [u8; 256],
    pub(crate) writes: Vec<(u8, u8)>,
}

impl Default for RecordingEcDevice {
    fn default() -> Self {
        Self {
            buffer: [0; 256],
            writes: Vec::new(),
        }
    }
}

impl EcDevice for RecordingEcDevice {
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
        self.writes.push((addr, val));
        Ok(())
    }
}

#[derive(Debug, Default)]
pub(crate) struct RecordingRgbWriter {
    pub(crate) writes: Vec<(String, Vec<u8>)>,
}

impl RgbDeviceWriter for RecordingRgbWriter {
    fn write_payload(&mut self, device: &str, payload: &[u8]) -> Result<(), NitroError> {
        self.writes.push((device.to_string(), payload.to_vec()));
        Ok(())
    }
}

/// Syscall-injection mock for the C hardware abstraction layer.
///
/// The C layer (`src/c_hw/ec_lowlevel.c` + `src/c_hw/msr_lowlevel.c`) calls
/// open/read/write/lseek/close/system through a global function table
/// (`struct ec_ops`). Tests install mock function pointers via `ec_set_ops`
/// to drive `ec_open` / `ec_refresh` / `ec_write_byte` / `msr_open` /
/// `msr_read` / `msr_write` through the FFI surface and assert the C side
/// performs the expected syscall sequence and error mapping.
///
/// All helpers in this module hold `MOCK_LOCK` for the duration of a test
/// because `ec_set_ops` mutates a process-global table. `install_mocks()`
/// returns a `MockGuard` that restores real syscalls on drop, and
/// `reset_mock_state` is invoked before installation so each test starts
/// from a clean slate.
pub(crate) mod ec_syscall_mock {
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::sync::Mutex;

    // The C `ec_ops.open` field is `int (*)(const char *, int, ...)` (variadic).
    // The C layer only ever calls it with exactly two arguments. On the System
    // V AMD64 ABI a non-variadic 2-argument function pointer is calling-
    // convention compatible with that variadic prototype because integer
    // arguments travel in registers and clang/gcc set AL=0 at variadic call
    // sites with no float arguments. Declaring `open` here as a non-variadic
    // 2-argument function keeps the test on stable Rust without enabling the
    // `c_variadic` feature.
    #[repr(C)]
    pub(crate) struct CEcOps {
        pub open: unsafe extern "C" fn(path: *const c_char, flags: c_int) -> c_int,
        pub read: unsafe extern "C" fn(fd: c_int, buf: *mut c_void, count: usize) -> isize,
        pub write: unsafe extern "C" fn(fd: c_int, buf: *const c_void, count: usize) -> isize,
        pub lseek: unsafe extern "C" fn(fd: c_int, offset: i64, whence: c_int) -> i64,
        pub close: unsafe extern "C" fn(fd: c_int) -> c_int,
        pub system: unsafe extern "C" fn(cmd: *const c_char) -> c_int,
    }

    unsafe extern "C" {
        pub fn ec_set_ops(ops: *const CEcOps);
        pub fn msr_write(fd: c_int, msr: u32, val: u64) -> c_int;
        pub fn msr_close(fd: c_int);
    }

    pub(crate) static MOCK_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    pub(crate) struct MockState {
        pub next_fd: c_int,
        pub opens: Vec<(String, c_int)>,
        /// Each entry is consumed by the next `mock_open` call. `0` allocates a
        /// fresh fd; `>0` returns that fd verbatim; `<0` sets `errno` to
        /// `-value` and returns `-1`.
        pub open_results: Vec<c_int>,
        pub reads: Vec<(c_int, usize)>,
        pub read_payloads: Vec<Vec<u8>>,
        /// `>0` causes the next read to return `-1` with `errno = value`.
        /// `0` is a no-op (use `read_payloads` to control the byte count).
        pub read_errors: Vec<i32>,
        pub writes: Vec<(c_int, Vec<u8>)>,
        /// `<0` causes the next write to return `-1` with `errno = -value`.
        /// `>=0` is the byte count returned (use to simulate short writes).
        pub write_results: Vec<isize>,
        pub lseeks: Vec<(c_int, i64, c_int)>,
        pub lseek_errors: Vec<i32>,
        pub closes: Vec<c_int>,
        pub systems: Vec<String>,
    }

    pub(crate) static MOCK_STATE: Mutex<MockState> = Mutex::new(MockState {
        next_fd: 3,
        opens: Vec::new(),
        open_results: Vec::new(),
        reads: Vec::new(),
        read_payloads: Vec::new(),
        read_errors: Vec::new(),
        writes: Vec::new(),
        write_results: Vec::new(),
        lseeks: Vec::new(),
        lseek_errors: Vec::new(),
        closes: Vec::new(),
        systems: Vec::new(),
    });

    pub(crate) fn reset_mock_state() {
        let mut s = MOCK_STATE.lock().unwrap();
        *s = MockState {
            next_fd: 3,
            ..Default::default()
        };
    }

    fn set_errno(value: i32) {
        unsafe {
            *libc::__errno_location() = value;
        }
    }

    unsafe extern "C" fn mock_open(path: *const c_char, _flags: c_int) -> c_int {
        let path = unsafe { CStr::from_ptr(path) }
            .to_string_lossy()
            .into_owned();
        let mut s = MOCK_STATE.lock().unwrap();
        let fd_or_err = if s.open_results.is_empty() {
            0
        } else {
            s.open_results.remove(0)
        };
        let fd = if fd_or_err == 0 {
            let allocated = s.next_fd;
            s.next_fd += 1;
            allocated
        } else if fd_or_err < 0 {
            set_errno(-fd_or_err);
            -1
        } else {
            fd_or_err
        };
        s.opens.push((path, fd));
        fd
    }

    unsafe extern "C" fn mock_read(fd: c_int, buf: *mut c_void, count: usize) -> isize {
        let mut s = MOCK_STATE.lock().unwrap();
        s.reads.push((fd, count));
        if !s.read_errors.is_empty() {
            let err = s.read_errors.remove(0);
            if err > 0 {
                set_errno(err);
                return -1;
            }
        }
        if s.read_payloads.is_empty() {
            return 0;
        }
        let payload = s.read_payloads.remove(0);
        let n = payload.len().min(count);
        // SAFETY: `buf` is non-null (callers in the C layer reject NULL with
        // `EINVAL` before calling read); `n` does not exceed the smaller of
        // the supplied buffer size or the payload length, so the destination
        // slice is valid for `n` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(payload.as_ptr(), buf as *mut u8, n);
        }
        n as isize
    }

    unsafe extern "C" fn mock_write(fd: c_int, buf: *const c_void, count: usize) -> isize {
        // SAFETY: The C layer validates `buf` before invoking write; tests
        // only ever drive write through `ec_write_byte` / `msr_write` which
        // pass non-null pointers to local stack values.
        let bytes = unsafe { std::slice::from_raw_parts(buf as *const u8, count) }.to_vec();
        let mut s = MOCK_STATE.lock().unwrap();
        s.writes.push((fd, bytes));
        if s.write_results.is_empty() {
            count as isize
        } else {
            let v = s.write_results.remove(0);
            if v < 0 {
                set_errno((-v) as i32);
                -1
            } else {
                v
            }
        }
    }

    unsafe extern "C" fn mock_lseek(fd: c_int, offset: i64, whence: c_int) -> i64 {
        let mut s = MOCK_STATE.lock().unwrap();
        s.lseeks.push((fd, offset, whence));
        if !s.lseek_errors.is_empty() {
            let err = s.lseek_errors.remove(0);
            if err > 0 {
                set_errno(err);
                return -1;
            }
        }
        offset
    }

    unsafe extern "C" fn mock_close(fd: c_int) -> c_int {
        let mut s = MOCK_STATE.lock().unwrap();
        s.closes.push(fd);
        0
    }

    unsafe extern "C" fn mock_system(cmd: *const c_char) -> c_int {
        let cmd = unsafe { CStr::from_ptr(cmd) }
            .to_string_lossy()
            .into_owned();
        let mut s = MOCK_STATE.lock().unwrap();
        s.systems.push(cmd);
        0
    }

    fn install_mock_ops() {
        let ops = CEcOps {
            open: mock_open,
            read: mock_read,
            write: mock_write,
            lseek: mock_lseek,
            close: mock_close,
            system: mock_system,
        };
        // SAFETY: `ec_set_ops` copies the struct on the C side, so the local
        // `ops` may safely be dropped after the call. Replacing the global
        // table is intrinsically unsafe; tests serialize on `MOCK_LOCK`.
        unsafe {
            ec_set_ops(&ops as *const CEcOps);
        }
    }

    fn restore_real_ops() {
        // SAFETY: `ec_set_ops(NULL)` resets the global table back to the real
        // libc syscalls. This is the documented "uninstall" path on the C
        // side and is always safe.
        unsafe {
            ec_set_ops(std::ptr::null());
        }
    }

    pub(crate) struct MockGuard<'a> {
        _lock: std::sync::MutexGuard<'a, ()>,
    }

    impl Drop for MockGuard<'_> {
        fn drop(&mut self) {
            restore_real_ops();
        }
    }

    /// Acquire the global mock lock, install the mock `ec_ops`, and reset
    /// per-test state. The returned guard restores real syscalls on drop.
    pub(crate) fn install_mocks() -> MockGuard<'static> {
        let lock = MOCK_LOCK.lock().unwrap();
        reset_mock_state();
        install_mock_ops();
        MockGuard { _lock: lock }
    }
}
