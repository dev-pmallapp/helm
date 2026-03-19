#!/usr/bin/env python3
"""Measure simulation MIPS at each boot phase checkpoint.

Useful for spotting performance cliffs — where simulation slows down due
to a tight loop, heavy MMU use, or many STUB emissions.

Usage:
    helm-system-aarch64 examples/debug/benchmark.py
    helm-system-aarch64 examples/debug/benchmark.py -- --max-insns 100000000
"""
import argparse, sys, time
import argparse, sys, types
from pathlib import Path

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


def main():
    p = argparse.ArgumentParser(description="Boot phase MIPS benchmark")
    p.add_argument("--kernel",    default=str(ASSETS/"vmlinuz-rpi"))
    p.add_argument("--initrd",    default=str(ASSETS/"initramfs-rpi"))
    p.add_argument("--max-insns", type=int, default=100_000_000)
    p.add_argument("--mem-mib",   type=int, default=1024)
    args = p.parse_args()

    dtb = _resolve_dtb(args.mem_mib, args.initrd,
                            f"earlycon=pl011,0x{UART_BASE:08x} console=ttyAMA0 loglevel=8")
    sim = _helm_ng.build_simulation(isa="aarch64", mode="fs", timing="virtual",
                                     mem_mib=args.mem_mib)
    sim.load_kernel(kernel=args.kernel, dtb=str(dtb), initrd=args.initrd)

    phases = [100_000, 1_000_000, 5_000_000, 10_000_000,
              25_000_000, 50_000_000, 100_000_000]
    print(f"  {'Target':>14}  {'Wall(s)':>8}  {'Phase MIPS':>12}  {'Cumul MIPS':>12}")
    print("  " + "-" * 55)
    t0 = time.monotonic(); prev = 0; t_prev = t0
    for cp in phases:
        if cp > args.max_insns: break
        sim.run(cp - prev)
        now = time.monotonic()
        phase_mips = (cp - prev) / (now - t_prev) / 1e6 if now > t_prev else 0
        cumul_mips = cp / (now - t0) / 1e6 if now > t0 else 0
        print(f"  {cp:>14,}  {now-t0:>8.2f}  {phase_mips:>12.2f}  {cumul_mips:>12.2f}")
        prev = cp; t_prev = now


if __name__ == "__main__":
    main()
