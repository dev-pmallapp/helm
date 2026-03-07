"""Character backends — stdio, file, buffer, null."""

from __future__ import annotations


class CharBackend:
    """Base class for character backends (UART, console, etc.)."""
    kind: str = "null"

    def to_dict(self) -> dict:
        return {"kind": self.kind}


class StdioBackend(CharBackend):
    """Connect to host stdin/stdout."""
    kind = "stdio"


class NullBackend(CharBackend):
    """Discard all output, never produce input."""
    kind = "null"


class FileBackend(CharBackend):
    """Log output to a file."""
    kind = "file"

    def __init__(self, path: str) -> None:
        self.path = path

    def to_dict(self) -> dict:
        return {"kind": self.kind, "path": self.path}


class BufferBackend(CharBackend):
    """In-memory buffer for testing."""
    kind = "buffer"

    def __init__(self, input_data: bytes = b"") -> None:
        self.input_data = input_data

    def to_dict(self) -> dict:
        return {"kind": self.kind, "input": self.input_data.hex()}


def backend_for(name: str, **kwargs) -> CharBackend:
    """Create a backend by name."""
    backends = {
        "stdio": StdioBackend,
        "null": NullBackend,
        "file": FileBackend,
        "buffer": BufferBackend,
    }
    cls = backends.get(name, NullBackend)
    return cls(**kwargs) if kwargs else cls()
