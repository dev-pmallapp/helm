#!/usr/bin/env python3
"""Track kernel boot progress: PC, EL, IRQ count and MIPS at each checkpoint.

Usage:
    helm-system-aarch64 examples/debug/boot_progress.py
    helm-system-aarch64 examples/debug/boot_progress.py -- --max-insns 200000000
"""
import argparse, sys, time
sys.path.insert(0, str(__import__('pathlib').Path(__file__).resolve().parents[2] / 'python'))
from examples.fs.boot_rpi_full import _import_helm_ng, _resolve_assets_dir, _resolve_dtb_path

_helm_ng = _import_helm_ng()
ASSETS   = _resolve_assets_dir()
UART_BASE = 0x0900_0000

sys.stdout.reconfigure(line_buffering=True)


def main():
    p = argparse.ArgumentParser(description="Kernel boot progress tracker")
    p.add_argument("--kernel",    default=str(ASSETS/"vmlinuz-rpi"))
    p.add_argument("--initrd",    default=str(ASSETS/"initramfs-rpi"))
    p.add_argument("--max-insns", type=int, default=500_000_000)
    p.add_argument("--mem-mib",   type=int, default=1024)
    args = p.parse_args()

    dtb = _resolve_dtb_path(None, args.mem_mib, args.initrd,
                            f"earlycon=pl011,0x{UART_BASE:08x} console=ttyAMA0 loglevel=8")
    sim = _helm_ng.build_simulation(isa="aarch64", mode="fs", timing="virtual",
                                     mem_mib=args.mem_mib)
    sim.load_kernel(kernel=args.kernel, dtb=str(dtb), initrd=args.initrd)
    sim.add_plugin("insn-count")

    checkpoints = [100_000, 1_000_000, 5_000_000, 10_000_000, 25_000_000,
                   50_000_000, 100_000_000, 200_000_000, 500_000_000]

    print(f"{'Insns':>14}  {'Wall(s)':>8}  {'MIPS':>7}  {'PC':>18}  Note")
    print("-" * 75)
    t0 = time.monotonic()
    prev = 0
    for cp in checkpoints:
        if cp > args.max_insns:
            break
        r = sim.run(cp - prev)
        prev = cp
        wall = time.monotonic() - t0
        mips = cp / wall / 1e6 if wall > 0 else 0
        # Heuristic: kernel VA space starts above 0xffff_0000_0000_0000
        in_kva = sim.pc > 0xffff_0000_0000_0000
        note = "kernel-VA" if in_kva else "physical"
        if r != "quantum":
            note += f" ({r})"
        print(f"{cp:>14,}  {wall:>8.2f}  {mips:>7.1f}  {sim.pc:#018x}  {note}")
        if r != "quantum":
            break

    sim.finish()


if __name__ == "__main__":
    main()
