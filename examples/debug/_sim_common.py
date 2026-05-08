"""Shared helpers for helm-ng debug scripts (examples/debug/).

Provides:
  - add_fs_sim_args(parser)   -- add standard FS-sim argparse arguments
  - build_fs_sim(args, ...)   -- build + load a simulation from parsed args
  - snapshot_regs(sim)        -- capture architectural register state
  - diff_regs(a, b)           -- compare two snapshots, return diff lines

All scripts that boot an AArch64 FS simulation should use these instead of
duplicating argument definitions and sim-setup code.
"""
from __future__ import annotations

import sys
from pathlib import Path


# ── Launcher guard ────────────────────────────────────────────────────────────

def require_helm_launcher() -> None:
    """Abort if not running under a helm launcher binary."""
    if getattr(sys, "_helm_launcher", None) not in {
        "helm-aarch64",
        "helm-system-aarch64",
    }:
        raise SystemExit(
            "Run via helm-system-aarch64, not directly via python."
        )


# ── Module import helpers ─────────────────────────────────────────────────────

def repo_root() -> Path:
    """Return the repository root (two levels above examples/debug/)."""
    if "__file__" in globals():
        return Path(__file__).resolve().parents[2]
    argv0 = Path(sys.argv[0])
    if argv0.is_absolute():
        return argv0.parents[2]
    return (Path.cwd() / argv0).resolve().parents[2]


def import_helm_ng():
    """Import the _helm_ng native extension from the active build."""
    root = repo_root()
    try:
        import _helm_ng
        return _helm_ng
    except ImportError:
        pass
    for build in ("release", "debug"):
        p = root / "target" / build / "lib_helm_ng.so"
        if p.is_file():
            import importlib.util
            spec = importlib.util.spec_from_file_location("_helm_ng", p)
            if spec and spec.loader:
                mod = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(mod)
                return mod
    import _helm_ng
    return _helm_ng


# ── Argparse helpers ──────────────────────────────────────────────────────────

def add_fs_sim_args(parser, *, default_mem_mib: int = 1024,
                    default_boot_el: int | None = None,
                    default_gic: str = "v2",
                    default_smp: int = 1) -> None:
    """Add standard FS-simulation arguments to an argparse.ArgumentParser.

    Covers: --kernel, --smp, --mem-mib, --boot-el, --gic-version, --cpu,
            --jit, --max-insns.
    """
    parser.add_argument(
        "--kernel",
        required=True,
        help="Path to the guest ELF/kernel image",
    )
    parser.add_argument(
        "--smp", type=int, default=default_smp,
        help=f"Number of vCPUs (default {default_smp})",
    )
    parser.add_argument(
        "--mem-mib", type=int, default=default_mem_mib,
        help=f"RAM size in MiB (default {default_mem_mib})",
    )
    parser.add_argument(
        "--boot-el", type=int, choices=(1, 2, 3), default=default_boot_el,
        help=f"Boot exception level (default {'auto' if default_boot_el is None else default_boot_el})",
    )
    parser.add_argument(
        "--gic-version", choices=("v2", "v3"), default=default_gic,
        help=f"GIC version (default {default_gic})",
    )
    parser.add_argument(
        "--cpu", default="cortex-a55",
        help="ARM core model (default cortex-a55)",
    )
    parser.add_argument(
        "--jit", action="store_true",
        help="Enable JIT execution",
    )
    parser.add_argument(
        "--max-insns", type=int, default=200_000_000,
        help="Maximum guest instructions to execute (default 200M)",
    )


# ── Simulation factory ────────────────────────────────────────────────────────

def build_fs_sim(args, *, helm_ng=None, plugins: list[tuple[str, str]] | None = None):
    """Build and load an AArch64 FS simulation from parsed args.

    Args:
        args:      Parsed argparse namespace (must include fields added by
                   add_fs_sim_args).
        helm_ng:   The _helm_ng module. If None, imported automatically.
        plugins:   Optional list of (name, args_str) tuples to install after
                   load_kernel. Applied before set_jit so plugins see JIT events.

    Returns the constructed sim object.
    """
    if helm_ng is None:
        helm_ng = import_helm_ng()

    sim = helm_ng.build_simulation(
        isa="aarch64",
        mode="fs",
        timing="virtual",
        mem_mib=args.mem_mib,
    )

    load_kwargs: dict = dict(
        kernel=args.kernel,
        gic_version=args.gic_version,
    )
    if getattr(args, "smp", 1) > 1:
        load_kwargs["num_cpus"] = args.smp
    boot_el = getattr(args, "boot_el", None)
    if boot_el is not None:
        load_kwargs["boot_el"] = boot_el

    sim.load_kernel(**load_kwargs)
    sim.set_cpu_model(args.cpu)

    for name, plugin_args in (plugins or []):
        sim.add_plugin(name, plugin_args)

    if getattr(args, "jit", False):
        sim.set_jit(True)

    return sim


# ── Register snapshot / diff ──────────────────────────────────────────────────

def snapshot_regs(sim) -> dict:
    """Return a dict of current architectural register values."""
    state: dict = {}
    for attr in ("pc", "nzcv", "current_el"):
        try:
            state[attr] = getattr(sim, attr)
        except AttributeError:
            pass
    try:
        state["sp"] = sim.current_sp
    except AttributeError:
        pass
    for i in range(31):
        try:
            state[f"x{i}"] = sim.xn(i)
        except (AttributeError, Exception):
            break
    return state


def diff_regs(a: dict, b: dict, label_a: str = "a", label_b: str = "b") -> list[str]:
    """Return human-readable lines for registers that differ between a and b."""
    lines = []
    for key in sorted(set(a) | set(b)):
        va = a.get(key)
        vb = b.get(key)
        if va != vb:
            if isinstance(va, int) and isinstance(vb, int):
                lines.append(f"  {key}: {label_a}={va:#x}  {label_b}={vb:#x}")
            else:
                lines.append(f"  {key}: {label_a}={va}  {label_b}={vb}")
    return lines
