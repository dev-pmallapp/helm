# HELM Examples

Python scripts for the embedded interpreter in `helm-system-aarch64`.

## Usage

```bash
helm-system-aarch64 examples/fs_boot_kernel.py
helm-system-aarch64 examples/fs_benchmark.py
helm-system-aarch64 examples/se_run_binary.py
```

## Scripts

| Script | Mode | Description |
|--------|------|-------------|
| `se_run_binary.py` | SE | Run an AArch64 static binary with phased execution |
| `fs_boot_kernel.py` | FS | Boot a Linux kernel on the ARM virt platform |
| `fs_benchmark.py` | FS | Measure MIPS throughput per boot phase |

## Prerequisites

- **FS mode**: Kernel image at `assets/alpine/boot/vmlinuz-rpi`
- **SE mode**: AArch64 static binary (default: `assets/binaries/fish`, override with `HELM_BINARY`)
