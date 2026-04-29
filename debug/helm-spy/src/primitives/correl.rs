//! `CorrelHist2D` -- dual-impl, feature-gated.

#[cfg(feature = "collection")]
pub use live::CorrelHist2D;
#[cfg(not(feature = "collection"))]
pub use noop::CorrelHist2D;

#[cfg(feature = "collection")]
mod live {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 2D joint histogram (correlation histogram).
    /// Flat Vec<AtomicU64> storage in row-major layout.
    /// Given X edges and Y edges, there are (len(x_edges)+1) * (len(y_edges)+1) buckets.
    ///
    /// Hot-path cost: two `partition_point` + one `fetch_add(Relaxed)`.
    pub struct CorrelHist2D {
        name: String,
        x_edges: Vec<u64>,
        y_edges: Vec<u64>,
        x_buckets: usize,
        y_buckets: usize,
        counts: Vec<AtomicU64>,
    }

    impl CorrelHist2D {
        pub fn new(name: impl Into<String>, x_edges: Vec<u64>, y_edges: Vec<u64>) -> Self {
            let x_buckets = x_edges.len() + 1;
            let y_buckets = y_edges.len() + 1;
            let total = x_buckets * y_buckets;
            let mut counts = Vec::with_capacity(total);
            for _ in 0..total {
                counts.push(AtomicU64::new(0));
            }
            Self {
                name: name.into(),
                x_edges,
                y_edges,
                x_buckets,
                y_buckets,
                counts,
            }
        }

        pub fn name(&self) -> &str {
            &self.name
        }

        #[inline]
        pub fn record(&self, x: u64, y: u64) {
            let xi = self.x_edges.partition_point(|&e| x >= e);
            let yi = self.y_edges.partition_point(|&e| y >= e);
            let idx = xi * self.y_buckets + yi;
            self.counts[idx].fetch_add(1, Ordering::Relaxed);
        }

        /// Get count at bucket (xi, yi) where xi and yi are bucket indices.
        pub fn get(&self, xi: usize, yi: usize) -> u64 {
            self.counts[xi * self.y_buckets + yi].load(Ordering::Relaxed)
        }

        /// Returns the full counts matrix as a 2D Vec.
        pub fn matrix(&self) -> Vec<Vec<u64>> {
            let mut result = Vec::with_capacity(self.x_buckets);
            for xi in 0..self.x_buckets {
                let mut row = Vec::with_capacity(self.y_buckets);
                for yi in 0..self.y_buckets {
                    row.push(self.counts[xi * self.y_buckets + yi].load(Ordering::Relaxed));
                }
                result.push(row);
            }
            result
        }

        pub fn total(&self) -> u64 {
            self.counts.iter().map(|c| c.load(Ordering::Relaxed)).sum()
        }

        pub fn x_buckets(&self) -> usize {
            self.x_buckets
        }

        pub fn y_buckets(&self) -> usize {
            self.y_buckets
        }

        pub fn reset(&self) {
            for c in &self.counts {
                c.store(0, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(not(feature = "collection"))]
mod noop {
    /// ZST no-op 2D correlation histogram.
    #[derive(Clone, Copy, Default)]
    pub struct CorrelHist2D;

    impl CorrelHist2D {
        #[inline(always)]
        pub fn new(_name: impl Into<String>, _x_edges: Vec<u64>, _y_edges: Vec<u64>) -> Self {
            Self
        }
        #[inline(always)]
        pub fn name(&self) -> &str {
            ""
        }
        #[inline(always)]
        pub fn record(&self, _x: u64, _y: u64) {}
        #[inline(always)]
        pub fn get(&self, _xi: usize, _yi: usize) -> u64 {
            0
        }
        #[inline(always)]
        pub fn matrix(&self) -> Vec<Vec<u64>> {
            Vec::new()
        }
        #[inline(always)]
        pub fn total(&self) -> u64 {
            0
        }
        #[inline(always)]
        pub fn x_buckets(&self) -> usize {
            0
        }
        #[inline(always)]
        pub fn y_buckets(&self) -> usize {
            0
        }
        #[inline(always)]
        pub fn reset(&self) {}
    }
}

#[cfg(all(test, feature = "collection"))]
mod tests {
    use super::*;

    #[test]
    fn correl_hist_basic() {
        // x_edges: [10, 20], y_edges: [100, 200]
        // x buckets: [<10, 10..20, >=20]  (3)
        // y buckets: [<100, 100..200, >=200]  (3)
        let ch = CorrelHist2D::new("test", vec![10, 20], vec![100, 200]);
        assert_eq!(ch.x_buckets(), 3);
        assert_eq!(ch.y_buckets(), 3);
        assert_eq!(ch.total(), 0);

        // (5, 50) -> x bucket 0, y bucket 0
        ch.record(5, 50);
        // (15, 150) -> x bucket 1, y bucket 1
        ch.record(15, 150);
        // (25, 250) -> x bucket 2, y bucket 2
        ch.record(25, 250);

        assert_eq!(ch.get(0, 0), 1);
        assert_eq!(ch.get(1, 1), 1);
        assert_eq!(ch.get(2, 2), 1);
        assert_eq!(ch.get(0, 1), 0);
        assert_eq!(ch.total(), 3);
    }

    #[test]
    fn correl_hist_matrix() {
        let ch = CorrelHist2D::new("matrix", vec![10], vec![100]);
        // 2x2 matrix
        ch.record(5, 50); // (0,0)
        ch.record(5, 150); // (0,1)
        ch.record(15, 50); // (1,0)
        ch.record(15, 150); // (1,1)
        ch.record(15, 150); // (1,1) again

        let m = ch.matrix();
        assert_eq!(m, vec![vec![1, 1], vec![1, 2]]);
    }

    #[test]
    fn correl_hist_reset() {
        let ch = CorrelHist2D::new("reset", vec![10], vec![10]);
        ch.record(5, 5);
        ch.record(15, 15);
        assert_eq!(ch.total(), 2);
        ch.reset();
        assert_eq!(ch.total(), 0);
    }
}
