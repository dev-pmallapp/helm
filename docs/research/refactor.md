# Refactor Research Backlog

Date: 2026-04-05

## Status

This document consolidates the still-relevant findings from:

- `docs/research/design-issues.md`
- `docs/research/jit-remodeled.md`
- `docs/research/jit-acceleration-no-llvm.md`

It is also refreshed against:

- `docs/research/jit-interpreter-performance-plan.md`
- the recent JIT/runtime boundary commits from `cfb3206` through `e69fda4`
- `a1c1b7b` deferring adaptive register binding until backend support exists
- `70442ef` deferring inline-cache specialization until metadata/runtime support exists

This file is now the single architecture refactor backlog. The performance plan
remains a useful execution document, but this file defines the structural
direction that the implementation work should follow.

## Purpose

The goal is not to list every possible code smell. The goal is to make the
workspace easier to evolve by clarifying:

- who owns construction
- who owns runtime policy
- who owns device behavior
- who owns shared contracts
- where future JIT work should live

The target architectural direction is:

```text
runtime/*  <->  framework/*  <->  hw/*
```

Meaning:

- `framework/*` owns stable contracts, shared data models, shared runtime
  adapters, and reusable subsystems
- `runtime/*` owns orchestration, ISA stepping, host integration, and platform
  realization
- `hw/*` owns concrete device models
- runtime and hardware should communicate through framework-defined boundaries,
  not by directly encoding each other's implementation details

## Inputs Reviewed

Primary source docs:

- `docs/research/design-issues.md`
- `docs/research/jit-remodeled.md`
- `docs/research/jit-acceleration-no-llvm.md`
- `docs/research/jit-interpreter-performance-plan.md`

Current code surfaces re-read for this rewrite:

- `runtime/helm-engine/Cargo.toml`
- `runtime/helm-engine/src/lib.rs`
- `runtime/helm-engine/src/jit.rs`
- `runtime/helm-engine/src/platform/arm_virt.rs`
- `runtime/helm-python/src/instantiate.rs`
- `runtime/helm-platform/src/lib.rs`
- `runtime/helm-platform/src/aarch64/virt.rs`
- `framework/helm-memory/src/lib.rs`
- `framework/helm-memory/src/address_space.rs`
- `framework/helm-jit/src/lib.rs`
- `framework/helm-jit/src/runtime.rs`
- `framework/helm-jit/src/regs.rs`
- `framework/helm-jit/src/helpers.rs`
- `framework/helm-jit/src/block.rs`
- `framework/helm-jit/src/cache.rs`
- `framework/helm-devices/src/lib.rs`
- `framework/helm-plugin/src/runtime/registry.rs`
- `framework/helm-probe/src/lib.rs`
- `debug/helm-spy/src/session.rs`
- `debug/helm-report/src/snapshot.rs`

Recent commits reviewed:

- `e69fda4` `refactor: share jit cache hit execution helper`
- `647f43c` `refactor: share jit promotion execution helper`
- `ab9f69b` `refactor: share jit compile miss resolution`
- `552dc38` `refactor: share jit interpreter handoff helper`
- `d7ba668` `refactor: share jit unsupported fallback helper`
- `a9b27a0` `refactor: share jit block execution helper`
- `8c2299c` `refactor: share jit cache miss policy helper`
- `c4b280e` `refactor: share jit fallback policy helper`
- `cfb3206` `refactor: extract jit runtime boundary types`
- `2c2a310` `docs: capture trace jit activation prerequisites`
- `86b92d5` `docs: track deferred jit reactivation tasks`
- `70442ef` `refactor: defer ic specialization until metadata support`
- `a1c1b7b` `refactor: defer adaptive jit binding until backend support`

## What Changed Since The Earlier Draft

The first version of `refactor.md` was already stale almost immediately because
the JIT/runtime boundary moved quickly.

### JIT boundary work that is now real

The following is no longer hypothetical:

- `framework/helm-jit/src/runtime.rs` now owns runtime-facing JIT helper policy
- `runtime/helm-engine/src/jit.rs` now depends on `helm_jit::runtime` for cache
  probing, fallback handling, compile-miss resolution, promotion, and
  interpreter handoff

This means the correct framing is no longer:

- "move all JIT policy out of the engine someday"

It is now:

- "finish the boundary move that has already started, and keep new JIT runtime
  work on the `framework/helm-jit` side unless there is a strong reason not to"

### Adaptive binding and IC specialization are now correctly dormant

Current truth:

- adaptive binding is not active
- IC specialization is not active
- both have explicit placeholders and documented prerequisites

That is the right state today. They should be tracked as future work, not
treated as half-live optimizations.

## Core Principles

1. One owner per concern.
2. Framework defines the shared language.
3. Runtime should consume build artifacts, not discover structure ad hoc.
4. Hardware should implement contracts, not invent replacement contracts.
5. Deferred features must be either:
   - fully active
   - clearly dormant with prerequisites
6. Preserve intentionally monolithic ISA execution files where the project has
   explicitly chosen that model.

## High-Level Findings

### F-1. Runtime and hardware are still too tightly coupled

Evidence:

- `runtime/helm-engine/Cargo.toml`
- `runtime/helm-engine/src/platform/arm_virt.rs`
- `runtime/helm-engine/src/lib.rs`
- `hw/helm-hw-virtio/Cargo.toml`

`runtime/helm-engine` still depends directly on concrete hardware crates such
as `helm-hw-char`, `helm-hw-rtc`, and `helm-hw-intc`, and still constructs and
wires those device models itself.

That violates the long-term intent that runtime and hardware communicate
through framework-owned contracts or through a narrow integration layer.

### F-2. Platform construction still has multiple partial owners

Evidence:

- `runtime/helm-platform/src/lib.rs`
- `runtime/helm-platform/src/aarch64/virt.rs`
- `runtime/helm-engine/src/platform/arm_virt.rs`
- `runtime/helm-python/src/instantiate.rs`

Today:

- `helm-platform` describes platforms
- `helm-engine` actually builds boards
- `helm-python` discovers and validates object graphs

There is still no single authoritative build/freeze artifact.

### F-3. Memory is still split across overlapping abstractions

Evidence:

- `framework/helm-memory/src/lib.rs`
- `framework/helm-memory/src/address_space.rs`
- `runtime/helm-engine/src/address_space.rs`
- `docs/TODO.md`

The workspace still has three overlapping answers to "what is the memory
subsystem?":

- `FlatMem`
- `HelmAddressSpace`
- `MemoryMap`

This makes future DMA, remap, alias, and device/MMIO integration work harder
than it should be because there is still not one obvious owner.

Phase 3 now treats this more concretely:

- `HelmAddressSpace` is the current authoritative physical-memory surface for
  live runtime work, including DMA-facing access and device-visible MMIO
- `MemoryMap` remains an experimental region-tree model until alias/container/
  remap behavior is complete and actually replaces or subsumes the active
  address-space path

### F-4. Observability still has multiple live stories

Evidence:

- `framework/helm-plugin/src/runtime/registry.rs`
- `framework/helm-probe/src/lib.rs`
- `debug/helm-spy/src/session.rs`
- `debug/helm-report/src/snapshot.rs`
- `runtime/helm-python/src/system.rs`

The codebase still has:

- legacy callback plugins
- probe-based observation
- `HelmSpy` session aggregation
- report-owned snapshot schema
- Python APIs that still install callback-based observers

This is too many partially-overlapping observability surfaces.

### F-5. JIT decoupling has started, but is incomplete

Evidence:

- `framework/helm-jit/src/runtime.rs`
- `runtime/helm-engine/src/jit.rs`
- `framework/helm-jit/src/regs.rs`
- `framework/helm-jit/src/helpers.rs`
- `framework/helm-jit/src/block.rs`

What is better now:

- fallback policy is partly framework-owned
- promotion/caching execution helpers are partly framework-owned
- host/runtime roles are starting to separate

What is still incomplete:

- `run_jit()` orchestration still lives in `helm-engine`
- engine still owns backend instances and JIT lifecycle wiring
- helper APIs still encode runtime-specific memory/MMU shapes
- backend contracts still cannot consume dynamic register binding
- emitters/runtime still cannot fully support IC specialization

### F-5a. Several JIT performance findings remain architecturally relevant

The older JIT research contained performance findings that are still important,
but should now be interpreted through the boundary-cleanup lens rather than as
standalone local optimizations.

Still-relevant structural themes:

- register pinning should be expressed through backend-consumable binding contracts
- inline TLB fast paths should sit behind a host/runtime memory adapter contract
- block chaining and trace activation should use framework-owned JIT metadata and
  invalidation contracts
- cache shape and block invalidation policy should not be hard-coded in ways
  that re-entangle `helm-engine` with backend-specific policy
- opcode coverage expansion should follow, not precede, control-flow and runtime
  boundary cleanup

### F-6. Some hardware crates still define local cross-cutting contracts

Evidence:

- `hw/helm-hw-dma/src/dma.rs`
- `hw/helm-hw-virtio/src/lib.rs`
- `hw/helm-hw-iommu/src/lib.rs`

Examples:

- `hw-hw-dma` now uses `helm_core::DmaPort`; remaining duplication is in other
  crates, not the DMA controller itself
- VirtIO descriptor walking and backend guest-memory access now use
  `helm_core::ByteMem`
- IOMMU table walks now use `helm_core::ByteMem`

These should converge toward framework-owned contracts rather than continuing
to proliferate per crate.

## Architectural Program

### Phase 1. Make Framework The Mediation Layer

Goal:

- runtime and hardware use framework contracts as their shared language

Current slice status:

- the current authoritative framework contracts are explicit in code:
  - memory access: `helm_core::MemInterface` + `helm_core::ByteMem`
  - DMA: `helm_core::DmaPort` with `helm_memory::SharedDmaPort`
  - interrupt routing: `helm_devices::InterruptSink` +
    `helm_devices::MessageInterruptSink`
  - timer scheduling: `helm_core::TimerScheduler`
  - platform build metadata: `helm_platform::PlatformBuildPlan`
  - JIT host/runtime interaction: `helm_jit::runtime::JitRuntimeHost`
- current hardware/runtime code is already using framework-owned contracts for
  the active cross-cutting concerns in this phase:
  - VirtIO and IOMMU guest-memory access use `ByteMem`
  - DMA uses `DmaPort`
  - interrupt wiring uses `InterruptSink` / `MessageInterruptSink`
  - event/timer scheduling uses `TimerScheduler`
- no remaining tests currently rely on `helm-engine` address-space reexports as
  substitutes for direct framework imports

Tasks:

- [x] Identify the authoritative framework contracts for:
  - memory access
  - DMA
  - interrupt routing
  - timer scheduling
  - platform build artifacts
  - JIT host/runtime interaction
- [x] Remove local replacement contracts from hardware crates where framework
  already owns the concern.
- [x] Stop using engine reexports as test-time substitutes for direct framework
  imports.

Acceptance:

- new runtime/hardware features can be added without inventing new local
  cross-cutting traits

### Phase 2. Establish A Single Build / Freeze Boundary

Goal:

- one owner for system construction

Current thin-slice status:

- shared typed handoff records now exist for:
  - discovered built-in config
  - frozen simulator config
  - simulator build request
- built-in platform selection, memory defaulting, mapped-device classification,
  overlap validation, and discovery->freeze defaulting/validation are now
  shared helper logic instead of Python-local ad hoc code
- a named built-system artifact now exists in the runtime integration layer:
  `BuiltSystem`
- the primary engine arm-virt entry points now consume a built-system artifact
  instead of open-coded board assembly from tuple parts
- `runtime/helm-platform` is now explicitly treated as the descriptor/metadata
  layer, with final executable built-system integration remaining in the
  runtime integration layer
- Python freeze/instantiate already emits typed build inputs for the core
  simulator through `FrozenSimulatorConfig` + `SimulatorBuildRequest`
- the remaining work is structural generalization rather than missing
  thin-slice plumbing: broaden the built-system boundary beyond the current
  built-in arm-virt path

Post-Phase-3 / Phase-4 follow-ons identified during the Phase 2 work:

- After Phase 3:
  - revisit the long-term home of the built-system artifact once the
    authoritative memory surface is chosen
  - today the built AArch64 / arm-virt artifacts still carry
    `HelmAddressSpace`-shaped state, so relocating them further before the
    memory-owner decision would risk hardening the wrong memory boundary
  - revisit kernel/load-path realization ownership together with the chosen
    memory surface, since boot asset loading and system realization both write
    directly into the same RAM owner
- After Phase 4:
  - revisit built-system install side effects such as vCPU init/observer
    lifecycle hooks once the primary observability story is fully probe/session
    based
  - today those effects still flow through engine-owned compatibility paths,
    and should not be frozen into the long-term builder boundary until the
    observability collapse is complete

Tasks:

- [x] Introduce a single frozen build artifact, for example `BuiltSystem`.
- [x] Decide whether `runtime/helm-platform` becomes:
  - the real builder
  - or a pure descriptor crate with a separate concrete integration layer
- [x] Change `runtime/helm-python` so it emits typed build inputs instead of
  performing discovery as the construction algorithm.
- [x] Make `helm-engine` consume a built system rather than directly assembling
  `arm_virt`.

Acceptance:

- board construction logic no longer lives as open-coded engine wiring

### Phase 3. Pick One Authoritative Memory Surface

Goal:

- one clear memory subsystem owner

Current slice status:

- `HelmAddressSpace` is now the explicit authoritative owner on `main` for the
  live runtime surface
- new DMA/remap/device-visible memory work should target
  `HelmAddressSpace`-based APIs or wrappers first
- `helm_core::ByteMem` is now the shared byte-access contract layered on top of
  that active surface, and is consumed by VirtIO, IOMMU table walks, and
  runtime guest-byte helpers
- `HelmAddressSpace` now exposes transactional map/unmap/remap helpers over the
  live `AddressMap`, so remap work no longer needs to wait on `MemoryMap`
  becoming authoritative
- `HelmAddressSpace` also tracks PCI BAR-backed regions by raw BDF/BAR key, so
  future `PciBus::drain_remaps()` integration can target the live surface
  without introducing a framework dependency on `hw-hw-pci`
- `PciBus` BAR config writes now emit concrete `RemapCommand`s, and runtime FS
  MMIO paths already drain and apply those remaps against `HelmAddressSpace`
- the built-in arm-virt system path now instantiates a live `pci0` ECAM bus
  by default, and Python instantiate can attach PCI-backed machine surfaces
  onto that path
- a real modern standard `virtio-pci` transport now exists on top of the
  active `HelmAddressSpace` / BAR-remap story, including:
  - capability-linked PCI config space
  - BAR0 common/ISR/notify/device-config regions
  - BAR4 MSI-X table + PBA regions
  - public Python/config wrappers for `rng`, `blk`, `net`, and `console`
- deferred standard `virtio-pci` queue work now emits framework-owned
  `MessageInterrupt` payloads through a `MessageInterruptEmitter` /
  `MessageInterruptSink` contract instead of raw tuple plumbing
- the built-in arm-virt platform now owns the current PCI MSI routing model:
  - synthetic `PCIE_MSI_ADDR` target
  - SPI-only data translation policy
  - GIC edge delivery through platform-owned sink adapters for GICv2/GICv3
- `PciVirtioRngMmio` is explicitly a compatibility shim; the standard modern
  `virtio-pci` path is the preferred public transport surface
- `MemoryMap` is explicitly non-authoritative until it grows complete
  alias/container/remap behavior and is adopted by runtime callers

Tasks:

- [x] Choose the active authoritative owner on `main`: `HelmAddressSpace`.
- [x] Make remap/DMA/current device-visible memory work target that surface.
- [x] Extract MSI-X delivery onto a framework-grade message interrupt
  sink/emitter contract.
- [x] Replace the local engine `MSI data -> INTID` shortcut with a
  platform-owned arm-virt PCI MSI routing model.
- [x] Land the operational standard `virtio-pci` transport path and demote the
  BAR-exposed MMIO bridge path to compatibility status.
- [x] Update docs so the advertised model matches the operational one.
- [x] Clearly demote incomplete parallel abstractions that are not yet
  authoritative.

Post-Phase-3 follow-ons:

- Future ARM-faithful MSI routing such as GICv2m or GICv3 ITS/LPI delivery is
  no longer Phase 3 cleanup. It is a deliberate platform/controller expansion.
- Future `MemoryMap` region-tree convergence is no longer a hidden competing
  runtime surface decision. It is separate experimental convergence work.

Acceptance:

- one obvious answer exists for RAM, MMIO, remap, DMA, and device-visible
  memory behavior

### Phase 4. Collapse Observability To One Primary Story

Goal:

- probes and report/spy collection become the primary path

Current slice status:

- callback-plugin observability is now explicitly quarantined as a legacy
  compatibility surface in the remaining top-level architecture/design docs
- snapshot-schema ownership now lives in `helm-spy`, with `helm-report`
  consuming the shared schema instead of owning it
- Python observation guidance now points new users at `helm.HelmSpy(...)` or
  `system.observe()`
- the primary intended path on `main` is:
  `helm-probe` event source -> `helm-spy` collection/session state ->
  `helm-report` formatting/sinks

Tasks:

- [x] Formally quarantine or deprecate callback-plugin observability where it
  overlaps with probes.
- [x] Move snapshot-schema ownership out of `debug/helm-report`.
- [x] Keep `helm-report` focused on formatting and sinks.
- [x] Keep `helm-spy` focused on collection/session state.
- [x] Ensure Python observation APIs prefer probe/session-backed flows.

Acceptance:

- one primary path exists from event source to collected snapshot to delivered
  report

### Phase 5. Continue The JIT Runtime Boundary Move

Goal:

- `framework/helm-jit` owns JIT runtime policy and backend-facing contracts
- `runtime/helm-engine` implements host services against those contracts

Current slice status:

- shared runtime helpers in `framework/helm-jit/src/runtime.rs` now own:
  - block-cache probe policy
  - trace pre-dispatch policy
  - guarded trace-exit accounting/retirement
  - compile-miss and bounded interpreter fallback policy
  - trace-recording plan/record policy
  - AArch64 JIT memory/MMU dispatch-context setup
  - AArch64/RISC-V64 JIT backend/cache/trace-runtime initialization policy
- `runtime/helm-engine/src/jit.rs` is now narrowed primarily to:
  - host state extraction
  - host service implementation via `JitRuntimeHost`
  - ISA-specific decode helpers
  - top-level control flow in `run_jit()`
- the AArch64 helper-slot wiring and FS `JitFsContext` construction no longer
  live as engine-local policy blobs
- backend/cache/trace-runtime constructor selection no longer lives as
  engine-local lifecycle policy
- remaining JIT work after this point is no longer boundary-cleanup work:
  - Phase 6: backend support for adaptive binding / IC specialization
  - Phase 7: broader trace-JIT activation and continuity on top of the shared
    runtime boundary

Tasks:

- [x] Keep extracting host/runtime-neutral JIT execution helpers into
  `framework/helm-jit/src/runtime.rs`.
- [x] Narrow `runtime/helm-engine/src/jit.rs` toward:
  - host state setup
  - host service implementation
  - top-level control flow only
- [x] Define whether memory and MMU helper setup belongs in:
  - a framework-owned host adapter protocol
  - or a narrower runtime-owned adapter layer
- [x] Remove policy duplication between `helm-engine` and `helm-jit`.

Acceptance:

- the engine no longer owns JIT policy algorithms that can live in the
  framework layer

### Phase 6. Reactivate Deferred JIT Features Only After Backend Support Exists

Goal:

- treat adaptive binding and IC specialization as real backlog items with
  prerequisites, not as dormant magic

Current slice status:

- adaptive binding is now absent from the active runtime path:
  - no runtime `RegHeatMap` feedback loop remains in `helm-jit`
  - active backends continue to use only the static binding path
- inline-cache specialization is now absent from the active runtime path:
  - no `IcPatch` metadata is carried on compiled blocks
  - no runtime IC patch arming hook remains in `helm-jit` helpers
  - no helper-side IC patching can trigger during active JIT execution
- future reactivation work remains explicitly tracked under:
  - `RFX-040` adaptive binding backend support
  - `RFX-041` inline-cache specialization backend/runtime support
  - `RFX-042` keep both features inactive until end-to-end support exists

Tasks:

- [x] Remove dormant adaptive-binding runtime scaffolding until backend support
  exists end to end.
- [x] Remove dormant inline-cache specialization scaffolding until emitter,
  runtime, and invalidation support exist end to end.
- [x] Keep both features explicitly off until end-to-end support exists.

Acceptance:

- adaptive binding is either fully end-to-end or absent from the active runtime
- IC specialization is either fully end-to-end or absent from the active runtime

### Phase 7. Keep Trace-JIT Work Behind A Clean Contract

Goal:

- trace JIT activation should happen on top of the new JIT boundary, not by
  re-growing engine-local policy

Current slice status:

- trace dispatch, recorder policy, guard-exit accounting, and trace retirement
  now flow through `framework/helm-jit/src/runtime.rs`
- conservative trace invalidation remains tied to framework-owned
  `TraceCache` / `TraceInvalidationEvent` contracts
- engine-side trace activation is limited to:
  - host-side decode for trace candidates
  - top-level control flow in `run_jit()`
  - SE-only opt-in runtime gating
- trace execution can now be enabled without reintroducing engine-local policy
  blobs for dispatch, guard exits, or retirement

Tasks:

- [x] Reconcile trace runtime activation with the new `helm_jit::runtime`
  boundary.
- [x] Keep trace invalidation, guard exits, and chaining semantics tied to
  framework-owned JIT contracts.
- [x] Ensure trace execution does not reintroduce engine-local policy blobs.

Acceptance:

- trace-JIT activation can happen without undoing the JIT boundary cleanup

## Crate-by-Crate Refactor Priorities

This section is intentionally brief per crate. The heavy detail from the older
research notes has been compressed into the smallest actionable architectural
summary for each crate.

### `framework/helm-core`

- Keep as the base contract crate.
- Separate execution-core contracts from service-style contracts more clearly.
- Revisit whether DMA and power-control contracts belong here or in a narrower
  runtime-contract layer.

Severity: medium

### `framework/helm-decode`

- Keep as a build/generation crate.
- Update docs so they describe the currently-real codegen role, not a larger
  future dual-backend story than the repo actually uses today.

Severity: low-medium

### `framework/helm-devices`

- Split the mental model between:
  - device core contracts
  - bus helpers
  - SDK/ABI surface
  - synchronous event bus
- Reduce root-level umbrella exports over time.

Severity: high

### `framework/helm-diag`

- Keep diagnostic data model and macros stable.
- Separate monitor lifecycle / fallback policy from the minimal emission model.

Severity: medium

### `framework/helm-event`

- Keep the queue small and focused.
- Treat the thread-safe wrapper as an adapter, not as part of the scheduler’s
  conceptual core.

Severity: medium

### `framework/helm-jit`

- Continue the current direction.
- Keep moving runtime-neutral JIT policy into this crate.
- Decide which parts are truly framework-grade and which are ISA/runtime adapter
  code that should be nested more explicitly.
- Add backlog support tasks for:
  - adaptive binding backend support
  - IC specialization metadata/runtime support

Severity: critical

### `framework/helm-memory`

- Choose one authoritative execution-facing memory surface.
- Stop presenting multiple parallel futures as if they are equally current.
- Make DMA/remap/alias/container work target one owner.

Severity: critical

### `framework/helm-plugin`

- Treat callback-registry instrumentation as legacy compatibility.
- Avoid growing new first-class observability features here.

Severity: high

### `framework/helm-probe`

- Keep as the primary observation mechanism.
- Isolate build-profile-dependent behavior and ambient thread-local helpers so
  they do not become the public architecture story.

Severity: medium

### `framework/helm-stats`

- Mostly fine.
- Keep metrics primitives here.
- Avoid letting stats become an execution-policy owner beyond typed counters.

Severity: low

### `framework/helm-timing`

- Split conceptual layers more clearly:
  - trait and metadata
  - virtual timing
  - interval timing
  - accurate timing
- Reconcile ownership of timing-related memory estimation with the engine.

Severity: medium-high

### `runtime/helm-arch`

- Keep the intentional monolithic ISA execute/decode shape.
- Isolate ambient instrumentation shortcuts such as probe context so they do not
  define the permanent contract.

Severity: medium-high

### `runtime/helm-cli`

- Separate CLI concerns from embedded Python host concerns more clearly.
- Keep launcher code thin once platform/build contracts improve.

Severity: medium

### `runtime/helm-debug`

- Separate live debugging, inspection, and checkpoint concerns more clearly in
  public structure and future ownership.

Severity: medium

### `runtime/helm-engine`

- Shrink responsibility.
- Stop being:
  - simulator core
  - platform builder
  - concrete hardware integrator
  - partial JIT policy owner
- Consume built artifacts and framework contracts instead.

Severity: critical

### `runtime/helm-platform`

- Either become the real platform realization boundary or become a pure
  descriptor crate with no implied construction ownership.
- The current halfway state should end.

Severity: critical

### `runtime/helm-python`

- Replace graph discovery with typed build input emission.
- Stop letting Python object traversal be the actual machine-construction path.
- Keep compatibility APIs separate from the main typed path.

Severity: critical

### `hw/helm-hw-char`

- Crate boundary is fine.
- Internal organization can be cleaned up, but this is not a strategic
  architecture problem.

Severity: low-medium

### `hw/helm-hw-dma`

- Remove the local DMA contract and converge on a framework-owned memory/DMA
  contract.
- Keep controller logic local, not the cross-cutting interface.

Severity: high

### `hw/helm-hw-intc`

- Keep controller implementation here.
- Reduce platform-specific wiring assumptions in public helper surfaces.
- Keep probe support adapter-like, not core to the controller mental model.

Severity: medium-high

### `hw/helm-hw-iommu`

- Make maturity levels explicit across SMMUv3 vs stub-style architectures.
- Converge on shared framework memory contracts for translation and DMA-facing
  access.

Severity: medium

### `hw/helm-hw-pci`

- Keep endpoint/config logic separate from host-bridge and remap-integration
  concerns.
- Tie BAR remap semantics to the authoritative memory owner once chosen.

Severity: medium-high

### `hw/helm-hw-rtc`

- Normalize tick/time semantics with the rest of the device model.
- Avoid dual time contracts that can be interpreted as seconds in one API and
  cycles in another.

Severity: medium-high

### `hw/helm-hw-timer`

- Similar to RTC: align ticking semantics with the shared timing/device
  contract.
- Internal cleanup is useful but not the primary architecture bottleneck.

Severity: low-medium

### `hw/helm-hw-virtio`

- Split protocol/transport from concrete backends more cleanly.
- Remove reliance on engine reexports in tests.
- Converge guest-memory access on a framework-level contract.

Severity: high

### `debug/helm-report`

- Move snapshot-schema ownership out of this crate.
- Keep this crate about formatting and sinks only.

Severity: high

### `debug/helm-spy`

- Keep this crate as the collection/session owner.
- Make it the natural home for report snapshot state once extraction from
  `helm-report` happens.

Severity: medium-high

## Task Ledger

These are the concrete follow-up tasks that fall out of the analysis above.
They are intentionally phrased as issue-sized work items rather than themes.

### Construction / Layering

- `RFX-001`
  - Introduce a single frozen machine build artifact, such as `BuiltSystem`.
  - Inputs: `runtime/helm-platform`, `runtime/helm-python`, `runtime/helm-engine`.
  - Goal: remove split ownership of build/freeze.
  - Current status:
    - precursor records now exist (`BuiltInDiscoveredConfig`,
      `FrozenSimulatorConfig`, `SimulatorBuildRequest`)
    - the remaining step is to unify them under one authoritative built-system
      artifact instead of a staged chain of smaller records
  - Post-Phase-3 dependency:
    - the final built-system artifact should be revisited once the memory owner
      is settled, because current built artifacts still carry
      `HelmAddressSpace`-shaped state

- `RFX-002`
  - Decide whether `runtime/helm-platform` becomes:
    - the real platform realization boundary, or
    - a pure descriptor crate with a separate integration layer.
  - Goal: end the current halfway state.
  - Current status:
    - built-in platform selection/defaulting helpers now exist in
      `runtime/helm-platform`
    - the unresolved work is no longer helper placement; it is the ownership
      decision for actual platform realization/build execution
  - Post-Phase-4 dependency:
    - final builder ownership should be revisited together with observability
      install side effects, so legacy plugin/vCPU-init compatibility paths do
      not become part of the permanent builder contract

- `RFX-003`
  - Move `arm_virt` board construction out of `runtime/helm-engine` into an
    explicit platform/integration layer.
  - Goal: engine consumes built systems instead of constructing device graphs.
  - Current status:
    - primary arm-virt engine entry points now consume a built AArch64 system
      artifact instead of open-coding `HelmBoard` assembly from tuple returns
    - arm-virt builders now produce that artifact directly for empty-board and
      loaded-kernel setup paths
    - remaining work is the bigger ownership move:
      - where the built artifact lives long-term
      - how it generalizes beyond arm-virt
      - how board realization leaves `runtime/helm-engine` entirely
  - Post-Phase-3 dependency:
    - the final relocation of board realization should happen after the
      authoritative memory surface is chosen
  - Post-Phase-4 dependency:
    - observer/plugin install side effects should be revisited after the
      primary probe/session/report path is fully established

- `RFX-004`
  - Replace Python object-graph discovery with typed build input emission.
  - Goal: `runtime/helm-python` stops being an implicit construction engine.
  - Current status:
    - Python no longer owns the discovered-record shape, frozen-record shape,
      mapped-device vocabulary, built-in platform defaulting rules, or the
      shared discovery->freeze validation/defaulting flow.
    - Remaining work is to replace the current PyO3 discovery walk plus
      request assembly with a true shared build-input emission path.
  - Post-Phase-3 dependency:
    - shared build-input emission should be aligned with the chosen memory
      owner instead of hardcoding current RAM/container assumptions

- `RFX-005`
  - Remove hardware test dependence on `runtime/helm-engine` reexports where
    direct framework imports are the correct contract.
  - Initial target: `hw/helm-hw-virtio`.

### Memory / DMA

- `RFX-010`
  - Choose the authoritative runtime-facing memory surface:
    - `MemoryMap`
    - `HelmAddressSpace`
    - or a merged successor
  - Goal: one owner for RAM/MMIO/remap/DMA-facing behavior.

- `RFX-011`
  - Converge DMA contracts onto a framework-owned interface.
  - Remove local DMA-style contracts from:
    - `hw/helm-hw-dma`
    - `hw/helm-hw-virtio`
    - any overlapping IOMMU memory access surfaces

- `RFX-012`
  - Tie PCI BAR remap and future remap workflows to the authoritative memory
    owner instead of parallel future abstractions.

### Observability

- `RFX-020`
  - Formally mark callback-plugin instrumentation as legacy compatibility.
  - Goal: probes + spy + report pipeline becomes the primary story.

- `RFX-021`
  - Move snapshot schema ownership out of `debug/helm-report`.
  - Preferred destination: `debug/helm-spy` or a shared instrumentation model crate.

- `RFX-022`
  - Audit Python observation APIs and move them to probe/session-backed flows
    wherever legacy callback-plugin paths are still the default.

### JIT Boundary

- `RFX-030`
  - Continue extracting host/runtime-neutral JIT execution policy into
    `framework/helm-jit/src/runtime.rs`.
  - Goal: `runtime/helm-engine/src/jit.rs` becomes host wiring plus top-level control flow.

- `RFX-031`
  - Define a framework-owned host adapter for JIT memory/MMU services, or
    explicitly isolate that adapter in a narrow runtime layer.
  - Goal: stop letting helper APIs silently encode engine-specific runtime shapes.

- `RFX-032`
  - Revisit `framework/helm-jit` crate classification and public structure.
  - Goal: distinguish reusable JIT runtime contracts from ISA- and backend-specific adapter code.

- `RFX-033`
  - Define a clean framework-owned contract for block chaining metadata,
    invalidation, and relinking so future chaining/trace work does not drift
    back into engine-local policy.

- `RFX-034`
  - Revisit JIT cache shape and invalidation policy after the host/runtime
    boundary is stable.
  - Goal: make cache topology a JIT-layer concern, not an engine-layer accident.

- `RFX-035`
  - Treat opcode coverage expansion as a post-boundary task.
  - Goal: avoid growing backend surface area faster than the JIT/runtime
    contracts that must support it.

### Deferred JIT Features With Explicit Prerequisites

- `RFX-040`
  - Adaptive register binding backend support.
  - Required work:
    - add backend contract support for non-static bindings
    - thread a real binding object through compile paths and prologue/epilogue generation
    - define recompilation/invalidation semantics for binding changes
    - only after that, reactivate runtime use of `RegHeatMap`
  - This task exists because `a1c1b7b` correctly deferred the feature until backend support exists.

- `RFX-041`
  - Inline-cache specialization backend/runtime support.
  - Required work:
    - emit real IC patch metadata
    - arm a specific IC patch context before block execution
    - define invalidation rules across memory-layout changes and cache flushes
    - only after that, enable helper-side IC specialization logic in the active runtime
  - This task exists because `70442ef` correctly deferred the feature until metadata/runtime support exists.

- `RFX-042`
  - Keep adaptive binding and IC specialization explicitly inactive until
    `RFX-040` and `RFX-041` are end-to-end complete.
  - Goal: prevent half-active optimizations from reappearing.

- `RFX-043`
  - Reconcile trace-JIT activation with the new `helm_jit::runtime` boundary.
  - Goal: future trace activation should build on the new JIT contracts instead
    of re-growing engine-local policy blobs.

## What Not To Do

- Do not split `runtime/helm-arch/src/aarch64/execute.rs` purely because it is large.
- Do not reactivate adaptive binding by only restoring engine-local counters.
- Do not reactivate IC specialization by only restoring helper-side patching.
- Do not add new per-crate DMA or guest-memory interfaces in hardware crates.
- Do not let `helm-engine` become the long-term owner of platform realization,
  device wiring, and JIT policy all at once.

## Definition Of Done

This refactor program is successful when:

- runtime and hardware primarily communicate through framework contracts
- one build/freeze artifact exists
- one authoritative memory surface exists
- one primary observability story exists
- `helm-jit` owns its runtime-facing policy layer
- deferred JIT features are either fully supported end to end or explicitly off
- the crate taxonomy tells the same story as the dependency graph
