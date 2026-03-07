"""Block backends — file, memory."""

from __future__ import annotations


class BlockBackend:
    """Base class for block backends (disk images, etc.)."""
    kind: str = "memory"

    def to_dict(self) -> dict:
        return {"kind": self.kind}


class FileBlockBackend(BlockBackend):
    """File-backed block device (raw image)."""
    kind = "file"

    def __init__(self, path: str, readonly: bool = False) -> None:
        self.path = path
        self.readonly = readonly

    def to_dict(self) -> dict:
        return {"kind": self.kind, "path": self.path, "readonly": self.readonly}


class MemoryBlockBackend(BlockBackend):
    """In-memory block device."""
    kind = "memory"

    def __init__(self, size: int = 0) -> None:
        self.size = size

    def to_dict(self) -> dict:
        return {"kind": self.kind, "size": self.size}
