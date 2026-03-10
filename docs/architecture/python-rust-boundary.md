# Python ↔ Rust Boundary

How the Python configuration layer communicates with Rust simulation
internals.

## Architecture

```text
python/helm/          Pure Python config classes
    │
    │  import _helm_core
    ▼
helm-python           PyO3 cdylib crate
    │
    │  links to:
    ▼
helm-engine           Rust simulation engine
helm-plugin           Plugin infrastructure
helm-core             Shared types
```

## PyO3 Bindings (helm-python)

The `helm-python` crate builds a Python extension module called
`_helm_core` via PyO3 / maturin. It exposes:

- `SeSession` — wraps `helm_engine::SeSession` with `run()`,
  `run_until_pc()`, `add_plugin()`, and register inspection.
- `FsSession` — wraps `helm_engine::FsSession` with `run()`,
  `run_until_symbol()`, and memory/register access.

## Python Config Classes

The pure-Python layer in `python/helm/` mirrors Rust structs:

| Python Class | Rust Struct |
|-------------|-------------|
| `Platform` | `PlatformConfig` |
| `Core` | `CoreConfig` |
| `MemorySystem` / `Cache` | `MemoryConfig` / `CacheConfig` |
| `BranchPredictor` | `BranchPredictorConfig` |
| `TimingModel` | `AccuracyLevel` + model params |
| `SeSession` | `helm_engine::SeSession` |
| `FsSession` | `helm_engine::FsSession` |

## Serialisation

Python `Platform.to_dict()` produces a JSON-compatible dict that
matches `PlatformConfig`'s serde layout. This dict can be:

1. Passed to the Rust engine via `_helm_core`.
2. Written to a JSON file for reproducibility.
3. Diffed across runs for parameter sweeps.

## Embedded Python

The `helm-aarch64` and `helm-system-aarch64` binaries embed a Python
interpreter (via `pyo3::prepare_freethreaded_python`). When given a
`.py` script they:

1. Register `_helm_core` with `pyo3::append_to_inittab!`.
2. Insert `python/` and the script's directory into `sys.path`.
3. Execute the script with `py.run()`.

This gives scripts direct access to `SeSession`, `FsSession`, and all
Python config classes without needing a separate `maturin` build.

## Dual State: CPU Struct vs Sysreg Array

In the TCG path, guest architectural state exists in two forms:

1. **CPU struct** (`Aarch64Regs`) — named fields for each register.
2. **Sysreg array** (`TcgInterp::sysregs`) — flat `Vec<u64>` indexed
   by the 15-bit sysreg encoding.

Sync functions (`regs_to_array`, `array_to_regs`) copy between them at
block boundaries. See [sysreg-sync.md](../internals/sysreg-sync.md).
