"""ARM core presets modeled after real Cortex-A series.

Each function returns a ``(Core, TimingModel)`` pair.
"""

from helm.core import Core
from helm.predictor import BranchPredictor
from helm.timing import TimingModel


def CortexA53(name: str = "a53"):
    """Cortex-A53 — 2-wide in-order, LITTLE core."""
    core = Core(
        name, width=2, rob_size=1, iq_size=8,
        lq_size=4, sq_size=4,
        branch_predictor=BranchPredictor.bimodal(512),
    )
    timing = TimingModel.ape(
        int_alu_latency=1, int_mul_latency=3, int_div_latency=12,
        fp_alu_latency=4, fp_mul_latency=5, fp_div_latency=15,
        load_latency=3, store_latency=1, branch_penalty=8,
        l1_latency=2, l2_latency=10, dram_latency=100,
    )
    return core, timing


def CortexA72(name: str = "a72"):
    """Cortex-A72 — 3-wide OoO, big core."""
    core = Core(
        name, width=3, rob_size=128, iq_size=60,
        lq_size=16, sq_size=16,
        branch_predictor=BranchPredictor.tournament(),
    )
    timing = TimingModel.ape(
        int_alu_latency=1, int_mul_latency=3, int_div_latency=12,
        fp_alu_latency=3, fp_mul_latency=5, fp_div_latency=15,
        load_latency=4, store_latency=1, branch_penalty=11,
        l1_latency=3, l2_latency=11, l3_latency=30, dram_latency=120,
    )
    return core, timing


def CortexA76(name: str = "a76"):
    """Cortex-A76 — 4-wide OoO, modern big core."""
    core = Core(
        name, width=4, rob_size=160, iq_size=120,
        lq_size=64, sq_size=36,
        branch_predictor=BranchPredictor.tage(history_length=64),
    )
    timing = TimingModel.ape(
        int_alu_latency=1, int_mul_latency=3, int_div_latency=10,
        fp_alu_latency=3, fp_mul_latency=4, fp_div_latency=12,
        load_latency=4, store_latency=1, branch_penalty=11,
        l1_latency=3, l2_latency=9, l3_latency=30, dram_latency=100,
    )
    return core, timing


def NeoVerseN1(name: str = "n1"):
    """Neoverse N1 — server-class Armv8.2-A core."""
    core = Core(
        name, width=4, rob_size=128, iq_size=120,
        lq_size=64, sq_size=36,
        branch_predictor=BranchPredictor.tage(history_length=64),
    )
    timing = TimingModel.ape(
        int_alu_latency=1, int_mul_latency=3, int_div_latency=12,
        fp_alu_latency=3, fp_mul_latency=5, fp_div_latency=15,
        load_latency=4, store_latency=1, branch_penalty=11,
        l1_latency=3, l2_latency=9, l3_latency=30, dram_latency=150,
    )
    return core, timing
