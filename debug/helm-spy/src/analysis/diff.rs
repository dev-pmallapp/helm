//! Differential analysis — compare two simulation metric sets.

use std::collections::BTreeMap;

/// A named metric with values from two sessions.
#[derive(Debug, Clone)]
pub struct MetricDiff {
    pub name: String,
    pub a: f64,
    pub b: f64,
}

impl MetricDiff {
    pub fn absolute_diff(&self) -> f64 { self.b - self.a }
    pub fn relative_diff_pct(&self) -> Option<f64> {
        if self.a == 0.0 { None } else { Some((self.b - self.a) / self.a * 100.0) }
    }
}

/// Result of comparing two simulation sessions.
#[derive(Debug, Clone)]
pub struct DiffReport {
    pub label_a: String,
    pub label_b: String,
    pub metrics: Vec<MetricDiff>,
    pub insn_mix_diff: BTreeMap<String, (u64, u64)>,
}

impl DiffReport {
    pub fn new(label_a: impl Into<String>, label_b: impl Into<String>) -> Self {
        Self { label_a: label_a.into(), label_b: label_b.into(),
            metrics: Vec::new(), insn_mix_diff: BTreeMap::new() }
    }

    pub fn add_metric(&mut self, name: impl Into<String>, a: f64, b: f64) {
        self.metrics.push(MetricDiff { name: name.into(), a, b });
    }

    pub fn add_insn_class(&mut self, class: impl Into<String>, a: u64, b: u64) {
        self.insn_mix_diff.insert(class.into(), (a, b));
    }

    pub fn to_text(&self) -> String {
        let mut out = format!("Diff: {} vs {}\n{}\n", self.label_a, self.label_b, "=".repeat(60));
        for m in &self.metrics {
            let pct = m.relative_diff_pct()
                .map(|p| format!("{p:+.2}%"))
                .unwrap_or_else(|| "N/A".into());
            out.push_str(&format!("{:<30} {:>12.2} {:>12.2}  {pct}\n", m.name, m.a, m.b));
        }
        if !self.insn_mix_diff.is_empty() {
            out.push_str("\nInstruction Mix:\n");
            for (class, (a, b)) in &self.insn_mix_diff {
                let pct = if *a > 0 {
                    format!("{:+.2}%", (*b as f64 - *a as f64) / *a as f64 * 100.0)
                } else { "N/A".into() };
                out.push_str(&format!("  {class:<20} {a:>10} {b:>10}  {pct}\n"));
            }
        }
        out
    }
}

/// Compare two sets of metrics.
pub fn diff_sessions(
    label_a: &str, label_b: &str,
    metrics_a: &[(String, f64)], metrics_b: &[(String, f64)],
) -> DiffReport {
    let mut report = DiffReport::new(label_a, label_b);
    let map_b: BTreeMap<&str, f64> = metrics_b.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let map_a: BTreeMap<&str, f64> = metrics_a.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    for (name, val_a) in metrics_a {
        report.add_metric(name, *val_a, map_b.get(name.as_str()).copied().unwrap_or(0.0));
    }
    for (name, val_b) in metrics_b {
        if !map_a.contains_key(name.as_str()) {
            report.add_metric(name, 0.0, *val_b);
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_diff() {
        let a = vec![("insn_count".into(), 1000.0), ("hit_rate".into(), 0.95)];
        let b = vec![("insn_count".into(), 1200.0), ("hit_rate".into(), 0.92)];
        let r = diff_sessions("base", "opt", &a, &b);
        assert_eq!(r.metrics.len(), 2);
        assert!((r.metrics[0].absolute_diff() - 200.0).abs() < 1e-10);
    }

    #[test]
    fn text_has_labels() {
        let r = diff_sessions("A", "B", &[("x".into(), 1.0)], &[("x".into(), 2.0)]);
        assert!(r.to_text().contains("A vs B"));
    }
}
