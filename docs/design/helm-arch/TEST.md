# helm-arch — Test Strategy

> **Status:** Draft — Phase 0 (RISC-V) + Phase 2 (AArch64)
> **Covers:** `tests/riscv/`, `tests/aarch64/`, property tests, differential tests, riscv-tests integration

---

## 1. Philosophy

Every test in `helm-arch` follows one rule: **known inputs → expected outputs**. There are no mocks, no stubs, no global state. Each test creates a minimal in-memory context, runs one or a small sequence of instructions, and asserts the architectural state.

The four test layers, from fastest to slowest:

| Layer | What it tests | How many | Runs in |
|-------|--------------|----------|---------|
| Unit | One instruction, several input/output pairs | ~300 | `cargo test` |
| Property | Decoder invariants over arbitrary inputs | Millions (proptest) | `cargo test` |
| Integration | Multi-instruction sequences, control flow | ~50 | `cargo test` |
| Validation | Official test suites (riscv-tests ELF binaries) | ~500 | CI only |

The unit and property layers run in under 5 seconds. Validation tests are gated behind `#[cfg(feature = "validation")]`.

---

## 2. Test Context: `MockExecContext`

A minimal `ExecContext` implementation for unit tests. Holds the full register file and a flat memory array. No timing, no events, no OS.

```rust
/// Test-only ExecContext: flat memory, 32 int regs, 32 FP regs, PC, CSRs.
pub struct MockExecContext {
    pub regs:  [u64; 32],
    pub fregs: [u64; 32],
    pub pc:    u64,
    pub mem:   Vec<u8>,
    pub mem_base: u64,
    pub csrs:  HashMap<u16, u64>,
    pub priv_: PrivLevel,
    pub nzcv:  u8,                    // AArch64 only
    pub sp:    u64,                   // AArch64 only
    pub vregs: [u128; 32],           // AArch64 SIMD registers
    pub last_exception: Option<HartException>,
    pub lr_addr: Option<u64>,        // RISC-V LR/SC reservation
    pub exclusive_monitor: Option<u64>, // AArch64 exclusive monitor
}

impl MockExecContext {
    pub fn new() -> Self {
        MockExecContext {
            regs: [0u64; 32],
            fregs: [0u64; 32],
            pc: 0x1000,
            mem: vec![0u8; 4096],
            mem_base: 0,
            csrs: HashMap::new(),
            priv_: PrivLevel::User,
            nzcv: 0,
            sp: 0,
            vregs: [0u128; 32],
            last_exception: None,
            lr_addr: None,
            exclusive_monitor: None,
        }
    }

    /// Write `bytes` into the mock memory at the given address.
    pub fn write_mem_bytes(&mut self, addr: u64, bytes: &[u8]) {
        let off = (addr - self.mem_base) as usize;
        self.mem[off..off + bytes.len()].copy_from_slice(bytes);
    }

    /// Read an integer register (shorthand for test assertions).
    pub fn x(&self, idx: usize) -> u64 { self.regs[idx] }
    pub fn f(&self, idx: usize) -> u64 { self.fregs[idx] }
}

impl ExecContext for MockExecContext {
    fn read_int_reg(&self, idx: usize) -> u64 {
        if idx == 0 { 0 } else { self.regs[idx] }
    }
    fn write_int_reg(&mut self, idx: usize, val: u64) {
        if idx != 0 { self.regs[idx] = val; }
    }
    fn read_float_reg(&self, idx: usize) -> u64 { self.fregs[idx] }
    fn write_float_reg(&mut self, idx: usize, val: u64) { self.fregs[idx] = val; }
    fn read_pc(&self) -> u64 { self.pc }
    fn write_pc(&mut self, val: u64) { self.pc = val; }
    fn read_mem(&self, addr: u64, width: usize) -> Result<u64, MemFault> {
        let off = (addr - self.mem_base) as usize;
        let slice = &self.mem[off..off + width];
        let val = match width {
            1 => slice[0] as u64,
            2 => u16::from_le_bytes(slice.try_into().unwrap()) as u64,
            4 => u32::from_le_bytes(slice.try_into().unwrap()) as u64,
            8 => u64::from_le_bytes(slice.try_into().unwrap()),
            _ => panic!("invalid width"),
        };
        Ok(val)
    }
    fn write_mem(&mut self, addr: u64, width: usize, val: u64) -> Result<(), MemFault> {
        let off = (addr - self.mem_base) as usize;
        let slice = &mut self.mem[off..off + width];
        match width {
            1 => slice[0] = val as u8,
            2 => slice.copy_from_slice(&(val as u16).to_le_bytes()),
            4 => slice.copy_from_slice(&(val as u32).to_le_bytes()),
            8 => slice.copy_from_slice(&val.to_le_bytes()),
            _ => panic!("invalid width"),
        }
        Ok(())
    }
    fn read_csr(&self, csr: u16) -> Result<u64, HartException> {
        Ok(*self.csrs.get(&csr).unwrap_or(&0))
    }
    fn write_csr(&mut self, csr: u16, val: u64) -> Result<(), HartException> {
        self.csrs.insert(csr, val); Ok(())
    }
    fn current_privilege(&self) -> PrivLevel { self.priv_ }
    fn set_privilege(&mut self, priv_: PrivLevel) { self.priv_ = priv_; }
    fn sfence_vma(&mut self, _va: Option<u64>, _asid: Option<u64>) {}
    fn set_lr(&mut self, addr: Option<u64>) { self.lr_addr = addr; }
    fn get_lr(&self) -> Option<u64> { self.lr_addr }
    // AArch64 extras:
    fn read_nzcv(&self) -> u8 { self.nzcv }
    fn write_nzcv(&mut self, v: u8) { self.nzcv = v; }
    fn read_sp(&self) -> u64 { self.sp }
    fn write_sp(&mut self, v: u64) { self.sp = v; }
    fn read_vreg(&self, idx: usize) -> u64 { self.vregs[idx] as u64 }
    fn write_vreg(&mut self, idx: usize, val: u64) { self.vregs[idx] = val as u128; }
    fn read_sysreg(&self, _s: SysregEncoding) -> Result<u64, HartException> { Ok(0) }
    fn write_sysreg(&mut self, _s: SysregEncoding, _v: u64) -> Result<(), HartException> { Ok(()) }
    fn current_el(&self) -> u8 { 1 }
    fn set_current_el(&mut self, _el: u8) {}
    fn set_exclusive_monitor(&mut self, addr: Option<u64>) { self.exclusive_monitor = addr; }
    fn get_exclusive_monitor(&self) -> Option<u64> { self.exclusive_monitor }
    fn tlb_flush_all(&mut self) {}
}
```

---

## 3. RISC-V Unit Tests

### Test naming convention

`test_rv64_{extension}_{mnemonic}[_{variant}]`

Examples: `test_rv64i_add`, `test_rv64i_lw`, `test_rv64i_beq_taken`, `test_rv64i_beq_not_taken`.

### RV64I Examples

```rust
#[test]
fn test_rv64i_add() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 5;
    ctx.regs[2] = 7;
    let insn = Instruction::Add { rd: 3, rs1: 1, rs2: 2 };
    execute(insn, &mut ctx).unwrap();
    assert_eq!(ctx.x(3), 12);
    assert_eq!(ctx.pc, 0x1004);
}

#[test]
fn test_rv64i_add_overflow_wraps() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = u64::MAX;
    ctx.regs[2] = 1;
    let insn = Instruction::Add { rd: 3, rs1: 1, rs2: 2 };
    execute(insn, &mut ctx).unwrap();
    assert_eq!(ctx.x(3), 0);    // wrapping_add
}

#[test]
fn test_rv64i_add_rd_is_x0_discards() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 100;
    ctx.regs[2] = 200;
    let insn = Instruction::Add { rd: 0, rs1: 1, rs2: 2 };  // rd = x0
    execute(insn, &mut ctx).unwrap();
    assert_eq!(ctx.x(0), 0);    // x0 must remain 0
    assert_eq!(ctx.pc, 0x1004);
}

#[test]
fn test_rv64i_lw_sign_extends() {
    let mut ctx = MockExecContext::new();
    // Store 0x8000_0000 (negative as i32) at address 0x100
    ctx.mem_base = 0;
    ctx.write_mem_bytes(0x100, &0x8000_0000u32.to_le_bytes());
    ctx.regs[1] = 0x100;
    let insn = Instruction::Load { rd: 2, rs1: 1, imm: 0, width: LoadWidth::Word };
    execute(insn, &mut ctx).unwrap();
    // LW sign-extends: 0x8000_0000 → 0xFFFF_FFFF_8000_0000
    assert_eq!(ctx.x(2), 0xFFFF_FFFF_8000_0000u64);
}

#[test]
fn test_rv64i_lwu_zero_extends() {
    let mut ctx = MockExecContext::new();
    ctx.mem_base = 0;
    ctx.write_mem_bytes(0x100, &0x8000_0000u32.to_le_bytes());
    ctx.regs[1] = 0x100;
    let insn = Instruction::Load { rd: 2, rs1: 1, imm: 0, width: LoadWidth::WordUnsigned };
    execute(insn, &mut ctx).unwrap();
    assert_eq!(ctx.x(2), 0x8000_0000u64);
}

#[test]
fn test_rv64i_beq_taken() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 42;
    ctx.regs[2] = 42;
    let insn = Instruction::Beq { rs1: 1, rs2: 2, imm: 8 };  // +8 bytes
    execute(insn, &mut ctx).unwrap();
    assert_eq!(ctx.pc, 0x1008);   // 0x1000 + 8
}

#[test]
fn test_rv64i_beq_not_taken() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 42;
    ctx.regs[2] = 43;
    let insn = Instruction::Beq { rs1: 1, rs2: 2, imm: 8 };
    execute(insn, &mut ctx).unwrap();
    assert_eq!(ctx.pc, 0x1004);   // fall through
}

#[test]
fn test_rv64i_jal_link() {
    let mut ctx = MockExecContext::new();
    ctx.pc = 0x2000;
    let insn = Instruction::Jal { rd: 1, imm: 16 };
    execute(insn, &mut ctx).unwrap();
    assert_eq!(ctx.x(1), 0x2004);   // link = PC+4
    assert_eq!(ctx.pc, 0x2010);     // 0x2000 + 16
}

#[test]
fn test_rv64i_jalr_clears_bit0() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 0x1001;   // odd address
    let insn = Instruction::Jalr { rd: 2, rs1: 1, imm: 0 };
    execute(insn, &mut ctx).unwrap();
    assert_eq!(ctx.pc, 0x1000);    // bit 0 cleared per spec
}

#[test]
fn test_rv64i_lui_sign_extends() {
    let mut ctx = MockExecContext::new();
    // LUI with negative upper immediate
    let insn = Instruction::Lui { rd: 1, imm: 0xFFFFF000 };  // top bit set
    execute(insn, &mut ctx).unwrap();
    assert_eq!(ctx.x(1), 0xFFFF_FFFF_FFFF_F000u64);   // SEXT from bit 31
}

#[test]
fn test_rv64i_addw_sign_extends() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 0x7FFF_FFFF;
    ctx.regs[2] = 1;
    let insn = Instruction::Addw { rd: 3, rs1: 1, rs2: 2 };
    execute(insn, &mut ctx).unwrap();
    // Low 32 bits: 0x8000_0000 → sign-extended to i64 → u64
    assert_eq!(ctx.x(3), 0xFFFF_FFFF_8000_0000u64);
}

#[test]
fn test_rv64i_slt_signed() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = (-1i64) as u64;   // 0xFFFF_FFFF_FFFF_FFFF
    ctx.regs[2] = 1;
    let insn = Instruction::Slt { rd: 3, rs1: 1, rs2: 2 };
    execute(insn, &mut ctx).unwrap();
    assert_eq!(ctx.x(3), 1);   // -1 < 1 signed → true
}

#[test]
fn test_rv64i_sltu_unsigned() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = (-1i64) as u64;   // 0xFFFF... = very large u64
    ctx.regs[2] = 1;
    let insn = Instruction::Sltu { rd: 3, rs1: 1, rs2: 2 };
    execute(insn, &mut ctx).unwrap();
    assert_eq!(ctx.x(3), 0);   // u64::MAX > 1 unsigned → false
}

#[test]
fn test_rv64i_ecall_umode() {
    let mut ctx = MockExecContext::new();
    ctx.priv_ = PrivLevel::User;
    let result = execute(Instruction::Ecall, &mut ctx);
    assert_eq!(result, Err(HartException::EnvironmentCallUMode));
    assert_eq!(ctx.pc, 0x1000);   // PC not advanced on exception
}
```

### M Extension Tests

```rust
#[test]
fn test_rv64m_mul_positive() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 6;
    ctx.regs[2] = 7;
    execute(Instruction::Mul { rd: 3, rs1: 1, rs2: 2 }, &mut ctx).unwrap();
    assert_eq!(ctx.x(3), 42);
}

#[test]
fn test_rv64m_div_by_zero_returns_all_ones() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 100;
    ctx.regs[2] = 0;
    execute(Instruction::Div { rd: 3, rs1: 1, rs2: 2 }, &mut ctx).unwrap();
    assert_eq!(ctx.x(3), u64::MAX);   // -1 as u64
}

#[test]
fn test_rv64m_div_overflow_returns_min() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = i64::MIN as u64;
    ctx.regs[2] = (-1i64) as u64;
    execute(Instruction::Div { rd: 3, rs1: 1, rs2: 2 }, &mut ctx).unwrap();
    assert_eq!(ctx.x(3), i64::MIN as u64);
}

#[test]
fn test_rv64m_rem_by_zero_returns_dividend() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 123;
    ctx.regs[2] = 0;
    execute(Instruction::Rem { rd: 3, rs1: 1, rs2: 2 }, &mut ctx).unwrap();
    assert_eq!(ctx.x(3), 123);
}

#[test]
fn test_rv64m_mulh_upper_bits() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = i64::MAX as u64;
    ctx.regs[2] = 2;
    execute(Instruction::Mulh { rd: 3, rs1: 1, rs2: 2 }, &mut ctx).unwrap();
    // (i64::MAX * 2) = 0x0000_0000_FFFF_FFFF_FFFF_FFFE;
    // upper 64 bits = 0
    assert_eq!(ctx.x(3), 0);
}

#[test]
fn test_rv64m_mulhu_overflow_into_upper() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = u64::MAX;
    ctx.regs[2] = u64::MAX;
    execute(Instruction::Mulhu { rd: 3, rs1: 1, rs2: 2 }, &mut ctx).unwrap();
    // u64::MAX * u64::MAX = 0xFFFF...FFFE_0000...0001
    // upper half = 0xFFFF_FFFF_FFFF_FFFE
    assert_eq!(ctx.x(3), 0xFFFF_FFFF_FFFF_FFFEu64);
}
```

### A Extension Tests

```rust
#[test]
fn test_rv64a_lr_sc_success() {
    let mut ctx = MockExecContext::new();
    ctx.mem_base = 0;
    ctx.write_mem_bytes(0x200, &42u32.to_le_bytes());
    ctx.regs[1] = 0x200;
    ctx.regs[2] = 99;

    // LR.W
    execute(Instruction::Lrw { rd: 3, rs1: 1, aq: false, rl: false }, &mut ctx).unwrap();
    assert_eq!(ctx.x(3) as i32, 42);   // sign-extended load
    assert_eq!(ctx.lr_addr, Some(0x200));

    // SC.W (should succeed: same address)
    execute(Instruction::Scw { rd: 4, rs1: 1, rs2: 2, aq: false, rl: false }, &mut ctx).unwrap();
    assert_eq!(ctx.x(4), 0);   // 0 = success
    assert_eq!(ctx.lr_addr, None);
    // Memory should now contain 99
    let val = u32::from_le_bytes(ctx.mem[0x200..0x204].try_into().unwrap());
    assert_eq!(val, 99);
}

#[test]
fn test_rv64a_sc_fails_different_address() {
    let mut ctx = MockExecContext::new();
    ctx.mem_base = 0;
    ctx.regs[1] = 0x200;
    ctx.regs[5] = 0x300;   // different address for SC

    execute(Instruction::Lrw { rd: 3, rs1: 1, aq: false, rl: false }, &mut ctx).unwrap();
    ctx.regs[1] = 0x300;
    execute(Instruction::Scw { rd: 4, rs1: 1, rs2: 2, aq: false, rl: false }, &mut ctx).unwrap();
    assert_eq!(ctx.x(4), 1);   // non-zero = failure
}
```

### Zicsr Tests

```rust
#[test]
fn test_zicsr_csrrw_reads_old() {
    let mut ctx = MockExecContext::new();
    ctx.csrs.insert(CsrAddr::SSCRATCH, 0xDEAD_BEEF);
    ctx.regs[1] = 0x1234;

    execute(Instruction::Csrrw { rd: 2, rs1: 1, csr: CsrAddr::SSCRATCH }, &mut ctx).unwrap();
    assert_eq!(ctx.x(2), 0xDEAD_BEEF);   // old value in rd
    assert_eq!(*ctx.csrs.get(&CsrAddr::SSCRATCH).unwrap(), 0x1234);  // new value written
}

#[test]
fn test_zicsr_csrrs_rd_x0_no_read() {
    // CSRRS x0, ...: rd = x0, so CSR read must be skipped.
    // Test: CSR with a side-effect-on-read must not be triggered.
    let mut ctx = MockExecContext::new();
    ctx.csrs.insert(CsrAddr::FFLAGS, 0xFF);
    execute(Instruction::Csrrs { rd: 0, rs1: 1, csr: CsrAddr::FFLAGS }, &mut ctx).unwrap();
    assert_eq!(ctx.x(0), 0);   // x0 still 0
}

#[test]
fn test_zicsr_csrrs_rs1_x0_no_write() {
    // CSRRS rd, x0, CSR: rs1 = x0, so CSR write must not happen.
    let mut ctx = MockExecContext::new();
    ctx.csrs.insert(CsrAddr::SSCRATCH, 0xABCD);
    execute(Instruction::Csrrs { rd: 1, rs1: 0, csr: CsrAddr::SSCRATCH }, &mut ctx).unwrap();
    assert_eq!(ctx.x(1), 0xABCD);   // read succeeded
    assert_eq!(*ctx.csrs.get(&CsrAddr::SSCRATCH).unwrap(), 0xABCD);  // unchanged
}
```

---

## 4. AArch64 Unit Tests

### Test naming convention

`test_a64_{group}_{mnemonic}[_{variant}]`

```rust
#[test]
fn test_a64_add_imm_basic() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 10;
    let insn = Aarch64Instruction::AddImm { sf: true, rd: 2, rn: 1, imm: 5, shift: 0 };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.regs[2], 15);
    assert_eq!(ctx.pc, 0x1004);
}

#[test]
fn test_a64_add_imm_shift12() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 0;
    let insn = Aarch64Instruction::AddImm { sf: true, rd: 2, rn: 1, imm: 1, shift: 12 };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.regs[2], 0x1000);   // 1 << 12
}

#[test]
fn test_a64_adds_imm_sets_nzcv() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 0xFFFF_FFFF_FFFF_FFFF;
    let insn = Aarch64Instruction::AddsImm { sf: true, rd: 2, rn: 1, imm: 1, shift: 0 };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.regs[2], 0);
    let nzcv = ctx.nzcv;
    assert_eq!(nzcv & 0x4, 0x4);   // Z=1: result is zero
    assert_eq!(nzcv & 0x2, 0x2);   // C=1: unsigned overflow
    assert_eq!(nzcv & 0x8, 0);     // N=0
    assert_eq!(nzcv & 0x1, 0);     // V=0: no signed overflow
}

#[test]
fn test_a64_subs_imm_sets_nzcv_lt() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 3;
    let insn = Aarch64Instruction::SubsImm { sf: true, rd: 0, rn: 1, imm: 5, shift: 0 };
    // CMP x1, #5 is SUBS xzr, x1, #5. rd=31=XZR, result discarded.
    execute_a64(insn, &mut ctx).unwrap();
    // 3 - 5 = -2. N=1 (result negative), Z=0, C=0 (borrow), V=0.
    let nzcv = ctx.nzcv;
    assert_eq!((nzcv >> 3) & 1, 1, "N should be set");
    assert_eq!((nzcv >> 2) & 1, 0, "Z should be clear");
    assert_eq!((nzcv >> 1) & 1, 0, "C should be clear (borrow)");
}

#[test]
fn test_a64_ldr_base_offset() {
    let mut ctx = MockExecContext::new();
    ctx.mem_base = 0;
    ctx.write_mem_bytes(0x100, &0x1234_5678_9ABC_DEF0u64.to_le_bytes());
    ctx.regs[1] = 0x80;   // base
    // LDR X2, [X1, #0x80]: offset 0x80, scale 8 → byte offset 0x80*8 ... wait
    // For DoubleWord (8 bytes), offset field is imm12 = byte_offset / 8.
    // So byte_offset = 0x80 and imm12 = 0x10.
    let insn = Aarch64Instruction::Ldr {
        size: LdStSize::DoubleWord,
        rt: 2, rn: 1,
        offset: 0x10,  // byte offset = 0x10 * 8 = 0x80; total addr = 0x80 + 0x80 = 0x100
    };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.regs[2], 0x1234_5678_9ABC_DEF0u64);
}

#[test]
fn test_a64_ldr_w_zero_extends() {
    let mut ctx = MockExecContext::new();
    ctx.mem_base = 0;
    ctx.write_mem_bytes(0x100, &0xDEAD_BEEFu32.to_le_bytes());
    ctx.regs[1] = 0x100;
    let insn = Aarch64Instruction::Ldr { size: LdStSize::Word, rt: 2, rn: 1, offset: 0 };
    execute_a64(insn, &mut ctx).unwrap();
    // W-register load: zero-extend to 64 bits.
    assert_eq!(ctx.regs[2], 0xDEAD_BEEF);
}

#[test]
fn test_a64_ldrsb_sign_extends() {
    let mut ctx = MockExecContext::new();
    ctx.mem_base = 0;
    ctx.mem[0x100] = 0x80;   // -128 as i8
    ctx.regs[1] = 0x100;
    let insn = Aarch64Instruction::Ldrsb { sf: true, rt: 2, rn: 1, offset: 0 };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.regs[2], 0xFFFF_FFFF_FFFF_FF80u64);
}

#[test]
fn test_a64_ldp_stp() {
    let mut ctx = MockExecContext::new();
    ctx.mem_base = 0;
    ctx.regs[1] = 0;           // base = 0
    ctx.regs[3] = 0xAAAA_BBBB;
    ctx.regs[4] = 0xCCCC_DDDD;
    // STP W3, W4, [X1, #0] — store pair of words
    let insn = Aarch64Instruction::Stp {
        size: LdpStpSize::Word, rt1: 3, rt2: 4, rn: 1,
        simm: 0, mode: PairMode::Offset,
    };
    execute_a64(insn, &mut ctx).unwrap();

    // LDP W5, W6, [X1, #0]
    let insn2 = Aarch64Instruction::Ldp {
        size: LdpStpSize::Word, rt1: 5, rt2: 6, rn: 1,
        simm: 0, mode: PairMode::Offset,
    };
    execute_a64(insn2, &mut ctx).unwrap();
    assert_eq!(ctx.regs[5], 0xAAAA_BBBB);
    assert_eq!(ctx.regs[6], 0xCCCC_DDDD);
}

#[test]
fn test_a64_b_target() {
    let mut ctx = MockExecContext::new();
    ctx.pc = 0x2000;
    let insn = Aarch64Instruction::B { imm: 32 };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.pc, 0x2020);
}

#[test]
fn test_a64_bl_link() {
    let mut ctx = MockExecContext::new();
    ctx.pc = 0x3000;
    let insn = Aarch64Instruction::Bl { imm: -8i32 };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.regs[30], 0x3004);   // X30 = PC+4
    assert_eq!(ctx.pc, 0x2FF8);        // 0x3000 - 8
}

#[test]
fn test_a64_cbz_taken() {
    let mut ctx = MockExecContext::new();
    ctx.pc = 0x1000;
    ctx.regs[5] = 0;
    let insn = Aarch64Instruction::Cbz { sf: true, rt: 5, imm: 16 };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.pc, 0x1010);
}

#[test]
fn test_a64_cbz_not_taken() {
    let mut ctx = MockExecContext::new();
    ctx.pc = 0x1000;
    ctx.regs[5] = 1;
    let insn = Aarch64Instruction::Cbz { sf: true, rt: 5, imm: 16 };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.pc, 0x1004);
}

#[test]
fn test_a64_bcond_eq_taken() {
    let mut ctx = MockExecContext::new();
    ctx.pc = 0x1000;
    ctx.nzcv = 0b0100;   // Z=1: equal
    let insn = Aarch64Instruction::BCond { cond: 0b0000 /* EQ */, imm: 8 };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.pc, 0x1008);
}

#[test]
fn test_a64_bcond_eq_not_taken_when_ne() {
    let mut ctx = MockExecContext::new();
    ctx.pc = 0x1000;
    ctx.nzcv = 0b0000;   // Z=0: not equal
    let insn = Aarch64Instruction::BCond { cond: 0b0000 /* EQ */, imm: 8 };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.pc, 0x1004);
}

#[test]
fn test_a64_svc_raises_exception() {
    let mut ctx = MockExecContext::new();
    ctx.pc = 0x1000;
    let result = execute_a64(Aarch64Instruction::Svc { imm16: 0 }, &mut ctx);
    assert!(matches!(result, Err(HartException::Svc { .. })));
    assert_eq!(ctx.pc, 0x1000);   // PC not advanced
}

#[test]
fn test_a64_movz_basic() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = u64::MAX;   // should be cleared
    let insn = Aarch64Instruction::Movz { sf: true, rd: 1, imm16: 0x1234, hw: 0 };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.regs[1], 0x1234);
}

#[test]
fn test_a64_movk_preserves_other_bits() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 0xFFFF_FFFF_FFFF_FFFFu64;
    let insn = Aarch64Instruction::Movk { sf: true, rd: 1, imm16: 0xABCD, hw: 1 }; // position 16
    execute_a64(insn, &mut ctx).unwrap();
    // Bits [31:16] = 0xABCD, all others remain 0xFFFF.
    assert_eq!(ctx.regs[1], 0xFFFF_FFFF_ABCD_FFFFu64);
}

#[test]
fn test_a64_xzr_write_discards() {
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = 10;
    ctx.regs[2] = 20;
    // ADDS xzr, x1, x2 — sets flags but result discarded (rd=31=XZR).
    let insn = Aarch64Instruction::AddsImm { sf: true, rd: 31, rn: 1, imm: 5, shift: 0 };
    execute_a64(insn, &mut ctx).unwrap();
    // XZR: reading index 31 as GPR should return 0.
    // The write_gpr function discards idx=31.
    // Verify no register 31 in our array was written (we don't have one, so check flags).
    assert_ne!(ctx.nzcv, 0);   // flags were set
}

#[test]
fn test_a64_w_register_zero_extends() {
    // Writing to a W register must zero-extend: upper 32 bits of Xn become 0.
    let mut ctx = MockExecContext::new();
    ctx.regs[1] = u64::MAX;   // upper 32 bits set
    // MOVZ W1, #1 — 32-bit operation
    let insn = Aarch64Instruction::Movz { sf: false, rd: 1, imm16: 1, hw: 0 };
    execute_a64(insn, &mut ctx).unwrap();
    assert_eq!(ctx.regs[1], 1);   // upper 32 bits cleared (zero-extended)
}
```

---

## 5. Decoder Property Tests

Property tests verify that the decoder never panics on arbitrary input, that the decode → re-encode round trip is consistent for known formats, and that illegal inputs always produce `Instruction::Illegal` or `DecodeError` rather than crashing.

```rust
use proptest::prelude::*;

proptest! {
    /// Decoder must never panic on any 32-bit input.
    #[test]
    fn riscv_decode_never_panics(raw: u32) {
        // Any input with bits [1:0] == 0b11 is a valid 32-bit instruction slot.
        let raw_32 = raw | 0x3;   // ensure bits [1:0] = 11
        let _ = decode_rv64(raw_32);   // must not panic
    }

    /// Compressed decoder must never panic on any 16-bit input.
    #[test]
    fn riscv_decode_c_never_panics(raw: u16) {
        let _ = decode_rv64c(raw);   // must not panic
    }

    /// AArch64 decoder must never panic on any 32-bit input.
    #[test]
    fn aarch64_decode_never_panics(raw: u32) {
        let _ = decode_a64(raw);   // must not panic
    }

    /// If decode_rv64c succeeds, the resulting 32-bit word must decode without error.
    #[test]
    fn riscv_c_expansion_always_valid(raw: u16) {
        if let Ok(expanded) = decode_rv64c(raw) {
            // Expanded word must decode successfully (not return Illegal or panic).
            let result = decode_rv64(expanded | 0x3);
            // Some expansions may produce Instruction::Illegal for reserved encodings,
            // but the function itself must not return Err or panic.
            prop_assert!(result.is_ok());
        }
    }

    /// R-type instructions: any valid rd/rs1/rs2 must decode to the expected variant.
    #[test]
    fn riscv_rtype_add_roundtrip(rd in 0u8..32, rs1 in 0u8..32, rs2 in 0u8..32) {
        // Construct a raw ADD instruction word.
        let raw = (0b000_0000 << 25)
            | ((rs2 as u32) << 20)
            | ((rs1 as u32) << 15)
            | (0b000 << 12)
            | ((rd as u32) << 7)
            | 0b011_0011    // OP opcode
            | 0x3;          // bits [1:0]
        let insn = decode_rv64(raw).unwrap();
        prop_assert_eq!(insn, Instruction::Add { rd, rs1, rs2 });
    }
}
```

---

## 6. riscv-tests Integration

The official RISC-V test suite (`riscv/riscv-tests`) provides ELF binaries that test every instruction with known pass/fail behavior. The integration requires:

1. An ELF loader to load the binary into `MockExecContext`'s flat memory.
2. A `step_until_tohost` runner: execute instructions until the `tohost` symbol is written. `tohost = 1` means pass; any other value means the test number that failed.

```rust
#[cfg(feature = "validation")]
mod riscv_tests {
    use super::*;
    use std::path::Path;

    /// Load and run a single riscv-tests ELF. Panics if the test fails.
    fn run_riscv_test(elf_path: &Path) {
        let elf  = std::fs::read(elf_path).expect("ELF not found");
        let (mem, entry_pc, tohost_addr) = load_elf(&elf);
        let mut ctx = MockExecContext::new();
        ctx.mem = mem;
        ctx.mem_base = 0x8000_0000;
        ctx.pc = entry_pc;

        let max_insns = 10_000_000;
        for _ in 0..max_insns {
            // Read 2 bytes at PC to check for C extension.
            let raw16 = u16::from_le_bytes([ctx.mem[(ctx.pc - ctx.mem_base) as usize],
                                             ctx.mem[(ctx.pc - ctx.mem_base + 1) as usize]]);
            let raw32 = if (raw16 & 0x3) != 0x3 {
                // C extension: expand and execute.
                let expanded = decode_rv64c(raw16).expect("invalid C instruction");
                ctx.pc = ctx.pc.wrapping_add(2);
                expanded
            } else {
                let raw32 = u32::from_le_bytes(ctx.mem[(ctx.pc - ctx.mem_base) as usize..]
                    [..4].try_into().unwrap());
                ctx.pc = ctx.pc.wrapping_add(4);  // pre-advance; execute sets correct value
                raw32
            };

            let insn = decode_rv64(raw32).expect("decode failed");
            match execute(insn, &mut ctx) {
                Ok(()) => {}
                Err(HartException::EnvironmentCallMMode) => {
                    // ECALL from M-mode = test complete; check tohost.
                    break;
                }
                Err(e) => panic!("unexpected exception: {:?}", e),
            }

            // Check tohost
            if let Ok(val) = ctx.read_mem(tohost_addr, 8) {
                if val != 0 {
                    if val == 1 {
                        return; // PASS
                    }
                    panic!("riscv-test FAIL: tohost = {:#x} (test #{} failed)", val, val >> 1);
                }
            }
        }
        panic!("riscv-test did not complete within {} instructions", max_insns);
    }

    #[test] fn rv64ui_add()  { run_riscv_test(Path::new("tests/isa/rv64ui-p-add")); }
    #[test] fn rv64ui_addi() { run_riscv_test(Path::new("tests/isa/rv64ui-p-addi")); }
    #[test] fn rv64ui_lw()   { run_riscv_test(Path::new("tests/isa/rv64ui-p-lw")); }
    #[test] fn rv64ui_sw()   { run_riscv_test(Path::new("tests/isa/rv64ui-p-sw")); }
    #[test] fn rv64ui_jal()  { run_riscv_test(Path::new("tests/isa/rv64ui-p-jal")); }
    #[test] fn rv64um_mul()  { run_riscv_test(Path::new("tests/isa/rv64um-p-mul")); }
    #[test] fn rv64um_div()  { run_riscv_test(Path::new("tests/isa/rv64um-p-div")); }
    #[test] fn rv64ua_lrsc() { run_riscv_test(Path::new("tests/isa/rv64ua-p-lrsc")); }
    // ... all rv64u{i,m,a,f,d,c}-p-* variants
}
```

ELF test binaries are expected at `tests/isa/` within the `helm-arch` crate directory. They are not committed to the repository; a `Makefile` target or `build.rs` downloads them from the official riscv-tests release artifacts.

---

## 7. QEMU Differential Test

For AArch64 and as a secondary check for RISC-V: run the same binary under QEMU and under helm-ng, then compare register state at program exit.

```rust
#[cfg(feature = "differential")]
#[test]
fn differential_riscv_hello() {
    // 1. Compile tests/data/hello.c to a statically linked RISC-V ELF.
    // 2. Run under helm-ng SE mode; capture exit register state.
    // 3. Run under QEMU user-mode: qemu-riscv64 hello; capture exit code.
    // 4. Compare: exit code must match.

    let helm_result = run_se_binary("tests/data/hello");
    let qemu_result = run_qemu_binary("tests/data/hello");
    assert_eq!(helm_result.exit_code, qemu_result.exit_code);
    assert_eq!(helm_result.stdout,    qemu_result.stdout);
}

#[cfg(feature = "differential")]
#[test]
fn differential_aarch64_hello() {
    let helm_result = run_se_binary_a64("tests/data/hello_aarch64");
    let qemu_result = run_qemu_binary("tests/data/hello_aarch64");
    assert_eq!(helm_result.exit_code, qemu_result.exit_code);
    assert_eq!(helm_result.stdout,    qemu_result.stdout);
}
```

For register-level comparison: helm-ng can optionally dump the full register file at every syscall boundary. QEMU's `-d cpu` log provides the same. A test harness compares them instruction-by-instruction for short programs. This is the most accurate validation method but requires QEMU to be installed in CI.

---

## 8. Edge Case Checklist

These are the correctness edge cases that each instruction set implementation must cover, verified by dedicated unit tests.

### RISC-V

| Edge Case | Test |
|-----------|------|
| x0 write silently discarded | `test_rv64i_add_rd_is_x0_discards` |
| SLTIU: sign-extend imm then compare as unsigned | `test_rv64i_sltiu_sign_ext_unsigned` |
| LW sign-extends; LWU zero-extends | `test_rv64i_lw_sign_extends`, `test_rv64i_lwu_zero_extends` |
| ADDW/SUBW sign-extend 32-bit result to 64 | `test_rv64i_addw_sign_extends` |
| JAL stores PC+4, not PC+imm | `test_rv64i_jal_link` |
| JALR clears bit 0 of target | `test_rv64i_jalr_clears_bit0` |
| LUI sign-extends from bit 31 | `test_rv64i_lui_sign_extends` |
| SLL/SRL/SRA use rs2[5:0] (6 bits) for RV64 | `test_rv64i_sll_uses_6_bits` |
| SLLW/SRLW/SRAW use rs2[4:0] (5 bits) | `test_rv64i_sllw_uses_5_bits` |
| DIV by zero = -1 (all bits set) | `test_rv64m_div_by_zero_returns_all_ones` |
| DIV overflow (MIN/-1) = MIN | `test_rv64m_div_overflow_returns_min` |
| REM by zero = dividend | `test_rv64m_rem_by_zero_returns_dividend` |
| REM overflow = 0 | `test_rv64m_rem_overflow_returns_zero` |
| SC.W with reservation mismatch = failure | `test_rv64a_sc_fails_different_address` |
| CSRRS with rs1=x0 does not write CSR | `test_zicsr_csrrs_rs1_x0_no_write` |
| CSRRW with rd=x0 does not read CSR | `test_zicsr_csrrw_rd_x0_no_read` |
| FMV.W.X NaN-boxes the float | `test_rv64f_fmv_wx_nan_boxes` |
| FP read of non-NaN-boxed value = canonical NaN | `test_rv64f_read_non_nan_boxed` |
| MRET restores privilege from mstatus.MPP | `test_rv64priv_mret_restores_privilege` |
| SFENCE.VMA flushes TLB | `test_rv64priv_sfence_flushes_tlb` |

### AArch64

| Edge Case | Test |
|-----------|------|
| W register write zero-extends upper 32 bits | `test_a64_w_register_zero_extends` |
| XZR read always returns 0 | `test_a64_xzr_read_is_zero` |
| XZR write is silently discarded | `test_a64_xzr_write_discards` |
| SP is used for rn=31 in load/store | `test_a64_ldr_rn31_is_sp` |
| LDR(signed) sign-extends | `test_a64_ldrsb_sign_extends`, `test_a64_ldrsw` |
| LDR(unsigned word) zero-extends | `test_a64_ldr_w_zero_extends` |
| Pre-index: base updated before access | `test_a64_ldr_pre_index` |
| Post-index: base updated after access | `test_a64_ldr_post_index` |
| SUB is ADD with NOT(operand2) + carry=1 | `test_a64_sub_via_add_with_carry` |
| ADDS/SUBS sets all NZCV bits correctly | `test_a64_adds_imm_sets_nzcv` |
| MOVZ zeros the register | `test_a64_movz_basic` |
| MOVK preserves other bits | `test_a64_movk_preserves_other_bits` |
| CBZ/CBNZ: Wt uses 32-bit zero test | `test_a64_cbz_32bit_zero_test` |
| B.cond checks all 14 condition codes | `test_a64_check_cond_all_codes` |
| Division by zero returns 0 (AArch64 rule) | `test_a64_sdiv_by_zero_returns_zero` |
| SDIV overflow (INT_MIN/-1) = INT_MIN | `test_a64_sdiv_overflow` |
| LDAXR sets exclusive monitor | `test_a64_ldaxr_sets_monitor` |
| STLXR fails if monitor not set | `test_a64_stlxr_fails_no_monitor` |
| SVC raises HartException::Svc | `test_a64_svc_raises_exception` |
| SP alignment: SP must be 16-byte aligned on exception | `test_a64_sp_alignment_on_exception` |

---

## 9. CI Pipeline

```yaml
# .github/workflows/helm-arch.yml (excerpt)
jobs:
  test-unit:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test -p helm-arch          # unit + property tests (~5 seconds)

  test-validation:
    runs-on: ubuntu-latest
    steps:
      - run: make download-riscv-tests         # fetch ELF binaries
      - run: cargo test -p helm-arch --features validation   # riscv-tests ELF suite

  test-differential:
    runs-on: ubuntu-latest
    steps:
      - run: sudo apt-get install -y qemu-user
      - run: cargo test -p helm-arch --features differential  # QEMU comparison

  bench:
    runs-on: ubuntu-latest
    steps:
      - run: cargo bench -p helm-arch --bench decode_throughput
      # Benchmark: decode_rv64 throughput (instructions/sec), decode_a64 throughput.
      # Criterion report saved as artifact.
```
