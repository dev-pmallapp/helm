"""AArch64 CPU model classes — thin Python wrappers over helm.Cpu."""

from _helm_ng import Cpu


class A53Cpu(Cpu):
    """Cortex-A53: in-order, 2-wide, small core (ARMv8.0)."""

    def __init__(self, name, **kwargs):
        super().__init__(name, isa="aarch64", model="cortex-a53", **kwargs)


class A55Cpu(Cpu):
    """Cortex-A55: in-order, 2-wide, small core."""

    def __init__(self, name, **kwargs):
        super().__init__(name, isa="aarch64", model="cortex-a55", **kwargs)


class A73Cpu(Cpu):
    """Cortex-A73: OoO, 2-wide, big core (ARMv8.0-A)."""

    def __init__(self, name, **kwargs):
        super().__init__(name, isa="aarch64", model="cortex-a73", **kwargs)


class A78Cpu(Cpu):
    """Cortex-A78: OoO, 4-wide, big core (ARMv8.2-A)."""

    def __init__(self, name, **kwargs):
        super().__init__(name, isa="aarch64", model="cortex-a78", **kwargs)


class A510Cpu(Cpu):
    """Cortex-A510: in-order, 2-wide, small core (ARMv9.0-A)."""

    def __init__(self, name, **kwargs):
        super().__init__(name, isa="aarch64", model="cortex-a510", **kwargs)


class A710Cpu(Cpu):
    """Cortex-A710: OoO, 4-wide, big core (ARMv9.0-A)."""

    def __init__(self, name, **kwargs):
        super().__init__(name, isa="aarch64", model="cortex-a710", **kwargs)


class NeoverseN1Cpu(Cpu):
    """Neoverse N1: server-class OoO core (ARMv8.2-A)."""

    def __init__(self, name, **kwargs):
        super().__init__(name, isa="aarch64", model="neoverse-n1", **kwargs)
