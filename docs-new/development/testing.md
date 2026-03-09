# Testing

## TDD Workflow

HELM follows strict red-green-refactor:

1. **Red** — write a failing test. Run `make test` to confirm.
2. **Green** — write the minimum code to pass. Run `make test`.
3. **Refactor** — clean up while keeping tests green. Run `make test`.

## Test Organisation

- Each crate keeps tests in `src/tests/`, one file per source module.
- Wire via `#[cfg(test)] mod tests;` in `src/lib.rs`.
- No test code in production source files.
- No production code without a corresponding test.

Example structure:
```
crates/helm-memory/src/
  cache.rs
  tlb.rs
  tests/
    mod.rs
    cache.rs
    tlb.rs
```

## Commands

| Command | Scope |
|---------|-------|
| `make test` | All Rust tests (excludes `helm-python`) |
| `make test-python` | Python unittest discovery |
| `make pre-commit` | fmt-check + clippy + test |
| `cargo test -p helm-isa` | Single crate |
| `cargo test -p helm-isa -- decode` | Filtered tests |

## Python Tests

Located in `python/tests/`:

- `test_config.py` — platform configuration.
- `test_session.py` — SE session API.
- `test_fault_detect.py` — fault detection plugin.

Run: `PYTHONPATH=python python3 -m unittest discover -s python/tests`

## Parity Tests

`helm-tcg/src/tests/parity.rs` and `jit_parity.rs` verify that the
interpreter and JIT produce identical results for the same TCG blocks.
