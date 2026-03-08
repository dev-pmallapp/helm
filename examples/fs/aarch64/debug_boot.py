#!/usr/bin/env python3
"""Debug Linux kernel boot — identify where it gets stuck and why.

Run: helm-system-aarch64 examples/fs/aarch64/debug_boot.py
"""
import _helm_core
import sys

KERNEL = "assets/alpine/boot/vmlinuz-rpi"
APPEND = "earlycon=pl011,0x09000000 console=ttyAMA0"

print("[HelmPy] === Linux Kernel Boot Debugger ===", flush=True)

s = _helm_core.FsSession(kernel=KERNEL, machine="virt", append=APPEND)
print(f"[HelmPy] Entry PC={s.pc:#x}", flush=True)

# Run in phases, checking state at each stop
phases = [
    (10_000_000,   "early boot"),
    (50_000_000,   "memory init"),
    (100_000_000,  "device init"),
    (200_000_000,  "driver probe"),
    (300_000_000,  "post-console"),
    (400_000_000,  "idle check 1"),
    (500_000_000,  "idle check 2"),
]

prev_pc = s.pc
stuck_count = 0

for count, label in phases:
    remaining = count - s.insn_count
    if remaining <= 0:
        continue
    result = s.run(remaining)
    regs = s.regs()
    pc = s.pc
    el = regs.get('current_el', 0)
    daif = regs.get('daif', 0)
    sp = regs.get('sp', 0)
    lr = s.xn(30)

    daif_i = (daif >> 7) & 1  # IRQ mask bit
    daif_f = (daif >> 6) & 1  # FIQ mask bit

    print(f"[HelmPy] --- {label} ({s.insn_count} insns) ---", flush=True)
    print(f"  PC={pc:#x}  EL{el}  DAIF={daif:#x} (I={daif_i} F={daif_f})", flush=True)
    print(f"  SP={sp:#x}  LR={lr:#x}", flush=True)
    print(f"  result: {result}", flush=True)

    # Check if we're stuck (same PC region)
    if abs(pc - prev_pc) < 0x1000:
        stuck_count += 1
        if stuck_count >= 2:
            print(f"[HelmPy] *** STUCK at PC={pc:#x} for {stuck_count} phases ***", flush=True)
            print(f"[HelmPy] X0={s.xn(0):#x} X1={s.xn(1):#x} X2={s.xn(2):#x}", flush=True)
            print(f"[HelmPy] X19={s.xn(19):#x} X20={s.xn(20):#x} X29={s.xn(29):#x}", flush=True)
            break
    else:
        stuck_count = 0
    prev_pc = pc

# Final diagnosis
print(f"\n[HelmPy] === DIAGNOSIS ===", flush=True)
print(f"  Total insns: {s.insn_count}", flush=True)
print(f"  Final PC: {s.pc:#x}", flush=True)
regs = s.regs()
daif = regs.get('daif', 0)
print(f"  DAIF: {daif:#x} (IRQs {'MASKED' if daif & 0x80 else 'UNMASKED'})", flush=True)

# Check ISA skip count by looking at X registers for clues
print(f"  EL: {regs.get('current_el', '?')}", flush=True)
