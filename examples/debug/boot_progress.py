#!/usr/bin/env python3
"""Track kernel boot progress: PC, fault state, stack pointer, and MIPS.

Usage:
    helm-system-aarch64 examples/debug/boot_progress.py
    helm-system-aarch64 examples/debug/boot_progress.py -- --max-insns 200000000
"""
import argparse, sys, time, types
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


def main():
    p = argparse.ArgumentParser(description="Kernel boot progress tracker")
    p.add_argument("--kernel",    default=str(ASSETS/"vmlinuz-rpi"))
    p.add_argument("--initrd",    default=str(ASSETS/"initramfs-rpi"))
    p.add_argument("--max-insns", type=int, default=500_000_000)
    p.add_argument("--mem-mib",   type=int, default=1024)
    p.add_argument("--core-model", default="cortex-a53")
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
    sim.add_plugin("insn-count")

    checkpoints = [100_000, 1_000_000, 5_000_000, 10_000_000, 25_000_000,
                   50_000_000, 100_000_000, 200_000_000, 500_000_000]

    print(
        f"{'Insns':>14}  {'Wall(s)':>8}  {'MIPS':>7}  {'PC':>18}  "
        f"{'EL':>2}  {'ELR_EL1':>18}  {'FAR_EL1':>18}  {'ESR_EL1':>10}  {'CSP':>18}  Note"
    )
    print("-" * 150)
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
        print(
            f"{cp:>14,}  {wall:>8.2f}  {mips:>7.1f}  "
            f"{sim.pc:#018x}  {sim.current_el:>2}  "
            f"{sim.elr_el1:#018x}  {sim.far_el1:#018x}  "
            f"{sim.esr_el1:#010x}  {sim.current_sp:#018x}  {note}"
        )
        if sim.far_el1 != 0:
            print(
                " " * 61
                + f"x0={sim.xn(0):#018x} x1={sim.xn(1):#018x} "
                + f"x2={sim.xn(2):#018x} x3={sim.xn(3):#018x} "
                + f"x29={sim.xn(29):#018x} x30={sim.xn(30):#018x}"
            )
        if r != "quantum":
            break

    sim.finish()


if __name__ == "__main__":
    main()
