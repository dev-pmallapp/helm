//! `@format` definitions — reusable bit-pattern templates.

/// A `@name` format definition from a `.decode` file.
///
/// QEMU syntax: `@name bit_pattern &argset`
///
/// Patterns reference formats with `@name` to inherit field positions
/// and argument sets.
#[derive(Debug, Clone)]
pub struct FormatDef {
    pub name: String,
    /// The raw pattern tokens (fixed bits, fields, don't-cares).
    pub tokens: Vec<String>,
    /// Argument set name (if any).
    pub arg_set: Option<String>,
}

/// Parse an `@name ...` line.
pub fn parse_format_def(line: &str) -> Option<FormatDef> {
    let line = line.trim();
    if !line.starts_with('@') {
        return None;
    }
    let mut parts: Vec<&str> = line.split_whitespace().collect();
    let name = parts.remove(0).trim_start_matches('@').to_string();

    let mut arg_set = None;
    let mut tokens = Vec::new();
    for p in parts {
        if p.starts_with('&') {
            arg_set = Some(p.trim_start_matches('&').to_string());
        } else {
            tokens.push(p.to_string());
        }
    }

    Some(FormatDef {
        name,
        tokens,
        arg_set,
    })
}
