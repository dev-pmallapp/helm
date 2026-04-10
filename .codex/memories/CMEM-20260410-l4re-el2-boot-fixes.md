# L4Re EL2 boot debugging session

- Date: 2026-04-10
- Branch: `workspace/l4re-el2-smp-boot`

## Command lines

```bash
# EL2 boot (default GICv3)
cargo run --bin helm-system-aarch64 -- examples/fs/virt.py \
  --kernel assets/aarch64/boot/l4re_hello-2_arm_virt.elf --boot-el 2 --max-insns 200000000

# EL1 boot (L4Re rejects -- prints "configured for EL2 mode")
cargo run --bin helm-system-aarch64 -- examples/fs/virt.py \
  --kernel assets/aarch64/boot/l4re_hello-2_arm_virt.elf --boot-el 1 --max-insns 10000000

# With plugins for debugging
cargo run --bin helm-system-aarch64 -- examples/fs/virt.py \
  --kernel assets/aarch64/boot/l4re_hello-2_arm_virt.elf --boot-el 2 --max-insns 100000000 \
  --plugin "execlog:el=0,max=200" --plugin fault-detect --plugin stub-tracer

# QEMU reference (works fully)
qemu-system-aarch64 -M virt,virtualization=on,gic-version=3 -cpu cortex-a53 \
  -m 1024 -nographic -kernel assets/aarch64/boot/l4re_hello-2_arm_virt.elf -no-reboot
```

## Bugs fixed (4 commits)

1. **GICv3 Group 0 interrupt support** (`hw/helm-hw-intc/src/gicv3/`)
   - `highest_pending_for_cpu` only checked Group 1 NS; fiasco at EL2 uses Group 1
     but the gate required both dist.ctlr EnableGrp1NS AND icc_igrpen1
   - Added EnableGrp0 + icc_igrpen0 checking with per-interrupt group filtering
   - Added ICC_IAR0/EOIR0/HPPIR0/BPR0/AP0R0-3 sysreg handlers
   - Timer IRQ (CNTHP, INTID 26) was firing but never delivered to CPU

2. **MMU stage-2 suppression with HCR_EL2.TGE=1** (`runtime/helm-arch/src/aarch64/mmu.rs`)
   - When TGE=1, stage-2 is architecturally disabled even if VM=1
   - Fiasco sets TGE=1 + VM=1 for non-VM EL0 tasks under EL2 kernel
   - Without fix: spurious stage-2 translation faults at level 0 prevented moe startup

3. **ESR IL bit for BRK/SVC/HVC/SMC** (`runtime/helm-arch/src/aarch64/execute/branch.rs`)
   - AArch64 syndromes must have IL=1 (bit 25) for 32-bit instruction exceptions
   - Fiasco jdb checks IL to classify debug entry types

4. **Plugin current_el support** (`framework/helm-plugin/src/runtime/info.rs`, execlog)
   - Added `current_el` to `ArchContext::Aarch64`
   - `execlog` plugin supports `el=N` filter: `--plugin "execlog:el=0,max=100"`

## Current state (after fixes)

- L4Re bootstrapper, fiasco kernel, SIGMA0 all run correctly
- Fiasco starts moe (roottask) but hits kernel assertion: `BRK #1` / `user ""`
- The jdb (kernel debugger) activates at `pc=0xffff4005002c`
- Sigma0 runs at EL0 (confirmed via execlog el= filter)
- No EL1 execution observed (fiasco uses TGE=1, all user code at EL0)
- Timer interrupts work correctly (CNTHP timer fires and is acknowledged)

## Next steps

- Investigate the fiasco assertion at `pc=0xffff4005002c` / `lr=0xffff4004ffe4`
- L4Re sources are at `../../tmp/l4re-snapshot-26.03.0/src/fiasco/`
- The assertion is likely caused by a missing/incorrect sysreg or feature
  during moe's initialization via fiasco's task creation path
- Key fiasco source files:
  - `src/kern/arm/64/` -- AArch64 kernel code
  - `src/jdb/arm/jdb-arm.cpp` -- jdb entry handling
  - `src/jdb/arm/jdb_entry_frame-arm.cpp` -- ESR classification

## Key diagnostic approaches

- `--plugin fault-detect` to see memory faults and BRK assertions
- `--plugin "execlog:el=0,max=N"` to trace user-mode execution
- `--plugin stub-tracer` + `--sim-trace=file:/tmp/trace.log` for STUB/WARN
- QEMU comparison: same ELF boots fully on QEMU with `virtualization=on`
