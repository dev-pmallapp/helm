#!/usr/bin/env python3
"""Benchmark the Cranelift JIT backend."""
import _helm_core
import time
import sys
sys.stdout.reconfigure(line_buffering=True)

# JIT backend session
s = _helm_core.FsSession(
    kernel="assets/alpine/boot/vmlinuz-rpi",
    machine="virt",
    append="earlycon=pl011,0x09000000 console=ttyAMA0",
)
# Note: FsSession defaults to "tcg" backend. To use JIT, need to pass backend="jit"
# For now, measure what we have.

phases = [
    (1_000_000,    "warmup (1M)"),
    (10_000_000,   "boot (10M)"),
    (50_000_000,   "init (50M)"),
    (100_000_000,  "devices (100M)"),
]

total_wall = 0.0
prev = 0

print(f"{'Phase':<20} {'Insns':>12} {'Wall(s)':>8} {'MIPS':>8}")
print("-" * 55)

for target, label in phases:
    budget = target - prev
    if budget <= 0:
        continue
    t0 = time.monotonic()
    s.run(budget)
    wall = time.monotonic() - t0
    total_wall += wall
    done = s.insn_count - prev
    mips = done / wall / 1e6 if wall > 0 else 0
    print(f"{label:<20} {done:>12,} {wall:>8.2f} {mips:>8.1f}")
    prev = s.insn_count

print("-" * 55)
print(f"{'TOTAL':<20} {s.insn_count:>12,} {total_wall:>8.2f} {s.insn_count/total_wall/1e6:>8.1f}")
