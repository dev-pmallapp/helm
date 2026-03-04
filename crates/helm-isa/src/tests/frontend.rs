use crate::frontend::*;

// Verifies the trait is object-safe (can be used as dyn).
#[test]
fn trait_is_object_safe() {
    fn _accepts_dyn(_f: &dyn IsaFrontend) {}
}
