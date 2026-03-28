/*
 * bc_runtime.h — Babel-C Runtime Public API
 *
 * All Babel-C programs link against this runtime. It provides the arena
 * allocator, checked arithmetic, dynamic arrays, immutable strings,
 * micro-lib I/O, type-cast helpers, and a panic handler.
 */

#ifndef BC_RUNTIME_H
#define BC_RUNTIME_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ------------------------------------------------------------------ */
/*  Panic handler                                                      */
/* ------------------------------------------------------------------ */

void bc_panic(const char *message, const char *file, int line);

#define BC_PANIC(msg) bc_panic((msg), __FILE__, __LINE__)

/* ------------------------------------------------------------------ */
/*  Arena allocator                                                    */
/* ------------------------------------------------------------------ */

typedef struct bc_arena_block {
    uint8_t *data;
    size_t   used;
    size_t   capacity;
    struct bc_arena_block *next;
} bc_arena_block;

typedef struct {
    bc_arena_block *head;     /* first block (for destroy/reset)  */
    bc_arena_block *current;  /* current allocation block          */
    size_t          block_size; /* default size for new blocks     */
} bc_arena;

#define BC_ARENA_DEFAULT_CAPACITY ((size_t)(1024 * 1024)) /* 1 MB */

bc_arena *bc_arena_create(size_t initial_capacity);
void     *bc_arena_alloc(bc_arena *arena, size_t size);
void      bc_arena_reset(bc_arena *arena);
void      bc_arena_destroy(bc_arena *arena);

/* ------------------------------------------------------------------ */
/*  String type (immutable UTF-8 view)                                 */
/* ------------------------------------------------------------------ */

typedef struct {
    const char *data;
    int32_t     len;
} bc_str;

bc_str   bc_str_from_cstr(const char *s);
bc_str   bc_str_concat(bc_arena *arena, bc_str a, bc_str b);
int32_t  bc_str_len(bc_str s);
uint8_t  bc_str_eq(bc_str a, bc_str b);
bc_str   bc_str_to_cstr(bc_arena *arena, bc_str s);

/* ------------------------------------------------------------------ */
/*  Dynamic array (generic via void* + elem_size)                      */
/* ------------------------------------------------------------------ */

typedef struct {
    void    *data;
    int32_t  len;
    int32_t  capacity;
    int32_t  elem_size;
} bc_array;

bc_array *bc_array_new(bc_arena *arena, int32_t elem_size,
                       int32_t initial_capacity);
void     *bc_array_get(bc_array *arr, int32_t index);
void      bc_array_set(bc_array *arr, int32_t index, void *value);
void      bc_array_push(bc_arena *arena, bc_array *arr, void *value);
int32_t   bc_array_len(bc_array *arr);

/* ------------------------------------------------------------------ */
/*  Result type (generic tagged union via macros)                      */
/* ------------------------------------------------------------------ */

/*
 * Usage:
 *   BC_RESULT_DECL(int32_t, bc_str)
 * Expands to a struct bc_result_int32_t_bc_str with tag + union.
 *
 * For typical Babel-C use the compiler generates concrete typedefs.
 * Here we provide a helper macro and a commonly-used concrete type
 * for read_line: Result<str, str>.
 */

#define BC_RESULT_DECL(ok_type, err_type, name)  \
    typedef struct {                             \
        uint8_t is_ok;                           \
        union {                                  \
            ok_type  ok;                         \
            err_type err;                        \
        } value;                                 \
    } name

/* Result<str, str> used by bc_read_line */
BC_RESULT_DECL(bc_str, bc_str, bc_result_str_str);

/* ------------------------------------------------------------------ */
/*  Checked arithmetic — i32                                           */
/* ------------------------------------------------------------------ */

int32_t bc_add_i32(int32_t a, int32_t b);
int32_t bc_sub_i32(int32_t a, int32_t b);
int32_t bc_mul_i32(int32_t a, int32_t b);
int32_t bc_div_i32(int32_t a, int32_t b);
int32_t bc_mod_i32(int32_t a, int32_t b);
int32_t bc_neg_i32(int32_t a);

/* ------------------------------------------------------------------ */
/*  Checked arithmetic — i64                                           */
/* ------------------------------------------------------------------ */

int64_t bc_add_i64(int64_t a, int64_t b);
int64_t bc_sub_i64(int64_t a, int64_t b);
int64_t bc_mul_i64(int64_t a, int64_t b);
int64_t bc_div_i64(int64_t a, int64_t b);
int64_t bc_mod_i64(int64_t a, int64_t b);
int64_t bc_neg_i64(int64_t a);

/* ------------------------------------------------------------------ */
/*  Type-cast functions                                                */
/* ------------------------------------------------------------------ */

int64_t bc_i32_to_i64(int32_t n);
int32_t bc_i64_to_i32(int64_t n);
double  bc_i32_to_f64(int32_t n);
double  bc_i64_to_f64(int64_t n);
int32_t bc_f64_to_i32(double n);
int64_t bc_f64_to_i64(double n);

/* ------------------------------------------------------------------ */
/*  Micro-lib I/O                                                      */
/* ------------------------------------------------------------------ */

void bc_print(bc_str s);
void bc_println(bc_str s);
void bc_print_i32(int32_t n);
void bc_print_i64(int64_t n);
void bc_print_f64(double n);
void bc_print_bool(uint8_t b);
bc_result_str_str bc_read_line(bc_arena *arena);

/* ------------------------------------------------------------------ */
/*  Conversion functions                                               */
/* ------------------------------------------------------------------ */

bc_str bc_i32_to_str(bc_arena *arena, int32_t n);

/* ------------------------------------------------------------------ */
/*  Math helpers (micro-lib)                                           */
/* ------------------------------------------------------------------ */

int32_t bc_abs_i32(int32_t n);
double  bc_abs_f64(double n);

/* ------------------------------------------------------------------ */
/*  Arena reset (micro-lib convenience)                                */
/* ------------------------------------------------------------------ */

void bc_arena_reset_global(void);

/* Global arena — created by generated main, used by micro-lib fns */
extern bc_arena *bc_global_arena;

#ifdef __cplusplus
}
#endif

#endif /* BC_RUNTIME_H */
