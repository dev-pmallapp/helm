"""RISC-V 64-bit ISA descriptor."""

from helm.isa.base import IsaBase


class RiscV(IsaBase):
    """Select the RISC-V 64 ISA frontend."""

    kind = "RiscV64"
