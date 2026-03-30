//! AArch64 decode/execute test suite.
//! Ported from ../helm.git/crates/helm-isa/src/arm/aarch64/tests/
mod exec_basic;
mod exec_branch;
mod exec_bulk;
mod exec_corner_cases;
mod exec_dp_imm;
mod exec_dp_reg;
mod exec_el2_el3;
mod exec_flags;
mod exec_fp;
mod exec_ldst;
mod exec_ldst_bulk;
mod exec_multiply;
mod exec_parametric;
mod exec_simd;
mod exec_sysreg;
mod harness;
