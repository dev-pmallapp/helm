# Decode-Tree Format

HELM uses QEMU-compatible decode-tree syntax.  QEMU's upstream ARM
`.decode` files (from `target/arm/tcg/`) can be parsed directly.

Reference: [QEMU decodetree docs](https://www.qemu.org/docs/master/devel/decodetree.html),
source: `scripts/decodetree.py`

## Syntax

### Comments and blanks

```
# This is a comment
```

### Field definitions (`%name`)

Named bit-field extractors.  Multi-segment fields are concatenated.

```
%rd      0:5            # bits [4:0]
%rn      5:5            # bits [9:5]
%imm12   10:12          # bits [21:10]
%imm     5:7 0:5        # concatenate bits [11:5] and [4:0]
```

Signed fields prefix the first segment with `s`:

```
%simm19  s5:19          # sign-extended 19-bit immediate
```

### Argument sets (`&name`)

Group of field names passed to translate functions.

```
&ri      rd rn imm
&branch  imm
```

### Formats (`@name`)

Reusable bit-pattern templates.

```
@addsub  .... .... .. imm:12 rn:5 rd:5   &ri
```

### Patterns

Instruction encodings.  Fixed bits as `0`/`1`, don't-care as `.`,
must-be-zero as `-`, fields as `name:N`, constraints as `name=value`.

```
ADD_imm    sf:1 0 0 10001 sh:2 imm12:12 rn:5 rd:5
CMP_imm    sf:1 1 1 10001 sh:2 imm12:12 rn:5 rd:5  rd=31
SUBS_imm   sf:1 1 1 10001 sh:2 imm12:12 rn:5 rd:5
```

Patterns can reference formats:

```
ADD_imm    sf:1 0 0 10001 sh:2 ............ ..... .....  @addsub
```

### Groups (`{ }`)

Overlapping patterns tested together.  First match wins.

```
{
  CMP_imm    sf:1 1 1 10001 sh:2 imm12:12 rn:5 rd:5  rd=31
  SUBS_imm   sf:1 1 1 10001 sh:2 imm12:12 rn:5 rd:5
}
```

### Summary

| Element | Prefix | Example |
|---------|--------|---------|
| Field | `%` | `%rd 0:5` |
| Arg set | `&` | `&ri rd rn imm` |
| Format | `@` | `@addsub .... rn:5 rd:5 &ri` |
| Pattern | `NAME` | `ADD_imm sf:1 0 0 10001 ...` |
| Group | `{`/`}` | `{ CMP_imm ... ; SUBS_imm ... }` |
| Constraint | `=` | `rd=31` |
| Comment | `#` | `# note` |
| Don't-care | `.` | `....` |
| Must-be-zero | `-` | `----` |

## Using QEMU .decode Files Directly

QEMU's AArch64 decode files live in `target/arm/tcg/`:

```
a64.decode              # top-level, includes the others
a64-base.decode         # core integer + branch
a64-ldst.decode         # loads and stores
a64-dp.decode           # data processing
a64-simd.decode         # SIMD and FP
a64-crypto.decode       # crypto extensions
```

HELM can load these directly:

```rust
let text = std::fs::read_to_string("a64.decode")?;
let tree = DecodeTree::from_decode_text(&text);
// tree.lookup(insn) works on any AArch64 instruction
```

The only feature not yet implemented is `!function=` annotations
(custom extraction functions).  These are rare and can be handled
with post-extraction fixups.

## Dual Backend

```
.decode file ──► DecodeTree (immutable, Arc-shared)
                       │
                       ├──► TCG emitter   ──► TcgOp chain    (SE / FE)
                       │    per-mnemonic       fast interp
                       │
                       └──► MicroOp emitter ──► Vec<MicroOp>  (APE / CAE)
                            per-mnemonic       pipeline model
```

Both emitters dispatch on the mnemonic string returned by
`tree.lookup()`.  The extracted fields are the same regardless
of which backend consumes them.

## Multi-Threaded Safety

`DecodeTree` is built once at startup and wrapped in `Arc`.
All core threads share it read-only.  No synchronisation needed.

Each core thread owns:
- Its own `TcgContext` (TCG path)
- Its own `Vec<MicroOp>` buffer (static path)
