# helm-engine/se — LLD: Linux ABI Mapping

> **Module:** `helm_engine::se::abi`
> **Types:** `SyscallAbi` trait, `RiscvSyscallAbi`, `Aarch64SyscallAbi`

---

## Table of Contents

1. [SyscallAbi Trait](#1-syscallabi-trait)
2. [SyscallArgs and SyscallNr](#2-syscallargs-and-syscallnr)
3. [RiscvSyscallAbi](#3-riscvsyscallabi)
4. [Aarch64SyscallAbi](#4-aarch64syscallabi)
5. [ABI Reference Tables](#5-abi-reference-tables)
6. [Integration with HelmEngine](#6-integration-with-helmengine)
7. [Extension Pattern — Adding a New ISA ABI](#7-extension-pattern--adding-a-new-isa-abi)

---

## 1. SyscallAbi Trait

`SyscallAbi` is the per-ISA adapter that translates between the guest register file and the ISA-neutral `SyscallArgs` / return value representation. It is separate from `SyscallHandler` so that new ISAs can be added without touching the dispatch logic.

```rust
use helm_core::ThreadContext;
use crate::{SyscallArgs, SyscallNr};

/// Per-ISA Linux syscall calling convention.
///
/// Implementors map ISA-specific registers to the ISA-neutral `SyscallArgs`
/// and write the return value back into the appropriate return register.
///
/// All methods take `&dyn ThreadContext` (cold path; one call per syscall).
pub trait SyscallAbi: Send + Sync {
    /// Extract the syscall number and arguments from the guest register file.
    ///
    /// Called once per syscall instruction, immediately before dispatching
    /// to `SyscallHandler::handle()`.
    fn extract_args(&self, ctx: &dyn ThreadContext) -> (SyscallNr, SyscallArgs);

    /// Write the syscall return value into the appropriate guest register.
    ///
    /// `ret` is the signed 64-bit return value:
    /// - Positive or zero → success.
    /// - Negative → errno negated (e.g. -22 for EINVAL).
    ///
    /// Called once per syscall instruction, after `SyscallHandler::handle()` returns.
    fn set_return(&self, ctx: &mut dyn ThreadContext, ret: i64);
}

/// Type alias for clarity in signatures.
pub type SyscallNr = u64;
```

---

## 2. SyscallArgs and SyscallNr

```rust
/// Up to 6 syscall arguments in ISA-neutral form.
///
/// Arguments are always treated as unsigned 64-bit values at the ABI layer.
/// Individual syscall handlers interpret them as signed/pointer/etc. as needed.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyscallArgs {
    pub a: [u64; 6],
}

impl SyscallArgs {
    pub fn new(a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> Self {
        Self { a: [a0, a1, a2, a3, a4, a5] }
    }

    #[inline] pub fn arg0(&self) -> u64 { self.a[0] }
    #[inline] pub fn arg1(&self) -> u64 { self.a[1] }
    #[inline] pub fn arg2(&self) -> u64 { self.a[2] }
    #[inline] pub fn arg3(&self) -> u64 { self.a[3] }
    #[inline] pub fn arg4(&self) -> u64 { self.a[4] }
    #[inline] pub fn arg5(&self) -> u64 { self.a[5] }
}
```

---

## 3. RiscvSyscallAbi

### RISC-V Linux Syscall Convention

| Register | Purpose |
|----------|---------|
| `a7` (x17) | Syscall number |
| `a0` (x10) | Argument 0 / Return value |
| `a1` (x11) | Argument 1 |
| `a2` (x12) | Argument 2 |
| `a3` (x13) | Argument 3 |
| `a4` (x14) | Argument 4 |
| `a5` (x15) | Argument 5 |

The syscall instruction is `ecall`. The CPU raises an `Environment Call from U-mode` exception (cause code 8) which `HelmEngine` intercepts and routes to `SyscallHandler::handle()`.

### Implementation

```rust
/// RISC-V (RV64GC) Linux syscall ABI.
///
/// Syscall nr: a7 (x17).
/// Arguments:  a0–a5 (x10–x15).
/// Return:     a0 (x10).
pub struct RiscvSyscallAbi;

impl SyscallAbi for RiscvSyscallAbi {
    fn extract_args(&self, ctx: &dyn ThreadContext) -> (SyscallNr, SyscallArgs) {
        let nr = ctx.read_int_reg(17);  // a7
        let args = SyscallArgs::new(
            ctx.read_int_reg(10),  // a0
            ctx.read_int_reg(11),  // a1
            ctx.read_int_reg(12),  // a2
            ctx.read_int_reg(13),  // a3
            ctx.read_int_reg(14),  // a4
            ctx.read_int_reg(15),  // a5
        );
        (nr, args)
    }

    fn set_return(&self, ctx: &mut dyn ThreadContext, ret: i64) {
        ctx.write_int_reg(10, ret as u64);  // a0
    }
}
```

### RISC-V Syscall Number Table (Phase 0)

RISC-V Linux uses a unified syscall table (same numbers for RV32 and RV64 starting with Linux 5.x):

```
nr   name
──   ────────────────────────────────
29   ioctl
56   openat
57   close
62   lseek
63   read
64   write
66   writev
80   fstat
93   exit
94   exit_group
160  uname
169  gettimeofday
172  getpid
174  getuid
175  geteuid
176  getgid
177  getegid
214  brk
215  munmap
222  mmap
```

---

## 4. Aarch64SyscallAbi

### AArch64 Linux Syscall Convention

| Register | Purpose |
|----------|---------|
| `x8` | Syscall number |
| `x0` | Argument 0 / Return value |
| `x1` | Argument 1 |
| `x2` | Argument 2 |
| `x3` | Argument 3 |
| `x4` | Argument 4 |
| `x5` | Argument 5 |

The syscall instruction is `svc #0`. The CPU raises a Supervisor Call exception which `HelmEngine` routes to `SyscallHandler::handle()`.

### Implementation

```rust
/// AArch64 Linux syscall ABI.
///
/// Syscall nr: x8.
/// Arguments:  x0–x5.
/// Return:     x0.
pub struct Aarch64SyscallAbi;

impl SyscallAbi for Aarch64SyscallAbi {
    fn extract_args(&self, ctx: &dyn ThreadContext) -> (SyscallNr, SyscallArgs) {
        let nr = ctx.read_int_reg(8);  // x8
        let args = SyscallArgs::new(
            ctx.read_int_reg(0),   // x0
            ctx.read_int_reg(1),   // x1
            ctx.read_int_reg(2),   // x2
            ctx.read_int_reg(3),   // x3
            ctx.read_int_reg(4),   // x4
            ctx.read_int_reg(5),   // x5
        );
        (nr, args)
    }

    fn set_return(&self, ctx: &mut dyn ThreadContext, ret: i64) {
        ctx.write_int_reg(0, ret as u64);  // x0
    }
}
```

### AArch64 Syscall Number Table (Phase 0)

AArch64 Linux uses the same unified syscall numbers as RISC-V for the Phase 0 set (both follow the `linux-abi` unified table for 64-bit ARM since Linux 4.x):

```
nr   name
──   ─────────────────────
29   ioctl
56   openat
57   close
62   lseek
63   read
64   write
66   writev
80   fstat
93   exit
94   exit_group
160  uname
169  gettimeofday
172  getpid
174  getuid
175  geteuid
176  getgid
177  getegid
214  brk
215  munmap
222  mmap
```

---

## 5. ABI Reference Tables

### Comparison Table

| Property | RISC-V RV64GC | AArch64 |
|----------|---------------|---------|
| Syscall instruction | `ecall` | `svc #0` |
| Syscall nr register | `a7` (x17) | `x8` |
| Arg 0 register | `a0` (x10) | `x0` |
| Arg 1 register | `a1` (x11) | `x1` |
| Arg 2 register | `a2` (x12) | `x2` |
| Arg 3 register | `a3` (x13) | `x3` |
| Arg 4 register | `a4` (x14) | `x4` |
| Arg 5 register | `a5` (x15) | `x5` |
| Return register | `a0` (x10) | `x0` |
| Error convention | Negative return = -errno | Negative return = -errno |
| Max arguments | 6 | 6 |

### Error Convention

Both RISC-V and AArch64 Linux syscalls return the negated errno on failure:

```
ret >= 0  → success; ret is the return value (count, fd, address, etc.)
ret < 0   → failure; -ret is the errno value (e.g. ret=-22 → EINVAL)
```

Libc wrappers (like `glibc`'s `write()`) convert this to the `errno` global and return -1. The simulator returns the raw kernel convention directly into the guest return register.

---

## 6. Integration with HelmEngine

The `SyscallAbi` is stored in `HelmEngine<T>` as a `Box<dyn SyscallAbi>`. It is set at construction time based on the `Isa` field and never changes during simulation.

```rust
impl<T: TimingModel> HelmEngine<T> {
    pub fn new(isa: Isa, mode: ExecMode, timing: T) -> Self {
        let abi: Box<dyn SyscallAbi> = match isa {
            Isa::RiscV   => Box::new(RiscvSyscallAbi),
            Isa::AArch64 => Box::new(Aarch64SyscallAbi),
            // AArch32 deferred to Phase 3
        };
        // ...
    }

    /// Called by the ISA execute loop when a syscall instruction is encountered.
    pub fn handle_syscall(&mut self) {
        // 1. Extract the syscall number and arguments.
        let tc = self.hart.thread_context();
        let (nr, args) = self.abi.extract_args(tc);

        // 2. Fire the SyscallEnter event on the bus (observed by TraceLogger).
        self.event_bus.fire(HelmEvent::SyscallEnter { nr, args: args.a });

        // 3. Dispatch to the handler.
        let tc_mut = self.hart.thread_context_mut();
        let result = self.syscall_handler.handle(nr, args, tc_mut);

        // 4. Write the return value.
        let ret = match result {
            SyscallResult::Ok(v)   => v,
            SyscallResult::Err(e)  => e.to_negative(),
            SyscallResult::Passthrough => {
                log::warn!("unimplemented syscall nr={nr}");
                SyscallError::ENOSYS.to_negative()
            }
        };
        self.abi.set_return(self.hart.thread_context_mut(), ret);

        // 5. Fire the SyscallReturn event.
        self.event_bus.fire(HelmEvent::SyscallReturn { nr, ret: ret as u64 });
    }
}
```

---

## 7. Extension Pattern — Adding a New ISA ABI

To support a new ISA (e.g. x86-64 or MIPS):

1. Define a new struct: `pub struct X86_64SyscallAbi;`
2. Implement `SyscallAbi` for it, mapping `rax` (syscall nr), `rdi/rsi/rdx/r10/r8/r9` (args), `rax` (return).
3. Add a new `Isa::X86_64` variant to the `Isa` enum in `helm-core`.
4. Add a match arm in `HelmEngine::new()` to instantiate `Box::new(X86_64SyscallAbi)`.
5. Add the ISA to the `SyscallDispatch` table (syscall numbers differ from RISC-V/AArch64 for some calls).

No changes to `LinuxSyscallHandler`, `SyscallDispatch`, or any individual syscall implementation are needed.

### Example: x86-64 ABI (not yet implemented, shown for reference)

```rust
pub struct X86_64SyscallAbi;

impl SyscallAbi for X86_64SyscallAbi {
    fn extract_args(&self, ctx: &dyn ThreadContext) -> (SyscallNr, SyscallArgs) {
        // x86-64 Linux: syscall nr in rax, args in rdi, rsi, rdx, r10, r8, r9
        let nr = ctx.read_int_reg(0);   // rax = reg index 0 in helm's x86 regfile
        let args = SyscallArgs::new(
            ctx.read_int_reg(5),   // rdi
            ctx.read_int_reg(4),   // rsi
            ctx.read_int_reg(3),   // rdx
            ctx.read_int_reg(10),  // r10
            ctx.read_int_reg(8),   // r8
            ctx.read_int_reg(9),   // r9
        );
        (nr, args)
    }

    fn set_return(&self, ctx: &mut dyn ThreadContext, ret: i64) {
        ctx.write_int_reg(0, ret as u64);  // rax
    }
}
```

> Note: x86-64 has different syscall numbers for most calls (e.g. `write` is nr 1, not 64). A separate `X86_64SyscallDispatch` would be needed alongside `LinuxSyscallDispatch`.
