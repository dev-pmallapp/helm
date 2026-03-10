# Error Codes

`HelmError` variants and what triggers them.

## Variants

| Variant | Format | Common Causes |
|---------|--------|---------------|
| `Isa(msg)` | ISA error: {msg} | Unimplemented instruction, bad encoding |
| `Decode { addr, reason }` | Decode error at {addr}: {reason} | Invalid instruction, insufficient bytes |
| `Translation(msg)` | Translation error: {msg} | Block translation failure |
| `Syscall { number, reason }` | Syscall error: syscall {number} — {reason} | Unimplemented or failed syscall |
| `Memory { addr, reason }` | Memory error at {addr}: {reason} | Unmapped address, permission violation |
| `Pipeline(msg)` | Pipeline error: {msg} | ROB full, scheduling failure |
| `Config(msg)` | Configuration error: {msg} | Invalid platform config |
| `Io(err)` | I/O error: {err} | File not found, read failure |

## Usage

All HELM functions return `HelmResult<T>` which is `Result<T, HelmError>`.

Library crates use `thiserror` for error types. Binary and integration
boundaries use `anyhow` for context-rich error propagation.

## SE Mode Syscall Errors

In SE mode, `HelmError::Syscall` is used as a control-flow mechanism:
the executor returns it when it encounters an SVC instruction, and
the SE runner dispatches to the syscall handler.
