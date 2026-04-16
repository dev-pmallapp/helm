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
from pathlib import Path

def _require_helm_launcher() -> None:
    if getattr(sys, "_helm_launcher", None) not in {"helm-aarch64", "helm-system-aarch64"}:
        raise SystemExit(
            "This example must be run via helm-aarch64 or helm-system-aarch64, not directly via python."
        )

_require_helm_launcher()

def _root() -> Path:
    if "__file__" in globals():
        return Path(__file__).resolve().parents[2]
    argv0 = Path(sys.argv[0])
    if argv0.is_absolute():
        return argv0.parents[2]
    return (Path.cwd() / argv0).resolve().parents[2]


def _preferred_build(root: Path) -> str | None:
    try:
        exe = Path(sys.executable).resolve()
    except OSError:
        return None

    try:
        rel = exe.relative_to(root)
    except ValueError:
        return None

    parts = rel.parts
    if len(parts) >= 2 and parts[0] == "target" and parts[1] in {"debug", "release"}:
        return parts[1]
    return None


def _build_candidates(root: Path) -> tuple[str | None, list[Path]]:
    preferred = _preferred_build(root)
    builds: list[str] = []
    if preferred is not None:
        builds.append(preferred)
    for build in ("release", "debug"):
        if build not in builds:
            builds.append(build)
    return preferred, [root / "target" / build / "lib_helm_ng.so" for build in builds]


def _import_helm_ng():
    root = _root()
    try:
        import _helm_ng as module
        return module
    except ImportError:
        pass

    preferred, candidates = _build_candidates(root)
    for path in candidates:
        if path.is_file():
            import importlib.util

            build = path.parent.name
            if preferred is not None and build != preferred:
                build_cmd = "cargo build --release -p helm-python" if preferred == "release" else "cargo build -p helm-python"
                print(
                    f"[helm] warning: launcher from target/{preferred} is loading "
                    f"target/{build}/lib_helm_ng.so; rebuild the matching extension with: {build_cmd}",
                    file=sys.stderr,
                )
            spec = importlib.util.spec_from_file_location("_helm_ng", path)
            if spec and spec.loader:
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                return module

    import _helm_ng as module
    return module


_helm_ng = _import_helm_ng()

sys.stdout.reconfigure(line_buffering=True)

# Resource management
sys.path.insert(0, str(_root() / "python"))
from helm.resources import obtain_resource


def _default_binary() -> str:
    env_val = os.environ.get("HELM_BINARY")
    if env_val:
        return env_val
    try:
        return obtain_resource("fish-shell", download=False).path("fish")
    except (FileNotFoundError, Exception):
        return "assets/aarch64/binaries/fish"


def parse_args():
    p = argparse.ArgumentParser(description="helm-ng SE — run AArch64 binary")
    p.add_argument("--binary", "-b",
                   default=_default_binary())
    p.add_argument("--max-insns", "-n", type=int, default=500_000_000,
                   help="Max guest instructions (default 500M)")
    p.add_argument("--cpu", default="atomic",
                   choices=["atomic", "timing", "minor", "o3", "big"],
                   help="CPU model (selects timing model)")
    p.add_argument("--core-model", "--core", default=None,
                   help="ARM core model for ID registers (use --core-model help to list)")
    p.add_argument("--caches", action="store_true",
                   help="Deprecated: map legacy cache flag to interval timing")
    p.add_argument("--l2cache", action="store_true",
                   help="Deprecated: imply interval timing with a default L2 cache")
    p.add_argument("--strace", action="store_true",
                   help="Print syscall trace")
    p.add_argument("--stub-trace", action="store_true",
                   help="Report unimplemented (stub) instructions")
    p.add_argument("--plugin", action="append", default=[],
                   help="Load a named plugin (e.g. insn-count, hotblocks, cache)")
    p.add_argument("--jit", action="store_true",
                   help="Enable HAJ (Helm Adaptive JIT: stencil baseline, dynasm hot-tier promotion, interpreter fallback)")
    p.add_argument("--interval-len", type=int, default=None,
                   help="Interval timing instruction window length")
    p.add_argument("--l1d-size", default=None,
                   help="Interval timing L1D size, e.g. 32KiB")
    p.add_argument("--l1d-assoc", type=int, default=None,
                   help="Interval timing L1D associativity")
    p.add_argument("--l1d-line", type=int, default=None,
                   help="Interval timing L1D line size in bytes")
    p.add_argument("--l2-size", default=None,
                   help="Interval timing L2 size, e.g. 256KiB")
    p.add_argument("--l2-assoc", type=int, default=None,
                   help="Interval timing L2 associativity")
    p.add_argument("--l2-line", type=int, default=None,
                   help="Interval timing L2 line size in bytes")
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

def _build_timing_string(base_timing, args):
    overrides = [
        ("interval_len", args.interval_len),
        ("l1d_size", args.l1d_size),
        ("l1d_assoc", args.l1d_assoc),
        ("l1d_line", args.l1d_line),
        ("l2_size", args.l2_size),
        ("l2_assoc", args.l2_assoc),
        ("l2_line", args.l2_line),
    ]
    active = [(key, value) for key, value in overrides if value is not None]
    if base_timing != "interval":
        if active:
            raise SystemExit(
                "interval timing overrides require a CPU model that maps to interval timing"
            )
        return base_timing
    if not active:
        return base_timing
    return "interval:" + ",".join(f"{key}={value}" for key, value in active)


def _resolve_timing(args):
    base_timing = CPU_TIMING.get(args.cpu, "virtual")
    if not (args.caches or args.l2cache):
        return _build_timing_string(base_timing, args)

    print(
        "[se] note: --caches and --l2cache are deprecated; "
        "prefer --cpu timing/minor or explicit interval timing options",
        file=sys.stderr,
    )
    if base_timing == "accurate":
        raise SystemExit(
            "--caches/--l2cache are not supported with accurate CPU models; "
            "use an interval-timing CPU model instead"
        )

    if args.l2cache and args.l2_size is None:
        args.l2_size = "256KiB"

    return _build_timing_string("interval", args)


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

    timing = _resolve_timing(args)
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
