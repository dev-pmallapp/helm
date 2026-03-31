#!/usr/bin/env python3
"""Boot Linux and trace a short instruction/branch window after a symbol.

Usage:
    helm-system-aarch64 examples/debug/trace_after_symbol.py
    helm-system-aarch64 examples/debug/trace_after_symbol.py -- --symbol init_kprobe_trace
"""
import argparse
import sys
import time
import types
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


ROOT = _root()
sys.path.insert(0, str(ROOT / "python"))


def _load_boot():
    path = ROOT / "examples" / "fs" / "boot_rpi_full.py"
    module = types.ModuleType("boot_rpi_full")
    module.__file__ = str(path)
    exec(compile(path.read_text(), str(path), "exec"), module.__dict__)
    return module


_boot = _load_boot()
_helm_ng = _boot._import_helm_ng()
ASSETS = _boot._resolve_assets_dir()
UART_BASE = _boot.UART_BASE


def main() -> None:
    parser = argparse.ArgumentParser(description="Trace after a kernel symbol during FS boot")
    parser.add_argument("--kernel", default=str(ASSETS / "vmlinuz-rpi"))
    parser.add_argument("--initrd", default=str(ASSETS / "initramfs-rpi"))
    parser.add_argument("--max-insns", type=int, default=200_000_000)
    parser.add_argument("--mem-mib", type=int, default=1024)
    parser.add_argument("--smp", type=int, default=1)
    parser.add_argument("--gic-version", choices=("v2", "v3"), default="v3")
    parser.add_argument("--tick-scale", type=int, default=100)
    parser.add_argument("--symbol", default="init_kprobe_trace")
    parser.add_argument("--pc", type=lambda s: int(s, 0), default=None)
    parser.add_argument("--insn-count", type=int, default=None)
    parser.add_argument("--events", default="insn,branch",
                        help="Comma-separated subset of: insn,branch,mem,all")
    parser.add_argument("--max-events", type=int, default=200)
    parser.add_argument(
        "--append",
        default=f"earlycon=pl011,0x{UART_BASE:08x} console=ttyAMA0 loglevel=8 printk.prefer_direct=1 initcall_debug",
    )
    args = parser.parse_args()

    dtb_path = _boot._resolve_dtb_path(None, args.mem_mib, args.initrd, args.append, args.smp)
    dtb_bytes = dtb_path.read_bytes()

    sim = _helm_ng.build_simulation(
        isa="aarch64",
        mode="fs",
        timing="virtual",
        mem_mib=args.mem_mib,
    )
    sim.load_kernel(
        kernel=args.kernel,
        dtb=None,
        dtb_bytes=dtb_bytes,
        initrd=args.initrd,
        num_cpus=args.smp,
        gic_version=args.gic_version,
    )
    if args.tick_scale > 1:
        sim.set_tick_scale(args.tick_scale)

    events = [part.strip() for part in args.events.split(",") if part.strip()]
    if args.insn_count is not None:
        sim.trace_after(insn_count=args.insn_count, events=events, max=args.max_events)
        trigger_desc = f"insn_count={args.insn_count}"
    elif args.pc is not None:
        sim.trace_after(pc=args.pc, events=events, max=args.max_events)
        trigger_desc = f"pc={args.pc:#018x}"
    else:
        sim.trace_after(symbol=args.symbol, events=events, max=args.max_events)
        trigger_desc = f"symbol={args.symbol}"

    print(
        f"[trace-after] trigger={trigger_desc} events={events} max={args.max_events} "
        f"tick_scale={args.tick_scale} max_insns={args.max_insns}",
        file=sys.stderr,
    )

    start = time.monotonic()
    stop_reason = "quantum"
    chunk = 10_000_000
    total = 0
    while total < args.max_insns:
        ran = min(chunk, args.max_insns - total)
        stop_reason = sim.run(ran)
        total += ran
        if stop_reason != "quantum":
            break

    wall = time.monotonic() - start
    print(
        f"[trace-after] stop_reason={stop_reason} insns={sim.insn_count} "
        f"wall={wall:.2f}s pc={sim.pc:#018x}",
        file=sys.stderr,
    )
    sim.finish()


if __name__ == "__main__":
    main()
