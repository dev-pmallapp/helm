#!/usr/bin/env python3
"""Debug memory corruption using watchpoints with value filtering.

Real-world scenario: During L4Re boot debugging (commit 383e095), memory
watchpoints were critical for catching stale TLB entries and unexpected
device register writes.  The watchpoint plugin tracks stores to a
specific address range, optionally filtering by value, and captures an
instruction window around the first hit for root-cause analysis.

This example demonstrates:
  - Setting a memory watchpoint on a stack or heap address
  - Value-aware filtering (trigger only when a specific value is stored)
  - Instruction context window around the watchpoint hit
  - Both SE-mode (stack variable) and FS-mode (MMIO register) patterns

Usage:
    helm-aarch64 examples/debug/watchpoint_debug.py --binary ./my_elf
    helm-aarch64 examples/debug/watchpoint_debug.py --binary ./my_elf \\
        --watch-addr 0x7fffffffe000 --watch-size 8 --watch-type write
    helm-aarch64 examples/debug/watchpoint_debug.py --binary ./my_elf \\
        --watch-value 0xdeadbeef  # only trigger on this value
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
        description="Memory watchpoint debugger — catch stores to a "
                    "target address with optional value filter"
    )
    p.add_argument("--binary", "-b", default=helmutil.default_binary())
    p.add_argument("--max-insns", "-n", type=int, default=10_000_000,
                   help="Max instructions to run (default 10M)")
    p.add_argument("--watch-addr", type=lambda x: int(x, 0), default=None,
                   help="Address to watch (hex).  If omitted, watches "
                        "the initial stack pointer region.")
    p.add_argument("--watch-size", type=int, default=8,
                   help="Watch region size in bytes (default 8)")
    p.add_argument("--watch-type", choices=["write", "all"], default="write",
                   help="Watch access type (default: write-only)")
    p.add_argument("--watch-value", type=lambda x: int(x, 0), default=None,
                   help="Optional value filter — trigger only on this "
                        "exact stored value (hex)")
    p.add_argument("--log-limit", type=int, default=16,
                   help="Max watchpoint hits to log (default 16)")
    p.add_argument("--window", type=int, default=32,
                   help="Instruction context window before first hit "
                        "(default 32)")
    p.add_argument("--argv", nargs="*", default=None)
    return p.parse_args()


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
        watch_addr = sim.sp - 256  # 256 bytes below initial SP
        print(f"[watch] No --watch-addr given; watching SP-256 = "
              f"{watch_addr:#018x}")

    wp_args = (
        f"addr={watch_addr:#x},"
        f"size={args.watch_size},"
        f"type={args.watch_type},"
        f"log-limit={args.log_limit},"
        f"window={args.window},"
        f"dump=atexit"
    )
    if args.watch_value is not None:
        wp_args += f",value={args.watch_value:#x}"

    print(f"[watch] binary={binary}")
    print(f"[watch] watchpoint: addr={watch_addr:#018x} "
          f"size={args.watch_size} type={args.watch_type}")
    if args.watch_value is not None:
        print(f"[watch] value filter: {args.watch_value:#018x}")
    print(f"[watch] window={args.window}  log-limit={args.log_limit}")
    print()

    sim.add_plugin("watchpoint", wp_args)

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
    print(f"[watch] stop_reason={stop_reason}  "
          f"final_pc={sim.pc:#018x}")
    if sim.has_exited:
        print(f"[watch] exit_code={sim.exit_code}")

    print(f"\n[watch] Register state at stop:")
    for i in range(0, 31, 4):
        regs = "  ".join(f"x{i+j:<2d}={sim.xn(i+j):#018x}"
                         for j in range(4) if i + j < 31)
        print(f"  {regs}")
    print(f"  SP ={sim.sp:#018x}  PC ={sim.pc:#018x}  "
          f"NZCV={sim.nzcv:#x}")

    print(f"\n{'='*60}")
    print("Watchpoint plugin report:")
    print("=" * 60)
    sim.finish()


if __name__ == "__main__":
    main()
