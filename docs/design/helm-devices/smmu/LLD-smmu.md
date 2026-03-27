# SMMU-v3 — Low-Level Design

> Cross-references: [`HLD.md`](./HLD.md) · [`ARCHITECTURE.md`](../../../ARCHITECTURE.md) · [`traits.md`](../../../traits.md) · [`LLD-device-trait.md`](../LLD-device-trait.md) · [`LLD-interrupt-model.md`](../LLD-interrupt-model.md) · [`gicv3/LLD-gicv3.md`](../gicv3/LLD-gicv3.md)

---

## Table of Contents

1. [SmmuState Struct Layout](#1-smmustate-struct-layout)
2. [Device Trait Implementation](#2-device-trait-implementation)
3. [Command Queue Processing](#3-command-queue-processing)
4. [Translation Walk Pipeline](#4-translation-walk-pipeline)
5. [TLB Cache Design](#5-tlb-cache-design)
6. [Fault Model](#6-fault-model)
7. [Module Structure](#7-module-structure)
8. [Work Items](#8-work-items)

---

## 1. SmmuState Struct Layout

### 1.1 GuestMem Trait

The SMMU must read guest physical memory to fetch stream table entries, context descriptors, and page table descriptors during translation walks. A trait abstracts this to allow unit testing with a mock memory:

```rust
/// Trait for reading guest physical memory during SMMU table walks.
///
/// The SMMU needs to fetch STEs, CDs, and page table descriptors from
/// guest RAM. This trait decouples the walk logic from HelmAddressSpace,
/// allowing unit tests to supply a simple Vec<u8> backing.
pub trait GuestMem {
    /// Read `size` bytes from guest physical address `pa`.
    /// Returns the value zero-extended to u64.
    /// Returns None if the address is unmapped or faults.
    fn guest_read(&self, pa: u64, size: usize) -> Option<u64>;

    /// Read a contiguous byte slice from guest memory.
    /// Used for multi-word STE/CD fetches (64 bytes).
    fn guest_read_bytes(&self, pa: u64, buf: &mut [u8]) -> bool;
}
```

`HelmAddressSpace` implements `GuestMem` by delegating to `FlatMem::read()`. The unit test mock is a `Vec<u8>` with bounds checking.

### 1.2 SmmuState

```rust
pub struct SmmuState {
    // ── Identification (read-only, set at construction) ─────────────────
    /// SMMU_IDR0: feature flags (ST_LEVEL=2level, STALL=0, HTTU=0,
    /// ASID16=1, S1P=1, S2P=1). Precomputed at build time.
    pub idr0: u32,
    /// SMMU_IDR1: CMDQS=log2 cmd queue depth, EVTQS=log2 evt queue depth.
    pub idr1: u32,
    /// SMMU_IDR2: IAS=48, OAS from platform (typically 48).
    pub idr2: u32,
    /// SMMU_IDR3: RIL=0 (no range invalidation large).
    pub idr3: u32,
    /// SMMU_IDR4: implementation defined, 0.
    pub idr4: u32,
    /// SMMU_IDR5: OAS encoding, GRAN4K=1, GRAN16K=0, GRAN64K=0, VAX=0.
    pub idr5: u32,
    /// SMMU_IIDR: implementer/product ID (helm-ng placeholder).
    pub iidr: u32,
    /// SMMU_AIDR: architecture version (0x1 = SMMU-v3.1).
    pub aidr: u32,

    // ── Global control (RW) ────────────────────────────────────────────
    /// SMMU_CR0: SMMUEN[0], CMDQEN[1], EVTQEN[2], PRIQEN[3].
    pub cr0: u32,
    /// SMMU_CR0ACK: mirrors CR0 after acknowledgment (written by sim
    /// synchronously on CR0 write — no delay modeled).
    pub cr0ack: u32,
    /// SMMU_CR1: cache/shareability attributes for table walks.
    pub cr1: u32,
    /// SMMU_CR2: RECINVSID, E2H.
    pub cr2: u32,
    /// SMMU_STATUSR: DORMANT/IDLE — always 0 in simulator (never idle).
    pub statusr: u32,
    /// SMMU_GBPA: Global Bypass/Abort. ABORT[20], SHCFG, MTCFG, MemAttr.
    pub gbpa: u32,
    /// SMMU_AGBPA: Alternate Global Bypass/Abort.
    pub agbpa: u32,

    // ── IRQ control ────────────────────────────────────────────────────
    /// SMMU_IRQ_CTRL: EVTQ_IRQEN[0], PRIQ_IRQEN[1], GERROR_IRQEN[2].
    pub irq_ctrl: u32,
    /// SMMU_IRQ_CTRLACK: mirrors IRQ_CTRL after acknowledgment.
    pub irq_ctrlack: u32,
    /// SMMU_GERROR: sticky global error flags.
    pub gerror: u32,
    /// SMMU_GERRORN: written by software to acknowledge GERROR bits.
    pub gerrorn: u32,
    /// SMMU_GERROR_IRQ_CFG0: MSI address for GERROR (64-bit).
    pub gerror_irq_cfg0: u64,
    /// SMMU_GERROR_IRQ_CFG1: MSI data for GERROR.
    pub gerror_irq_cfg1: u32,
    /// SMMU_GERROR_IRQ_CFG2: MSI control for GERROR.
    pub gerror_irq_cfg2: u32,

    // ── Stream table ───────────────────────────────────────────────────
    /// SMMU_STRTAB_BASE: base PA (page-aligned) + RA/ADDR fields (64-bit).
    pub strtab_base: u64,
    /// SMMU_STRTAB_BASE_CFG: FMT[17:16], SPLIT[10:6], LOG2SIZE[5:0].
    pub strtab_base_cfg: u32,
    // Parsed (cached on STRTAB_BASE_CFG write):
    /// 0 = linear, 1 = 2-level.
    pub strtab_fmt: StrtabFmt,
    /// Number of SID bits (from LOG2SIZE field).
    pub strtab_log2size: u8,
    /// L1/L2 split for 2-level tables (from SPLIT field).
    pub strtab_split: u8,

    // ── Command queue ──────────────────────────────────────────────────
    /// SMMU_CMDQ_BASE: base PA + LOG2SIZE (64-bit).
    pub cmdq_base: u64,
    /// SMMU_CMDQ_PROD: producer index (written by software).
    /// Bits [19:0] = index + wrap bit in bit [log2size].
    pub cmdq_prod: u32,
    /// SMMU_CMDQ_CONS: consumer index (advanced by SMMU after processing).
    /// Bits [19:0] = index + wrap bit; bits [30:24] = ERR on error.
    pub cmdq_cons: u32,

    // ── Event queue ────────────────────────────────────────────────────
    /// SMMU_EVTQ_BASE: base PA + LOG2SIZE (64-bit).
    pub evtq_base: u64,
    /// SMMU_EVTQ_PROD: producer index (advanced by SMMU on fault record).
    pub evtq_prod: u32,
    /// SMMU_EVTQ_CONS: consumer index (advanced by software after read).
    pub evtq_cons: u32,
    /// SMMU_EVTQ_IRQ_CFG0: MSI address for event queue (64-bit).
    pub evtq_irq_cfg0: u64,
    /// SMMU_EVTQ_IRQ_CFG1: MSI data.
    pub evtq_irq_cfg1: u32,
    /// SMMU_EVTQ_IRQ_CFG2: MSI control.
    pub evtq_irq_cfg2: u32,

    // ── PRI queue (Phase 3, stubbed) ───────────────────────────────────
    /// SMMU_PRIQ_BASE, SMMU_PRIQ_PROD, SMMU_PRIQ_CONS.
    pub priq_base: u64,
    pub priq_prod: u32,
    pub priq_cons: u32,

    // ── TLB ────────────────────────────────────────────────────────────
    pub tlb: SmmuTlb,

    // ── IRQ output pins ────────────────────────────────────────────────
    /// Wired to GICv3 SPI 74 (GERROR).
    pub gerror_irq: InterruptPin,
    /// Wired to GICv3 SPI 76 (event queue).
    pub evtq_irq: InterruptPin,

    // ── Guest memory reference for table walks ─────────────────────────
    /// Set during elaborate(). The SMMU reads STEs, CDs, and page table
    /// descriptors from guest RAM via this reference.
    pub mem: Option<Arc<Mutex<dyn GuestMem + Send>>>,
}
```

### 1.3 StrtabFmt Enum

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrtabFmt {
    Linear,
    TwoLevel,
}
```

### 1.4 SmmuState Constructor

```rust
impl SmmuState {
    pub fn new(oas_bits: u8) -> Self {
        // IDR0: S1P=1, S2P=1, ASID16=1, ST_LEVEL=1 (2-level supported),
        //       STALL_MODEL=0 (no stall), HTTU=0 (no hardware update).
        let idr0 = (1 << 27)  // S1P
                 | (1 << 26)  // S2P
                 | (1 << 12)  // ASID16
                 | (1 << 0);  // ST_LEVEL (2-level capable)

        // IDR1: CMDQS=8 (256 entries), EVTQS=8 (256 entries).
        let idr1 = (8 << 21)  // CMDQS
                 | (8 << 16); // EVTQS

        // IDR5: OAS encoding + GRAN4K=1.
        let oas_enc = encode_oas(oas_bits);
        let idr5 = (oas_enc << 0)  // OAS
                 | (1 << 4);       // GRAN4K

        Self {
            idr0, idr1, idr2: 0, idr3: 0, idr4: 0, idr5,
            iidr: 0x48454C4D, // "HELM"
            aidr: 0x1,        // SMMUv3.1
            cr0: 0, cr0ack: 0, cr1: 0, cr2: 0,
            statusr: 0, gbpa: 0, agbpa: 0,
            irq_ctrl: 0, irq_ctrlack: 0,
            gerror: 0, gerrorn: 0,
            gerror_irq_cfg0: 0, gerror_irq_cfg1: 0, gerror_irq_cfg2: 0,
            strtab_base: 0, strtab_base_cfg: 0,
            strtab_fmt: StrtabFmt::Linear, strtab_log2size: 0, strtab_split: 0,
            cmdq_base: 0, cmdq_prod: 0, cmdq_cons: 0,
            evtq_base: 0, evtq_prod: 0, evtq_cons: 0,
            evtq_irq_cfg0: 0, evtq_irq_cfg1: 0, evtq_irq_cfg2: 0,
            priq_base: 0, priq_prod: 0, priq_cons: 0,
            tlb: SmmuTlb::new(),
            gerror_irq: InterruptPin::new(),
            evtq_irq: InterruptPin::new(),
            mem: None,
        }
    }
}
```

---

## 2. Device Trait Implementation

### 2.1 MMIO Region

`SmmuState` implements `Device` for a 64KB MMIO region (page 0 of the SMMU register space).

```rust
impl Device for SmmuState {
    fn region_size(&self) -> u64 { 0x1_0000 } // 64KB
    fn read(&mut self, offset: u64, size: usize) -> u64 { ... }
    fn write(&mut self, offset: u64, size: usize, val: u64) { ... }
}
```

### 2.2 Read Dispatch Table

The `read()` method dispatches on `offset`:

```rust
fn read(&mut self, offset: u64, size: usize) -> u64 {
    match offset {
        // ── Identification (RO) ──────────────────────────────────────
        0x0000 => self.idr0 as u64,
        0x0004 => self.idr1 as u64,
        0x0008 => self.idr2 as u64,
        0x000C => self.idr3 as u64,
        0x0010 => self.idr4 as u64,
        0x0014 => self.idr5 as u64,
        0x0018 => self.iidr as u64,
        0x001C => self.aidr as u64,

        // ── Control ──────────────────────────────────────────────────
        0x0020 => self.cr0 as u64,
        0x0024 => self.cr0ack as u64,
        0x0028 => self.cr1 as u64,
        0x002C => self.cr2 as u64,
        0x0040 => self.statusr as u64,
        0x0044 => self.gbpa as u64,
        0x0048 => self.agbpa as u64,

        // ── IRQ control ──────────────────────────────────────────────
        0x0060 => self.irq_ctrl as u64,
        0x0064 => self.irq_ctrlack as u64,
        0x0068 => self.gerror as u64,
        0x006C => self.gerrorn as u64,
        0x0070 => self.gerror_irq_cfg0,           // 64-bit
        0x0078 => self.gerror_irq_cfg1 as u64,
        0x007C => self.gerror_irq_cfg2 as u64,

        // ── Stream table ─────────────────────────────────────────────
        0x0080 => self.strtab_base,                // 64-bit
        0x0088 => self.strtab_base_cfg as u64,

        // ── Command queue ────────────────────────────────────────────
        0x0090 => self.cmdq_base,                  // 64-bit
        0x0098 => self.cmdq_prod as u64,
        0x009C => self.cmdq_cons as u64,

        // ── Event queue ──────────────────────────────────────────────
        0x00A0 => self.evtq_base,                  // 64-bit
        0x00A8 => self.evtq_prod as u64,
        0x00AC => self.evtq_cons as u64,
        0x00B0 => self.evtq_irq_cfg0,             // 64-bit
        0x00B8 => self.evtq_irq_cfg1 as u64,
        0x00BC => self.evtq_irq_cfg2 as u64,

        // ── PRI queue (stubbed) ──────────────────────────────────────
        0x00C0 => self.priq_base,                  // 64-bit
        0x00C8 => self.priq_prod as u64,
        0x00CC => self.priq_cons as u64,

        _ => {
            log::debug!("SMMU: read from undefined offset {:#06x}", offset);
            0
        }
    }
}
```

**64-bit register reads.** Registers at `0x0070`, `0x0080`, `0x0090`, `0x00A0`, `0x00B0`, `0x00C0` are 64-bit. When `size == 4`, the read returns the low or high 32 bits depending on the exact offset alignment. The dispatch shown above handles only aligned 64-bit accesses; sub-word access falls through to the 32-bit paths by splitting internally:

```rust
// Inside read(), before the main match:
if size == 4 {
    let aligned = offset & !0x7;
    let full = self.read(aligned, 8);
    return if offset & 4 == 0 { full & 0xFFFF_FFFF } else { full >> 32 };
}
```

### 2.3 Write Dispatch Table

```rust
fn write(&mut self, offset: u64, size: usize, val: u64) {
    match offset {
        // ── RO registers: silently ignore writes ─────────────────────
        0x0000..=0x001C | 0x0024 | 0x0040 | 0x0064 | 0x0068 => {}

        // ── CR0 → CR0ACK mirror + side effects ──────────────────────
        0x0020 => {
            self.cr0 = val as u32 & 0xF;  // bits [3:0] only
            self.cr0ack = self.cr0;       // instant acknowledgment
        }

        0x0028 => { self.cr1 = val as u32; }
        0x002C => { self.cr2 = val as u32; }

        // ── GBPA ─────────────────────────────────────────────────────
        0x0044 => { self.gbpa = val as u32; }
        0x0048 => { self.agbpa = val as u32; }

        // ── IRQ_CTRL → IRQ_CTRLACK mirror ────────────────────────────
        0x0060 => {
            self.irq_ctrl = val as u32 & 0x7;  // bits [2:0]
            self.irq_ctrlack = self.irq_ctrl;
        }

        // ── GERRORN: acknowledge GERROR bits ─────────────────────────
        // Active errors = GERROR ^ GERRORN. Writing a 1 to GERRORN
        // clears the corresponding GERROR active bit.
        0x006C => {
            self.gerrorn = val as u32;
            self.update_irq_lines();
        }

        // ── GERROR MSI config ────────────────────────────────────────
        0x0070 => { self.gerror_irq_cfg0 = val; }
        0x0078 => { self.gerror_irq_cfg1 = val as u32; }
        0x007C => { self.gerror_irq_cfg2 = val as u32; }

        // ── Stream table config ──────────────────────────────────────
        0x0080 => { self.strtab_base = val; }
        0x0088 => {
            self.strtab_base_cfg = val as u32;
            // Parse cached fields:
            self.strtab_fmt = match (val >> 16) & 0x3 {
                0 => StrtabFmt::Linear,
                1 => StrtabFmt::TwoLevel,
                _ => StrtabFmt::Linear,  // reserved → treat as linear
            };
            self.strtab_log2size = (val & 0x3F) as u8;
            self.strtab_split = ((val >> 6) & 0x1F) as u8;
        }

        // ── Command queue ────────────────────────────────────────────
        0x0090 => { self.cmdq_base = val; }
        0x0098 => {
            self.cmdq_prod = val as u32;
            // Side effect: drain the command queue synchronously.
            if self.cr0 & 0x2 != 0 {  // CMDQEN
                self.process_cmdq();
            }
        }
        0x009C => { self.cmdq_cons = val as u32; }

        // ── Event queue ──────────────────────────────────────────────
        0x00A0 => { self.evtq_base = val; }
        0x00A8 => {}  // EVTQ_PROD is SMMU-managed (RO to software)
        0x00AC => {
            self.evtq_cons = val as u32;
            // Software advanced consumer → may deassert EVTQ IRQ.
            self.update_irq_lines();
        }
        0x00B0 => { self.evtq_irq_cfg0 = val; }
        0x00B8 => { self.evtq_irq_cfg1 = val as u32; }
        0x00BC => { self.evtq_irq_cfg2 = val as u32; }

        // ── PRI queue (stubbed, Phase 3) ─────────────────────────────
        0x00C0 => { self.priq_base = val; }
        0x00C8 => {}  // PRIQ_PROD is SMMU-managed
        0x00CC => { self.priq_cons = val as u32; }

        _ => {
            log::debug!("SMMU: write to undefined offset {:#06x}", offset);
        }
    }
}
```

### 2.4 CR0 → CR0ACK Mirroring

Real hardware may delay the acknowledgment of CR0 writes (e.g., waiting for queue drain). The simulator acknowledges instantly: every CR0 write copies the value to CR0ACK in the same call. Software that polls `CR0ACK` after a `CR0` write will see the new value immediately on the next read.

### 2.5 CMDQ_PROD Write Side Effect

Writing `SMMU_CMDQ_PROD` triggers `process_cmdq()` synchronously (see section 3). This is the only register write with a complex side effect. All command processing completes before the MMIO write returns to the CPU.

---

## 3. Command Queue Processing

### 3.1 Ring Buffer Algorithm

The command queue is a power-of-two circular buffer. The producer index (`CMDQ_PROD`) and consumer index (`CMDQ_CONS`) use a wrap bit in bit `[log2size]` to distinguish full from empty:

```
depth     = 1 << log2size          (number of 16-byte entries)
mask      = depth - 1              (index mask, excludes wrap bit)
wrap_mask = (2 * depth) - 1        (index + wrap bit mask)

empty:    (PROD & wrap_mask) == (CONS & wrap_mask)
entries:  entry_pa = base_pa + (index & mask) * 16
```

### 3.2 Drain Loop

```rust
impl SmmuState {
    /// Process all pending commands in the command queue.
    /// Called synchronously on CMDQ_PROD write when CMDQEN=1.
    fn process_cmdq(&mut self) {
        let log2size = (self.cmdq_base & 0x1F) as u32;
        let depth = 1u32 << log2size;
        let mask = depth - 1;
        let wrap_mask = (2 * depth) - 1;
        let base_pa = self.cmdq_base & !0x1F & !0xFFF
                    | (self.cmdq_base & 0xFFFF_FFFF_FFFF_F000);
        // More precisely: base PA is bits [51:5] << 5, page-aligned.
        let base_pa = self.cmdq_base & 0x000F_FFFF_FFFF_FFE0;

        let mem = match &self.mem {
            Some(m) => m.clone(),
            None => return,  // no guest memory wired — skip
        };
        let mem = mem.lock().unwrap();

        let mut cons = self.cmdq_cons & wrap_mask;
        let prod = self.cmdq_prod & wrap_mask;

        while cons != prod {
            let idx = cons & mask;
            let entry_pa = base_pa + (idx as u64) * 16;

            // Read 16-byte command entry from guest RAM.
            let mut cmd_bytes = [0u8; 16];
            if !mem.guest_read_bytes(entry_pa, &mut cmd_bytes) {
                // Memory read failed — set CMDQ_ERR in GERROR.
                self.gerror |= 1 << 4;  // CMDQ_ERR
                self.update_irq_lines();
                return;
            }

            let dw0 = u64::from_le_bytes(cmd_bytes[0..8].try_into().unwrap());
            let dw1 = u64::from_le_bytes(cmd_bytes[8..16].try_into().unwrap());
            let opcode = (dw0 & 0xFF) as u8;

            self.process_command(opcode, dw0, dw1);

            cons = (cons + 1) & wrap_mask;
        }

        self.cmdq_cons = cons;
    }
}
```

### 3.3 Command Dispatch

```rust
impl SmmuState {
    fn process_command(&mut self, opcode: u8, dw0: u64, dw1: u64) {
        match opcode {
            // ── CFGI_STE_RANGE (0x04) ────────────────────────────────
            // Invalidate cached STE entries for a range of SIDs.
            // DW0[46:32] = SID, DW0[52:48] = RANGE (2^RANGE entries).
            0x04 => {
                let sid = ((dw0 >> 32) & 0x7FFF) as u32;
                let range = ((dw0 >> 48) & 0x1F) as u32;
                let count = 1u32 << range;
                for s in sid..sid.saturating_add(count) {
                    self.tlb.flush_by_sid(s);
                }
            }

            // ── CFGI_ALL (0x05) ──────────────────────────────────────
            // Invalidate all cached STEs.
            0x05 => {
                self.tlb.flush_all();
            }

            // ── CFGI_CD (0x06) ───────────────────────────────────────
            // Invalidate CD for a specific SID + SubstreamID.
            // For the simulator, this is equivalent to flushing by SID.
            0x06 => {
                let sid = ((dw0 >> 32) & 0x7FFF) as u32;
                self.tlb.flush_by_sid(sid);
            }

            // ── CFGI_CD_ALL (0x07) ───────────────────────────────────
            // Invalidate all CDs for a SID.
            0x07 => {
                let sid = ((dw0 >> 32) & 0x7FFF) as u32;
                self.tlb.flush_by_sid(sid);
            }

            // ── TLBI_NH_ALL (0x20) ───────────────────────────────────
            // Invalidate all non-hypervisor TLB entries.
            0x20 => {
                self.tlb.flush_all();
            }

            // ── TLBI_NH_ASID (0x21) ──────────────────────────────────
            // Invalidate all entries matching an ASID.
            // DW0[63:48] = ASID.
            0x21 => {
                let asid = ((dw0 >> 48) & 0xFFFF) as u16;
                self.tlb.flush_by_asid(asid);
            }

            // ── TLBI_NH_VA (0x22) ────────────────────────────────────
            // Invalidate by VA + ASID.
            // DW0[63:48] = ASID, DW1[63:12] = VA[63:12].
            0x22 => {
                let asid = ((dw0 >> 48) & 0xFFFF) as u16;
                let va = dw1 & 0xFFFF_FFFF_FFFF_F000;
                self.tlb.flush_by_va_asid(va, asid);
            }

            // ── TLBI_NH_VAA (0x23) ───────────────────────────────────
            // Invalidate by VA, all ASIDs.
            0x23 => {
                let va = dw1 & 0xFFFF_FFFF_FFFF_F000;
                self.tlb.flush_by_va(va);
            }

            // ── TLBI_EL3_ALL (0x24) ──────────────────────────────────
            0x24 => {
                self.tlb.flush_all();
            }

            // ── TLBI_EL3_VA (0x26) ───────────────────────────────────
            0x26 => {
                let va = dw1 & 0xFFFF_FFFF_FFFF_F000;
                self.tlb.flush_by_va(va);
            }

            // ── TLBI_S2_IPA (0x28) ──────────────────────────────────
            // Stage 2 IPA invalidation — flush all (conservative).
            0x28 => {
                self.tlb.flush_all();
            }

            // ── TLBI_NSNH_ALL (0x44) ─────────────────────────────────
            0x44 => {
                self.tlb.flush_all();
            }

            // ── CMD_SYNC (0x46) ──────────────────────────────────────
            // Completion synchronization. In the simulator, all prior
            // commands have already been processed synchronously, so
            // CMD_SYNC is a no-op. CS field (MSI/SEV) is ignored.
            0x46 => {
                // No-op: commands are synchronous in the simulator.
            }

            _ => {
                log::debug!("SMMU: unknown command opcode {:#04x}", opcode);
            }
        }
    }
}
```

### 3.4 Command Processing Invariants

- Commands are processed **synchronously** during the `CMDQ_PROD` write. By the time the MMIO write returns, `CMDQ_CONS == CMDQ_PROD`.
- `CMD_SYNC` is a no-op because synchronous processing guarantees all prior commands have completed.
- Unknown opcodes are logged and skipped. No `CMDQ_ERR` is raised for unknown opcodes (permissive behavior matching QEMU).
- Memory read failure during command fetch sets `GERROR.CMDQ_ERR` and halts queue processing. The consumer index is not advanced past the failing entry.

---

## 4. Translation Walk Pipeline

### 4.1 Top-Level Entry Point

```rust
impl SmmuState {
    /// Translate an IOVA to a physical address for a given stream ID.
    ///
    /// Called by the DMA transaction path when a device issues a memory
    /// access with a stream_id in TransactionAttrs.
    ///
    /// Returns Ok(pa) on success, Err(SmmuFault) on translation failure.
    /// The caller (HelmAddressSpace) converts SmmuFault into a bus error
    /// response to the initiating device.
    pub fn translate(
        &mut self,
        stream_id: u32,
        iova: u64,
        is_write: bool,
    ) -> Result<u64, SmmuFault> {
        // ── Step 1: Check SMMUEN ─────────────────────────────────────
        if self.cr0 & 0x1 == 0 {
            // SMMU disabled — apply Global Bypass/Abort Policy (GBPA).
            return self.apply_gbpa(stream_id, iova, is_write);
        }

        // ── Step 2: TLB lookup ───────────────────────────────────────
        if let Some(entry) = self.tlb.lookup(stream_id, iova) {
            // Permission check on cached entry.
            if is_write && (entry.prot & PROT_WRITE) == 0 {
                return Err(SmmuFault::Permission {
                    stream_id, va: iova, stage: 1,
                });
            }
            let page_offset = iova & (entry.size - 1);
            return Ok(entry.pa + page_offset);
        }

        // ── Step 3: STE lookup ───────────────────────────────────────
        let ste = self.lookup_ste(stream_id)?;

        // ── Step 4: Check STE valid + config ─────────────────────────
        if ste.valid == 0 {
            self.write_event_record(FAULT_C_BAD_STE, stream_id, iova, is_write);
            return Err(SmmuFault::BadSte { stream_id });
        }

        match ste.config {
            SteConfig::Abort => {
                self.write_event_record(FAULT_C_BAD_STE, stream_id, iova, is_write);
                return Err(SmmuFault::Abort { stream_id });
            }
            SteConfig::Bypass => {
                // Passthrough — IOVA == PA.
                return Ok(iova);
            }
            SteConfig::S1Only => {
                // ── Step 5a: Stage 1 only ────────────────────────────
                let cd = self.lookup_cd(&ste, 0)?;
                let pa = self.walk_s1(&cd, iova, is_write, stream_id)?;
                self.tlb.fill(stream_id, cd.asid, iova, pa,
                              PAGE_4K, prot_from_walk(is_write));
                return Ok(pa);
            }
            SteConfig::S2Only => {
                // ── Step 5b: Stage 2 only ────────────────────────────
                let pa = self.walk_s2(&ste, iova, is_write, stream_id)?;
                self.tlb.fill(stream_id, 0, iova, pa,
                              PAGE_4K, prot_from_walk(is_write));
                return Ok(pa);
            }
            SteConfig::S1S2 => {
                // ── Step 5c: Nested (S1 → S2) ────────────────────────
                let cd = self.lookup_cd(&ste, 0)?;
                let ipa = self.walk_s1(&cd, iova, is_write, stream_id)?;
                let pa = self.walk_s2(&ste, ipa, is_write, stream_id)?;
                self.tlb.fill(stream_id, cd.asid, iova, pa,
                              PAGE_4K, prot_from_walk(is_write));
                return Ok(pa);
            }
        }
    }
}
```

### 4.2 GBPA (Global Bypass/Abort Policy)

When SMMUEN=0, the GBPA register controls behavior:

```rust
fn apply_gbpa(&mut self, stream_id: u32, iova: u64, is_write: bool)
    -> Result<u64, SmmuFault>
{
    if self.gbpa & (1 << 20) != 0 {
        // ABORT bit set — all transactions aborted.
        self.write_event_record(FAULT_F_STREAM_DISABLED,
                                stream_id, iova, is_write);
        Err(SmmuFault::Abort { stream_id })
    } else {
        // Bypass — passthrough.
        Ok(iova)
    }
}
```

### 4.3 STE Lookup

```rust
/// Parsed Stream Table Entry fields.
struct ParsedSte {
    valid: u8,
    config: SteConfig,
    s1_context_ptr: u64,  // PA of CD table
    s1_cd_max: u8,        // max SubstreamID bits
    s2_ttb: u64,          // Stage 2 table base
    s2_t0sz: u16,
    s2_tg: u8,            // granule
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SteConfig {
    Abort,    // Config = 0b000
    Bypass,   // Config = 0b100
    S1Only,   // Config = 0b101
    S2Only,   // Config = 0b110
    S1S2,     // Config = 0b111
}

impl SmmuState {
    /// Fetch and parse an STE from guest RAM.
    fn lookup_ste(&self, stream_id: u32) -> Result<ParsedSte, SmmuFault> {
        // Bounds check: SID must fit in the configured table size.
        if stream_id >= (1 << self.strtab_log2size) {
            self.write_event_record(FAULT_C_BAD_STREAMID,
                                    stream_id, 0, false);
            return Err(SmmuFault::BadStreamId { stream_id });
        }

        let ste_pa = match self.strtab_fmt {
            StrtabFmt::Linear => {
                // Linear: STE at base + SID * 64.
                let base = self.strtab_base & 0x000F_FFFF_FFFF_FFC0;
                base + (stream_id as u64) * 64
            }
            StrtabFmt::TwoLevel => {
                // 2-level: L1 index = SID >> split,
                //          L2 index = SID & ((1 << split) - 1).
                let split = self.strtab_split as u32;
                let l1_idx = stream_id >> split;
                let l2_idx = stream_id & ((1 << split) - 1);

                let l1_base = self.strtab_base & 0x000F_FFFF_FFFF_FFC0;
                let l1_desc_pa = l1_base + (l1_idx as u64) * 8;

                let mem = self.mem.as_ref().unwrap().lock().unwrap();
                let l1_desc = mem.guest_read(l1_desc_pa, 8)
                    .ok_or(SmmuFault::SteFetch { stream_id })?;

                if l1_desc & 1 == 0 {
                    // L1 descriptor invalid.
                    return Err(SmmuFault::BadSte { stream_id });
                }

                let l2_base = l1_desc & 0x000F_FFFF_FFFF_FFC0;
                l2_base + (l2_idx as u64) * 64
            }
        };

        // Read 64-byte STE from guest RAM.
        let mem = self.mem.as_ref().unwrap().lock().unwrap();
        let mut ste_bytes = [0u8; 64];
        if !mem.guest_read_bytes(ste_pa, &mut ste_bytes) {
            return Err(SmmuFault::SteFetch { stream_id });
        }

        let dw0 = u64::from_le_bytes(ste_bytes[0..8].try_into().unwrap());
        let dw2 = u64::from_le_bytes(ste_bytes[16..24].try_into().unwrap());
        let dw3 = u64::from_le_bytes(ste_bytes[24..32].try_into().unwrap());

        let valid = (dw0 & 0x1) as u8;
        let config_bits = ((dw0 >> 1) & 0x7) as u8;
        let config = match config_bits {
            0b000 => SteConfig::Abort,
            0b100 => SteConfig::Bypass,
            0b101 => SteConfig::S1Only,
            0b110 => SteConfig::S2Only,
            0b111 => SteConfig::S1S2,
            _     => SteConfig::Abort,  // reserved → abort
        };

        let s1_context_ptr = (dw0 >> 6) & 0x0003_FFFF_FFFF_FFC0;
        let s1_cd_max = ((dw0 >> 59) & 0x1F) as u8;

        let s2_ttb = (dw2 >> 4) & 0x000F_FFFF_FFFF_FFF0;
        let s2_t0sz = (dw2 & 0x3F) as u16;
        let s2_tg = ((dw3 >> 56) & 0x3) as u8;

        Ok(ParsedSte {
            valid, config, s1_context_ptr, s1_cd_max,
            s2_ttb, s2_t0sz, s2_tg,
        })
    }
}
```

### 4.4 CD Lookup

```rust
/// Parsed Context Descriptor fields.
struct ParsedCd {
    valid: bool,
    t0sz: u8,
    tg0: u8,       // granule (0=4KB, 1=64KB, 2=16KB)
    ttb0: u64,      // Stage 1 page table base
    asid: u16,
    aarch64: bool,
    epd0: bool,     // TTB0 disabled
    affd: bool,     // access flag fault disable
    wxn: bool,      // write-execute-never
}

impl SmmuState {
    /// Fetch and parse a Context Descriptor from guest RAM.
    ///
    /// For single-substream devices (SubstreamID=0), the CD is at the
    /// S1ContextPtr address directly. Multi-substream lookup is
    /// deferred to Phase 3.
    fn lookup_cd(&self, ste: &ParsedSte, ssid: u32)
        -> Result<ParsedCd, SmmuFault>
    {
        let cd_pa = if ssid == 0 {
            ste.s1_context_ptr
        } else {
            // Multi-substream: CD table indexed by SSID.
            // cd_pa = s1_context_ptr + ssid * 64
            ste.s1_context_ptr + (ssid as u64) * 64
        };

        let mem = self.mem.as_ref().unwrap().lock().unwrap();
        let mut cd_bytes = [0u8; 64];
        if !mem.guest_read_bytes(cd_pa, &mut cd_bytes) {
            return Err(SmmuFault::CdFetch { stream_id: 0 });
        }

        let dw0 = u64::from_le_bytes(cd_bytes[0..8].try_into().unwrap());
        let dw1 = u64::from_le_bytes(cd_bytes[8..16].try_into().unwrap());

        let valid = (dw0 >> 30) & 1 == 1;
        if !valid {
            return Err(SmmuFault::BadCd);
        }

        let t0sz = ((dw0 >> 0) & 0x3F) as u8;
        let tg0 = ((dw0 >> 6) & 0x3) as u8;
        let epd0 = (dw0 >> 14) & 1 == 1;
        let affd = (dw0 >> 15) & 1 == 1;
        let wxn = (dw0 >> 16) & 1 == 1;
        let aarch64 = (dw0 >> 9) & 1 == 1;

        let asid = ((dw1 >> 48) & 0xFFFF) as u16;
        let ttb0 = dw1 & 0x000F_FFFF_FFFF_F000;  // PA[47:12], page-aligned

        Ok(ParsedCd {
            valid, t0sz, tg0, ttb0, asid, aarch64, epd0, affd, wxn,
        })
    }
}
```

### 4.5 Stage 1 Walk (VA to IPA)

The Stage 1 page table walk uses the same AArch64 descriptor format as the CPU MMU. The initial implementation supports 4KB granule only (matching `IDR5.GRAN4K=1`).

```rust
/// 4KB granule constants.
const PAGE_4K: u64 = 0x1000;
const PAGE_2M: u64 = 0x20_0000;
const PAGE_1G: u64 = 0x4000_0000;

impl SmmuState {
    /// Walk a Stage 1 AArch64 page table (4KB granule).
    ///
    /// Input: CD (containing TTB0, T0SZ, TG0), VA to translate.
    /// Output: PA (IPA for nested, or final PA for S1-only).
    fn walk_s1(
        &mut self,
        cd: &ParsedCd,
        va: u64,
        is_write: bool,
        stream_id: u32,
    ) -> Result<u64, SmmuFault> {
        if cd.epd0 {
            self.write_event_record(FAULT_F_TRANSLATION,
                                    stream_id, va, is_write);
            return Err(SmmuFault::Translation {
                stream_id, va, stage: 1,
            });
        }

        // Determine number of walk levels from T0SZ (4KB granule).
        //   T0SZ  16 → 4-level (L0-L3), input bits = 48
        //   T0SZ  25 → 3-level (L1-L3), input bits = 39
        //   T0SZ  34 → 2-level (L2-L3), input bits = 30
        let input_bits = 64 - cd.t0sz as u32;
        let start_level: i32 = match input_bits {
            37..=48 => 0,   // L0
            28..=36 => 1,   // L1
            22..=27 => 2,   // L2
            _       => {
                self.write_event_record(FAULT_F_TRANSLATION,
                                        stream_id, va, is_write);
                return Err(SmmuFault::Translation {
                    stream_id, va, stage: 1,
                });
            }
        };

        let mem = self.mem.as_ref().unwrap().lock().unwrap();
        let mut table_pa = cd.ttb0;

        for level in start_level..=3 {
            // VA index bits for this level (4KB granule, 9 bits per level):
            //   L0: VA[47:39], L1: VA[38:30], L2: VA[29:21], L3: VA[20:12]
            let shift = (3 - level) * 9 + 12;
            let idx = ((va >> shift) & 0x1FF) as u64;
            let desc_pa = table_pa + idx * 8;

            let desc = mem.guest_read(desc_pa, 8)
                .ok_or_else(|| {
                    SmmuFault::WalkEabt { stream_id, va, stage: 1 }
                })?;

            // Check valid bit.
            if desc & 1 == 0 {
                drop(mem);
                self.write_event_record(FAULT_F_TRANSLATION,
                                        stream_id, va, is_write);
                return Err(SmmuFault::Translation {
                    stream_id, va, stage: 1,
                });
            }

            let is_table = (desc >> 1) & 1 == 1;

            if level < 3 && is_table {
                // Table descriptor → continue to next level.
                table_pa = desc & 0x0000_FFFF_FFFF_F000;
                continue;
            }

            // Block (level 1 or 2) or Page (level 3) descriptor.
            let output_pa = match level {
                1 => desc & 0x0000_FFFF_C000_0000, // 1GB block
                2 => desc & 0x0000_FFFF_FFE0_0000, // 2MB block
                3 => desc & 0x0000_FFFF_FFFF_F000, // 4KB page
                _ => unreachable!(),
            };

            let block_size: u64 = match level {
                1 => PAGE_1G,
                2 => PAGE_2M,
                3 => PAGE_4K,
                _ => unreachable!(),
            };

            // Access flag check.
            let af = (desc >> 10) & 1;
            if af == 0 && !cd.affd {
                drop(mem);
                self.write_event_record(FAULT_F_ACCESS,
                                        stream_id, va, is_write);
                return Err(SmmuFault::Access {
                    stream_id, va, stage: 1,
                });
            }

            // Permission check.
            // AP[2:1] at bits [7:6]:
            //   AP=0b00 → EL1 RW,   EL0 none
            //   AP=0b01 → EL1 RW,   EL0 RW
            //   AP=0b10 → EL1 RO,   EL0 none
            //   AP=0b11 → EL1 RO,   EL0 RO
            let ap = (desc >> 6) & 0x3;
            let read_only = (ap & 0x2) != 0;
            if is_write && read_only {
                drop(mem);
                self.write_event_record(FAULT_F_PERMISSION,
                                        stream_id, va, is_write);
                return Err(SmmuFault::Permission {
                    stream_id, va, stage: 1,
                });
            }

            let page_offset = va & (block_size - 1);
            return Ok(output_pa | page_offset);
        }

        // Should never reach here — level 3 always terminates.
        Err(SmmuFault::Translation { stream_id, va, stage: 1 })
    }
}
```

### 4.6 Stage 2 Walk (IPA to PA)

Stage 2 uses the same walk algorithm with parameters drawn from the STE instead of the CD:

```rust
impl SmmuState {
    /// Walk a Stage 2 page table.
    ///
    /// Uses STE.S2TTB as the table base and STE.S2T0SZ as the input
    /// address size. Same descriptor format as Stage 1.
    fn walk_s2(
        &mut self,
        ste: &ParsedSte,
        ipa: u64,
        is_write: bool,
        stream_id: u32,
    ) -> Result<u64, SmmuFault> {
        // Construct a synthetic CD-like struct from STE S2 fields,
        // then delegate to the same walk logic with stage=2.
        let input_bits = 64 - ste.s2_t0sz as u32;
        let start_level: i32 = match input_bits {
            37..=48 => 0,
            28..=36 => 1,
            22..=27 => 2,
            _       => {
                self.write_event_record(FAULT_F_TRANSLATION,
                                        stream_id, ipa, is_write);
                return Err(SmmuFault::Translation {
                    stream_id, va: ipa, stage: 2,
                });
            }
        };

        let mem = self.mem.as_ref().unwrap().lock().unwrap();
        let mut table_pa = ste.s2_ttb;

        for level in start_level..=3 {
            let shift = (3 - level) * 9 + 12;
            let idx = ((ipa >> shift) & 0x1FF) as u64;
            let desc_pa = table_pa + idx * 8;

            let desc = mem.guest_read(desc_pa, 8)
                .ok_or_else(|| {
                    SmmuFault::WalkEabt { stream_id, va: ipa, stage: 2 }
                })?;

            if desc & 1 == 0 {
                drop(mem);
                self.write_event_record(FAULT_F_TRANSLATION,
                                        stream_id, ipa, is_write);
                return Err(SmmuFault::Translation {
                    stream_id, va: ipa, stage: 2,
                });
            }

            let is_table = (desc >> 1) & 1 == 1;
            if level < 3 && is_table {
                table_pa = desc & 0x0000_FFFF_FFFF_F000;
                continue;
            }

            let output_pa = match level {
                1 => desc & 0x0000_FFFF_C000_0000,
                2 => desc & 0x0000_FFFF_FFE0_0000,
                3 => desc & 0x0000_FFFF_FFFF_F000,
                _ => unreachable!(),
            };

            let block_size: u64 = match level {
                1 => PAGE_1G,
                2 => PAGE_2M,
                3 => PAGE_4K,
                _ => unreachable!(),
            };

            // S2 permission: S2AP[1:0] at bits [7:6].
            //   S2AP=0b00 → none
            //   S2AP=0b01 → read-only
            //   S2AP=0b10 → write-only
            //   S2AP=0b11 → read-write
            let s2ap = (desc >> 6) & 0x3;
            if is_write && (s2ap & 0x2) == 0 {
                drop(mem);
                self.write_event_record(FAULT_F_PERMISSION,
                                        stream_id, ipa, is_write);
                return Err(SmmuFault::Permission {
                    stream_id, va: ipa, stage: 2,
                });
            }

            let page_offset = ipa & (block_size - 1);
            return Ok(output_pa | page_offset);
        }

        Err(SmmuFault::Translation { stream_id, va: ipa, stage: 2 })
    }
}
```

---

## 5. TLB Cache Design

### 5.1 SmmuTlb Structure

```rust
const SMMU_TLB_SIZE: usize = 256;

pub struct SmmuTlb {
    entries: Vec<SmmuTlbEntry>,
}

pub struct SmmuTlbEntry {
    pub valid: bool,
    pub stream_id: u32,
    pub asid: u16,
    pub va: u64,        // page-aligned input address
    pub pa: u64,        // page-aligned output address
    pub size: u64,      // 4KB, 2MB, or 1GB
    pub prot: u32,      // PROT_READ | PROT_WRITE flags
}

const PROT_READ: u32  = 1 << 0;
const PROT_WRITE: u32 = 1 << 1;
```

### 5.2 Hash Function

```rust
impl SmmuTlb {
    /// Direct-mapped index from (stream_id, va).
    ///
    /// XOR the stream_id with the page number to spread entries
    /// from the same device across the cache.
    fn index(stream_id: u32, va: u64) -> usize {
        let page_num = (va >> 12) as u32;
        ((stream_id ^ page_num) as usize) & (SMMU_TLB_SIZE - 1)
    }
}
```

The hash combines stream_id with the page number (VA >> 12) via XOR, then masks to the cache size. This ensures:
- Different devices (different SIDs) addressing the same VA map to different slots.
- Sequential pages from the same device spread across the cache.

### 5.3 Lookup

```rust
impl SmmuTlb {
    /// Look up a cached translation.
    ///
    /// Returns a reference to the TLB entry if (stream_id, va) matches
    /// a valid entry. The caller must still check permissions.
    pub fn lookup(&self, stream_id: u32, va: u64) -> Option<&SmmuTlbEntry> {
        let idx = Self::index(stream_id, va);
        let entry = &self.entries[idx];
        if entry.valid
            && entry.stream_id == stream_id
            && (va & !(entry.size - 1)) == entry.va
        {
            Some(entry)
        } else {
            None
        }
    }
}
```

The lookup compares the page-aligned VA against the stored VA, accounting for block sizes larger than 4KB (2MB and 1GB blocks mask more low bits).

### 5.4 Fill

```rust
impl SmmuTlb {
    /// Insert a new translation into the TLB (direct-mapped, unconditional).
    pub fn fill(
        &mut self,
        stream_id: u32,
        asid: u16,
        va: u64,
        pa: u64,
        size: u64,
        prot: u32,
    ) {
        let aligned_va = va & !(size - 1);
        let aligned_pa = pa & !(size - 1);
        let idx = Self::index(stream_id, aligned_va);
        self.entries[idx] = SmmuTlbEntry {
            valid: true,
            stream_id,
            asid,
            va: aligned_va,
            pa: aligned_pa,
            size,
            prot,
        };
    }
}
```

### 5.5 Invalidation Methods

```rust
impl SmmuTlb {
    pub fn new() -> Self {
        Self {
            entries: (0..SMMU_TLB_SIZE)
                .map(|_| SmmuTlbEntry {
                    valid: false, stream_id: 0, asid: 0,
                    va: 0, pa: 0, size: PAGE_4K, prot: 0,
                })
                .collect(),
        }
    }

    /// Invalidate all entries.
    pub fn flush_all(&mut self) {
        for e in &mut self.entries {
            e.valid = false;
        }
    }

    /// Invalidate all entries matching a given ASID.
    pub fn flush_by_asid(&mut self, asid: u16) {
        for e in &mut self.entries {
            if e.valid && e.asid == asid {
                e.valid = false;
            }
        }
    }

    /// Invalidate the entry matching a specific (VA, ASID) pair.
    pub fn flush_by_va_asid(&mut self, va: u64, asid: u16) {
        for e in &mut self.entries {
            if e.valid && e.asid == asid {
                let page_va = va & !(e.size - 1);
                if e.va == page_va {
                    e.valid = false;
                }
            }
        }
    }

    /// Invalidate all entries matching a given VA (any ASID).
    pub fn flush_by_va(&mut self, va: u64) {
        for e in &mut self.entries {
            if e.valid {
                let page_va = va & !(e.size - 1);
                if e.va == page_va {
                    e.valid = false;
                }
            }
        }
    }

    /// Invalidate all entries for a given stream ID.
    pub fn flush_by_sid(&mut self, stream_id: u32) {
        for e in &mut self.entries {
            if e.valid && e.stream_id == stream_id {
                e.valid = false;
            }
        }
    }
}
```

**Design note on linear scan.** All invalidation methods scan the full 256-entry array. At 256 entries this is trivially fast (one cache line worth of `valid` checks). A set-associative design or hash-bucketed approach would be premature optimization for the simulator's expected workload (tens of devices, not thousands).

---

## 6. Fault Model

### 6.1 SmmuFault Enum

```rust
#[derive(Debug, Clone)]
pub enum SmmuFault {
    /// SID exceeds configured stream table size.
    BadStreamId { stream_id: u32 },
    /// STE fetch from guest memory failed (external abort).
    SteFetch { stream_id: u32 },
    /// STE valid=0 or Config=Abort.
    BadSte { stream_id: u32 },
    /// CD fetch from guest memory failed.
    CdFetch { stream_id: u32 },
    /// CD valid=0.
    BadCd,
    /// Aborted by GBPA or STE Config.
    Abort { stream_id: u32 },
    /// Page table descriptor invalid (no mapping).
    Translation { stream_id: u32, va: u64, stage: u8 },
    /// Access flag fault (AF=0, AFFD=0).
    Access { stream_id: u32, va: u64, stage: u8 },
    /// Permission fault (write to RO page).
    Permission { stream_id: u32, va: u64, stage: u8 },
    /// External abort during page table walk.
    WalkEabt { stream_id: u32, va: u64, stage: u8 },
}
```

### 6.2 Event Record Format

Each event record is 32 bytes (4 doublewords) written to the event queue:

```
DW0  [7:0]   fault_type    — SmmuFaultCode (see section 6.3)
     [9:8]   flags         — Stall=0 (no stall model)
     [63:32] stream_id     — SID of the faulting transaction

DW1  [63:0]  input_addr    — faulting VA or IPA

DW2  [0]     write         — 1 if the faulting access was a write
     [1]     instfetch     — 1 if instruction fetch (always 0 for DMA)
     [2]     s2            — 1 if Stage 2 fault (0 for Stage 1)

DW3  [63:0]  reserved      — 0 (implementation-defined, unused)
```

### 6.3 Fault Type Constants

```rust
const FAULT_C_BAD_STREAMID: u8    = 0x01;
const FAULT_F_STE_FETCH: u8       = 0x02;
const FAULT_C_BAD_STE: u8         = 0x03;
const FAULT_F_STREAM_DISABLED: u8 = 0x05;
const FAULT_C_BAD_SUBSTREAMID: u8 = 0x07;
const FAULT_F_CD_FETCH: u8        = 0x08;
const FAULT_C_BAD_CD: u8          = 0x09;
const FAULT_F_WALK_EABT: u8       = 0x0A;
const FAULT_F_TRANSLATION: u8     = 0x10;
const FAULT_F_ADDR_SIZE: u8       = 0x11;
const FAULT_F_ACCESS: u8          = 0x12;
const FAULT_F_PERMISSION: u8      = 0x13;
```

### 6.4 write_event_record

```rust
impl SmmuState {
    /// Write a 32-byte fault record to the event queue and advance
    /// the producer index.
    ///
    /// If EVTQEN=0 or the queue is full, the record is dropped and
    /// GERROR.EVENTQ_OVF is set.
    fn write_event_record(
        &mut self,
        fault_type: u8,
        stream_id: u32,
        va: u64,
        is_write: bool,
    ) {
        // Check EVTQEN.
        if self.cr0 & 0x4 == 0 {
            return;
        }

        let log2size = (self.evtq_base & 0x1F) as u32;
        let depth = 1u32 << log2size;
        let mask = depth - 1;
        let wrap_mask = (2 * depth) - 1;

        let prod = self.evtq_prod & wrap_mask;
        let cons = self.evtq_cons & wrap_mask;

        // Check if queue is full.
        // Full when (prod + 1) mod (2 * depth) == cons.
        let next_prod = (prod + 1) & wrap_mask;
        if next_prod == cons {
            // Queue overflow — set GERROR sticky bit.
            self.gerror |= 1 << 2;  // EVENTQ_OVF
            self.update_irq_lines();
            return;
        }

        let base_pa = self.evtq_base & 0x000F_FFFF_FFFF_FFE0;
        let idx = prod & mask;
        let record_pa = base_pa + (idx as u64) * 32;

        // Build 32-byte record.
        let dw0: u64 = (fault_type as u64)
                      | ((stream_id as u64) << 32);
        let dw1: u64 = va;
        let dw2: u64 = if is_write { 1 } else { 0 };
        let dw3: u64 = 0;

        let mut record = [0u8; 32];
        record[0..8].copy_from_slice(&dw0.to_le_bytes());
        record[8..16].copy_from_slice(&dw1.to_le_bytes());
        record[16..24].copy_from_slice(&dw2.to_le_bytes());
        record[24..32].copy_from_slice(&dw3.to_le_bytes());

        // Write to guest memory.
        // (Requires a mutable GuestMem or a write path — the GuestMem
        // trait is extended with guest_write_bytes for event recording.)
        if let Some(mem) = &self.mem {
            let mem = mem.lock().unwrap();
            // guest_write_bytes returns false on failure.
            if !mem.guest_write_bytes(record_pa, &record) {
                self.gerror |= 1 << 6;  // EVTQ_ABT_ERR
                self.update_irq_lines();
                return;
            }
        }

        // Advance producer.
        self.evtq_prod = next_prod;

        // Assert event queue IRQ if enabled.
        self.update_irq_lines();
    }
}
```

**GuestMem extension for writes.** The `GuestMem` trait needs a write method for event record and fault recording. This is added as:

```rust
pub trait GuestMem {
    fn guest_read(&self, pa: u64, size: usize) -> Option<u64>;
    fn guest_read_bytes(&self, pa: u64, buf: &mut [u8]) -> bool;
    /// Write bytes to guest physical memory. Returns false on failure.
    fn guest_write_bytes(&self, pa: u64, data: &[u8]) -> bool;
}
```

### 6.5 GERROR Sticky Bits and IRQ Assertion

```rust
impl SmmuState {
    /// Recompute IRQ line assertions based on queue state and IRQ_CTRL.
    ///
    /// Called after:
    ///   - GERRORN write (may clear active GERROR bits)
    ///   - Event record write (EVTQ_PROD advanced)
    ///   - EVTQ_CONS write by software (may deassert EVTQ IRQ)
    ///   - GERROR bit set (CMDQ_ERR, EVENTQ_OVF, etc.)
    fn update_irq_lines(&mut self) {
        // ── GERROR IRQ ───────────────────────────────────────────────
        // Active errors = bits that differ between GERROR and GERRORN.
        let active_gerror = self.gerror ^ self.gerrorn;
        let gerror_irq_en = self.irq_ctrl & (1 << 2) != 0;

        if active_gerror != 0 && gerror_irq_en {
            self.gerror_irq.assert();
        } else {
            self.gerror_irq.deassert();
        }

        // ── Event queue IRQ ──────────────────────────────────────────
        // IRQ asserted when EVTQ_PROD != EVTQ_CONS (unread events)
        // and EVTQ_IRQEN is set.
        let evtq_irq_en = self.irq_ctrl & (1 << 0) != 0;
        let evtq_has_entries = self.evtq_prod != self.evtq_cons;

        if evtq_has_entries && evtq_irq_en {
            self.evtq_irq.assert();
        } else {
            self.evtq_irq.deassert();
        }
    }
}
```

**GERROR toggle semantics.** GERROR uses a toggle model: the hardware sets bits in GERROR, and software acknowledges by writing the same bit pattern to GERRORN. Active (unacknowledged) errors are `GERROR ^ GERRORN`. This is different from a simple "write 1 to clear" model.

---

## 7. Module Structure

### 7.1 File Layout

```
hw/helm-hw-smmu/
├── Cargo.toml
└── src/
    ├── lib.rs          — crate root, re-exports SmmuState, SmmuTlb, SmmuFault
    ├── smmu.rs         — SmmuState struct, Device trait impl, GuestMem trait
    ├── tlb.rs          — SmmuTlb, SmmuTlbEntry, lookup/fill/flush methods
    ├── translate.rs    — lookup_ste, lookup_cd, walk_s1, walk_s2, translate()
    ├── cmdq.rs         — process_cmdq(), process_command(), command opcodes
    └── fault.rs        — SmmuFault enum, fault type constants, write_event_record,
                          update_irq_lines
```

### 7.2 Module Dependency Graph

```
lib.rs
  ├── mod smmu        (SmmuState, Device impl, StrtabFmt, GuestMem trait)
  │     ├── uses tlb       (SmmuTlb)
  │     ├── uses translate (walk functions)
  │     ├── uses cmdq      (command processing)
  │     └── uses fault     (event recording, IRQ lines)
  ├── mod tlb         (SmmuTlb, SmmuTlbEntry — standalone, no deps on smmu)
  ├── mod translate   (lookup_ste, lookup_cd, walk_s1, walk_s2 — methods on SmmuState)
  ├── mod cmdq        (process_cmdq, process_command — methods on SmmuState)
  └── mod fault       (SmmuFault, write_event_record, update_irq_lines — methods on SmmuState)
```

### 7.3 lib.rs Re-exports

```rust
mod smmu;
mod tlb;
mod translate;
mod cmdq;
mod fault;

pub use smmu::{SmmuState, StrtabFmt, GuestMem};
pub use tlb::{SmmuTlb, SmmuTlbEntry};
pub use fault::SmmuFault;
```

### 7.4 Cargo.toml Dependencies

```toml
[package]
name = "helm-hw-smmu"
version = "0.1.0"
edition = "2021"

[dependencies]
helm-devices = { path = "../../framework/helm-devices" }
helm-memory  = { path = "../../framework/helm-memory" }
log          = "0.4"
```

`helm-devices` provides `Device`, `InterruptPin`, and `SimObject`. `helm-memory` provides `HelmAddressSpace` for the `GuestMem` impl.

### 7.5 Method Placement

The `SmmuState` struct is defined in `smmu.rs`. The methods on it are split across files using `impl SmmuState` blocks in each module file:

- `smmu.rs`: `new()`, `Device::read()`, `Device::write()`
- `translate.rs`: `translate()`, `apply_gbpa()`, `lookup_ste()`, `lookup_cd()`, `walk_s1()`, `walk_s2()`
- `cmdq.rs`: `process_cmdq()`, `process_command()`
- `fault.rs`: `write_event_record()`, `update_irq_lines()`

Each file imports `use super::smmu::SmmuState;` (or `use crate::smmu::SmmuState;` depending on module nesting) and adds `impl SmmuState { ... }` blocks. This keeps each file focused on one concern while maintaining a single `SmmuState` type.

---

## 8. Work Items

### Phase 2 — Core Implementation

- [ ] WI-LLD-001: Define `GuestMem` trait with `guest_read`, `guest_read_bytes`, `guest_write_bytes`
- [ ] WI-LLD-002: Define `SmmuTlb`, `SmmuTlbEntry` in `tlb.rs` with all lookup/fill/flush methods
- [ ] WI-LLD-003: Define `SmmuState` struct in `smmu.rs` with all register fields
- [ ] WI-LLD-004: Implement `SmmuState::new(oas_bits)` with IDR precomputation
- [ ] WI-LLD-005: Implement `Device::read()` — all register offsets per section 2.2
- [ ] WI-LLD-006: Implement `Device::write()` — all register offsets per section 2.3, including CR0→CR0ACK, STRTAB_BASE_CFG parse, CMDQ_PROD drain trigger
- [ ] WI-LLD-007: Implement `process_cmdq()` — ring buffer drain loop per section 3.2
- [ ] WI-LLD-008: Implement `process_command()` — dispatch for CFGI_STE_RANGE, CFGI_ALL, CFGI_CD, CFGI_CD_ALL, TLBI_NH_ALL, TLBI_NH_ASID, TLBI_NH_VA, TLBI_NH_VAA, CMD_SYNC
- [ ] WI-LLD-009: Implement `lookup_ste()` — linear and 2-level stream table fetch per section 4.3
- [ ] WI-LLD-010: Implement `lookup_cd()` — CD fetch from guest RAM per section 4.4
- [ ] WI-LLD-011: Implement `walk_s1()` — 4KB granule, 2–4 level AArch64 page table walk per section 4.5
- [ ] WI-LLD-012: Implement `walk_s2()` — Stage 2 walk using STE S2 fields per section 4.6
- [ ] WI-LLD-013: Implement `translate()` — full pipeline per section 4.1
- [ ] WI-LLD-014: Implement `SmmuFault` enum and fault type constants per section 6.1
- [ ] WI-LLD-015: Implement `write_event_record()` — 32-byte record to EVTQ per section 6.4
- [ ] WI-LLD-016: Implement `update_irq_lines()` — GERROR toggle + EVTQ IRQ per section 6.5
- [ ] WI-LLD-017: Implement `GuestMem` for `HelmAddressSpace` in `helm-memory`
- [ ] WI-LLD-018: Create `hw/helm-hw-smmu/` crate with `Cargo.toml` per section 7.4

### Phase 2 — Testing

- [ ] WI-LLD-019: Unit test: SmmuTlb lookup/fill/flush_all/flush_by_asid/flush_by_va_asid/flush_by_sid
- [ ] WI-LLD-020: Unit test: Device read/write for all defined register offsets (round-trip)
- [ ] WI-LLD-021: Unit test: CR0→CR0ACK and IRQ_CTRL→IRQ_CTRLACK mirroring
- [ ] WI-LLD-022: Unit test: STRTAB_BASE_CFG parse (linear + 2-level, various LOG2SIZE/SPLIT values)
- [ ] WI-LLD-023: Unit test: process_cmdq with mock GuestMem — CFGI_STE_RANGE flushes correct SID range, CMD_SYNC is no-op
- [ ] WI-LLD-024: Unit test: lookup_ste with linear table (valid STE, invalid STE, out-of-range SID)
- [ ] WI-LLD-025: Unit test: lookup_ste with 2-level table (valid L1→L2, invalid L1)
- [ ] WI-LLD-026: Unit test: walk_s1 with 3-level 4KB walk (L1→L2→L3), permission fault on RO write, AF fault
- [ ] WI-LLD-027: Unit test: translate() end-to-end — SMMUEN=0 bypass, SMMUEN=0 GBPA abort, SMMUEN=1 S1-only, bypass STE
- [ ] WI-LLD-028: Unit test: write_event_record — verify 32-byte record in mock memory, EVTQ_PROD advance, overflow → GERROR
- [ ] WI-LLD-029: Unit test: update_irq_lines — GERROR toggle model, EVTQ IRQ assert/deassert

### Phase 3 — Extensions

- [ ] WI-LLD-030: 16KB and 64KB granule support in walk_s1/walk_s2
- [ ] WI-LLD-031: MSI-based interrupt delivery (EVTQ_IRQ_CFG → ITS translate_msi)
- [ ] WI-LLD-032: PRI queue implementation for PCIe ATS
- [ ] WI-LLD-033: Stage 2 protected table walk (S2PTW=1): S1 walk descriptors go through S2 translation
- [ ] WI-LLD-034: Integration with `HelmAddressSpace` DMA path (`SmmuTranslate` front-end)
