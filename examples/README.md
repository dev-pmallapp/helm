# HELM Examples

Python scripts for `helm-system-aarch64`.  Each takes `--help`.

```
examples/
  fs/            Full-system boot scripts
    virt.py        ARM virt (GIC + PL011 + VirtIO)
    rpi3.py        Raspberry Pi 3 (BCM2837)
  se/            Syscall-emulation scripts
    run_binary.py  Run an AArch64 static ELF
  debug/         Diagnostic / development tools
    benchmark.py        MIPS per boot phase
    boot_progress.py    Track PC / EL through boot
    compare_backends.py JIT vs interpreter divergence finder
    dump_sysregs.py     Print MMU / exception registers
    read_memory.py      Hexdump physical or virtual memory
  tmp/           Scratch area for one-off debugging scripts (git-ignored)
```

## Quick start

```bash
# Full-system — boot Linux on ARM virt
helm-system-aarch64 examples/fs/virt.py

# Syscall-emulation — run a binary
helm-system-aarch64 examples/se/run_binary.py

# Debug — find JIT/interp divergence
helm-system-aarch64 examples/debug/compare_backends.py
```

## Prerequisites

- **FS mode**: kernel image at `assets/alpine/boot/vmlinuz-rpi`
- **SE mode**: AArch64 static binary (default `assets/binaries/fish`, override with `--binary` or `HELM_BINARY`)
