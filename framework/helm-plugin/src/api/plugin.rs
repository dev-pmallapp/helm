use crate::runtime::HelmPluginRegistry;
use std::collections::HashMap;

/// Key-value argument bag passed to a legacy callback plugin at install time.
#[derive(Debug, Default, Clone)]
pub struct HelmPluginArgs {
    inner: HashMap<String, String>,
}

impl HelmPluginArgs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a `key=value,key2=value2` string into a `HelmPluginArgs`.
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

/// Stable trait implemented by legacy callback plugins.
///
/// New observation flows should prefer probe/session-backed collection in
/// `helm-probe` + `helm-spy` instead of adding new users of this trait.
pub trait HelmPlugin: Send + Sync {
    fn name(&self) -> &str;

    /// Register callbacks into the registry.  Called once at startup.
    fn install(&mut self, reg: &mut HelmPluginRegistry, args: &HelmPluginArgs);

    /// Called when simulation is ending (teardown / report).
    fn atexit(&mut self) {}
}
