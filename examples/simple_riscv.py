#!/usr/bin/env python3
"""
Example: Single-core RISC-V exploration with HELM.

Run with:
    python examples/simple_riscv.py
"""

from helm import Platform, Core, Cache, MemorySystem, Simulation
from helm.predictor import BranchPredictor
from helm.isa import RiscV

# 1. Define a core
core = Core(
    "rv-ooo",
    width=4,
    rob_size=128,
    iq_size=64,
    lq_size=32,
    sq_size=32,
    branch_predictor=BranchPredictor.tage(history_length=64),
)

# 2. Define the memory hierarchy
memory = MemorySystem(
    l1i=Cache("32KB", assoc=8, latency=1),
    l1d=Cache("32KB", assoc=8, latency=1),
    l2=Cache("256KB", assoc=4, latency=10),
    l3=Cache("8MB", assoc=16, latency=30),
    dram_latency=100,
)

# 3. Assemble the platform
platform = Platform(
    name="riscv-exploration",
    isa=RiscV(),
    cores=[core],
    memory=memory,
)

print(f"Platform: {platform}")
print(f"Core:     {core}")
print(f"Memory:   {memory}")

# 4. Run the simulation
sim = Simulation(platform, binary="./test_binary", mode="microarch")
results = sim.run()
print(f"\nResults:  {results}")
