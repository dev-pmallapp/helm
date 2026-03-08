use crate::irq;

#[test]
fn spi_irq_encoding() {
    assert_eq!(irq::spi_irq(0), 0);
    assert_eq!(irq::spi_irq(33), 33);
    assert_eq!(irq::spi_irq(255), 255);
}

#[test]
fn ppi_irq_encoding() {
    let ppi = irq::ppi_irq(11, 0);
    assert_eq!(ppi & 0xFF, 11);
    assert_eq!((ppi >> 16) & 0xFF, 0); // vcpu 0
    assert_eq!((ppi >> 24) & 0xFF, 1); // PPI type flag
}

#[test]
fn ppi_irq_vcpu1() {
    let ppi = irq::ppi_irq(14, 1);
    assert_eq!(ppi & 0xFF, 14);
    assert_eq!((ppi >> 16) & 0xFF, 1); // vcpu 1
    assert_eq!((ppi >> 24) & 0xFF, 1);
}

#[test]
fn spi_and_ppi_are_distinct() {
    let spi = irq::spi_irq(11);
    let ppi = irq::ppi_irq(11, 0);
    assert_ne!(spi, ppi);
}
