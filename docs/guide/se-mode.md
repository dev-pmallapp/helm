# Syscall-Emulation Mode

Run user-space AArch64 binaries without an OS kernel.

## What SE Mode Simulates

- All user-space AArch64 instructions via the CPU executor.
- Virtual address space backed by host memory.
- ELF binary loading (static, AArch64 ELF64 only).
- ~50 Linux syscalls via libc passthrough.
- File descriptor table with stdin/stdout/stderr pre-seeded.
- Heap management via `brk` and anonymous `mmap`.
- All timing models (FE / ITE / CAE).
- Plugin callbacks for instrumentation.

## What SE Mode Does Not Simulate

- OS kernel code (zero kernel instructions execute).
- Interrupt controllers or interrupt delivery.
- Device drivers or I/O devices.
- Process scheduler or preemption.
- Dynamic linking (static binaries only).
- `/proc`, `/sys` filesystems.

## Usage

### Command Line

```bash
# Basic run
helm-aarch64 ./binary

# With arguments
helm-aarch64 ./binary arg1 arg2

# With environment variables
helm-aarch64 -E HOME=/tmp -E LANG=C ./binary

# With timing model
helm-aarch64 --cpu o3 ./binary

# With plugins
helm-aarch64 --plugin insn-count --plugin hotblocks ./binary

# With syscall tracing
helm-aarch64 -strace ./binary

# Instruction limit
helm-aarch64 -n 5000000 ./binary
```

### Python API

```python
from helm.session import SeSession

s = SeSession("./binary", ["binary"], ["HOME=/tmp"])
result = s.run(10_000_000)
print(f"PC={s.pc:#x}, insns={s.insn_count}")
```

## Supported Syscalls

Key AArch64 Linux syscalls implemented in `helm-syscall`:

| Nr | Name | Nr | Name |
|----|------|----|------|
| 25 | fcntl | 64 | write |
| 29 | ioctl | 73 | ppoll |
| 56 | openat | 93 | exit |
| 57 | close | 94 | exit_group |
| 63 | read | 113 | clock_gettime |
| 78 | readlinkat | 134 | rt_sigaction |
| 160 | uname | 172 | getpid |
| 174 | getuid | 214 | brk |
| 222 | mmap | 226 | mprotect |
| 278 | getrandom | 261 | prlimit64 |

## Binary Requirements

- **Architecture**: AArch64 (EM_AARCH64 = 183).
- **Linking**: Static (no dynamic linker support).
- **Format**: ELF64, little-endian.
- **Entry point**: Standard ELF `e_entry`.
