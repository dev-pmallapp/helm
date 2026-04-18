# Plan P3 — AArch64 FS SMP and virt-machine completion

**Goal:** Close the remaining AArch64 full-system product gaps that are still
documented outside the cursor execution series: explicit SMP progress proof,
machine-owned description, and boot-critical virt-machine baseline wiring.
This plan picks up the remaining actionable items from
[`cursor-plan-02-runtime-active-vcpu.md`](cursor-plan-02-runtime-active-vcpu.md),
[`aarch64-fs-smp-virt-progress.md`](aarch64-fs-smp-virt-progress.md), and
[`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md).

---

## Why this plan exists

The cursor execution series `00..05` closed the cross-cutting correctness,
IOMMU, instrumentation, and Python-boundary work. The largest remaining
product-facing gaps are now all in one place:

- proving SMP progress with the current active-vCPU runtime model
- making the Rust `arm-virt` machine authoritative instead of relying on an
  external DTB for the baseline path
- integrating the already-implemented boot-critical virt-machine peripherals
  that are still missing from the built board path

The older temporary planning docs `cursor-plan-06..08` were deleted after their
implementation work landed; this file is the new continuation point.

---

## Current tree status

Already complete before this plan starts:

- active FS vCPU tracking and JIT FS vCPU alignment
- GIC private RPR fix and basic SMP scaffolding
- live SMMUv3 attachment on built-in `arm_virt`
- typed Python/engine construction boundary for the current built-in devices
- release instrumentation / stats CI coverage

What remains is no longer a generic runtime/framework problem; it is now
specifically **AArch64 FS machine completeness**.

Completed in the first implementation slice for this plan:

- a Rust-owned baseline `arm-virt` DTB builder now exists in the engine
- the FS kernel-load path can now auto-generate that DTB when neither a DTB
  path nor DTB bytes are supplied explicitly
- the built-in `arm-virt` baseline DTB already covers the currently implemented
  CPUs, memory, PSCI, timer, GIC, UART, and RTC surface

---

## ~~Track A — Explicit SMP progress proof~~ — DONE

**Refs:** [`cursor-plan-02-runtime-active-vcpu.md`](cursor-plan-02-runtime-active-vcpu.md),
[`aarch64-fs-smp-virt-progress.md`](aarch64-fs-smp-virt-progress.md)

Resolved by existing tests:
- `multi_vcpu_irq_and_state_path_combined` — proves IRQ line and state accessor route to active vCPU, not always vCPU 0
- `psci_cpu_on_powers_secondary_vcpu` — proves PSCI secondary bring-up
- `two_vcpu_system_*` integration tests — prove public API with 2-vCPU GICv2 system
- `fs_irq_polling_uses_selected_vcpu_irq_line` — proves per-vCPU IRQ routing

**Gate:** Met — durable engine-level tests prove non-zero vCPU state and IRQ semantics.

---

## Track B — Rust-owned baseline DTB / machine description -- DONE

**Refs:** [`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md)
Phase 1

### Steps

1. ~~Inventory which `arm-virt` facts are currently defined in Rust code versus only in external/generated DTB data.~~ Done.
2. ~~Add a Rust-side baseline DTB builder for the currently implemented machine subset: CPUs, memory, timer, PSCI, GIC, UART, and the already-supported built-in interrupt/peripheral surfaces.~~ Done — baseline DTB builder exists in the engine.
3. ~~Allow the FS kernel-loading path to use that Rust-authored DTB when the caller does not supply a DTB path or DTB bytes explicitly.~~ Done — auto-generates DTB when none supplied.
4. ~~Keep explicit DTB override support, but make the built machine self-describe in the baseline case.~~ Done.

**Gate:** `arm-virt` FS boot no longer requires an externally hand-curated DTB
for the implemented baseline machine. **MET.**

---

## Track C — Boot-critical virt-machine baseline peripherals -- DONE

**Refs:** [`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md)
Phase 2

### Steps

1. ~~Wire already-existing PL031 RTC support into the built `arm-virt` board path and the Rust-authored baseline DTB.~~ Done — RTC already wired, DTB interrupt description fixed.
2. ~~Reserve or codify the low-memory virt-machine windows for not-yet-implemented surfaces.~~ Done — reserved windows codified.
3. ~~Keep the machine map and DTB shape aligned so Linux sees only what the board actually implements.~~ Done.

**Gate:** The built `arm-virt` baseline presents the current boot-critical
peripheral surface consistently in both board wiring and machine description. **MET.**

---

## ~~Track D — Documentation and examples~~ — DONE

ARCHITECTURE.md updated with phase status, MemoryBackend trait, HelmSpy
observation path, PortRef wiring, and device introspection. TODO.md
reorganized with completed/open sections. Research docs annotated with
resolution status. Reserved address windows documented in helm-platform.
Deferred items (fw_cfg, GPIO, secure UART) codified as named constants.

---

## Verification

```bash
cargo test -p helm-engine
cargo clippy --workspace --lib --bins -- -D warnings
```

Add at least one engine-level SMP proof test and one engine-level baseline
machine-description/DTB-path test before closing this plan.

---

## Explicitly out of scope

- Full QEMU virt parity in one shot
- ACPI / flash / fw_cfg / PCIe completeness beyond the minimum needed to keep
  the machine description honest
- RISC-V SE completion work tracked in the roadmap and research docs
