#!/usr/bin/env python3
"""Boot an AArch64 Linux kernel on helm-ng's ARM virt platform.

Usage:
    helm-system-aarch64 examples/fs/boot_rpi_full.py [--kernel PATH] [--dtb PATH] [--initrd PATH] [--max-insns N]

If ``--dtb`` is omitted, the script generates a minimal ARM virt DTB that
matches the devices currently implemented in helm-ng. If ``--dtb`` is
provided, it is used as-is and no temporary DTS/DTB is created.
"""
import argparse
import atexit
import importlib.util
import os
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Optional

def _require_helm_launcher() -> None:
    if getattr(sys, "_helm_launcher", None) not in {"helm-aarch64", "helm-system-aarch64"}:
        raise SystemExit(
            "This example must be run via helm-aarch64 or helm-system-aarch64, not directly via python."
        )

_require_helm_launcher()

RAM_BASE = 0x4000_0000
INITRD_OFFSET = 0x0400_0000
UART_BASE = 0x0900_0000
GICD_BASE = 0x0800_0000
GICR_BASE = 0x080A_0000


def _script_path() -> Path:
    if "__file__" in globals():
        return Path(__file__).resolve()
    argv0 = Path(sys.argv[0])
    if argv0.is_absolute():
        return argv0
    return (Path.cwd() / argv0).resolve()


ROOT = _script_path().parents[2]
sys.path.insert(0, str(ROOT / "python"))


def _import_helm_ng():
    try:
        import _helm_ng as module
        return module
    except ImportError:
        pass

    candidates = [
        ROOT / "target" / "debug" / "lib_helm_ng.so",
        ROOT / "target" / "release" / "lib_helm_ng.so",
    ]
    for path in candidates:
        if path.is_file():
            spec = importlib.util.spec_from_file_location("_helm_ng", path)
            if spec and spec.loader:
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                return module

    print("Error: _helm_ng module not found. Build with: cargo build -p helm-python", file=sys.stderr)
    sys.exit(1)


_helm_ng = _import_helm_ng()


def _resolve_assets_dir() -> Path:
    candidates = [
        ROOT / "assets" / "aarch64" / "boot" / "boot",
        ROOT / "assets" / "aarch64" / "boot",
        ROOT / "assets" / "aarch64" / "alpine" / "boot",
    ]
    for path in candidates:
        if path.is_dir():
            return path
    return candidates[-1]


ASSETS = _resolve_assets_dir()


def _default_boot_asset(*candidates: str) -> str:
    for candidate in candidates:
        path = ASSETS / candidate
        if path.is_file():
            return str(path)
    return str(ASSETS / candidates[0])


def _write_temp_file(suffix: str, data: str) -> Path:
    tmp_root = Path(os.environ.get("TMPDIR", ROOT / "tmp"))
    tmp_root.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w",
        suffix=suffix,
        delete=False,
        encoding="utf-8",
        dir=tmp_root,
    ) as f:
        f.write(data)
        path = Path(f.name)
    atexit.register(lambda p=path: p.unlink(missing_ok=True))
    return path


def _generate_arm_virt_dtb(mem_mib: int, initrd_path: Optional[str], append: str, num_cpus: int = 1) -> Path:
    initrd_props = ""
    if initrd_path:
        initrd_size = Path(initrd_path).stat().st_size
        initrd_start = RAM_BASE + INITRD_OFFSET
        initrd_end = initrd_start + initrd_size
        initrd_props = (
            f"        linux,initrd-start = <0x0 0x{initrd_start:08x}>;\n"
            f"        linux,initrd-end = <0x0 0x{initrd_end:08x}>;\n"
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
        bootargs = "{append}";
{initrd_props}    }};

    aliases {{
        serial0 = &uart;
    }};

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
    dts_path = _write_temp_file(".dts", dts)
    dtb_path = dts_path.with_suffix(".dtb")
    atexit.register(lambda p=dtb_path: p.unlink(missing_ok=True))
    subprocess.run(
        ["dtc", "-I", "dts", "-O", "dtb", "-o", str(dtb_path), str(dts_path)],
        check=True,
    )
    return dtb_path


def _resolve_dtb_path(explicit_dtb: Optional[str], mem_mib: int, initrd_path: Optional[str], append: str, num_cpus: int = 1) -> Path:
    """Use caller-provided DTB when present; otherwise synthesize one for arm-virt."""
    if explicit_dtb:
        return Path(explicit_dtb)
    return _generate_arm_virt_dtb(mem_mib, initrd_path, append, num_cpus)


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
    parser = argparse.ArgumentParser(description="Boot AArch64 Linux kernel")
    parser.add_argument("--kernel", default=_default_boot_asset("vmlinuz-rpi", "vmlinuz-lts"),
                        help="Path to ARM64 kernel Image")
    parser.add_argument("--dtb", default=None,
                        help="Path to DTB file. Defaults to an auto-generated arm-virt DTB")
    parser.add_argument("--initrd", default=_default_boot_asset("initramfs-rpi", "initramfs-lts"),
                        help="Path to initramfs (optional)")
    parser.add_argument("--append",
                        default=f"earlycon=pl011,0x{UART_BASE:08x} console=ttyAMA0 loglevel=8 printk.prefer_direct=1",
                        help="Kernel command line used when auto-generating a DTB")
    parser.add_argument("--core-model", default=None,
                        help="ARM core model to apply after load_kernel")
    parser.add_argument("--cpu", default=None,
                        help="ARM core model (alias for --core-model; use --cpu help to list)")
    parser.add_argument("--max-insns", type=int, default=10_000_000_000,
                        help="Maximum instructions to execute")
    parser.add_argument("--mem-mib", type=int, default=1024,
                        help="RAM size in MiB")
    parser.add_argument("--smp", type=int, default=1,
                        help="Number of vCPUs / CPU nodes to expose")
    parser.add_argument("--gic-version", choices=("v2", "v3"), default="v3",
                        help="Interrupt controller model to expose")
    parser.add_argument("--tick-scale", type=int, default=1,
                        help="Virtual-time scale factor (default 1). Higher values speed up delay loops.")
    parser.add_argument("--plugin", action="append", default=[],
                        help="Install a built-in plugin as NAME or NAME:arg=val,... (repeatable)")
    args = parser.parse_args()

    # Handle --cpu help / --machine help
    cpu_val = args.cpu or args.core_model
    if cpu_val in ("help", "?", "list"):
        print("Available CPU models:")
        print()
        for name, desc in _helm_ng.list_cpu_models():
            print(f"  {name:<16} {desc}")
        return

    dtb_path = _resolve_dtb_path(args.dtb, args.mem_mib, args.initrd, args.append, args.smp)
    dtb_arg = str(dtb_path) if args.dtb else None
    dtb_bytes = None if args.dtb else Path(dtb_path).read_bytes()

    print(f"helm-ng full-system AArch64 boot")
    print(f"  Kernel:  {args.kernel}")
    print(f"  DTB:     {dtb_path}")
    print(f"  Initrd:  {args.initrd}")
    print(f"  RAM:     {args.mem_mib} MiB")
    print(f"  SMP:     {args.smp}")
    print(f"  Max:     {args.max_insns:,} instructions")
    print()

    # Build simulation in FS mode
    sim = _helm_ng.build_simulation(
        isa="aarch64",
        mode="fs",
        timing="virtual",
        mem_mib=args.mem_mib,
    )

    # Load kernel (when FS mode support is wired to Python)
    sim.load_kernel(
        kernel=args.kernel,
        dtb=dtb_arg,
        dtb_bytes=dtb_bytes,
        initrd=args.initrd,
        num_cpus=args.smp,
        gic_version=args.gic_version,
    )
    if cpu_val:
        sim.set_cpu_model(cpu_val)
    if args.tick_scale > 1:
        sim.set_tick_scale(args.tick_scale)
    for spec in args.plugin:
        name, plugin_args = spec.split(":", 1) if ":" in spec else (spec, "")
        sim.add_plugin(name, plugin_args)

    # Run in chunks to show progress
    chunk_size = 10_000_000  # 10M instructions per chunk
    total = 0
    t0 = time.monotonic()
    wall = 0.0
    last_progress = 0.0  # wall-clock time of last progress print
    interrupted = False
    stop_reason = "quantum"
    sigint = _SigintFlag()
    old_sigint = signal.getsignal(signal.SIGINT)
    signal.signal(signal.SIGINT, sigint.handler)
    try:
        while total < args.max_insns:
            if sigint.triggered:
                interrupted = True
                stop_reason = "interrupt"
                print(f"\n[Simulation interrupted after {sim.insn_count:,} instructions]", file=sys.stderr)
                break
            ran = min(chunk_size, args.max_insns - total)
            result = sim.run(ran)
            stop_reason = result
            total += ran
            wall = time.monotonic() - t0
            if result == "quantum" and wall > 2.0 and wall - last_progress >= 5.0:
                last_progress = wall
                mips = sim.insn_count / wall / 1e6
                print(
                    f"\r[fs] {sim.insn_count/1e6:.0f}M insns  {wall:.0f}s  {mips:.0f} MIPS  PC={sim.pc:#x}",
                    end="",
                    file=sys.stderr,
                    flush=True,
                )
            if result.startswith("exit"):
                print(f"\n[Simulation exited after {sim.insn_count:,} instructions: {result}]")
                break
            if result.startswith("exception"):
                print(f"\n[Unhandled exception after {sim.insn_count:,} instructions]")
                break
            if result == "unsupported":
                print(f"\n[Stopped on unsupported instruction after {sim.insn_count:,} instructions]")
                break
        else:
            print(f"\n[Reached instruction limit: {args.max_insns:,}]")
    except KeyboardInterrupt:
        interrupted = True
        stop_reason = "interrupt"
        wall = time.monotonic() - t0
        if wall > 2.0:
            print(file=sys.stderr)
        print("\n[Simulation interrupted by Ctrl+C]", file=sys.stderr)
    finally:
        signal.signal(signal.SIGINT, old_sigint)
        wall = time.monotonic() - t0
        sim.finish()
        _print_sim_stats(sim, wall, stream=sys.stderr)

    if stop_reason == "interrupt":
        sys.exit(130)

if __name__ == '__main__':
    main()
