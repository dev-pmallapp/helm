"""Cache presets — mirrors gem5's ``Caches.py``.

Each function returns a ``helm.memory.Cache`` with sensible defaults
for its level.  Users can override any parameter::

    l1d = L1DCache(size="64KB", assoc=4)
"""

from helm.memory import Cache


def L1ICache(size="32KB", *, assoc=4, latency=1, line_size=64) -> Cache:
    return Cache(size, assoc=assoc, latency=latency, line_size=line_size)


def L1DCache(size="64KB", *, assoc=4, latency=4, line_size=64) -> Cache:
    return Cache(size, assoc=assoc, latency=latency, line_size=line_size)


def L2Cache(size="256KB", *, assoc=8, latency=12, line_size=64) -> Cache:
    return Cache(size, assoc=assoc, latency=latency, line_size=line_size)


def L3Cache(size="8MB", *, assoc=16, latency=40, line_size=64) -> Cache:
    return Cache(size, assoc=assoc, latency=latency, line_size=line_size)
