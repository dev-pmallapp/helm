# JIT Inflate Bug — Debugging Context

## Status
**Open.** The JIT has an instruction correctness bug that causes the kernel's
gzip decompressor (`lib/decompress_inflate.c`) to produce corrupt output.
The initramfs is detected and present in memory, but `unpack_to_rootfs`
fails silently, leaving rootfs empty → `VFS: Unable to mount root fs`.

## Confirmed Facts

1. **Initrd data intact** — MD5 verified at PA `0x44000000` in both interp
   and JIT modes up to `populate_rootfs` entry (1.17B insns).

2. **Kernel detects initrd** — `phys_initrd_start = 0x44000000` in both modes
   (read from kernel variable at `0xffffffc0812a9118`).

3. **Both hit `populate_rootfs`** — interp at 1.181B insns, JIT at 1.171B insns
   (address `0xffffffc081203814`).

4. **Interp decompresses successfully** — prints `Trying to unpack rootfs image
   as initramfs...` then `Freeing initrd memory: 6660K`.

5. **JIT fails** — no unpack messages, panics with `VFS: Unable to mount root fs
   on unknown-block(0,0)` at ~2B insns (virtual time 23.6s).

6. **`isa_skip_count = 0`** — no unhandled instructions during boot. All
   instructions the kernel uses are translated to TCG IR (REV/CLZ etc. are
   never hit by this kernel).

7. **Register comparison is impractical** — the 10M insn-count gap between
   interp and JIT at `unpack_to_rootfs` means all heap pointers differ.
   The kernel's memory layout diverges due to timing-sensitive allocation.

## Key Symbols (System.map)

| Symbol | Address |
|--------|---------|
| `populate_rootfs` | `ffffffc081203814` |
| `unpack_to_rootfs` | `ffffffc081202d04` |
| `do_populate_rootfs` | `ffffffc0812039d0` |
| `phys_initrd_start` | `ffffffc0812a9118` (data) |
| `initrd_start` | `ffffffc0815f7088` (bss) |
| `__initramfs_start` | `ffffffc0812dc2ac` (data) |

## Next Steps (Planned)

### 1. Standalone gzip SE-mode test
Build a small AArch64 binary (cross-compiled C) that:
- Reads a gzip-compressed buffer from a known address
- Calls the kernel's inflate algorithm (or zlib's)
- Writes decompressed output to another buffer
- Compares against expected output and exits with 0/1

Run in SE mode (`helm-aarch64` or `SeSession`) with both `interp` and `jit`
backends. This eliminates timing non-determinism and makes register
comparison trivial.

### 2. Add `nokaslr` to FS boot
Add `nokaslr` to the kernel command line to disable address space
randomisation. This should make heap addresses deterministic between
interp and JIT, enabling direct register comparison in FS mode.

### 3. TCG IR dump for the inflate hot loop
Once a divergence is found in the SE test, dump the TCG IR for the
block containing the buggy instruction. Compare the IR against the
AArch64 spec to find the codegen error.

### Likely culprits
- UBFM/SBFM edge cases (bit extract with `imms < immr` wrap-around)
- CCMP/CCMN NZCV setting on the false path
- CSEL/CSINC with inverted condition
- 32-bit ADD/SUB carry flag computation
- MADD/MSUB operand ordering
- EXTR (extract from pair) — used in some inflate implementations

## Commits This Session

| Hash | Description |
|------|-------------|
| `d7708a8` | fix: compute ISTATUS bit dynamically in JIT sysreg reads |
| `5b76c49` | fix: pass initrd placement to DTB and remove timer edge-trigger guard |
| `77da290` | fix: wire PL011 interrupts to GIC and fix TX IRQ storm |
| `122fb72` | fix: write correct PC before exception ops in JIT blocks |

## Uncommitted Changes
- TPIDR_EL0 mirroring in regs array (slot 42, NUM_REGS → 43)
- CNTVCT lazy update (per-timer-check instead of per-block)
- Block cache: cache empty blocks to avoid re-translation
- Various CLI/Python cleanups

## Build & Test Commands
```bash
make check          # fast cargo check
make test           # all Rust tests (334 pass, 1 pre-existing GIC failure)
# Boot test:
cargo run --release --bin helm-system-aarch64 -- examples/tmp/boot_rpi_full.py --backend jit
# Interp boot (slow but works):
cargo run --release --bin helm-system-aarch64 -- examples/tmp/boot_rpi_full.py --backend interp
```

## SE-Mode Inflate Test (Added)

### Bug Found: SVC Not Handled in SE-Mode TCG

The standalone SE-mode inflate test revealed that **SVC instructions in TCG
blocks were silently ignored** in SE mode:

- The A64 emitter translates SVC → `SvcExc` (an FS-mode exception op).
- The TCG interpreter's `SvcExc` handler routes the exception (sets
  PC → VBAR, saves SPSR/ELR, raises EL).
- **SE-mode `exec_tcg` treated `InterpExit::Exception` as a no-op** — the
  syscall was never invoked and the corrupted exception state caused a
  jump to address 0x400 (VBAR + 0x400 with VBAR=0).

**Fix**: intercept `Exception { class: 0x15 }` (SVC) in `exec_tcg`,
extract the syscall number from X8 and return address from ELR_EL1 in
the regs array, restore PSTATE from SPSR_EL1, and route through
`handle_sc`.

### SE-Mode Inflate Result

With the SVC fix, the standalone inflate test **passes on both interp and
TCG backends**:

```
inflate_interp_passes ... ok
inflate_tcg_passes    ... ok
```

This confirms that the core TCG instruction translation (UBFM, SBFM,
CCMP, CSEL, MADD, EXTR, etc.) is **correct for this workload** in SE
mode.  The FS-mode inflate failure may involve:

1. **MMU / page-table walk** differences between interp and JIT.
2. **Sysreg read/write** ordering in the JIT sysreg file.
3. **Timer / interrupt** injection timing causing different kernel
   control flow (allocation addresses diverge → inflate buffer at
   different VA → TLB miss path differs).

### Next: FS-Mode Debugging

The SE-mode test eliminates instruction-level bugs as the cause.
Focus should shift to:

1. **Add `nokaslr` to FS boot** — stabilise heap addresses.
2. **Compare memory contents** at `unpack_to_rootfs` entry between
   interp and JIT with `nokaslr` enabled.
3. **Trace page-table state** around inflate to check for MMU
   translation errors in the JIT path.

## Instruction-Level Bugs Found (Session 2)

Targeted edge-case testing discovered **4 instruction-level TCG bugs** that
were NOT caught by the initial SE-mode inflate test (small data):

### 1. SBFM wrap-around: wrong sign-extension width

**File**: `crates/helm-tcg/src/a64_emitter.rs` (`handle_sbfm`)

When `imms < immr`, the emitter sign-extended from `(w + esize - immr)` bits
instead of `esize` bits.  This caused sign bits to be replicated too
aggressively.

- Example: `SBFM X0, X1, #60, #3` with X1=0xF
- TCG result: `0xFFFFFFFFFFFFFFF0` (sign-extended from 8 bits)
- Correct:    `0xF0` (sign-extend from 64 bits = no extension)

**Fix**: `from_bits: esize as u8` (matching the reference interpreter).

### 2–4. LSLV / LSRV / ASRV 32-bit: wrong shift masking & sign-extension

**File**: `crates/helm-tcg/src/a64_emitter.rs` (`handle_lslv/lsrv/asrv`)

Three related bugs in 32-bit variable shift instructions:

| Bug | Instruction | Issue | Example |
|-----|-------------|-------|---------|
| 2 | `LSLV W` | Shift not masked MOD 32 | `W1 << 32` gives 0 instead of W1 |
| 3 | `LSRV W` | Source not zero-extended; shift not masked MOD 32 | Upper-bit leakage |
| 4 | `ASRV W` | Source not sign-extended from 32; shift not masked MOD 32 | `0x80000000 >> 1` gives `0x40000000` instead of `0xC0000000` |

**Fix**: Added `mask_shift_amount()` helper that ANDs the shift register
with 31 for W-register ops.  LSLV/LSRV zero-extend the source;
ASRV sign-extends it.

### Impact on FS Inflate

These bugs could absolutely cause the kernel's inflate to produce corrupt
output:

- **SBFM wrap**: used by the kernel for signed bitfield extraction in
  Huffman table building.
- **ASRV/LSRV W**: used by inflate's variable-length bit reader to
  extract Huffman codes from a 32-bit accumulator.

The bugs only trigger with specific operand combinations (shift ≥ 32,
or specific immr/imms values), explaining why the small SE test passed.

## Session 3: SDIV Bug Found

### SDIV uses unsigned division

**File**: `crates/helm-tcg/src/a64_emitter.rs` (`handle_sdiv`)
**File**: `crates/helm-tcg/src/ir.rs` (added `SDiv` op)

`handle_sdiv` emitted `TcgOp::Div` which performs **unsigned** division
in all three backends (interpreter, threaded, JIT/Cranelift).  For
negative operands this produces wildly wrong results.

- Example: `SDIV X0, X1, X2` with X1=−10, X2=3
- Old (unsigned): `0xFFFFFFFFFFFFFFF6 / 3 = huge positive number`
- Correct (signed): `−10 / 3 = −3`

**Fix**: Added `TcgOp::SDiv` to the IR with signed division semantics
in all three backends.  The emitter now sign-extends 32-bit operands
before emitting `SDiv`.

### Also fixed: UDIV 32-bit operand truncation

`handle_udiv` for W-register operations didn't zero-extend operands
before the unsigned division, allowing stale upper bits to affect
the result.

### Impact on boot hang

The SDIV bug is the most likely cause of the boot stall after
"Mountpoint-cache":

- The kernel's scheduler uses `SDIV` for weight/load calculations.
- Wrong division of negative values produces huge positive results.
- This corrupts scheduling decisions, causing the kernel to spin
  indefinitely in a miscalculated busy-wait or miss a wakeup.
