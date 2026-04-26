// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::time::Duration;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use nitrosense::app::events::Command;
use nitrosense::app::handler::execute_command;
use nitrosense::app::state::{AppState, FanMode, PerformanceProfile};
use nitrosense::error::NitroError;
use nitrosense::hardware::ec::{Ec, EcDevice};
use nitrosense::hardware::platform::AN515_46_REGS;
use nitrosense::hardware::power;

#[derive(Clone)]
struct BenchEcDevice {
    buffer: [u8; 256],
}

impl BenchEcDevice {
    fn seeded() -> Self {
        let mut buffer = [0u8; 256];
        buffer[AN515_46_REGS.cpu_temp as usize] = 65;
        buffer[AN515_46_REGS.gpu_temp as usize] = 62;
        buffer[AN515_46_REGS.nitro_mode as usize] = AN515_46_REGS.default_mode;
        buffer[AN515_46_REGS.cpu_fan_mode_control as usize] = AN515_46_REGS.cpu_auto_mode;
        buffer[AN515_46_REGS.gpu_fan_mode_control as usize] = AN515_46_REGS.gpu_auto_mode;
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

fn bench_toggle_battery_limit(c: &mut Criterion) {
    c.bench_function("power_toggle_battery_limit_mock", |b| {
        b.iter_batched(
            || {
                Ec::new(BenchEcDevice::seeded(), &AN515_46_REGS)
                    .with_min_write_interval(Duration::ZERO)
            },
            |mut ec| {
                power::toggle_battery_limit(&mut ec, true).expect("toggle should succeed");
                std::hint::black_box(&ec);
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_toggle_usb_charging(c: &mut Criterion) {
    c.bench_function("power_toggle_usb_charging_mock", |b| {
        b.iter_batched(
            || {
                Ec::new(BenchEcDevice::seeded(), &AN515_46_REGS)
                    .with_min_write_interval(Duration::ZERO)
            },
            |mut ec| {
                power::toggle_usb_charging(&mut ec, true).expect("toggle should succeed");
                std::hint::black_box(&ec);
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_toggle_kb_timer(c: &mut Criterion) {
    c.bench_function("power_toggle_kb_timer_mock", |b| {
        b.iter_batched(
            || {
                Ec::new(BenchEcDevice::seeded(), &AN515_46_REGS)
                    .with_min_write_interval(Duration::ZERO)
            },
            |mut ec| {
                power::toggle_kb_timer(&mut ec, true).expect("toggle should succeed");
                std::hint::black_box(&ec);
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_execute_command_toggle(c: &mut Criterion) {
    c.bench_function("handler_execute_command_toggle_mock", |b| {
        b.iter_batched(
            || {
                (
                    Ec::new(BenchEcDevice::seeded(), &AN515_46_REGS)
                        .with_min_write_interval(Duration::ZERO),
                    AppState::default(),
                )
            },
            |(mut ec, mut state)| {
                execute_command(&mut ec, &mut state, Command::ToggleBatteryLimit(true))
                    .expect("command should succeed");
                std::hint::black_box(&state);
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_execute_command_turbo(c: &mut Criterion) {
    c.bench_function("handler_execute_command_turbo_mock", |b| {
        b.iter_batched(
            || {
                (
                    Ec::new(BenchEcDevice::seeded(), &AN515_46_REGS)
                        .with_min_write_interval(Duration::ZERO),
                    AppState::default(),
                )
            },
            |(mut ec, mut state)| {
                execute_command(&mut ec, &mut state, Command::ToggleTurbo(true))
                    .expect("turbo command should succeed");
                std::hint::black_box(&state);
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_execute_command_profile(c: &mut Criterion) {
    c.bench_function("handler_execute_command_profile_mock", |b| {
        b.iter_batched(
            || {
                (
                    Ec::new(BenchEcDevice::seeded(), &AN515_46_REGS)
                        .with_min_write_interval(Duration::ZERO),
                    AppState::default(),
                )
            },
            |(mut ec, mut state)| {
                execute_command(
                    &mut ec,
                    &mut state,
                    Command::SetProfile(PerformanceProfile::Extreme),
                )
                .expect("profile command should succeed");
                std::hint::black_box(&state);
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_execute_command_fan_mode(c: &mut Criterion) {
    c.bench_function("handler_execute_command_fan_mode_mock", |b| {
        b.iter_batched(
            || {
                (
                    Ec::new(BenchEcDevice::seeded(), &AN515_46_REGS)
                        .with_min_write_interval(Duration::ZERO),
                    AppState::default(),
                )
            },
            |(mut ec, mut state)| {
                execute_command(&mut ec, &mut state, Command::SetCpuFanMode(FanMode::Turbo))
                    .expect("fan mode command should succeed");
                std::hint::black_box(&state);
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    benches,
    bench_toggle_battery_limit,
    bench_toggle_usb_charging,
    bench_toggle_kb_timer,
    bench_execute_command_toggle,
    bench_execute_command_turbo,
    bench_execute_command_profile,
    bench_execute_command_fan_mode,
);
criterion_main!(benches);
