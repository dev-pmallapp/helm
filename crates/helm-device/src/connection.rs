//! Type-safe device connections — Simics-inspired attribute-backed slots.
//!
//! A [`Connection<I>`] is a typed slot that holds a reference to a backend
//! implementing a [`DeviceInterface`]. Devices declare their connections at
//! construction time and the platform builder wires them up.
//!
//! ```text
//! let mut conn: Connection<dyn CharBackend> = Connection::new("serial0");
//! conn.connect(Box::new(BufferCharBackend::new()))?;
//! conn.try_get_mut().unwrap().write(b"hello")?;
//! conn.disconnect();
//! ```

use std::fmt;

/// Marker trait for connectable device interfaces.
///
/// Implemented by backend trait objects (`dyn CharBackend`, `dyn BlockBackend`,
/// `dyn NetBackend`). The `interface_name` method provides runtime type
/// identification for diagnostics and configuration.
pub trait DeviceInterface {
    /// Human-readable interface type name (e.g. "char", "block", "net").
    fn interface_name() -> &'static str
    where
        Self: Sized;
}

// ── DeviceInterface impls for existing backend traits ────────────────────────

#[allow(dead_code)]
impl DeviceInterface for dyn crate::backend::CharBackend {
    fn interface_name() -> &'static str {
        "char"
    }
}

#[allow(dead_code)]
impl DeviceInterface for dyn crate::backend::BlockBackend {
    fn interface_name() -> &'static str {
        "block"
    }
}

#[allow(dead_code)]
impl DeviceInterface for dyn crate::backend::NetBackend {
    fn interface_name() -> &'static str {
        "net"
    }
}

/// Error returned when a connection operation fails.
#[derive(Debug)]
pub enum ConnectionError {
    /// Tried to connect to a non-hotplug slot that already has a backend.
    AlreadyConnected { slot_name: String },
    /// Tried to disconnect from an empty slot.
    NotConnected { slot_name: String },
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyConnected { slot_name } => {
                write!(f, "connection '{slot_name}' already connected (not hotpluggable)")
            }
            Self::NotConnected { slot_name } => {
                write!(f, "connection '{slot_name}' is not connected")
            }
        }
    }
}

impl std::error::Error for ConnectionError {}

/// A typed slot that holds an optional backend implementing interface `I`.
///
/// - `hotplug: true` — allows disconnect + reconnect at any time
/// - `hotplug: false` — once connected, a second `connect()` fails
pub struct Connection<I: ?Sized> {
    name: String,
    backend: Option<Box<I>>,
    hotplug: bool,
}

impl<I: ?Sized> Connection<I> {
    /// Create a new empty connection slot.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            backend: None,
            hotplug: false,
        }
    }

    /// Create a hotpluggable connection slot.
    pub fn hotpluggable(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            backend: None,
            hotplug: true,
        }
    }

    /// Whether this slot allows hot-swap.
    pub fn is_hotplug(&self) -> bool {
        self.hotplug
    }

    /// Slot name for diagnostics.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Connect a backend to this slot.
    ///
    /// Returns `Err` if the slot is already connected and not hotpluggable.
    pub fn connect(&mut self, backend: Box<I>) -> Result<(), ConnectionError> {
        if self.backend.is_some() && !self.hotplug {
            return Err(ConnectionError::AlreadyConnected {
                slot_name: self.name.clone(),
            });
        }
        self.backend = Some(backend);
        Ok(())
    }

    /// Disconnect the current backend, returning it.
    pub fn disconnect(&mut self) -> Option<Box<I>> {
        self.backend.take()
    }

    /// Get a shared reference to the connected backend, if any.
    pub fn try_get(&self) -> Option<&I> {
        self.backend.as_deref()
    }

    /// Get a mutable reference to the connected backend, if any.
    pub fn try_get_mut(&mut self) -> Option<&mut I> {
        self.backend.as_deref_mut()
    }

    /// Whether a backend is currently connected.
    pub fn is_connected(&self) -> bool {
        self.backend.is_some()
    }
}

impl<I: ?Sized> fmt::Debug for Connection<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("name", &self.name)
            .field("connected", &self.backend.is_some())
            .field("hotplug", &self.hotplug)
            .finish()
    }
}
