//! Validation of decode trees: detect overlapping, shadowed, and
//! malformed instruction patterns.
//!
//! Call [`validate`] after building a [`DecodeTree`] to get a list of
//! diagnostics.  Each diagnostic has a severity ([`Severity`]) and a
//! human-readable message.

use crate::tree::DecodeTree;

/// Parse a `.decode` file and validate it, returning diagnostics.
///
/// Performs both structural validation (bit widths) during parsing
/// and semantic validation (overlaps, shadows) after.
pub fn parse_and_validate(text: &str) -> (Option<DecodeTree>, Vec<Diagnostic>) {
    let mut diags = Vec::new();

    let mut line_no = 0;
    for raw_line in text.lines() {
        line_no += 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('%') || line.starts_with('&') || line.starts_with('@')
            || line.starts_with('{') || line.starts_with('}')
            || line.starts_with('[') || line.starts_with(']')
        {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 { continue; }

        let mut bit_count: u32 = 0;
        for token in &parts[1..] {
            if token.starts_with('@') || token.starts_with('&')
                || token.starts_with('!') || token.starts_with('%')
                || token.contains('=')
            {
                continue;
            }
            if token.contains(':') {
                let w_str = token.rsplit(':').next().unwrap_or("0");
                if let Ok(w) = w_str.parse::<u32>() {
                    bit_count += w;
                }
            } else {
                for ch in token.chars() {
                    match ch {
                        '0' | '1' | '.' | '-' => bit_count += 1,
                        '_' => {}
                        _ => {}
                    }
                }
            }
        }
        if bit_count != 0 && bit_count != 32 {
            diags.push(Diagnostic {
                severity: Severity::Error,
                message: format!(
                    "line {line_no}: '{}' has {bit_count} bits (expected 32)",
                    parts[0]
                ),
                pattern_idx: 0,
                related_idx: None,
            });
        }
    }

    if has_errors(&diags) {
        return (None, diags);
    }

    let tree = DecodeTree::from_decode_text(text);

    // ── field/fixedbit overlap checks ──────────────────────────────
    for (i, node) in tree.nodes.iter().enumerate() {
        let p = &node.pattern;
        for f in &p.fields {
            let field_mask = ((1u32 << f.width) - 1) << f.lsb;
            if p.mask & field_mask != 0 {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "'{}': field '{}' (bits {}:{}) overlaps fixed bits",
                        node.mnemonic, f.name, f.lsb, f.width
                    ),
                    pattern_idx: i,
                    related_idx: None,
                });
            }
        }
    }

    // ── field-definition segment overlap ───────────────────────────
    for (name, fd) in &tree.field_defs {
        let mut covered = 0u32;
        for &(lsb, width) in &fd.segments {
            let seg_mask = ((1u32 << width) - 1) << lsb;
            if covered & seg_mask != 0 {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "%{name}: sub-field segments overlap at bits {lsb}:{width}"
                    ),
                    pattern_idx: 0,
                    related_idx: None,
                });
            }
            covered |= seg_mask;
        }
    }

    let mut post_diags = validate(&tree);
    diags.append(&mut post_diags);
    (Some(tree), diags)
}

/// Severity of a validation diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational (e.g. duplicate mnemonic with different encoding).
    Info,
    /// Potential problem that may be intentional (e.g. overlap inside
    /// a `{}` group where first-match semantics apply).
    Warning,
    /// Definite error: unreachable pattern, bit-count mismatch, etc.
    Error,
}

/// A single validation diagnostic.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    /// Index of the primary pattern (in `DecodeTree::nodes`).
    pub pattern_idx: usize,
    /// Index of a related pattern, if applicable (e.g. the shadowing one).
    pub related_idx: Option<usize>,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tag = match self.severity {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "ERROR",
        };
        write!(f, "[{tag}] pattern #{}: {}", self.pattern_idx, self.message)
    }
}

/// Validate a decode tree and return all diagnostics.
///
/// Checks performed:
/// - **Overlap**: two patterns (outside `{}` groups) whose mask/value
///   ranges intersect — some instruction word would match both.
/// - **Shadowed**: pattern B is completely covered by earlier pattern A,
///   so B is unreachable.
/// - **Duplicate**: same mnemonic appears with identical mask/value.
/// - **Empty mask**: a pattern with mask=0 matches everything.
pub fn validate(tree: &DecodeTree) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    let nodes = &tree.nodes;

    for i in 0..nodes.len() {
        let pi = &nodes[i].pattern;

        // ── empty mask ─────────────────────────────────────────
        if pi.mask == 0 {
            diags.push(Diagnostic {
                severity: Severity::Error,
                message: format!(
                    "'{}': mask is 0 — matches every instruction",
                    nodes[i].mnemonic
                ),
                pattern_idx: i,
                related_idx: None,
            });
        }

        for j in (i + 1)..nodes.len() {
            let pj = &nodes[j].pattern;

            // Can the two patterns match the same instruction?
            // They overlap iff there exists an insn where
            //   (insn & mask_i) == value_i  AND  (insn & mask_j) == value_j
            //
            // A necessary condition is that the bits fixed by BOTH
            // patterns agree on their overlapping positions:
            //   (value_i & mask_j) == (value_j & mask_i)  — on the
            // intersection of their fixed bits.
            //
            // Formally: let common = mask_i & mask_j.  They can
            // co-match iff (value_i & common) == (value_j & common).
            let common = pi.mask & pj.mask;
            if (pi.value & common) != (pj.value & common) {
                continue; // disjoint — no overlap
            }

            // They overlap.  Determine the relationship.
            // i shadows j: every insn matching j also matches i.
            // Requires mask_i ⊆ mask_j (i fixes fewer or equal bits)
            // and they agree on the bits i does fix.
            let i_shadows_j = (pi.mask & pj.mask) == pi.mask
                && (pi.value & pi.mask) == (pj.value & pi.mask);

            // ── exact duplicate ────────────────────────────────
            if pi.mask == pj.mask
                && pi.value == pj.value
                && pi.constraints == pj.constraints
            {
                let sev = if nodes[i].mnemonic == nodes[j].mnemonic {
                    Severity::Warning
                } else {
                    Severity::Error
                };
                diags.push(Diagnostic {
                    severity: sev,
                    message: format!(
                        "'{}' and '{}': identical encoding (mask={:#010x} value={:#010x})",
                        nodes[i].mnemonic, nodes[j].mnemonic, pi.mask, pi.value
                    ),
                    pattern_idx: j,
                    related_idx: Some(i),
                });
                continue;
            }

            // ── shadowed (j unreachable because i matches first) ──
            if i_shadows_j
                && pi.mask != pj.mask
                && pi.constraints.is_empty()
                && pj.constraints.is_empty()
            {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "'{}' (#{j}) is shadowed by '{}' (#{i}) — unreachable",
                        nodes[j].mnemonic, nodes[i].mnemonic,
                    ),
                    pattern_idx: j,
                    related_idx: Some(i),
                });
                continue;
            }

            // ── general overlap ────────────────────────────────
            // Constraints may disambiguate at runtime, so only warn.
            let has_constraints =
                !pi.constraints.is_empty() || !pj.constraints.is_empty();
            let sev = Severity::Warning;
            let _ = has_constraints;

            diags.push(Diagnostic {
                severity: sev,
                message: format!(
                    "'{}' (#{i}) and '{}' (#{j}) overlap \
                     (common_mask={:#010x}, i_mask={:#010x}, j_mask={:#010x})",
                    nodes[i].mnemonic,
                    nodes[j].mnemonic,
                    common,
                    pi.mask,
                    pj.mask,
                ),
                pattern_idx: j,
                related_idx: Some(i),
            });
        }
    }

    diags.sort_by(|a, b| b.severity.cmp(&a.severity));
    diags
}

/// Returns `true` if any diagnostic is [`Severity::Error`].
pub fn has_errors(diags: &[Diagnostic]) -> bool {
    diags.iter().any(|d| d.severity == Severity::Error)
}

/// Format all diagnostics as a multi-line string.
pub fn format_diagnostics(diags: &[Diagnostic]) -> String {
    diags
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}
