# Cache Model

Set-associative cache simulation in `helm-memory::cache`.

## Structure

- `Cache` — set-associative cache with configurable size, associativity,
  line size, and latency.
- `CacheSet` — one set containing `associativity` cache lines.
- `CacheLine` — `(tag, valid, dirty)`.

## Configuration

Via `CacheConfig`:

```rust
CacheConfig {
    size: "32KB",
    associativity: 8,
    latency_cycles: 4,
    line_size: 64,
}
```

Size strings are parsed (e.g. "32KB" → 32768 bytes). Number of sets
is computed as `total_bytes / (line_size × associativity)`.

## Access

`Cache::access(addr, is_write)` returns `Hit` or `Miss`:

1. Compute `set_idx = (addr >> offset_bits) % num_sets`.
2. Compute `tag = addr >> (offset_bits + set_index_bits)`.
3. Search the set for a matching valid line.
4. On miss: evict the first invalid line or the last line (LRU stub).

## MemorySubsystem

Assembles L1i, L1d, L2, L3 caches and DRAM latency from a
`MemoryConfig`. Each level is optional.

## Coherence

`CoherenceController` is a MOESI-style stub with a directory of
`(addr, state, sharers)` entries. States: Modified, Owned, Exclusive,
Shared, Invalid. Full protocol implementation is future work.
