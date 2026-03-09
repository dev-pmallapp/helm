# Internals

Deep dives into how each subsystem works.  These documents are aimed
at developers working *inside* the relevant crate.

## Contents

### Instruction Pipeline

| Document | Description |
|----------|-------------|
| [decode-tree.md](decode-tree.md) | `.decode` file format, code generation, overlap groups, validation |
| [tcg-ir.md](tcg-ir.md) | TcgOp IR design, temp allocation, block structure |
| [a64-emitter.md](a64-emitter.md) | AArch64 → TCG translation — handler patterns, flag computation, gotchas |
| [jit-compiler.md](jit-compiler.md) | Cranelift JIT — compilation, helper functions, block cache, exit codes |
| [interpreter.md](interpreter.md) | Match-based interpreter, threaded dispatch, parity with JIT |

### CPU & System

| Document | Description |
|----------|-------------|
| [aarch64-cpu.md](aarch64-cpu.md) | Aarch64Cpu struct, register file, sysreg dispatch, step/step_fast |
| [mmu-and-tlb.md](mmu-and-tlb.md) | Page table walks, TLB management, TLBI handling, stage-1/stage-2 |
| [exception-delivery.md](exception-delivery.md) | take_exception, check_irq, ERET, SPSR packing, vector offsets |
| [timer-subsystem.md](timer-subsystem.md) | CNTV/CNTP timers, check_timers, GIC integration, sysreg sync pitfalls |
| [sysreg-sync.md](sysreg-sync.md) | Dual-state problem: CPU regs vs interp sysreg array, sync points |

### Memory

| Document | Description |
|----------|-------------|
| [address-space.md](address-space.md) | AddressSpace, RAM regions, IO dispatch, read_phys/write |
| [cache-model.md](cache-model.md) | L1/L2/L3 cache simulation, associativity, replacement, coherence |
| [dma-engine.md](dma-engine.md) | Scatter-gather DMA, bus-beat fragmentation |

### Devices

| Document | Description |
|----------|-------------|
| [device-trait.md](device-trait.md) | Device trait lifecycle, read/write/tick/reset, DeviceEvent |
| [bus-hierarchy.md](bus-hierarchy.md) | DeviceBus, APB/AHB/PCI bridges, routing, latency |
| [gic.md](gic.md) | GICv2/v3 distributor, redistributor, ICC sysregs, ITS, LPI |
| [pl011.md](pl011.md) | PL011 UART — register model, FIFO, CharBackend |
| [virtio.md](virtio.md) | VirtIO MMIO transport, virtqueue, blk/net/console/rng devices |
| [bcm2837.md](bcm2837.md) | RPi3 peripherals — system timer, mailbox, mini UART, GPIO |
| [dtb-generation.md](dtb-generation.md) | FDT generation, platform → DTB, CLI overlays |

### Engine

| Document | Description |
|----------|-------------|
| [fs-session.md](fs-session.md) | FsSession run loop, JIT dispatch, interp fallback, IRQ delivery |
| [se-session.md](se-session.md) | SeSession, syscall emulation, ELF loading, brk/mmap |
| [block-translation.md](block-translation.md) | translate_block_fs, block cache, JIT cache, Unhandled fallback |
| [state-sync.md](state-sync.md) | regs_to_array, array_to_regs, sync_mmu_to_cpu, sync_sysregs_* |

### Timing

| Document | Description |
|----------|-------------|
| [fe-model.md](fe-model.md) | Functional-equivalent: IPC=1, no pipeline |
| [ape-model.md](ape-model.md) | Approximate: per-class instruction latency, branch penalty |
| [cae-model.md](cae-model.md) | Cycle-accurate: ROB, IQ, LSQ, pipeline stages |
| [timing-integration.md](timing-integration.md) | How timing models plug into the execution loop |
