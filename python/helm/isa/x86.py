"""x86-64 ISA descriptor."""

from helm.isa.base import IsaBase


class X86(IsaBase):
    """Select the x86-64 ISA frontend."""

    kind = "X86_64"
