#!/usr/bin/env python3
"""Trace execution at a specific PC or PC range with full register and
memory context.

Real-world scenario: During L4Re EL2 boot debugging (commit 05ca4b4),
pc-trace was used to capture every execution of a specific relocation
processing loop at 0x4100d160.  The plugin recorded register state and
memory accesses at each hit, revealing an off-by-one in loop iteration
count caused by stale NZCV flags from JIT block chaining.

Also useful for:
  - Monitoring a hot function to understand register flow
  - Catching unexpected re-entry into an error handler
  - Watching MMIO register accesses in FS mode

This example demonstrates:
  - pc-trace plugin with exact PC or range match
  - Full register logging (regs=full) on each hit
  - Memory access correlation (mem=all)
  - Configurable dump timing (on fault vs atexit)

Usage:
    helm-aarch64 examples/debug/pc_trace.py --binary ./my_elf \\
        --pc 0x400500
    helm-aarch64 examples/debug/pc_trace.py --binary ./my_elf \\
        --pc-start 0x400500 --pc-end 0x400600 --max-hits 64
    helm-aarch64 examples/debug/pc_trace.py --binary ./my_elf \\
        --symbol main --regs delta
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
        description="PC-trace debugger — capture execution context at "
                    "specific addresses"
    )
    p.add_argument("--binary", "-b", default=helmutil.default_binary())
    p.add_argument("--max-insns", "-n", type=int, default=50_000_000)
    grp = p.add_mutually_exclusive_group()
    grp.add_argument("--pc", type=lambda x: int(x, 0), default=None,
                     help="Exact PC to trace (hex)")
    grp.add_argument("--symbol", type=str, default=None,
                     help="Symbol name to trace (resolved from ELF)")
    p.add_argument("--pc-start", type=lambda x: int(x, 0), default=None,
                   help="Start of PC range (hex, with --pc-end)")
    p.add_argument("--pc-end", type=lambda x: int(x, 0), default=None,
                   help="End of PC range (hex, with --pc-start)")
    p.add_argument("--max-hits", type=int, default=32,
                   help="Max hits to record (default 32)")
    p.add_argument("--regs", choices=["full", "delta", "none"],
                   default="full",
                   help="Register logging mode (default: full)")
    p.add_argument("--mem", choices=["all", "reads", "writes", "none"],
                   default="all",
                   help="Memory access filter (default: all)")
    p.add_argument("--mem-max", type=int, default=4,
                   help="Max memory accesses per hit (default 4)")
    p.add_argument("--dump", choices=["fault", "atexit", "both"],
                   default="atexit",
                   help="When to emit report (default: atexit)")
    p.add_argument("--argv", nargs="*", default=None)
    return p.parse_args()


def main():
    args = parse_args()
    binary = args.binary
    if not os.path.isfile(binary):
        print(f"[pc-trace] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    argv = args.argv or [os.path.basename(binary), "-c", "echo hello"]
    envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C",
            "USER=helm"]

    sim = _helm_ng.build_simulation(isa="aarch64", mode="se",
                                     timing="virtual")
    sim.load_elf(binary, argv, envp)

    trace_pc = args.pc
    if args.symbol:
        addr = sim.resolve_symbol(args.symbol)
        if addr is None:
            # Try to find it in the symbol table
            syms = sim.symbols()
            matches = [(n, a) for n, a, _ in syms
                       if args.symbol in n]
            if matches:
                name, addr = matches[0]
                print(f"[pc-trace] Partial match: '{args.symbol}' -> "
                      f"'{name}' @ {addr:#x}")
            else:
                print(f"[pc-trace] Symbol '{args.symbol}' not found. "
                      f"Available symbols ({len(syms)}):")
                for name, addr, size in syms[:20]:
                    print(f"  {addr:#018x}  {name} ({size} bytes)")
                if len(syms) > 20:
                    print(f"  ... and {len(syms)-20} more")
                sys.exit(1)
        trace_pc = addr
        print(f"[pc-trace] Resolved '{args.symbol}' -> {trace_pc:#018x}")

    pt_args_parts = []
    if trace_pc is not None:
        pt_args_parts.append(f"pc={trace_pc:#x}")
    elif args.pc_start is not None and args.pc_end is not None:
        pt_args_parts.append(f"pc_start={args.pc_start:#x}")
        pt_args_parts.append(f"pc_end={args.pc_end:#x}")
    else:
        # Default: trace the entry point
        trace_pc = sim.pc
        pt_args_parts.append(f"pc={trace_pc:#x}")
        print(f"[pc-trace] No --pc/--symbol given; tracing entry point "
              f"{trace_pc:#018x}")

    pt_args_parts.extend([
        f"max={args.max_hits}",
        f"regs={args.regs}",
        f"mem={args.mem}",
        f"mem-max={args.mem_max}",
        f"dump={args.dump}",
    ])
    pt_args = ",".join(pt_args_parts)

    print(f"[pc-trace] binary={binary}")
    print(f"[pc-trace] plugin args: {pt_args}")
    print(f"[pc-trace] Running up to {args.max_insns:,} instructions...")
    print()

    sim.add_plugin("pc-trace", pt_args)

    t0 = time.monotonic()
    chunk = 5_000_000
    remaining = args.max_insns

    while remaining > 0 and not sim.has_exited:
        n = min(chunk, remaining)
        stop_reason = sim.run(n)
        remaining -= n
        if stop_reason != "quantum":
            print(f"[pc-trace] stopped: {stop_reason}")
            break

    wall = time.monotonic() - t0
    mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0

    print(f"\n[pc-trace] {sim.insn_count:,} insns in {wall:.2f}s "
          f"({mips:.0f} MIPS)")
    if sim.has_exited:
        print(f"[pc-trace] exit_code={sim.exit_code}")

    print(f"\n[pc-trace] Final register state:")
    for i in range(0, 31, 4):
        regs = "  ".join(f"x{i+j:<2d}={sim.xn(i+j):#018x}"
                         for j in range(4) if i + j < 31)
        print(f"  {regs}")
    print(f"  SP={sim.sp:#018x}  PC={sim.pc:#018x}  NZCV={sim.nzcv:#x}")

    print(f"\n{'='*60}")
    print("PC-trace plugin report:")
    print("=" * 60)
    sim.finish()


if __name__ == "__main__":
    main()
