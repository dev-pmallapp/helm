# HAE Mode (Hardware-Assisted Emulation)

HAE mode delegates guest CPU execution to the host KVM hypervisor,
achieving near-native speed while HELM retains full control of the
device model, memory map, and interrupt delivery.

## When to Use HAE

| Use case | Why HAE | Timing detail |
|----------|---------|---------------|
| Fast OS boot for FS-mode checkpointing | Native speed — seconds not hours | CPU: none; devices: FE |
| Mixed-fidelity accelerator research | CPU not the bottleneck; accelerator is | CPU: HAE; accel: CAE |
| Driver development with real peripherals | MMIO exits give full device control | CPU: HAE; devices: CAE |
| Platform bring-up iteration | Boot kernel, probe devices, check dmesg | CPU: HAE; devices: FE |

## Requirements

- **Linux host** with `/dev/kvm` available
- **AArch64 host** (KVM requires host ISA = guest ISA)
- The `kvm` feature must be enabled at build time

## Architecture

HAE uses the `helm-kvm` crate which wraps the Linux KVM ioctl
interface:

```text
┌──────────────────────────────────────────────────────┐
│                    FsSession                          │
│                                                      │
│  ┌──────────┐   ┌─────────────┐   ┌──────────────┐  │
│  │ KvmVcpu  │   │ AddressSpace│   │  Platform     │  │
│  │ /dev/kvm │◄─►│ (guest RAM) │   │  DeviceBus    │  │
│  │ ioctl()  │   │ + IoHandler │◄─►│  IrqRouter    │  │
│  └────┬─────┘   └─────────────┘   │  GIC / PL011  │  │
│       │                           └──────────────┘  │
│       │ VM_EXIT_MMIO / IRQ / SHUTDOWN                │
│       ▼                                              │
│  exit_dispatch() → DeviceBus.read_fast/write_fast    │
└──────────────────────────────────────────────────────┘
```

### How It Works

1. Guest RAM is `mmap`'d and registered with KVM
2. KVM runs the guest at native speed
3. When the guest accesses an unmapped MMIO address, KVM exits
4. HELM dispatches the MMIO access to the [device bus](../internals/bus-hierarchy.md)
5. Devices assert IRQs via `KVM_IRQ_LINE`
6. KVM re-enters and the guest handles the interrupt

## Relationship to Other Modes

HAE is an **execution mode**, orthogonal to the
[timing model](../architecture/timing-model.md):

| Axis | Options |
|------|---------|
| **Execution mode** | SE, FS, **HAE** |
| **Timing fidelity** | FE, [ITE](../internals/ite-model.md), [CAE](../internals/cae-model.md) |

HAE always runs the CPU at FE-equivalent timing (IPC = native hardware).
However, devices attached to the platform can run at any timing
fidelity — this enables mixed-fidelity simulation where the CPU is
fast but attached accelerators model cycle-accurate behaviour.

## Usage

### CLI

```bash
helm-system-aarch64 --kernel vmlinux --backend kvm
```

### Python

```python
from helm import FsSession, FsOpts

opts = FsOpts(kernel="vmlinux", machine="virt", backend="kvm")
session = FsSession("vmlinux", "virt", opts)
session.run(1_000_000_000)
```

## See Also

- [FS Mode](fs-mode.md) — full-system emulation without KVM
- [Timing Model](../architecture/timing-model.md) — FE / ITE / CAE levels
- [Device Model](../architecture/device-model.md) — MMIO device interaction
