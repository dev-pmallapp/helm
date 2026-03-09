# Coding Style

## Rust

- **Edition**: 2021.
- **Dependencies**: managed at workspace level in root `Cargo.toml`.
- **Error handling**: `thiserror` for library error types, `anyhow` at
  binary/integration boundaries.
- **Doc comments**: all public items must have `///` or `//!` comments.
- **Modules**: prefer many small, focused modules over large files.
- **Formatting**: `cargo fmt --all` (enforced by CI).
- **Linting**: `cargo clippy -- -D warnings` (warnings are errors).

### Naming

- Types: `PascalCase` (e.g. `Aarch64Cpu`, `DeviceBus`).
- Functions / methods: `snake_case` (e.g. `translate_insn`).
- Constants: `SCREAMING_SNAKE_CASE` (e.g. `GICD_CTLR`).
- Crates: `helm-*` with hyphens (e.g. `helm-device`).
- Modules: `snake_case` (e.g. `address_space.rs`).

### Error Types

Each crate may define local error types with `thiserror`. The shared
`HelmError` in `helm-core` is the top-level error type.

## Python

- **Version**: 3.9+.
- **Style**: PEP 8, enforced by the project's conventions.
- **Tests**: `unittest` framework in `python/tests/`.
- **Type hints**: used for function signatures.
- **Docstrings**: Google-style or NumPy-style.
