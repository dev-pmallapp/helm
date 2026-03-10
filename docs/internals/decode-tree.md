# Decode Tree

The `helm-decode` crate provides a QEMU-compatible decode-tree engine.

## .decode File Format

HELM uses the same `.decode` file syntax as QEMU's `decodetree.py`:

| Element | Syntax | Purpose |
|---------|--------|---------|
| Field | `%name pos:len` | Named bit extraction |
| Argument set | `&name field1 field2 ...` | Group of fields for handler function |
| Format | `@name pattern &argset` | Reusable bit-pattern template |
| Pattern | `MNEMONIC bits @format` | Instruction encoding |
| Group | `{ pat1 \n pat2 }` | Overlapping patterns (first-match) |
| Constraint | `field=value` | Fixed field value requirement |
| Comment | `# ...` | Ignored |

## Internal Representation

- `DecodeTree` — collection of `DecodeNode`s, field definitions, argument sets, and format definitions.
- `DecodeNode` — mnemonic + `DecodePattern` + overlap group IDs.
- `DecodePattern` — `(mask, value)` pair plus extracted `BitField`s and constraints.
- `BitField` — `(name, position, length)` for extracting fields from an instruction word.
- `FieldDef` — named field definition parsed from `%name pos:len`.
- `FormatDef` — reusable pattern template parsed from `@name`.
- `ArgSet` — argument set parsed from `&name field1 field2`.

## Matching

An instruction matches a pattern when `(insn & mask) == value` AND all
field-value constraints are satisfied.

## Code Generation

`codegen::generate_decoder()` produces Rust source from a `DecodeTree`:

```text
.decode file → (parse) → DecodeTree → (codegen) → Rust match fn
```

The generated code dispatches to handler trait methods. Options control:
- Function name and visibility.
- Trait name (optional — generates a trait with one method per mnemonic).
- Return type and fallthrough expression.
- Field extraction as local `let` bindings.

## Dual Backend

The same `.decode` file generates two code paths at build time:

- **TCG path** — handler methods emit `TcgOp` sequences (in `helm-tcg`).
- **Static path** — handler methods return `MicroOp` classification (in `helm-isa`).

## Validation

`validate::validate()` checks for:
- Overlapping patterns outside `{}` groups.
- Unreachable patterns.
- Undefined field or format references.
- Constraint conflicts.

Diagnostics are returned as `Vec<Diagnostic>` with severity levels.
