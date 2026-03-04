# ARM Syscall-Emulation Implementation

Detailed implementation guide for running statically-linked AArch64
binaries (including fish-shell) in HELM's SE mode.

---

## 1. Target Binary Profile

**fish-shell** is a user-friendly interactive shell.  A static musl
build produces a single ~1.5 MB ELF64 binary that exercises:

- Full integer ALU, shifts, bitfields, multiplies, divides.
- Conditional and unconditional branches, tables (TBZ/TBNZ).
- Byte/halfword/word/doubleword loads and stores with every
  addressing mode (immediate, register, pre/post-index, literal).
- Pair loads and stores (LDP/STP) for stack frames.
- SIMD/FP for `strtod`/`printf` float formatting.
- Atomics (LDXR/STXR, CAS) for musl's `malloc` lock.
- ~50 distinct Linux syscalls covering I/O, memory, signals,
  terminal control, and process queries.

---

## 2. AArch64 Instruction Coverage

### 2.1 Encoding Groups

A64 instructions are 32-bit, classified by bits [28:25]:

| Bits [28:25] | Group | Section |
|--------------|-------|---------|
| `x0x0` | Unallocated / reserved | — |
| `100x` | Data processing — immediate | 2.2 |
| `101x` | Branches, exception, system | 2.3 |
| `x1x0` | Loads and stores | 2.4 |
| `x101` | Data processing — register | 2.5 |
| `0111` / `1111` | Data processing — SIMD & FP | 2.6 |

### 2.2 Data Processing — Immediate

| Instruction | Encoding (top bits) | Notes |
|-------------|-------------------|-------|
| ADD/ADDS (imm) | `x001_0001` | 12-bit imm, optional LSL #12 |
| SUB/SUBS (imm) | `x101_0001` | CMP is SUBS with Rd=XZR |
| AND/ORR/EOR/ANDS (imm) | `x00_100100` | bitmask immediate (logical) |
| MOVN/MOVZ/MOVK | `x00_100101` | 16-bit imm + shift (0/16/32/48) |
| SBFM/BFM/UBFM | `x00_100110` | ASR, LSR, LSL, SXTB, UXTB, etc. |
| EXTR | `x00_100111` | bit-field extract / ROR |
| ADR/ADRP | `x_xx_10000` | PC-relative address |

**Key aliases:**
- `CMP Xn, #imm` = `SUBS XZR, Xn, #imm`
- `MOV Xd, #imm` = `MOVZ` or `ORR Xd, XZR, #imm`
- `LSL Xd, Xn, #s` = `UBFM Xd, Xn, #(64-s), #(63-s)`
- `LSR Xd, Xn, #s` = `UBFM Xd, Xn, #s, #63`
- `ASR Xd, Xn, #s` = `SBFM Xd, Xn, #s, #63`

### 2.3 Branches, Exception, System

| Instruction | Encoding | Notes |
|-------------|----------|-------|
| B | `000101` | 26-bit signed offset * 4 |
| BL | `100101` | link: X30 = PC + 4 |
| B.cond | `01010100` | 4-bit cond, 19-bit offset |
| CBZ/CBNZ | `x011010x` | compare-and-branch |
| TBZ/TBNZ | `x011011x` | test-bit-and-branch |
| BR | `1101011_0000` | Rn |
| BLR | `1101011_0001` | Rn, link |
| RET | `1101011_0010` | default X30 |
| SVC | `11010100_000` | imm16, syscall trap |
| HVC/SMC | — | not needed in SE |
| MRS | `1101010100_11` | read system register |
| MSR | `1101010100_01` | write system register |
| NOP/YIELD/WFE/WFI/SEV | hint | NOP = `D503201F` |
| CLREX | `1101010100_0` | clear exclusive monitor |
| DMB/DSB/ISB | barriers | can be NOPs in SE for L0 |

**System registers needed for SE (MRS/MSR):**
- `TPIDR_EL0` — thread pointer (musl TLS).
- `FPCR`, `FPSR` — FP control/status.
- `CNTFRQ_EL0`, `CNTVCT_EL0` — timer (can return fixed values).
- `DCZID_EL0` — data-cache zero ID (for `DC ZVA`; return 4 = 64-byte).
- `CTR_EL0` — cache type register (return sane defaults).
- `MIDR_EL1` — CPU ID (can return a Cortex-A53 value).

### 2.4 Loads and Stores

| Instruction | Variants | Notes |
|-------------|----------|-------|
| LDR/STR (imm) | unsigned offset, pre-index, post-index | 8/16/32/64-bit |
| LDR/STR (reg) | Rm, extend/shift | |
| LDR (literal) | PC-relative, ±1 MB | |
| LDRB/LDRH/LDRSB/LDRSH/LDRSW | sign/zero extend | |
| STRB/STRH | byte/halfword stores | |
| LDP/STP | signed offset, pre/post-index | pair loads/stores |
| LDXR/STXR | exclusive (LL/SC) | atomics for malloc |
| LDADD/LDCLR/LDSET/SWP | LSE atomics | musl can use these |
| CAS/CASA/CASAL | compare-and-swap | optional, musl fallback to LDXR/STXR |
| LDR (SIMD/FP) | Bt/Ht/St/Dt/Qt | float loads |
| STR (SIMD/FP) | same | float stores |
| PRFM | prefetch | NOP in SE |

### 2.5 Data Processing — Register

| Instruction | Notes |
|-------------|-------|
| ADD/SUB (shifted reg) | LSL/LSR/ASR/ROR |
| ADD/SUB (extended reg) | UXTB, UXTH, UXTW, SXTB, SXTH, SXTW, SXTX |
| ADC/SBC | add/subtract with carry |
| AND/ORR/EOR/BIC/ORN/EON | logical (shifted reg) |
| ANDS/BICS | set flags |
| MUL/MADD/MSUB | multiply-add/sub |
| SMULL/UMULL/SMULH/UMULH | widening / high multiply |
| SDIV/UDIV | integer divide |
| CLS/CLZ/RBIT/REV/REV16/REV32 | bit manipulation |
| CSEL/CSINC/CSINV/CSNEG | conditional select |
| CCMN/CCMP | conditional compare |

**Key aliases:**
- `MOV Xd, Xn` = `ORR Xd, XZR, Xn`
- `MVN Xd, Xn` = `ORN Xd, XZR, Xn`
- `NEG Xd, Xn` = `SUB Xd, XZR, Xn`
- `CSET Xd, cond` = `CSINC Xd, XZR, XZR, invert(cond)`

### 2.6 SIMD and Floating-Point

fish-shell uses libc float formatting (`printf %f`, `strtod`).
Minimum FP coverage:

| Instruction | Notes |
|-------------|-------|
| FMOV (imm, reg, GP↔FP) | move between GP and FP regs |
| FADD/FSUB/FMUL/FDIV | scalar double/single |
| FCMP/FCMPE | compare, set NZCV |
| FCVTZS/FCVTZU | FP → integer |
| SCVTF/UCVTF | integer → FP |
| FCSEL | conditional select |
| FABS/FNEG/FSQRT | unary |
| LDR/STR Dt, St | FP load/store (already in 2.4) |

---

## 3. AArch64 Linux Syscall Table

fish-shell (musl static, AArch64) uses the following syscalls.
Number is the AArch64 `__NR_` value (in X8).

### 3.1 File I/O

| Nr | Name | Signature | Implementation notes |
|----|------|-----------|---------------------|
| 56 | `openat` | (dirfd, path, flags, mode) | Map to host `open()`; AT_FDCWD = -100 |
| 57 | `close` | (fd) | Close host fd |
| 63 | `read` | (fd, buf, count) | Read from host fd |
| 64 | `write` | (fd, buf, count) | Write to host fd |
| 62 | `lseek` | (fd, offset, whence) | Pass-through |
| 48 | `faccessat` | (dirfd, path, mode, flags) | Check file access |
| 79 | `fstatat` / `newfstatat` | (dirfd, path, statbuf, flags) | Fill struct stat |
| 80 | `fstat` | (fd, statbuf) | Fill struct stat |
| 61 | `getdents64` | (fd, buf, count) | Directory listing |
| 25 | `fcntl` | (fd, cmd, arg) | F_DUPFD, F_GETFL, F_SETFL, F_GETFD, F_SETFD |
| 23 | `dup` | (oldfd) | Duplicate fd |
| 24 | `dup3` | (oldfd, newfd, flags) | |
| 29 | `ioctl` | (fd, cmd, arg) | TIOCGWINSZ, TCGETS, TCSETS, FIONREAD |
| 59 | `pipe2` | (pipefd[2], flags) | Create pipe |
| 78 | `readlinkat` | (dirfd, path, buf, bufsiz) | Resolve symlinks |
| 49 | `chdir` | (path) | Change cwd |
| 17 | `getcwd` | (buf, size) | Get cwd |

### 3.2 Memory Management

| Nr | Name | Signature | Notes |
|----|------|-----------|-------|
| 214 | `brk` | (addr) | Heap management |
| 222 | `mmap` | (addr, len, prot, flags, fd, off) | MAP_ANONYMOUS, MAP_PRIVATE |
| 215 | `munmap` | (addr, len) | |
| 226 | `mprotect` | (addr, len, prot) | NOP (accept all) |
| 233 | `madvise` | (addr, len, advice) | NOP |

### 3.3 Process and Thread

| Nr | Name | Signature | Notes |
|----|------|-----------|-------|
| 172 | `getpid` | () | Return 1000 |
| 173 | `getppid` | () | Return 1 |
| 174 | `getuid` | () | Return 1000 |
| 175 | `geteuid` | () | Return 1000 |
| 176 | `getgid` | () | Return 1000 |
| 177 | `getegid` | () | Return 1000 |
| 178 | `gettid` | () | Return 1000 |
| 93 | `exit` | (status) | Halt simulation |
| 94 | `exit_group` | (status) | Halt simulation |
| 96 | `set_tid_address` | (tidptr) | Store ptr, return tid |
| 97 | `set_robust_list`| (head, len) | NOP, return 0 |
| 122 | `sched_yield` | () | NOP |
| 124 | `sched_getaffinity` | (pid,len,mask) | Return 1-CPU mask |
| 261 | `prlimit64` | (pid, resource, new, old) | RLIMIT_STACK, RLIMIT_NOFILE |
| 167 | `prctl` | (option, arg2-5) | PR_SET_NAME: NOP |

### 3.4 Signals

| Nr | Name | Signature | Notes |
|----|------|-----------|-------|
| 134 | `rt_sigaction` | (signum, act, oldact, sigsetsize) | Record handlers, don't deliver |
| 135 | `rt_sigprocmask` | (how, set, oldset, sigsetsize) | Track blocked mask |
| 139 | `rt_sigreturn` | () | Should not be called in basic SE |

### 3.5 Time

| Nr | Name | Signature | Notes |
|----|------|-----------|-------|
| 113 | `clock_gettime` | (clk_id, tp) | Return host wall time or simulated |
| 169 | `gettimeofday` | (tv, tz) | Same |

### 3.6 Polling and Select

| Nr | Name | Signature | Notes |
|----|------|-----------|-------|
| 73 | `ppoll` | (fds, nfds, tmo, sigmask, sigsetsize) | fish uses this for input |
| 72 | `pselect6` | (nfds, r, w, e, tmo, sigmask) | Alternative |

### 3.7 Miscellaneous

| Nr | Name | Signature | Notes |
|----|------|-----------|-------|
| 160 | `uname` | (buf) | Fill sysname=Linux, machine=aarch64 |
| 278 | `getrandom` | (buf, len, flags) | Fill from host `/dev/urandom` or zeros |
| 46 | `ftruncate` | (fd, length) | Pass-through |
| 43 | `statfs` | (path, buf) | Return sane defaults |
| 35 | `unlinkat` | (dirfd, path, flags) | Delete file |
| 34 | `mkdirat` | (dirfd, path, mode) | Create directory |
| 280 | `memfd_create`| (name, flags) | Return anon fd or -ENOSYS |

---

## 4. ELF64 Loader

### 4.1 Header Validation

```
Offset  Field           Expected
0x00    e_ident[0..4]   0x7F 'E' 'L' 'F'
0x04    EI_CLASS        2 (ELFCLASS64)
0x05    EI_DATA         1 (ELFDATA2LSB) — little-endian
0x10    e_type          2 (ET_EXEC) or 3 (ET_DYN — PIE)
0x12    e_machine       183 (EM_AARCH64)
```

### 4.2 Segment Loading

For each `PT_LOAD` program header:

1. Compute page-aligned base: `aligned_vaddr = p_vaddr & ~0xFFF`
2. Compute map size: `map_size = align_up(p_vaddr + p_memsz, 0x1000) - aligned_vaddr`
3. Map region in `AddressSpace` with permissions from `p_flags` (PF_R/PF_W/PF_X).
4. Copy `p_filesz` bytes from file offset `p_offset` to `p_vaddr`.
5. Zero-fill from `p_vaddr + p_filesz` to `p_vaddr + p_memsz` (.bss).

### 4.3 Stack Setup

Allocate 8 MB stack at a high address (e.g. `0x7FFF_FFE0_0000`).
Build the initial stack frame from high to low:

```
[random bytes — 16 bytes for AT_RANDOM]
[null-terminated env strings]
[null-terminated argv strings]
[null-terminated argv[0] = binary path]
[padding to 16-byte alignment]
[AT_NULL, 0]
[AT_RANDOM, ptr to random bytes]
[AT_PAGESZ, 4096]
[AT_PHNUM, e_phnum]
[AT_PHENT, e_phentsize]
[AT_PHDR, vaddr of loaded phdr]
[AT_ENTRY, e_entry]
[AT_UID, 1000]  [AT_EUID, 1000]
[AT_GID, 1000]  [AT_EGID, 1000]
[AT_CLKTCK, 100]
[AT_HWCAP, 0]   [AT_HWCAP2, 0]
[0 (envp terminator)]
[envp[0] pointer]
[0 (argv terminator)]
[argv[0] pointer]
[argc]               <-- initial SP
```

### 4.4 Initial Register State

```
PC  = e_entry
SP  = bottom of stack frame (16-byte aligned)
X0  = 0   (Linux kernel sets argc in stack, not X0)
X1-X30 = 0
NZCV = 0
FPCR = 0
TPIDR_EL0 = 0  (musl sets this itself via set_tid_address)
```

---

## 5. Terminal Emulation for fish-shell

fish-shell is interactive — it reads from stdin and writes formatted
output to stdout.  In SE mode we need to handle:

### 5.1 Terminal ioctls

| ioctl | Value | Response |
|-------|-------|----------|
| `TCGETS` | `0x5401` | Return a `struct termios` with sane defaults (raw mode) |
| `TCSETS`/`TCSETSW`/`TCSETSF` | `0x5402-04` | Store termios, return 0 |
| `TIOCGWINSZ` | `0x5413` | Return `{ rows: 24, cols: 80 }` |
| `TIOCSWINSZ` | `0x5414` | NOP, return 0 |
| `FIONREAD` | `0x541B` | Return 0 (no pending input) |
| `TIOCGPGRP` | `0x540F` | Return getpid() |
| `TIOCSPGRP` | `0x5410` | NOP |

### 5.2 Non-blocking I/O

fish sets `O_NONBLOCK` on stdin via `fcntl(0, F_SETFL, O_NONBLOCK)`.
Reads from stdin should return `EAGAIN` when no data is available.

### 5.3 ppoll

fish's main loop calls `ppoll` on stdin.  The simplest implementation:
- If stdin has data (piped input), return immediately.
- If stdin is a tty with no data, return timeout or block.
- For non-interactive testing, pipe a command: `echo "echo hello" | fish`

---

## 6. File Descriptor Table

SE mode maintains a guest → host fd mapping:

```rust
pub struct FdTable {
    /// guest_fd -> host_fd (or special)
    map: HashMap<i32, FdEntry>,
    next_fd: i32,
}

pub enum FdEntry {
    /// Maps to a real host file descriptor.
    HostFd(i32),
    /// stdin/stdout/stderr — pass through to host.
    Stdio(i32),
    /// A pipe created by pipe2().
    Pipe { read_end: i32, write_end: i32 },
    /// An anonymous memory fd (memfd_create).
    MemFd { data: Vec<u8>, pos: usize },
}
```

Initial state: fd 0 = stdin, fd 1 = stdout, fd 2 = stderr.

---

## 7. Signal Handling (Minimal)

fish registers signal handlers for SIGINT, SIGCHLD, SIGWINCH, etc.
In SE mode:

1. `rt_sigaction`: record the handler address and flags; don't actually
   install a host signal handler.
2. `rt_sigprocmask`: track the blocked mask.
3. Never deliver signals to the guest unless the user explicitly
   sends one (future feature).
4. `sigaltstack`: record alt-stack address, return 0.

This is sufficient — fish will run normally, it just won't receive
any signals.

---

## 8. TDD Test Plan — fish-shell Milestones

Each milestone below is a set of tests to write **before** the
implementation.

### M1 — ELF loader parses real binary

```
test_load_fish_elf          parse PT_LOAD segments from fish binary
test_entry_point            e_entry is a valid code address
test_bss_zeroed             .bss region is all zeros
test_stack_argc_argv        argc=2, argv=["fish","-c"], envp=["HOME=/"]
test_auxv_has_at_entry      AT_ENTRY matches e_entry
test_auxv_has_at_pagesz     AT_PAGESZ = 4096
test_auxv_has_at_random     AT_RANDOM points to 16 random bytes
```

### M2 — musl init sequence

musl's `_start` → `__libc_start_main` does:
1. Read argc/argv/envp from stack.
2. Call `__init_tls` → `set_tid_address` + `mmap` for TLS.
3. Call `__init_libc` → `getauxval`, `brk`, `mprotect`.
4. Call `main(argc, argv, envp)`.

```
test_set_tid_address        returns a tid > 0
test_mmap_tls               MAP_ANONYMOUS|MAP_PRIVATE returns valid addr
test_brk_advance            brk(0) then brk(+N) extends heap
test_tpidr_el0_writable     MSR TPIDR_EL0, Xn works
test_musl_reaches_main      breakpoint at main() is hit
```

### M3 — Basic I/O (`fish -c "echo hello"`)

```
test_write_stdout           write(1, "hello\n", 6) -> output captured
test_openat_dev_null        openat(AT_FDCWD, "/dev/null", O_RDONLY) succeeds
test_fstat_stdout           fstat(1, buf) fills st_mode with S_IFCHR
test_ioctl_tcgets           ioctl(0, TCGETS, buf) returns 0
test_ioctl_tiocgwinsz       ioctl(1, TIOCGWINSZ, buf) returns rows/cols
test_fcntl_getfl            fcntl(0, F_GETFL) returns flags
test_ppoll_stdin_pipe       ppoll with data on stdin returns immediately
test_exit_group_0           exit_group(0) halts with status 0
```

### M4 — Full fish-shell (`echo hello` via pipe)

```
test_fish_echo_hello        echo "echo hello" | helm --isa aarch64
                            --binary fish-static -- -c "echo hello"
                            stdout contains "hello\n"
                            exit status = 0
```

### M5 — Interactive smoke (stretch)

```
test_fish_version           fish --version prints version string
test_fish_help_c            fish -c "echo \$status" prints "0"
```

---

## 9. Decoder Implementation Strategy

Implement the decoder as a big `match` on encoding groups, then
sub-match on specific instruction fields.

```rust
pub fn decode_insn(&self, pc: Addr, insn: u32) -> HelmResult<Vec<MicroOp>> {
    let op0 = (insn >> 25) & 0xF; // bits [28:25]

    match op0 {
        0b1000 | 0b1001 => self.decode_dp_imm(pc, insn),
        0b1010 | 0b1011 => self.decode_branch_sys(pc, insn),
        0b0100 | 0b0110 | 0b1100 | 0b1110 => self.decode_ldst(pc, insn),
        0b0101 | 0b1101 => self.decode_dp_reg(pc, insn),
        0b0111 | 0b1111 => self.decode_simd_fp(pc, insn),
        _ => self.decode_unallocated(pc, insn),
    }
}
```

Each sub-decoder returns `Vec<MicroOp>` with:
- `opcode`: from the `Opcode` enum
- `sources` / `dest`: architectural register IDs
- `immediate`: decoded immediate value
- `flags`: `is_branch`, `is_call`, `is_return`, etc.

---

## 10. Execution Loop (SE + FE)

```rust
pub fn run_se_fe(&mut self) -> HelmResult<SimResults> {
    loop {
        // 1. Fetch 4 bytes from AddressSpace at regs.pc
        let mut insn_bytes = [0u8; 4];
        self.address_space.read(self.regs.pc, &mut insn_bytes)?;
        let insn = u32::from_le_bytes(insn_bytes);

        // 2. Decode
        let uops = self.decoder.decode_insn(self.regs.pc, insn)?;

        // 3. Execute each uop
        for uop in &uops {
            match uop.opcode {
                Opcode::Syscall => {
                    let nr = self.regs.x[8];
                    let args = [
                        self.regs.x[0], self.regs.x[1], self.regs.x[2],
                        self.regs.x[3], self.regs.x[4], self.regs.x[5],
                    ];
                    let result = self.syscall_handler.handle(
                        nr, &args, &mut self.address_space,
                    )?;
                    self.regs.x[0] = result;
                    if self.syscall_handler.should_exit() {
                        return Ok(self.stats.results.clone());
                    }
                }
                _ => self.execute_uop(uop)?,
            }
        }

        // 4. Advance PC (unless a branch already changed it)
        if !self.pc_modified {
            self.regs.pc += 4;
        }
        self.pc_modified = false;

        // 5. Stats
        self.stats.results.instructions_committed += 1;
        self.stats.results.cycles += 1; // FE: IPC=1
    }
}
```

---

## 11. Implementation Order

| Phase | What | Tests first |
|-------|------|-------------|
| 1 | ELF64 loader + stack setup | M1 |
| 2 | Integer ALU decoder + executor | 2.2 + 2.5 subset |
| 3 | Branch decoder + executor | 2.3 subset |
| 4 | Load/store decoder + executor | 2.4 subset |
| 5 | System instructions (SVC, MRS, NOP) | 2.3 system |
| 6 | Syscall table (exit, write, brk, mmap) | M2 |
| 7 | musl init runs to main() | M2 tests |
| 8 | File I/O syscalls (open, read, fstat, fcntl) | M3 |
| 9 | Terminal ioctls, ppoll | M3 |
| 10 | FP/SIMD subset (FMOV, FADD, FCVT) | 2.6 subset |
| 11 | Atomics (LDXR/STXR) | 2.4 atomics |
| 12 | `fish -c "echo hello"` end-to-end | M4 |
