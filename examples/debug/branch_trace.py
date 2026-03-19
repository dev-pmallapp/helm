#!/usr/bin/env python3
"""Branch trace with System.map symbol resolution.

Routes sim-trace BRNC events (emitted by step_aarch64_system whenever
the PC changes non-linearly) to a temp file, then resolves each address
to a kernel symbol and prints the call-flow.

Most useful for diagnosing hangs: run with --from-insns set to just
before the suspected hang and read the last N branches.

Usage:
    helm-system-aarch64 examples/debug/branch_trace.py
    helm-system-aarch64 examples/debug/branch_trace.py -- --from-insns 12000000 --trace-insns 2000000
"""
import argparse, atexit, bisect, collections, os, sys, tempfile
from pathlib import Path

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


def _load_sysmap(path: Path):
    addrs, names = [], []
    with open(path) as f:
        for line in f:
            p = line.split()
            if len(p) >= 3:
                try: addrs.append(int(p[0], 16)); names.append(p[2])
                except ValueError: pass
    return addrs, names


def _sym(va, addrs, names):
    i = bisect.bisect_right(addrs, va) - 1
    if i < 0: return f"<{va:#018x}>"
    off = va - addrs[i]; return f"{names[i]}+{off:#x}" if off else names[i]


def _funcname(va, addrs, names):
    i = bisect.bisect_right(addrs, va) - 1
    return names[i] if i >= 0 else f"?{va:#x}"


def _parse_brnc(path: Path):
    out = []
    with open(path) as f:
        for line in f:
            if "[BRNC]" not in line: continue
            try:
                insns = int(next(p for p in line.split() if p.startswith("insns="))[6:])
                pc    = int(next(p for p in line.split() if p.startswith("pc="))[3:], 16)
                dst   = int(line[line.index("-> ")+3:].split()[0], 16)
                out.append((insns, pc, dst))
            except (StopIteration, ValueError, IndexError): pass
    return out


def main():
    p = argparse.ArgumentParser(description="Branch trace with symbol resolution")
    p.add_argument("--kernel",       default=str(ASSETS/"vmlinuz-rpi"))
    p.add_argument("--initrd",       default=str(ASSETS/"initramfs-rpi"))
    p.add_argument("--sysmap",       default=None)
    p.add_argument("--mem-mib",      type=int, default=1024)
    p.add_argument("--from-insns",   type=int, default=12_000_000,
                   help="Fast-forward without tracing (default 12M)")
    p.add_argument("--trace-insns",  type=int, default=3_000_000,
                   help="Insns to collect branches (default 3M)")
    p.add_argument("--show",         type=int, default=100,
                   help="Branch entries to print (default 100)")
    args = p.parse_args()

    # Auto-detect System.map
    sysmap_path = args.sysmap
    if not sysmap_path:
        suffix = Path(args.kernel).name.replace("vmlinuz", "")
        for c in [Path(args.kernel).parent/f"System.map{suffix}",
                  ASSETS/f"System.map-6.12.67-0{suffix}",
                  ASSETS/f"System.map-6.18.7-0-lts"]:
            if c.exists(): sysmap_path = str(c); break
    if not sysmap_path:
        print("System.map not found; pass --sysmap PATH", file=sys.stderr); sys.exit(1)

    addrs, names = _load_sysmap(Path(sysmap_path))
    print(f"System.map: {sysmap_path} ({len(addrs)} symbols)", file=sys.stderr)

    # BRNC events come through the sim-trace channel (emitted as eprintln!
    # when no MonitorSink is installed).  Redirect fd-2 to a temp file.
    fd, trace_path = tempfile.mkstemp(suffix=".brnc", prefix="helm-")
    os.close(fd)
    atexit.register(lambda: Path(trace_path).unlink(missing_ok=True))
    _old2 = os.dup(2); _tf = open(trace_path, "w", buffering=1)
    os.dup2(_tf.fileno(), 2)
    sys.stderr = _tf

    dtb = _resolve_dtb(args.mem_mib, args.initrd,
                            f"earlycon=pl011,0x{UART_BASE:08x} console=ttyAMA0 loglevel=8")
    sim = _helm_ng.build_simulation(isa="aarch64", mode="fs", timing="virtual",
                                     mem_mib=args.mem_mib)
    sim.load_kernel(kernel=args.kernel, dtb=str(dtb), initrd=args.initrd)

    sim.run(args.from_insns)
    sim.run(args.trace_insns)

    # Restore stderr before printing analysis
    os.dup2(_old2, 2); os.close(_old2); _tf.flush(); _tf.close(); sys.stderr = sys.__stderr__

    branches = _parse_brnc(Path(trace_path))
    window   = branches[-args.show:]
    print(f"\n=== {len(branches)} branches captured; last {len(window)} shown ===\n")
    fn_counts = collections.Counter()
    for insns, src, dst in window:
        fn_counts[_funcname(dst, addrs, names)] += 1
        loop = " <-- LOOP" if _funcname(src,addrs,names)==_funcname(dst,addrs,names) else ""
        print(f"  [{insns:>12}]  {_sym(src,addrs,names):<50}  ->  {_sym(dst,addrs,names)}{loop}")
    print(f"\nTop functions by branch-target count:")
    for fn, cnt in fn_counts.most_common(15):
        print(f"  {cnt:5}x  {fn}")
    print(f"\nFinal  pc={sim.pc:#018x}  {_sym(sim.pc,addrs,names)}")
    print(f"       lr={sim.xn(30):#018x}  {_sym(sim.xn(30),addrs,names)}")


if __name__ == "__main__":
    main()
