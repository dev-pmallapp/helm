#!/usr/bin/env python3
"""Debug memory access with the native Python watchpoint control plane.

This example demonstrates the new Python-first debug workflow:

- configure a watchpoint with `System.watchpoint(...)`
- inspect active watchpoints with `System.watchpoints()`
- optionally save and restore a checkpoint to prove debug intent persists
- run a short SE-mode binary and let the native watchpoint engine log hits

Usage:
    helm-aarch64 examples/debug/watchpoint_debug.py --binary ./my_elf
    helm-aarch64 examples/debug/watchpoint_debug.py --binary ./my_elf \\
        --watch-addr 0x7fffffffe000 --watch-size 8 --watch-kind write
    helm-aarch64 examples/debug/watchpoint_debug.py --binary ./my_elf \\
        --checkpoint-roundtrip
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


def parse_args():
    p = argparse.ArgumentParser(
        description="Native watchpoint debugger using the Python control plane"
    )
    p.add_argument("--binary", "-b", default=helmutil.default_binary())
    p.add_argument("--max-insns", "-n", type=int, default=10_000_000,
                   help="Max instructions to run (default 10M)")
    p.add_argument("--watch-addr", type=lambda x: int(x, 0), default=None,
                   help="Address to watch (hex). If omitted, uses SP-256.")
    p.add_argument("--watch-size", type=int, default=8,
                   help="Watched region size in bytes (default 8)")
    p.add_argument("--watch-kind", choices=["read", "write", "rw"],
                   default="write",
                   help="Watchpoint kind (default: write)")
    p.add_argument("--checkpoint-roundtrip", action="store_true",
                   help="Save a checkpoint before run and restore it after stop")
    p.add_argument("--argv", nargs="*", default=None)
    return p.parse_args()


def _print_watchpoints(sim):
    watchpoints = sim.watchpoints()
    if not watchpoints:
        print("[watch] no active watchpoints")
        return
    print("[watch] active watchpoints:")
    for wp_id, start, size, kind, action, enabled in watchpoints:
        print(f"  id={wp_id} start={start:#018x} size={size:<4d} "
              f"kind={kind:<5s} action={action:<5s} enabled={enabled}")


def main():
    args = parse_args()
    binary = args.binary
    if not os.path.isfile(binary):
        print(f"[watch] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    argv = args.argv or [os.path.basename(binary), "-c", "echo hello"]
    envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C",
            "USER=helm"]

    sim = _helm_ng.build_simulation(isa="aarch64", mode="se",
                                     timing="virtual")
    sim.load_elf(binary, argv, envp)

    watch_addr = args.watch_addr
    if watch_addr is None:
        watch_addr = sim.sp - 256
        print(f"[watch] No --watch-addr given; watching SP-256 = "
              f"{watch_addr:#018x}")

    sim.watchpoint(watch_addr, size=args.watch_size, kind=args.watch_kind)

    print(f"[watch] binary={binary}")
    print(f"[watch] watchpoint: addr={watch_addr:#018x} "
          f"size={args.watch_size} kind={args.watch_kind}")
    _print_watchpoints(sim)

    checkpoint = None
    if args.checkpoint_roundtrip:
        checkpoint = bytes(sim.save_checkpoint())
        print(f"[watch] saved checkpoint ({len(checkpoint)} bytes)")

    t0 = time.monotonic()
    chunk = 1_000_000
    remaining = args.max_insns
    stop_reason = "quantum"

    while remaining > 0 and not sim.has_exited:
        n = min(chunk, remaining)
        stop_reason = sim.run(n)
        remaining -= n
        if stop_reason not in ("quantum",):
            break

    wall = time.monotonic() - t0
    mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0

    print(f"\n[watch] {sim.insn_count:,} insns in {wall:.2f}s "
          f"({mips:.0f} MIPS)")
    print(f"[watch] stop_reason={stop_reason}  final_pc={sim.pc:#018x}")
    if sim.has_exited:
        print(f"[watch] exit_code={sim.exit_code}")

    print(f"\n[watch] Register state at stop:")
    for i in range(0, 31, 4):
        regs = "  ".join(f"x{i+j:<2d}={sim.xn(i+j):#018x}"
                         for j in range(4) if i + j < 31)
        print(f"  {regs}")
    print(f"  SP ={sim.sp:#018x}  PC ={sim.pc:#018x}  "
          f"NZCV={sim.nzcv:#x}")

    if checkpoint is not None:
        print("\n[watch] restoring checkpoint to verify debug intent persists...")
        sim.restore_checkpoint(checkpoint)
        print(f"[watch] restored PC={sim.pc:#018x} SP={sim.sp:#018x}")
        _print_watchpoints(sim)

    sim.finish()


if __name__ == "__main__":
    main()
