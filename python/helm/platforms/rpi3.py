"""Raspberry Pi 3 (BCM2837) platform.

Memory map per BCM2835 ARM Peripherals documentation.
"""

from helm.platform import Platform
from helm.isa import Arm
from helm.components.cores import InOrderCore
from helm.components.memory import TypicalDesktop
from helm.devices.uart import Pl011, BcmMiniUart
from helm.devices.timer import BcmSysTimer
from helm.devices.gpio import BcmGpio
from helm.devices.mailbox import BcmMailbox
from helm.backends.char import backend_for, NullBackend


def rpi3(*, serial: str = "stdio") -> Platform:
    """Raspberry Pi 3 Model B.

    Parameters
    ----------
    serial : str
        Backend for UART0: "stdio", "null", or "file".
    """
    platform = Platform(
        name="rpi3",
        isa=Arm(),
        cores=[InOrderCore(f"cortex-a53-{i}") for i in range(4)],
        memory=TypicalDesktop(),
    )

    # System timer
    platform.add_device(BcmSysTimer("sys-timer", base_address=0x3F00_3000))

    # Mailbox
    platform.add_device(BcmMailbox("mailbox", base_address=0x3F00_B880))

    # GPIO
    platform.add_device(BcmGpio("gpio", base_address=0x3F20_0000))

    # PL011 UART0 (full UART)
    platform.add_device(
        Pl011("uart0", backend=backend_for(serial), base_address=0x3F20_1000, irq=57)
    )

    # Mini UART (UART1)
    platform.add_device(
        BcmMiniUart("uart1", backend=NullBackend(), base_address=0x3F21_5000, irq=29)
    )

    return platform
