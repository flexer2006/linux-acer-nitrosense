/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Copyright (C) 2024-2026 NitroSense Contributors */

#define _GNU_SOURCE

#include "nitro_hw.h"

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/statfs.h>
#include <time.h>
#include <unistd.h>

/* ---- Default syscall forwarders ---- */

static int     real_open(const char *p, int f, ...) { return open(p, f); }
static ssize_t real_read(int fd, void *b, size_t n) { return read(fd, b, n); }
static ssize_t real_write(int fd, const void *b, size_t n) { return write(fd, b, n); }
static off_t   real_lseek(int fd, off_t o, int w) { return lseek(fd, o, w); }
static int     real_close(int fd) { return close(fd); }
static int     real_system(const char *c) { return system(c); }

struct ec_ops nitro_hw_ops = {
    .open   = real_open,
    .read   = real_read,
    .write  = real_write,
    .lseek  = real_lseek,
    .close  = real_close,
    .system = real_system,
};

void ec_set_ops(const struct ec_ops *ops) {
    if (ops) {
        nitro_hw_ops = *ops;
    } else {
        nitro_hw_ops = (struct ec_ops){
            .open   = real_open,
            .read   = real_read,
            .write  = real_write,
            .lseek  = real_lseek,
            .close  = real_close,
            .system = real_system,
        };
    }
}

/* ---- EC path constants ---- */

static const char EC_SYS_PATH[]  = "/sys/kernel/debug/ec/ec0/io";
static const char EC_DEV_PATH[]  = "/dev/ec";

/* debugfs magic number (DEBUGFS_MAGIC) */
#define NITRO_DEBUGFS_MAGIC 0x6462672f

/* Ensure debugfs is mounted at /sys/kernel/debug. The ec_sys interface is
   exposed through debugfs, so without it /sys/kernel/debug/ec/ec0/io will
   never appear even when the module is loaded. */
static int ec_ensure_debugfs(void) {
    struct statfs st;
    if (statfs("/sys/kernel/debug", &st) == 0 &&
        (unsigned long)st.f_type == NITRO_DEBUGFS_MAGIC) {
        return 0;
    }
    return nitro_hw_ops.system("mount -t debugfs none /sys/kernel/debug 2>/dev/null");
}

/* ---- High-resolution timing for EC write latency tracing ---- */

static inline uint64_t nanos_now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
}

/* Cache the NITRO_TRACE env var check on first call so we don't call
   getenv() on every EC refresh/write. The value is stable for the
   lifetime of the process (tracing is either on or off at startup). */
static int nitro_trace_enabled(void) {
    static int cached = -1;
    if (cached < 0) {
        cached = getenv("NITRO_TRACE") != NULL;
    }
    return cached;
}

/* ---- EC implementation ---- */

int ec_open(struct ec_handle *out) {
    if (!out) {
        return -EINVAL;
    }
    memset(out, 0, sizeof(*out));
    out->fd = -1;

    /* Try ec_sys first. The ec_sys interface is exposed through debugfs, so
       make sure debugfs is mounted before attempting the open or modprobe. */
    (void)ec_ensure_debugfs();
    (void)nitro_hw_ops.system("modprobe -r ec_sys 2>/dev/null");
    (void)nitro_hw_ops.system("modprobe ec_sys write_support=y 2>/dev/null");

    int fd = nitro_hw_ops.open(EC_SYS_PATH, O_RDWR | O_CLOEXEC);
    if (fd >= 0) {
        out->fd         = fd;
        out->uses_ec_sys = true;
        return 0;
    }
    int ec_sys_errno = errno;

    /* Fallback to acpi_ec */
    (void)nitro_hw_ops.system("modprobe acpi_ec 2>/dev/null");

    fd = nitro_hw_ops.open(EC_DEV_PATH, O_RDWR | O_CLOEXEC);
    if (fd >= 0) {
        out->fd         = fd;
        out->uses_ec_sys = false;
        return 0;
    }

    /* Preserve the most informative error: EACCES (missing write_support)
       is more actionable than ENOENT (path simply missing). */
    if (ec_sys_errno == EACCES || errno == EACCES) {
        return -EACCES;
    }
    return -errno;
}

void ec_close(struct ec_handle *h) {
    if (!h) {
        return;
    }
    if (h->fd >= 0) {
        nitro_hw_ops.close(h->fd);
    }
    memset(h, 0, sizeof(*h));
    h->fd = -1;
}

int ec_refresh(struct ec_handle *h, uint8_t *buffer, size_t len) {
    if (!h || h->fd < 0 || !buffer || len == 0) {
        return -EINVAL;
    }

    int tracing = nitro_trace_enabled();
    uint64_t t0 = tracing ? nanos_now() : 0;

    if (nitro_hw_ops.lseek(h->fd, 0, SEEK_SET) == (off_t)-1) {
        return -errno;
    }

    ssize_t n = nitro_hw_ops.read(h->fd, buffer, len);
    if (n < 0) {
        return -errno;
    }

    if (tracing) {
        uint64_t dt_ns = nanos_now() - t0;
        fprintf(stderr, "TRACE ec_refresh bytes=%zd latency=%lu_ns\n",
                n, (unsigned long)dt_ns);
    }

    return (int)n;
}

int ec_write_byte(struct ec_handle *h, uint8_t addr, uint8_t val) {
    if (!h || h->fd < 0) {
        return -EINVAL;
    }

    int tracing = nitro_trace_enabled();
    uint64_t t0 = tracing ? nanos_now() : 0;

    if (nitro_hw_ops.lseek(h->fd, (off_t)addr, SEEK_SET) == (off_t)-1) {
        return -errno;
    }

    ssize_t n = nitro_hw_ops.write(h->fd, &val, 1);
    if (n < 0) {
        return -errno;
    }
    if (n != 1) {
        return -EIO;
    }

    if (tracing) {
        uint64_t dt_ns = nanos_now() - t0;
        fprintf(stderr, "TRACE ec_write_byte addr=0x%02X val=0x%02X latency=%lu_ns\n",
                addr, val, (unsigned long)dt_ns);
    }

    return 0;
}
