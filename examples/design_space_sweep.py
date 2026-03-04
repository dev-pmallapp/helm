#!/usr/bin/env python3
"""
Example: Sweep ROB sizes and branch predictors across ISAs.

Demonstrates how HELM's Python layer enables rapid design-space exploration.
"""

from helm import Platform, Core, Cache, MemorySystem, Simulation
from helm.predictor import BranchPredictor
from helm.isa import RiscV, X86, Arm

# Shared memory hierarchy
memory = MemorySystem(
    l1i=Cache("32KB", assoc=8, latency=1),
    l1d=Cache("32KB", assoc=8, latency=1),
    l2=Cache("256KB", assoc=4, latency=10),
    dram_latency=100,
)

# Sweep parameters
rob_sizes = [64, 128, 256]
predictors = [
    ("static", BranchPredictor.static()),
    ("bimodal-2k", BranchPredictor.bimodal(2048)),
    ("tage-64", BranchPredictor.tage(64)),
]
isas = [("riscv", RiscV()), ("x86", X86()), ("arm", Arm())]

print(f"{'ISA':<8} {'ROB':<6} {'Predictor':<14} {'IPC':>8} {'MPKI':>8}")
print("-" * 50)

for isa_name, isa in isas:
    for rob in rob_sizes:
        for bp_name, bp in predictors:
            core = Core(
                f"{isa_name}-{rob}-{bp_name}",
                width=4,
                rob_size=rob,
                iq_size=rob // 2,
                lq_size=32,
                sq_size=32,
                branch_predictor=bp,
            )
            platform = Platform(
                name=f"sweep-{isa_name}-{rob}-{bp_name}",
                isa=isa,
                cores=[core],
                memory=memory,
            )
            sim = Simulation(platform, binary="./benchmark", mode="microarch")
            r = sim.run()
            print(f"{isa_name:<8} {rob:<6} {bp_name:<14} {r.ipc:>8.3f} {r.branch_mpki:>8.2f}")
