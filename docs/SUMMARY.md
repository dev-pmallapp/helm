# Summary

[Introduction](README.md)

# User Guide

- [Installation](guide/installation.md)
- [Quick Start](guide/quickstart.md)
- [SE Mode](guide/se-mode.md)
- [FS Mode](guide/fs-mode.md)
- [HAE Mode (KVM)](guide/hae-mode.md)
- [CLI Reference](guide/cli-reference.md)
- [Python Configuration](guide/python-config.md)
- [Examples](guide/examples.md)
- [FAQ](guide/faq.md)

# Architecture

- [Overview](architecture/overview.md)
- [Crate Map](architecture/crate-map.md)
- [Execution Pipeline](architecture/execution-pipeline.md)
- [Memory Model](architecture/memory-model.md)
- [Timing Model](architecture/timing-model.md)
- [Exception Model](architecture/exception-model.md)
- [Device Model](architecture/device-model.md)
- [Platform & SoC](architecture/platform-and-soc.md)
- [Plugin Architecture](architecture/plugin-architecture.md)
- [Python-Rust Boundary](architecture/python-rust-boundary.md)
- [Comparison: QEMU](architecture/comparison-qemu.md)
- [Comparison: gem5](architecture/comparison-gem5.md)

# Internals

- [Decode Tree](internals/decode-tree.md)
- [A64 Emitter](internals/a64-emitter.md)
- [TCG IR](internals/tcg-ir.md)
- [JIT Compiler](internals/jit-compiler.md)
- [Interpreter](internals/interpreter.md)
- [AArch64 CPU](internals/aarch64-cpu.md)
- [MMU & TLB](internals/mmu-and-tlb.md)
- [Exception Delivery](internals/exception-delivery.md)
- [Timer Subsystem](internals/timer-subsystem.md)
- [Sysreg Sync](internals/sysreg-sync.md)
- [Address Space](internals/address-space.md)
- [Block Translation](internals/block-translation.md)
- [State Sync](internals/state-sync.md)
- [FS Session](internals/fs-session.md)
- [SE Session](internals/se-session.md)
- [Cache Model](internals/cache-model.md)
- [DMA Engine](internals/dma-engine.md)
- [Device Trait](internals/device-trait.md)
- [Bus Hierarchy](internals/bus-hierarchy.md)
- [GIC](internals/gic.md)
- [PL011 UART](internals/pl011.md)
- [VirtIO](internals/virtio.md)
- [BCM2837](internals/bcm2837.md)
- [DTB Generation](internals/dtb-generation.md)
- [FE Model](internals/fe-model.md)
- [ITE Model](internals/ite-model.md)
- [CAE Model](internals/cae-model.md)
- [Timing Integration](internals/timing-integration.md)

# Reference

- [Sysreg Map](reference/sysreg-map.md)
- [Machine Types](reference/machine-types.md)
- [Instruction Coverage](reference/instruction-coverage.md)
- [Python API](reference/python-api.md)
- [FsOpts](reference/fsopts.md)
- [Plugin Catalog](reference/plugin-catalog.md)
- [Decode Files](reference/decode-files.md)
- [Memory Map: RPi3](reference/memory-map-rpi3.md)
- [Memory Map: Virt](reference/memory-map-virt.md)
- [Memory Map: RealView](reference/memory-map-realview.md)
- [Error Codes](reference/error-codes.md)
- [Glossary](reference/glossary.md)

# Development

- [Contributing](development/contributing.md)
- [Coding Style](development/coding-style.md)
- [Debugging](development/debugging.md)
- [Testing](development/testing.md)
- [Adding Instructions](development/adding-instructions.md)
- [Adding Platforms](development/adding-platforms.md)
- [Adding Devices](development/adding-devices.md)
- [Adding ISA Support](development/adding-isa.md)
- [CI & Release](development/ci-and-release.md)
- [Performance](development/performance.md)
- [Known Issues](development/known-issues.md)

# Research

- [Speed Analysis](research/helm-speed-issues.md)
- [Speed Roadmap](research/helm-speed-next.md)
- [JIT Inflate Bug](research/jit-inflate-bug.md)
