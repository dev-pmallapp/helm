# ARM Implementation

HELM's primary target ISA. AArch64 (ARMv8-A) SE mode is implemented and
running real binaries including fish-shell. AArch32 and ARMv9 extensions
are future work.

---

## Current Implementation Status

### AArch64 Executor (`helm-isa`)

`Aarch64Cpu` in `crates/helm-isa/src/arm/aarch64/exec.rs` provides a
direct-execution SE-mode CPU with full instruction coverage:

| Group | Instructions |
|-------|-------------|
| **Integer ALU** | ADD/ADDS/SUB/SUBS (imm/reg/ext), AND/ORR/EOR/BIC/ORN (imm/reg/shifted), TST, MOV, MVN |
| **Shift / bitfield** | LSL/LSR/ASR (imm), UBFM, SBFM, BFM, EXTR |
| **Multiply** | MUL, MADD, MSUB, SMULL, SMULH, UMULH, MNeg |
| **Divide** | UDIV, SDIV |
| **Wide moves** | MOVZ, MOVN, MOVK (all hw shifts) |
| **Address** | ADR, ADRP |
| **Compare / select** | CMP, CMN, CSEL, CSET, CSINC, CSINV, CSNEG |
| **Carry** | ADC, ADCS, SBC, SBCS |
| **Conditional** | B.cond, CBZ, CBNZ, TBZ, TBNZ |
| **Branches** | B, BL, BR, BLR, RET |
| **Load/store** | LDR/STR (imm/reg/pre/post-index/literal), LDP/STP |
| **Load/store ext** | LDRB/LDRH/LDRSB/LDRSH/LDRSW, STRB/STRH (all modes) |
| **Load/store unscaled** | LDUR/STUR and byte/half/signed variants |
| **Exclusive** | LDXR, STXR, LDAXR, STLXR (32/64-bit) |
| **SIMD** | FMOV, FADD, FSUB, FMUL, FDIV, FCMP, FCSEL, FCVT, FABS, FNEG, FSQRT; integer SIMD (ADD, SUB, MUL, AND, ORR, EOR, shift vectors) |
| **System** | SVC #0, NOP, MRS/MSR (NZCV, TPIDR_EL0, FPSR/FPCR), ISB/DSB/DMB |

**Register file** (`Aarch64Regs`):
- X0-X30 (64-bit GP), XZR (read-as-zero / writes discarded)
- SP (`xn_sp(31)` / `set_xn_sp(31)` → SP, not XZR)
- PC, NZCV (N/Z/C/V flags)
- V0-V31 (128-bit SIMD/FP), FPCR, FPSR
- TPIDR_EL0 (thread-local storage base)

### AArch64 Decoder (`helm-isa`)

`Aarch64Decoder` in `decode.rs` is a stub that emits a NOP micro-op for
every instruction. It is only used in the CAE path (ISA frontend →
`Vec<MicroOp>`). The SE path uses `Aarch64Cpu.step()` directly.

### Syscall Emulation (`helm-syscall`)

`Aarch64SyscallHandler` handles ~50 Linux AArch64 syscalls via libc
passthrough. Key syscall numbers:

| Nr  | Name            | Nr  | Name          |
|-----|-----------------|-----|---------------|
| 25  | fcntl           | 64  | write         |
| 29  | ioctl           | 73  | ppoll         |
| 56  | openat          | 93  | exit          |
| 57  | close           | 94  | exit_group    |
| 63  | read            | 113 | clock_gettime |
| 160 | uname           | 214 | brk           |
| 172 | getpid          | 222 | mmap          |
| 134 | rt_sigaction    | 278 | getrandom     |

The `FdTable` in `fd_table.rs` tracks open file descriptors and remaps
them for read/write/close.

### ELF Loader (`helm-engine`)

`load_elf()` handles static AArch64 ELF64 binaries:
1. Validate magic, EI_CLASS=2 (ELF64), EI_DATA=1 (LE), e_machine=183 (AArch64).
2. Walk `PT_LOAD` segments, map each into `AddressSpace`.
3. Build the initial stack: argc, argv, envp, auxiliary vector
   (AT_PHDR, AT_PHENT, AT_PHNUM, AT_PAGESZ, AT_ENTRY, AT_RANDOM).
4. Set PC = `e_entry`, SP = top of stack.

Limitations: static only (no dynamic linker), AArch64 only.

---

## AArch64 Instruction Encoding Groups

Bit positions [28:25] of every A64 instruction:

| Bits [28:25] | Group |
|:-------------|-------|
| `100x` | Data processing — immediate |
| `101x` | Branches, exceptions, system |
| `x1x0` | Loads and stores |
| `x101` | Data processing — register |
| `0111` | SIMD / floating-point |

---

## Syscall Convention (AArch64 Linux)

- Syscall number: **X8**
- Arguments: **X0–X5**
- Return value: **X0** (negative = `-errno`)
- Invocation: **SVC #0**

---

## Test Coverage

Tests live in `crates/helm-isa/src/arm/aarch64/tests/`:

| File | Coverage |
|------|---------|
| `exec.rs` | Core ALU: ADD, SUB, CMP, MOVZ, MOVZ+MOVK, ADRP |
| `exec_dp_imm.rs` | All immediate data-processing encodings |
| `exec_dp_reg.rs` | All register data-processing encodings |
| `exec_branch.rs` | B, BL, BLR, RET, B.cond, CBZ, CBNZ, TBZ, TBNZ |
| `exec_ldst.rs` | LDR/STR/LDP/STP all modes, LDRB/LDRH/LDRSB/LDRSW |
| `exec_ldst_bulk.rs` | Systematic load/store coverage matrix |
| `exec_flags.rs` | Flag-setting instructions, NZCV correctness |
| `exec_multiply.rs` | MUL, MADD, MSUB, SMULL, SMULH, UMULH |
| `exec_simd.rs` | FMOV, FADD/FSUB/FMUL/FDIV, FCMP, integer SIMD |
| `exec_corner_cases.rs` | XZR disambiguation, 32-bit zero-extension, LDUR/STUR offsets |
| `exec_cpu.rs` | Aarch64Cpu methods: xn/xzr, sp, wn, step() |
| `exec_parametric.rs` | Systematic parametric instruction matrix |
| `exec_bulk.rs` | Large-scale encoding coverage |
| `decode.rs` | Aarch64Decoder: NOP stub path |

---

## Roadmap

### AArch32 (ARMv7-A) — Stage 1

- `Aarch32Regs`: R0-R15 (R13=SP, R14=LR, R15=PC), CPSR.
- A32 instruction set (32-bit fixed width) + T32 Thumb-2 (16/32-bit mixed).
- Condition codes on every A32 instruction.
- ELF32 loader.
- ARMv7 Linux syscall table (different numbers from AArch64).

### ARMv9-A Extensions — Stage 2

- **SVE2**: Variable-length vector registers (VL up to 2048 bits), predicate
  registers, gather/scatter loads.
- **MTE**: Memory tagging — tag bits in pointers, checked on every access.
- **BTI/PAC**: Branch Target Identification and Pointer Authentication — can
  be stubbed (NOP) for SE mode.
- **SME**: Scalable Matrix Extension — matrix operations on ZA storage.

### CAE Decoder — Future

Wire `helm-decode` (QEMU `.decode` file parser) into `Aarch64Decoder`
to generate structured `MicroOp` sequences from the A64 encoding rather
than the current NOP stub. See [decode-tree.md](decode-tree.md) and
[proposals.md §A3](proposals.md).
