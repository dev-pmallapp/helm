#!/usr/bin/env python3
"""Quick JIT vs interp comparison."""
import _helm_core, time, sys
sys.stdout.reconfigure(line_buffering=True)
print("[trace] creating interp...", flush=True)
si = _helm_core.FsSession(
    kernel="assets/alpine/boot/vmlinuz-rpi", machine="virt",
    append="earlycon=pl011,0x09000000 console=ttyAMA0", backend="interp")
print("[trace] creating jit...", flush=True)
sj = _helm_core.FsSession(
    kernel="assets/alpine/boot/vmlinuz-rpi", machine="virt",
    append="earlycon=pl011,0x09000000 console=ttyAMA0", backend="jit")
print("[trace] running 10M interp...", flush=True)
si.run(10_000_000)
print(f"[trace] interp: PC={si.pc:#x} ic={si.insn_count}", flush=True)
print("[trace] running 10M jit...", flush=True)
sj.run(10_000_000)
print(f"[trace] jit:    PC={sj.pc:#x} ic={sj.insn_count}", flush=True)
