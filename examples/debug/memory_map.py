#!/usr/bin/env python3
"""Inspect memory layout and contents of an SE-mode binary.

Real-world scenario: During mmap debugging (commit ad327be), the
grow-upward mmap allocator was producing overlapping regions, and
huge-hint addresses were causing guest SIGSEGV.  Being able to inspect
the memory map — stack layout, heap position, mmap regions, and ELF
segment placement — was essential for root-cause analysis.

Also used for:
  - Verifying ELF loader placed segments correctly (PT_LOAD alignment)
  - Checking auxv entries on the initial stack
  - Watching heap growth via brk() and mmap()
  - Debugging stack corruption by dumping stack contents

This example demonstrates:
  - read_mem() for arbitrary memory reads
  - Stack frame inspection (SP-relative reads)
  - ELF entry point and segment verification
  - Symbol table enumeration for address-to-name mapping
  - Memory region dumps with ASCII rendering

Usage:
    helm-aarch64 examples/debug/memory_map.py --binary ./my_elf
    helm-aarch64 examples/debug/memory_map.py --binary ./my_elf \\
        --dump-stack 128 --dump-addr 0x400000 --dump-len 64
    helm-aarch64 examples/debug/memory_map.py --binary ./my_elf \\
        --run-first 1000  # run N insns before inspecting
"""
import argparse
import os
import struct
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import helmutil

helmutil.require_launcher()
_helm_ng = helmutil.import_helm_ng()
sys.stdout.reconfigure(line_buffering=True)


def parse_args():
    p = argparse.ArgumentParser(
        description="Memory layout inspector for SE-mode binaries"
    )
    p.add_argument("--binary", "-b", default=helmutil.default_binary())
    p.add_argument("--run-first", type=int, default=0,
                   help="Run this many instructions before inspecting "
                        "(useful to see post-init memory layout)")
    p.add_argument("--dump-stack", type=int, default=64,
                   help="Bytes of stack to dump from SP (default 64)")
    p.add_argument("--dump-addr", type=lambda x: int(x, 0), default=None,
                   help="Hex address to dump memory from")
    p.add_argument("--dump-len", type=int, default=64,
                   help="Bytes to dump from --dump-addr (default 64)")
    p.add_argument("--show-symbols", action="store_true",
                   help="Print full symbol table from ELF")
    p.add_argument("--max-symbols", type=int, default=50,
                   help="Max symbols to display (default 50)")
    p.add_argument("--argv", nargs="*", default=None)
    return p.parse_args()


def _hex_dump(sim, addr, length, label=""):
    """Hex dump memory region with ASCII sidebar."""
    if label:
        print(f"\n  {label}:")
    print(f"  {'Address':>18s}  {'Hex':48s}  ASCII")
    print(f"  {'-'*18}  {'-'*48}  {'-'*16}")

    for off in range(0, length, 16):
        row_addr = addr + off
        row_len = min(16, length - off)

        # Read 8 bytes at a time via u64 reads (fewer FFI calls)
        raw_bytes = []
        for q in range(0, row_len, 8):
            try:
                val = sim.read_mem(row_addr + q)
                chunk = val.to_bytes(8, "little")
                raw_bytes.extend(chunk[:min(8, row_len - q)])
            except Exception:
                raw_bytes.extend([None] * min(8, row_len - q))

        hex_parts = []
        ascii_parts = []
        for i in range(16):
            if i >= row_len:
                hex_parts.append("  ")
                ascii_parts.append(" ")
            elif i < len(raw_bytes) and raw_bytes[i] is not None:
                b = raw_bytes[i]
                hex_parts.append(f"{b:02x}")
                ascii_parts.append(chr(b) if 32 <= b < 127 else ".")
            else:
                hex_parts.append("??")
                ascii_parts.append("?")

        hex_str = " ".join(hex_parts[:8]) + "  " + " ".join(hex_parts[8:])
        ascii_str = "".join(ascii_parts)
        print(f"  {row_addr:#018x}  {hex_str}  |{ascii_str}|")


def _read_u64(sim, addr):
    """Read a 64-bit value from memory."""
    try:
        return sim.read_mem(addr)
    except Exception:
        return None


def _dump_stack_frame(sim, depth=64):
    """Dump stack contents from current SP."""
    sp = sim.sp
    print(f"\n  Stack dump (SP={sp:#018x}, {depth} bytes):")
    print(f"  {'Offset':>8s}  {'Address':>18s}  {'Value':>18s}  Notes")
    print(f"  {'-'*8}  {'-'*18}  {'-'*18}  {'-'*20}")

    for off in range(0, depth, 8):
        addr = sp + off
        val = _read_u64(sim, addr)
        if val is None:
            print(f"  {off:>+8d}  {addr:#018x}  {'(unreadable)':>18s}")
            continue

        # Annotate potential pointers
        notes = ""
        if 0x400000 <= val <= 0x800000:
            notes = "  (text segment?)"
        elif val > 0x7f0000000000:
            notes = "  (stack region?)"
        elif val == 0:
            notes = "  (NULL)"

        print(f"  {off:>+8d}  {addr:#018x}  {val:#018x}{notes}")


def _dump_auxv(sim):
    """Try to find and dump the auxiliary vector on the initial stack."""
    sp = sim.sp
    # On AArch64 Linux SE, stack layout is:
    #   SP+0:  argc
    #   SP+8:  argv[0] ... argv[argc-1], NULL
    #   then:  envp[0] ... envp[N], NULL
    #   then:  auxv entries (pairs of u64: type, value)
    argc = _read_u64(sim, sp)
    if argc is None or argc > 100:
        return

    print(f"\n  Initial stack layout:")
    print(f"    argc = {argc}")

    # Skip past argv
    argv_start = sp + 8
    for i in range(int(argc) + 1):  # +1 for NULL terminator
        val = _read_u64(sim, argv_start + i * 8)
        if val is None:
            return
        if i < int(argc):
            print(f"    argv[{i}] = {val:#018x}")

    # Skip past envp
    envp_start = argv_start + (int(argc) + 1) * 8
    envp_count = 0
    for i in range(256):
        val = _read_u64(sim, envp_start + i * 8)
        if val is None or val == 0:
            envp_count = i
            break

    print(f"    envp: {envp_count} entries")

    # Auxv starts after envp NULL
    AT_NAMES = {
        0: "AT_NULL", 3: "AT_PHDR", 4: "AT_PHENT", 5: "AT_PHNUM",
        6: "AT_PAGESZ", 7: "AT_BASE", 9: "AT_ENTRY", 11: "AT_UID",
        12: "AT_EUID", 13: "AT_GID", 14: "AT_EGID", 16: "AT_HWCAP",
        17: "AT_CLKTCK", 23: "AT_SECURE", 25: "AT_RANDOM",
        26: "AT_HWCAP2", 33: "AT_SYSINFO_EHDR",
    }
    auxv_start = envp_start + (envp_count + 1) * 8
    print(f"\n  Auxiliary vector (at {auxv_start:#018x}):")
    for i in range(32):
        at_type = _read_u64(sim, auxv_start + i * 16)
        at_val = _read_u64(sim, auxv_start + i * 16 + 8)
        if at_type is None or at_type == 0:
            print(f"    AT_NULL")
            break
        name = AT_NAMES.get(int(at_type), f"AT_{int(at_type)}")
        print(f"    {name:<20s} = {at_val:#018x}")


def main():
    args = parse_args()
    binary = args.binary
    if not os.path.isfile(binary):
        print(f"[mem] binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    argv = args.argv or [os.path.basename(binary), "-c", "echo hello"]
    envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C",
            "USER=helm"]

    sim = _helm_ng.build_simulation(isa="aarch64", mode="se",
                                     timing="virtual")
    sim.load_elf(binary, argv, envp)

    print(f"[mem] binary={binary}")
    print(f"[mem] Entry point: {sim.pc:#018x}")
    print(f"[mem] Initial SP:  {sim.sp:#018x}")
    print()

    # Optionally run some instructions first
    if args.run_first > 0:
        print(f"[mem] Running {args.run_first:,} instructions first...")
        t0 = time.monotonic()
        stop = sim.run(args.run_first)
        wall = time.monotonic() - t0
        print(f"[mem] Ran {sim.insn_count:,} insns ({wall:.2f}s), "
              f"stop={stop}")
        print(f"[mem] Current PC:  {sim.pc:#018x}")
        print(f"[mem] Current SP:  {sim.sp:#018x}")

    # Register state
    print(f"\n{'='*60}")
    print("REGISTER STATE")
    print("=" * 60)
    for i in range(0, 31, 4):
        regs = "  ".join(f"x{i+j:<2d}={sim.xn(i+j):#018x}"
                         for j in range(4) if i + j < 31)
        print(f"  {regs}")
    print(f"  SP ={sim.sp:#018x}  PC ={sim.pc:#018x}  "
          f"NZCV={sim.nzcv:#x}")

    # Stack dump
    print(f"\n{'='*60}")
    print("STACK INSPECTION")
    print("=" * 60)
    _dump_stack_frame(sim, args.dump_stack)

    # Auxv dump (only meaningful before running instructions)
    if args.run_first == 0:
        _dump_auxv(sim)

    # Symbol table
    if args.show_symbols:
        print(f"\n{'='*60}")
        print("SYMBOL TABLE")
        print("=" * 60)
        syms = sim.symbols()
        print(f"  {len(syms)} symbols found")
        for name, addr, size in syms[:args.max_symbols]:
            print(f"  {addr:#018x}  {size:>6d}  {name}")
        if len(syms) > args.max_symbols:
            print(f"  ... and {len(syms) - args.max_symbols} more")

    # Memory region around entry point
    print(f"\n{'='*60}")
    print("ENTRY POINT MEMORY")
    print("=" * 60)
    entry_pc = sim.pc
    _hex_dump(sim, entry_pc, 64, f"Code at PC ({entry_pc:#018x})")

    # Custom memory dump
    if args.dump_addr is not None:
        print(f"\n{'='*60}")
        print(f"MEMORY DUMP @ {args.dump_addr:#018x}")
        print("=" * 60)
        _hex_dump(sim, args.dump_addr, args.dump_len,
                  f"Region {args.dump_addr:#x}")

    # Stack as hex dump
    _hex_dump(sim, sim.sp, min(args.dump_stack, 128),
              f"Stack (SP={sim.sp:#018x})")

    sim.finish()


if __name__ == "__main__":
    main()
