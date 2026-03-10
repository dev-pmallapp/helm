# CI and Release

## CI Pipeline

The `make pre-commit` target is the CI gate:

1. `fmt-check` — verify `cargo fmt` compliance.
2. `clippy` — lint with `-D warnings` (warnings are errors).
3. `test` — run all Rust tests (excludes `helm-python`).

All three must pass before committing.

## Makefile Targets

| Target | What It Does |
|--------|-------------|
| `make check` | Fast `cargo check` across workspace |
| `make test` | Run all Rust tests |
| `make test-python` | Python unittest discovery |
| `make fmt` | Format with `cargo fmt --all` |
| `make fmt-check` | Verify formatting |
| `make clippy` | Lint with Clippy |
| `make pre-commit` | fmt-check + clippy + test |
| `make clean` | `cargo clean` |

## Cargo Flags

Tests exclude `helm-python` by default:
`--offline --workspace --exclude helm-python`

## Versioning

All crates share a workspace version (`0.1.0`) defined in the root
`Cargo.toml` under `[workspace.package]`.

## Crate Publishing

Crates are published in dependency order:
1. `helm-core`
2. `helm-object`, `helm-stats`
3. `helm-memory`, `helm-timing`, `helm-decode`
4. `helm-isa`, `helm-pipeline`, `helm-syscall`
5. `helm-tcg`, `helm-translate`, `helm-device`
6. `helm-plugin`, `helm-engine`
7. `helm-python`, `helm-cli`
