#!/usr/bin/env python3
"""Compare JIT vs interp execution — run large budgets and compare state.

Usage:
    helm-system-aarch64 examples/trace_compare.py
"""
import _helm_core
import sys

sys.stdout.reconfigure(line_buffering=True)

KERNEL = "assets/alpine/boot/vmlinuz-rpi"
APPEND = "earlycon=pl011,0x09000000 console=ttyAMA0"

print(f"[trace] Creating sessions...")
si = _helm_core.FsSession(kernel=KERNEL, machine="virt", append=APPEND, backend="interp")
sj = _helm_core.FsSession(kernel=KERNEL, machine="virt", append=APPEND, backend="jit")

# Run both with same large budget — don't step, just run
checkpoints = [1000, 10_000, 100_000, 1_000_000, 10_000_000, 50_000_000]

print(f"[trace] {'Target':>12}  {'iPC':>18} {'iInsn':>10}  {'jPC':>18} {'jInsn':>10}  Match")
print(f"[trace] " + "-" * 95)

prev_i = 0
prev_j = 0
for cp in checkpoints:
    bi = cp - prev_i
    bj = cp - prev_j
    if bi > 0:
        si.run(bi)
    if bj > 0:
        sj.run(bj)
    prev_i = si.insn_count
    prev_j = sj.insn_count

    match = "OK" if si.pc == sj.pc else "DIFF"
    print(f"[trace] {cp:>12,}  {si.pc:#018x} {si.insn_count:>10,}  {sj.pc:#018x} {sj.insn_count:>10,}  {match}")

    if si.pc != sj.pc:
        print(f"\n[trace] *** DIVERGENCE ***")
        ri = si.regs()
        rj = sj.regs()
        for reg in ['sp', 'current_el', 'daif', 'nzcv']:
            vi = ri.get(reg, 0)
            vj = rj.get(reg, 0)
            flag = " ***" if vi != vj else ""
            print(f"[trace]   {reg}: i={vi:#x}  j={vj:#x}{flag}")
        diffs = 0
        for xn in range(31):
            vi = si.xn(xn)
            vj = sj.xn(xn)
            if vi != vj:
                if diffs < 10:
                    print(f"[trace]   X{xn}: i={vi:#x}  j={vj:#x} ***")
                diffs += 1
        if diffs > 10:
            print(f"[trace]   ... and {diffs-10} more register diffs")
        break

print(f"\n[trace] Done.")
