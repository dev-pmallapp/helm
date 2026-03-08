#!/usr/bin/env python3
"""Run an AArch64 static binary in SE (syscall-emulation) mode.

Run with the ``helm-arm`` SE runner::

    helm-arm examples/se/aarch64/run_binary.py

Or with the embedded interpreter::

    helm-system-aarch64 examples/se/aarch64/run_binary.py

This script demonstrates Python-controlled SE execution:
  1. Create an SE session with a binary
  2. Run with pause/inspect/continue workflow
  3. Hot-load plugins mid-simulation
  4. Query registers and exit status
"""
import _helm_core
import os
import sys

# Embedded-Python stdout is fully buffered; force line-buffering so
# status messages appear interleaved with guest output.
sys.stdout.reconfigure(line_buffering=True)

# Default to the fish shell binary if it exists, otherwise prompt
BINARY = os.environ.get("HELM_BINARY", "assets/binaries/fish")
ARGV = [os.path.basename(BINARY), "--no-config", "-c", "echo hello"]
ENVP = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm"]

if not os.path.isfile(BINARY):
    print(f"[HelmPy] Binary not found: {BINARY}")
    print(f"[HelmPy] Set HELM_BINARY=/path/to/aarch64-elf to specify a binary")
    sys.exit(1)

print(f"[HelmPy] SE mode: {BINARY}")
print(f"[HelmPy]   argv: {ARGV}")

s = _helm_core.SeSession(BINARY, ARGV, ENVP)

# Phase 1: warm-up (1M instructions, no plugins)
result = s.run(1_000_000)
print(f"[HelmPy] Phase 1 (warm-up): PC={s.pc:#x}, insns={s.insn_count}")

# Phase 2: enable execlog plugin and continue
# s.add_plugin("execlog", "")  # uncomment to enable instruction logging
result = s.run(10_000_000)
print(f"[HelmPy] Phase 2: PC={s.pc:#x}, insns={s.insn_count}")

if s.has_exited:
    print(f"[HelmPy] Binary exited with code {s.exit_code}")
else:
    # Phase 3: run to completion
    result = s.run(100_000_000)
    if s.has_exited:
        print(f"[HelmPy] Binary exited with code {s.exit_code}")
    else:
        print(f"[HelmPy] Hit instruction limit at PC={s.pc:#x}")

# Show final register state
regs = s.regs()
print(f"[HelmPy] Final state: {s.insn_count} insns, {s.virtual_cycles} cycles")
s.finish()
