#!/usr/bin/env python3
"""Syscall-emulation mode configuration — like gem5's configs/example/se.py.

Usage::

    python -m helm.configs.se --binary ./hello
    python -m helm.configs.se --binary ./test --timing ape --max-insns 1000000
"""

from __future__ import annotations

import argparse
import json


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="HELM Syscall-Emulation Simulation")
    p.add_argument("--binary", required=True, help="Path to guest binary")
    p.add_argument("--args", nargs="*", default=[], help="Arguments for the binary")
    p.add_argument("--env", nargs="*", default=[], help="Environment variables")
    p.add_argument(
        "--timing", default="fe", choices=["fe", "ape", "cae"],
        help="Timing model (default: fe)",
    )
    p.add_argument("--max-insns", type=int, default=1_000_000, help="Max instructions")
    p.add_argument("--backend", default="interp", choices=["interp", "tcg"],
                    help="Execution backend (default: interp)")
    p.add_argument("--dump-config", action="store_true",
                    help="Print config JSON and exit")
    return p


def main(argv: list[str] = None) -> None:
    args = build_parser().parse_args(argv)

    config = {
        "mode": "se",
        "binary": args.binary,
        "argv": [args.binary] + args.args,
        "envp": args.env,
        "timing": args.timing,
        "backend": args.backend,
        "max_insns": args.max_insns,
    }

    if args.dump_config:
        print(json.dumps(config, indent=2))
        return

    print(f"[HELM] SE mode: {args.binary}")
    print(f"[HELM] Timing: {args.timing}, Backend: {args.backend}")

    try:
        from helm._helm_core import run_se, TimingModel, PluginManager

        timing = None
        if args.timing != "fe":
            timing = TimingModel(args.timing)

        result = run_se(
            args.binary,
            config["argv"],
            config["envp"],
            args.max_insns,
            timing=timing,
        )
        print(f"[HELM] {result}")
    except ImportError:
        print("[HELM] Native engine not available — dry run only.")
        print(f"[HELM] Config: {json.dumps(config, indent=2)}")


if __name__ == "__main__":
    main()
