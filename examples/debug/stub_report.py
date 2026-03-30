#!/usr/bin/env python3
"""Report unimplemented instruction sites encountered during boot.

The stub-tracer plugin records each unique unimplemented instruction and
prints a summary on finish().  Use this to find gaps in the execute.rs
implementation that are exercised by a real kernel workload.

Usage:
    helm-system-aarch64 examples/debug/stub_report.py
    helm-system-aarch64 examples/debug/stub_report.py -- --max-insns 50000000
"""
import argparse, sys
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


def main():
    p = argparse.ArgumentParser(description="Unimplemented-instruction report")
    p.add_argument("--kernel",    default=str(ASSETS/"vmlinuz-rpi"))
    p.add_argument("--initrd",    default=str(ASSETS/"initramfs-rpi"))
    p.add_argument("--max-insns", type=int, default=20_000_000)
    p.add_argument("--mem-mib",   type=int, default=1024)
    args = p.parse_args()

    dtb = _resolve_dtb(args.mem_mib, args.initrd,
                            f"earlycon=pl011,0x{UART_BASE:08x} console=ttyAMA0 loglevel=8")
    sim = _helm_ng.build_simulation(isa="aarch64", mode="fs", timing="virtual",
                                     mem_mib=args.mem_mib)
    sim.load_kernel(kernel=args.kernel, dtb=str(dtb), initrd=args.initrd)
    sim.add_plugin("stub-tracer")

    print(f"Running {args.max_insns:,} insns with stub-tracer...")
    sim.run(args.max_insns)

    if sim.has_unimplemented_instructions:
        print(f"\n{sim.unimplemented_instruction_count} unique unimplemented sites encountered.")
    else:
        print("\nNo unimplemented instructions detected in this run.")
    print()
    print("Stub report:")
    print("-" * 60)
    sim.finish()


if __name__ == "__main__":
    main()
