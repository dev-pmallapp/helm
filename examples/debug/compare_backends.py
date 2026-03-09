#!/usr/bin/env python3
"""Compare JIT vs interpreter execution to find divergences.

Runs both backends with the same kernel and reports the first PC
divergence along with a register diff.

Usage:
    helm-system-aarch64 examples/debug/compare_backends.py
"""
import sys
import _helm_core

sys.stdout.reconfigure(line_buffering=True)

KERNEL = "assets/alpine/boot/vmlinuz-rpi"
APPEND = "earlycon=pl011,0x09000000 console=ttyAMA0"

print("[cmp] creating interp + jit sessions...")
si = _helm_core.FsSession(kernel=KERNEL, machine="virt", append=APPEND, backend="interp")
sj = _helm_core.FsSession(kernel=KERNEL, machine="virt", append=APPEND, backend="jit")

checkpoints = [1_000, 10_000, 100_000, 1_000_000, 10_000_000, 50_000_000]

print(f"{'Target':>12}  {'interp PC':>18} {'insns':>10}  {'JIT PC':>18} {'insns':>10}  Status")
print("-" * 90)

prev_i = prev_j = 0
for cp in checkpoints:
    si.run(cp - prev_i)
    sj.run(cp - prev_j)
    prev_i, prev_j = si.insn_count, sj.insn_count
    match = "OK" if si.pc == sj.pc else "DIFF"
    print(f"{cp:>12,}  {si.pc:#018x} {si.insn_count:>10,}  {sj.pc:#018x} {sj.insn_count:>10,}  {match}")

    if si.pc != sj.pc:
        print("\n*** DIVERGENCE — register diff: ***")
        ri, rj = si.regs(), sj.regs()
        for reg in sorted(set(ri) | set(rj)):
            vi, vj = ri.get(reg, 0), rj.get(reg, 0)
            if vi != vj:
                print(f"  {reg:>12}: interp={vi:#x}  jit={vj:#x}")
        break

print("\n[cmp] done.")
