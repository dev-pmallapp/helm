"""CPU type factory — maps short names to (Core, TimingModel) pairs.

Mirrors gem5's ``CpuConfig.py``.
"""

from helm.core import Core
from helm.predictor import BranchPredictor
from helm.timing import TimingModel


# ── CPU presets ──────────────────────────────────────────────────────────

_CPU_TYPES = {}


def _register(name, core_fn, timing_fn):
    _CPU_TYPES[name] = (core_fn, timing_fn)


# AtomicSimpleCPU equivalent — functional, IPC=1
_register(
    "atomic",
    lambda name: Core(name, width=1, rob_size=1, iq_size=1,
                       lq_size=1, sq_size=1,
                       branch_predictor=BranchPredictor.static()),
    lambda: TimingModel.fe(),
)

# TimingSimpleCPU equivalent — adds memory/cache timing
_register(
    "timing",
    lambda name: Core(name, width=1, rob_size=1, iq_size=1,
                       lq_size=8, sq_size=8,
                       branch_predictor=BranchPredictor.bimodal(2048)),
    lambda: TimingModel.ape(),
)

# MinorCPU equivalent — in-order pipelined with timing
_register(
    "minor",
    lambda name: Core(name, width=2, rob_size=1, iq_size=8,
                       lq_size=8, sq_size=8,
                       branch_predictor=BranchPredictor.bimodal(4096)),
    lambda: TimingModel.ape(
        int_alu_latency=1, int_mul_latency=3, int_div_latency=9,
        load_latency=3, store_latency=1, branch_penalty=6),
)

# DerivO3CPU equivalent — out-of-order superscalar
_register(
    "o3",
    lambda name: Core(name, width=4, rob_size=192, iq_size=64,
                       lq_size=32, sq_size=32,
                       branch_predictor=BranchPredictor.tournament()),
    lambda: TimingModel.ape(
        int_alu_latency=1, int_mul_latency=3, int_div_latency=12,
        fp_alu_latency=4, fp_mul_latency=5, fp_div_latency=15,
        load_latency=4, store_latency=1, branch_penalty=10,
        l1_latency=3, l2_latency=12, l3_latency=40, dram_latency=200),
)

# Wide aggressive core (like Apple M-series big core)
_register(
    "big",
    lambda name: Core(name, width=8, rob_size=630, iq_size=160,
                       lq_size=128, sq_size=72,
                       branch_predictor=BranchPredictor.tage(history_length=128)),
    lambda: TimingModel.ape(
        int_alu_latency=1, int_mul_latency=3, int_div_latency=10,
        fp_alu_latency=3, fp_mul_latency=4, fp_div_latency=12,
        load_latency=3, store_latency=1, branch_penalty=14,
        l1_latency=3, l2_latency=11, l3_latency=35, dram_latency=150),
)


# ── Public API ───────────────────────────────────────────────────────────

def cpu_factory(cpu_type: str, name: str = "cpu0"):
    """Return ``(core, timing_model)`` for the given CPU type name."""
    cpu_type = cpu_type.lower()
    if cpu_type not in _CPU_TYPES:
        raise ValueError(
            f"Unknown CPU type {cpu_type!r}. "
            f"Available: {', '.join(sorted(_CPU_TYPES))}"
        )
    core_fn, timing_fn = _CPU_TYPES[cpu_type]
    return core_fn(name), timing_fn()


def list_cpu_types():
    """Return list of registered CPU type names."""
    return sorted(_CPU_TYPES.keys())
