//! AArch64 decode/execute test suite.
//! Ported from ../helm.git/crates/helm-isa/src/arm/aarch64/tests/
mod harness;
mod exec_basic;
mod exec_dp_imm;
mod exec_dp_reg;
mod exec_branch;
mod exec_ldst;
mod exec_flags;
mod exec_multiply;
mod exec_sysreg;
mod exec_bulk;
mod exec_corner_cases;
mod exec_fp;
mod exec_parametric;
mod exec_ldst_bulk;
