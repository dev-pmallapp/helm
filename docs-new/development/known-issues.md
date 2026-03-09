# Known Issues

## Current Limitations

- **Static binaries only** — SE mode does not support dynamic linking
  or `PT_INTERP`.
- **AArch64 only** — RISC-V and x86 have stub frontends but no
  functional implementation.
- **Single-core FS** — multi-core FS mode is not yet implemented.
- **Coherence stub** — `CoherenceController` is a MOESI skeleton with
  no functional protocol.
- **Cache replacement** — uses a simple LRU stub (first invalid or
  last line in set).
- **KVM requires AArch64 host** — the KVM backend only works on
  AArch64 Linux hosts.

## Sysreg Sync Gaps

Timer registers (`CNTVCT_EL0`, `CNTV_CVAL_EL0`) must be synced at
block boundaries. If sync is missed, timers may fire late or not at
all.

## Unimplemented Instructions

- Some SIMD instructions are treated as NOP with a log warning.
- PAC instructions (PACIA, AUTIA, etc.) are implemented as NOP.
- SVE / SME are not implemented.
- AArch32 (Thumb, ARM) execution is not implemented.

## TCG Emitter Gaps

Some instruction classes fall back to the direct executor:
- Complex SIMD operations.
- Some system register accesses.
- Exclusive load/store pairs.

## Known Bugs

Check the issue tracker for the latest known bugs and planned fixes.
