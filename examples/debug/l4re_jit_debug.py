#!/usr/bin/env python3
"""Debug L4Re JIT execution on helm-ng's ARM virt platform.

Uses the JIT debug/trace framework to diagnose JIT issues during L4Re
EL2 boot. Supports three operating modes:

  compare   -- Run interpreter and JIT side by side, stop on first divergence
  trace     -- Run with JIT and emit block-level execution log
  bisect    -- Binary-search for the first instruction where JIT diverges

The compare and trace modes pre-install trace-window-fault, fault-detect,
and jit_rejects plugins on the JIT sim so any data abort dumps a full
pre-fault instruction window and JIT reject-reason histogram automatically.

Usage:
    cargo run --release --bin helm-system-aarch64 -- \\
        examples/debug/l4re_jit_debug.py \\
        --kernel assets/aarch64/l4re/l4re_ipcbench_arm_virt-el2.elf \\
        --smp 4 --boot-el 2 --gic-version v2 [--mode compare|trace|bisect]

Examples:
    # Compare interp vs JIT (default) — stops on first register divergence
    cargo run --release --bin helm-system-aarch64 -- \\
        examples/debug/l4re_jit_debug.py \\
        --kernel assets/aarch64/l4re/l4re_ipcbench_arm_virt-el2.elf \\
        --smp 4 --boot-el 2 --gic-version v2

    # Trace JIT blocks near the crash PC
    ... --mode trace --skip 0 --window 50000000 \\
        --pc-lo 0xffff40040000 --pc-hi 0xffff40050000

    # Bisect to the exact divergence instruction
    ... --mode bisect --max-insns 500000000
"""
import argparse
import os
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _sim_common import (
    require_helm_launcher, repo_root, import_helm_ng,
    add_fs_sim_args, build_fs_sim, snapshot_regs, diff_regs,
)

require_helm_launcher()
_helm_ng = import_helm_ng()
ROOT = repo_root()
sys.stdout.reconfigure(line_buffering=True)
sys.stderr.reconfigure(line_buffering=True)

CHUNK = 10_000_000


def parse_hex(s: str) -> int:
    return int(s, 16) if s.startswith(("0x", "0X")) else int(s)


def parse_args():
    p = argparse.ArgumentParser(description="L4Re JIT debug")
    add_fs_sim_args(p, default_boot_el=2, default_gic="v2", default_smp=1)
    p.add_argument("--mode", choices=("compare", "trace", "bisect"),
                   default="compare",
                   help="Debug mode (default: compare)")
    p.add_argument("--skip", type=int, default=0,
                   help="Instructions to fast-forward before tracing (trace mode)")
    p.add_argument("--window", type=int, default=5_000_000,
                   help="Instructions to trace after skip (trace mode)")
    p.add_argument("--pc-lo", type=parse_hex, default=None,
                   help="Trace window PC lower bound (hex)")
    p.add_argument("--pc-hi", type=parse_hex, default=None,
                   help="Trace window PC upper bound (hex)")
    p.add_argument("--tail", type=int, default=500,
                   help="Tail buffer size for jit-execlog (default 500)")
    p.add_argument("--checkpoint-interval", type=int, default=1_000_000,
                   help="Compare-mode check interval (default 1M)")
    p.add_argument("--fault-window", type=int, default=128,
                   help="trace-window-fault history depth (default 128)")
    p.add_argument("--verbose", "-v", action="store_true",
                   help="Print extra diagnostics")
    return p.parse_args()


def _fault_plugin_list(args) -> list[tuple[str, str]]:
    """Return the list of fault-debug plugins for the JIT sim."""
    fw = args.fault_window
    return [
        ("trace-window-fault", f"before={fw},after=8"),
        ("fault-detect", f"history={fw}"),
        ("jit_rejects", ""),
    ]


def _run_chunk(sim, n, jit=False):
    """Run n instructions, return (stop_reason, elapsed)."""
    t0 = time.monotonic()
    if jit:
        result = sim.run_jit(n)
    else:
        result = sim.run(n)
    return result, time.monotonic() - t0


# ── Compare mode ─────────────────────────────────────────────────────────────

def mode_compare(args):
    """Run interpreter and JIT in lockstep, stop on first divergence."""
    print(f"[jit-debug] compare mode: kernel={args.kernel}", file=sys.stderr)
    print(f"[jit-debug] max_insns={args.max_insns:,}  "
          f"checkpoint_interval={args.checkpoint_interval:,}", file=sys.stderr)

    sim_interp = build_fs_sim(args, helm_ng=_helm_ng)
    sim_jit = build_fs_sim(args, helm_ng=_helm_ng, plugins=_fault_plugin_list(args))
    sim_jit.set_jit(True)

    interval = args.checkpoint_interval
    done = 0
    t0 = time.monotonic()

    while done < args.max_insns:
        n = min(interval, args.max_insns - done)

        r_interp, _ = _run_chunk(sim_interp, n, jit=False)
        r_jit, _ = _run_chunk(sim_jit, n, jit=True)

        done += n

        regs_interp = snapshot_regs(sim_interp)
        regs_jit = snapshot_regs(sim_jit)
        diffs = diff_regs(regs_interp, regs_jit, label_a="interp", label_b="jit")

        if diffs:
            wall = time.monotonic() - t0
            print(f"\n[jit-debug] DIVERGENCE at insn ~{done:,} ({wall:.1f}s)",
                  file=sys.stderr)
            print(f"[jit-debug] interp stop={r_interp}  jit stop={r_jit}",
                  file=sys.stderr)
            for d in diffs:
                print(f"[jit-debug] {d}", file=sys.stderr)
            print(f"[jit-debug] interp PC={sim_interp.pc:#x}  "
                  f"jit PC={sim_jit.pc:#x}", file=sys.stderr)

            # Print JIT stats
            stats = sim_jit.stats()
            jit_keys = [k for k in sorted(stats.keys()) if "jit" in k.lower()
                        or "block" in k.lower() or "trace" in k.lower()
                        or "fallback" in k.lower()]
            if jit_keys:
                print("[jit-debug] JIT stats:", file=sys.stderr)
                for k in jit_keys:
                    print(f"  {k}: {stats[k]}", file=sys.stderr)

            sim_interp.finish()
            sim_jit.finish()
            return False

        if r_interp != "quantum" or r_jit != "quantum":
            wall = time.monotonic() - t0
            if r_interp != r_jit:
                print(f"\n[jit-debug] STOP REASON MISMATCH at ~{done:,} insns "
                      f"({wall:.1f}s)", file=sys.stderr)
                print(f"  interp: {r_interp}  jit: {r_jit}", file=sys.stderr)
                sim_interp.finish()
                sim_jit.finish()
                return False
            print(f"\n[jit-debug] both stopped: {r_interp} at ~{done:,} insns "
                  f"({wall:.1f}s)", file=sys.stderr)
            break

        # Progress
        wall = time.monotonic() - t0
        if wall > 2.0 and done % (interval * 10) == 0:
            mips = done / wall / 1e6
            print(f"\r[jit-debug] {done/1e6:.0f}M insns  {wall:.0f}s  "
                  f"{mips:.0f} MIPS  PC interp={sim_interp.pc:#x} "
                  f"jit={sim_jit.pc:#x}",
                  end="", file=sys.stderr, flush=True)

    wall = time.monotonic() - t0
    print(f"\n[jit-debug] OK: {done:,} insns match ({wall:.1f}s)",
          file=sys.stderr)
    print(f"[jit-debug] final PC interp={sim_interp.pc:#x}  "
          f"jit={sim_jit.pc:#x}", file=sys.stderr)
    sim_interp.finish()
    sim_jit.finish()
    return True


# ── Trace mode ───────────────────────────────────────────────────────────────

def mode_trace(args):
    """Run with JIT and emit block-level trace in a window."""
    print(f"[jit-debug] trace mode: kernel={args.kernel}", file=sys.stderr)
    print(f"[jit-debug] skip={args.skip:,}  window={args.window:,}",
          file=sys.stderr)

    sim = build_fs_sim(args, helm_ng=_helm_ng, plugins=_fault_plugin_list(args))
    sim.set_jit(True)

    # Phase 1: fast-forward
    if args.skip > 0:
        print(f"[jit-debug] fast-forwarding {args.skip:,} insns...",
              file=sys.stderr)
        t0 = time.monotonic()
        done = 0
        while done < args.skip:
            n = min(CHUNK, args.skip - done)
            r = sim.run_jit(n)
            done += n
            if r != "quantum":
                wall = time.monotonic() - t0
                print(f"[jit-debug] stopped during skip at {sim.insn_count:,} "
                      f"insns ({wall:.1f}s): {r}  PC={sim.pc:#x}",
                      file=sys.stderr)
                sim.finish()
                return
        wall = time.monotonic() - t0
        print(f"[jit-debug] skip done: {done:,} insns  {wall:.1f}s  "
              f"PC={sim.pc:#x}", file=sys.stderr)

    # Phase 2: set up trace window and logging
    tw_args = {}
    if args.pc_lo is not None:
        tw_args["start_pc"] = args.pc_lo
    if args.pc_hi is not None:
        tw_args["stop_pc"] = args.pc_hi
    tw_args["max_events"] = args.tail
    if tw_args:
        sim.set_jit_trace_window(**tw_args)
        print(f"[jit-debug] trace window: {tw_args}", file=sys.stderr)

    # Attach jit-execlog plugin
    execlog_args = f"tail=true,max={args.tail},show_exit=true"
    if args.pc_lo is not None:
        execlog_args += f",pc_start={args.pc_lo:#x}"
    if args.pc_hi is not None:
        execlog_args += f",pc_end={args.pc_hi:#x}"
    sim.add_plugin("jit-execlog", execlog_args)

    # Also attach interpreter execlog for the window (force_interpreter
    # gives per-instruction detail)
    interp_args = f"tail=true,max={args.tail},regs=true"
    if args.pc_lo is not None:
        interp_args += f",pc_start={args.pc_lo:#x}"
    if args.pc_hi is not None:
        interp_args += f",pc_end={args.pc_hi:#x}"
    sim.add_plugin("execlog", interp_args)

    # Use interpreter fallback so per-instruction plugins fire
    sim.set_jit_force_interpreter(True)

    # Phase 3: run the trace window
    total = args.skip + args.window
    print(f"[jit-debug] tracing {args.window:,} insns...", file=sys.stderr)
    t1 = time.monotonic()
    done = args.skip
    while done < total:
        n = min(CHUNK, total - done)
        r = sim.run_jit(n)
        done += n
        if r != "quantum":
            wall = time.monotonic() - t1
            print(f"[jit-debug] stopped at {sim.insn_count:,} insns "
                  f"({wall:.1f}s): {r}  PC={sim.pc:#x}", file=sys.stderr)
            break

    wall = time.monotonic() - t1
    print(f"[jit-debug] trace done: {sim.insn_count:,} insns  {wall:.1f}s  "
          f"PC={sim.pc:#x}", file=sys.stderr)

    # Print JIT stats
    stats = sim.stats()
    jit_keys = [k for k in sorted(stats.keys()) if "jit" in k.lower()
                or "block" in k.lower() or "trace" in k.lower()
                or "compiled" in k.lower() or "fallback" in k.lower()
                or "cache" in k.lower() or "unsupported" in k.lower()]
    if jit_keys:
        print("[jit-debug] JIT stats:", file=sys.stderr)
        for k in jit_keys:
            v = stats[k]
            if isinstance(v, dict):
                for sk, sv in sorted(v.items()):
                    print(f"  {k}.{sk}: {sv}", file=sys.stderr)
            else:
                print(f"  {k}: {v}", file=sys.stderr)

    sim.finish()


# ── Bisect mode ──────────────────────────────────────────────────────────────

def mode_bisect(args):
    """Binary search for the first instruction where JIT diverges."""
    print(f"[jit-debug] bisect mode: kernel={args.kernel}", file=sys.stderr)
    print(f"[jit-debug] searching in [0, {args.max_insns:,}]", file=sys.stderr)

    def _check_at(n):
        """Run both up to n insns and compare. Returns True if they match."""
        s_i = build_fs_sim(args, helm_ng=_helm_ng)
        s_j = build_fs_sim(args, helm_ng=_helm_ng)
        s_j.set_jit(True)
        r_i, _ = _run_chunk(s_i, n)
        r_j, _ = _run_chunk(s_j, n, jit=True)
        regs_i = snapshot_regs(s_i)
        regs_j = snapshot_regs(s_j)
        diffs = diff_regs(regs_i, regs_j, label_a="interp", label_b="jit")
        s_i.finish()
        s_j.finish()
        return len(diffs) == 0 and (r_i == r_j)

    # First check if there is any divergence at all
    t0 = time.monotonic()
    if _check_at(args.max_insns):
        wall = time.monotonic() - t0
        print(f"[jit-debug] no divergence found in {args.max_insns:,} insns "
              f"({wall:.1f}s)", file=sys.stderr)
        return

    # Binary search
    lo, hi = 0, args.max_insns
    while hi - lo > args.checkpoint_interval:
        mid = (lo + hi) // 2
        print(f"\r[jit-debug] bisect: [{lo:,}, {hi:,}]  testing {mid:,}",
              end="", file=sys.stderr, flush=True)
        if _check_at(mid):
            lo = mid
        else:
            hi = mid

    wall = time.monotonic() - t0
    print(f"\n[jit-debug] divergence between insn {lo:,} and {hi:,} "
          f"({wall:.1f}s)", file=sys.stderr)

    # Final comparison at hi with detail
    print(f"[jit-debug] running final comparison at {hi:,}...", file=sys.stderr)
    s_i = build_fs_sim(args, helm_ng=_helm_ng)
    s_j = build_fs_sim(args, helm_ng=_helm_ng)
    s_j.set_jit(True)

    # Run both to lo (known good)
    if lo > 0:
        s_i.run(lo)
        s_j.run_jit(lo)

    # Now run the remaining interval with execlog on both
    remain = hi - lo
    s_j.add_plugin("execlog", f"tail=true,max=200,regs=true")
    s_i.add_plugin("execlog", f"tail=true,max=200,regs=true")

    s_i.run(remain)
    s_j.run_jit(remain)

    regs_i = snapshot_regs(s_i)
    regs_j = snapshot_regs(s_j)
    diffs = diff_regs(regs_i, regs_j, label_a="interp", label_b="jit")

    print(f"[jit-debug] divergence detail:", file=sys.stderr)
    for d in diffs:
        print(f"  {d}", file=sys.stderr)

    # Print JIT stats
    stats = s_j.stats()
    jit_keys = [k for k in sorted(stats.keys()) if "jit" in k.lower()
                or "unsupported" in k.lower() or "fallback" in k.lower()]
    if jit_keys:
        print("[jit-debug] JIT stats:", file=sys.stderr)
        for k in jit_keys:
            v = stats[k]
            if isinstance(v, dict):
                for sk, sv in sorted(v.items()):
                    print(f"  {k}.{sk}: {sv}", file=sys.stderr)
            else:
                print(f"  {k}: {v}", file=sys.stderr)

    s_i.finish()
    s_j.finish()


def main():
    args = parse_args()

    if not os.path.isfile(args.kernel):
        print(f"[jit-debug] kernel not found: {args.kernel}", file=sys.stderr)
        sys.exit(1)

    if args.mode == "compare":
        ok = mode_compare(args)
        sys.exit(0 if ok else 1)
    elif args.mode == "trace":
        mode_trace(args)
    elif args.mode == "bisect":
        mode_bisect(args)


if __name__ == "__main__":
    main()
