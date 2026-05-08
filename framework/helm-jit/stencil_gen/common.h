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

/* Helper function pointer holes (kept for reference; stencils now load
 * function pointers from fixed register-array slots instead to avoid
 * PLT32 reach issues with mmap'd code). */
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
#define EXIT_SYSCALL      1
#define EXIT_EXCEPTION    2

/* ── Memory helper types ─────────────────────────────────────────────────── */

typedef uint64_t (*mem_read_fn)(uint8_t* mem, uint64_t addr, uint32_t size, uint64_t* out);
typedef uint64_t (*mem_write_fn)(uint8_t* mem, uint64_t addr, uint64_t val, uint32_t size);

/* PC slot offset (AArch64: slot 32, RISC-V: slot 32) */
#define PC_OFF (32 * 8)

/* NZCV slot offset (AArch64 only: slot 33) */
#define NZCV_OFF (33 * 8)

/* XZR slot offset (AArch64 only: slot 34) */
#define XZR_OFF (34 * 8)

/* ── Helper function pointer slots in the register array ─────────────────
 *
 * The engine populates these slots before entering the JIT loop.
 * Stencil load/store functions read the 64-bit function pointer from
 * the register array and use an indirect call (call *rax). This avoids
 * R_X86_64_PLT32 relocations that have only ±2GB reach.
 */
#define JIT_MEM_READ_OFF  (46 * 8)  /* slot 46 */
#define JIT_MEM_WRITE_OFF (47 * 8)  /* slot 47 */

/* Load helper function pointer from register array */
#define GET_MEM_READ(regs)  ((mem_read_fn)(*(uint64_t*)((char*)(regs) + JIT_MEM_READ_OFF)))
#define GET_MEM_WRITE(regs) ((mem_write_fn)(*(uint64_t*)((char*)(regs) + JIT_MEM_WRITE_OFF)))

/* ── 64-bit hole load (R_X86_64_64 via movabsq) ──────────────────────────
 *
 * Forces the compiler to emit `movabsq $HOLE_X, %reg` (10 bytes, full 64-bit
 * immediate) instead of a 32-bit `mov $HOLE_X, %eax`. The build script picks
 * up the R_X86_64_64 reloc and emits RelocKind::Abs64.
 *
 * Use for branch targets / next-PC values in FS mode where guest PCs live in
 * kernel address space (e.g. `0xFFFF_xxxx_xxxx`) and won't fit signed-32.
 *
 * The expression form keeps it usable inside `? :` and assignments without
 * forcing a separate temporary in the source.
 */
#define LOAD_HOLE_64(hole) __extension__ ({                                 \
    uint64_t _v;                                                            \
    __asm__("movabsq $" #hole ", %0" : "=r"(_v));                          \
    _v;                                                                     \
})

/* ── Preserve rsi (mem pointer) across chained stencils ──────────────────
 *
 * Leaf stencils are chained by stripping their trailing `ret` and
 * concatenating bodies.  The `mem` argument lives in rsi and must survive
 * all bodies so that subsequent load/store stencils can use it.
 *
 * Stencils that do NOT reference `mem` must call PRESERVE_MEM() once (at
 * the end is sufficient) so the compiler keeps rsi live.
 */
#define PRESERVE_MEM() __asm__ volatile("" : : "D"(regs), "S"(mem))

#endif /* HELM_STENCIL_COMMON_H */
