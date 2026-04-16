#!/usr/bin/env python3
"""Debug Ned abort during L4Re hello-2.cfg loading.

Runs the L4Re EL2 boot fast (no plugins) to just before the Ned abort,
then attaches execlog with register state to capture the crash context.

Usage:
    target/release/helm-system-aarch64 --sim-trace=null: \
        examples/debug/l4re_ned.py [OPTIONS]

Options:
    --skip INSNS     Instructions to fast-forward before arming plugins
                     (default: 88000000)
    --window INSNS   Instructions to trace after arming (default: 10000000)
    --tail N         Ring-buffer size for execlog tail (default: 500)
    --pc-lo ADDR     Low bound of PC filter (hex, default: 0x200000 = ned .text)
    --pc-hi ADDR     High bound of PC filter (hex, default: 0x2aa000)
    --regs           Include register state in execlog lines
    --el EL          Filter to this exception level (default: all)
    --all-pcs        Trace all PCs (not just Ned range)
    --plugin SPEC    Extra phase-2 plugin in the form name:arg1=...,arg2=...
                     May be repeated.
"""
import argparse
import importlib.util
import os
import sys
import time
from pathlib import Path


def _require_helm_launcher() -> None:
    if getattr(sys, "_helm_launcher", None) not in {
        "helm-aarch64",
        "helm-system-aarch64",
    }:
        raise SystemExit(
            "Run via helm-system-aarch64, not directly via python."
        )


_require_helm_launcher()


def _root() -> Path:
    if "__file__" in globals():
        return Path(__file__).resolve().parents[2]
    a = Path(sys.argv[0])
    return (Path.cwd() / a).resolve().parents[2] if not a.is_absolute() else a.parents[2]


ROOT = _root()


def _import_helm_ng():
    try:
        import _helm_ng
        return _helm_ng
    except ImportError:
        pass
    for build in ("release", "debug"):
        p = ROOT / "target" / build / "lib_helm_ng.so"
        if p.is_file():
            spec = importlib.util.spec_from_file_location("_helm_ng", p)
            if spec and spec.loader:
                mod = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(mod)
                return mod
    import _helm_ng
    return _helm_ng


_helm_ng = _import_helm_ng()
sys.stdout.reconfigure(line_buffering=True)
sys.stderr.reconfigure(line_buffering=True)

# L4Re kernel + initrd paths
sys.path.insert(0, str(ROOT / "python"))
from helm.resources import obtain_resource

def _default_l4re_kernel() -> str:
    try:
        return obtain_resource("l4re-hello", download=False).path()
    except (FileNotFoundError, Exception):
        return str(ROOT / "assets" / "aarch64" / "boot" / "l4re" / "l4re_hello-2_arm_virt.elf")

KERNEL = _default_l4re_kernel()


def parse_hex(s: str) -> int:
    return int(s, 16) if s.startswith("0x") or s.startswith("0X") else int(s)


def parse_args():
    p = argparse.ArgumentParser(description="Debug L4Re Ned abort")
    p.add_argument("--skip", type=int, default=88_000_000,
                   help="Instructions to fast-forward (default 88M)")
    p.add_argument("--window", type=int, default=10_000_000,
                   help="Instructions to run with tracing (default 10M)")
    p.add_argument("--tail", type=int, default=500,
                   help="Execlog ring-buffer size (default 500)")
    p.add_argument("--pc-lo", type=parse_hex, default=0x200000,
                   help="PC filter low bound (default 0x200000)")
    p.add_argument("--pc-hi", type=parse_hex, default=0x2AA000,
                   help="PC filter high bound (default 0x2AA000)")
    p.add_argument("--regs", action="store_true",
                   help="Include register state in execlog")
    p.add_argument("--el", type=int, default=None,
                   help="Filter to exception level")
    p.add_argument("--all-pcs", action="store_true",
                   help="Trace all PCs (ignore pc-lo/pc-hi)")
    p.add_argument("--plugin", action="append", default=[],
                   help="Extra phase-2 plugin (name:arg1=...,arg2=...)")
    p.add_argument("--tpidrro", type=parse_hex, default=None,
                   help="Filter to tpidrro_el0 value (hex)")
    p.add_argument("--kernel", default=KERNEL,
                   help="L4Re ELF kernel path")
    p.add_argument("--max-insns", type=int, default=None,
                   help="Total instruction limit (default: skip + window)")
    return p.parse_args()


def main():
    args = parse_args()
    total = args.max_insns or (args.skip + args.window)

    if not os.path.isfile(args.kernel):
        print(f"[ned-debug] kernel not found: {args.kernel}", file=sys.stderr)
        sys.exit(1)

    # Build simulation
    sim = _helm_ng.build_simulation(
        isa="aarch64", mode="fs", timing="virtual", mem_mib=1024,
    )
    sim.load_kernel(
        kernel=args.kernel,
        gic_version="v2",
        boot_el=2,
    )
    sim.set_cpu_model("cortex-a55")

    # Phase 1: fast-forward without plugins
    print(f"[ned-debug] fast-forwarding {args.skip:,} instructions...",
          file=sys.stderr)
    t0 = time.monotonic()
    chunk = 10_000_000
    done = 0
    while done < args.skip:
        n = min(chunk, args.skip - done)
        r = sim.run(n)
        done += n
        if r != "quantum":
            wall = time.monotonic() - t0
            print(f"[ned-debug] stopped early at {sim.insn_count:,} insns "
                  f"({wall:.1f}s): {r}  PC={sim.pc:#x}", file=sys.stderr)
            sim.finish()
            return
    wall = time.monotonic() - t0
    mips = done / wall / 1e6 if wall > 0.01 else 0
    print(f"[ned-debug] skip done: {done:,} insns  {wall:.1f}s  {mips:.0f} MIPS  "
          f"PC={sim.pc:#x}  EL={sim.current_el}", file=sys.stderr)

    # Phase 2: attach execlog plugin
    execlog_args = f"tail=true,max={args.tail}"
    if args.regs:
        execlog_args += ",regs=true"
    if not args.all_pcs:
        execlog_args += f",pc_start={args.pc_lo:#x},pc_end={args.pc_hi:#x}"
    if args.el is not None:
        execlog_args += f",el={args.el}"
    if args.tpidrro is not None:
        execlog_args += f",tpidrro={args.tpidrro:#x}"
    print(f"[ned-debug] arming execlog: {execlog_args}", file=sys.stderr)
    sim.add_plugin("execlog", execlog_args)
    for plugin_spec in args.plugin:
        name, sep, plugin_args = plugin_spec.partition(":")
        if not name:
            print(f"[ned-debug] ignoring empty plugin spec: {plugin_spec!r}",
                  file=sys.stderr)
            continue
        if not sep:
            plugin_args = ""
        print(f"[ned-debug] arming plugin: {name} {plugin_args}",
              file=sys.stderr)
        sim.add_plugin(name, plugin_args)

    # Phase 3: run the trace window
    remaining = total - done
    print(f"[ned-debug] running {remaining:,} more instructions...",
          file=sys.stderr)
    t1 = time.monotonic()
    while done < total:
        n = min(chunk, total - done)
        r = sim.run(n)
        done += n
        wall2 = time.monotonic() - t1
        if r != "quantum":
            print(f"[ned-debug] stopped at {sim.insn_count:,} insns "
                  f"({wall2:.1f}s): {r}  PC={sim.pc:#x}", file=sys.stderr)
            break
        if wall2 > 5.0:
            mips2 = (done - args.skip) / wall2 / 1e6
            print(f"\r[ned-debug] {done/1e6:.0f}M insns  {wall2:.0f}s  "
                  f"{mips2:.0f} MIPS  PC={sim.pc:#x}",
                  end="", file=sys.stderr)

    # Print final state
    print(file=sys.stderr)
    print(f"[ned-debug] final: {sim.insn_count:,} insns  PC={sim.pc:#x}  "
          f"EL={sim.current_el}", file=sys.stderr)
    print(f"[ned-debug] x0={sim.xn(0):#x}  x1={sim.xn(1):#x}  "
          f"x2={sim.xn(2):#x}  x30={sim.xn(30):#x}", file=sys.stderr)

    # Check for unimplemented instructions
    if sim.has_unimplemented_instructions:
        print(f"[ned-debug] WARNING: {sim.unimplemented_instruction_count} "
              "unimplemented instructions encountered!", file=sys.stderr)
    else:
        print("[ned-debug] no unimplemented instructions", file=sys.stderr)

    # Flush execlog (prints to stderr via atexit)
    sim.finish()

    total_wall = time.monotonic() - t0
    print(f"[ned-debug] total wall time: {total_wall:.1f}s", file=sys.stderr)


if __name__ == "__main__":
    main()
