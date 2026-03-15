# helm-engine/se — Test Plan

> **Crate:** `helm-engine/se`
> **Test targets:** Phase 0 syscall correctness, hello-world binary execution, ABI extraction

---

## 1. End-to-End: Run Statically-Linked `hello_world` Binary

**Goal:** Execute a statically-linked RISC-V hello-world binary; verify that stdout receives exactly `"Hello, World!\n"`.

### Test Fixture: Building the Binary

```bash
# Produce tests/fixtures/hello_world.riscv64
riscv64-linux-musl-gcc -static -O2 -o tests/fixtures/hello_world.riscv64 tests/fixtures/hello_world.c
```

```c
// tests/fixtures/hello_world.c
#include <stdio.h>
int main(void) {
    puts("Hello, World!");
    return 0;
}
```

### Test: `test_se_hello_world_stdout`

```rust
// tests/se_end_to_end.rs
use helm_test::SeSimulation;

/// Run hello_world.riscv64 and capture its stdout output.
#[test]
fn test_se_hello_world_stdout() {
    let mut sim = SeSimulation::load_elf("tests/fixtures/hello_world.riscv64")
        .expect("failed to load ELF");

    // Redirect guest stdout (fd 1) to a buffer instead of the real stdout.
    let mut captured = Vec::new();
    sim.redirect_stdout(&mut captured);

    // Run until sys_exit is called (simulation terminates naturally).
    let exit_code = sim.run_to_exit();

    assert_eq!(exit_code, 0, "hello_world must exit with code 0");

    let output = String::from_utf8(captured).expect("non-UTF-8 output");
    assert_eq!(
        output,
        "Hello, World!\n",
        "expected 'Hello, World!\\n', got: {output:?}"
    );
}
```

### Test: `test_se_hello_world_aarch64`

```rust
/// Same binary, compiled for AArch64.
#[test]
fn test_se_hello_world_aarch64() {
    let mut sim = SeSimulation::load_elf("tests/fixtures/hello_world.aarch64")
        .expect("failed to load AArch64 ELF");

    let mut captured = Vec::new();
    sim.redirect_stdout(&mut captured);
    let exit_code = sim.run_to_exit();

    assert_eq!(exit_code, 0);
    let output = String::from_utf8(captured).unwrap();
    assert_eq!(output, "Hello, World!\n");
}
```

---

## 2. Syscall Unit Tests — Verify Results vs Host

### Test: `test_sys_write_returns_byte_count`

```rust
use helm_se::{LinuxSyscallHandler, SyscallHandler, SyscallArgs, SyscallResult};
use helm_test::{MockThreadContext, MockMemoryMap};

#[test]
fn test_sys_write_returns_byte_count() {
    let mut ctx = MockThreadContext::new();
    let mem = MockMemoryMap::new();

    // Write "hello" (5 bytes) to a pipe so we can capture it.
    let (rx, tx) = os_pipe::pipe().unwrap();
    let mut process = LinuxProcess::new(0x8000_0000, 0xC000_0000);
    let guest_fd = process.fds.insert(tx.into_raw_fd());

    // Place "hello" in mock guest memory at address 0x1000.
    ctx.write_mem(0x1000, b"hello");

    let mut handler = LinuxSyscallHandler::new(process, Arc::new(Mutex::new(mem)));
    let args = SyscallArgs::new(guest_fd as u64, 0x1000, 5, 0, 0, 0);

    let result = handler.handle(64, args, &mut ctx);  // nr 64 = write

    assert!(matches!(result, SyscallResult::Ok(5)), "write must return 5");

    // Verify the data reached the pipe.
    drop(handler);  // closes tx
    let mut buf = [0u8; 5];
    std::io::Read::read_exact(&mut rx.into(), &mut buf).unwrap();
    assert_eq!(&buf, b"hello");
}
```

### Test: `test_sys_read_returns_data`

```rust
#[test]
fn test_sys_read_returns_data() {
    let (rx, tx) = os_pipe::pipe().unwrap();
    std::io::Write::write_all(&mut (&tx).into(), b"world").unwrap();
    drop(tx);

    let mut ctx = MockThreadContext::new();
    let mem = MockMemoryMap::new();
    let mut process = LinuxProcess::new(0x8000_0000, 0xC000_0000);
    let guest_fd = process.fds.insert(rx.into_raw_fd());

    let mut handler = LinuxSyscallHandler::new(process, Arc::new(Mutex::new(mem)));
    let buf_addr = 0x2000u64;
    let args = SyscallArgs::new(guest_fd as u64, buf_addr, 5, 0, 0, 0);

    let result = handler.handle(63, args, &mut ctx);  // nr 63 = read

    assert!(matches!(result, SyscallResult::Ok(5)));

    let data = ctx.read_mem(buf_addr, 5);
    assert_eq!(&data, b"world");
}
```

### Test: `test_sys_openat_close_roundtrip`

```rust
#[test]
fn test_sys_openat_close_roundtrip() {
    use std::io::Write;

    // Write a temp file to open.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    tmp.as_file().write_all(b"content").unwrap();

    let mut ctx = MockThreadContext::new();
    let mem = MockMemoryMap::new();
    let mut process = LinuxProcess::new(0x8000_0000, 0xC000_0000);

    // Place null-terminated path in mock guest memory.
    let path_addr = 0x3000u64;
    ctx.write_cstring(path_addr, &path);

    let mem_arc = Arc::new(Mutex::new(mem));
    let mut handler = LinuxSyscallHandler::new(process, Arc::clone(&mem_arc));

    // openat(AT_FDCWD, path, O_RDONLY, 0)
    let open_args = SyscallArgs::new(libc::AT_FDCWD as u64, path_addr, libc::O_RDONLY as u64, 0, 0, 0);
    let open_result = handler.handle(56, open_args, &mut ctx);

    let guest_fd = match open_result {
        SyscallResult::Ok(fd) => fd as i32,
        other => panic!("openat failed: {other:?}"),
    };

    assert!(guest_fd >= 3, "guest fd must be >= 3");

    // close(fd)
    let close_args = SyscallArgs::new(guest_fd as u64, 0, 0, 0, 0, 0);
    let close_result = handler.handle(57, close_args, &mut ctx);
    assert!(matches!(close_result, SyscallResult::Ok(0)));
}
```

### Test: `test_sys_brk_allocates_heap`

```rust
#[test]
fn test_sys_brk_allocates_heap() {
    let mut ctx = MockThreadContext::new();
    let mem = MockMemoryMap::new();
    let brk_base = 0x8000_0000u64;
    let mut process = LinuxProcess::new(brk_base, 0xC000_0000);

    let mem_arc = Arc::new(Mutex::new(mem));
    let mut handler = LinuxSyscallHandler::new(process, Arc::clone(&mem_arc));

    // Query current brk (arg0 = 0).
    let result = handler.handle(214, SyscallArgs::new(0, 0, 0, 0, 0, 0), &mut ctx);
    assert_eq!(result, SyscallResult::Ok(brk_base as i64));

    // Extend brk by 4096 bytes.
    let new_brk = brk_base + 4096;
    let result = handler.handle(214, SyscallArgs::new(new_brk, 0, 0, 0, 0, 0), &mut ctx);
    assert_eq!(result, SyscallResult::Ok(new_brk as i64));

    // Verify brk_addr updated.
    let result2 = handler.handle(214, SyscallArgs::new(0, 0, 0, 0, 0, 0), &mut ctx);
    assert_eq!(result2, SyscallResult::Ok(new_brk as i64));
}
```

### Test: `test_sys_mmap_anonymous`

```rust
#[test]
fn test_sys_mmap_anonymous() {
    let mut ctx = MockThreadContext::new();
    let mem = MockMemoryMap::new();
    let mut process = LinuxProcess::new(0x8000_0000, 0xC000_0000);
    let initial_mmap_next = process.mmap_next;

    let mem_arc = Arc::new(Mutex::new(mem));
    let mut handler = LinuxSyscallHandler::new(process, Arc::clone(&mem_arc));

    // mmap(0, 4096, PROT_READ|PROT_WRITE, MAP_ANONYMOUS|MAP_PRIVATE, -1, 0)
    const MAP_PRIVATE: u64 = 0x02;
    const MAP_ANONYMOUS: u64 = 0x20;
    const PROT_READ_WRITE: u64 = 0x03;

    let args = SyscallArgs::new(0, 4096, PROT_READ_WRITE, MAP_ANONYMOUS | MAP_PRIVATE, u64::MAX, 0);
    let result = handler.handle(222, args, &mut ctx);

    let mapped_addr = match result {
        SyscallResult::Ok(addr) => addr as u64,
        other => panic!("mmap failed: {other:?}"),
    };

    assert_eq!(mapped_addr, initial_mmap_next, "mmap must return mmap_next when hint=0");

    // Verify the region is accessible in the MemoryMap.
    let mem_locked = mem_arc.lock().unwrap();
    assert!(
        mem_locked.region_at(mapped_addr).is_some(),
        "mmap'd region must be present in MemoryMap"
    );
}
```

### Test: `test_sys_exit_terminates`

Since `sys_exit` calls `std::process::exit()`, we test it in a subprocess:

```rust
#[test]
fn test_sys_exit_terminates() {
    // Run hello_world (which calls exit(0) at the end) in a subprocess
    // and verify the process terminates with exit code 0.
    let status = std::process::Command::new(
        std::env::current_exe().unwrap()
    )
    .args(["--test", "se_hello_world_stdout"])
    .env("HELM_TEST_EXIT_TRAP", "1")
    .status()
    .unwrap();

    assert!(status.success(), "simulation process must exit cleanly");
}
```

---

## 3. SyscallAbi Unit Tests

### Test: `test_riscv_extract_args`

```rust
use helm_se::abi::{RiscvSyscallAbi, SyscallAbi};
use helm_test::MockThreadContext;

#[test]
fn test_riscv_extract_args() {
    let mut ctx = MockThreadContext::new();
    // Simulate: write(fd=1, buf=0x1000, count=14)
    // a7=64 (write), a0=1, a1=0x1000, a2=14
    ctx.set_int_reg(17, 64);      // a7: syscall nr
    ctx.set_int_reg(10, 1);       // a0: fd
    ctx.set_int_reg(11, 0x1000);  // a1: buf
    ctx.set_int_reg(12, 14);      // a2: count
    ctx.set_int_reg(13, 0);
    ctx.set_int_reg(14, 0);
    ctx.set_int_reg(15, 0);

    let abi = RiscvSyscallAbi;
    let (nr, args) = abi.extract_args(&ctx);

    assert_eq!(nr, 64, "syscall nr must be from a7");
    assert_eq!(args.arg0(), 1, "arg0 (fd) from a0");
    assert_eq!(args.arg1(), 0x1000, "arg1 (buf) from a1");
    assert_eq!(args.arg2(), 14, "arg2 (count) from a2");
}
```

### Test: `test_riscv_set_return`

```rust
#[test]
fn test_riscv_set_return() {
    let mut ctx = MockThreadContext::new();
    let abi = RiscvSyscallAbi;

    abi.set_return(&mut ctx, 14);
    assert_eq!(ctx.read_int_reg(10), 14, "return value must go into a0");

    abi.set_return(&mut ctx, -22i64);  // EINVAL
    assert_eq!(ctx.read_int_reg(10) as i64, -22, "negative return for errno");
}
```

### Test: `test_aarch64_extract_args`

```rust
use helm_se::abi::Aarch64SyscallAbi;

#[test]
fn test_aarch64_extract_args() {
    let mut ctx = MockThreadContext::new();
    // AArch64: write(fd=1, buf=0x2000, count=5) → x8=64, x0=1, x1=0x2000, x2=5
    ctx.set_int_reg(8, 64);      // x8: syscall nr
    ctx.set_int_reg(0, 1);       // x0: fd
    ctx.set_int_reg(1, 0x2000);  // x1: buf
    ctx.set_int_reg(2, 5);       // x2: count

    let abi = Aarch64SyscallAbi;
    let (nr, args) = abi.extract_args(&ctx);

    assert_eq!(nr, 64);
    assert_eq!(args.arg0(), 1);
    assert_eq!(args.arg1(), 0x2000);
    assert_eq!(args.arg2(), 5);
}
```

### Test: `test_aarch64_set_return`

```rust
#[test]
fn test_aarch64_set_return() {
    let mut ctx = MockThreadContext::new();
    let abi = Aarch64SyscallAbi;

    abi.set_return(&mut ctx, 42);
    assert_eq!(ctx.read_int_reg(0), 42, "AArch64 return value must go into x0");
}
```

---

## 4. Multiple Syscalls in Sequence

**Goal:** Verify that a sequence of syscalls produces the correct cumulative result, matching host behavior.

### Test: `test_multiple_syscalls_sequence`

```rust
#[test]
fn test_multiple_syscalls_sequence() {
    use helm_se::{LinuxSyscallHandler, SyscallHandler, SyscallResult};
    use helm_test::{MockThreadContext, MockMemoryMap};

    let (rx_pipe, tx_pipe) = os_pipe::pipe().unwrap();
    let mut ctx = MockThreadContext::new();
    let mem = MockMemoryMap::new();
    let mut process = LinuxProcess::new(0x8000_0000, 0xC000_0000);
    let guest_fd = process.fds.insert(tx_pipe.into_raw_fd());
    let mem_arc = Arc::new(Mutex::new(mem));
    let mut handler = LinuxSyscallHandler::new(process, Arc::clone(&mem_arc));

    // Syscall 1: write "Hello" (5 bytes)
    ctx.write_mem(0x1000, b"Hello");
    let r1 = handler.handle(64, SyscallArgs::new(guest_fd as u64, 0x1000, 5, 0, 0, 0), &mut ctx);
    assert_eq!(r1, SyscallResult::Ok(5), "first write must return 5");

    // Syscall 2: write ", World!\n" (9 bytes)
    ctx.write_mem(0x2000, b", World!\n");
    let r2 = handler.handle(64, SyscallArgs::new(guest_fd as u64, 0x2000, 9, 0, 0, 0), &mut ctx);
    assert_eq!(r2, SyscallResult::Ok(9), "second write must return 9");

    // Close the write end and read from the pipe.
    drop(handler);
    let mut buf = String::new();
    std::io::Read::read_to_string(&mut rx_pipe.into(), &mut buf).unwrap();
    assert_eq!(buf, "Hello, World!\n");
}
```

---

## Test Matrix

| Test | Type | Phase | ISA |
|------|------|-------|-----|
| Run hello_world, verify stdout | End-to-end | Phase 0 | RISC-V |
| Run hello_world AArch64 | End-to-end | Phase 0 | AArch64 |
| `sys_write` returns byte count | Unit | Phase 0 | ISA-neutral |
| `sys_read` returns data | Unit | Phase 0 | ISA-neutral |
| `sys_openat` + `sys_close` round-trip | Unit | Phase 0 | ISA-neutral |
| `sys_brk` query + extend | Unit | Phase 0 | ISA-neutral |
| `sys_mmap` anonymous creates region | Unit | Phase 0 | ISA-neutral |
| `sys_exit` terminates process | Subprocess | Phase 0 | ISA-neutral |
| RISC-V `extract_args` correct registers | Unit | Phase 0 | RISC-V |
| RISC-V `set_return` writes `a0` | Unit | Phase 0 | RISC-V |
| AArch64 `extract_args` correct registers | Unit | Phase 0 | AArch64 |
| AArch64 `set_return` writes `x0` | Unit | Phase 0 | AArch64 |
| Multiple syscalls in sequence | Integration | Phase 0 | ISA-neutral |

### Running the Tests

```bash
# All helm-engine/se unit tests
cargo test -p helm-engine

# End-to-end tests (requires cross-compiled binaries in tests/fixtures/)
cargo test -p helm-engine --test se_end_to_end -- --nocapture

# Build test fixtures (requires riscv64 and aarch64 musl toolchains)
make -C tests/fixtures

# Verbose syscall tracing during tests
RUST_LOG=helm_se=debug cargo test -p helm-engine -- --nocapture
```

### Test Fixture Dependencies

| Binary | Build command | Required toolchain |
|--------|---------------|-------------------|
| `hello_world.riscv64` | `riscv64-linux-musl-gcc -static -O2 -o $@ $<` | `riscv64-linux-musl-gcc` |
| `hello_world.aarch64` | `aarch64-linux-musl-gcc -static -O2 -o $@ $<` | `aarch64-linux-musl-gcc` |
