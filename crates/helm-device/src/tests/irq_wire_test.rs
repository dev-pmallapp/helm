use crate::irq_wire::{IrqSink, IrqWire};
use std::sync::{Arc, Mutex};

/// Mock IRQ sink that records level changes.
struct MockSink {
    levels: Mutex<Vec<(u32, bool)>>,
}

impl MockSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            levels: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<(u32, bool)> {
        self.levels.lock().unwrap().clone()
    }
}

impl IrqSink for MockSink {
    fn set_level(&self, line: u32, level: bool) {
        self.levels.lock().unwrap().push((line, level));
    }
}

#[test]
fn connect_assert_deassert() {
    let sink = MockSink::new();
    let mut wire = IrqWire::new(33);
    wire.connect(sink.clone());
    assert!(wire.is_connected());

    wire.set_level(true);
    wire.set_level(false);

    let calls = sink.calls();
    assert_eq!(calls, vec![(33, true), (33, false)]);
}

#[test]
fn disconnect_silences() {
    let sink = MockSink::new();
    let mut wire = IrqWire::new(10);
    wire.connect(sink.clone());

    wire.set_level(true);
    wire.disconnect();
    assert!(!wire.is_connected());

    // This should be silently dropped
    wire.set_level(false);

    // Only one call should have reached the sink
    assert_eq!(sink.calls().len(), 1);
}

#[test]
fn reconnect() {
    let sink1 = MockSink::new();
    let sink2 = MockSink::new();

    let mut wire = IrqWire::new(5);

    wire.connect(sink1.clone());
    wire.set_level(true);

    wire.disconnect();
    wire.connect(sink2.clone());
    wire.set_level(false);

    assert_eq!(sink1.calls(), vec![(5, true)]);
    assert_eq!(sink2.calls(), vec![(5, false)]);
}

#[test]
fn unconnected_wire_silently_drops() {
    let wire = IrqWire::new(0);
    // Should not panic
    wire.set_level(true);
    wire.set_level(false);
}

#[test]
fn debug_impl() {
    let wire = IrqWire::new(7);
    let dbg = format!("{:?}", wire);
    assert!(dbg.contains("7"));
    assert!(dbg.contains("false"));
}

#[test]
fn line_accessor() {
    let wire = IrqWire::new(42);
    assert_eq!(wire.line(), 42);
}
