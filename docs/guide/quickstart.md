# Quickstart

Build HELM from source, run your first SE binary, and boot a kernel.

## Prerequisites

- **Rust** — 1.75+ with the stable toolchain (`rustup default stable`).
- **Python** — 3.9+ (for the configuration layer and tests).
- **Linux host** — x86-64 or AArch64 (KVM backend requires AArch64 host).
- **Cross-compiled binaries** — AArch64 static ELF binaries for SE mode.

## Build

```bash
git clone https://github.com/helm-sim/helm.git
cd helm
make check   # fast cargo check (excludes helm-python)
make test    # run all Rust tests
```

## First SE Run

Run a statically-linked AArch64 binary:

```bash
cargo run --release --bin helm -- -b ./hello-aarch64 --max-insns 10000000
```

Or using the `helm-aarch64` runner with plugins:

```bash
cargo run --release --bin helm-aarch64 -- ./hello-aarch64
cargo run --release --bin helm-aarch64 -- -strace ./hello-aarch64
cargo run --release --bin helm-aarch64 -- --plugin insn-count ./hello-aarch64
```

## First FS Boot

Boot an AArch64 Linux kernel on the `virt` machine:

```bash
cargo run --release --bin helm-system-aarch64 -- \
    -M virt \
    --kernel path/to/Image \
    -m 256M \
    --serial stdio
```

Or with a Python configuration script:

```bash
cargo run --release --bin helm-system-aarch64 -- examples/fs/virt.py
```

## Python Configuration

```bash
PYTHONPATH=python python3 examples/se/run_binary.py
```

## Verify

```bash
make pre-commit   # fmt-check + clippy + test
make test-python  # Python unit tests
```
