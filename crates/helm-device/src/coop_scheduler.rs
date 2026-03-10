//! Cooperative multi-clock scheduler — higan-inspired.
//!
//! Extends the existing [`DeviceScheduler`](crate::scheduler::DeviceScheduler)
//! with absolute timestamps, scalar normalization across clock domains,
//! and integration with [`AddressMap`](crate::address_map::AddressMap).
//!
//! Devices that declare `clock_hz() > 0` are registered. The scheduler
//! picks the device with the smallest timestamp and ticks it, maintaining
//! correct inter-clock relationships.
//!
//! ```text
//! CPU @ 1 GHz:    |---|---|---|---|---|---|---|---|
//! Timer @ 1 MHz:  |------------------------------|
//! UART @ 115200:  |--------------------------------------------------|
//! ```

use crate::address_map::AddressMap;
use crate::device::{DeviceEvent, DeviceId};
use helm_core::HelmResult;

/// Per-device clock state with scalar normalization.
#[derive(Debug, Clone)]
pub struct DeviceClock {
    /// Absolute timestamp in normalized tick space (femtoseconds).
    pub timestamp: u128,
    /// Native clock frequency in Hz.
    pub freq_hz: u64,
}

impl DeviceClock {
    /// Create a clock at the given frequency.
    pub fn new(freq_hz: u64) -> Self {
        Self {
            timestamp: 0,
            freq_hz,
        }
    }

    /// Advance by N native cycles.
    ///
    /// Converts native cycles to femtoseconds: `cycles * 1e15 / freq_hz`.
    pub fn step(&mut self, native_cycles: u64) {
        if self.freq_hz > 0 {
            self.timestamp +=
                (native_cycles as u128) * 1_000_000_000_000_000 / (self.freq_hz as u128);
        }
    }

    /// Current time in femtoseconds.
    pub fn time_fs(&self) -> u128 {
        self.timestamp
    }
}

/// An entry in the cooperative scheduler.
struct SchedulerEntry {
    device_id: DeviceId,
    clock: DeviceClock,
}

/// Cooperative scheduler that advances devices in timestamp order.
///
/// Integrated with [`AddressMap`] — ticks devices directly through
/// the address map's device storage.
pub struct CoopScheduler {
    entries: Vec<SchedulerEntry>,
}

impl CoopScheduler {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Register a clocked device.
    pub fn register(&mut self, device_id: DeviceId, freq_hz: u64) {
        // Don't double-register
        if self.entries.iter().any(|e| e.device_id == device_id) {
            return;
        }
        self.entries.push(SchedulerEntry {
            device_id,
            clock: DeviceClock::new(freq_hz),
        });
    }

    /// Unregister a device (on detach).
    pub fn unregister(&mut self, device_id: DeviceId) {
        self.entries.retain(|e| e.device_id != device_id);
    }

    /// Number of registered devices.
    pub fn num_entries(&self) -> usize {
        self.entries.len()
    }

    /// Find the entry with the smallest timestamp.
    fn earliest_idx(&self) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .min_by_key(|(_, e)| e.clock.timestamp)
            .map(|(i, _)| i)
    }

    /// Advance the earliest device by one native cycle. Returns events.
    pub fn step(&mut self, map: &mut AddressMap) -> HelmResult<Vec<DeviceEvent>> {
        let idx = match self.earliest_idx() {
            Some(i) => i,
            None => return Ok(vec![]),
        };

        let entry = &mut self.entries[idx];
        entry.clock.step(1);
        let device_id = entry.device_id;

        match map.device_mut(device_id) {
            Some(dev) => dev.tick(1),
            None => Ok(vec![]),
        }
    }

    /// Run until the earliest device's timestamp reaches `target_fs` femtoseconds.
    pub fn run_until_fs(
        &mut self,
        target_fs: u128,
        map: &mut AddressMap,
    ) -> HelmResult<Vec<DeviceEvent>> {
        let mut all_events = Vec::new();
        loop {
            let idx = match self.earliest_idx() {
                Some(i) => i,
                None => break,
            };
            if self.entries[idx].clock.timestamp >= target_fs {
                break;
            }
            all_events.extend(self.step(map)?);
        }
        Ok(all_events)
    }

    /// Run for N steps (tick the earliest device N times).
    pub fn run_steps(
        &mut self,
        steps: u64,
        map: &mut AddressMap,
    ) -> HelmResult<Vec<DeviceEvent>> {
        let mut all_events = Vec::new();
        for _ in 0..steps {
            all_events.extend(self.step(map)?);
        }
        Ok(all_events)
    }

    /// Prevent timestamp overflow: subtract the minimum timestamp from all entries.
    pub fn renormalize(&mut self) {
        if let Some(min_ts) = self.entries.iter().map(|e| e.clock.timestamp).min() {
            for entry in &mut self.entries {
                entry.clock.timestamp -= min_ts;
            }
        }
    }

    /// Get the clock state for a device.
    pub fn clock(&self, device_id: DeviceId) -> Option<&DeviceClock> {
        self.entries
            .iter()
            .find(|e| e.device_id == device_id)
            .map(|e| &e.clock)
    }

    /// Check if a device is registered.
    pub fn is_registered(&self, device_id: DeviceId) -> bool {
        self.entries.iter().any(|e| e.device_id == device_id)
    }
}

impl Default for CoopScheduler {
    fn default() -> Self {
        Self::new()
    }
}
