#!/usr/bin/env python3
"""Narrow down JIT exception point and compare state."""
import _helm_core, sys
sys.stdout.reconfigure(line_buffering=True)

si = _helm_core.FsSession(
    kernel="assets/alpine/boot/vmlinuz-rpi", machine="virt",
    append="earlycon=pl011,0x09000000 console=ttyAMA0", backend="interp")
sj = _helm_core.FsSession(
    kernel="assets/alpine/boot/vmlinuz-rpi", machine="virt",
    append="earlycon=pl011,0x09000000 console=ttyAMA0", backend="jit")

# Binary search for divergence with large budgets
lo, hi = 0, 50000
while hi - lo > 100:
    mid = (lo + hi) // 2
    si2 = _helm_core.FsSession(kernel="assets/alpine/boot/vmlinuz-rpi", machine="virt",
        append="earlycon=pl011,0x09000000 console=ttyAMA0", backend="interp")
    sj2 = _helm_core.FsSession(kernel="assets/alpine/boot/vmlinuz-rpi", machine="virt",
        append="earlycon=pl011,0x09000000 console=ttyAMA0", backend="jit")
    si2.run(mid)
    sj2.run(mid)
    if si2.pc == sj2.pc or (abs(int(si2.pc) - int(sj2.pc)) < 0x100 and sj2.pc > 0x1000):
        lo = mid
    else:
        hi = mid
        print(f"  {mid:>6}: DIFF  interp={si2.pc:#x}  jit={sj2.pc:#x}")

print(f"\nRange: [{lo}, {hi}]")

# Now step through [lo, lo+200] and compare
si3 = _helm_core.FsSession(kernel="assets/alpine/boot/vmlinuz-rpi", machine="virt",
    append="earlycon=pl011,0x09000000 console=ttyAMA0", backend="interp")
sj3 = _helm_core.FsSession(kernel="assets/alpine/boot/vmlinuz-rpi", machine="virt",
    append="earlycon=pl011,0x09000000 console=ttyAMA0", backend="jit")
si3.run(lo)
sj3.run(lo)
print(f"At {lo}: interp={si3.pc:#x}  jit={sj3.pc:#x}")

for step in range(hi - lo + 20):
    pi, pj = si3.pc, sj3.pc
    if pj < 0x1000 and pi > 0x1000:
        ri = si3.regs()
        rj = sj3.regs()
        print(f"\n*** JIT exception at step {lo+step} ***")
        print(f"  interp: PC={pi:#x}  jit: PC={pj:#x}")
        print(f"  interp EL={ri.get('current_el',0)}  jit EL={rj.get('current_el',0)}")
        for key in sorted(ri.keys()):
            vi, vj = ri.get(key, 0), rj.get(key, 0)
            if vi != vj:
                print(f"  {key}: i={vi:#x}  j={vj:#x}")
        for xn in range(31):
            vi, vj = si3.xn(xn), sj3.xn(xn)
            if vi != vj:
                print(f"  X{xn}: i={vi:#x}  j={vj:#x}")
        break
    si3.run(1)
    sj3.run(1)
