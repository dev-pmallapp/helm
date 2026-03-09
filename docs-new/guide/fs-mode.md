# Full-System Mode

Boot real OS kernels on emulated ARM platforms.

## What FS Mode Simulates

- Full AArch64 CPU with EL0–EL3 and MMU.
- Device tree generation and patching.
- GIC (v2/v3) interrupt controller.
- PL011 UART for serial console.
- VirtIO MMIO devices (block, net, console, RNG).
- Platform-specific peripherals (BCM2837 for RPi3).
- Timer subsystem (CNTV/CNTP).
- TCG binary translation with JIT compilation.

## Usage

### Command Line

```bash
# Boot on virt machine
helm-system-aarch64 -M virt --kernel Image -m 256M --serial stdio

# Boot on RPi3 platform
helm-system-aarch64 -M rpi3 --kernel Image -m 256M

# With custom DTB
helm-system-aarch64 -M virt --kernel Image --dtb custom.dtb

# With kernel command line
helm-system-aarch64 -M virt --kernel Image \
    --append "earlycon=pl011,0x09000000 console=ttyAMA0"

# With block device
helm-system-aarch64 -M virt --kernel Image \
    --drive file=rootfs.img,format=raw

# With Python config
helm-system-aarch64 examples/fs/virt.py
```

### Python API

```python
from helm.session import FsSession

s = FsSession("Image", machine="virt",
              append="earlycon=pl011,0x09000000 console=ttyAMA0",
              memory_size="256M")
s.run(100_000_000)
print(f"PC={s.pc:#x}, EL={s.current_el}, insns={s.insn_count}")
```

## Machine Types

| Name | Description | GIC | UARTs |
|------|-------------|-----|-------|
| `virt` | QEMU-compatible virtual platform | GICv2 (default) or GICv3 | 2× PL011 |
| `realview-pb` | ARM RealView Platform Baseboard | GICv2 | 4× PL011 |
| `rpi3` | Raspberry Pi 3 (BCM2837) | — | PL011 + Mini UART |

## Kernel Image Format

HELM loads standard ARM64 Linux `Image` files:

1. Validates the magic number (`0x644d5241` = "ARMd") at offset 0x38.
2. Reads `text_offset` and `image_size` from the header.
3. Supports gzip-compressed images (auto-detected).
4. Loads at `RAM_BASE + text_offset` (2 MB aligned).
5. Sets x0 = DTB physical address, x1–x3 = 0.

## Boot Flow

1. Platform is constructed with buses, devices, and IRQ routes.
2. DTB is generated or patched based on CLI arguments.
3. Kernel image is loaded into RAM.
4. DTB is placed at a 2 MB-aligned address after the kernel.
5. CPU starts at EL1 with MMU off, PC = kernel entry.
6. Execution proceeds via TCG (JIT or interpreter).
