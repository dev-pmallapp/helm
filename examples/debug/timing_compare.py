#!/usr/bin/env python3
"""Compare simulation metrics across timing models on the same workload.

Runs the same kernel boot (or SE binary) under virtual, interval, and
accurate timing models, reporting IPC, cycle count, and wall-clock MIPS
for each.  Useful for validating that timing model changes don't regress
functional correctness and for understanding the performance/accuracy
tradeoff.

Usage:
    target/release/helm-system-aarch64 examples/debug/timing_compare.py [OPTIONS]

Examples:
    # Compare all three models on Linux boot (default 10M insns)
    ... timing_compare.py

    # Compare on SE binary
    target/release/helm-aarch64 examples/debug/timing_compare.py \\
        --mode se --binary assets/aarch64/binaries/fish

    # Compare only virtual vs interval with more instructions
    ... timing_compare.py --models virtual,interval --max-insns 50000000
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
    p = argparse.ArgumentParser(description="Compare timing models")
    p.add_argument("--mode", choices=("fs", "se"), default="fs",
                   help="fs=kernel boot, se=user binary (default fs)")
    p.add_argument("--kernel", default=helmutil.default_kernel())
    p.add_argument("--initrd", default=helmutil.default_initrd())
    p.add_argument("--binary", default=helmutil.default_binary())
    p.add_argument("--max-insns", "-n", type=int, default=10_000_000,
                   help="Instructions per run (default 10M)")
    p.add_argument("--models", default="virtual,interval,accurate",
                   help="Comma-separated timing models to compare")
    return p.parse_args()


CHUNK = 5_000_000


def run_one(sim_mode, timing, args):
    """Run one simulation, return (insns, cycles, wall_secs, stop)."""
    if sim_mode == "fs":
        sim = _helm_ng.build_simulation(
            isa="aarch64", mode="fs", timing=timing, mem_mib=512,
        )
        sim.load_kernel(kernel=args.kernel, initrd=args.initrd)
    else:
        sim = _helm_ng.build_simulation(
            isa="aarch64", mode="se", timing=timing,
        )
        argv = [os.path.basename(args.binary), "-c", "echo hello"]
        envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C"]
        sim.load_elf(args.binary, argv, envp)

    t0 = time.monotonic()
    remaining = args.max_insns
    stop = "quantum"
    while remaining > 0 and not sim.has_exited:
        n = min(CHUNK, remaining)
        stop = sim.run(n)
        remaining -= n
        if stop != "quantum":
            break
    wall = time.monotonic() - t0

    stats = sim.stats()
    insns = sim.insn_count
    cycles = int(stats.get("tick_count", stats.get("virtual_cycles", 0)))
    sim.finish()
    return insns, cycles, wall, stop


def main():
    args = parse_args()
    models = [m.strip() for m in args.models.split(",")]

    if args.mode == "fs" and not args.kernel:
        print("[timing-cmp] no kernel found; run: scripts/manage-assets.sh download linux-rpi-kernel",
              file=sys.stderr)
        sys.exit(1)

    print(f"[timing-cmp] mode={args.mode}  max_insns={args.max_insns:,}  "
          f"models={models}", file=sys.stderr)

    results = {}
    for model in models:
        print(f"\n[timing-cmp] running {model}...", file=sys.stderr)
        try:
            insns, cycles, wall, stop = run_one(args.mode, model, args)
            ipc = insns / cycles if cycles > 0 else float("inf")
            mips = insns / wall / 1e6 if wall > 0.001 else 0
            results[model] = {
                "insns": insns, "cycles": cycles, "wall": wall,
                "ipc": ipc, "mips": mips, "stop": stop,
            }
        except Exception as e:
            print(f"[timing-cmp] {model} failed: {e}", file=sys.stderr)
            results[model] = None

    # Summary table
    print(f"\n{'Model':<14} {'Insns':>14} {'Cycles':>14} {'IPC':>8} "
          f"{'MIPS':>8} {'Wall(s)':>8} {'Stop':<10}")
    print("-" * 78)
    for model in models:
        r = results.get(model)
        if r is None:
            print(f"{model:<14} {'FAILED':>14}")
            continue
        print(f"{model:<14} {r['insns']:>14,} {r['cycles']:>14,} "
              f"{r['ipc']:>8.3f} {r['mips']:>8.1f} {r['wall']:>8.2f} "
              f"{r['stop']:<10}")


if __name__ == "__main__":
    main()
