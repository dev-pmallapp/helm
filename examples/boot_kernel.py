#!/usr/bin/env python3
"""Example: Boot a Linux kernel using embedded Python.

Run with:
    helm-system-arm examples/boot_kernel.py
"""
import _helm_core

# Create an FS session — equivalent to:
#   helm-system-arm -M virt --kernel vmlinuz-rpi --append "..."
s = _helm_core.FsSession(
    kernel="assets/alpine/boot/vmlinuz-rpi",
    machine="virt",
    append="earlycon=pl011,0x09000000 console=ttyAMA0",
)

print(f"[Helm-Py] Session created, PC={s.pc:#x}")

# Phase 1: run 10M instructions (early boot)
result = s.run(10_000_000)
print(f"[Helm-Py] After 10M insns: PC={s.pc:#x}, insns={s.insn_count}")
print(f"[Helm-Py] Stop reason: {result}")

# Phase 2: continue to 10M instructions
result = s.run(20_000_000)
print(f"[Helm-Py] After 100M insns: PC={s.pc:#x}, insns={s.insn_count}")

# Phase 3: show registers
regs = s.regs()
print(f"[Helm-Py] Registers: EL={regs.get('current_el', '?')}, "
      f"SP={regs.get('sp', 0):#x}, LR={s.xn(30):#x}")

print("[Python] Done!")
