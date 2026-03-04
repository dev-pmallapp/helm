# HELM Documentation

## Project Documentation

| Document | Description |
|----------|-------------|
| [Architecture](architecture.md) | Crate layout, data flow, and design decisions |
| [Accuracy Levels](accuracy-levels.md) | FE / APE / CAE tier definitions |
| [ARM Implementation Guide](arm-implementation-guide.md) | ARMv7, v8, v9 bring-up plan and SE-mode syscall map |
| [ARM SE Implementation](arm-se-implementation.md) | Detailed spec for running fish-shell in SE mode |
| [fish Instruction Analysis](fish-instruction-analysis.md) | Binary analysis of the fish-shell AArch64 static build |
| [Device Authoring](device-authoring.md) | How to build custom MMIO devices |
| [SystemC Integration](systemc-integration.md) | TLM-2.0 bridge architecture |

## Research Notes

Background research that informed the design lives in `research/`:

- [QOM/QMP Adaptation](qom-qmp-adaptation-for-helm.md) - Runtime introspection and control protocol
- [Cycle-Accurate Simulation](cycle-accurate-simulation.md) - Multi-level timing model architecture
- [Dynamic Modules](dynamic-modules-and-executables.md) - Plugin system and executable generation
- [Simulator Comparison](simulator-comparison.md) - QEMU vs Simics vs gem5 vs HELM categorization
- [SystemC Integration](systemc-integration.md) - TLM-2.0 co-simulation and RTL integration
