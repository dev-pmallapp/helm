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

---

## Track A — Explicit SMP progress proof

**Refs:** [`cursor-plan-02-runtime-active-vcpu.md`](cursor-plan-02-runtime-active-vcpu.md),
[`aarch64-fs-smp-virt-progress.md`](aarch64-fs-smp-virt-progress.md)

### Steps

1. Add an integration test that boots or simulates a two-vCPU `arm-virt`
   machine and proves the currently stepped CPU observes its own IRQ line and
   state path.
2. Add one regression for secondary-CPU bring-up through the current PSCI path
   that demonstrates more than one runnable vCPU is maintained by the machine.
3. If any remaining SMP-specific host panic or wrong-CPU accessor path appears
   during that proof, fix it in this plan rather than reopening plan 02.

**Gate:** We have a durable engine-level test proving non-zero vCPU state and
IRQ semantics on the built-in board path.

---

## Track B — Rust-owned baseline DTB / machine description

**Refs:** [`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md)
Phase 1

### Steps

1. Inventory which `arm-virt` facts are currently defined in Rust code versus
   only in external/generated DTB data.
2. Add a Rust-side baseline DTB builder for the currently implemented machine
   subset: CPUs, memory, timer, PSCI, GIC, UART, and the already-supported
   built-in interrupt/peripheral surfaces.
3. Allow the FS kernel-loading path to use that Rust-authored DTB when the
   caller does not supply a DTB path or DTB bytes explicitly.
4. Keep explicit DTB override support, but make the built machine self-describe
   in the baseline case.

**Gate:** `arm-virt` FS boot no longer requires an externally hand-curated DTB
for the implemented baseline machine.

---

## Track C — Boot-critical virt-machine baseline peripherals

**Refs:** [`aarch64-fs-virt-machine-completeness.md`](aarch64-fs-virt-machine-completeness.md)
Phase 2

### Steps

1. Wire already-existing PL031 RTC support into the built `arm-virt` board path
   and the Rust-authored baseline DTB.
2. Reserve or codify the low-memory virt-machine windows for not-yet-implemented
   surfaces where the address-map contract matters even before the devices are
   fully modeled.
3. Keep the machine map and DTB shape aligned so Linux sees only what the board
   actually implements, plus clearly documented reserved windows where needed.

**Gate:** The built `arm-virt` baseline presents the current boot-critical
peripheral surface consistently in both board wiring and machine description.

---

## Track D — Documentation and examples

### Steps

1. Update the Python and architecture docs once the baseline DTB ownership path
   moves into Rust.
2. Refresh any example invocation that currently assumes an external DTB is
   mandatory for the implemented baseline machine.
3. Record any intentionally deferred virt-machine items so they do not look like
   accidental omissions.

**Gate:** The docs and examples describe the same baseline machine that the code
   now builds.

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
