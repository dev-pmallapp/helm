# helm-spy — Test Plan

> **Crate:** `helm-spy`
> **Total tests:** 74
> **All tests are inline** in source files (no separate `tests/` directory).
> **Run command:** `cargo test --package helm-spy`

---

## Test Inventory by Module

### 1. Counter (`primitives/counter.rs`) — 5 tests

| Test name | What it verifies |
|---|---|
| `counter_basic_increment_and_read` | Fresh counter is 0; `inc()` increments by 1; reads are consistent |
| `counter_add_by_n` | `add(500)` and `add(1_000_000)` accumulate correctly |
| `counter_reset` | `add(42)` then `reset()` returns value to 0 |
| `counter_name` | `name()` returns the string given to `new()` |
| `per_vcpu_counter_basic` | 4-slot counter: `inc(0)` × 2, `inc(2)` × 1, `add(3, 100)`; `value()` per slot; `total()` = 103; `per_vcpu()` snapshot |

### 2. IndexedCounter (`primitives/indexed.rs`) — 5 tests

| Test name | What it verifies |
|---|---|
| `indexed_counter_basic` | 3-label counter: `inc(0)` × 2, `inc(1)` × 1, `add(2, 7)`; per-bucket values; total = 10 |
| `indexed_counter_fraction` | 75 + 25 split → fractions 0.75 and 0.25 (within 1e-10) |
| `indexed_counter_fraction_zero_total` | All fractions return 0.0 when total == 0 |
| `indexed_counter_table` | `table()` returns correct (label, count, fraction) triples |
| `indexed_counter_reset` | `reset()` zeros all buckets |

### 3. Histogram (`primitives/histogram.rs`) — 6 tests

| Test name | What it verifies |
|---|---|
| `histogram_basic_record` | Edges [10, 100, 1000]: records in buckets 0, 1, 2, 3 each get count 1 |
| `histogram_edge_values` | Boundary semantics: val==10 → bucket 1 (not 0); val==100 → bucket 2 |
| `histogram_percentile` | 50/30/20 distribution: p50 = 10, p51 = 10, p80 = 10, p81 = 20 |
| `histogram_empty_percentile` | Returns 0 when total == 0 |
| `interval_histogram_window_boundary` | `tick()` 100 times in window 0, then once in window 1 causes at least 1 sample recorded |
| `histogram_reset` | `reset()` zeros all buckets |

### 4. HeatMap (`primitives/heatmap.rs`) — 4 tests

| Test name | What it verifies |
|---|---|
| `heatmap_basic_inc_and_get` | `inc(0x1000)` × 2, `inc(0x2000)` × 1; `get()` per address; `len()` = 2; missing key → 0 |
| `heatmap_top_ordering` | 4 addresses with counts 100/50/200/10 → `top(3)` returns [(0xC000, 200), (0xA000, 100), (0xB000, 50)] |
| `heatmap_top_fewer_than_n` | `top(10)` with 1 entry returns slice of length 1 |
| `heatmap_clear` | `clear()` empties the map |

### 5. RingBuffer (`primitives/ringbuf.rs`) — 4 tests

| Test name | What it verifies |
|---|---|
| `ringbuffer_push_and_snapshot` | 3 pushes into capacity-4; `len()` = 3; `snapshot()` = [10, 20, 30] |
| `ringbuffer_overflow_evicts_oldest` | 5 pushes into capacity-3; oldest 2 evicted; `snapshot()` = [3, 4, 5] |
| `ringbuffer_clear` | `clear()` empties the buffer |
| `ringbuffer_capacity` | `capacity()` returns the value given to `new()` |

### 6. EventStream (`primitives/ringbuf.rs`) — 3 tests

| Test name | What it verifies |
|---|---|
| `event_stream_push_and_drain` | 3 pushes into max-10; `drain()` returns [1, 2, 3]; stream is empty after drain |
| `event_stream_stops_at_max` | 4th push into max-3 returns false; stream stays at 3 entries |
| `event_stream_drain_allows_new_pushes` | After drain, new pushes succeed up to max |

### 7. TraceRing (`primitives/trace_ring.rs`) — 7 tests

| Test name | What it verifies |
|---|---|
| `trace_ring_push_and_drain` | 3 pushes into capacity-8; `len()` = 3; `drain_into()` produces [10, 20, 30]; ring empty after |
| `trace_ring_full_drops` | 5th push into capacity-4 returns false; drain yields [1, 2, 3, 4] |
| `trace_ring_push_after_drain` | Push 1, 2; drain; push 3, 4, 5, 6; 7th returns false; drain yields [3, 4, 5, 6] |
| `branch_record_size_is_32` | `size_of::<BranchRecord>() == 32` |
| `branch_record_taken_flag` | `flags & 1` correctly reflects taken bit; other bits do not affect taken |
| `trace_ring_branch_records` | Push a `BranchRecord`, drain, verify pc/target/taken fields |
| `trace_ring_non_power_of_two_panics` | `TraceRing::new(7)` panics with "capacity must be power of 2" |

### 8. CorrelHist2D (`primitives/correl.rs`) — 3 tests

| Test name | What it verifies |
|---|---|
| `correl_hist_basic` | x_edges [10, 20], y_edges [100, 200]; 3×3 matrix; 3 records land in (0,0), (1,1), (2,2); off-diagonal = 0; total = 3 |
| `correl_hist_matrix` | 2×2 matrix; records at all 4 cells; `matrix()` returns [[1, 1], [1, 2]] |
| `correl_hist_reset` | `reset()` zeros all cells |

### 9. Trigger (`trigger.rs`) — 7 tests

| Test name | What it verifies |
|---|---|
| `trigger_at_insn_fires_at_n` | Does not fire at 50 or 99; fires exactly at 100; does not fire at 101 |
| `trigger_every_n` | Fires at 0, 10, 20, 30 (4 times over 0..=30) |
| `trigger_at_pc` | Fires on matching PC; does not fire on other PCs; fires again on repeat (not one-shot) |
| `trigger_pc_range` | Fires for [0x1000, 0x2000); exclusive end boundary enforced |
| `trigger_one_shot_disarms_after_fire` | `is_armed()` true before fire, false after; second `check()` returns false |
| `trigger_disarmed_does_not_fire` | `disarm()` prevents firing; `arm()` re-enables |
| `trigger_every_n_zero_never_fires` | `EveryN(0)` never fires (guard: `n > 0 && insn_count % n == 0`) |

### 10. Window (`window.rs`) — 4 tests

| Test name | What it verifies |
|---|---|
| `window_basic_range` | `[100, 200)`: 0, 99 → false; 100, 150, 199 → true; 200, 1000 → false |
| `window_cached_state` | `is_active_cached()` defaults false; updates after `is_active()` calls |
| `windowed_gates_access` | `get_if_active()` returns None outside window, Some(&inner) inside |
| `windowed_boundary_exact` | start=0 inclusive; end=10 exclusive |

### 11. InsnMix (`analysis/insn_mix.rs`) — 5 tests

| Test name | What it verifies |
|---|---|
| `insn_mix_record_and_table` | 50 IntAlu, 20 Load, 15 Store, 10 Branch, 5 FpAlu → total 100; `value(IntAlu)` = 50; `fraction(IntAlu)` ≈ 0.5 |
| `insn_mix_fractions_sum_to_one` | 5 distinct classes recorded; fraction sum == 1.0 (within 1e-10) |
| `insn_mix_empty` | `total()` = 0; all fractions = 0.0 |
| `insn_mix_all_classes` | All 11 `InsnClass` variants recorded once; total = 11; each count = 1; `table().len()` = `InsnClass::COUNT` |
| `insn_mix_reset` | `reset()` zeros all buckets |

### 12. CacheModel (`analysis/cache.rs`) — 7 tests

| Test name | What it verifies |
|---|---|
| `cache_hit_on_second_access` | 1KB/2-way/64B: first access misses, second hits same line; hit_rate = 0.5 |
| `cache_miss_on_new_address` | 3 different cache lines → 3 misses, 0 hits |
| `cache_same_line_hits` | 3 accesses within same 64-byte line → 1 miss, 2 hits |
| `cache_lru_eviction` | 1-set/2-way: access A, B, A (hit), C (evicts B), B (miss); total 4 misses 1 hit |
| `cache_mpki` | 3 misses over 1000 instructions → mpki = 3.0 |
| `cache_hit_rate_empty` | Returns 0.0 with no accesses |
| `cache_reset` | After hits/misses, `reset()` zeros stats; same address misses again |

### 13. BranchPredictor (`analysis/branch_pred.rs`) — 7 tests

| Test name | What it verifies |
|---|---|
| `bimodal_always_taken_stream` | 100 always-taken at same PC: only 1 misprediction (initial weakly-not-taken); miss_rate < 2% |
| `bimodal_always_not_taken_stream` | 100 always-not-taken: 0 mispredictions (starts weakly-not-taken = correct) |
| `gshare_alternating_stream` | 1000 alternating taken/not-taken; `predictions()` == 1000 |
| `predictor_miss_rate_empty` | Returns 0.0 with no predictions |
| `predictor_mpki` | 1 misprediction over 1000 instructions → mpki = 1.0 |
| `predictor_reset` | After 2 predictions, `reset()` zeros all; table returns to weakly-not-taken |
| `gshare_different_history_different_index` | Same PC with different history produces different table updates; 3 predictions recorded |

### 14. HelmSpy (`session.rs`) — 7 tests

| Test name | What it verifies |
|---|---|
| `session_new_defaults` | `new()`: insn_count = 0; insn_mix total = 0; hot_pcs empty; cache_l1d = None; branch_pred = None |
| `session_with_cache` | `with_cache_l1d(32*1024, 8, 64)` sets cache_l1d to Some |
| `session_with_branch_pred` | `with_branch_predictor(BiModal { bits: 10 })` sets branch_pred to Some |
| `session_snapshot` | After recording to insn_count/insn_mix/hot_pcs: snapshot has correct insn_count; insn_mix_table has `InsnClass::COUNT` rows; hot_pcs_top20 is non-empty; cache/branch fields are None |
| `session_check_triggers` | One-shot AtInsn(100) trigger: fires at 100, not at 50 or again after |
| `session_fault_history` | `fault_history.push()` and `snapshot()` round-trip; entries contain expected strings |
| `session_integrated_workflow` | 100 instructions: count, mix, hot_pcs, L1D cache access; snapshot shows count = 100; cache_hit_rate > 0 |

---

## Test Count Summary

| Module | File | Tests |
|---|---|---|
| Counter + PerVcpuCounter | `primitives/counter.rs` | 5 |
| IndexedCounter | `primitives/indexed.rs` | 5 |
| Histogram + IntervalHistogram | `primitives/histogram.rs` | 6 |
| HeatMap | `primitives/heatmap.rs` | 4 |
| RingBuffer | `primitives/ringbuf.rs` | 4 |
| EventStream | `primitives/ringbuf.rs` | 3 |
| TraceRing + BranchRecord | `primitives/trace_ring.rs` | 7 |
| CorrelHist2D | `primitives/correl.rs` | 3 |
| Trigger | `trigger.rs` | 7 |
| Window + Windowed | `window.rs` | 4 |
| InsnMix | `analysis/insn_mix.rs` | 5 |
| CacheModel | `analysis/cache.rs` | 7 |
| BranchPredictor | `analysis/branch_pred.rs` | 7 |
| HelmSpy + HelmSpySnapshot | `session.rs` | 7 |
| **Total** | | **74** |
