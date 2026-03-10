# Installation

## Prerequisites

| Requirement | Version | Purpose |
|-------------|---------|---------|
| Rust toolchain | 1.75+ (stable) | Building all crates |
| Python | 3.9+ | Configuration layer, tests |
| maturin | latest | Building `helm-python` (optional) |
| AArch64 cross-compiler | any | Building guest binaries for SE mode |
| Linux kernel Image | 5.x+ | FS mode testing |

## Rust Toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable
```

## Building

```bash
make check       # Fast workspace-wide type check
make test        # All Rust tests (excludes helm-python)
make fmt         # Format all code
make clippy      # Lint with warnings-as-errors
make pre-commit  # All of the above
```

## Python Bindings (Optional)

The `helm-python` crate provides native bindings via PyO3:

```bash
cd crates/helm-python
pip install maturin
maturin develop --features extension-module
```

After building, `import _helm_core` becomes available in Python.
Without the native bindings, the Python layer falls back to a
pure-Python stub.

## Platform Support

| Host | SE Mode | FS Mode | KVM |
|------|---------|---------|-----|
| x86-64 Linux | ✅ (Cranelift JIT) | ✅ (interp/JIT) | ❌ |
| AArch64 Linux | ✅ | ✅ | ✅ |
| macOS (x86/ARM) | ✅ (tests pass) | ✅ (no KVM) | ❌ |

## Dependencies

All external dependencies are managed at the workspace level in the
root `Cargo.toml`. Key external crates:

- `cranelift-*` 0.116 — JIT compilation backend.
- `pyo3` 0.24 — Python bindings.
- `thiserror` / `anyhow` — error handling.
- `clap` 4 — CLI argument parsing.
- `serde` / `serde_json` — serialisation.
- `flate2` — gzip decompression for kernel images.
- `libc` — syscall passthrough in SE mode.
