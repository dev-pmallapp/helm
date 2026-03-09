# Dynamic Module Loading and Executable Generation for HELM

**Date:** March 4, 2026  
**Version:** 1.0

## Executive Summary

This document describes HELM's plugin architecture for dynamic device/component loading (inspired by Simics) and the ability to generate standalone platform-specific executables from Python configurations. This approach combines the flexibility of dynamic composition with the performance and deployment simplicity of static linking when needed.

---

## Table of Contents

1. [Overview](#overview)
2. [Dynamic Module Architecture](#dynamic-module-architecture)
3. [Plugin Development](#plugin-development)
4. [Module Registration and Discovery](#module-registration-and-discovery)
5. [Executable Generation](#executable-generation)
6. [Use Cases](#use-cases)
7. [Implementation Details](#implementation-details)

---

## 1. Overview

### 1.1 Design Goals

HELM should support two deployment modes:

**Dynamic Mode (Development & Flexibility):**
- Load component implementations as shared libraries (`.so`/`.dylib`/`.dll`)
- Hot-reload models during development
- Mix-and-match components from different sources
- Keep core simulator lightweight

**Static Mode (Deployment & Performance):**
- Generate standalone executables from Python platform definitions
- Embed all required components at compile time
- Optimize with LTO and PGO for maximum performance
- Single binary deployment (e.g., `helm-rpi3`)

### 1.2 Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                     HELM Core Simulator                      │
│  ┌────────────────┐  ┌────────────────┐  ┌───────────────┐ │
│  │  Module Loader │  │ Object Registry│  │  ABI Checker  │ │
│  └────────────────┘  └────────────────┘  └───────────────┘ │
└─────────────────────────────────────────────────────────────┘
                              │
                ┌─────────────┼──────────────┐
                │             │              │
         ┌──────▼──────┐ ┌───▼────────┐ ┌──▼─────────────┐
         │ Plugin API  │ │ Static API │ │ Python Bridge  │
         └──────┬──────┘ └───┬────────┘ └──┬─────────────┘
                │            │              │
    ┌───────────┼────────────┼──────────────┼──────────┐
    │           │            │              │          │
┌───▼───┐  ┌───▼───┐   ┌────▼────┐    ┌───▼──────┐   │
│ ARM   │  │RISC-V │   │  x86    │    │ Platform │   │
│ Core  │  │ Core  │   │  Core   │    │ Builder  │   │
│(.so)  │  │(.so)  │   │ (static)│    │          │   │
└───────┘  └───────┘   └─────────┘    └──────────┘   │
                                                      │
┌───────────┐  ┌────────────┐  ┌──────────────┐     │
│TAGE Pred  │  │ L1 Cache   │  │ DDR4 Ctrl    │     │
│  (.so)    │  │  (static)  │  │   (.so)      │     │
└───────────┘  └────────────┘  └──────────────┘     │
                                                     │
                                      ┌──────────────▼────┐
                                      │  helm-rpi3.py     │
                                      │  ↓ (code-gen)     │
                                      │  helm-rpi3 (exe)  │
                                      └───────────────────┘
```

---

## 2. Dynamic Module Architecture

### 2.1 Rust Dynamic Libraries (cdylib)

HELM plugins are Rust crates compiled as C-compatible dynamic libraries:

```toml
# helm-plugin-arm-cortex-a72/Cargo.toml
[package]
name = "helm-plugin-arm-cortex-a72"
version = "0.1.0"

[lib]
crate-type = ["cdylib"]  # Dynamic library

[dependencies]
helm-plugin-api = "0.1"  # Stable plugin API
```

### 2.2 Plugin Entry Point

Every plugin must export a well-known entry point:

```rust
// helm-plugin-arm-cortex-a72/src/lib.rs
use helm_plugin_api::*;

// Plugin metadata
#[no_mangle]
pub static HELM_PLUGIN_METADATA: PluginMetadata = PluginMetadata {
    api_version: HELM_PLUGIN_API_VERSION,
    plugin_version: "0.1.0",
    name: "arm-cortex-a72",
    description: "ARM Cortex-A72 core implementation",
    author: "HELM Contributors",
    components: &["core.arm.cortex-a72"],
};

// Plugin initialization
#[no_mangle]
pub extern "C" fn helm_plugin_init(registry: &mut dyn ComponentRegistry) -> Result<(), PluginError> {
    // Register component types
    registry.register_component(
        "core.arm.cortex-a72",
        ComponentInfo {
            constructor: Box::new(|| Box::new(CortexA72Core::new())),
            properties: cortex_a72_properties(),
            interfaces: &["core", "arm-core", "cycle-accurate"],
        },
    )?;
    
    Ok(())
}

// Component implementation
pub struct CortexA72Core {
    // Core state
    rob_size: usize,
    issue_width: usize,
    // ... microarchitectural state
}

impl HelmComponent for CortexA72Core {
    fn component_type(&self) -> &'static str {
        "core.arm.cortex-a72"
    }
    
    fn reset(&mut self) -> Result<()> {
        // Reset core state
        Ok(())
    }
    
    fn step(&mut self, cycles: u64) -> Result<StepResult> {
        // Execute simulation step
        Ok(StepResult::new(cycles))
    }
    
    // ... other component methods
}
```

### 2.3 ABI Stability

HELM defines a stable plugin API with version checking:

```rust
// helm-plugin-api/src/lib.rs

/// Plugin API version - increment on breaking changes
pub const HELM_PLUGIN_API_VERSION: u32 = 1;

/// Checked at plugin load time
#[repr(C)]
pub struct PluginMetadata {
    pub api_version: u32,
    pub plugin_version: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub author: &'static str,
    pub components: &'static [&'static str],
}

/// Core plugin trait (FFI-safe)
pub trait ComponentRegistry {
    fn register_component(
        &mut self,
        type_name: &str,
        info: ComponentInfo,
    ) -> Result<(), PluginError>;
}

/// Component factory and metadata
pub struct ComponentInfo {
    pub constructor: Box<dyn Fn() -> Box<dyn HelmComponent>>,
    pub properties: Vec<PropertyDescriptor>,
    pub interfaces: &'static [&'static str],
}
```

### 2.4 Module Loading

The HELM core provides a dynamic loader:

```rust
// helm-engine/src/module_loader.rs
use libloading::{Library, Symbol};

pub struct ModuleLoader {
    search_paths: Vec<PathBuf>,
    loaded_modules: HashMap<String, LoadedModule>,
}

struct LoadedModule {
    library: Library,
    metadata: PluginMetadata,
    components: Vec<String>,
}

impl ModuleLoader {
    pub fn load_module(&mut self, name: &str) -> Result<&LoadedModule> {
        // Check if already loaded
        if let Some(module) = self.loaded_modules.get(name) {
            return Ok(module);
        }
        
        // Find module file
        let lib_path = self.find_module(name)?;
        
        // Load dynamic library
        let library = unsafe { Library::new(&lib_path)? };
        
        // Get metadata
        let metadata: Symbol<&PluginMetadata> = unsafe {
            library.get(b"HELM_PLUGIN_METADATA")?
        };
        
        // Verify API version
        if metadata.api_version != HELM_PLUGIN_API_VERSION {
            return Err(PluginError::ApiVersionMismatch {
                expected: HELM_PLUGIN_API_VERSION,
                found: metadata.api_version,
            });
        }
        
        // Call init function
        let init_fn: Symbol<extern "C" fn(&mut dyn ComponentRegistry) -> Result<(), PluginError>> =
            unsafe { library.get(b"helm_plugin_init")? };
        
        init_fn(&mut self.registry)?;
        
        // Store loaded module
        let module = LoadedModule {
            library,
            metadata: **metadata,
            components: metadata.components.iter().map(|s| s.to_string()).collect(),
        };
        
        self.loaded_modules.insert(name.to_string(), module);
        Ok(self.loaded_modules.get(name).unwrap())
    }
    
    fn find_module(&self, name: &str) -> Result<PathBuf> {
        let lib_name = format!("libhelm_plugin_{}", name.replace('-', "_"));
        let lib_file = format!("{}{}", lib_name, std::env::consts::DLL_SUFFIX);
        
        for path in &self.search_paths {
            let full_path = path.join(&lib_file);
            if full_path.exists() {
                return Ok(full_path);
            }
        }
        
        Err(PluginError::ModuleNotFound(name.to_string()))
    }
    
    pub fn add_search_path(&mut self, path: impl Into<PathBuf>) {
        self.search_paths.push(path.into());
    }
}
```

### 2.5 Hot Reload Support

For development workflows:

```rust
impl ModuleLoader {
    pub fn reload_module(&mut self, name: &str) -> Result<()> {
        // Unload existing instances (requires component lifecycle management)
        self.unload_module(name)?;
        
        // Reload from disk
        self.load_module(name)?;
        
        Ok(())
    }
    
    pub fn watch_for_changes(&mut self) -> Result<()> {
        // Use notify crate to watch plugin directories
        // Automatically reload when .so files change
        // Useful for iterative development
        todo!()
    }
}
```

---

## 3. Plugin Development

### 3.1 Creating a Plugin

**Step 1: Create plugin crate**

```bash
cargo new --lib helm-plugin-my-device
cd helm-plugin-my-device
```

**Step 2: Configure Cargo.toml**

```toml
[package]
name = "helm-plugin-my-device"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
helm-plugin-api = { path = "../helm-plugin-api" }
anyhow = "1.0"
```

**Step 3: Implement plugin**

```rust
use helm_plugin_api::*;

#[no_mangle]
pub static HELM_PLUGIN_METADATA: PluginMetadata = PluginMetadata {
    api_version: HELM_PLUGIN_API_VERSION,
    plugin_version: "0.1.0",
    name: "my-device",
    description: "Custom device model",
    author: "Your Name",
    components: &["device.custom.my-device"],
};

#[no_mangle]
pub extern "C" fn helm_plugin_init(registry: &mut dyn ComponentRegistry) -> Result<(), PluginError> {
    registry.register_component(
        "device.custom.my-device",
        ComponentInfo {
            constructor: Box::new(|| Box::new(MyDevice::default())),
            properties: my_device_properties(),
            interfaces: &["memory-mapped", "interrupt-source"],
        },
    )
}

#[derive(Default)]
pub struct MyDevice {
    base_address: u64,
    interrupt_line: u32,
    // ... device state
}

impl HelmComponent for MyDevice {
    // Implement required methods
}
```

**Step 4: Build and install**

```bash
cargo build --release
cp target/release/libhelm_plugin_my_device.so ~/.helm/plugins/
```

**Step 5: Use in Python**

```python
from helm import Platform

platform = Platform()

# HELM automatically loads plugin when component is referenced
platform.add_device(
    type="device.custom.my-device",
    name="my_dev0",
    base_address=0x10000000,
    interrupt_line=42
)
```

### 3.2 Plugin Development Macro

Provide a convenience macro to reduce boilerplate:

```rust
// In helm-plugin-api
use helm_plugin_macro::helm_plugin;

#[helm_plugin(
    name = "my-device",
    version = "0.1.0",
    author = "Your Name",
    description = "Custom device model"
)]
pub struct MyDevicePlugin;

impl PluginInit for MyDevicePlugin {
    fn register_components(registry: &mut dyn ComponentRegistry) -> Result<()> {
        registry.register::<MyDevice>("device.custom.my-device")?;
        Ok(())
    }
}

// Macro generates all the boilerplate:
// - HELM_PLUGIN_METADATA static
// - helm_plugin_init extern "C" function
// - ComponentInfo wrappers
```

---

## 4. Module Registration and Discovery

### 4.1 Plugin Manifest

Each plugin directory contains a manifest:

```toml
# ~/.helm/plugins/arm-cortex-a72/plugin.toml
[plugin]
name = "arm-cortex-a72"
version = "0.1.0"
author = "HELM Contributors"
library = "libhelm_plugin_arm_cortex_a72.so"

[components]
"core.arm.cortex-a72" = { interfaces = ["core", "arm-core"] }

[dependencies]
helm-plugin-api = ">=0.1"
```

### 4.2 Component Discovery

```rust
pub struct ComponentRegistry {
    registered: HashMap<String, RegisteredComponent>,
    plugins: HashMap<String, PluginInfo>,
}

impl ComponentRegistry {
    pub fn discover_plugins(&mut self) -> Result<()> {
        for plugin_dir in self.plugin_search_paths() {
            for entry in fs::read_dir(plugin_dir)? {
                let path = entry?.path();
                if path.join("plugin.toml").exists() {
                    self.register_plugin_from_manifest(&path)?;
                }
            }
        }
        Ok(())
    }
    
    pub fn create_component(&self, type_name: &str) -> Result<Box<dyn HelmComponent>> {
        let component = self.registered.get(type_name)
            .ok_or_else(|| anyhow!("Unknown component type: {}", type_name))?;
        
        // Lazy-load plugin if needed
        if !component.plugin_loaded {
            self.load_plugin(&component.plugin_name)?;
        }
        
        // Call constructor
        (component.constructor)()
    }
    
    pub fn list_components(&self) -> Vec<ComponentDescriptor> {
        self.registered.values()
            .map(|c| ComponentDescriptor {
                type_name: c.type_name.clone(),
                plugin: c.plugin_name.clone(),
                interfaces: c.interfaces.clone(),
                properties: c.properties.clone(),
            })
            .collect()
    }
}
```

### 4.3 Python Integration

```python
# helm/__init__.py
import helm_core  # PyO3 bindings

class PluginManager:
    def __init__(self):
        self._core = helm_core.PluginManager()
    
    def discover_plugins(self):
        """Scan for available plugins"""
        return self._core.discover_plugins()
    
    def list_components(self):
        """List all registered component types"""
        return self._core.list_components()
    
    def install_plugin(self, url_or_path: str):
        """Install a plugin from URL or local path"""
        # Download/copy plugin to ~/.helm/plugins/
        # Verify signature (optional)
        # Register with core
        pass

# Usage
from helm import PluginManager

pm = PluginManager()
pm.discover_plugins()

for component in pm.list_components():
    print(f"{component.type_name}: {component.description}")
```

---

## 5. Executable Generation

### 5.1 From Python to Standalone Binary

The goal: Take a Python platform definition (e.g., `rpi3.py`) and generate a standalone executable (`helm-rpi3`) that includes all necessary components statically linked.

### 5.2 Build System Integration

**Architecture:**

```
  rpi3.py (Python config)
      │
      ▼
  helm-build tool (analyzes dependencies)
      │
      ├─> Generates main.rs (platform initialization code)
      ├─> Creates Cargo.toml (with all required components)
      │
      ▼
  cargo build --release
      │
      ▼
  helm-rpi3 (standalone executable)
```

### 5.3 Implementation

#### 5.3.1 Python Platform Definition

```python
# platforms/rpi3.py
from helm import Platform, ARMCore, CacheHierarchy, MemoryController, Device

def create_platform():
    platform = Platform(name="raspberry-pi-3")
    
    # 4x ARM Cortex-A53 cores
    for i in range(4):
        platform.add_core(
            ARMCore(
                type="cortex-a53",
                name=f"cpu{i}",
                frequency="1.2GHz",
                l1i_cache="32KB",
                l1d_cache="32KB",
            )
        )
    
    # Shared L2 cache
    platform.add_cache(
        type="l2-cache",
        name="l2",
        size="512KB",
        associativity=16,
    )
    
    # Memory controller
    platform.add_memory_controller(
        type="lpddr2-sdram",
        size="1GB",
        frequency="900MHz",
    )
    
    # Peripherals
    platform.add_device(
        type="bcm2837-gpio",
        name="gpio",
        base_address=0x3F200000,
    )
    
    platform.add_device(
        type="pl011-uart",
        name="uart0",
        base_address=0x3F201000,
        interrupt=57,
    )
    
    # ... more devices
    
    return platform

if __name__ == "__main__":
    platform = create_platform()
    platform.run()
```

#### 5.3.2 Build Tool

```rust
// helm-build/src/main.rs
use std::process::Command;

struct PlatformAnalyzer {
    python_file: PathBuf,
}

impl PlatformAnalyzer {
    fn analyze(&self) -> Result<PlatformManifest> {
        // Run Python script in analysis mode
        let output = Command::new("python3")
            .arg(&self.python_file)
            .arg("--analyze")
            .output()?;
        
        let manifest: PlatformManifest = serde_json::from_slice(&output.stdout)?;
        Ok(manifest)
    }
}

struct PlatformManifest {
    name: String,
    components: Vec<ComponentReference>,
    memory_layout: MemoryLayout,
}

struct ComponentReference {
    type_name: String,
    instance_name: String,
    properties: HashMap<String, serde_json::Value>,
}

fn generate_executable(
    platform_file: &Path,
    output_name: &str,
) -> Result<()> {
    println!("Analyzing platform definition...");
    let analyzer = PlatformAnalyzer {
        python_file: platform_file.to_path_buf(),
    };
    let manifest = analyzer.analyze()?;
    
    println!("Generating Rust code...");
    let codegen = CodeGenerator::new(manifest);
    let build_dir = PathBuf::from(format!("target/platform-builds/{}", output_name));
    fs::create_dir_all(&build_dir)?;
    
    // Generate main.rs
    let main_rs = codegen.generate_main()?;
    fs::write(build_dir.join("main.rs"), main_rs)?;
    
    // Generate Cargo.toml
    let cargo_toml = codegen.generate_cargo_toml()?;
    fs::write(build_dir.join("Cargo.toml"), cargo_toml)?;
    
    println!("Building executable...");
    let status = Command::new("cargo")
        .current_dir(&build_dir)
        .args(&["build", "--release"])
        .status()?;
    
    if !status.success() {
        return Err(anyhow!("Build failed"));
    }
    
    // Copy to output
    let exe_name = if cfg!(windows) {
        format!("{}.exe", output_name)
    } else {
        output_name.to_string()
    };
    
    fs::copy(
        build_dir.join("target/release").join(&exe_name),
        exe_name,
    )?;
    
    println!("✓ Generated executable: {}", exe_name);
    Ok(())
}
```

#### 5.3.3 Code Generation

```rust
struct CodeGenerator {
    manifest: PlatformManifest,
}

impl CodeGenerator {
    fn generate_main(&self) -> Result<String> {
        let mut code = String::new();
        
        // Imports
        code.push_str("use helm_engine::*;\n");
        code.push_str("use helm_core::*;\n");
        
        for component in &self.manifest.components {
            let module = self.component_to_module(&component.type_name);
            code.push_str(&format!("use {}::*;\n", module));
        }
        
        code.push_str("\nfn main() -> Result<()> {\n");
        code.push_str("    let mut platform = Platform::new();\n\n");
        
        // Generate component instantiation
        for component in &self.manifest.components {
            code.push_str(&self.generate_component_creation(component)?);
        }
        
        code.push_str("\n    // Run simulation\n");
        code.push_str("    platform.run()?;\n");
        code.push_str("    Ok(())\n");
        code.push_str("}\n");
        
        Ok(code)
    }
    
    fn generate_component_creation(&self, comp: &ComponentReference) -> Result<String> {
        let mut code = String::new();
        
        code.push_str(&format!("    let {} = ", comp.instance_name));
        
        // Determine constructor based on type
        match comp.type_name.as_str() {
            "core.arm.cortex-a53" => {
                code.push_str("CortexA53Core::new()");
            }
            "cache.l2" => {
                code.push_str("L2Cache::new(");
                code.push_str(&format!("{})", self.format_properties(&comp.properties)?));
            }
            // ... more component types
            _ => {
                return Err(anyhow!("Unknown component type: {}", comp.type_name));
            }
        }
        
        code.push_str(";\n");
        
        // Set properties
        for (key, value) in &comp.properties {
            code.push_str(&format!(
                "    {}.set_property(\"{}\", {})?;\n",
                comp.instance_name,
                key,
                self.format_value(value)?
            ));
        }
        
        code.push_str(&format!(
            "    platform.add_component(\"{}\", {});\n\n",
            comp.instance_name, comp.instance_name
        ));
        
        Ok(code)
    }
    
    fn generate_cargo_toml(&self) -> Result<String> {
        let mut deps = vec![
            ("helm-engine", "0.1"),
            ("helm-core", "0.1"),
            ("anyhow", "1.0"),
        ];
        
        // Add dependencies for each component type
        for component in &self.manifest.components {
            let crate_name = self.component_to_crate(&component.type_name);
            if !deps.iter().any(|(name, _)| *name == crate_name) {
                deps.push((crate_name, "0.1"));
            }
        }
        
        let mut toml = format!(
            "[package]\n\
             name = \"{}\"\n\
             version = \"0.1.0\"\n\
             edition = \"2021\"\n\n\
             [[bin]]\n\
             name = \"{}\"\n\
             path = \"main.rs\"\n\n\
             [dependencies]\n",
            self.manifest.name,
            self.manifest.name
        );
        
        for (dep, version) in deps {
            toml.push_str(&format!("{} = \"{}\"\n", dep, version));
        }
        
        // Optimization settings
        toml.push_str("\n[profile.release]\n");
        toml.push_str("lto = true\n");
        toml.push_str("codegen-units = 1\n");
        toml.push_str("opt-level = 3\n");
        
        Ok(toml)
    }
}
```

#### 5.3.4 CLI Tool

```bash
# Build a platform-specific executable
helm build platforms/rpi3.py --output helm-rpi3

# With optimizations
helm build platforms/rpi3.py --output helm-rpi3 --profile pgo

# Cross-compile
helm build platforms/rpi3.py --output helm-rpi3 --target aarch64-unknown-linux-gnu
```

```rust
// helm-cli/src/commands/build.rs
pub fn build_command(args: BuildArgs) -> Result<()> {
    println!("HELM Platform Builder");
    println!("Platform: {}", args.platform_file.display());
    
    // Analyze Python platform definition
    let manifest = analyze_platform(&args.platform_file)?;
    println!("Components: {}", manifest.components.len());
    
    // Generate Rust code
    let codegen = CodeGenerator::new(manifest);
    let build_dir = prepare_build_directory(&args.output)?;
    codegen.generate(&build_dir)?;
    
    // Build executable
    let mut build_cmd = Command::new("cargo");
    build_cmd
        .current_dir(&build_dir)
        .args(&["build", "--release"]);
    
    if let Some(target) = &args.target {
        build_cmd.args(&["--target", target]);
    }
    
    if args.profile == "pgo" {
        // Profile-guided optimization
        build_pgo(&build_dir, &args)?;
    } else {
        build_cmd.status()?;
    }
    
    // Copy to output location
    install_executable(&build_dir, &args.output)?;
    
    println!("✓ Built: {}", args.output);
    Ok(())
}
```

### 5.4 Running Generated Executables

```bash
# Run directly
./helm-rpi3 --kernel path/to/vmlinux --dtb bcm2837-rpi-3-b.dtb

# With HMP interface
./helm-rpi3 --hmp-socket /tmp/rpi3.sock

# Boot Linux
./helm-rpi3 \
    --kernel vmlinux \
    --initrd initrd.img \
    --append "console=ttyAMA0 root=/dev/mmcblk0p2" \
    --sd-card rpi3-rootfs.img
```

The executable contains:
- All component implementations (cores, caches, devices)
- Platform configuration (hardcoded from Python definition)
- HMP server (optional)
- Optimized with LTO and potentially PGO

---

## 6. Use Cases

### 6.1 Development Workflow (Dynamic)

```bash
# Develop a new cache model
cd helm-plugin-custom-cache
cargo build --release

# Copy to plugin directory
cp target/release/libhelm_plugin_custom_cache.so ~/.helm/plugins/

# Test in Python
python test_cache.py  # HELM auto-loads plugin

# Iterate: edit code, rebuild, test again (hot-reload enabled)
```

### 6.2 Research Platform (Dynamic)

```python
# Experiment with different core configurations
from helm import Platform, PluginManager

pm = PluginManager()
pm.discover_plugins()

# Try different core implementations
for core_type in ["cortex-a53", "cortex-a72", "neoverse-n1"]:
    platform = Platform()
    platform.add_core(type=f"core.arm.{core_type}")
    results = platform.run_benchmark("spec2017")
    print(f"{core_type}: IPC = {results.ipc}")
```

### 6.3 Production Deployment (Static)

```bash
# Build optimized platform-specific simulators
helm build platforms/rpi3.py --output helm-rpi3 --profile pgo
helm build platforms/nvidia-jetson.py --output helm-jetson
helm build platforms/apple-m1.py --output helm-m1

# Deploy as standalone binaries
scp helm-rpi3 user@server:/opt/simulators/
scp helm-jetson user@server:/opt/simulators/
```

### 6.4 Third-Party Device Models

```bash
# Install proprietary device model from vendor
helm plugin install https://vendor.com/plugins/proprietary-gpu-v2.tar.gz

# Use in platform
python <<EOF
from helm import Platform

platform = Platform()
platform.add_device(
    type="vendor.proprietary-gpu",
    version="v2.0",
    # ... configuration
)
EOF
```

### 6.5 Educational Use

```python
# Students develop and test custom components
# platforms/student-soc.py
from helm import Platform

platform = Platform()

# Student implements their own cache replacement policy
platform.add_cache(
    type="cache.student.lru-with-prediction",  # Their plugin
    size="64KB",
)

# Test against reference implementation
reference = Platform()
reference.add_cache(type="cache.reference.lru", size="64KB")

# Compare performance
student_results = platform.run_trace("memory-trace.txt")
reference_results = reference.run_trace("memory-trace.txt")

print(f"Hit rate: {student_results.hit_rate} vs {reference_results.hit_rate}")
```

---

## 7. Implementation Details

### 7.1 Plugin API Crate Structure

```
helm-plugin-api/
├── Cargo.toml
└── src/
    ├── lib.rs           # Core traits and types
    ├── component.rs     # HelmComponent trait
    ├── registry.rs      # ComponentRegistry
    ├── property.rs      # Property system
    ├── abi.rs           # ABI version checking
    └── ffi.rs           # FFI-safe types
```

### 7.2 Core Engine Integration

```rust
// helm-engine/src/platform.rs
pub struct Platform {
    components: HashMap<String, Box<dyn HelmComponent>>,
    registry: ComponentRegistry,
    module_loader: ModuleLoader,
}

impl Platform {
    pub fn add_component(
        &mut self,
        type_name: &str,
        instance_name: &str,
        properties: HashMap<String, PropertyValue>,
    ) -> Result<()> {
        // Create component (may trigger plugin load)
        let mut component = self.registry.create_component(type_name)?;
        
        // Set properties
        for (key, value) in properties {
            component.set_property(&key, value)?;
        }
        
        // Add to platform
        self.components.insert(instance_name.to_string(), component);
        
        Ok(())
    }
}
```

### 7.3 Build System Integration

```toml
# Workspace Cargo.toml
[workspace]
members = [
    "crates/*",
    "plugins/*",
]

[profile.release-plugin]
inherits = "release"
# Plugin-specific optimizations
lto = "thin"  # Faster build, still optimized

[profile.release-static]
inherits = "release"
# Maximum optimization for standalone binaries
lto = "fat"
codegen-units = 1
```

### 7.4 Version Management

```rust
/// Plugin compatibility matrix
const PLUGIN_API_COMPATIBILITY: &[(u32, u32)] = &[
    (1, 1),  // API v1 compatible with core v1
    (1, 2),  // API v1 compatible with core v2 (forward compat)
    (2, 2),  // API v2 compatible with core v2
];

fn check_compatibility(plugin_api: u32, core_version: u32) -> bool {
    PLUGIN_API_COMPATIBILITY
        .iter()
        .any(|(api, core)| *api == plugin_api && *core == core_version)
}
```

### 7.5 Security Considerations

```rust
// Optional: Plugin signing and verification
pub struct PluginVerifier {
    trusted_keys: Vec<PublicKey>,
}

impl PluginVerifier {
    pub fn verify_plugin(&self, plugin_path: &Path) -> Result<()> {
        let signature = fs::read(plugin_path.with_extension("sig"))?;
        let plugin_data = fs::read(plugin_path)?;
        
        for key in &self.trusted_keys {
            if key.verify(&plugin_data, &signature).is_ok() {
                return Ok(());
            }
        }
        
        Err(anyhow!("Plugin signature verification failed"))
    }
}
```

---

## Conclusion

HELM's dual-mode architecture provides:

1. **Development Flexibility**: Dynamic plugins enable rapid iteration and third-party contributions
2. **Deployment Simplicity**: Generate standalone executables for specific platforms
3. **Performance**: Static linking with LTO/PGO for production deployments
4. **Ecosystem**: Plugin marketplace for sharing component implementations
5. **Security**: Optional plugin signing and ABI version checking

This approach combines the best of both worlds - the flexibility of dynamic systems like Simics with the performance and simplicity of static compilation when needed.
