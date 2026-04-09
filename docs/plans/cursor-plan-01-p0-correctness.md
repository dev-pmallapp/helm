# Plan P0 — Critical and high correctness (cursor research)

**Goal:** Address items marked **Critical** and selected **High** in [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) without large architectural rewrites.  
**Prerequisites:** Read [`cursor-v2-hw.md`](../research/cursor-v2-hw.md), [`cursor-v2-runtime.md`](../research/cursor-v2-runtime.md), [`cursor-v2-debug.md`](../research/cursor-v2-debug.md), [`cursor-v2-framework.md`](../research/cursor-v2-framework.md).

---

## 1. GICv2 GICC running priority for private IRQs (HW-C1)

**Refs:** [`aarch64-fs-smp-virt-progress.md`](aarch64-fs-smp-virt-progress.md) (GIC correctness), [`cursor-hw.md`](../research/cursor-hw.md) C1.

### Steps

1. Open `hw/helm-hw-intc/src/gicv2/cpu_interface.rs` and locate the GICC read path that returns running priority / RPR (see audit for line context).
2. Implement banked priority for IRQ IDs 0–31: read from per-CPU `private_priority` (or equivalent banked array), not `dist.priority[]`.
3. For SPIs (32+), keep distributor `priority[]` as today.
4. Add a **unit test** that programs a private IRQ priority, acknowledges it, and asserts RPR matches the banked value.
5. Run `cargo test -p helm-hw-intc` and a minimal FS smoke if available.

**Gate:** Test passes; Linux GIC driver no longer sees inconsistent RPR for PPI/SGI.

---

## 2. Virtqueue guest memory — no panic on `MemFault` (HW-C3)

**Refs:** [`cursor-hw.md`](../research/cursor-hw.md), [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) §5 Pattern B.

### Steps

1. Audit `hw/helm-hw-virtio/src/proto/virtqueue.rs` (and call sites) for `unwrap()` / `expect()` on memory reads/writes.
2. Change helpers to return `Result<_, VirtioError>` (or existing error type) where guest GPA is invalid.
3. Map errors to VirtIO device behavior: set `FAILED` / reset queue per spec section, or documented stub.
4. Add unit test with intentionally unmapped GPA — expect **error**, not panic.
5. Run `cargo test -p helm-hw-virtio`.

**Gate:** No host panic on bad descriptor GPA in tested paths.

---

## 3. Fault plugin `ArchContext` — no panic when AArch64/RISC-V missing (RT-C1)

**Refs:** [`cursor-v2-runtime.md`](../research/cursor-v2-runtime.md) RC1.

### Steps

1. Locate fault-plugin construction in `runtime/helm-engine/src/lib.rs` (search for `ArchContext::` / `riscv()`).
2. Replace `self.riscv().expect(...)` with `self.session.riscv()` or equivalent `Option` chain.
3. If neither AArch64 nor RISC-V state applies, use `ArchContext::Unknown` or skip plugin invocation with `sim_warn!` — **choose one policy** and document in code comment.
4. Add a **unit or integration test** that triggers fault callback in a configuration where only one ISA is active (as applicable).

**Gate:** No panic in fault path for misconfigured or single-ISA sessions.

---

## 4. `HelmPluginRegistry::has_any_callbacks` bitmask (FW-D4)

**Refs:** [`cursor-v2-framework.md`](../research/cursor-v2-framework.md) FE1.

### Steps

1. Read `framework/helm-plugin/src/runtime/registry.rs`: list all callback kinds and `CB_*` bits.
2. Ensure every kind that the engine can invoke sets a bit and is included in `has_any_callbacks()`.
3. Add bits for `CB_FAULT`, syscall pair, vCPU lifecycle if implemented — match engine call sites.
4. Add unit test: register only a syscall callback; assert `has_any_callbacks()` is true if engine would still call it.

**Gate:** Fast-path skip never drops registered syscall/fault handlers.

---

## 5. `helm-report` CSV column order (DB-C1)

**Refs:** [`cursor-v2-debug.md`](../research/cursor-v2-debug.md).

### Steps

1. Identify formatter that emits CSV; extract ordered field list used for header and for rows.
2. Ensure **one** ordered `&'static [&str]` or enum drives both.
3. Add round-trip test: format sample session → parse lines → assert column count and key cells.
4. Run `cargo test -p helm-report`.

**Gate:** Test fails if header/row order drifts.

---

## 6. MMIO `size` — pilot on PCI config (CC-8 subset)

**Refs:** [`cursor-cross-cutting.md`](../research/cursor-cross-cutting.md) §8.

### Steps

1. Add `helm-devices` helper (or local fn): `mask_read_u64(value, size)`, `mask_write_u64(old, data, size)` for `size in [1,2,4,8]`.
2. Apply to **PCI config space** reads/writes in `helm-hw-pci` first (highest impact).
3. Document in module rustdoc that PCI honors `size`; other devices unchanged in this PR.
4. Add unit tests for byte/word access to config space.

**Gate:** PCI narrow accesses tested; pattern ready for wider rollout (P3 HW plan).

---

## Verification (end of P0)

```bash
cargo test --workspace
cargo clippy --all --all-targets -- -D warnings
```

Record any intentional `allow` additions in PR description.

---

## Explicitly out of scope for P0

- **Active vCPU index** through `state()` — [`cursor-plan-02-runtime-active-vcpu.md`](cursor-plan-02-runtime-active-vcpu.md).
- **SMMU/IOMMU full walks** — [`cursor-plan-03-hw-iommu-pci.md`](cursor-plan-03-hw-iommu-pci.md).
- **`instantiate.rs` refactor** — [`cursor-plan-05-python-tooling-ci.md`](cursor-plan-05-python-tooling-ci.md).
