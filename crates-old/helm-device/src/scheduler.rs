//! Cooperative device scheduler for full-system mode.
//!
//! Inspired by higan: each device thread has its own clock domain.
//! The scheduler picks the thread with the smallest tick count, runs it
//! forward, then re-evaluates. This enables cycle-accurate multi-clock
//! simulation without OS threads.

use crate::bus::DeviceBus;
use crate::device::DeviceEvent;
use helm_core::HelmResult;

/// A device that can be driven by the scheduler's clock.
pub trait TickableDevice: Send + Sync {
    /// Run the device until its clock reaches `target_tick`.
    fn run_until(&mut self, target_tick: u64, bus: &mut DeviceBus) -> HelmResult<Vec<DeviceEvent>>;

    /// Human-readable name.
    fn name(&self) -> &str;
}

/// A scheduled device thread with its own clock domain.
pub struct DeviceThread {
    pub name: String,
    /// Clock frequency in Hz.
    pub clock_hz: u64,
    /// Current tick count (in device-local clock cycles).
    pub clock_ticks: u64,
    /// The device driven by this thread.
    pub device: Box<dyn TickableDevice>,
}

impl DeviceThread {
    pub fn new(name: impl Into<String>, clock_hz: u64, device: Box<dyn TickableDevice>) -> Self {
        Self {
            name: name.into(),
            clock_hz,
            clock_ticks: 0,
            device,
        }
    }

    /// Convert device-local ticks to a global time unit (attoseconds or
    /// a common reference tick). Uses ticks * (1e18 / clock_hz) for
    /// attosecond precision, but we approximate with integer math.
    pub fn global_time(&self) -> u128 {
        if self.clock_hz == 0 {
            return u128::MAX;
        }
        // Time in femtoseconds: ticks * 1_000_000_000_000_000 / clock_hz
        (self.clock_ticks as u128) * 1_000_000_000_000_000 / (self.clock_hz as u128)
    }
}

/// Cooperative scheduler that advances devices in clock-tick order.
///
/// For SE mode this is not needed (devices only respond to CPU
/// transactions). For FS mode it enables cycle-accurate multi-chip
/// simulation.
pub struct DeviceScheduler {
    threads: Vec<DeviceThread>,
}

impl DeviceScheduler {
    pub fn new() -> Self {
        Self {
            threads: Vec::new(),
        }
    }

    /// Add a device thread.
    pub fn add(&mut self, thread: DeviceThread) {
        self.threads.push(thread);
    }

    /// Number of registered threads.
    pub fn num_threads(&self) -> usize {
        self.threads.len()
    }

    /// Find the thread with the smallest global time.
    fn earliest_thread_idx(&self) -> Option<usize> {
        self.threads
            .iter()
            .enumerate()
            .min_by_key(|(_, t)| t.global_time())
            .map(|(i, _)| i)
    }

    /// Step the scheduler: advance the earliest thread by one tick.
    /// Returns events produced by the device.
    pub fn step(&mut self, bus: &mut DeviceBus) -> HelmResult<Vec<DeviceEvent>> {
        let idx = match self.earliest_thread_idx() {
            Some(i) => i,
            None => return Ok(vec![]),
        };
        let thread = &mut self.threads[idx];
        let target = thread.clock_ticks + 1;
        let events = thread.device.run_until(target, bus)?;
        thread.clock_ticks = target;
        Ok(events)
    }

    /// Advance all threads until the earliest reaches `global_ticks` cycles
    /// on the reference clock.
    pub fn run_until(
        &mut self,
        global_ticks: u64,
        bus: &mut DeviceBus,
    ) -> HelmResult<Vec<DeviceEvent>> {
        let mut all_events = Vec::new();
        for _ in 0..global_ticks {
            all_events.extend(self.step(bus)?);
        }
        Ok(all_events)
    }

    /// Access a thread by index.
    pub fn thread(&self, idx: usize) -> Option<&DeviceThread> {
        self.threads.get(idx)
    }

    /// Mutably access a thread by index.
    pub fn thread_mut(&mut self, idx: usize) -> Option<&mut DeviceThread> {
        self.threads.get_mut(idx)
    }
}

impl Default for DeviceScheduler {
    fn default() -> Self {
        Self::new()
    }
}
