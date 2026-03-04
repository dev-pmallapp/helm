"""Pre-built memory configurations."""

from helm.memory import Cache, MemorySystem


def SimpleMemory() -> MemorySystem:
    """Minimal memory hierarchy: L1 only."""
    return MemorySystem(
        l1i=Cache("32KB", assoc=4, latency=1),
        l1d=Cache("32KB", assoc=4, latency=1),
        dram_latency=100,
    )


def TypicalDesktop() -> MemorySystem:
    """Three-level cache hierarchy typical of desktop processors."""
    return MemorySystem(
        l1i=Cache("32KB", assoc=8, latency=1),
        l1d=Cache("48KB", assoc=12, latency=1),
        l2=Cache("512KB", assoc=8, latency=12),
        l3=Cache("16MB", assoc=16, latency=40),
        dram_latency=120,
    )


def ServerMemory() -> MemorySystem:
    """Large shared L3, higher DRAM latency."""
    return MemorySystem(
        l1i=Cache("32KB", assoc=8, latency=1),
        l1d=Cache("32KB", assoc=8, latency=1),
        l2=Cache("1MB", assoc=8, latency=14),
        l3=Cache("32MB", assoc=16, latency=50),
        dram_latency=200,
    )
