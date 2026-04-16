#!/usr/bin/env python3
"""Compare interpreter vs JIT during Linux kernel boot.

Runs two FS-mode simulations side by side -- one interpreter, one JIT --
and checks register state at regular intervals.  Useful for catching
JIT codegen bugs that only manifest during MMU-enabled kernel execution.

Usage:
    target/release/helm-system-aarch64 examples/debug/fs_jit_compare.py [OPTIONS]

Examples:
    # Compare first 50M insns of Linux boot
    ... fs_jit_compare.py

    # Finer checkpoints
    ... fs_jit_compare.py --checkpoint-interval 100000

    # SMP boot comparison
    ... fs_jit_compare.py --smp 2 --max-insns 100000000
"""
import argparse
import sys
import os
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import helmutil

helmutil.require_launcher()
_helm_ng = helmutil.import_helm_ng()
sys.stdout.reconfigure(line_buffering=True)
sys.stderr.reconfigure(line_buffering=True)


def parse_args():
    p = argparse.ArgumentParser(description="FS JIT vs interpreter comparison")
    p.add_argument("--kernel", "-k", default=helmutil.default_kernel())
    p.add_argument("--initrd", default=helmutil.default_initrd())
    p.add_argument("--max-insns", "-n", type=int, default=50_000_000,
                   help="Max instructions (default 50M)")
    p.add_argument("--smp", type=int, default=1)
    p.add_argument("--mem-mib", type=int, default=512)
    p.add_argument("--checkpoint-interval", type=int, default=500_000,
                   help="Register comparison interval (default 500K)")
    return p.parse_args()


def _make_sim(args):
    sim = _helm_ng.build_simulation(
        isa="aarch64", mode="fs", timing="virtual", mem_mib=args.mem_mib,
    )
    sim.load_kernel(kernel=args.kernel, initrd=args.initrd, smp=args.smp)
    return sim


def main():
    args = parse_args()

    if not args.kernel:
        print("[fs-jit-cmp] no kernel; run: scripts/manage-assets.sh download linux-rpi-kernel",
              file=sys.stderr)
        sys.exit(1)

    print(f"[fs-jit-cmp] kernel={args.kernel}  smp={args.smp}  "
          f"max={args.max_insns:,}  interval={args.checkpoint_interval:,}",
          file=sys.stderr)

    sim_i = _make_sim(args)
    sim_j = _make_sim(args)
    sim_j.set_jit(True)

    interval = args.checkpoint_interval
    done = 0
    t0 = time.monotonic()

    while done < args.max_insns:
        n = min(interval, args.max_insns - done)
        r_i = sim_i.run(n)
        r_j = sim_j.run_jit(n)
        done += n

        diffs = helmutil.reg_diff(
            helmutil.snapshot_regs(sim_i),
            helmutil.snapshot_regs(sim_j),
        )
        if diffs:
            wall = time.monotonic() - t0
            print(f"\n[fs-jit-cmp] DIVERGENCE at ~{done:,} insns ({wall:.1f}s)",
                  file=sys.stderr)
            for d in diffs:
                print(f"[fs-jit-cmp] {d}", file=sys.stderr)
            sim_i.finish()
            sim_j.finish()
            sys.exit(1)

        if r_i != "quantum" or r_j != "quantum":
            if r_i != r_j:
                print(f"[fs-jit-cmp] STOP MISMATCH: interp={r_i}  jit={r_j}",
                      file=sys.stderr)
                sys.exit(1)
            break

        wall = time.monotonic() - t0
        if wall > 2.0 and done % (interval * 10) == 0:
            mips = done / wall / 1e6
            print(f"\r[fs-jit-cmp] {done/1e6:.0f}M  {wall:.0f}s  {mips:.0f} MIPS  "
                  f"PC i={sim_i.pc:#x} j={sim_j.pc:#x}",
                  end="", file=sys.stderr, flush=True)

    wall = time.monotonic() - t0
    print(f"\n[fs-jit-cmp] OK: {done:,} insns match ({wall:.1f}s)", file=sys.stderr)
    sim_i.finish()
    sim_j.finish()


if __name__ == "__main__":
    main()
