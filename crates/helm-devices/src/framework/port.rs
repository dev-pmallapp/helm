//! Typed port abstraction for inter-device connections.
//!
//! Ports are declared by devices and resolved during `elaborate()`.
//! A [`Port<T>`] holds an optional `Arc<T>` that is set when the platform
//! wires two devices together. Before elaborate, the port is empty;
//! after elaborate, it holds a reference to the connected peer.

use std::sync::Arc;

/// A typed port that holds an optional connection to a peer of type `T`.
///
/// Devices declare ports in their struct and the platform fills them
/// during `elaborate()` by calling [`connect()`](Port::connect).
///
/// # Example
///
/// ```ignore
/// struct MyDevice {
///     upstream: Port<dyn MemInterface>,
/// }
///
/// // During elaborate:
/// my_device.upstream.connect(mem_arc);
/// ```
pub struct Port<T: ?Sized> {
    inner: Option<Arc<T>>,
}

impl<T: ?Sized> Port<T> {
    /// Create an unconnected port.
    pub fn new() -> Self {
        Self { inner: None }
    }

    /// Connect this port to a peer.
    ///
    /// # Panics
    ///
    /// Panics if the port is already connected (double-wiring is a
    /// configuration error that should be caught during elaborate).
    pub fn connect(&mut self, peer: Arc<T>) {
        assert!(
            self.inner.is_none(),
            "Port::connect() called on already-connected port"
        );
        self.inner = Some(peer);
    }

    /// Get a reference to the connected peer, if any.
    pub fn get(&self) -> Option<&Arc<T>> {
        self.inner.as_ref()
    }

    /// Get a reference to the connected peer.
    ///
    /// # Panics
    ///
    /// Panics if the port is not connected. Use after `elaborate()` only.
    pub fn connected(&self) -> &Arc<T> {
        self.inner
            .as_ref()
            .expect("Port::connected() called on unconnected port -- was elaborate() called?")
    }

    /// Returns `true` if this port has been connected to a peer.
    pub fn is_connected(&self) -> bool {
        self.inner.is_some()
    }
}

impl<T: ?Sized> Default for Port<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for types that expose a typed port.
///
/// Implemented by devices that need to advertise a connectable port
/// to the platform wiring layer.
pub trait Connect<T: ?Sized> {
    /// Return a reference to the device's port of type `T`.
    fn port(&self) -> &Port<T>;
}
