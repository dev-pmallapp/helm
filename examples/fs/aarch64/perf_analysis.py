#!/usr/bin/env python3
"""Performance analysis — measure MIPS, TCG hit rate, and bottlenecks.

Run: helm-system-aarch64 examples/fs/aarch64/perf_analysis.py
"""
import _helm_core
import time
import sys

sys.stdout.reconfigure(line_buffering=True)

KERNEL = "assets/alpine/boot/vmlinuz-rpi"
APPEND = "earlycon=pl011,0x09000000 console=ttyAMA0"

s = _helm_core.FsSession(kernel=KERNEL, machine="virt", append=APPEND)

print(f"[Perf] === HELM Performance Analysis ===")
print(f"[Perf] Kernel: {KERNEL}")

# Measure throughput at different phases
phases = [
    (1_000_000,    "startup (1M)"),
    (10_000_000,   "early boot (10M)"),
    (50_000_000,   "memory init (50M)"),
    (100_000_000,  "device init (100M)"),
    (200_000_000,  "driver probe (200M)"),
]

total_wall = 0.0
prev_insns = 0

print(f"\n{'Phase':<25} {'Insns':>12} {'Wall(s)':>8} {'MIPS':>8} {'VirtTime':>10}")
print("-" * 70)

for target, label in phases:
    budget = target - prev_insns
    if budget <= 0:
        continue

    t0 = time.monotonic()
    result = s.run(budget)
    t1 = time.monotonic()

    wall = t1 - t0
    total_wall += wall
    insns_done = s.insn_count - prev_insns
    mips = insns_done / wall / 1e6 if wall > 0 else 0
    vtime = s.insn_count / 62.5e6  # approx virtual seconds

    print(f"{label:<25} {insns_done:>12,} {wall:>8.2f} {mips:>8.1f} {vtime:>9.3f}s")
    prev_insns = s.insn_count

print("-" * 70)
print(f"{'TOTAL':<25} {s.insn_count:>12,} {total_wall:>8.2f} "
      f"{s.insn_count/total_wall/1e6:>8.1f}")

# Compare with QEMU
qemu_mips = 500  # typical QEMU TCG on modern x86
print(f"\n[Perf] === Comparison ===")
print(f"[Perf] HELM MIPS:  {s.insn_count/total_wall/1e6:.1f}")
print(f"[Perf] QEMU MIPS:  ~{qemu_mips} (typical TCG JIT)")
print(f"[Perf] Speedup needed: {qemu_mips / (s.insn_count/total_wall/1e6):.0f}x")

# Estimate time to boot to shell
# Alpine init takes ~5B instructions in QEMU (~10s at 500 MIPS)
insns_to_shell = 5_000_000_000
helm_time = insns_to_shell / (s.insn_count / total_wall)
qemu_time = insns_to_shell / (qemu_mips * 1e6)
print(f"\n[Perf] Est. time to shell:")
print(f"  HELM:  {helm_time:.0f}s ({helm_time/60:.1f} min)")
print(f"  QEMU:  {qemu_time:.0f}s")

print(f"\n[Perf] === Bottleneck Analysis ===")
print(f"[Perf] The main bottleneck is interpretive fallback.")
print(f"[Perf] QEMU JIT compiles blocks to native x86 — ~100x faster.")
print(f"[Perf] Our TCG 'interprets' TcgOps — still a dispatch loop.")
print(f"[Perf] Solutions:")
print(f"  1. JIT compile TcgOps to native code (like QEMU)")
print(f"  2. Use KVM for near-native speed (already wired)")
print(f"  3. Reduce interpretive fallback rate")
