# HELM Documentation

Comprehensive documentation for HELM — Hybrid Emulation Layer for
Microarchitecture.  Organised into six top-level categories.

Build the docs with [mdBook](https://rust-lang.github.io/mdBook/):
```bash
cd docs && mdbook build    # output in docs/book/
mdbook serve               # local preview at http://localhost:3000
```

```
docs/
  guide/           Getting started, tutorials, and user-facing guides
  architecture/    System design, crate layout, and high-level data flows
  internals/       Deep dives into subsystem implementation details
  reference/       API surfaces, configuration knobs, and ISA coverage
  development/     Contributing, testing, coding style, and release process
  research/        Performance analysis and speed improvement roadmaps
```

## Category overview

| Category | Audience | Purpose |
|----------|----------|---------|
| **guide/** | Users, new developers | How to build, run, and configure HELM |
| **architecture/** | All developers | Why the system is shaped the way it is |
| **internals/** | Core developers | How each subsystem works under the hood |
| **reference/** | All | Lookup tables, knob lists, API docs |
| **development/** | Contributors | Process, style, testing, CI |
