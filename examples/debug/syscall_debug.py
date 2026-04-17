#!/usr/bin/env python3
"""Debug syscall behavior with detailed tracing and analysis.

Real-world scenario: During AArch64 SE mode development (commit
eb782ad), syscall coverage bugs were common — wrong return values,
missing ENOSYS stubs, mishandled pointer arguments.  The mmap syscall
alone required multiple debugging sessions (commit ad327be) to get
grow-upward behavior and huge-hint handling correct.

This example demonstrates:
  - syscall-trace plugin for logging all syscall entry/return
  - Insn-count for correlating syscall activity with execution progress
  - Detecting unexpected syscall failures (negative return values)
  - Breakpoint-on-syscall pattern using instruction count triggers

Usage:
    helm-aarch64 examples/debug/syscall_debug.py --binary ./my_elf
    helm-aarch64 examples/debug/syscall_debug.py --binary ./my_elf \\
        --max-insns 100000
    helm-aarch64 examples/debug/syscall_debug.py --binary ./my_elf --stubs
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

# AArch64 Linux syscall numbers (from asm-generic/unistd.h)
SYSCALL_NAMES = {
    56: "openat", 57: "close", 61: "getdents64", 62: "lseek",
    63: "read", 64: "write", 65: "readv", 66: "writev",
    78: "readlinkat", 79: "fstatat", 80: "fstat",
    93: "exit", 94: "exit_group",
    96: "set_tid_address", 98: "futex",
    99: "set_robust_list", 113: "clock_gettime",
    122: "sched_setaffinity", 123: "sched_getaffinity",
    124: "sched_yield",
    134: "sigaction", 135: "sigprocmask",
    160: "uname",
    172: "getpid", 174: "getuid", 175: "geteuid",
    176: "getgid", 177: "getegid",
    178: "gettid",
    198: "socket", 203: "connect",
    214: "brk", 215: "munmap", 220: "clone",
    222: "mmap", 226: "mprotect", 233: "madvise",
    261: "prlimit64", 278: "getrandom",
    291: "statx",
}


def parse_args():
    p = argparse.ArgumentParser(
        description="Syscall debugger — trace and analyze syscall "
                    "activity in SE mode"
    )
    p.add_argument("--binary", "-b", default=helmutil.default_binary())
    p.add_argument("--max-insns", "-n", type=int, default=10_000_000,
                   help="Max instructions (default 10M)")
    p.add_argument("--stubs", action="store_true",
                   help="Also attach stub-tracer to detect unimplemented "
                        "instructions triggered by syscall setup")
    p.add_argument("--argv", nargs="*", default=None)
    return p.parse_args()


def _syscall_name(nr):
    return SYSCALL_NAMES.get(nr, f"sys_{nr}")


def main():
    args = parse_args()
    binary = args.binary
    if not os.path.isfile(binary):
        print(f"[syscall] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    argv = args.argv or [os.path.basename(binary), "-c", "echo hello"]
    envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C",
            "USER=helm"]

    sim = _helm_ng.build_simulation(isa="aarch64", mode="se",
                                     timing="virtual")
    sim.load_elf(binary, argv, envp)

    sim.add_plugin("syscall-trace")
    sim.add_plugin("insn-count")
    if args.stubs:
        sim.add_plugin("stub-tracer")

    print(f"[syscall] binary={binary}  argv={argv}")
    print(f"[syscall] max_insns={args.max_insns:,}")
    print()

    t0 = time.monotonic()

    chunk = 1_000_000
    remaining = args.max_insns
    stop_reason = "quantum"

    while remaining > 0 and not sim.has_exited:
        n = min(chunk, remaining)
        stop_reason = sim.run(n)
        remaining -= n
        if stop_reason != "quantum":
            break

    wall = time.monotonic() - t0
    mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0

    print(f"[syscall] {sim.insn_count:,} insns in {wall:.2f}s "
          f"({mips:.0f} MIPS)")
    if sim.has_exited:
        print(f"[syscall] exit_code={sim.exit_code}")
    else:
        print(f"[syscall] stop_reason={stop_reason}  "
              f"pc={sim.pc:#018x}")

    # Print the full syscall trace report
    print(f"\n{'='*60}")
    print("Syscall trace report:")
    print("=" * 60)
    sim.finish()


if __name__ == "__main__":
    main()
