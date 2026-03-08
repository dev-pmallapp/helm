#!/usr/bin/env python3
"""JIT boot progress — check for kernel VA and output."""
import _helm_core, time, sys
sys.stdout.reconfigure(line_buffering=True)

sj = _helm_core.FsSession(
    kernel="assets/alpine/boot/vmlinuz-rpi", machine="virt",
    append="earlycon=pl011,0x09000000 console=ttyAMA0", backend="jit")

for cp in [100_000, 1_000_000, 10_000_000, 50_000_000, 100_000_000, 500_000_000]:
    budget = cp - sj.insn_count
    if budget <= 0: continue
    t0 = time.monotonic()
    sj.run(budget)
    w = time.monotonic() - t0
    mips = budget / w / 1e6 if w > 0.001 else 0
    kva = sj.pc > 0xffff_0000_0000_0000
    marker = 'KERNEL-VA' if kva else ('EXCEPTION' if sj.pc < 0x1000 else 'phys')
    print(f"  {cp:>12,}: PC={sj.pc:#018x}  {w:.2f}s  {mips:.0f}MIPS  {marker}")
    if sj.pc < 0x1000:
        print(f"    *** exception vector at {sj.pc:#x} ***")
        break
