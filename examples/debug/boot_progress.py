#!/usr/bin/env python3
"""Track kernel boot progress with optional late-boot focused reporting.

Usage:
    helm-system-aarch64 examples/debug/boot_progress.py
    helm-system-aarch64 examples/debug/boot_progress.py -- --max-insns 200000000
"""
import argparse, bisect, sys, time, types
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
    a = Path(sys.argv[0])
    return (Path.cwd() / a).resolve().parents[2] if not a.is_absolute() else a.parents[2]


ROOT = _root()
sys.path.insert(0, str(ROOT / "python"))


def _load_boot():
    """Load boot_rpi_full.py without requiring the repo root as a package path."""
    p = ROOT / "examples" / "fs" / "boot_rpi_full.py"
    m = types.ModuleType("boot_rpi_full")
    m.__file__ = str(p)
    exec(compile(p.read_text(), str(p), "exec"), m.__dict__)
    return m


_boot = _load_boot()
_helm_ng = _boot._import_helm_ng()
ASSETS = _boot._resolve_assets_dir()
UART_BASE = _boot.UART_BASE

sys.stdout.reconfigure(line_buffering=True)


def load_sysmap(path: str):
    addrs, names = [], []
    with open(path) as f:
        for line in f:
            parts = line.split()
            if len(parts) >= 3:
                try:
                    addrs.append(int(parts[0], 16))
                    names.append(parts[2])
                except ValueError:
                    pass
    return addrs, names


def sym(va: int, addrs, names) -> str:
    if not addrs:
        return ""
    idx = bisect.bisect_right(addrs, va) - 1
    if idx < 0:
        return ""
    off = va - addrs[idx]
    return f"{names[idx]}+{off:#x}" if off else names[idx]


def print_jit_stats(stats, prefix="  "):
    print(f"{prefix}jit_fallback_count={stats.get('jit_fallback_count', 0)}")
    print(f"{prefix}jit_fallback_insns={stats.get('jit_fallback_insns', 0)}")
    print(
        f"{prefix}jit_unsupported_block_starts="
        f"{stats.get('jit_unsupported_block_starts', 0)}"
    )
    print(f"{prefix}jit_block_cache_hits={stats.get('jit_block_cache_hits', 0)}")
    print(f"{prefix}jit_block_cache_misses={stats.get('jit_block_cache_misses', 0)}")
    print(f"{prefix}jit_blocks_compiled={stats.get('jit_blocks_compiled', 0)}")
    print(f"{prefix}jit_blocks_executed={stats.get('jit_blocks_executed', 0)}")
    print(f"{prefix}jit_traces_compiled={stats.get('jit_traces_compiled', 0)}")
    print(f"{prefix}jit_traces_executed={stats.get('jit_traces_executed', 0)}")
    unsupported = stats.get("jit_unsupported_opcodes", {})
    if unsupported:
        items = sorted(unsupported.items(), key=lambda kv: (-kv[1], kv[0]))[:10]
        print(f"{prefix}jit_unsupported_opcodes_top10:")
        for opcode, count in items:
            print(f"{prefix}  {opcode}: {count}")


def print_user_stage2_stats(stats, prefix="  "):
    print(
        f"{prefix}user_stage2_insn_abort_events="
        f"{stats.get('user_stage2_insn_abort_events', 0)}"
    )
    print(
        f"{prefix}user_stage2_insn_abort_repeats="
        f"{stats.get('user_stage2_insn_abort_repeats', 0)}"
    )


def main():
    p = argparse.ArgumentParser(description="Kernel boot progress tracker")
    p.add_argument("--kernel",    default=str(ASSETS/"vmlinuz-rpi"))
    p.add_argument("--initrd",    default=str(ASSETS/"initramfs-rpi"))
    p.add_argument("--max-insns", type=int, default=500_000_000)
    p.add_argument("--mem-mib",   type=int, default=1024)
    p.add_argument("--core-model", default="cortex-a53")
    p.add_argument("--jit", action="store_true",
                   help="Use HAJ (Helm Adaptive JIT: stencil baseline, dynasm hot-tier promotion, interpreter fallback) instead of the interpreter for each checkpoint chunk")
    p.add_argument("--jit-stats", action="store_true",
                   help="Print selected JIT stats after the run")
    p.add_argument("--after-insns", type=int, default=None,
                   help="Once this retired-insn threshold is reached, switch to denser checkpoints and optionally arm trace_after")
    p.add_argument("--after-step", type=int, default=25_000_000,
                   help="Checkpoint spacing after --after-insns is crossed")
    p.add_argument("--attach-plugin", action="append", default=[],
                   help="Attach a built-in plugin after --after-insns as NAME or NAME:arg=val,... (repeatable)")
    p.add_argument("--trace-events", default="insn,branch",
                   help="Comma-separated trace_after events to arm at --after-insns (default: insn,branch)")
    p.add_argument("--trace-max", type=int, default=200,
                   help="Maximum trace_after events once --after-insns triggers")
    p.add_argument("--sysmap", default=None,
                   help="Optional System.map for PC symbolization")
    args = p.parse_args()

    dtb = _boot._resolve_dtb_path(
        None,
        args.mem_mib,
        args.initrd,
        f"earlycon=pl011,0x{UART_BASE:08x} console=ttyAMA0 loglevel=8",
    )
    sim = _helm_ng.build_simulation(isa="aarch64", mode="fs", timing="virtual",
                                     mem_mib=args.mem_mib)
    sim.load_kernel(kernel=args.kernel, dtb=str(dtb), initrd=args.initrd)
    if args.core_model:
        sim.set_cpu_model(args.core_model)
    if args.jit:
        sim.set_jit(True)
    sim.add_plugin("insn-count")

    addrs, names = [], []
    if args.sysmap:
        addrs, names = load_sysmap(args.sysmap)

    if args.after_insns is not None:
        events = [part.strip() for part in args.trace_events.split(",") if part.strip()]
        sim.trace_after(insn_count=args.after_insns, events=events, max=args.trace_max)

    default_checkpoints = [
        100_000,
        1_000_000,
        5_000_000,
        10_000_000,
        25_000_000,
        50_000_000,
        100_000_000,
        200_000_000,
        500_000_000,
    ]
    checkpoints = []
    seen = set()
    for cp in default_checkpoints:
        if cp <= args.max_insns and cp not in seen:
            checkpoints.append(cp)
            seen.add(cp)
    if args.after_insns is not None and args.after_step > 0:
        cp = args.after_insns
        while cp <= args.max_insns:
            if cp not in seen:
                checkpoints.append(cp)
                seen.add(cp)
            cp += args.after_step
    if args.max_insns not in seen:
        checkpoints.append(args.max_insns)
    checkpoints.sort()

    print(
        f"{'Insns':>14}  {'Wall(s)':>8}  {'MIPS':>7}  {'PC':>18}  "
        f"{'EL':>2}  {'ELR_EL1':>18}  {'FAR_EL1':>18}  {'ESR_EL1':>10}  {'CSP':>18}  Note"
    )
    print("-" * 150)
    t0 = time.monotonic()
    prev = 0
    attached = False
    after_stats_printed = False
    for cp in checkpoints:
        if cp > args.max_insns:
            break
        chunk = cp - prev
        if chunk <= 0:
            continue
        r = sim.run_jit(chunk) if args.jit else sim.run(chunk)
        prev = cp
        wall = time.monotonic() - t0
        mips = cp / wall / 1e6 if wall > 0 else 0
        # Heuristic: kernel VA space starts above 0xffff_0000_0000_0000
        in_kva = sim.pc > 0xffff_0000_0000_0000
        note = "kernel-VA" if in_kva else "physical"
        if r != "quantum":
            note += f" ({r})"
        print(
            f"{cp:>14,}  {wall:>8.2f}  {mips:>7.1f}  "
            f"{sim.pc:#018x}  {sim.current_el:>2}  "
            f"{sim.elr_el1:#018x}  {sim.far_el1:#018x}  "
            f"{sim.esr_el1:#010x}  {sim.current_sp:#018x}  {note}"
        )
        if (
            args.after_insns is not None
            and not attached
            and cp >= args.after_insns
            and args.attach_plugin
        ):
            print(" " * 61 + f"attaching plugins at insn_count={cp:,}")
            for spec in args.attach_plugin:
                name, plugin_args = spec.split(":", 1) if ":" in spec else (spec, "")
                sim.add_plugin(name, plugin_args)
                if plugin_args:
                    print(" " * 61 + f"plugin={name} args={plugin_args}")
                else:
                    print(" " * 61 + f"plugin={name}")
            attached = True
        symbol = sym(sim.pc, addrs, names)
        if symbol:
            print(" " * 61 + f"pc_sym={symbol}")
        if sim.far_el1 != 0:
            print(
                " " * 61
                + f"x0={sim.xn(0):#018x} x1={sim.xn(1):#018x} "
                + f"x2={sim.xn(2):#018x} x3={sim.xn(3):#018x} "
                + f"x29={sim.xn(29):#018x} x30={sim.xn(30):#018x}"
            )
        if args.after_insns is not None and cp >= args.after_insns:
            stats = sim.stats()
            print(" " * 61 + "late-window stats:")
            print(
                " " * 61
                + f"tick_count={stats.get('tick_count', 0)} "
                + f"ipc={stats.get('ipc', 0):.3f}"
            )
            print_user_stage2_stats(stats, prefix=" " * 61)
            if args.jit_stats:
                print_jit_stats(stats, prefix=" " * 61)
            after_stats_printed = True
        if r != "quantum":
            break

    sim.finish()
    if args.jit_stats and not after_stats_printed:
        stats = sim.stats()
        print()
        print("JIT stats:")
        print_user_stage2_stats(stats)
        print_jit_stats(stats)


if __name__ == "__main__":
    main()
