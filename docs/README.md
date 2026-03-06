# HELM Documentation

## Core Docs

| Document | Description |
|----------|-------------|
| [Architecture](architecture.md) | Crate layout, dependency graph, data flows |
| [Execution Modes](execution-modes.md) | SE vs FS mode, timing submodes (FE/APE/CAE), OS targets, accelerators, SystemC co-sim |
| [Accuracy Levels](accuracy-levels.md) | FE / APE / CAE tier definitions and comparisons |
| [ARM](arm.md) | AArch64 implementation status, AArch32/ARMv9 roadmap |
| [Decode Tree](decode-tree.md) | QEMU `.decode` file format, dual TCG/static backends |
| [Device Authoring](device-authoring.md) | Building custom MMIO devices |
| [Plugin System](plugin-system.md) | Plugin API, built-in plugins, dynamic loading |
| [Multi-Threaded Execution](multi-threaded-execution.md) | Thread model, temporal decoupling, quantum sync |
| [SystemC Integration](systemc-integration.md) | TLM-2.0 bridge, co-simulation, clock domain crossing |

## Proposals & Plans

| Document | Description |
|----------|-------------|
| [Proposals](proposals.md) | Architectural problems, performance, release/usability |
| [Decode Test Plan](decode-test-plan.md) | Plan for importing QEMU ARM decode tests into helm-decode and helm-isa |

## Research Notes

Background that informed the design — not normative:

| Document | Description |
|----------|-------------|
| [Simulator Comparison](research/simulator-comparison.md) | QEMU vs Simics vs gem5 vs HELM |
| [Cycle-Accurate Simulation](research/cycle-accurate-simulation.md) | Multi-level timing model rationale |
| [QOM/QMP Adaptation](research/qom-qmp-adaptation-for-helm.md) | Runtime introspection and control |
| [Dynamic Modules](research/dynamic-modules-and-executables.md) | Plugin system and executable generation |
| [fish Instruction Analysis](research/fish-instruction-analysis.md) | AArch64 binary analysis that validated the SE implementation |
