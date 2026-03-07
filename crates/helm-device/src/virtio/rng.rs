//! VirtIO entropy source (type 4) — spec 5.4.
//!
//! The simplest VirtIO device. The guest provides buffers via the
//! virtqueue; the device fills them with random bytes.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;

/// VirtIO RNG device — entropy source.
pub struct VirtioRng {
    /// Seed for the simple PRNG (simulation only).
    seed: u64,
    pub pending_irq: bool,
}

impl VirtioRng {
    pub fn new() -> Self {
        Self {
            seed: 0x1234_5678_9ABC_DEF0,
            pending_irq: false,
        }
    }

    pub fn with_seed(seed: u64) -> Self {
        Self {
            seed,
            pending_irq: false,
        }
    }

    /// Simple xorshift64 PRNG for simulation.
    fn next_u64(&mut self) -> u64 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.seed = x;
        x
    }

    fn process_request(&mut self, queues: &mut [Virtqueue]) {
        let q = match queues.get_mut(0) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };

        while let Some(head) = q.pop_avail() {
            let chain = q.walk_chain(head);
            let total_bytes: u32 = chain.iter().map(|d| d.len).sum();
            // In a real implementation, we'd fill the guest memory buffers.
            // For simulation, we just advance the PRNG state.
            let words = (total_bytes as u64 + 7) / 8;
            for _ in 0..words {
                self.next_u64();
            }
            q.push_used(head, total_bytes);
            self.pending_irq = true;
        }
    }
}

impl Default for VirtioRng {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDeviceBackend for VirtioRng {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_RNG
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1
    }

    fn config_size(&self) -> u32 {
        0 // No config space
    }

    fn read_config(&self, _offset: u32) -> u8 {
        0
    }

    fn write_config(&mut self, _offset: u32, _value: u8) {}

    fn num_queues(&self) -> u16 {
        1 // requestq
    }

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        if queue_idx == 0 {
            self.process_request(queues);
        }
    }

    fn reset(&mut self) {
        self.pending_irq = false;
    }

    fn name(&self) -> &str {
        "virtio-rng"
    }
}
