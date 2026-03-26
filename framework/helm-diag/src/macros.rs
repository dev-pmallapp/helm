// src/macros.rs — sim_stub!, sim_warn!, sim_info! diagnostic macros

/// Emit a `Stub`-level diagnostic message.
///
/// Use for unimplemented features that return a default value and should not
/// abort the simulation. Common in device register stubs and unimplemented
/// sysreg handlers.
///
/// # Call forms
///
/// ```rust,ignore
/// // With PC:
/// sim_stub!(component = "gicv2-dist", pc = state.pc, "GICD_TYPER read -> 0");
/// sim_stub!(component = "pl011",      pc = pc, "write to read-only FCR: {:#x}", val);
///
/// // Without PC:
/// sim_stub!(component = "aarch64-fp", "FPCR feature not implemented");
/// ```
#[macro_export]
macro_rules! sim_stub {
    (component=$comp:expr, pc=$pc:expr, $($arg:tt)*) => {
        $crate::emit(
            $crate::DiagLevel::Stub,
            $comp,
            Some($pc),
            ::std::format!($($arg)*),
        )
    };
    (component=$comp:expr, $($arg:tt)*) => {
        $crate::emit(
            $crate::DiagLevel::Stub,
            $comp,
            None,
            ::std::format!($($arg)*),
        )
    };
}

/// Emit a `Warn`-level diagnostic message.
///
/// Use for unexpected but recoverable conditions: a write to a reserved
/// register, an unsupported combination of flags, a device reset while active.
///
/// # Call forms
///
/// ```rust,ignore
/// // With PC:
/// sim_warn!(component = "mmu", pc = state.pc, "unmapped VA {:#010x} -- returning 0", va);
///
/// // Without PC:
/// sim_warn!(component = "helm-loader", "ELF has PT_LOAD with zero filesz");
/// ```
#[macro_export]
macro_rules! sim_warn {
    (component=$comp:expr, pc=$pc:expr, $($arg:tt)*) => {
        $crate::emit(
            $crate::DiagLevel::Warn,
            $comp,
            Some($pc),
            ::std::format!($($arg)*),
        )
    };
    (component=$comp:expr, $($arg:tt)*) => {
        $crate::emit(
            $crate::DiagLevel::Warn,
            $comp,
            None,
            ::std::format!($($arg)*),
        )
    };
}

/// Emit an `Info`-level diagnostic message.
///
/// Use for normal operational events: loader progress, device initialization,
/// boot stage transitions.
///
/// `sim_info!` does not accept a `pc=` argument -- informational messages
/// are typically not associated with a specific guest instruction.
///
/// # Call form
///
/// ```rust,ignore
/// sim_info!(component = "helm-loader", "ELF loaded: entry={:#018x}", entry);
/// sim_info!(component = "arm-virt",    "GICv2 mapped at {:#010x}", base);
/// ```
#[macro_export]
macro_rules! sim_info {
    (component=$comp:expr, pc=$pc:expr, $($arg:tt)*) => {
        $crate::emit(
            $crate::DiagLevel::Info,
            $comp,
            Some($pc),
            ::std::format!($($arg)*),
        )
    };
    (component=$comp:expr, $($arg:tt)*) => {
        $crate::emit(
            $crate::DiagLevel::Info,
            $comp,
            None,
            ::std::format!($($arg)*),
        )
    };
}
