/*
 * riscv64.c — RISC-V64 guest stencil functions.
 *
 * Each function implements a single RV64I/M guest instruction.
 * Calling convention: rdi=regs, rsi=mem. Non-terminators fall through;
 * terminators return an exit code in rax.
 *
 * RISC-V register mapping: x0-x31 at flat array slots 0-31, PC at slot 32.
 * x0 is hardwired zero — the caller re-zeros it after each block.
 */

#include "common.h"

/* ═══════════════════════════════════════════════════════════════════════════
 * ALU — Immediate (I-type)
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_rv_addi(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, (uint64_t)((int64_t)rs1 + imm));
}

void stencil_rv_slti(uint64_t* regs, uint8_t* mem) {
    int64_t rs1 = (int64_t)REG_LOAD(HOLE_RN_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, rs1 < imm ? 1 : 0);
}

void stencil_rv_sltiu(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, rs1 < imm ? 1 : 0);
}

void stencil_rv_xori(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, rs1 ^ imm);
}

void stencil_rv_ori(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, rs1 | imm);
}

void stencil_rv_andi(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, rs1 & imm);
}

void stencil_rv_slli(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t shamt = (uint64_t)(uintptr_t)HOLE_SHAMT;
    REG_STORE(HOLE_RD_OFF, rs1 << (shamt & 63));
}

void stencil_rv_srli(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t shamt = (uint64_t)(uintptr_t)HOLE_SHAMT;
    REG_STORE(HOLE_RD_OFF, rs1 >> (shamt & 63));
}

void stencil_rv_srai(uint64_t* regs, uint8_t* mem) {
    int64_t rs1 = (int64_t)REG_LOAD(HOLE_RN_OFF);
    uint64_t shamt = (uint64_t)(uintptr_t)HOLE_SHAMT;
    REG_STORE(HOLE_RD_OFF, (uint64_t)(rs1 >> (shamt & 63)));
}

/* ═══════════════════════════════════════════════════════════════════════════
 * ALU — Register (R-type)
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_rv_add(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rs1 + rs2);
}

void stencil_rv_sub(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rs1 - rs2);
}

void stencil_rv_sll(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rs1 << (rs2 & 63));
}

void stencil_rv_slt(uint64_t* regs, uint8_t* mem) {
    int64_t rs1 = (int64_t)REG_LOAD(HOLE_RN_OFF);
    int64_t rs2 = (int64_t)REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rs1 < rs2 ? 1 : 0);
}

void stencil_rv_sltu(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rs1 < rs2 ? 1 : 0);
}

void stencil_rv_xor(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rs1 ^ rs2);
}

void stencil_rv_srl(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rs1 >> (rs2 & 63));
}

void stencil_rv_sra(uint64_t* regs, uint8_t* mem) {
    int64_t rs1 = (int64_t)REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, (uint64_t)(rs1 >> (rs2 & 63)));
}

void stencil_rv_or(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rs1 | rs2);
}

void stencil_rv_and(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rs1 & rs2);
}

/* ═══════════════════════════════════════════════════════════════════════════
 * ALU — Word ops (RV64I only, 32-bit with sign extension)
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_rv_addiw(uint64_t* regs, uint8_t* mem) {
    int32_t rs1 = (int32_t)(uint32_t)REG_LOAD(HOLE_RN_OFF);
    int32_t imm = (int32_t)(intptr_t)HOLE_IMM;
    int64_t result = (int64_t)(rs1 + imm);
    REG_STORE(HOLE_RD_OFF, (uint64_t)result);
}

void stencil_rv_addw(uint64_t* regs, uint8_t* mem) {
    int32_t rs1 = (int32_t)(uint32_t)REG_LOAD(HOLE_RN_OFF);
    int32_t rs2 = (int32_t)(uint32_t)REG_LOAD(HOLE_RM_OFF);
    int64_t result = (int64_t)(rs1 + rs2);
    REG_STORE(HOLE_RD_OFF, (uint64_t)result);
}

void stencil_rv_subw(uint64_t* regs, uint8_t* mem) {
    int32_t rs1 = (int32_t)(uint32_t)REG_LOAD(HOLE_RN_OFF);
    int32_t rs2 = (int32_t)(uint32_t)REG_LOAD(HOLE_RM_OFF);
    int64_t result = (int64_t)(rs1 - rs2);
    REG_STORE(HOLE_RD_OFF, (uint64_t)result);
}

/* ═══════════════════════════════════════════════════════════════════════════
 * Loads (via helper function pointer)
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_rv_lb(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rs1 + (uint64_t)imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    if (mr(mem, addr, 1, &val) == 0) {
        int64_t sval = (int64_t)(int8_t)(uint8_t)val;
        REG_STORE(HOLE_RD_OFF, (uint64_t)sval);
    }
}

void stencil_rv_lh(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rs1 + (uint64_t)imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    if (mr(mem, addr, 2, &val) == 0) {
        int64_t sval = (int64_t)(int16_t)(uint16_t)val;
        REG_STORE(HOLE_RD_OFF, (uint64_t)sval);
    }
}

void stencil_rv_lw(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rs1 + (uint64_t)imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    if (mr(mem, addr, 4, &val) == 0) {
        int64_t sval = (int64_t)(int32_t)(uint32_t)val;
        REG_STORE(HOLE_RD_OFF, (uint64_t)sval);
    }
}

void stencil_rv_ld(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rs1 + (uint64_t)imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    if (mr(mem, addr, 8, &val) == 0) {
        REG_STORE(HOLE_RD_OFF, val);
    }
}

void stencil_rv_lbu(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rs1 + (uint64_t)imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    if (mr(mem, addr, 1, &val) == 0) {
        REG_STORE(HOLE_RD_OFF, val & 0xFF);
    }
}

void stencil_rv_lhu(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rs1 + (uint64_t)imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    if (mr(mem, addr, 2, &val) == 0) {
        REG_STORE(HOLE_RD_OFF, val & 0xFFFF);
    }
}

void stencil_rv_lwu(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rs1 + (uint64_t)imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    if (mr(mem, addr, 4, &val) == 0) {
        REG_STORE(HOLE_RD_OFF, val & 0xFFFFFFFF);
    }
}

/* ═══════════════════════════════════════════════════════════════════════════
 * Stores (via helper function pointer)
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_rv_sb(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rs1 + (uint64_t)imm;
    mem_write_fn mw = GET_MEM_WRITE(regs);
    mw(mem, addr, rs2 & 0xFF, 1);
}

void stencil_rv_sh(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rs1 + (uint64_t)imm;
    mem_write_fn mw = GET_MEM_WRITE(regs);
    mw(mem, addr, rs2 & 0xFFFF, 2);
}

void stencil_rv_sw(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rs1 + (uint64_t)imm;
    mem_write_fn mw = GET_MEM_WRITE(regs);
    mw(mem, addr, rs2 & 0xFFFFFFFF, 4);
}

void stencil_rv_sd(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rs1 + (uint64_t)imm;
    mem_write_fn mw = GET_MEM_WRITE(regs);
    mw(mem, addr, rs2, 8);
}

/* ═══════════════════════════════════════════════════════════════════════════
 * Branches (terminators)
 * ═══════════════════════════════════════════════════════════════════════════ */

uint64_t stencil_rv_beq(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    *(uint64_t*)((char*)regs + PC_OFF) = (rs1 == rs2) ? target : next_pc;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_rv_bne(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    *(uint64_t*)((char*)regs + PC_OFF) = (rs1 != rs2) ? target : next_pc;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_rv_blt(uint64_t* regs, uint8_t* mem) {
    int64_t rs1 = (int64_t)REG_LOAD(HOLE_RN_OFF);
    int64_t rs2 = (int64_t)REG_LOAD(HOLE_RM_OFF);
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    *(uint64_t*)((char*)regs + PC_OFF) = (rs1 < rs2) ? target : next_pc;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_rv_bge(uint64_t* regs, uint8_t* mem) {
    int64_t rs1 = (int64_t)REG_LOAD(HOLE_RN_OFF);
    int64_t rs2 = (int64_t)REG_LOAD(HOLE_RM_OFF);
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    *(uint64_t*)((char*)regs + PC_OFF) = (rs1 >= rs2) ? target : next_pc;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_rv_bltu(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    *(uint64_t*)((char*)regs + PC_OFF) = (rs1 < rs2) ? target : next_pc;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_rv_bgeu(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    *(uint64_t*)((char*)regs + PC_OFF) = (rs1 >= rs2) ? target : next_pc;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_rv_jal(uint64_t* regs, uint8_t* mem) {
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    REG_STORE(HOLE_RD_OFF, next_pc);
    *(uint64_t*)((char*)regs + PC_OFF) = target;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_rv_jalr(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    uint64_t target = (rs1 + (uint64_t)imm) & ~1ULL;
    REG_STORE(HOLE_RD_OFF, next_pc);
    *(uint64_t*)((char*)regs + PC_OFF) = target;
    return EXIT_END_OF_BLOCK;
}

/* ═══════════════════════════════════════════════════════════════════════════
 * Upper Immediate (U-type)
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_rv_lui(uint64_t* regs, uint8_t* mem) {
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, (uint64_t)imm);
}

void stencil_rv_auipc(uint64_t* regs, uint8_t* mem) {
    /* IMM is the already-shifted upper immediate, TARGET has pc value */
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t pc = (uint64_t)(uintptr_t)HOLE_TARGET; /* we reuse HOLE_TARGET for current PC */
    REG_STORE(HOLE_RD_OFF, pc + (uint64_t)imm);
}

/* ═══════════════════════════════════════════════════════════════════════════
 * Multiply/Divide (RV64M)
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_rv_mul(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rs1 * rs2);
}

void stencil_rv_div(uint64_t* regs, uint8_t* mem) {
    int64_t rs1 = (int64_t)REG_LOAD(HOLE_RN_OFF);
    int64_t rs2 = (int64_t)REG_LOAD(HOLE_RM_OFF);
    if (rs2 == 0) {
        REG_STORE(HOLE_RD_OFF, (uint64_t)-1);
    } else if (rs1 == (int64_t)0x8000000000000000LL && rs2 == -1) {
        REG_STORE(HOLE_RD_OFF, (uint64_t)rs1); /* overflow */
    } else {
        REG_STORE(HOLE_RD_OFF, (uint64_t)(rs1 / rs2));
    }
}

void stencil_rv_divu(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    if (rs2 == 0) {
        REG_STORE(HOLE_RD_OFF, (uint64_t)-1);
    } else {
        REG_STORE(HOLE_RD_OFF, rs1 / rs2);
    }
}

void stencil_rv_rem(uint64_t* regs, uint8_t* mem) {
    int64_t rs1 = (int64_t)REG_LOAD(HOLE_RN_OFF);
    int64_t rs2 = (int64_t)REG_LOAD(HOLE_RM_OFF);
    if (rs2 == 0) {
        REG_STORE(HOLE_RD_OFF, (uint64_t)rs1);
    } else if (rs1 == (int64_t)0x8000000000000000LL && rs2 == -1) {
        REG_STORE(HOLE_RD_OFF, 0);
    } else {
        REG_STORE(HOLE_RD_OFF, (uint64_t)(rs1 % rs2));
    }
}

void stencil_rv_remu(uint64_t* regs, uint8_t* mem) {
    uint64_t rs1 = REG_LOAD(HOLE_RN_OFF);
    uint64_t rs2 = REG_LOAD(HOLE_RM_OFF);
    if (rs2 == 0) {
        REG_STORE(HOLE_RD_OFF, rs1);
    } else {
        REG_STORE(HOLE_RD_OFF, rs1 % rs2);
    }
}

/* ═══════════════════════════════════════════════════════════════════════════
 * System — ECALL (terminator)
 * ═══════════════════════════════════════════════════════════════════════════ */

uint64_t stencil_rv_ecall(uint64_t* regs, uint8_t* mem) {
    return EXIT_SYSCALL;
}
