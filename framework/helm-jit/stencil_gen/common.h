/*
 * common.h — Shared macros and hole symbol declarations for stencil functions.
 *
 * Each "hole" is an extern char[] symbol. When the C compiler references it,
 * it generates an R_X86_64_32S relocation in the .o file. The build.rs
 * pipeline maps these symbol names to HoleKind variants and records the
 * relocation offsets.
 *
 * The stencil calling convention matches the JIT block ABI:
 *   rdi = pointer to flat register array [u64; N]
 *   rsi = pointer to FlatMem (passed to memory helpers)
 *   return value in rax = exit code
 */

#ifndef HELM_STENCIL_COMMON_H
#define HELM_STENCIL_COMMON_H

#include <stdint.h>

/* ── Hole symbol declarations ────────────────────────────────────────────── */

/* Register offset holes — patched to byte offset = reg_index * 8 */
extern char HOLE_RD_OFF[];
extern char HOLE_RN_OFF[];
extern char HOLE_RM_OFF[];
extern char HOLE_RA_OFF[];
extern char HOLE_RT_OFF[];
extern char HOLE_RT2_OFF[];

/* Immediate holes */
extern char HOLE_IMM[];      /* zero-extended immediate */
extern char HOLE_SIMM[];     /* signed immediate (pre/post-index) */
extern char HOLE_SHAMT[];    /* shift amount */

/* Address holes */
extern char HOLE_TARGET[];   /* branch target (absolute guest PC) */
extern char HOLE_NEXT_PC[];  /* fallthrough PC */

/* Helper function pointer holes */
extern char HOLE_MEM_READ[];
extern char HOLE_MEM_WRITE[];

/* ── Register access macros ──────────────────────────────────────────────
 *
 * These use the hole symbols as byte offsets into the register array.
 * Example: REG_LOAD(HOLE_RN_OFF) loads the register at the patched offset.
 */

#define REG_LOAD(hole)       (*(uint64_t*)((char*)regs + (uintptr_t)(hole)))
#define REG_STORE(hole, val) (*(uint64_t*)((char*)regs + (uintptr_t)(hole)) = (val))

/* ── Exit codes (must match block.rs) ────────────────────────────────────── */

#define EXIT_END_OF_BLOCK 0
#define EXIT_CHAIN        1
#define EXIT_SYSCALL      2
#define EXIT_EXCEPTION    3

/* ── Memory helper types ─────────────────────────────────────────────────── */

typedef uint64_t (*mem_read_fn)(uint8_t* mem, uint64_t addr, uint32_t size, uint64_t* out);
typedef uint64_t (*mem_write_fn)(uint8_t* mem, uint64_t addr, uint64_t val, uint32_t size);

/* PC slot offset (AArch64: slot 32, RISC-V: slot 32) */
#define PC_OFF (32 * 8)

/* NZCV slot offset (AArch64 only: slot 33) */
#define NZCV_OFF (33 * 8)

/* XZR slot offset (AArch64 only: slot 34) */
#define XZR_OFF (34 * 8)

#endif /* HELM_STENCIL_COMMON_H */
