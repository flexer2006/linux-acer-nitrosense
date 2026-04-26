/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Copyright (C) 2024-2026 NitroSense Contributors */

#ifndef NITRO_HW_H
#define NITRO_HW_H

#include <assert.h>
#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>
#include <sys/types.h>

/* ---------- EC handle ---------- */

struct ec_handle {
    int  fd;
    bool uses_ec_sys;  /* true = /sys/kernel/debug/ec/ec0/io */
};

static_assert(sizeof(struct ec_handle) == 8,
              "ec_handle must be 8 bytes for Rust FFI");

/* ---------- Syscall injection (testability) ---------- */

typedef int     (*sys_open_fn)(const char *path, int flags, ...);
typedef ssize_t (*sys_read_fn)(int fd, void *buf, size_t count);
typedef ssize_t (*sys_write_fn)(int fd, const void *buf, size_t count);
typedef off_t   (*sys_lseek_fn)(int fd, off_t offset, int whence);
typedef int     (*sys_close_fn)(int fd);
typedef int     (*sys_system_fn)(const char *cmd);

struct ec_ops {
    sys_open_fn   open;
    sys_read_fn   read;
    sys_write_fn  write;
    sys_lseek_fn  lseek;
    sys_close_fn  close;
    sys_system_fn system;
};

void ec_set_ops(const struct ec_ops *ops);

/* ---------- EC operations ---------- */

[[nodiscard]] int  ec_open(struct ec_handle *out);
void               ec_close(struct ec_handle *h);
[[nodiscard]] int  ec_refresh(struct ec_handle *h, uint8_t *buffer, size_t len);
[[nodiscard]] int  ec_write_byte(struct ec_handle *h, uint8_t addr, uint8_t val);

/* ---------- Assembly-backed port I/O ---------- */

[[nodiscard]] uint8_t  asm_inb(uint16_t port);
void                   asm_outb(uint16_t port, uint8_t val);
[[nodiscard]] uint64_t asm_rdmsr(uint32_t msr);
void                   asm_wrmsr(uint32_t msr, uint64_t val);

/* ---------- MSR operations ---------- */

[[nodiscard]] int  msr_open(int cpu);
void               msr_close(int fd);
[[nodiscard]] int  msr_read(int fd, uint32_t msr, uint64_t *out);
[[nodiscard]] int  msr_write(int fd, uint32_t msr, uint64_t val);

#endif /* NITRO_HW_H */
