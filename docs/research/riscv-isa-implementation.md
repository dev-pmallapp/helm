# RISC-V ISA Implementation Research

**Target:** RV64GC (RV64I + M + A + F + D + C + Zicsr + Zifencei)
**Role in helm-ng:** First ISA implementation; establishes the decode/execute/CSR/MMU patterns for all future ISAs.

---

## Table of Contents

1. [RISC-V Specification Overview](#1-risc-v-specification-overview)
2. [Register File](#2-register-file)
3. [Privilege Levels](#3-privilege-levels)
4. [Memory Model](#4-memory-model)
5. [Interrupt and Exception Model](#5-interrupt-and-exception-model)
6. [Floating Point](#6-floating-point)
7. [Atomic Instructions (A Extension)](#7-atomic-instructions-a-extension)
8. [Implementation Strategy in Rust](#8-implementation-strategy-in-rust)
9. [CSR Implementation](#9-csr-implementation)
10. [Compressed Instructions (C Extension)](#10-compressed-instructions-c-extension)
11. [Testing RISC-V Implementation](#11-testing-risc-v-implementation)

---

## 1. RISC-V Specification Overview

### What RV64GC Means

**RV64** — 64-bit base address and integer width (XLEN = 64).

**G** is a shorthand for the "general-purpose" extension bundle:

| Letter | Extension | Description |
|--------|-----------|-------------|
| I      | Base Integer | Core integer instructions |
| M      | Multiply/Divide | Integer multiply, divide, remainder |
| A      | Atomic | LR/SC, AMO instructions |
| F      | Single-precision FP | IEEE 754 32-bit float |
| D      | Double-precision FP | IEEE 754 64-bit float (superset of F) |
| Zicsr  | CSR instructions | CSRRW, CSRRS, CSRRC and their immediate forms |
| Zifencei | Instruction-Fetch Fence | FENCE.I instruction |

**C** — Compressed 16-bit instructions. Not part of G, but required by most real-world ABIs (RISC-V Linux ABI mandates C). Every 16-bit instruction is an alias for a specific 32-bit instruction; the ISA defines the mapping precisely.

### RV64I Base Integer ISA

**Key design properties:**
- Fixed 32-bit instruction width (before C extension)
- Load/store architecture — all arithmetic operates on registers
- No condition codes / flags register
- 32 general-purpose integer registers (x0 hardwired to zero)
- PC not directly addressable as a register
- All branches are PC-relative

**Instruction categories in RV64I:**

| Category | Mnemonics |
|----------|-----------|
| Load | LB, LH, LW, LD, LBU, LHU, LWU |
| Store | SB, SH, SW, SD |
| Integer ALU (register) | ADD, SUB, SLL, SLT, SLTU, XOR, SRL, SRA, OR, AND |
| Integer ALU (immediate) | ADDI, SLTI, SLTIU, XORI, ORI, ANDI, SLLI, SRLI, SRAI |
| 32-bit word ops (RV64 only) | ADDW, SUBW, SLLW, SRLW, SRAW, ADDIW, SLLIW, SRLIW, SRAIW |
| Branches | BEQ, BNE, BLT, BGE, BLTU, BGEU |
| Jumps | JAL, JALR |
| Upper immediate | LUI, AUIPC |
| System | ECALL, EBREAK, FENCE, WFI |

The `*W` instructions operate on the low 32 bits and sign-extend the result to 64 bits. This is a critical correctness point — `ADDW` is not the same as `ADD` on a 64-bit machine.

### Instruction Encoding Formats

RISC-V uses six encoding formats. All share: opcode in bits [6:0].

#### R-Type (register-register operations)

```
 31      25 24    20 19    15 14  12 11     7 6       0
┌──────────┬────────┬────────┬──────┬────────┬─────────┐
│  funct7  │  rs2   │  rs1   │funct3│   rd   │ opcode  │
│  [31:25] │ [24:20]│ [19:15]│[14:12]│ [11:7]│  [6:0]  │
└──────────┴────────┴────────┴──────┴────────┴─────────┘
     7          5        5       3       5         7      = 32 bits
```

Example — `ADD rd, rs1, rs2`:
- opcode = `0110011` (OP)
- funct3 = `000`
- funct7 = `0000000`

Example — `SUB rd, rs1, rs2`:
- Same opcode/funct3, but funct7 = `0100000` (bit 30 set differentiates ADD/SUB, SRL/SRA, etc.)

#### I-Type (immediate, loads, JALR, CSR)

```
 31           20 19    15 14  12 11     7 6       0
┌───────────────┬────────┬──────┬────────┬─────────┐
│   imm[11:0]   │  rs1   │funct3│   rd   │ opcode  │
│    [31:20]    │ [19:15]│[14:12]│ [11:7]│  [6:0]  │
└───────────────┴────────┴──────┴────────┴─────────┘
       12             5       3       5         7
```

Immediate is sign-extended from bit 31. Range: -2048 to +2047.

Shifts (SLLI/SRLI/SRAI) in RV64 encode the shift amount in imm[5:0] (6 bits needed for 0–63), with imm[11:6] acting as additional funct bits.

#### S-Type (stores)

```
 31      25 24    20 19    15 14  12 11     7 6       0
┌──────────┬────────┬────────┬──────┬────────┬─────────┐
│ imm[11:5]│  rs2   │  rs1   │funct3│imm[4:0]│ opcode  │
└──────────┴────────┴────────┴──────┴────────┴─────────┘
```

Immediate split: `imm[11:5]` in bits [31:25], `imm[4:0]` in bits [11:7]. Sign-extended from bit 31.

```rust
fn decode_s_imm(raw: u32) -> i32 {
    let lo = (raw >> 7) & 0x1f;          // imm[4:0]
    let hi = (raw >> 25) & 0x7f;         // imm[11:5]
    let imm = (hi << 5) | lo;
    // sign-extend from bit 11
    ((imm as i32) << 20) >> 20
}
```

#### B-Type (branches)

```
 31      25 24    20 19    15 14  12 11     7 6       0
┌──────────┬────────┬────────┬──────┬────────┬─────────┐
│imm[12|10:5] rs2   │  rs1   │funct3│imm[4:1|11] opcode│
└──────────┴────────┴────────┴──────┴────────┴─────────┘
```

Bit layout (B scrambles to allow sign extension and max reuse with S-type):
- bit 31 → imm[12] (sign bit)
- bits 30:25 → imm[10:5]
- bits 11:8 → imm[4:1]
- bit 7 → imm[11]

Immediate is always even (bit 0 implicitly 0). Range: ±4 KiB.

```rust
fn decode_b_imm(raw: u32) -> i32 {
    let imm12  = (raw >> 31) & 1;
    let imm11  = (raw >> 7)  & 1;
    let imm10_5 = (raw >> 25) & 0x3f;
    let imm4_1  = (raw >> 8)  & 0xf;
    let imm = (imm12 << 12) | (imm11 << 11) | (imm10_5 << 5) | (imm4_1 << 1);
    ((imm as i32) << 19) >> 19   // sign-extend from bit 12
}
```

#### U-Type (LUI, AUIPC)

```
 31                      12 11     7 6       0
┌──────────────────────────┬────────┬─────────┐
│        imm[31:12]        │   rd   │ opcode  │
│          [31:12]         │ [11:7] │  [6:0]  │
└──────────────────────────┴────────┴─────────┘
```

Encodes a 20-bit immediate shifted left by 12. The lower 12 bits of the result are zeroed. Used with I-type immediates for full 32-bit constant materialization:

```asm
lui  a0, %hi(0xDEADBEEF)    # loads upper 20 bits
addi a0, a0, %lo(0xDEADBEEF) # adds lower 12 bits
```

#### J-Type (JAL)

```
 31      30        21 20 19        12 11     7 6       0
┌──┬──────────────┬───┬─────────────┬────────┬─────────┐
│imm[20]│imm[10:1]│imm[11]│imm[19:12]│  rd   │ opcode  │
└──┴──────────────┴───┴─────────────┴────────┴─────────┘
```

Bit layout (scrambled similarly to B-type):
- bit 31 → imm[20]
- bits 30:21 → imm[10:1]
- bit 20 → imm[11]
- bits 19:12 → imm[19:12]

Immediate is always even. Range: ±1 MiB. JAL stores PC+4 in rd (link register).

### Opcode Map (Primary Opcodes)

```
0000011  LOAD
0000111  LOAD-FP
0001111  MISC-MEM (FENCE)
0010011  OP-IMM
0010111  AUIPC
0011011  OP-IMM-32 (RV64 word immediates)
0100011  STORE
0100111  STORE-FP
0101111  AMO (atomics)
0110011  OP (register-register)
0110111  LUI
0111011  OP-32 (RV64 word operations)
1000011  MADD (FP fused)
1000111  MSUB (FP fused)
1001011  NMSUB (FP fused)
1001111  NMADD (FP fused)
1010011  OP-FP (float ops)
1100011  BRANCH
1100111  JALR
1101111  JAL
1110011  SYSTEM (ECALL, EBREAK, CSR*, WFI, MRET, SRET)
```

### 16-bit Compressed Instruction Overlap

Bits [1:0] of any RISC-V instruction word determine whether it is 16-bit or 32-bit:

| bits [1:0] | Interpretation |
|------------|----------------|
| `00`       | Compressed (C extension quadrant 0) |
| `01`       | Compressed (C extension quadrant 1) |
| `10`       | Compressed (C extension quadrant 2) |
| `11`       | 32-bit instruction (may be wider — `11111` → 48-bit, etc.) |

The decode loop must check bits [1:0] first, before assuming instruction width. Since RISC-V allows mixed 16/32-bit streams, the PC advances by 2 or 4 depending on the instruction fetched.

---

## 2. Register File

### Integer Registers (x0–x31)

| Register | ABI Name | Role | Saved by |
|----------|----------|------|----------|
| x0 | zero | Hardwired zero | — |
| x1 | ra | Return address | Caller |
| x2 | sp | Stack pointer | Callee |
| x3 | gp | Global pointer | — |
| x4 | tp | Thread pointer | — |
| x5 | t0 | Temporary / alternate link | Caller |
| x6 | t1 | Temporary | Caller |
| x7 | t2 | Temporary | Caller |
| x8 | s0 / fp | Saved / frame pointer | Callee |
| x9 | s1 | Saved register | Callee |
| x10 | a0 | Arg 0 / return value 0 | Caller |
| x11 | a1 | Arg 1 / return value 1 | Caller |
| x12 | a2 | Argument 2 | Caller |
| x13 | a3 | Argument 3 | Caller |
| x14 | a4 | Argument 4 | Caller |
| x15 | a5 | Argument 5 | Caller |
| x16 | a6 | Argument 6 | Caller |
| x17 | a7 | Argument 7 / syscall number | Caller |
| x18 | s2 | Saved register | Callee |
| x19 | s3 | Saved register | Callee |
| x20 | s4 | Saved register | Callee |
| x21 | s5 | Saved register | Callee |
| x22 | s6 | Saved register | Callee |
| x23 | s7 | Saved register | Callee |
| x24 | s8 | Saved register | Callee |
| x25 | s9 | Saved register | Callee |
| x26 | s10 | Saved register | Callee |
| x27 | s11 | Saved register | Callee |
| x28 | t3 | Temporary | Caller |
| x29 | t4 | Temporary | Caller |
| x30 | t5 | Temporary | Caller |
| x31 | t6 | Temporary | Caller |

**x0 is always zero.** Writes to x0 are silently discarded. Reads always return 0. The implementation must enforce this; it is not optional.

```rust
pub struct IntRegFile([u64; 32]);

impl IntRegFile {
    pub fn read(&self, reg: usize) -> u64 {
        if reg == 0 { 0 } else { self.0[reg] }
    }
    pub fn write(&mut self, reg: usize, val: u64) {
        if reg != 0 { self.0[reg] = val; }
    }
}
```

### Float Registers (f0–f31)

Float registers are 64-bit wide (holding either f32 NaN-boxed or f64 values). The D extension unifies f32 and f64 into the same register file.

| Register | ABI Name | Role | Saved by |
|----------|----------|------|----------|
| f0 | ft0 | Temporary | Caller |
| f1 | ft1 | Temporary | Caller |
| f2 | ft2 | Temporary | Caller |
| f3 | ft3 | Temporary | Caller |
| f4 | ft4 | Temporary | Caller |
| f5 | ft5 | Temporary | Caller |
| f6 | ft6 | Temporary | Caller |
| f7 | ft7 | Temporary | Caller |
| f8 | fs0 | Saved | Callee |
| f9 | fs1 | Saved | Callee |
| f10 | fa0 | Arg 0 / return 0 | Caller |
| f11 | fa1 | Arg 1 / return 1 | Caller |
| f12 | fa2 | Argument 2 | Caller |
| f13 | fa3 | Argument 3 | Caller |
| f14 | fa4 | Argument 4 | Caller |
| f15 | fa5 | Argument 5 | Caller |
| f16 | fa6 | Argument 6 | Caller |
| f17 | fa7 | Argument 7 | Caller |
| f18 | fs2 | Saved | Callee |
| f19 | fs3 | Saved | Callee |
| f20 | fs4 | Saved | Callee |
| f21 | fs5 | Saved | Callee |
| f22 | fs6 | Saved | Callee |
| f23 | fs7 | Saved | Callee |
| f24 | fs8 | Saved | Callee |
| f25 | fs9 | Saved | Callee |
| f26 | fs10 | Saved | Callee |
| f27 | fs11 | Saved | Callee |
| f28 | ft8 | Temporary | Caller |
| f29 | ft9 | Temporary | Caller |
| f30 | ft10 | Temporary | Caller |
| f31 | ft11 | Temporary | Caller |

### CSR Register Map

CSRs are addressed by a 12-bit address in the CSRRW/CSRRS/CSRRC instructions. The address encodes the privilege level and read/write status in bits [11:8]:

| Bits [11:10] | Access | Bits [9:8] | Lowest privilege |
|-------------|--------|-----------|-----------------|
| 00 | Read/write | 00 | U-mode |
| 01 | Read/write | 01 | S-mode |
| 10 | Read/write | 10 | H-mode (hypervisor) |
| 11 | Read-only | 11 | M-mode |

#### Key M-mode CSRs

| Address | Name | Description |
|---------|------|-------------|
| `0x300` | mstatus | Machine status register |
| `0x301` | misa | ISA and extensions |
| `0x302` | medeleg | Machine exception delegation |
| `0x303` | mideleg | Machine interrupt delegation |
| `0x304` | mie | Machine interrupt enable |
| `0x305` | mtvec | Machine trap-handler base address |
| `0x306` | mcounteren | Machine counter enable |
| `0x310` | mstatush | Additional machine status (RV32 only, but reserved in RV64) |
| `0x340` | mscratch | Machine scratch register |
| `0x341` | mepc | Machine exception program counter |
| `0x342` | mcause | Machine trap cause |
| `0x343` | mtval | Machine trap value |
| `0x344` | mip | Machine interrupt pending |
| `0x3A0` | pmpcfg0 | PMP configuration register 0 |
| `0x3B0` | pmpaddr0 | PMP address register 0 |
| `0xB00` | mcycle | Machine cycle counter |
| `0xB02` | minstret | Machine instructions-retired counter |
| `0xF11` | mvendorid | Vendor ID (read-only) |
| `0xF12` | marchid | Architecture ID (read-only) |
| `0xF13` | mimpid | Implementation ID (read-only) |
| `0xF14` | mhartid | Hardware thread ID (read-only) |

#### Key S-mode CSRs

| Address | Name | Description |
|---------|------|-------------|
| `0x100` | sstatus | Supervisor status (restricted view of mstatus) |
| `0x102` | sedeleg | Supervisor exception delegation |
| `0x103` | sideleg | Supervisor interrupt delegation |
| `0x104` | sie | Supervisor interrupt enable |
| `0x105` | stvec | Supervisor trap vector |
| `0x106` | scounteren | Supervisor counter enable |
| `0x140` | sscratch | Supervisor scratch |
| `0x141` | sepc | Supervisor exception PC |
| `0x142` | scause | Supervisor trap cause |
| `0x143` | stval | Supervisor trap value |
| `0x144` | sip | Supervisor interrupt pending |
| `0x180` | satp | Supervisor address translation and protection |

#### Floating-Point CSRs

| Address | Name | Description |
|---------|------|-------------|
| `0x001` | fflags | FP accrued exception flags |
| `0x002` | frm | FP rounding mode |
| `0x003` | fcsr | FP control and status (fflags + frm combined) |

`fcsr` layout:
```
 63      8 7   5 4  0
┌──────────┬─────┬─────┐
│ reserved │ frm │flags│
└──────────┴─────┴─────┘
```

`frm` encoding:
| frm | Mode | Description |
|-----|------|-------------|
| 000 | RNE | Round to nearest, ties to even |
| 001 | RTZ | Round toward zero |
| 010 | RDN | Round down (toward -∞) |
| 011 | RUP | Round up (toward +∞) |
| 100 | RMM | Round to nearest, ties to max magnitude |
| 101–110 | — | Reserved |
| 111 | DYN | Use frm field from instruction |

#### User-mode CSRs (read-only shadows)

| Address | Name | Description |
|---------|------|-------------|
| `0xC00` | cycle | Cycle counter (shadow of mcycle) |
| `0xC01` | time | Real-time clock |
| `0xC02` | instret | Instructions retired (shadow of minstret) |

---

## 3. Privilege Levels

### Mode Overview

RISC-V defines three privilege levels, from highest to lowest:

| Mode | Encoding | Description |
|------|----------|-------------|
| Machine (M) | 11 | Highest privilege; always present; full hardware access |
| Supervisor (S) | 01 | OS kernel; controls MMU, handles S-mode traps |
| User (U) | 00 | Application code; no privileged instructions |

On reset, the hart starts in M-mode. The current privilege level is tracked in hardware (not directly readable by software), encoded in `mstatus.MPP` and `mstatus.SPP` when relevant.

**M-mode can do everything.** It can read/write any CSR, access any physical memory, and install trap handlers. A minimal bare-metal environment only needs M-mode.

**S-mode** enables virtual memory (satp), delegates trap handling to S-mode handlers, and runs OS kernels (Linux, FreeBSD, etc.). S-mode cannot access M-mode CSRs.

**U-mode** runs unprivileged user applications. Any access to privileged resources triggers an exception (illegal instruction or access fault) routed to the appropriate trap handler.

### mstatus Fields

`mstatus` is a 64-bit CSR that controls interrupt enables, prior privilege state, and various extension status bits.

```
Bit(s)  Field  Description
─────────────────────────────────────────────────────────
  63    SD     Summary: FS/XS/VS dirty (any extension state dirty)
[62:36] —      Reserved (WPRI)
[35:34] SXL    S-mode XLEN (RV64: always 10)
[33:32] UXL    U-mode XLEN (RV64: always 10)
[31:23] —      Reserved
  22    TSR    Trap SRET (when 1, SRET in S-mode traps to M-mode)
  21    TW     Timeout Wait (WFI traps to M-mode after timeout)
  20    TVM    Trap Virtual Memory (SFENCE.VMA/satp access traps to M-mode)
  19    MXR    Make eXecutable Readable (for page table walks)
  18    SUM    Supervisor User Memory access (allows S-mode to access U pages)
  17    MPRV   Modify PRiVilege (use MPP mode for loads/stores)
[16:15] XS     User extension state (dirty/clean/off)
[14:13] FS     FP unit state (00=Off, 01=Initial, 10=Clean, 11=Dirty)
[12:11] MPP    M-mode Previous Privilege (mode before last M-mode trap)
[10:9]  VS     Vector extension state
   8    SPP    S-mode Previous Privilege (0=U, 1=S)
   7    MPIE   M-mode Previous Interrupt Enable
   6    UBE    U-mode Big-Endian
   5    SPIE   S-mode Previous Interrupt Enable
   4    —      Reserved
   3    MIE    M-mode Interrupt Enable (global)
   2    —      Reserved
   1    SIE    S-mode Interrupt Enable (global)
   0    —      Reserved
```

Key invariant: when a trap is taken into M-mode, `MPIE ← MIE`, `MIE ← 0`, `MPP ← prior_priv`. On MRET: `MIE ← MPIE`, `MPIE ← 1`, privilege ← `MPP`.

### Trap Handling: Exception Flow

When an exception or interrupt occurs and is handled in M-mode:

1. `mepc` ← PC of faulting instruction (or next instruction for interrupts)
2. `mcause` ← cause code (interrupt bit [63] + code [62:0])
3. `mtval` ← additional info (faulting address, illegal instruction word, etc.)
4. `mstatus.MPIE` ← `mstatus.MIE`
5. `mstatus.MIE` ← 0 (disable interrupts)
6. `mstatus.MPP` ← current privilege level
7. PC ← `mtvec` (direct mode) or `mtvec` base + 4×cause (vectored mode)

### mtvec Modes

`mtvec` has two modes controlled by bits [1:0]:

| MODE | Value | Behavior |
|------|-------|----------|
| Direct | 0 | All traps jump to BASE (bits [63:2] << 2) |
| Vectored | 1 | Interrupts jump to BASE + 4×cause; exceptions always jump to BASE |

```rust
fn compute_trap_vector(mtvec: u64, cause: u64, is_interrupt: bool) -> u64 {
    let base = mtvec & !0x3;
    let mode = mtvec & 0x3;
    if mode == 1 && is_interrupt {
        base + 4 * (cause & 0x3FFF_FFFF_FFFF_FFFF)
    } else {
        base
    }
}
```

### mcause Encoding

| Bit 63 | Bits [62:0] | Meaning |
|--------|------------|---------|
| 1 | cause code | Interrupt |
| 0 | cause code | Exception (synchronous) |

**Synchronous exceptions (bit 63 = 0):**

| Code | Exception |
|------|-----------|
| 0 | Instruction address misaligned |
| 1 | Instruction access fault |
| 2 | Illegal instruction |
| 3 | Breakpoint (EBREAK) |
| 4 | Load address misaligned |
| 5 | Load access fault |
| 6 | Store/AMO address misaligned |
| 7 | Store/AMO access fault |
| 8 | Environment call from U-mode |
| 9 | Environment call from S-mode |
| 11 | Environment call from M-mode |
| 12 | Instruction page fault |
| 13 | Load page fault |
| 15 | Store/AMO page fault |

**Interrupts (bit 63 = 1):**

| Code | Interrupt |
|------|-----------|
| 1 | Supervisor software interrupt |
| 3 | Machine software interrupt |
| 5 | Supervisor timer interrupt |
| 7 | Machine timer interrupt |
| 9 | Supervisor external interrupt |
| 11 | Machine external interrupt |

### ECALL Flow (U-mode → M-mode)

```
U-mode executes ECALL (opcode 0x73, funct12 = 0x000)
  → exception code 8 (ECALL from U-mode)
  → mepc = PC of ECALL
  → mcause = 8
  → mtval = 0
  → M-mode trap handler invoked at mtvec
  → handler reads a7 for syscall number, a0–a5 for args
  → sets a0/a1 for return values
  → executes MRET
MRET:
  → PC = mepc (the ECALL instruction)
  → privilege ← mstatus.MPP
  Wait — mepc points to ECALL; handler must add 4 to mepc before MRET!
```

This is a common bug: the trap handler must advance `mepc` by 4 before executing MRET, or the system will loop back to the ECALL.

### MRET / SRET

**MRET** (Machine Return, encoding `0x30200073`):
1. PC ← mepc
2. privilege ← mstatus.MPP
3. mstatus.MIE ← mstatus.MPIE
4. mstatus.MPIE ← 1
5. mstatus.MPP ← U (least-privilege mode supported)

**SRET** (Supervisor Return, encoding `0x10200073`):
1. PC ← sepc
2. privilege ← mstatus.SPP (0=U, 1=S)
3. mstatus.SIE ← mstatus.SPIE
4. mstatus.SPIE ← 1
5. mstatus.SPP ← U

SRET in M-mode is always valid. SRET in S-mode is only valid if mstatus.TSR = 0.

### Physical Memory Protection (PMP)

PMP allows M-mode to restrict memory access for lower privilege modes. Up to 16 PMP regions (more in newer specs). Each region has:
- `pmpaddrN` — the address register (encoding depends on mode)
- `pmpcfgN` — 8-bit config per region: `L | -- | A[1:0] | X | W | R`

PMP address matching modes (A field):
| A | Mode | Description |
|---|------|-------------|
| 0 | OFF | Disabled |
| 1 | TOR | Top Of Range (addr > prev pmpaddr, addr ≤ pmpaddr) |
| 2 | NA4 | Naturally aligned 4-byte region |
| 3 | NAPOT | Naturally aligned power-of-two region |

For a minimal M-mode-only simulator, PMP can initially be a stub that grants all accesses (no PMP entries configured = M-mode can access everything, S/U-mode cannot access anything unless PMP explicitly allows it — but for M-mode-only this doesn't matter).

---

## 4. Memory Model

### Virtual Memory: Sv39 and Sv48

Virtual memory is controlled by the `satp` CSR (Supervisor Address Translation and Protection):

```
 63  60 59          44 43                           0
┌──────┬──────────────┬───────────────────────────────┐
│ MODE │    ASID      │            PPN                │
│ [4]  │   [16]       │           [44]                │
└──────┴──────────────┴───────────────────────────────┘
```

| MODE | Value | Description |
|------|-------|-------------|
| Bare | 0 | No translation (physical addresses) |
| Sv39 | 8 | 39-bit virtual address space, 3-level page table |
| Sv48 | 9 | 48-bit virtual address space, 4-level page table |
| Sv57 | 10 | 57-bit virtual address space, 5-level page table |

**PPN** is the physical page number of the root page table (shifted right by 12 — multiply by 4096 to get physical address).

**ASID** (Address Space ID) allows TLB tagging by process; SFENCE.VMA can flush specific ASIDs.

### Sv39 Address Translation

Sv39 uses 39-bit virtual addresses (bits [38:0] valid; bits [63:39] must be sign-extensions of bit 38).

Virtual address breakdown:
```
 38      30 29      21 20      12 11                0
┌──────────┬──────────┬──────────┬───────────────────┐
│  VPN[2]  │  VPN[1]  │  VPN[0]  │   Page Offset     │
│   [9]    │   [9]    │   [9]    │      [12]          │
└──────────┴──────────┴──────────┴───────────────────┘
```

Translation walk (3 levels):
1. Root page table at physical address `satp.PPN × 4096`
2. Index into level-2 table using VPN[2] (512 entries × 8 bytes)
3. Load PTE; if leaf (R or X set), done; else descend to level-1 table
4. Index into level-1 table using VPN[1]
5. Load PTE; if leaf, done; else descend to level-0 table
6. Index into level-0 table using VPN[0]
7. Load PTE; must be a leaf
8. Physical address = PTE.PPN × 4096 + VA.offset

### Page Table Entry (PTE) Format

Each PTE is 8 bytes:

```
 63    54 53        28 27        19 18        10 9  8 7 6 5 4 3 2 1 0
┌────────┬────────────┬────────────┬────────────┬────┬─┬─┬─┬─┬─┬─┬─┬─┐
│ Rsrvd  │  PPN[2]    │  PPN[1]    │  PPN[0]    │ RSW│D│A│G│U│X│W│R│V│
│ [10]   │  [26]      │  [9]       │  [9]       │[2] │ │ │ │ │ │ │ │ │
└────────┴────────────┴────────────┴────────────┴────┴─┴─┴─┴─┴─┴─┴─┴─┘
```

| Flag | Bit | Meaning |
|------|-----|---------|
| V | 0 | Valid |
| R | 1 | Readable |
| W | 2 | Writable |
| X | 3 | Executable |
| U | 4 | User-accessible (U-mode can access) |
| G | 5 | Global mapping (not flushed by ASID-specific SFENCE.VMA) |
| A | 6 | Accessed (set by hardware on access) |
| D | 7 | Dirty (set by hardware on write) |
| RSW | [9:8] | Reserved for supervisor software |

**Leaf PTE rules:**
- If R=0 and W=0 and X=0: this is a pointer PTE (non-leaf), PPN points to next-level table
- If R=1 or X=1: this is a leaf PTE
- W=1 without R=1 is reserved (illegal)

**Superpage:** A leaf PTE at level 1 or 2 creates a 2 MiB or 1 GiB superpage (Sv39). The low PPN bits in a superpage PTE must be zero (naturally aligned).

**A and D bits:** Hardware must set A on any access and D on any write. Simulators can always set both to 1 when creating PTEs to avoid A/D fault handling complexity, or implement proper A/D fault raising.

### TLB Invalidation: SFENCE.VMA

```
SFENCE.VMA rs1, rs2
```

- rs1 = 0, rs2 = 0: flush all TLB entries
- rs1 ≠ 0, rs2 = 0: flush TLB entries for virtual address in rs1
- rs1 = 0, rs2 ≠ 0: flush TLB entries with ASID in rs2
- rs1 ≠ 0, rs2 ≠ 0: flush TLB entry for virtual address in rs1 with ASID in rs2

In a simple simulator without a real TLB, SFENCE.VMA can be a no-op. In a simulator with a TLB cache for performance, the appropriate entries must be invalidated.

### FENCE Instruction

```
FENCE pred, succ
```

RISC-V has a weak (relaxed) memory model (RVWMO). FENCE ensures memory ordering between predecessor and successor operations. The pred/succ fields encode which operation classes to order:

| Bit | Meaning |
|-----|---------|
| I | Device input |
| O | Device output |
| R | Memory reads |
| W | Memory writes |

For a single-hart simulator, FENCE is a no-op (total store order is guaranteed). For multi-hart simulation, FENCE must synchronize the memory subsystem.

**FENCE.I** — instruction-fetch barrier. Must flush any instruction cache / JIT cache. For a simple interpreter, no-op.

---

## 5. Interrupt and Exception Model

### Synchronous vs. Asynchronous

- **Exceptions** (synchronous): caused by the currently-executing instruction. PC is known precisely. Examples: illegal instruction, ECALL, page fault.
- **Interrupts** (asynchronous): caused by external events (timers, devices). Checked at instruction boundaries. The interrupted instruction has not yet executed; mepc points to it.

### Interrupt Enabling and Pending

Interrupts are enabled globally by `mstatus.MIE` (M-mode) and `mstatus.SIE` (S-mode).

Individual interrupt sources are controlled by `mie` (Machine Interrupt Enable):

```
Bit 11: MEIE — Machine External Interrupt Enable
Bit  9: SEIE — Supervisor External Interrupt Enable
Bit  7: MTIE — Machine Timer Interrupt Enable
Bit  5: STIE — Supervisor Timer Interrupt Enable
Bit  3: MSIE — Machine Software Interrupt Enable
Bit  1: SSIE — Supervisor Software Interrupt Enable
```

`mip` (Machine Interrupt Pending) has the same layout but reflects pending interrupt signals:

```
Bit 11: MEIP — Machine External Interrupt Pending
Bit  9: SEIP — Supervisor External Interrupt Pending
Bit  7: MTIP — Machine Timer Interrupt Pending
Bit  5: STIP — Supervisor Timer Interrupt Pending
Bit  3: MSIP — Machine Software Interrupt Pending
Bit  1: SSIP — Supervisor Software Interrupt Pending
```

A trap is taken when: `(mip & mie) != 0` AND `mstatus.MIE = 1` (for M-mode interrupts).

### Interrupt Priority

When multiple interrupts are pending, RISC-V defines a priority order (higher = taken first):

1. MEI (Machine External, bit 11)
2. MSI (Machine Software, bit 3)
3. MTI (Machine Timer, bit 7)
4. SEI (Supervisor External, bit 9)
5. SSI (Supervisor Software, bit 1)
6. STI (Supervisor Timer, bit 5)

### Interrupt Delegation: mideleg / medeleg

By default, all traps are handled in M-mode. Delegation registers allow M-mode to route traps to S-mode:

- `medeleg` — each bit corresponds to a synchronous exception cause code. Setting bit N delegates exception N to S-mode.
- `mideleg` — each bit corresponds to an interrupt cause code. Setting bit N delegates interrupt N to S-mode.

When an exception is delegated and occurs in U-mode or S-mode:
- `sepc` ← PC of faulting instruction
- `scause` ← cause
- `stval` ← additional info
- `mstatus.SPIE` ← `mstatus.SIE`, `mstatus.SIE` ← 0
- `mstatus.SPP` ← prior privilege
- PC ← stvec

M-mode traps are **never** delegated to S-mode (a trap in M-mode stays in M-mode).

### CLINT (Core-Local Interruptor)

The CLINT provides two interrupt sources per hart:

- **mtime** — read-only memory-mapped register, increments at fixed frequency. Typically at physical address `0x0200BFF8` (SiFive/QEMU convention).
- **mtimecmp** — write causes MTI (Machine Timer Interrupt) to be pending when `mtime >= mtimecmp`. Clear the interrupt by writing a future value to `mtimecmp`. Typically at `0x02004000` (hart 0).
- **msip** — software interrupt pending bit (write 1 to trigger MSI). Typically at `0x02000000` (hart 0).

CLINT base address is platform-defined. QEMU `virt` machine uses `0x02000000`.

```rust
pub struct Clint {
    pub mtime: u64,      // increments with simulation ticks
    pub mtimecmp: u64,   // timer compare value
    pub msip: u32,       // software interrupt pending (bit 0 = hart 0)
}

impl Clint {
    pub fn mtip_pending(&self) -> bool {
        self.mtime >= self.mtimecmp
    }
    pub fn msip_pending(&self) -> bool {
        self.msip & 1 != 0
    }
}
```

### PLIC (Platform-Level Interrupt Controller)

The PLIC aggregates external interrupt sources and routes them to harts. It is considerably more complex than the CLINT. Key registers (SiFive PLIC memory map):

- `priority[src]` — interrupt priority for source N (0 = disabled)
- `pending[word]` — bitmap of pending interrupts
- `enable[ctx][word]` — per-context enable bitmaps
- `threshold[ctx]` — minimum priority for this context
- `claim[ctx]` — read to claim highest pending interrupt; write to complete

For a minimal simulator targeting Linux boot, a stub PLIC that always returns "no interrupt" is sufficient initially.

---

## 6. Floating Point

### RV64D Overview

RV64D implements IEEE 754-2008 double-precision floating point. When D is present, F (single-precision) is also required. The unified 64-bit register file holds both f32 and f64 values.

### FP Instruction Categories

| Category | Instructions |
|----------|-------------|
| Load/Store | FLW, FSW, FLD, FSD |
| Compute | FADD, FSUB, FMUL, FDIV, FSQRT (.S and .D variants) |
| Fused Multiply-Add | FMADD, FMSUB, FNMADD, FNMSUB (.S and .D) |
| Min/Max | FMIN, FMAX (.S and .D) |
| Compare | FEQ, FLT, FLE (.S and .D) → result in integer register |
| Classify | FCLASS (.S and .D) → 10-bit classification in integer register |
| Convert | FCVT.W.S, FCVT.L.S, FCVT.W.D, FCVT.L.D, FCVT.S.W, etc. |
| Sign inject | FSGNJ, FSGNJN, FSGNJX (.S and .D) |
| Move | FMV.X.W, FMV.W.X (between float and integer regs, 32-bit) |
| Move | FMV.X.D, FMV.D.X (between float and integer regs, 64-bit) |

### Rounding Modes

Each FP instruction encodes a 3-bit `rm` field. When `rm = 111` (DYN), the rounding mode comes from `fcsr.frm`. Otherwise the instruction-embedded mode is used.

Illegal `rm` values (101 and 110) raise an illegal instruction exception.

### FP Exception Flags in fcsr

`fcsr.fflags` (bits [4:0]):

| Bit | Flag | Name | Condition |
|-----|------|------|-----------|
| 4 | NV | Invalid Operation | e.g., 0/0, ∞−∞, sqrt(-1), unordered comparison |
| 3 | DZ | Divide by Zero | finite/0 |
| 2 | OF | Overflow | result too large to represent |
| 1 | UF | Underflow | result too small (after rounding) |
| 0 | NX | Inexact | result not exactly representable |

Flags **accumulate** (OR semantics). CSRRS can set flags; CSRRC can clear them. The `fflags` CSR is the low 5 bits of `fcsr`.

### NaN Boxing (f32 in f64 Registers)

When a 32-bit float is written to an f-register in an RV64D system, it occupies the low 32 bits and the high 32 bits are set to all-ones (`0xFFFF_FFFF`). This creates a quiet NaN in the 64-bit interpretation.

When reading an f-register for a 32-bit FP operation, if the high 32 bits are not all-ones, the value is treated as a canonical NaN (not the stored value). This prevents undefined behavior from uninitialized upper bits.

```rust
fn write_f32_to_freg(freg: &mut u64, val: f32) {
    let bits = val.to_bits() as u64;
    *freg = bits | 0xFFFF_FFFF_0000_0000;  // NaN-box
}

fn read_f32_from_freg(freg: u64) -> f32 {
    if freg >> 32 == 0xFFFF_FFFF {
        f32::from_bits(freg as u32)
    } else {
        f32::NAN  // canonical NaN
    }
}
```

### FP Comparison Semantics

- `FEQ` — returns 1 if equal; returns 0 for unordered (NaN inputs). Sets NV if either operand is a signaling NaN.
- `FLT` — returns 1 if rs1 < rs2; returns 0 for unordered. Sets NV if either operand is any NaN.
- `FLE` — returns 1 if rs1 ≤ rs2; returns 0 for unordered. Sets NV if either operand is any NaN.

`FMIN`/`FMAX` with NaN inputs: if one operand is a quiet NaN, return the other. If both are NaN, return canonical quiet NaN. Signaling NaN raises NV.

### FCLASS Bit Pattern

`FCLASS.S`/`FCLASS.D` returns a 10-bit bitmask:

| Bit | Condition |
|-----|-----------|
| 0 | −∞ |
| 1 | Negative normal |
| 2 | Negative subnormal |
| 3 | −0 |
| 4 | +0 |
| 5 | Positive subnormal |
| 6 | Positive normal |
| 7 | +∞ |
| 8 | Signaling NaN |
| 9 | Quiet NaN |

---

## 7. Atomic Instructions (A Extension)

### Load-Reserved / Store-Conditional

**LR.W** (Load-Reserved Word) / **LR.D** (Load-Reserved Doubleword):
- Loads a value from memory
- Establishes a **reservation set** on that memory address
- If reservation is already held, it is replaced

**SC.W** (Store-Conditional Word) / **SC.D** (Store-Conditional Doubleword):
- Attempts to store a value to the reserved address
- Succeeds (writes value, rd ← 0) only if the reservation is still valid
- Fails (no write, rd ← 1) if the reservation was invalidated
- **Always clears the reservation**, whether successful or not

Reservation invalidation happens when:
- Another hart executes any store to an overlapping address
- The hart executes any store (optional: implementations may invalidate on any exception or context switch)
- SC.W/SC.D is executed (regardless of outcome)

```rust
pub struct Hart {
    // ...
    reservation: Option<u64>,  // physical address, or None
}

fn exec_lr_w(hart: &mut Hart, mem: &mut Memory, rd: usize, rs1: usize) -> Result<(), HartException> {
    let addr = hart.x[rs1];
    let val = mem.load_word(addr)? as i32 as i64 as u64;  // sign-extend
    hart.reservation = Some(addr);
    hart.x[rd] = val;
    Ok(())
}

fn exec_sc_w(hart: &mut Hart, mem: &mut Memory, rd: usize, rs1: usize, rs2: usize) -> Result<(), HartException> {
    let addr = hart.x[rs1];
    let success = hart.reservation == Some(addr);
    hart.reservation = None;  // always clear
    if success {
        mem.store_word(addr, hart.x[rs2] as u32)?;
        hart.x[rd] = 0;
    } else {
        hart.x[rd] = 1;
    }
    Ok(())
}
```

### AMO Instructions

AMO (Atomic Memory Operation) instructions atomically read-modify-write a memory location:

| Instruction | Operation | .W (32-bit) | .D (64-bit) |
|-------------|-----------|-------------|-------------|
| AMOSWAP | rd←mem, mem←rs2 | AMOSWAP.W | AMOSWAP.D |
| AMOADD | rd←mem, mem←mem+rs2 | AMOADD.W | AMOADD.D |
| AMOAND | rd←mem, mem←mem&rs2 | AMOAND.W | AMOAND.D |
| AMOOR | rd←mem, mem←mem\|rs2 | AMOOR.W | AMOOR.D |
| AMOXOR | rd←mem, mem←mem^rs2 | AMOXOR.W | AMOXOR.D |
| AMOMIN | rd←mem, mem←min(mem,rs2) signed | AMOMIN.W | AMOMIN.D |
| AMOMAX | rd←mem, mem←max(mem,rs2) signed | AMOMAX.W | AMOMAX.D |
| AMOMINU | rd←mem, mem←min(mem,rs2) unsigned | AMOMINU.W | AMOMINU.D |
| AMOMAXU | rd←mem, mem←max(mem,rs2) unsigned | AMOMAXU.W | AMOMAXU.D |

`.W` variants sign-extend the loaded 32-bit value to 64 bits before writing to rd.

### Ordering Suffixes

AMO and LR/SC instructions encode acquire/release semantics in bits [26:25] of the encoding:

| aq | rl | Semantic |
|----|----|---------|
| 0 | 0 | No ordering guarantee |
| 1 | 0 | Acquire — no subsequent memory ops may move before this |
| 0 | 1 | Release — no prior memory ops may move after this |
| 1 | 1 | Sequentially consistent |

For a single-hart simulator, these bits can be ignored (single-thread execution is already sequentially consistent). For multi-hart simulation, these constrain memory operation reordering.

---

## 8. Implementation Strategy in Rust

### Instruction Decoding

Decoding is pure bit manipulation with no allocation. The pattern is a two-stage decode: opcode selects format, then funct3/funct7 select the specific instruction.

```rust
/// Decode a 32-bit RISC-V instruction word into a structured Instruction.
/// Returns DecodeError::IllegalInstruction if encoding is invalid.
pub fn decode_rv64(raw: u32) -> Result<Instruction, DecodeError> {
    let opcode = raw & 0x7f;
    match opcode {
        0b011_0011 => decode_r_type(raw),        // OP: ADD, SUB, SLL, SLT...
        0b001_0011 => decode_i_type_imm(raw),    // OP-IMM: ADDI, SLTI...
        0b000_0011 => decode_load(raw),           // LOAD: LB, LH, LW, LD, LBU...
        0b010_0011 => decode_store(raw),          // STORE: SB, SH, SW, SD
        0b110_0011 => decode_branch(raw),         // BRANCH: BEQ, BNE, BLT...
        0b110_1111 => decode_jal(raw),            // JAL
        0b110_0111 => decode_jalr(raw),           // JALR
        0b011_0111 => decode_lui(raw),            // LUI
        0b001_0111 => decode_auipc(raw),          // AUIPC
        0b111_0011 => decode_system(raw),         // SYSTEM: ECALL, EBREAK, CSR*, WFI, MRET, SRET
        0b000_1111 => decode_fence(raw),          // MISC-MEM: FENCE, FENCE.I
        0b010_1111 => decode_atomic(raw),         // AMO: LR, SC, AMO*
        0b111_1011 => decode_op32(raw),           // OP-32: ADDW, SUBW...
        0b001_1011 => decode_op_imm32(raw),       // OP-IMM-32: ADDIW, SLLIW...
        // Floating point
        0b000_0111 => decode_load_fp(raw),        // LOAD-FP: FLW, FLD
        0b010_0111 => decode_store_fp(raw),       // STORE-FP: FSW, FSD
        0b100_0011 => decode_fmadd(raw),          // MADD: FMADD.S/D
        0b100_0111 => decode_fmsub(raw),          // MSUB: FMSUB.S/D
        0b100_1011 => decode_fnmsub(raw),         // NMSUB: FNMSUB.S/D
        0b100_1111 => decode_fnmadd(raw),         // NMADD: FNMADD.S/D
        0b101_0011 => decode_op_fp(raw),          // OP-FP: FADD, FSUB, FMUL...
        _ => Err(DecodeError::IllegalInstruction(raw)),
    }
}
```

Helper extractors:

```rust
#[inline(always)]
fn rd(raw: u32) -> usize    { ((raw >> 7)  & 0x1f) as usize }
#[inline(always)]
fn rs1(raw: u32) -> usize   { ((raw >> 15) & 0x1f) as usize }
#[inline(always)]
fn rs2(raw: u32) -> usize   { ((raw >> 20) & 0x1f) as usize }
#[inline(always)]
fn funct3(raw: u32) -> u32  {  (raw >> 12) & 0x07 }
#[inline(always)]
fn funct7(raw: u32) -> u32  {  (raw >> 25) & 0x7f }

#[inline(always)]
fn i_imm(raw: u32) -> i32 {
    (raw as i32) >> 20  // arithmetic shift preserves sign
}

#[inline(always)]
fn u_imm(raw: u32) -> u32 {
    raw & 0xFFFF_F000  // upper 20 bits, lower 12 zeroed
}
```

### Instruction Representation

```rust
type Reg = u8;  // 0–31, fits in u8

pub enum Instruction {
    // --- RV64I Base ---
    Lui    { rd: Reg, imm: u32 },
    Auipc  { rd: Reg, imm: u32 },
    Jal    { rd: Reg, offset: i32 },
    Jalr   { rd: Reg, rs1: Reg, offset: i32 },

    Beq    { rs1: Reg, rs2: Reg, offset: i32 },
    Bne    { rs1: Reg, rs2: Reg, offset: i32 },
    Blt    { rs1: Reg, rs2: Reg, offset: i32 },
    Bge    { rs1: Reg, rs2: Reg, offset: i32 },
    Bltu   { rs1: Reg, rs2: Reg, offset: i32 },
    Bgeu   { rs1: Reg, rs2: Reg, offset: i32 },

    Lb     { rd: Reg, rs1: Reg, offset: i32 },
    Lh     { rd: Reg, rs1: Reg, offset: i32 },
    Lw     { rd: Reg, rs1: Reg, offset: i32 },
    Ld     { rd: Reg, rs1: Reg, offset: i32 },
    Lbu    { rd: Reg, rs1: Reg, offset: i32 },
    Lhu    { rd: Reg, rs1: Reg, offset: i32 },
    Lwu    { rd: Reg, rs1: Reg, offset: i32 },

    Sb     { rs1: Reg, rs2: Reg, offset: i32 },
    Sh     { rs1: Reg, rs2: Reg, offset: i32 },
    Sw     { rs1: Reg, rs2: Reg, offset: i32 },
    Sd     { rs1: Reg, rs2: Reg, offset: i32 },

    Addi   { rd: Reg, rs1: Reg, imm: i32 },
    Slti   { rd: Reg, rs1: Reg, imm: i32 },
    Sltiu  { rd: Reg, rs1: Reg, imm: i32 },
    Xori   { rd: Reg, rs1: Reg, imm: i32 },
    Ori    { rd: Reg, rs1: Reg, imm: i32 },
    Andi   { rd: Reg, rs1: Reg, imm: i32 },
    Slli   { rd: Reg, rs1: Reg, shamt: u32 },
    Srli   { rd: Reg, rs1: Reg, shamt: u32 },
    Srai   { rd: Reg, rs1: Reg, shamt: u32 },

    Add    { rd: Reg, rs1: Reg, rs2: Reg },
    Sub    { rd: Reg, rs1: Reg, rs2: Reg },
    Sll    { rd: Reg, rs1: Reg, rs2: Reg },
    Slt    { rd: Reg, rs1: Reg, rs2: Reg },
    Sltu   { rd: Reg, rs1: Reg, rs2: Reg },
    Xor    { rd: Reg, rs1: Reg, rs2: Reg },
    Srl    { rd: Reg, rs1: Reg, rs2: Reg },
    Sra    { rd: Reg, rs1: Reg, rs2: Reg },
    Or     { rd: Reg, rs1: Reg, rs2: Reg },
    And    { rd: Reg, rs1: Reg, rs2: Reg },

    // RV64I word-size operations
    Addiw  { rd: Reg, rs1: Reg, imm: i32 },
    Slliw  { rd: Reg, rs1: Reg, shamt: u32 },
    Srliw  { rd: Reg, rs1: Reg, shamt: u32 },
    Sraiw  { rd: Reg, rs1: Reg, shamt: u32 },
    Addw   { rd: Reg, rs1: Reg, rs2: Reg },
    Subw   { rd: Reg, rs1: Reg, rs2: Reg },
    Sllw   { rd: Reg, rs1: Reg, rs2: Reg },
    Srlw   { rd: Reg, rs1: Reg, rs2: Reg },
    Sraw   { rd: Reg, rs1: Reg, rs2: Reg },

    Fence  { pred: u8, succ: u8 },
    FenceI,
    Ecall,
    Ebreak,

    // CSR instructions
    Csrrw  { rd: Reg, rs1: Reg, csr: u16 },
    Csrrs  { rd: Reg, rs1: Reg, csr: u16 },
    Csrrc  { rd: Reg, rs1: Reg, csr: u16 },
    Csrrwi { rd: Reg, uimm: u8, csr: u16 },
    Csrrsi { rd: Reg, uimm: u8, csr: u16 },
    Csrrci { rd: Reg, uimm: u8, csr: u16 },

    Mret,
    Sret,
    Wfi,

    // M extension
    Mul    { rd: Reg, rs1: Reg, rs2: Reg },
    Mulh   { rd: Reg, rs1: Reg, rs2: Reg },
    Mulhsu { rd: Reg, rs1: Reg, rs2: Reg },
    Mulhu  { rd: Reg, rs1: Reg, rs2: Reg },
    Div    { rd: Reg, rs1: Reg, rs2: Reg },
    Divu   { rd: Reg, rs1: Reg, rs2: Reg },
    Rem    { rd: Reg, rs1: Reg, rs2: Reg },
    Remu   { rd: Reg, rs1: Reg, rs2: Reg },
    Mulw   { rd: Reg, rs1: Reg, rs2: Reg },
    Divw   { rd: Reg, rs1: Reg, rs2: Reg },
    Divuw  { rd: Reg, rs1: Reg, rs2: Reg },
    Remw   { rd: Reg, rs1: Reg, rs2: Reg },
    Remuw  { rd: Reg, rs1: Reg, rs2: Reg },

    // A extension
    LrW    { rd: Reg, rs1: Reg, aq: bool, rl: bool },
    ScW    { rd: Reg, rs1: Reg, rs2: Reg, aq: bool, rl: bool },
    AmoswapW { rd: Reg, rs1: Reg, rs2: Reg, aq: bool, rl: bool },
    AmoaddW  { rd: Reg, rs1: Reg, rs2: Reg, aq: bool, rl: bool },
    // ... etc for all AMO variants and .D versions

    // F/D extensions — abbreviated
    Flw    { rd: Reg, rs1: Reg, offset: i32 },
    Fsw    { rs1: Reg, rs2: Reg, offset: i32 },
    Fld    { rd: Reg, rs1: Reg, offset: i32 },
    Fsd    { rs1: Reg, rs2: Reg, offset: i32 },
    FaddS  { rd: Reg, rs1: Reg, rs2: Reg, rm: u8 },
    FaddD  { rd: Reg, rs1: Reg, rs2: Reg, rm: u8 },
    // ... remaining ~60 FP instructions
}
```

### Execution Engine

```rust
/// Context that an instruction can read/write. Abstracting this allows
/// testing instruction semantics without a full hart.
pub trait ExecContext {
    fn read_xreg(&self, reg: Reg) -> u64;
    fn write_xreg(&mut self, reg: Reg, val: u64);
    fn read_freg(&self, reg: Reg) -> u64;
    fn write_freg(&mut self, reg: Reg, val: u64);
    fn get_pc(&self) -> u64;
    fn set_pc(&mut self, pc: u64);
    fn advance_pc(&mut self, by: u64);
    fn load_u8(&mut self, addr: u64) -> Result<u8, HartException>;
    fn load_u16(&mut self, addr: u64) -> Result<u16, HartException>;
    fn load_u32(&mut self, addr: u64) -> Result<u32, HartException>;
    fn load_u64(&mut self, addr: u64) -> Result<u64, HartException>;
    fn store_u8(&mut self, addr: u64, val: u8) -> Result<(), HartException>;
    fn store_u16(&mut self, addr: u64, val: u16) -> Result<(), HartException>;
    fn store_u32(&mut self, addr: u64, val: u32) -> Result<(), HartException>;
    fn store_u64(&mut self, addr: u64, val: u64) -> Result<(), HartException>;
    fn read_csr(&mut self, addr: u16) -> Result<u64, HartException>;
    fn write_csr(&mut self, addr: u16, val: u64) -> Result<(), HartException>;
    fn take_exception(&mut self, exc: HartException);
}

pub fn execute(insn: &Instruction, ctx: &mut impl ExecContext) -> Result<(), HartException> {
    match insn {
        Instruction::Add { rd, rs1, rs2 } => {
            let val = ctx.read_xreg(*rs1).wrapping_add(ctx.read_xreg(*rs2));
            ctx.write_xreg(*rd, val);
            ctx.advance_pc(4);
        }
        Instruction::Addi { rd, rs1, imm } => {
            let val = ctx.read_xreg(*rs1).wrapping_add(*imm as i64 as u64);
            ctx.write_xreg(*rd, val);
            ctx.advance_pc(4);
        }
        Instruction::Lw { rd, rs1, offset } => {
            let addr = ctx.read_xreg(*rs1).wrapping_add(*offset as i64 as u64);
            let word = ctx.load_u32(addr)?;
            ctx.write_xreg(*rd, word as i32 as i64 as u64); // sign-extend!
            ctx.advance_pc(4);
        }
        Instruction::Addw { rd, rs1, rs2 } => {
            let val = ctx.read_xreg(*rs1).wrapping_add(ctx.read_xreg(*rs2));
            ctx.write_xreg(*rd, val as u32 as i32 as i64 as u64); // sign-extend from 32 bits
            ctx.advance_pc(4);
        }
        Instruction::Jal { rd, offset } => {
            let pc = ctx.get_pc();
            ctx.write_xreg(*rd, pc.wrapping_add(4));
            ctx.set_pc(pc.wrapping_add(*offset as i64 as u64));
        }
        Instruction::Jalr { rd, rs1, offset } => {
            let pc = ctx.get_pc();
            let base = ctx.read_xreg(*rs1);
            let target = base.wrapping_add(*offset as i64 as u64) & !1; // clear bit 0
            ctx.write_xreg(*rd, pc.wrapping_add(4));
            ctx.set_pc(target);
        }
        Instruction::Ecall => {
            // Determine current privilege level and raise appropriate exception
            return Err(HartException::EnvCall);
        }
        Instruction::Mret => {
            // Restore PC and privilege from mepc/mstatus.MPP
            return Err(HartException::Mret);
        }
        // ... ~200 more match arms
    }
    Ok(())
}
```

### Main Simulation Loop

```rust
pub fn run_hart(hart: &mut Hart) {
    loop {
        // 1. Check for pending interrupts (before fetch)
        if let Some(interrupt) = hart.pending_interrupt() {
            hart.take_trap(interrupt, true);
            continue;
        }

        // 2. Fetch
        let pc = hart.pc;
        let raw = match hart.fetch_instruction(pc) {
            Ok(r) => r,
            Err(e) => { hart.take_trap(e, false); continue; }
        };

        // 3. Handle 16-bit vs 32-bit
        let (insn_bits, insn_len) = if raw & 0x3 != 0x3 {
            (raw & 0xFFFF, 2u64)  // compressed
        } else {
            (raw, 4u64)
        };

        // 4. Decode
        let insn = if insn_len == 2 {
            match decode_compressed(insn_bits as u16) {
                Ok(i) => i,
                Err(_) => { hart.take_trap(HartException::IllegalInstruction(raw), false); continue; }
            }
        } else {
            match decode_rv64(insn_bits) {
                Ok(i) => i,
                Err(_) => { hart.take_trap(HartException::IllegalInstruction(raw), false); continue; }
            }
        };

        // 5. Execute
        match execute(&insn, hart) {
            Ok(()) => {}
            Err(e) => { hart.take_trap(e, false); }
        }

        // 6. Tick counters
        hart.mcycle += 1;
        hart.minstret += 1;
    }
}
```

---

## 9. CSR Implementation

### CSR Access Rules

Before reading or writing any CSR:
1. Check that the current privilege level ≥ the privilege required by CSR address bits [9:8]
2. For writes (CSRRW, CSRRS, CSRRC, or immediate forms): check that bits [11:10] ≠ 11 (not read-only)
3. If either check fails, raise `IllegalInstruction`

CSRRS/CSRRC with rs1=x0 (or uimm=0 for immediate forms) are **read-only accesses** — even to a read-only CSR, this is valid (no write side-effect).

### Minimum CSR Set for M-mode Operation

A bare-metal M-mode environment minimally needs:

| CSR | Required | Notes |
|-----|----------|-------|
| mstatus | Yes | Global interrupt enable, privilege state |
| misa | Yes | Can be read-only constant |
| mtvec | Yes | Trap vector base |
| mepc | Yes | Exception return address |
| mcause | Yes | Trap cause |
| mtval | Yes | Trap additional info |
| mip | Yes | Interrupt pending |
| mie | Yes | Interrupt enable |
| mscratch | Yes | Scratch for trap handler |
| mhartid | Yes | Can be read-only 0 |
| mvendorid | Yes | Can be read-only 0 |
| marchid | Yes | Can be read-only 0 |
| mimpid | Yes | Can be read-only 0 |
| mcycle | Recommended | Performance counter |
| minstret | Recommended | Performance counter |
| fcsr / frm / fflags | If F/D | FP control/status |

### Additional CSRs for S-mode (OS Boot)

| CSR | Notes |
|-----|-------|
| sstatus | Restricted view of mstatus |
| stvec | S-mode trap vector |
| sepc | S-mode exception PC |
| scause | S-mode trap cause |
| stval | S-mode trap value |
| sip | S-mode interrupt pending (subset of mip) |
| sie | S-mode interrupt enable |
| satp | Virtual address translation control |
| sscratch | S-mode scratch |
| medeleg | Exception delegation to S-mode |
| mideleg | Interrupt delegation to S-mode |
| mcounteren | Controls U/S access to cycle/time/instret |
| scounteren | Controls U access to cycle/time/instret |

### CSR Dispatch Table

```rust
pub fn csr_read(hart: &Hart, addr: u16) -> Result<u64, HartException> {
    let priv_required = (addr >> 8) & 0x3;
    if hart.privilege < priv_required {
        return Err(HartException::IllegalInstruction(0));
    }

    match addr {
        // Machine-mode read/write
        0x300 => Ok(hart.mstatus),
        0x301 => Ok(hart.misa),
        0x302 => Ok(hart.medeleg),
        0x303 => Ok(hart.mideleg),
        0x304 => Ok(hart.mie),
        0x305 => Ok(hart.mtvec),
        0x306 => Ok(hart.mcounteren),
        0x340 => Ok(hart.mscratch),
        0x341 => Ok(hart.mepc),
        0x342 => Ok(hart.mcause),
        0x343 => Ok(hart.mtval),
        0x344 => Ok(hart.mip),

        // Supervisor-mode
        0x100 => Ok(hart.mstatus & SSTATUS_MASK),  // sstatus is masked mstatus
        0x104 => Ok(hart.mie & SIE_MASK),
        0x105 => Ok(hart.stvec),
        0x106 => Ok(hart.scounteren),
        0x140 => Ok(hart.sscratch),
        0x141 => Ok(hart.sepc),
        0x142 => Ok(hart.scause),
        0x143 => Ok(hart.stval),
        0x144 => Ok(hart.mip & SIP_MASK),
        0x180 => Ok(hart.satp),

        // FP
        0x001 => Ok(hart.fcsr & 0x1f),        // fflags
        0x002 => Ok((hart.fcsr >> 5) & 0x7),  // frm
        0x003 => Ok(hart.fcsr & 0xff),         // fcsr

        // Read-only machine info
        0xF11 => Ok(0),  // mvendorid
        0xF12 => Ok(0),  // marchid
        0xF13 => Ok(0),  // mimpid
        0xF14 => Ok(0),  // mhartid (hart 0)

        // Counters
        0xB00 | 0xC00 => Ok(hart.mcycle),
        0xB02 | 0xC02 => Ok(hart.minstret),
        0xC01 => Ok(hart.read_time()),   // real-time clock

        _ => Err(HartException::IllegalInstruction(0)),
    }
}

pub fn csr_write(hart: &mut Hart, addr: u16, val: u64) -> Result<(), HartException> {
    let priv_required = (addr >> 8) & 0x3;
    let read_only = (addr >> 10) & 0x3 == 0x3;

    if hart.privilege < priv_required || read_only {
        return Err(HartException::IllegalInstruction(0));
    }

    match addr {
        0x300 => hart.mstatus = val & MSTATUS_WRITABLE_MASK,
        0x302 => hart.medeleg = val,
        0x303 => hart.mideleg = val,
        0x304 => hart.mie = val,
        0x305 => hart.mtvec = val,
        0x340 => hart.mscratch = val,
        0x341 => hart.mepc = val & !1,  // always clear bit 0
        0x342 => hart.mcause = val,
        0x343 => hart.mtval = val,
        0x344 => hart.mip = val & MIP_WRITABLE_MASK,

        0x100 => hart.mstatus = (hart.mstatus & !SSTATUS_MASK) | (val & SSTATUS_MASK),
        0x104 => hart.mie = (hart.mie & !SIE_MASK) | (val & SIE_MASK),
        0x105 => hart.stvec = val,
        0x140 => hart.sscratch = val,
        0x141 => hart.sepc = val & !1,
        0x142 => hart.scause = val,
        0x143 => hart.stval = val,
        0x144 => hart.mip = (hart.mip & !SIP_MASK) | (val & SIP_MASK),
        0x180 => hart.satp = val,

        0x001 => hart.fcsr = (hart.fcsr & !0x1f) | (val & 0x1f),
        0x002 => hart.fcsr = (hart.fcsr & !0xe0) | ((val & 0x7) << 5),
        0x003 => hart.fcsr = val & 0xff,

        _ => return Err(HartException::IllegalInstruction(0)),
    }
    Ok(())
}
```

### mstatus Write Masking

Not all bits of `mstatus` are writable. Important writable field masks:

```rust
// Bits that software can write to mstatus in M-mode
const MSTATUS_WRITABLE_MASK: u64 =
    (1 << 3)  |   // MIE
    (1 << 5)  |   // SPIE
    (1 << 7)  |   // MPIE
    (1 << 8)  |   // SPP
    (3 << 11) |   // MPP
    (3 << 13) |   // FS
    (1 << 17) |   // MPRV
    (1 << 18) |   // SUM
    (1 << 19) |   // MXR
    (1 << 20) |   // TVM
    (1 << 21) |   // TW
    (1 << 22) |   // TSR
    (1 << 1)  |   // SIE
    (1 << 6);     // UBE

// sstatus is a restricted view of mstatus
const SSTATUS_MASK: u64 =
    (1 << 1)  |   // SIE
    (1 << 5)  |   // SPIE
    (1 << 6)  |   // UBE
    (1 << 8)  |   // SPP
    (3 << 13) |   // FS
    (3 << 15) |   // XS
    (1 << 18) |   // SUM
    (1 << 19) |   // MXR
    (1u64 << 63); // SD
```

---

## 10. Compressed Instructions (C Extension)

### Detection

Check bits [1:0] of the halfword at PC. All values except `11` are compressed:

```rust
let halfword = mem.load_u16(pc)?;
if halfword & 0x3 != 0x3 {
    // 16-bit compressed instruction
    let expanded = decode_compressed(halfword)?;
    // execute expanded; advance PC by 2
} else {
    // 32-bit instruction: also load upper halfword
    let fullword = (mem.load_u16(pc + 2)? as u32) << 16 | halfword as u32;
    let insn = decode_rv64(fullword)?;
    // execute insn; advance PC by 4
}
```

### C Register Encoding

Many C instructions use a 3-bit register field (CL/CS/CB/CIW formats) encoding only 8 registers: x8–x15 (s0–s1, a0–a5):

```rust
fn c_reg(bits3: u16) -> Reg {
    (bits3 + 8) as Reg  // 0b000=x8, 0b001=x9, ..., 0b111=x15
}
```

Full-width (5-bit) register encodings appear in CI, CR, and CSS formats.

### C Instruction Quadrants

Bits [15:13] (funct3) and bits [1:0] (opcode, always 00/01/10 for compressed) determine the instruction:

**Quadrant 0 (op=00):**

| funct3 | Instruction | Description |
|--------|-------------|-------------|
| 000 | C.ADDI4SPN | rd'= sp + nzuimm×4 |
| 010 | C.LW | rd'= mem[rs1'+offset] (word) |
| 011 | C.LD | rd'= mem[rs1'+offset] (dword, RV64) |
| 110 | C.SW | mem[rs1'+offset] = rs2' (word) |
| 111 | C.SD | mem[rs1'+offset] = rs2' (dword, RV64) |

**Quadrant 1 (op=01):**

| funct3 | Instruction | Description |
|--------|-------------|-------------|
| 000 | C.ADDI | rd += nzimm (nonzero imm, else C.NOP) |
| 001 | C.ADDIW | rd = (rd+imm) sign-extended to 32 bits (RV64) |
| 010 | C.LI | rd = imm |
| 011 | C.ADDI16SP / C.LUI | sp += imm×16, or rd = imm<<12 |
| 100 | C.SRLI, C.SRAI, C.ANDI, C.SUB/XOR/OR/AND | various |
| 101 | C.J | PC += offset |
| 110 | C.BEQZ | if rs1'==0: PC += offset |
| 111 | C.BNEZ | if rs1'!=0: PC += offset |

**Quadrant 2 (op=10):**

| funct3 | Instruction | Description |
|--------|-------------|-------------|
| 000 | C.SLLI | rd <<= uimm |
| 010 | C.LWSP | rd = mem[sp+offset] (word) |
| 011 | C.LDSP | rd = mem[sp+offset] (dword, RV64) |
| 100 | C.JR / C.MV / C.EBREAK / C.JALR / C.ADD | see below |
| 110 | C.SWSP | mem[sp+offset] = rs2 (word) |
| 111 | C.SDSP | mem[sp+offset] = rs2 (dword, RV64) |

Quadrant 2, funct3=100 decodes as:
- bit 12=0, rs1≠0, rs2=0: C.JR (jalr x0, 0(rs1))
- bit 12=0, rs1≠0, rs2≠0: C.MV (add rd, x0, rs2)
- bit 12=1, rs1=0, rs2=0: C.EBREAK
- bit 12=1, rs1≠0, rs2=0: C.JALR (jalr ra, 0(rs1))
- bit 12=1, rs1≠0, rs2≠0: C.ADD (add rd, rd, rs2)

### C-to-32-bit Expansion

The recommended implementation approach: decode a C instruction by directly producing the equivalent 32-bit `Instruction` enum variant. No separate compressed execution path is needed.

```rust
pub fn decode_compressed(raw: u16) -> Result<Instruction, DecodeError> {
    let op = raw & 0x3;
    let funct3 = (raw >> 13) & 0x7;

    match (op, funct3) {
        // C.ADDI4SPN: rd'= x2 + nzuimm×4
        (0b00, 0b000) => {
            let nzuimm = ((raw >> 6) & 0x1) << 2
                | ((raw >> 5) & 0x1) << 3
                | ((raw >> 11) & 0x3) << 4
                | ((raw >> 7) & 0xf) << 6;
            let rd = c_reg((raw >> 2) & 0x7);
            if nzuimm == 0 { return Err(DecodeError::IllegalInstruction(raw as u32)); }
            Ok(Instruction::Addi { rd, rs1: 2, imm: nzuimm as i32 })
        }

        // C.LW: rd'= mem[rs1'+offset]
        (0b00, 0b010) => {
            let offset = ((raw >> 6) & 0x1) << 2
                | ((raw >> 10) & 0x7) << 3
                | ((raw >> 5) & 0x1) << 6;
            let rd  = c_reg((raw >> 2) & 0x7);
            let rs1 = c_reg((raw >> 7) & 0x7);
            Ok(Instruction::Lw { rd, rs1, offset: offset as i32 })
        }

        // C.LD (RV64): rd'= mem[rs1'+offset]
        (0b00, 0b011) => {
            let offset = ((raw >> 10) & 0x7) << 3
                | ((raw >> 5) & 0x3) << 6;
            let rd  = c_reg((raw >> 2) & 0x7);
            let rs1 = c_reg((raw >> 7) & 0x7);
            Ok(Instruction::Ld { rd, rs1, offset: offset as i32 })
        }

        // C.ADDI: rd = rd + nzimm
        (0b01, 0b000) => {
            let rd = ((raw >> 7) & 0x1f) as Reg;
            let nzimm = ((raw >> 2) & 0x1f) | (((raw >> 12) & 0x1) << 5);
            let imm = ((nzimm as i16) << 10) >> 10; // sign-extend from bit 5
            Ok(Instruction::Addi { rd, rs1: rd, imm: imm as i32 })
        }

        // C.J: unconditional jump
        (0b01, 0b101) => {
            let offset = decode_cj_offset(raw);
            Ok(Instruction::Jal { rd: 0, offset }) // rd=x0 (discard link)
        }

        // C.BEQZ
        (0b01, 0b110) => {
            let rs1 = c_reg((raw >> 7) & 0x7);
            let offset = decode_cb_offset(raw);
            Ok(Instruction::Beq { rs1, rs2: 0, offset })
        }

        // C.BNEZ
        (0b01, 0b111) => {
            let rs1 = c_reg((raw >> 7) & 0x7);
            let offset = decode_cb_offset(raw);
            Ok(Instruction::Bne { rs1, rs2: 0, offset })
        }

        // ... remaining quadrant 2 ...

        _ => Err(DecodeError::IllegalInstruction(raw as u32)),
    }
}
```

---

## 11. Testing RISC-V Implementation

### riscv-tests Suite

The official `riscv/riscv-tests` repository contains assembly tests that run on bare metal. Each test:
1. Loads a known binary into memory at a fixed address
2. Executes it
3. Checks `tohost` memory location (at a well-known address) for pass/fail signal

Test naming convention:
- `rv64ui-p-*` — RV64I user-level integer, physical memory (no MMU)
- `rv64um-p-*` — RV64 M extension
- `rv64ua-p-*` — RV64 A extension
- `rv64ud-p-*` — RV64 D (double-precision FP)
- `rv64uc-p-*` — RV64 C (compressed)

Running the tests with a simulator (example `tohost` polling loop):

```rust
// After executing each instruction, check if simulation should stop
const TOHOST_ADDR: u64 = 0x8000_1000; // platform-specific

fn check_tohost(mem: &Memory) -> Option<u64> {
    let val = mem.load_u64(TOHOST_ADDR).ok()?;
    if val != 0 { Some(val) } else { None }
}

// In main loop:
if let Some(tohost) = check_tohost(&hart.mem) {
    if tohost == 1 {
        println!("PASS");
    } else {
        println!("FAIL: test number {}", tohost >> 1);
    }
    break;
}
```

### Spike as Oracle

Spike (the official RISC-V ISA reference simulator) can be used to validate correct behavior by comparing register state at each instruction:

```bash
# Install spike and pk (proxy kernel)
# Run a binary with logging
spike --log-commits rv64gc binary.elf 2>spike.log

# spike --log-commits format (one line per instruction):
# core   0: 0x0000000080000000 (0x00000297) auipc t0, 0x0
# core   0: 3 0x0000000080000000 (0x00000297) x5  0x0000000080000000
```

A differential testing harness can step both Spike and helm-ng one instruction at a time and compare all register values after each step:

```rust
// Pseudocode for differential testing
fn diff_test(binary: &[u8]) {
    let mut spike = SpikeProcess::spawn(binary);
    let mut helm  = HelmHart::new(binary);

    loop {
        spike.step();
        helm.step();

        for reg in 0..32 {
            assert_eq!(spike.xreg(reg), helm.xreg(reg),
                "Divergence at PC={:#x}: x{} spike={:#x} helm={:#x}",
                helm.pc(), reg, spike.xreg(reg), helm.xreg(reg));
        }
        assert_eq!(spike.pc(), helm.pc(), "PC divergence");
    }
}
```

Spike exposes a C++ library interface (`libspike_main`) or can be driven via its remote bitbang interface.

### RISCOF Compliance Framework

RISCOF (RISC-V Compliance Framework) is the official compliance test infrastructure:

```bash
pip install riscof
riscof setup --refconfig spike_config.yaml --dutconfig helm_config.yaml
riscof run --config config.ini --suite riscv-arch-test/riscv-test-suite/ --env riscv-arch-test/riscv-test-suite/env/
```

RISCOF runs a test suite, collects signature memory regions from both the reference model (Spike) and the DUT (helm-ng), and diffs them. Passing RISCOF achieves official RISC-V compliance certification eligibility.

DUT plugin structure:

```python
# helm_plugin.py — minimal RISCOF DUT plugin
class helm_ng(pluginTemplate):
    def __init__(self):
        super().__init__()
        self.name = "helm-ng"

    def initialise(self, suite, workdir, archtest_env):
        self.work_dir = workdir

    def build(self, isa_yaml, platform_yaml):
        # compile the test with riscv-gcc
        pass

    def runTests(self, testlist):
        for test in testlist:
            # run helm-ng on the compiled ELF
            subprocess.run(["helm-ng", "--elf", test['elf'],
                           "--signature-output", test['signature']])
```

### Common Implementation Bugs

#### 1. LW Sign Extension (Most Common)

`LW` loads a 32-bit value and **sign-extends** to 64 bits. `LWU` zero-extends.

```rust
// WRONG:
let val = ctx.load_u32(addr)? as u64;  // zero-extends! Wrong for LW

// CORRECT:
let val = ctx.load_u32(addr)? as i32 as i64 as u64;  // sign-extends
```

Same issue applies to `LH`/`LB` (vs `LHU`/`LBU`) and all the `*W` operations.

#### 2. ADDW/SUBW/etc. Sign Extension

The `*W` instructions operate on 32-bit values and sign-extend the result:

```rust
// WRONG:
let val = (ctx.read_xreg(rs1) as u32).wrapping_add(ctx.read_xreg(rs2) as u32) as u64;

// CORRECT:
let val = (ctx.read_xreg(rs1) as u32).wrapping_add(ctx.read_xreg(rs2) as u32) as i32 as i64 as u64;
```

#### 3. PC Alignment with C Extension

Without C extension: all instruction addresses must be 4-byte aligned. With C extension: 2-byte alignment is required. A misaligned branch target raises `InstructionAddressMisaligned` (mcause=0).

```rust
fn check_pc_alignment(pc: u64, has_c: bool) -> Result<(), HartException> {
    let align = if has_c { 2 } else { 4 };
    if pc % align != 0 {
        Err(HartException::InstructionAddressMisaligned(pc))
    } else {
        Ok(())
    }
}
```

#### 4. Reservation Set Invalidation on SC Failure

`SC.W`/`SC.D` must **always** clear the reservation, even when it fails:

```rust
// WRONG — only clears on success:
if hart.reservation == Some(addr) {
    hart.reservation = None;
    mem.store_u32(addr, val)?;
    hart.x[rd] = 0;
} else {
    hart.x[rd] = 1;
    // BUG: reservation not cleared!
}

// CORRECT:
let success = hart.reservation == Some(addr);
hart.reservation = None; // ALWAYS clear first
if success { /* store */ hart.x[rd] = 0; } else { hart.x[rd] = 1; }
```

#### 5. MRET Must Set MPIE=1, Not Restore It

After MRET:
- `mstatus.MIE` ← `mstatus.MPIE`
- `mstatus.MPIE` ← **1** (not 0, not the old MIE)
- `mstatus.MPP` ← least-privileged mode (U if U-mode supported)

```rust
fn exec_mret(hart: &mut Hart) {
    let mpie = (hart.mstatus >> 7) & 1;
    let mpp  = (hart.mstatus >> 11) & 3;

    // Restore MIE from MPIE
    hart.mstatus &= !(1 << 3);
    hart.mstatus |= mpie << 3;

    // Set MPIE to 1
    hart.mstatus |= 1 << 7;

    // Set MPP to U (or least privileged)
    hart.mstatus &= !(3 << 11);
    // If U-mode not supported, MPP stays M (11). Set to 0 if U supported.

    // Restore privilege and PC
    hart.privilege = mpp as u8;
    hart.pc = hart.mepc;
}
```

#### 6. ECALL Must Advance mepc Before Returning

When a trap handler for ECALL executes MRET, `mepc` still points to the ECALL instruction. The handler must explicitly advance it:

```asm
# In M-mode trap handler:
csrr t0, mepc
addi t0, t0, 4    # skip the ECALL instruction
csrw mepc, t0
# ... handle the syscall ...
mret
```

A simulator implementing a built-in ECALL handler (not delegating to a real trap handler) should automatically advance `mepc` by 4.

#### 7. misa Read-Only or Carefully Writable

`misa` reports the supported ISA. For RV64GC:

```rust
const MISA_RV64GC: u64 =
    (2u64 << 62)    | // MXL=2 (RV64)
    (1 << ('I' - 'A')) |
    (1 << ('M' - 'A')) |
    (1 << ('A' - 'A')) |
    (1 << ('F' - 'A')) |
    (1 << ('D' - 'A')) |
    (1 << ('C' - 'A'));
```

Writes to `misa` can be ignored (treat as read-only) for a fixed-ISA simulator.

#### 8. JALR Target Bit-0 Must Be Cleared

JALR clears the LSB of the computed target address. This is specified in the ISA:

```rust
let target = base.wrapping_add(offset as i64 as u64) & !1u64;
```

#### 9. Shift Amount Masking

For 64-bit shifts, the shift amount is `rs2 & 63`. For 32-bit shifts (`SLLW`, `SRLW`, `SRAW`), it is `rs2 & 31`. The ISA requires this masking; it mirrors what x86 does but differs from naive Rust behavior on shifts ≥ 64.

```rust
// SLLI rd, rs1, shamt  (shamt is 6-bit in encoding)
let result = ctx.read_xreg(rs1) << (shamt & 63);

// SLL rd, rs1, rs2
let result = ctx.read_xreg(rs1) << (ctx.read_xreg(rs2) & 63);

// SLLW rd, rs1, rs2 (32-bit)
let result = (ctx.read_xreg(rs1) as u32) << (ctx.read_xreg(rs2) & 31);
// then sign-extend to 64 bits
```

#### 10. Division Edge Cases (M Extension)

RISC-V integer division has defined behavior for corner cases that would be UB in C:

| Operation | Inputs | Result |
|-----------|--------|--------|
| DIV/DIVU | divisor=0 | -1 (all ones) |
| REM/REMU | divisor=0 | dividend |
| DIV | MIN_INT / -1 | MIN_INT (overflow defined) |
| REM | MIN_INT / -1 | 0 |

```rust
fn exec_div(rs1_val: i64, rs2_val: i64) -> i64 {
    if rs2_val == 0 {
        -1i64  // defined: all bits set
    } else if rs1_val == i64::MIN && rs2_val == -1 {
        i64::MIN  // defined overflow
    } else {
        rs1_val.wrapping_div(rs2_val)
    }
}
```

---

## References

- [RISC-V Unprivileged ISA Specification](https://github.com/riscv/riscv-isa-manual) — Volume I
- [RISC-V Privileged Architecture Specification](https://github.com/riscv/riscv-isa-manual) — Volume II
- [riscv-tests](https://github.com/riscv-software-src/riscv-tests) — Official ISA test suite
- [Spike RISC-V ISA Simulator](https://github.com/riscv-software-src/riscv-isa-sim) — Reference implementation
- [RISCOF](https://github.com/riscv-software-src/riscof) — Compliance testing framework
- [riscv-arch-test](https://github.com/riscv-non-isa/riscv-arch-test) — Architecture compliance test suite
- [QEMU RISC-V virt machine](https://www.qemu.org/docs/master/system/riscv/virt.html) — Reference platform for memory map
- [SiFive E51 Core Manual](https://sifive.cdn.prismic.io/sifive/c26e6c3c-45a5-4e7c-9e24-a25b93e35f5d_e51_core_manual.pdf) — CLINT/PLIC reference implementation
