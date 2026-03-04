# Plugin and Trace System

HELM's instrumentation framework, modelled after QEMU's TCG plugin
API (`qemu-plugin.h`).  Plugins observe simulation events (instruction
execution, memory access, syscalls, translation) without modifying the
core engine, enabling trace collection, profiling, cache simulation,
and custom analysis.

Reference: QEMU plugin API
- Header: `include/qemu/qemu-plugin.h`
- Built-in plugins: `contrib/plugins/`
- Docs: <https://www.qemu.org/docs/master/devel/tcg-plugins.html>

---

## 1. QEMU Plugin Architecture (Research)

### 1.1 Lifecycle

```
                qemu -plugin file=libmyplugin.so,arg=foo
                              │
                              ▼
                    qemu_plugin_install()
                    │  register callbacks:
                    │  - vcpu_init
                    │  - vcpu_tb_trans  ◄── most important
                    │  - vcpu_syscall
                    │  - atexit
                    │
          ┌─────────▼──────────────────────────────────┐
          │  Translation of each TB:                    │
          │   vcpu_tb_trans_cb(id, tb)                 │
          │     for each insn in tb:                    │
          │       register_vcpu_insn_exec_cb(insn, fn) │
          │       register_vcpu_mem_cb(insn, fn)       │
          │     register_vcpu_tb_exec_cb(tb, fn)       │
          └────────────────────────────────────────────┘
                              │
          ┌─────────▼──────────────────────────────────┐
          │  Execution:                                 │
          │   per-TB:  tb_exec_cb(vcpu_idx, tb_id)     │
          │   per-insn: insn_exec_cb(vcpu_idx, insn)   │
          │   per-mem:  mem_cb(vcpu_idx, info, vaddr)   │
          │   per-syscall: syscall_cb(...)              │
          └────────────────────────────────────────────┘
                              │
                              ▼
                         atexit_cb()
                    (print stats, flush logs)
```

### 1.2 Key Design Decisions in QEMU

1. **Callbacks registered during translation, not execution.**
   The `vcpu_tb_trans` callback runs once per TB translation.
   Within it, you decide which per-instruction or per-memory
   callbacks to attach.  This avoids runtime dispatch overhead
   for instructions you don't care about.

2. **Scoreboard API for per-vCPU data.**
   `qemu_plugin_scoreboard_new(sizeof(MyData))` allocates a
   per-vCPU array.  Accessed via `qemu_plugin_scoreboard_find(sb, vcpu_idx)`.
   No locks needed — each vCPU only touches its own slot.

3. **Inline operations for counters.**
   `qemu_plugin_register_vcpu_tb_exec_inline_per_vcpu()` can
   increment a scoreboard counter without calling into plugin
   code at all — the TCG backend emits the increment directly.

4. **Conditional callbacks.**
   Memory callbacks can be registered for reads only, writes only,
   or both.  Instruction callbacks can be conditional on flags.

5. **Multiple plugins loaded simultaneously.**
   Each gets its own `qemu_plugin_id_t`.  Callbacks are chained.

6. **Introspection during translation.**
   `qemu_plugin_insn_vaddr(insn)` — guest virtual address.
   `qemu_plugin_insn_size(insn)` — instruction byte count.
   `qemu_plugin_insn_data(insn)` — raw instruction bytes.
   `qemu_plugin_insn_disas(insn)` — disassembly string.
   `qemu_plugin_insn_symbol(insn)` — symbol name (if available).

7. **Memory access details.**
   `qemu_plugin_mem_is_store(info)` — read or write.
   `qemu_plugin_mem_size_shift(info)` — log2 of access size.
   `qemu_plugin_get_hwaddr(info, vaddr)` — physical address
   (for cache simulation).

### 1.3 QEMU Built-in Plugins

| Plugin | Purpose | Key technique |
|--------|---------|---------------|
| `execlog` | Log every executed instruction | per-insn exec callback |
| `hotblocks` | Rank basic blocks by execution count | per-TB inline counter |
| `hotpages` | Rank pages by access count | per-mem callback, page histogram |
| `howvec` | Instruction-class histogram (ALU, branch, etc.) | insn decode + per-class counter |
| `cache` | Multi-level cache simulation | per-mem callback, set-associative sim |
| `lockstep` | Compare execution of two QEMU instances | per-insn state hash |
| `bbv` | SimPoint basic-block vectors | per-TB counter, periodic dump |

### 1.4 Plugin Loading

```bash
# QEMU
qemu-aarch64 -plugin file=contrib/plugins/libexeclog.so,arg=noexec \
             -plugin file=contrib/plugins/libcache.so \
             ./my-binary

# HELM equivalent
helm run --plugin execlog --plugin cache=l1-size=32K \
         --binary ./my-binary --isa aarch64
```

---

## 2. HELM Plugin Design

### 2.1 Core Trait

```rust
/// A HELM trace/analysis plugin.
///
/// Plugins implement this trait and register callbacks during `install()`.
/// The engine calls the appropriate callbacks during simulation.
pub trait HelmPlugin: Send + Sync {
    /// Plugin name (e.g. "execlog", "cache").
    fn name(&self) -> &str;

    /// Called once at load time.  Register callbacks here.
    fn install(&mut self, ctx: &mut PluginContext, args: &PluginArgs);

    /// Called at simulation end.  Print stats, flush logs.
    fn atexit(&mut self);
}
```

### 2.2 Callback Types

```rust
/// Events a plugin can subscribe to.
pub enum PluginCallback {
    // -- vCPU lifecycle --
    VcpuInit,
    VcpuExit,

    // -- Translation (runs once per TB) --
    TbTranslate,

    // -- Execution (runs every time) --
    TbExec,
    InsnExec,

    // -- Memory --
    MemAccess { filter: MemFilter },

    // -- Syscall --
    SyscallEntry,
    SyscallReturn,

    // -- Inline (no function call, TCG emits counter increment) --
    InsnExecInline,
    TbExecInline,
}

pub enum MemFilter {
    All,
    ReadsOnly,
    WritesOnly,
}
```

### 2.3 Scoreboard (per-vCPU counters)

```rust
/// Lock-free per-vCPU data array.  Each vCPU only writes to its own
/// slot; the plugin reads all slots at atexit.
pub struct Scoreboard<T: Default + Send> {
    slots: Vec<UnsafeCell<T>>,
}

// Safe because each vCPU only accesses its own index.
unsafe impl<T: Default + Send> Sync for Scoreboard<T> {}

impl<T: Default + Send> Scoreboard<T> {
    pub fn new(num_vcpus: usize) -> Self { ... }
    pub fn get(&self, vcpu_idx: usize) -> &T { ... }
    pub fn get_mut(&self, vcpu_idx: usize) -> &mut T { ... }
}
```

### 2.4 Introspection API

```rust
/// Read-only view of an instruction during translation.
pub struct InsnInfo<'a> {
    /// Guest virtual address.
    pub vaddr: u64,
    /// Raw instruction bytes.
    pub bytes: &'a [u8],
    /// Instruction size in bytes.
    pub size: usize,
    /// Decoded mnemonic (e.g. "ADD_imm").
    pub mnemonic: &'a str,
    /// Symbol name, if debug info is available.
    pub symbol: Option<&'a str>,
}

/// Read-only view of a translated block.
pub struct TbInfo {
    /// Guest start address.
    pub pc: u64,
    /// Number of guest instructions in this TB.
    pub insn_count: usize,
    /// Total guest bytes.
    pub size: usize,
}

/// Memory access details provided to mem callbacks.
pub struct MemInfo {
    pub vaddr: u64,
    pub size: usize,
    pub is_store: bool,
    /// Physical address (if available from TLB).
    pub paddr: Option<u64>,
}

/// Syscall details.
pub struct SyscallInfo {
    pub number: u64,
    pub args: [u64; 6],
    pub vcpu_idx: usize,
}

/// Syscall return details.
pub struct SyscallRetInfo {
    pub number: u64,
    pub ret_value: u64,
    pub vcpu_idx: usize,
}
```

### 2.5 Plugin Context (registration API)

```rust
/// Passed to `HelmPlugin::install()`.  Plugins register callbacks
/// through this context.
pub struct PluginContext {
    callbacks: CallbackRegistry,
    num_vcpus: usize,
}

impl PluginContext {
    /// Register a callback for vCPU initialisation.
    pub fn on_vcpu_init(&mut self, cb: fn(vcpu_idx: usize));

    /// Register a callback for TB translation (runs once per TB).
    /// Within this callback, use `tb.on_insn_exec()` and
    /// `tb.on_mem_access()` to attach per-instruction callbacks.
    pub fn on_tb_translate(&mut self, cb: fn(tb: &mut TbTranslateCtx));

    /// Register a callback for every TB execution.
    pub fn on_tb_exec(&mut self, cb: fn(vcpu_idx: usize, tb: &TbInfo));

    /// Register a callback for syscall entry.
    pub fn on_syscall(&mut self, cb: fn(info: &SyscallInfo));

    /// Register a callback for syscall return.
    pub fn on_syscall_ret(&mut self, cb: fn(info: &SyscallRetInfo));

    /// Create a per-vCPU scoreboard.
    pub fn scoreboard<T: Default + Send>(&self) -> Scoreboard<T>;

    /// Number of vCPUs.
    pub fn num_vcpus(&self) -> usize;
}

/// Context available inside a TB-translate callback.
pub struct TbTranslateCtx {
    pub tb: TbInfo,
    pub insns: Vec<InsnInfo>,
}

impl TbTranslateCtx {
    /// Attach a per-instruction execution callback.
    pub fn on_insn_exec(&mut self, insn_idx: usize, cb: InsnExecCb);

    /// Attach a memory-access callback for a specific instruction.
    pub fn on_mem_access(
        &mut self,
        insn_idx: usize,
        filter: MemFilter,
        cb: MemAccessCb,
    );

    /// Attach an inline counter increment (no function call overhead).
    pub fn on_insn_exec_inline(
        &mut self,
        insn_idx: usize,
        counter: &ScoreboardCounter,
    );
}
```

### 2.6 Injection into TCG / Engine

During translation, the TCG emitter checks for registered callbacks
and injects instrumentation ops:

```
TCG translation of a TB:
  for each guest insn:
    1. Decode insn → TcgOps (normal)
    2. If plugin has insn_exec_cb for this insn:
         emit TcgOp::PluginCallback { id, insn_vaddr }
    3. If plugin has inline counter:
         emit TcgOp::Addi { scoreboard_slot, 1 }
    4. If plugin has mem_cb and insn is load/store:
         emit TcgOp::PluginMemCallback { id, vaddr_temp, info }
```

For the static decoder path (APE/CAE), callbacks are invoked
directly from the pipeline model's commit stage.

---

## 3. Built-in Plugins

### 3.1 execlog — Execution Trace

Logs every executed instruction with optional register state.

```
# Output format:
# vCPU  PC         insn_hex  mnemonic  [regs...]
0  0x0041112c  d2800000  MOVZ X0, #0
0  0x00411130  94000040  BL   #0x100
0  0x00411230  d10043ff  SUB  SP, SP, #16
```

### 3.2 insn-count — Instruction Counter

Counts instructions per vCPU using inline scoreboard counters.
Zero function-call overhead.  Prints total at exit.

### 3.3 hotblocks — Basic Block Profiling

Ranks TBs by execution count.  Identifies hot loops.

```
# Output:
# exec_count  tb_pc       insn_count
  1,234,567  0x00412000  12
    456,789  0x00411100  4
    123,456  0x00413800  28
```

### 3.4 howvec — Instruction Mix

Classifies instructions by type and counts each:

```
# Category     Count       %
  IntAlu     345,678   42.1%
  Branch     198,765   24.2%
  Load       123,456   15.0%
  Store       87,654   10.7%
  FpAlu        5,432    0.7%
  Syscall        189    0.0%
```

### 3.5 cache — Cache Simulation

Simulates L1D/L1I/L2/L3 caches using memory callbacks.
Reports hit/miss rates per level.

```
# Level   Hits       Misses     Hit Rate
  L1D   12,345,678    234,567   98.1%
  L1I    8,765,432     12,345   99.9%
  L2       234,567     45,678   83.7%
```

### 3.6 syscall-trace — Syscall Logger

Logs every syscall entry and return:

```
# vCPU  syscall          args                  ret
0  write(1, 0x412000, 6)                        6
0  openat(-100, "/etc/passwd", 0, 0)            3
0  read(3, 0x7fffe000, 4096)                  1234
0  close(3)                                      0
```

### 3.7 bbv — Basic Block Vectors (SimPoint)

Produces basic-block frequency vectors for SimPoint analysis.
Dumps a vector every N instructions (configurable interval).

---

## 4. Plugin Loading

### 4.1 CLI

```bash
# Single plugin
helm run --plugin insn-count --binary ./test --isa aarch64

# Multiple plugins with arguments
helm run --plugin execlog=regs=true,output=trace.log \
         --plugin cache=l1d-size=32K,l2-size=256K \
         --binary ./test --isa aarch64

# List available plugins
helm plugins list
```

### 4.2 Python

```python
from helm import Simulation, TimingMode
from helm.plugins import InsnCount, ExecLog, CacheSim

sim = Simulation(platform, binary="./test", mode="se")

# Attach plugins
sim.add_plugin(InsnCount())
sim.add_plugin(ExecLog(regs=True, output="trace.log"))
sim.add_plugin(CacheSim(l1d_size="32KB", l2_size="256KB"))

results = sim.run()

# Access plugin results
print(f"Instructions: {sim.plugin('insn-count').total()}")
print(f"L1D hit rate: {sim.plugin('cache').l1d_hit_rate():.1%}")
```

### 4.3 Dynamic Loading (Future)

External plugins as shared libraries:

```bash
helm run --plugin file=./libmy_analysis.so,arg=foo \
         --binary ./test
```

---

## 5. Performance Considerations

### 5.1 Overhead Hierarchy

| Technique | Overhead | When to use |
|-----------|----------|-------------|
| Inline counter (scoreboard) | <1% | Counting events |
| Per-TB exec callback | ~5% | Block profiling |
| Per-insn exec callback | ~20-50% | Instruction tracing |
| Per-mem callback | ~30-80% | Cache simulation |
| Per-mem + per-insn | ~50-100% | Full trace |

### 5.2 Minimising Overhead

1. **Register callbacks selectively.** In `on_tb_translate()`,
   only attach `on_insn_exec()` to instructions you care about.

2. **Use inline counters.** `on_insn_exec_inline()` avoids the
   function-call overhead entirely — the TCG backend emits a
   single `add [scoreboard_slot], 1` instruction.

3. **Filter memory callbacks.** Use `MemFilter::WritesOnly` if
   you only need store addresses.

4. **Batch output.** Buffer trace lines and flush periodically,
   not per-instruction.

---

## 6. Comparison with QEMU

| Feature | QEMU | HELM |
|---------|------|------|
| Plugin trait | C ABI (`qemu_plugin_install`) | Rust trait (`HelmPlugin`) |
| Loading | `dlopen` | Built-in + future `dlopen` |
| Scoreboard | `qemu_plugin_scoreboard_*` | `Scoreboard<T>` generic |
| Inline counters | TCG-emitted adds | `TcgOp::Addi` to scoreboard |
| Memory info | `qemu_plugin_mem_*` | `MemInfo` struct |
| Introspection | `qemu_plugin_insn_*` | `InsnInfo` struct |
| Multiple plugins | Yes (chained) | Yes (Vec of plugins) |
| Thread safety | Plugin must be thread-safe | `Send + Sync` enforced by Rust |
| Pipeline integration | TCG only | TCG + pipeline commit (APE/CAE) |
