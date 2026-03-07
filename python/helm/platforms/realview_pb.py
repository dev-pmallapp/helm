"""ARM RealView Platform Baseboard for Cortex-A8.

Memory map per ARM DUI0417D. Devices are attached to the system bus
at their documented addresses with correct IRQ routing.
"""

from helm.platform import Platform
from helm.isa import Arm
from helm.components.cores import InOrderCore
from helm.components.memory import TypicalDesktop
from helm.devices.uart import Pl011
from helm.devices.timer import Sp804Timer
from helm.devices.gpio import Pl061Gpio
from helm.devices.interrupt import Gic
from helm.devices.watchdog import Sp805
from helm.devices.rtc import Pl031Rtc
from helm.devices.sysregs import RealViewSysRegs
from helm.backends.char import backend_for, NullBackend


def realview_pb(*, serial: str = "stdio", num_uarts: int = 4) -> Platform:
    """ARM RealView Platform Baseboard for Cortex-A8.

    Parameters
    ----------
    serial : str
        Backend for UART0: "stdio", "null", or "file".
    num_uarts : int
        Number of UARTs to instantiate (1-4).
    """
    platform = Platform(
        name="realview-pb-a8",
        isa=Arm(),
        cores=[InOrderCore("cortex-a8")],
        memory=TypicalDesktop(),
    )

    # System registers
    platform.add_device(RealViewSysRegs(base_address=0x1000_0000))

    # Timer
    platform.add_device(Sp804Timer("timer01", base_address=0x1000_1000, irq=36))

    # RTC
    platform.add_device(Pl031Rtc("rtc", base_address=0x1000_6000, irq=42))

    # UARTs
    uart_bases = [0x1000_9000, 0x1000_A000, 0x1000_B000, 0x1000_C000]
    uart_irqs = [44, 45, 46, 47]
    for i in range(min(num_uarts, 4)):
        be = backend_for(serial) if i == 0 else NullBackend()
        platform.add_device(
            Pl011(f"uart{i}", backend=be, base_address=uart_bases[i], irq=uart_irqs[i])
        )

    # Watchdog
    platform.add_device(Sp805("watchdog", base_address=0x1000_F000))

    # GPIO
    for i in range(3):
        platform.add_device(
            Pl061Gpio(f"gpio{i}", base_address=0x1001_3000 + i * 0x1000, irq=38 + i)
        )

    # GIC
    platform.add_device(Gic("gic", num_irqs=96, base_address=0x1F00_0000))

    return platform
