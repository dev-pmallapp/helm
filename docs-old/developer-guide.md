# Developer Guide

This guide is for developers contributing to HELM, providing information on building, code structure, contributing guidelines, and development workflows.

## Building HELM

HELM uses a Rust workspace with a Python extension. All builds are orchestrated via the root `Makefile`.

### Prerequisites

- Rust 1.70+ (edition 2021)
- Python 3.9+
- `maturin` for building Python extensions
- Standard build tools (make, etc.)

### Build Commands

- `make check`: Fast compilation check across all Rust crates (excludes `helm-python`).
- `make test`: Run all Rust tests.
- `make test-python`: Run Python tests.
- `make fmt`: Format code with `rustfmt`.
- `make clippy`: Lint with Clippy (treats warnings as errors).
- `make pre-commit`: Full pre-commit check (`fmt-check` + `clippy` + `test`).
- `make clean`: Clean build artifacts.

To build the Python extension: Run `maturin develop` in the `python/` directory or use the Makefile targets.

### Development Workflow

1. **Red-Green-Refactor TDD Cycle**:
   - Write failing tests first.
   - Implement minimal code to pass.
   - Refactor while keeping tests green.

2. **Pre-commit Checks**: Always run `make pre-commit` before committing. No commits with failing tests or lint errors.

3. **Conventional Commits**: Use subjects like `feat:`, `fix:`, `test:`, `refactor:`, `docs:`, `chore:`.

## Code Structure

HELM is a modular Rust workspace with 19 crates, all depending on `helm-core`.

### Key Crates

- **`helm-core`**: Shared types, IR, errors, utilities.
- **`helm-engine`**: Main simulation engine (SE and microarch modes).
- **`helm-isa`**: ISA abstraction and frontends.
- **`helm-pipeline`**: OOO pipeline modeling.
- **`helm-memory`**: Memory hierarchy.
- **`helm-translate`**: Dynamic translation (Cranelift JIT).
- **`helm-syscall`**: Syscall emulation.
- **`helm-python`**: PyO3 bindings for Python API.
- **`helm-cli`**: Command-line interface.

### Python Layer

- `python/helm/`: High-level API for configuration (gem5-style).
- Key classes: `Core`, `SeSession`, platforms, devices.

### Adding New Features

- **New ISA**: Add frontend in `helm-isa`, implement decoding/lifting.
- **New Microarch**: Extend `helm-pipeline` or `helm-memory`.
- **Python API**: Add to `python/helm/`, expose via `helm-python`.

## Contributing

- Follow Rust style: `rustfmt`, Clippy clean.
- All public items must have `///` doc comments.
- No production code without tests.
- Tests in `src/tests/` per crate, Python tests in `python/tests/`.

## APIs and Interfaces

- Rust: Traits in `helm-core` for extensibility.
- Python: Classes in `python/helm/` for scripting.

For detailed API docs, see inline comments and examples in `examples/`.

## Debugging and Profiling

- Use `cargo build --profile profiling` for debug symbols in release.
- Profiling targets in `target/profiling/`.
- For Python debugging, use standard Python debuggers on scripts.