"""QEMU virt machine — minimal ARM platform for quick testing."""

from helm.platform import Platform
from helm.isa import Arm
from helm.components.cores import SmallOoOCore
from helm.components.memory import TypicalDesktop
from helm.devices.uart import Pl011
from helm.backends.char import backend_for, NullBackend


def arm_virt(*, serial: str = "stdio") -> Platform:
    """QEMU-style ARM virt machine.

    Minimal platform with PL011 UARTs on an APB bus and VirtIO MMIO
    slots for storage/network.
    """
    platform = Platform(
        name="arm-virt",
        isa=Arm(),
        cores=[SmallOoOCore("cortex-a72")],
        memory=TypicalDesktop(),
    )

    # UARTs
    platform.add_device(
        Pl011("uart0", backend=backend_for(serial), base_address=0x0900_0000, irq=33)
    )
    platform.add_device(
        Pl011("uart1", backend=NullBackend(), base_address=0x0900_1000, irq=34)
    )

    return platform
