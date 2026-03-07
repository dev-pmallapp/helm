"""Network backends — tap, buffer, null."""

from __future__ import annotations


class NetBackend:
    """Base class for network backends."""
    kind: str = "null"

    def to_dict(self) -> dict:
        return {"kind": self.kind}


class NullNetBackend(NetBackend):
    """Drop all packets."""
    kind = "null"


class BufferNetBackend(NetBackend):
    """In-memory packet buffer for testing."""
    kind = "buffer"
