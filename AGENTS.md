# Repository Guidelines

## Project Structure

HELM is a Rust workspace with a Python configuration layer. Key paths:

- `crates/` — 18 Rust crates (e.g. `helm-core`, `helm-isa`, `helm-engine`). `helm-core` is the shared foundation; all other crates depend on it.
- `python/helm/` — Pure-Python configuration package (gem5-style API). Native bindings come from the `helm-python` crate via PyO3 / maturin.
- `python/tests/` — Python-side tests (`test_*.py`).
- `examples/` — Example Python configuration scripts.
- `docs/` — Design and research documents.
- `assets/` — Binary assets (e.g. Alpine rootfs).

## Build, Test & Development Commands

All commands are defined in the root `Makefile`:

- `make check` — Fast `cargo check` across the workspace (excludes `helm-python`).
- `make test` — Run all Rust tests (excludes `helm-python`).
- `make test-python` — Run Python tests via `unittest discover`.
- `make fmt` / `make fmt-check` — Format (or verify formatting) with `rustfmt`.
- `make clippy` — Lint with Clippy; treats warnings as errors (`-D warnings`).
- `make pre-commit` — `fmt-check` + `clippy` + `test`. **Run before every commit.**
- `make clean` — `cargo clean`.

## Coding Style & Naming Conventions

### Rust

- Edition 2021; dependency versions are managed at the workspace level in the root `Cargo.toml`.
- Use `thiserror` for library error types and `anyhow` at binary/integration boundaries.
- All public items must have a doc comment (`///` or `//!`).
- Prefer many small, focused modules over large files.
- Formatting: `cargo fmt --all`. Linting: `cargo clippy -- -D warnings`.

### Python

- Config layer lives in `python/helm/`. Python ≥ 3.9 is required.
- Tests in `python/tests/` use the `unittest` framework.

## Testing Guidelines

HELM follows a strict **red-green-refactor** TDD cycle:

1. **Red** — Write a failing test first. Run `make test` to confirm.
2. **Green** — Write the minimum code to pass. Run `make test`.
3. **Refactor** — Clean up while keeping tests green. Run `make test` again.

### Test Organisation

- Each crate keeps tests in `src/tests/`, one file per source module (e.g. `src/tests/rob.rs` tests `src/rob.rs`).
- Wire the module via `#[cfg(test)] mod tests;` in `src/lib.rs`.
- **No test code in production source files.**
- No production code without a corresponding test.

## Commit & Pull Request Guidelines

- Use conventional-commit subjects: `feat:`, `fix:`, `test:`, `refactor:`, `docs:`, `chore:`.
- Keep the subject line under 72 characters.
- Do **not** include AI-tool attribution (e.g. `Co-authored-by: …`) in commit messages.
- Run `make pre-commit` before committing — never commit with broken tests or lint failures.

## What Not to Commit

The `.gitignore` excludes AI-agent state directories. Never commit:

- `.codex/`, `.claude/`, `.cline_storage/`, `.cursor/`, `.aider*`, `.continue/`, `.copilot/`
- `target/`, `__pycache__/`, `*.pyc`, `*.so`
