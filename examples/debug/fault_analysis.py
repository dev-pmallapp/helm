#!/usr/bin/env python3
"""Analyze exceptions and faults with rolling trace windows.

Real-world scenario: During FS-mode kernel boot, translation faults,
data aborts, and timer interrupts are expected — but incorrect ESR
encoding or stale TLB entries cause unexpected faults.  Commit 92c017b
fixed BRK setting ELR to PC+4 instead of PC; commit 0a4d0d6 fixed
stale TLB entries after guest page-table writes; commit 94e4ef0 fixed
a timer infinite loop where cpu_eoi() re-pended IRQ 30 indefinitely.

The trace-window-fault plugin keeps rolling windows of recent
instructions, memory accesses, branches, and syscalls, dumping them
on fault for immediate context.

This example demonstrates:
  - trace-window-fault plugin with configurable window sizes
  - fault-detect for ring buffer context
  - ESR/FAR/ELR register analysis for AArch64 exceptions
  - Exception level tracking (EL0/EL1/EL2)
  - Both SE faults (SIGILL, SIGSEGV) and FS faults (translation,
    permission, alignment)

Usage:
    helm-system-aarch64 examples/debug/fault_analysis.py
    helm-system-aarch64 examples/debug/fault_analysis.py -- \\
        --max-insns 100000000 --insn-window 64 --mem-window 32
    helm-aarch64 examples/debug/fault_analysis.py --mode se \\
        --binary ./crashing_binary
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

# AArch64 ESR exception class decoding (EC field, bits [31:26])
ESR_EC_NAMES = {
    0x00: "Unknown",
    0x01: "WFI/WFE",
    0x07: "SIMD/FP access",
    0x0E: "Illegal execution state",
    0x15: "SVC (AArch64)",
    0x16: "HVC (AArch64)",
    0x17: "SMC (AArch64)",
    0x18: "MSR/MRS system insn",
    0x20: "Insn abort (lower EL)",
    0x21: "Insn abort (same EL)",
    0x22: "PC alignment fault",
    0x24: "Data abort (lower EL)",
    0x25: "Data abort (same EL)",
    0x26: "SP alignment fault",
    0x2C: "FP exception",
    0x30: "Serror",
    0x32: "Sw step (lower EL)",
    0x33: "Sw step (same EL)",
    0x34: "Watchpoint (lower EL)",
    0x35: "Watchpoint (same EL)",
    0x38: "BRK (AArch64)",
    0x3C: "BRK (AArch32)",
}


def parse_args():
    p = argparse.ArgumentParser(
        description="Exception/fault analyzer with rolling trace windows"
    )
    p.add_argument("--mode", choices=["se", "fs"], default="fs",
                   help="Execution mode (default: fs)")
    p.add_argument("--binary", "-b", default=None,
                   help="Binary for SE mode")
    p.add_argument("--kernel", default=None)
    p.add_argument("--initrd", default=None)
    p.add_argument("--max-insns", "-n", type=int, default=50_000_000,
                   help="Max instructions (default 50M)")
    p.add_argument("--mem-mib", type=int, default=1024)
    p.add_argument("--insn-window", type=int, default=32,
                   help="trace-window-fault insn history (default 32)")
    p.add_argument("--mem-window", type=int, default=16,
                   help="trace-window-fault memory history (default 16)")
    p.add_argument("--branch-window", type=int, default=16,
                   help="trace-window-fault branch history (default 16)")
    p.add_argument("--syscall-window", type=int, default=8,
                   help="trace-window-fault syscall history (default 8)")
    p.add_argument("--argv", nargs="*", default=None)
    return p.parse_args()


def _decode_esr(esr):
    """Decode AArch64 ESR_ELx into human-readable fields."""
    ec = (esr >> 26) & 0x3F
    il = (esr >> 25) & 1
    iss = esr & 0x1FFFFFF
    ec_name = ESR_EC_NAMES.get(ec, f"Reserved({ec:#x})")
    il_str = "32-bit" if il else "16-bit"

    result = [
        f"ESR = {esr:#010x}",
        f"  EC  = {ec:#04x} ({ec_name})",
        f"  IL  = {il} ({il_str} instruction)",
        f"  ISS = {iss:#09x}",
    ]

    # Decode DFSC for data aborts
    if ec in (0x24, 0x25):
        dfsc = iss & 0x3F
        wnr = (iss >> 6) & 1
        cm = (iss >> 8) & 1
        level = dfsc & 0x3
        fault_types = {
            0x04: "Translation fault",
            0x05: "Translation fault",
            0x06: "Translation fault",
            0x07: "Translation fault",
            0x08: "Access flag fault",
            0x09: "Access flag fault",
            0x0A: "Access flag fault",
            0x0B: "Access flag fault",
            0x0C: "Permission fault",
            0x0D: "Permission fault",
            0x0E: "Permission fault",
            0x0F: "Permission fault",
            0x10: "External abort",
            0x21: "Alignment fault",
        }
        ftype = fault_types.get(dfsc, f"Other({dfsc:#x})")
        result.append(f"  DFSC= {dfsc:#04x} L{level} {ftype}")
        result.append(f"  WnR = {wnr} ({'Write' if wnr else 'Read'})")
        result.append(f"  CM  = {cm} ({'Cache maintenance' if cm else ''})")

    return "\n".join(result)


def main():
    args = parse_args()

    if args.mode == "se":
        binary = args.binary or helmutil.default_binary()
        if not os.path.isfile(binary):
            print(f"[fault-analysis] binary not found: {binary}",
                  file=sys.stderr)
            sys.exit(1)
        argv = args.argv or [os.path.basename(binary), "-c", "echo hello"]
        envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C",
                "USER=helm"]
        sim = _helm_ng.build_simulation(isa="aarch64", mode="se",
                                         timing="virtual")
        sim.load_elf(binary, argv, envp)
        print(f"[fault-analysis] SE mode: binary={binary}")
    else:
        boot = helmutil.load_boot_module()
        kernel = args.kernel or helmutil.default_kernel()
        initrd = args.initrd or helmutil.default_initrd()
        if not kernel:
            print("[fault-analysis] No kernel found; use --kernel",
                  file=sys.stderr)
            sys.exit(1)
        dtb = boot._resolve_dtb_path(
            None, args.mem_mib, initrd,
            f"earlycon=pl011,0x{helmutil.UART_BASE:08x} "
            f"console=ttyAMA0 loglevel=8")
        sim = _helm_ng.build_simulation(isa="aarch64", mode="fs",
                                         timing="virtual",
                                         mem_mib=args.mem_mib)
        sim.load_kernel(kernel=str(kernel), dtb=str(dtb),
                        initrd=str(initrd) if initrd else None)
        print(f"[fault-analysis] FS mode: kernel={kernel}")

    # Attach trace-window-fault plugin
    twf_args = (
        f"insns={args.insn_window},"
        f"mem={args.mem_window},"
        f"branches={args.branch_window},"
        f"syscalls={args.syscall_window}"
    )
    sim.add_plugin("trace-window-fault", twf_args)

    # Also attach fault-detect for ring buffer
    sim.add_plugin("fault-detect", "history=64")

    print(f"[fault-analysis] trace-window-fault: "
          f"insns={args.insn_window} mem={args.mem_window} "
          f"branches={args.branch_window} "
          f"syscalls={args.syscall_window}")
    print(f"[fault-analysis] Running {args.max_insns:,} insns...\n")

    t0 = time.monotonic()
    chunk = 10_000_000
    remaining = args.max_insns
    stop_reason = "quantum"

    while remaining > 0 and not sim.has_exited:
        n = min(chunk, remaining)
        stop_reason = sim.run(n)
        remaining -= n

        if stop_reason != "quantum":
            break

        wall = time.monotonic() - t0
        if wall > 2.0:
            mips = sim.insn_count / wall / 1e6
            print(f"\r[fault-analysis] {sim.insn_count/1e6:.0f}M insns  "
                  f"{wall:.0f}s  {mips:.0f} MIPS",
                  end="", file=sys.stderr, flush=True)

    wall = time.monotonic() - t0
    mips = sim.insn_count / wall / 1e6 if wall > 0.001 else 0
    print(f"\n\n[fault-analysis] {sim.insn_count:,} insns in {wall:.2f}s "
          f"({mips:.0f} MIPS)")
    print(f"[fault-analysis] stop_reason={stop_reason}")

    # Exception register analysis (FS mode)
    if args.mode == "fs":
        print(f"\n{'='*60}")
        print("EXCEPTION STATE ANALYSIS")
        print("=" * 60)
        esr = sim.esr_el1
        far = sim.far_el1
        elr = sim.elr_el1
        el = sim.current_el
        print(f"\n  Current EL: {el}")
        print(f"  ELR_EL1:    {elr:#018x}")
        print(f"  FAR_EL1:    {far:#018x}")
        print(f"\n  {_decode_esr(esr)}")

    # Register dump
    print(f"\n  Registers at stop:")
    for i in range(0, 31, 4):
        regs = "  ".join(f"x{i+j:<2d}={sim.xn(i+j):#018x}"
                         for j in range(4) if i + j < 31)
        print(f"    {regs}")
    print(f"    SP ={sim.sp:#018x}  PC ={sim.pc:#018x}  "
          f"NZCV={sim.nzcv:#x}")

    # Plugin reports
    print(f"\n{'='*60}")
    print("Plugin reports (trace windows, fault history):")
    print("=" * 60)
    sim.finish()


if __name__ == "__main__":
    main()
