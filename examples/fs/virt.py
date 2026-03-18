#!/usr/bin/env python3
"""Boot an AArch64 Linux kernel on helm-ng's ARM virt platform.

Usage:
    helm-system-aarch64 examples/fs/virt.py --kernel Image --dtb virt.dtb
    helm-system-aarch64 --kernel Image --dtb virt.dtb   # embedded mode
"""
import argparse
import os
import sys
import time

import _helm_ng

sys.stdout.reconfigure(line_buffering=True)


def parse_args():
    p = argparse.ArgumentParser(description="helm-ng FS — boot AArch64 Linux kernel")
    p.add_argument("--kernel", "-k",
                   default=os.environ.get("HELM_KERNEL", "assets/aarch64/alpine/boot/vmlinuz-rpi"),
                   help="Path to ARM64 kernel Image (default: $HELM_KERNEL or assets/)")
    p.add_argument("--dtb",
                   default=os.environ.get("HELM_DTB", None),
                   help="Path to DTB file (required unless $HELM_DTB is set)")
    p.add_argument("--initrd",
                   default=os.environ.get("HELM_INITRD", None),
                   help="Path to initramfs image (optional)")
    p.add_argument("--append",
                   default=None,
                   help="Override kernel cmdline (highest precedence over DTB bootargs)")
    p.add_argument("--max-insns", "-n", type=int, default=10_000_000_000,
                   help="Max guest instructions (default 10B)")
    p.add_argument("--mem-mib", type=int, default=1024,
                   help="RAM size in MiB (default 1024)")
    p.add_argument("--cpu", default="atomic",
                   choices=["atomic", "timing", "minor", "o3", "big"],
                   help="CPU model (selects timing model)")
    return p.parse_args()


CPU_TIMING = {
    "atomic":  "virtual",
    "timing":  "interval",
    "minor":   "interval",
    "o3":      "accurate",
    "big":     "accurate",
}


def main():
    args = parse_args()

    if not os.path.isfile(args.kernel):
        print(f"[fs] kernel not found: {args.kernel}", file=sys.stderr)
        sys.exit(1)

    if args.dtb and not os.path.isfile(args.dtb):
        print(f"[fs] DTB not found: {args.dtb}", file=sys.stderr)
        sys.exit(1)

    timing = CPU_TIMING.get(args.cpu, "virtual")
    print(f"[fs] kernel={args.kernel}  dtb={args.dtb or '(none)'}  initrd={args.initrd or '(none)'}  cpu={args.cpu}  timing={timing}")

    sim = _helm_ng.build_simulation(
        isa="aarch64",
        mode="fs",
        timing=timing,
        mem_mib=args.mem_mib,
    )

    sim.load_kernel(
        kernel=args.kernel,
        dtb=args.dtb or "",
        initrd=args.initrd or None,
        append=args.append or None,
    )

    t0 = time.monotonic()
    chunk = 10_000_000
    remaining = args.max_insns
    stop_reason = "quantum"
    wall = 0.0

    while remaining > 0:
        n = min(chunk, remaining)
        stop_reason = sim.run(n)
        remaining -= n
        wall = time.monotonic() - t0
        if stop_reason != "quantum":
            break
        if wall > 2.0:
            mips = sim.insn_count / wall / 1e6
            print(f"\r[fs] {sim.insn_count/1e6:.0f}M insns  {wall:.0f}s  {mips:.0f} MIPS",
                  end="", file=sys.stderr, flush=True)

    if wall > 2.0:
        print(file=sys.stderr)

    wall = time.monotonic() - t0
    mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0

    if stop_reason == "exit":
        print(f"[fs] exited with code {sim.exit_code}")
    elif stop_reason != "quantum":
        print(f"[fs] stopped: {stop_reason} at PC={sim.pc:#x}", file=sys.stderr)
    else:
        print(f"[fs] hit instruction limit at PC={sim.pc:#x}")

    print(f"[fs] {sim.insn_count:,} insns  {wall:.2f}s  {mips:.0f} MIPS")

    sim.finish()

    if stop_reason == "exit":
        sys.exit(sim.exit_code)
    if stop_reason != "quantum":
        sys.exit(1)


if __name__ == "__main__":
    main()
