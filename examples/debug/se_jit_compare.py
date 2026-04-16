#!/usr/bin/env python3
"""Compare interpreter vs JIT execution on an SE-mode AArch64 binary.

Runs two simulation instances side by side -- one interpreter-only, one
JIT-enabled -- and checks register state at regular intervals.  Stops on
the first divergence and prints a diff of register values.

Usage:
    target/release/helm-aarch64 examples/debug/se_jit_compare.py [OPTIONS]

Examples:
    # Compare on fish shell (default)
    target/release/helm-aarch64 examples/debug/se_jit_compare.py

    # Compare on a custom binary
    ... se_jit_compare.py --binary assets/aarch64/binaries/inflate_test \\
        --max-insns 100000

    # Finer checkpoint granularity for precise divergence location
    ... se_jit_compare.py --checkpoint-interval 10000
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
    p = argparse.ArgumentParser(description="SE JIT vs interpreter comparison")
    p.add_argument("--binary", "-b", default=helmutil.default_binary())
    p.add_argument("--max-insns", "-n", type=int, default=50_000_000,
                   help="Max instructions to compare (default 50M)")
    p.add_argument("--checkpoint-interval", type=int, default=500_000,
                   help="Instructions between register comparisons (default 500K)")
    p.add_argument("--argv", nargs="*", default=None,
                   help="Guest argv (default: binary-name -c 'echo hello')")
    return p.parse_args()


def _make_sim(binary, argv):
    sim = _helm_ng.build_simulation(isa="aarch64", mode="se", timing="virtual")
    envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm"]
    sim.load_elf(binary, argv, envp)
    return sim


def main():
    args = parse_args()
    binary = args.binary

    if not os.path.isfile(binary):
        print(f"[se-jit-cmp] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    argv = args.argv or [os.path.basename(binary), "-c", "echo hello"]
    print(f"[se-jit-cmp] binary={binary}  argv={argv}", file=sys.stderr)
    print(f"[se-jit-cmp] max_insns={args.max_insns:,}  "
          f"checkpoint_interval={args.checkpoint_interval:,}", file=sys.stderr)

    sim_interp = _make_sim(binary, argv)
    sim_jit = _make_sim(binary, argv)
    sim_jit.set_jit(True)

    interval = args.checkpoint_interval
    done = 0
    t0 = time.monotonic()

    while done < args.max_insns:
        n = min(interval, args.max_insns - done)

        r_i = sim_interp.run(n)
        r_j = sim_jit.run_jit(n)
        done += n

        diffs = helmutil.reg_diff(
            helmutil.snapshot_regs(sim_interp),
            helmutil.snapshot_regs(sim_jit),
        )

        if diffs:
            wall = time.monotonic() - t0
            print(f"\n[se-jit-cmp] DIVERGENCE at insn ~{done:,} ({wall:.1f}s)",
                  file=sys.stderr)
            for d in diffs:
                print(f"[se-jit-cmp] {d}", file=sys.stderr)
            sim_interp.finish()
            sim_jit.finish()
            sys.exit(1)

        if r_i != "quantum" or r_j != "quantum":
            if r_i != r_j:
                print(f"[se-jit-cmp] STOP MISMATCH: interp={r_i}  jit={r_j}",
                      file=sys.stderr)
                sys.exit(1)
            break

        wall = time.monotonic() - t0
        if wall > 2.0 and done % (interval * 10) == 0:
            mips = done / wall / 1e6
            print(f"\r[se-jit-cmp] {done/1e6:.0f}M insns  {wall:.0f}s  {mips:.0f} MIPS",
                  end="", file=sys.stderr, flush=True)

    wall = time.monotonic() - t0
    print(f"\n[se-jit-cmp] OK: {done:,} insns match ({wall:.1f}s)", file=sys.stderr)
    sim_interp.finish()
    sim_jit.finish()


if __name__ == "__main__":
    main()
