# GICv3 / GICv4 Implementation Plan

> ARM IHI0069H (GICv3), IHI0069H §9-10 (GICv4/v4.1)

This document describes a phased plan for adding GICv3 and GICv4
device models to HELM. Each phase is self-contained: it produces a
testable, usable increment and is gated by `make pre-commit`.

---

## Current State

| Component | Status |
|-----------|--------|
| `arm::gic::Gic` | GICv2 stub — GICD + GICC in one struct |
| `irq::InterruptController` | Trait with `inject`, `pending_for_cpu`, `ack` |
| `irq::IrqRouter` | Routes `(device, line) → (controller, irq)` |
| `fdt.rs` | `DtbConfig::gic_version` already handles v2/v3 compatible strings |
| `platform.rs` | `arm_virt_platform` + `realview_pb_platform` use GICv2 |

### Gaps to close

- No Redistributor (GICR) — required for GICv3.
- No system-register interface (ICC\_\*, ICH\_\*) — GICv3 replaces the
  MMIO CPU interface with system registers for EL1/EL2.
- No affinity routing — GICv3 replaces GICD\_ITARGETSR with
  GICD\_IROUTER (64-bit affinity).
- No LPI / ITS — needed for PCIe MSI and large interrupt counts.
- No virtual interrupt support — GICv4 vLPI / vSGI.
- `InterruptController` trait assumes single-call `ack` and has no
  concept of per-PE redistributors or priority drop/deactivate split.

---

## Phase 1 — Refactor & GICv3 Distributor

**Goal:** Extract shared GIC logic, introduce `GicVersion` enum, and
implement the GICv3 Distributor register set with affinity routing.

### 1.1 Introduce `GicVersion` and shared types

```
arm/gic/
  mod.rs          — re-exports, GicVersion enum
  common.rs       — shared bitmap helpers, priority logic
  v2.rs           — current Gic struct (renamed GicV2)
  distributor.rs  — GicDistributor (shared GICD state)
  v3.rs           — GicV3 (new)
```

- `GicVersion { V2, V3, V4 }` — used by `DtbConfig` and constructors.
- Move bitmap helpers (`is_enabled`, `set_pending`, `clear_pending`,
  `highest_pending`) into `common.rs`.
- `GicV2` keeps its current single-struct layout; existing tests stay
  green.

### 1.2 GICv3 Distributor (GICD) deltas

New / changed registers (vs GICv2):

| Register | Offset | Change in v3 |
|----------|--------|--------------|
| `GICD_CTLR` | 0x000 | Adds ARE\_S, ARE\_NS, DS bits |
| `GICD_TYPER` | 0x004 | Adds LPIS, MBIS, num\_LPIs, ESPI fields |
| `GICD_IROUTER<n>` | 0x6100+ | 64-bit affinity routing (replaces ITARGETSR) |
| `GICD_IGRPMODR<n>` | 0xD00+ | Group modifier for Grp1 NS vs Grp1 S |
| `GICD_PIDR2` | 0xFFE8 | ArchRev field = 3 |

- When ARE=1, `GICD_ITARGETSR` becomes RAZ/WI; routing uses
  `GICD_IROUTER` instead.
- SGIs (0-15) are now per-PE — handled by the Redistributor.

### 1.3 Deliverables

- [ ] `arm/gic/mod.rs` with `GicVersion` enum.
- [ ] `arm/gic/common.rs` with extracted bitmap/priority helpers.
- [ ] `arm/gic/v2.rs` — moved `Gic` → `GicV2`, same API.
- [ ] `arm/gic/distributor.rs` — shared `GicDistributor` struct
      (GICD registers), parameterised by version.
- [ ] GICD_IROUTER read/write in distributor when version ≥ V3.
- [ ] All existing GICv2 tests pass unchanged.
- [ ] New tests in `tests/gic.rs` for v3 distributor registers.

---

## Phase 2 — GICv3 Redistributor (GICR)

**Goal:** Implement the per-PE Redistributor that manages SGIs, PPIs,
and (later) LPIs.

### 2.1 GICR register frames

Each PE gets two 64 KB frames:

| Frame | Offset | Key registers |
|-------|--------|---------------|
| RD_base | +0x00000 | GICR\_CTLR, GICR\_TYPER, GICR\_WAKER, GICR\_PROPBASER, GICR\_PENDBASER |
| SGI\_base | +0x10000 | GICR\_ISENABLER0, GICR\_ICENABLER0, GICR\_ISPENDR0, GICR\_IPRIORITYR0-7 |

PE N lives at `gicr_base + N * 0x20000`.

### 2.2 `GicRedistributor` struct

```rust
pub struct GicRedistributor {
    pe_id: u32,
    affinity: u64,           // Aff3.Aff2.Aff1.Aff0
    waker: u32,              // GICR_WAKER
    sgi_ppi_enabled: u32,    // 32 bits for IRQs 0-31
    sgi_ppi_pending: u32,
    sgi_ppi_priority: [u8; 32],
    sgi_ppi_config: u32,     // GICR_ICFGR0/1
    // LPI fields (Phase 4)
    prop_baser: u64,
    pend_baser: u64,
}
```

- `transact()` handles both RD\_base and SGI\_base frames based on
  offset within the PE's 128 KB window.
- `GICR_TYPER` encodes affinity, PE number, and `Last` bit for the
  final PE.
- `GICR_WAKER.ProcessorSleep` / `ChildrenAsleep` — simple state
  machine for PE online/offline.

### 2.3 Wire into `GicV3`

```rust
pub struct GicV3 {
    distributor: GicDistributor,
    redistributors: Vec<GicRedistributor>,  // one per PE
    num_pes: u32,
    irq_signals: Vec<Option<IrqSignal>>,    // one per PE
}
```

`GicV3` implements `Device` with a combined MMIO region:

| Base offset | Size | Component |
|-------------|------|-----------|
| 0x0000_0000 | 64 KB | Distributor |
| 0x000A_0000 | N×128 KB | Redistributors (QEMU virt layout) |

### 2.4 Deliverables

- [ ] `arm/gic/redistributor.rs` — `GicRedistributor` struct.
- [ ] GICR register read/write for RD\_base + SGI\_base frames.
- [ ] `GicV3` struct wiring distributor + redistributors.
- [ ] SGI/PPI enable, pending, priority via GICR.
- [ ] `GICR_WAKER` sleep/wake protocol.
- [ ] Tests: GICR\_TYPER encoding, SGI enable/pending via GICR,
      multi-PE discovery (walk `Last` bit).

---

## Phase 3 — System Register Interface (ICC)

**Goal:** Expose the CPU interface as system registers instead of MMIO,
matching how real GICv3 software interacts with the GIC.

### 3.1 ICC system registers

| Sysreg | Op0 | Op1 | CRn | CRm | Op2 | Function |
|--------|-----|-----|-----|-----|-----|----------|
| ICC\_IAR1\_EL1 | 3 | 0 | 12 | 12 | 0 | Acknowledge (Grp1) |
| ICC\_EOIR1\_EL1 | 3 | 0 | 12 | 12 | 1 | End of interrupt |
| ICC\_PMR\_EL1 | 3 | 0 | 4 | 6 | 0 | Priority mask |
| ICC\_CTLR\_EL1 | 3 | 0 | 12 | 12 | 4 | Control |
| ICC\_SRE\_EL1 | 3 | 0 | 12 | 12 | 5 | Sysreg enable (RAO) |
| ICC\_IGRPEN1\_EL1 | 3 | 0 | 12 | 12 | 7 | Group 1 enable |
| ICC\_SGI1R\_EL1 | 3 | 0 | 12 | 11 | 5 | Generate SGI |
| ICC\_BPR1\_EL1 | 3 | 0 | 12 | 12 | 3 | Binary point |

### 3.2 Integration with `helm-isa` / `helm-engine`

- Add a `SysRegAccess` trait or callback in `helm-engine` so the CPU
  model can delegate `MSR`/`MRS` of ICC registers to the GIC.
- `GicV3` implements a `fn sysreg_read(&mut self, pe: u32, reg: IccReg) -> u64`
  and `sysreg_write(...)` interface.
- `ICC_SRE_EL1.SRE` is hardwired to 1 (system-register-only mode),
  meaning the legacy MMIO CPU interface is disabled.

### 3.3 ICC\_SGI1R — software-generated interrupts

Writing `ICC_SGI1R_EL1` generates an SGI to target PEs:

```
Bits [55:48] = Aff3, [39:32] = Aff2, [23:16] = Aff1
Bits [15:0]  = TargetList (one bit per PE in the affinity group)
Bits [27:24] = INTID (SGI number 0-15)
Bit  [40]    = IRM (1 = all PEs except self)
```

The write must set the SGI pending bit in each targeted PE's
redistributor.

### 3.4 Priority drop / deactivate split

GICv3 separates priority drop (IAR read) from deactivate (EOIR write)
when `ICC_CTLR_EL1.EOImode=1`. This is important for hypervisors.

- Maintain an "active priorities" list per PE.
- IAR read: drop running priority, return INTID.
- EOIR write: deactivate interrupt (clear active bit).

### 3.5 Update `InterruptController` trait

The current trait is too GICv2-centric. Proposed changes:

```rust
pub trait InterruptController: Device {
    fn inject(&mut self, irq: u32, level: bool);
    fn pending_for_cpu(&self, cpu_id: u32) -> bool;
    fn ack(&mut self, cpu_id: u32) -> Option<u32>;

    // New for GICv3+
    fn sysreg_read(&mut self, _pe: u32, _reg: u32) -> Option<u64> { None }
    fn sysreg_write(&mut self, _pe: u32, _reg: u32, _val: u64) -> bool { false }
}
```

Default impls return `None`/`false` so GICv2 is unaffected.

### 3.6 Deliverables

- [ ] `arm/gic/icc.rs` — ICC register handling per PE.
- [ ] `sysreg_read` / `sysreg_write` on `GicV3`.
- [ ] SGI generation via `ICC_SGI1R_EL1`.
- [ ] Active priority tracking, EOImode support.
- [ ] `InterruptController` trait extension (backward-compatible).
- [ ] Tests: sysreg IAR/EOIR cycle, SGI delivery to multi-PE,
      priority drop vs deactivate, `ICC_SRE` returns RAO.

---

## Phase 4 — LPI & ITS (Interrupt Translation Service)

**Goal:** Support Locality-specific Peripheral Interrupts and the ITS
for PCIe MSI-X translation.

### 4.1 LPI overview

LPIs are message-based, edge-triggered interrupts with IDs ≥ 8192.
Configuration and pending state live in memory (not registers):

- **LPI Configuration Table** — pointed to by `GICR_PROPBASER`.
  1 byte per LPI: priority (bits [7:2]), enable (bit 0).
- **LPI Pending Table** — pointed to by `GICR_PENDBASER`.
  1 bit per LPI.

The redistributor reads these tables from guest memory.

### 4.2 ITS

The ITS translates `(DeviceID, EventID)` → `(INTID, Collection)` →
`(target PE, LPI)` using in-memory tables:

| Table | Indexed by | Contains |
|-------|-----------|----------|
| Device Table | DeviceID | Pointer to Interrupt Translation Table |
| ITT | EventID | INTID + Collection ID |
| Collection Table | Collection ID | Target redistributor |

Key ITS registers:

| Register | Offset | Purpose |
|----------|--------|---------|
| GITS\_CTLR | 0x000 | Enable ITS |
| GITS\_TYPER | 0x008 | Capabilities |
| GITS\_CBASER | 0x080 | Command queue base |
| GITS\_CWRITER | 0x088 | Command queue write pointer |
| GITS\_CREADR | 0x090 | Command queue read pointer |
| GITS\_BASER<n> | 0x100+ | Table base addresses |

ITS commands (written to the command queue):

- `MAPD` — map DeviceID → ITT
- `MAPTI` — map (DeviceID, EventID) → (INTID, CollectionID)
- `MAPI` — shorthand MAPTI where INTID = EventID
- `MAPC` — map CollectionID → target PE
- `INV` / `INVALL` — invalidate cached config
- `INT` — inject an interrupt
- `SYNC` — ensure all prior commands for a PE have taken effect

### 4.3 Struct outline

```rust
pub struct GicIts {
    ctrl: u32,
    typer: u64,
    cmd_queue_base: u64,
    cmd_write: u64,
    cmd_read: u64,
    table_bases: [u64; 8],
    // Cached translation state:
    device_table: BTreeMap<u32, Vec<IttEntry>>,
    collections: BTreeMap<u16, u32>,  // CollectionID → target PE
}

struct IttEntry {
    event_id: u32,
    intid: u32,
    collection: u16,
}
```

### 4.4 Integration

- ITS is a separate MMIO device placed at `gits_base` (e.g.
  `0x0808_0000` in QEMU virt layout).
- When a PCIe device writes its MSI-X doorbell, the bus translates
  that into an ITS `INT` command or direct `inject()` call.
- The ITS resolves the target PE and sets the LPI pending bit in the
  redistributor's pending table.

### 4.5 Deliverables

- [ ] `arm/gic/lpi.rs` — LPI table read/write helpers.
- [ ] `arm/gic/its.rs` — `GicIts` device with command queue
      processing.
- [ ] `GICR_PROPBASER` / `GICR_PENDBASER` handling in redistributor.
- [ ] `MAPD`, `MAPTI`, `MAPI`, `MAPC`, `INT`, `SYNC` commands.
- [ ] Wire ITS into `GicV3` composite device.
- [ ] Tests: LPI enable/pending via tables, ITS command sequence
      mapping a device and triggering an LPI, collection re-targeting.

---

## Phase 5 — Platform Integration & DTB

**Goal:** Create `arm_virt_v3_platform()`, update FDT generation, and
wire everything into the engine.

### 5.1 Platform builder

```rust
pub fn arm_virt_v3_platform(
    uart_backend: Box<dyn CharBackend>,
    num_cpus: u32,
    irq_signals: Vec<IrqSignal>,
) -> Platform { ... }
```

Memory map (QEMU virt compatible):

| Base | Size | Component |
|------|------|-----------|
| `0x0800_0000` | 64 KB | GICv3 Distributor |
| `0x080A_0000` | N×128 KB | GICv3 Redistributors |
| `0x0808_0000` | 128 KB | ITS (optional) |
| `0x0900_0000` | — | APB peripherals (PL011, etc.) |

### 5.2 DTB updates

When `gic_version == 3`:

- Set `compatible = "arm,gic-v3"` (already done).
- Add GICR reg entry: `(gicr_base, num_cpus * 0x20000)`.
- Add `redistributor-stride` property.
- If ITS is present, add `/intc/its@...` child node with
  `compatible = "arm,gic-v3-its"`.

Extend `DtbConfig`:

```rust
pub struct DtbConfig {
    // ... existing fields ...
    pub gic_redist_base: u64,   // NEW
    pub gic_its_base: Option<u64>, // NEW
}
```

### 5.3 Engine integration

- `helm-engine` needs to intercept `MRS`/`MSR` to ICC system
  registers and dispatch to the GIC device.
- Add a `GicHandle` (index or `Arc<Mutex<GicV3>>`) to the engine's
  per-vCPU state.
- On each instruction-complete or exception-check path, poll
  `pending_for_cpu()` to check for virtual IRQ assertion.

### 5.4 KVM pass-through

When running under KVM (`helm-kvm`):

- Create `KVM_DEV_TYPE_ARM_VGIC_V3` via `KVM_CREATE_DEVICE`.
- Set `KVM_VGIC_V3_ADDR_TYPE_DIST` and `KVM_VGIC_V3_ADDR_TYPE_REDIST`.
- IRQ injection uses `KVM_IRQ_LINE` or `KVM_SIGNAL_MSI`.
- No user-space MMIO emulation needed — KVM handles it in-kernel.

### 5.5 Deliverables

- [ ] `arm_virt_v3_platform()` builder.
- [ ] `DtbConfig` extensions and DTB skeleton updates for GICv3.
- [ ] Engine sysreg dispatch hook.
- [ ] KVM GICv3 creation in `helm-kvm`.
- [ ] Integration test: boot Linux kernel with GICv3 DTB, verify
      UART IRQ delivery.

---

## Phase 6 — GICv4 Virtual Interrupts

**Goal:** Add GICv4 virtual LPI injection and GICv4.1 vSGI support
for direct interrupt delivery to VMs.

### 6.1 GICv4 additions

GICv4 extends the ITS with virtual interrupt mapping:

- **vPE Table** — maps virtual PE IDs to physical PEs.
- **vLPI** — virtual LPIs delivered directly to a vPE without
  hypervisor trap.
- New ITS commands: `VMAPP`, `VMAPTI`, `VMOVI`, `VINVALL`, `VSYNC`.

### 6.2 GICv4.1 additions

- **vSGI** — virtual SGIs delivered directly (no trap).
- `GICR_VSGIR` — per-redistributor vSGI register.
- `VSGI` ITS command.
- Enhanced `VMAPP` with `V` (valid) and `Default_DoorBell` fields.

### 6.3 `GicV4` struct

```rust
pub struct GicV4 {
    inner: GicV3,                       // all v3 functionality
    vpe_table: BTreeMap<u32, VpeEntry>,  // vPE ID → config
    version: GicV4Version,              // V4 or V4_1
}

enum GicV4Version { V4, V4_1 }

struct VpeEntry {
    vpe_id: u32,
    target_pe: u32,
    vlpi_pending_base: u64,
    vlpi_config_base: u64,
    resident: bool,
    // v4.1:
    vsgi_config: [u8; 16],  // vSGI priority/enable for SGIs 0-15
}
```

### 6.4 vLPI flow

1. Hypervisor issues `VMAPTI` → ITS creates virtual mapping
   `(DeviceID, EventID) → (vINTID, vPE)`.
2. Device writes MSI doorbell → ITS looks up virtual mapping.
3. If vPE is resident on this PE, inject vLPI directly (set pending
   in virtual pending table, signal via `ICH_LR`).
4. If vPE is not resident, set doorbell pending for later delivery.

### 6.5 Deliverables

- [ ] `arm/gic/v4.rs` — `GicV4` wrapping `GicV3`.
- [ ] vPE table management (`VMAPP`, `VMOVI`).
- [ ] `VMAPTI` / `VMAPI` — virtual LPI mapping in ITS.
- [ ] vLPI injection path: resident vs non-resident vPE.
- [ ] GICv4.1: vSGI support (`VSGI` command, `GICR_VSGIR`).
- [ ] `GICD_TYPER.DVIS` and `GITS_TYPER.Virtual` capability bits.
- [ ] Tests: vLPI map-and-inject, vPE schedule/deschedule,
      vSGI delivery (v4.1), doorbell on non-resident vPE.

---

## Phase 7 — Hardening & Completeness

**Goal:** Polish, edge cases, and feature parity with QEMU's GICv3.

### Deliverables

- [ ] Security state support: Group 0 / Group 1 Secure / Group 1 NS.
- [ ] `ICH_*` system registers for hypervisor (EL2) list registers.
- [ ] GICD\_STATUSR, GICR\_STATUSR error reporting.
- [ ] Extended SPI / Extended PPI ranges (GICv3.1, INTID 4096+).
- [ ] Checkpoint / restore for full GIC state.
- [ ] Performance: fast-path `read_fast` / `write_fast` for hot
      registers (IAR, EOIR, PMR).
- [ ] Python bindings: expose GIC version selection in `helm-python`.
- [ ] Documentation: update `docs/device-authoring.md` with GIC
      examples.

---

## Dependency Graph

```
Phase 1 ─── Phase 2 ─── Phase 3 ─── Phase 5
                │             │          │
                └─── Phase 4 ─┘          │
                                         │
                     Phase 6 ────────────┘
                         │
                     Phase 7
```

- Phases 1-3 are the critical path for a working GICv3.
- Phase 4 (LPI/ITS) can start after Phase 2 (needs redistributor).
- Phase 5 (platform integration) needs Phases 1-4.
- Phase 6 (GICv4) needs Phase 5.
- Phase 7 can run in parallel with Phase 6.

## Test Strategy

Every phase adds tests in `src/tests/gic.rs` (split into sub-modules
as needed). Following the project TDD convention:

1. **Unit tests** per register bank — verify read/write semantics in
   isolation (distributor, redistributor, ICC, ITS).
2. **Integration tests** — wire a `GicV3` into a `Platform`, inject
   IRQs from a device, verify pending / ack / EOI cycle.
3. **Multi-PE tests** — verify affinity routing, SGI fan-out, and
   per-PE GICR discovery.
4. **LPI/ITS tests** — command queue processing, translation
   lookup, LPI pending via memory tables.
5. **GICv4 tests** — vLPI injection, vPE schedule/deschedule,
   doorbell.

Each phase gates on `make pre-commit` (fmt + clippy + test).
