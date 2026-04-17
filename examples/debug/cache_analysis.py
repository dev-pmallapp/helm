#!/usr/bin/env python3
"""Analyze cache and branch prediction behavior using HelmSpy.

Real-world scenario: Performance debugging across the three timing
models (virtual/interval/accurate) requires understanding the
microarchitectural behavior of workloads.  During design-issues analysis
(commits 1fdb0cb through 1e08535), cache miss rates and branch
misprediction rates were key metrics for identifying hot-path
optimization opportunities.

HelmSpy (the observe() API) provides a non-intrusive observation layer
that models L1D cache and branch predictor behavior without affecting
simulation speed significantly.

This example demonstrates:
  - HelmSpy observe() with L1D cache model (configurable size/ways/line)
  - Branch prediction analysis (bimodal or gshare predictor)
  - Instruction mix histogram (IntAlu, Load, Store, Branch, FP, SIMD)
  - Hot-PC profiling (most executed instruction addresses)
  - Branch heatmap (most taken/not-taken branch sites)
  - Phase-based analysis (metrics at regular intervals)

Usage:
    helm-aarch64 examples/debug/cache_analysis.py --binary ./my_elf
    helm-aarch64 examples/debug/cache_analysis.py --binary ./my_elf \\
        --l1d-size 32768 --l1d-ways 8 --predictor gshare
    helm-system-aarch64 examples/debug/cache_analysis.py \\
        --mode fs --max-insns 50000000
"""
import argparse
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import helmutil

helmutil.require_launcher()
_helm_ng = helmutil.import_helm_ng()
sys.stdout.reconfigure(line_buffering=True)


def parse_args():
    p = argparse.ArgumentParser(
        description="Cache and branch prediction analysis via HelmSpy"
    )
    p.add_argument("--binary", "-b", default=helmutil.default_binary())
    p.add_argument("--max-insns", "-n", type=int, default=10_000_000,
                   help="Max instructions (default 10M)")
    p.add_argument("--mode", choices=["se", "fs"], default="se",
                   help="Execution mode (default: se)")
    p.add_argument("--l1d-size", type=int, default=32768,
                   help="L1D cache size in bytes (default 32768)")
    p.add_argument("--l1d-ways", type=int, default=8,
                   help="L1D associativity (default 8)")
    p.add_argument("--l1d-line", type=int, default=64,
                   help="L1D line size in bytes (default 64)")
    p.add_argument("--predictor", choices=["bimodal", "gshare"],
                   default="gshare",
                   help="Branch predictor type (default: gshare)")
    p.add_argument("--predictor-bits", type=int, default=10,
                   help="Predictor table index bits (default 10)")
    p.add_argument("--top-pcs", type=int, default=15,
                   help="Number of hot PCs to show (default 15)")
    p.add_argument("--top-branches", type=int, default=15,
                   help="Number of hot branch sites to show (default 15)")
    p.add_argument("--phases", type=int, default=5,
                   help="Number of phase checkpoints (default 5)")
    p.add_argument("--argv", nargs="*", default=None)
    # FS mode options
    p.add_argument("--kernel", default=None)
    p.add_argument("--initrd", default=None)
    return p.parse_args()


def _fmt_pct(value):
    if value is None:
        return "N/A"
    return f"{value*100:.2f}%"


def _fmt_count(value):
    if value is None:
        return "N/A"
    return f"{value:,}"


_sym_cache = None

def _build_sym_cache(sim):
    """Fetch symbol table once and sort for bisect lookup."""
    global _sym_cache
    if _sym_cache is not None:
        return _sym_cache
    import bisect
    try:
        syms = sim.symbols()
        addrs = [a for _, a, _ in syms]
        names = [n for n, _, _ in syms]
        _sym_cache = (addrs, names)
    except Exception:
        _sym_cache = ([], [])
    return _sym_cache


def _resolve_symbol(sim, addr):
    """Resolve address to symbol name via bisect lookup."""
    import bisect
    addrs, names = _build_sym_cache(sim)
    if not addrs:
        return None
    i = bisect.bisect_right(addrs, addr) - 1
    if i < 0:
        return None
    off = addr - addrs[i]
    if off > 0x10000:
        return None
    return f"{names[i]}+{off:#x}" if off else names[i]


def main():
    args = parse_args()

    if args.mode == "se":
        binary = args.binary
        if not os.path.isfile(binary):
            print(f"[cache] binary not found: {binary}", file=sys.stderr)
            sys.exit(1)
        argv = args.argv or [os.path.basename(binary), "-c", "echo hello"]
        envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C",
                "USER=helm"]
        sim = _helm_ng.build_simulation(isa="aarch64", mode="se",
                                         timing="virtual")
        sim.load_elf(binary, argv, envp)
        print(f"[cache] SE mode: binary={binary}")
    else:
        boot = helmutil.load_boot_module()
        assets = helmutil.resolve_assets_dir()
        kernel = args.kernel or helmutil.default_kernel()
        initrd = args.initrd or helmutil.default_initrd()
        if not kernel:
            print("[cache] No kernel found; use --kernel", file=sys.stderr)
            sys.exit(1)
        dtb = boot._resolve_dtb_path(None, 1024, initrd,
                                      f"earlycon=pl011,0x{helmutil.UART_BASE:08x} "
                                      f"console=ttyAMA0 loglevel=4")
        sim = _helm_ng.build_simulation(isa="aarch64", mode="fs",
                                         timing="virtual", mem_mib=1024)
        sim.load_kernel(kernel=str(kernel), dtb=str(dtb),
                        initrd=str(initrd) if initrd else None)
        print(f"[cache] FS mode: kernel={kernel}")

    # Create HelmSpy observation session
    spy = sim.observe(
        cache_l1d_size=args.l1d_size,
        cache_l1d_ways=args.l1d_ways,
        cache_l1d_line=args.l1d_line,
        predictor=args.predictor,
        predictor_bits=args.predictor_bits,
    )

    print(f"[cache] L1D: {args.l1d_size//1024}KB, {args.l1d_ways}-way, "
          f"{args.l1d_line}B line")
    print(f"[cache] Predictor: {args.predictor} "
          f"({1 << args.predictor_bits} entries)")
    print(f"[cache] Running {args.max_insns:,} instructions...\n")

    # Phase-based analysis
    phase_size = args.max_insns // args.phases
    t0 = time.monotonic()

    print(f"{'Phase':>6s}  {'Insns':>12s}  {'CacheHit%':>10s}  "
          f"{'BrMPKI':>8s}  {'MIPS':>6s}")
    print("-" * 55)

    for phase in range(args.phases):
        n = phase_size
        if phase == args.phases - 1:
            n = args.max_insns - phase * phase_size

        stop = sim.run(n)

        wall = time.monotonic() - t0
        mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0
        hit_rate = spy.cache_hit_rate
        mpki = spy.branch_mpki()

        mpki_str = f"{mpki:.3f}" if mpki is not None else "N/A"
        print(f"{phase+1:>6d}  {sim.insn_count:>12,}  "
              f"{_fmt_pct(hit_rate):>10s}  "
              f"{mpki_str:>8s}  "
              f"{mips:>6.0f}")

        if stop != "quantum":
            print(f"  (stopped: {stop})")
            break
        if sim.has_exited:
            break

    wall = time.monotonic() - t0

    # Final analysis report
    print(f"\n{'='*60}")
    print("CACHE ANALYSIS REPORT")
    print("=" * 60)

    print(f"\nOverall:")
    print(f"  Instructions:    {sim.insn_count:>15,}")
    print(f"  L1D Hit Rate:    {_fmt_pct(spy.cache_hit_rate):>15s}")
    print(f"  L1D Hits:        {_fmt_count(spy.cache_hits):>15s}")
    print(f"  L1D Misses:      {_fmt_count(spy.cache_misses):>15s}")
    print(f"  Branch Miss Rate:{_fmt_pct(spy.branch_miss_rate):>15s}")
    mpki = spy.branch_mpki()
    mpki_str = f"{mpki:.3f}" if mpki is not None else "N/A"
    print(f"  Branch MPKI:     {mpki_str:>15s}")
    print(f"  Wall Time:       {wall:>14.2f}s")

    # Instruction mix
    print(f"\nInstruction Mix:")
    insn_mix = spy.insn_mix()
    if insn_mix:
        for cls, count, frac in insn_mix:
            bar = "#" * int(frac * 40)
            print(f"  {cls:<12s} {count:>12,}  ({frac*100:5.1f}%) {bar}")

    # Hot PCs
    print(f"\nTop {args.top_pcs} Hot PCs:")
    hot_pcs = spy.hot_pcs(args.top_pcs)
    if hot_pcs:
        for pc, count in hot_pcs:
            sym = _resolve_symbol(sim, pc) or ""
            print(f"  {pc:#018x}  {count:>10,}  {sym}")

    # Branch heatmap
    print(f"\nTop {args.top_branches} Branch Sites:")
    branches = spy.branch_heatmap(args.top_branches)
    if branches:
        for pc, count in branches:
            sym = _resolve_symbol(sim, pc) or ""
            print(f"  {pc:#018x}  {count:>10,}  {sym}")

    # Full snapshot
    print(f"\nFull Spy Snapshot:")
    snap = spy.snapshot()
    for key, val in sorted(snap.items()):
        print(f"  {key}: {val}")

    spy.detach()
    sim.finish()


if __name__ == "__main__":
    main()
