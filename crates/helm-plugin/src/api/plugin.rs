use std::collections::HashMap;
use crate::runtime::PluginRegistry;

/// Key-value argument bag passed to a plugin at install time.
#[derive(Debug, Default, Clone)]
pub struct PluginArgs {
    inner: HashMap<String, String>,
}

impl PluginArgs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a `key=value,key2=value2` string into a `PluginArgs`.
    pub fn parse(s: &str) -> Self {
        let mut inner = HashMap::new();
        for pair in s.split(',') {
            let mut kv = pair.splitn(2, '=');
            if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
                inner.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        Self { inner }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.inner.get(key).map(String::as_str)
    }

    pub fn get_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).unwrap_or(default)
    }

    pub fn get_usize(&self, key: &str) -> Option<usize> {
        self.get(key)?.parse().ok()
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            "1" | "true" | "yes" => Some(true),
            "0" | "false" | "no" => Some(false),
            _ => None,
        }
    }
}

/// Stable trait that every plugin must implement.
pub trait HelmPlugin: Send + Sync {
    fn name(&self) -> &str;

    /// Register callbacks into the registry.  Called once at startup.
    fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs);

    /// Called when simulation is ending (teardown / report).
    fn atexit(&mut self) {}
}
