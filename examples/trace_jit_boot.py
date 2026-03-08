#!/usr/bin/env python3
"""Compare JIT vs interp at large checkpoint intervals.

Usage:
    helm-system-aarch64 examples/trace_jit_boot.py
"""
import _helm_core
import sys, time

sys.stdout.reconfigure(line_buffering=True)

KERNEL = "assets/alpine/boot/vmlinuz-rpi"
APPEND = "earlycon=pl011,0x09000000 console=ttyAMA0"

si = _helm_core.FsSession(kernel=KERNEL, machine="virt", append=APPEND, backend="interp")
sj = _helm_core.FsSession(kernel=KERNEL, machine="virt", append=APPEND, backend="jit")

checkpoints = [10_000_000, 50_000_000, 100_000_000, 200_000_000, 500_000_000]

print(f"{'Target':>12} | {'Interp':>18} {'ic':>12} | {'JIT':>18} {'ic':>12} | Match | Wall")
print("-" * 100)

for cp in checkpoints:
    bi = cp - si.insn_count
    bj = cp - sj.insn_count
    if bi <= 0: bi = 1
    if bj <= 0: bj = 1

    t0 = time.monotonic()
    si.run(bi)
    t_i = time.monotonic() - t0

    t0 = time.monotonic()
    sj.run(bj)
    t_j = time.monotonic() - t0

    m = "OK" if si.pc == sj.pc else "DIFF"
    print(f"{cp:>12,} | {si.pc:#018x} {si.insn_count:>12,} | {sj.pc:#018x} {sj.insn_count:>12,} | {m:5} | {t_i:.1f}s/{t_j:.1f}s")

    if si.pc != sj.pc:
        ri = si.regs()
        rj = sj.regs()
        print(f"  EL: i={ri.get('current_el',0)} j={rj.get('current_el',0)}")
        print(f"  SCTLR: i={ri.get('sctlr_el1',0):#x}" if 'sctlr_el1' in ri else "  SCTLR: (not exposed)")
        break
