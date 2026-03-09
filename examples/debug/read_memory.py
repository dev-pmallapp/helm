#!/usr/bin/env python3
"""Read and hexdump physical or virtual memory at a given point in boot.

Usage:
    helm-system-aarch64 examples/debug/read_memory.py
    helm-system-aarch64 examples/debug/read_memory.py -- --addr 0x40200000 --size 128
    helm-system-aarch64 examples/debug/read_memory.py -- --va 0xffff800040200000
"""
import argparse, sys
import _helm_core

sys.stdout.reconfigure(line_buffering=True)


def hexdump(data: bytes, base: int):
    for off in range(0, len(data), 16):
        chunk = data[off:off + 16]
        hexs = " ".join(f"{b:02x}" for b in chunk)
        ascii_str = "".join(chr(b) if 32 <= b < 127 else "." for b in chunk)
        print(f"  {base + off:#010x}: {hexs:<48} {ascii_str}")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--kernel", default="assets/alpine/boot/vmlinuz-rpi")
    p.add_argument("--at", type=int, default=10_000_000,
                    help="Run this many insns before reading")
    p.add_argument("--addr", type=lambda x: int(x, 0), default=0x4000_0000,
                    help="Physical address to read")
    p.add_argument("--va", type=lambda x: int(x, 0), default=None,
                    help="Virtual address to read (uses MMU)")
    p.add_argument("--size", type=int, default=64)
    args = p.parse_args()

    s = _helm_core.FsSession(
        kernel=args.kernel, machine="virt",
        append="earlycon=pl011,0x09000000 console=ttyAMA0",
    )
    s.run(args.at)
    print(f"PC={s.pc:#x}  insns={s.insn_count:,}\n")

    if args.va is not None:
        data = s.read_virtual(args.va, args.size)
        label = f"VA {args.va:#x}"
    else:
        data = s.read_memory(args.addr, args.size)
        label = f"PA {args.addr:#x}"

    if data is None:
        print(f"  read failed for {label}")
    else:
        print(f"  {label} ({len(data)} bytes):")
        hexdump(bytes(data), args.va if args.va is not None else args.addr)


if __name__ == "__main__":
    main()
