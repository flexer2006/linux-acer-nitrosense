/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Copyright (C) 2024-2026 NitroSense Contributors */

#define _GNU_SOURCE

#include "nitro_hw.h"

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

/* MSR uses the same syscall injection layer as EC */
extern struct ec_ops nitro_hw_ops;

/* ---- MSR path format ---- */

#define MSR_PATH_MAX 32

int msr_open(int cpu) {
    char path[MSR_PATH_MAX];
    int  n = snprintf(path, sizeof(path), "/dev/cpu/%d/msr", cpu);
    if (n < 0 || (size_t)n >= sizeof(path)) {
        return -ENAMETOOLONG;
    }

    int fd = nitro_hw_ops.open(path, O_RDWR | O_CLOEXEC);
    if (fd < 0) {
        return -errno;
    }
    return fd;
}

void msr_close(int fd) {
    if (fd >= 0) {
        nitro_hw_ops.close(fd);
    }
}

int msr_read(int fd, uint32_t msr, uint64_t *out) {
    if (fd < 0 || !out) {
        return -EINVAL;
    }

    if (nitro_hw_ops.lseek(fd, (off_t)msr, SEEK_SET) == (off_t)-1) {
        return -errno;
    }

    ssize_t n = nitro_hw_ops.read(fd, out, sizeof(*out));
    if (n < 0) {
        return -errno;
    }
    if ((size_t)n != sizeof(*out)) {
        return -EIO;
    }

    return 0;
}

int msr_write(int fd, uint32_t msr, uint64_t val) {
    if (fd < 0) {
        return -EINVAL;
    }

    if (nitro_hw_ops.lseek(fd, (off_t)msr, SEEK_SET) == (off_t)-1) {
        return -errno;
    }

    ssize_t n = nitro_hw_ops.write(fd, &val, sizeof(val));
    if (n < 0) {
        return -errno;
    }
    if ((size_t)n != sizeof(val)) {
        return -EIO;
    }

    return 0;
}
