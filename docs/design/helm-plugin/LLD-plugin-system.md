# helm-plugin — Low-Level Design: Plugin System

## 1. Crate Structure

```
crates/helm-plugin/
├── Cargo.toml
└── src/
    ├── lib.rs                    # re-exports, feature gates
    ├── api/
    │   ├── mod.rs
    │   ├── plugin.rs             # HelmPlugin trait, PluginArgs
    │   ├── component.rs          # HelmComponent trait, ComponentInfo
    │   ├── loader.rs             # ComponentRegistry
    │   ├── metadata.rs           # PluginMetadata, PLUGIN_API_VERSION
    │   └── dynamic.rs            # DynamicPluginLoader (unix, feature-gated)
    ├── runtime/
    │   ├── mod.rs
    │   ├── registry.rs           # PluginRegistry (callback storage + dispatch)
    │   ├── callback.rs           # Callback type aliases
    │   ├── info.rs               # InsnInfo, MemInfo, BranchInfo, FaultInfo, etc.
    │   ├── scoreboard.rs         # Per-vCPU lock-free counters
    │   └── bridge.rs             # PluginComponentAdapter + register_builtins()
    ├── builtins/
    │   ├── mod.rs
    │   ├── trace/
    │   │   ├── mod.rs
    │   │   ├── insn_count.rs     # InsnCount
    │   │   ├── execlog.rs        # ExecLog
    │   │   ├── hotblocks.rs      # HotBlocks
    │   │   ├── howvec.rs         # HowVec
    │   │   ├── syscall_trace.rs  # SyscallTrace
    │   │   ├── branch_trace.rs   # BranchTrace ← NEW
    │   │   └── mem_trace.rs      # MemTrace ← NEW
    │   ├── memory/
    │   │   ├── mod.rs
    │   │   ├── cache_sim.rs      # CacheSim
    │   │   └── tlb_sim.rs        # TlbSim ← NEW
    │   ├── debug/
    │   │   ├── mod.rs
    │   │   ├── fault_detect.rs   # FaultDetect
    │   │   ├── watchpoint.rs     # Watchpoint ← NEW
    │   │   └── coverage.rs       # CoverageMap ← NEW
    │   └── profiling/            # ← NEW category
    │       ├── mod.rs
    │       ├── func_profile.rs   # FuncProfile
    │       ├── flamegraph.rs     # FlameGraph
    │       └── bb_vectors.rs     # BBVectors (SimPoint)
    └── tests.rs
```

## 2. Core Traits

### 2.1 HelmPlugin

```rust
pub trait HelmPlugin: Send + Sync {
    /// Plugin name (e.g. "execlog", "cache").
    fn name(&self) -> &str;

    /// Register callbacks. Called once at load time.
    fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs);

    /// Called at simulation end. Print stats, flush logs, write output files.
    fn atexit(&mut self) {}

    /// Called between run() phases. Allows stats reset or reconfiguration.
    fn on_phase_change(&mut self, _phase: &str) {}
}
```

### 2.2 PluginArgs

```rust
pub struct PluginArgs {
    inner: HashMap<String, String>,
}

impl PluginArgs {
    pub fn parse(s: &str) -> Self;        // "key=val,key2=val2"
    pub fn get(&self, key: &str) -> Option<&str>;
    pub fn get_or(&self, key: &str, default: &str) -> String;
    pub fn get_usize(&self, key: &str, default: usize) -> usize;
    pub fn get_bool(&self, key: &str, default: bool) -> bool;  // NEW
}
```

## 3. PluginRegistry — Callback Storage

```rust
pub struct PluginRegistry {
    // ── Callback vectors ──────────────────────────────────────────
    pub insn_exec:    Vec<InsnExecCb>,
    pub mem_access:   Vec<(MemFilter, MemAccessCb)>,
    pub branch:       Vec<BranchCb>,          // NEW
    pub syscall:      Vec<SyscallCb>,
    pub syscall_ret:  Vec<SyscallRetCb>,
    pub fault:        Vec<FaultCb>,
    pub exception:    Vec<ExceptionCb>,        // NEW
    pub tb_exec:      Vec<TbExecCb>,
    pub vcpu_init:    Vec<VcpuInitCb>,
    pub vcpu_exit:    Vec<VcpuExitCb>,
    pub timer:        Vec<(u64, TimerCb)>,     // NEW: (interval, callback)

    // ── Fast-path flags (checked in hot loop) ─────────────────────
    // These are `bool` flags set when the first callback of each type
    // is registered. The engine checks these before constructing
    // callback arguments (avoids overhead when unused).
}

impl PluginRegistry {
    pub fn has_insn_callbacks(&self) -> bool;
    pub fn has_mem_callbacks(&self) -> bool;
    pub fn has_branch_callbacks(&self) -> bool;  // NEW

    pub fn fire_insn_exec(&self, vcpu: usize, insn: &InsnInfo);
    pub fn fire_mem_access(&self, vcpu: usize, info: &MemInfo);
    pub fn fire_branch(&self, vcpu: usize, info: &BranchInfo);  // NEW
    pub fn fire_syscall(&self, info: &SyscallInfo);
    pub fn fire_syscall_ret(&self, info: &SyscallRetInfo);
    pub fn fire_fault(&self, info: &FaultInfo);
}
```

## 4. Info Structs

### 4.1 InsnInfo

```rust
pub struct InsnInfo {
    pub pc: u64,
    pub raw: u32,
    pub size: u8,           // 2 (Thumb) or 4 (A64/A32/RV)
    pub class: InsnClass,
}

#[derive(Debug, Clone, Copy)]
pub enum InsnClass {
    IntAlu, IntMul, Branch, Load, Store,
    FpAlu, SimdAlu, System, Nop, Atomic,
    Unknown,
}
```

### 4.2 BranchInfo (NEW)

```rust
pub struct BranchInfo {
    pub pc: u64,
    pub target: u64,
    pub taken: bool,
    pub kind: BranchKind,
}

pub enum BranchKind {
    DirectCond,     // B.cond, CBZ, CBNZ, TBZ, TBNZ
    DirectUncond,   // B
    Call,           // BL, BLR
    Return,         // RET
    IndirectJump,   // BR
    IndirectCall,   // BLR
}
```

### 4.3 MemInfo

```rust
pub struct MemInfo {
    pub vaddr: u64,
    pub paddr: Option<u64>,
    pub size: u8,
    pub is_store: bool,
    pub is_atomic: bool,
    pub value: Option<u64>,  // only populated when a plugin requests it
}
```

### 4.4 SyscallInfo / SyscallRetInfo

```rust
pub struct SyscallInfo {
    pub vcpu_idx: usize,
    pub number: u64,
    pub args: [u64; 6],
}

pub struct SyscallRetInfo {
    pub vcpu_idx: usize,
    pub number: u64,
    pub ret_value: u64,
}
```

### 4.5 FaultInfo

```rust
pub struct FaultInfo {
    pub vcpu_idx: usize,
    pub pc: u64,
    pub raw: u32,
    pub kind: FaultKind,
    pub message: String,
    pub insn_count: u64,
    pub context: ArchContext,
}

pub enum FaultKind {
    IllegalInstruction,
    MemoryFault,
    StackCorruption,
    NullDereference,
    WildJump,
    UnsupportedSyscall,
    Breakpoint,
}

pub enum ArchContext {
    Aarch64 {
        x: [u64; 31], sp: u64, pc: u64,
        nzcv: u32, tpidr_el0: u64, current_el: u8,
    },
    RiscV { x: [u64; 32], pc: u64 },
    None,
}
```

## 5. Scoreboard (Lock-Free Per-vCPU Counters)

```rust
pub struct Scoreboard<T> {
    slots: Vec<UnsafeCell<T>>,
}

// Safe because: single-writer (one vCPU per slot), readers only at atexit
unsafe impl<T: Send> Sync for Scoreboard<T> {}

impl<T: Default> Scoreboard<T> {
    pub fn new(n: usize) -> Self;
    pub fn get(&self, vcpu_idx: usize) -> &T;
    pub fn get_mut(&self, vcpu_idx: usize) -> &mut T;
    pub fn iter(&self) -> impl Iterator<Item = &T>;
}
```

Used by InsnCount for zero-lock per-instruction counting:
```rust
reg.on_insn_exec(Box::new(move |vcpu_idx, _insn| {
    *scoreboard.get_mut(vcpu_idx) += 1;
}));
```

## 6. Engine Integration Points

### 6.1 In `HelmEngine::step_aarch64()`

```rust
fn step_aarch64(&mut self) -> Result<(), HartException> {
    let pc = ...;
    let raw = self.memory.fetch32(pc)?;
    let insn = aarch64_decode(raw, pc)?;

    // ── PLUGIN: insn_exec ──────────────────────────────────────
    if self.plugins.has_insn_callbacks() {
        self.plugins.fire_insn_exec(0, &InsnInfo {
            pc, raw, size: 4,
            class: classify_opcode(insn.opcode),
        });
    }

    let pc_written = aarch64_execute(&insn, a64, &mut self.memory)?;

    // ── PLUGIN: branch ─────────────────────────────────────────
    if self.plugins.has_branch_callbacks() && insn.is_branch() {
        self.plugins.fire_branch(0, &BranchInfo {
            pc,
            target: a64.pc,
            taken: pc_written,
            kind: classify_branch(insn.opcode),
        });
    }

    // ── PLUGIN: mem_access ─────────────────────────────────────
    // (fired inside execute() via a MemInterface wrapper that
    //  intercepts reads/writes and calls fire_mem_access)

    ...
}
```

### 6.2 In `handle_exception()` (syscall dispatch)

```rust
HartException::EnvironmentCall { nr, .. } => {
    if self.plugins.has_syscall_callbacks() {
        self.plugins.fire_syscall(&SyscallInfo {
            vcpu_idx: 0, number: nr, args: [x0,x1,x2,x3,x4,x5],
        });
    }

    let ret = self.dispatch_aarch64_syscall(nr);

    if self.plugins.has_syscall_ret_callbacks() {
        self.plugins.fire_syscall_ret(&SyscallRetInfo {
            vcpu_idx: 0, number: nr, ret_value: a64.x[0],
        });
    }
}
```

## 7. Built-in Plugin Specifications

### 7.1 CacheSim

**Purpose**: Set-associative cache simulation for L1I, L1D, L2, L3.

**Args**: `l1d_size=32KB`, `l1d_assoc=8`, `l1d_line=64`, `l2_size=256KB`, ...

**Algorithm**: LRU replacement. On `mem_access`:
1. Extract cache line address: `line = addr & ~(line_size - 1)`
2. Compute set index: `set = (line / line_size) % num_sets`
3. Search set for tag match → HIT (move to MRU)
4. Miss → evict LRU, install new line → MISS

**Outputs**: `hits`, `misses`, `hit_rate`, `mpki` (misses per kilo-instructions).

### 7.2 BranchTrace (NEW)

**Purpose**: Record branch direction/target for branch predictor analysis.

**Args**: `top=20` (show top N mispredicted PCs), `predictor=bimodal|gshare|tage`

**Algorithm**: Maintain a simple predictor model (2-bit bimodal by default). On each `branch` callback, predict and compare. Count mispredictions per-PC.

**Outputs**: `total_branches`, `mispredictions`, `mpki`, per-PC misprediction rate.

### 7.3 CoverageMap (NEW)

**Purpose**: Basic-block coverage bitmap (AFL-compatible).

**Args**: `output=coverage.bin`, `map_size=65536`

**Algorithm**: On `tb_exec`, hash PC to bitmap index, set bit. At atexit, write bitmap to file.

**Outputs**: `covered_blocks`, `total_blocks` (if symbol table available), bitmap file.

### 7.4 FuncProfile (NEW)

**Purpose**: Function-level profiling using the ELF symbol table.

**Args**: `symbols=binary.elf`, `top=20`

**Algorithm**: Load symbol table. On each `insn_exec`, look up PC in symbol ranges. Increment per-function counter.

**Outputs**: Per-function instruction count, sorted by hotness.

### 7.5 FlameGraph (NEW)

**Purpose**: Generate flamegraph-compatible folded stacks.

**Args**: `output=flame.folded`, `sample_interval=10000`

**Algorithm**: On `branch` callbacks with kind=Call, push function onto shadow stack. On Return, pop. Every `sample_interval` instructions, emit the current stack as a folded line.

**Outputs**: Folded stack file (compatible with `flamegraph.pl` / `inferno`).

### 7.6 Watchpoint (NEW)

**Purpose**: Break-on-access with optional value conditions.

**Args**: `addr=0x1000`, `size=8`, `type=write`, `value=0xDEAD` (optional)

**Algorithm**: On `mem_access`, check if address falls in watchpoint range. If value condition is set, compare. On match, fire fault with `Breakpoint` kind.

## 8. Cargo.toml

```toml
[package]
name    = "helm-plugin"
version.workspace = true
edition.workspace = true

[features]
default  = ["builtins"]
builtins = []
dynamic  = ["libc"]

[dependencies]
helm-core.workspace   = true
log.workspace         = true
serde.workspace       = true
serde_json.workspace  = true

[target.'cfg(unix)'.dependencies]
libc = { version = "0.2", optional = true }
```

## 9. Dependencies

```
helm-plugin
├── helm-core (MemInterface, HartException, AccessType)
├── log
├── serde + serde_json (for stats output)
└── libc (optional, for dynamic loading)

helm-engine depends on helm-plugin for:
├── PluginRegistry (callback dispatch)
├── InsnInfo, MemInfo, BranchInfo, etc.
└── register_builtins()
```

## 10. Testing Strategy

1. **Unit tests per plugin**: Construct synthetic `InsnInfo`/`MemInfo`, fire callbacks, verify counters/output
2. **Integration tests**: Run a short sequence through `HelmEngine` with plugins, verify counts match
3. **Property tests**: Random instruction sequences → InsnCount.total == engine.insns_retired
4. **Regression tests**: Known binaries → expected syscall count, branch count
5. **Performance tests**: Measure overhead of each plugin level (no plugins, insn-count only, full trace)
