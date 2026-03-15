"""Core — CPU core configuration."""

from __future__ import annotations


class Core:
    """Configurable CPU core model.

    Parameters
    ----------
    name : str
        Core label.
    width : int
        Issue / dispatch width.
    rob_size : int
        Reorder-buffer capacity.
    """

    def __init__(
        self,
        name: str = "core-0",
        *,
        width: int = 4,
        rob_size: int = 128,
        iq_size: int = 64,
        lq_size: int = 32,
        sq_size: int = 32,
    ) -> None:
        self.name     = name
        self.width    = width
        self.rob_size = rob_size
        self.iq_size  = iq_size
        self.lq_size  = lq_size
        self.sq_size  = sq_size

    def to_dict(self) -> dict:
        return {
            "name":     self.name,
            "width":    self.width,
            "rob_size": self.rob_size,
            "iq_size":  self.iq_size,
            "lq_size":  self.lq_size,
            "sq_size":  self.sq_size,
        }

    def __repr__(self) -> str:
        return f"Core({self.name!r}, width={self.width}, rob={self.rob_size})"
