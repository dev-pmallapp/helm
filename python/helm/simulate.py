"""
helm.simulate -- run-control API for helm-ng simulations.

Equivalent to gem5's ``m5.simulate``: wraps the low-level ``sim.run()``
with progress reporting, interrupt handling, and stop-reason dispatch.

Usage::

    from helm.simulate import simulate, SimResult

    result = simulate(sim, max_insns=100_000_000)
    print(result.insns, result.stop_reason, result.wall_seconds)
"""

from __future__ import annotations

import signal
import sys
import time
from dataclasses import dataclass, field
from typing import Optional

__all__ = ["simulate", "SimResult"]


@dataclass
class SimResult:
    """Result of a simulate() call."""
    insns: int = 0
    stop_reason: str = "quantum"
    wall_seconds: float = 0.0
    mips: float = 0.0
    exit_code: Optional[int] = None
    interrupted: bool = False


def simulate(
    sim,
    max_insns: int = 100_000_000,
    *,
    chunk: int = 50_000_000,
    jit: bool = False,
    progress: bool = True,
    progress_interval: float = 5.0,
) -> SimResult:
    """Run a simulation with progress reporting and Ctrl-C handling.

    Parameters
    ----------
    sim
        A helm System/Simulation instance (has ``.run()``, ``.insn_count``, etc.).
    max_insns : int
        Maximum guest instructions to execute.
    chunk : int
        Instructions per ``run()`` call (controls progress granularity).
    jit : bool
        Use ``run_jit()`` instead of ``run()``.
    progress : bool
        Print periodic progress to stderr.
    progress_interval : float
        Seconds between progress lines.

    Returns
    -------
    SimResult
        Dataclass with insns, stop_reason, wall time, MIPS, etc.
    """
    run_fn = sim.run_jit if jit else sim.run
    remaining = max_insns
    stop = "quantum"
    interrupted = False
    last_progress = 0.0

    # Ctrl-C handling
    _sigint_fired = False

    def _sigint_handler(_signum, _frame):
        nonlocal _sigint_fired
        _sigint_fired = True

    old_handler = signal.getsignal(signal.SIGINT)
    signal.signal(signal.SIGINT, _sigint_handler)

    t0 = time.monotonic()
    try:
        while remaining > 0 and not sim.has_exited:
            if _sigint_fired:
                interrupted = True
                stop = "interrupt"
                break
            n = min(chunk, remaining)
            stop = run_fn(n)
            remaining -= n
            if stop != "quantum":
                break
            wall = time.monotonic() - t0
            if progress and wall > 2.0 and wall - last_progress >= progress_interval:
                last_progress = wall
                mips = sim.insn_count / wall / 1e6
                print(
                    f"\r[helm] {sim.insn_count / 1e6:.0f}M insns  "
                    f"{wall:.0f}s  {mips:.0f} MIPS",
                    end="",
                    file=sys.stderr,
                    flush=True,
                )
    except KeyboardInterrupt:
        interrupted = True
        stop = "interrupt"
    finally:
        signal.signal(signal.SIGINT, old_handler)

    wall = time.monotonic() - t0
    if progress and wall > 2.0:
        print(file=sys.stderr)  # newline after progress

    result = SimResult(
        insns=sim.insn_count,
        stop_reason=stop,
        wall_seconds=wall,
        mips=sim.insn_count / wall / 1e6 if wall > 0.001 else 0.0,
        exit_code=sim.exit_code if sim.has_exited else None,
        interrupted=interrupted,
    )
    return result
