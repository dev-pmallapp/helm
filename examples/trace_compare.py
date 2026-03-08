#!/usr/bin/env python3
"""Compare JIT vs interp execution traces to find divergence.

Usage:
    helm-system-aarch64 examples/trace_compare.py

Single-steps both backends for N instructions, recording PC + X0-X3
at each step, and reports the first divergence.
"""
import _helm_core
import sys

sys.stdout.reconfigure(line_buffering=True)

KERNEL = "assets/alpine/boot/vmlinuz-rpi"
APPEND = "earlycon=pl011,0x09000000 console=ttyAMA0"
N = 5000  # instructions to compare

print(f"[trace] Creating interp session...")
s_interp = _helm_core.FsSession(
    kernel=KERNEL, machine="virt", append=APPEND, backend="interp",
)

print(f"[trace] Creating JIT session...")
s_jit = _helm_core.FsSession(
    kernel=KERNEL, machine="virt", append=APPEND, backend="jit",
)

print(f"[trace] Comparing {N} steps: interp vs JIT")
print(f"[trace] {'Step':>6}  {'interp PC':>18}  {'JIT PC':>18}  {'Match':>5}")
print(f"[trace] " + "-" * 60)

diverged = False
for i in range(N):
    pc_i = s_interp.pc
    pc_j = s_jit.pc

    match = "OK" if pc_i == pc_j else "DIFF"

    if pc_i != pc_j or i < 20 or i % 500 == 0:
        x0_i = s_interp.xn(0)
        x0_j = s_jit.xn(0)
        print(f"[trace] {i:>6}  {pc_i:#018x}  {pc_j:#018x}  {match}"
              f"  X0={x0_i:#x}/{x0_j:#x}")

    if pc_i != pc_j and not diverged:
        diverged = True
        print(f"\n[trace] *** DIVERGENCE at step {i} ***")
        print(f"[trace] interp: PC={pc_i:#018x}")
        print(f"[trace]    jit: PC={pc_j:#018x}")
        # Dump more register state
        ri = s_interp.regs()
        rj = s_jit.regs()
        for reg in ['sp', 'current_el', 'daif', 'nzcv']:
            vi = ri.get(reg, 0)
            vj = rj.get(reg, 0)
            flag = " ***" if vi != vj else ""
            print(f"[trace]   {reg}: interp={vi:#x}  jit={vj:#x}{flag}")
        for xn in range(31):
            vi = s_interp.xn(xn)
            vj = s_jit.xn(xn)
            if vi != vj:
                print(f"[trace]   X{xn}: interp={vi:#x}  jit={vj:#x} ***")
        # Show a few more steps to see the pattern
        for j in range(min(10, N - i - 1)):
            s_interp.run(1)
            s_jit.run(1)
            pi = s_interp.pc
            pj = s_jit.pc
            print(f"[trace] {i+j+1:>6}  {pi:#018x}  {pj:#018x}  {'OK' if pi == pj else 'DIFF'}")
        break

    s_interp.run(1)
    s_jit.run(1)

if not diverged:
    print(f"\n[trace] No divergence in {N} steps. Both backends match.")
    print(f"[trace] Final PC: interp={s_interp.pc:#x}  jit={s_jit.pc:#x}")
