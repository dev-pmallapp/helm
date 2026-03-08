#!/usr/bin/env python3
"""Debug IRQ delivery — check if kernel ever unmasks interrupts.

Run: helm-system-aarch64 examples/fs/aarch64/debug_irq.py
"""
import _helm_core
import sys

KERNEL = "assets/alpine/boot/vmlinuz-rpi"
APPEND = "earlycon=pl011,0x09000000 console=ttyAMA0"

s = _helm_core.FsSession(kernel=KERNEL, machine="virt", append=APPEND)

# Boot past initial setup (200M instructions gets us past console switch)
print("[HelmPy] Booting to 200M insns...", flush=True)
s.run(200_000_000)
print(f"[HelmPy] At 200M: PC={s.pc:#x} DAIF={s.regs().get('daif',0):#x}", flush=True)

# Now sample DAIF every 100 instructions for 100K samples
# to see if IRQs are ever unmasked
daif_unmasked = 0
daif_masked = 0
wfi_seen = 0

print("[HelmPy] Sampling DAIF state over 10M insns (100-insn steps)...", flush=True)

for i in range(100_000):
    s.run(100)
    daif = s.regs().get('daif', 0)
    if daif & 0x80 == 0:
        daif_unmasked += 1
        if daif_unmasked <= 5:
            print(f"  [sample {i}] IRQs UNMASKED! PC={s.pc:#x} DAIF={daif:#x}", flush=True)
    else:
        daif_masked += 1

print(f"[HelmPy] Results:", flush=True)
print(f"  Samples with IRQs unmasked: {daif_unmasked}", flush=True)
print(f"  Samples with IRQs masked:   {daif_masked}", flush=True)
print(f"  Total insns: {s.insn_count}", flush=True)

# Check timer state
regs = s.regs()
print(f"[HelmPy] Timer state:", flush=True)
# CNTV_CTL is not exposed via regs() dict, check via sysreg
# Let's just print what we have
print(f"  DAIF={regs.get('daif',0):#x}", flush=True)
print(f"  PC={s.pc:#x}", flush=True)
