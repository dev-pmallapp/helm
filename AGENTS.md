# helm-ng Codex Agent Instructions

## Common Principles

**Incremental Progress**:
- Small, testable changes
- Commit working code frequently
- Build on previous work

**Evidence-Based**:
- Match existing project patterns
- Verify behavior with concrete code and artifacts
- Avoid assumptions when repository context can answer the question
- **Always check `../helm.git` first** — it is the previous generation reference implementation. When adding features or fixing bugs, look there before inventing a solution.

**Pragmatic**:
- Boring solutions over clever code
- Simple over complex
- Adapt to project reality

**Testing**:
- Write or update tests alongside changes
- Verify after each meaningful change
- Treat passing verification as the completion gate

---

## How to Run the Simulator

**Do NOT use `python3` directly.**

The correct entry points are the compiled binaries:

```bash
# Full-system (FS) mode — boot a Linux kernel
target/debug/helm-system-aarch64 examples/fs/boot_rpi_full.py --max-insns 100000000

# Syscall-emulation (SE) mode — run a static AArch64 ELF
target/debug/helm-aarch64 examples/se/run_binary.py --binary assets/aarch64/bin/fish

# Embedded default script (no .py needed)
target/debug/helm-system-aarch64 --max-insns 100000000
target/debug/helm-aarch64 assets/aarch64/bin/fish
```

**Sim-trace (simulator log channel)**:

Use `--sim-trace=<URI>` to control where simulator diagnostic output goes.
This is separate from guest serial output (PL011/UART which goes to stdout).

```bash
# Default: simulator logs to stderr
target/debug/helm-system-aarch64 examples/fs/boot_rpi_full.py

# Write simulator logs to a file
target/debug/helm-system-aarch64 --sim-trace=file:/tmp/helm-trace.log examples/fs/boot_rpi_full.py

# Stream logs to TCP (nc -l 3333 in another terminal)
target/debug/helm-system-aarch64 --sim-trace=tcp:localhost:3333 examples/fs/boot_rpi_full.py

# Suppress all simulator logs (benchmarking)
target/debug/helm-system-aarch64 --sim-trace=null: examples/fs/boot_rpi_full.py
```

`--sim-trace=` is stripped before the Python script sees `sys.argv`.

---

## Asset Locations

Default boot assets: `assets/aarch64/alpine/boot/`

| File | Description |
|------|-------------|
| `vmlinuz-rpi` | Raw ARM64 Image — use this for FS boot |
| `vmlinuz-lts` | Compressed zboot EFI — NOT usable by current loader |
| `initramfs-rpi` | Alpine Linux initramfs |

**The platform is `arm-virt`** (QEMU-compatible), NOT Raspberry Pi hardware.
The FS example auto-generates a compatible arm-virt DTB when `--dtb` is omitted.
Never pass a board-specific DTB (bcm2711, etc.) to the arm-virt platform.

---

## Platform Architecture (arm-virt)

| Address | Device |
|---------|--------|
| `0x0800_0000` | GICv2 Distributor (GICD) |
| `0x0801_0000` | GICv2 CPU Interface (GICC) |
| `0x0900_0000` | PL011 UART |
| `0x4000_0000` | RAM base |

Kernel load layout:
- Kernel Image: `RAM_BASE + 0x20_0000` (2 MB aligned)
- Initramfs: `RAM_BASE + 0x400_0000` (64 MB offset)
- DTB: after kernel image end, 2 MB-aligned

---

## Reference Implementation

`../helm.git` is the previous generation. **Check it first** when:
- Adding or fixing instruction decode/execute (`crates/helm-isa/src/arm/aarch64/`)
- Adding devices (`crates/helm-device/src/arm/`)
- Fixing GIC wiring (`crates/helm-device/src/arm/gic/v2.rs`)
- Understanding FS boot session setup (`crates/helm-engine/src/fs/session.rs`)
- Looking at test patterns (`crates/helm-isa/src/arm/aarch64/tests/`)

The old repo had ~6000 AArch64 decode/execute tests. They are ported to
`runtime/helm-arch/src/aarch64/tests/` with a harness adapting old `Aarch64Cpu`
to the current `Aarch64ArchState + MemInterface` design.

---

## Logging / Diagnostics

All simulator diagnostic output uses macros from `helm_debug::sim_trace`:
- `sim_stub!(component="name", pc=a.pc, "message {}", val)` — unimplemented feature
- `sim_warn!(component="name", pc=a.pc, "message")` — unexpected but recoverable
- `sim_info!(component="name", "message")` — normal progress

Output format:
```
[STUB] sim_ns=000000000019 insns=000000000019 aarch64-sys      pc=0x0000000041408d74 | SYS insn (TLBI/DC/IC) raw=0xd5033fbf
[WARN] sim_ns=000000000123 insns=000000000123 gicv2-gicd       pc=?                  | read unhandled offset=0x380 -> 0
```

**Do NOT use `eprintln!` or bare `log::warn!` for simulator diagnostics.**
Guest serial output goes to stdout via `StdioCharBackend` — keep it separate.

---

## Key Decode/Execute Bugs Fixed (do not re-introduce)

1. **`LdrLit` 64-bit**: `sf = size == 1` (opc bits[31:30]=01 means Xt). Was wrongly `size==3`.
2. **`SMULH` decode**: `op1=0b010` always = SMULH; `0b110` always = UMULH. Was checking `bit31`.
3. **`SWP` decode**: `bit[15]` (o3=1) selects SWP; o3=0 selects arithmetic LSE ops. Was using `opc=4`.
4. **`MSR` bit pattern**: Writes use `0b1101_0101_0001` at bits[31:20], not `0010`.
5. **`Csel/Csinc/Csinv/Csneg` 32-bit**: Must mask result with `0xFFFF_FFFF` when `!insn.sf`.
6. **`Asr` 32-bit**: Must zero-extend: `(((src as u32 as i32) >> sh) as u32) as u64`.
7. **`Sdiv` 32-bit**: Sign-extend inputs from 32 bits; zero-extend result.
8. **`ERET` decode**: Must be checked before the `BR/BLR/RET` bit-pattern guard.
9. **`SWP` test encoding**: Correct SWP X0,X1,[X2] = `0xF820_8041` (o3=1). `0xF820_4041` is SMAX.

---

## Intentional Design Decisions

**`runtime/helm-arch/src/aarch64/execute.rs` is intentionally one large file.**

It mirrors the industry-standard pattern of a single exhaustive `match` over all
instruction opcodes, as used in QEMU's TCG interpreter, gem5's execute models,
and most decode-then-execute simulators:
- All opcodes visible in one place — easy to audit ISA coverage
- One location to add a new instruction
- The Rust compiler enforces exhaustiveness across the whole ISA at compile time
- No module boundary to cross when an instruction's execution touches helpers

**Do not split it into submodules.** The size is a feature, not a bug.

Similarly, `decode.rs` is intentionally structured as a single top-level dispatch
followed by per-group decode functions — matching the ARM architecture encoding
hierarchy (op0 → group → sub-group).

---

## Build Commands

```bash
cargo build                    # build everything
cargo build --release          # release build
cargo test -p helm-arch        # AArch64 decode/execute tests (~800 tests)
cargo build -p helm-python     # rebuild Python extension after engine changes
cargo build -p helm-cli        # rebuild helm-aarch64 and helm-system-aarch64
```

After adding variants to `insn.rs`, always rebuild `helm-engine` — the
`classify_aarch64_opcode` match is exhaustive and will fail on new opcodes.
