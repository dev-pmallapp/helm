"""Base class for ISA descriptors."""

from __future__ import annotations


class IsaBase:
    """Abstract ISA descriptor.  Subclass per architecture."""

    kind: str = "unknown"

    def __repr__(self) -> str:
        return f"{self.__class__.__name__}()"
