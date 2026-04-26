// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

/// Stack-allocated formatting buffer for rendering dynamic labels without
/// per-frame `String` heap allocations.
///
/// Used in the egui render loop to format temperature, fan speed, and voltage
/// labels on the stack instead of the heap. The const-generic `N` parameter
/// controls buffer capacity.
///
/// # Example
///
/// ```
/// use nitrosense::ui::fmtbuf::FmtBuf;
/// use std::fmt::Write;
///
/// let mut buf = FmtBuf::<32>::new();
/// let _ = write!(buf, "CPU: {}°C", 65);
/// assert_eq!(buf.as_str(), "CPU: 65°C");
/// assert_eq!(buf.as_str_or("fallback"), "CPU: 65°C");
///
/// let empty = FmtBuf::<32>::new();
/// assert_eq!(empty.as_str_or("—"), "—");
/// ```
pub struct FmtBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> Default for FmtBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> FmtBuf<N> {
    pub fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    /// Return the formatted content as `&str`.
    ///
    /// Returns the fallback string "?" if the buffer content is not valid UTF-8
    /// (which should never happen since `write_str` only accepts `&str` input,
    /// but serves as a safety net).
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len]).unwrap_or("?")
    }

    /// Whether any bytes have been written to the buffer.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Return the formatted content as `&str`, or `fallback` if the buffer
    /// is empty. Useful for optional values (e.g. voltage display when
    /// no sensor data is available).
    pub fn as_str_or<'a>(&'a self, fallback: &'a str) -> &'a str {
        if self.len == 0 {
            fallback
        } else {
            std::str::from_utf8(&self.buf[..self.len]).unwrap_or(fallback)
        }
    }
}

impl<const N: usize> std::fmt::Write for FmtBuf<N> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.len + s.len();
        if end > self.buf.len() {
            return Err(std::fmt::Error);
        }
        self.buf[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn empty_buffer_is_empty() {
        let buf = FmtBuf::<32>::new();
        assert!(buf.is_empty());
    }

    #[test]
    fn write_formats_integer() {
        let mut buf = FmtBuf::<32>::new();
        let _ = write!(buf, "CPU: {}°C", 65);
        assert_eq!(buf.as_str(), "CPU: 65°C");
        assert!(!buf.is_empty());
    }

    #[test]
    fn write_formats_float() {
        let mut buf = FmtBuf::<16>::new();
        let _ = write!(buf, "{:.2} V", 1.35);
        assert_eq!(buf.as_str(), "1.35 V");
    }

    #[test]
    fn overflow_truncates_and_returns_error() {
        let mut buf = FmtBuf::<4>::new();
        let result = write!(buf, "hello");
        assert!(result.is_err());
        // Partial write may have occurred
    }

    #[test]
    fn nonempty_buffer_returns_content_via_as_str() {
        let mut buf = FmtBuf::<32>::new();
        let _ = write!(buf, "test");
        assert_eq!(buf.as_str(), "test");
    }

    #[test]
    fn as_str_or_returns_content_when_nonempty() {
        let mut buf = FmtBuf::<32>::new();
        let _ = write!(buf, "1.35 V");
        assert_eq!(buf.as_str_or("—"), "1.35 V");
    }

    #[test]
    fn as_str_or_returns_fallback_when_empty() {
        let buf = FmtBuf::<32>::new();
        assert_eq!(buf.as_str_or("—"), "—");
    }
}
