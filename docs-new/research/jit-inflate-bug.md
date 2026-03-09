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

Run in SE mode (`helm-arm` or `SeSession`) with both `interp` and `jit`
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
