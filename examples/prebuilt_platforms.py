#!/usr/bin/env python3
"""
Example: Using pre-built platform templates from helm.components.
"""

from helm.components.platforms import SingleCoreRiscV, QuadCoreX86, DualCoreArm
from helm.simulation import Simulation
import json

platforms = [
    SingleCoreRiscV(),
    QuadCoreX86(),
    DualCoreArm(),
]

for p in platforms:
    print(f"\n{'='*60}")
    print(f"Platform: {p.name}")
    print(json.dumps(p.to_dict(), indent=2))

    sim = Simulation(p, binary="./workload", mode="se")
    results = sim.run()
    print(f"Results: {results}")
