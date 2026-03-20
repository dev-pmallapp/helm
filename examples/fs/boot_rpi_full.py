#!/usr/bin/env python3
"""Boot an AArch64 Linux kernel on helm-ng's ARM virt platform.

Usage:
    python3 examples/fs/boot_rpi_full.py [--kernel PATH] [--dtb PATH] [--initrd PATH] [--max-insns N]

If ``--dtb`` is omitted, the script generates a minimal ARM virt DTB that
matches the devices currently implemented in helm-ng. If ``--dtb`` is
provided, it is used as-is and no temporary DTS/DTB is created.
"""
import argparse
import atexit
import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Optional

RAM_BASE = 0x4000_0000
INITRD_OFFSET = 0x0400_0000
UART_BASE = 0x0900_0000
GICD_BASE = 0x0800_0000
GICC_BASE = 0x0801_0000


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
        ROOT / "assets" / "aarch64" / "boot",
        ROOT / "assets" / "aarch64" / "alpine" / "boot",
    ]
    for path in candidates:
        if path.is_dir():
            return path
    return candidates[-1]


ASSETS = _resolve_assets_dir()


def _write_temp_file(suffix: str, data: str) -> Path:
    with tempfile.NamedTemporaryFile("w", suffix=suffix, delete=False, encoding="utf-8") as f:
        f.write(data)
        path = Path(f.name)
    atexit.register(lambda p=path: p.unlink(missing_ok=True))
    return path


def _generate_arm_virt_dtb(mem_mib: int, initrd_path: Optional[str], append: str) -> Path:
    initrd_props = ""
    if initrd_path:
        initrd_size = Path(initrd_path).stat().st_size
        initrd_start = RAM_BASE + INITRD_OFFSET
        initrd_end = initrd_start + initrd_size
        initrd_props = (
            f"        linux,initrd-start = <0x0 0x{initrd_start:08x}>;\n"
            f"        linux,initrd-end = <0x0 0x{initrd_end:08x}>;\n"
        )

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

        cpu@0 {{
            device_type = "cpu";
            compatible = "arm,cortex-a53";
            reg = <0x0 0x0>;
        }};
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
        interrupts = <1 13 4>, <1 14 4>, <1 11 4>, <1 10 4>;
        always-on;
    }};

    apb_pclk: apb-pclk {{
        compatible = "fixed-clock";
        #clock-cells = <0>;
        clock-frequency = <24000000>;
        clock-output-names = "clk24mhz";
    }};

    gic: interrupt-controller@{GICD_BASE:x} {{
        compatible = "arm,cortex-a15-gic";
        #address-cells = <0>;
        #interrupt-cells = <3>;
        interrupt-controller;
        reg = <0x0 0x{GICD_BASE:08x} 0x0 0x1000>,
              <0x0 0x{GICC_BASE:08x} 0x0 0x1000>;
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


def _resolve_dtb_path(explicit_dtb: Optional[str], mem_mib: int, initrd_path: Optional[str], append: str) -> Path:
    """Use caller-provided DTB when present; otherwise synthesize one for arm-virt."""
    if explicit_dtb:
        return Path(explicit_dtb)
    return _generate_arm_virt_dtb(mem_mib, initrd_path, append)

def main():
    parser = argparse.ArgumentParser(description="Boot AArch64 Linux kernel")
    parser.add_argument("--kernel", default=str(ASSETS / "vmlinuz-rpi"),
                        help="Path to ARM64 kernel Image")
    parser.add_argument("--dtb", default=None,
                        help="Path to DTB file. Defaults to an auto-generated arm-virt DTB")
    parser.add_argument("--initrd", default=str(ASSETS / "initramfs-rpi"),
                        help="Path to initramfs (optional)")
    parser.add_argument("--append",
                        default=f"earlycon=pl011,0x{UART_BASE:08x} console=ttyAMA0 loglevel=8 printk.prefer_direct=1",
                        help="Kernel command line used when auto-generating a DTB")
    parser.add_argument("--core-model", default="cortex-a53",
                        help="ARM core model to apply after load_kernel")
    parser.add_argument("--max-insns", type=int, default=10_000_000_000,
                        help="Maximum instructions to execute")
    parser.add_argument("--mem-mib", type=int, default=1024,
                        help="RAM size in MiB")
    args = parser.parse_args()
    dtb_path = _resolve_dtb_path(args.dtb, args.mem_mib, args.initrd, args.append)

    print(f"helm-ng full-system AArch64 boot")
    print(f"  Kernel:  {args.kernel}")
    print(f"  DTB:     {dtb_path}")
    print(f"  Initrd:  {args.initrd}")
    print(f"  RAM:     {args.mem_mib} MiB")
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
        dtb=str(dtb_path),
        initrd=args.initrd,
    )
    if args.core_model:
        sim.set_cpu_model(args.core_model)

    # Run in chunks to show progress
    chunk_size = 10_000_000  # 10M instructions per chunk
    total = 0
    while total < args.max_insns:
        ran = min(chunk_size, args.max_insns - total)
        result = sim.run(ran)
        total += ran
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

    print(f"Total instructions retired: {sim.insn_count:,}")

if __name__ == '__main__':
    main()
