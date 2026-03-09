# GIC

Generic Interrupt Controller device models.

## Versions

| Model | Module | Description |
|-------|--------|-------------|
| GICv2 | `arm::gic::v2::Gic` | Distributor + CPU interface (MMIO) |
| GICv3 | `arm::gic::v3::GicV3` | Distributor + redistributors + ICC sysregs |
| GICv4 | `arm::gic::v4::GicV4` | GICv3 + vLPI/vSGI (virtualisation) |

## GICv2

MMIO register interface:

| Offset | Register | Description |
|--------|----------|-------------|
| GICD+0x000 | GICD_CTLR | Distributor control |
| GICD+0x004 | GICD_TYPER | Type (number of IRQ lines) |
| GICD+0x100 | GICD_ISENABLER | IRQ set-enable |
| GICD+0x200 | GICD_ISPENDR | IRQ set-pending |
| GICD+0x400 | GICD_IPRIORITYR | IRQ priority |
| GICC+0x000 | GICC_CTLR | CPU interface control |
| GICC+0x004 | GICC_PMR | Priority mask |
| GICC+0x00C | GICC_IAR | Interrupt acknowledge |
| GICC+0x010 | GICC_EOIR | End of interrupt |

Implements `InterruptController` trait for programmatic IRQ injection.
Connects to the CPU via `IrqSignal`.

## GICv3

MMIO layout (QEMU-virt compatible):

| Offset | Size | Component |
|--------|------|-----------|
| 0x0_0000 | 64 KB | Distributor (GICD) |
| 0x8_0000 | 128 KB | ITS (optional) |
| 0xA_0000 | N × 128 KB | Redistributors (GICR) |

CPU interface via ICC system registers (MRS/MSR):
- ICC_IAR1_EL1 — interrupt acknowledge.
- ICC_EOIR1_EL1 — end of interrupt.
- ICC_PMR_EL1 — priority mask.
- ICC_SGI1R_EL1 — SGI generation.

## Supporting Modules

- `common` — shared bitmap and priority helpers.
- `distributor` — GICD register state (shared by v2 and v3).
- `redistributor` — per-PE GICR state (GICv3+).
- `icc` — ICC system register definitions and per-PE state.
- `lpi` — LPI configuration and pending table helpers.
- `its` — ITS command queue and translation tables.
