#!/usr/bin/env python3
"""Measure MIPS per boot phase — useful for spotting performance cliffs.

Usage:
    helm-system-aarch64 examples/debug/benchmark.py
    helm-system-aarch64 examples/debug/benchmark.py -- --backend interp
"""
import argparse, sys, time
import _helm_core

sys.stdout.reconfigure(line_buffering=True)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--kernel", default="assets/alpine/boot/vmlinuz-rpi")
    p.add_argument("--backend", default="jit", choices=["jit", "interp"])
    args = p.parse_args()

    s = _helm_core.FsSession(
        kernel=args.kernel, machine="virt",
        append="earlycon=pl011,0x09000000 console=ttyAMA0",
        backend=args.backend,
    )

    phases = [1_000_000, 10_000_000, 50_000_000, 100_000_000, 200_000_000]
    prev = 0
    total_wall = 0.0

    print(f"{'Phase':>14} {'Insns':>12} {'Wall(s)':>8} {'MIPS':>8}")
    print("-" * 46)

    for target in phases:
        budget = target - prev
        t0 = time.monotonic()
        s.run(budget)
        wall = time.monotonic() - t0
        total_wall += wall
        done = s.insn_count - prev
        mips = done / wall / 1e6 if wall > 0.001 else 0
        print(f"{target:>14,} {done:>12,} {wall:>8.2f} {mips:>8.0f}")
        prev = s.insn_count

    print("-" * 46)
    mips = s.insn_count / total_wall / 1e6 if total_wall > 0.001 else 0
    print(f"{'TOTAL':>14} {s.insn_count:>12,} {total_wall:>8.2f} {mips:>8.0f}")


if __name__ == "__main__":
    main()
