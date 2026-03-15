# ISA Author Guide

> For: engineers adding a new ISA (e.g., MIPS, x86-64) or extending an existing one (e.g., adding a RISC-V extension, adding AArch32 support to the AArch64 crate).
>
> Prerequisites: deep familiarity with the target ISA's architecture manual. Understanding of Rust traits and generics. Read [`AGENT.md`](../../AGENT.md) and [`docs/design/helm-arch/HLD.md`](../design/helm-arch/HLD.md) first.
>
> Cross-references: [`docs/design/helm-arch/HLD.md`](../design/helm-arch/HLD.md) · [`docs/design/helm-arch/LLD-riscv-decode.md`](../design/helm-arch/LLD-riscv-decode.md) · [`docs/design/helm-arch/LLD-riscv-execute.md`](../design/helm-arch/LLD-riscv-execute.md) · [`docs/design/helm-arch/TEST.md`](../design/helm-arch/TEST.md) · [`DESIGN-QUESTIONS.md`](../design/DESIGN-QUESTIONS.md) Q1–Q6

---

## 1. What a New ISA Requires

Adding a new ISA involves four components, all inside a new crate `crates/helm-arch-{name}/` (or as a new module under `crates/helm-arch/src/{name}/`):

| Component | Trait / Type | Crate |
|-----------|-------------|-------|
| Architectural state (registers, PC, privilege) | Implement `ExecContext` (hot) | `helm-core` |
| Cold-path state (GDB, checkpoint, syscall ABI) | Implement `ThreadContext` (cold) | `helm-core` |
| Instruction decode | `fn decode_{name}(raw: u32) -> Result<Instruction, DecodeError>` | new ISA crate |
| Instruction execute | `fn execute_{name}<C: ExecContext>(insn, ctx: &mut C) -> Result<(), HartException>` | new ISA crate |
| Syscall ABI mapping | Implement `SyscallAbi` on the hart struct | `helm-engine/se` |

Additionally: add an `Isa` enum variant in `helm-core`, add a factory arm in `helm-engine`'s `build_simulator()`, and add the new variant to `HelmSim` dispatch.

**Key design constraint:** `helm-arch-{name}` depends only on `helm-core`. It must not depend on `helm-memory`, `helm-timing`, `helm-event`, `helm-devices`, or `helm-engine`. Memory access goes through `ExecContext::read_mem` / `write_mem`, which the engine implements. This keeps the ISA crate independently testable.

---

## 2. ExecContext — Hot Path Trait

`ExecContext` is the interface between the instruction execute loop and the execution state. It is invoked for every instruction — at 100+ MIPS, every nanosecond of overhead accumulates. It is **always statically dispatched** (generic parameter `H: ExecContext`), never `dyn ExecContext`.

The complete method set, as specified in [`DESIGN-QUESTIONS.md` Q6](../design/DESIGN-QUESTIONS.md):

```rust
// helm-core/src/exec_context.rs

pub trait ExecContext {
    // ── Integer registers — called on nearly every instruction ───────────────
    fn read_ireg(&self, reg: IReg) -> u64;
    fn write_ireg(&mut self, reg: IReg, val: u64);

    // ── Floating-point registers — raw u64 bits (NaN-box invariant) ─────────
    // Stored as [u64; 32] — NaN-boxing handled at instruction boundaries,
    // not in the register file. See Q2.
    fn read_freg(&self, reg: FReg) -> u64;
    fn write_freg(&mut self, reg: FReg, val: u64);

    // ── Program counter — called every instruction ───────────────────────────
    fn read_pc(&self) -> u64;
    fn write_next_pc(&mut self, val: u64);    // sets PC for the NEXT cycle

    // ── CSR / system registers — called on CSR instructions only ────────────
    // These may trigger side effects (satp → TLB flush, mstatus → mode change).
    // Returns Err(CsrFault) for access violations or unimplemented CSRs.
    fn read_csr(&self, csr: u16) -> Result<u64, CsrFault>;
    fn write_csr(&mut self, csr: u16, val: u64) -> Result<(), CsrFault>;

    // ── Privilege level — needed for address translation mode selection ───────
    fn privilege_level(&self) -> PrivilegeLevel;

    // ── Exception entry — unwinds execute loop via StopReason::Exception ─────
    // This method is diverging (never returns normally). The execute loop
    // catches the stop reason and dispatches accordingly.
    fn raise_exception(&mut self, cause: ExceptionCause) -> !;

    // ── SC failure counter — RISC-V LR/SC atomics ────────────────────────────
    // Non-RISC-V ISAs implement these as no-ops returning 0.
    fn read_sc_failures(&self) -> u32;
    fn write_sc_failures(&mut self, n: u32);
}
```

**Implementation notes:**

- `read_ireg(IReg::zero)` must always return 0. `write_ireg(IReg::zero, val)` must be a no-op. Enforce this in the implementation, not in the execute functions.
- `write_next_pc()` sets the PC that will be used at the start of the next instruction fetch. It does not change the current PC mid-instruction.
- `raise_exception()` is implemented using `StopReason` — a Rust mechanism (e.g., `panic!` with a caught payload, or a thread-local flag checked by the execute loop) that unwinds back to the engine without going through the Rust panic infrastructure. The exact mechanism is an implementation detail of `helm-engine`.

---

## 3. ThreadContext — Cold Path Trait

`ThreadContext` is the interface for operations that happen at thread-management granularity: GDB register reads, checkpoint save/restore, OS syscall dispatch, context switches. It is always `&mut dyn ThreadContext` — dynamic dispatch is acceptable because these methods are not called in the hot execution loop.

The complete method set, as specified in [`DESIGN-QUESTIONS.md` Q6](../design/DESIGN-QUESTIONS.md):

```rust
// helm-core/src/thread_context.rs

pub trait ThreadContext {
    // ── Identity ──────────────────────────────────────────────────────────────
    fn hart_id(&self) -> u32;
    fn isa(&self) -> Isa;

    // ── Full register file access — for GDB, checkpoint, context switch ───────
    // idx 0..32 for integer registers; x0 returns 0, writes to x0 are no-ops.
    fn read_ireg_raw(&self, idx: usize) -> u64;
    fn write_ireg_raw(&mut self, idx: usize, val: u64);
    fn read_freg_raw(&self, idx: usize) -> u64;
    fn write_freg_raw(&mut self, idx: usize, val: u64);

    // ── Program counter — direct set for GDB, checkpoint, context switch ─────
    fn read_pc(&self) -> u64;
    fn set_pc(&mut self, val: u64);

    // ── Privilege level — for GDB, checkpoint, OS context switch ─────────────
    fn privilege_level(&self) -> PrivilegeLevel;
    fn set_privilege_level(&mut self, pl: PrivilegeLevel);

    // ── CSR / system-register raw access — no side effects ───────────────────
    // Used by GDB to read/write CSRs without triggering architecture side effects.
    // Also used by checkpoint save/restore.
    fn read_csr_raw(&self, csr: u16) -> u64;
    fn write_csr_raw(&mut self, csr: u16, val: u64);

    // ── Syscall ABI convenience — SE mode only ────────────────────────────────
    // Reads syscall arguments from the ISA-specific ABI registers (a0-a7 for RISC-V,
    // x0-x7 for AArch64 AAPCS64). Returns them in a canonical SyscallArgs struct.
    fn syscall_args(&self) -> SyscallArgs;
    // Writes the syscall return value to the ISA-specific return register.
    fn set_syscall_return(&mut self, val: i64);

    // ── Lifecycle — activate/suspend for multi-hart scheduling ────────────────
    fn status(&self) -> HartStatus;    // Running | Suspended | Halted
    fn activate(&mut self);
    fn suspend(&mut self);
    fn halt(&mut self);

    // ── Checkpoint hooks — called by CheckpointManager ───────────────────────
    fn save_attrs(&self, store: &mut AttrStore);
    fn restore_attrs(&mut self, store: &AttrStore);
}
```

**Implementing both traits on the same struct:**

The hart struct implements both `ExecContext` and `ThreadContext`. A bridge method allows cold-path callers to get a `ThreadContext` reference:

```rust
impl MyMipsHart {
    /// Cold-path only. Marked inline(never) to keep off the hot path.
    #[inline(never)]
    pub fn as_thread_context(&mut self) -> &mut dyn ThreadContext {
        self
    }
}
```

The engine holds `hart: H where H: ExecContext` in the hot loop. When a syscall or GDB access is needed, it calls `hart.as_thread_context()` and passes the result to the cold-path handler. This matches gem5's `xc->tcBase()` pattern exactly.

---

## 4. Decode Loop Structure

The decode loop in `helm-engine` drives the ISA crate. The engine calls the ISA's decode and execute functions; the ISA crate knows nothing about the engine's timing model or event queue.

### 4.1 Fetch-Decode-Execute Pattern

```rust
// Inside helm-engine — simplified
fn step_mips<H: ExecContext>(hart: &mut H, mem: &mut MemoryMap) {
    // 1. Fetch: read instruction word from PC
    let pc = hart.read_pc();
    let raw: u32 = mem.read_atomic(pc, 4)
        .map_err(|fault| hart.raise_exception(fault.into_exception_cause()))
        .unwrap(); // raise_exception diverges; this is unreachable on error

    // 2. Decode: raw u32 → Instruction enum
    let insn = helm_arch_mips::decode(raw)
        .unwrap_or_else(|_err| {
            hart.raise_exception(ExceptionCause::IllegalInstruction);
        });

    // 3. Execute: Instruction × ExecContext → side effects
    if let Err(exc) = helm_arch_mips::execute(insn, hart) {
        hart.raise_exception(exc.into());
    }

    // 4. Advance PC (if not already set by a branch/jump)
    hart.write_next_pc(pc.wrapping_add(4));
}
```

### 4.2 RISC-V C Extension Handling

RISC-V compressed (C) instructions are 16-bit. The RISC-V decode path expands them to 32-bit equivalents before the main decode function:

```rust
fn step_riscv<H: ExecContext>(hart: &mut H, mem: &mut MemoryMap) {
    let pc = hart.read_pc();

    // Fetch first 16 bits to check for C extension (bits [1:0] != 11)
    let half: u16 = mem.read_atomic(pc, 2).unwrap_as_exception(hart);

    let raw: u32 = if (half & 0x3) != 0x3 {
        // C extension: expand 16→32 before main decode
        helm_arch::riscv::decode_rv64c(half).unwrap_or_else(|_| {
            hart.raise_exception(ExceptionCause::IllegalInstruction);
        })
        // PC advances by 2, not 4
    } else {
        // Standard 32-bit instruction: fetch the full word
        mem.read_atomic(pc, 4).unwrap_as_exception(hart)
    };

    // Main decode and execute see only 32-bit instructions
    let insn = helm_arch::riscv::decode_rv64(raw).unwrap_or_else(|_| {
        hart.raise_exception(ExceptionCause::IllegalInstruction);
    });
    if let Err(exc) = helm_arch::riscv::execute(insn, hart) {
        hart.raise_exception(exc.into());
    }
}
```

For other ISAs with fixed-width instructions (MIPS: always 32-bit, AArch64: always 32-bit), the C-expansion step is omitted and the decode function receives the raw word directly.

---

## 5. CSR and System Register Modeling

CSRs and system registers have complex side-effect semantics that are ISA-specific. The design decision (Q3) is to keep CSR storage ISA-specific — there is no shared `CsrFile` abstraction in `helm-core`.

### 5.1 RISC-V CSR File

RISC-V has a 12-bit CSR address space (4096 slots). Implemented as a flat array in `RiscvCsrFile` for O(1) access:

```rust
pub struct RiscvCsrFile {
    regs: [u64; 4096],   // indexed by CSR address
}
```

Most slots are unimplemented. Accessing an unimplemented CSR raises an illegal-instruction exception.

### 5.2 Side Effects After `write_csr()`

CSR writes are handled in the `execute()` function. After calling `ctx.write_csr(csr_addr, new_val)`, the execute arm checks for known side-effect CSRs and triggers the appropriate action:

```rust
// Inside execute() for CSRRW instruction (RISC-V):
fn execute_csrrw<C: ExecContext>(insn: CsrRwInsn, ctx: &mut C) -> Result<(), HartException> {
    let old = ctx.read_csr(insn.csr)?;
    ctx.write_ireg(insn.rd, old);
    ctx.write_csr(insn.csr, ctx.read_ireg(insn.rs1))?;

    // CSR-specific side effects dispatched here, not inside write_csr():
    match insn.csr {
        CSR_SATP => ctx.flush_tlb(),        // address space switch → TLB invalidation
        CSR_MSTATUS => ctx.recompute_mode(), // privilege mode bits changed
        CSR_FCSR | CSR_FRM | CSR_FFLAGS => ctx.sync_fp_mode(), // FP rounding mode
        _ => {}
    }
    Ok(())
}
```

This co-locates the side-effect dispatch with the instruction semantics (design decision Q20). It is explicit, auditable, and testable per CSR.

### 5.3 AArch64 System Registers

AArch64 uses a 5-field encoding (`op0:op1:CRn:CRm:op2`) yielding ~700 defined registers. The `Aarch64SysregFile` uses a sparse `HashMap` or a match-dispatch approach, with per-EL banking handled by the execute logic. The AArch64 ISA crate owns this entirely — helm-core has no knowledge of AArch64 system register structure.

---

## 6. Exception and Interrupt Entry

### 6.1 How `raise_exception` Unwinds the Execute Loop

`raise_exception()` is a diverging function — it never returns normally. The implementation uses `StopReason`, a mechanism that unwinds back to the engine's step loop without using Rust panics:

```rust
// Conceptual; actual mechanism is implementation-defined (thread-local + longjmp-equivalent)
fn raise_exception(&mut self, cause: ExceptionCause) -> ! {
    self.pending_exception = Some(cause);
    // Unwind back to the engine's fetch-decode-execute loop
    // The engine checks pending_exception after each step
    longjmp_equivalent();  // implementation detail of helm-engine
}
```

The engine's outer loop catches the stop:

```rust
// helm-engine inner loop (simplified):
loop {
    match self.step(hart) {
        StopReason::Ok => {}
        StopReason::Exception(cause) => self.handle_exception(hart, cause),
        StopReason::Syscall => self.handle_syscall(hart),
        StopReason::Breakpoint(addr) => { self.event_bus.fire(HelmEvent::Breakpoint { addr }); break; }
        StopReason::SimulationEnd => break,
    }
}
```

### 6.2 Mapping MemFault to Exception

Memory reads and writes return `Result<_, MemFault>`. The instruction handler maps the fault to an ISA-specific exception cause before calling `raise_exception()`:

```rust
// RISC-V load instruction execute arm:
fn execute_load<C: ExecContext>(insn: LoadInsn, ctx: &mut C) -> Result<(), HartException> {
    let addr = ctx.read_ireg(insn.rs1).wrapping_add(insn.imm as u64);
    let val = ctx.read_mem(addr, insn.width)
        .map_err(|fault| match fault {
            MemFault::AccessFault { addr } =>
                HartException::LoadAccessFault { tval: addr },
            MemFault::PageFault { addr } =>
                HartException::LoadPageFault { tval: addr },
            MemFault::Misaligned { addr } =>
                HartException::LoadAddressMisaligned { tval: addr },
        })?;
    ctx.write_ireg(insn.rd, sign_extend(val, insn.width));
    Ok(())
}
```

The `?` propagates `HartException` up to the engine's step function, which calls `raise_exception()`.

---

## 7. Privilege Levels

### 7.1 RISC-V: M / S / U

RISC-V has three privilege levels:

| Level | `PrivilegeLevel` variant | Used in |
|-------|-------------------------|---------|
| Machine | `PrivilegeLevel::Machine` | M-mode, boot, firmware |
| Supervisor | `PrivilegeLevel::Supervisor` | OS kernel |
| User | `PrivilegeLevel::User` | User-space processes |

In **SE mode** (`ExecMode::Syscall`), the hart always runs at `PrivilegeLevel::User`. ECALL raises `ExceptionCause::Ecall` which unwinds to the engine's syscall handler.

In **FE mode** (`ExecMode::Functional`), the hart runs at `PrivilegeLevel::Machine` by default — there is no OS model, and there is no privilege checking. Address translation is disabled (physical address mode).

In **FS mode** (`ExecMode::System`, Phase 3), full privilege is modeled: the kernel boots in M-mode, drops to S-mode, user processes run in U-mode. `mstatus`, `medeleg`, `mideleg`, `sstatus`, `satp` are all live.

### 7.2 AArch64: EL0 / EL1 / EL2 / EL3

AArch64 uses Exception Levels:

| EL | Usage |
|----|-------|
| EL0 | User-space (AArch64 or AArch32 at EL0) |
| EL1 | OS kernel |
| EL2 | Hypervisor |
| EL3 | Secure monitor / firmware |

In SE mode, the hart runs at EL0. In FS mode (Phase 3), EL0 and EL1 are live.

### 7.3 Usage in ExecContext

The `privilege_level()` method is called by the execute loop to determine address translation mode:

```rust
fn translate_address<C: ExecContext>(ctx: &C, va: u64) -> Result<u64, MemFault> {
    match ctx.privilege_level() {
        PrivilegeLevel::Machine => Ok(va),  // physical address mode
        PrivilegeLevel::Supervisor | PrivilegeLevel::User => {
            // Address translation via satp
            page_table_walk(ctx, va)
        }
    }
}
```

---

## 8. SyscallAbi Implementation

In SE mode, the engine's `LinuxSyscallHandler` receives a `&mut dyn ThreadContext` and calls `ctx.syscall_args()` to get the syscall number and arguments, then `ctx.set_syscall_return(val)` to write the result.

The `SyscallArgs` struct is ISA-agnostic:

```rust
pub struct SyscallArgs {
    pub nr: u64,            // syscall number
    pub args: [u64; 6],     // arguments a0–a5 (or equivalent)
}
```

The implementation reads from ISA-specific registers:

```rust
// RISC-V: syscall number in a7 (x17), args in a0–a5 (x10–x15)
impl ThreadContext for RiscvHart {
    fn syscall_args(&self) -> SyscallArgs {
        SyscallArgs {
            nr: self.regs[17],                         // a7
            args: [
                self.regs[10], self.regs[11], self.regs[12],  // a0, a1, a2
                self.regs[13], self.regs[14], self.regs[15],  // a3, a4, a5
            ],
        }
    }

    fn set_syscall_return(&mut self, val: i64) {
        self.regs[10] = val as u64;  // a0
    }
}

// AArch64 AAPCS64: syscall number in x8, args in x0–x5
impl ThreadContext for Aarch64Hart {
    fn syscall_args(&self) -> SyscallArgs {
        SyscallArgs {
            nr: self.regs[8],                                          // x8
            args: [
                self.regs[0], self.regs[1], self.regs[2],            // x0, x1, x2
                self.regs[3], self.regs[4], self.regs[5],            // x3, x4, x5
            ],
        }
    }

    fn set_syscall_return(&mut self, val: i64) {
        self.regs[0] = val as u64;  // x0
    }
}
```

The ECALL/SVC instruction execute arm calls `ctx.raise_exception(ExceptionCause::Ecall)`, which unwinds to the engine. The engine then calls the syscall handler with `hart.as_thread_context()`. The syscall handler never sees `ExecContext` — only `ThreadContext`. This is the dual-access resolution from Q6.

---

## 9. Registering the New ISA

### 9.1 Add an `Isa` Enum Variant

In `helm-core/src/isa.rs`:

```rust
pub enum Isa {
    RiscV,
    AArch64,
    AArch32,
    Mips,       // new variant
}
```

### 9.2 Add a Factory Arm in `build_simulator()`

In `helm-engine/src/build.rs`:

```rust
pub fn build_simulator(config: &SimConfig) -> HelmSim {
    match (config.isa, config.timing) {
        (Isa::RiscV,  TimingKind::Virtual)  => HelmSim::Virtual(HelmEngine::new(RiscvHart::new(config.hart_id), ...)),
        (Isa::RiscV,  TimingKind::Interval) => HelmSim::Interval(HelmEngine::new(RiscvHart::new(config.hart_id), ...)),
        (Isa::Mips,   TimingKind::Virtual)  => HelmSim::Virtual(HelmEngine::new(MipsHart::new(config.hart_id), ...)),
        // ... all (Isa, TimingKind) combinations for the new ISA
        _ => panic!("unsupported (isa, timing) combination: {:?}", (config.isa, config.timing)),
    }
}
```

### 9.3 Add to `HelmSim` Dispatch

`HelmSim` is the PyO3 boundary enum. The `step()` and `run()` methods dispatch on the variant. The new ISA variants are handled by the same arms as existing ones because `HelmEngine<T>` is generic only on the timing model, not the ISA — the ISA is selected by the concrete type of the hart inside `HelmEngine`:

```rust
// HelmSim::step() dispatch — ISA is inside HelmEngine, not in HelmSim variants
pub enum HelmSim {
    Virtual(HelmEngine<Virtual>),
    Interval(HelmEngine<Interval>),
    Accurate(HelmEngine<Accurate>),
    Hardware(HardwareEngine),
}

impl HelmSim {
    pub fn step(&mut self) -> StopReason {
        match self {
            HelmSim::Virtual(e)   => e.step(),   // e contains any ISA
            HelmSim::Interval(e)  => e.step(),
            HelmSim::Accurate(e)  => e.step(),
            HelmSim::Hardware(e)  => e.step(),
        }
    }
}
```

No new `HelmSim` variants are needed for a new ISA — only new factory arms in `build_simulator()` that construct `HelmEngine<T>` with the new hart type.

---

## 10. Testing a New ISA

### 10.1 Unit Tests in the ISA Crate

Test individual instruction execute arms with a mock `ExecContext`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::testing::MockExecContext;

    #[test]
    fn add_basic() {
        let mut ctx = MockExecContext::new();
        ctx.set_ireg(1, 10u64);  // x1 = 10
        ctx.set_ireg(2, 20u64);  // x2 = 20

        // Execute: ADD x3, x1, x2
        let insn = decode_rv64(0x002080B3).expect("decode ADD");
        execute(insn, &mut ctx).expect("execute");

        assert_eq!(ctx.read_ireg(3), 30, "ADD result");
    }

    #[test]
    fn load_word_page_fault_maps_to_correct_cause() {
        let mut ctx = MockExecContext::new_with_fault(
            MemFault::PageFault { addr: 0xDEAD_BEEF }
        );
        // LW x1, 0(x0)
        let insn = decode_rv64(0x0000_2083).expect("decode LW");
        let result = execute(insn, &mut ctx);
        assert!(matches!(result,
            Err(HartException::LoadPageFault { tval }) if tval == 0xDEAD_BEEF
        ));
    }
}
```

`MockExecContext` from `helm-core::testing` provides a configurable in-memory register file and programmable memory fault injection.

### 10.2 Official Test Suites

**RISC-V:** Use `riscv-tests` (the official ISA test suite from the RISC-V Foundation). Each test is a small ELF that writes to a specific MMIO address to signal pass/fail.

```bash
# Run riscv-tests in FE mode using World (no OS needed)
cargo test -p helm-engine --test riscv_tests
```

Test runner pattern:

```rust
fn run_riscv_test(path: &str) -> TestResult {
    let mut world = World::new();
    // Map a "tohost" device at 0x8000_0000 that captures write value
    let tohost = world.add_device("tohost", Box::new(TohostDevice::new()));
    world.map_device(tohost, 0x8000_0000).unwrap();
    world.elaborate().unwrap();

    // Load test ELF into RAM (RAM mapped at 0x8000_1000 for these tests)
    world.load_elf(path).unwrap();

    // Run until tohost device receives a write
    world.run_until_halt(timeout_insns: 100_000).unwrap();

    // riscv-tests: tohost value 1 = pass, any other = fail with code
    let tohost_val = world.device_read(tohost, 0).unwrap();
    if tohost_val == 1 { TestResult::Pass } else { TestResult::Fail(tohost_val) }
}
```

**AArch64:** Use the AArch64 validation suite or run against QEMU for differential testing.

### 10.3 QEMU Differential Testing

For broad ISA coverage, compare helm-ng output against QEMU instruction-by-instruction. The `helm-debug` `TraceLogger` records PC, register state, and memory accesses at each instruction. QEMU's `gdb-xml` trace format or `-d in_asm` can generate a comparable trace.

Differential test framework (outline):

```rust
fn differential_test(binary: &str, n_insns: usize) {
    let helm_trace = run_in_helm(binary, n_insns);
    let qemu_trace = run_in_qemu(binary, n_insns);

    for (step, (h, q)) in helm_trace.iter().zip(qemu_trace.iter()).enumerate() {
        assert_eq!(h.pc, q.pc, "PC mismatch at step {step}");
        for i in 0..32 {
            assert_eq!(h.regs[i], q.regs[i],
                "Register x{i} mismatch at step {step}: helm={:#x} qemu={:#x}",
                h.regs[i], q.regs[i]);
        }
    }
}
```

### 10.4 Running a Minimal Binary in FE Mode

FE mode (Functional, no OS) is the simplest test environment. A hand-crafted binary that uses only base ISA instructions and writes a result to a fixed memory location:

```rust
#[test]
fn minimal_binary_runs_to_completion() {
    // A simple RISC-V binary:
    //   li a0, 42      # load 42 into a0
    //   sw a0, 0(x0)   # store to address 0 (halts via tohost)
    let binary: &[u8] = &[
        0x13, 0x05, 0xa0, 0x02,  // addi a0, x0, 42
        0x23, 0x20, 0xa0, 0x00,  // sw a0, 0(x0)
        0x73, 0x00, 0x50, 0x10,  // wfi (wait for interrupt = halt in FE)
    ];

    let mut world = World::new();
    world.load_binary(binary, 0x8000_0000).unwrap();
    world.elaborate().unwrap();
    world.run_until_halt(timeout_insns: 10_000).unwrap();

    let result = world.mem_read(0x0000_0000, 4).unwrap();
    assert_eq!(result, 42u64);
}
```
