use crate::dynamic::*;
use crate::loader::ComponentRegistry;

#[test]
fn loader_starts_empty() {
    let loader = DynamicPluginLoader::new();
    assert_eq!(loader.count(), 0);
    assert!(loader.loaded_plugins().is_empty());
}

#[test]
fn load_nonexistent_library_fails() {
    let mut loader = DynamicPluginLoader::new();
    let mut reg = ComponentRegistry::new();
    let result = unsafe { loader.load("/tmp/nonexistent_helm_plugin.so", &mut reg) };
    assert!(result.is_err());
    match result.unwrap_err() {
        DynLoadError::LibraryOpen(_) => {} // expected
        other => panic!("expected LibraryOpen, got: {other}"),
    }
}

#[test]
fn load_library_without_symbol_fails() {
    // libc.so exists on all Linux systems and won't have our symbol
    let mut loader = DynamicPluginLoader::new();
    let mut reg = ComponentRegistry::new();
    let result = unsafe { loader.load("libc.so.6", &mut reg) };
    assert!(result.is_err());
    match result.unwrap_err() {
        DynLoadError::SymbolNotFound(sym) => {
            assert_eq!(sym, ENTRY_SYMBOL);
        }
        DynLoadError::LibraryOpen(_) => {} // acceptable on some systems
        other => panic!("expected SymbolNotFound, got: {other}"),
    }
}

#[test]
fn dyn_load_error_display() {
    let e = DynLoadError::VersionMismatch {
        plugin_name: "test".into(),
        expected: 1,
        found: 99,
    };
    let msg = format!("{e}");
    assert!(msg.contains("test"));
    assert!(msg.contains("99"));

    let e2 = DynLoadError::NullVTable;
    assert!(format!("{e2}").contains("null"));
}
