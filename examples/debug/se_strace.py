#!/usr/bin/env python3
"""Trace syscalls made by an SE-mode AArch64 binary.

Attaches the syscall-trace plugin and runs the binary, printing each
syscall name, arguments, and return value as they execute.

Usage:
    target/release/helm-aarch64 examples/debug/se_strace.py [OPTIONS]

Examples:
    # Trace fish shell syscalls
    ... se_strace.py

    # Trace a custom binary with arguments
    ... se_strace.py --binary ./my_elf -- arg1 arg2

    # Trace with instruction count + stub report
    ... se_strace.py --plugin insn-count --plugin stub-tracer
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
    p = argparse.ArgumentParser(description="SE syscall trace")
    p.add_argument("--binary", "-b", default=helmutil.default_binary())
    p.add_argument("--max-insns", "-n", type=int, default=500_000_000,
                   help="Max instructions (default 500M)")
    p.add_argument("--plugin", action="append", default=[],
                   help="Additional plugin (e.g. insn-count, stub-tracer)")
    args, guest_args = p.parse_known_args()
    if guest_args and guest_args[0] == "--":
        guest_args = guest_args[1:]
    args.guest_args = guest_args
    return args


def main():
    args = parse_args()
    binary = args.binary

    if not os.path.isfile(binary):
        print(f"[strace] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    argv = args.guest_args or [os.path.basename(binary), "-c", "echo hello"]
    envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm"]

    print(f"[strace] binary={binary}  argv={argv}", file=sys.stderr)

    sim = _helm_ng.build_simulation(isa="aarch64", mode="se", timing="virtual")
    sim.load_elf(binary, argv, envp)
    sim.add_plugin("syscall-trace")

    for plugin_spec in args.plugin:
        parts = plugin_spec.split(":", 1)
        sim.add_plugin(parts[0], parts[1] if len(parts) > 1 else "")

    t0 = time.monotonic()
    chunk = 50_000_000
    remaining = args.max_insns
    while remaining > 0 and not sim.has_exited:
        n = min(chunk, remaining)
        stop = sim.run(n)
        remaining -= n
        if stop != "quantum":
            break

    wall = time.monotonic() - t0
    mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0
    print(f"\n[strace] {sim.insn_count:,} insns  {wall:.2f}s  {mips:.0f} MIPS",
          file=sys.stderr)

    if sim.has_exited:
        print(f"[strace] exited with code {sim.exit_code}")
    else:
        print(f"[strace] stopped at PC={sim.pc:#x}", file=sys.stderr)

    sim.finish()
    if sim.has_exited:
        sys.exit(sim.exit_code)


if __name__ == "__main__":
    main()
