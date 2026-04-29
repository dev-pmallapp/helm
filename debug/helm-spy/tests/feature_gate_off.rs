//! Verify that with the `collection` feature disabled (the default
//! for release builds), every primitive type collapses to a ZST and
//! the hot-path methods compile to nothing.
//!
//! Run with: `cargo test -p helm-spy --no-default-features
//!           --test feature_gate_off`
//!
//! The whole file is gated `#[cfg(not(feature = "collection"))]` so a
//! `cargo test --features=collection` (the workspace's default test
//! pass) does not fail it.

#![cfg(not(feature = "collection"))]

use helm_spy::primitives::{
    BranchRecord, CorrelHist2D, Counter, EventStream, HeatMap, Histogram, IndexedCounter,
    IntervalHistogram, PerVcpuCounter, RingBuffer, TraceRing,
};

#[test]
fn counter_is_zst() {
    assert_eq!(std::mem::size_of::<Counter>(), 0);
}

#[test]
fn per_vcpu_counter_is_zst() {
    assert_eq!(std::mem::size_of::<PerVcpuCounter>(), 0);
}

#[test]
fn indexed_counter_is_zst() {
    assert_eq!(std::mem::size_of::<IndexedCounter>(), 0);
}

#[test]
fn histogram_is_zst() {
    assert_eq!(std::mem::size_of::<Histogram>(), 0);
}

#[test]
fn interval_histogram_is_zst() {
    assert_eq!(std::mem::size_of::<IntervalHistogram>(), 0);
}

#[test]
fn heatmap_is_zst() {
    assert_eq!(std::mem::size_of::<HeatMap>(), 0);
}

#[test]
fn ringbuffer_is_zst() {
    assert_eq!(std::mem::size_of::<RingBuffer<u64>>(), 0);
}

#[test]
fn event_stream_is_zst() {
    assert_eq!(std::mem::size_of::<EventStream<u64>>(), 0);
}

#[test]
fn trace_ring_is_zst() {
    assert_eq!(std::mem::size_of::<TraceRing<u64>>(), 0);
    assert_eq!(std::mem::size_of::<TraceRing<BranchRecord>>(), 0);
}

#[test]
fn correl_hist_is_zst() {
    assert_eq!(std::mem::size_of::<CorrelHist2D>(), 0);
}

// BranchRecord is not feature-gated -- it is a POD `repr(C)` type and
// keeps its 32-byte layout in both builds. Asserted in
// `primitives/trace_ring.rs::live_tests` as well.
#[test]
fn branch_record_size_unchanged() {
    assert_eq!(std::mem::size_of::<BranchRecord>(), 32);
}

#[test]
fn counter_inc_compiles_to_nothing() {
    let c = Counter::new("noop");
    for _ in 0..1_000_000 {
        c.inc();
        c.add(7);
    }
    assert_eq!(c.value(), 0, "no-op counter must always report 0");
    assert_eq!(c.name(), "");
}

#[test]
fn per_vcpu_counter_inc_compiles_to_nothing() {
    let c = PerVcpuCounter::new("noop", 8);
    for _ in 0..1_000 {
        c.inc(0);
        c.add(1, 5);
    }
    assert_eq!(c.value(0), 0);
    assert_eq!(c.total(), 0);
    assert_eq!(c.num_vcpus(), 0);
    assert!(c.per_vcpu().is_empty());
}

#[test]
fn indexed_counter_inc_compiles_to_nothing() {
    let labels: &[&str] = &["a", "b", "c"];
    let ic = IndexedCounter::new("noop", labels);
    for _ in 0..1_000 {
        ic.inc(0);
        ic.add(1, 7);
    }
    assert_eq!(ic.total(), 0);
    assert_eq!(ic.fraction(0), 0.0);
    assert!(ic.is_empty());
    assert_eq!(ic.len(), 0);
    assert!(ic.table().is_empty());
}

#[test]
fn histogram_record_compiles_to_nothing() {
    let h = Histogram::new("noop", vec![10, 100, 1000]);
    for v in 0..1_000 {
        h.record(v);
    }
    assert!(h.counts().is_empty());
    assert_eq!(h.total(), 0);
    assert_eq!(h.percentile(0.5), 0);
}

#[test]
fn interval_histogram_tick_compiles_to_nothing() {
    let ih = IntervalHistogram::new("noop", vec![1, 10, 100], 16);
    for i in 0..10_000 {
        ih.tick(i, i);
    }
    assert_eq!(ih.total(), 0);
}

#[test]
fn heatmap_inc_compiles_to_nothing() {
    let hm = HeatMap::new("noop");
    for pc in 0..1_000u64 {
        hm.inc(pc);
    }
    assert_eq!(hm.len(), 0);
    assert!(hm.is_empty());
    assert!(hm.top(10).is_empty());
    assert_eq!(hm.get(0), 0);
}

#[test]
fn ringbuffer_push_compiles_to_nothing() {
    let rb: RingBuffer<u64> = RingBuffer::new(8);
    for v in 0..100u64 {
        rb.push(v);
    }
    assert_eq!(rb.len(), 0);
    assert!(rb.is_empty());
    assert!(rb.snapshot().is_empty());
    assert_eq!(rb.capacity(), 0);
}

#[test]
fn event_stream_push_compiles_to_nothing() {
    let es: EventStream<u64> = EventStream::new(16);
    for v in 0..100u64 {
        assert!(!es.push(v), "no-op event stream rejects all pushes");
    }
    assert!(es.is_empty());
    assert!(es.drain().is_empty());
    assert_eq!(es.max(), 0);
}

#[test]
fn trace_ring_push_compiles_to_nothing() {
    let ring: TraceRing<u64> = TraceRing::new(16);
    for v in 0..100u64 {
        assert!(!ring.push(v), "no-op trace ring rejects all pushes");
    }
    let mut out = Vec::new();
    ring.drain_into(&mut out);
    assert!(out.is_empty());
    assert_eq!(ring.len(), 0);
    assert_eq!(ring.capacity(), 0);
}

#[test]
fn correl_hist_record_compiles_to_nothing() {
    let ch = CorrelHist2D::new("noop", vec![10, 100], vec![5, 50]);
    for x in 0..100u64 {
        ch.record(x, x);
    }
    assert_eq!(ch.total(), 0);
    assert_eq!(ch.x_buckets(), 0);
    assert_eq!(ch.y_buckets(), 0);
    assert!(ch.matrix().is_empty());
}
