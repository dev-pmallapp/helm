use crate::backend::{BufferCharBackend, CharBackend, NullCharBackend};
use crate::connection::{Connection, ConnectionError};

#[test]
fn connect_and_use() {
    let mut conn: Connection<dyn CharBackend> = Connection::new("serial0");
    assert!(!conn.is_connected());

    let backend = BufferCharBackend::new();
    conn.connect(Box::new(backend)).unwrap();
    assert!(conn.is_connected());

    // Write through the connection
    let be = conn.try_get_mut().unwrap();
    be.write(b"hello").unwrap();
}

#[test]
fn disconnect_returns_none() {
    let mut conn: Connection<dyn CharBackend> = Connection::new("serial0");
    assert!(conn.try_get().is_none());

    conn.connect(Box::new(NullCharBackend)).unwrap();
    assert!(conn.try_get().is_some());

    let old = conn.disconnect();
    assert!(old.is_some());
    assert!(conn.try_get().is_none());
}

#[test]
fn hotplug_swap() {
    let mut conn: Connection<dyn CharBackend> = Connection::hotpluggable("serial0");
    assert!(conn.is_hotplug());

    // First connection
    conn.connect(Box::new(NullCharBackend)).unwrap();
    assert!(conn.is_connected());

    // Disconnect
    conn.disconnect();

    // Reconnect with a different backend
    let mut buf = BufferCharBackend::new();
    buf.inject(b"data");
    conn.connect(Box::new(buf)).unwrap();
    assert!(conn.is_connected());
    assert!(conn.try_get().unwrap().can_read());
}

#[test]
fn non_hotplug_rejects_reconnect() {
    let mut conn: Connection<dyn CharBackend> = Connection::new("serial0");
    assert!(!conn.is_hotplug());

    conn.connect(Box::new(NullCharBackend)).unwrap();

    // Second connect should fail
    let result = conn.connect(Box::new(NullCharBackend));
    assert!(matches!(result, Err(ConnectionError::AlreadyConnected { .. })));
}

#[test]
fn hotplug_allows_overwrite() {
    let mut conn: Connection<dyn CharBackend> = Connection::hotpluggable("serial0");

    conn.connect(Box::new(NullCharBackend)).unwrap();
    // Overwrite without disconnect — allowed for hotplug
    conn.connect(Box::new(BufferCharBackend::new())).unwrap();
    assert!(conn.is_connected());
}

#[test]
fn debug_impl() {
    let conn: Connection<dyn CharBackend> = Connection::new("test");
    let dbg = format!("{:?}", conn);
    assert!(dbg.contains("test"));
    assert!(dbg.contains("false")); // connected = false
}

#[test]
fn name_accessor() {
    let conn: Connection<dyn CharBackend> = Connection::new("my-slot");
    assert_eq!(conn.name(), "my-slot");
}
