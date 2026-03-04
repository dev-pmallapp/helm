# ARM Implementation Guide

Full bring-up plan for ARMv7-A (AArch32), ARMv8-A (AArch64), and
ARMv9-A in HELM, targeting static-binary syscall-emulation (SE) mode
as the first milestone.

## Stage 0 — Goal

Run a statically-linked AArch64 "hello world" binary in SE mode at
Express accuracy.  Every step is TDD: write the test first, watch it
fail, implement, watch it pass.

---

## 1. Architecture Variants

### ARMv7-A (AArch32)

- 32-bit ARM and Thumb instruction sets.
- 16 general-purpose registers (R0-R15), CPSR.
- VFPv3/v4, NEON (optional).
- Condition codes on (nearly) every instruction.
- Coprocessor interface (CP15 for system control).

### ARMv8-A (AArch64)

- 64-bit execution state with fixed-width 32-bit instructions.
- 31 general-purpose registers (X0-X30), SP, PC, PSTATE.
- SIMD/FP via 32x 128-bit V registers.
- Exception levels EL0-EL3 (SE mode runs at EL0).
- A64 instruction set — no condition codes on most instructions.
- Can also run AArch32 code (interprocessing).

### ARMv9-A

- Superset of ARMv8-A.
- Scalable Vector Extension 2 (SVE2) — variable-length vectors.
- Scalable Matrix Extension (SME).
- Memory Tagging Extension (MTE).
- Realm Management Extension (RME).
- For SE mode the immediate concern is SVE2; the rest is system-level.

---

## 2. Register File

```rust
// crates/helm-isa/src/arm/regs.rs

/// AArch64 register file for SE mode (EL0).
pub struct Aarch64Regs {
    /// General-purpose registers X0-X30.
    pub x: [u64; 31],
    /// Stack pointer (SP_EL0).
    pub sp: u64,
    /// Program counter.
    pub pc: u64,
    /// Condition flags (NZCV) packed into a u32.
    pub nzcv: u32,
    /// SIMD/FP registers V0-V31 (128-bit each).
    pub v: [u128; 32],
    /// Floating-point status register.
    pub fpcr: u32,
    pub fpsr: u32,
    /// Thread-local storage base (TPIDR_EL0).
    pub tpidr_el0: u64,
}
```

For AArch32 (ARMv7):

```rust
pub struct Aarch32Regs {
    pub r: [u32; 16],   // R0-R15 (R13=SP, R14=LR, R15=PC)
    pub cpsr: u32,
    pub d: [u64; 32],   // VFP/NEON D0-D31
    pub fpscr: u32,
}
```

---

## 3. Instruction Decoding — AArch64

AArch64 instructions are fixed 32-bit.  Top bits select the encoding
group:

| Bits [28:25] | Group |
|--------------|-------|
| `100x` | Data processing — immediate |
| `101x` | Branches, exceptions, system |
| `x1x0` | Loads and stores |
| `x101` | Data processing — register |
| `0111` | Data processing — SIMD/FP |

### Stage-0 instruction subset (enough for hello-world)

| Category | Instructions |
|----------|-------------|
| **Integer ALU** | ADD, SUB, AND, ORR, EOR, MOV, MOVZ, MOVK |
| **Shift/bitfield** | LSL, LSR, ASR, UBFM, SBFM |
| **Multiply** | MUL, MADD, SMULL |
| **Compare** | CMP, CMN, TST |
| **Branch** | B, BL, BR, BLR, RET, B.cond, CBZ, CBNZ, TBZ, TBNZ |
| **Load/store** | LDR, STR, LDP, STP (imm, reg, pre/post-index) |
| **Load/store** | LDRB, LDRH, LDRSB, LDRSH, LDRSW, STRB, STRH |
| **Address** | ADR, ADRP |
| **System** | SVC (syscall), NOP, MRS (TPIDR_EL0), MSR |

### Decoder structure

```
crates/helm-isa/src/arm/
    mod.rs            ArmFrontend (dispatches AArch32 vs AArch64)
    regs.rs           register files
    aarch64/
        mod.rs        top-level A64 decoder
        decode.rs     bit-pattern matching
        alu.rs        data-processing instructions
        branch.rs     branches and conditions
        ldst.rs       loads and stores
        system.rs     SVC, MRS, MSR, NOP
    aarch32/
        mod.rs        top-level A32/T32 decoder (later)
```

---

## 4. Syscall Emulation — AArch64 Linux

AArch64 Linux syscall convention:

- Syscall number in **X8**.
- Arguments in **X0-X5**.
- Return value in **X0** (negative = -errno).
- Invoked via **SVC #0**.

### Stage-0 syscall subset

| Nr | Name | Args | Notes |
|----|------|------|-------|
| 56 | `openat` | dirfd, path, flags, mode | AT_FDCWD = -100 |
| 57 | `close` | fd | |
| 63 | `read` | fd, buf, count | |
| 64 | `write` | fd, buf, count | stdout/stderr |
| 93 | `exit` | status | terminate |
| 94 | `exit_group` | status | terminate |
| 96 | `set_tid_address` | tidptr | return fake tid |
| 122 | `sched_yield` | | nop |
| 160 | `uname` | buf | fill sysname etc. |
| 172 | `getpid` | | return 1 |
| 174 | `getuid` | | return 0 |
| 175 | `geteuid` | | return 0 |
| 176 | `getgid` | | return 0 |
| 177 | `getegid` | | return 0 |
| 214 | `brk` | addr | heap management |
| 215 | `munmap` | addr, len | |
| 222 | `mmap` | addr, len, prot, flags, fd, off | anon only initially |
| 226 | `mprotect` | addr, len, prot | nop |
| 261 | `prlimit64` | pid, resource, new, old | stub RLIMIT_STACK |
| 278 | `getrandom` | buf, len, flags | fill with zeros |

This set is sufficient for musl-libc static binaries.

---

## 5. ELF Loader

The loader must handle static AArch64 ELF binaries:

1. Parse ELF64 header, verify `e_machine == EM_AARCH64` (183).
2. Walk `PT_LOAD` segments, `mmap` each into the guest `AddressSpace`.
3. Set up the initial stack:
   - argc, argv pointers, envp pointers, auxiliary vector.
   - AT_PHDR, AT_PHENT, AT_PHNUM, AT_PAGESZ, AT_ENTRY, AT_RANDOM.
4. Set PC = `e_entry`, SP = top of stack.

```
High address
    ┌────────────────────┐
    │  AT_NULL           │  auxiliary vector
    │  AT_RANDOM (16 B)  │
    │  AT_ENTRY          │
    │  AT_PAGESZ (4096)  │
    │  AT_PHNUM          │
    │  AT_PHENT          │
    │  AT_PHDR           │
    ├────────────────────┤
    │  NULL              │  envp terminator
    │  NULL              │  argv terminator
    │  argv[0] ptr       │
    │  argc              │  <-- SP
    └────────────────────┘
Low address
```

---

## 6. TDD Stage-0 Test Plan

Every item below is a test that must exist **before** the implementation.
Tests live in `crates/helm-isa/src/tests/arm/`.

### 6.1 Register file

```
test_x_regs_init_to_zero
test_sp_independent_of_x31
test_nzcv_pack_unpack
test_pc_advances_by_four
```

### 6.2 Decoder — data processing

```
test_decode_add_imm         ADD X0, X1, #42
test_decode_sub_imm         SUB X0, X1, #1
test_decode_movz            MOVZ X0, #0x1234
test_decode_movk            MOVK X0, #0x5678, LSL #16
test_decode_and_reg         AND X0, X1, X2
test_decode_orr_reg         ORR X0, X1, X2
test_decode_lsl_imm         LSL X0, X1, #4
test_decode_cmp_imm         CMP X1, #0
```

### 6.3 Decoder — branches

```
test_decode_b_imm           B #offset
test_decode_bl              BL #offset
test_decode_br              BR X30
test_decode_blr             BLR X1
test_decode_ret             RET (alias of BR X30)
test_decode_b_cond_eq       B.EQ #offset
test_decode_cbz             CBZ X0, #offset
test_decode_cbnz            CBNZ X0, #offset
```

### 6.4 Decoder — loads and stores

```
test_decode_ldr_imm         LDR X0, [X1, #8]
test_decode_str_imm         STR X0, [X1, #8]
test_decode_ldr_pre_index   LDR X0, [X1, #8]!
test_decode_str_post_index  STR X0, [X1], #8
test_decode_ldp             LDP X0, X1, [SP, #16]
test_decode_stp             STP X0, X1, [SP, #-16]!
test_decode_ldrb            LDRB W0, [X1]
test_decode_strb            STRB W0, [X1]
test_decode_adrp            ADRP X0, #page
```

### 6.5 Decoder — system

```
test_decode_svc             SVC #0
test_decode_nop             NOP
test_decode_mrs_tpidr       MRS X0, TPIDR_EL0
```

### 6.6 Execution — ALU

```
test_exec_add_imm           X0=1, X1=2, ADD => X0=3
test_exec_sub_sets_flags    SUB + CMP sets NZCV
test_exec_movz_movk_chain  MOVZ+MOVK builds 64-bit constant
test_exec_and_orr_eor       bitwise ops
```

### 6.7 Execution — branches

```
test_exec_b_forward         PC jumps forward
test_exec_bl_saves_lr       X30 = return address
test_exec_ret_to_lr         PC = X30
test_exec_beq_taken         Z=1 -> taken
test_exec_beq_not_taken     Z=0 -> fallthrough
test_exec_cbz_zero          X0=0 -> taken
```

### 6.8 Execution — load/store

```
test_exec_str_ldr_roundtrip write then read same address
test_exec_stp_ldp_pair      store pair, load pair
test_exec_ldrb_zero_extends byte load zero-extends to 64-bit
test_exec_ldrsb_sign_extends signed byte load sign-extends
```

### 6.9 Syscall emulation

```
test_syscall_write_stdout   write(1, "hello", 5) returns 5
test_syscall_exit            exit(0) halts core
test_syscall_brk             brk(0) returns current break
test_syscall_mmap_anon       mmap anonymous returns valid addr
test_syscall_uname           uname fills sysname="Linux"
test_syscall_getpid          getpid returns nonzero
```

### 6.10 ELF loader

```
test_load_aarch64_elf       parse PT_LOAD, set entry point
test_stack_layout            argc/argv/auxv on stack
test_reject_non_arm_elf      x86 ELF returns error
```

### 6.11 End-to-end

```
test_hello_world_aarch64    run a real static musl hello-world binary,
                            capture write(1, ...) output, verify "Hello"
```

---

## 7. Directory Layout After Stage 0

```
crates/helm-isa/src/arm/
    mod.rs
    regs.rs
    aarch64/
        mod.rs
        decode.rs
        alu.rs
        branch.rs
        ldst.rs
        system.rs
    aarch32/          (stub — stage 1)
        mod.rs

crates/helm-isa/src/tests/
    arm.rs            existing smoke tests
    arm_regs.rs       register-file tests
    arm_decode.rs     decoder tests (6.2-6.5)
    arm_exec.rs       execution tests (6.6-6.8)

crates/helm-syscall/src/
    table.rs          add AArch64 syscall numbers
    handler.rs        add AArch64-specific handlers
    aarch64.rs        extended syscall table

crates/helm-syscall/src/tests/
    aarch64.rs        syscall tests (6.9)

crates/helm-engine/src/
    loader.rs         proper ELF parser

crates/helm-engine/src/tests/
    loader.rs         ELF loader tests (6.10)
    e2e_arm.rs        end-to-end tests (6.11)

tests/
    fixtures/
        hello-aarch64  pre-compiled static binary (musl)
```

---

## 8. Stage 1 — ARMv7 (AArch32)

After AArch64 SE mode works:

1. Add `Aarch32Regs` and `Aarch32Decoder`.
2. A32 instruction set (fixed 32-bit) + T32 Thumb-2 (16/32-bit mixed).
3. Condition codes on every A32 instruction (4-bit cond field).
4. ARMv7 Linux syscall table (different numbers from AArch64).
5. ELF32 loader.
6. Same TDD approach: decoder tests first, then execution, then syscalls.

---

## 9. Stage 2 — ARMv9 Extensions

After ARMv8 baseline is solid:

1. **SVE2:** Variable-length vector registers (VL up to 2048 bits),
   predicate registers, gather/scatter loads, per-lane predication.
2. **MTE:** Memory tagging — tag bits in pointers and memory, checked
   on every access.
3. **BTI/PAC:** Branch Target Identification and Pointer Authentication
   — can be stubbed (NOP) for SE mode.

---

## 10. Validation Targets

| Milestone | Binary | Expected Output |
|-----------|--------|-----------------|
| Stage 0a | `hello-aarch64` (musl static) | `Hello, world!\n` on fd 1 |
| Stage 0b | `fib-aarch64` (compute fib(30)) | correct result in X0 |
| Stage 0c | musl `busybox echo hello` | `hello\n` |
| Stage 1a | `hello-arm32` (musl static) | `Hello, world!\n` |
| Stage 2a | SVE2 vector add test | correct sum |

---

## 11. Implementation Priorities

1. **AArch64 decoder** — the A64 fixed-width encoding is the cleanest
   starting point.
2. **Syscall table** — small subset above is enough for musl libc init.
3. **ELF loader** — real ELF64 parsing (replace the current stub).
4. **End-to-end test** — compile a static hello-world with musl,
   commit the binary as a test fixture.
5. **ARMv7** — add A32/T32 decoder and 32-bit syscall table.
6. **ARMv9 SVE2** — extend the register file and decoder.
