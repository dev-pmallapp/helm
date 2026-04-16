"""
helm.stats -- simulation statistics formatting and output.

Equivalent to gem5's ``m5.stats``: reads the stats dict from a finished
simulation and formats it as gem5-compatible text, JSON, or CSV.

Usage::

    from helm.stats import dump_stats, format_stats

    sim.finish()
    dump_stats(sim, stream=sys.stderr)
    text = format_stats(sim)
"""

from __future__ import annotations

import json
import sys
import time
from typing import IO, Optional

__all__ = ["dump_stats", "format_stats", "format_stats_json"]


def _stat_line(name: str, value, comment: str = "") -> str:
    comment_part = "" if not comment else f"  # {comment}"
    return f"{name:<40}{value:>20}{comment_part}"


def format_stats(sim, wall: Optional[float] = None) -> str:
    """Format simulation statistics as gem5-style text.

    Parameters
    ----------
    sim
        A helm System/Simulation instance with ``.stats()`` and ``.insn_count``.
    wall : float, optional
        Wall-clock seconds.  If omitted, stats-only fields are emitted.

    Returns
    -------
    str
        Multi-line statistics block.
    """
    stats = sim.stats()
    insns = int(stats.get("insn_count", sim.insn_count))
    ticks = int(stats.get("tick_count", stats.get("virtual_cycles", 0)))
    freq = int(stats.get("sim_freq", 1_000_000_000))
    ipc = float(stats.get("ipc", (insns / ticks) if ticks else 0.0))
    sim_seconds = ticks / freq if freq > 0 else 0.0

    lines = []
    lines.append("---------- Begin Simulation Statistics ----------")
    lines.append(_stat_line("sim_insns", insns, "Instructions retired"))
    lines.append(_stat_line("sim_ticks", ticks, "Simulated cycles/ticks"))
    lines.append(_stat_line("sim_seconds", f"{sim_seconds:.6f}", "Simulated time"))
    lines.append(_stat_line("system.cpu.committedInsts", insns, "Committed instructions"))
    lines.append(_stat_line("system.cpu.ipc", f"{ipc:.6f}", "Instructions per tick"))
    lines.append(_stat_line("system.final_pc", f"{sim.pc:#018x}", "Final PC"))

    if wall is not None and wall > 0.001:
        host_mips = insns / wall / 1e6
        lines.append(_stat_line("host_seconds", f"{wall:.6f}", "Wall clock runtime"))
        lines.append(_stat_line("host_mips", f"{host_mips:.3f}", "Million insts/sec"))

    lines.append("----------  End Simulation Statistics  ----------")
    return "\n".join(lines)


def dump_stats(
    sim,
    wall: Optional[float] = None,
    stream: IO = sys.stderr,
) -> None:
    """Print simulation statistics to a stream.

    Parameters
    ----------
    sim
        A helm System/Simulation instance.
    wall : float, optional
        Wall-clock seconds.
    stream
        Output stream (default: stderr).
    """
    print(format_stats(sim, wall), file=stream)


def format_stats_json(sim, wall: Optional[float] = None) -> str:
    """Format simulation statistics as JSON.

    Returns a compact JSON object with all stat keys.
    """
    stats = sim.stats()
    insns = int(stats.get("insn_count", sim.insn_count))
    ticks = int(stats.get("tick_count", stats.get("virtual_cycles", 0)))
    freq = int(stats.get("sim_freq", 1_000_000_000))

    out = {
        "sim_insns": insns,
        "sim_ticks": ticks,
        "sim_freq": freq,
        "sim_seconds": ticks / freq if freq > 0 else 0.0,
        "ipc": insns / ticks if ticks else 0.0,
        "final_pc": sim.pc,
    }
    if wall is not None:
        out["host_seconds"] = wall
        out["host_mips"] = insns / wall / 1e6 if wall > 0.001 else 0.0
    # Include any extra keys from the engine stats dict
    for k, v in stats.items():
        if k not in out:
            out[k] = v
    return json.dumps(out, indent=2)
