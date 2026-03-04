//! Core plugin trait and argument parsing.

use super::registry::PluginRegistry;
use std::collections::HashMap;

/// Arguments passed to a plugin at load time.
#[derive(Debug, Clone, Default)]
pub struct PluginArgs {
    inner: HashMap<String, String>,
}

impl PluginArgs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parse(s: &str) -> Self {
        let mut inner = HashMap::new();
        for pair in s.split(',') {
            if let Some((k, v)) = pair.split_once('=') {
                inner.insert(k.to_string(), v.to_string());
            }
        }
        Self { inner }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.inner.get(key).map(String::as_str)
    }

    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.inner
            .get(key)
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    pub fn get_usize(&self, key: &str, default: usize) -> usize {
        self.inner
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
}

/// A HELM trace/analysis plugin.
///
/// Implement this trait to create custom instrumentation.
/// Register callbacks in [`install()`](HelmPlugin::install).
pub trait HelmPlugin: Send + Sync {
    /// Plugin name (e.g. `"execlog"`, `"cache"`).
    fn name(&self) -> &str;

    /// Called once at load time.  Register callbacks via the registry.
    fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs);

    /// Called at simulation end.  Print stats, flush logs.
    fn atexit(&mut self) {}
}
