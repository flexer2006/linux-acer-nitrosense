// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use nitrosense::error::NitroError;
use nitrosense::hardware::ec::{Ec, EcDevice};
use nitrosense::hardware::platform::AN515_46_REGS;

#[derive(Clone)]
struct BenchEcDevice {
    buffer: [u8; 256],
}

impl BenchEcDevice {
    fn seeded() -> Self {
        let mut buffer = [0u8; 256];
        buffer[AN515_46_REGS.cpu_temp as usize] = 65;
        buffer[AN515_46_REGS.gpu_temp as usize] = 62;
        buffer[AN515_46_REGS.sys_temp as usize] = 50;
        buffer[AN515_46_REGS.cpu_fan_speed_high as usize] = 0x8B;
        buffer[AN515_46_REGS.cpu_fan_speed_low as usize] = 0x06;
        buffer[AN515_46_REGS.gpu_fan_speed_high as usize] = 0x10;
        buffer[AN515_46_REGS.gpu_fan_speed_low as usize] = 0x07;
        buffer[AN515_46_REGS.power_status as usize] = 0x01;
        buffer[AN515_46_REGS.battery_status as usize] = 0x02;
        Self { buffer }
    }
}

impl EcDevice for BenchEcDevice {
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
        Ok(())
    }
}

fn bench_ec_snapshot(c: &mut Criterion) {
    c.bench_function("ec_refresh_and_snapshot_mock", |b| {
        b.iter_batched(
            || {
                Ec::new(BenchEcDevice::seeded(), &AN515_46_REGS)
                    .with_min_write_interval(Duration::ZERO)
            },
            |mut ec| {
                ec.refresh().expect("mock refresh should succeed");
                std::hint::black_box(ec.snapshot());
            },
            criterion::BatchSize::SmallInput,
        )
    });
}

fn bench_ec_validated_write(c: &mut Criterion) {
    c.bench_function("ec_validated_write_mock", |b| {
        b.iter_batched(
            || {
                Ec::new(BenchEcDevice::seeded(), &AN515_46_REGS)
                    .with_min_write_interval(Duration::ZERO)
            },
            |mut ec| {
                ec.write(
                    AN515_46_REGS.cpu_fan_mode_control,
                    AN515_46_REGS.cpu_manual_mode,
                )
                .expect("valid mock write should succeed");
            },
            criterion::BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, bench_ec_snapshot, bench_ec_validated_write);
criterion_main!(benches);
