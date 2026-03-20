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
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import _helm_ng

sys.stdout.reconfigure(line_buffering=True)

# ── ARM virt address map (must match helm-engine/src/platform/arm_virt.rs) ──
RAM_BASE   = 0x4000_0000
UART_BASE  = 0x0900_0000
GICD_BASE  = 0x0800_0000
GICC_BASE  = 0x0801_0000

DEFAULT_APPEND = "earlycon=pl011,0x09000000 console=ttyAMA0 loglevel=8 printk.prefer_direct=1"
ASSET_BOOT = Path("assets/aarch64/alpine/boot")


def _default_asset(env_name: str, *candidates: str) -> str | None:
    env_val = os.environ.get(env_name)
    if env_val:
        return env_val
    for candidate in candidates:
        path = ASSET_BOOT / candidate
        if path.is_file():
            return str(path)
    return None


def parse_args():
    p = argparse.ArgumentParser(description="helm-ng FS — boot AArch64 Linux kernel")
    p.add_argument("--kernel", "-k",
                   default=_default_asset("HELM_KERNEL", "vmlinuz-lts", "vmlinuz-rpi"),
                   help="Path to ARM64 kernel Image (default: $HELM_KERNEL or arm-virt assets/)")
    p.add_argument("--dtb",
                   default=os.environ.get("HELM_DTB", None),
                   help="Path to DTB file (auto-generated if omitted)")
    p.add_argument("--initrd",
                   default=_default_asset("HELM_INITRD", "initramfs-lts", "initramfs-rpi"),
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
                   help="Timing model (selects simulation accuracy)")
    p.add_argument("--core-model", "--core", default=None,
                   help="ARM core model: cortex-a55, cortex-a73, neoverse-n1, cortex-a78, "
                        "cortex-x1, cortex-a510, cortex-a710, generic (default: cortex-a55)")
    return p.parse_args()


CPU_TIMING = {
    "atomic":  "virtual",
    "timing":  "interval",
    "minor":   "interval",
    "o3":      "accurate",
    "big":     "accurate",
}


def _temp_file(suffix: str, content: str) -> Path:
    with tempfile.NamedTemporaryFile("w", suffix=suffix, delete=False, encoding="utf-8") as f:
        f.write(content)
        path = Path(f.name)
    atexit.register(lambda p=path: p.unlink(missing_ok=True))
    return path


def generate_virt_dtb(mem_mib: int, initrd_path: str | None, bootargs: str) -> Path:
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


def main():
    args = parse_args()

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
        dtb_path = str(generate_virt_dtb(args.mem_mib, args.initrd, baked_cmdline))
        # Bootargs already baked in; don't double-apply via Rust patcher.
        append_override = None
    else:
        append_override = args.append or None

    timing = CPU_TIMING.get(args.cpu, "virtual")
    print(f"[fs] kernel={args.kernel}  dtb={dtb_path}  "
          f"initrd={args.initrd or '(none)'}  cpu={args.cpu}  timing={timing}")

    sim = _helm_ng.build_simulation(
        isa="aarch64",
        mode="fs",
        timing=timing,
        mem_mib=args.mem_mib,
    )

    # Apply ARM core model (ID registers, MIDR, feature bits)
    sim.load_kernel(
        kernel=args.kernel,
        dtb=dtb_path,
        initrd=args.initrd or None,
        append=append_override,
    )

    # Apply ARM core model AFTER load_kernel so a64_state exists.
    # set_cpu_model is a no-op when called before load_kernel (a64_state=None).
    core_model = args.core_model or "cortex-a53"
    try:
        sim.set_cpu_model(core_model)
        print(f"[fs] core-model={core_model}")
    except Exception as e:
        print(f"[fs] Warning: could not set core model '{core_model}': {e}", file=sys.stderr)

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
