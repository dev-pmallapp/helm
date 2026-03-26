# SMMU-v3 — High-Level Design

> Cross-references: [`ARCHITECTURE.md`](../../../ARCHITECTURE.md) · [`HLD.md`](../../HLD.md) · [`object-model.md`](../../../object-model.md) · [`traits.md`](../../../traits.md) · [`gicv3/HLD.md`](../gicv3/HLD.md) · [`helm-devices LLD-interrupt-model`](../../helm-devices/LLD-interrupt-model.md)

---

## Table of Contents

1. [Why SMMU-v3 — IOMMU for DMA Isolation](#1-why-smmu-v3--iommu-for-dma-isolation)
2. [SMMU-v3 Register Map](#2-smmu-v3-register-map)
3. [Stream Table — STE and CD Format](#3-stream-table--ste-and-cd-format)
4. [Command Queue](#4-command-queue)
5. [Translation Walk](#5-translation-walk)
6. [Fault Model](#6-fault-model)
7. [GICv3 Integration](#7-gicv3-integration)
8. [helm-ng Integration](#8-helm-ng-integration)
9. [GICv4 Note — Virtual LPI Direct Injection](#9-gicv4-note--virtual-lpi-direct-injection)
10. [Work Items](#10-work-items)

---

## 1. Why SMMU-v3 — IOMMU for DMA Isolation

### 1.1 The DMA Problem

Without an IOMMU, a DMA-capable peripheral (PCIe endpoint, GPU, NIC) can read or write any physical address it is programmed with. This creates two problems:

1. **Security**: a malicious or compromised device can exfiltrate or corrupt memory outside its intended window.
2. **Virtualization**: a guest OS programs devices with guest physical addresses (GPA); without translation, the device accesses the wrong host physical memory.

The SMMU-v3 (ARM System Memory Management Unit version 3) solves both by interposing on every DMA transaction issued by a peripheral and translating or blocking it based on per-stream configuration.

### 1.2 Core Concepts

**Stream ID (SID)**: a per-transaction identifier issued by the requesting device. For PCIe, the SID is the function's Requester ID (Bus/Device/Function). The SID selects a Stream Table Entry (STE) that configures translation for that device.

**Stage 1 (S1)**: translates IOVA (I/O Virtual Address, as programmed by the guest driver) to IPA (Intermediate Physical Address). Uses the same TTB0/TTB1 page table format as the CPU MMU (with SMMU-specific SMMU_S1_PTW_CFG).

**Stage 2 (S2)**: translates IPA to PA (Physical Address). Used in virtualization: the hypervisor controls S2, the guest controls S1. Nested translation applies both.

**Bypass**: STE may be configured to pass transactions through with no translation (used for trusted devices or those managed by a host OS with IOMMU passthrough).

**Abort**: STE may be configured to abort all transactions from a device (default before software configures the STE, producing a `C_BAD_STE` fault event).

### 1.3 SMMU in the helm-ng Context

In helm-ng Phase 3 (full-system simulation with PCIe and virtio), DMA transactions from VirtIO or emulated NIC/GPU models will carry a `stream_id` in `TransactionAttrs`. The SMMU sits between the device bus and the `HelmAddressSpace`, intercepting these transactions and translating them before forwarding to `FlatMem`.

For Phase 0–2 (no real DMA devices), the SMMU can be instantiated in bypass mode — all transactions pass through — satisfying the DTB requirement while deferring translation complexity.

### 1.4 SMMU-v3 vs SMMU-v2

| Feature | SMMU-v2 | SMMU-v3 |
|---------|---------|---------|
| Stream table | Linear only | Linear or 2-level |
| Command/Event queues | Polling or MSI | Command queue (ring buffer) |
| Fault reporting | Registers only | Event queue in RAM |
| MSI support | No | Yes (SMMU can generate MSI to ITS) |
| PCIe ATS | No | Yes (PRIQ — PRI queue) |
| Max SIDs | 16-bit | 32-bit |
| GICv4 integration | No | SMMU can route vSPE/vLPI directly |

---

## 2. SMMU-v3 Register Map

The SMMU is a single MMIO block. The base address is platform-specific. In the QEMU arm-virt machine, the SMMU is typically mapped at `0x09050000` (64KB region for page 0).

### 2.1 Identification and Control Registers

| Offset | Register | R/W | Description |
|--------|----------|-----|-------------|
| `0x0000` | `SMMU_IDR0` | RO | Feature flags: ST_LEVEL, TERM_MODEL, STALL_MODEL, TTENDIAN, VATOS, CD2L, VMID16, PRI, ATOS, NS1ATS, ASID16, ATS, SEV, MSI, CE, HTTU |
| `0x0004` | `SMMU_IDR1` | RO | Queue capacity: CMDQ_MAX_SZ_LOG2, EVTQ_MAX_SZ_LOG2, PRIQ_MAX_SZ_LOG2; TABLES_PRESET, QUEUES_PRESET, REL |
| `0x0008` | `SMMU_IDR2` | RO | Input/output address size: OAS, IAS; BBML (break-before-make) |
| `0x000C` | `SMMU_IDR3` | RO | RIL (Range Invalidation Large), MPAM, e.g. |
| `0x0010` | `SMMU_IDR4` | RO | Implementation defined |
| `0x0014` | `SMMU_IDR5` | RO | OAS (physical address size), VAX (VA extension), GRAN64K/16K/4K |
| `0x0018` | `SMMU_IIDR` | RO | Implementer / product ID |
| `0x001C` | `SMMU_AIDR` | RO | Architecture version: `ARCH_REV = 0x1` (SMMU-v3.1) or `0x2` (v3.2) |
| `0x0020` | `SMMU_CR0` | RW | Global enable: SMMUEN, PRIQEN, EVTQEN, CMDQEN |
| `0x0024` | `SMMU_CR0ACK` | RO | Mirrors CR0 after hardware acknowledgment |
| `0x0028` | `SMMU_CR1` | RW | Cache/shareability attrs for table walks |
| `0x002C` | `SMMU_CR2` | RW | RECINVSID, E2H, PKVM |
| `0x0040` | `SMMU_STATUSR` | RO | DORMANT, IDLE — reflects SMMU quiescence |
| `0x0044` | `SMMU_GBPA` | RW | Global Bypass/Abort; ABORT bit, SHCFG, MTCFG, MemAttr |
| `0x0048` | `SMMU_AGBPA` | RW | Alternate Global Bypass/Abort |
| `0x0060` | `SMMU_IRQ_CTRL` | RW | EVTQ_IRQEN, PRIQ_IRQEN, GERROR_IRQEN |
| `0x0064` | `SMMU_IRQ_CTRLACK` | RO | Mirrors IRQ_CTRL after ack |
| `0x0068` | `SMMU_GERROR` | RO | Global error flags (sticky) |
| `0x006C` | `SMMU_GERRORN` | RW | Clear GERROR bits (write 1 to acknowledge) |
| `0x0070` | `SMMU_GERROR_IRQ_CFG0` | RW64 | MSI address for GERROR interrupt |
| `0x0078` | `SMMU_GERROR_IRQ_CFG1` | RW | MSI data for GERROR interrupt |
| `0x007C` | `SMMU_GERROR_IRQ_CFG2` | RW | MSI control for GERROR interrupt |

### 2.2 Stream Table Registers

| Offset | Register | R/W | Description |
|--------|----------|-----|-------------|
| `0x0080` | `SMMU_STRTAB_BASE` | RW64 | Stream table base PA (page-aligned) + LOG2SIZE of SID space |
| `0x0088` | `SMMU_STRTAB_BASE_CFG` | RW | FMT (linear=0, 2-level=1), LOG2SIZE, SPLIT |

**SMMU_STRTAB_BASE_CFG fields**:
```
Bits  17:16  FMT    — 0b00 = linear table; 0b01 = 2-level table
Bits  10:6   SPLIT  — L1 table entry span (used for 2-level only; typically 8)
Bits   5:0   LOG2SIZE — log2 of number of STE entries (e.g. 8 = 256 entries)
```

### 2.3 Command Queue Registers

| Offset | Register | R/W | Description |
|--------|----------|-----|-------------|
| `0x0090` | `SMMU_CMDQ_BASE` | RW64 | Command queue base PA + LOG2SIZE (entries = 2^LOG2SIZE) |
| `0x0098` | `SMMU_CMDQ_PROD` | RW | Producer index (write pointer, wrapping ring buffer) |
| `0x009C` | `SMMU_CMDQ_CONS` | RW | Consumer index (read pointer); ERR field on error |

### 2.4 Event Queue Registers

| Offset | Register | R/W | Description |
|--------|----------|-----|-------------|
| `0x00A0` | `SMMU_EVTQ_BASE` | RW64 | Event queue base PA + LOG2SIZE |
| `0x00A8` | `SMMU_EVTQ_PROD` | RW | Producer index (SMMU writes) |
| `0x00AC` | `SMMU_EVTQ_CONS` | RW | Consumer index (OS reads and advances) |
| `0x00B0` | `SMMU_EVTQ_IRQ_CFG0` | RW64 | MSI address for event queue interrupt |
| `0x00B8` | `SMMU_EVTQ_IRQ_CFG1` | RW | MSI data |
| `0x00BC` | `SMMU_EVTQ_IRQ_CFG2` | RW | MSI control |

### 2.5 PRI Queue Registers (PCIe ATS — Phase 3)

| Offset | Register | R/W | Description |
|--------|----------|-----|-------------|
| `0x00C0` | `SMMU_PRIQ_BASE` | RW64 | PRI queue base PA + LOG2SIZE |
| `0x00C8` | `SMMU_PRIQ_PROD` | RW | Producer |
| `0x00CC` | `SMMU_PRIQ_CONS` | RW | Consumer |

### 2.6 SMMU_CR0 Field Encoding

```
Bit  3   PRIQEN  — Enable PRI queue
Bit  2   EVTQEN  — Enable Event queue (must be set before SMMUEN)
Bit  1   CMDQEN  — Enable Command queue
Bit  0   SMMUEN  — Enable SMMU (translation active; GBPA applies when 0)
```

Enable sequence required by software:
```
1. Configure STRTAB_BASE, STRTAB_BASE_CFG
2. Configure CMDQ_BASE, EVTQ_BASE
3. Write CR0 with CMDQEN=1, EVTQEN=1 (wait for CR0ACK)
4. Write CR0 with SMMUEN=1 (wait for CR0ACK)
```

---

## 3. Stream Table — STE and CD Format

### 3.1 Linear vs 2-Level Stream Table

**Linear table**: a flat array of 64-byte STEs indexed directly by SID. Size = `2^LOG2SIZE * 64` bytes. Suitable for systems with few SIDs (< 64K).

**2-Level table**: L1 descriptor array (each entry points to an L2 STE page) + multiple L2 pages. Used when SIDs are sparse across a 32-bit namespace. The `SPLIT` field controls how many bits are used for L1 vs L2 index.

### 3.2 Stream Table Entry (STE) Format

Each STE is 64 bytes (8 × 64-bit doublewords):

```
DW0  Bits 63:32  Config[31:0] (type, S2, S1, EATS, etc.)
     Bit  1      V (valid) — if 0, transaction aborts with C_BAD_STE
     Bits 4:2    Config[2:0]:
                   0b000 = Abort
                   0b100 = Bypass
                   0b101 = Stage 1 only
                   0b110 = Stage 2 only
                   0b111 = Stage 1 + Stage 2 (nested)
     Bits 11:6   S1Fmt — CD format: 0=linear CD table, 1=indirect (SubstreamID)
     Bits 51:6   S1ContextPtr — PA[51:6] of CD or L1 CD table
     Bits 5:0    S1CDMax — max SubstreamID bits (for multi-substream)

DW1  Bits 63:48  S1CIR/S1COR/S1CSH — inner/outer cache, shareability for CD walk
     Bits 47:44  S1DSS — DS Substream behavior
     Bits 15:0   VMID (16-bit virtual machine ID for Stage 2)

DW2  Bits 63:16  S2TTB (Stage 2 translation table base, PA[63:4])
     Bits 15:0   S2T0SZ (address space size for Stage 2)

DW3  Bits 63:56  S2TG (granule: 4KB=0, 64KB=1, 16KB=2)
     Bits 55:50  S2PS (physical address size)
     Bit  55     S2AA64 (AArch64 page tables)
     Bits 49:48  S2ENDI (endianness)
     Bits 47:46  S2AFFD (access flag fault disable)
     Bits 45:44  S2HD/S2HA (hardware dirty/access flag update)
     Bit  43     S2PTW (protected table walk)
     Bit  42     S2R (record fault)
     Bits 33:32  EATS (IOMMU PCIe ATS operation)

DW4–DW7: Implementation defined / reserved
```

### 3.3 Context Descriptor (CD) Format

The CD is 64 bytes and describes a single translation context (ASID, TTB0, TCR, etc.) for Stage 1:

```
DW0  Bits 63:59  T0SZ (address space size for TTB0, same encoding as TCR_EL1)
     Bit  57     EPD0 (disable TTB0 walks — abort)
     Bits 56:54  TG0 (TTB0 granule: 0=4KB, 1=64KB, 2=16KB)
     Bits 53:51  IR0/OR0/SH0 (inner/outer cache, shareability for TTB0)
     Bit  48     AFFD (access flag fault disable)
     Bit  47     WXN (write-execute-never for TTB0)
     Bit  6      AARCH64 (1 = use AArch64 descriptor format)
     Bit  5      ENDI (endianness)
     Bit  4      V (valid) — must be 1
     Bits 3:1    Configuration (STRW, ASET, NSCFG)
     Bit  0      T1SZ enable

DW1  Bits 63:48  ASID (16-bit; used with S1 ASID-tagged TLB entries)
     Bits 47:1   TTB0[48:2] (page table base for VA→IPA Stage 1)
     Bit  0      HAD0 (hardware access dirty, TTB0)

DW2  Bits 47:1   TTB1[48:2] (page table base for second range)
     Bit  0      HAD1

DW3  Bits 63:32  MAIR0 (memory attribute indirection register, low 32 bits)
     Bits 31:0   MAIR1 (memory attribute indirection register, high 32 bits)

DW4–DW7: Reserved
```

### 3.4 2-Level Stream Table L1 Descriptor

Each 8-byte L1 descriptor:
```
Bits 63:6   L2Ptr — PA[63:6] of the L2 STE page (4KB page, 64 STEs per page)
Bit   0     V    — valid
```

---

## 4. Command Queue

### 4.1 Ring Buffer Model

The command queue is a circular buffer in RAM:
- `SMMU_CMDQ_BASE` holds the base PA and `LOG2SIZE` (queue depth = `2^LOG2SIZE` entries, each 16 bytes)
- `SMMU_CMDQ_PROD` is the write pointer (managed by software)
- `SMMU_CMDQ_CONS` is the read pointer (managed by the SMMU hardware)

The SMMU processes commands when `CMDQEN=1` and `PROD ≠ CONS`. The queue is full when `(PROD + 1) mod (2 * depth) == CONS` (the wrap bit prevents aliasing).

### 4.2 Command Format

Each command is 16 bytes. The first byte (bits [7:0]) contains the opcode:

```
Opcode (hex)  Command            Description
0x04          CFGI_STE_RANGE     Invalidate STE(s) for a range of SIDs
0x05          CFGI_ALL           Invalidate all STEs (global config invalidate)
0x06          CFGI_CD            Invalidate CD for a specific SID+SubstreamID
0x07          CFGI_CD_ALL        Invalidate all CDs for a specific SID
0x20          TLBI_NH_ALL        Non-hypervisor TLB invalidate all
0x21          TLBI_NH_ASID       Invalidate by ASID
0x22          TLBI_NH_VA         Invalidate by VA (non-hypervisor)
0x23          TLBI_NH_VAA        Invalidate by VA, all ASIDs
0x24          TLBI_EL3_ALL       EL3 TLB invalidate all
0x26          TLBI_EL3_VA        Invalidate EL3 TLB by VA
0x28          TLBI_S2_IPA        Stage 2 IPA invalidate
0x44          TLBI_NSNH_ALL      Non-secure non-hypervisor all
0x46          ATC_INV            PCIe ATS cache invalidate
0x47          PRI_RESP           PRI queue response
0x04…         RESUME             Resume a stalled transaction
0x46          CMD_SYNC           Completion sync: sets SMMU_CMDQ_CONS.WR_ALLOC on drain
```

**CMD_SYNC** is the most important command for software: it ensures all prior commands have been processed before software proceeds (e.g., before re-enabling a device after TLBI).

### 4.3 Key Command Encodings

**CFGI_STE_RANGE**:
```
DW0  Bits 31:0   opcode=0x04
     Bits 46:32  SID (stream ID)
     Bits 52:48  RANGE (2^RANGE STEs to invalidate, starting at SID)
```

**TLBI_NH_VA**:
```
DW0  Bits 63:48  ASID
     Bits 31:0   opcode=0x22
DW1  Bits 63:12  VA[63:12] (page-aligned)
     Bit  63     Leaf — if 1, invalidate only leaf entries; if 0, all levels
```

**CMD_SYNC**:
```
DW0  Bits 31:0   opcode=0x46
     Bits 35:32  CS (completion signal: 0=none, 1=MSI, 2=SEV)
     Bits 63:36  MSI address (if CS=1)
DW1  Bits 31:0   MSI data (if CS=1)
```

### 4.4 Simulator Command Processing

In the simulator, command processing is **synchronous** — when software advances `CMDQ_PROD`, the simulator processes all pending commands immediately during the same MMIO write. This avoids background thread complexity while preserving correctness.

Pseudocode:
```
on CMDQ_PROD write:
  while CONS != PROD:
    cmd = read_cmd(base + CONS * 16)
    process_command(cmd)
    CONS = (CONS + 1) % (2 * depth)
  write_back CONS
```

---

## 5. Translation Walk

### 5.1 Stage 1 Page Table Walk (VA → IPA)

The SMMU Stage 1 walk uses AArch64-compatible page tables, identical in format to the CPU MMU with these notes:
- Input address size: controlled by `CD.T0SZ` (same encoding as `TCR_EL1.T0SZ`)
- Granule: `CD.TG0` (0=4KB, 1=64KB, 2=16KB)
- Table base: `CD.TTB0`
- Number of levels: determined by `T0SZ` and `TG0`:

```
4KB granule:
  T0SZ  48 → 4-level walk (L0/L1/L2/L3)
  T0SZ  39 → 3-level walk (L1/L2/L3)
  T0SZ  30 → 2-level walk (L2/L3)

64KB granule:
  T0SZ  42 → 3-level walk (L1/L2/L3)
  T0SZ  29 → 2-level walk (L2/L3)
```

**Descriptor format** (4KB granule, 64-bit descriptor):
```
Bit   0    Valid
Bit   1    Type (0=block/page, 1=table)
Bits 47:12  Output address [47:12] (for block) or next-level table PA (for table)
Bit  10    AF (access flag)
Bits  9:8   SH (shareability)
Bits  7:6   AP[2:1] (access permissions: EL0/EL1 R/W)
Bit   5    NS (non-secure)
Bits  4:2   AttrIdx (index into MAIR)
Bits 54:52  XN/PXN (execute-never)
Bits 62:59  Software-defined
Bit  63    XN (execute-never)
```

**Fault conditions checked during S1 walk**:
- Descriptor not valid → `F_TRANSLATION`
- Permission violation (write to read-only, EL0 access to EL1-only) → `F_PERMISSION`
- Address size fault (output PA > SMMU OAS) → `F_ADDR_SIZE`
- Access flag fault (AF=0 and AFFD=0) → `F_ACCESS`

### 5.2 Stage 2 Page Table Walk (IPA → PA)

Stage 2 uses `STE.S2TTB` as the base, `STE.S2T0SZ` as the input size, `STE.S2TG` as the granule. The descriptor format is identical to Stage 1.

### 5.3 Nested Translation

When the STE Config field specifies both S1 and S2 (`Config=0b111`):
1. The IO device issues a transaction with VA
2. SMMU walks Stage 1 (VA → IPA using CD)
3. SMMU walks Stage 2 (IPA → PA using STE S2 fields)
4. The Stage 1 table walk itself also goes through Stage 2 (protected table walk, `S2PTW=1`)

### 5.4 TLB Model in the Simulator

The simulator uses a simple direct-mapped software TLB to cache translation results:

```rust
pub struct SmmuTlb {
    /// Direct-mapped by (stream_id ^ va >> 12) & (SMMU_TLB_SIZE - 1)
    entries: Vec<SmmuTlbEntry>,
}

pub struct SmmuTlbEntry {
    pub valid: bool,
    pub stream_id: u32,
    pub asid: u16,
    pub va: u64,
    pub pa: u64,
    pub size: u64,   // 4KB, 2MB, 1GB
    pub prot: u32,   // R/W/X flags
}
```

TLB invalidation is performed synchronously on `CFGI_STE_RANGE`, `TLBI_NH_VA`, `TLBI_NH_ASID`, and `TLBI_NH_ALL` commands.

---

## 6. Fault Model

### 6.1 Event Queue

When a translation fault occurs, the SMMU writes a 32-byte **event record** to the event queue at `SMMU_EVTQ_PROD` and advances the producer pointer. An interrupt is generated if `SMMU_IRQ_CTRL.EVTQ_IRQEN=1`.

### 6.2 Event Record Format

```
DW0  Bits 7:0    Type (fault type code — see table below)
     Bits 15:8   Stall (1 = stalled transaction, awaiting RESUME command)
     Bits 63:32  StreamID
DW1  Bits 63:0   Input (faulting) address (VA or IPA)
DW2  Bits 63:0   Flags (INSTFETCH, WRITE, etc.)
DW3  Bits 63:0   Implementation-defined / IPA (Stage 2 fault)
```

### 6.3 Fault Type Codes

| Code (hex) | Name | Trigger |
|------------|------|---------|
| `0x01` | `C_BAD_STREAMID` | SID exceeds table size |
| `0x02` | `F_STE_FETCH` | Error reading STE from memory |
| `0x03` | `C_BAD_STE` | STE valid=0 or Config=Abort |
| `0x04` | `F_BAD_ATS_TREQ` | PCIe ATS translation request error |
| `0x05` | `F_STREAM_DISABLED` | SMMU disabled (SMMUEN=0) and GBPA=Abort |
| `0x06` | `F_TRANSL_FORBIDDEN` | Translation globally forbidden |
| `0x07` | `C_BAD_SUBSTREAMID` | SubstreamID exceeds CD table size |
| `0x08` | `F_CD_FETCH` | Error reading CD from memory |
| `0x09` | `C_BAD_CD` | CD valid=0 |
| `0x0A` | `F_WALK_EABT` | External abort during page table walk |
| `0x10` | `F_TRANSLATION` | Page table entry not valid (no mapping) |
| `0x11` | `F_ADDR_SIZE` | Output PA exceeds physical address size |
| `0x12` | `F_ACCESS` | Access flag fault (AF=0) |
| `0x13` | `F_PERMISSION` | Permission fault (write to RO, EL0 to EL1-only) |
| `0x20` | `F_TLB_CONFLICT` | Conflicting TLB entries (impl-defined) |
| `0x24` | `F_CFG_CONFLICT` | Configuration conflict |
| `0x25` | `E_PAGE_REQUEST` | PCIe Page Request (PRI queue) |
| `0x26` | `F_VMS_FETCH` | Error reading VSID mapping (GICv4) |

### 6.4 GERROR Register

The `SMMU_GERROR` register holds sticky global error flags:

```
Bit  8   MSI_ABT_ERR  — MSI delivery aborted
Bit  7   PRIQ_ABT_ERR — PRI queue write aborted
Bit  6   EVTQ_ABT_ERR — Event queue write aborted
Bit  4   CMDQ_ERR     — Command processing error
Bit  2   EVENTQ_OVF   — Event queue overflow
Bit  0   PRIQ_OVF     — PRI queue overflow
```

---

## 7. GICv3 Integration

### 7.1 SPI-Based Interrupt Wiring

The SMMU generates up to three interrupt outputs:
- **Global fault interrupt (GERROR)**: fires when `SMMU_GERROR` has unacknowledged errors and `SMMU_IRQ_CTRL.GERROR_IRQEN=1`
- **Event queue interrupt**: fires when the event queue has unread entries and `EVTQ_IRQEN=1`
- **PRI queue interrupt** (Phase 3): fires when PRI queue has entries and `PRIQ_IRQEN=1`

These are wired as SPIs to the GICv3 distributor using `InterruptPin`:

```rust
pub struct SmmuState {
    pub gerror_irq: InterruptPin,   // → GICv3 SPI, e.g. INTID 74
    pub evtq_irq:   InterruptPin,   // → GICv3 SPI, e.g. INTID 75
    pub priq_irq:   InterruptPin,   // → GICv3 SPI, e.g. INTID 76 (Phase 3)
}
```

The platform wires these pins to the GIC sink during `elaborate()`:

```python
# Python config
smmu.gerror_irq_num = 74
smmu.evtq_irq_num   = 75
```

### 7.2 MSI-Based Interrupt Wiring (Optional)

If the platform's GICv3 supports LPIs and has an ITS, the SMMU can deliver interrupts via MSI instead of SPI. In this case, `SMMU_EVTQ_IRQ_CFG0/1/2` are programmed by software with the `GITS_TRANSLATER` address and MSI data. The SMMU then writes to the ITS on interrupt.

In the simulator, MSI delivery maps to `its.translate_msi(device_id, event_id)` → sets LPI pending in the target GICR.

### 7.3 INTID Allocation for QEMU virt Compatibility

Following the QEMU `virt` machine allocation for SMMU-v3:

```
SPI 74 (INTID 106)  SMMU global fault (GERROR)
SPI 75 (INTID 107)  SMMU command queue error
SPI 76 (INTID 108)  SMMU event queue
SPI 77 (INTID 109)  SMMU PRI queue (Phase 3)
```

---

## 8. helm-ng Integration

### 8.1 Where SMMU Sits in HelmAddressSpace

The `HelmAddressSpace` (FlatMem + AddressMap + Device dispatch) is augmented with an optional SMMU layer:

```
DMA device issues transaction with TransactionAttrs { stream_id: u32, ... }
    ↓
SmmuTranslate::translate(stream_id, iova, attrs) → PA
    ↓
HelmAddressSpace::read/write(PA, size, data)
    ↓
FlatMem or MMIO device
```

The `SmmuTranslate` struct wraps `Arc<Mutex<SmmuState>>` and provides a translation front-end that can be bypassed when `SMMUEN=0` or when no SMMU is configured.

### 8.2 TransactionAttrs.stream_id

The existing `TransactionAttrs` struct in `helm-devices` gains a `stream_id` field:

```rust
pub struct TransactionAttrs {
    pub initiator: InitiatorId,
    pub secure: bool,
    pub privileged: bool,
    pub instruction_fetch: bool,
    pub stream_id: Option<u32>,  // None = CPU transaction (not subject to SMMU)
    pub sub_stream_id: Option<u32>,
}
```

CPU-initiated transactions (`stream_id = None`) bypass the SMMU entirely. Device-initiated DMA transactions carry a `stream_id` matching the device's SID assignment.

### 8.3 SmmuState Struct

```rust
pub struct SmmuState {
    // ── SMMU global control ──────────────────────────────────────────────────
    pub cr0: u32,           // SMMU_CR0: SMMUEN, CMDQEN, EVTQEN, PRIQEN
    pub cr1: u32,           // SMMU_CR1: cache/shareability attrs
    pub cr2: u32,           // SMMU_CR2: RECINVSID etc.
    pub gbpa: u32,          // SMMU_GBPA: global bypass/abort config
    pub gerror: u32,        // SMMU_GERROR: sticky global errors
    pub irq_ctrl: u32,      // SMMU_IRQ_CTRL: per-queue IRQ enables
    pub statusr: u32,       // SMMU_STATUSR: IDLE, DORMANT

    // ── Stream table ────────────────────────────────────────────────────────
    pub strtab_base: u64,       // SMMU_STRTAB_BASE
    pub strtab_base_cfg: u32,   // SMMU_STRTAB_BASE_CFG
    // Parsed config:
    pub strtab_fmt: StrtabFmt,  // Linear or TwoLevel
    pub strtab_log2size: u8,    // number of SID bits
    pub strtab_split: u8,       // L1/L2 split for 2-level

    // ── Command queue ────────────────────────────────────────────────────────
    pub cmdq_base: u64,     // SMMU_CMDQ_BASE
    pub cmdq_prod: u32,     // SMMU_CMDQ_PROD (mirrored)
    pub cmdq_cons: u32,     // SMMU_CMDQ_CONS

    // ── Event queue ─────────────────────────────────────────────────────────
    pub evtq_base: u64,     // SMMU_EVTQ_BASE
    pub evtq_prod: u32,     // SMMU_EVTQ_PROD
    pub evtq_cons: u32,     // SMMU_EVTQ_CONS
    pub evtq_irq_cfg: [u64; 3], // MSI config for EVTQ IRQ

    // ── TLB ─────────────────────────────────────────────────────────────────
    pub tlb: SmmuTlb,

    // ── IRQ output pins ─────────────────────────────────────────────────────
    pub gerror_irq: InterruptPin,
    pub evtq_irq:   InterruptPin,

    // ── Reference to guest memory for table walks ────────────────────────────
    pub mem: Arc<Mutex<HelmAddressSpace>>,
}
```

### 8.4 SMMU as a Device

`SmmuState` implements the `Device` trait for its MMIO control register region (64KB):

```rust
impl Device for Smmu {
    fn read(&mut self, offset: u64, size: usize) -> u64 { ... }
    fn write(&mut self, offset: u64, size: usize, val: u64) { ... }
    fn region_size(&self) -> u64 { 0x1_0000 } // 64KB page 0
}
```

A second region may be mapped at `SMMU_BASE + 0x10000` for page 1 (implementation-defined; not required for Phase 2).

### 8.5 Platform Placement

In the QEMU virt-compatible arm-virt platform:
```
SMMU at 0x09050000 (64KB)
GICv3 GICD at 0x08000000 (64KB)
GICv3 GICR at 0x080A0000 (128KB per PE)
UART at 0x09000000 (4KB)
RAM at 0x40000000+
```

The DTB node:
```
iommu@9050000 {
    compatible = "arm,smmu-v3";
    reg = <0x0 0x09050000 0x0 0x20000>;
    interrupts = <GIC_SPI 74 IRQ_TYPE_EDGE_RISING>,
                 <GIC_SPI 75 IRQ_TYPE_EDGE_RISING>,
                 <GIC_SPI 76 IRQ_TYPE_EDGE_RISING>,
                 <GIC_SPI 77 IRQ_TYPE_EDGE_RISING>;
    interrupt-names = "eventq", "gerror", "priq", "cmdq-sync";
    #iommu-cells = <1>;
};
```

---

## 9. GICv4 Note — Virtual LPI Direct Injection

### 9.1 What GICv4 Adds

GICv4 (and GICv4.1) extends the GICv3 redistributor with hardware support for **direct injection of virtual LPIs (vLPIs)** to guest VMs, eliminating the need for a hypervisor trap on every guest interrupt.

In GICv3, when a peripheral targets a vCPU (guest CPU), the ITS must:
1. Write the LPI pending bit in the pLPI pending table (physical)
2. Trigger a maintenance interrupt to the hypervisor
3. The hypervisor injects the virtual interrupt via the GIC virtual CPU interface

In GICv4, the ITS can directly inject a **virtual interrupt** into a vCPU without a hypervisor trap, provided the vPE (virtual PE) is currently scheduled on a physical PE.

### 9.2 GICv4 Redistributor Changes

The GICv4 redistributor adds a **VLPI frame** (a third 64KB sub-frame beyond the RD_base and SGI_base):

```
GICR_BASE + cpu_idx * 0x30000   (note: 192KB per PE in GICv4 vs 128KB in GICv3)
  RD_base  offset 0x00000  — unchanged from GICv3
  SGI_base offset 0x10000  — unchanged from GICv3
  VLPI frame offset 0x20000  — NEW in GICv4
```

VLPI frame registers (selected):
```
Offset   Register          Description
0x0000   GICR_VPROPBASER   vLPI configuration table base (for virtual LPI config)
0x0008   GICR_VPENDBASER   vLPI pending table base (virtual pending)
```

`GICR_TYPER` gains additional bits:
```
Bit 17   RVPEID   — GICR_VPENDBASER.RVPEID supported (resident vPE ID)
Bit 16   VLPIS    — virtual LPI injection supported (GICv4)
```

### 9.3 ITS Changes for GICv4

The ITS gains new commands for virtual interrupt management:
```
VMAPP   — map a vPE (virtual PE) to a physical redistributor
VMAPTI  — map an EventID to a virtual INTID + vPE
VMOVI   — move a virtual interrupt to a different vPE
VINVALL — invalidate all virtual interrupts for a vPE
VSYNC   — sync virtual interrupt state for a vPE
```

### 9.4 Scope in helm-ng

GICv4 is **Phase 3 complexity** and is not required for bare-metal Linux boot (Phase 0–2). The GICv3 implementation will set `GICR_TYPER.VLPIS=0` to advertise no vLPI support. A GICv4 upgrade requires:

1. Extending `Gicv3RedistState` with `vpropbaser` and `vpendbaser` fields
2. Adding the VLPI sub-frame at `+0x20000` per redistributor
3. Changing `build_gicv3_mp` stride to `0x30000`
4. Extending `ItsState` with virtual mapping tables and GICv4 command handlers
5. Wiring VMAPP/VMAPTI to the hypervisor VGIC model in `helm-engine`

This is deferred until Phase 3 hypervisor support (`HCR_EL2` virtualization, EL2 exception model).

---

## 10. Work Items

- [ ] WI-SMMU-001: Define `SmmuState` struct in `hw/helm-hw-smmu/src/lib.rs` with all control register fields
- [ ] WI-SMMU-002: Implement `Device` trait for `Smmu`: `read()` / `write()` for all registers in section 2
- [ ] WI-SMMU-003: Implement `SmmuState::process_cmdq()` — ring buffer drain on CMDQ_PROD write
- [ ] WI-SMMU-004: Implement `process_command()` for CFGI_STE_RANGE, CFGI_ALL, TLBI_NH_VA, TLBI_NH_ASID, TLBI_NH_ALL, CMD_SYNC
- [ ] WI-SMMU-005: Implement `SmmuState::lookup_ste(stream_id)` — linear and 2-level stream table lookup in guest RAM
- [ ] WI-SMMU-006: Implement `SmmuState::lookup_cd(ste, sub_stream_id)` — CD fetch from guest RAM
- [ ] WI-SMMU-007: Implement `SmmuState::walk_s1(cd, va)` — 4KB/16KB/64KB granule, 2–4 level walk
- [ ] WI-SMMU-008: Implement `SmmuState::walk_s2(ste, ipa)` — Stage 2 walk using STE S2TTB/S2T0SZ/S2TG
- [ ] WI-SMMU-009: Implement `SmmuState::translate(stream_id, va, attrs)` — full pipeline: STE → CD → S1 → S2 → PA
- [ ] WI-SMMU-010: Implement `SmmuTlb` direct-mapped cache; `tlb_lookup()` and `tlb_fill()` functions
- [ ] WI-SMMU-011: Implement fault recording: `write_event_record(fault_type, stream_id, va)` → EVTQ_PROD advance
- [ ] WI-SMMU-012: Implement `update_irq_lines()` — assert `gerror_irq` / `evtq_irq` based on queue state and IRQ_CTRL
- [ ] WI-SMMU-013: Add `stream_id: Option<u32>` and `sub_stream_id: Option<u32>` to `TransactionAttrs` in `helm-devices`
- [ ] WI-SMMU-014: Implement `SmmuTranslate::translate()` front-end that bypasses when `SMMUEN=0` or `stream_id=None`
- [ ] WI-SMMU-015: Wire `SmmuTranslate` into DMA transaction path in `HelmAddressSpace` (Phase 3)
- [ ] WI-SMMU-016: Create `hw/helm-hw-smmu/` crate with `Cargo.toml` depending on `helm-devices` and `helm-memory`
- [ ] WI-SMMU-017: Update arm-virt platform to map SMMU at `0x09050000` and wire GERROR/EVTQ SPIs to GICv3
- [ ] WI-SMMU-018: Generate DTB SMMU-v3 node with correct `reg`, `interrupts`, and `#iommu-cells = <1>`
- [ ] WI-SMMU-019: Implement bypass mode (SMMU disabled or STE Config=Bypass): passthrough with no translation
- [ ] WI-SMMU-020: Implement abort mode (STE valid=0 or Config=Abort): write fault event, return bus error
- [ ] WI-SMMU-021: Write unit tests: STE lookup, CD fetch, single-stage 4KB walk, fault generation, command queue drain
- [ ] WI-SMMU-022: Write integration test: device DMA with SMMU enabled, verify PA matches expected translation
- [ ] WI-SMMU-023: Phase 3 — Implement MSI event queue interrupt delivery via ITS (EVTQ_IRQ_CFG path)
- [ ] WI-SMMU-024: Phase 3 — Implement PRI queue for PCIe ATS support
- [ ] WI-SMMU-025: Phase 3 (GICv4) — Extend GICR to 192KB with VLPI sub-frame and new GICR_TYPER bits
