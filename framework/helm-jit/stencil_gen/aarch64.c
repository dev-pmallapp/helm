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
    PRESERVE_MEM();
}


void stencil_sub_imm(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t result = rn - imm;
    REG_STORE(HOLE_RD_OFF, result);
    PRESERVE_MEM();
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
    PRESERVE_MEM();
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
    PRESERVE_MEM();
}


void stencil_and_imm(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, rn & imm);
    PRESERVE_MEM();
}


void stencil_orr_imm(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, rn | imm);
    PRESERVE_MEM();
}


void stencil_eor_imm(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, rn ^ imm);
    PRESERVE_MEM();
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
    PRESERVE_MEM();
}


/* ═══════════════════════════════════════════════════════════════════════════
 * MOV variants
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_movz(uint64_t* regs, uint8_t* mem) {
    /* Decoder pre-computes final value in imm (shifted, not inverted) */
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, imm);
    PRESERVE_MEM();
}


void stencil_movn(uint64_t* regs, uint8_t* mem) {
    /* Decoder pre-computes final value (~(imm16 << hw)) in imm */
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    REG_STORE(HOLE_RD_OFF, imm);
    PRESERVE_MEM();
}


void stencil_movk(uint64_t* regs, uint8_t* mem) {
    /* MOVK keeps other bits; IMM=raw imm16, SHAMT=hw*16 */
    uint64_t rd = REG_LOAD(HOLE_RD_OFF);
    uint64_t imm16 = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t shift = (uint64_t)(uintptr_t)HOLE_SHAMT;
    uint64_t mask = ~(0xFFFFULL << shift);
    rd = (rd & mask) | (imm16 << shift);
    REG_STORE(HOLE_RD_OFF, rd);
    PRESERVE_MEM();
}


/* ═══════════════════════════════════════════════════════════════════════════
 * Data Processing — Register
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_add_reg(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rn + rm);
    PRESERVE_MEM();
}


void stencil_sub_reg(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rn - rm);
    PRESERVE_MEM();
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
    PRESERVE_MEM();
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
    PRESERVE_MEM();
}


/* ═══════════════════════════════════════════════════════════════════════════
 * Logical — Register
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_and_reg(uint64_t* regs, uint8_t* mem) {
    REG_STORE(HOLE_RD_OFF, REG_LOAD(HOLE_RN_OFF) & REG_LOAD(HOLE_RM_OFF));
    PRESERVE_MEM();
}


void stencil_orr_reg(uint64_t* regs, uint8_t* mem) {
    REG_STORE(HOLE_RD_OFF, REG_LOAD(HOLE_RN_OFF) | REG_LOAD(HOLE_RM_OFF));
    PRESERVE_MEM();
}


void stencil_eor_reg(uint64_t* regs, uint8_t* mem) {
    REG_STORE(HOLE_RD_OFF, REG_LOAD(HOLE_RN_OFF) ^ REG_LOAD(HOLE_RM_OFF));
    PRESERVE_MEM();
}


void stencil_orn_reg(uint64_t* regs, uint8_t* mem) {
    REG_STORE(HOLE_RD_OFF, REG_LOAD(HOLE_RN_OFF) | ~REG_LOAD(HOLE_RM_OFF));
    PRESERVE_MEM();
}


void stencil_bic_reg(uint64_t* regs, uint8_t* mem) {
    REG_STORE(HOLE_RD_OFF, REG_LOAD(HOLE_RN_OFF) & ~REG_LOAD(HOLE_RM_OFF));
    PRESERVE_MEM();
}


/* ═══════════════════════════════════════════════════════════════════════════
 * Bitfield (SBFM/UBFM) — decoder pre-computes result in imm for common aliases
 * For general case: immr in IMM, imms in SHAMT
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_sbfm_ext(uint64_t* regs, uint8_t* mem) {
    /* Sign-extending extraction (ASR, SXTB, SBFX, etc.): imms >= immr */
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    unsigned immr = (unsigned)(uintptr_t)HOLE_IMM & 63;
    unsigned imms = (unsigned)(uintptr_t)HOLE_SHAMT & 63;
    unsigned lsl = 63 - imms;
    int64_t result = (int64_t)(rn << lsl) >> (lsl + immr);
    REG_STORE(HOLE_RD_OFF, (uint64_t)result);
    PRESERVE_MEM();
}


void stencil_sbfm_ins(uint64_t* regs, uint8_t* mem) {
    /* Sign-extending insertion (SBFIZ): imms < immr */
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    unsigned immr = (unsigned)(uintptr_t)HOLE_IMM & 63;
    unsigned imms = (unsigned)(uintptr_t)HOLE_SHAMT & 63;
    int64_t result = (int64_t)(rn << (63 - imms)) >> (immr - imms - 1);
    REG_STORE(HOLE_RD_OFF, (uint64_t)result);
    PRESERVE_MEM();
}


void stencil_ubfm_ext(uint64_t* regs, uint8_t* mem) {
    /* Zero-extending extraction (LSR, UBFX, UXTB, etc.): imms >= immr */
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t immr = (uint64_t)(uintptr_t)HOLE_IMM & 63;
    uint64_t imms = (uint64_t)(uintptr_t)HOLE_SHAMT & 63;
    uint64_t width = imms - immr + 1;
    uint64_t result = rn >> immr;
    /* Mask to extraction width; when width==64 (immr==0) mask is all-ones */
    uint64_t mask = (width >= 64) ? ~0ULL : (1ULL << width) - 1;
    REG_STORE(HOLE_RD_OFF, result & mask);
    PRESERVE_MEM();
}


void stencil_ubfm_ins(uint64_t* regs, uint8_t* mem) {
    /* Zero-extending insertion (LSL, UBFIZ): imms < immr */
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t immr = (uint64_t)(uintptr_t)HOLE_IMM & 63;
    uint64_t imms = (uint64_t)(uintptr_t)HOLE_SHAMT & 63;
    uint64_t result = (rn & ((1ULL << (imms + 1)) - 1)) << (64 - immr);
    REG_STORE(HOLE_RD_OFF, result);
    PRESERVE_MEM();
}


/* ═══════════════════════════════════════════════════════════════════════════
 * PC-relative addressing
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_adr(uint64_t* regs, uint8_t* mem) {
    /* IMM = pre-computed pc + offset */
    REG_STORE(HOLE_RD_OFF, (uint64_t)(uintptr_t)HOLE_IMM);
    PRESERVE_MEM();
}


void stencil_adrp(uint64_t* regs, uint8_t* mem) {
    /* IMM = pre-computed (pc & ~0xFFF) + (imm << 12) */
    REG_STORE(HOLE_RD_OFF, (uint64_t)(uintptr_t)HOLE_IMM);
    PRESERVE_MEM();
}


/* ═══════════════════════════════════════════════════════════════════════════
 * Conditional select
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_csel(uint64_t* regs, uint8_t* mem) {
    /* Condition evaluated at runtime from NZCV + IMM (cond code) */
    uint32_t nzcv = *(uint32_t*)((char*)regs + NZCV_OFF);
    uint64_t cond_val = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    int n = (nzcv >> 31) & 1, z = (nzcv >> 30) & 1;
    int c = (nzcv >> 29) & 1, v = (nzcv >> 28) & 1;
    uint64_t cc = cond_val >> 1;
    int taken;
    if      (cc == 0) taken = z;
    else if (cc == 1) taken = c;
    else if (cc == 2) taken = n;
    else if (cc == 3) taken = v;
    else if (cc == 4) taken = c & !z;
    else if (cc == 5) taken = (n == v);
    else if (cc == 6) taken = (n == v) & !z;
    else              taken = 1;
    if ((cond_val & 1) && cc != 7) taken = !taken;
    REG_STORE(HOLE_RD_OFF, taken ? rn : rm);
    PRESERVE_MEM();
}


void stencil_csinc(uint64_t* regs, uint8_t* mem) {
    uint32_t nzcv = *(uint32_t*)((char*)regs + NZCV_OFF);
    uint64_t cond_val = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    int n = (nzcv >> 31) & 1, z = (nzcv >> 30) & 1;
    int c = (nzcv >> 29) & 1, v = (nzcv >> 28) & 1;
    uint64_t cc = cond_val >> 1;
    int taken;
    if      (cc == 0) taken = z;
    else if (cc == 1) taken = c;
    else if (cc == 2) taken = n;
    else if (cc == 3) taken = v;
    else if (cc == 4) taken = c & !z;
    else if (cc == 5) taken = (n == v);
    else if (cc == 6) taken = (n == v) & !z;
    else              taken = 1;
    if ((cond_val & 1) && cc != 7) taken = !taken;
    REG_STORE(HOLE_RD_OFF, taken ? rn : rm + 1);
    PRESERVE_MEM();
}


/* ═══════════════════════════════════════════════════════════════════════════
 * Multiply/Divide
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_madd(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    uint64_t ra = REG_LOAD(HOLE_RA_OFF);
    REG_STORE(HOLE_RD_OFF, ra + rn * rm);
    PRESERVE_MEM();
}


void stencil_msub(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    uint64_t ra = REG_LOAD(HOLE_RA_OFF);
    REG_STORE(HOLE_RD_OFF, ra - rn * rm);
    PRESERVE_MEM();
}


void stencil_sdiv(uint64_t* regs, uint8_t* mem) {
    int64_t rn = (int64_t)REG_LOAD(HOLE_RN_OFF);
    int64_t rm = (int64_t)REG_LOAD(HOLE_RM_OFF);
    if (rm == 0) {
        REG_STORE(HOLE_RD_OFF, 0);
    } else if (rn == (int64_t)0x8000000000000000LL && rm == -1) {
        REG_STORE(HOLE_RD_OFF, (uint64_t)rn);
    } else {
        REG_STORE(HOLE_RD_OFF, (uint64_t)(rn / rm));
    }
    PRESERVE_MEM();
}


void stencil_udiv(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    REG_STORE(HOLE_RD_OFF, rm == 0 ? 0 : rn / rm);
    PRESERVE_MEM();
}


/* ═══════════════════════════════════════════════════════════════════════════
 * Miscellaneous DP
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_extr(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    uint64_t lsb = (uint64_t)(uintptr_t)HOLE_SHAMT & 63;
    uint64_t result = (rm >> lsb) | (rn << (64 - lsb));
    if (lsb == 0) result = rm; /* EXTR with lsb=0 is MOV */
    REG_STORE(HOLE_RD_OFF, result);
    PRESERVE_MEM();
}


void stencil_clz(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    REG_STORE(HOLE_RD_OFF, rn == 0 ? 64 : (uint64_t)__builtin_clzll(rn));
    PRESERVE_MEM();
}


void stencil_rev(uint64_t* regs, uint8_t* mem) {
    REG_STORE(HOLE_RD_OFF, __builtin_bswap64(REG_LOAD(HOLE_RN_OFF)));
    PRESERVE_MEM();
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
 * Conditional select — additional variants
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_csinv(uint64_t* regs, uint8_t* mem) {
    uint32_t nzcv = *(uint32_t*)((char*)regs + NZCV_OFF);
    uint64_t cond_val = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    int n = (nzcv >> 31) & 1, z = (nzcv >> 30) & 1;
    int c = (nzcv >> 29) & 1, v = (nzcv >> 28) & 1;
    uint64_t cc = cond_val >> 1;
    int taken;
    if      (cc == 0) taken = z;
    else if (cc == 1) taken = c;
    else if (cc == 2) taken = n;
    else if (cc == 3) taken = v;
    else if (cc == 4) taken = c & !z;
    else if (cc == 5) taken = (n == v);
    else if (cc == 6) taken = (n == v) & !z;
    else              taken = 1;
    if ((cond_val & 1) && cc != 7) taken = !taken;
    REG_STORE(HOLE_RD_OFF, taken ? rn : ~rm);
    PRESERVE_MEM();
}


void stencil_csneg(uint64_t* regs, uint8_t* mem) {
    uint32_t nzcv = *(uint32_t*)((char*)regs + NZCV_OFF);
    uint64_t cond_val = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rm = REG_LOAD(HOLE_RM_OFF);
    int n = (nzcv >> 31) & 1, z = (nzcv >> 30) & 1;
    int c = (nzcv >> 29) & 1, v = (nzcv >> 28) & 1;
    uint64_t cc = cond_val >> 1;
    int taken;
    if      (cc == 0) taken = z;
    else if (cc == 1) taken = c;
    else if (cc == 2) taken = n;
    else if (cc == 3) taken = v;
    else if (cc == 4) taken = c & !z;
    else if (cc == 5) taken = (n == v);
    else if (cc == 6) taken = (n == v) & !z;
    else              taken = 1;
    if ((cond_val & 1) && cc != 7) taken = !taken;
    REG_STORE(HOLE_RD_OFF, taken ? rn : (uint64_t)(-(int64_t)rm));
    PRESERVE_MEM();
}


/* ═══════════════════════════════════════════════════════════════════════════
 * Unscaled loads/stores (LDUR/STUR)
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_ldur64(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rn + (uint64_t)imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    if (mr(mem, addr, 8, &val) == 0) {
        REG_STORE(HOLE_RD_OFF, val);
    }
}

void stencil_ldur32(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rn + (uint64_t)imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t val;
    if (mr(mem, addr, 4, &val) == 0) {
        REG_STORE(HOLE_RD_OFF, val & 0xFFFFFFFF);
    }
}

void stencil_stur64(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rt = REG_LOAD(HOLE_RT_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rn + (uint64_t)imm;
    mem_write_fn mw = GET_MEM_WRITE(regs);
    mw(mem, addr, rt, 8);
}

void stencil_stur32(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rt = REG_LOAD(HOLE_RT_OFF);
    int64_t imm = (int64_t)(intptr_t)HOLE_IMM;
    uint64_t addr = rn + (uint64_t)imm;
    mem_write_fn mw = GET_MEM_WRITE(regs);
    mw(mem, addr, rt & 0xFFFFFFFF, 4);
}

/* ═══════════════════════════════════════════════════════════════════════════
 * TBZ/TBNZ (test bit and branch)
 * ═══════════════════════════════════════════════════════════════════════════ */

uint64_t stencil_tbz(uint64_t* regs, uint8_t* mem) {
    uint64_t rt = REG_LOAD(HOLE_RT_OFF);
    uint64_t bit = (uint64_t)(uintptr_t)HOLE_SHAMT;
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    *(uint64_t*)((char*)regs + PC_OFF) = ((rt >> bit) & 1) == 0 ? target : next_pc;
    return EXIT_END_OF_BLOCK;
}

uint64_t stencil_tbnz(uint64_t* regs, uint8_t* mem) {
    uint64_t rt = REG_LOAD(HOLE_RT_OFF);
    uint64_t bit = (uint64_t)(uintptr_t)HOLE_SHAMT;
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;
    *(uint64_t*)((char*)regs + PC_OFF) = ((rt >> bit) & 1) != 0 ? target : next_pc;
    return EXIT_END_OF_BLOCK;
}

/* ═══════════════════════════════════════════════════════════════════════════
 * SVC (supervisor call — exits block with EXIT_SYSCALL)
 * ═══════════════════════════════════════════════════════════════════════════ */

uint64_t stencil_svc(uint64_t* regs, uint8_t* mem) {
    return EXIT_SYSCALL;
}

/* ═══════════════════════════════════════════════════════════════════════════
 * Load/Store Pair (LDP/STP) — via helper function pointers
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_ldp64(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t v1, v2;
    if (mr(mem, addr, 8, &v1) == 0) {
        REG_STORE(HOLE_RT_OFF, v1);
        if (mr(mem, addr + 8, 8, &v2) == 0) {
            REG_STORE(HOLE_RT2_OFF, v2);
        }
    }
}

void stencil_ldp32(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_read_fn mr = GET_MEM_READ(regs);
    uint64_t v1, v2;
    if (mr(mem, addr, 4, &v1) == 0) {
        REG_STORE(HOLE_RT_OFF, v1 & 0xFFFFFFFF);
        if (mr(mem, addr + 4, 4, &v2) == 0) {
            REG_STORE(HOLE_RT2_OFF, v2 & 0xFFFFFFFF);
        }
    }
}

void stencil_stp64(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rt1 = REG_LOAD(HOLE_RT_OFF);
    uint64_t rt2 = REG_LOAD(HOLE_RT2_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_write_fn mw = GET_MEM_WRITE(regs);
    mw(mem, addr, rt1, 8);
    mw(mem, addr + 8, rt2, 8);
}

void stencil_stp32(uint64_t* regs, uint8_t* mem) {
    uint64_t rn = REG_LOAD(HOLE_RN_OFF);
    uint64_t rt1 = REG_LOAD(HOLE_RT_OFF);
    uint64_t rt2 = REG_LOAD(HOLE_RT2_OFF);
    uint64_t imm = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t addr = rn + imm;
    mem_write_fn mw = GET_MEM_WRITE(regs);
    mw(mem, addr, rt1 & 0xFFFFFFFF, 4);
    mw(mem, addr + 4, rt2 & 0xFFFFFFFF, 4);
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
     * Simplified BCond: the field extraction pre-evaluates the condition
     * and passes the result as HOLE_IMM (1=taken, 0=not-taken).
     *
     * WAIT — we can't pre-evaluate because NZCV is a runtime value!
     * Instead, we use a bitmask approach: pack the 4 NZCV bits into a
     * 4-bit index and use a 16-bit bitmask per condition to look up
     * taken/not-taken. The bitmask is passed as HOLE_IMM.
     *
     * For condition code `cc`, the bitmask has bit[nzcv_4bit] set if
     * that NZCV combination means "taken" for this condition.
     */
    uint32_t nzcv = *(uint32_t*)((char*)regs + NZCV_OFF);
    uint64_t cond_mask = (uint64_t)(uintptr_t)HOLE_IMM;
    uint64_t target = (uint64_t)(uintptr_t)HOLE_TARGET;
    uint64_t next_pc = (uint64_t)(uintptr_t)HOLE_NEXT_PC;

    /* Extract NZCV as a 4-bit index: N=bit3, Z=bit2, C=bit1, V=bit0 */
    uint32_t nzcv_idx = (nzcv >> 28) & 0xF;

    /* Lookup: is bit `nzcv_idx` set in the condition mask? */
    int taken = (cond_mask >> nzcv_idx) & 1;

    *(uint64_t*)((char*)regs + PC_OFF) = taken ? target : next_pc;
    return EXIT_END_OF_BLOCK;
}

/* ═══════════════════════════════════════════════════════════════════════════
 * System — NOP (passthrough)
 * ═══════════════════════════════════════════════════════════════════════════ */

void stencil_nop(uint64_t* regs, uint8_t* mem) {
    /* intentionally empty */
    PRESERVE_MEM();
}

