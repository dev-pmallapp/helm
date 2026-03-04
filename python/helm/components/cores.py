"""Pre-built core configurations."""

from helm.core import Core
from helm.predictor import BranchPredictor


def InOrderCore(name: str = "in-order") -> Core:
    """A simple single-issue in-order core approximation."""
    return Core(
        name,
        width=1,
        rob_size=1,
        iq_size=1,
        lq_size=8,
        sq_size=8,
        branch_predictor=BranchPredictor.static(),
    )


def SmallOoOCore(name: str = "small-ooo") -> Core:
    """A modest 2-wide out-of-order core."""
    return Core(
        name,
        width=2,
        rob_size=64,
        iq_size=32,
        lq_size=16,
        sq_size=16,
        branch_predictor=BranchPredictor.bimodal(2048),
    )


def BigOoOCore(name: str = "big-ooo") -> Core:
    """An aggressive wide-issue OoO core."""
    return Core(
        name,
        width=8,
        rob_size=256,
        iq_size=128,
        lq_size=64,
        sq_size=64,
        branch_predictor=BranchPredictor.tage(history_length=128),
    )
