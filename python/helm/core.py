"""
Core — microarchitectural core configuration.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from helm.predictor import BranchPredictor


class Core:
    """Configurable out-of-order core model.

    Parameters
    ----------
    name : str
        Core label (e.g. ``"big-core"``).
    width : int
        Issue / dispatch width.
    rob_size : int
        Reorder-buffer capacity.
    iq_size : int
        Instruction-queue (scheduler) capacity.
    lq_size : int
        Load-queue capacity.
    sq_size : int
        Store-queue capacity.
    branch_predictor : BranchPredictor, optional
        Branch predictor configuration.  Defaults to static.
    """

    def __init__(
        self,
        name: str = "core",
        *,
        width: int = 4,
        rob_size: int = 128,
        iq_size: int = 64,
        lq_size: int = 32,
        sq_size: int = 32,
        branch_predictor: "Optional[BranchPredictor]" = None,
    ) -> None:
        self.name = name
        self.width = width
        self.rob_size = rob_size
        self.iq_size = iq_size
        self.lq_size = lq_size
        self.sq_size = sq_size

        from helm.predictor import BranchPredictor
        self.branch_predictor = branch_predictor or BranchPredictor.static()

    def to_dict(self) -> dict:
        return {
            "name": self.name,
            "width": self.width,
            "rob_size": self.rob_size,
            "iq_size": self.iq_size,
            "lq_size": self.lq_size,
            "sq_size": self.sq_size,
            "branch_predictor": self.branch_predictor.to_dict(),
        }

    def __repr__(self) -> str:
        return (
            f"Core({self.name!r}, width={self.width}, "
            f"rob={self.rob_size}, bp={self.branch_predictor})"
        )
