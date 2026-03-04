# Contributing to HELM

## Test-Driven Development (TDD) Workflow

Every change to this repository **must** follow a strict red-green-refactor
cycle.  This applies to human contributors and AI agents alike.

### The Cycle

1. **Red** — Write a failing test that describes the desired behaviour.
   Run `make test` and confirm the new test fails.
2. **Green** — Write the minimum code to make the test pass.
   Run `make test` and confirm all tests pass.
3. **Refactor** — Clean up the code while keeping all tests green.
   Run `make test` one more time before committing.

### Rules

- **No production code without a test.**
  If you add or change behaviour, there must be a corresponding test.
- **Tests live next to the code.**
  Use inline `#[cfg(test)] mod tests` blocks in each Rust source file.
  Integration tests go in `crates/<crate>/tests/`.
- **Run the full suite before committing.**
  `make pre-commit` runs formatting, linting, and all tests in one shot.
- **Never commit with broken tests.**

### Quick Reference

```bash
make test           # Run all Rust tests (excludes helm-python)
make check          # cargo check (fast compile verification)
make clippy         # Lint with clippy
make fmt            # Format code with rustfmt
make fmt-check      # Check formatting without modifying files
make pre-commit     # fmt-check + clippy + test (run before every commit)
make test-python    # Run Python-side tests
```

---

## Repository Layout

```
Cargo.toml                  Workspace root
crates/
  helm-core/                Shared types, IR, config, error, events
  helm-isa/                 ISA frontend trait + x86/riscv/arm stubs
  helm-pipeline/            OOO pipeline: ROB, rename, scheduler, branch pred
  helm-memory/              Cache hierarchy, TLB, address space, coherence
  helm-translate/           Dynamic binary translation engine
  helm-syscall/             Syscall emulation layer
  helm-engine/              Simulation orchestrator
  helm-stats/               Statistics collection
  helm-python/              PyO3 bindings (cdylib)
  helm-cli/                 CLI binary
python/helm/                Python configuration package (GEM5-style API)
examples/                   Example Python configuration scripts
```

### Crate Dependency Graph

```
helm-core  (no internal deps — everything depends on this)
  |
  +-- helm-isa              (core)
  +-- helm-pipeline         (core)
  +-- helm-memory           (core)
  +-- helm-stats            (core)
  +-- helm-translate        (core, isa)
  +-- helm-syscall          (core, memory)
  +-- helm-engine           (core, isa, pipeline, memory, translate, syscall, stats)
  +-- helm-python           (core, engine, stats)
  +-- helm-cli              (core, engine)
```

---

## Conventions

### Rust

- Edition 2021, workspace-level dependency versions.
- `thiserror` for error types, `anyhow` at binary/integration boundaries.
- Public items get a doc comment (`///` or `//!`).
- Keep each module focused; prefer many small files over large ones.

### Python

- Pure-Python config layer lives in `python/helm/`.
- The `Simulation.run()` method tries the native Rust engine first, then
  falls back to a stub for development without a compiled extension.
- Python tests live in `python/tests/` and run with `pytest`.

### Commits

- Use conventional-style subjects: `feat:`, `fix:`, `test:`, `refactor:`,
  `docs:`, `chore:`.
- Keep the subject line under 72 characters.
- Do **not** include AI tool attribution (e.g. "Co-authored-by: …") in
  commit messages.

### What NOT to Commit

The `.gitignore` excludes AI-agent state directories.  Never commit any of:

- `.codex/`, `.claude/`, `.cline_storage/`, `.cursor/`, `.aider*`,
  `.continue/`, `.copilot/`
- `target/`, `__pycache__/`, `*.pyc`, `*.so`

---

## Adding a New Feature (Step-by-Step)

1. Identify which crate(s) are affected.
2. Write one or more `#[test]` functions in the relevant `mod tests` block.
3. Run `make test` — confirm the new tests fail.
4. Implement the feature.
5. Run `make test` — confirm everything passes.
6. Run `make pre-commit` — ensure formatting and lints are clean.
7. Commit with a descriptive message.

## Adding a New Crate

1. Create `crates/<name>/` with `Cargo.toml` and `src/lib.rs`.
2. Add the crate to `[workspace.members]` and `[workspace.dependencies]`
   in the root `Cargo.toml`.
3. Include at least one `#[cfg(test)] mod tests` block with a smoke test.
4. Run `make test` before committing.
