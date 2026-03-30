use std::sync::atomic::{AtomicU64, Ordering};

/// Fixed-bucket histogram. Bucket boundaries are defined by `edges`.
/// For N edges, there are N+1 buckets:
///   bucket[0]: val < edges[0]
///   bucket[i]: edges[i-1] <= val < edges[i]  (for 0 < i < N)
///   bucket[N]: val >= edges[N-1]
///
/// Hot-path cost: one `partition_point` + one `fetch_add(Relaxed)`.
pub struct Histogram {
    name: String,
    edges: Vec<u64>,
    buckets: Vec<AtomicU64>,
}

impl Histogram {
    pub fn new(name: impl Into<String>, edges: Vec<u64>) -> Self {
        let num_buckets = edges.len() + 1;
        let mut buckets = Vec::with_capacity(num_buckets);
        for _ in 0..num_buckets {
            buckets.push(AtomicU64::new(0));
        }
        Self {
            name: name.into(),
            edges,
            buckets,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn record(&self, val: u64) {
        let idx = self.edges.partition_point(|&e| val >= e);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    pub fn counts(&self) -> Vec<u64> {
        self.buckets
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .collect()
    }

    pub fn total(&self) -> u64 {
        self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).sum()
    }

    /// Returns the bucket edge at the p-th percentile (0.0..=1.0).
    /// Returns the lower edge of the bucket containing the p-th percentile sample.
    pub fn percentile(&self, p: f64) -> u64 {
        let total = self.total();
        if total == 0 {
            return 0;
        }
        let threshold = (p * total as f64).ceil() as u64;
        let mut cumulative = 0u64;
        for (i, bucket) in self.buckets.iter().enumerate() {
            cumulative += bucket.load(Ordering::Relaxed);
            if cumulative >= threshold {
                if i == 0 {
                    return if self.edges.is_empty() {
                        0
                    } else {
                        self.edges[0]
                    };
                }
                return self.edges[i - 1];
            }
        }
        // Should not reach here, but return the last edge
        *self.edges.last().unwrap_or(&0)
    }

    pub fn reset(&self) {
        for b in &self.buckets {
            b.store(0, Ordering::Relaxed);
        }
    }
}

/// IntervalHistogram: samples a scalar every N instructions, buckets
/// the per-window value into a Histogram.
pub struct IntervalHistogram {
    hist: Histogram,
    window_size: u64,
    window_accum: AtomicU64,
    last_window: AtomicU64,
}

impl IntervalHistogram {
    pub fn new(name: impl Into<String>, edges: Vec<u64>, window_size: u64) -> Self {
        Self {
            hist: Histogram::new(name, edges),
            window_size,
            window_accum: AtomicU64::new(0),
            last_window: AtomicU64::new(u64::MAX), // sentinel: no window seen yet
        }
    }

    /// Call this every step with the current value (e.g. IPC metric)
    /// and the global insn_count.
    pub fn tick(&self, value: u64, insn_count: u64) {
        let window = insn_count / self.window_size;
        let prev = self.last_window.swap(window, Ordering::Relaxed);
        if window != prev && prev != u64::MAX {
            let sample = self.window_accum.swap(value, Ordering::Relaxed);
            self.hist.record(sample);
        } else {
            self.window_accum.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn histogram(&self) -> &Histogram {
        &self.hist
    }

    pub fn counts(&self) -> Vec<u64> {
        self.hist.counts()
    }

    pub fn total(&self) -> u64 {
        self.hist.total()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_basic_record() {
        // Edges: [10, 100, 1000]
        // Buckets: [<10, 10..100, 100..1000, >=1000]
        let h = Histogram::new("test", vec![10, 100, 1000]);
        h.record(5); // bucket 0
        h.record(50); // bucket 1
        h.record(500); // bucket 2
        h.record(5000); // bucket 3

        let c = h.counts();
        assert_eq!(c, vec![1, 1, 1, 1]);
        assert_eq!(h.total(), 4);
    }

    #[test]
    fn histogram_edge_values() {
        let h = Histogram::new("edge", vec![10, 100]);
        // val == 10 -> partition_point finds first edge where val < e -> 10 >= 10 is true, 10 >= 100 is false -> idx=1
        h.record(10); // bucket 1 (10..100)
        h.record(100); // bucket 2 (>=100)
        h.record(0); // bucket 0 (<10)
        h.record(9); // bucket 0 (<10)

        let c = h.counts();
        assert_eq!(c, vec![2, 1, 1]);
    }

    #[test]
    fn histogram_percentile() {
        // Create histogram with edges [10, 20, 30]
        // Buckets: [<10, 10..20, 20..30, >=30]
        let h = Histogram::new("pctl", vec![10, 20, 30]);
        // Put 50 samples in bucket 0 (<10), 30 in bucket 1, 20 in bucket 2
        for _ in 0..50 {
            h.record(5);
        }
        for _ in 0..30 {
            h.record(15);
        }
        for _ in 0..20 {
            h.record(25);
        }
        assert_eq!(h.total(), 100);

        // p50 -> sample 50 -> cumulative 50 at bucket 0 -> edge[0] = 10
        assert_eq!(h.percentile(0.50), 10);
        // p51 -> sample 51 -> cumulative 50 at bucket 0, then 80 at bucket 1 -> edge[0] = 10
        assert_eq!(h.percentile(0.51), 10);
        // p80 -> sample 80 -> cumulative 80 at bucket 1 -> edge[0] = 10
        assert_eq!(h.percentile(0.80), 10);
        // p81 -> sample 81 -> cumulative 80 at bucket 1, then 100 at bucket 2 -> edge[1] = 20
        assert_eq!(h.percentile(0.81), 20);
    }

    #[test]
    fn histogram_empty_percentile() {
        let h = Histogram::new("empty", vec![10, 20]);
        assert_eq!(h.percentile(0.5), 0);
    }

    #[test]
    fn interval_histogram_window_boundary() {
        let ih = IntervalHistogram::new("interval", vec![5, 10, 20], 100);

        // First window (insn 0..99): accumulate values
        for i in 0..100 {
            ih.tick(1, i);
        }
        // When we cross into window 1, the accumulated value from window 0 gets recorded
        ih.tick(1, 100);

        // The histogram should have one sample recorded (from window 0 boundary)
        assert!(
            ih.total() >= 1,
            "should have at least 1 sample after window boundary"
        );
    }

    #[test]
    fn histogram_reset() {
        let h = Histogram::new("reset", vec![10]);
        h.record(5);
        h.record(15);
        assert_eq!(h.total(), 2);
        h.reset();
        assert_eq!(h.total(), 0);
    }
}
