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

try:
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
        build_simulation as _build_simulation_raw,
        set_sim_trace as _set_sim_trace_raw,
    )

    # Re-export the old Simulation name as an alias for System
    Simulation = System

    __version__ = _helm_ng.__version__


    def build_simulation(*args, **kwargs):
        """Backward-compatible factory. Use System() API instead."""
        import warnings
        warnings.warn(
            "build_simulation() is deprecated, use helm.System() API instead",
            DeprecationWarning,
            stacklevel=2,
        )
        return _build_simulation_raw(*args, **kwargs)


    def set_sim_trace(*args, **kwargs):
        """Deprecated. Use system.observe() API instead."""
        import warnings
        warnings.warn(
            "set_sim_trace() is deprecated, use system.observe() API instead",
            DeprecationWarning,
            stacklevel=2,
        )
        return _set_sim_trace_raw(*args, **kwargs)
except ImportError:
    # Native extension not available -- resource management still works.
    _helm_ng = None

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
    # Resource management
    "obtain_resource",
    "resource_path",
    "list_resources",
]

# Import ISA-specific devices at top level for convenience
try:
    from _helm_ng import GicV2, Pl011
except ImportError:
    pass

# Resource management (pure-Python, no _helm_ng dependency).
# Deferred import so helm.resources works without _helm_ng loaded.
def __getattr__(name):
    _lazy_modules = {
        "obtain_resource": "resources",
        "resource_path": "resources",
        "list_resources": "resources",
        "simulate": "simulate",
        "SimResult": "simulate",
        "dump_stats": "stats",
        "format_stats": "stats",
        "format_stats_json": "stats",
        "snapshot_regs": "debug",
        "reg_diff": "debug",
        "compare_lockstep": "debug",
        "CompareResult": "debug",
        "project_root": "util",
        "import_helm_ng": "util",
        "require_launcher": "util",
        "to_bytes": "util",
    }
    if name in _lazy_modules:
        mod = __import__(f"helm.{_lazy_modules[name]}", fromlist=[name])
        val = getattr(mod, name)
        globals()[name] = val
        return val
    raise AttributeError(f"module 'helm' has no attribute {name!r}")
