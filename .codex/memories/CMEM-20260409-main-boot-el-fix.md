# main boot_el loader fix checkpoint

- Date: 2026-04-09
- Branch: `main`

## Summary

Imported the missing uncommitted boot-EL support from the
`workspace/l4re-el2-smp-boot` worktree onto `main`.

## Changes

- `runtime/helm-engine/src/loader/arm64_image.rs`
  - added `LoadedKernel.boot_el`
  - added ELF-vs-Image ARM64 kernel load split
  - ELF AArch64 payloads now return `boot_el = 2`
  - Image loads continue to return `boot_el = 1`
  - added regression test covering ELF kernel loading
- `runtime/helm-arch/src/aarch64/core_model.rs`
  - preserve pre-existing EL2/EL3 feature bits when applying CPU model data
  - added regression test
- `runtime/helm-engine/src/tests/engine.rs`
  - tightened PSCI CPU-on test setup
  - added EL2-preservation PSCI test

## Verification

- `cargo test -p helm-engine --lib`
