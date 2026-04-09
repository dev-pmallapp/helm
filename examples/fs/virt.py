#!/usr/bin/env python3
"""Boot an AArch64 Linux kernel on helm-ng's ARM virt platform.

Usage:
    helm-system-aarch64 --kernel Image [--dtb virt.dtb] [--initrd initrd] [--append "..."]
    helm-system-aarch64 examples/fs/virt.py --kernel Image ...

If --dtb is omitted a minimal arm-virt DTB is generated automatically using dtc.
--append overrides /chosen/bootargs (highest precedence).
"""
import argparse
import atexit
import os
import signal
import subprocess
import sys
import tempfile
import time
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
    argv0 = Path(sys.argv[0])
    if argv0.is_absolute():
        return argv0.parents[2]
    return (Path.cwd() / argv0).resolve().parents[2]


def _preferred_build(root: Path) -> str | None:
    try:
        exe = Path(sys.executable).resolve()
    except OSError:
        return None

    try:
        rel = exe.relative_to(root)
    except ValueError:
        return None

    parts = rel.parts
    if len(parts) >= 2 and parts[0] == "target" and parts[1] in {"debug", "release"}:
        return parts[1]
    return None


def _build_candidates(root: Path) -> tuple[str | None, list[Path]]:
    preferred = _preferred_build(root)
    builds: list[str] = []
    if preferred is not None:
        builds.append(preferred)
    for build in ("release", "debug"):
        if build not in builds:
            builds.append(build)
    return preferred, [root / "target" / build / "lib_helm_ng.so" for build in builds]


def _import_helm_ng():
    root = _root()
    try:
        import _helm_ng as module
        return module
    except ImportError:
        pass

    preferred, candidates = _build_candidates(root)
    for path in candidates:
        if path.is_file():
            import importlib.util

            build = path.parent.name
            if preferred is not None and build != preferred:
                build_cmd = "cargo build --release -p helm-python" if preferred == "release" else "cargo build -p helm-python"
                print(
                    f"[helm] warning: launcher from target/{preferred} is loading "
                    f"target/{build}/lib_helm_ng.so; rebuild the matching extension with: {build_cmd}",
                    file=sys.stderr,
                )
            spec = importlib.util.spec_from_file_location("_helm_ng", path)
            if spec and spec.loader:
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                return module

    import _helm_ng as module
    return module


_helm_ng = _import_helm_ng()

sys.stdout.reconfigure(line_buffering=True)

# ── ARM virt address map (must match helm-engine/src/platform/arm_virt.rs) ──
RAM_BASE   = 0x4000_0000
UART_BASE  = 0x0900_0000
GICD_BASE  = 0x0800_0000
GICR_BASE  = 0x080A_0000

DEFAULT_APPEND = "earlycon=pl011,0x09000000 console=ttyAMA0 loglevel=8 printk.prefer_direct=1"
ASSET_BOOT_CANDIDATES = [
    Path("assets/aarch64/boot/boot"),
    Path("assets/aarch64/boot"),
    Path("assets/aarch64/alpine/boot"),
]


def _resolve_boot_dir() -> Path:
    for path in ASSET_BOOT_CANDIDATES:
        if path.is_dir():
            return path
    return ASSET_BOOT_CANDIDATES[-1]


ASSET_BOOT = _resolve_boot_dir()


def _default_asset(env_name: str, *candidates: str) -> str | None:
    env_val = os.environ.get(env_name)
    if env_val:
        return env_val
    for candidate in candidates:
        for boot_dir in ASSET_BOOT_CANDIDATES:
            path = boot_dir / candidate
            if path.is_file():
                return str(path)
    return None


def _print_cpu_help():
    """Print available CPU models and exit."""
    print("Available CPU models:")
    print()
    for name, desc in _helm_ng.list_cpu_models():
        print(f"  {name:<16} {desc}")
    print()
    print("Usage: --cpu <model>")


def _print_machine_help():
    """Print available machine/platform types and exit."""
    print("Available machines/platforms:")
    print()
    for name, desc, isa in _helm_ng.list_platforms():
        print(f"  {name:<16} {desc} [{isa}]")


def parse_args():
    p = argparse.ArgumentParser(description="helm-ng FS — boot AArch64 Linux kernel")
    p.add_argument("--kernel", "-k",
                   default=_default_asset("HELM_KERNEL", "vmlinuz-rpi", "vmlinuz-lts"),
                   help="Path to ARM64 kernel Image (default: $HELM_KERNEL or arm-virt assets/)")
    p.add_argument("--dtb",
                   default=os.environ.get("HELM_DTB", None),
                   help="Path to DTB file (auto-generated if omitted)")
    p.add_argument("--initrd",
                   default=_default_asset("HELM_INITRD", "initramfs-rpi", "initramfs-lts"),
                   help="Path to initramfs image (optional)")
    p.add_argument("--append",
                   default=None,
                   help="Override kernel cmdline (highest precedence over DTB bootargs)")
    p.add_argument("--max-insns", "-n", type=int, default=10_000_000_000,
                   help="Max guest instructions (default 10B)")
    p.add_argument("--mem-mib", type=int, default=1024,
                   help="RAM size in MiB (default 1024)")
    p.add_argument("--smp", type=int, default=1,
                   help="Number of vCPUs / CPU nodes to expose (default 1)")
    p.add_argument("--gic-version", choices=("v2", "v3"), default="v3",
                   help="Interrupt controller model to expose")
    p.add_argument("--cpu", default="cortex-a55",
                   help="ARM core model (use --cpu help to list). Default: cortex-a55")
    p.add_argument("--machine", default="arm-virt",
                   help="Machine/platform type (use --machine help to list). Default: arm-virt")
    p.add_argument("--timing", default="virtual",
                   choices=["virtual", "interval", "accurate"],
                   help="Timing model (selects simulation accuracy)")
    p.add_argument("--interval-len", type=int, default=None,
                   help="Interval timing instruction window length")
    p.add_argument("--l1d-size", default=None,
                   help="Interval timing L1D size, e.g. 32KiB")
    p.add_argument("--l1d-assoc", type=int, default=None,
                   help="Interval timing L1D associativity")
    p.add_argument("--l1d-line", type=int, default=None,
                   help="Interval timing L1D line size in bytes")
    p.add_argument("--l2-size", default=None,
                   help="Interval timing L2 size, e.g. 256KiB")
    p.add_argument("--l2-assoc", type=int, default=None,
                   help="Interval timing L2 associativity")
    p.add_argument("--l2-line", type=int, default=None,
                   help="Interval timing L2 line size in bytes")
    p.add_argument("--tick-scale", type=int, default=1,
                   help="Virtual-time scale factor (default 1). Higher values speed up delay loops.")
    p.add_argument("--plugin", action="append", default=[],
                   help="Install a built-in plugin as NAME or NAME:arg=val,... (repeatable)")
    p.add_argument("--jit", action="store_true",
                   help="Enable HAJ (Helm Adaptive JIT: stencil baseline, dynasm hot-tier promotion, interpreter fallback)")
    return p.parse_args()


CPU_TIMING = {
    "atomic":  "virtual",
    "timing":  "interval",
    "minor":   "interval",
    "o3":      "accurate",
    "big":     "accurate",
}


def _build_timing_string(base_timing, args):
    overrides = [
        ("interval_len", args.interval_len),
        ("l1d_size", args.l1d_size),
        ("l1d_assoc", args.l1d_assoc),
        ("l1d_line", args.l1d_line),
        ("l2_size", args.l2_size),
        ("l2_assoc", args.l2_assoc),
        ("l2_line", args.l2_line),
    ]
    active = [(key, value) for key, value in overrides if value is not None]
    if base_timing != "interval":
        if active:
            raise SystemExit(
                "interval timing overrides require --timing interval "
                "(or a CPU model that maps to interval timing)"
            )
        return base_timing
    if not active:
        return base_timing
    return "interval:" + ",".join(f"{key}={value}" for key, value in active)


def _temp_file(suffix: str, content: str) -> Path:
    tmp_root = Path(os.environ.get("TMPDIR", "tmp"))
    tmp_root.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w",
        suffix=suffix,
        delete=False,
        encoding="utf-8",
        dir=tmp_root,
    ) as f:
        f.write(content)
        path = Path(f.name)
    atexit.register(lambda p=path: p.unlink(missing_ok=True))
    return path


def generate_virt_dtb(mem_mib: int, initrd_path: str | None, bootargs: str, num_cpus: int = 1) -> Path:
    """Generate a minimal arm-virt DTB via dtc. Returns path to the .dtb file."""
    initrd_props = ""
    if initrd_path and os.path.isfile(initrd_path):
        size = os.path.getsize(initrd_path)
        start = RAM_BASE + 0x0400_0000
        end   = start + size
        initrd_props = (
            f"        linux,initrd-start = <0x0 0x{start:08x}>;\n"
            f"        linux,initrd-end   = <0x0 0x{end:08x}>;\n"
        )

    cpu_nodes = []
    for cpu_idx in range(max(1, num_cpus)):
        cpu_nodes.append(
            f"""        cpu@{cpu_idx:x} {{
            device_type = "cpu";
            compatible = "arm,cortex-a53";
            reg = <0x0 0x{cpu_idx:x}>;
            enable-method = "psci";
        }};"""
        )

    # QEMU's GICv2 virt DT uses bits[15:8] as a PPI CPU mask.
    timer_irq_flags = 4 | ((1 << min(max(1, num_cpus), 8)) - 1) << 8

    dts = f"""/dts-v1/;
/ {{
    compatible = "linux,dummy-virt";
    #address-cells = <2>;
    #size-cells = <2>;
    interrupt-parent = <&gic>;

    chosen {{
        stdout-path = "/pl011@{UART_BASE:x}";
        bootargs = "{bootargs}";
{initrd_props}    }};

    aliases {{ serial0 = &uart; }};

    cpus {{
        #address-cells = <2>;
        #size-cells = <0>;
{chr(10).join(cpu_nodes)}
    }};

    memory@{RAM_BASE:x} {{
        device_type = "memory";
        reg = <0x0 0x{RAM_BASE:08x} 0x0 0x{mem_mib * 1024 * 1024:08x}>;
    }};

    psci {{
        compatible = "arm,psci-0.2";
        method = "smc";
    }};

    timer {{
        compatible = "arm,armv8-timer";
        interrupts = <1 13 0x{timer_irq_flags:x}>, <1 14 0x{timer_irq_flags:x}>,
                     <1 11 0x{timer_irq_flags:x}>, <1 10 0x{timer_irq_flags:x}>;
        always-on;
    }};

    apb_pclk: apb-pclk {{
        compatible = "fixed-clock";
        #clock-cells = <0>;
        clock-frequency = <24000000>;
        clock-output-names = "clk24mhz";
    }};

    gic: interrupt-controller@{GICD_BASE:x} {{
        compatible = "arm,gic-v3";
        #interrupt-cells = <3>;
        interrupt-controller;
        #address-cells = <2>;
        #size-cells = <2>;
        ranges;
        reg = <0x0 0x{GICD_BASE:08x} 0x0 0x10000>,
              <0x0 0x{GICR_BASE:08x} 0x0 0x{max(1, num_cpus) * 0x20000:08x}>;
    }};

    uart: pl011@{UART_BASE:x} {{
        compatible = "arm,pl011", "arm,primecell";
        reg = <0x0 0x{UART_BASE:08x} 0x0 0x1000>;
        interrupts = <0 1 4>;
        clocks = <&apb_pclk>, <&apb_pclk>;
        clock-names = "uartclk", "apb_pclk";
    }};
}};
"""
    dts_path = _temp_file(".dts", dts)
    dtb_path = dts_path.with_suffix(".dtb")
    atexit.register(lambda p=dtb_path: p.unlink(missing_ok=True))
    try:
        subprocess.run(
            ["dtc", "-I", "dts", "-O", "dtb", "-o", str(dtb_path), str(dts_path)],
            check=True, capture_output=True,
        )
    except FileNotFoundError:
        print("[fs] 'dtc' not found — install device-tree-compiler or pass --dtb", file=sys.stderr)
        sys.exit(1)
    except subprocess.CalledProcessError as e:
        print(f"[fs] dtc failed: {e.stderr.decode()}", file=sys.stderr)
        sys.exit(1)
    return dtb_path


def _stat_line(name: str, value, comment: str = "") -> str:
    comment_part = "" if not comment else f"  # {comment}"
    return f"{name:<40}{value:>20}{comment_part}"


def _print_sim_stats(sim, wall: float, stream=sys.stderr) -> None:
    stats = sim.stats()
    insns = int(stats.get("insn_count", sim.insn_count))
    ticks = int(stats.get("tick_count", stats.get("virtual_cycles", 0)))
    freq = int(stats.get("sim_freq", 1_000_000_000))
    ipc = float(stats.get("ipc", (insns / ticks) if ticks else 0.0))
    sim_seconds = ticks / freq if freq > 0 else 0.0
    host_mips = insns / wall / 1e6 if wall > 0.001 else 0.0

    print("---------- Begin Simulation Statistics ----------", file=stream)
    print(_stat_line("sim_insns", insns, "Instructions retired"), file=stream)
    print(_stat_line("sim_ticks", ticks, "Simulated cycles/ticks"), file=stream)
    print(_stat_line("sim_seconds", f"{sim_seconds:.6f}", "Simulated time"), file=stream)
    print(_stat_line("system.cpu.committedInsts", insns, "Committed instructions"), file=stream)
    print(_stat_line("system.cpu.ipc", f"{ipc:.6f}", "Instructions per tick"), file=stream)
    print(_stat_line("host_seconds", f"{wall:.6f}", "Wall clock runtime"), file=stream)
    print(_stat_line("host_mips", f"{host_mips:.3f}", "Million insts/sec"), file=stream)
    print(_stat_line("system.final_pc", f"{sim.pc:#018x}", "Final PC"), file=stream)
    print("----------  End Simulation Statistics  ----------", file=stream)


class _SigintFlag:
    def __init__(self) -> None:
        self.triggered = False

    def handler(self, _signum, _frame) -> None:
        self.triggered = True


def main():
    args = parse_args()

    # Handle help requests for --cpu and --machine
    if args.cpu in ("help", "?", "list"):
        _print_cpu_help()
        return
    if args.machine in ("help", "?", "list"):
        _print_machine_help()
        return

    if not os.path.isfile(args.kernel):
        print(f"[fs] kernel not found: {args.kernel}", file=sys.stderr)
        sys.exit(1)

    if args.dtb and not os.path.isfile(args.dtb):
        print(f"[fs] DTB not found: {args.dtb}", file=sys.stderr)
        sys.exit(1)

    # When --append is given it will override DTB bootargs in Rust.
    # When no DTB is given, bake --append (or default) into the generated DTB.
    dtb_path = args.dtb
    if not dtb_path:
        baked_cmdline = args.append or DEFAULT_APPEND
        dtb_path = str(generate_virt_dtb(args.mem_mib, args.initrd, baked_cmdline, args.smp))
        # Bootargs already baked in; don't double-apply via Rust patcher.
        append_override = None
        dtb_bytes = Path(dtb_path).read_bytes()
        dtb_arg = None
    else:
        append_override = args.append or None
        dtb_bytes = None
        dtb_arg = dtb_path

    timing = _build_timing_string(args.timing, args)
    print(f"[fs] kernel={args.kernel}  dtb={dtb_path}  "
          f"initrd={args.initrd or '(none)'}  cpu={args.cpu}  timing={timing}  smp={args.smp}")

    sim = _helm_ng.build_simulation(
        isa="aarch64",
        mode="fs",
        timing=timing,
        mem_mib=args.mem_mib,
    )

    # Apply ARM core model (ID registers, MIDR, feature bits)
    sim.load_kernel(
        kernel=args.kernel,
        dtb=dtb_arg,
        dtb_bytes=dtb_bytes,
        initrd=args.initrd or None,
        append=append_override,
        num_cpus=args.smp,
        gic_version=args.gic_version,
    )

    # Apply ARM core model AFTER load_kernel so a64_state exists.
    try:
        sim.set_cpu_model(args.cpu)
        print(f"[fs] cpu={args.cpu}")
    except Exception as e:
        print(f"[fs] Warning: could not set cpu model '{args.cpu}': {e}", file=sys.stderr)

    if args.tick_scale > 1:
        sim.set_tick_scale(args.tick_scale)
        print(f"[fs] tick-scale={args.tick_scale}")

    for spec in args.plugin:
        name, plugin_args = spec.split(":", 1) if ":" in spec else (spec, "")
        sim.add_plugin(name, plugin_args)
        if plugin_args:
            print(f"[fs] plugin={name} args={plugin_args}")
        else:
            print(f"[fs] plugin={name}")

    if args.jit:
        sim.set_jit(True)
        print("[fs] jit=on")

    t0 = time.monotonic()
    chunk = 10_000_000
    remaining = args.max_insns
    stop_reason = "quantum"
    wall = 0.0
    last_progress = 0.0  # wall-clock time of last progress print
    interrupted = False
    sigint = _SigintFlag()
    old_sigint = signal.getsignal(signal.SIGINT)
    signal.signal(signal.SIGINT, sigint.handler)

    try:
        while remaining > 0:
            if sigint.triggered:
                interrupted = True
                stop_reason = "interrupt"
                print(f"\n[fs] interrupted after {sim.insn_count:,} instructions", file=sys.stderr)
                break
            n = min(chunk, remaining)
            stop_reason = sim.run_jit(n) if args.jit else sim.run(n)
            remaining -= n
            wall = time.monotonic() - t0
            if stop_reason != "quantum":
                break
            if wall > 2.0 and wall - last_progress >= 5.0:
                last_progress = wall
                mips = sim.insn_count / wall / 1e6
                print(f"\r[fs] {sim.insn_count/1e6:.0f}M insns  {wall:.0f}s  {mips:.0f} MIPS  PC={sim.pc:#x}",
                      end="", file=sys.stderr, flush=True)
    except KeyboardInterrupt:
        interrupted = True
        stop_reason = "interrupt"
        wall = time.monotonic() - t0
        if wall > 2.0:
            print(file=sys.stderr)
        print("\n[fs] interrupted by Ctrl+C", file=sys.stderr)
    finally:
        signal.signal(signal.SIGINT, old_sigint)
        wall = time.monotonic() - t0
        sim.finish()
        _print_sim_stats(sim, wall, stream=sys.stderr)

    if wall > 2.0 and not interrupted:
        print(file=sys.stderr)

    wall = time.monotonic() - t0
    mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0

    if stop_reason == "exit":
        print(f"[fs] exited with code {sim.exit_code}")
    elif stop_reason == "interrupt":
        print(f"[fs] interrupted at PC={sim.pc:#x}", file=sys.stderr)
    elif stop_reason != "quantum":
        print(f"[fs] stopped: {stop_reason} at PC={sim.pc:#x}", file=sys.stderr)
    else:
        print(f"[fs] hit instruction limit at PC={sim.pc:#x}")

    print(f"[fs] {sim.insn_count:,} insns  {wall:.2f}s  {mips:.0f} MIPS")

    if stop_reason == "exit":
        sys.exit(sim.exit_code)
    if stop_reason == "interrupt":
        sys.exit(130)
    if stop_reason != "quantum":
        sys.exit(1)


if __name__ == "__main__":
    main()
