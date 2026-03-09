# AArch64 CPU

The `Aarch64Cpu` struct in `helm-isa` is the central execution unit.

## Struct Layout

```rust
pub struct Aarch64Cpu {
    pub regs: Aarch64Regs,    // Full architectural state
    pub halted: bool,          // WFI state
    pub exit_code: u64,        // Guest exit code
    pub insn_count: u64,       // Retired instruction counter
    tlb: Tlb,                  // 256-entry TLB
    se_mode: bool,             // SE vs FS behaviour
    mmu_hook: Option<Box<dyn MmuDebugHook>>,
}
```

## Aarch64Regs

Full AArch64 architectural state:

- **GP registers**: X0–X30 (64-bit), SP, PC.
- **Flags**: NZCV packed in bits [31:28].
- **SIMD/FP**: V0–V31 (128-bit), FPCR, FPSR.
- **Thread-local**: TPIDR_EL0.
- **EL state**: `current_el` (0–3), DAIF, `sp_sel`.
- **Per-EL registers**: SP_ELx, ELR_ELx, SPSR_ELx, VBAR_ELx,
  SCTLR_ELx, TCR_ELx, TTBR0/1_ELx, MAIR_ELx, ESR_ELx, FAR_ELx.
- **EL2**: HCR_EL2, VTTBR_EL2, VTCR_EL2, CPTR_EL2, and more.
- **EL3**: SCR_EL3, SCTLR_EL3, etc.
- **ID registers** (read-only): MIDR_EL1 (Cortex-A53), MPIDR_EL1,
  ID_AA64PFR0_EL1, ID_AA64MMFR0_EL1, ID_AA64ISAR0_EL1, etc.
- **Timer**: CNTFRQ_EL0, CNTVCT_EL0, CNTV_CTL_EL0, CNTV_CVAL_EL0,
  CNTP_CTL_EL0, CNTP_CVAL_EL0.

Default values model a Cortex-A53 (MIDR = `0x410F_D034`).

## step() Method

The core execution method for direct-executor mode:

1. **Fetch** — read 4 bytes from `AddressSpace` at PC (with optional
   MMU translation in FS mode).
2. **Decode** — dispatch through op0 bits to instruction group handlers.
3. **Execute** — update registers, perform memory accesses, compute
   flags.
4. **Advance PC** — `pc += 4` unless a branch wrote PC.
5. **Return** `StepTrace` with instruction class, memory accesses, and
   branch outcome.

## step_fast() Method

Optimised version that skips `StepTrace` construction for maximum
throughput. Returns only the instruction class for timing.

## MMU Integration

When `se_mode` is false, memory accesses go through the TLB and MMU:

1. `translate_va(va)` checks the TLB.
2. On miss, `mmu::walk()` performs a page-table walk using `read_phys`.
3. The result fills the TLB and returns the physical address.
4. Translation faults generate data/instruction abort exceptions.

## MmuDebugHook

An optional callback interface for observing:
- Translation faults (before exception delivery).
- TLBI instructions.
- Successful VA→PA translations.
