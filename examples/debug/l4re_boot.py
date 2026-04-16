#!/usr/bin/env python3
"""Boot any L4Re pre-built image on the arm-virt platform.

Supports all L4Re images from the asset manifest: hello-2, vm-basic,
vm-multi, vm-multi-p2p, L4Linux-basic, ipcbench-el1, ipcbench-el2.

Usage:
    target/release/helm-system-aarch64 examples/debug/l4re_boot.py [OPTIONS]

Examples:
    # Boot hello-2 (default)
    ... l4re_boot.py

    # Boot vm-multi with SMP
    ... l4re_boot.py --image l4re-vm-multi --smp 2

    # Boot with EL2 virtualization
    ... l4re_boot.py --image l4re-vm-basic --boot-el 2

    # Boot vm-multi-p2p (peer-to-peer IPC between VMs)
    ... l4re_boot.py --image l4re-vm-multi-p2p --boot-el 2 --smp 2

    # Boot L4Linux guest
    ... l4re_boot.py --image l4re-l4linux-basic --boot-el 2 --max-insns 500000000

    # IPC benchmark at EL1 vs EL2
    ... l4re_boot.py --image l4re-ipcbench-el1
    ... l4re_boot.py --image l4re-ipcbench-el2 --boot-el 2

    # Boot with JIT enabled
    ... l4re_boot.py --image l4re-hello --jit

    # Compare JIT vs interpreter on L4Re boot
    ... l4re_boot.py --image l4re-hello --mode compare --max-insns 200000000
"""
import argparse
import os
import sys
import time
from pathlib import Path


sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import helmutil

helmutil.require_launcher()
_helm_ng = helmutil.import_helm_ng()
sys.stdout.reconfigure(line_buffering=True)
sys.stderr.reconfigure(line_buffering=True)

# L4Re images available via helmutil.obtain_resource
L4RE_IMAGES = {
    "l4re-hello":          "l4re_hello-2_arm_virt.elf",
    "l4re-vm-basic":       "l4re_vm-basic_arm_virt.elf",
    "l4re-vm-multi":       "l4re_vm-multi_arm_virt.elf",
    "l4re-vm-multi-p2p":   "l4re_vm-multi-p2p_arm_virt.elf",
    "l4re-l4linux-basic":  "l4re_L4Linux-basic_arm_virt.elf",
    "l4re-ipcbench-el1":   "l4re_ipcbench_arm_virt-el1.elf",
    "l4re-ipcbench-el2":   "l4re_ipcbench_arm_virt-el2.elf",
}


def _resolve_image(name: str) -> str:
    """Resolve an L4Re image name to a local path."""
    if os.path.isfile(name):
        return name
    if name in L4RE_IMAGES:
        try:
            return helmutil.obtain_resource(name, download=False).path()
        except (FileNotFoundError, Exception):
            pass
    # Try as a filename in the l4re boot directory
    try:
        res = helmutil.obtain_resource("l4re-hello", download=False)
        candidate = Path(res.path()).parent / name
        if candidate.is_file():
            return str(candidate)
    except (FileNotFoundError, Exception):
        pass
    return name


def parse_args():
    p = argparse.ArgumentParser(
        description="Boot L4Re image on arm-virt",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="Available images: " + ", ".join(sorted(L4RE_IMAGES.keys())),
    )
    p.add_argument("--image", "-i", default="l4re-hello",
                   help="L4Re image resource ID or path (default: l4re-hello)")
    p.add_argument("--mode", choices=("run", "compare", "trace"), default="run",
                   help="run=normal, compare=JIT-vs-interp, trace=JIT block log")
    p.add_argument("--max-insns", "-n", type=int, default=200_000_000,
                   help="Max instructions (default 200M)")
    p.add_argument("--smp", type=int, default=1,
                   help="Number of vCPUs (default 1)")
    p.add_argument("--mem-mib", type=int, default=512,
                   help="RAM in MiB (default 512)")
    p.add_argument("--boot-el", type=int, choices=(1, 2, 3), default=None,
                   help="Boot exception level (default: auto)")
    p.add_argument("--jit", action="store_true",
                   help="Enable JIT (run mode)")
    p.add_argument("--checkpoint-interval", type=int, default=1_000_000,
                   help="Register comparison interval for compare mode")
    p.add_argument("--plugin", action="append", default=[],
                   help="Attach plugin (name or name:args)")
    return p.parse_args()


CHUNK = 10_000_000


def _make_sim(args, kernel):
    sim = _helm_ng.build_simulation(
        isa="aarch64", mode="fs", timing="virtual",
        mem_mib=args.mem_mib,
    )
    load_kwargs = dict(kernel=kernel, smp=args.smp)
    if args.boot_el is not None:
        load_kwargs["boot_el"] = args.boot_el
    sim.load_kernel(**load_kwargs)
    return sim


def _snapshot(sim) -> dict:
    regs = ["pc", "sp", "nzcv"] + [f"x{i}" for i in range(31)]
    return {r: getattr(sim, r, None) for r in regs
            if getattr(sim, r, None) is not None}


def _diff(a: dict, b: dict) -> list[str]:
    out = []
    for k in sorted(set(a) | set(b)):
        va, vb = a.get(k), b.get(k)
        if va != vb:
            out.append(f"  {k}: interp={va:#018x}  jit={vb:#018x}" if isinstance(va, int)
                       else f"  {k}: interp={va}  jit={vb}")
    return out


def mode_run(args, kernel):
    """Normal execution."""
    sim = _make_sim(args, kernel)
    if args.jit:
        sim.set_jit(True)
    for plugin_spec in args.plugin:
        parts = plugin_spec.split(":", 1)
        sim.add_plugin(parts[0], parts[1] if len(parts) > 1 else "")

    tag = "jit" if args.jit else "interp"
    print(f"[l4re] {tag} kernel={kernel}  smp={args.smp}  max_insns={args.max_insns:,}",
          file=sys.stderr)
    t0 = time.monotonic()
    remaining = args.max_insns
    run_fn = sim.run_jit if args.jit else sim.run
    while remaining > 0 and not sim.has_exited:
        n = min(CHUNK, remaining)
        stop = run_fn(n)
        remaining -= n
        if stop != "quantum":
            break
        wall = time.monotonic() - t0
        if wall > 2.0 and (args.max_insns - remaining) % (CHUNK * 5) == 0:
            mips = sim.insn_count / wall / 1e6
            print(f"\r[l4re] {sim.insn_count/1e6:.0f}M insns  {wall:.0f}s  "
                  f"{mips:.0f} MIPS  PC={sim.pc:#x}",
                  end="", file=sys.stderr, flush=True)

    wall = time.monotonic() - t0
    mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0
    print(f"\n[l4re] done: {sim.insn_count:,} insns  {wall:.1f}s  {mips:.0f} MIPS  "
          f"PC={sim.pc:#x}", file=sys.stderr)
    sim.finish()


def mode_compare(args, kernel):
    """JIT vs interpreter lockstep comparison."""
    print(f"[l4re-cmp] kernel={kernel}  max_insns={args.max_insns:,}", file=sys.stderr)

    sim_i = _make_sim(args, kernel)
    sim_j = _make_sim(args, kernel)
    sim_j.set_jit(True)

    interval = args.checkpoint_interval
    done = 0
    t0 = time.monotonic()

    while done < args.max_insns:
        n = min(interval, args.max_insns - done)
        r_i = sim_i.run(n)
        r_j = sim_j.run_jit(n)
        done += n

        diffs = _diff(_snapshot(sim_i), _snapshot(sim_j))
        if diffs:
            wall = time.monotonic() - t0
            print(f"\n[l4re-cmp] DIVERGENCE at ~{done:,} insns ({wall:.1f}s)",
                  file=sys.stderr)
            for d in diffs:
                print(f"[l4re-cmp] {d}", file=sys.stderr)
            sim_i.finish()
            sim_j.finish()
            sys.exit(1)

        if r_i != "quantum" or r_j != "quantum":
            if r_i != r_j:
                print(f"[l4re-cmp] STOP MISMATCH: interp={r_i}  jit={r_j}",
                      file=sys.stderr)
                sys.exit(1)
            break

        wall = time.monotonic() - t0
        if wall > 2.0 and done % (interval * 10) == 0:
            mips = done / wall / 1e6
            print(f"\r[l4re-cmp] {done/1e6:.0f}M  {wall:.0f}s  {mips:.0f} MIPS",
                  end="", file=sys.stderr, flush=True)

    wall = time.monotonic() - t0
    print(f"\n[l4re-cmp] OK: {done:,} insns match ({wall:.1f}s)", file=sys.stderr)
    sim_i.finish()
    sim_j.finish()


def mode_trace(args, kernel):
    """Run with JIT and emit block-level execution log."""
    print(f"[l4re-trace] kernel={kernel}  max_insns={args.max_insns:,}", file=sys.stderr)

    sim = _make_sim(args, kernel)
    sim.set_jit(True)
    sim.add_plugin("jit-execlog", f"tail=true,max=2000")
    for plugin_spec in args.plugin:
        parts = plugin_spec.split(":", 1)
        sim.add_plugin(parts[0], parts[1] if len(parts) > 1 else "")

    t0 = time.monotonic()
    remaining = args.max_insns
    while remaining > 0 and not sim.has_exited:
        n = min(CHUNK, remaining)
        stop = sim.run_jit(n)
        remaining -= n
        if stop != "quantum":
            break

    wall = time.monotonic() - t0
    print(f"\n[l4re-trace] done: {sim.insn_count:,} insns  {wall:.1f}s  "
          f"PC={sim.pc:#x}", file=sys.stderr)
    sim.finish()


def main():
    args = parse_args()
    kernel = _resolve_image(args.image)

    if not os.path.isfile(kernel):
        print(f"[l4re] kernel not found: {kernel}", file=sys.stderr)
        print(f"[l4re] run: scripts/manage-assets.sh download {args.image}",
              file=sys.stderr)
        sys.exit(1)

    if args.mode == "run":
        mode_run(args, kernel)
    elif args.mode == "compare":
        mode_compare(args, kernel)
    elif args.mode == "trace":
        mode_trace(args, kernel)


if __name__ == "__main__":
    main()
