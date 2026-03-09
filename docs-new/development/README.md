# Development

Contributing, coding standards, testing, and release process.

## Contents

| Document | Description |
|----------|-------------|
| [contributing.md](contributing.md) | How to contribute — fork, branch, PR, review process |
| [coding-style.md](coding-style.md) | Rust and Python conventions — naming, modules, error handling |
| [testing.md](testing.md) | TDD workflow, test organisation, `make pre-commit`, parity tests |
| [debugging.md](debugging.md) | Debugging kernel boot, JIT vs interp divergence, tracing tips |
| [adding-instructions.md](adding-instructions.md) | How to add a new AArch64 instruction — decode → emitter → exec → test |
| [adding-devices.md](adding-devices.md) | How to add a new device model — trait impl, bus wiring, DTB node |
| [adding-platforms.md](adding-platforms.md) | How to add a new machine type — Rust platform fn + Python + example |
| [adding-isa.md](adding-isa.md) | How to add a new ISA (RISC-V, x86) — frontend, decoder, exec |
| [ci-and-release.md](ci-and-release.md) | CI pipeline, `make pre-commit`, versioning, crate publishing |
| [performance.md](performance.md) | Profiling, MIPS benchmarks, JIT compilation overhead, hot paths |
| [known-issues.md](known-issues.md) | Current limitations, sysreg sync gaps, unimplemented instructions |
