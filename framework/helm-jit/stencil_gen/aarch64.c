/*
 * aarch64.c — AArch64 guest stencil functions.
 *
 * Each function is a stencil template: it reads registers via hole-based
 * offsets, performs the operation, and writes the result back. Relocations
 * are generated for each use of a HOLE_* symbol.
 *
 * Calling convention: rdi=regs, rsi=mem. Non-terminators fall through;
 * terminators return an exit code in rax.
 */

#include "common.h"

/* ═══════════════════════════════════════════════════════════════════════════
 * Data Processing — Immediate
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_add_imm(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t result = rn + imm;
    REG_STORE(HOLE_RD_OFF, result);
}

void stencil_sub_imm(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t result = rn - imm;
    REG_STORE(HOLE_RD_OFF, result);
}

void stencil_adds_imm(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t result = rn + imm;
    REG_STORE(HOLE_RD_OFF, result);

    /* Capture NZCV flags via inline asm after the add */
    uint32_t nzcv;
    __asm__ volatile(
        "addq %[imm_val], %[rn_val]\n\t"
        "pushfq\n\t"
        "popq %%rax\n\t"
        /* Build NZCV: N=bit31(result), Z=ZF, C=CF, V=OF */
        "xorl %[nzcv], %[nzcv]\n\t"
        "bt $7, %%eax\n\t"          /* SF → N */
        "jnc 1f\n\t"
        "orl $0x80000000, %[nzcv]\n\t"
        "1:\n\t"
        "bt $6, %%eax\n\t"          /* ZF → Z */
        "jnc 2f\n\t"
        "orl $0x40000000, %[nzcv]\n\t"
        "2:\n\t"
        "bt $0, %%eax\n\t"          /* CF → C */
        "jnc 3f\n\t"
        "orl $0x20000000, %[nzcv]\n\t"
        "3:\n\t"
        "bt $11, %%eax\n\t"         /* OF → V */
        "jnc 4f\n\t"
        "orl $0x10000000, %[nzcv]\n\t"
        "4:\n\t"
        : [nzcv] "=&r"(nzcv)
        : [rn_val] "r"(rn), [imm_val] "r"(imm)
        : "rax", "cc", "memory"
    );
    *(uint32_t*)((char*)regs + NZCV_OFF) = nzcv;
}

void stencil_subs_imm(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t result = rn - imm;
    REG_STORE(HOLE_RD_OFF, result);

    /* Capture NZCV flags — note ARM C = !x86 CF for subtraction */
    uint32_t nzcv;
    __asm__ volatile(
        "subq %[imm_val], %[rn_val]\n\t"
        "pushfq\n\t"
        "popq %%rax\n\t"
        "xorl %[nzcv], %[nzcv]\n\t"
        "bt $7, %%eax\n\t"
        "jnc 1f\n\t"
        "orl $0x80000000, %[nzcv]\n\t"
        "1:\n\t"
        "bt $6, %%eax\n\t"
        "jnc 2f\n\t"
        "orl $0x40000000, %[nzcv]\n\t"
        "2:\n\t"
        "bt $0, %%eax\n\t"          /* CF → invert for ARM C */
        "jc 3f\n\t"                 /* CF=1 means borrow → ARM C=0 */
        "orl $0x20000000, %[nzcv]\n\t"
        "3:\n\t"
        "bt $11, %%eax\n\t"
        "jnc 4f\n\t"
        "orl $0x10000000, %[nzcv]\n\t"
        "4:\n\t"
        : [nzcv] "=&r"(nzcv)
        : [rn_val] "r"(rn), [imm_val] "r"(imm)
        : "rax", "cc", "memory"
    );
    *(uint32_t*)((char*)regs + NZCV_OFF) = nzcv;
}

void stencil_and_imm(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, rn & imm);
}

void stencil_orr_imm(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, rn | imm);
}

void stencil_eor_imm(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, rn ^ imm);
}

void stencil_ands_imm(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t result = rn & imm;
    REG_STORE(HOLE_RD_OFF, result);

    /* Logical ops: N from bit63, Z from result==0, C=0, V=0 */
    uint32_t nzcv = 0;
    if (result & (1ULL << 63)) nzcv |= 0x80000000;
    if (result == 0) nzcv |= 0x40000000;
    *(uint32_t*)((char*)regs + NZCV_OFF) = nzcv;
}

/* ═══════════════════════════════════════════════════════════════════════════
 * MOV variants
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_movz(uint64_t* regs, uint8_t* mem) {
    /* Decoder pre-computes final value in imm (shifted, not inverted) */
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, imm);
}

void stencil_movn(uint64_t* regs, uint8_t* mem) {
    /* Decoder pre-computes final value (~(imm16 << hw)) in imm */
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, imm);
}

void stencil_movk(uint64_t* regs, uint8_t* mem) {
    /* MOVK keeps other bits; IMM=raw imm16, SHAMT=hw*16 */
    uint64_t rd = REG_LOAD(HOLE_RD_OFF);
    uint64_t imm16 = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t shift = (uint64_t)(uintptr_t)HOLE_SHAMT;
    uint64_t mask = ~(0xFFFFULL << shift);
    rd = (rd & mask) | (imm16 << shift);
    REG_STORE(HOLE_RD_OFF, rd);
}

/* ═══════════════════════════════════════════════════════════════════════════
 * Data Processing — Register
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_add_reg(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rn + rm);
}

void stencil_sub_reg(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rn - rm);
}

void stencil_adds_reg(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    uint64_t result = rn + rm;
    REG_STORE(HOLE_RD_OFF, result);

    uint32_t nzcv;
    __asm__ volatile(
        "addq %[rm_val], %[rn_val]\n\t"
        "pushfq\n\t"
        "popq %%rax\n\t"
        "xorl %[nzcv], %[nzcv]\n\t"
        "bt $7, %%eax\n\t"
        "jnc 1f\n\t"
        "orl $0x80000000, %[nzcv]\n\t"
        "1: bt $6, %%eax\n\t"
        "jnc 2f\n\t"
        "orl $0x40000000, %[nzcv]\n\t"
        "2: bt $0, %%eax\n\t"
        "jnc 3f\n\t"
        "orl $0x20000000, %[nzcv]\n\t"
        "3: bt $11, %%eax\n\t"
        "jnc 4f\n\t"
        "orl $0x10000000, %[nzcv]\n\t"
        "4:\n\t"
        : [nzcv] "=&r"(nzcv)
        : [rn_val] "r"(rn), [rm_val] "r"(rm)
        : "rax", "cc", "memory"
    );
    *(uint32_t*)((char*)regs + NZCV_OFF) = nzcv;
}

void stencil_subs_reg(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    uint64_t result = rn - rm;
    REG_STORE(HOLE_RD_OFF, result);

    uint32_t nzcv;
    __asm__ volatile(
        "subq %[rm_val], %[rn_val]\n\t"
        "pushfq\n\t"
        "popq %%rax\n\t"
        "xorl %[nzcv], %[nzcv]\n\t"
        "bt $7, %%eax\n\t"
        "jnc 1f\n\t"
        "orl $0x80000000, %[nzcv]\n\t"
        "1: bt $6, %%eax\n\t"
        "jnc 2f\n\t"
        "orl $0x40000000, %[nzcv]\n\t"
        "2: bt $0, %%eax\n\t"
        "jc 3f\n\t"
        "orl $0x20000000, %[nzcv]\n\t"
        "3: bt $11, %%eax\n\t"
        "jnc 4f\n\t"
        "orl $0x10000000, %[nzcv]\n\t"
        "4:\n\t"
        : [nzcv] "=&r"(nzcv)
        : [rn_val] "r"(rn), [rm_val] "r"(rm)
        : "rax", "cc", "memory"
    );
    *(uint32_t*)((char*)regs + NZCV_OFF) = nzcv;
}

/* ═══════════════════════════════════════════════════════════════════════════
 * Loads (via helper function pointer)
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_ldr64(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    uint64_t err = mr(mem, addr, 8, &val);
    if (err == 0) {
        REG_STORE(HOLE_RD_OFF, val);
    }
    /* On fault, the engine checks EXIT_EXCEPTION — but stencils don't
       handle faults; the block is re-executed via interpreter. */
}

void stencil_ldr32(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    uint64_t err = mr(mem, addr, 4, &val);
    if (err == 0) {
        REG_STORE(HOLE_RD_OFF, val & 0xFFFFFFFF);
    }
}

void stencil_ldr16(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    uint64_t err = mr(mem, addr, 2, &val);
    if (err == 0) {
        REG_STORE(HOLE_RD_OFF, val & 0xFFFF);
    }
}

void stencil_ldr8(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    uint64_t err = mr(mem, addr, 1, &val);
    if (err == 0) {
        REG_STORE(HOLE_RD_OFF, val & 0xFF);
    }
}

void stencil_ldrsw(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    uint64_t err = mr(mem, addr, 4, &val);
    if (err == 0) {
        /* Sign-extend from 32 bits */
        int64_t sval = (int64_t)(int32_t)(uint32_t)val;
        REG_STORE(HOLE_RD_OFF, (uint64_t)sval);
    }
}

void stencil_ldrsh(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    uint64_t err = mr(mem, addr, 2, &val);
    if (err == 0) {
        int64_t sval = (int64_t)(int16_t)(uint16_t)val;
        REG_STORE(HOLE_RD_OFF, (uint64_t)sval);
    }
}

void stencil_ldrsb(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    uint64_t err = mr(mem, addr, 1, &val);
    if (err == 0) {
        int64_t sval = (int64_t)(int8_t)(uint8_t)val;
        REG_STORE(HOLE_RD_OFF, (uint64_t)sval);
    }
}

/* ═══════════════════════════════════════════════════════════════════════════
 * Stores (via helper function pointer)
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_str64(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rt = REG_LOAD(HOLE_RT_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_write_fn mw = GET_MEM_WRITE(regs);
    mw(mem, addr, rt, 8);
}

void stencil_str32(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rt = REG_LOAD(HOLE_RT_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_write_fn mw = GET_MEM_WRITE(regs);
    mw(mem, addr, rt & 0xFFFFFFFF, 4);
}

void stencil_str16(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rt = REG_LOAD(HOLE_RT_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_write_fn mw = GET_MEM_WRITE(regs);
    mw(mem, addr, rt & 0xFFFF, 2);
}

void stencil_str8(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rt = REG_LOAD(HOLE_RT_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_write_fn mw = GET_MEM_WRITE(regs);
    mw(mem, addr, rt & 0xFF, 1);
}

/* ═══════════════════════════════════════════════════════════════════════════
 * Branches (terminators — return exit code)
 * ═══════════════════════════════════════════════════════════════════════════ */

uint64_t stencil_b(uint64_t* regs, uint8_t* mem) {
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    *(uint64_t*)((char*)regs + PC_OFF) = target;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_bl(uint64_t* regs, uint8_t* mem) {
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    /* X30 = return address */
    *(uint64_t*)((char*)regs + 30 * 8) = next_pc;
    *(uint64_t*)((char*)regs + PC_OFF) = target;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_br(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    *(uint64_t*)((char*)regs + PC_OFF) = rn;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_blr(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    *(uint64_t*)((char*)regs + 30 * 8) = next_pc;
    *(uint64_t*)((char*)regs + PC_OFF) = rn;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_ret(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    *(uint64_t*)((char*)regs + PC_OFF) = rn;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_cbz(uint64_t* regs, uint8_t* mem) {
    uint64_t rt = REG_LOAD(HOLE_RT_OFF);
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    if (rt == 0) {
        *(uint64_t*)((char*)regs + PC_OFF) = target;
    } else {
        *(uint64_t*)((char*)regs + PC_OFF) = next_pc;
    }
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_cbnz(uint64_t* regs, uint8_t* mem) {
    uint64_t rt = REG_LOAD(HOLE_RT_OFF);
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    if (rt != 0) {
        *(uint64_t*)((char*)regs + PC_OFF) = target;
    } else {
        *(uint64_t*)((char*)regs + PC_OFF) = next_pc;
    }
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_bcond(uint64_t* regs, uint8_t* mem) {
    /*
     * Condition code evaluation without switch/jump-table.
     * Uses if/else chain to avoid .rodata references.
     */
    uint32_t nzcv = *(uint32_t*)((char*)regs + NZCV_OFF);
    uint64_t cond_val = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;

    int n = (nzcv >> 31) & 1;
    int z = (nzcv >> 30) & 1;
    int c = (nzcv >> 29) & 1;
    int v = (nzcv >> 28) & 1;
    uint64_t cc = cond_val >> 1;

    int taken;
    if      (cc == 0) taken = z;                /* EQ/NE */
    else if (cc == 1) taken = c;                /* CS/CC */
    else if (cc == 2) taken = n;                /* MI/PL */
    else if (cc == 3) taken = v;                /* VS/VC */
    else if (cc == 4) taken = c & !z;           /* HI/LS */
    else if (cc == 5) taken = (n == v);          /* GE/LT */
    else if (cc == 6) taken = (n == v) & !z;    /* GT/LE */
    else              taken = 1;                /* AL/NV */

    if ((cond_val & 1) && cc != 7) taken = !taken;

    *(uint64_t*)((char*)regs + PC_OFF) = taken ? target : next_pc;
    return EXIT_END_OF_BLOCK;
}

/* ═══════════════════════════════════════════════════════════════════════════
 * System — NOP (passthrough)
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_nop(uint64_t* regs, uint8_t* mem) {
    /* intentionally empty */
}
