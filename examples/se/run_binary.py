#!/usr/bin/env python3
"""Run an AArch64 static binary in syscall-emulation mode.

Usage:
    helm-system-aarch64 examples/se/run_binary.py
    helm-system-aarch64 examples/se/run_binary.py -- --binary ./my_elf --max-insns 50000000

Environment:
    HELM_BINARY   Override the default binary path.
"""
import argparse
import os
import sys
import time

import _helm_core

sys.stdout.reconfigure(line_buffering=True)


def parse_args(argv=None):
    p = argparse.ArgumentParser(description="HELM SE — run AArch64 binary")
    p.add_argument("--binary", "-b",
                    default=os.environ.get("HELM_BINARY", "assets/binaries/fish"))
    p.add_argument("--args", nargs="*", default=["--no-config", "-c", "echo hello"],
                    help="Guest argv (after argv[0])")
    p.add_argument("--max-insns", type=int, default=100_000_000)
    return p.parse_args(argv)


def main():
    args = parse_args()
    binary = args.binary
    argv = [os.path.basename(binary)] + args.args
    envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm"]

    if not os.path.isfile(binary):
        print(f"[se] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    print(f"[se] binary={binary}  argv={argv}")
    s = _helm_core.SeSession(binary, argv, envp)

    t0 = time.monotonic()
    s.run(args.max_insns)
    wall = time.monotonic() - t0
    mips = s.insn_count / wall / 1e6 if wall > 0.001 else 0

    if s.has_exited:
        print(f"[se] exited with code {s.exit_code}")
    else:
        print(f"[se] hit limit at PC={s.pc:#x}")

    print(f"[se] {s.insn_count:,} insns  {wall:.2f}s  {mips:.0f} MIPS")


if __name__ == "__main__":
    main()
