#!/usr/bin/env python3
"""Self-contained example of native breakpoint/checkpoint control.

This script does not depend on an external ELF. It builds a small functional
RISC-V simulation, loads a few instructions directly, and demonstrates:

- `System.breakpoint(...)`
- `System.breakpoints()`
- `System.enable_breakpoint(...)`
- `System.save_checkpoint()`
- `System.restore_checkpoint()`

Usage:
    helm-aarch64 examples/debug/checkpoint_control.py
"""
import sys


def _require_helm_launcher() -> None:
    if getattr(sys, "_helm_launcher", None) not in {"helm-aarch64", "helm-system-aarch64"}:
        raise SystemExit(
            "This example must be run via helm-aarch64 or helm-system-aarch64, not directly via python."
        )


_require_helm_launcher()

import _helm_ng  # noqa: E402


def dump_breakpoints(sim, label):
    print(f"\n[{label}] breakpoints:")
    for bp_id, addr, action, enabled, hit_count in sim.breakpoints():
        print(f"  id={bp_id} addr={addr:#x} action={action} "
              f"enabled={enabled} hit_count={hit_count}")


def main():
    sim = _helm_ng.build_simulation(
        isa="riscv",
        mode="functional",
        timing="virtual",
        mem_base=0,
        mem_mib=1,
    )

    # Simple sequence:
    #   0x100: addi x1, x0, 42
    #   0x104: addi x2, x0, 7
    #   0x108: ecall
    code = bytes([
        0x93, 0x00, 0xA0, 0x02,
        0x13, 0x01, 0x70, 0x00,
        0x73, 0x00, 0x00, 0x00,
    ])
    sim.load_bytes(0x100, code)
    sim.set_pc(0x100)

    sim.breakpoint(0x104, action="log")
    dump_breakpoints(sim, "initial")

    checkpoint = bytes(sim.save_checkpoint())
    print(f"\n[saved] checkpoint bytes={len(checkpoint)}")

    sim.run(2)
    print(f"[run] pc={sim.pc:#x} x1={sim.xn(1):#x} x2={sim.xn(2):#x}")

    bp_id = sim.breakpoints()[0][0]
    sim.enable_breakpoint(bp_id, enabled=False)
    dump_breakpoints(sim, "disabled")

    sim.restore_checkpoint(checkpoint)
    print(f"\n[restored] pc={sim.pc:#x} x1={sim.xn(1):#x} x2={sim.xn(2):#x}")
    dump_breakpoints(sim, "after restore")

    sim.finish()


if __name__ == "__main__":
    main()
