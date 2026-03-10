# Architecture

System design, crate relationships, and high-level data flows.
These documents explain *why* HELM is structured the way it is,
drawing comparisons to QEMU and gem5 where relevant.

## Contents

| Document | Description |
|----------|-------------|
| [overview.md](overview.md) | Bird's-eye view — what HELM is, design goals, positioning vs QEMU/gem5 |
| [crate-map.md](crate-map.md) | All 18 crates, dependency graph, ownership boundaries |
| [execution-pipeline.md](execution-pipeline.md) | Instruction flow: fetch → decode → translate → execute (interp / JIT) |
| [memory-model.md](memory-model.md) | Address spaces, MMU, TLB, caches, coherence — how guest memory works |
| [timing-model.md](timing-model.md) | FE / ITE / CAE accuracy tiers, timing integration points |
| [exception-model.md](exception-model.md) | Exception levels, IRQ delivery, ERET, VBAR routing |
| [device-model.md](device-model.md) | Device trait, bus hierarchy, MMIO dispatch, IRQ routing |
| [platform-and-soc.md](platform-and-soc.md) | Platform struct, machine types, DTB generation, Python ↔ Rust boundary |
| [plugin-architecture.md](plugin-architecture.md) | Plugin registry, component model, hot-loading, callback hooks |
| [python-rust-boundary.md](python-rust-boundary.md) | PyO3 bindings, sysreg array vs CPU struct, state synchronisation |
| [comparison-qemu.md](comparison-qemu.md) | Architectural comparison with QEMU — TCG, softmmu, device model, QOM |
| [comparison-gem5.md](comparison-gem5.md) | Architectural comparison with gem5 — SimObjects, ports, timing, configs |
