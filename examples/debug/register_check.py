#!/usr/bin/env python3
"""Check register state consistency between two simulation runs.

Real-world scenario: During JIT development, the most reliable
correctness signal was comparing interpreter vs JIT register state at
regular intervals.  The L4Re JIT divergence (commit a9969a2) was caught
at instruction 192 when x3 differed by 0x10 between interpreter and JIT
— caused by NZCV corruption in fused CMP+B.cond (commit df3023b).

The XZR stale-value bug (commit 2b95254) was similarly caught: MOV X19,
X0 produced X19=0x30 instead of X0 because the JIT block prologue
didn't re-zero the flat-array XZR slot.

Unlike se_jit_compare.py (which compares interp vs JIT), this script
compares two interpreter runs with different configurations to detect
non-determinism or configuration-dependent state corruption.

This example demonstrates:
  - Side-by-side simulation with configurable differences
  - Register snapshot and comparison at configurable intervals
  - Divergence localization with binary search refinement
  - NZCV flag decomposition for detailed flag analysis

Usage:
    helm-aarch64 examples/debug/register_check.py --binary ./my_elf
    helm-aarch64 examples/debug/register_check.py --binary ./my_elf \\
        --interval 1000 --max-insns 100000
    helm-aarch64 examples/debug/register_check.py --binary ./my_elf \\
        --timing-a virtual --timing-b interval  # compare timing models
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
    p = argparse.ArgumentParser(
        description="Register divergence checker — compare two "
                    "simulation runs at regular intervals"
    )
    p.add_argument("--binary", "-b", default=helmutil.default_binary())
    p.add_argument("--max-insns", "-n", type=int, default=10_000_000,
                   help="Max instructions to compare (default 10M)")
    p.add_argument("--interval", type=int, default=100_000,
                   help="Instructions between comparisons (default 100K)")
    p.add_argument("--timing-a", default="virtual",
                   help="Timing model for run A (default: virtual)")
    p.add_argument("--timing-b", default="virtual",
                   help="Timing model for run B (default: virtual)")
    p.add_argument("--bisect", action="store_true",
                   help="On divergence, binary-search to find the exact "
                        "instruction that diverges")
    p.add_argument("--argv", nargs="*", default=None)
    return p.parse_args()


def _nzcv_str(nzcv):
    """Decompose NZCV flags into readable string."""
    n = "N" if (nzcv >> 31) & 1 else "n"
    z = "Z" if (nzcv >> 30) & 1 else "z"
    c = "C" if (nzcv >> 29) & 1 else "c"
    v = "V" if (nzcv >> 28) & 1 else "v"
    return f"{n}{z}{c}{v}"


def _diff_regs(snap_a, snap_b):
    """Return list of register differences with NZCV decomposition."""
    raw = helmutil.reg_diff(snap_a, snap_b)
    # Enhance NZCV entries with flag decomposition
    enhanced = []
    for d in raw:
        enhanced.append(d)
        if "nzcv" in d.lower():
            va = snap_a.get("nzcv", 0)
            vb = snap_b.get("nzcv", 0)
            enhanced.append(f"         (A={_nzcv_str(va)} B={_nzcv_str(vb)})")
    return enhanced


def _make_sim(binary, argv, timing):
    sim = _helm_ng.build_simulation(isa="aarch64", mode="se",
                                     timing=timing)
    envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C",
            "USER=helm"]
    sim.load_elf(binary, argv, envp)
    return sim


def _bisect_divergence(binary, argv, timing_a, timing_b, low, high):
    """Binary search for the exact divergence instruction."""
    print(f"\n[reg-check] Bisecting divergence between insn "
          f"{low:,} and {high:,}...")

    while high - low > 1:
        mid = (low + high) // 2
        sim_a = _make_sim(binary, argv, timing_a)
        sim_b = _make_sim(binary, argv, timing_b)
        sim_a.run(mid)
        sim_b.run(mid)

        diffs = _diff_regs(helmutil.snapshot_regs(sim_a), helmutil.snapshot_regs(sim_b))
        if diffs:
            high = mid
            print(f"  [{low:>10,} .. {high:>10,}]  diverged at {mid:,}")
        else:
            low = mid
            print(f"  [{low:>10,} .. {high:>10,}]  match at {mid:,}")
        sim_a.finish()
        sim_b.finish()

    print(f"\n[reg-check] Divergence at insn ~{high:,}")
    # One final run to show the state
    sim_a = _make_sim(binary, argv, timing_a)
    sim_b = _make_sim(binary, argv, timing_b)
    sim_a.run(high)
    sim_b.run(high)
    diffs = _diff_regs(helmutil.snapshot_regs(sim_a), helmutil.snapshot_regs(sim_b))
    for d in diffs:
        print(d)
    sim_a.finish()
    sim_b.finish()


def main():
    args = parse_args()
    binary = args.binary
    if not os.path.isfile(binary):
        print(f"[reg-check] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    argv = args.argv or [os.path.basename(binary), "-c", "echo hello"]

    config_diff = args.timing_a != args.timing_b
    print(f"[reg-check] binary={binary}  argv={argv}")
    print(f"[reg-check] run A: timing={args.timing_a}")
    print(f"[reg-check] run B: timing={args.timing_b}")
    print(f"[reg-check] interval={args.interval:,}  "
          f"max_insns={args.max_insns:,}")
    if config_diff:
        print(f"[reg-check] Comparing different timing models — "
              f"architectural state should still match")
    print()

    sim_a = _make_sim(binary, argv, args.timing_a)
    sim_b = _make_sim(binary, argv, args.timing_b)

    interval = args.interval
    done = 0
    t0 = time.monotonic()
    checks = 0

    while done < args.max_insns:
        n = min(interval, args.max_insns - done)

        r_a = sim_a.run(n)
        r_b = sim_b.run(n)
        done += n
        checks += 1

        snap_a = helmutil.snapshot_regs(sim_a)
        snap_b = helmutil.snapshot_regs(sim_b)
        diffs = _diff_regs(snap_a, snap_b)

        if diffs:
            wall = time.monotonic() - t0
            print(f"[reg-check] *** DIVERGENCE at insn ~{done:,} "
                  f"(check #{checks}, {wall:.1f}s) ***")
            for d in diffs:
                print(d)

            if args.bisect:
                sim_a.finish()
                sim_b.finish()
                _bisect_divergence(binary, argv, args.timing_a,
                                   args.timing_b,
                                   done - interval, done)
            else:
                sim_a.finish()
                sim_b.finish()
            sys.exit(1)

        # Check for mismatched stop reasons
        if r_a != r_b:
            print(f"[reg-check] STOP MISMATCH at insn ~{done:,}: "
                  f"A={r_a}  B={r_b}")
            sim_a.finish()
            sim_b.finish()
            sys.exit(1)

        if r_a != "quantum":
            break

        # Progress
        wall = time.monotonic() - t0
        if wall > 2.0 and checks % 10 == 0:
            mips = done / wall / 1e6
            print(f"\r[reg-check] {done/1e6:.1f}M insns  "
                  f"{checks} checks OK  {mips:.0f} MIPS",
                  end="", file=sys.stderr, flush=True)

    wall = time.monotonic() - t0
    print(f"\n[reg-check] OK: {done:,} insns, {checks} register "
          f"checks passed ({wall:.1f}s)")
    sim_a.finish()
    sim_b.finish()


if __name__ == "__main__":
    main()
