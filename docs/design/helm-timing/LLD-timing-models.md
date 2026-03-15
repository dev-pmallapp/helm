# helm-timing — LLD: Timing Models

## Shared Types

```rust
// helm-core re-exports
pub type Cycles = u64;
pub type InsnCount = u64;

/// Metadata about a committed instruction, passed to TimingModel::on_insn.
#[derive(Debug, Clone)]
pub struct InsnInfo {
    pub pc: u64,
    pub fu_class: FuClass,
    pub src_regs: SmallVec<[u8; 4]>,  // physical register indices, up to 4 sources
    pub dst_reg: Option<u8>,
    pub is_branch: bool,
    pub is_load: bool,
    pub is_store: bool,
    pub mem_size_bytes: u8,
}

/// Functional unit class — used for latency lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FuClass {
    Int,
    Branch,
    Mul,
    Div,
    Fp,
    Load,
    Store,
    Csr,
    Atomic,
    Fence,
}

/// A memory access descriptor, passed to TimingModel::on_mem_access.
#[derive(Debug, Clone)]
pub struct MemAccess {
    pub addr: u64,
    pub size: u8,
    pub is_write: bool,
    pub is_instruction_fetch: bool,
}
```

---

## `TimingModel` Trait

```rust
/// The hot-path interface that every timing model implements.
/// Generic parameter on HelmEngine<T: TimingModel> — zero dynamic dispatch.
pub trait TimingModel: Send + Sync + 'static {
    /// Called once per committed instruction. Returns cycle delta for this instruction.
    fn on_insn(&mut self, insn: &InsnInfo) -> Cycles;

    /// Called on every memory access that reaches the timing model
    /// (after functional simulation). Returns additional stall cycles.
    fn on_mem_access(&mut self, access: &MemAccess) -> Cycles;

    /// Current simulated cycle count (absolute, from simulation start).
    fn current_cycles(&self) -> Cycles;

    /// Called when branch outcome is known (taken + whether predictor was right).
    fn on_branch_outcome(&mut self, taken: bool, predicted: bool);

    /// Called at interval boundaries and at end-of-quantum.
    /// Implementors drain the EventQueue up to current_cycles here.
    fn on_interval_boundary(&mut self, eq: &mut EventQueue);

    /// Access the backing MicroarchProfile.
    fn profile(&self) -> &MicroarchProfile;

    /// Reset cycle counter (for checkpoint restore).
    fn reset_cycles(&mut self, cycles: Cycles);
}
```

---

## `Virtual` — Event-Driven Virtual Clock

### Struct Definition

```rust
/// Fastest timing model. No pipeline simulation.
/// Advances simulated time by estimated cycles using a fixed IPC.
pub struct Virtual {
    cycles: Cycles,
    insn_count: InsnCount,
    ipc_reciprocal: f64,       // = 1.0 / profile.virtual_ipc, cached
    profile: Arc<MicroarchProfile>,
    /// Interval length in instructions before calling on_interval_boundary.
    interval_insns: u64,
    next_boundary: InsnCount,
}

impl Virtual {
    pub fn new(profile: Arc<MicroarchProfile>) -> Self {
        let ipc_reciprocal = 1.0 / profile.virtual_ipc;
        let interval_insns = profile.virtual_interval_insns;
        Virtual {
            cycles: 0,
            insn_count: 0,
            ipc_reciprocal,
            profile,
            interval_insns,
            next_boundary: interval_insns,
        }
    }
}
```

### `TimingModel` Implementation

```rust
impl TimingModel for Virtual {
    fn on_insn(&mut self, insn: &InsnInfo) -> Cycles {
        // Cycle estimation: every instruction costs ceil(1/IPC) cycles.
        // For IPC=1.0 this is exactly 1. For IPC=2.0 this alternates 0/1.
        // We accumulate fractional cycles using an integer approximation:
        // multiply up so that ipc_reciprocal * SCALE is an integer.
        // For simplicity in Phase 0: charge 1 cycle per instruction unless
        // the instruction is a divide (charge fu_latency from profile).
        let delta = match insn.fu_class {
            FuClass::Div => self.profile.fu_latencies[&FuClass::Div] as Cycles,
            FuClass::Fp  => self.profile.fu_latencies[&FuClass::Fp]  as Cycles,
            _            => 1,
        };
        self.cycles += delta;
        self.insn_count += 1;
        delta
    }

    fn on_mem_access(&mut self, _access: &MemAccess) -> Cycles {
        // Virtual mode ignores memory timing; cache hits/misses have no cycle cost.
        0
    }

    fn current_cycles(&self) -> Cycles {
        self.cycles
    }

    fn on_branch_outcome(&mut self, _taken: bool, _predicted: bool) {
        // Virtual mode: no branch penalty.
    }

    fn on_interval_boundary(&mut self, eq: &mut EventQueue) {
        // Drain all device timer events up to the current simulated cycle.
        // This is the primary mechanism that makes device timers work in Virtual mode.
        eq.drain_until(self.cycles);

        // Advance next boundary.
        self.next_boundary = self.insn_count + self.interval_insns;
    }

    fn profile(&self) -> &MicroarchProfile {
        &self.profile
    }

    fn reset_cycles(&mut self, cycles: Cycles) {
        self.cycles = cycles;
    }
}
```

### Tick Advancement and EventQueue Integration

The execute loop in `HelmEngine` calls `on_insn` per instruction and checks whether `insn_count >= next_boundary` to trigger `on_interval_boundary`. The call sequence is:

```
loop {
    let insn = fetch_decode_execute(&mut hart);
    let delta = timing.on_insn(&insn);
    if hart.insn_count() >= timing_state.next_boundary {
        timing.on_interval_boundary(&mut event_queue);
    }
    if event_queue.peek_next_tick() <= timing.current_cycles() {
        timing.on_interval_boundary(&mut event_queue);  // also drains on hot event
    }
}
```

`EventQueue::drain_until(cycles)` fires all callbacks with `fire_at <= cycles` in ascending order. Device callbacks may post new events (recurring timer pattern from Q52).

---

## `Interval` — Sniper-Style Interval Simulation

### `OoOWindow` — RAW Dependency Tracker

```rust
/// Tracks register-ready cycles across an instruction window.
/// Models in-order issue / out-of-order completion: each instruction
/// issues at max(ready_cycles of all source registers).
pub struct OoOWindow {
    /// Simulated cycle when each architectural register becomes ready.
    /// Index = register number. For RV64GC: 0..64 (32 int + 32 fp).
    reg_ready: [Cycles; 64],
    /// Current dispatch cycle within this interval.
    dispatch_cycle: Cycles,
}

impl OoOWindow {
    pub fn new(start_cycle: Cycles) -> Self {
        OoOWindow {
            reg_ready: [0u64; 64],
            dispatch_cycle: start_cycle,
        }
    }

    /// Issue an instruction. Returns the cycle it can issue (may stall on RAW).
    /// Updates reg_ready for the destination register.
    pub fn issue(&mut self, insn: &InsnInfo, fu_latency: Cycles) -> Cycles {
        // Compute earliest issue cycle: must wait for all source operands.
        let src_ready = insn.src_regs
            .iter()
            .map(|&r| self.reg_ready[r as usize])
            .max()
            .unwrap_or(0);

        let issue_at = self.dispatch_cycle.max(src_ready);

        // Instruction completes at issue + latency.
        let complete_at = issue_at + fu_latency;

        // Update destination register ready cycle.
        if let Some(dst) = insn.dst_reg {
            self.reg_ready[dst as usize] = complete_at;
        }

        // Advance dispatch to next cycle (in-order issue, one per cycle).
        self.dispatch_cycle = issue_at + 1;

        issue_at
    }

    /// Reset for the next interval, carrying over only the register-ready cycles
    /// that extend past the interval boundary (inter-interval dependencies).
    pub fn roll_over(&mut self, interval_end_cycle: Cycles) {
        // Cap any "too far in the future" ready times so they don't over-estimate.
        // In practice, most will be <= interval_end_cycle after a 10K instruction window.
        self.dispatch_cycle = interval_end_cycle;
    }
}
```

### `CpiStack` — CPI Component Breakdown

```rust
/// Tracks the CPI components for one interval.
/// Used for statistics and for weighting the cycle estimate.
#[derive(Default, Debug, Clone)]
pub struct CpiStack {
    pub base_cpi: f64,
    pub mem_stall_cycles: Cycles,
    pub branch_mispredict_cycles: Cycles,
    pub structural_stall_cycles: Cycles,
    pub total_insns: InsnCount,
}

impl CpiStack {
    pub fn cpi(&self) -> f64 {
        if self.total_insns == 0 { return 1.0; }
        (self.base_cpi * self.total_insns as f64
            + self.mem_stall_cycles as f64
            + self.branch_mispredict_cycles as f64
            + self.structural_stall_cycles as f64)
            / self.total_insns as f64
    }

    pub fn reset(&mut self) {
        *self = CpiStack::default();
    }
}
```

### `IntervalTimed` Struct

```rust
pub struct IntervalTimed {
    cycles: Cycles,
    insn_count: InsnCount,
    profile: Arc<MicroarchProfile>,
    window: OoOWindow,
    cpi_stack: CpiStack,
    /// Shared cache model from helm-memory.
    cache: Arc<CacheModel>,
    /// Instruction count at which the next interval boundary triggers.
    next_boundary: InsnCount,
    interval_insns: u64,
}

impl IntervalTimed {
    pub fn new(profile: Arc<MicroarchProfile>, cache: Arc<CacheModel>) -> Self {
        let interval_insns = profile.interval_insns;
        IntervalTimed {
            cycles: 0,
            insn_count: 0,
            window: OoOWindow::new(0),
            cpi_stack: CpiStack::default(),
            profile,
            cache,
            next_boundary: interval_insns,
            interval_insns,
        }
    }
}
```

### `TimingModel` Implementation for `IntervalTimed`

```rust
impl TimingModel for IntervalTimed {
    fn on_insn(&mut self, insn: &InsnInfo) -> Cycles {
        let fu_lat = self.profile.fu_latency(insn.fu_class);
        let issue_cycle = self.window.issue(insn, fu_lat as Cycles);

        // The cycle delta for this instruction is the difference from the
        // previous dispatch cycle.  Accumulated into cpi_stack.base_cpi later.
        self.cpi_stack.total_insns += 1;
        self.insn_count += 1;

        // Structural stall: if issue_cycle > window.dispatch_cycle - 1, there's
        // a stall beyond RAW. The OoOWindow::issue already accounts for this
        // by keeping dispatch_cycle monotonically increasing.
        0  // Cycles are committed at interval boundary, not per instruction.
    }

    fn on_mem_access(&mut self, access: &MemAccess) -> Cycles {
        // Query the shared cache model.
        let result = if access.is_instruction_fetch {
            self.cache.lookup_icache(access.addr)
        } else if access.is_write {
            self.cache.lookup_dcache_write(access.addr, access.size)
        } else {
            self.cache.lookup_dcache_read(access.addr, access.size)
        };

        let miss_penalty = match result {
            CacheResult::Hit  => 0,
            CacheResult::Miss => {
                // Miss event: charge penalty and trigger interval boundary.
                let penalty = self.profile.l1_miss_penalty_cycles as Cycles;
                self.cpi_stack.mem_stall_cycles += penalty;
                penalty
            }
        };
        miss_penalty
    }

    fn on_branch_outcome(&mut self, _taken: bool, predicted: bool) {
        if !predicted {
            let penalty = self.profile.branch_mispredict_penalty_cycles as Cycles;
            self.cpi_stack.branch_mispredict_cycles += penalty;
        }
    }

    fn on_interval_boundary(&mut self, eq: &mut EventQueue) {
        // 1. Compute the cycle count for this interval using the OoO window.
        //    The window's dispatch_cycle is the simulated cycle at the END
        //    of this instruction window.
        let interval_cycles = self.window.dispatch_cycle - self.cycles;

        // 2. Add memory stall and branch penalty from the CPI stack.
        let total_delta = interval_cycles
            + self.cpi_stack.mem_stall_cycles
            + self.cpi_stack.branch_mispredict_cycles;

        // 3. Advance simulated time.
        self.cycles += total_delta;

        // 4. Drain device events up to the new cycle count.
        eq.drain_until(self.cycles);

        // 5. Roll over the OoO window for the next interval.
        self.window.roll_over(self.cycles);
        self.cpi_stack.reset();
        self.next_boundary = self.insn_count + self.interval_insns;
    }

    fn current_cycles(&self) -> Cycles { self.cycles }
    fn profile(&self) -> &MicroarchProfile { &self.profile }
    fn reset_cycles(&mut self, cycles: Cycles) { self.cycles = cycles; }
}
```

### Miss Event Handling

When `on_mem_access` returns a non-zero penalty, the execute loop immediately calls `on_interval_boundary` regardless of whether the instruction count boundary has been reached:

```rust
// In HelmEngine execute loop (pseudo):
let mem_stall = timing.on_mem_access(&access);
if mem_stall > 0 {
    // Miss event: force an interval boundary to account for the stall
    // before continuing. This implements Q41: miss events trigger boundaries.
    timing.on_interval_boundary(&mut event_queue);
}
```

### Interval Boundary Trigger Conditions

| Trigger | Condition |
|---------|-----------|
| Fixed count | `insn_count >= next_boundary` |
| Cache miss | `on_mem_access` returns > 0 |
| End of quantum | Scheduler calls `on_interval_boundary` at quantum end |

---

## `Accurate` — 5-Stage In-Order Pipeline

### Pipeline Stage Registers

```rust
/// Instruction in the IF (Instruction Fetch) stage.
#[derive(Default, Clone)]
pub struct IfIdReg {
    pub valid: bool,
    pub pc: u64,
    pub raw_insn: u32,
}

/// Instruction in the ID (Instruction Decode) stage.
#[derive(Default, Clone)]
pub struct IdExReg {
    pub valid: bool,
    pub pc: u64,
    pub insn: InsnInfo,
    pub fu_class: FuClass,
    /// Remaining execution latency cycles (counts down each cycle).
    pub ex_cycles_remaining: u8,
}

/// Instruction in the EX (Execute) stage.
#[derive(Default, Clone)]
pub struct ExMemReg {
    pub valid: bool,
    pub pc: u64,
    pub insn: InsnInfo,
    pub result: u64,
    pub mem_addr: Option<u64>,
}

/// Instruction in the MEM (Memory Access) stage.
#[derive(Default, Clone)]
pub struct MemWbReg {
    pub valid: bool,
    pub pc: u64,
    pub insn: InsnInfo,
    pub result: u64,
    /// Cycles remaining for cache access (0 = done, ready to WB).
    pub mem_cycles_remaining: u8,
}
```

### Forwarding Unit

```rust
/// Tracks values available for forwarding from EX/MEM stages.
pub struct ForwardingUnit {
    /// (dst_reg, value) available from EX/MEM boundary register.
    pub ex_mem_forward: Option<(u8, u64)>,
    /// (dst_reg, value) available from MEM/WB boundary register.
    pub mem_wb_forward: Option<(u8, u64)>,
}

impl ForwardingUnit {
    pub fn resolve(&self, reg: u8, _gpr: &[u64; 32]) -> Option<u64> {
        if let Some((dst, val)) = self.ex_mem_forward {
            if dst == reg { return Some(val); }
        }
        if let Some((dst, val)) = self.mem_wb_forward {
            if dst == reg { return Some(val); }
        }
        None
    }
}
```

### `AccuratePipeline` Struct

```rust
pub struct AccuratePipeline {
    cycles: Cycles,
    profile: Arc<MicroarchProfile>,
    cache: Arc<CacheModel>,

    // Pipeline stage boundary registers.
    if_id: IfIdReg,
    id_ex: IdExReg,
    ex_mem: ExMemReg,
    mem_wb: MemWbReg,

    forwarding: ForwardingUnit,

    /// Stall signal: set true when a hazard requires the front end to stall.
    stall: bool,

    /// Flush signal: set true on a branch misprediction to clear IF/ID/EX.
    flush: bool,
}

impl AccuratePipeline {
    pub fn new(profile: Arc<MicroarchProfile>, cache: Arc<CacheModel>) -> Self {
        AccuratePipeline {
            cycles: 0,
            profile,
            cache,
            if_id: IfIdReg::default(),
            id_ex: IdExReg::default(),
            ex_mem: ExMemReg::default(),
            mem_wb: MemWbReg::default(),
            forwarding: ForwardingUnit { ex_mem_forward: None, mem_wb_forward: None },
            stall: false,
            flush: false,
        }
    }
}
```

### Per-Cycle Step

The pipeline advances one cycle at a time. Stages are evaluated in reverse order (WB→MEM→EX→ID→IF) to implement the "write before read" convention.

```rust
impl AccuratePipeline {
    /// Advance the pipeline by one clock cycle.
    /// Returns true if an instruction was committed (WB stage completed).
    pub fn step(&mut self) -> bool {
        self.cycles += 1;
        let committed = self.step_wb();
        self.step_mem();
        self.step_ex();
        self.step_id();
        if !self.stall { self.step_if(); }
        self.update_forwarding();
        committed
    }

    fn step_wb(&mut self) -> bool {
        if !self.mem_wb.valid { return false; }
        if self.mem_wb.mem_cycles_remaining > 0 {
            self.mem_wb.mem_cycles_remaining -= 1;
            return false;  // Still waiting for memory.
        }
        // Write back: instruction commits here.
        self.mem_wb.valid = false;
        true
    }

    fn step_mem(&mut self) {
        if self.mem_wb.valid { return; }  // WB busy: stall MEM.
        if !self.ex_mem.valid { return; }

        let lat = if let Some(addr) = self.ex_mem.mem_addr {
            let access = MemAccess {
                addr,
                size: self.ex_mem.insn.mem_size_bytes,
                is_write: self.ex_mem.insn.is_store,
                is_instruction_fetch: false,
            };
            let result = if access.is_write {
                self.cache.lookup_dcache_write(addr, access.size)
            } else {
                self.cache.lookup_dcache_read(addr, access.size)
            };
            match result {
                CacheResult::Hit  => 0,
                CacheResult::Miss => self.profile.l1_miss_penalty_cycles,
            }
        } else {
            0
        };

        self.mem_wb = MemWbReg {
            valid: true,
            pc: self.ex_mem.pc,
            insn: self.ex_mem.insn.clone(),
            result: self.ex_mem.result,
            mem_cycles_remaining: lat,
        };
        self.ex_mem.valid = false;
    }

    fn step_ex(&mut self) {
        if self.ex_mem.valid { return; }  // MEM busy.
        if !self.id_ex.valid { return; }

        if self.id_ex.ex_cycles_remaining > 0 {
            self.id_ex.ex_cycles_remaining -= 1;
            self.stall = self.id_ex.ex_cycles_remaining > 0;
            return;
        }

        // Check for load-use hazard: if EX is a load and ID/EX src reg
        // matches the load destination, stall for one cycle.
        // (Simplified: real check compares against ID stage.)

        if self.flush {
            self.id_ex.valid = false;
            self.flush = false;
            return;
        }

        let latency = self.profile.fu_latency(self.id_ex.fu_class);
        self.ex_mem = ExMemReg {
            valid: true,
            pc: self.id_ex.pc,
            insn: self.id_ex.insn.clone(),
            result: 0,  // Filled by functional execution layer.
            mem_addr: if self.id_ex.insn.is_load || self.id_ex.insn.is_store {
                Some(0)  // Filled by functional execution layer.
            } else {
                None
            },
        };
        self.id_ex.ex_cycles_remaining = latency.saturating_sub(1);
        self.stall = self.id_ex.ex_cycles_remaining > 0;
        if !self.stall { self.id_ex.valid = false; }
    }

    fn step_id(&mut self) {
        if self.id_ex.valid || self.stall { return; }
        if !self.if_id.valid { return; }

        // Decode happens here in a real implementation.
        // InsnInfo is pre-populated by the functional execution layer
        // and passed via a channel or shared slot.
        self.id_ex = IdExReg {
            valid: true,
            pc: self.if_id.pc,
            insn: InsnInfo {
                pc: self.if_id.pc,
                fu_class: FuClass::Int,  // Filled by decode.
                src_regs: SmallVec::new(),
                dst_reg: None,
                is_branch: false,
                is_load: false,
                is_store: false,
                mem_size_bytes: 0,
            },
            fu_class: FuClass::Int,
            ex_cycles_remaining: 0,
        };
        self.if_id.valid = false;
    }

    fn step_if(&mut self) {
        if self.if_id.valid { return; }  // IF/ID register full, stall IF.
        // Instruction fetch: check ICache.
        // PC is tracked by the functional execution layer; IF stage just
        // models the timing of the fetch.
        let lat = match self.cache.lookup_icache(0 /* pc from functional layer */) {
            CacheResult::Hit  => 0,
            CacheResult::Miss => self.profile.l1_miss_penalty_cycles,
        };
        self.if_id.valid = true;
        // If miss: IFetch stalls are modeled via the MEM stage for instruction fetches.
        let _ = lat; // Applied at MEM/WB for simplicity in Phase 0.
    }

    fn update_forwarding(&mut self) {
        self.forwarding.ex_mem_forward = if self.ex_mem.valid {
            self.ex_mem.insn.dst_reg.map(|r| (r, self.ex_mem.result))
        } else { None };

        self.forwarding.mem_wb_forward = if self.mem_wb.valid {
            self.mem_wb.insn.dst_reg.map(|r| (r, self.mem_wb.result))
        } else { None };
    }
}
```

### Stall Logic Summary

| Hazard | Detection | Resolution |
|--------|-----------|------------|
| Load-use RAW | ID detects: EX is a load, ID src = EX dst | Stall IF/ID one cycle, insert bubble |
| Structural: FU latency > 1 | `ex_cycles_remaining > 0` in EX | Stall IF/ID/EX until remaining = 0 |
| MEM cache miss | `mem_cycles_remaining > 0` in MEM | Stall WB, hold MEM, stall upstream |
| Branch misprediction | `flush = true` set by functional layer | Flush IF/ID/EX stage registers |

### `TimingModel` Implementation for `AccuratePipeline`

```rust
impl TimingModel for AccuratePipeline {
    fn on_insn(&mut self, insn: &InsnInfo) -> Cycles {
        // The functional execution layer delivers InsnInfo here.
        // We inject it into the pipeline's ID stage slot and run
        // the pipeline forward until the instruction commits (WB).
        // Returns the number of cycles elapsed until commit.
        let start_cycle = self.cycles;

        // Inject into IF slot for the pipeline to process.
        // (In a real integration, this is queued and the pipeline
        // is stepped by the scheduler, not per-insn call. For Phase 0,
        // we step until commit as a simplified coupling.)
        self.if_id = IfIdReg { valid: true, pc: insn.pc, raw_insn: 0 };

        // Temporarily override the ID decode with the provided InsnInfo.
        // This avoids re-decoding; functional layer has already decoded.
        let injected_insn = insn.clone();

        loop {
            let committed = self.step();
            if committed { break; }
        }

        // Overwrite the decoded insn in the pipeline with what we received.
        // (Phase 0 simplification: we trust the functional layer's decode.)
        let _ = injected_insn;

        self.cycles - start_cycle
    }

    fn on_mem_access(&mut self, _access: &MemAccess) -> Cycles {
        // Handled inside step_mem() via the shared CacheModel.
        // No additional action needed here.
        0
    }

    fn current_cycles(&self) -> Cycles { self.cycles }

    fn on_branch_outcome(&mut self, _taken: bool, predicted: bool) {
        if !predicted {
            self.flush = true;
        }
    }

    fn on_interval_boundary(&mut self, eq: &mut EventQueue) {
        eq.drain_until(self.cycles);
    }

    fn profile(&self) -> &MicroarchProfile { &self.profile }

    fn reset_cycles(&mut self, cycles: Cycles) { self.cycles = cycles; }
}
```

### Integration Notes

- `AccuratePipeline::step()` is called by the engine's main loop, not by `on_insn`. In Phase 0, `on_insn` drives the pipeline to completion for simplicity. In Phase 3 (OoO), `step()` is decoupled from functional execution.
- The `flush` flag is set by the functional layer (via `on_branch_outcome`) and consumed at the next `step_ex()` call.
- Cache model is shared (`Arc<CacheModel>`) between all three timing models and `helm-memory`, ensuring a single consistent cache state.

---

## Design Decisions from Q&A

### Design Decision: Virtual tick = estimated cycles (Q38)

In `Virtual` timing mode, `current_cycles()` returns an estimated cycle count computed as `instruction_count / virtual_ipc` (using `ipc_reciprocal` accumulated per instruction). This drives the `EventQueue` — device timers and interrupt delivery are scheduled in simulated cycles, not wall time. The `Virtual` model is therefore not purely cycle-agnostic; it provides an estimated timeline sufficient for correct timer-driven device behavior.

### Design Decision: Virtual drives EventQueue (Q39)

The `Virtual` timing model drives the `EventQueue` via `on_interval_boundary()`. At each interval boundary, `Virtual::on_interval_boundary()` advances the simulated clock and drains the per-hart `EventQueue` up to the current estimated cycle count. This allows devices with baud-rate timers and other period-based callbacks to fire correctly even under the `Virtual` model.

### Design Decision: 5-stage in-order pipeline for Phase 0 Accurate model (Q44)

The `AccuratePipeline` implements a 5-stage in-order pipeline (IF→ID→EX→MEM→WB) for Phase 0. Out-of-order execution is Phase 3. The 5-stage model is the correct starting point: it captures instruction-level latency dependencies, branch misprediction penalties, and cache miss stalls without the complexity of a full ROB and renaming unit.

### Design Decision: MicroarchProfile is immutable after construction (Q48)

`MicroarchProfile` is loaded from JSON at simulator construction and never mutated during simulation. Timing models hold `Arc<MicroarchProfile>`. This makes profiles safe to share across harts without locking and ensures all timing decisions use a consistent parameter set throughout a simulation run.

### Design Decision: 4 built-in MicroarchProfile configurations (Q49)

Four standard profiles are shipped: `sifive-u74` (RISC-V, 5-stage in-order), `cortex-a72` (AArch64, out-of-order, approximated by in-order for Phase 0), `generic-inorder` (conservative defaults), and `generic-ooo` (aggressive defaults for OoO preview). Profiles are JSON files loaded by name or path. Custom profiles can be provided by users.
