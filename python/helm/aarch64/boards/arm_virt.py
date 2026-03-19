"""ArmVirt — QEMU-compatible ARM virt platform board.

Pre-configured with GICv2, PL011 UART, and standard address map.
"""

import _helm_ng


class ArmVirt:
    """Pre-built ARM virt board (QEMU-compatible address map).

    Parameters
    ----------
    mem : str
        RAM size (default "512MiB").
    cpu_model : str
        ARM core model (default "cortex-a55").
    timing : str
        Timing model: "virtual", "interval", "accurate" (default "virtual").

    Usage::

        from helm.aarch64.boards import ArmVirt

        board = ArmVirt(mem="1GiB", cpu_model="cortex-a55")
        board.system.load_kernel("Image", dtb="virt.dtb")
        board.system.run(10_000_000)
    """

    # QEMU-compatible address constants
    GIC_DIST = 0x0800_0000
    GIC_CPUIF = 0x0801_0000
    UART0 = 0x0900_0000
    RAM_BASE = 0x4000_0000

    def __init__(self, mem="512MiB", cpu_model="cortex-a55", timing="virtual"):
        self.system = _helm_ng.System("virt", timing=timing, mode="fs")
        self.system.cpu = _helm_ng.Cpu("cpu0", isa="aarch64", model=cpu_model)
        self.system.gic = _helm_ng.GicV2("gic0", num_irqs=96)
        self.system.uart = _helm_ng.Pl011("uart0")
        self.system.ram = _helm_ng.Ram("ram0", size=mem)
        self.system.mem = _helm_ng.MemorySpace("phys_mem")
        self.system.mem.add_map(self.RAM_BASE, self.system.ram, 0x4000_0000)
        self.system.mem.add_map(self.GIC_DIST, self.system.gic, 0x1_0000, bank=0)
        self.system.mem.add_map(self.GIC_CPUIF, self.system.gic, 0x1_0000, bank=1)
        self.system.mem.add_map(self.UART0, self.system.uart, 0x1000)
