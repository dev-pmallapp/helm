use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Lock-free single-producer, single-consumer (SPSC) ring buffer.
/// Capacity must be a power of 2.
///
/// Hot-path cost: one `ptr::write` + one `AtomicUsize::store(Release)`.
/// No allocation per push. No locks.
pub struct TraceRing<T: Copy + Send> {
    buf: Box<[UnsafeCell<MaybeUninit<T>>]>,
    mask: usize,
    head: AtomicUsize, // producer writes here
    tail: AtomicUsize, // consumer reads here
}

// Safety: TraceRing is designed for SPSC use. The atomic head/tail provide
// the necessary synchronization between a single producer and single consumer.
unsafe impl<T: Copy + Send> Send for TraceRing<T> {}
unsafe impl<T: Copy + Send> Sync for TraceRing<T> {}

impl<T: Copy + Send> TraceRing<T> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity.is_power_of_two(), "capacity must be power of 2");
        assert!(capacity > 0, "capacity must be > 0");
        let buf = (0..capacity)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            buf,
            mask: capacity - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Non-blocking push. Returns false if ring is full (drops the value).
    #[inline]
    pub fn push(&self, val: T) -> bool {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h.wrapping_sub(t) >= self.buf.len() {
            return false; // full
        }
        unsafe {
            (*self.buf[h & self.mask].get()).write(val);
        }
        self.head.store(h.wrapping_add(1), Ordering::Release);
        true
    }

    /// Drain all available entries into a Vec.
    pub fn drain_into(&self, out: &mut Vec<T>) {
        let h = self.head.load(Ordering::Acquire);
        let mut t = self.tail.load(Ordering::Relaxed);
        while t != h {
            let val = unsafe { (*self.buf[t & self.mask].get()).assume_init_read() };
            out.push(val);
            t = t.wrapping_add(1);
        }
        self.tail.store(t, Ordering::Release);
    }

    pub fn len(&self) -> usize {
        let h = self.head.load(Ordering::Acquire);
        let t = self.tail.load(Ordering::Acquire);
        h.wrapping_sub(t)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }
}

/// Compact branch record for high-rate tracing.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct BranchRecord {
    pub pc: u64,
    pub target: u64,
    pub insn_count: u64,
    pub flags: u8,    // bit 0 = taken, bits 1..3 = kind
    pub _pad: [u8; 7],
}

impl BranchRecord {
    pub fn taken(self) -> bool {
        self.flags & 1 != 0
    }
}

const _: () = assert!(std::mem::size_of::<BranchRecord>() == 32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_ring_push_and_drain() {
        let ring = TraceRing::<u64>::new(8);
        assert!(ring.is_empty());
        assert_eq!(ring.capacity(), 8);

        assert!(ring.push(10));
        assert!(ring.push(20));
        assert!(ring.push(30));
        assert_eq!(ring.len(), 3);

        let mut out = Vec::new();
        ring.drain_into(&mut out);
        assert_eq!(out, vec![10, 20, 30]);
        assert!(ring.is_empty());
    }

    #[test]
    fn trace_ring_full_drops() {
        let ring = TraceRing::<u32>::new(4);
        assert!(ring.push(1));
        assert!(ring.push(2));
        assert!(ring.push(3));
        assert!(ring.push(4));
        // Ring is full
        assert!(!ring.push(5)); // dropped
        assert_eq!(ring.len(), 4);

        let mut out = Vec::new();
        ring.drain_into(&mut out);
        assert_eq!(out, vec![1, 2, 3, 4]);
    }

    #[test]
    fn trace_ring_push_after_drain() {
        let ring = TraceRing::<u32>::new(4);
        assert!(ring.push(1));
        assert!(ring.push(2));

        let mut out = Vec::new();
        ring.drain_into(&mut out);
        assert_eq!(out, vec![1, 2]);

        // Can push again after drain
        assert!(ring.push(3));
        assert!(ring.push(4));
        assert!(ring.push(5));
        assert!(ring.push(6));
        // Full again
        assert!(!ring.push(7));

        let mut out2 = Vec::new();
        ring.drain_into(&mut out2);
        assert_eq!(out2, vec![3, 4, 5, 6]);
    }

    #[test]
    fn branch_record_size_is_32() {
        assert_eq!(std::mem::size_of::<BranchRecord>(), 32);
    }

    #[test]
    fn branch_record_taken_flag() {
        let mut br = BranchRecord::default();
        assert!(!br.taken());
        br.flags = 0x01;
        assert!(br.taken());
        br.flags = 0x03; // taken + kind bits
        assert!(br.taken());
        br.flags = 0x02; // kind bit set, taken clear
        assert!(!br.taken());
    }

    #[test]
    fn trace_ring_branch_records() {
        let ring = TraceRing::<BranchRecord>::new(16);
        let rec = BranchRecord {
            pc: 0x1000,
            target: 0x2000,
            insn_count: 42,
            flags: 0x01, // taken
            _pad: [0; 7],
        };
        assert!(ring.push(rec));

        let mut out = Vec::new();
        ring.drain_into(&mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pc, 0x1000);
        assert_eq!(out[0].target, 0x2000);
        assert!(out[0].taken());
    }

    #[test]
    #[should_panic(expected = "capacity must be power of 2")]
    fn trace_ring_non_power_of_two_panics() {
        let _ring = TraceRing::<u32>::new(7);
    }
}
