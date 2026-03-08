#!/usr/bin/env python3
"""Benchmark FS-mode kernel boot -- measure MIPS per phase.

Usage:
    helm-system-aarch64 examples/fs_benchmark.py

Runs the kernel boot in phases and reports wall-clock time and
throughput (MIPS) for each phase.
"""
import _helm_core
import time
import sys

sys.stdout.reconfigure(line_buffering=True)

KERNEL = "assets/alpine/boot/vmlinuz-rpi"
APPEND = "earlycon=pl011,0x09000000 console=ttyAMA0"

s = _helm_core.FsSession(kernel=KERNEL, machine="virt", append=APPEND)

phases = [
    (1_000_000,    "startup (1M)"),
    (10_000_000,   "early boot (10M)"),
    (50_000_000,   "memory init (50M)"),
    (100_000_000,  "device init (100M)"),
    (200_000_000,  "driver probe (200M)"),
]

total_wall = 0.0
prev_insns = 0

print(f"{'Phase':<25} {'Insns':>12} {'Wall(s)':>8} {'MIPS':>8}")
print("-" * 58)

for target, label in phases:
    budget = target - prev_insns
    if budget <= 0:
        continue

    t0 = time.monotonic()
    s.run(budget)
    wall = time.monotonic() - t0

    total_wall += wall
    done = s.insn_count - prev_insns
    mips = done / wall / 1e6 if wall > 0 else 0

    print(f"{label:<25} {done:>12,} {wall:>8.2f} {mips:>8.1f}")
    prev_insns = s.insn_count

print("-" * 58)
print(f"{'TOTAL':<25} {s.insn_count:>12,} {total_wall:>8.2f} "
      f"{s.insn_count/total_wall/1e6:>8.1f}")
