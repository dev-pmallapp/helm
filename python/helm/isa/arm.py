"""AArch64 ISA descriptor."""

from helm.isa.base import IsaBase


class Arm(IsaBase):
    """Select the AArch64 ISA frontend."""

    kind = "Arm64"
