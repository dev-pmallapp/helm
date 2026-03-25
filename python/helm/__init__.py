"""
helm — Python configuration layer for the helm-ng simulator.

Gem5-style: Python describes the machine, Rust simulates it.

ISA-neutral classes live here. ISA-specific classes live in
``helm.aarch64``, ``helm.riscv`` (future), etc.

Usage::

    import helm
    from helm.aarch64 import A55Cpu, GicV2, Pl011

    # New SimObject API:
    system = helm.System("virt", timing="virtual", mode="fs")
    system.cpu = A55Cpu("cpu0")
    system.instantiate()
    system.load_kernel("Image", dtb="virt.dtb")
    system.run(10_000_000)

    # Backward-compatible:
    sim = helm.build_simulation(isa="aarch64", mode="se")
    sim.load_elf("./hello", ["hello"])
    sim.run(10_000_000)
"""

import _helm_ng
from _helm_ng import (
    # SimObject hierarchy (ISA-neutral)
    SimObject,
    System,
    Cpu,
    Ram,
    MemorySpace,
    MapEntry,
    Cache,
    # Support
    PortRef,
    HelmSpy,
    # Backward-compat
    build_simulation,
    set_sim_trace,
)

# Re-export the old Simulation name as an alias for System
Simulation = System

__version__ = "0.2.0"

__all__ = [
    # SimObject hierarchy
    "SimObject",
    "System",
    "Simulation",
    "Cpu",
    "Ram",
    "MemorySpace",
    "MapEntry",
    "Cache",
    # Devices (re-exported from aarch64 for convenience)
    "GicV2",
    "Pl011",
    # Support
    "PortRef",
    "HelmSpy",
    # Functions
    "build_simulation",
    "set_sim_trace",
]

# Import ISA-specific devices at top level for convenience
try:
    from _helm_ng import GicV2, Pl011
except ImportError:
    pass
