use dashmap::DashMap;

/// Per-PC (or per-address) counter map using DashMap for concurrent access.
/// Hot-path cost: one DashMap shard lock (brief critical section).
pub struct HeatMap {
    name: String,
    counts: DashMap<u64, u64>,
}

impl HeatMap {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            counts: DashMap::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn inc(&self, pc: u64) {
        *self.counts.entry(pc).or_insert(0) += 1;
    }

    /// Returns the top N entries sorted by count (descending).
    pub fn top(&self, n: usize) -> Vec<(u64, u64)> {
        let mut v: Vec<_> = self.counts.iter().map(|e| (*e.key(), *e.value())).collect();
        v.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        v.truncate(n);
        v
    }

    pub fn get(&self, pc: u64) -> u64 {
        self.counts.get(&pc).map(|r| *r).unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.counts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    pub fn clear(&self) {
        self.counts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heatmap_basic_inc_and_get() {
        let hm = HeatMap::new("test");
        assert_eq!(hm.len(), 0);
        assert!(hm.is_empty());

        hm.inc(0x1000);
        hm.inc(0x1000);
        hm.inc(0x2000);

        assert_eq!(hm.get(0x1000), 2);
        assert_eq!(hm.get(0x2000), 1);
        assert_eq!(hm.get(0x3000), 0);
        assert_eq!(hm.len(), 2);
    }

    #[test]
    fn heatmap_top_ordering() {
        let hm = HeatMap::new("top");
        for _ in 0..100 {
            hm.inc(0xA000);
        }
        for _ in 0..50 {
            hm.inc(0xB000);
        }
        for _ in 0..200 {
            hm.inc(0xC000);
        }
        for _ in 0..10 {
            hm.inc(0xD000);
        }

        let top3 = hm.top(3);
        assert_eq!(top3.len(), 3);
        assert_eq!(top3[0], (0xC000, 200));
        assert_eq!(top3[1], (0xA000, 100));
        assert_eq!(top3[2], (0xB000, 50));
    }

    #[test]
    fn heatmap_top_fewer_than_n() {
        let hm = HeatMap::new("few");
        hm.inc(0x1000);
        let top10 = hm.top(10);
        assert_eq!(top10.len(), 1);
    }

    #[test]
    fn heatmap_clear() {
        let hm = HeatMap::new("clear");
        hm.inc(0x1000);
        hm.inc(0x2000);
        assert_eq!(hm.len(), 2);
        hm.clear();
        assert_eq!(hm.len(), 0);
        assert!(hm.is_empty());
    }
}
