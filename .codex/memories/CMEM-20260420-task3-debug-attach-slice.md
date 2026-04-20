# Task 3 debug attach slice

- Date: 2026-04-20
- Branch: `main`
- Base SHA before slice: `00d3867`

## Scope completed

Continued Task 3 after the debug-intent checkpoint centralization slice by
moving live native breakpoint/watchpoint probe attachment into
`runtime/helm-debug`.

Implemented:

- `runtime/helm-debug/Cargo.toml`
  - added an explicit `instrumentation` feature that forwards to
    `helm-probe/instrumentation`
- `runtime/helm-debug/src/breakpoint.rs`
  - added `attach_breakpoint_engine(...)`
  - added an instrumentation-only probe-attachment test
- `runtime/helm-debug/src/watchpoint.rs`
  - added `attach_watchpoint_engine(...)`
  - added an instrumentation-only probe-attachment test
- `runtime/helm-debug/src/lib.rs`
  - re-exported the new attach helpers behind the same feature gate
- `runtime/helm-python/Cargo.toml`
  - forwarded `instrumentation` into `helm-debug/instrumentation`
- `runtime/helm-python/src/system.rs`
  - `ensure_breakpoint_engine()` now delegates probe subscription setup to
    `helm_debug::attach_breakpoint_engine(...)`
  - `ensure_watchpoint_engine()` now delegates probe subscription setup to
    `helm_debug::attach_watchpoint_engine(...)`

## Verification

- `cargo test -p helm-debug`
- `cargo test -p helm-debug --features instrumentation`
- `cargo test -p helm-python --features instrumentation`

## Result

- `runtime/helm-debug` now owns:
  - native debug-intent checkpoint encode/decode
  - native probe-backed breakpoint/watchpoint live attachment
- `runtime/helm-python` remains the public control surface, but no longer owns
  the probe-subscription wiring itself.

## Next step

The next Task 3 slice should centralize current debug trigger state and stop
result presentation:

- one shared debug-state snapshot in `runtime/helm-debug`
- one shared stop-reason rendering path for Python / future GDB integration
- optional capture of the last native breakpoint/watchpoint hit if/when the
  engine grows a stop-on-native-trigger path
