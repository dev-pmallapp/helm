# Sysreg Map

Complete system register table for AArch64 in HELM.

Encoding: `(op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2`

## EL1 Control Registers

| Name | Encoding | R/W | Description |
|------|----------|-----|-------------|
| SCTLR_EL1 | `sysreg(3,0,1,0,0)` | RW | System control |
| ACTLR_EL1 | `sysreg(3,0,1,0,1)` | RW | Auxiliary control |
| CPACR_EL1 | `sysreg(3,0,1,0,2)` | RW | Coprocessor access control |

## Translation Registers

| Name | Encoding | R/W | Description |
|------|----------|-----|-------------|
| TTBR0_EL1 | `sysreg(3,0,2,0,0)` | RW | Translation table base 0 |
| TTBR1_EL1 | `sysreg(3,0,2,0,1)` | RW | Translation table base 1 |
| TCR_EL1 | `sysreg(3,0,2,0,2)` | RW | Translation control |

## Fault Registers

| Name | Encoding | R/W | Description |
|------|----------|-----|-------------|
| ESR_EL1 | `sysreg(3,0,5,2,0)` | RW | Exception syndrome |
| FAR_EL1 | `sysreg(3,0,6,0,0)` | RW | Fault address |
| PAR_EL1 | `sysreg(3,0,7,4,0)` | RW | Physical address (AT result) |
| AFSR0_EL1 | `sysreg(3,0,5,1,0)` | RW | Auxiliary fault status 0 |
| AFSR1_EL1 | `sysreg(3,0,5,1,1)` | RW | Auxiliary fault status 1 |

## Memory Attribute Registers

| Name | Encoding | R/W | Description |
|------|----------|-----|-------------|
| MAIR_EL1 | `sysreg(3,0,10,2,0)` | RW | Memory attribute indirection |
| AMAIR_EL1 | `sysreg(3,0,10,3,0)` | RW | Auxiliary memory attribute |

## Exception / Vector Registers

| Name | Encoding | R/W | Description |
|------|----------|-----|-------------|
| VBAR_EL1 | `sysreg(3,0,12,0,0)` | RW | Vector base address |
| ELR_EL1 | `sysreg(3,0,4,0,1)` | RW | Exception link |
| SPSR_EL1 | `sysreg(3,0,4,0,0)` | RW | Saved program status |
| SP_EL0 | `sysreg(3,0,4,1,0)` | RW | Stack pointer (EL0) |
| SP_EL1 | `sysreg(3,4,4,1,0)` | RW | Stack pointer (EL1) |
| CURRENT_EL | `sysreg(3,0,4,2,2)` | R | Current exception level |
| DAIF | `sysreg(3,3,4,2,1)` | RW | Interrupt mask bits |
| NZCV | `sysreg(3,3,4,2,0)` | RW | Condition flags |
| SPSEL | `sysreg(3,0,4,2,0)` | RW | Stack pointer select |

## Thread ID Registers

| Name | Encoding | R/W | Description |
|------|----------|-----|-------------|
| TPIDR_EL0 | `sysreg(3,3,13,0,2)` | RW | Thread ID (EL0) |
| TPIDR_EL1 | `sysreg(3,0,13,0,4)` | RW | Thread ID (EL1) |
| TPIDRRO_EL0 | `sysreg(3,3,13,0,3)` | R | Thread ID (read-only EL0) |

## Timer Registers

| Name | Encoding | R/W | Description |
|------|----------|-----|-------------|
| CNTFRQ_EL0 | `sysreg(3,3,14,0,0)` | R | Counter frequency |
| CNTVCT_EL0 | `sysreg(3,3,14,0,2)` | R | Virtual counter |
| CNTV_CTL_EL0 | `sysreg(3,3,14,3,1)` | RW | Virtual timer control |
| CNTV_CVAL_EL0 | `sysreg(3,3,14,3,2)` | RW | Virtual timer compare |
| CNTP_CTL_EL0 | `sysreg(3,3,14,2,1)` | RW | Physical timer control |
| CNTP_CVAL_EL0 | `sysreg(3,3,14,2,2)` | RW | Physical timer compare |
| CNTKCTL_EL1 | `sysreg(3,0,14,1,0)` | RW | Kernel timer control |

## ID Registers (Read-Only)

| Name | Default Value | Description |
|------|---------------|-------------|
| MIDR_EL1 | `0x410F_D034` | Main ID (Cortex-A53) |
| MPIDR_EL1 | `0x8000_0000` | Multiprocessor affinity |
| ID_AA64PFR0_EL1 | `0x1100_0000_0000_1111` | Processor feature 0 |
| ID_AA64MMFR0_EL1 | `0x0000_0000_0000_1125` | Memory model feature 0 |
| ID_AA64ISAR0_EL1 | `0x0000_0001_0011_0000` | ISA feature 0 |
| ID_AA64DFR0_EL1 | `0x0000_0000_0000_0006` | Debug feature 0 |
| CTR_EL0 | `0x8444_C004` | Cache type |
| DCZID_EL0 | `0x04` | DC ZVA block size |

## EL2 Registers

See `crates/helm-isa/src/arm/aarch64/sysreg.rs` for the complete
list including HCR_EL2, SCTLR_EL2, VTTBR_EL2, VTCR_EL2, CPTR_EL2,
and all hypervisor timer registers.

## EL3 Registers

SCR_EL3, SCTLR_EL3, TCR_EL3, TTBR0_EL3, MAIR_EL3, ESR_EL3,
FAR_EL3, VBAR_EL3, MDCR_EL3, CPTR_EL3, TPIDR_EL3.
