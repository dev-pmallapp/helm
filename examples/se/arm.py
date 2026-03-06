#!/usr/bin/env python3
"""
HELM SE-mode — AArch64.

Usage:
    helm-arm examples/se/arm.py -- --binary ./hello
    helm-arm examples/se/arm.py -- --binary ./fish --args '--no-config -c "echo hi"'
    helm-arm examples/se/arm.py -- --binary ./test --cpu-type o3 --l2cache
    helm-arm examples/se/arm.py -- --binary ./bench --cpu-type big --max-insns 100000000
"""

import argparse, json, os, shlex, sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "python"))

from helm.memory import Cache
from helm.timing import TimingModel

p = argparse.ArgumentParser(description="HELM SE — AArch64")
p.add_argument("--binary", "-b", required=True)
p.add_argument("--args", "-a", default="", help="guest arguments (quoted string)")
p.add_argument("--env", nargs="*", default=["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm"])
p.add_argument("--cpu-type", default="atomic", choices=["atomic", "timing", "minor", "o3", "big"])
p.add_argument("--caches", action=argparse.BooleanOptionalAction, default=True)
p.add_argument("--l2cache", action="store_true", default=False)
p.add_argument("--l1d-size", default="64KB")
p.add_argument("--l1i-size", default="32KB")
p.add_argument("--l2-size", default="256KB")
p.add_argument("--max-insns", type=int, default=50_000_000)
p.add_argument("--plugin", action="append", default=[])
args = p.parse_args()

# timing
_TIMING = {
    "atomic": lambda: TimingModel.fe(),
    "timing": lambda: TimingModel.ape(),
    "minor":  lambda: TimingModel.ape(int_mul_latency=3, load_latency=3, branch_penalty=6),
    "o3":     lambda: TimingModel.ape(int_mul_latency=3, int_div_latency=12, fp_alu_latency=4,
                                       load_latency=4, branch_penalty=10),
    "big":    lambda: TimingModel.ape(int_mul_latency=3, int_div_latency=10, fp_alu_latency=3,
                                       load_latency=3, branch_penalty=14),
}

# memory
mem = {}
if args.caches:
    mem["l1i"] = Cache(args.l1i_size, assoc=4, latency=1).to_dict()
    mem["l1d"] = Cache(args.l1d_size, assoc=4, latency=4).to_dict()
if args.l2cache:
    mem["l2"] = Cache(args.l2_size, assoc=8, latency=12).to_dict()
mem["dram_latency_cycles"] = 100

# argv
cmd = os.path.basename(args.binary)
argv = [cmd] + (shlex.split(args.args) if args.args else [])

print(json.dumps({
    "binary": args.binary,
    "argv": argv,
    "envp": args.env,
    "max_insns": args.max_insns,
    "platform": {
        "name": f"arm-{args.cpu_type}",
        "isa": "aarch64",
        "cores": [{"name": "cpu0"}],
        "memory": mem,
        "timing": _TIMING[args.cpu_type]().to_dict(),
    },
    "plugins": args.plugin,
}))
