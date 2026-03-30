#!/usr/bin/env python3
"""Show the hottest basic blocks executed during kernel boot.

Uses the hotblocks plugin to count how many times each basic block is
entered.  After the run the top-N blocks are printed.  Pass --sysmap to
resolve addresses to kernel symbols.

Usage:
    helm-system-aarch64 examples/debug/hotblocks.py
    helm-system-aarch64 examples/debug/hotblocks.py -- --max-insns 50000000 --top 30
"""
import argparse, bisect, sys
import argparse, sys, types
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
    """Load boot_rpi_full.py via compile()+exec() — no __pycache__ created,
    equivalent to how the CLI launcher runs scripts via py.run_bound()."""
    p = ROOT / "examples" / "fs" / "boot_rpi_full.py"
    m = types.ModuleType("boot_rpi_full")
    m.__file__ = str(p)
    exec(compile(p.read_text(), str(p), "exec"), m.__dict__)
    return m

_boot     = _load_boot()
_helm_ng  = _boot._import_helm_ng()
ASSETS    = _boot._resolve_assets_dir()
UART_BASE = _boot.UART_BASE

def _resolve_dtb(mem_mib, initrd, append):
    return _boot._resolve_dtb_path(None, mem_mib, initrd, append)

sys.stdout.reconfigure(line_buffering=True)


def load_sysmap(path):
    addrs, names = [], []
    with open(path) as f:
        for line in f:
            p = line.split()
            if len(p) >= 3:
                try: addrs.append(int(p[0], 16)); names.append(p[2])
                except ValueError: pass
    return addrs, names


def sym(va, addrs, names):
    i = bisect.bisect_right(addrs, va) - 1
    if i < 0: return f"<{va:#018x}>"
    off = va - addrs[i]
    return f"{names[i]}+{off:#x}" if off else names[i]


def main():
    p = argparse.ArgumentParser(description="Hot basic block profiler")
    p.add_argument("--kernel",    default=str(ASSETS/"vmlinuz-rpi"))
    p.add_argument("--initrd",    default=str(ASSETS/"initramfs-rpi"))
    p.add_argument("--max-insns", type=int, default=20_000_000)
    p.add_argument("--mem-mib",   type=int, default=1024)
    p.add_argument("--top",       type=int, default=20)
    p.add_argument("--sysmap",    default=None)
    args = p.parse_args()

    addrs, names = [], []
    if args.sysmap:
        addrs, names = load_sysmap(args.sysmap)
    else:
        # Auto-detect System.map alongside the kernel
        suffix = __import__('pathlib').Path(args.kernel).name.replace("vmlinuz", "")
        for c in [ASSETS/f"System.map-6.12.67-0{suffix}", ASSETS/f"System.map-6.18.7-0-lts"]:
            if c.exists(): addrs, names = load_sysmap(c); break

    dtb = _resolve_dtb(args.mem_mib, args.initrd,
                            f"earlycon=pl011,0x{UART_BASE:08x} console=ttyAMA0 loglevel=8")
    sim = _helm_ng.build_simulation(isa="aarch64", mode="fs", timing="virtual",
                                     mem_mib=args.mem_mib)
    sim.load_kernel(kernel=args.kernel, dtb=str(dtb), initrd=args.initrd)
    sim.add_plugin("hotblocks")

    print(f"Running {args.max_insns:,} insns...")
    sim.run(args.max_insns)
    print()
    print(f"Top {args.top} hot basic blocks (plugin report follows):")
    print("-" * 60)
    sim.finish()   # hotblocks prints its report on finish()


if __name__ == "__main__":
    main()
