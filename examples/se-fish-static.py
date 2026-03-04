#!/usr/bin/env python3
"""
Run static fish-shell binary in HELM AArch64 SE mode.

Usage:
    helm-arm examples/se-fish-static.py
    helm-arm --max-insns 50000000 examples/se-fish-static.py
"""

import json
import sys

config = {
    "binary": "assets/binaries/fish",
    "argv": ["fish", "-c", "echo hello"],
    "envp": [
        "HOME=/tmp",
        "TERM=dumb",
        "PATH=/usr/bin:/bin",
        "LANG=C",
        "USER=helm",
    ],
    "max_insns": 50_000_000,
}

# Print JSON config to stdout for helm-arm to consume
print(json.dumps(config))
