use crate::context::TcgContext;
use crate::ir::TcgOp;

#[test]
fn temps_are_unique() {
    let mut ctx = TcgContext::new();
    let t0 = ctx.temp();
    let t1 = ctx.temp();
    assert_ne!(t0, t1);
}

#[test]
fn emit_add_imm_sequence() {
    let mut ctx = TcgContext::new();

    // Translate: ADD X0, X1, #42
    let rn = ctx.read_reg(1); // X1
    let imm = ctx.movi(42);
    let result = ctx.add(rn, imm);
    ctx.write_reg(0, result); // X0

    let ops = ctx.finish();
    assert_eq!(ops.len(), 4); // ReadReg, Movi, Add, WriteReg
    assert!(matches!(ops[0], TcgOp::ReadReg { .. }));
    assert!(matches!(ops[1], TcgOp::Movi { .. }));
    assert!(matches!(ops[2], TcgOp::Add { .. }));
    assert!(matches!(ops[3], TcgOp::WriteReg { .. }));
}

#[test]
fn emit_load_store() {
    let mut ctx = TcgContext::new();

    // LDR X0, [X1, #8]
    let base = ctx.read_reg(1);
    let addr = ctx.addi(base, 8);
    let val = ctx.load(addr, 8);
    ctx.write_reg(0, val);

    assert_eq!(ctx.op_count(), 4);
}

#[test]
fn emit_branch() {
    let mut ctx = TcgContext::new();
    ctx.emit(TcgOp::GotoTb { target_pc: 0x1000 });
    let ops = ctx.finish();
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], TcgOp::GotoTb { target_pc: 0x1000 }));
}
