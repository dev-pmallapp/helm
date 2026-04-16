"""
helm.debug -- JIT comparison and debugging utilities.

Reusable building blocks for JIT-vs-interpreter comparison, register
snapshotting, and divergence bisection.

Usage::

    from helm.debug import snapshot_regs, reg_diff, compare_lockstep

    result = compare_lockstep(sim_interp, sim_jit, max_insns=50_000_000)
    if result.diverged:
        print(result.diffs)
"""

from __future__ import annotations

import sys
import time
from dataclasses import dataclass, field
from typing import Optional

__all__ = [
    "snapshot_regs",
    "reg_diff",
    "compare_lockstep",
    "CompareResult",
    "COMPARE_REGS",
]

COMPARE_REGS = ["pc", "sp", "nzcv"] + [f"x{i}" for i in range(31)]


def snapshot_regs(sim) -> dict:
    """Capture current register values from a simulation instance."""
    return {
        r: getattr(sim, r, None)
        for r in COMPARE_REGS
        if getattr(sim, r, None) is not None
    }


def reg_diff(a: dict, b: dict) -> list[str]:
    """Return human-readable list of register differences between two snapshots."""
    diffs = []
    for k in sorted(set(a) | set(b)):
        va, vb = a.get(k), b.get(k)
        if va != vb:
            if isinstance(va, int) and isinstance(vb, int):
                diffs.append(f"  {k}: a={va:#018x}  b={vb:#018x}")
            else:
                diffs.append(f"  {k}: a={va}  b={vb}")
    return diffs


@dataclass
class CompareResult:
    """Result of a lockstep JIT-vs-interpreter comparison."""
    diverged: bool = False
    insns_checked: int = 0
    diffs: list[str] = field(default_factory=list)
    wall_seconds: float = 0.0
    stop_interp: str = "quantum"
    stop_jit: str = "quantum"


def compare_lockstep(
    sim_interp,
    sim_jit,
    max_insns: int = 50_000_000,
    *,
    interval: int = 500_000,
    progress: bool = True,
    label: str = "cmp",
) -> CompareResult:
    """Run interpreter and JIT side by side, checking registers periodically.

    Parameters
    ----------
    sim_interp
        Interpreter-mode simulation instance.
    sim_jit
        JIT-mode simulation instance (must have ``set_jit(True)`` called).
    max_insns : int
        Maximum instructions to compare.
    interval : int
        Register comparison interval (in instructions).
    progress : bool
        Print periodic progress to stderr.
    label : str
        Prefix for progress/error messages.

    Returns
    -------
    CompareResult
    """
    done = 0
    t0 = time.monotonic()

    while done < max_insns:
        n = min(interval, max_insns - done)
        r_i = sim_interp.run(n)
        r_j = sim_jit.run_jit(n)
        done += n

        diffs = reg_diff(snapshot_regs(sim_interp), snapshot_regs(sim_jit))
        if diffs:
            wall = time.monotonic() - t0
            if progress:
                print(f"\n[{label}] DIVERGENCE at ~{done:,} insns ({wall:.1f}s)",
                      file=sys.stderr)
                for d in diffs:
                    print(f"[{label}] {d}", file=sys.stderr)
            return CompareResult(
                diverged=True, insns_checked=done, diffs=diffs,
                wall_seconds=wall, stop_interp=r_i, stop_jit=r_j,
            )

        if r_i != "quantum" or r_j != "quantum":
            wall = time.monotonic() - t0
            if r_i != r_j and progress:
                print(f"[{label}] STOP MISMATCH: interp={r_i}  jit={r_j}",
                      file=sys.stderr)
                return CompareResult(
                    diverged=True, insns_checked=done,
                    diffs=[f"stop reason: interp={r_i}  jit={r_j}"],
                    wall_seconds=wall, stop_interp=r_i, stop_jit=r_j,
                )
            break

        wall = time.monotonic() - t0
        if progress and wall > 2.0 and done % (interval * 10) == 0:
            mips = done / wall / 1e6
            print(f"\r[{label}] {done / 1e6:.0f}M  {wall:.0f}s  {mips:.0f} MIPS",
                  end="", file=sys.stderr, flush=True)

    wall = time.monotonic() - t0
    if progress:
        print(f"\n[{label}] OK: {done:,} insns match ({wall:.1f}s)", file=sys.stderr)

    return CompareResult(
        diverged=False, insns_checked=done, wall_seconds=wall,
        stop_interp=r_i, stop_jit=r_j,
    )
