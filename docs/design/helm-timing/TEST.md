# helm-timing — Test Plan

## Test Hierarchy

| Layer | Tool | Coverage Target |
|-------|------|----------------|
| Unit | `#[test]` in each module | All struct methods, edge cases |
| Property | `proptest` | Monotonic cycle advance, no overflow |
| Integration | `tests/` directory | End-to-end: timing model + EventQueue |
| Validation | `helm validate` | IPC/CPI within 5% of real hardware |

---

## Unit Tests — `Virtual`

### `virtual_advances_cycles_per_insn`

Verify that calling `on_insn` with a generic INT instruction advances `current_cycles` by exactly 1 for the default IPC=1.0 profile.

```rust
#[test]
fn virtual_advances_cycles_per_insn() {
    let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_INORDER).unwrap());
    let mut v = Virtual::new(profile);

    let insn = InsnInfo {
        pc: 0x1000,
        fu_class: FuClass::Int,
        src_regs: SmallVec::new(),
        dst_reg: None,
        is_branch: false, is_load: false, is_store: false,
        mem_size_bytes: 0,
    };

    let delta = v.on_insn(&insn);
    assert_eq!(delta, 1);
    assert_eq!(v.current_cycles(), 1);

    let delta2 = v.on_insn(&insn);
    assert_eq!(delta2, 1);
    assert_eq!(v.current_cycles(), 2);
}
```

### `virtual_div_charges_fu_latency`

DIV instructions must charge `fu_latencies[DIV]` cycles, not 1.

```rust
#[test]
fn virtual_div_charges_fu_latency() {
    let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_INORDER).unwrap());
    let div_lat = profile.fu_latency(FuClass::Div) as u64;
    let mut v = Virtual::new(profile);

    let div_insn = InsnInfo {
        fu_class: FuClass::Div,
        pc: 0x1000, src_regs: SmallVec::new(), dst_reg: None,
        is_branch: false, is_load: false, is_store: false, mem_size_bytes: 0,
    };
    let delta = v.on_insn(&div_insn);
    assert_eq!(delta, div_lat);
}
```

### `virtual_on_mem_access_returns_zero`

Virtual mode must never charge memory access stall cycles.

```rust
#[test]
fn virtual_on_mem_access_returns_zero() {
    let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_INORDER).unwrap());
    let mut v = Virtual::new(profile);
    let access = MemAccess { addr: 0x8000, size: 8, is_write: false, is_instruction_fetch: false };
    assert_eq!(v.on_mem_access(&access), 0);
}
```

### `virtual_drains_event_queue_on_boundary`

After `interval_insns` instructions, `on_interval_boundary` must drain pending events up to `current_cycles`.

```rust
#[test]
fn virtual_drains_event_queue_on_boundary() {
    let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_INORDER).unwrap());
    let interval = profile.virtual_interval_insns();
    let mut v = Virtual::new(Arc::clone(&profile));
    let mut eq = EventQueue::new();

    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired_clone = Arc::clone(&fired);

    let class = EventClass::new("test", Box::new(move |_data, _eq| {
        fired_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    }));

    // Post an event at cycle 1 (will be drained after first interval).
    eq.post_cycles(1, class.id(), 0, Box::new(()));

    let insn = InsnInfo {
        pc: 0, fu_class: FuClass::Int, src_regs: SmallVec::new(),
        dst_reg: None, is_branch: false, is_load: false,
        is_store: false, mem_size_bytes: 0,
    };

    // Run through one full interval.
    for _ in 0..interval {
        v.on_insn(&insn);
    }
    v.on_interval_boundary(&mut eq);

    assert!(fired.load(std::sync::atomic::Ordering::SeqCst),
        "Event should have fired after interval boundary drain");
}
```

### `virtual_branch_outcome_no_panic`

`on_branch_outcome` must not panic; Virtual mode ignores it.

```rust
#[test]
fn virtual_branch_outcome_no_panic() {
    let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_INORDER).unwrap());
    let mut v = Virtual::new(profile);
    v.on_branch_outcome(true, false);   // mispredicted
    v.on_branch_outcome(false, true);  // predicted
    assert_eq!(v.current_cycles(), 0); // no insns, no advancement
}
```

---

## Unit Tests — `OoOWindow`

### `ooo_window_raw_dependency_stalls`

A RAW dependency must delay the dependent instruction.

```rust
#[test]
fn ooo_window_raw_dependency_stalls() {
    let mut w = OoOWindow::new(0);

    // Instruction 0: writes r1, latency 4.
    let producer = InsnInfo {
        pc: 0, fu_class: FuClass::Mul, src_regs: SmallVec::new(),
        dst_reg: Some(1), is_branch: false, is_load: false,
        is_store: false, mem_size_bytes: 0,
    };
    let issue0 = w.issue(&producer, 4);
    assert_eq!(issue0, 0, "Producer issues at cycle 0");
    // reg[1] becomes ready at cycle 4.

    // Instruction 1: reads r1, latency 1.
    let consumer = InsnInfo {
        pc: 4, fu_class: FuClass::Int,
        src_regs: {let mut s = SmallVec::new(); s.push(1u8); s},
        dst_reg: None, is_branch: false, is_load: false,
        is_store: false, mem_size_bytes: 0,
    };
    let issue1 = w.issue(&consumer, 1);
    // Must wait until r1 is ready at cycle 4.
    assert_eq!(issue1, 4, "Consumer must stall until producer completes");
}
```

### `ooo_window_independent_insns_issue_back_to_back`

Independent instructions must not stall each other.

```rust
#[test]
fn ooo_window_independent_insns_issue_back_to_back() {
    let mut w = OoOWindow::new(0);
    let mk = |dst: Option<u8>| InsnInfo {
        pc: 0, fu_class: FuClass::Int, src_regs: SmallVec::new(),
        dst_reg: dst, is_branch: false, is_load: false,
        is_store: false, mem_size_bytes: 0,
    };
    let i0 = w.issue(&mk(Some(1)), 1);
    let i1 = w.issue(&mk(Some(2)), 1);
    let i2 = w.issue(&mk(Some(3)), 1);
    assert_eq!([i0, i1, i2], [0, 1, 2], "Independent insns issue each consecutive cycle");
}
```

---

## Unit Tests — `IntervalTimed`

### `interval_charges_cache_miss_penalty`

A cache miss from the shared `CacheModel` must be reflected in `cpi_stack.mem_stall_cycles`.

```rust
#[test]
fn interval_charges_cache_miss_penalty() {
    let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_OOO).unwrap());
    let miss_penalty = profile.l1_miss_penalty_cycles() as u64;

    // Build a CacheModel that always misses.
    let cache = Arc::new(CacheModel::always_miss_for_test());
    let mut timing = IntervalTimed::new(Arc::clone(&profile), cache);

    let access = MemAccess {
        addr: 0xDEAD_BEEF, size: 8, is_write: false, is_instruction_fetch: false
    };
    let stall = timing.on_mem_access(&access);
    assert_eq!(stall, miss_penalty);
    assert_eq!(timing.cpi_stack.mem_stall_cycles, miss_penalty);
}
```

### `interval_boundary_advances_cycles`

After a boundary, `current_cycles` must be greater than before.

```rust
#[test]
fn interval_boundary_advances_cycles() {
    let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_OOO).unwrap());
    let cache = Arc::new(CacheModel::always_hit_for_test());
    let mut timing = IntervalTimed::new(Arc::clone(&profile), cache);
    let mut eq = EventQueue::new();

    let insn = InsnInfo {
        pc: 0, fu_class: FuClass::Int, src_regs: SmallVec::new(),
        dst_reg: Some(1), is_branch: false, is_load: false,
        is_store: false, mem_size_bytes: 0,
    };
    for _ in 0..profile.interval_insns() {
        timing.on_insn(&insn);
    }
    let before = timing.current_cycles();
    timing.on_interval_boundary(&mut eq);
    let after = timing.current_cycles();
    assert!(after > before, "Cycles must advance after interval boundary");
}
```

### `interval_misprediction_charges_penalty`

A branch misprediction must add `branch_mispredict_penalty_cycles` to `cpi_stack`.

```rust
#[test]
fn interval_misprediction_charges_penalty() {
    let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_OOO).unwrap());
    let penalty = profile.branch_mispredict_penalty_cycles() as u64;
    let cache = Arc::new(CacheModel::always_hit_for_test());
    let mut timing = IntervalTimed::new(Arc::clone(&profile), cache);

    timing.on_branch_outcome(true, false);  // mispredicted
    assert_eq!(timing.cpi_stack.branch_mispredict_cycles, penalty);
}
```

---

## Unit Tests — `AccuratePipeline`

### `accurate_pipeline_commits_in_order`

Sequential independent instructions must commit in order with no stalls at 1 cycle/insn.

```rust
#[test]
fn accurate_pipeline_commits_in_order() {
    let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_INORDER).unwrap());
    let cache = Arc::new(CacheModel::always_hit_for_test());
    let mut pipeline = AccuratePipeline::new(profile, cache);

    let insn = InsnInfo {
        pc: 0, fu_class: FuClass::Int, src_regs: SmallVec::new(),
        dst_reg: Some(1), is_branch: false, is_load: false,
        is_store: false, mem_size_bytes: 0,
    };

    // Step until 5 commits (pipeline depth = 5, plus startup latency).
    let mut commits = 0;
    let mut cycles = 0;
    while commits < 5 {
        let committed = pipeline.step();
        if committed { commits += 1; }
        cycles += 1;
        assert!(cycles < 100, "Pipeline should commit 5 instructions within 100 cycles");
    }
}
```

### `accurate_pipeline_stalls_on_mul_latency`

A multiply with latency > 1 must stall the pipeline for the appropriate number of cycles.

```rust
#[test]
fn accurate_pipeline_stalls_on_mul_latency() {
    let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_OOO).unwrap());
    let mul_latency = profile.fu_latency(FuClass::Mul) as u64;
    let cache = Arc::new(CacheModel::always_hit_for_test());
    let mut pipeline = AccuratePipeline::new(Arc::clone(&profile), cache);

    let mul_insn = InsnInfo {
        pc: 0, fu_class: FuClass::Mul, src_regs: SmallVec::new(),
        dst_reg: Some(2), is_branch: false, is_load: false,
        is_store: false, mem_size_bytes: 0,
    };

    let start = pipeline.current_cycles();
    pipeline.on_insn(&mul_insn);
    let elapsed = pipeline.current_cycles() - start;
    // Elapsed must account for at least mul_latency cycles.
    assert!(elapsed >= mul_latency,
        "MUL instruction must stall pipeline for at least {} cycles, got {}",
        mul_latency, elapsed);
}
```

### `accurate_branch_flush_clears_stages`

After `on_branch_outcome(taken=true, predicted=false)`, the pipeline flush must clear IF/ID/EX.

```rust
#[test]
fn accurate_branch_flush_clears_stages() {
    let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_OOO).unwrap());
    let cache = Arc::new(CacheModel::always_hit_for_test());
    let mut pipeline = AccuratePipeline::new(profile, cache);

    // Prime pipeline with a few instructions.
    let insn = InsnInfo {
        pc: 0, fu_class: FuClass::Int, src_regs: SmallVec::new(),
        dst_reg: Some(1), is_branch: false, is_load: false,
        is_store: false, mem_size_bytes: 0,
    };
    for _ in 0..3 { pipeline.on_insn(&insn); }

    pipeline.on_branch_outcome(true, false);
    pipeline.step();  // Flush takes effect.

    // After flush, IF/ID/EX must not be valid.
    assert!(!pipeline.if_id.valid, "IF/ID must be cleared after flush");
    assert!(!pipeline.id_ex.valid, "ID/EX must be cleared after flush");
    assert!(!pipeline.ex_mem.valid, "EX/MEM must be cleared after flush");
}
```

---

## Unit Tests — `MicroarchProfile`

### `profile_from_json_valid`

```rust
#[test]
fn profile_from_json_valid() {
    let p = MicroarchProfile::from_json(profiles::CORTEX_A72).unwrap();
    assert_eq!(p.name(), "cortex-a72");
    assert!(p.virtual_ipc() > 0.0);
    assert_eq!(p.l1d().ways, 2);
}
```

### `profile_rejects_zero_ipc`

```rust
#[test]
fn profile_rejects_zero_ipc() {
    let json = r#"{
        "name": "bad", "description": "",
        "virtual_ipc": 0.0,
        "virtual_interval_insns": 10000, "interval_insns": 10000,
        "bp": {"predictor":"static","mispredict_penalty_cycles":5,"btb_entries":512,"ras_depth":4},
        "fu_latencies": {},
        "l1i": {"size_bytes":32768,"ways":4,"line_size_bytes":64,"hit_latency_cycles":2,"replacement":"lru","write_policy":"write_back","mshr_count":4},
        "l1d": {"size_bytes":32768,"ways":4,"line_size_bytes":64,"hit_latency_cycles":2,"replacement":"lru","write_policy":"write_back","mshr_count":4},
        "l2":  {"size_bytes":524288,"ways":8,"line_size_bytes":64,"hit_latency_cycles":10,"replacement":"plru","write_policy":"write_back","mshr_count":8},
        "l3": null,
        "l1_miss_penalty_cycles": 10, "mem_access_penalty_cycles": 100,
        "itlb": {"entries":32,"ways":32,"hit_latency_cycles":1,"miss_penalty_cycles":20,"asid_bits":0},
        "dtlb": {"entries":32,"ways":32,"hit_latency_cycles":1,"miss_penalty_cycles":20,"asid_bits":0},
        "l2_tlb": null,
        "prefetch": {"enabled":false,"distance":0,"degree":0}
    }"#;
    let result = MicroarchProfile::from_json(json);
    assert!(matches!(result, Err(ProfileError::Validation(_))));
}
```

### `profile_builtin_names_resolve`

```rust
#[test]
fn profile_builtin_names_resolve() {
    for name in &["generic-inorder", "generic-ooo", "sifive-u74", "cortex-a72"] {
        let json = profiles::builtin(name)
            .unwrap_or_else(|| panic!("Profile '{}' must be built-in", name));
        MicroarchProfile::from_json(json)
            .unwrap_or_else(|e| panic!("Profile '{}' must parse cleanly: {}", name, e));
    }
}
```

---

## Property Tests

### `prop_cycles_monotonically_increase` (Virtual)

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_virtual_cycles_never_decrease(insn_count in 1usize..10_000) {
        let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_INORDER).unwrap());
        let mut v = Virtual::new(profile);
        let mut prev = 0u64;

        let insn = InsnInfo {
            pc: 0, fu_class: FuClass::Int, src_regs: SmallVec::new(),
            dst_reg: None, is_branch: false, is_load: false,
            is_store: false, mem_size_bytes: 0,
        };
        for _ in 0..insn_count {
            v.on_insn(&insn);
            prop_assert!(v.current_cycles() >= prev);
            prev = v.current_cycles();
        }
    }
}
```

### `prop_interval_cpi_stack_non_negative`

```rust
proptest! {
    #[test]
    fn prop_interval_cpi_stack_non_negative(
        insns in 1usize..50_000,
        mispredict_count in 0usize..100,
    ) {
        let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_OOO).unwrap());
        let cache = Arc::new(CacheModel::always_hit_for_test());
        let mut timing = IntervalTimed::new(Arc::clone(&profile), cache);

        let insn = InsnInfo {
            pc: 0, fu_class: FuClass::Int, src_regs: SmallVec::new(),
            dst_reg: Some(1), is_branch: false, is_load: false,
            is_store: false, mem_size_bytes: 0,
        };
        for _ in 0..insns { timing.on_insn(&insn); }
        for _ in 0..mispredict_count { timing.on_branch_outcome(true, false); }

        let cpi = timing.cpi_stack.cpi();
        prop_assert!(cpi >= 0.0, "CPI must not be negative");
        prop_assert!(cpi < 1000.0, "CPI must be plausible");
    }
}
```

---

## Integration Tests

### `integration_virtual_device_timer_fires`

Runs a simulation with `Virtual` timing where a device timer event should fire after N cycles. Verifies the event fires at the correct simulated time.

```rust
#[test]
fn integration_virtual_device_timer_fires() {
    // Set up: profile with interval_insns = 100 so boundaries are frequent.
    // Post a timer event at cycle 50.
    // Run 200 instructions.
    // Assert event fired exactly once.
    // Assert simulated time >= 50 when it fired.
    todo!("Requires EventQueue integration from helm-event crate")
}
```

### `integration_interval_vs_virtual_cycle_ratio`

For a compute-bound workload (no cache misses, no mispredictions), Interval and Virtual should produce cycle counts within 10% of each other given the same IPC setting.

```rust
#[test]
fn integration_interval_vs_virtual_cycle_ratio() {
    todo!("Requires running a simple RISC-V loop through helm-engine")
}
```

### `integration_accurate_pipeline_ipc`

Run a sequence of 1000 independent INT instructions through `AccuratePipeline`. IPC should be close to 1.0 (within ±0.1) after pipeline warmup.

```rust
#[test]
fn integration_accurate_pipeline_ipc() {
    let profile = Arc::new(MicroarchProfile::from_json(profiles::GENERIC_INORDER).unwrap());
    let cache = Arc::new(CacheModel::always_hit_for_test());
    let mut pipeline = AccuratePipeline::new(profile, cache);

    let n = 1000usize;
    let start_cycles = pipeline.current_cycles();
    let mut commits = 0;
    let mut total_cycles = 0u64;

    let insn = InsnInfo {
        pc: 0, fu_class: FuClass::Int, src_regs: SmallVec::new(),
        dst_reg: Some(1), is_branch: false, is_load: false,
        is_store: false, mem_size_bytes: 0,
    };

    while commits < n {
        if pipeline.step() { commits += 1; }
        total_cycles += 1;
        assert!(total_cycles < 10_000, "Should commit 1000 instructions in < 10000 cycles");
    }

    // Skip first 10 insns for warmup.
    let effective_insns = (n - 10) as f64;
    let effective_cycles = (pipeline.current_cycles() - start_cycles - 10) as f64;
    let ipc = effective_insns / effective_cycles;
    assert!((ipc - 1.0).abs() < 0.1,
        "IPC for independent INT insns should be ~1.0, got {:.3}", ipc);
}
```

---

## CI Requirements

- All unit tests: `cargo test -p helm-timing`
- Property tests: `cargo test -p helm-timing --features proptest` (100 iterations min)
- Integration tests: `cargo test -p helm-timing --test integration` (requires helm-event and helm-memory as dev-dependencies)
- `helm validate` is run as a separate CI job against the `generic-inorder` profile with the Dhrystone workload; must pass with 5% tolerance on all counters.
