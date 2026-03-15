# LLD: HelmEngine\<T\>

> Low-Level Design for `HelmEngine<T: TimingModel>` — the simulation kernel struct.

**Crate:** `helm-engine`
**File:** `crates/helm-engine/src/engine.rs`

---

## Table of Contents

1. [Struct Definition](#1-struct-definition)
2. [Construction](#2-construction)
3. [Execute Trait and run() Loop](#3-execute-trait-and-run-loop)
4. [Inner Loop: fetch → dispatch → timing](#4-inner-loop-fetch--dispatch--timing)
5. [ExecMode Cold Path](#5-execmode-cold-path)
6. [Syscall Dispatch](#6-syscall-dispatch)
7. [HelmEventBus Integration](#7-helmeventbus-integration)
8. [Checkpoint via HelmAttr](#8-checkpoint-via-helmattr)
9. [Hart Trait Implementation](#9-hart-trait-implementation)
10. [ThreadContext Implementation](#10-threadcontext-implementation)
11. [Performance Invariants](#11-performance-invariants)

---

## 1. Struct Definition

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use helm_core::{ArchState, ExecMode, Isa, ThreadContext, SyscallHandler};
use helm_memory::MemoryMap;
use helm_timing::TimingModel;
use helm_devices::bus::event_bus::{HelmEventBus, HelmEvent};

use crate::{StopReason, CHECKPOINT_VERSION};

/// The simulation kernel. Generic over timing model only.
///
/// # Type Parameter
///
/// `T: TimingModel` is monomorphized at compile time. `T::on_memory_access()` is
/// `#[inline(always)]` and emits zero vtable overhead. The three concrete variants
/// (`Virtual`, `Interval`, `Accurate`) each produce a distinct binary specialization.
///
/// # Ownership
///
/// `HelmEngine<T>` owns `ArchState` and `MemoryMap`. This is a deliberate choice:
/// - Checkpoint serialization needs a single owner with a clear lifetime boundary.
/// - PyO3 requires `'static` wrapped types; borrows cannot satisfy this.
/// - Phase 0 is single-hart; shared memory is introduced in Phase 3 via `Arc<MemoryMap>`.
pub struct HelmEngine<T: TimingModel> {
    // ── Identity ─────────────────────────────────────────────────────────────
    /// ISA this hart executes. Checked every instruction via enum match.
    /// Branch predictor learns the constant value; cost is effectively zero.
    pub isa: Isa,

    /// Execution mode: Functional | Syscall | System.
    /// Checked only on syscall-class instructions — cold path.
    pub mode: ExecMode,

    // ── Timing model ─────────────────────────────────────────────────────────
    /// Timing model, monomorphized. Zero-sized for `Virtual`; lightweight for others.
    /// `T::on_memory_access()` is inlined into the fetch and memory-access sites.
    pub timing: T,

    // ── Architectural state ───────────────────────────────────────────────────
    /// Owned architectural register file. Includes integer regs, FP regs, PC, CSRs.
    /// Passed by mutable reference to ISA step functions; never shared concurrently.
    pub arch: ArchState,

    // ── Memory ────────────────────────────────────────────────────────────────
    /// Owned memory map. Contains the FlatView of all mapped regions.
    /// Phase 0: owned. Phase 3 (multi-hart FS): becomes Arc<MemoryMap>.
    pub memory: MemoryMap,

    // ── Syscall handler ───────────────────────────────────────────────────────
    /// Syscall handler, set at configuration time, called on cold path only.
    /// `None` in Functional mode (traps would fault). `Some(LinuxSyscallHandler)`
    /// in Syscall mode. `Some(KernelSyscallRouter)` in System mode.
    pub syscall_handler: Option<Box<dyn SyscallHandler>>,

    // ── Event bus ─────────────────────────────────────────────────────────────
    /// Shared event bus. `fire()` is called on exception, magic instruction,
    /// breakpoint hit, and syscall enter/return. Subscribers include TraceLogger,
    /// GdbServer, and Python callbacks. `Arc` because multiple components subscribe.
    pub event_bus: Arc<HelmEventBus>,

    // ── Execution control ─────────────────────────────────────────────────────
    /// Stop flag. Set by an event bus subscriber (e.g., breakpoint handler).
    /// Checked at the top of each instruction iteration. `Relaxed` ordering —
    /// on x86_64/AArch64 this compiles to a plain load, no fence.
    stop_flag: Arc<AtomicBool>,

    /// Reason for the most recent stop, written by the inner loop, read by callers.
    last_stop_reason: StopReason,

    // ── Counters ──────────────────────────────────────────────────────────────
    /// Total instructions retired by this hart since construction.
    pub insns_executed: u64,

    /// Current simulated tick (instruction count in Virtual mode;
    /// estimated cycle count in Interval/Accurate).
    pub current_tick: u64,
}
```

---

## 2. Construction

```rust
impl<T: TimingModel> HelmEngine<T> {
    /// Construct a new hart. Called by `build_simulator()` and directly in tests.
    ///
    /// Does not run any SimObject lifecycle (no `init`, `elaborate`, `startup`) —
    /// those belong to the System tree, which is optional (SE mode, no devices).
    pub fn new(isa: Isa, mode: ExecMode, timing: T) -> Self {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let event_bus = Arc::new(HelmEventBus::new());

        // Register the stop-on-breakpoint subscriber immediately.
        // This subscriber fires when HelmEvent::Breakpoint is received,
        // setting the stop_flag so the inner loop exits.
        {
            let flag = Arc::clone(&stop_flag);
            event_bus.subscribe(HelmEventKind::Breakpoint, move |_event| {
                flag.store(true, Ordering::Relaxed);
            });
        }

        Self {
            isa,
            mode,
            timing,
            arch: ArchState::new(isa),
            memory: MemoryMap::new(),
            syscall_handler: None,
            event_bus,
            stop_flag,
            last_stop_reason: StopReason::QuantumExhausted,
            insns_executed: 0,
            current_tick: 0,
        }
    }

    /// Install a syscall handler. Called by `build_simulator()` for Syscall/System modes.
    pub fn set_syscall_handler(&mut self, h: Box<dyn SyscallHandler>) {
        self.syscall_handler = Some(h);
    }
}
```

---

## 3. Execute Trait and run() Loop

`HelmEngine<T>` implements the `Execute` trait, which is the interface used by the `Scheduler`:

```rust
/// Execute trait — implemented by HelmEngine<T>, used by Scheduler.
///
/// `budget` is an instruction count ceiling. The engine runs until one of:
///   - `budget` instructions have retired → returns QuantumExhausted
///   - `stop_flag` is set (breakpoint, external pause) → returns Breakpoint
///   - A SimExit syscall is intercepted → returns SimExit { code }
///   - An unhandled exception (Functional mode) → returns Exception { vector, pc }
pub trait Execute {
    fn run(&mut self, budget: u64) -> StopReason;
    fn step_once(&mut self) -> StopReason;
}

impl<T: TimingModel> Execute for HelmEngine<T> {
    fn run(&mut self, budget: u64) -> StopReason {
        self.stop_flag.store(false, Ordering::Relaxed);
        let mut retired = 0u64;

        loop {
            // Check stop flag first (breakpoint, external pause).
            // Relaxed load — plain mov on x86_64, no fence.
            if self.stop_flag.load(Ordering::Relaxed) {
                self.last_stop_reason = StopReason::Breakpoint {
                    pc: self.arch.pc(),
                };
                return self.last_stop_reason;
            }

            // Check budget.
            if retired >= budget {
                self.last_stop_reason = StopReason::QuantumExhausted;
                return self.last_stop_reason;
            }

            // Execute one instruction. Returns Err on fatal exception.
            match self.step_inner() {
                Ok(()) => {
                    retired += 1;
                    self.insns_executed += 1;
                    self.current_tick += 1;
                }
                Err(reason) => {
                    self.last_stop_reason = reason;
                    return reason;
                }
            }
        }
    }

    fn step_once(&mut self) -> StopReason {
        self.run(1)
    }
}
```

---

## 4. Inner Loop: fetch → dispatch → timing

```rust
impl<T: TimingModel> HelmEngine<T> {
    /// Execute one instruction. Hot path — must not allocate or take locks.
    ///
    /// Returns Ok(()) on successful retire, Err(StopReason) on fault/exit.
    #[inline(always)]
    fn step_inner(&mut self) -> Result<(), StopReason> {
        // ── 1. Fetch ─────────────────────────────────────────────────────────
        // Fetch 4 bytes at PC. For RISC-V C extension, check bit [1:0];
        // if != 0b11, it's a 16-bit compressed instruction.
        let pc = self.arch.pc();

        let raw_bytes = self.memory.read_u32_ifetch(pc)
            .map_err(|fault| {
                self.fire_exception(fault.vector(), pc, fault.tval());
                StopReason::Exception { vector: fault.vector(), pc }
            })?;

        // Notify timing model of the instruction fetch (I-cache access).
        // For `Virtual`, this is a no-op (zero-sized struct, inlined away).
        // For `Interval`/`Accurate`, this updates cache model state.
        self.timing.on_memory_access(pc, /*is_write=*/false, /*size=*/4);

        // ── 2. ISA dispatch ──────────────────────────────────────────────────
        // Enum match here. Branch predictor learns the constant arm after
        // a handful of iterations — effectively as fast as a static dispatch.
        match self.isa {
            Isa::RiscV   => self.step_riscv(raw_bytes)?,
            Isa::AArch64 => self.step_aarch64(raw_bytes)?,
            Isa::AArch32 => self.step_aarch32(raw_bytes)?,
        }

        Ok(())
    }

    /// Fire an architectural exception via the event bus, then handle it.
    ///
    /// In Functional mode: fires HelmEvent::Exception and returns Err to stop loop.
    /// In Syscall/System mode: installs trap PC into arch state, continues loop.
    fn fire_exception(&mut self, vector: u32, pc: u64, tval: u64) {
        self.event_bus.fire(HelmEvent::Exception { cpu: "cpu0", vector, pc, tval });

        match self.mode {
            ExecMode::Functional => {
                // Exceptions terminate execution in functional mode.
                // The Err propagates out of step_inner → run().
            }
            ExecMode::Syscall | ExecMode::System => {
                // Install trap vector PC. The ISA step function handles
                // trap entry (save PC to mepc/ELR_EL1, update privilege level).
                // For now, we set a flag and let the ISA layer handle it.
                self.arch.set_pending_exception(vector, tval);
            }
        }
    }
}
```

### RISC-V Step Function

`step_riscv()` is implemented in `helm-arch` and injected into `HelmEngine<T>` via a blanket impl or direct method resolution. The key design point is that the timing model's `on_memory_access()` is called inline from within the step function:

```rust
// In helm-arch/src/riscv/execute.rs, compiled into HelmEngine<T>:

impl<T: TimingModel> HelmEngine<T> {
    /// Execute one RISC-V instruction. Called from step_inner().
    ///
    /// Memory accesses call self.timing.on_memory_access() inline.
    /// T is monomorphized — the compiler inlines the timing model call
    /// and can eliminate dead code for zero-cost timing models (Virtual).
    pub(crate) fn step_riscv(&mut self, raw: u32) -> Result<(), StopReason> {
        use helm_arch::riscv::{decode, Insn};

        let insn = decode(raw);

        match insn {
            Insn::Nop => {
                self.arch.advance_pc(4);
            }

            Insn::Addi { rd, rs1, imm } => {
                let val = self.arch.read_int(rs1).wrapping_add(imm as u64);
                self.arch.write_int(rd, val);
                self.arch.advance_pc(4);
            }

            Insn::Lw { rd, rs1, offset } => {
                let addr = self.arch.read_int(rs1).wrapping_add(offset as u64);

                // Memory read — cold/warm depending on cache model.
                let val = self.memory.read_u32(addr)
                    .map_err(|f| StopReason::Exception { vector: f.vector(), pc: self.arch.pc() })?;

                // Timing hook — inlined. For Virtual: noop.
                // For Interval: update cache model, compute miss penalty.
                self.timing.on_memory_access(addr, /*is_write=*/false, /*size=*/4);

                self.arch.write_int(rd, val as u64);
                self.arch.advance_pc(4);
            }

            Insn::Sw { rs1, rs2, offset } => {
                let addr = self.arch.read_int(rs1).wrapping_add(offset as u64);
                let val = self.arch.read_int(rs2) as u32;

                self.memory.write_u32(addr, val)
                    .map_err(|f| StopReason::Exception { vector: f.vector(), pc: self.arch.pc() })?;

                // Timing hook for write.
                self.timing.on_memory_access(addr, /*is_write=*/true, /*size=*/4);

                self.arch.advance_pc(4);
            }

            Insn::Ecall => {
                // Syscall instruction — ExecMode dispatch on cold path.
                self.handle_ecall()?;
            }

            Insn::Ebreak => {
                // Magic breakpoint instruction.
                let pc = self.arch.pc();
                self.event_bus.fire(HelmEvent::MagicInsn { pc, value: 0 });
                // stop_flag will be set by the Breakpoint subscriber above.
                self.arch.advance_pc(4);
            }

            // ... all other RISC-V instructions
        }

        Ok(())
    }
}
```

---

## 5. ExecMode Cold Path

`ExecMode` is checked only when a syscall-class instruction (`ecall` for RISC-V, `svc` for AArch64) is encountered. This is a cold path — user binaries execute syscalls at a rate of roughly 1 per 10K–100K instructions.

```rust
impl<T: TimingModel> HelmEngine<T> {
    /// Handle an ecall (RISC-V syscall instruction).
    ///
    /// This is called from step_riscv() on the cold path only.
    /// ExecMode dispatch here does NOT need branch predictor optimization —
    /// the mode is constant per simulation run.
    fn handle_ecall(&mut self) -> Result<(), StopReason> {
        // Fire SyscallEnter event (for tracing).
        let nr = self.arch.read_int(17);  // a7 = syscall number in RISC-V ABI
        let args = [
            self.arch.read_int(10),  // a0
            self.arch.read_int(11),  // a1
            self.arch.read_int(12),  // a2
            self.arch.read_int(13),  // a3
            self.arch.read_int(14),  // a4
            self.arch.read_int(15),  // a5
        ];
        self.event_bus.fire(HelmEvent::SyscallEnter { nr, args });

        match self.mode {
            ExecMode::Functional => {
                // Functional mode: ecall causes an illegal instruction exception.
                // The caller sees StopReason::Exception.
                self.fire_exception(/*Environment call from U-mode*/8, self.arch.pc(), 0);
                return Err(StopReason::Exception { vector: 8, pc: self.arch.pc() });
            }

            ExecMode::Syscall => {
                // Syscall emulation: dispatch to host OS handler.
                let handler = self.syscall_handler.as_mut()
                    .expect("ExecMode::Syscall requires syscall_handler to be set");

                let ret = handler.handle(nr, &args, self as &mut dyn ThreadContext);

                // Check for exit/exit_group.
                if nr == 93 || nr == 94 {  // __NR_exit, __NR_exit_group
                    let code = args[0] as i32;
                    return Err(StopReason::SimExit { code });
                }

                // Write return value to a0.
                self.arch.write_int(10, ret);

                // Fire SyscallReturn event.
                self.event_bus.fire(HelmEvent::SyscallReturn { nr, ret });

                self.arch.advance_pc(4);
                Ok(())
            }

            ExecMode::System => {
                // Full system: route to simulated OS (through trap vector).
                // The kernel handles this via the interrupt/trap mechanism.
                self.arch.take_exception(/*M-mode ecall*/11);
                self.arch.advance_pc(4);
                Ok(())
            }
        }
    }
}
```

### Why ExecMode Is a Cold-Path Enum (Not a Generic Parameter)

The alternative design would be `HelmEngine<T: TimingModel, M: ExecMode>`, producing 9 variants (3 timing × 3 mode). The council debate concluded:

1. `ecall` instructions appear ~1 per 10K–100K instructions. Even a branch misprediction (worst case: 15 cycles) at this frequency costs less than 0.002% of total cycles.
2. Three generic parameters (`T`, `M`, `I` for ISA) would produce 27 monomorphized variants, bloating binary size and compile time significantly.
3. The branch predictor reliably predicts the constant mode value after the first iteration.

---

## 6. Syscall Dispatch

```rust
/// Syscall handler trait — implemented by helm-engine/se (LinuxSyscallHandler)
/// and potentially by a full-system OS router in helm-fs.
///
/// Receives the syscall number, argument array, and a mutable ThreadContext
/// for register file access. Returns the syscall return value (i64).
pub trait SyscallHandler: Send + 'static {
    fn handle(
        &mut self,
        nr: u64,
        args: &[u64; 6],
        tc: &mut dyn ThreadContext,
    ) -> u64;
}
```

The ISA-specific ABI mapping (RISC-V: a7=nr, a0–a5=args, a0=ret; AArch64: x8=nr, x0–x5=args, x0=ret) is handled in `handle_ecall()` / `handle_svc()` before calling the handler. The `SyscallHandler` trait receives an ISA-neutral normalized view: syscall number as `u64`, six arguments as `[u64; 6]`.

---

## 7. HelmEventBus Integration

`HelmEngine<T>` fires events on the following occasions:

| Event | When fired | Frequency |
|---|---|---|
| `HelmEvent::Exception` | Any trap/fault | Rare |
| `HelmEvent::SyscallEnter` | Every `ecall`/`svc` before dispatch | ~1 per 10K–100K insns |
| `HelmEvent::SyscallReturn` | Every `ecall`/`svc` after return | ~1 per 10K–100K insns |
| `HelmEvent::MagicInsn` | `ebreak` (RISC-V) or magic constant | Debug only |
| `HelmEvent::CsrWrite` | CSR write instruction | Rare |
| `HelmEvent::MemWrite` | If MemWrite subscriber present (opt-in) | Per data write |

`HelmEvent::MemWrite` is **opt-in** because firing it on every store instruction would add an event bus check to the hot path. When no subscriber is registered for `MemWrite`, the engine skips the `fire()` call entirely via a `has_subscribers` flag in `HelmEventBus`.

```rust
// Opt-in MemWrite notification — only fires if a subscriber is registered.
if self.event_bus.has_subscribers(HelmEventKind::MemWrite) {
    self.event_bus.fire(HelmEvent::MemWrite { addr, size: 4, val: val as u64, cycle: self.current_tick });
}
```

### Stop Flag and Breakpoints

The stop flag mechanism is described in HLD Q15. The concrete wiring:

```rust
// In HelmEngine::new():
let flag = Arc::clone(&self.stop_flag);
self.event_bus.subscribe(HelmEventKind::Breakpoint, move |_event| {
    flag.store(true, Ordering::Relaxed);
});

// The GDB stub calls:
engine.event_bus.fire(HelmEvent::Breakpoint { pc });
// → subscriber sets stop_flag → inner loop exits at next iteration top
```

---

## 8. Checkpoint via HelmAttr

`HelmEngine<T>` implements checkpoint save/restore directly (not via `SimObject`, which it does not implement):

```rust
const ENGINE_CKPT_VERSION: u32 = 1;

impl<T: TimingModel> HelmEngine<T> {
    /// Serialize all architectural state. Does NOT serialize:
    ///   - Performance counters (insns_executed, current_tick) — reset on restore
    ///   - Timing model internal state — not architectural
    ///   - Event bus subscriptions — re-established at construction
    pub fn checkpoint_save(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4096);

        // 4-byte version tag
        buf.extend_from_slice(&ENGINE_CKPT_VERSION.to_le_bytes());

        // ISA and mode (for compatibility check on restore)
        buf.push(self.isa as u8);
        buf.push(self.mode as u8);

        // ArchState: PC + integer registers + FP registers + CSRs
        let arch_bytes = self.arch.checkpoint_save();
        let arch_len = arch_bytes.len() as u32;
        buf.extend_from_slice(&arch_len.to_le_bytes());
        buf.extend(arch_bytes);

        // MemoryMap: RAM contents only (MMIO state is in device checkpoints)
        let mem_bytes = self.memory.checkpoint_save_ram();
        let mem_len = mem_bytes.len() as u32;
        buf.extend_from_slice(&mem_len.to_le_bytes());
        buf.extend(mem_bytes);

        buf
    }

    /// Restore from checkpoint. Panics on version or ISA mismatch.
    pub fn checkpoint_restore(&mut self, data: &[u8]) {
        let version = u32::from_le_bytes(data[0..4].try_into().unwrap());
        assert_eq!(version, ENGINE_CKPT_VERSION,
            "HelmEngine checkpoint version mismatch: blob={version} current={ENGINE_CKPT_VERSION}");

        let isa_byte = data[4];
        assert_eq!(isa_byte, self.isa as u8,
            "HelmEngine checkpoint ISA mismatch");
        let mode_byte = data[5];
        assert_eq!(mode_byte, self.mode as u8,
            "HelmEngine checkpoint ExecMode mismatch");

        let arch_len = u32::from_le_bytes(data[6..10].try_into().unwrap()) as usize;
        self.arch.checkpoint_restore(&data[10..10 + arch_len]);

        let mem_off = 10 + arch_len;
        let mem_len = u32::from_le_bytes(data[mem_off..mem_off + 4].try_into().unwrap()) as usize;
        self.memory.checkpoint_restore_ram(&data[mem_off + 4..mem_off + 4 + mem_len]);

        // Reset non-architectural counters.
        self.insns_executed = 0;
        self.current_tick = 0;
        self.stop_flag.store(false, Ordering::Relaxed);
    }
}
```

---

## 9. Hart Trait Implementation

`HelmEngine<T>` implements the `Hart` trait from `helm-core`, enabling the `Scheduler` to manage harts without knowing `T`:

```rust
pub trait Hart: Send {
    fn step(&mut self, mem: &mut dyn MemInterface) -> Result<(), HartException>;
    fn get_pc(&self) -> u64;
    fn get_int_reg(&self, idx: usize) -> u64;
    fn set_int_reg(&mut self, idx: usize, val: u64);
    fn isa(&self) -> Isa;
    fn exec_mode(&self) -> ExecMode;
    fn thread_context(&mut self) -> &mut dyn ThreadContext;
}

impl<T: TimingModel> Hart for HelmEngine<T> {
    fn step(&mut self, _mem: &mut dyn MemInterface) -> Result<(), HartException> {
        // HelmEngine owns its own MemoryMap; the MemInterface arg is ignored here.
        // It exists for compatibility with the Hart trait contract.
        self.step_inner().map_err(|reason| HartException::from(reason))
    }

    fn get_pc(&self) -> u64 { self.arch.pc() }
    fn get_int_reg(&self, idx: usize) -> u64 { self.arch.read_int(idx as u32) }
    fn set_int_reg(&mut self, idx: usize, val: u64) { self.arch.write_int(idx as u32, val); }
    fn isa(&self) -> Isa { self.isa }
    fn exec_mode(&self) -> ExecMode { self.mode }
    fn thread_context(&mut self) -> &mut dyn ThreadContext { self }
}
```

---

## 10. ThreadContext Implementation

`HelmEngine<T>` implements `ThreadContext` from `helm-core`. This is the cold-path interface used by GDB stub, syscall handler, Python API, and checkpoint:

```rust
impl<T: TimingModel> ThreadContext for HelmEngine<T> {
    fn read_int_reg(&self, idx: u32) -> u64 { self.arch.read_int(idx) }
    fn write_int_reg(&mut self, idx: u32, val: u64) { self.arch.write_int(idx, val); }

    fn read_float_reg(&self, idx: u32) -> u64 { self.arch.read_float_raw(idx) }
    fn write_float_reg(&mut self, idx: u32, val: u64) { self.arch.write_float_raw(idx, val); }

    fn read_csr(&self, csr: u16) -> u64 { self.arch.read_csr(csr) }
    fn write_csr(&mut self, csr: u16, val: u64) {
        let old = self.arch.read_csr(csr);
        self.arch.write_csr(csr, val);
        self.event_bus.fire(HelmEvent::CsrWrite { csr, old, new: val });
    }

    fn read_pc(&self) -> u64 { self.arch.pc() }
    fn write_pc(&mut self, pc: u64) { self.arch.set_pc(pc); }

    fn get_hart_id(&self) -> u64 { 0 }  // single-hart in Phase 0
    fn get_isa(&self) -> Isa { self.isa }
    fn get_exec_mode(&self) -> ExecMode { self.mode }

    fn pause(&mut self) { self.stop_flag.store(true, Ordering::Relaxed); }
    fn resume(&mut self) { self.stop_flag.store(false, Ordering::Relaxed); }

    fn read_mem_functional(&self, addr: u64, size: usize) -> Result<u64, MemFault> {
        self.memory.read_functional(addr, size)
    }
    fn write_mem_functional(&mut self, addr: u64, size: usize, val: u64) -> Result<(), MemFault> {
        self.memory.write_functional(addr, size, val)
    }
}
```

---

## 11. Performance Invariants

The following invariants must hold for the inner loop. Violations are regressions:

| Invariant | Mechanism | Verification |
|---|---|---|
| No heap allocation per instruction | All step_* methods are allocation-free | `#[global_allocator]` counter in bench |
| No mutex acquisition per instruction | All hot-path state is owned by HelmEngine | Thread sanitizer + lock counting |
| `T::on_memory_access()` inlined | `#[inline(always)]` on TimingModel methods | `objdump` / `cargo-asm` inspection |
| `stop_flag` load is a plain mov | `Ordering::Relaxed` on x86_64/AArch64 | `cargo-asm` inspection |
| ISA dispatch: single branch | `match self.isa` in `step_inner` | Benchmark: Virtual mode instruction rate |
| ExecMode checked only on ecall/svc | Mode match inside `handle_ecall` only | Code review invariant |

### Target Performance

| Timing model | Target insns/sec | Notes |
|---|---|---|
| `Virtual` | 100M–500M | Zero timing overhead; limited by fetch + decode |
| `Interval` | 10M–50M | Cache model lookup per instruction |
| `Accurate` | 0.1M–1M | Pipeline stage simulation per cycle |

These targets are for RISC-V RV64 on a modern x86_64 host. AArch64 decode is ~15% slower due to more irregular encoding.

---

## 12. Design Decisions from Q&A

### Design Decision: ArchState ownership and MemoryMap sharing (Q10)

`HelmEngine<T>` **owns `ArchState`** (by value) and holds **`Arc<RwLock<MemoryMap>>`** for the shared memory map. Register state is intrinsically per-hart and must be checkpointed with the hart. The memory map is intrinsically shared in a multi-hart system — owning it per-engine would require full duplication or a reference-counted wrapper. The struct shown in §1 uses owned `MemoryMap` as a Phase-0 simplification; in multi-hart mode (Phase 3) this becomes `Arc<RwLock<MemoryMap>>`. Python inspection of register state goes through `HelmEngine::arch_state() -> &dyn ArchState`.

### Design Decision: HelmEngine does not implement SimObject (Q11)

`HelmEngine<T>` implements `Hart` and `Execute` only. `HelmSim` (the outer type-erasing enum) implements `SimObject`. Rationale: `HelmEngine` is generic over `T: TimingModel` — making it implement `SimObject` directly would require `SimObject` to be object-safe or tie the component tree to the timing model. `HelmSim` already erases `T` for external consumers and is the natural `SimObject` boundary. `World` stores `Vec<Box<dyn SimObject>>` where each entry may be a `HelmSim`. Checkpoint calls `HelmSim::serialize()` which delegates to `HelmEngine`.

### Design Decision: Execute::run() returns Result (Q15)

`Execute::run()` returns `Result<u64, StopReason>` where `StopReason` includes `Breakpoint`, `Exception`, and `QuantumEnd`. Each instruction step returns `?` to propagate early exits. The `Result`-based approach is idiomatic Rust and integrates naturally with the error-propagation model already used for memory faults. It avoids global `AtomicBool` state and makes control flow explicit. The struct shown in §3 shows a simplified `StopReason` return — the final signature is `fn run(&mut self, budget: u64) -> Result<u64, StopReason>`.

### Design Decision: Shared MemoryMap for multi-hart (Q16)

All harts share a single `Arc<RwLock<MemoryMap>>`. TLB state is per-hart (owned by `ArchState`); the physical address map is global. The `RwLock` is acceptable because: (a) in functional mode, harts run sequentially (no contention); (b) in timing mode with temporal decoupling, harts run in separate quanta and only synchronize at boundaries, so the map is effectively read-only during a quantum. `World::add_hart()` clones the `Arc` for each new hart.

### Design Decision: ThreadContext exposure from HelmSim (Q12)

`HelmSim` exposes `&dyn ThreadContext` via a method, where `ThreadContext` is the object-safe cold-path inspection trait. The PyO3 layer calls `sim.thread_context()` and dispatches named reads/writes through `ThreadContext`. `ThreadContext` trait must be object-safe — no generic methods. `HelmSim` adds `fn thread_context(&self) -> &dyn ThreadContext` with a match arm per variant.

---

*See [`HLD.md`](HLD.md) for crate-level context.*
*See [`LLD-helm-sim.md`](LLD-helm-sim.md) for the HelmSim enum and factory.*
*See [`LLD-scheduler.md`](LLD-scheduler.md) for multi-hart scheduling.*
*See [`TEST.md`](TEST.md) for unit and benchmark tests.*
