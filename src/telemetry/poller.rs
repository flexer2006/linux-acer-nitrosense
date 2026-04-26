// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::state::BatteryStatus;
use crate::error::NitroError;
use crate::hardware::ec::{Ec, EcDevice};
use tokio::sync::{Mutex, watch};

#[derive(Debug, Clone)]
pub struct TelemetrySnapshot {
    pub cpu_temp: u8,
    pub gpu_temp: u8,
    pub sys_temp: u8,
    pub cpu_fan_rpm: u16,
    pub gpu_fan_rpm: u16,
    pub power_plugged_in: bool,
    pub battery_status: BatteryStatus,
    pub cpu_fan_mode: u8,
    pub gpu_fan_mode: u8,
    pub nitro_mode: u8,
    pub battery_charge_limit: u8,
    pub kb_30_timeout: u8,
    pub usb_charging: u8,
    pub cpu_manual_speed: u8,
    pub gpu_manual_speed: u8,
    pub timestamp: Instant,
}

impl Default for TelemetrySnapshot {
    fn default() -> Self {
        Self {
            cpu_temp: 0,
            gpu_temp: 0,
            sys_temp: 0,
            cpu_fan_rpm: 0,
            gpu_fan_rpm: 0,
            power_plugged_in: false,
            battery_status: BatteryStatus::NotInUse,
            cpu_fan_mode: 0,
            gpu_fan_mode: 0,
            nitro_mode: 0,
            battery_charge_limit: 0,
            kb_30_timeout: 0,
            usb_charging: 0,
            cpu_manual_speed: 0,
            gpu_manual_speed: 0,
            timestamp: Instant::now(),
        }
    }
}

pub async fn poll_once<D>(ec: Arc<Mutex<Ec<D>>>) -> Result<TelemetrySnapshot, NitroError>
where
    D: EcDevice + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut ec = ec.blocking_lock();
        ec.refresh()?;
        Ok(ec.snapshot())
    })
    .await
    .map_err(|e| NitroError::Poller(format!("poller task join failed: {e}")))?
}

pub async fn run_poller<D>(
    ec: Arc<Mutex<Ec<D>>>,
    tx: watch::Sender<TelemetrySnapshot>,
    period: Duration,
) -> Result<(), NitroError>
where
    D: EcDevice + 'static,
{
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    run_poller_until_shutdown(ec, tx, shutdown_rx, period).await
}

pub async fn run_poller_until_shutdown<D>(
    ec: Arc<Mutex<Ec<D>>>,
    tx: watch::Sender<TelemetrySnapshot>,
    mut shutdown_rx: watch::Receiver<bool>,
    period: Duration,
) -> Result<(), NitroError>
where
    D: EcDevice + 'static,
{
    let mut interval = tokio::time::interval(period);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let snapshot = poll_once(Arc::clone(&ec)).await?;
                if tx.send(snapshot).is_err() {
                    return Ok(());
                }
            }
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::platform::AN515_46_REGS;

    #[derive(Debug)]
    struct MockPollEcDevice {
        buffer: [u8; 256],
        refresh_calls: usize,
    }

    impl EcDevice for MockPollEcDevice {
        fn open(&mut self) -> Result<(), NitroError> {
            Ok(())
        }

        fn close(&mut self) {}

        fn refresh(&mut self, buffer: &mut [u8]) -> Result<usize, NitroError> {
            self.refresh_calls += 1;
            buffer.copy_from_slice(&self.buffer);
            Ok(self.buffer.len())
        }

        fn write_byte(&mut self, addr: u8, val: u8) -> Result<(), NitroError> {
            self.buffer[addr as usize] = val;
            Ok(())
        }
    }

    #[tokio::test]
    async fn poll_once_refreshes_ec_and_returns_latest_snapshot() {
        let mut buffer = [0u8; 256];
        buffer[AN515_46_REGS.cpu_temp as usize] = 71;
        buffer[AN515_46_REGS.cpu_fan_speed_high as usize] = 0x2C;
        buffer[AN515_46_REGS.cpu_fan_speed_low as usize] = 0x05;
        let ec = Arc::new(Mutex::new(Ec::new(
            MockPollEcDevice {
                buffer,
                refresh_calls: 0,
            },
            &AN515_46_REGS,
        )));

        let snapshot = poll_once(Arc::clone(&ec))
            .await
            .expect("mock poll should refresh and snapshot");

        assert_eq!(
            snapshot.cpu_temp, 71,
            "poller should publish refreshed CPU temp"
        );
        assert_eq!(
            snapshot.cpu_fan_rpm, 0x052C,
            "poller snapshot should use original low<<8|high RPM ordering"
        );
        assert_eq!(
            ec.lock().await.device_ref().refresh_calls,
            1,
            "poll_once should refresh EC exactly once"
        );
    }

    #[tokio::test]
    async fn run_poller_until_shutdown_exits_when_shutdown_signal_changes() {
        let ec = Arc::new(Mutex::new(Ec::new(
            MockPollEcDevice {
                buffer: [0; 256],
                refresh_calls: 0,
            },
            &AN515_46_REGS,
        )));
        let (telemetry_tx, _telemetry_rx) = watch::channel(TelemetrySnapshot::default());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(run_poller_until_shutdown(
            ec,
            telemetry_tx,
            shutdown_rx,
            Duration::from_secs(60),
        ));
        shutdown_tx
            .send(true)
            .expect("shutdown signal should be sent");

        let result = tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("poller should stop promptly after shutdown")
            .expect("poller task should not panic");

        assert!(result.is_ok(), "poller shutdown should return Ok");
    }

    #[test]
    fn telemetry_snapshot_default_has_zero_values_and_recent_timestamp() {
        let before = Instant::now();
        let snapshot = TelemetrySnapshot::default();
        let after = Instant::now();

        assert_eq!(snapshot.cpu_temp, 0);
        assert_eq!(snapshot.gpu_temp, 0);
        assert_eq!(snapshot.sys_temp, 0);
        assert_eq!(snapshot.cpu_fan_rpm, 0);
        assert_eq!(snapshot.gpu_fan_rpm, 0);
        assert!(!snapshot.power_plugged_in);
        assert_eq!(snapshot.battery_status, BatteryStatus::NotInUse);
        assert_eq!(snapshot.cpu_fan_mode, 0);
        assert_eq!(snapshot.gpu_fan_mode, 0);
        assert_eq!(snapshot.nitro_mode, 0);
        assert_eq!(snapshot.battery_charge_limit, 0);
        assert_eq!(snapshot.kb_30_timeout, 0);
        assert_eq!(snapshot.usb_charging, 0);
        assert_eq!(snapshot.cpu_manual_speed, 0);
        assert_eq!(snapshot.gpu_manual_speed, 0);
        assert!(
            snapshot.timestamp >= before && snapshot.timestamp <= after,
            "default timestamp should be wall-clock-now"
        );
    }

    #[test]
    fn telemetry_snapshot_clone_produces_field_equal_copy() {
        let original = TelemetrySnapshot {
            cpu_temp: 71,
            gpu_temp: 65,
            sys_temp: 50,
            cpu_fan_rpm: 0x0500,
            gpu_fan_rpm: 0x0700,
            power_plugged_in: true,
            battery_status: BatteryStatus::Charging,
            cpu_fan_mode: 0x04,
            gpu_fan_mode: 0x10,
            nitro_mode: 0x04,
            battery_charge_limit: 0x51,
            kb_30_timeout: 0x1E,
            usb_charging: 0x0F,
            cpu_manual_speed: 12,
            gpu_manual_speed: 13,
            timestamp: Instant::now(),
        };

        let cloned = original.clone();

        assert_eq!(cloned.cpu_temp, original.cpu_temp);
        assert_eq!(cloned.gpu_temp, original.gpu_temp);
        assert_eq!(cloned.sys_temp, original.sys_temp);
        assert_eq!(cloned.cpu_fan_rpm, original.cpu_fan_rpm);
        assert_eq!(cloned.gpu_fan_rpm, original.gpu_fan_rpm);
        assert_eq!(cloned.power_plugged_in, original.power_plugged_in);
        assert_eq!(cloned.battery_status, original.battery_status);
        assert_eq!(cloned.cpu_fan_mode, original.cpu_fan_mode);
        assert_eq!(cloned.gpu_fan_mode, original.gpu_fan_mode);
        assert_eq!(cloned.nitro_mode, original.nitro_mode);
        assert_eq!(cloned.battery_charge_limit, original.battery_charge_limit);
        assert_eq!(cloned.kb_30_timeout, original.kb_30_timeout);
        assert_eq!(cloned.usb_charging, original.usb_charging);
        assert_eq!(cloned.cpu_manual_speed, original.cpu_manual_speed);
        assert_eq!(cloned.gpu_manual_speed, original.gpu_manual_speed);
    }

    #[tokio::test]
    async fn run_poller_until_shutdown_returns_ok_when_telemetry_receiver_dropped() {
        // When the telemetry receiver is dropped, the watch sender's `send`
        // returns Err and the poller is expected to terminate cleanly.
        let ec = Arc::new(Mutex::new(Ec::new(
            MockPollEcDevice {
                buffer: [0; 256],
                refresh_calls: 0,
            },
            &AN515_46_REGS,
        )));
        let (telemetry_tx, telemetry_rx) = watch::channel(TelemetrySnapshot::default());
        drop(telemetry_rx);
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        tokio::time::timeout(
            Duration::from_secs(1),
            run_poller_until_shutdown(ec, telemetry_tx, shutdown_rx, Duration::from_millis(5)),
        )
        .await
        .expect("poller should exit promptly when receiver dropped")
        .expect("poller should return Ok on receiver drop");
    }

    #[tokio::test]
    async fn run_poller_until_shutdown_exits_when_shutdown_sender_dropped() {
        // Dropping the watch sender causes `shutdown_rx.changed()` to return
        // an error; the poller must treat that as a termination signal.
        let ec = Arc::new(Mutex::new(Ec::new(
            MockPollEcDevice {
                buffer: [0; 256],
                refresh_calls: 0,
            },
            &AN515_46_REGS,
        )));
        let (telemetry_tx, _telemetry_rx) = watch::channel(TelemetrySnapshot::default());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        drop(shutdown_tx);

        tokio::time::timeout(
            Duration::from_secs(1),
            run_poller_until_shutdown(ec, telemetry_tx, shutdown_rx, Duration::from_secs(60)),
        )
        .await
        .expect("poller should exit promptly when shutdown sender drops")
        .expect("poller should return Ok on shutdown sender drop");
    }

    #[tokio::test]
    async fn run_poller_publishes_at_least_one_snapshot_before_shutdown() {
        let mut buffer = [0u8; 256];
        buffer[AN515_46_REGS.cpu_temp as usize] = 88;
        let ec = Arc::new(Mutex::new(Ec::new(
            MockPollEcDevice {
                buffer,
                refresh_calls: 0,
            },
            &AN515_46_REGS,
        )));
        let (telemetry_tx, mut telemetry_rx) = watch::channel(TelemetrySnapshot::default());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(run_poller_until_shutdown(
            ec,
            telemetry_tx,
            shutdown_rx,
            Duration::from_millis(10),
        ));

        // Wait for at least one tick to publish a snapshot.
        let changed = tokio::time::timeout(Duration::from_secs(1), telemetry_rx.changed()).await;
        assert!(
            changed.is_ok(),
            "poller should publish a snapshot within 1 second"
        );
        let snap = telemetry_rx.borrow_and_update().clone();
        assert_eq!(
            snap.cpu_temp, 88,
            "poller must propagate the CPU temp from the EC buffer"
        );

        shutdown_tx.send(true).unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
    }

    #[tokio::test]
    async fn run_poller_compatibility_wrapper_runs_until_telemetry_receiver_dropped() {
        // The legacy `run_poller` wrapper creates an internal shutdown channel
        // whose sender is dropped immediately when the function returns; we
        // can still exit the loop by dropping the telemetry receiver.
        let ec = Arc::new(Mutex::new(Ec::new(
            MockPollEcDevice {
                buffer: [0; 256],
                refresh_calls: 0,
            },
            &AN515_46_REGS,
        )));
        let (telemetry_tx, telemetry_rx) = watch::channel(TelemetrySnapshot::default());
        let handle = tokio::spawn(run_poller(ec, telemetry_tx, Duration::from_millis(5)));

        // Give the poller a tick to do at least one refresh, then drop the rx
        // to make `tx.send` fail and exit the loop.
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(telemetry_rx);

        let result = tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("run_poller should terminate when receiver drops")
            .expect("run_poller task should not panic");
        assert!(
            result.is_ok(),
            "run_poller should return Ok on receiver drop"
        );
    }
}
