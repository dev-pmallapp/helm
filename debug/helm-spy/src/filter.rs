#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PcRangeFilter {
    pub start: u64,
    pub end: u64,
}

impl PcRangeFilter {
    pub fn new(start: u64, end: u64) -> Result<Self, &'static str> {
        if end <= start {
            return Err("pc range end must be greater than start");
        }
        Ok(Self { start, end })
    }

    #[inline]
    pub fn contains(&self, pc: u64) -> bool {
        pc >= self.start && pc < self.end
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddrRangeFilter {
    pub start: u64,
    pub end: u64,
}

impl AddrRangeFilter {
    pub fn new(start: u64, end: u64) -> Result<Self, &'static str> {
        if end <= start {
            return Err("address range end must be greater than start");
        }
        Ok(Self { start, end })
    }

    #[inline]
    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.start && addr < self.end
    }
}

#[cfg(test)]
mod tests {
    use super::{AddrRangeFilter, PcRangeFilter};

    #[test]
    fn pc_range_filter_is_half_open() {
        let filter = PcRangeFilter::new(0x1000, 0x1100).unwrap();
        assert!(!filter.contains(0x0fff));
        assert!(filter.contains(0x1000));
        assert!(filter.contains(0x10ff));
        assert!(!filter.contains(0x1100));
    }

    #[test]
    fn pc_range_filter_rejects_empty_range() {
        assert!(PcRangeFilter::new(0x1000, 0x1000).is_err());
        assert!(PcRangeFilter::new(0x2000, 0x1000).is_err());
    }

    #[test]
    fn addr_range_filter_is_half_open() {
        let filter = AddrRangeFilter::new(0x2000, 0x2100).unwrap();
        assert!(!filter.contains(0x1fff));
        assert!(filter.contains(0x2000));
        assert!(filter.contains(0x20ff));
        assert!(!filter.contains(0x2100));
    }

    #[test]
    fn addr_range_filter_rejects_empty_range() {
        assert!(AddrRangeFilter::new(0x1000, 0x1000).is_err());
        assert!(AddrRangeFilter::new(0x2000, 0x1000).is_err());
    }
}
