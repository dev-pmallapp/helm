#!/usr/bin/env python3
"""Diagnose where kernel is stuck after KVM message."""
import _helm_core
import sys

s = _helm_core.FsSession(
    kernel="assets/alpine/boot/vmlinuz-rpi",
    machine="virt",
    append="earlycon=pl011,0x09000000 console=ttyAMA0",
)

# Run to get past KVM message - the last output was at ~1.05s virtual time
# which is around 65M instructions (62.5 MIPS × 1.05s)
print("[HelmPy] Running 100M insns...", flush=True)
s.run(100_000_000)
print(f"[HelmPy] At 100M: PC={s.pc:#x}", flush=True)

# Run more and check if PC is moving
prev_pc = s.pc
for i in range(5):
    s.run(50_000_000)
    pc = s.pc
    regs = s.regs()
    daif = regs.get('daif', 0)
    el = regs.get('current_el', 0)
    print(f"[HelmPy] {(i+1)*50+100}M: PC={pc:#x} EL{el} DAIF={daif:#x} I={'masked' if daif & 0x80 else 'open'}", flush=True)

    if abs(pc - prev_pc) < 0x10000:
        print(f"[HelmPy]   Stuck in same region!", flush=True)
        # Sample more finely to understand the loop
        pcs = set()
        for j in range(1000):
            s.run(1000)
            pcs.add(s.pc & ~0xFFF)  # page-aligned
        print(f"[HelmPy]   Unique PC pages: {len(pcs)}", flush=True)
        for p in sorted(pcs)[:10]:
            print(f"[HelmPy]     {p:#x}", flush=True)
        break
    prev_pc = pc

print(f"[HelmPy] Final: insns={s.insn_count} PC={s.pc:#x}", flush=True)
