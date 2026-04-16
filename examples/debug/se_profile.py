#!/usr/bin/env python3
"""Profile an SE-mode binary with multiple analysis plugins.

Combines hotblocks, howvec (instruction mix), and insn-count plugins in
a single run to give a complete workload characterization without
running the binary three times.

Usage:
    target/release/helm-aarch64 examples/debug/se_profile.py [OPTIONS]

Examples:
    # Profile fish shell (default)
    ... se_profile.py

    # Profile inflate_test for 100K insns
    ... se_profile.py --binary assets/aarch64/binaries/inflate_test \\
        --max-insns 100000

    # Profile with JIT enabled
    ... se_profile.py --jit
"""
import argparse
import os
import sys
import time


sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import helmutil

helmutil.require_launcher()
_helm_ng = helmutil.import_helm_ng()
sys.stdout.reconfigure(line_buffering=True)
sys.stderr.reconfigure(line_buffering=True)


def parse_args():
    p = argparse.ArgumentParser(description="SE workload profiling")
    p.add_argument("--binary", "-b", default=helmutil.default_binary())
    p.add_argument("--max-insns", "-n", type=int, default=50_000_000,
                   help="Max instructions (default 50M)")
    p.add_argument("--jit", action="store_true",
                   help="Enable JIT backend")
    p.add_argument("--top", type=int, default=20,
                   help="Top-N hotblocks to show (default 20)")
    args, guest_args = p.parse_known_args()
    if guest_args and guest_args[0] == "--":
        guest_args = guest_args[1:]
    args.guest_args = guest_args
    return args


def main():
    args = parse_args()
    binary = args.binary

    if not os.path.isfile(binary):
        print(f"[profile] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    argv = args.guest_args or [os.path.basename(binary), "-c", "echo hello"]
    envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm"]

    tag = "jit" if args.jit else "interp"
    print(f"[profile] {tag}  binary={binary}  max_insns={args.max_insns:,}", file=sys.stderr)

    sim = _helm_ng.build_simulation(isa="aarch64", mode="se", timing="virtual")
    sim.load_elf(binary, argv, envp)

    sim.add_plugin("insn-count")
    sim.add_plugin("hotblocks")
    sim.add_plugin("howvec")

    if args.jit:
        sim.set_jit(True)

    t0 = time.monotonic()
    chunk = 50_000_000
    remaining = args.max_insns
    run_fn = sim.run_jit if args.jit else sim.run
    while remaining > 0 and not sim.has_exited:
        n = min(chunk, remaining)
        stop = run_fn(n)
        remaining -= n
        if stop != "quantum":
            break

    wall = time.monotonic() - t0
    mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0
    print(f"\n[profile] {sim.insn_count:,} insns  {wall:.2f}s  {mips:.0f} MIPS",
          file=sys.stderr)

    if sim.has_exited:
        print(f"[profile] exited with code {sim.exit_code}")

    # Plugin reports are printed by finish()
    sim.finish()


if __name__ == "__main__":
    main()
