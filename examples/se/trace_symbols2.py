#!/usr/bin/env python3
"""Use insn_exec callback to detect when key symbols are entered."""
import os, sys, time

def _require_helm_launcher() -> None:
    if getattr(sys, "_helm_launcher", None) not in {"helm-aarch64", "helm-system-aarch64"}:
        raise SystemExit(
            "This example must be run via helm-aarch64 or helm-system-aarch64, not directly via python."
        )

_require_helm_launcher()

sys.stdout.reconfigure(line_buffering=True)
import _helm_ng

binary = os.environ.get("HELM_BINARY", "assets/binaries/fish")
argv = ["fish", "-N", "-c", "echo hello"]
envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm"]

sim = _helm_ng.build_simulation(isa="aarch64", mode="se", timing="virtual")
sim.load_elf(binary, argv, envp)

# Key addresses to watch — resolve from symbol table
watch = {}
for name, addr, sz in sim.symbols():
    for t in ["_start_c", "__libc_start_main", "fish4main", "fish13throwing_main",
              "__init_libc", "__init_tls", "exit_group"]:
        if t in name and addr not in watch:
            watch[addr] = name[:80]

# Also add known addresses from objdump
watch[0x411120] = "_start"
watch[0x41113c] = "_start_c"
watch[0x696230] = "__libc_start_main"
watch[0x695fdc] = "__init_libc"
watch[0x6a44d8] = "__init_tls"
watch[0x419a60] = "main"
watch[0x4193cc] = "fish::main"
watch[0x41343c] = "fish::throwing_main"

print(f"Watching {len(watch)} entry points via insn callback", file=sys.stderr)

# Use trace_after(insn_count=0) to activate immediately, watch all insns
# but only print when hitting watched PCs
sim.trace_after(insn_count=1, events=["insn"], max=0)  # max=0 → no trace output

# Install a custom insn callback that checks against our watchlist
# We can't do this directly from Python, so let's use execlog with max=0 for overhead,
# and just run shorter with strace to see if main is called

sim.add_plugin("syscall-trace")
t0 = time.monotonic()
# Run first 100K insns and check
for limit in [1000, 5000, 10000, 50000, 100000]:
    if sim.has_exited:
        break
    target = limit - sim.insn_count
    if target <= 0:
        continue
    sim.run(target)
    pc = sim.pc
    if pc in watch:
        print(f"  [{sim.insn_count:>10,}] AT {watch[pc]} @ {pc:#x}", file=sys.stderr)

# Now check: where is PC at key insn counts?
for milestone in [200000, 500000, 1000000, 5000000]:
    if sim.has_exited:
        break
    target = milestone - sim.insn_count
    if target <= 0:
        continue
    sim.run(target)
    pc = sim.pc
    sym = watch.get(pc, "???")
    print(f"  [{sim.insn_count:>10,}] PC={pc:#x} ({sym})", file=sys.stderr)

wall = time.monotonic() - t0
if sim.has_exited:
    print(f"\nExited code={sim.exit_code} at {sim.insn_count:,} insns ({wall:.1f}s)", file=sys.stderr)
else:
    print(f"\nRunning at PC={sim.pc:#x} at {sim.insn_count:,} insns ({wall:.1f}s)", file=sys.stderr)

# Look up PC in symbol table
pc = sim.pc
closest = None
for name, addr, sz in sim.symbols():
    if addr <= pc and (closest is None or addr > closest[1]):
        closest = (name, addr, sz)
if closest:
    print(f"  Current function: {closest[0]} @ {closest[1]:#x} (offset +{pc-closest[1]:#x})", file=sys.stderr)

sim.finish()
