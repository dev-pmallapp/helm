# Plan: RISC-V 64 User-Mode Emulation (`helm-riscv64`)

> **Status:** Planned — 2026-03
> **Goal:** Ship a working `helm-riscv64` binary that runs statically linked RV64 Linux binaries
> **Completion gate:** `assets/riscv/bin/busybox sh -c 'echo hello'` exits 0 and prints `hello`

---

## Current State

| Component | State |
|-----------|-------|
| `runtime/helm-arch/src/riscv/decode.rs` | ✅ RV64I + C-extension partial; M/A/F/D decodes to `Err(Unimplemented)` |
| `runtime/helm-arch/src/riscv/execute.rs` | ✅ RV64I all 47 insns; Zicsr 6 insns; M/A/F/D pending |
| `runtime/helm-engine/src/se/mod.rs` | ✅ `SyscallHandler` trait defined; `syscall_handler` field on `HelmEngine` |
| `runtime/helm-engine/src/lib.rs` → `handle_exception` | ✅ RISC-V ecall routes to `self.syscall_handler` if `Some` |
| `runtime/helm-engine/src/loader/elf64.rs` | ✅ ELF64 loader (ISA-agnostic; works for any RISC-V ELF) |
| `LinuxRiscv64SyscallHandler` | ❌ Does not exist |
| `HelmSim::load_riscv64_elf()` | ❌ Does not exist |
| `helm-riscv64` binary | ❌ Does not exist |

The engine `step_riscv()` already fetches, decodes, executes, and on `EnvironmentCall` exception
routes to `syscall_handler`. The dispatch path is ready — only the handler and CLI are missing.

---

## ABI Reference: RISC-V Linux vs AArch64 Linux

| | RISC-V Linux | AArch64 Linux |
|---|---|---|
| Syscall instruction | `ecall` | `svc #0` |
| Syscall number register | `a7` (x17) | `x8` |
| Arg registers | `a0`–`a5` (x10–x15) | `x0`–`x5` |
| Return value | `a0` (x10) | `x0` |
| Error encoding | negative errno in `a0` | negative errno in `x0` |
| C extension | yes (16-bit compressed insns) | no |
| Endianness | little-endian | little-endian |

The existing `SyscallArgs { a0, a1, a2, a3, a4, a5 }` struct maps exactly to RISC-V Linux
convention. `nr` is passed as the `EnvironmentCall { nr }` field (extracted from `x17` in execute.rs).

---

## Implementation Steps

### 1. `LinuxRiscv64SyscallHandler` — new file `runtime/helm-engine/src/se/linux_riscv64.rs`

Mirror the structure of `linux_aarch64.rs`. RISC-V Linux syscall numbers differ from AArch64 but
host libc calls are the same. Key syscalls for `busybox` SE:

```rust
pub struct LinuxRiscv64SyscallHandler {
    brk_base: u64,
    brk_cur:  u64,
    fd_map:   HashMap<i32, RawFd>,  // guest fd → host fd
}

impl SyscallHandler for LinuxRiscv64SyscallHandler {
    fn handle(&mut self, nr: u64, args: SyscallArgs) -> Result<i64, HartException> {
        match nr {
            // syscall numbers from: include/uapi/asm-generic/unistd.h (used by RISC-V)
            29  => sys_ioctl(args),        // IOCTL
            56  => sys_openat(args),       // openat
            57  => sys_close(args),        // close
            63  => sys_read(args),         // read
            64  => sys_write(args),        // write
            78  => sys_readlinkat(args),   // readlinkat
            79  => sys_fstatat(args),      // newfstatat
            80  => sys_fstat(args),        // fstat
            93  => sys_exit(args),         // exit
            94  => sys_exit_group(args),   // exit_group
            96  => sys_set_tid_address(args),
            99  => sys_set_robust_list(args),
            113 => sys_clock_gettime(args),
            129 => sys_kill(args),
            134 => sys_rt_sigaction(args),
            135 => sys_rt_sigprocmask(args),
            160 => sys_uname(args),
            163 => sys_getrlimit(args),    // actually prlimit64
            172 => sys_getpid(args),
            174 => sys_getuid(args),
            175 => sys_geteuid(args),
            176 => sys_getgid(args),
            177 => sys_getegid(args),
            214 => sys_brk(args, &mut self.brk_cur),
            215 => sys_munmap(args),
            222 => sys_mmap(args),
            226 => sys_mprotect(args),
            261 => sys_prlimit64(args),
            // ... extend as needed
            _   => { Err(HartException::Exit { code: -ENOSYS as i32 }) }
        }
    }
}
```

RISC-V uses the `asm-generic/unistd.h` numbering (shared with arm64 for most syscalls, but not all).
Refer to `linux/arch/riscv/include/uapi/asm/unistd.h` — it includes `asm-generic/unistd.h` directly.
Many numbers are identical to AArch64; copy implementations from `linux_aarch64.rs` where numbers differ.

**Common gotchas:**
- `fstat` (80) vs `fstatat` (79): `busybox` uses `fstatat` with `AT_FDCWD` (-100), not `fstat`.
- `brk(0)` → return current `brk_cur`; `brk(addr)` → update `brk_cur` if valid.
- `mmap` with `MAP_ANONYMOUS|MAP_PRIVATE`: allocate in `FlatMem`, return mapped guest address.
- `exit_group` (94) should return `HartException::Exit { code }`, not just `ENOSYS`.

### 2. `HelmEngine::load_riscv64_elf()` — add to `runtime/helm-engine/src/lib.rs`

```rust
pub fn load_riscv64_elf(&mut self, path: &str, argv: &[&str], envp: &[&str]) -> Result<(), String> {
    use crate::loader::elf64::Elf64Loader;
    let loaded = Elf64Loader::load(path, &mut self.memory, argv, envp)
        .map_err(|e| e.to_string())?;
    // Set PC and stack pointer — RISC-V: PC = entry, sp = x2
    self.pc = loaded.entry;
    self.iregs[2] = loaded.initial_sp;  // sp
    self.iregs[10] = loaded.argc as u64;  // a0 = argc
    self.iregs[11] = loaded.argv_ptr;    // a1 = argv ptr
    self.isa  = Isa::RiscV;
    self.mode = ExecMode::Syscall;
    let handler = LinuxRiscv64SyscallHandler::new(loaded.brk_base);
    self.set_syscall_handler(Box::new(handler));
    Ok(())
}
```

Add `HelmSim::load_riscv64_elf()` forwarding variant (same pattern as `load_aarch64_elf`).

### 3. RV64M — Multiply/Divide (needed by busybox)

`busybox` uses `mul`, `div`, `rem` from RV64M. These decode to `Err(Unimplemented)` today.
Implement M-extension in `execute.rs` before testing busybox:

```rust
Mul { rd, rs1, rs2 } => {
    let result = (ctx.read_int_reg(rs1) as i64)
        .wrapping_mul(ctx.read_int_reg(rs2) as i64) as u64;
    ctx.write_int_reg(rd, result);
    ctx.advance_pc(4);
}
// Mulh, Mulhsu, Mulhu, Div, Divu, Rem, Remu, Mulw, Divw, Divuw, Remw, Remuw
```

Division by zero: `div` → `-1` (all bits set), `divu` → `u64::MAX`, `rem`/`remu` → dividend.
Overflow: `(-MIN_I64) / (-1)` → `MIN_I64`, remainder → `0`.

### 4. `helm-riscv64` binary — `runtime/helm-cli/src/bin/helm_riscv64.rs`

```rust
fn main() {
    let args = RiscvArgs::parse();  // clap: --mem-size, binary, argv
    let mut sim = HelmSim::new_virtual(Isa::RiscV, ExecMode::Syscall, mem_base, mem_size);
    sim.load_riscv64_elf(&args.binary, &args.argv, &[]).unwrap();
    loop {
        match sim.run(1_000_000) {
            StopReason::Exit { code } => std::process::exit(code),
            StopReason::Quantum      => continue,
            other                    => { eprintln!("stop: {:?}", other); break; }
        }
    }
}
```

Add to `runtime/helm-cli/Cargo.toml`:
```toml
[[bin]]
name = "helm-riscv64"
path = "src/bin/helm_riscv64.rs"
```

### 5. C Extension (compressed insns)

`busybox` is compiled with `-march=rv64gc` — all G+C extensions. The decoder has `expand_c()` but
the engine must call it. In `step_riscv()`:

```rust
// Fetch: try 16-bit first (check low 2 bits ≠ 0b11)
let raw16 = self.memory.fetch16(pc)?;
let (raw32, insn_size) = if (raw16 & 0b11) != 0b11 {
    (riscv_expand_c(raw16)?, 2u64)   // C extension: expand to 32-bit
} else {
    let raw32 = self.memory.fetch32(pc)?;
    (raw32, 4u64)
};
```

`riscv_expand_c()` is in `helm-arch/src/riscv/decode.rs`. The engine's `step_riscv()` currently
always fetches 32 bits — this needs updating before C-extension binaries will run.

---

## Test Strategy

### Smoke test (gate 1)
```bash
cargo build --bin helm-riscv64
./target/debug/helm-riscv64 assets/riscv/bin/static-sh -c 'echo hello'
# Expected: "hello\n", exit 0
```

### Busybox test (gate 2)
```bash
./target/debug/helm-riscv64 assets/riscv/bin/busybox sh -c 'echo hello && ls /'
# Expected: "hello\n", directory listing, exit 0
```

### riscv-tests (gate 3 — Phase 0 completion)
```bash
# Download riscv-tests from riscv/riscv-tests (rv64ui-p-* suite)
cargo test --package helm-arch -- riscv
# All rv64ui-p-* tests pass (no_mmu, bare-metal ELF format)
```

The `rv64ui-p-*` tests use `ecall` to signal pass/fail via `a0`. No OS needed — FE mode (not SE).

### Differential test (gate 4)
```bash
# Run same binary under Spike and helm-riscv64, compare output
spike pk assets/riscv/bin/busybox sh -c 'echo hello'
./target/debug/helm-riscv64 assets/riscv/bin/busybox sh -c 'echo hello'
```

---

## Scope (Phase 0.5 — what's in, what's deferred)

| In scope | Deferred |
|----------|---------|
| RV64I + M + C extensions | F/D floating-point |
| ~50 essential syscalls for busybox | Full POSIX syscall coverage |
| Static ELF binaries (no dynamic linker) | Dynamic linking (`ld.so`) |
| Single-threaded binaries | `clone`/threads |
| `helm-riscv64` CLI binary | Python config layer for RISC-V |
| riscv-tests rv64ui pass | riscv-tests rv64um/rv64uf/rv64ua |
| Differential test vs Spike | Full differential trace comparison |

F/D extensions: the decoder already has stubs that return `Unimplemented`. `busybox` and `static-sh`
are compiled with soft-float (`-mabi=lp64d` but linked against musl which handles FP in software for
basic use). Most basic utilities will work without hardware FP.

---

## File Change Summary

| File | Action |
|------|--------|
| `runtime/helm-engine/src/se/linux_riscv64.rs` | **Create** — `LinuxRiscv64SyscallHandler` |
| `runtime/helm-engine/src/se/mod.rs` | **Edit** — `pub mod linux_riscv64; pub use ...` |
| `runtime/helm-engine/src/lib.rs` | **Edit** — `load_riscv64_elf()`, C-ext in `step_riscv()` |
| `runtime/helm-arch/src/riscv/execute.rs` | **Edit** — add RV64M arms |
| `runtime/helm-arch/src/riscv/decode.rs` | **Edit** — wire `expand_c` into decode path |
| `runtime/helm-cli/src/bin/helm_riscv64.rs` | **Create** — CLI main |
| `runtime/helm-cli/Cargo.toml` | **Edit** — add `[[bin]]` entry |
| `Cargo.toml` (workspace) | No change — `runtime/*` already a member |

---

## Acceptance Criteria

- [ ] `cargo build --bin helm-riscv64` succeeds
- [ ] `helm-riscv64 assets/riscv/bin/static-sh -c 'echo hello'` → `hello`, exit 0
- [ ] `helm-riscv64 assets/riscv/bin/busybox sh -c 'echo hello'` → `hello`, exit 0
- [ ] All `rv64ui-p-*` riscv-tests pass via `cargo test --package helm-arch`
- [ ] No regressions on AArch64 SE/FS (run `examples/fs/boot_rpi_full.py`)
