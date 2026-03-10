//! Dynamic plugin loading via shared libraries (`.so` / `.dylib`).
//!
//! Plugin authors build a `cdylib` crate that exports a single C-ABI
//! entry point:
//!
//! ```ignore
//! #[no_mangle]
//! pub extern "C" fn helm_plugin_entry() -> *const HelmPluginVTable {
//!     // return a pointer to a static vtable
//! }
//! ```
//!
//! The [`DynamicPluginLoader`] opens the library, verifies the API
//! version from [`PluginMetadata`], and calls the factory to obtain
//! component instances.
//!
//! Currently unix-only (uses `dlopen`/`dlsym`).  A future version
//! may add `libloading` for cross-platform support.

use crate::component::{ComponentInfo, HelmComponent};
use crate::loader::ComponentRegistry;
use crate::{PluginMetadata, PLUGIN_API_VERSION};
use std::ffi::CString;
use std::path::Path;

/// Errors specific to dynamic plugin loading.
#[derive(Debug)]
pub enum DynLoadError {
    /// The shared library could not be opened.
    LibraryOpen(String),
    /// The required symbol was not found in the library.
    SymbolNotFound(String),
    /// The plugin was compiled against an incompatible API version.
    VersionMismatch {
        plugin_name: String,
        expected: u32,
        found: u32,
    },
    /// The plugin's entry point returned a null pointer.
    NullVTable,
}

impl std::fmt::Display for DynLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LibraryOpen(msg) => write!(f, "failed to open plugin library: {msg}"),
            Self::SymbolNotFound(sym) => write!(f, "symbol not found: {sym}"),
            Self::VersionMismatch {
                plugin_name,
                expected,
                found,
            } => write!(
                f,
                "plugin '{plugin_name}' API version mismatch: expected {expected}, found {found}"
            ),
            Self::NullVTable => write!(f, "plugin entry point returned null"),
        }
    }
}

impl std::error::Error for DynLoadError {}

/// C-ABI vtable that a plugin shared library exports.
///
/// The plugin library must export a function
/// `helm_plugin_entry() -> *const HelmPluginVTable`
/// that returns a pointer to a **static** vtable.
#[repr(C)]
pub struct HelmPluginVTable {
    /// Plugin metadata (name, version, API version, etc.).
    pub metadata: PluginMetadata,
    /// Factory that creates a new boxed component instance.
    /// The caller owns the returned pointer and must eventually drop it
    /// via `Box::from_raw`.
    #[allow(improper_ctypes_definitions)]
    pub create: extern "C" fn() -> *mut dyn HelmComponent,
}

// SAFETY: The vtable is a static, read-only struct.
unsafe impl Send for HelmPluginVTable {}
unsafe impl Sync for HelmPluginVTable {}

/// Name of the C symbol plugin libraries must export.
pub const ENTRY_SYMBOL: &str = "helm_plugin_entry";

/// Entry-point function signature.
type EntryFn = unsafe extern "C" fn() -> *const HelmPluginVTable;

/// Thin wrapper around a `dlopen`-ed handle.
struct LibHandle {
    handle: *mut std::ffi::c_void,
}

// SAFETY: The handle is only used behind &self and closed on drop.
unsafe impl Send for LibHandle {}
unsafe impl Sync for LibHandle {}

impl LibHandle {
    /// Open a shared library by path.
    unsafe fn open(path: &str) -> Result<Self, DynLoadError> {
        let c_path =
            CString::new(path).map_err(|_| DynLoadError::LibraryOpen("invalid path".into()))?;
        let handle = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            let err = unsafe { std::ffi::CStr::from_ptr(libc::dlerror()) };
            return Err(DynLoadError::LibraryOpen(
                err.to_string_lossy().into_owned(),
            ));
        }
        Ok(Self { handle })
    }

    /// Look up a symbol.
    unsafe fn sym<T>(&self, name: &str) -> Result<T, DynLoadError> {
        let c_name = CString::new(name)
            .map_err(|_| DynLoadError::SymbolNotFound("invalid symbol name".into()))?;
        let ptr = unsafe { libc::dlsym(self.handle, c_name.as_ptr()) };
        if ptr.is_null() {
            return Err(DynLoadError::SymbolNotFound(name.into()));
        }
        Ok(unsafe { std::mem::transmute_copy(&ptr) })
    }
}

impl Drop for LibHandle {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                libc::dlclose(self.handle);
            }
        }
    }
}

/// Loads plugin shared libraries and registers their components.
///
/// Keeps loaded libraries alive for the lifetime of the loader to
/// prevent use-after-free of vtable pointers.
pub struct DynamicPluginLoader {
    _libraries: Vec<LibHandle>,
    loaded: Vec<LoadedPluginInfo>,
}

/// Info about a successfully loaded dynamic plugin.
#[derive(Debug, Clone)]
pub struct LoadedPluginInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub path: String,
}

impl DynamicPluginLoader {
    pub fn new() -> Self {
        Self {
            _libraries: Vec::new(),
            loaded: Vec::new(),
        }
    }

    /// Load a plugin from a shared library path and register it in the
    /// given [`ComponentRegistry`].
    ///
    /// # Safety
    /// Loading arbitrary shared libraries is inherently unsafe.  The
    /// caller must ensure the library at `path` is a valid HELM plugin
    /// compiled against a compatible API version.
    pub unsafe fn load(
        &mut self,
        path: impl AsRef<Path>,
        registry: &mut ComponentRegistry,
    ) -> Result<LoadedPluginInfo, DynLoadError> {
        let path_str = path.as_ref().display().to_string();

        let lib = unsafe { LibHandle::open(&path_str)? };

        let entry: EntryFn = unsafe { lib.sym(ENTRY_SYMBOL)? };

        let vtable_ptr = unsafe { entry() };
        if vtable_ptr.is_null() {
            return Err(DynLoadError::NullVTable);
        }
        let vtable: &'static HelmPluginVTable = unsafe { &*vtable_ptr };

        // API version check
        if vtable.metadata.api_version != PLUGIN_API_VERSION {
            return Err(DynLoadError::VersionMismatch {
                plugin_name: vtable.metadata.name.to_string(),
                expected: PLUGIN_API_VERSION,
                found: vtable.metadata.api_version,
            });
        }

        let info = LoadedPluginInfo {
            name: vtable.metadata.name.to_string(),
            version: vtable.metadata.version.to_string(),
            description: vtable.metadata.description.to_string(),
            author: vtable.metadata.author.to_string(),
            path: path_str,
        };

        // Register the component factory
        let type_name: &'static str =
            Box::leak(format!("dynamic.{}", vtable.metadata.name).into_boxed_str());
        let desc: &'static str =
            Box::leak(vtable.metadata.description.to_string().into_boxed_str());

        let create_fn = vtable.create;

        registry.register(ComponentInfo {
            type_name,
            description: desc,
            interfaces: &["dynamic"],
            factory: Box::new(move || {
                let raw = create_fn();
                assert!(!raw.is_null(), "plugin factory returned null");
                unsafe { Box::from_raw(raw) }
            }),
        });

        self._libraries.push(lib);
        self.loaded.push(info.clone());

        log::info!(
            "loaded dynamic plugin '{}' v{} from {}",
            info.name,
            info.version,
            info.path
        );

        Ok(info)
    }

    /// List all successfully loaded plugins.
    pub fn loaded_plugins(&self) -> &[LoadedPluginInfo] {
        &self.loaded
    }

    /// Number of loaded plugin libraries.
    pub fn count(&self) -> usize {
        self._libraries.len()
    }
}

impl Default for DynamicPluginLoader {
    fn default() -> Self {
        Self::new()
    }
}
