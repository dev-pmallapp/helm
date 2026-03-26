# GICv3 — High-Level Design

> Cross-references: [`ARCHITECTURE.md`](../../../ARCHITECTURE.md) · [`HLD.md`](../../HLD.md) · [`object-model.md`](../../../object-model.md) · [`traits.md`](../../../traits.md) · [`LLD-gicv3.md`](./LLD-gicv3.md) · [`helm-hw-intc gicv2`](../../../../hw/helm-hw-intc/src/gicv2/)

---

## Table of Contents

1. [Why GICv3 — Advances Over GICv2](#1-why-gicv3--advances-over-gicv2)
2. [GICv3 Register Map](#2-gicv3-register-map)
3. [Interrupt Types and ID Ranges](#3-interrupt-types-and-id-ranges)
4. [LPI Frame and Interrupt Translation Service (ITS)](#4-lpi-frame-and-interrupt-translation-service-its)
5. [Affinity Routing](#5-affinity-routing)
6. [Integration with helm-ng](#6-integration-with-helm-ng)
7. [Work Items](#7-work-items)

---

## 1. Why GICv3 — Advances Over GICv2

GICv2 is the ARM interrupt controller used in ARM Cortex-A7/A15 era systems and the original QEMU `virt` machine (for guests not requesting GICv3). It has three fundamental limitations that GICv3 removes:

### 1.1 CPU Interface: MMIO → System Registers

GICv2 exposes a CPU interface as an MMIO region (`GICC_*` registers) that each core must access via load/store instructions. This is slow — every interrupt acknowledgment and EOI is a memory-mapped operation that cannot be optimised by the CPU pipeline.

GICv3 replaces GICC with **ICC_* system registers** (accessible via `MRS`/`MSR` instructions). These are accessed in the same clock domain as the CPU core, far faster, and do not require an MMIO region. The distributor (GICD) and redistributors (GICR) remain memory-mapped.

### 1.2 Redistributor Per PE

GICv2 has a single monolithic CPU interface region and targets interrupts to CPUs via an 8-bit target mask in `GICD_ITARGETSR`. This limits SMP to 8 cores and requires serialisation through the distributor for per-CPU interrupt state.

GICv3 introduces a **redistributor (GICR)** per PE (Processing Element / CPU). Each redistributor owns:
- Per-PE SGI and PPI enable/pending/active/priority registers (formerly banked in GICD)
- LPI pending table pointer (for LPI delivery to this PE)
- `WAKER` register to wake the PE from `WFI`

The redistributors are memory-mapped in a contiguous array: `GICR_BASE + cpu_idx * 0x20000` (two 64KB frames per PE: `RD_base` + `SGI_base`).

### 1.3 Locality-Specific Peripheral Interrupts (LPIs) and ITS

GICv2 has no concept of message-signalled interrupts (MSI) for peripheral devices like PCIe endpoints. GICv3 introduces **LPIs** (INTID 8192+) with:
- An **ITS** (Interrupt Translation Service) that translates `(DeviceID, EventID)` pairs into target-PE + INTID tuples
- Device tables and collection tables in RAM, not in GIC registers
- An LPI pending table per PE in RAM

This enables PCIe MSI/MSI-X without a dedicated MSI controller, and scales to thousands of interrupt sources.

### 1.4 Affinity Routing (ARE)

GICv2 uses an 8-bit CPU target mask (`GICD_ITARGETSR`). GICv3 uses MPIDR-based affinity routing when `GICD_CTLR.ARE_NS=1`:
- SPI routing: `GICD_IROUTER<n>` holds a 64-bit affinity value (Aff3.Aff2.Aff1.Aff0) matching MPIDR
- SGI routing: `ICC_SGI1R_EL1` encodes target affinity + SGI ID
- Enables routing to any PE in any cluster without the 8-core limit

### Summary of Differences

| Feature | GICv2 | GICv3 |
|---------|-------|-------|
| CPU interface | MMIO GICC region | ICC_* system registers |
| Per-CPU state | Banked in GICD | Dedicated GICR per PE |
| Max SPI targets | 8 CPUs (target mask) | Any PE via MPIDR affinity |
| MSI/MSI-X | Not supported natively | LPIs + ITS |
| Max INTIDs | 1020 (INTID 1020 reserved) | 8192+ for LPIs |
| SGI generation | GICD_SGIR (MMIO write) | ICC_SGI1R_EL1 (sysreg write) |

---

## 2. GICv3 Register Map

### 2.1 Physical Address Layout (QEMU virt-compatible)

```
GICD_BASE    0x0800_0000   Distributor                    64KB
             0x0801_0000   Reserved (GICv3 GICD is 64KB)
GICR_BASE    0x080A_0000   Redistributor frame, CPU 0    128KB (RD_base + SGI_base)
             0x080C_0000   Redistributor frame, CPU 1    128KB
             ...
             GICR_BASE + cpu_idx * 0x20000
```

No `GICC_*` MMIO region exists in GICv3. The CPU interface is entirely ICC_* system registers.

### 2.2 GICD — Distributor Register Map

The distributor manages SPI (INTID 32–1019) routing and global enable.

```
Offset   Register                 Width   Description
0x0000   GICD_CTLR                32      Global enable; ARE_NS/ARE_S bits
0x0004   GICD_TYPER               32      ITLinesNumber, MBIS, ESPI, IDbits
0x0008   GICD_IIDR                32      Implementer ID
0x000C   GICD_TYPER2              32      Extended SPI range (v3.1+)
0x0010   GICD_STATUSR             32      Error reporting
0x0040   GICD_SETSPI_NSR          32      Set SPI (non-secure)
0x0048   GICD_CLRSPI_NSR          32      Clear SPI (non-secure)
0x0080   GICD_SETSPI_SR           32      Set SPI (secure)
0x0088   GICD_CLRSPI_SR           32      Clear SPI (secure)
0x0100   GICD_IGROUPR[0..31]      32×32   Group 0/1 assignment per SPI
0x0180   GICD_ISENABLER[0..31]    32×32   Set enable (write 1 to enable)
0x0200   GICD_ICENABLER[0..31]    32×32   Clear enable (write 1 to disable)
0x0280   GICD_ISPENDR[0..31]      32×32   Set pending
0x0300   GICD_ICPENDR[0..31]      32×32   Clear pending
0x0380   GICD_ISACTIVER[0..31]    32×32   Set active
0x0400   GICD_ICACTIVER[0..31]    32×32   Clear active
0x0400   GICD_IPRIORITYR[0..254]  32×254  Priority bytes (4 per word)
0x0800   (GICD_ITARGETSR — only in GICv3 compatibility mode, ARE=0)
0x6000   GICD_IROUTER[32..1019]   64×988  64-bit affinity routing (ARE=1 only)
0xFFD0   GICD_PIDR2               32      Peripheral ID2: GICv3 = 0x3x
0xFFE8   GICD_PIDR4               32      Peripheral ID4
```

Key `GICD_CTLR` fields (non-secure view):
```
Bit 31   RWP     Register Write Pending (1 = write in progress)
Bit  4   ARE_NS  Affinity Routing Enable (non-secure)
Bit  1   EnableGrp1NS  Enable Group 1 non-secure interrupts
Bit  0   EnableGrp1S   Enable Group 1 secure interrupts
```

### 2.3 GICR — Redistributor Register Map

Each PE has a 128KB redistributor frame split into two 64KB sub-frames:

**RD_base** (offset 0x0000 within the PE's GICR frame):
```
Offset   Register          Width   Description
0x0000   GICR_CTLR         32      Enable LPIs, DPG* bits
0x0004   GICR_IIDR         32      Implementer ID
0x0008   GICR_TYPER        64      PE identity (Affinity, VLPIS, RVPEID, Last)
0x0010   GICR_STATUSR      32      Error status
0x0014   GICR_WAKER        32      ProcessorSleep, ChildrenAsleep
0x0040   GICR_SETLPIR      64      Set LPI pending (direct injection)
0x0048   GICR_CLRLPIR      64      Clear LPI pending
0x0070   GICR_PROPBASER    64      LPI configuration table base address + attr
0x0078   GICR_PENDBASER    64      LPI pending table base address + attr
0xFFD0   GICR_PIDR2        32      Peripheral ID2: matches GICD_PIDR2
```

**SGI_base** (offset 0x10000 within the PE's GICR frame):
```
Offset   Register              Width   Description
0x0080   GICR_IGROUPR0         32      Group assignment: SGI 0-15, PPI 16-31
0x0100   GICR_ISENABLER0       32      Set enable: SGI/PPI (write 1)
0x0180   GICR_ICENABLER0       32      Clear enable: SGI/PPI (write 1)
0x0200   GICR_ISPENDR0         32      Set pending: SGI/PPI
0x0280   GICR_ICPENDR0         32      Clear pending: SGI/PPI
0x0300   GICR_ISACTIVER0       32      Set active: SGI/PPI
0x0380   GICR_ICACTIVER0       32      Clear active: SGI/PPI
0x0400   GICR_IPRIORITYR[0-7]  32×8    Priority bytes for SGI/PPI (4 per word)
0x0C00   GICR_ICFGR0           32      SGI 0-15 edge/level config
0x0C04   GICR_ICFGR1           32      PPI 16-31 edge/level config
```

### 2.4 ICC_* System Registers (CPU Interface)

There is no MMIO region. The CPU interface is accessed exclusively via AArch64 system register instructions. The registers are banked per-EL where applicable:

```
Register          EL access   Op0  Op1  CRn   CRm   Op2   Description
ICC_SRE_EL1       EL1 RW      3    0    12    12    5     Enable sysreg interface, disable FIQ
ICC_SRE_EL2       EL2 RW      3    4    12    9     5     Hypervisor sysreg enable
ICC_SRE_EL3       EL3 RW      3    6    12    12    5     Monitor sysreg enable
ICC_IAR1_EL1      EL1 RO      3    0    12    12    0     Interrupt Acknowledge (Group 1)
ICC_EOIR1_EL1     EL1 WO      3    0    12    12    1     End of Interrupt (Group 1)
ICC_HPPIR1_EL1    EL1 RO      3    0    12    12    2     Highest Priority Pending IRQ (Grp 1)
ICC_BPR1_EL1      EL1 RW      3    0    12    12    3     Binary Point (Group 1)
ICC_DIR_EL1       EL1 WO      3    0    12    11    1     Deactivate interrupt
ICC_PMR_EL1       EL1 RW      3    0    4     6     0     Priority Mask Register
ICC_RPR_EL1       EL1 RO      3    0    12    11    3     Running Priority
ICC_CTLR_EL1      EL1 RW      3    0    12    12    4     Control (EOImode, CBPR, etc.)
ICC_CTLR_EL3      EL3 RW      3    6    12    12    4     Control at EL3
ICC_SGI1R_EL1     EL1 WO      3    0    12    11    5     Generate Group 1 SGI
ICC_ASGI1R_EL1    EL1 WO      3    0    12    11    6     Generate Group 1 SGI (alias)
ICC_SGI0R_EL1     EL1 WO      3    0    12    11    7     Generate Group 0 SGI
ICC_AP1R0_EL1     EL1 RW      3    0    12    9     0     Active Priorities Group 1 (bank 0)
ICC_IGRPEN0_EL1   EL1 RW      3    0    12    12    6     Enable Group 0
ICC_IGRPEN1_EL1   EL1 RW      3    0    12    12    7     Enable Group 1 (NS)
ICC_IGRPEN1_EL3   EL3 RW      3    6    12    12    7     Enable Group 1 (S+NS)
```

---

## 3. Interrupt Types and ID Ranges

| Type | INTID Range  | Source                     | Routing                       |
|------|-------------|----------------------------|-------------------------------|
| SGI  | 0–15        | CPU-generated (IPI)        | `ICC_SGI1R_EL1` → GICR targets|
| PPI  | 16–31       | Per-PE peripherals (timers)| Always delivered to owning PE |
| SPI  | 32–1019     | Shared peripherals (UART)  | `GICD_IROUTER` or target mask |
| SPI  | 1020–1023   | Special/reserved           | INTID 1023 = spurious         |
| LPI  | 8192+       | MSI (PCIe, ITS-routed)     | ITS device table → PE         |

PPIs in GICv3 that differ from GICv2:
- Managed in GICR SGI_base registers, not in GICD banked registers
- INTID 17: VCPU maintenance interrupt (hypervisor use)
- INTID 26: Hypervisor timer (CNTHP)
- INTID 27: Virtual timer (CNTV)
- INTID 29: Non-secure physical timer (CNTP)
- INTID 30: Non-secure physical timer (EL1)

---

## 4. LPI Frame and Interrupt Translation Service (ITS)

### 4.1 LPI Overview

LPIs (INTID ≥ 8192) are edge-triggered, always Group 1 non-secure, always targeting a specific PE. Their configuration is in RAM, not in GIC registers:

- **LPI configuration table** (pointed to by `GICR_PROPBASER`): one byte per LPI → `{priority[7:2], reserved, group, enable}`. Size = 2^(IDbits+1) bytes.
- **LPI pending table** (pointed to by `GICR_PENDBASER`): one bit per LPI, 64KB minimum. Lives in cacheable memory.

### 4.2 Interrupt Translation Service (ITS)

The ITS translates PCIe MSI writes into GIC LPI assertions. It is optional but required for PCIe MSI-X support.

**ITS MMIO layout** (typically mapped at `GITS_BASE`):

```
Offset   Register           Width   Description
0x0000   GITS_CTLR          32      Enable ITS
0x0004   GITS_IIDR          32      Implementer ID
0x0008   GITS_TYPER         64      Supported features (PTA, HCC, VMOVP, etc.)
0x0080   GITS_CBASER        64      Command queue base address + size
0x0088   GITS_CWRITER       64      Command queue write pointer
0x0090   GITS_CREADR        64      Command queue read pointer
0x0100   GITS_BASER[0]      64      Device table base (TYPE=DEVICE)
0x0108   GITS_BASER[1]      64      Collection table base (TYPE=COLLECTION)
0x0110   GITS_BASER[2..7]   64×6    Additional tables (implementation-defined)
0x10000  GITS_TRANSLATER    32      Write-only; triggers translation
```

**ITS Tables (in RAM)**:
- **Device table**: maps DeviceID → ITT (Interrupt Translation Table) base
- **ITT per device**: maps EventID → (INTID, Collection)
- **Collection table**: maps CollectionID → target PE (redistributor)

**ITS command queue**: ring buffer of 32-byte commands:

```
Command         Function
MAPD            Map a DeviceID to an ITT
MAPC            Map a CollectionID to a target redistributor
MAPTI           Map an EventID → INTID + Collection for a device
INV             Invalidate a single mapping in the ITS cache
INVALL          Invalidate all cached state for a collection
SYNC            Ensure all previous commands are visible
CLEAR           Clear pending state for an EventID
DISCARD         Unmap an EventID
MOVI            Move an interrupt from one collection to another
```

**Translation flow** (PCIe MSI write → IRQ delivery):
```
1. PCIe device writes to GITS_TRANSLATER with EventID
2. ITS reads DeviceID from the AXI transaction attribute (stream ID)
3. ITS looks up Device table → ITT base
4. ITS looks up ITT[EventID] → (INTID, CollectionID)
5. ITS looks up Collection table[CollectionID] → target GICR PE
6. ITS sets LPI pending bit in the target PE's GICR_PENDBASER table
7. GICR signals IRQ line to the PE
```

---

## 5. Affinity Routing

### 5.1 GICD_CTLR.ARE

When `GICD_CTLR.ARE_NS=1` (affinity routing enable, the GICv3 default), SPI routing uses `GICD_IROUTER<n>` instead of `GICD_ITARGETSR`. The 64-bit `GICD_IROUTER` field encodes:

```
Bits  63    Interrupt_Routing_Mode (IRM): 1 = any PE, 0 = specific PE
Bits  39:32 Aff3 (MPIDR_EL1[39:32])
Bits  23:16 Aff2 (MPIDR_EL1[23:16])
Bits  15:8  Aff1 (MPIDR_EL1[15:8])
Bits   7:0  Aff0 (MPIDR_EL1[7:0])
```

The GIC matches the affinity value against each PE's `MPIDR_EL1` to find the target redistributor. When `IRM=1`, the interrupt may be delivered to any online PE (1-of-N routing).

### 5.2 SGI Affinity (ICC_SGI1R_EL1)

```
Bits  55:48  Aff3
Bits  47:44  RS (range selector, for >16 targets in one cluster)
Bits  43:40  Aff2
Bits  39:32  Aff1
Bit   40     IRM (1 = broadcast to all except self)
Bits  15:0   TargetList (one bit per Aff0 value 0..15 within the cluster)
Bits  27:24  INTID (SGI ID, 0-15)
```

Each set bit in `TargetList` selects the PE with `Aff0 = bit_position` within the cluster identified by `{Aff3, Aff2, Aff1}`.

### 5.3 MPIDR in helm-ng

`HelmVcpu` exposes `MPIDR_EL1` as a system register. The GICR per-PE `GICR_TYPER` register carries the PE's affinity value at bits `[63:32]`, derived from the vCPU's MPIDR at build time. The distributor uses this to match `GICD_IROUTER` values during SPI delivery.

---

## 6. Integration with helm-ng

### 6.1 Module Plan

```
hw/helm-hw-intc/src/
├── gicv2/          (existing)
│   ├── mod.rs
│   ├── distributor.rs
│   └── cpu_interface.rs
└── gicv3/          (new)
    ├── mod.rs          — GicV3SharedState, build_gicv3(), build_gicv3_mp()
    ├── distributor.rs  — Gicv3Distributor (Device trait, GICD MMIO)
    ├── redistributor.rs— Gicv3Redistributor (Device trait, GICR per-PE MMIO)
    ├── sysregs.rs      — ICC_* system register handlers (called from helm-arch sysreg.rs)
    └── its.rs          — ItsState, ITS MMIO, command queue (Phase 2)
```

### 6.2 Key Struct Plan

```rust
/// Shared GICv3 state: one distributor + N redistributors.
pub struct GicV3SharedState {
    pub dist: Gicv3DistState,
    pub redists: Vec<Gicv3RedistState>,  // one per vCPU
    pub its: Option<ItsState>,           // Phase 2
}

/// Per-vCPU CPU interface state (held in GicV3SharedState.redists).
pub struct Gicv3CpuIfState {
    pub icc_pmr:     u8,    // ICC_PMR_EL1
    pub icc_bpr1:    u8,    // ICC_BPR1_EL1
    pub icc_ctlr:    u32,   // ICC_CTLR_EL1
    pub icc_sre:     u32,   // ICC_SRE_EL1 (SRE bit must be 1)
    pub icc_igrpen1: u32,   // ICC_IGRPEN1_EL1
    pub running_pri: u8,    // highest active interrupt priority
    pub active_stack: Vec<(u32, u8)>, // (INTID, priority) deactivation stack
}

/// Per-vCPU redistributor MMIO state.
pub struct Gicv3RedistState {
    pub cpu_if: Gicv3CpuIfState,
    pub ctlr: u32,          // GICR_CTLR
    pub waker: u32,         // GICR_WAKER
    pub propbaser: u64,     // GICR_PROPBASER
    pub pendbaser: u64,     // GICR_PENDBASER
    // Banked SGI/PPI state:
    pub sgi_ppi_enabled: u32,
    pub sgi_ppi_pending: u32,
    pub sgi_ppi_active:  u32,
    pub sgi_ppi_priority: [u8; 32],
    pub sgi_ppi_group:   u32,
    pub sgi_ppi_config:  [u32; 2],
    // IRQ line to vCPU step loop:
    pub irq_line: Arc<AtomicBool>,
    // Affinity (matches MPIDR_EL1):
    pub affinity: u64,
}

/// Distributor state (SPI-only in affinity routing mode).
pub struct Gicv3DistState {
    pub ctlr: u32,
    pub typer: u32,
    pub enabled: Vec<u32>,          // ITLinesNumber+1 words
    pub pending: Vec<u32>,
    pub active:  Vec<u32>,
    pub priority: Vec<u8>,          // one byte per INTID
    pub group:   Vec<u32>,
    pub config:  Vec<u32>,
    pub irouter: Vec<u64>,          // one per SPI (INTID 32..1020)
    pub num_irqs: u32,
}
```

### 6.3 What Changes from GICv2

| Aspect | GICv2 | GICv3 |
|--------|-------|-------|
| CPU interface type | `Gicv2CpuInterface` (Device) | `sysregs.rs` hooks only |
| Per-CPU state home | `GicCpuState` in `GicSharedState` | `Gicv3RedistState` per vCPU |
| SGI/PPI registers | Banked in GICD offsets 0-0x1FC | In GICR SGI_base |
| SPI targeting | `targets[u8]` byte mask | `irouter[u64]` affinity |
| Spurious INTID | 1023 | 1023 (unchanged) |
| Max INTIDs | 256 (configurable) | Up to 1020 SPI + LPIs |

### 6.4 helm-arch Sysreg Hooks

The ICC_* registers must be wired into `runtime/helm-arch/src/aarch64/execute/sysregs.rs` through the existing `read_sysreg`/`write_sysreg` dispatch. The vCPU's `FsState` will carry an `Arc<Mutex<GicV3SharedState>>` reference stored during `elaborate()`. The sysreg handler calls into `gicv3::sysregs::icc_read(state, op0, op1, crn, crm, op2)` / `icc_write(...)`.

---

## 7. Work Items

- [ ] WI-001: Create `hw/helm-hw-intc/src/gicv3/mod.rs` with `GicV3SharedState`, `Gicv3DistState`, `Gicv3RedistState`, `Gicv3CpuIfState` structs
- [ ] WI-002: Implement `Gicv3Distributor` struct wrapping `Arc<Mutex<GicV3SharedState>>` with `Device` trait (GICD MMIO, all offsets per LLD)
- [ ] WI-003: Implement `Gicv3Redistributor` struct with `Device` trait (RD_base + SGI_base, 128KB region, per-PE MMIO)
- [ ] WI-004: Implement `build_gicv3(num_irqs, num_cpus)` builder returning distributor + Vec<redistributor> + Vec<irq_line>
- [ ] WI-005: Implement `sysregs.rs`: `icc_read()` / `icc_write()` handlers for all ICC_* registers in the table above
- [ ] WI-006: Wire ICC_* sysreg handlers into `helm-arch` `read_sysreg`/`write_sysreg` via `FsState` GIC ref
- [ ] WI-007: Implement `GicV3SharedState::highest_pending_for_cpu()` respecting priority mask, group enable, and running priority
- [ ] WI-008: Implement `GicV3SharedState::cpu_acknowledge()` — priority drop, active stack push
- [ ] WI-009: Implement `GicV3SharedState::cpu_eoi()` — combined priority-drop+deactivation (EOImode=0) or deactivation only (EOImode=1, ICC_DIR_EL1)
- [ ] WI-010: Implement SGI generation: `ICC_SGI1R_EL1` write → parse affinity+TargetList → set per-target GICR sgi_ppi_pending
- [ ] WI-011: Implement `GicSink` for GICv3 compatible with `helm_devices::InterruptSink` (same pattern as GICv2)
- [ ] WI-012: Wire GICR WAKER register: `ProcessorSleep` → suppress IRQ delivery; `ChildrenAsleep` status read
- [ ] WI-013: Update `HelmBoard` / arm-virt platform to support GICv3 alongside GICv2 (feature flag or config param)
- [ ] WI-014: Update DTB generation to emit GICv3 compatible node (`compatible = "arm,gic-v3"`, GICD + GICR ranges)
- [ ] WI-015: Write unit tests for priority ordering, SGI multicast, SPI affinity routing, EOImode=1 deactivation
- [ ] WI-016: Phase 2 — Implement `its.rs`: ITS MMIO, command queue ring buffer, device/collection tables
- [ ] WI-017: Phase 2 — Implement LPI configuration table reads from guest RAM via `HelmAddressSpace`
- [ ] WI-018: Phase 2 — Wire `GITS_TRANSLATER` write path (MSI delivery from PCIe endpoint simulation)
