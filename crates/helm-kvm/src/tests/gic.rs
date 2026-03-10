use crate::gic::{GicConfig, GicVersion};

#[test]
fn gic_config_default() {
    let cfg = GicConfig::default();
    assert_eq!(cfg.version, GicVersion::V3);
    assert_eq!(cfg.num_irqs, 128);
    assert_eq!(cfg.dist_addr, 0x0800_0000);
    assert_eq!(cfg.num_cpus, 1);
}

#[test]
fn gic_v2_config() {
    let cfg = GicConfig {
        version: GicVersion::V2,
        num_irqs: 64,
        dist_addr: 0x0800_0000,
        cpu_or_redist_addr: 0x0801_0000,
        num_cpus: 1,
    };
    assert_eq!(cfg.version, GicVersion::V2);
    assert_eq!(cfg.num_irqs, 64);
}

#[test]
fn gic_version_equality() {
    assert_eq!(GicVersion::V2, GicVersion::V2);
    assert_eq!(GicVersion::V3, GicVersion::V3);
    assert_ne!(GicVersion::V2, GicVersion::V3);
}
