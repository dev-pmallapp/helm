# AArch64 (ARMv8-A) Implementation Research

**Target**: helm-ng multi-ISA simulator — second ISA after RISC-V
**Scope**: ARMv8-A A-profile (Application profile), EL0/EL1 operation, full SE-mode bring-up
**Date**: 2026-03-13

---

## Table of Contents

1. [AArch64 ISA Overview](#1-aarch64-isa-overview)
2. [Register File](#2-register-file)
3. [Exception Levels](#3-exception-levels)
4. [Instruction Encoding](#4-instruction-encoding)
5. [Memory Model](#5-memory-model)
6. [System Instructions](#6-system-instructions)
7. [Addressing Modes](#7-addressing-modes)
8. [Condition Codes and Branches](#8-condition-codes-and-branches)
9. [SIMD/FP Instructions](#9-simdfp-instructions)
10. [Implementation Strategy in Rust](#10-implementation-strategy-in-rust)
11. [AArch32 / Thumb Interworking](#11-aarch32--thumb-interworking)
12. [Testing AArch64 Implementation](#12-testing-aarch64-implementation)

---

## 1. AArch64 ISA Overview

### Execution State

AArch64 is the 64-bit execution state introduced in ARMv8-A. It coexists with AArch32 (the 32-bit legacy state) within the same chip, but a given PE (Processing Element) runs in one state at a time. The execution state at each Exception Level is determined by hardware configuration registers, not a per-instruction flag.

Key properties of AArch64:

- **64-bit general-purpose registers** (X0–X30), each 64 bits wide
- **Fixed 32-bit instruction width** — every instruction is exactly 4 bytes, naturally aligned
- **PC-relative addressing** throughout (no load from absolute address without ADRP/ADR)
- **Load-acquire / store-release** instructions for lock-free concurrency (LDAXR, STLXR)
- **Separate stack pointer per EL** (SP_EL0, SP_EL1, SP_EL2, SP_EL3)
- **No condition codes on most instructions** — only explicit flag-setting variants (ADDS, SUBS, etc.)
- **No predicated execution** — branch instead; eliminates IT-block complexity of AArch32

### A-Profile Focus

ARMv8-A is the Application profile — designed for full-featured OS execution with a virtual memory system (MMU), multiple privilege levels (EL0–EL3), and a complete memory model. This is what Linux, macOS, and Windows on Arm use. The other profiles (R = realtime, M = microcontroller) have different register files, no MMU, and are out of scope.

### Key Differences from AArch32

| Feature | AArch32 | AArch64 |
|---|---|---|
| Instruction width | 32-bit ARM + 16/32-bit Thumb (variable) | Fixed 32-bit only |
| GPR count | 16 (R0–R15, PC is R15) | 31 + ZR (PC not a GPR) |
| PC as operand | Yes (R15) | No — PC-relative ops only |
| Condition codes | On almost all instructions | Only explicit flag-set variants |
| IT blocks | Yes (predicated execution) | No |
| SIMD registers | D0–D15 (64-bit), Q0–Q15 (128-bit) | V0–V31 (128-bit) + aliases |
| System registers | CP15 coprocessor via MRC/MCR | Direct MRS/MSR encoding |
| Exception model | Modes (USR/SVC/ABT/FIQ/IRQ/UND/SYS) | Exception Levels (EL0–EL3) |
| SPSR | Banked per mode | One per EL: SPSR_EL1/EL2/EL3 |

### Instruction Groups

The ARMv8-A Architecture Reference Manual (ARM DDI 0487) organizes instructions into top-level groups decoded from bits [28:25] of each 32-bit word:

| Group | Description |
|---|---|
| Data Processing — Immediate | ADD/SUB/AND/OR/XOR with immediate operand |
| Data Processing — Register | Two-source or three-source register ops |
| Data Processing — SIMD/FP | NEON and floating-point instructions |
| Loads and Stores | LDR/STR, LDP/STP, load-acquire/store-release |
| Branches, Exception Generating, System | B, BL, CBZ, SVC, MRS/MSR, barriers |

---

## 2. Register File

### General-Purpose Registers

AArch64 has 31 general-purpose registers numbered 0–30, accessible as 64-bit (Xn) or 32-bit (Wn):

| Name | Alias | Width | Notes |
|---|---|---|---|
| X0–X7 | W0–W7 | 64/32-bit | Argument / result registers (ABI) |
| X8 | W8 | 64/32-bit | Indirect result location / syscall number |
| X9–X15 | W9–W15 | 64/32-bit | Caller-saved temporaries |
| X16–X17 | IP0, IP1 | 64/32-bit | Intra-procedure call scratch (linker veneers) |
| X18 | — | 64/32-bit | Platform register (OS-specific use) |
| X19–X28 | W19–W28 | 64/32-bit | Callee-saved |
| X29 | FP | 64/32-bit | Frame pointer |
| X30 | LR | 64/32-bit | Link register (return address) |

**W register semantics**: writing to a Wn register zero-extends into the upper 32 bits of Xn — it does **not** preserve the upper bits. This is a critical correctness point (see §12).

### Zero Register (XZR / WZR)

Register encoding `0b11111` (31 decimal) acts as a zero register in most contexts:

- **Read**: always returns 0
- **Write**: discards the result (acts as `/dev/null`)
- **Exception**: in addressing contexts, encoding 31 refers to SP, not XZR

This context-sensitivity (SP vs ZR depending on instruction field) must be handled in the decoder.

### Special Registers

| Register | Notes |
|---|---|
| SP | Stack pointer. In AArch64, encoding 31 in base-register fields = SP, not XZR. Current SP depends on SPSel (see §3). |
| PC | Program counter. Not a GPR. Cannot be read/written by data instructions. Only modified by branches, exception entry/return. |
| LR (X30) | Link register — conventional, not architectural constraint. BL/BLR write to X30. |

### PSTATE

PSTATE is the current processor state. It is not a single register but a collection of fields accessible via dedicated instructions or through the `NZCV`, `DAIF`, `CurrentEL`, `SPSel` system registers when in AArch64 state.

| Field | Bits | Description |
|---|---|---|
| N | PSTATE.N | Negative flag — set when result MSB is 1 |
| Z | PSTATE.Z | Zero flag — set when result is zero |
| C | PSTATE.C | Carry flag — set on unsigned overflow or borrow |
| V | PSTATE.V | Overflow flag — set on signed overflow |
| SP | PSTATE.SP | Stack pointer select: 0 = use SP_EL0, 1 = use SP_ELn |
| EL | PSTATE.EL | Current Exception Level (0–3) |
| nRW | PSTATE.nRW | Execution state: 0 = AArch64, 1 = AArch32 |
| DAIF | PSTATE.DAIF | Interrupt/Abort mask bits: D(ebug), A(serror), I(IRQ), F(FIQ) |
| SS | PSTATE.SS | Software step |
| IL | PSTATE.IL | Illegal execution state |
| PAN | PSTATE.PAN | Privileged Access Never (ARMv8.1) |
| UAO | PSTATE.UAO | User Access Override (ARMv8.2) |
| TCO | PSTATE.TCO | Tag Check Override (ARMv8.5-MTE) |

NZCV flags are the most commonly manipulated. The `MSR NZCV, Xn` / `MRS Xn, NZCV` instructions transfer the NZCV fields as bits [31:28] of a 64-bit register.

### SIMD / FP Registers

AArch64 has 32 SIMD+FP registers, each 128 bits wide:

| Alias | Width | Description |
|---|---|---|
| Vn | 128-bit | Full SIMD register (NEON operations) |
| Qn | 128-bit | Alias for the full 128-bit view |
| Dn | 64-bit | Lower 64 bits of Vn |
| Sn | 32-bit | Lower 32 bits of Vn |
| Hn | 16-bit | Lower 16 bits of Vn (FP16, ARMv8.2+) |
| Bn | 8-bit | Lower 8 bits of Vn |

Writing to a narrower alias (Sn, Dn) zero-extends the upper bits of Vn — same rule as Wn/Xn.

### Key System Registers

System registers are 64-bit and accessed via `MRS`/`MSR`. The register is identified by a 5-field encoding: `op0:op1:CRn:CRm:op2`.

| Register | Purpose |
|---|---|
| SCTLR_EL1 | System Control (MMU enable, cache enable, alignment check) |
| TCR_EL1 | Translation Control (page size, VA range, TG0/TG1) |
| TTBR0_EL1 | Translation Table Base Register 0 (user: VA bit55=0) |
| TTBR1_EL1 | Translation Table Base Register 1 (kernel: VA bit55=1) |
| MAIR_EL1 | Memory Attribute Indirection Register (8 × 8-bit attribute slots) |
| ESR_EL1 | Exception Syndrome Register (fault cause after exception) |
| FAR_EL1 | Fault Address Register (VA that caused the fault) |
| ELR_EL1 | Exception Link Register (return address on exception entry) |
| SPSR_EL1 | Saved Program State Register (PSTATE at time of exception) |
| VBAR_EL1 | Vector Base Address Register (exception vector table base) |
| SP_EL0 | Stack pointer for EL0 |
| SP_EL1 | Stack pointer for EL1 |
| CPACR_EL1 | Architectural Feature Access Control (SIMD/FP enable at EL0/EL1) |
| TPIDR_EL0 | Thread pointer — user-mode thread-local storage |
| TPIDR_EL1 | Thread pointer — EL1 (OS per-thread data) |
| CurrentEL | Read-only: current EL in bits [3:2] |
| NZCV | PSTATE condition flags |
| DAIF | PSTATE interrupt mask |
| SPSel | PSTATE stack pointer select |

---

## 3. Exception Levels

### Privilege Hierarchy

```
EL3 — Secure Monitor (TrustZone / secure boot)
EL2 — Hypervisor (virtualization)
EL1 — OS Kernel
EL0 — User application
```

For SE (Syscall Emulation) mode in helm-ng, only EL0 and EL1 are needed. EL2 and EL3 are out of scope unless full-system simulation is added.

### SPSR — Saved Program State Register

On exception entry to ELn, the hardware automatically saves PSTATE to `SPSR_ELn`. On `ERET`, PSTATE is restored from `SPSR_ELn`. Each EL has its own SPSR:

- `SPSR_EL1` — saved on entry from EL0 (SVC) or EL1 (unexpected exceptions)
- `SPSR_EL2` — saved on entry from EL0/EL1/EL2 (HVC, traps to hypervisor)
- `SPSR_EL3` — saved on entry from any EL (SMC, traps to secure monitor)

SPSR format (AArch64 exception):

```
[63:32] — reserved/RAZ
[31]    — N flag
[30]    — Z flag
[29]    — C flag
[28]    — V flag
[27:26] — reserved
[25]    — SSBS (ARMv8.5)
[24]    — DIT (ARMv8.4)
[23:22] — reserved
[21]    — SS (software step active)
[20]    — IL (illegal execution state)
[19:16] — reserved
[15:10] — DAIF bits (D, A, I, F in bits 9:6)
[9]     — D (debug mask)
[8]     — A (SError mask)
[7]     — I (IRQ mask)
[6]     — F (FIQ mask)
[5]     — reserved
[4]     — M[4]: 0 = AArch64, 1 = AArch32
[3:2]   — M[3:2]: target EL
[1]     — M[1]: reserved (0)
[0]     — M[0]: SP select (0 = SP_EL0, 1 = SP_ELn)
```

### ELR — Exception Link Register

On exception entry to ELn, the hardware saves the preferred return address to `ELR_ELn`:

- For synchronous exceptions (SVC, data abort): ELR = address of the faulting instruction (or next instruction for SVC)
- For IRQ/FIQ: ELR = address of the interrupted instruction
- `ERET` loads PC from `ELR_ELn` and PSTATE from `SPSR_ELn`

### Stack Pointer Selection (SPSel)

`PSTATE.SP` (accessible as `SPSel` system register) controls which SP is active:

- `SPSel = 0`: use `SP_EL0` at all ELs (unusual; mainly used in EL1 kernel code for simplicity)
- `SPSel = 1`: use `SP_ELn` where n = current EL

Linux uses `SPSel = 1` (SP_EL1 in kernel, SP_EL0 in user). At EL1 entry, Linux switches to SP_EL1.

### Exception Vectors (VBAR_EL1)

`VBAR_EL1` holds the base address of the vector table. The table is 2KB (0x800 bytes) aligned and has 16 entries organized as 4 groups × 4 exception types:

```
Offset  Group               Exception Types
------  ------------------  --------------------------------------------------------
+0x000  Current EL, SP_EL0  Synchronous
+0x080  Current EL, SP_EL0  IRQ / vIRQ
+0x100  Current EL, SP_EL0  FIQ / vFIQ
+0x180  Current EL, SP_EL0  SError / vSError

+0x200  Current EL, SP_ELx  Synchronous
+0x280  Current EL, SP_ELx  IRQ / vIRQ
+0x300  Current EL, SP_ELx  FIQ / vFIQ
+0x380  Current EL, SP_ELx  SError / vSError

+0x400  Lower EL, AArch64   Synchronous   ← SVC from EL0 lands here
+0x480  Lower EL, AArch64   IRQ / vIRQ
+0x500  Lower EL, AArch64   FIQ / vFIQ
+0x580  Lower EL, AArch64   SError / vSError

+0x600  Lower EL, AArch32   Synchronous
+0x680  Lower EL, AArch32   IRQ / vIRQ
+0x700  Lower EL, AArch32   FIQ / vFIQ
+0x780  Lower EL, AArch32   SError / vSError
```

Each slot is 128 bytes (0x80) — enough for 32 instructions before branching to a handler.

### Exception Types

| Type | Trigger |
|---|---|
| Synchronous | SVC, HVC, SMC, instruction/data abort, alignment fault, undefined instruction, breakpoint/watchpoint |
| IRQ | External interrupt request (level-triggered) |
| FIQ | Fast interrupt request (level-triggered, higher priority than IRQ) |
| SError | System Error — asynchronous bus error, correctable ECC, implementation-defined |

For SE mode, only synchronous exceptions (SVC for syscalls, data/instruction aborts for page faults) are needed.

---

## 4. Instruction Encoding

### Fixed 32-bit Width

Every AArch64 instruction is exactly 32 bits, naturally aligned. There is no Thumb mode in AArch64 (Thumb is AArch32-only). This simplifies decode dramatically: fetch 4 bytes, always a valid instruction boundary.

### Top-Level Decode: bits [28:25]

The ARMv8-A manual defines the top-level decode tree from bits [28:25] (the `op0` field):

```
Bits [28:25]   Group
─────────────  ──────────────────────────────────────
0b0000         Unallocated / reserved
0b0001         Unallocated / reserved
0b0010         SVE encodings (ARMv8.2+)
0b0011         Unallocated
0b1000         Data Processing — Immediate
0b1001         Data Processing — Immediate
0b1010         Branches, Exception Generating, System
0b1011         Branches, Exception Generating, System
0b0100         Loads and Stores
0b0110         Loads and Stores
0b1100         Loads and Stores
0b1110         Loads and Stores
0b0101         Data Processing — Register
0b1101         Data Processing — Register
0b0111         Data Processing — SIMD/FP
0b1111         Data Processing — SIMD/FP
```

Note: The match is on a 4-bit field (bits 28:25), but some groups share multiple values (e.g., loads/stores cover 0100, 0110, 1100, 1110). The ARMv8-A spec further disambiguates using bits [31:29] within each group.

### Encoding Bit Diagrams

All diagrams are MSB-first (bit 31 on the left). Bit ranges shown as `[high:low]`.

#### ADD / SUB (Immediate) — `ADD Xd, Xn, #imm`

```
31   30  29  28    23   22  21        10  9     5  4     0
┌──┬───┬───┬──────────┬───┬────────────┬────────┬────────┐
│sf│ op│ S │1 0 0 0 1 0│sh │   imm12    │   Rn   │   Rd   │
└──┴───┴───┴──────────┴───┴────────────┴────────┴────────┘
  1   1   0              0

sf=1 → 64-bit (Xn), sf=0 → 32-bit (Wn)
op=0 → ADD, op=1 → SUB
S=1  → set flags (ADDS/SUBS)
sh=0 → imm12 unshifted, sh=1 → imm12 << 12
```

#### LDR (Register Offset) — `LDR Xt, [Xn, Xm, LSL #3]`

```
31  30  29  28    21  20    16  15  13  12  11  10  9    5  4    0
┌────┬───────────┬──────────┬───────┬───┬───┬───────┬───────┐
│size│1 1 1 0 0 0│    Rm    │option │ S │1 0│  Rn   │  Rt   │
└────┴───────────┴──────────┴───────┴───┴───┴───────┴───────┘
  11                          010     1

size=11 → 64-bit (X)
option=010 → UXTW, 011 → LSL, 110 → SXTW, 111 → SXTX
S=1 → shift by register size in bytes (e.g., 3 for 64-bit)
```

#### CBZ — Compare and Branch if Zero — `CBZ Xt, label`

```
31  30   25  24  23                              5  4    0
┌──┬───────┬───┬──────────────────────────────┬────────┐
│sf│0 1 1 0 1│ op│            imm19            │   Rt   │
└──┴───────┴───┴──────────────────────────────┴────────┘
  1          0

sf=1 → 64-bit compare, sf=0 → 32-bit compare
op=0 → CBZ (branch if Rt == 0)
op=1 → CBNZ (branch if Rt != 0)
Branch target = PC + SignExtend(imm19 << 2, 64)
Range: ±1MB
```

#### B — Unconditional Branch — `B label`

```
31  30   26  25                                          0
┌──┬───────┬──────────────────────────────────────────────┐
│op│0 0 1 0 1│                   imm26                     │
└──┴───────┴──────────────────────────────────────────────┘
  0

op=0 → B (branch, no link)
op=1 → BL (branch with link: X30 = PC+4)
Branch target = PC + SignExtend(imm26 << 2, 64)
Range: ±128MB
```

#### BL — Branch with Link — `BL label`

Same encoding as B with op=1:

```
31  30   26  25                                          0
┌──┬───────┬──────────────────────────────────────────────┐
│ 1│0 0 1 0 1│                   imm26                     │
└──┴───────┴──────────────────────────────────────────────┘
```

#### SVC — Supervisor Call — `SVC #imm16`

```
31                         21  20                 5  4  0
┌──────────────────────────┬────────────────────┬──────┐
│1 1 0 1 0 1 0 0 0 0 0     │       imm16         │0 0 0 0 1│
└──────────────────────────┴────────────────────┴──────┘

Fixed: [31:21] = 11010100000, [4:0] = 00001
imm16: 16-bit immediate (convention: Linux uses 0 for all SVCs)
Exception class in ESR_EL1.EC = 0b010101 (SVC from AArch64)
imm16 accessible as ESR_EL1.ISS[15:0]
```

---

## 5. Memory Model

### Virtual Address Space

AArch64 supports two simultaneous VA ranges, split by the value of bit 55 of the virtual address:

- **TTBR0_EL1**: VA[55] = 0 → user space (low addresses, e.g., 0x0000_xxxx_xxxx_xxxx)
- **TTBR1_EL1**: VA[55] = 1 → kernel space (high addresses, e.g., 0xFFFF_xxxx_xxxx_xxxx)

The split is controlled by `TCR_EL1.T0SZ` and `TCR_EL1.T1SZ`, which determine the width of each half. A typical 48-bit VA (T0SZ = T1SZ = 16) gives each half a 256TB range.

### Page Sizes

| Page Size | TG0/TG1 | Levels | VA bits used |
|---|---|---|---|
| 4KB | 00 | 4 (L0–L3) | [47:0] |
| 16KB | 10 | 4 (L0–L3) | [47:0] |
| 64KB | 01 | 3 (L1–L3) | [47:0] |

Linux on AArch64 almost universally uses 4KB pages with 4-level page tables.

### Page Table Walk (4KB, 48-bit VA)

```
VA [47:39] → L0 index (512 entries)
VA [38:30] → L1 index (512 entries)
VA [29:21] → L2 index (512 entries)
VA [20:12] → L3 index (512 entries)
VA [11:0]  → Page offset (4KB)
```

Each table entry is 8 bytes. Tables are 4KB (512 × 8 bytes).

### Page Table Descriptor Format

Bits [1:0] determine the descriptor type:

```
[1:0] = 00 → Invalid (fault)
[1:0] = 01 → Block descriptor (L1 maps 1GB, L2 maps 2MB)
[1:0] = 11 → Table descriptor (L0–L2) or Page descriptor (L3)
```

Block / Page descriptor layout:

```
Bits    Field
[63]    NSE / PBHA[3] (optional)
[62:59] PBHA (page-based hardware attributes)
[58:55] Reserved
[54]    UXN (unprivileged execute never)
[53]    PXN (privileged execute never)
[52]    Contiguous hint
[51:48] Reserved (upper attributes)
[47:12] Output address (OA) — physical page/block address
[11:10] Reserved
[9:8]   Shareability: 00=non-shareable, 10=outer, 11=inner
[7:6]   AP[2:1]: access permissions
        01 = EL1 R/W, EL0 no access
        11 = EL1 R/W, EL0 R/W
[5]     NS (non-secure)
[4:2]   AttrIdx[2:0] — index into MAIR_EL1 (0–7)
[1:0]   Descriptor type (11 = page/table)
```

### Memory Attributes (MAIR_EL1)

`MAIR_EL1` is 8 bytes, each byte defining one memory attribute slot (AttrIdx 0–7):

| AttrIdx | Typical configuration | Usage |
|---|---|---|
| 0 | `0b00000000` (Device-nGnRnE) | Strongly ordered device memory |
| 1 | `0b00000100` (Device-nGnRE) | Device memory with early write ack |
| 2 | `0b11111111` (Normal, WB, RA, WA) | Normal cacheable memory |
| 3 | `0b01000100` (Normal, NC) | Normal non-cacheable |

The Linux kernel configures MAIR_EL1 at boot and assigns indexes for device, normal-NC, and normal-cacheable.

### TLB Invalidation

| Instruction | Operation |
|---|---|
| `TLBI VMALLE1` | Invalidate all TLB entries for EL1 (both TTBR0 and TTBR1 ranges) |
| `TLBI VAE1, Xt` | Invalidate TLB entry by VA for EL1 (Xt[55:12] = VA[55:12]) |
| `TLBI ASIDE1, Xt` | Invalidate by ASID (Xt[63:48]) |
| `TLBI VAAE1, Xt` | Invalidate by VA, all ASIDs |

After `TLBI`, a `DSB ISH` is required before the invalidation takes effect for other PEs in the shareability domain.

### Memory Barrier Instructions

| Instruction | Name | Description |
|---|---|---|
| `DSB SY` | Data Synchronization Barrier | All memory accesses before DSB complete before any after |
| `DSB ISH` | DSB Inner Shareable | Same, scoped to inner shareable domain |
| `DSB NSH` | DSB Non-Shareable | Same, scoped to this PE only |
| `DMB ISH` | Data Memory Barrier | Orders memory accesses but does not wait for completion |
| `ISB` | Instruction Synchronization Barrier | Flushes pipeline; context synchronization event |

`ISB` is required after writing system registers that affect instruction fetch (e.g., SCTLR_EL1 MMU enable, VBAR_EL1).

---

## 6. System Instructions

### MRS / MSR — System Register Access

```
MRS Xt, <sysreg>   ; Move from system register to Xt
MSR <sysreg>, Xt   ; Move from Xt to system register
```

System registers are encoded by a 5-tuple: `op0:op1:CRn:CRm:op2`. For example:

- `MRS X0, SCTLR_EL1` encodes as: op0=3, op1=0, CRn=1, CRm=0, op2=0
- `MRS X0, ESR_EL1` encodes as: op0=3, op1=0, CRn=5, CRm=2, op2=0

The assembler maps mnemonics to these 5-tuples. The simulator must maintain a hash map of `(op0, op1, CRn, CRm, op2) → register_id`.

### Key System Registers for EL1 Operation

**SCTLR_EL1** — System Control Register:

| Bit | Name | Description |
|---|---|---|
| [0] | M | MMU enable |
| [1] | A | Alignment fault enable |
| [2] | C | Data cache enable |
| [12] | I | Instruction cache enable |
| [23] | SPAN | Set PAN on exception return |
| [26] | UCI | Trap cache instruction at EL0 |
| [28] | nTLSMD | No trap load/store multiple to device |

**TCR_EL1** — Translation Control Register:

| Bits | Name | Description |
|---|---|---|
| [5:0] | T0SZ | Size offset for TTBR0 region (VA = 64 - T0SZ bits) |
| [7] | EPD0 | Disable table walk for TTBR0 |
| [9:8] | IRGN0 | Inner cacheability for TTBR0 walks |
| [11:10] | ORGN0 | Outer cacheability for TTBR0 walks |
| [13:12] | SH0 | Shareability for TTBR0 walks |
| [15:14] | TG0 | Granule size for TTBR0 (00=4KB, 10=16KB, 01=64KB) |
| [21:16] | T1SZ | Size offset for TTBR1 region |
| [22] | A1 | ASID select (0=TTBR0.ASID, 1=TTBR1.ASID) |
| [23] | EPD1 | Disable table walk for TTBR1 |
| [31:30] | TG1 | Granule size for TTBR1 (10=4KB, 01=16KB, 11=64KB) |
| [35:32] | IPS | Intermediate Physical Address size |
| [36] | AS | ASID size (0=8-bit, 1=16-bit) |
| [37] | TBI0 | Top Byte Ignore for TTBR0 |
| [38] | TBI1 | Top Byte Ignore for TTBR1 |

**ESR_EL1** — Exception Syndrome Register:

| Bits | Name | Description |
|---|---|---|
| [31:26] | EC | Exception Class — reason for exception |
| [25] | IL | Instruction Length (0=16-bit Thumb, 1=32-bit) |
| [24:0] | ISS | Instruction Specific Syndrome |

Key EC values:

| EC | Hex | Description |
|---|---|---|
| 000000 | 0x00 | Unknown reason |
| 000001 | 0x01 | WFI/WFE trap |
| 010101 | 0x15 | SVC (AArch64) |
| 100000 | 0x20 | Instruction abort from lower EL |
| 100001 | 0x21 | Instruction abort from same EL |
| 100100 | 0x24 | Data abort from lower EL |
| 100101 | 0x25 | Data abort from same EL |
| 101100 | 0x2C | SP alignment fault |
| 110000 | 0x30 | FP exception (AArch32) |
| 110100 | 0x34 | Illegal execution state |

For data aborts (EC=0x24/0x25), the ISS encodes:

| Bits | Field | Description |
|---|---|---|
| [5:0] | DFSC | Data Fault Status Code (translation fault L0–L3, access flag, permission, etc.) |
| [6] | WnR | 0=read, 1=write |
| [7] | S1PTW | Stage 1 page table walk fault |
| [8] | CM | Cache maintenance fault |
| [9] | EA | External abort type |
| [10] | FnV | FAR not valid |
| [23:14] | SAS+SSE+SRT+SF+AR | Syndrome access size, sign extension, register, etc. |

### SVC — Supervisor Call

```asm
SVC #0          ; Linux syscall convention — immediate is always 0
                ; syscall number is in X8
                ; args: X0–X5
                ; return value: X0
```

On `SVC #imm16` from EL0:
1. Hardware saves PSTATE to `SPSR_EL1`, PC+4 to `ELR_EL1`
2. PSTATE.EL set to 1, PSTATE.SP set to 1 (switch to SP_EL1)
3. PC set to `VBAR_EL1 + 0x400` (lower EL AArch64 synchronous vector)
4. `ESR_EL1.EC = 0x15`, `ESR_EL1.ISS = imm16`

### ERET — Exception Return

```asm
ERET            ; Restore PSTATE from SPSR_ELn, PC from ELR_ELn
```

1. PSTATE ← `SPSR_EL1` (restores flags, EL, SP select, DAIF)
2. PC ← `ELR_EL1`
3. Execution continues at restored EL with restored state

`ERET` is undefined at EL0 — it generates an Undefined Instruction exception.

### HVC and SMC

```asm
HVC #imm16      ; Hypervisor Call — EL1 → EL2, or trapped at EL2
SMC #imm16      ; Secure Monitor Call — EL1/EL2 → EL3
```

For SE mode: these can be `UNDEF`-trapped or ignored. A guest OS should not issue HVC/SMC in SE mode.

### WFI / WFE

```asm
WFI             ; Wait for Interrupt — halt until interrupt pending
WFE             ; Wait for Event — halt until event register set
```

In SE mode: implement as a yield hint or a no-op. Do not block the simulator.

### Cache Maintenance

```asm
DC CIVAC, Xt   ; Clean and Invalidate data cache by VA to PoC
DC CVAU, Xt   ; Clean data cache by VA to PoU (for I-cache coherency)
IC IVAU, Xt   ; Invalidate instruction cache by VA to PoU
IC IALLU       ; Invalidate all instruction caches to PoU (EL1)
```

In SE mode: these are typically no-ops (coherent unified cache model).

---

## 7. Addressing Modes

All load/store addressing modes in AArch64 are encoded in the instruction. There is no separate addressing mode specifier.

### Base Register Only

```asm
LDR X0, [X1]           ; Load from address in X1
```

Encoded as base + immediate offset with imm = 0.

### Base + Immediate Offset (unsigned)

```asm
LDR X0, [X1, #8]       ; Load from X1 + 8
STR X0, [X1, #16]      ; Store to X1 + 16
```

The immediate is scaled by the access size:
- LDR X (8-byte): imm12 encodes byte offset / 8 (range: 0 to 32760)
- LDR W (4-byte): imm12 encodes byte offset / 4 (range: 0 to 16380)
- LDR H (2-byte): imm12 encodes byte offset / 2
- LDR B (1-byte): imm12 encodes byte offset directly

### Base + Register Offset (extended register)

```asm
LDR X0, [X1, X2]               ; Load from X1 + X2
LDR X0, [X1, X2, LSL #3]       ; Load from X1 + (X2 << 3)
LDR X0, [X1, W2, UXTW #3]      ; Load from X1 + ZeroExtend(W2) << 3
LDR X0, [X1, W2, SXTW #3]      ; Load from X1 + SignExtend(W2) << 3
```

The `option` field selects extension type; `S` bit enables the shift.

### Pre-indexed

```asm
LDR X0, [X1, #8]!      ; X1 = X1 + 8, then load from new X1
```

The base register is updated before the memory access. The writeback exclamation mark is encoded in the instruction (`pre-index` form: imm9 + writeback bit).

### Post-indexed

```asm
LDR X0, [X1], #8       ; Load from X1, then X1 = X1 + 8
```

The base register is updated after the memory access. Useful for sequential memory traversal.

### PC-Relative

```asm
ADR  X0, label         ; X0 = PC + SignExtend(imm21)  — ±1MB
ADRP X0, label         ; X0 = (PC & ~0xFFF) + imm21<<12 — ±4GB, page-aligned
```

`ADRP` computes the page address of `label` relative to the current instruction's page. Typically followed by:

```asm
ADRP X0, symbol
ADD  X0, X0, :lo12:symbol    ; or LDR/STR with :lo12: offset
```

The linker fills in `:lo12:` as the bottom 12 bits of the symbol address.

### Load Pair

```asm
LDP X0, X1, [X2]           ; X0 = Mem[X2], X1 = Mem[X2+8]
LDP X0, X1, [X2, #16]      ; X0 = Mem[X2+16], X1 = Mem[X2+24]
LDP X0, X1, [X2, #16]!     ; X2 += 16, then load pair
LDP X0, X1, [X2], #16      ; load pair, then X2 += 16
STP X0, X1, [X2, #-16]!    ; Push pair (X2 -= 16, store X0, X1)
```

`LDP`/`STP` are the canonical push/pop idioms. The imm7 field is scaled by pair size.

---

## 8. Condition Codes and Branches

### NZCV Flags

| Flag | Bit | Set when |
|---|---|---|
| N | [31] of NZCV | Result MSB is 1 (negative for signed) |
| Z | [30] of NZCV | Result is zero |
| C | [29] of NZCV | Unsigned overflow (carry out); for SUB: borrow is inverted C |
| V | [28] of NZCV | Signed overflow |

**Carry for subtraction**: AArch64 uses "carry = NOT borrow" convention. `SUBS X0, X1, X2` sets C=1 if X1 >= X2 (no borrow), C=0 if X1 < X2 (borrow). This is the opposite of x86's CF behavior.

### Condition Codes

| Mnemonic | Meaning | Flags |
|---|---|---|
| EQ | Equal | Z=1 |
| NE | Not equal | Z=0 |
| CS / HS | Carry set / unsigned higher or same | C=1 |
| CC / LO | Carry clear / unsigned lower | C=0 |
| MI | Minus (negative) | N=1 |
| PL | Plus (non-negative) | N=0 |
| VS | Overflow set | V=1 |
| VC | Overflow clear | V=0 |
| HI | Unsigned higher | C=1 AND Z=0 |
| LS | Unsigned lower or same | C=0 OR Z=1 |
| GE | Signed greater or equal | N=V |
| LT | Signed less than | N!=V |
| GT | Signed greater than | Z=0 AND N=V |
| LE | Signed less than or equal | Z=1 OR N!=V |
| AL | Always | (unconditional) |
| NV | Never (reserved, behaves as AL) | — |

### Conditional Branch

```asm
B.EQ label     ; Branch if Z=1 (imm19, ±1MB)
B.NE label     ; Branch if Z=0
B.LT label     ; Branch if N!=V (signed less than)
B.GE label     ; Branch if N=V
```

Encoding: `[31:24] = 0b01010100`, `[23:5] = imm19`, `[4] = 0`, `[3:0] = cond`

### CBZ / CBNZ — Compare and Branch

```asm
CBZ  X0, label   ; Branch to label if X0 == 0 (imm19, ±1MB)
CBNZ X0, label   ; Branch to label if X0 != 0
```

No flags are set. Range is ±1MB (imm19 × 4).

### TBZ / TBNZ — Test Bit and Branch

```asm
TBZ  X0, #3, label   ; Branch if bit 3 of X0 is 0 (imm14, ±32KB)
TBNZ X0, #3, label   ; Branch if bit 3 of X0 is 1
```

`b5:b40` encodes the bit number (0–63). Useful for testing flag bits without setting NZCV.

### Conditional Select Instructions

```asm
CSEL  X0, X1, X2, EQ   ; X0 = (Z==1) ? X1 : X2
CSET  X0, EQ            ; X0 = (Z==1) ? 1 : 0
CSINC X0, X1, X2, NE   ; X0 = (Z==0) ? X1 : X2+1
CSINV X0, X1, X2, GT   ; X0 = (GT) ? X1 : ~X2
CSNEG X0, X1, X2, GE   ; X0 = (GE) ? X1 : -X2
```

These implement branchless conditional moves. `CSET` and `CSINC` with `XZR` are the idiom for boolean-to-integer conversion.

---

## 9. SIMD/FP Instructions

### Scalar Floating-Point

All scalar FP instructions use the `Sn` (32-bit) or `Dn` (64-bit) register aliases.

#### Basic Arithmetic

```asm
FMOV S0, S1           ; S0 = S1 (register move)
FMOV S0, #1.0         ; S0 = 1.0 (8-bit encoded immediate)
FADD D0, D1, D2       ; D0 = D1 + D2
FSUB D0, D1, D2       ; D0 = D1 - D2
FMUL D0, D1, D2       ; D0 = D1 * D2
FDIV D0, D1, D2       ; D0 = D1 / D2
FNEG D0, D1           ; D0 = -D1
FABS D0, D1           ; D0 = |D1|
FSQRT D0, D1          ; D0 = sqrt(D1)
```

#### Comparison

```asm
FCMP D0, D1           ; Set NZCV based on D0 - D1 (quiet NaN → no exception)
FCMPE D0, D1          ; Same, but invalid operation exception on NaN
FCMP D0, #0.0         ; Compare against 0.0
```

FCMP result encoding in NZCV: equal → Z=1,C=1; less than → N=1; greater → C=1; unordered (NaN) → C=1,V=1.

#### Fused Multiply-Add

```asm
FMADD D0, D1, D2, D3  ; D0 = D1*D2 + D3  (single rounding)
FMSUB D0, D1, D2, D3  ; D0 = -(D1*D2) + D3
FNMADD D0, D1, D2, D3 ; D0 = -(D1*D2 + D3)
FNMSUB D0, D1, D2, D3 ; D0 = D1*D2 - D3
```

#### Conversion

```asm
FCVT  D0, S1          ; S1 (f32) → D0 (f64)
FCVT  S0, D1          ; D1 (f64) → S0 (f32)
SCVTF D0, X1          ; X1 (i64 signed) → D0 (f64)
UCVTF D0, X1          ; X1 (u64 unsigned) → D0 (f64)
SCVTF S0, W1          ; W1 (i32 signed) → S0 (f32)
FCVTZS X0, D1         ; D1 (f64) → X0 (i64, round toward zero, signed)
FCVTZU X0, D1         ; D1 (f64) → X0 (u64, round toward zero, unsigned)
FCVTNS X0, D1         ; D1 → X0 (round to nearest, ties to even, signed)
```

The FPCR (Floating-Point Control Register) controls rounding mode, exception enables, and flush-to-zero. FPSR holds cumulative exception flags.

### NEON Vector Instructions

NEON operates on V registers interpreted as vectors of elements. The arrangement specifier describes element type and count: `8B`, `16B`, `4H`, `8H`, `2S`, `4S`, `1D`, `2D`.

#### Load / Store

```asm
LD1 {V0.4S}, [X1]             ; Load 4 × 32-bit floats from X1 into V0
LD1 {V0.4S, V1.4S}, [X1]      ; Load 2 × 4S into V0 and V1 (8 floats)
LD1 {V0.4S}, [X1], #16        ; Load and post-increment X1 by 16
ST1 {V0.4S}, [X1]             ; Store 4 × 32-bit from V0 to X1
LD2 {V0.4S, V1.4S}, [X1]      ; Deinterleave load (V0=evens, V1=odds)
LD3 {V0.8B, V1.8B, V2.8B}, [X1] ; Deinterleave 3-way (RGB loads)
```

#### Integer Vector Arithmetic

```asm
ADD V0.4S, V1.4S, V2.4S       ; V0 = V1 + V2 (4 × i32, wrapping)
SUB V0.8H, V1.8H, V2.8H       ; V0 = V1 - V2 (8 × i16)
MUL V0.4S, V1.4S, V2.4S       ; V0 = V1 * V2 (low 32 bits of each)
SQADD V0.16B, V1.16B, V2.16B  ; Saturating add (8-bit signed)
UQADD V0.16B, V1.16B, V2.16B  ; Saturating add (8-bit unsigned)
```

#### Floating-Point Vector Arithmetic

```asm
FADD V0.4S, V1.4S, V2.4S      ; V0 = V1 + V2 (4 × f32)
FMUL V0.2D, V1.2D, V2.2D      ; V0 = V1 * V2 (2 × f64)
FMLA V0.4S, V1.4S, V2.4S      ; V0 += V1 * V2 (fused multiply-accumulate)
FDIV V0.4S, V1.4S, V2.4S      ; V0 = V1 / V2
FMAX V0.4S, V1.4S, V2.4S      ; V0 = max(V1, V2) element-wise
```

#### Data Rearrangement

```asm
DUP V0.4S, W1                  ; Broadcast W1 to all 4 lanes of V0
DUP V0.4S, V1.S[2]             ; Broadcast lane 2 of V1 to all lanes of V0
EXT V0.16B, V1.16B, V2.16B, #4 ; Concatenate V1:V2, extract bytes [4:19]
ZIP1 V0.4S, V1.4S, V2.4S       ; Interleave low halves
ZIP2 V0.4S, V1.4S, V2.4S       ; Interleave high halves
UZP1 V0.4S, V1.4S, V2.4S       ; Deinterleave even elements
UZP2 V0.4S, V1.4S, V2.4S       ; Deinterleave odd elements
TRN1 V0.4S, V1.4S, V2.4S       ; Transpose even elements
TRN2 V0.4S, V1.4S, V2.4S       ; Transpose odd elements
REV64 V0.4S, V1.4S              ; Reverse 32-bit elements within each 64-bit pair
```

#### Reduction

```asm
ADDV S0, V1.4S         ; S0 = V1[0] + V1[1] + V1[2] + V1[3] (reduce)
FADDP V0.4S, V1.4S, V2.4S ; Pairwise add: V0 = {V2[3]+V2[2], V2[1]+V2[0], V1[3]+V1[2], V1[1]+V1[0]}
FMAXV S0, V1.4S        ; S0 = max of all lanes in V1
```

---

## 10. Implementation Strategy in Rust

### Crate Architecture

```
helm-ng/
├── helm-core/          # Simulator core (Rust)
│   ├── src/
│   │   ├── isa/
│   │   │   ├── aarch64/
│   │   │   │   ├── mod.rs          # ISA trait impl
│   │   │   │   ├── decode.rs       # Top-level decode dispatch
│   │   │   │   ├── decode_dp_imm.rs
│   │   │   │   ├── decode_dp_reg.rs
│   │   │   │   ├── decode_ls.rs
│   │   │   │   ├── decode_branch.rs
│   │   │   │   ├── decode_simd.rs
│   │   │   │   ├── execute.rs      # Instruction execution
│   │   │   │   ├── regs.rs         # Register file
│   │   │   │   ├── sysregs.rs      # System register map
│   │   │   │   ├── pstate.rs       # PSTATE/NZCV
│   │   │   │   ├── mem.rs          # MMU / address translation
│   │   │   │   └── exception.rs    # Exception injection
│   │   │   └── riscv/              # existing
│   │   └── ...
```

### Register File

```rust
/// AArch64 general-purpose register file.
/// Index 31 is ZR (reads 0, writes discarded) in data context,
/// or SP in memory addressing context.
#[derive(Debug, Default)]
pub struct RegFile {
    /// X0–X30. X31 is not stored (ZR/SP handled separately).
    gpr: [u64; 31],
    /// Stack pointer for current EL (SP_EL0 or SP_EL1 per SPSel).
    sp: u64,
    /// Program counter.
    pub pc: u64,
    /// PSTATE flags and fields.
    pub pstate: PState,
    /// SIMD/FP registers V0–V31 (128-bit each).
    vregs: [u128; 32],
}

impl RegFile {
    /// Read a GPR. Index 31 = ZR (returns 0).
    #[inline]
    pub fn x(&self, idx: u8) -> u64 {
        if idx == 31 { 0 } else { self.gpr[idx as usize] }
    }

    /// Write a GPR. Index 31 = ZR (discards write).
    #[inline]
    pub fn set_x(&mut self, idx: u8, val: u64) {
        if idx != 31 { self.gpr[idx as usize] = val; }
    }

    /// Read a W register (32-bit). Zero-extended from X register.
    #[inline]
    pub fn w(&self, idx: u8) -> u32 {
        self.x(idx) as u32
    }

    /// Write a W register. Zero-extends into the full X register.
    #[inline]
    pub fn set_w(&mut self, idx: u8, val: u32) {
        self.set_x(idx, val as u64);  // zero-extend, not sign-extend
    }

    /// Read SP (addressing context — index 31 = SP, not ZR).
    #[inline]
    pub fn sp(&self) -> u64 { self.sp }

    #[inline]
    pub fn set_sp(&mut self, val: u64) { self.sp = val; }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PState {
    pub n: bool,
    pub z: bool,
    pub c: bool,
    pub v: bool,
    pub el: u8,       // 0–3
    pub sp_sel: bool, // false = SP_EL0, true = SP_ELn
    pub daif: u8,     // bits: D=3, A=2, I=1, F=0
    pub nrw: bool,    // false = AArch64, true = AArch32
}

impl PState {
    /// Encode as a 32-bit SPSR value (AArch64 save format).
    pub fn to_spsr(&self) -> u64 {
        let mut v: u64 = 0;
        if self.n { v |= 1 << 31; }
        if self.z { v |= 1 << 30; }
        if self.c { v |= 1 << 29; }
        if self.v { v |= 1 << 28; }
        v |= (self.daif as u64 & 0xf) << 6;
        if self.nrw { v |= 1 << 4; }
        v |= (self.el as u64 & 0x3) << 2;
        if self.sp_sel { v |= 1; }
        v
    }

    /// Restore from a saved SPSR value.
    pub fn from_spsr(spsr: u64) -> Self {
        Self {
            n: (spsr >> 31) & 1 != 0,
            z: (spsr >> 30) & 1 != 0,
            c: (spsr >> 29) & 1 != 0,
            v: (spsr >> 28) & 1 != 0,
            daif: ((spsr >> 6) & 0xf) as u8,
            nrw: (spsr >> 4) & 1 != 0,
            el: ((spsr >> 2) & 0x3) as u8,
            sp_sel: spsr & 1 != 0,
        }
    }
}
```

### Instruction Representation

```rust
#[derive(Debug, Clone)]
pub enum Instruction {
    // Data Processing — Immediate
    AddImm { sf: bool, rd: u8, rn: u8, imm12: u16, shift: bool, set_flags: bool },
    SubImm { sf: bool, rd: u8, rn: u8, imm12: u16, shift: bool, set_flags: bool },
    AndImm { sf: bool, rd: u8, rn: u8, n: u8, immr: u8, imms: u8 },
    OrrImm { sf: bool, rd: u8, rn: u8, n: u8, immr: u8, imms: u8 },
    EorImm { sf: bool, rd: u8, rn: u8, n: u8, immr: u8, imms: u8 },
    MovN   { sf: bool, rd: u8, imm16: u16, hw: u8 },
    MovZ   { sf: bool, rd: u8, imm16: u16, hw: u8 },
    MovK   { sf: bool, rd: u8, imm16: u16, hw: u8 },
    Adr    { rd: u8, imm21: i32 },
    Adrp   { rd: u8, imm21: i32 },

    // Branches
    B    { imm26: i32 },
    Bl   { imm26: i32 },
    Br   { rn: u8 },
    Blr  { rn: u8 },
    Ret  { rn: u8 },
    BCond { cond: u8, imm19: i32 },
    Cbz  { sf: bool, rt: u8, imm19: i32 },
    Cbnz { sf: bool, rt: u8, imm19: i32 },
    Tbz  { rt: u8, bit: u8, imm14: i32 },
    Tbnz { rt: u8, bit: u8, imm14: i32 },

    // System
    Svc  { imm16: u16 },
    Eret,
    Wfi,
    Wfe,
    Nop,
    Mrs  { rt: u8, sysreg: SysRegId },
    Msr  { sysreg: SysRegId, rt: u8 },
    Dsb  { option: u8 },
    Dmb  { option: u8 },
    Isb,

    // Loads and Stores
    LdrImm   { size: u8, rt: u8, rn: u8, imm9: i16, wback: bool, post: bool },
    LdrUoff  { size: u8, rt: u8, rn: u8, imm12: u16 },
    LdrReg   { size: u8, rt: u8, rn: u8, rm: u8, ext: u8, s: bool },
    StrImm   { size: u8, rt: u8, rn: u8, imm9: i16, wback: bool, post: bool },
    LdpImm   { sf: bool, rt1: u8, rt2: u8, rn: u8, imm7: i16, wback: bool, post: bool },
    StpImm   { sf: bool, rt1: u8, rt2: u8, rn: u8, imm7: i16, wback: bool, post: bool },
    Ldaxr    { size: u8, rt: u8, rn: u8 },
    Stlxr    { size: u8, rs: u8, rt: u8, rn: u8 },

    // Data Processing — Register
    AddReg   { sf: bool, rd: u8, rn: u8, rm: u8, shift: u8, imm6: u8, set_flags: bool },
    SubReg   { sf: bool, rd: u8, rn: u8, rm: u8, shift: u8, imm6: u8, set_flags: bool },
    AndReg   { sf: bool, rd: u8, rn: u8, rm: u8, shift: u8, imm6: u8, set_flags: bool },
    OrrReg   { sf: bool, rd: u8, rn: u8, rm: u8, shift: u8, imm6: u8 },
    EorReg   { sf: bool, rd: u8, rn: u8, rm: u8, shift: u8, imm6: u8 },
    Lsl      { sf: bool, rd: u8, rn: u8, rm: u8 },
    Lsr      { sf: bool, rd: u8, rn: u8, rm: u8 },
    Asr      { sf: bool, rd: u8, rn: u8, rm: u8 },
    Ror      { sf: bool, rd: u8, rn: u8, rm: u8 },
    Clz      { sf: bool, rd: u8, rn: u8 },
    Rev      { sf: bool, rd: u8, rn: u8 },
    Csel     { sf: bool, rd: u8, rn: u8, rm: u8, cond: u8 },
    Csinc    { sf: bool, rd: u8, rn: u8, rm: u8, cond: u8 },
    Csinv    { sf: bool, rd: u8, rn: u8, rm: u8, cond: u8 },
    Csneg    { sf: bool, rd: u8, rn: u8, rm: u8, cond: u8 },
    Mul      { sf: bool, rd: u8, rn: u8, rm: u8 },
    SMull    { rd: u8, rn: u8, rm: u8 },
    UMull    { rd: u8, rn: u8, rm: u8 },
    UDiv     { sf: bool, rd: u8, rn: u8, rm: u8 },
    SDiv     { sf: bool, rd: u8, rn: u8, rm: u8 },

    // SIMD / FP (scalar)
    FmovRegScalar  { ftype: u8, rd: u8, rn: u8 },
    FmovGprToFp    { sf: bool, ftype: u8, rd: u8, rn: u8 },
    FmovFpToGpr    { sf: bool, ftype: u8, rd: u8, rn: u8 },
    Fadd           { ftype: u8, rd: u8, rn: u8, rm: u8 },
    Fsub           { ftype: u8, rd: u8, rn: u8, rm: u8 },
    Fmul           { ftype: u8, rd: u8, rn: u8, rm: u8 },
    Fdiv           { ftype: u8, rd: u8, rn: u8, rm: u8 },
    Fcmp           { ftype: u8, rn: u8, rm: u8, with_exception: bool },
    Fmadd          { ftype: u8, rd: u8, rn: u8, rm: u8, ra: u8 },
    Fmsub          { ftype: u8, rd: u8, rn: u8, rm: u8, ra: u8 },
    Fcvt           { dst_type: u8, src_type: u8, rd: u8, rn: u8 },
    Scvtf          { sf: bool, ftype: u8, rd: u8, rn: u8 },
    Ucvtf          { sf: bool, ftype: u8, rd: u8, rn: u8 },
    Fcvtzs         { sf: bool, ftype: u8, rd: u8, rn: u8 },
    Fcvtzu         { sf: bool, ftype: u8, rd: u8, rn: u8 },

    Undefined { raw: u32 },
}
```

### Decoding with `deku`

`deku` is a Rust crate for declarative bit-field parsing. It generates `DekuRead`/`DekuWrite` impls from field-level `#[deku(bits = "N")]` annotations, handling endianness and field ordering automatically.

```rust
use deku::prelude::*;

/// ADD/SUB (immediate) — sf:op:S:100010:shift:imm12:Rn:Rd
#[derive(Debug, DekuRead)]
#[deku(endian = "little")]
pub struct AddSubImm {
    #[deku(bits = "5")] pub rd: u8,
    #[deku(bits = "5")] pub rn: u8,
    #[deku(bits = "12")] pub imm12: u16,
    #[deku(bits = "1")] pub shift: u8,    // 0=unshifted, 1=LSL#12
    #[deku(bits = "6")] pub fixed: u8,    // must be 0b100010
    #[deku(bits = "1")] pub s: u8,        // set flags
    #[deku(bits = "1")] pub op: u8,       // 0=add, 1=sub
    #[deku(bits = "1")] pub sf: u8,       // 0=32-bit, 1=64-bit
}

/// CBZ/CBNZ — sf:011010:op:imm19:Rt
#[derive(Debug, DekuRead)]
#[deku(endian = "little")]
pub struct CbzCbnz {
    #[deku(bits = "5")] pub rt: u8,
    #[deku(bits = "19")] pub imm19: u32, // sign-extend before use
    #[deku(bits = "1")] pub op: u8,      // 0=CBZ, 1=CBNZ
    #[deku(bits = "6")] pub fixed: u8,   // 0b011010
    #[deku(bits = "1")] pub sf: u8,
}

/// LDP/STP (signed offset) — opc:101:V:00:L:imm7:Rt2:Rn:Rt
#[derive(Debug, DekuRead)]
#[deku(endian = "little")]
pub struct LdpStp {
    #[deku(bits = "5")] pub rt: u8,
    #[deku(bits = "5")] pub rn: u8,
    #[deku(bits = "5")] pub rt2: u8,
    #[deku(bits = "7")] pub imm7: u8,   // scaled by pair element size
    #[deku(bits = "1")] pub l: u8,       // 0=store, 1=load
    #[deku(bits = "1")] pub v: u8,       // 0=GPR, 1=SIMD
    #[deku(bits = "3")] pub fixed: u8,   // 0b101
    #[deku(bits = "2")] pub index: u8,   // 01=post, 10=offset, 11=pre
    #[deku(bits = "2")] pub opc: u8,     // 00=32-bit, 10=64-bit, 01=LDPSW
}
```

Note: `deku` reads fields LSB-first from the raw bits in little-endian integer mode, which matches AArch64's field layout (Rd at bits [4:0], etc.). Verify field ordering with your specific `deku` version — the API evolved between 0.15 and 0.18.

### Top-Level Decode Dispatch

```rust
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("Unallocated encoding: {0:#010x}")]
    Unallocated(u32),
    #[error("Undefined instruction: {0:#010x}")]
    Undefined(u32),
    #[error("Unsupported instruction: {0:#010x}")]
    Unsupported(u32),
}

pub fn decode_aarch64(raw: u32) -> Result<Instruction, DecodeError> {
    // Primary decode: bits [28:25]
    let op0 = (raw >> 25) & 0xf;

    match op0 {
        // Unallocated
        0b0000 | 0b0001 | 0b0011 => Err(DecodeError::Unallocated(raw)),

        // Data Processing — Immediate
        0b1000 | 0b1001 => decode_data_processing_imm(raw),

        // Branches, Exception Generating, System
        0b1010 | 0b1011 => decode_branch_exception_system(raw),

        // Loads and Stores
        0b0100 | 0b0110 | 0b1100 | 0b1110 => decode_loads_stores(raw),

        // Data Processing — Register
        0b0101 | 0b1101 => decode_data_processing_reg(raw),

        // Data Processing — SIMD/FP
        0b0111 | 0b1111 => decode_data_processing_simd_fp(raw),

        // SVE (not implemented in initial version)
        0b0010 => Err(DecodeError::Unsupported(raw)),

        _ => Err(DecodeError::Unallocated(raw)),
    }
}

fn decode_branch_exception_system(raw: u32) -> Result<Instruction, DecodeError> {
    let op1 = (raw >> 29) & 0x7;  // bits [31:29]

    match op1 {
        // Unconditional branch (immediate): B, BL
        0b000 | 0b100 => {
            let op = (raw >> 31) & 1;
            let imm26 = sign_extend((raw & 0x3FF_FFFF) as i32, 26);
            let imm26_bytes = imm26 << 2;
            if op == 0 { Ok(Instruction::B { imm26: imm26_bytes }) }
            else        { Ok(Instruction::Bl { imm26: imm26_bytes }) }
        }
        // Compare and branch (immediate): CBZ, CBNZ
        0b001 | 0b101 => {
            let sf  = (raw >> 31) & 1 != 0;
            let op  = (raw >> 24) & 1 != 0;
            let imm19 = sign_extend(((raw >> 5) & 0x7_FFFF) as i32, 19) << 2;
            let rt  = (raw & 0x1f) as u8;
            if !op { Ok(Instruction::Cbz  { sf, rt, imm19 }) }
            else   { Ok(Instruction::Cbnz { sf, rt, imm19 }) }
        }
        // Conditional branch: B.cond
        0b010 => {
            let cond  = (raw & 0xf) as u8;
            let imm19 = sign_extend(((raw >> 5) & 0x7_FFFF) as i32, 19) << 2;
            Ok(Instruction::BCond { cond, imm19 })
        }
        // Exception generating / system / hints
        0b110 => decode_exception_system(raw),
        // Unconditional branch (register): BR, BLR, RET, ERET
        0b111 => decode_branch_register(raw),
        _ => Err(DecodeError::Unallocated(raw)),
    }
}

#[inline]
fn sign_extend(val: i32, bits: u32) -> i32 {
    let shift = 32 - bits;
    (val << shift) >> shift
}
```

### Flag Computation Helpers

```rust
/// Compute NZCV flags for addition: result = a + b (+ carry_in).
pub fn flags_add(a: u64, b: u64, carry_in: u64, sf: bool) -> (u64, bool, bool, bool, bool) {
    let result128 = (a as u128) + (b as u128) + (carry_in as u128);
    let result = result128 as u64;
    let (n, z, c, v) = if sf {
        let c = result128 > u64::MAX as u128;
        let v = ((!(a ^ b) & (a ^ result)) >> 63) & 1 != 0;
        ((result >> 63) & 1 != 0, result == 0, c, v)
    } else {
        let r32 = result as u32;
        let result64_32 = (a as u32 as u64) + (b as u32 as u64) + carry_in;
        let c = result64_32 > u32::MAX as u64;
        let v = ((!(a as u32 ^ b as u32) & (a as u32 ^ r32)) >> 31) & 1 != 0;
        ((r32 >> 31) & 1 != 0, r32 == 0, c, v)
    };
    (result, n, z, c, v)
}

/// Subtraction: a - b = a + NOT(b) + 1
/// C flag: AArch64 convention — C=1 means NO borrow (a >= b unsigned).
pub fn flags_sub(a: u64, b: u64, sf: bool) -> (u64, bool, bool, bool, bool) {
    flags_add(a, !b, 1, sf)
    // The C flag from flags_add for NOT(b)+1 gives the correct AArch64 carry
}
```

### System Register Map

```rust
use std::collections::HashMap;

/// System register identifier (op0:op1:CRn:CRm:op2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SysRegId {
    pub op0: u8, pub op1: u8,
    pub crn: u8, pub crm: u8,
    pub op2: u8,
}

impl SysRegId {
    pub fn from_encoding(op0: u8, op1: u8, crn: u8, crm: u8, op2: u8) -> Self {
        Self { op0, op1, crn, crm, op2 }
    }
}

pub mod sysregs {
    use super::SysRegId;

    pub const SCTLR_EL1:  SysRegId = SysRegId { op0: 3, op1: 0, crn: 1, crm: 0, op2: 0 };
    pub const TCR_EL1:    SysRegId = SysRegId { op0: 3, op1: 0, crn: 2, crm: 0, op2: 2 };
    pub const TTBR0_EL1:  SysRegId = SysRegId { op0: 3, op1: 0, crn: 2, crm: 0, op2: 0 };
    pub const TTBR1_EL1:  SysRegId = SysRegId { op0: 3, op1: 0, crn: 2, crm: 0, op2: 1 };
    pub const MAIR_EL1:   SysRegId = SysRegId { op0: 3, op1: 0, crn: 10, crm: 2, op2: 0 };
    pub const ESR_EL1:    SysRegId = SysRegId { op0: 3, op1: 0, crn: 5, crm: 2, op2: 0 };
    pub const FAR_EL1:    SysRegId = SysRegId { op0: 3, op1: 0, crn: 6, crm: 0, op2: 0 };
    pub const ELR_EL1:    SysRegId = SysRegId { op0: 3, op1: 0, crn: 4, crm: 0, op2: 1 };
    pub const SPSR_EL1:   SysRegId = SysRegId { op0: 3, op1: 0, crn: 4, crm: 0, op2: 0 };
    pub const VBAR_EL1:   SysRegId = SysRegId { op0: 3, op1: 0, crn: 12, crm: 0, op2: 0 };
    pub const SP_EL0:     SysRegId = SysRegId { op0: 3, op1: 0, crn: 4, crm: 1, op2: 0 };
    pub const SP_EL1:     SysRegId = SysRegId { op0: 3, op1: 4, crn: 4, crm: 1, op2: 0 };
    pub const TPIDR_EL0:  SysRegId = SysRegId { op0: 3, op1: 3, crn: 13, crm: 0, op2: 2 };
    pub const TPIDR_EL1:  SysRegId = SysRegId { op0: 3, op1: 0, crn: 13, crm: 0, op2: 4 };
    pub const CPACR_EL1:  SysRegId = SysRegId { op0: 3, op1: 0, crn: 1, crm: 0, op2: 2 };
    pub const CURRENT_EL: SysRegId = SysRegId { op0: 3, op1: 0, crn: 4, crm: 2, op2: 2 };
    pub const NZCV:       SysRegId = SysRegId { op0: 3, op1: 3, crn: 4, crm: 2, op2: 0 };
    pub const DAIF:       SysRegId = SysRegId { op0: 3, op1: 3, crn: 4, crm: 2, op2: 1 };
    pub const FPCR:       SysRegId = SysRegId { op0: 3, op1: 3, crn: 4, crm: 4, op2: 0 };
    pub const FPSR:       SysRegId = SysRegId { op0: 3, op1: 3, crn: 4, crm: 4, op2: 1 };
}

pub struct SysRegFile {
    regs: HashMap<SysRegId, u64>,
}

impl SysRegFile {
    pub fn read(&self, id: SysRegId) -> u64 {
        *self.regs.get(&id).unwrap_or(&0)
    }

    pub fn write(&mut self, id: SysRegId, val: u64) {
        self.regs.insert(id, val);
    }
}
```

### Exception Handling

```rust
pub struct AArch64Cpu {
    pub regs: RegFile,
    pub sys: SysRegFile,
}

impl AArch64Cpu {
    /// Inject a synchronous exception to EL1.
    /// Called for: SVC, data abort, instruction abort, alignment fault, undefined instruction.
    pub fn take_exception_to_el1(&mut self, esr: u64, far: Option<u64>, elr: u64) {
        // 1. Save current state
        let spsr = self.regs.pstate.to_spsr();
        self.sys.write(sysregs::SPSR_EL1, spsr);
        self.sys.write(sysregs::ELR_EL1, elr);

        // 2. Set ESR and optionally FAR
        self.sys.write(sysregs::ESR_EL1, esr);
        if let Some(addr) = far {
            self.sys.write(sysregs::FAR_EL1, addr);
        }

        // 3. Update PSTATE for EL1
        self.regs.pstate.el = 1;
        self.regs.pstate.sp_sel = true;       // use SP_EL1
        self.regs.pstate.daif = 0xf;           // mask all interrupts
        self.regs.pstate.n = false;
        self.regs.pstate.z = false;
        self.regs.pstate.c = false;
        self.regs.pstate.v = false;

        // 4. Set PC to vector table entry
        let vbar = self.sys.read(sysregs::VBAR_EL1);
        // Lower EL, AArch64, Synchronous = VBAR + 0x400
        self.regs.pc = vbar + 0x400;
    }

    /// Execute ERET — restore PSTATE and PC from EL1 saved state.
    pub fn eret(&mut self) {
        let spsr = self.sys.read(sysregs::SPSR_EL1);
        let elr = self.sys.read(sysregs::ELR_EL1);
        self.regs.pstate = PState::from_spsr(spsr);
        self.regs.pc = elr;
    }

    /// Handle SVC: save state, set ESR, jump to vector.
    pub fn svc(&mut self, imm16: u16) {
        // ESR_EL1: EC=0x15 (SVC AArch64), ISS = imm16
        let esr = (0x15u64 << 26) | (1u64 << 25) | (imm16 as u64);
        let elr = self.regs.pc + 4; // return to instruction after SVC
        self.take_exception_to_el1(esr, None, elr);
    }
}
```

---

## 11. AArch32 / Thumb Interworking

### Coexistence in ARMv8-A

ARMv8-A hardware can run both AArch64 and AArch32 code on the same chip, but not simultaneously in the same EL. The execution state for a given EL is determined by:

- `HCR_EL2.RW`: controls whether EL1 runs in AArch64 (1) or AArch32 (0)
- `SCR_EL3.RW`: controls whether EL2 runs in AArch64 or AArch32
- For EL0: controlled by `PSTATE.nRW` which follows from the EL1 state unless HCR_EL2 allows EL0 to flip independently

On a system where EL1 is AArch64 (Linux 64-bit kernel):
- EL0 can run AArch32 code (32-bit userland processes via compatibility mode)
- The kernel services them via the `compat_` syscall wrappers
- The AArch32 register file is mapped onto the lower registers of AArch64

### Register Mapping (AArch32 EL0 under AArch64 EL1)

| AArch32 | AArch64 Equivalent |
|---|---|
| R0–R12 | X0–X12 (lower 32 bits) |
| SP (R13) | W13 (SP_EL0 lower 32 bits) |
| LR (R14) | W14 |
| PC (R15) | PC (managed separately) |
| CPSR | PSTATE (N, Z, C, V, DAIF, etc.) |
| FPSCR | FPCR + FPSR |

The banked registers (R13_fiq, R14_fiq, etc.) from AArch32 modes do not exist in AArch64; they are stored in dedicated memory or virtualized by the OS.

### What Must Be Stubbed for SE Mode

For SE mode targeting AArch64 binaries only, AArch32/Thumb support can be entirely deferred. The minimal stubs required:

1. **Exception on AArch32 entry**: if `PSTATE.nRW = 1` is somehow entered, inject an `Illegal Execution State` exception (ESR.EC = 0b001110) or panic.

2. **`PSTATE.nRW` tracking**: always 0 in pure AArch64 SE mode. No action needed.

3. **`HCR_EL2.RW` bit**: if a guest program writes HCR_EL2 (which it should not in EL1), trap or ignore.

4. **Thumb instruction identification**: if the PC is at an odd address (Thumb state indicator), return `DecodeError::Unsupported`. A correctly compiled AArch64 binary will never produce this.

5. **Document limitation**: the SE mode AArch64 simulator does not support AArch32 compat mode. 64-bit ELF binaries only.

---

## 12. Testing AArch64 Implementation

### ARM Architecture Conformance Suite (AACS)

The ARM Architecture Conformance Suite (formerly LISA) tests AArch64 implementation correctness. It is commercial and available to ARM licensees. Key areas tested:

- Instruction semantics for every encoding (including edge cases: XZR/SP disambiguation, 32-bit zero-extension)
- Exception model: vector offsets, SPSR save/restore, ELR correctness
- System register access semantics (side effects on write, RES0/RES1 bit handling)
- Memory model: ordering guarantees, TLB invalidation sequencing
- NZCV flag computation for all arithmetic instructions

For open-source testing, use the combination below.

### Linaro LTP (Linux Test Project)

The Linaro LTP port runs the full Linux Test Project suite on AArch64:

```bash
# Build LTP for AArch64
git clone https://github.com/linux-test-project/ltp.git
cd ltp
make autotools && ./configure --host=aarch64-linux-gnu
make -j8
# Run on helm-ng (once syscall emulation is functional)
helm-ng run ltp/testcases/kernel/syscalls/read/read01
```

LTP exercises thousands of syscall paths and validates userland-visible behavior.

### QEMU as Reference Oracle

QEMU's `qemu-aarch64` (user-mode emulation) is the most practical reference. Use it to generate expected outputs for test binaries:

```bash
# Install QEMU user-mode AArch64
sudo apt install qemu-user-static

# Run a cross-compiled AArch64 binary
qemu-aarch64-static ./hello_aarch64

# With strace (syscall tracing)
qemu-aarch64-static -strace ./hello_aarch64 2>&1 | head -50

# Enable verbose instruction tracing (requires QEMU debug build)
qemu-aarch64-static -d in_asm,int,cpu_reset ./test_binary 2>qemu_trace.txt

# Compare register state at key points using GDB stub
qemu-aarch64-static -g 1234 ./test_binary &
gdb-multiarch -ex "target remote :1234" -ex "info registers" ./test_binary
```

### Differential Testing Strategy

Build a test harness that runs the same binary on QEMU and helm-ng, comparing:

1. **Register state** after each basic block (or at program exit)
2. **Memory writes** — the set of (address, value) pairs produced
3. **Syscall trace** — sequence of (syscall_nr, args, return_value) tuples
4. **Exit code** — final process exit status

```rust
// Pseudocode for differential testing
fn diff_test(binary: &Path) {
    let qemu_trace = run_qemu_with_trace(binary);
    let helm_trace = run_helm_with_trace(binary);
    assert_traces_equal(qemu_trace, helm_trace);
}
```

### Common Implementation Bugs

These are the most frequently encountered correctness bugs in AArch64 simulators:

#### 1. SP Alignment Fault

AArch64 requires that SP is 16-byte aligned when used for EL1 stack operations. A misaligned SP on exception entry or `LDP`/`STP` at EL1 generates a Stack Pointer Alignment Fault (ESR.EC = 0x26). Symptoms: kernel stack corruption or fault loops.

```rust
// Check SP alignment when entering EL1 from EL0
fn check_sp_alignment(sp: u64) -> Result<(), AArch64Fault> {
    if sp & 0xf != 0 {
        Err(AArch64Fault::SpAlignment)
    } else {
        Ok(())
    }
}
```

#### 2. PSTATE.DAIF Not Preserved Across Exception Return

`ERET` must restore the full PSTATE including DAIF bits from SPSR_EL1. A common bug is restoring only the NZCV flags. If DAIF is not restored, interrupts may remain masked (or unmasked) incorrectly after returning from an exception handler.

Verification: after `ERET`, read `DAIF` and compare to the value that was in PSTATE before the exception was taken.

#### 3. W Register Zero-Extension (Not Sign-Extension)

Writing to a W register (32-bit) must zero-extend into X, not sign-extend. A 32-bit write of `0xFFFF_FFFF` to W0 should produce X0 = `0x0000_0000_FFFF_FFFF`, not `0xFFFF_FFFF_FFFF_FFFF`.

This is the opposite of what happens on x86-64 (where 32-bit writes also zero-extend) but distinct from ARM AArch32 conventions. Arithmetic instructions that target W registers implicitly perform this extension.

```rust
// WRONG — sign-extends
fn set_w_wrong(&mut self, idx: u8, val: u32) {
    self.set_x(idx, val as i32 as i64 as u64); // sign-extends!
}

// CORRECT — zero-extends
fn set_w_correct(&mut self, idx: u8, val: u32) {
    self.set_x(idx, val as u64); // zero-extends
}
```

#### 4. NZCV Carry Flag for Subtraction

AArch64 uses "carry = NOT borrow" for subtraction. The C flag for `SUBS Xd, Xn, Xm`:
- C = 1 if Xn >= Xm (unsigned) — no borrow
- C = 0 if Xn < Xm (unsigned) — borrow occurred

This is computed correctly by `ADD(Xn, NOT(Xm), 1)` — the carry out of that addition is the AArch64 C flag. A common bug is using `(result < a)` directly (which gives the wrong polarity for subtraction).

```rust
// Correct subtraction flags via complement-add
fn flags_sub(a: u64, b: u64, sf: bool) -> FlagResult {
    // a - b = a + NOT(b) + 1
    // C from this addition = 1 when a >= b (no borrow)
    flags_add(a, !b, 1, sf)
}
```

#### 5. ADRP Page Rounding

`ADRP` computes a page-aligned PC-relative address. The page is 4KB (4096 bytes). The base for the calculation is the current PC **rounded down to 4KB alignment**, not the raw PC:

```
ADRP Xd, label:
  page_base = PC & ~0xFFF            // zero bottom 12 bits
  offset    = SignExtend(imm21, 64) << 12
  Xd        = page_base + offset
```

A common bug is to skip the page rounding and use the raw PC. This produces wrong addresses for code that is not at a page-aligned address.

```rust
fn execute_adrp(&mut self, rd: u8, imm21: i32) {
    let page_base = self.regs.pc & !0xFFF_u64;  // mask bottom 12 bits
    let offset = (imm21 as i64 as u64).wrapping_shl(12); // sign-extend then shift
    let result = page_base.wrapping_add(offset);
    self.regs.set_x(rd, result);
}
```

#### 6. XZR vs SP Disambiguation in Encoding 31

Register encoding 31 means different things in different instruction fields:
- In most data processing fields (Rd, Rn, Rm): encoding 31 = XZR (zero register)
- In base register fields of load/store instructions (Rn): encoding 31 = SP
- In `ADD`/`SUB` extended register (Rd field): encoding 31 = SP (used for `MOV SP, Xn`)

The decoder must track which semantic applies per field per instruction class. A lookup table of `(instruction_class, field_name) → RegisterInterpretation` is the clearest approach.

#### 7. Condition Code Edge Cases

- `AL` (always): condition = 0b1110, unconditionally true. Some assemblers emit `B.AL` for unconditional branches; it should always branch.
- `NV` (never): condition = 0b1111, historically undefined, now executes as `AL` in AArch64 per the spec. Treat as always true.
- `HI` (unsigned higher): C=1 AND Z=0. A common bug is `C=1 OR Z=0`.
- `LS` (unsigned lower or same): C=0 OR Z=1. The OR is required.

#### 8. Bitfield Immediate Encoding (AND/ORR/EOR Immediate)

Logical immediate instructions (AND, ORR, EOR, TST with immediate) use a compact 3-field encoding: `(N, immr, imms)` that encodes a repeating bitmask. The decode is non-trivial — it specifies an element size, rotation, and pattern length. Incorrectly implementing `DecodeBitMasks(N, imms, immr, sf)` produces wrong constants for logical operations.

Reference: ARM DDI 0487 section C4.1.2 "Decode of modified immediate constants in A64 instructions".

---

## References

- **ARM Architecture Reference Manual ARMv8-A** — ARM DDI 0487 (the authoritative source; freely available from developer.arm.com)
- **ARM Cortex-A Programmer's Guide for ARMv8-A** — DEN0024A (gentler introduction)
- **ARM Architecture Reference Manual for A-profile architecture** — DDI 0487K (2024 edition; supersedes DDI 0487)
- **AAPCS64** — Procedure Call Standard for the Arm 64-bit Architecture (defines ABI: register usage, stack layout, alignment)
- **deku crate** — https://docs.rs/deku (bit-field parsing)
- **QEMU AArch64 TCG** — `target/arm/` in QEMU source (reference implementation in C)
- **Linux kernel `arch/arm64/`** — canonical EL1 code: exception vectors (`entry.S`), system register usage, MMU setup
- **Linaro LTP** — https://github.com/linux-test-project/ltp
- **ARM Fast Models** — commercial cycle-accurate reference model (if ARM licensee)
- **LLVM AArch64 backend** — `llvm/lib/Target/AArch64/` (reference for encoding tables and ABI)
