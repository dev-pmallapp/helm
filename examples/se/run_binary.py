#!/usr/bin/env python3
"""Run an AArch64 static binary in syscall-emulation mode.

Usage:
    helm-aarch64 examples/se/run_binary.py --binary ./my_elf
    helm-aarch64 ./hello                  # embedded mode
    helm-aarch64 ./hello -c "echo hi"
"""
import argparse
import os
import signal
import sys
import time

def _require_helm_launcher() -> None:
    if getattr(sys, "_helm_launcher", None) not in {"helm-aarch64", "helm-system-aarch64"}:
        raise SystemExit(
            "This example must be run via helm-aarch64 or helm-system-aarch64, not directly via python."
        )

_require_helm_launcher()

import _helm_ng

sys.stdout.reconfigure(line_buffering=True)


def parse_args():
    p = argparse.ArgumentParser(description="helm-ng SE — run AArch64 binary")
    p.add_argument("--binary", "-b",
                   default=os.environ.get("HELM_BINARY", "assets/aarch64/binaries/fish"))
    p.add_argument("--max-insns", "-n", type=int, default=500_000_000,
                   help="Max guest instructions (default 500M)")
    p.add_argument("--cpu", default="atomic",
                   choices=["atomic", "timing", "minor", "o3", "big"],
                   help="CPU model (selects timing model)")
    p.add_argument("--core-model", "--core", default=None,
                   help="ARM core model for ID registers (use --core-model help to list)")
    p.add_argument("--caches", action="store_true",
                   help="Enable cache simulation (Phase 1)")
    p.add_argument("--l2cache", action="store_true",
                   help="Enable L2 cache (Phase 1)")
    p.add_argument("--strace", action="store_true",
                   help="Print syscall trace")
    p.add_argument("--stub-trace", action="store_true",
                   help="Report unimplemented (stub) instructions")
    p.add_argument("--plugin", action="append", default=[],
                   help="Load a named plugin (e.g. insn-count, hotblocks, cache)")
    p.add_argument("--jit", action="store_true",
                   help="Enable dynasm JIT backend (AArch64 only)")
    p.add_argument("-E", dest="env_vars", action="append", default=[],
                   metavar="VAR=VALUE", help="Set target environment variable")
    args, guest_args = p.parse_known_args()

    # Handle --core-model help
    if args.core_model in ("help", "?", "list"):
        print("Available CPU models:")
        print()
        for name, desc in _helm_ng.list_cpu_models():
            print(f"  {name:<16} {desc}")
        sys.exit(0)

    # Strip leading '--' separator if present
    if guest_args and guest_args[0] == "--":
        guest_args = guest_args[1:]
    args.guest_args = guest_args
    return args


# Map gem5-style CPU names to helm-ng timing models
CPU_TIMING = {
    "atomic":  "virtual",
    "timing":  "interval",
    "minor":   "interval",
    "o3":      "accurate",
    "big":     "accurate",
}


def _stat_line(name: str, value, comment: str = "") -> str:
    comment_part = "" if not comment else f"  # {comment}"
    return f"{name:<40}{value:>20}{comment_part}"


def _print_sim_stats(sim, wall: float, stream=sys.stderr) -> None:
    stats = sim.stats()
    insns = int(stats.get("insn_count", sim.insn_count))
    ticks = int(stats.get("tick_count", stats.get("virtual_cycles", 0)))
    freq = int(stats.get("sim_freq", 1_000_000_000))
    ipc = float(stats.get("ipc", (insns / ticks) if ticks else 0.0))
    sim_seconds = ticks / freq if freq > 0 else 0.0
    host_mips = insns / wall / 1e6 if wall > 0.001 else 0.0

    print("---------- Begin Simulation Statistics ----------", file=stream)
    print(_stat_line("sim_insns", insns, "Instructions retired"), file=stream)
    print(_stat_line("sim_ticks", ticks, "Simulated cycles/ticks"), file=stream)
    print(_stat_line("sim_seconds", f"{sim_seconds:.6f}", "Simulated time"), file=stream)
    print(_stat_line("system.cpu.committedInsts", insns, "Committed instructions"), file=stream)
    print(_stat_line("system.cpu.ipc", f"{ipc:.6f}", "Instructions per tick"), file=stream)
    print(_stat_line("host_seconds", f"{wall:.6f}", "Wall clock runtime"), file=stream)
    print(_stat_line("host_mips", f"{host_mips:.3f}", "Million insts/sec"), file=stream)
    print(_stat_line("system.final_pc", f"{sim.pc:#018x}", "Final PC"), file=stream)
    print("----------  End Simulation Statistics  ----------", file=stream)


class _SigintFlag:
    def __init__(self) -> None:
        self.triggered = False

    def handler(self, _signum, _frame) -> None:
        self.triggered = True


def main():
    args = parse_args()
    binary = args.binary

    guest_args = args.guest_args
    if not guest_args:
        #guest_args = ["--no-config", "-c", "echo hello"]
        guest_args = ["-c", "echo hello"]

    argv = [os.path.basename(binary)] + guest_args
    envp = args.env_vars if args.env_vars else [
        "HOME=/tmp/home/pmallapp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm",
    ]

    if not os.path.isfile(binary):
        print(f"[se] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    timing = CPU_TIMING.get(args.cpu, "virtual")
    jit_tag = "  jit=on" if args.jit else ""
    print(f"[se] binary={binary}  argv={argv}  cpu={args.cpu}  timing={timing}{jit_tag}")

    # Build simulation
    sim = _helm_ng.build_simulation(
        isa="aarch64", mode="se", timing=timing,
    )
    sim.load_elf(binary, argv, envp)

    # Apply ARM core model (ID registers) if requested
    if args.core_model:
        try:
            sim.set_cpu_model(args.core_model)
        except Exception as e:
            print(f"[se] Warning: could not set core model '{args.core_model}': {e}",
                  file=sys.stderr)

    # Install plugins
    if args.strace:
        sim.add_plugin("syscall-trace")
    if args.stub_trace:
        sim.add_plugin("stub-tracer")
    for p in args.plugin:
        sim.add_plugin(p)

    # Enable JIT if requested
    if args.jit:
        sim.set_jit(True)

    t0 = time.monotonic()
    # Run in chunks so we can print progress for long-running binaries
    chunk = 50_000_000
    remaining = args.max_insns
    stop_reason = "quantum"
    wall = 0.0
    last_progress = 0.0
    run_fn = sim.run_jit if args.jit else sim.run
    interrupted = False
    sigint = _SigintFlag()
    old_sigint = signal.getsignal(signal.SIGINT)
    signal.signal(signal.SIGINT, sigint.handler)
    try:
        while remaining > 0 and not sim.has_exited:
            if sigint.triggered:
                interrupted = True
                stop_reason = "interrupt"
                print(f"\n[se] interrupted after {sim.insn_count:,} instructions", file=sys.stderr)
                break
            n = min(chunk, remaining)
            stop_reason = run_fn(n)
            remaining -= n
            wall = time.monotonic() - t0
            if stop_reason != "quantum":
                break
            if not sim.has_exited and wall > 2.0 and wall - last_progress >= 5.0:
                last_progress = wall
                mips = sim.insn_count / wall / 1e6
                print(f"\r[se] {sim.insn_count/1e6:.0f}M insns  {wall:.0f}s  {mips:.0f} MIPS",
                      end="", file=sys.stderr, flush=True)
    except KeyboardInterrupt:
        interrupted = True
        stop_reason = "interrupt"
        wall = time.monotonic() - t0
        if wall > 2.0:
            print(file=sys.stderr)
        print("\n[se] interrupted by Ctrl+C", file=sys.stderr)
    finally:
        signal.signal(signal.SIGINT, old_sigint)
        wall = time.monotonic() - t0
        sim.finish()  # trigger plugin atexit reports
        _print_sim_stats(sim, wall, stream=sys.stderr)

    if wall > 2.0 and not sim.has_exited and stop_reason == "quantum" and not interrupted:
        print(file=sys.stderr)  # newline after progress
    wall = time.monotonic() - t0
    mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0

    if sim.has_unimplemented_instructions:
        print(
            "[se] warning: binary executed "
            f"{sim.unimplemented_instruction_count} unique unimplemented instructions; "
            "future encounters of the same sites are ignored",
            file=sys.stderr,
        )

    if sim.has_exited:
        print(f"[se] exited with code {sim.exit_code}")
    elif stop_reason == "interrupt":
        print(f"[se] interrupted at PC={sim.pc:#x}", file=sys.stderr)
    elif stop_reason != "quantum":
        print(f"[se] stopped: {stop_reason} at PC={sim.pc:#x}", file=sys.stderr)
    else:
        print(f"[se] hit limit at PC={sim.pc:#x}")

    print(f"[se] {sim.insn_count:,} insns  {wall:.2f}s  {mips:.0f} MIPS")

    if sim.has_exited:
        sys.exit(sim.exit_code)
    if stop_reason == "interrupt":
        sys.exit(130)
    if stop_reason != "quantum":
        sys.exit(1)


if __name__ == "__main__":
    main()
