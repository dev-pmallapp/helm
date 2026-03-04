# Decode-Tree Format

HELM uses a QEMU-style decode-tree DSL to specify instruction
encodings.  A single `.decode` file generates **two** decoder backends:

```
 aarch64-dp-imm.decode ──┐
 aarch64-branch.decode ──┤
 aarch64-ldst.decode ────┤──► helm-decode parser
 aarch64-dp-reg.decode ──┘         │
                                   ├──► TCG backend  ──► TcgOp chain   (SE / FE)
                                   │
                                   └──► Static backend ──► MicroOp vec (APE / CAE)
```

## File Format

```
# Comments start with #
# MNEMONIC  bit_tokens...
#
# Bit tokens (MSB-first, must total exactly 32):
#   0        fixed zero bit
#   1        fixed one bit
#   .        don't-care bit
#   name:N   N-bit variable field
#   _        cosmetic separator (ignored)
```

### Example

```
# Add/subtract immediate
ADD_imm   sf:1 0 0 10001 sh:1 imm12:12 rn:5 rd:5
SUBS_imm  sf:1 1 1 10001 sh:1 imm12:12 rn:5 rd:5

# Unconditional branch
B         0 00101 imm26:26
BL        1 00101 imm26:26

# NOP (all 32 bits fixed)
NOP       11010101 00000011 00100000 00011111
```

### Rules

1. Every line must produce exactly 32 bits (sum of all widths).
2. Fixed bits (`0`/`1`) form the mask and value used for matching.
3. Fields are extracted at the LSB position determined by their
   left-to-right order (MSB-first layout).
4. First matching pattern wins — put more specific patterns first.

## How Matching Works

For each pattern, the parser produces a `(mask, value)` pair from the
fixed bits.  An instruction matches when:

```
(insn & mask) == value
```

Fields are extracted with:

```
field_value = (insn >> lsb) & ((1 << width) - 1)
```

## Dual Backend Generation

### TCG Path (SE / FE mode)

The decode tree drives a `match` that calls per-instruction TCG
emitters:

```rust
fn translate_insn(ctx: &mut TcgContext, mnemonic: &str, fields: &Fields) {
    match mnemonic {
        "ADD_imm" => {
            let rn = ctx.read_reg(fields.rn);
            let imm = ctx.movi(fields.imm12 as u64);
            let result = ctx.add(rn, imm);
            ctx.write_reg(fields.rd, result);
        }
        "B" => {
            let target = pc + sign_extend(fields.imm26, 26) * 4;
            ctx.emit(TcgOp::GotoTb { target_pc: target });
        }
        // ...
    }
}
```

The TCG ops are then interpreted or (future) JIT-compiled to host code.

### Static Path (APE / CAE mode)

The same decode tree drives a `match` that produces `MicroOp`
sequences for the pipeline model:

```rust
fn decode_to_uops(mnemonic: &str, fields: &Fields) -> Vec<MicroOp> {
    match mnemonic {
        "ADD_imm" => vec![MicroOp {
            opcode: Opcode::IntAlu,
            sources: vec![fields.rn],
            dest: Some(fields.rd),
            immediate: Some(fields.imm12 as u64),
            ..Default::default()
        }],
        "B" => vec![MicroOp {
            opcode: Opcode::Branch,
            flags: MicroOpFlags { is_branch: true, ..Default::default() },
            ..Default::default()
        }],
        // ...
    }
}
```

## File Organisation

```
crates/helm-isa/src/arm/aarch64/decode_files/
    aarch64-dp-imm.decode     integer ALU immediate
    aarch64-dp-reg.decode     integer ALU register
    aarch64-branch.decode     branches, exceptions, system
    aarch64-ldst.decode       loads, stores, pairs, atomics
    aarch64-simd-fp.decode    SIMD and floating-point (future)
```

## Multi-Threaded Considerations

The `DecodeTree` is immutable after construction — it is built once at
startup and shared read-only across all core threads with zero
synchronisation cost.  Each core owns its own `TcgContext` (TCG path)
or `MicroOp` buffer (static path).

See [multi-threaded-execution.md](multi-threaded-execution.md) for the
full threading model.
