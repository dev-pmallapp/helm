# CLI Reference

## helm

Generic entry point:

```
helm -b <binary> [--isa arm64] [--mode se] [--max-insns N] [-- guest args...]
```

| Flag | Default | Description |
|------|---------|-------------|
| `-b, --binary` | (required) | Path to guest binary |
| `-i, --isa` | `arm64` | Target ISA: `arm64`, `riscv64`, `x86-64` |
| `-m, --mode` | `se` | Execution mode: `se` or `cae` |
| `--max-insns` | 100000000 | Maximum instructions |

## helm-arm

AArch64 SE mode runner with full plugin and timing support:

```
helm-arm [options] <binary> [guest args...]
helm-arm script.py
```

| Flag | Default | Description |
|------|---------|-------------|
| `-n, --max-insns` | 100000000 | Maximum instructions (0 = unlimited) |
| `--cpu` | `atomic` | CPU model: `atomic`, `timing`, `minor`, `o3`, `big` |
| `--caches` | false | Enable L1 caches |
| `--l2cache` | false | Enable L2 cache |
| `-E VAR=VALUE` | | Set target environment variable (repeatable) |
| `--strace` | false | Log system calls |
| `--plugin NAME` | | Enable a plugin (repeatable) |

Plugin names: `insn-count`, `execlog`, `hotblocks`, `howvec`,
`syscall-trace`, `fault-detect`, `cache`.

## helm-system-aarch64

Full-system AArch64 simulator:

```
helm-system-aarch64 [options]
helm-system-aarch64 script.py
```

| Flag | Default | Description |
|------|---------|-------------|
| `-M, --machine` | `virt` | Machine type: `virt`, `realview-pb`, `rpi3` |
| `--kernel` | | Kernel image path |
| `--dtb` | | Device tree blob |
| `--sd` | | SD card / disk image |
| `--drive SPEC` | | Drive specification |
| `--device SPEC` | | Add a device |
| `--serial` | `stdio` | Serial backend: `stdio`, `null`, `file:path` |
| `--smp` | 1 | Number of CPUs |
| `-m, --memory` | `256M` | RAM size |
| `--append` | | Kernel command line |
| `--timing` | `fe` | Timing model: `fe`, `ite` |
| `--backend` | `jit` | Execution backend: `jit`, `interp` |
| `--monitor` | false | Enable debug monitor |
| `--sysmap` | | Path to System.map |
