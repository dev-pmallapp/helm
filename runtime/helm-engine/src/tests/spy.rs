//! Tests for HelmSpy integration into the engine.

#[cfg(feature = "instrumentation")]
mod instrumented {
    use crate::{ExecMode, HelmEngine, Isa, VirtualTiming};
    use helm_core::{AccessType, MemInterface};
    use helm_spy::session::HelmSpy;
    use std::sync::Arc;

    /// Helper: build a minimal RISC-V SE engine for testing.
    fn make_riscv_engine() -> HelmEngine<VirtualTiming> {
        HelmEngine::new(
            Isa::RiscV,
            ExecMode::Syscall,
            VirtualTiming::new(1.0),
            0x0,
            64 * 1024,
        )
    }

    /// Helper: build a minimal AArch64 functional engine for testing.
    fn make_aarch64_engine() -> HelmEngine<VirtualTiming> {
        HelmEngine::new(
            Isa::AArch64,
            ExecMode::Functional,
            VirtualTiming::new(1.0),
            0x0,
            64 * 1024,
        )
    }

    #[test]
    fn attach_spy_returns_arc() {
        let mut engine = make_riscv_engine();
        let spy = Arc::new(HelmSpy::new());
        let returned = engine.attach_spy(Arc::clone(&spy));
        assert!(Arc::ptr_eq(&spy, &returned));
        assert!(engine.spy().is_some());
    }

    #[test]
    fn detach_spy_clears_field() {
        let mut engine = make_riscv_engine();
        let spy = Arc::new(HelmSpy::new());
        engine.attach_spy(Arc::clone(&spy));
        assert!(engine.spy().is_some());

        let detached = engine.detach_spy();
        assert!(detached.is_some());
        assert!(engine.spy().is_none());
    }

    #[test]
    fn spy_coexists_with_plugins() {
        let mut engine = make_riscv_engine();
        // Register a plugin callback.
        engine.plugins.on_insn_exec(Box::new(|_vcpu, _info| {}));
        assert!(engine.plugins.has_insn_callbacks());

        // Attach a spy alongside the plugin.
        let spy = Arc::new(HelmSpy::new());
        engine.attach_spy(spy);
        assert!(engine.spy().is_some());

        // Plugin still works.
        assert!(engine.plugins.has_insn_callbacks());
    }

    #[test]
    fn spy_collects_data_from_aarch64_run() {
        let mut engine = make_aarch64_engine();
        let spy = Arc::new(HelmSpy::new());
        engine.attach_spy(Arc::clone(&spy));

        // Write a short sequence of AArch64 NOP instructions (0xD503201F)
        // followed by an undefined instruction to stop execution.
        let nop: u32 = 0xD503_201F;
        let udf: u32 = 0x0000_0000; // UDF #0
        let base = 0u64;
        for i in 0..5u64 {
            engine
                .memory
                .write(base + i * 4, 4, nop as u64, AccessType::Store)
                .unwrap();
        }
        engine
            .memory
            .write(base + 5 * 4, 4, udf as u64, AccessType::Store)
            .unwrap();

        // Set PC to the start of the sequence.
        if let Some(a64) = engine
            .session
            .aarch64_mut()
            .and_then(crate::Aarch64Core::state_mut)
        {
            a64.pc = base;
        }

        // Run; expect it to retire the 5 NOPs before hitting UDF.
        let _stop = engine.run(10);

        // The spy should have observed at least the NOPs via probes.
        let snap = spy.snapshot();
        assert!(
            snap.insn_count >= 5,
            "spy should have observed at least 5 instructions, got {}",
            snap.insn_count
        );
    }

    #[test]
    fn spy_collects_data_from_riscv_run() {
        let mut engine = make_riscv_engine();
        let spy = Arc::new(HelmSpy::new());
        engine.attach_spy(Arc::clone(&spy));

        // Write a short sequence of RISC-V NOP instructions (ADDI x0, x0, 0 = 0x00000013)
        // followed by ECALL (0x00000073) which triggers a syscall exception.
        let nop: u32 = 0x0000_0013;
        let ecall: u32 = 0x0000_0073;
        let base = 0u64;
        for i in 0..5u64 {
            engine
                .memory
                .write(base + i * 4, 4, nop as u64, AccessType::Store)
                .unwrap();
        }
        engine
            .memory
            .write(base + 5 * 4, 4, ecall as u64, AccessType::Store)
            .unwrap();

        // Set PC to the start.
        engine.riscv_mut().pc = base;

        // Run a few instructions.
        let _stop = engine.run(10);

        // The spy should have counted instruction retirements.
        // With probes wired, insn_count increments via the probe subscriber.
        let snap = spy.snapshot();
        assert!(
            snap.insn_count >= 5,
            "spy should have observed at least 5 instructions, got {}",
            snap.insn_count
        );
    }
}
