#!/usr/bin/env python3
"""Run JIT and check if kernel boots (larger budgets)."""
import _helm_core
import sys, time
sys.stdout.reconfigure(line_buffering=True)

KERNEL = "assets/alpine/boot/vmlinuz-rpi"
APPEND = "earlycon=pl011,0x09000000 console=ttyAMA0"

sj = _helm_core.FsSession(kernel=KERNEL, machine="virt", append=APPEND, backend="jit")

checkpoints = [
    100_000, 1_000_000, 5_000_000, 10_000_000,
    50_000_000, 100_000_000, 200_000_000, 500_000_000,
]

for cp in checkpoints:
    budget = cp - sj.insn_count
    if budget <= 0: continue
    t0 = time.monotonic()
    sj.run(budget)
    wall = time.monotonic() - t0
    mips = budget / wall / 1e6 if wall > 0.001 else 0
    is_kernel_va = sj.pc > 0xffff_0000_0000_0000
    print(f"  {cp:>12,}: PC={sj.pc:#018x}  ic={sj.insn_count:>12,}  {wall:.2f}s  {mips:.0f}MIPS  {'KERNEL-VA' if is_kernel_va else 'phys'}")
    if is_kernel_va:
        print(f"  *** Kernel VA reached! MMU is on. ***")
