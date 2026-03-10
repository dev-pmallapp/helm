#!/usr/bin/env python3
"""Run an AArch64 static binary in syscall-emulation mode.

Usage:
    helm-aarch64 examples/se/run_binary.py --binary ./my_elf
    helm-aarch64 ./hello                  # embedded mode
    helm-aarch64 ./hello -c "echo hi"
"""
import argparse
import os
import sys
import time

import _helm_core

sys.stdout.reconfigure(line_buffering=True)


def parse_args():
    p = argparse.ArgumentParser(description="HELM SE — run AArch64 binary")
    p.add_argument("--binary", "-b",
                    default=os.environ.get("HELM_BINARY", "assets/binaries/fish"))
    p.add_argument("--max-insns", "-n", type=int, default=500_000_000,
                    help="Max guest instructions (default 500M)")
    p.add_argument("--cpu", default="atomic",
                    choices=["atomic", "timing", "minor", "o3", "big"])
    p.add_argument("--caches", action="store_true")
    p.add_argument("--l2cache", action="store_true")
    p.add_argument("--strace", action="store_true")
    p.add_argument("--plugin", action="append", default=[])
    p.add_argument("-E", dest="env_vars", action="append", default=[],
                    metavar="VAR=VALUE", help="Set target environment variable")
    args, guest_args = p.parse_known_args()
    # Strip leading '--' separator if present
    if guest_args and guest_args[0] == "--":
        guest_args = guest_args[1:]
    args.guest_args = guest_args
    return args


def main():
    args = parse_args()
    binary = args.binary

    guest_args = args.guest_args
    if not guest_args:
        guest_args = ["-c", "echo hello"]

    argv = [os.path.basename(binary)] + guest_args
    envp = args.env_vars if args.env_vars else [
        "HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm",
    ]

    if not os.path.isfile(binary):
        print(f"[se] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    print(f"[se] binary={binary}  argv={argv}  cpu={args.cpu}")

    # Build plugins list
    plugins = list(args.plugin)
    if args.strace:
        plugins.append("syscall-trace")

    # Create session
    s = _helm_core.SeSession(binary, argv, envp)
    for p in plugins:
        s.add_plugin(p, "")

    t0 = time.monotonic()
    # Run in chunks so we can print progress for long-running binaries
    chunk = 50_000_000
    remaining = args.max_insns
    while remaining > 0 and not s.has_exited:
        n = min(chunk, remaining)
        s.run(n)
        remaining -= n
        wall = time.monotonic() - t0
        if not s.has_exited and wall > 2.0:
            mips = s.insn_count / wall / 1e6
            print(f"\r[se] {s.insn_count/1e6:.0f}M insns  {wall:.0f}s  {mips:.0f} MIPS",
                  end="", file=sys.stderr, flush=True)
    if wall > 2.0 and not s.has_exited:
        print(file=sys.stderr)  # newline after progress
    wall = time.monotonic() - t0
    mips = s.insn_count / wall / 1e6 if wall > 0.001 else 0

    if s.has_exited:
        print(f"[se] exited with code {s.exit_code}")
    else:
        print(f"[se] hit limit at PC={s.pc:#x}")

    print(f"[se] {s.insn_count:,} insns  {wall:.2f}s  {mips:.0f} MIPS")

    if s.has_exited:
        sys.exit(s.exit_code)


if __name__ == "__main__":
    main()
