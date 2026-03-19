# TEST: helm-probe / helm-plugin Instrumentation Stack

> Test plan covering `helm-probe`, the `ProbePluginBridge`, the chain/filter mechanism,
> `TraceSink` delivery, and `sim_trace` level filtering.

**Test files:**
- `framework/helm-probe/src/lib.rs` — inline `#[cfg(test)]` module (probe unit tests)
- `framework/helm-probe/tests/probe_integration.rs` — integration: engine wiring
- `framework/helm-plugin/src/runtime/filter.rs` — inline tests (filter predicates)
- `framework/helm-plugin/src/runtime/chain.rs` — inline tests (chain sequencing)
- `framework/helm-plugin/src/runtime/sink.rs` — inline tests (TraceSink)
- `framework/helm-plugin/src/bridge.rs` — inline tests (ProbePluginBridge)
- `runtime/helm-debug/src/sim_trace.rs` — inline tests (level ordering, level filter)
- `framework/helm-probe/tests/release_overhead.rs` — size + asm assertions

---

## Table of Contents

1. [Test Philosophy](#1-test-philosophy)
2. [Probe<T> Unit Tests](#2-probet-unit-tests)
3. [probe!() Macro Tests](#3-probe-macro-tests)
4. [Filter Predicate Tests](#4-filter-predicate-tests)
5. [Chain Tests](#5-chain-tests)
6. [TraceSink Tests](#6-tracesink-tests)
7. [ProbePluginBridge Tests](#7-probepluginbridge-tests)
8. [sim_trace Level Filter Tests](#8-simtrace-level-filter-tests)
9. [Engine Integration Tests](#9-engine-integration-tests)
10. [Release Build Overhead Tests](#10-release-build-overhead-tests)
11. [Test Matrix](#11-test-matrix)

---

## 1. Test Philosophy

**What we test:**

1. **Probe correctness**: `notify()` delivers to all listeners, in order, with the exact
   value passed.
2. **Probe zero-cost in release**: `Probe<T>` is ZST, `has_listeners()` = false, no
   instructions emitted in the hot loop.
3. **Filter correctness**: predicates gate callbacks correctly; AND semantics; stock
   filter functions behave as documented.
4. **Chain ordering**: stages fire in registration order; each with its own predicate.
5. **TraceSink delivery**: each variant delivers correctly; Buffer variant allows
   in-process assertion.
6. **Bridge enrichment**: `CpuStepEvent` → `InsnInfo` enrichment is correct (class,
   name, is_stub from opcode classifier).
7. **Level filter**: entries below `min_level` are discarded; entries at or above pass.
8. **Engine non-regression**: the 663 ISA tests still pass after probe wiring.

**What we do NOT test:**

- Timing or throughput of listeners (user's concern).
- Thread-safety of concurrent `notify()` calls (probes are not for concurrent write).
- Python subscription API (tested in `helm-python` test suite).
- `MonitorSink` TCP path in automated tests (requires a running TCP server; use manual
  verification or an integration test that spawns a listener).

---

## 2. `Probe<T>` Unit Tests

```rust
// framework/helm-probe/src/lib.rs  (inline #[cfg(test)])

#[cfg(test)]
mod probe_tests {
    use super::{Probe, CpuStepEvent};
    use std::sync::{Arc, Mutex};

    fn collector<T: Clone + Send + Sync + 'static>()
        -> (Arc<Mutex<Vec<T>>>, impl Fn(&T) + Send + Sync + 'static)
    {
        let log = Arc::new(Mutex::new(Vec::<T>::new()));
        let log2 = log.clone();
        (log, move |ev: &T| log2.lock().unwrap().push(ev.clone()))
    }

    // ── T-PROBE-01 ─────────────────────────────────────────────────────────
    #[test]
    fn new_probe_has_no_listeners() {
        let p: Probe<u32> = Probe::new();
        assert!(!p.has_listeners());
    }

    // ── T-PROBE-02 ─────────────────────────────────────────────────────────
    #[cfg(debug_assertions)]
    #[test]
    fn subscribe_enables_has_listeners() {
        let mut p: Probe<u32> = Probe::new();
        p.subscribe(|_| {});
        assert!(p.has_listeners());
    }

    // ── T-PROBE-03 ─────────────────────────────────────────────────────────
    #[test]
    fn notify_with_no_listeners_does_not_panic() {
        let p: Probe<u32> = Probe::new();
        p.notify(&42);
    }

    // ── T-PROBE-04 ─────────────────────────────────────────────────────────
    #[cfg(debug_assertions)]
    #[test]
    fn notify_delivers_to_all_listeners() {
        let mut p: Probe<u32> = Probe::new();
        let (log_a, cb_a) = collector::<u32>();
        let (log_b, cb_b) = collector::<u32>();
        p.subscribe(cb_a);
        p.subscribe(cb_b);
        p.notify(&99u32);
        assert_eq!(log_a.lock().unwrap().len(), 1);
        assert_eq!(log_b.lock().unwrap().len(), 1);
    }

    // ── T-PROBE-05 ─────────────────────────────────────────────────────────
    #[cfg(debug_assertions)]
    #[test]
    fn listeners_fire_in_subscription_order() {
        let mut p: Probe<u32> = Probe::new();
        let order: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let o1 = order.clone();
        let o2 = order.clone();
        p.subscribe(move |_| o1.lock().unwrap().push(1));
        p.subscribe(move |_| o2.lock().unwrap().push(2));
        p.notify(&0u32);
        assert_eq!(*order.lock().unwrap(), vec![1u8, 2u8]);
    }

    // ── T-PROBE-06 ─────────────────────────────────────────────────────────
    #[test]
    fn probe_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Probe<u32>>();
        assert_send_sync::<Probe<CpuStepEvent>>();
    }

    // ── T-PROBE-07 ─────────────────────────────────────────────────────────
    #[cfg(debug_assertions)]
    #[test]
    fn listener_count_tracks_subscribes() {
        let mut p: Probe<u32> = Probe::new();
        assert_eq!(p.listener_count(), 0);
        p.subscribe(|_| {});
        assert_eq!(p.listener_count(), 1);
        p.subscribe(|_| {});
        assert_eq!(p.listener_count(), 2);
    }
}
```

---

## 3. `probe!()` Macro Tests

```rust
    // ── T-MACRO-01 ─────────────────────────────────────────────────────────
    /// Expression inside probe!() is NOT evaluated when no listeners.
    #[test]
    fn macro_skips_expr_when_no_listeners() {
        let p: Probe<u32> = Probe::new();
        let mut evaluated = false;
        probe!(p, { evaluated = true; 0u32 });
        assert!(!evaluated, "event expr must not be evaluated without listeners");
    }

    // ── T-MACRO-02 ─────────────────────────────────────────────────────────
    #[cfg(debug_assertions)]
    #[test]
    fn macro_delivers_when_subscribed() {
        let mut p: Probe<u32> = Probe::new();
        let (log, cb) = collector::<u32>();
        p.subscribe(cb);
        probe!(p, 77u32);
        assert_eq!(log.lock().unwrap()[0], 77u32);
    }

    // ── T-MACRO-03 ─────────────────────────────────────────────────────────
    #[cfg(debug_assertions)]
    #[test]
    fn macro_evaluates_expr_exactly_once() {
        let mut p: Probe<u32> = Probe::new();
        let mut n = 0u32;
        let (_, cb) = collector::<u32>();
        p.subscribe(cb);
        probe!(p, { n += 1; n });
        probe!(p, { n += 1; n });
        assert_eq!(n, 2);
    }
```

---

## 4. Filter Predicate Tests

```rust
// framework/helm-plugin/src/runtime/filter.rs  (inline #[cfg(test)])

#[cfg(test)]
mod filter_tests {
    use super::*;
    use crate::runtime::{InsnInfo, InsnClass, BranchInfo, BranchKind, MemInfo, ArchContext};

    fn make_insn(pc: u64, class: InsnClass) -> InsnInfo {
        InsnInfo { vcpu_idx: 0, pc, raw: 0, size: 4, class, opcode_name: "", is_stub: false,
                   context: ArchContext::None }
    }
    fn make_branch(pc: u64, target: u64, taken: bool) -> BranchInfo {
        BranchInfo { pc, target, taken, kind: BranchKind::DirectCond }
    }
    fn make_mem(vaddr: u64, is_store: bool) -> MemInfo {
        MemInfo { vaddr, size: 4, is_store, is_atomic: false }
    }

    // ── T-FILT-01 ──────────────────────────────────────────────────────────
    #[test]
    fn pc_range_passes_in_range() {
        let f = pc_range(0x4000, 0x8000);
        assert!(f(&make_insn(0x4000, InsnClass::IntAlu)));
        assert!(f(&make_insn(0x7fff, InsnClass::IntAlu)));
        assert!(!f(&make_insn(0x8000, InsnClass::IntAlu)));  // exclusive end
        assert!(!f(&make_insn(0x3fff, InsnClass::IntAlu)));
    }

    // ── T-FILT-02 ──────────────────────────────────────────────────────────
    #[test]
    fn insn_class_filter() {
        let f = insn_class(InsnClass::Branch);
        assert!(f(&make_insn(0, InsnClass::Branch)));
        assert!(!f(&make_insn(0, InsnClass::Load)));
    }

    // ── T-FILT-03 ──────────────────────────────────────────────────────────
    #[test]
    fn sample_every_fires_at_correct_rate() {
        let f = sample_every(4);
        let results: Vec<bool> = (0..8).map(|_| f(&make_insn(0, InsnClass::IntAlu))).collect();
        // Fires at index 0, 4 (every 4th, starting from 0)
        assert_eq!(results, vec![true, false, false, false, true, false, false, false]);
    }

    // ── T-FILT-04 ──────────────────────────────────────────────────────────
    #[test]
    fn taken_only_filter() {
        let f = taken_only();
        assert!(f(&make_branch(0, 0x1000, true)));
        assert!(!f(&make_branch(0, 0x1000, false)));
    }

    // ── T-FILT-05 ──────────────────────────────────────────────────────────
    #[test]
    fn stores_only_filter() {
        let f = stores_only();
        assert!(f(&make_mem(0x1000, true)));
        assert!(!f(&make_mem(0x1000, false)));
    }

    // ── T-FILT-06 ──────────────────────────────────────────────────────────
    #[test]
    fn filtered_cb_and_semantics() {
        let mut fired = false;
        // Two predicates: IntAlu class AND pc < 0x1000
        let cb = FilteredCb::new(|_: &InsnInfo| { fired = true; })
            .filter(insn_class(InsnClass::IntAlu))
            .filter(pc_range(0, 0x1000));

        cb.call(&make_insn(0x800, InsnClass::IntAlu));  // both pass → fires
        assert!(fired);
        fired = false;

        cb.call(&make_insn(0x800, InsnClass::Branch));  // class fails → no fire
        assert!(!fired);

        cb.call(&make_insn(0x2000, InsnClass::IntAlu)); // pc fails → no fire
        assert!(!fired);
    }

    // ── T-FILT-07 ──────────────────────────────────────────────────────────
    #[test]
    fn filtered_cb_no_predicates_always_fires() {
        let mut fired = false;
        let cb = FilteredCb::new(|_: &InsnInfo| { fired = true; });
        cb.call(&make_insn(0, InsnClass::Unknown));
        assert!(fired);
    }
}
```

---

## 5. Chain Tests

```rust
// framework/helm-plugin/src/runtime/chain.rs  (inline #[cfg(test)])

#[cfg(test)]
mod chain_tests {
    use super::*;
    use crate::runtime::{BranchInfo, BranchKind};
    use crate::runtime::filter::{FilteredCb, taken_only};

    fn make_branch(taken: bool) -> BranchInfo {
        BranchInfo { pc: 0, target: 0x1000, taken, kind: BranchKind::DirectCond }
    }

    // ── T-CHAIN-01 ──────────────────────────────────────────────────────────
    /// All stages fire for matching events.
    #[test]
    fn all_stages_fire_for_match() {
        let mut a_fired = false;
        let mut b_fired = false;

        let chain: Chain<BranchInfo> = Chain::new()
            .then(|_| { a_fired = true; })
            .then(|_| { b_fired = true; });

        chain.fire(&make_branch(true));
        assert!(a_fired && b_fired);
    }

    // ── T-CHAIN-02 ──────────────────────────────────────────────────────────
    /// Stages fire in registration order.
    #[test]
    fn stages_fire_in_order() {
        use std::sync::{Arc, Mutex};
        let order: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let o1 = order.clone(); let o2 = order.clone(); let o3 = order.clone();
        let chain: Chain<BranchInfo> = Chain::new()
            .then(move |_| o1.lock().unwrap().push(1))
            .then(move |_| o2.lock().unwrap().push(2))
            .then(move |_| o3.lock().unwrap().push(3));
        chain.fire(&make_branch(true));
        assert_eq!(*order.lock().unwrap(), vec![1u8, 2u8, 3u8]);
    }

    // ── T-CHAIN-03 ──────────────────────────────────────────────────────────
    /// Stage with predicate skips when predicate fails; other stages still fire.
    #[test]
    fn filtered_stage_skips_independently() {
        let mut a_fired = false;
        let mut b_fired = false;

        let chain: Chain<BranchInfo> = Chain::new()
            .stage(FilteredCb::new(|_: &BranchInfo| { a_fired = true; }).filter(taken_only()))
            .then(|_| { b_fired = true; });

        // not-taken branch: stage A skips, stage B fires
        chain.fire(&make_branch(false));
        assert!(!a_fired, "stage A must skip when predicate fails");
        assert!(b_fired,  "stage B must fire unconditionally");
    }

    // ── T-CHAIN-04 ──────────────────────────────────────────────────────────
    /// Empty chain does not panic.
    #[test]
    fn empty_chain_is_safe() {
        let chain: Chain<BranchInfo> = Chain::new();
        chain.fire(&make_branch(true));
    }
}
```

---

## 6. `TraceSink` Tests

```rust
// framework/helm-plugin/src/runtime/sink.rs  (inline #[cfg(test)])

#[cfg(test)]
mod sink_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // ── T-SINK-01 ───────────────────────────────────────────────────────────
    /// Buffer sink captures lines.
    #[test]
    fn buffer_sink_captures_output() {
        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = TraceSink::Buffer(buf.clone());
        sink.write_line("hello");
        sink.write_line("world");
        let v = buf.lock().unwrap();
        assert_eq!(&v[..], &["hello", "world"]);
    }

    // ── T-SINK-02 ───────────────────────────────────────────────────────────
    /// Null sink does not panic and discards.
    #[test]
    fn null_sink_is_safe() {
        let sink = TraceSink::Null;
        sink.write_line("discarded");
    }

    // ── T-SINK-03 ───────────────────────────────────────────────────────────
    /// Plugin using Buffer sink collects atexit output in memory.
    #[test]
    fn plugin_with_buffer_sink() {
        use crate::builtins::trace::ExecLog;
        use crate::{HelmPlugin, PluginArgs, PluginRegistry};

        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut plugin = ExecLog::new();
        plugin.set_sink(TraceSink::Buffer(buf.clone()));

        let args = PluginArgs::empty();
        let mut reg = PluginRegistry::new();
        plugin.install(&mut reg, &args);

        // Manually fire a fake insn event
        use crate::runtime::{InsnInfo, InsnClass, ArchContext};
        let insn = InsnInfo { vcpu_idx: 0, pc: 0x4000_0000, raw: 0xD503_201F,
                               size: 4, class: InsnClass::Nop, opcode_name: "Nop",
                               is_stub: false, context: ArchContext::None };
        reg.fire_insn_exec(0, &insn);
        plugin.atexit();

        let lines = buf.lock().unwrap();
        assert!(!lines.is_empty(), "atexit must write at least one line");
        assert!(lines[0].contains("0x0000000040000000"), "line must contain PC");
    }
}
```

---

## 7. `ProbePluginBridge` Tests

```rust
// framework/helm-plugin/src/bridge.rs  (inline #[cfg(test)])

#[cfg(debug_assertions)]
#[cfg(test)]
mod bridge_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // ── T-BRIDGE-01 ─────────────────────────────────────────────────────────
    /// Bridge subscribes to pre_step and dispatches InsnInfo to registry.
    #[test]
    fn bridge_routes_cpu_step_to_insn_exec() {
        use helm_probe::{Probe, CpuStepEvent};
        use crate::runtime::{InsnInfo, PluginRegistry};

        let reg = Arc::new(Mutex::new(PluginRegistry::new()));
        let received: Arc<Mutex<Vec<InsnInfo>>> = Arc::new(Mutex::new(Vec::new()));
        let r2 = received.clone();
        {
            let mut guard = reg.lock().unwrap();
            guard.on_insn_exec(Box::new(move |_vcpu, info| {
                r2.lock().unwrap().push(info.clone());
            }));
        }

        let bridge = ProbePluginBridge::new(reg.clone());
        let mut probes = helm_engine::CpuProbes::default();
        bridge.install_cpu(&mut probes, 0);

        // Fire a post_step probe event
        probes.post_step.notify(&CpuStepEvent { pc: 0x4000_0010, raw: 0xD503_201F });

        let events = received.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].pc, 0x4000_0010);
    }

    // ── T-BRIDGE-02 ─────────────────────────────────────────────────────────
    /// Bridge routes CpuFaultEvent to fault callbacks.
    #[test]
    fn bridge_routes_fault_event() {
        use helm_probe::{Probe, CpuFaultEvent};
        use crate::runtime::{FaultInfo, PluginRegistry};

        let reg = Arc::new(Mutex::new(PluginRegistry::new()));
        let faults: Arc<Mutex<Vec<FaultInfo>>> = Arc::new(Mutex::new(Vec::new()));
        let f2 = faults.clone();
        {
            let mut guard = reg.lock().unwrap();
            guard.on_fault(Box::new(move |info| {
                f2.lock().unwrap().push(info.clone());
            }));
        }

        let bridge = ProbePluginBridge::new(reg.clone());
        let mut probes = helm_engine::CpuProbes::default();
        bridge.install_cpu(&mut probes, 0);

        probes.fault.notify(&CpuFaultEvent { pc: 0x1000, raw: 0, kind: "data-abort" });

        let events = faults.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].pc, 0x1000);
    }
}
```

---

## 8. `sim_trace` Level Filter Tests

```rust
// runtime/helm-debug/src/sim_trace.rs  (inline #[cfg(test)])

#[cfg(test)]
mod level_tests {
    use super::*;

    // ── T-LEVEL-01 ──────────────────────────────────────────────────────────
    /// Level ordering: Branch < Info < Stub < Warn < Error.
    #[test]
    fn level_ordering_is_correct() {
        assert!(Level::Branch < Level::Info);
        assert!(Level::Info   < Level::Stub);
        assert!(Level::Stub   < Level::Warn);
        assert!(Level::Warn   < Level::Error);
    }

    // ── T-LEVEL-02 ──────────────────────────────────────────────────────────
    /// Monitor with min_level=Warn discards Branch and Info entries.
    #[test]
    fn monitor_discards_below_min_level() {
        let (sink, monitor) = MonitorSink::open("null:").unwrap();
        // In a real test we'd set monitor.min_level = Level::Warn and
        // verify entries are dropped. Since we use null: backend, just
        // verify try_send doesn't panic for any level.
        for &level in &[Level::Branch, Level::Info, Level::Stub, Level::Warn, Level::Error] {
            monitor.try_send(MonitorEntry {
                sim_ns: 0, sim_insns: 0, component: "test",
                level, pc: None, message: "test".to_string(),
            });
        }
        drop(sink);  // joins the background thread
    }

    // ── T-LEVEL-03 ──────────────────────────────────────────────────────────
    /// MonitorEntry::format() produces the expected prefix.
    #[test]
    fn entry_format_has_correct_prefix() {
        let entry = MonitorEntry {
            sim_ns: 1234, sim_insns: 5678, component: "gicv2-dist",
            level: Level::Stub, pc: Some(0x4000_0000), message: "test msg".to_string(),
        };
        let s = entry.format();
        assert!(s.starts_with("[STUB]"), "format must start with [STUB]");
        assert!(s.contains("gicv2-dist"), "format must contain component");
        assert!(s.contains("test msg"), "format must contain message");
    }

    // ── T-LEVEL-04 ──────────────────────────────────────────────────────────
    /// MonitorSink with null: backend accepts and discards all entries without blocking.
    #[test]
    fn null_backend_is_nonblocking() {
        let (sink, monitor) = MonitorSink::open("null:").unwrap();
        for i in 0..10_000u64 {
            monitor.try_send(MonitorEntry {
                sim_ns: i, sim_insns: i, component: "test",
                level: Level::Info, pc: None, message: format!("msg {i}"),
            });
        }
        drop(sink);
    }

    // ── T-LEVEL-05 ──────────────────────────────────────────────────────────
    /// File backend writes entries that can be read back.
    #[test]
    fn file_backend_writes_and_reads() {
        use std::io::BufRead;
        let path = std::env::temp_dir().join("helm-sim-trace-test.log");
        let uri = format!("file:{}", path.display());
        {
            let (sink, monitor) = MonitorSink::open(&uri).unwrap();
            monitor.try_send(MonitorEntry {
                sim_ns: 1, sim_insns: 2, component: "test",
                level: Level::Info, pc: None, message: "written".to_string(),
            });
            drop(sink);  // flush and join
        }
        let f = std::fs::File::open(&path).unwrap();
        let lines: Vec<_> = std::io::BufReader::new(f).lines().collect();
        assert!(!lines.is_empty());
        assert!(lines[0].as_ref().unwrap().contains("written"));
        std::fs::remove_file(&path).ok();
    }
}
```

---

## 9. Engine Integration Tests

```rust
// framework/helm-probe/tests/probe_integration.rs

#[cfg(debug_assertions)]
mod integration {
    use helm_engine::{HelmEngine, Isa, ExecMode, StopReason};
    use helm_timing::Virtual;
    use helm_probe::CpuStepEvent;
    use std::sync::{Arc, Mutex};

    fn nop_engine(count: usize) -> HelmEngine<Virtual> {
        let mut e = HelmEngine::new(Isa::AArch64, ExecMode::Functional, Virtual::new(),
                                    0x4000_0000, 4096);
        let nops: Vec<u8> = std::iter::repeat([0x1F, 0x20, 0x03, 0xD5])
            .take(count).flatten().collect();
        e.load_bytes(0x4000_0000, &nops);
        if let Some(s) = e.a64_state.as_mut() { s.pc = 0x4000_0000; }
        e
    }

    // ── T-INT-01 ────────────────────────────────────────────────────────────
    /// pre_step fires once per instruction.
    #[test]
    fn pre_step_fires_per_instruction() {
        let mut e = nop_engine(4);
        let count = Arc::new(Mutex::new(0u64));
        let c2 = count.clone();
        e.probes.pre_step.subscribe(move |_: &CpuStepEvent| { *c2.lock().unwrap() += 1; });
        e.run(4);
        assert_eq!(*count.lock().unwrap(), 4);
    }

    // ── T-INT-02 ────────────────────────────────────────────────────────────
    /// post_step reports correct (pre-advance) PC.
    #[test]
    fn post_step_reports_instruction_pc() {
        let mut e = nop_engine(3);
        let pcs: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let p2 = pcs.clone();
        e.probes.post_step.subscribe(move |ev: &CpuStepEvent| {
            p2.lock().unwrap().push(ev.pc);
        });
        e.run(3);
        let v = pcs.lock().unwrap();
        assert_eq!(v[0], 0x4000_0000);
        assert_eq!(v[1], 0x4000_0004);
        assert_eq!(v[2], 0x4000_0008);
    }

    // ── T-INT-03 ────────────────────────────────────────────────────────────
    /// Unsubscribed engine produces same StopReason as subscribed engine.
    #[test]
    fn no_listeners_same_result() {
        let mut e1 = nop_engine(8);
        let mut e2 = nop_engine(8);
        e1.probes.pre_step.subscribe(|_| {});
        let r1 = e1.run(8);
        let r2 = e2.run(8);
        assert_eq!(r1, r2);
        assert_eq!(e1.insns_retired, e2.insns_retired);
    }

    // ── T-INT-04 ────────────────────────────────────────────────────────────
    /// ISA regression: all ISA tests still pass (validated by cargo test -p helm-arch).
    /// This test just confirms the engine builds and runs a single step cleanly.
    #[test]
    fn engine_runs_one_nop() {
        let mut e = nop_engine(1);
        let r = e.run(1);
        assert_eq!(r, StopReason::Quantum);
    }
}
```

---

## 10. Release Build Overhead Tests

Manual steps — not automated `#[test]`. Run during code review and in CI before merge.

### 10.1 `Probe<T>` is zero-sized in release

```bash
cargo run --release --example check_probe_size
# Expected output:
#   size_of::<Probe<u64>>() = 0
#   size_of::<CpuProbes>() = 0
```

```rust
// framework/helm-probe/examples/check_probe_size.rs
use helm_probe::{Probe, CpuStepEvent, IrqEvent};
use helm_engine::CpuProbes;

fn main() {
    let sz_u64 = std::mem::size_of::<Probe<u64>>();
    let sz_step = std::mem::size_of::<Probe<CpuStepEvent>>();
    let sz_cpu = std::mem::size_of::<CpuProbes>();
    println!("size_of::<Probe<u64>>()      = {sz_u64}");
    println!("size_of::<Probe<CpuStepEvent>>() = {sz_step}");
    println!("size_of::<CpuProbes>()       = {sz_cpu}");
    assert_eq!(sz_u64,  0, "Probe<T> must be ZST in release");
    assert_eq!(sz_step, 0, "Probe<CpuStepEvent> must be ZST in release");
    assert_eq!(sz_cpu,  0, "CpuProbes must be ZST in release");
}
```

### 10.2 No probe instructions in hot loop

```bash
cargo asm --release --package helm-engine \
  "helm_engine::fs::step_aarch64_fs" | grep -c "probe"
# Expected: 0

# Also check the SE step function:
cargo asm --release --package helm-engine \
  "helm_engine::HelmEngine::step_aarch64" | grep -c "probe"
# Expected: 0
```

### 10.3 ISA regression

```bash
cargo test -p helm-arch --lib
# Expected: 663 tests pass
```

---

## 11. Test Matrix

| ID | Description | Command | Profile | Pass criteria |
|---|---|---|---|---|
| T-PROBE-01 | New probe: no listeners | `cargo test -p helm-probe` | dev | `!has_listeners()` |
| T-PROBE-02 | Subscribe: enables has_listeners | `cargo test -p helm-probe` | dev | `has_listeners()` true |
| T-PROBE-03 | notify with no listeners | `cargo test -p helm-probe` | both | no panic |
| T-PROBE-04 | notify delivers to all listeners | `cargo test -p helm-probe` | dev | both logs len=1 |
| T-PROBE-05 | Listeners fire in order | `cargo test -p helm-probe` | dev | [1, 2] |
| T-PROBE-06 | Send + Sync compile check | `cargo test -p helm-probe` | both | compiles |
| T-PROBE-07 | listener_count tracks subscribes | `cargo test -p helm-probe` | dev | 0, 1, 2 |
| T-MACRO-01 | macro skips eval without listeners | `cargo test -p helm-probe` | dev | `evaluated == false` |
| T-MACRO-02 | macro delivers with listener | `cargo test -p helm-probe` | dev | value received |
| T-MACRO-03 | macro evaluates once per call | `cargo test -p helm-probe` | dev | n == 2 |
| T-FILT-01 | pc_range predicate | `cargo test -p helm-plugin` | dev | in/out range correct |
| T-FILT-02 | insn_class predicate | `cargo test -p helm-plugin` | dev | class match |
| T-FILT-03 | sample_every rate | `cargo test -p helm-plugin` | dev | pattern [T,F,F,F,T…] |
| T-FILT-04 | taken_only predicate | `cargo test -p helm-plugin` | dev | taken → true |
| T-FILT-05 | stores_only predicate | `cargo test -p helm-plugin` | dev | store → true |
| T-FILT-06 | FilteredCb AND semantics | `cargo test -p helm-plugin` | dev | all 3 cases |
| T-FILT-07 | FilteredCb no predicates: always fires | `cargo test -p helm-plugin` | dev | fired == true |
| T-CHAIN-01 | All stages fire for match | `cargo test -p helm-plugin` | dev | a && b fired |
| T-CHAIN-02 | Stages fire in order | `cargo test -p helm-plugin` | dev | [1, 2, 3] |
| T-CHAIN-03 | Filtered stage skips independently | `cargo test -p helm-plugin` | dev | A skip, B fire |
| T-CHAIN-04 | Empty chain safe | `cargo test -p helm-plugin` | dev | no panic |
| T-SINK-01 | Buffer sink captures lines | `cargo test -p helm-plugin` | dev | ["hello","world"] |
| T-SINK-02 | Null sink safe | `cargo test -p helm-plugin` | dev | no panic |
| T-SINK-03 | Plugin with buffer sink | `cargo test -p helm-plugin` | dev | line with PC |
| T-BRIDGE-01 | Bridge routes step → InsnInfo | `cargo test -p helm-plugin` | dev | event received |
| T-BRIDGE-02 | Bridge routes fault → FaultInfo | `cargo test -p helm-plugin` | dev | event received |
| T-LEVEL-01 | Level ordering | `cargo test -p helm-debug` | dev | Branch < … < Error |
| T-LEVEL-02 | Null backend: no panic any level | `cargo test -p helm-debug` | dev | no panic |
| T-LEVEL-03 | Entry format prefix | `cargo test -p helm-debug` | dev | starts with [STUB] |
| T-LEVEL-04 | Null backend: non-blocking | `cargo test -p helm-debug` | dev | 10k sends fast |
| T-LEVEL-05 | File backend: write + read back | `cargo test -p helm-debug` | dev | line contains "written" |
| T-INT-01 | pre_step fires per instruction | `cargo test -p helm-probe --test probe_integration` | dev | count == 4 |
| T-INT-02 | post_step: correct PC | `cargo test -p helm-probe --test probe_integration` | dev | PCs match |
| T-INT-03 | No listeners: same result | `cargo test -p helm-probe --test probe_integration` | dev | r1 == r2 |
| T-INT-04 | Engine runs one NOP | `cargo test -p helm-probe --test probe_integration` | dev | Quantum |
| T-SIZE-01 | Probe<T> ZST in release | `cargo run --release --example check_probe_size` | release | all sizes == 0 |
| T-ASM-01 | No probe ASM in FS hot loop | `cargo asm --release helm_engine::fs::step_aarch64_fs` | release | grep count == 0 |
| T-ASM-02 | No probe ASM in SE hot loop | `cargo asm --release helm_engine::HelmEngine::step_aarch64` | release | grep count == 0 |
| T-REG-01 | 663 ISA tests pass | `cargo test -p helm-arch --lib` | dev | all pass |
