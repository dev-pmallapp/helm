//! The HelmComponent trait that all pluggable components implement.

use helm_core::HelmResult;

/// Metadata returned when registering a component with the loader.
pub struct ComponentInfo {
    pub type_name: &'static str,
    pub description: &'static str,
    /// Interfaces this component implements (e.g. `"memory-mapped"`).
    pub interfaces: &'static [&'static str],
    /// Factory that creates a default instance.
    pub factory: Box<dyn Fn() -> Box<dyn HelmComponent> + Send + Sync>,
}

/// Trait for all pluggable simulation components — devices, timing
/// models, branch predictors, cache replacement policies, etc.
///
/// This is intentionally a super-trait that combines object-model
/// capabilities (properties, lifecycle) with simulation duties.
pub trait HelmComponent: Send + Sync {
    /// Fully-qualified component type (e.g. `"device.uart.pl011"`).
    fn component_type(&self) -> &'static str;

    /// Interfaces this instance implements.
    fn interfaces(&self) -> &[&str] {
        &[]
    }

    /// Called once before simulation starts.
    fn realize(&mut self) -> HelmResult<()> {
        Ok(())
    }

    /// Reset to initial state.
    fn reset(&mut self) -> HelmResult<()> {
        Ok(())
    }

    /// Advance internal state by `cycles` (optional — not all components
    /// are clocked).
    fn tick(&mut self, _cycles: u64) -> HelmResult<()> {
        Ok(())
    }
}
