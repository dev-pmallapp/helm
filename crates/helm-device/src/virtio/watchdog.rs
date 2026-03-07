//! VirtIO watchdog device (type 35).

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use crate::device::DeviceEvent;

pub struct VirtioWatchdog {
    /// Timeout in milliseconds.
    pub timeout_ms: u32,
    /// Whether the watchdog is armed.
    pub armed: bool,
    /// Ticks since last kick.
    pub ticks_since_kick: u64,
    pub pending_irq: bool,
}

impl VirtioWatchdog {
    pub fn new(timeout_ms: u32) -> Self {
        Self {
            timeout_ms,
            armed: false,
            ticks_since_kick: 0,
            pending_irq: false,
        }
    }

    pub fn kick(&mut self) {
        self.ticks_since_kick = 0;
    }

    pub fn is_expired(&self) -> bool {
        self.armed && self.ticks_since_kick > self.timeout_ms as u64
    }
}

impl Default for VirtioWatchdog {
    fn default() -> Self { Self::new(30_000) }
}

impl VirtioDeviceBackend for VirtioWatchdog {
    fn device_id(&self) -> u32 { VIRTIO_DEV_WATCHDOG }
    fn device_features(&self) -> u64 { VIRTIO_F_VERSION_1 }

    fn config_size(&self) -> u32 { 0 }
    fn read_config(&self, _offset: u32) -> u8 { 0 }
    fn write_config(&mut self, _offset: u32, _value: u8) {}

    fn num_queues(&self) -> u16 { 1 }

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        let q = match queues.get_mut(queue_idx as usize) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };
        while let Some(head) = q.pop_avail() {
            self.kick();
            q.push_used(head, 0);
            self.pending_irq = true;
        }
    }

    fn tick(&mut self, cycles: u64, _queues: &mut [Virtqueue]) -> Vec<DeviceEvent> {
        if self.armed {
            self.ticks_since_kick += cycles;
        }
        vec![]
    }

    fn activate(&mut self, _features: u64, _queues: &mut [Virtqueue]) {
        self.armed = true;
    }

    fn reset(&mut self) {
        self.armed = false;
        self.ticks_since_kick = 0;
        self.pending_irq = false;
    }

    fn name(&self) -> &str { "virtio-watchdog" }
}
