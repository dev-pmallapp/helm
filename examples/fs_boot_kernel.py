#!/usr/bin/env python3
"""Boot a Linux kernel on the ARM virt platform.

Usage:
    helm-system-aarch64 examples/fs_boot_kernel.py

Demonstrates Python-controlled kernel boot:
  1. Create an FS session with kernel and platform config
  2. Run in phases -- pause, inspect, continue
  3. Query CPU registers and system state at each stop
"""
import _helm_core
import sys

sys.stdout.reconfigure(line_buffering=True)

KERNEL = "assets/alpine/boot/vmlinuz-rpi"
APPEND = "earlycon=pl011,0x09000000 console=ttyAMA0"

print(f"[HelmPy] Creating FS session: machine=virt, kernel={KERNEL}")
try:
    s = _helm_core.FsSession(
        kernel=KERNEL,
        machine="virt",
        append=APPEND,
    )
except Exception as e:
    print(f"[HelmPy] Failed to create session: {e}", file=sys.stderr)
    sys.exit(1)

print(f"[HelmPy] Session created -- entry PC={s.pc:#x}")

# Phase 1: early boot (10M instructions)
result = s.run(10_000_000)
print(f"[HelmPy] Phase 1 done: PC={s.pc:#x}, insns={s.insn_count}")
print(f"[HelmPy]   result: {result}")

# Phase 2: continue through kernel init (100M more)
result = s.run(100_000_000)
print(f"[HelmPy] Phase 2 done: PC={s.pc:#x}, insns={s.insn_count}")

# Dump register state
regs = s.regs()
print(f"[HelmPy] Registers:")
print(f"  EL={regs.get('current_el', '?')}  DAIF={regs.get('daif', 0):#x}  SP={regs.get('sp', 0):#x}")
print(f"  X0={s.xn(0):#x}  X1={s.xn(1):#x}  LR={s.xn(30):#x}")

print(f"[HelmPy] Done -- {s.insn_count} instructions executed")
