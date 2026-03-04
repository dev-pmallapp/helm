#!/usr/bin/env python3
"""
Example: Building a custom timer device and attaching it to a platform.

Demonstrates how developers use HELM's Python layer to define devices
and compose them into a simulated system.
"""

from helm import Platform, Core, Cache, MemorySystem, Device, TimingMode, Simulation
from helm.isa import RiscV
from helm.predictor import BranchPredictor


# 1. Define a custom device by subclassing Device
class SimpleTimer(Device):
    """A minimal 32-bit countdown timer with an interrupt."""

    # Register offsets
    REG_COUNTER = 0x00
    REG_RELOAD = 0x04
    REG_CONTROL = 0x08
    REG_STATUS = 0x0C

    def __init__(self, name: str = "timer0", base_address: int = 0x4000_0000):
        super().__init__(name, region_size=0x10, base_address=base_address, irq=5)
        self.counter = 0
        self.reload_value = 0
        self.enabled = False
        self.fired = False

    def read(self, offset: int, size: int) -> int:
        if offset == self.REG_COUNTER:
            return self.counter
        if offset == self.REG_RELOAD:
            return self.reload_value
        if offset == self.REG_CONTROL:
            return int(self.enabled)
        if offset == self.REG_STATUS:
            return int(self.fired)
        return 0

    def write(self, offset: int, size: int, value: int) -> None:
        if offset == self.REG_COUNTER:
            self.counter = value
        elif offset == self.REG_RELOAD:
            self.reload_value = value
        elif offset == self.REG_CONTROL:
            self.enabled = bool(value & 1)
            if self.enabled:
                self.counter = self.reload_value
        elif offset == self.REG_STATUS:
            self.fired = False  # write to clear

    def reset(self) -> None:
        self.counter = 0
        self.reload_value = 0
        self.enabled = False
        self.fired = False


class SimpleUart(Device):
    """A minimal UART transmit-only device."""

    REG_DATA = 0x00
    REG_STATUS = 0x04

    def __init__(self, name: str = "uart0", base_address: int = 0x4000_1000):
        super().__init__(name, region_size=0x08, base_address=base_address)
        self.tx_buffer: list[int] = []

    def read(self, offset: int, size: int) -> int:
        if offset == self.REG_STATUS:
            return 1  # always ready
        return 0

    def write(self, offset: int, size: int, value: int) -> None:
        if offset == self.REG_DATA:
            self.tx_buffer.append(value & 0xFF)

    def reset(self) -> None:
        self.tx_buffer.clear()


# 2. Compose the platform
core = Core("rv-core", width=2, rob_size=64,
            branch_predictor=BranchPredictor.bimodal(2048))

memory = MemorySystem(
    l1i=Cache("32KB", assoc=8, latency=1),
    l1d=Cache("32KB", assoc=8, latency=1),
    dram_latency=100,
)

platform = Platform(
    name="custom-soc",
    isa=RiscV(),
    cores=[core],
    memory=memory,
    devices=[
        SimpleTimer("timer0", base_address=0x4000_0000),
        SimpleTimer("timer1", base_address=0x4000_0010),
        SimpleUart("uart0", base_address=0x4000_1000),
    ],
    timing=TimingMode.ape(),
)

print(f"Platform: {platform}")
print(f"Devices:  {platform.devices}")
print()

# 3. Run simulation
sim = Simulation(platform, binary="./firmware.elf", mode="se")
results = sim.run()
print(f"Results: {results}")
