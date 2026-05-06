# CMEM 2026-05-04 -- Follow-ups L12-L16 (VirtIO console/RNG, per-region MemStats, IOMMU, descriptor counters, config.ini params)

**Session:** 019dd683-b83c-7690-b67e-01d197018dbf (resumed; prior
CMEMs in this chain are `CMEM-20260504-stats-followup-l1-l2-l3.md`,
`CMEM-20260504-stats-followup-l4-l5-l6.md`, and
`CMEM-20260504-stats-followup-l7-l11.md`).
**Branch:** `main`
**Commits (oldest first):**
- `c0c7bb5 feat(stats): VirtIO console/RNG IoStats producers per device`
- `a0448d5 feat(stats): per-region FlatMem fan-out under system.mem.region<N>.*`
- `2a77786 feat(stats): SMMUv3 IommuStats producer at system.iommu.smmu.*`
- `701dace feat(stats): VirtIO descriptor counter on IoStats producer`
- `77c99fa feat(stats): per-SimObject parameter sections in config.ini`

## Outcome

- **L12 (VirtIO console/RNG):** VirtioConsole and VirtioRng now
  carry `stats: IoStats` and bump tx_bytes/rx_bytes/requests/
  completions from their hot paths. helm-python instantiate
  registers each transport at
  `system.virtio.{console,rng,rng_mmio}_<bus>_<slot>_<func>`.
- **L13 (per-region FlatMem):** FlatMem grows region_stats: Vec<MemStats>
  sized to match its mapped regions; engine fans out
  `system.mem.region<N>.{loads,stores,bytes_read,bytes_written}`
  on first stats_registry() borrow.
- **L14 (SMMUv3 IommuStats):** New IommuStats producer in
  helm-stats with translations / tlb_hits / tlb_misses / faults
  slots. SmmuState carries `pub stats: IommuStats`; translate()
  bumps translations on entry, tlb_hits on a TLB hit,
  tlb_misses on the walk path, and record_fault() bumps faults.
  Engine wires it at system.iommu.smmu via
  platform::arm_virt::smmu_iommu_stats() (the helper isolates
  the LiveFlatMemByteMem adapter type from the engine root).
- **L15 (descriptor counter):** IoStats grows
  `pub descriptors: PerfCounter`; blk/net/console/rng all bump
  it once per `collect_chain` so callers can derive average chain
  depth from descriptors / requests. Per-queue fan-out (one
  IoStats per virtqueue) stays deferred.
- **L16 (config.ini params):** New
  helm_report::format::emit_config_ini_with_params writer entry
  point preseeds INI sections with `type = <class>` and
  pre-rendered (leaf, value) pairs. Existing emit_config_ini
  delegates to it with an empty params slice. helm-python adds
  config_ini::collect_params which walks HelmSystem.children,
  emits parameter rows per known pyclass (Cpu, Ram, GicV2,
  Pl011, PciRamBar, PciVirtioRng/RngMmio, PciVirtioBlk/Net/
  Console, MemorySpace, Cache), and stub rows for unknown
  SimObject subclasses. dump_stats was retyped to take
  `Py<Self>` so it can borrow the SimObject base for the param
  walk before grabbing the mut borrow needed for stats_registry().

## Verification

- `cargo build --workspace` -- clean.
- `cargo test -p helm-stats --features stats --tests` -- 27/27.
- `cargo test -p helm-hw-virtio --tests` -- 32/32.
- `cargo test -p helm-hw-iommu` -- 79/79.
- `cargo test -p helm-engine --test sim_stats_registry` -- 16/16.
- `cargo test -p helm-report --features helmstats emit_config_ini` -- 3/3.
- Baseline workspace failures unchanged: 3 jit_system_mode_*, 4
  helm-jit::runtime::tests::execute_*, 2 helm-python::spy::tests::*
  (pyo3 GIL init), and 2 helm-report::sink::tcp::tests::* (sandbox
  network restriction). No new regressions.

## Next steps (open follow-ups)

1. **PCI bus / device counters** (`helm-hw-pci`). PciBus,
   PciConfigSpace accesses. Add IoStats-style producer at
   `system.pci.<bus>`.
2. **Other helm-hw-* (rtc, firmware, timer)** -- same pattern as
   PL011: add helm-stats workspace dep, attach a `pub stats:
   IoStats` field, bump from device read/write paths, register
   under `system.<name>` from helm-python.
3. **Per-queue VirtIO descriptor counters** (deferred from L15).
   Today IoStats publishes a single `descriptors` total per
   device; gem5 also exposes per-queue counts.
4. **Opaque PerfCounter / PerfHistogram Python handle objects**
   so scripts can `inc/add` from Python (debug feature). Read
   side already shipped in L11.
5. **Per-vCPU CPU stats real fan-out via Aarch64ArchState.**
   Today the engine's cpu_stats: Vec<CpuStats> indexes by
   active_fs_vcpu at retire -- works, but a per-vCPU
   ArchState.stats field would be cleaner.

## Working patterns reminder

- New `<Foo>Stats` struct in `framework/helm-stats/src/<name>.rs`
  with `Clone + Default + StatsProducer`; mod-declared in
  `lib.rs`, re-exported from helm-engine.
- The producing crate takes `helm-stats.workspace = true` as a
  non-optional dep; the field is `pub stats: helm_stats::<Foo>Stats`.
- Engine clones the handle once at the first `stats_registry()`
  call and registers it with `StatsScope::new(&mut
  self.stats_registry, "system.<path>")`.
- helm-python registers via `sim.register_producer(format!(...),
  Box::new(stats))`.
- Tests live in
  `runtime/helm-engine/tests/sim_stats_registry.rs` (the
  helm-stats/stats feature is dev-dep-forced on for that crate).
- Commit message: `feat(stats): <subject>` plus crate-grouped
  bullet list. Plain ASCII (no backticks/em-dashes; they don't
  survive `git commit -m`).
- Avoid `cargo test --workspace`; verify per-crate.
