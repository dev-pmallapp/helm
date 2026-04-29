//! `RingBuffer<T>` and `EventStream<T>` -- dual-impl, feature-gated.

#[cfg(feature = "collection")]
pub use live::{EventStream, RingBuffer};
#[cfg(not(feature = "collection"))]
pub use noop::{EventStream, RingBuffer};

#[cfg(feature = "collection")]
mod live {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Fixed-capacity ring buffer. Overwrites oldest entries on push when full.
    /// Uses Mutex -- suitable only for low-rate events (faults, syscalls).
    pub struct RingBuffer<T: Clone + Send> {
        capacity: usize,
        buf: Mutex<VecDeque<T>>,
    }

    impl<T: Clone + Send> RingBuffer<T> {
        pub fn new(capacity: usize) -> Self {
            Self {
                capacity,
                buf: Mutex::new(VecDeque::with_capacity(capacity)),
            }
        }

        pub fn push(&self, val: T) {
            let mut buf = self.buf.lock().unwrap();
            if buf.len() >= self.capacity {
                buf.pop_front();
            }
            buf.push_back(val);
        }

        /// Returns a snapshot (clone) of all current entries.
        pub fn snapshot(&self) -> Vec<T> {
            let buf = self.buf.lock().unwrap();
            buf.iter().cloned().collect()
        }

        pub fn len(&self) -> usize {
            self.buf.lock().unwrap().len()
        }

        pub fn is_empty(&self) -> bool {
            self.buf.lock().unwrap().is_empty()
        }

        pub fn clear(&self) {
            self.buf.lock().unwrap().clear();
        }

        pub fn capacity(&self) -> usize {
            self.capacity
        }
    }

    /// Bounded event stream. Records events up to `max`, then stops.
    /// Uses Mutex -- suitable only for low-rate events.
    pub struct EventStream<T: Clone + Send> {
        max: usize,
        events: Mutex<Vec<T>>,
    }

    impl<T: Clone + Send> EventStream<T> {
        pub fn new(max: usize) -> Self {
            Self {
                max,
                events: Mutex::new(Vec::with_capacity(max.min(1024))),
            }
        }

        /// Push an event. Returns false if the stream is full.
        pub fn push(&self, val: T) -> bool {
            let mut events = self.events.lock().unwrap();
            if events.len() >= self.max {
                return false;
            }
            events.push(val);
            true
        }

        /// Drain all events, returning them and clearing the stream.
        pub fn drain(&self) -> Vec<T> {
            let mut events = self.events.lock().unwrap();
            std::mem::take(&mut *events)
        }

        pub fn len(&self) -> usize {
            self.events.lock().unwrap().len()
        }

        pub fn is_empty(&self) -> bool {
            self.events.lock().unwrap().is_empty()
        }

        pub fn max(&self) -> usize {
            self.max
        }
    }
}

#[cfg(not(feature = "collection"))]
mod noop {
    use std::marker::PhantomData;

    /// ZST no-op ring buffer. Pushes are dropped, snapshot is empty.
    pub struct RingBuffer<T: Clone + Send> {
        _t: PhantomData<fn() -> T>,
    }

    impl<T: Clone + Send> RingBuffer<T> {
        #[inline(always)]
        pub fn new(_capacity: usize) -> Self {
            Self { _t: PhantomData }
        }
        #[inline(always)]
        pub fn push(&self, _val: T) {}
        #[inline(always)]
        pub fn snapshot(&self) -> Vec<T> {
            Vec::new()
        }
        #[inline(always)]
        pub fn len(&self) -> usize {
            0
        }
        #[inline(always)]
        pub fn is_empty(&self) -> bool {
            true
        }
        #[inline(always)]
        pub fn clear(&self) {}
        #[inline(always)]
        pub fn capacity(&self) -> usize {
            0
        }
    }

    /// ZST no-op bounded event stream. Pushes are dropped, drain is empty.
    pub struct EventStream<T: Clone + Send> {
        _t: PhantomData<fn() -> T>,
    }

    impl<T: Clone + Send> EventStream<T> {
        #[inline(always)]
        pub fn new(_max: usize) -> Self {
            Self { _t: PhantomData }
        }
        #[inline(always)]
        pub fn push(&self, _val: T) -> bool {
            false
        }
        #[inline(always)]
        pub fn drain(&self) -> Vec<T> {
            Vec::new()
        }
        #[inline(always)]
        pub fn len(&self) -> usize {
            0
        }
        #[inline(always)]
        pub fn is_empty(&self) -> bool {
            true
        }
        #[inline(always)]
        pub fn max(&self) -> usize {
            0
        }
    }
}

#[cfg(all(test, feature = "collection"))]
mod tests {
    use super::*;

    #[test]
    fn ringbuffer_push_and_snapshot() {
        let rb = RingBuffer::new(4);
        rb.push(10);
        rb.push(20);
        rb.push(30);
        assert_eq!(rb.len(), 3);
        assert_eq!(rb.snapshot(), vec![10, 20, 30]);
    }

    #[test]
    fn ringbuffer_overflow_evicts_oldest() {
        let rb = RingBuffer::new(3);
        rb.push(1);
        rb.push(2);
        rb.push(3);
        rb.push(4); // evicts 1
        rb.push(5); // evicts 2

        assert_eq!(rb.len(), 3);
        assert_eq!(rb.snapshot(), vec![3, 4, 5]);
    }

    #[test]
    fn ringbuffer_clear() {
        let rb = RingBuffer::new(10);
        rb.push(1);
        rb.push(2);
        rb.clear();
        assert!(rb.is_empty());
        assert_eq!(rb.len(), 0);
    }

    #[test]
    fn ringbuffer_capacity() {
        let rb: RingBuffer<u32> = RingBuffer::new(16);
        assert_eq!(rb.capacity(), 16);
    }

    #[test]
    fn event_stream_push_and_drain() {
        let es = EventStream::new(10);
        assert!(es.push(1));
        assert!(es.push(2));
        assert!(es.push(3));
        assert_eq!(es.len(), 3);

        let drained = es.drain();
        assert_eq!(drained, vec![1, 2, 3]);
        assert!(es.is_empty());
    }

    #[test]
    fn event_stream_stops_at_max() {
        let es = EventStream::new(3);
        assert!(es.push(1));
        assert!(es.push(2));
        assert!(es.push(3));
        assert!(!es.push(4)); // full, rejected
        assert_eq!(es.len(), 3);

        let drained = es.drain();
        assert_eq!(drained, vec![1, 2, 3]);
    }

    #[test]
    fn event_stream_drain_allows_new_pushes() {
        let es = EventStream::new(2);
        assert!(es.push(1));
        assert!(es.push(2));
        assert!(!es.push(3)); // full

        es.drain();
        assert!(es.push(4));
        assert!(es.push(5));
        assert_eq!(es.drain(), vec![4, 5]);
    }
}
