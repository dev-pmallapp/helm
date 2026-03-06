"""Board / SoC definitions — reusable system-level compositions.

Mirrors gem5's board concept: combines cores, memory hierarchy,
and (optionally) devices into a named platform.
"""

from helm.platform import Platform
from helm.memory import Cache, MemorySystem
from helm.isa import Arm
from configs.components.arm_cores import CortexA53, CortexA72, CortexA76, NeoVerseN1


class ArmBoard:
    """Base for ARM board definitions."""

    def __init__(self, name, cores_with_timing, memory):
        first_timing = cores_with_timing[0][1]
        self.platform = Platform(
            name=name,
            isa=Arm(),
            cores=[c for c, _ in cores_with_timing],
            memory=memory,
            timing=first_timing,
        )
        self.timing = first_timing

    @property
    def name(self):
        return self.platform.name

    def to_dict(self):
        return self.platform.to_dict()


class RaspberryPi3(ArmBoard):
    """Raspberry Pi 3 Model B — quad Cortex-A53, no L3."""

    def __init__(self, name="rpi3"):
        memory = MemorySystem(
            l1i=Cache("16KB", assoc=2, latency=1),
            l1d=Cache("16KB", assoc=4, latency=3),
            l2=Cache("512KB", assoc=16, latency=15),
            dram_latency=100,
        )
        cores = [CortexA53(f"a53-{i}") for i in range(4)]
        super().__init__(name, cores, memory)


class RaspberryPi4(ArmBoard):
    """Raspberry Pi 4 — quad Cortex-A72."""

    def __init__(self, name="rpi4"):
        memory = MemorySystem(
            l1i=Cache("48KB", assoc=3, latency=1),
            l1d=Cache("32KB", assoc=2, latency=4),
            l2=Cache("1MB", assoc=16, latency=11),
            dram_latency=80,
        )
        cores = [CortexA72(f"a72-{i}") for i in range(4)]
        super().__init__(name, cores, memory)


class ArmBigLittle(ArmBoard):
    """big.LITTLE — 2× Cortex-A72 + 4× Cortex-A53."""

    def __init__(self, name="big-little"):
        memory = MemorySystem(
            l1i=Cache("32KB", assoc=4, latency=1),
            l1d=Cache("32KB", assoc=4, latency=4),
            l2=Cache("256KB", assoc=8, latency=11),
            l3=Cache("4MB", assoc=16, latency=30),
            dram_latency=120,
        )
        big = [CortexA72(f"big-{i}") for i in range(2)]
        little = [CortexA53(f"little-{i}") for i in range(4)]
        super().__init__(name, big + little, memory)


class NeoVerseServer(ArmBoard):
    """Arm Neoverse server — 4× N1 with large L3."""

    def __init__(self, name="neoverse-n1"):
        memory = MemorySystem(
            l1i=Cache("64KB", assoc=4, latency=1),
            l1d=Cache("64KB", assoc=4, latency=4),
            l2=Cache("1MB", assoc=8, latency=9),
            l3=Cache("32MB", assoc=16, latency=35),
            dram_latency=150,
        )
        cores = [NeoVerseN1(f"n1-{i}") for i in range(4)]
        super().__init__(name, cores, memory)
