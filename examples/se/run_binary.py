#!/usr/bin/env python3
"""Run an AArch64 static binary in syscall-emulation mode.

Usage:
    helm-aarch64 examples/se/run_binary.py --binary ./my_elf
    helm-aarch64 ./hello                  # embedded mode
    helm-aarch64 ./hello -c "echo hi"
"""
import argparse
import os
import sys
import time

import _helm_ng

sys.stdout.reconfigure(line_buffering=True)


def parse_args():
    p = argparse.ArgumentParser(description="helm-ng SE — run AArch64 binary")
    p.add_argument("--binary", "-b",
                   default=os.environ.get("HELM_BINARY", "assets/binaries/fish"))
    p.add_argument("--max-insns", "-n", type=int, default=500_000_000,
                   help="Max guest instructions (default 500M)")
    p.add_argument("--cpu", default="atomic",
                   choices=["atomic", "timing", "minor", "o3", "big"],
                   help="CPU model (selects timing model)")
    p.add_argument("--caches", action="store_true",
                   help="Enable cache simulation (Phase 1)")
    p.add_argument("--l2cache", action="store_true",
                   help="Enable L2 cache (Phase 1)")
    p.add_argument("--strace", action="store_true",
                   help="Print syscall trace")
    p.add_argument("-E", dest="env_vars", action="append", default=[],
                   metavar="VAR=VALUE", help="Set target environment variable")
    args, guest_args = p.parse_known_args()
    # Strip leading '--' separator if present
    if guest_args and guest_args[0] == "--":
        guest_args = guest_args[1:]
    args.guest_args = guest_args
    return args


# Map gem5-style CPU names to helm-ng timing models
CPU_TIMING = {
    "atomic":  "virtual",
    "timing":  "interval",
    "minor":   "interval",
    "o3":      "accurate",
    "big":     "accurate",
}


def main():
    args = parse_args()
    binary = args.binary

    guest_args = args.guest_args
    if not guest_args:
        guest_args = ["--no-config", "-c", "echo hello"]

    argv = [os.path.basename(binary)] + guest_args
    envp = args.env_vars if args.env_vars else [
        "HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm",
    ]

    if not os.path.isfile(binary):
        print(f"[se] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    timing = CPU_TIMING.get(args.cpu, "virtual")
    print(f"[se] binary={binary}  argv={argv}  cpu={args.cpu}  timing={timing}")

    # Build simulation
    sim = _helm_ng.build_simulation(
        isa="aarch64", mode="se", timing=timing,
    )
    sim.load_elf(binary, argv, envp)

    t0 = time.monotonic()
    # Run in chunks so we can print progress for long-running binaries
    chunk = 50_000_000
    remaining = args.max_insns
    while remaining > 0 and not sim.has_exited:
        n = min(chunk, remaining)
        sim.run(n)
        remaining -= n
        wall = time.monotonic() - t0
        if not sim.has_exited and wall > 2.0:
            mips = sim.insn_count / wall / 1e6
            print(f"\r[se] {sim.insn_count/1e6:.0f}M insns  {wall:.0f}s  {mips:.0f} MIPS",
                  end="", file=sys.stderr, flush=True)
    if wall > 2.0 and not sim.has_exited:
        print(file=sys.stderr)  # newline after progress
    wall = time.monotonic() - t0
    mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0

    if sim.has_exited:
        print(f"[se] exited with code {sim.exit_code}")
    else:
        print(f"[se] hit limit at PC={sim.pc:#x}")

    print(f"[se] {sim.insn_count:,} insns  {wall:.2f}s  {mips:.0f} MIPS")

    if sim.has_exited:
        sys.exit(sim.exit_code)


if __name__ == "__main__":
    main()
