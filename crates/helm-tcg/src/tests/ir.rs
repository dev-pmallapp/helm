use crate::ir::*;

#[test]
fn tcg_temp_equality() {
    assert_eq!(TcgTemp(0), TcgTemp(0));
    assert_ne!(TcgTemp(0), TcgTemp(1));
}

#[test]
fn movi_stores_value() {
    let op = TcgOp::Movi {
        dst: TcgTemp(0),
        value: 0xDEAD,
    };
    if let TcgOp::Movi { value, .. } = op {
        assert_eq!(value, 0xDEAD);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn read_reg_construction() {
    let op = TcgOp::ReadReg { dst: TcgTemp(3), reg_id: 5 };
    if let TcgOp::ReadReg { dst, reg_id } = op {
        assert_eq!(dst, TcgTemp(3));
        assert_eq!(reg_id, 5);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn write_reg_construction() {
    let op = TcgOp::WriteReg { reg_id: 0, src: TcgTemp(7) };
    if let TcgOp::WriteReg { reg_id, src } = op {
        assert_eq!(reg_id, 0);
        assert_eq!(src, TcgTemp(7));
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn goto_tb_target_pc() {
    let op = TcgOp::GotoTb { target_pc: 0x4000_0000 };
    if let TcgOp::GotoTb { target_pc } = op {
        assert_eq!(target_pc, 0x4000_0000);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn add_construction() {
    let op = TcgOp::Add { dst: TcgTemp(0), a: TcgTemp(1), b: TcgTemp(2) };
    if let TcgOp::Add { dst, a, b } = op {
        assert_eq!(dst, TcgTemp(0));
        assert_eq!(a, TcgTemp(1));
        assert_eq!(b, TcgTemp(2));
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn addi_with_negative_immediate() {
    let op = TcgOp::Addi { dst: TcgTemp(0), a: TcgTemp(1), imm: -8 };
    if let TcgOp::Addi { imm, .. } = op {
        assert_eq!(imm, -8);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn load_construction() {
    let op = TcgOp::Load { dst: TcgTemp(0), addr: TcgTemp(1), size: 8 };
    if let TcgOp::Load { size, .. } = op {
        assert_eq!(size, 8);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn store_construction() {
    let op = TcgOp::Store { addr: TcgTemp(1), val: TcgTemp(2), size: 4 };
    if let TcgOp::Store { size, .. } = op {
        assert_eq!(size, 4);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn syscall_op_carries_nr_temp() {
    let op = TcgOp::Syscall { nr: TcgTemp(8) };
    if let TcgOp::Syscall { nr } = op {
        assert_eq!(nr, TcgTemp(8));
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn exit_tb_is_clonable() {
    let op = TcgOp::ExitTb;
    let _ = op.clone();
}

#[test]
fn tcg_temp_zero() {
    let t = TcgTemp(0);
    assert_eq!(t, TcgTemp(0));
}
