# TCG Backend — Missing System Instruction Support

## Executive Summary

The TCG backend (`helm-tcg`) decodes and dispatches all AArch64 system
instructions but **implements every one as a no-op or error stub**.
The interpretive FS-mode CPU (`helm-isa` `Aarch64Cpu::step()`) has
full implementations for MRS/MSR to ~100 system registers, cache/TLB
maintenance, PSTATE manipulation, barriers, exception generation, and
timers.  None of this logic is reachable from the TCG path.

This means the TCG backend works only for user-space (SE) workloads
that never touch system registers.  Any FS-mode workload (Linux
kernel, bare-metal firmware) immediately falls back to the slower
interpretive path the moment it executes an MRS, MSR, SYS, WFI, or
exception-generating instruction.

---

## Detailed Gap Analysis

### 1. MRS / MSR (System Register Access)

**Status:** Decoded → handler is a silent no-op (`Ok(())`).

The interpretive path (`exec.rs:read_sysreg` / `write_sysreg`)
supports reads and writes to the full register set below.  The TCG
emitter receives the full encoding fields (`o0, op1, crn, crm, op2,
rt`) but discards them.

| Category | Registers | Count | Impact |
|----------|-----------|-------|--------|
| **EL1 control** | SCTLR, ACTLR, CPACR | 3 | MMU enable, FP/SIMD trapping |
| **Translation** | TTBR0/1, TCR | 3 | Page table base/format — MMU setup |
| **Fault** | ESR, FAR, PAR, AFSR0/1 | 5 | Exception syndrome, fault address |
| **Memory attr** | MAIR, AMAIR | 2 | Memory type attributes |
| **Exception** | VBAR, ELR, SPSR | 3 | Vector table, exception return addr |
| **PSTATE** | NZCV, DAIF, SPSel, CurrentEL | 4 | Flags, interrupt mask, EL |
| **Thread ID** | TPIDR_EL0/EL1, TPIDRRO_EL0 | 3 | TLS pointer (glibc/musl) |
| **Timer** | CNTFRQ, CNTVCT, CNTx_CTL/CVAL/TVAL | 8 | Generic timer |
| **Cache** | CTR, DCZID, CSSELR, CCSIDR, CLIDR | 5 | Cache geometry |
| **ID** | MIDR, MPIDR, ID_AA64PFR0/1, MMFR0-2, ISAR0-2, DFR0/1 | 15+ | Feature discovery |
| **Debug** | MDSCR, MDCCSR, OSLAR/OSLSR | 4 | Debug/trace |
| **FP** | FPCR, FPSR | 2 | FP rounding, exception flags |
| **PMU** | PMCR, PMCNTENSET, PMCCNTR, etc. | 8 | Performance counters |
| **EL2** | HCR, SCTLR_EL2, VTTBR, VTCR, etc. | 20+ | Hypervisor control |
| **EL3** | SCR, SCTLR_EL3, TTBR0_EL3, etc. | 15+ | Secure world |

**What's needed:** The TCG emitter must encode the sysreg ID from
`(o0, op1, crn, crm, op2)` and emit a new `TcgOp` variant (e.g.
`ReadSysReg` / `WriteSysReg`) that the interpreter dispatches to the
same `read_sysreg`/`write_sysreg` logic used by `exec.rs`.

**Priority:** Critical — MRS/MSR are executed thousands of times
during Linux boot (SCTLR, TCR, TTBR, VBAR, DAIF, TPIDR, timer regs).

### 2. MSR (Immediate) — PSTATE Field Writes

**Status:** All 10 variants are silent no-ops.

| Instruction | Target PSTATE field | Impact |
|-------------|-------------------|--------|
| `MSR_i_DAIFSET` | DAIF (set bits) | Mask IRQs — **critical** for Linux |
| `MSR_i_DAIFCLEAR` | DAIF (clear bits) | Unmask IRQs — **critical** |
| `MSR_i_SPSEL` | SPSel | Select SP_EL0 vs SP_EL1 |
| `MSR_i_PAN` | PAN | Privileged Access Never |
| `MSR_i_UAO` | UAO | User Access Override |
| `MSR_i_DIT` | DIT | Data Independent Timing |
| `MSR_i_TCO` | TCO | Tag Check Override (MTE) |
| `MSR_i_SBSS` | SSBS | Speculative Store Bypass |
| `MSR_i_ALLINT` | ALLINT | All interrupt mask |
| `MSR_i_SVCR` | SVCR | Streaming SVE control |

**Priority:** High — DAIFSet/DAIFClear are on every interrupt
entry/exit path.  SPSel is used in early kernel boot.

### 3. SYS / SYSL (System Instructions with Register)

**Status:** Silent no-op for SYS; silent no-op for SYSL.

The interpretive path handles:

| Sub-class | Encoding | Implementation |
|-----------|----------|----------------|
| **DC ZVA** | op1=3, CRn=7, CRm=4, op2=1 | Zeroes a cache-line block via VA→PA translation |
| **DC CIVAC/CVAC/IVAC** | CRn=7, various | NOP (no real cache) |
| **IC IALLU/IALLUIS/IVAU** | CRn=7, various | NOP (flush icache) |
| **TLBI** | CRn=8 | Full TLB invalidation with 12+ sub-variants |
| **AT S1E1R/W, S12E1R/W** | CRn=7, CRm=8 | Address Translation — writes PAR_EL1 |

**Priority:** High — `DC ZVA` is used by Linux `clear_page()` and
glibc `memset`.  TLBI is critical for page table management.  AT is
needed for software page-table walkers.

### 4. Exception Generation

| Instruction | TCG Status | Interpretive Status |
|-------------|-----------|-------------------|
| `SVC` | Emits `Syscall` (SE only — reads X8 as nr) | Full exception with ESR, ELR, SPSR, vector dispatch |
| `HVC` | `Err(Decode)` | Full EL2 exception |
| `SMC` | `Err(Decode)` | Full EL3 exception |
| `BRK` | `ExitTb` (no ESR) | Full BRK exception with ISS |
| `HLT` | `ExitTb` (no ESR) | Full HLT handling |
| `ERET` | `ExitTb` (no restore) | Restores PC←ELR, PSTATE←SPSR, switches EL |

**What's needed:**
- `SVC` in FS mode must set ESR_EL1, save PC to ELR_EL1, save
  PSTATE to SPSR_EL1, and jump to VBAR_EL1 + offset.
- `ERET` must restore PSTATE from SPSR, PC from ELR, and switch
  exception level.
- `HVC`/`SMC` need analogous EL2/EL3 exception entry.

**Priority:** Critical — SVC is every syscall; ERET is every
exception return.  Without these, the kernel cannot handle any trap.

### 5. Barriers

| Instruction | TCG Status | Needed Behavior |
|-------------|-----------|-----------------|
| `DSB` | NOP | Synchronization point (must drain stores before continuing) |
| `DMB` | NOP | Memory ordering (all prior loads/stores visible) |
| `ISB` | NOP | Instruction barrier (flush pipeline, refetch) |
| `SB` | NOP | Speculation barrier |
| `CLREX` | NOP | Clear exclusive monitor |
| `DSB nXS` | NOP | Non-XS DSB variant |

**What's needed for correctness:** In a single-threaded interpreter,
barriers are semantically NOPs.  However:

- `ISB` after writing SCTLR (MMU enable) must flush the
  translation block cache, since subsequent instructions execute in
  a different address-translation regime.
- `DSB` + `ISB` sequences around TLBI must ensure the TLB flush is
  visible before continuing.
- `CLREX` must clear the exclusive monitor state (currently the
  LDXR/STXR pair is simplified to plain load/store).

**Priority:** Medium — correctness matters once MRS/MSR and TLBI
are implemented.  The block cache must be invalidated on ISB.

### 6. Hints

| Instruction | TCG Status | Needed Behavior |
|-------------|-----------|-----------------|
| `NOP` | NOP ✓ | Correct |
| `YIELD` | NOP | Should hint scheduler (useful for spin-loops) |
| `WFE` | NOP | Wait For Event — needed for spin-locks |
| `WFI` | NOP | Wait For Interrupt — **critical** for idle loop |
| `SEV/SEVL` | NOP | Send Event — paired with WFE |

**What's needed:** `WFI` must signal the CPU to halt until an IRQ
is pending (the interpretive path sets `wfi_pending = true`).
Without it, the kernel's idle loop busy-spins instead of yielding.

**Priority:** High for WFI (power/performance); low for others.

### 7. Pointer Authentication (PAuth)

All PAC\* and AUT\* hint instructions are NOPs.  This is acceptable
for simulation (PAC is transparent when keys are zero), but means
authentication failures are never detected.

**Priority:** Low — NOPs are functionally correct.

### 8. PSTATE Flag Manipulation

| Instruction | TCG Status | Needed Behavior |
|-------------|-----------|-----------------|
| `CFINV` | NOP | Invert C flag |
| `XAFLAG` | NOP | Convert AArch32 flags → AArch64 format |
| `AXFLAG` | NOP | Convert AArch64 flags → AArch32 format |

**Priority:** Low — rarely used; CFINV needs NZCV read-modify-write.

### 9. TcgOp IR Gaps

The `TcgOp` enum lacks several operations needed for system
instruction support:

| Missing Op | Purpose |
|-----------|---------|
| `ReadSysReg { dst, sysreg_id }` | MRS — read system register |
| `WriteSysReg { sysreg_id, src }` | MSR — write system register |
| `Exception { class, iss }` | SVC/HVC/SMC/BRK exception entry |
| `Eret` | Exception return (restore PSTATE, switch EL) |
| `Barrier { kind }` | DSB/DMB/ISB (for block-cache invalidation) |
| `Wfi` | Wait for interrupt |
| `DcZva { addr }` | Zero cache-line block |
| `Tlbi { op, addr }` | TLB invalidation |
| `At { op, addr }` | Address translation |

The interpreter (`TcgInterp`) must handle these by calling into the
CPU state struct (the same `Aarch64Cpu` methods used by `exec.rs`).

---

## Recommended Implementation Order

| Phase | Instructions | Unlocks |
|-------|-------------|---------|
| **1** | `MRS`/`MSR` (register) | System register access — minimum for FS-mode TCG |
| **2** | `MSR_i_DAIFSET`/`DAIFCLEAR`/`SPSEL` | Interrupt masking, SP selection |
| **3** | `SVC` (FS-mode), `ERET` | Exception entry/return — kernel can handle traps |
| **4** | `WFI` | Idle loop — stop busy-spinning |
| **5** | `DC ZVA`, `TLBI`, barriers | Page management, cache-line zeroing |
| **6** | `HVC`/`SMC`, `BRK`/`HLT` | Hypervisor, secure world, debug |
| **7** | `AT`, PMU, debug regs | Address translation, performance monitoring |
| **8** | `CFINV`, flag conversion, PAC | Niche instructions |

Phases 1-4 are sufficient for TCG to execute a Linux kernel boot.
Phase 5 is needed for correct page-table management.  Phases 6-8
are for completeness and virtualisation support.
