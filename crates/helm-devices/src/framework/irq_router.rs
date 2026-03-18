//! Simple interrupt routing table.
//!
//! Maps `(source_device, source_line)` to a destination interrupt controller
//! and IRQ number. Used by the platform configuration layer to describe
//! interrupt topology declaratively before wiring pins at elaborate time.

use std::sync::Arc;

use super::interrupt::InterruptSink;

// ── IrqRoute ─────────────────────────────────────────────────────────────────

/// A single interrupt routing entry.
///
/// Describes how one device output line maps to one interrupt controller input.
/// The `source_device` / `source_line` pair identifies the device pin; the
/// `dest_controller` / `dest_irq` pair identifies where it is routed.
#[derive(Debug, Clone)]
pub struct IrqRoute {
    /// Opaque device identifier (e.g. device index in a `DeviceRegistry`).
    pub source_device: u64,
    /// Which output line on the source device (devices may have multiple pins).
    pub source_line: u32,
    /// Index into the router's controller list (returned by
    /// [`IrqRouter::add_controller`]).
    pub dest_controller: usize,
    /// IRQ number at the destination controller (becomes `WireId`).
    pub dest_irq: u32,
}

// ── IrqRouter ────────────────────────────────────────────────────────────────

/// Interrupt routing table.
///
/// Holds a set of [`IrqRoute`] entries and the interrupt controllers they
/// reference. The platform builds this table during configuration, then uses
/// it during `elaborate()` to call `World::wire_interrupt()` for each route.
///
/// # Example
///
/// ```ignore
/// use helm_devices::framework::irq_router::{IrqRouter, IrqRoute};
/// use helm_devices::framework::interrupt::WireId;
///
/// let mut router = IrqRouter::new();
/// let plic_idx = router.add_controller(plic_sink);
/// router.add_route(IrqRoute {
///     source_device: uart_id,
///     source_line: 0,
///     dest_controller: plic_idx,
///     dest_irq: 10,
/// });
///
/// // At elaborate time:
/// if let Some((sink, irq)) = router.route(uart_id, 0) {
///     world.wire_interrupt(uart_pin, sink, WireId::from(irq));
/// }
/// ```
pub struct IrqRouter {
    /// Registered interrupt controllers, indexed by the value returned from
    /// [`add_controller`](Self::add_controller).
    controllers: Vec<Arc<dyn InterruptSink>>,
    /// All routing entries.
    routes: Vec<IrqRoute>,
}

impl IrqRouter {
    /// Create an empty routing table.
    pub fn new() -> Self {
        Self {
            controllers: Vec::new(),
            routes: Vec::new(),
        }
    }

    /// Register an interrupt controller and return its index.
    ///
    /// The returned index is used as `dest_controller` in [`IrqRoute`] entries.
    pub fn add_controller(&mut self, controller: Arc<dyn InterruptSink>) -> usize {
        let idx = self.controllers.len();
        self.controllers.push(controller);
        idx
    }

    /// Add a routing entry.
    ///
    /// Does not validate that `route.dest_controller` is in range -- that is
    /// checked at lookup time by [`route()`](Self::route).
    pub fn add_route(&mut self, route: IrqRoute) {
        self.routes.push(route);
    }

    /// Look up the destination for a given `(source_device, source_line)` pair.
    ///
    /// Returns `Some((sink, dest_irq))` if a matching route exists and the
    /// destination controller index is valid. Returns `None` otherwise.
    pub fn route(
        &self,
        source_device: u64,
        source_line: u32,
    ) -> Option<(Arc<dyn InterruptSink>, u32)> {
        self.routes
            .iter()
            .find(|r| r.source_device == source_device && r.source_line == source_line)
            .and_then(|r| {
                self.controllers
                    .get(r.dest_controller)
                    .map(|ctrl| (Arc::clone(ctrl), r.dest_irq))
            })
    }
}

impl Default for IrqRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::interrupt::WireId;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Minimal test sink for routing tests.
    struct CountingSink {
        assert_count: AtomicU32,
    }

    impl CountingSink {
        fn new() -> Self {
            Self {
                assert_count: AtomicU32::new(0),
            }
        }
    }

    impl InterruptSink for CountingSink {
        fn on_assert(&self, _wire_id: WireId) {
            self.assert_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_deassert(&self, _wire_id: WireId) {}
    }

    #[test]
    fn empty_router_returns_none() {
        let router = IrqRouter::new();
        assert!(router.route(0, 0).is_none());
    }

    #[test]
    fn add_controller_returns_sequential_indices() {
        let mut router = IrqRouter::new();
        let a = router.add_controller(Arc::new(CountingSink::new()));
        let b = router.add_controller(Arc::new(CountingSink::new()));
        assert_eq!(a, 0);
        assert_eq!(b, 1);
    }

    #[test]
    fn route_finds_matching_entry() {
        let mut router = IrqRouter::new();
        let ctrl_idx = router.add_controller(Arc::new(CountingSink::new()));
        router.add_route(IrqRoute {
            source_device: 42,
            source_line: 0,
            dest_controller: ctrl_idx,
            dest_irq: 10,
        });

        let result = router.route(42, 0);
        assert!(result.is_some());
        let (_, irq) = result.unwrap();
        assert_eq!(irq, 10);
    }

    #[test]
    fn route_returns_none_for_unknown_source() {
        let mut router = IrqRouter::new();
        let ctrl_idx = router.add_controller(Arc::new(CountingSink::new()));
        router.add_route(IrqRoute {
            source_device: 42,
            source_line: 0,
            dest_controller: ctrl_idx,
            dest_irq: 10,
        });

        assert!(router.route(99, 0).is_none());
        assert!(router.route(42, 1).is_none());
    }

    #[test]
    fn route_returns_none_for_bad_controller_index() {
        let mut router = IrqRouter::new();
        // No controllers added, but route references index 0.
        router.add_route(IrqRoute {
            source_device: 1,
            source_line: 0,
            dest_controller: 0,
            dest_irq: 5,
        });
        assert!(router.route(1, 0).is_none());
    }

    #[test]
    fn multiple_routes_to_same_controller() {
        let mut router = IrqRouter::new();
        let ctrl = router.add_controller(Arc::new(CountingSink::new()));

        router.add_route(IrqRoute {
            source_device: 1,
            source_line: 0,
            dest_controller: ctrl,
            dest_irq: 10,
        });
        router.add_route(IrqRoute {
            source_device: 2,
            source_line: 0,
            dest_controller: ctrl,
            dest_irq: 11,
        });

        let (_, irq1) = router.route(1, 0).unwrap();
        let (_, irq2) = router.route(2, 0).unwrap();
        assert_eq!(irq1, 10);
        assert_eq!(irq2, 11);
    }

    #[test]
    fn routes_to_different_controllers() {
        let mut router = IrqRouter::new();
        let ctrl_a = router.add_controller(Arc::new(CountingSink::new()));
        let ctrl_b = router.add_controller(Arc::new(CountingSink::new()));

        router.add_route(IrqRoute {
            source_device: 1,
            source_line: 0,
            dest_controller: ctrl_a,
            dest_irq: 5,
        });
        router.add_route(IrqRoute {
            source_device: 2,
            source_line: 0,
            dest_controller: ctrl_b,
            dest_irq: 8,
        });

        let (_, irq1) = router.route(1, 0).unwrap();
        let (_, irq2) = router.route(2, 0).unwrap();
        assert_eq!(irq1, 5);
        assert_eq!(irq2, 8);
    }

    #[test]
    fn default_creates_empty_router() {
        let router = IrqRouter::default();
        assert!(router.route(0, 0).is_none());
    }
}
