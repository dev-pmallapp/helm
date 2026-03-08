#!/usr/bin/env python3
"""Diagnose kernel init after initramfs unpack."""
import _helm_core

s = _helm_core.FsSession(
    kernel="assets/alpine/boot/vmlinuz-rpi",
    machine="virt",
    append="earlycon=pl011,0x09000000 console=ttyAMA0",
)

# Run until past initramfs unpack (virtual time ~10s, need lots of insns)
# The 3-min run showed "Freeing initrd memory" at [9.646982]
# Let's break at kernel_init_freeable which runs after rootfs is populated
print("[HelmPy] Breaking at kernel_init_freeable...", flush=True)
result = s.run_until_symbol("kernel_init_freeable", 5_000_000_000)
print(f"[HelmPy] Result: {result}", flush=True)
print(f"[HelmPy] PC={s.pc:#x} insns={s.insn_count}", flush=True)

# If we hit it, step through to see what happens
if "breakpoint" in str(result).lower() or "Breakpoint" in str(result):
    print("[HelmPy] Hit kernel_init_freeable! Continuing...", flush=True)
    # Run a bit more to see what kernel_init does next
    s.run(10_000_000)
    print(f"[HelmPy] After 10M more: PC={s.pc:#x}", flush=True)
    regs = s.regs()
    print(f"[HelmPy] EL={regs.get('current_el')} DAIF={regs.get('daif',0):#x}", flush=True)
else:
    print(f"[HelmPy] Didn't hit symbol in budget", flush=True)
    print(f"[HelmPy] Final PC={s.pc:#x}", flush=True)
