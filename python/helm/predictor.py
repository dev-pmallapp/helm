"""
BranchPredictor — branch-predictor model selection.
"""

from __future__ import annotations


class BranchPredictor:
    """Factory for branch-predictor configurations.

    Use the class methods to create instances::

        bp = BranchPredictor.tage(history_length=64)
        bp = BranchPredictor.gshare(history_bits=16)
    """

    def __init__(self, kind: str, **params: int) -> None:
        self.kind = kind
        self.params = params

    # -- Factory class methods -----------------------------------------------

    @classmethod
    def static(cls) -> "BranchPredictor":
        """Always not-taken."""
        return cls("Static")

    @classmethod
    def bimodal(cls, table_size: int = 4096) -> "BranchPredictor":
        return cls("Bimodal", table_size=table_size)

    @classmethod
    def gshare(cls, history_bits: int = 16) -> "BranchPredictor":
        return cls("GShare", history_bits=history_bits)

    @classmethod
    def tage(cls, history_length: int = 64) -> "BranchPredictor":
        return cls("TAGE", history_length=history_length)

    @classmethod
    def tournament(cls) -> "BranchPredictor":
        return cls("Tournament")

    # -- Serialisation -------------------------------------------------------

    def to_dict(self) -> dict:
        if self.params:
            return {self.kind: self.params}
        return self.kind

    def __repr__(self) -> str:
        if self.params:
            p = ", ".join(f"{k}={v}" for k, v in self.params.items())
            return f"BranchPredictor.{self.kind.lower()}({p})"
        return f"BranchPredictor.{self.kind.lower()}()"
