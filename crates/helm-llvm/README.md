# helm-llvm

LLVM IR-based hardware accelerator simulation for HELM, inspired by gem5-SALAM.

## Overview

This crate enables cycle-accurate simulation of custom hardware accelerators using LLVM IR as the behavioral specification. Instead of writing RTL or custom ISA decoders, developers can:

1. Write accelerator logic in C/C++ or other high-level languages
2. Compile to LLVM IR with optimizations
3. Simulate cycle-accurate execution with configurable hardware resources

## Key Features

- **LLVM IR Frontend**: Parse and execute LLVM IR instructions
- **Dynamic Scheduling**: Data-dependency-driven instruction scheduling
- **Configurable Resources**: Fine-grained functional unit configuration
- **Lockstep/OOO Modes**: Support both lockstep and out-of-order execution
- **MicroOp Abstraction**: Unified execution layer shared with CPU simulation

## Architecture

```
Accelerator C/C++ → Compiler → LLVM IR → MicroOps → Unified Pipeline
                                    ↑
                          Same pipeline as CPU simulation!
```

## Usage Example

```rust
use helm_llvm::Accelerator;

// Create accelerator from LLVM IR file
let mut accel = Accelerator::from_file("matmul.ll")
    .with_int_adders(4)           // 4 integer adders
    .with_fp_multipliers(8)       // 8 FP multipliers
    .with_scratchpad_size(65536)  // 64KB scratchpad
    .with_lockstep_mode(false)    // Out-of-order execution
    .build()?;

// Run simulation
accel.run()?;

// Get statistics
let stats = accel.stats();
println!("Total cycles: {}", stats.total_cycles);
```

## Gem5-SALAM Compatibility

Configuration follows gem5-SALAM conventions:

```rust
let accel = Accelerator::from_file("kernel.ll")
    .with_int_adders(-1)          // -1 = unlimited resources
    .with_fp_sp_multipliers(4)    // 4 single-precision FP multipliers
    .with_fp_dp_multipliers(2)    // 2 double-precision FP multipliers
    .with_load_store_units(2)     // 2 load/store units
    .with_lockstep_mode(true)     // Lockstep execution
    .with_scheduling_threshold(10000)
    .build()?;
```

## Components

### LLVM IR Types (`ir.rs`)
- `LLVMModule`: Module container
- `LLVMFunction`: Function representation
- `LLVMBasicBlock`: Basic block with instructions
- `LLVMInstruction`: Simplified LLVM instruction set
- `LLVMValue`: SSA values and constants
- `LLVMType`: Type system

### MicroOps (`micro_op.rs`)
- Unified execution representation
- Shared with TCG-based CPU simulation
- Maps LLVM IR operations to hardware operations

### Functional Units (`functional_units.rs`)
- Per-operation-type resource modeling
- Configurable counts, latencies, pipelining
- gem5-SALAM compatible configuration

### Scheduler (`scheduler.rs`)
- Dynamic instruction scheduling
- Dependency tracking
- Reservation table and in-flight queues
- Lockstep vs out-of-order modes

### Accelerator (`accelerator.rs`)
- Top-level device model
- Builder pattern for configuration
- Simulation execution and statistics

## Relationship to Other HELM Components

### Complements ISA Frontends
- ISA frontends (ARM, x86, RISC-V): For CPU simulation
- LLVM frontend: For accelerator simulation
- Both use same microarchitecture backend

### Uses TCG Concepts
- MicroOps inspired by TCG operations
- No direct TCG ↔ LLVM IR conversion needed
- Both map to unified MicroOp representation

### Integration Points
- `helm-core`: Core types and IR
- `helm-pipeline`: Shared OOO pipeline (future)
- `helm-memory`: Shared memory hierarchy
- `helm-device`: For device modeling

## Future Enhancements

- [ ] Complete LLVM IR parsing (currently uses inkwell/llvm-sys stubs)
- [ ] Binary lifting support (Remill integration)
- [ ] Scratchpad memory implementation
- [ ] DMA controller support
- [ ] Stream buffer support
- [ ] Power modeling integration
- [ ] Python bindings for accelerator configuration
- [ ] Integration with helm-pipeline for unified execution

## Design Decisions

### Why MicroOps instead of direct TCG?

MicroOps provide a clean abstraction layer:
- TCG: Low-level, binary translation focused
- LLVM IR: High-level, compiler optimizations
- MicroOps: Unified execution representation

Both TCG and LLVM IR map to MicroOps, which are then executed by the unified pipeline.

### Why not lift CPU binaries to LLVM IR?

We support both approaches:
- **ISA decoders**: Fast, accurate for CPU simulation
- **LLVM IR**: Flexible, for accelerator simulation
- **Optional binary lifting**: For research and comparison

The choice depends on use case.

## References

- [gem5-SALAM](https://github.com/TeCSAR-UNCC/gem5-SALAM) - Original LLVM IR-based accelerator simulation
- [QEMU TCG](https://www.qemu.org/docs/master/devel/tcg.html) - Binary translation inspiration
- [LLVM](https://llvm.org/) - Compiler infrastructure
