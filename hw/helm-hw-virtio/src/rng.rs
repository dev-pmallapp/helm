//! VirtIO entropy (RNG) device backend (device type 4, VirtIO spec §5.4).
//!
//! The entropy device is the simplest VirtIO device: it has one virtqueue
//! (requestq, queue 0) into which the driver places write-only buffers.
//! The device fills those buffers with random bytes and returns them.
//!
//! This implementation uses a simple LFSR-based PRNG seeded at construction
//! so that output is deterministic across runs (matching the simulator's
//! determinism-by-default rule). For non-deterministic output, seed from
//! the host OS via `std::collections::hash_map::RandomState` or similar.
//!
//! # Limitations
//!
//! - Single queue only.
//! - PRNG is deterministic by default; not cryptographically secure.
//! - No device features beyond `VIRTIO_F_VERSION_1`.

use crate::proto::features::{VIRTIO_DEVICE_RNG, VIRTIO_F_VERSION_1};
use crate::proto::virtqueue::VirtQueue;
use crate::{VirtioBackend, VirtioPendingEvents};
use helm_diag::sim_warn;

// ── Minimal 64-bit xorshift PRNG ─────────────────────────────────────────────

/// 64-bit xorshift PRNG (Marsaglia 2003).
///
/// Deterministic, fast, and passes basic statistical tests. Not
/// cryptographically secure. State must be non-zero.
#[derive(Debug, Clone)]
struct Xorshift64 {
    state: u64,
}

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        // Ensure state is never zero (xorshift is undefined for state=0)
        Self {
            state: if seed == 0 {
                0xDEAD_BEEF_CAFE_1234
            } else {
                seed
            },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn fill_bytes(&mut self, buf: &mut [u8]) {
        let mut i = 0;
        while i + 8 <= buf.len() {
            let val = self.next_u64();
            buf[i..i + 8].copy_from_slice(&val.to_le_bytes());
            i += 8;
        }
        if i < buf.len() {
            let val = self.next_u64();
            let bytes = val.to_le_bytes();
            let rem = buf.len() - i;
            buf[i..].copy_from_slice(&bytes[..rem]);
        }
    }
}

// ── VirtioRng ────────────────────────────────────────────────────────────────

/// VirtIO entropy (RNG) device backend.
///
/// Fills guest-supplied buffers with pseudo-random bytes on every request.
/// The PRNG is seeded at construction; use [`with_seed`](Self::with_seed) to
/// control the seed for reproducible test runs.
pub struct VirtioRng {
    prng: Xorshift64,
    notify_pending: bool,
}

impl VirtioRng {
    /// Create an RNG device with the default deterministic seed.
    pub fn new() -> Self {
        Self {
            prng: Xorshift64::new(0x0123_4567_89AB_CDEF),
            notify_pending: false,
        }
    }

    /// Create an RNG device with a custom seed.
    ///
    /// Passing the same seed across runs produces identical output,
    /// which is useful for reproducible tests.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            prng: Xorshift64::new(seed),
            notify_pending: false,
        }
    }

    /// Fill a buffer with pseudo-random bytes.
    ///
    /// Called by the caller after walking the requestq descriptor chain.
    /// `buf` should point to the write-only guest buffer segment.
    pub fn fill_entropy(&mut self, buf: &mut [u8]) {
        self.prng.fill_bytes(buf);
    }

    /// Take (and clear) the queue notify pending flag.
    pub fn take_notify_pending(&mut self) -> bool {
        let v = self.notify_pending;
        self.notify_pending = false;
        v
    }
}

impl Default for VirtioRng {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioBackend for VirtioRng {
    fn device_type(&self) -> u32 {
        VIRTIO_DEVICE_RNG
    }

    fn vendor_id(&self) -> u32 {
        crate::proto::features::VIRTIO_VENDOR_ID as u32
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1
    }

    fn queue_max_size(&self, queue: usize) -> u32 {
        if queue == 0 {
            64
        } else {
            0
        }
    }

    fn queue_notify(&mut self, _queue: usize, _mem: Option<&mut dyn helm_core::ByteMem>) {
        self.notify_pending = true;
    }

    fn read_config(&self, _offset: u32) -> u32 {
        // RNG has no device-specific config space
        0
    }

    fn write_config(&mut self, _offset: u32, _val: u32) {}

    fn reset(&mut self) {
        self.notify_pending = false;
        // Note: PRNG state is NOT reset; this preserves determinism
        // while avoiding seed reuse across resets.
    }

    fn process_pending(
        &mut self,
        mem: &mut dyn helm_core::ByteMem,
        queues: &mut [VirtQueue],
    ) -> VirtioPendingEvents {
        if !self.take_notify_pending() {
            return VirtioPendingEvents::default();
        }

        let Some(queue) = queues.get_mut(0) else {
            return VirtioPendingEvents::default();
        };

        let mut queue_irq = false;
        let mut failed = false;
        'queue: loop {
            let head = match queue.pop_chain(mem) {
                Ok(Some(head)) => head,
                Ok(None) => break,
                Err(err) => {
                    sim_warn!(component = "virtio-rng", "queue 0 pop_chain failed: {err}");
                    failed = true;
                    break;
                }
            };
            let chain = match queue.collect_chain(mem, head) {
                Ok(chain) => chain,
                Err(err) => {
                    sim_warn!(
                        component = "virtio-rng",
                        "queue 0 collect_chain failed head={head}: {err}"
                    );
                    failed = true;
                    break;
                }
            };
            let mut bytes_written = 0u32;
            for (addr, len, is_write) in chain {
                if !is_write {
                    continue;
                }
                let mut buf = vec![0u8; len as usize];
                self.fill_entropy(&mut buf);
                if let Err(err) = mem.write_bytes(addr, &buf) {
                    sim_warn!(
                        component = "virtio-rng",
                        "queue 0 guest write failed head={head} addr={addr:#x}: {err}"
                    );
                    failed = true;
                    break 'queue;
                }
                bytes_written = bytes_written.saturating_add(len);
            }
            match queue.push_used(mem, head, bytes_written) {
                Ok(raise_irq) => queue_irq |= raise_irq,
                Err(err) => {
                    sim_warn!(
                        component = "virtio-rng",
                        "queue 0 push_used failed head={head}: {err}"
                    );
                    failed = true;
                    break;
                }
            }
        }

        VirtioPendingEvents {
            queue_irq,
            config_irq: false,
            failed,
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_device_type() {
        assert_eq!(VirtioRng::new().device_type(), VIRTIO_DEVICE_RNG);
    }

    #[test]
    fn rng_queue_max_size() {
        let r = VirtioRng::new();
        assert_eq!(r.queue_max_size(0), 64);
        assert_eq!(r.queue_max_size(1), 0);
    }

    #[test]
    fn rng_fill_entropy_non_zero() {
        let mut r = VirtioRng::new();
        let mut buf = [0u8; 64];
        r.fill_entropy(&mut buf);
        // Very unlikely to be all-zero
        assert!(buf.iter().any(|&b| b != 0));
    }

    #[test]
    fn rng_fill_entropy_deterministic() {
        let seed = 0xCAFE_BABE_1234_5678;
        let mut r1 = VirtioRng::with_seed(seed);
        let mut r2 = VirtioRng::with_seed(seed);
        let mut b1 = [0u8; 32];
        let mut b2 = [0u8; 32];
        r1.fill_entropy(&mut b1);
        r2.fill_entropy(&mut b2);
        assert_eq!(b1, b2);
    }

    #[test]
    fn rng_fill_entropy_different_per_call() {
        let mut r = VirtioRng::new();
        let mut b1 = [0u8; 16];
        let mut b2 = [0u8; 16];
        r.fill_entropy(&mut b1);
        r.fill_entropy(&mut b2);
        assert_ne!(b1, b2);
    }

    #[test]
    fn rng_notify_pending() {
        let mut r = VirtioRng::new();
        r.queue_notify(0, None);
        assert!(r.take_notify_pending());
        assert!(!r.take_notify_pending());
    }

    #[test]
    fn rng_reset_clears_pending() {
        let mut r = VirtioRng::new();
        r.queue_notify(0, None);
        r.reset();
        assert!(!r.take_notify_pending());
    }

    #[test]
    fn xorshift_never_zero() {
        let mut prng = Xorshift64::new(1);
        for _ in 0..1000 {
            assert_ne!(prng.next_u64(), 0);
        }
    }

    #[test]
    fn rng_fill_partial_block() {
        // Test fill_bytes when buf.len() is not a multiple of 8
        let mut r = VirtioRng::new();
        let mut buf = [0u8; 13];
        r.fill_entropy(&mut buf); // Should not panic
    }
}
