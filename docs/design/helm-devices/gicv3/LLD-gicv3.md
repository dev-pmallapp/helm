# GICv3 — Low-Level Design

> Cross-references: [`HLD.md`](./HLD.md) · [`ARCHITECTURE.md`](../../../ARCHITECTURE.md) · [`traits.md`](../../../traits.md) · [`LLD-interrupt-model.md`](../../helm-devices/LLD-interrupt-model.md) · [`helm-hw-intc gicv2`](../../../../hw/helm-hw-intc/src/gicv2/)

---

## Table of Contents

1. [Struct Layout](#1-struct-layout)
2. [GICD Register Offsets and Fields](#2-gicd-register-offsets-and-fields)
3. [GICR Register Offsets and Fields](#3-gicr-register-offsets-and-fields)
4. [ICC_* System Register Interface](#4-icc_-system-register-interface)
5. [Priority Model](#5-priority-model)
6. [SGI Generation](#6-sgi-generation)
7. [Integration with HelmBoard](#7-integration-with-helmboard)
8. [Checkpoint Protocol](#8-checkpoint-protocol)
9. [Work Items](#9-work-items)

---

## 1. Struct Layout

### 1.1 Module Layout

```
hw/helm-hw-intc/src/gicv3/
├── mod.rs           — GicV3SharedState, build functions, GicV3Sink
├── distributor.rs   — Gicv3Distributor (Device trait, 64KB GICD MMIO)
├── redistributor.rs — Gicv3Redistributor (Device trait, 128KB GICR per PE)
├── sysregs.rs       — ICC_* read/write functions (no Device trait)
└── its.rs           — ItsState, ITS MMIO, command queue (Phase 2)
```

### 1.2 Distributor State

```rust
/// Distributor registers — manage SPIs (INTID 32..=1019).
pub struct Gicv3DistState {
    /// GICD_CTLR: EnableGrp1S, EnableGrp1NS, ARE_NS, ARE_S, RWP.
    pub ctlr: u32,
    /// GICD_TYPER: read-only, computed from num_irqs at build time.
    pub typer: u32,
    /// Maximum INTID count (rounded up to next multiple of 32, capped at 1020).
    pub num_irqs: u32,
    /// GICD_IGROUPR[n]: 1 bit per INTID, 1=Group1NS. Words for INTID 32..num_irqs.
    /// Indexed as group[(intid - 32) / 32].
    pub group: Vec<u32>,
    /// GICD_ISENABLER / GICD_ICENABLER: set/clear use same array.
    pub enabled: Vec<u32>,
    /// GICD_ISPENDR / GICD_ICPENDR.
    pub pending: Vec<u32>,
    /// GICD_ISACTIVER / GICD_ICACTIVER.
    pub active: Vec<u32>,
    /// GICD_IPRIORITYR: one byte per INTID.
    pub priority: Vec<u8>,
    /// GICD_ICFGR: 2 bits per INTID (0=level, 2=edge).
    pub config: Vec<u32>,
    /// GICD_IROUTER: 64-bit per SPI (INTID 32..num_irqs). Index = intid - 32.
    /// bit[63]=IRM (any-PE), bits[39:32]=Aff3, [23:16]=Aff2, [15:8]=Aff1, [7:0]=Aff0.
    pub irouter: Vec<u64>,
    /// Level-sensitive state: physical asserted level (not guest-visible directly).
    pub physical_level: Vec<u32>,
}
```

### 1.3 Redistributor State (per vCPU)

```rust
/// Per-vCPU redistributor and CPU interface state.
pub struct Gicv3RedistState {
    // ── GICR RD_base registers ───────────────────────────────────────────────
    /// GICR_CTLR: EnableLPIs, CES, DPG0, DPG1NS, DPG1S.
    pub ctlr: u32,
    /// GICR_WAKER: ProcessorSleep (bit1), ChildrenAsleep (bit2, RO).
    pub waker: u32,
    /// GICR_PROPBASER: LPI configuration table base (PA) + attributes.
    pub propbaser: u64,
    /// GICR_PENDBASER: LPI pending table base (PA) + attributes.
    pub pendbaser: u64,

    // ── GICR SGI_base registers (banked per PE, replacing GICD banked regs) ──
    /// GICR_IGROUPR0: group assignment for SGI (0-15) and PPI (16-31).
    pub sgi_ppi_group: u32,
    /// GICR_ISENABLER0 / GICR_ICENABLER0.
    pub sgi_ppi_enabled: u32,
    /// GICR_ISPENDR0 / GICR_ICPENDR0.
    pub sgi_ppi_pending: u32,
    /// GICR_ISACTIVER0 / GICR_ICACTIVER0.
    pub sgi_ppi_active: u32,
    /// GICR_IPRIORITYR[0..7]: 4 bytes per word, 8 words → 32 priority bytes.
    pub sgi_ppi_priority: [u8; 32],
    /// GICR_ICFGR0 (SGIs), GICR_ICFGR1 (PPIs).
    pub sgi_ppi_config: [u32; 2],

    // ── CPU interface state (ICC_* system registers) ─────────────────────────
    pub cpu_if: Gicv3CpuIfState,

    // ── IRQ line to vCPU step loop ───────────────────────────────────────────
    /// Asserted by GIC when an interrupt is pending and unmasked.
    pub irq_line: Arc<AtomicBool>,

    // ── Affinity ─────────────────────────────────────────────────────────────
    /// Packed Aff3[39:32].Aff2[23:16].Aff1[15:8].Aff0[7:0] matching MPIDR_EL1.
    pub affinity: u64,
    /// GICR_TYPER value (read-only; precomputed at build time).
    pub typer: u64,
}
```

### 1.4 CPU Interface State

```rust
/// Per-vCPU ICC_* system register state.
pub struct Gicv3CpuIfState {
    /// ICC_SRE_EL1: SRE=bit0 (must be 1 for GICv3), DFB=bit1, DIB=bit2.
    pub icc_sre_el1: u32,
    /// ICC_PMR_EL1: priority mask (0xFF = all unmasked, 0x00 = all masked).
    pub icc_pmr: u8,
    /// ICC_BPR1_EL1: binary point register (Group 1).
    pub icc_bpr1: u8,
    /// ICC_CTLR_EL1: EOImode (bit1), CBPR (bit0), A3V, SEIS, IDbits.
    pub icc_ctlr: u32,
    /// ICC_IGRPEN0_EL1: enable Group 0.
    pub icc_igrpen0: u32,
    /// ICC_IGRPEN1_EL1: enable Group 1 Non-Secure.
    pub icc_igrpen1: u32,
    /// Running priority: lowest active interrupt priority, or 0xFF if none.
    /// Updated on acknowledge (priority drop) and EOI (deactivation in EOImode=0).
    pub running_pri: u8,
    /// Active interrupt stack for deactivation tracking (EOImode=1 path).
    /// Vec of (INTID, priority). LIFO order; push on acknowledge, pop on ICC_DIR_EL1 write.
    pub active_stack: Vec<(u32, u8)>,
    /// ICC_AP1R0_EL1..ICC_AP1R3_EL1: Active Priorities bitmap (Group 1).
    /// One bit per priority group. Used for preemption tracking.
    pub active_priorities: [u32; 4],
}
```

### 1.5 Shared State and Builder

```rust
/// Combined GICv3 state: one distributor + N redistributors.
pub struct GicV3SharedState {
    pub dist: Gicv3DistState,
    /// One entry per vCPU, indexed by cpu_idx.
    pub redists: Vec<Gicv3RedistState>,
    /// ITS state (None if ITS is not configured). Phase 2.
    pub its: Option<Box<ItsState>>,
    /// Log budget for noisy SGI traces (decrements to silence after N events).
    sgi_log_budget: u32,
}

/// Build a single-CPU GICv3 instance.
pub fn build_gicv3(num_irqs: u32) -> (Gicv3Distributor, Gicv3Redistributor, Arc<AtomicBool>, Arc<Mutex<GicV3SharedState>>);

/// Build an N-CPU GICv3 instance.
pub fn build_gicv3_mp(num_irqs: u32, num_cpus: usize, affinities: &[u64])
    -> (Gicv3Distributor, Vec<Gicv3Redistributor>, Vec<Arc<AtomicBool>>, Arc<Mutex<GicV3SharedState>>);
```

### 1.6 ITS State (Phase 2)

```rust
/// ITS command queue and table state.
pub struct ItsState {
    /// GITS_CTLR: Enabled bit.
    pub ctlr: u32,
    /// GITS_CBASER: command queue base PA, size (encoded as log2-1 pages), valid.
    pub cbaser: u64,
    /// GITS_CWRITER: write pointer (byte offset into command queue).
    pub cwriter: u64,
    /// GITS_CREADR: read pointer.
    pub creadr: u64,
    /// GITS_BASER[0..7]: table base registers for device, collection, etc.
    pub baser: [u64; 8],
    /// Shadow: pending commands that have been consumed but not yet committed.
    pub pending_cmds: VecDeque<ItsCommand>,
}

pub enum ItsCommand {
    Mapd  { dev_id: u32, size: u8, itt_addr: u64, valid: bool },
    Mapc  { col_id: u16, target_addr: u64, valid: bool },
    Mapti { dev_id: u32, event_id: u32, intid: u32, col_id: u16 },
    Inv   { dev_id: u32, event_id: u32 },
    Invall{ col_id: u16 },
    Sync  { target_addr: u64 },
    Clear { dev_id: u32, event_id: u32 },
    Discard { dev_id: u32, event_id: u32 },
    Movi  { dev_id: u32, event_id: u32, col_id: u16 },
}
```

---

## 2. GICD Register Offsets and Fields

The `Gicv3Distributor::read()` and `write()` match on `offset` within the 64KB GICD region.

### 2.1 Core Registers

| Offset | Register | R/W | Notes |
|--------|----------|-----|-------|
| `0x0000` | `GICD_CTLR` | RW | Bits[4:0] only; RWP=bit31 always 0 in sim |
| `0x0004` | `GICD_TYPER` | RO | `ITLinesNumber=(num_irqs/32)-1`, `IDbits=0b00111` (1020 SPIs) |
| `0x0008` | `GICD_IIDR` | RO | `0x0102_43B4` (ARM implementer, GICv3) |
| `0x000C` | `GICD_TYPER2` | RO | 0 (no extended SPI) |
| `0x0010` | `GICD_STATUSR` | RW | Sticky error bits; 0 on reset |
| `0x0040` | `GICD_SETSPI_NSR` | WO | Writes INTID field; assert SPI pending |
| `0x0048` | `GICD_CLRSPI_NSR` | WO | Writes INTID field; clear SPI pending |
| `0xFFD0` | `GICD_PIDR2` | RO | `0x3B` (GICv3 ArchRev=3) |

### 2.2 SPI Bit-Array Registers

All use the pattern: word index `n = (intid - 32) / 32`, bit index `b = intid & 31`.
Words for INTID 0..31 are reserved/RAZ in GICD; those INTIDs live in GICR SGI_base.

| Offset Range | Register | Notes |
|---|---|---|
| `0x0100`–`0x017C` | `GICD_IGROUPR[n]` | INTID 0..31 = GICR; 32..1019 in GICD |
| `0x0180`–`0x01FC` | `GICD_ISENABLER[n]` | Write 1 to enable; read returns current enable |
| `0x0200`–`0x027C` | `GICD_ICENABLER[n]` | Write 1 to disable; read returns current enable |
| `0x0280`–`0x02FC` | `GICD_ISPENDR[n]` | Write 1 to set pending |
| `0x0300`–`0x037C` | `GICD_ICPENDR[n]` | Write 1 to clear pending |
| `0x0380`–`0x03FC` | `GICD_ISACTIVER[n]` | Write 1 to set active |
| `0x0400`–`0x047C` | `GICD_ICACTIVER[n]` | Write 1 to clear active |

### 2.3 Byte-Array Registers

| Offset Range | Register | Notes |
|---|---|---|
| `0x0400`–`0x07F8` | `GICD_IPRIORITYR[n]` | 4 priority bytes per 32-bit word; byte n = priority of INTID n |
| `0x0C00`–`0x0CFC` | `GICD_ICFGR[n]` | 2 bits per INTID: `00`=level, `10`=edge |

### 2.4 Affinity Routing Registers

| Offset Range | Register | Notes |
|---|---|---|
| `0x6100`–`0x7EF8` | `GICD_IROUTER[intid]` | 64-bit per SPI; INTID 32 → offset 0x6100, stride 8 bytes |

Formula: `offset = 0x6000 + intid * 8`. INTID 0..31 are reserved (RAZ/WI).

### 2.5 GICD_CTLR Field Encoding

```
Bit 31   RWP          Register Write Pending (RO, always 0 in sim)
Bit  6   DS           Disable Security (when implemented)
Bit  5   ARE_S        Affinity Routing Enable (Secure)
Bit  4   ARE_NS       Affinity Routing Enable (Non-Secure) — must be 1 for GICv3 behaviour
Bit  1   EnableGrp1NS Enable Group 1 Non-Secure interrupts
Bit  0   EnableGrp1S  Enable Group 1 Secure interrupts
```

Simulator behaviour: when `ARE_NS=0` (GICv2 legacy mode), reads/writes to `GICD_IROUTER` are RAZ/WI and `GICD_ITARGETSR` must be used instead. In GICv3-only mode, `ARE_NS=1` is enforced (writes that clear ARE_NS are ignored or RAZ).

---

## 3. GICR Register Offsets and Fields

Each `Gicv3Redistributor` covers a 128KB region. The `Device::read()` / `Device::write()` dispatch on offset within the full 128KB window:

- Offsets `0x0000`–`0xFFFF`: RD_base frame
- Offsets `0x10000`–`0x1FFFF`: SGI_base frame

### 3.1 RD_base Frame (0x0000–0xFFFF)

| Offset | Register | R/W | Notes |
|--------|----------|-----|-------|
| `0x0000` | `GICR_CTLR` | RW | `EnableLPIs`=bit0; once set, cannot be cleared |
| `0x0004` | `GICR_IIDR` | RO | Same implementer ID as GICD_IIDR |
| `0x0008` | `GICR_TYPER` | RO64 | See below |
| `0x0010` | `GICR_STATUSR` | RW | Error bits; 0 on reset |
| `0x0014` | `GICR_WAKER` | RW | `ProcessorSleep`=bit1 (RW); `ChildrenAsleep`=bit2 (RO) |
| `0x0040` | `GICR_SETLPIR` | WO64 | Direct LPI set-pending (INTID in bits [31:0]) |
| `0x0048` | `GICR_CLRLPIR` | WO64 | Direct LPI clear-pending |
| `0x0070` | `GICR_PROPBASER` | RW64 | LPI config table base address + attrs |
| `0x0078` | `GICR_PENDBASER` | RW64 | LPI pending table base address + attrs |
| `0xFFD0` | `GICR_PIDR2` | RO | Matches GICD_PIDR2 |

**GICR_TYPER fields** (64-bit, read-only, precomputed at build):
```
Bits  63:32  Affinity value (Aff3[39:32], Aff2[23:16], Aff1[15:8], Aff0[7:0])
Bit   24     CommonLPIAff — all redistributors with same Aff3.Aff2.Aff1 share LPI config table
Bit   17     RVPEID — redistributor supports GICv4 vPE direct injection (Phase 3)
Bit   16     VLPIS — redistributor supports virtual LPIs (GICv4, Phase 3)
Bit    4     Last — set on the final redistributor in the array (for discovery)
Bit    3     DirectLPI — GICR_SETLPIR/CLRLPIR supported
Bits   2:1   PMSS, PPInum — extended PPI range
Bit    0     PLPIS — redistributor supports physical LPIs
```

**GICR_WAKER** semantics:
- `ProcessorSleep` (bit 1): OS writes 1 to signal the PE is sleeping (WFI). GIC should not assert nIRQ when set.
- `ChildrenAsleep` (bit 2, RO): Set by GIC when it has no pending interrupts for this PE and `ProcessorSleep` is set.
- Boot sequence: OS checks `ChildrenAsleep` before entering low-power mode; must poll until both match.
- Simulator: when `ProcessorSleep=1`, suppress `irq_line.store(true)` for this PE.

**GICR_PROPBASER** fields:
```
Bits  51:12  PA[51:12] of LPI config table (page-aligned)
Bits  11:10  OuterCache — outer shareability attributes
Bits   9:7   InnerCache — inner shareability attributes
Bits   6:5   Shareability — memory shareability domain
Bit    1     Valid — set when table is valid
```

**GICR_PENDBASER** fields:
```
Bit   63     PTZ — pending table is zero (optimization hint)
Bits  51:16  PA[51:16] of LPI pending table (64KB-aligned minimum)
Bits  11:10  OuterCache
Bits   9:7   InnerCache
Bits   6:5   Shareability
Bit    1     Valid
```

### 3.2 SGI_base Frame (0x10000–0x1FFFF)

| Offset | Register | R/W | Notes |
|--------|----------|-----|-------|
| `0x10080` | `GICR_IGROUPR0` | RW | Group assignment: bits 0-15=SGI, 16-31=PPI |
| `0x10100` | `GICR_ISENABLER0` | RW | Write 1 to enable SGI/PPI; read returns current |
| `0x10180` | `GICR_ICENABLER0` | RW | Write 1 to disable SGI/PPI; read returns current |
| `0x10200` | `GICR_ISPENDR0` | RW | Write 1 to set pending |
| `0x10280` | `GICR_ICPENDR0` | RW | Write 1 to clear pending |
| `0x10300` | `GICR_ISACTIVER0` | RW | Write 1 to set active |
| `0x10380` | `GICR_ICACTIVER0` | RW | Write 1 to clear active |
| `0x10400`–`0x1041C` | `GICR_IPRIORITYR[0..7]` | RW | 4 priority bytes per word; 32 total |
| `0x10C00` | `GICR_ICFGR0` | RW | SGI 0-15 edge/level (all SGIs are edge, so RAZ/WI bits[1::2]) |
| `0x10C04` | `GICR_ICFGR1` | RW | PPI 16-31 edge/level configuration |

---

## 4. ICC_* System Register Interface

### 4.1 Enabling the System Register Interface

The first write a guest OS must perform is to set `ICC_SRE_EL1.SRE=1`:

```
ICC_SRE_EL1 = {DIB[2], DFB[1], SRE[0]}
```

- `SRE=0`: CPU interface is MMIO (GICv2 compat mode — not supported in GICv3-only builds)
- `SRE=1`: CPU interface is system registers — required

In the simulator, `SRE` is hardwired to 1 on reset (any write that clears it is ignored). `ICC_SRE_EL2` and `ICC_SRE_EL3` must also have `SRE=1` before EL1 can see system register access.

### 4.2 Hooks into helm-arch

System register dispatch lives in:
```
runtime/helm-arch/src/aarch64/execute/sysregs.rs
```

The function signatures to add:

```rust
/// Called from read_sysreg() on ICC_* match.
pub fn icc_read(
    state: &mut Gicv3CpuIfState,
    shared: &mut GicV3SharedState,
    cpu_idx: usize,
    op0: u8, op1: u8, crn: u8, crm: u8, op2: u8,
) -> u64;

/// Called from write_sysreg() on ICC_* match.
pub fn icc_write(
    state: &mut Gicv3CpuIfState,
    shared: &mut GicV3SharedState,
    cpu_idx: usize,
    op0: u8, op1: u8, crn: u8, crm: u8, op2: u8,
    val: u64,
);
```

`FsState` must carry `gicv3: Option<Arc<Mutex<GicV3SharedState>>>` populated during `elaborate()`.

### 4.3 ICC Register Semantics

**ICC_PMR_EL1** — Priority Mask:
```
Read:  return current icc_pmr
Write: icc_pmr = val as u8; recompute irq_line for this PE
```

**ICC_IAR1_EL1** — Interrupt Acknowledge (Group 1):
```
Read:  call cpu_acknowledge(cpu_idx) → INTID
       if INTID != 1023:
           priority_drop: running_pri = priority[INTID]
           active_stack.push((INTID, priority[INTID]))
           pending bit cleared, active bit set
           active_priorities[(prio >> 3) as usize] |= 1 << ((prio >> 3) & 31)
       return INTID
```

**ICC_EOIR1_EL1** — End of Interrupt (Group 1):
```
Write (val = INTID):
  if ICC_CTLR.EOImode == 0:
    # Combined priority drop + deactivation
    active_stack.pop → priority, clear active bit in GICR/GICD
    running_pri = active_stack.last().map(|(_,p)| *p).unwrap_or(0xFF)
    active_priorities update
  if ICC_CTLR.EOImode == 1:
    # Priority drop only (deactivation via ICC_DIR_EL1)
    running_pri = active_stack.last().map(|(_,p)| *p).unwrap_or(0xFF)
    (active bit NOT cleared here)
  recompute irq_line
```

**ICC_DIR_EL1** — Deactivate Interrupt (EOImode=1 only):
```
Write (val = INTID):
  if ICC_CTLR.EOImode == 1:
    clear active bit in GICR/GICD for INTID
  recompute irq_line
```

**ICC_HPPIR1_EL1** — Highest Priority Pending (peek, no side effects):
```
Read:  return highest_pending_for_cpu(cpu_idx) without any state change
```

**ICC_RPR_EL1** — Running Priority:
```
Read:  return running_pri (0xFF if no active interrupt)
```

**ICC_CTLR_EL1** — Control:
```
Bits  8    IDbits    RO — 0b111 (1020 INTIDs)
Bits  6    A3V       RO — 1 (full 32-bit affinity)
Bits  5    SEIS      RO — 0
Bit   1    EOImode   RW — 0=combined drop+deact, 1=split (ICC_DIR_EL1 needed)
Bit   0    CBPR      RW — common binary point (use BPR0 for Group 0)
```

**ICC_BPR1_EL1** — Binary Point (Group 1):
```
Read/Write: icc_bpr1 field. Minimum value = 0 (all bits preempt).
Preemption check: preempting_priority >> (icc_bpr1+1) < running_priority >> (icc_bpr1+1)
```

**ICC_SGI1R_EL1** — Generate Group 1 SGI:
```
Write (64-bit):
  aff3 = (val >> 48) & 0xFF
  aff2 = (val >> 32) & 0xFF
  aff1 = (val >> 16) & 0xFF
  rs   = (val >> 44) & 0xF      # range selector (RS*16 + bit_position = Aff0)
  irm  = (val >> 40) & 1        # 1 = broadcast (all except self)
  intid = (val >> 24) & 0xF     # SGI ID 0..15
  tlist = val & 0xFFFF          # target list: bit n → Aff0 = RS*16 + n
  → call generate_sgi(cpu_idx, intid, affinity_base={aff3,aff2,aff1}, rs, tlist, irm)
```

**ICC_IGRPEN1_EL1** — Group 1 Enable:
```
Bit 0: enable Group 1 Non-Secure interrupts.
Write: icc_igrpen1 = val & 1; recompute irq_line.
```

### 4.4 System Register Encoding Table

Used to pattern-match incoming MRS/MSR encodings in `sysregs.rs`:

| Register | Op0 | Op1 | CRn | CRm | Op2 |
|----------|-----|-----|-----|-----|-----|
| ICC_PMR_EL1 | 3 | 0 | 4 | 6 | 0 |
| ICC_IAR1_EL1 | 3 | 0 | 12 | 12 | 0 |
| ICC_EOIR1_EL1 | 3 | 0 | 12 | 12 | 1 |
| ICC_HPPIR1_EL1 | 3 | 0 | 12 | 12 | 2 |
| ICC_BPR1_EL1 | 3 | 0 | 12 | 12 | 3 |
| ICC_CTLR_EL1 | 3 | 0 | 12 | 12 | 4 |
| ICC_SRE_EL1 | 3 | 0 | 12 | 12 | 5 |
| ICC_IGRPEN0_EL1 | 3 | 0 | 12 | 12 | 6 |
| ICC_IGRPEN1_EL1 | 3 | 0 | 12 | 12 | 7 |
| ICC_RPR_EL1 | 3 | 0 | 12 | 11 | 3 |
| ICC_DIR_EL1 | 3 | 0 | 12 | 11 | 1 |
| ICC_SGI1R_EL1 | 3 | 0 | 12 | 11 | 5 |
| ICC_ASGI1R_EL1 | 3 | 0 | 12 | 11 | 6 |
| ICC_SGI0R_EL1 | 3 | 0 | 12 | 11 | 7 |
| ICC_AP1R0_EL1 | 3 | 0 | 12 | 9 | 0 |
| ICC_AP1R1_EL1 | 3 | 0 | 12 | 9 | 1 |
| ICC_AP1R2_EL1 | 3 | 0 | 12 | 9 | 2 |
| ICC_AP1R3_EL1 | 3 | 0 | 12 | 9 | 3 |
| ICC_SRE_EL2 | 3 | 4 | 12 | 9 | 5 |
| ICC_SRE_EL3 | 3 | 6 | 12 | 12 | 5 |
| ICC_IGRPEN1_EL3 | 3 | 6 | 12 | 12 | 7 |
| ICC_CTLR_EL3 | 3 | 6 | 12 | 12 | 4 |

---

## 5. Priority Model

### 5.1 Priority Concepts

GICv3 uses 8-bit priority values where **lower number = higher priority** (0x00 = highest). The simulator tracks:

| Concept | Field | Reset |
|---------|-------|-------|
| Priority threshold | `icc_pmr` | 0xFF (all unmasked) |
| Running priority | `running_pri` | 0xFF (no active) |
| Interrupt priority | `priority[intid]` | 0x00 |

An interrupt is eligible for delivery when:
1. Its INTID is pending and enabled (in GICR for SGI/PPI, GICD for SPI)
2. Its priority < `icc_pmr` (strictly less than — mask register)
3. Its priority < `running_pri` (preemption check — strictly less than)
4. Group 1 is enabled (`icc_igrpen1.bit0 = 1`)
5. GICD global enable is set (`dist.ctlr.EnableGrp1NS = 1`)
6. GICR for this PE is not asleep (`waker.ProcessorSleep = 0`)

### 5.2 Priority Drop vs Deactivation

GICv3 separates two concepts that were combined in GICv2:

**Priority drop**: happens at `IAR` read time (or `EOIR` write in EOImode=0). Sets `running_pri` to the interrupt's priority, preventing lower-priority interrupts from preempting.

**Deactivation**: clears the `active` bit in the GICR/GICD, allowing the interrupt to be re-asserted. In EOImode=0, happens at `EOIR` write. In EOImode=1, happens at `ICC_DIR_EL1` write.

```
EOImode=0 (default, Linux uses this):
  IAR read  → priority drop + active bit set
  EOIR write → deactivation (active cleared) + running_pri restored

EOImode=1 (split model, used by hypervisors):
  IAR read  → priority drop + active bit set
  EOIR write → running_pri restored (active NOT cleared)
  DIR write  → deactivation (active cleared)
```

### 5.3 Preemption and Active Priorities

The `active_priorities` array (`ICC_AP1R0..3_EL1`) tracks which priority groups are currently active. Bit `N` is set when an interrupt with priority group `N` is active (priority group = priority >> (icc_bpr1 + 1)).

The running priority is the minimum (most significant) bit set in the active priorities bitmap, used for preemption decisions.

### 5.4 Highest-Pending Selection Algorithm

```
fn highest_pending_for_cpu(shared: &GicV3SharedState, cpu_idx: usize, pmr: u8) -> Option<(u32, u8)>:
  let redist = &shared.redists[cpu_idx];
  let running = redist.cpu_if.running_pri;
  let mut best: Option<(u32, u8)> = None;

  // Check SGI/PPI (in GICR)
  let pending_enabled = redist.sgi_ppi_pending & redist.sgi_ppi_enabled;
  for intid in 0..32:
    if pending_enabled & (1 << intid) != 0 && active bit not set:
      let prio = redist.sgi_ppi_priority[intid];
      if prio < pmr && prio < running && better_than(best, prio):
        best = Some((intid, prio));

  // Check SPI (in GICD), only if ARE=1 and interrupt routes to this PE
  if dist.ctlr.ARE_NS:
    for intid in 32..dist.num_irqs:
      if dist.pending[word] & bit != 0 && dist.enabled[word] & bit != 0 && dist.active[word] & bit == 0:
        let prio = dist.priority[intid];
        if prio < pmr && prio < running && routes_to(dist.irouter[intid-32], redist.affinity):
          if better_than(best, prio):
            best = Some((intid, prio));

  best
```

---

## 6. SGI Generation

### 6.1 ICC_SGI1R_EL1 Write Path

```rust
pub fn generate_sgi(
    shared: &mut GicV3SharedState,
    source_cpu_idx: usize,
    intid: u32,          // SGI ID, 0..15
    affinity: u64,       // Aff3[55:48].Aff2[43:32].Aff1[15:8] from register
    rs: u8,              // Range Selector: Aff0 base = rs * 16
    tlist: u16,          // bit N → target Aff0 = rs*16 + N
    irm: bool,           // true = broadcast to all except source
)
```

**Broadcast mode (irm=true)**:
- Assert SGI pending in all redistributors except `source_cpu_idx`
- Regardless of affinity fields

**Targeted mode (irm=false)**:
- For each set bit `n` in `tlist`:
  - Compute target affinity: `{Aff3, Aff2, Aff1, rs*16 + n}`
  - Find redistributor whose `affinity` field matches
  - Set `redist.sgi_ppi_pending |= 1 << intid`
  - Call `update_irq_line(target_cpu_idx)`

### 6.2 SGI Pending State

Unlike GICv2 where SGIs used a source-tracking model (GICD_SPENDSGIR had source CPU bits), GICv3 SGI pending is a simple bit in `GICR_ISPENDR0`. Multiple SGIs from different CPUs targeting the same INTID on the same PE simply set the same bit — source tracking is not required.

---

## 7. Integration with HelmBoard

### 7.1 Per-vCPU Redistributor Indexing

`HelmBoard` holds `Vec<HelmVcpu>`. Each vCPU's `FsState` stores:
```rust
pub struct FsState {
    // ...existing fields...
    pub gic_cpu_idx: usize,          // index into GicV3SharedState.redists
    pub gicv3: Option<Arc<Mutex<GicV3SharedState>>>,
}
```

During `elaborate()`, the board wires each vCPU to its redistributor index (matching order of `build_gicv3_mp` call).

### 7.2 Redistributor MMIO Placement

The `HelmAddressSpace` maps each `Gicv3Redistributor` at:
```
base_addr = GICR_BASE + cpu_idx * 0x20000
```

Each `Gicv3Redistributor` implements `Device::region_size()` → `0x20000` (128KB).

The `Device::read()` and `Device::write()` dispatch on offset within the 128KB window, splitting at 0x10000 for RD_base vs SGI_base.

### 7.3 Replacing GICv2

The arm-virt platform will support both GICv2 and GICv3 via a config parameter:

```python
# Python config (helm_ng)
system.gic_version = "gicv3"  # or "gicv2" (default for compatibility)
```

When GICv3 is selected:
- GICD at same base address (`0x0800_0000`)
- GICR frames at `0x080A_0000` (with space for up to 8 PE × 128KB = 1MB)
- DTB `compatible = "arm,gic-v3"` node with `#redistributor-regions = <1>` and `redistributor-stride = <0 0x20000>`
- No GICC MMIO region (GICv3 has no CPU interface MMIO)

### 7.4 irq_line Semantics

The `Arc<AtomicBool>` IRQ line is unchanged from GICv2. The vCPU step loop checks it before each instruction fetch:

```rust
// In fs.rs step loop:
if state.gicv3_irq_line().load(Ordering::Acquire) {
    // deliver EL1 IRQ exception (same path as GICv2)
    take_exception_el1(arch_state, ExceptionVector::Irq);
}
```

The GIC calls `irq_line.store(true/false, Ordering::Release)` after any state change via `update_irq_line(cpu_idx)`.

---

## 8. Checkpoint Protocol

### 8.1 What Is Architectural State

The following fields are **architectural state** — they must be included in checkpoint save/restore:

| Struct | Fields |
|--------|--------|
| `Gicv3DistState` | `ctlr`, `group`, `enabled`, `pending`, `active`, `priority`, `config`, `irouter` |
| `Gicv3RedistState` | `ctlr`, `waker`, `propbaser`, `pendbaser`, `sgi_ppi_*` (all 7 fields), `affinity` |
| `Gicv3CpuIfState` | `icc_sre_el1`, `icc_pmr`, `icc_bpr1`, `icc_ctlr`, `icc_igrpen0`, `icc_igrpen1`, `running_pri`, `active_stack`, `active_priorities` |

### 8.2 What Is NOT Architectural State

| Struct | Fields | Reason |
|--------|--------|--------|
| `Gicv3RedistState` | `irq_line` | Re-computed after restore |
| `Gicv3DistState` | `physical_level` | Derived from device input signals |
| `GicV3SharedState` | `sgi_log_budget` | Diagnostic, not functional |
| `ItsState` | `pending_cmds` | In-flight commands are non-deterministic |

### 8.3 Restore Sequence

After loading checkpoint data:
1. Restore all architectural fields
2. Call `update_all_irq_lines()` to re-assert `irq_line` for any pending+unmasked interrupts
3. Re-register all MMIO regions (distributor, redistributors, ITS) in `HelmAddressSpace`
4. Re-wire `FsState.gicv3` Arc references for each vCPU

---

## 9. Work Items

### Done

- [x] WI-LLD-001: `Gicv3DistState`, `Gicv3RedistState`, `Gicv3CpuIfState`, `GicV3SharedState` structs in `mod.rs`
- [x] WI-LLD-002: `Gicv3DistState::new(num_irqs)` with `typer` precomputation
- [x] WI-LLD-003: `Gicv3RedistState::new(cpu_idx, affinity, irq_line)` with `GICR_TYPER` precomputation
- [x] WI-LLD-004: `Gicv3Distributor::read()` — all GICD offsets (CTLR, TYPER, IGROUPR, IS/ICENABLER, IS/ICPENDR, IS/ICACTIVER, IPRIORITYR, ICFGR, IROUTER, PIDR2, SETSPI/CLRSPI)
- [x] WI-LLD-005: `Gicv3Distributor::write()` — all writable GICD offsets including SETSPI/CLRSPI
- [x] WI-LLD-006: `Gicv3Redistributor::read()` — all GICR RD_base offsets (CTLR, TYPER, WAKER, PROPBASER, PENDBASER, PIDR2)
- [x] WI-LLD-007: `Gicv3Redistributor::write()` — all GICR RD_base writable offsets
- [x] WI-LLD-008: `Gicv3Redistributor::read()` — all GICR SGI_base offsets
- [x] WI-LLD-009: `Gicv3Redistributor::write()` — all GICR SGI_base writable offsets
- [x] WI-LLD-010: `icc_read()` — ICC_PMR, IAR1, HPPIR1, BPR1, RPR, CTLR, SRE (EL1/2/3), IGRPEN0/1, AP1R0..3
- [x] WI-LLD-011: `icc_write()` — ICC_PMR, EOIR1, DIR, BPR1, CTLR, SRE, IGRPEN0/1, SGI1R, ASGI1R, SGI0R, AP1R0..3
- [x] WI-LLD-012: `highest_pending_for_cpu()` — SGI/PPI from GICR + SPI from GICD with affinity match
- [x] WI-LLD-013: `cpu_acknowledge()` — priority drop, active stack push, active bit set
- [x] WI-LLD-014: `cpu_eoi()` — EOImode=0 (combined) and EOImode=1 (priority restore only)
- [x] WI-LLD-015: `cpu_deactivate()` — ICC_DIR_EL1 path with level-sensitive re-pending
- [x] WI-LLD-016: `generate_sgi()` — affinity targeted + IRM broadcast
- [x] WI-LLD-017: `update_irq_line(cpu_idx)` — checks EnableGrp1NS, IGRPEN1, WAKER.ProcessorSleep
- [x] WI-LLD-018: `GicV3Sink` implementing `InterruptSink` (assert_spi/deassert_spi)
- [x] WI-LLD-021: WAKER ProcessorSleep suppression in `update_irq_line`

### Pending — Phase 1 (wiring + tests)

- [x] WI-LLD-019: Wire ICC_* sysreg dispatch in `helm-arch` to call `icc_read`/`icc_write` — done via `try_exec_gicv3_sysreg()` in FS step loop
- [x] WI-LLD-020: Wire GICv3 into `arm_virt` platform as default (`build_arm_virt_gicv3` in `arm_virt.rs`)
- [ ] WI-LLD-022: Checkpoint save/restore for all architectural fields (section 8)
- [ ] WI-LLD-023: Unit tests: priority ordering, preemption, EOImode=0, EOImode=1 + DIR, SGI broadcast, SGI targeted, affinity routing
- [x] WI-LLD-026: DTB generation emits GICv3 node (`arm,gic-v3`) with correct GICR region size (num_cpus * 128KB)

### Pending — Phase 2 (LPI/ITS)

- [ ] WI-LLD-024: `ItsState` and command queue processor per section 1.6
- [ ] WI-LLD-025: LPI config/pending table reads from guest RAM
