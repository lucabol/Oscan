/*
 * osc_runtime.h — Oscan Runtime Public API
 *
 * All Oscan programs link against this runtime. It provides the arena
 * allocator, checked arithmetic, dynamic arrays, immutable strings,
 * micro-lib I/O, type-cast helpers, and a panic handler.
 */

#ifndef OSC_RUNTIME_H
#define OSC_RUNTIME_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ------------------------------------------------------------------ */
/*  Panic handler                                                      */
/* ------------------------------------------------------------------ */

void osc_panic(const char *message, const char *file, int line);

#define OSC_PANIC(msg) osc_panic((msg), __FILE__, __LINE__)

/* ------------------------------------------------------------------ */
/*  Arena allocator                                                    */
/* ------------------------------------------------------------------ */

typedef struct osc_arena_block {
    uint8_t *data;
    size_t   used;
    size_t   capacity;
    struct osc_arena_block *next;
} osc_arena_block;

typedef struct {
    osc_arena_block *head;     /* first block (for destroy/reset)  */
    osc_arena_block *current;  /* current allocation block          */
    size_t          block_size; /* default size for new blocks     */
} osc_arena;

#define OSC_ARENA_DEFAULT_CAPACITY ((size_t)(1024 * 1024)) /* 1 MB */

osc_arena *osc_arena_create(size_t initial_capacity);
void     *osc_arena_alloc(osc_arena *arena, size_t size);
void      osc_arena_reset(osc_arena *arena);
void      osc_arena_destroy(osc_arena *arena);

/* ------------------------------------------------------------------ */
/*  String type (immutable UTF-8 view)                                 */
/* ------------------------------------------------------------------ */

typedef struct {
    const char *data;
    int32_t     len;
} osc_str;

osc_str   osc_str_from_cstr(const char *s);
osc_str   osc_str_concat(osc_arena *arena, osc_str a, osc_str b);
int32_t  osc_str_len(osc_str s);
uint8_t  osc_str_eq(osc_str a, osc_str b);
osc_str   osc_str_to_cstr(osc_arena *arena, osc_str s);

/* ------------------------------------------------------------------ */
/*  Dynamic array (generic via void* + elem_size)                      */
/* ------------------------------------------------------------------ */

typedef struct {
    void    *data;
    int32_t  len;
    int32_t  capacity;
    int32_t  elem_size;
} osc_array;

osc_array *osc_array_new(osc_arena *arena, int32_t elem_size,
                       int32_t initial_capacity);
void     *osc_array_get(osc_array *arr, int32_t index);
void      osc_array_set(osc_array *arr, int32_t index, void *value);
void      osc_array_push(osc_arena *arena, osc_array *arr, void *value);
int32_t   osc_array_len(osc_array *arr);

/* ------------------------------------------------------------------ */
/*  Result type (generic tagged union via macros)                      */
/* ------------------------------------------------------------------ */

/*
 * Usage:
 *   OSC_RESULT_DECL(int32_t, osc_str)
 * Expands to a struct osc_result_int32_t_osc_str with tag + union.
 *
 * For typical Oscan use the compiler generates concrete typedefs.
 * Here we provide a helper macro and a commonly-used concrete type
 * for read_line: Result<str, str>.
 */

#define OSC_RESULT_DECL(ok_type, err_type, name)  \
    typedef struct {                             \
        uint8_t is_ok;                           \
        union {                                  \
            ok_type  ok;                         \
            err_type err;                        \
        } value;                                 \
    } name

/* Result<str, str> used by osc_read_line */
OSC_RESULT_DECL(osc_str, osc_str, osc_result_str_str);

/* ------------------------------------------------------------------ */
/*  Checked arithmetic — i32                                           */
/* ------------------------------------------------------------------ */

int32_t osc_add_i32(int32_t a, int32_t b);
int32_t osc_sub_i32(int32_t a, int32_t b);
int32_t osc_mul_i32(int32_t a, int32_t b);
int32_t osc_div_i32(int32_t a, int32_t b);
int32_t osc_mod_i32(int32_t a, int32_t b);
int32_t osc_neg_i32(int32_t a);

/* ------------------------------------------------------------------ */
/*  Checked arithmetic — i64                                           */
/* ------------------------------------------------------------------ */

int64_t osc_add_i64(int64_t a, int64_t b);
int64_t osc_sub_i64(int64_t a, int64_t b);
int64_t osc_mul_i64(int64_t a, int64_t b);
int64_t osc_div_i64(int64_t a, int64_t b);
int64_t osc_mod_i64(int64_t a, int64_t b);
int64_t osc_neg_i64(int64_t a);

/* ------------------------------------------------------------------ */
/*  Type-cast functions                                                */
/* ------------------------------------------------------------------ */

int64_t osc_i32_to_i64(int32_t n);
int32_t osc_i64_to_i32(int64_t n);
double  osc_i32_to_f64(int32_t n);
double  osc_i64_to_f64(int64_t n);
int32_t osc_f64_to_i32(double n);
int64_t osc_f64_to_i64(double n);

/* ------------------------------------------------------------------ */
/*  Micro-lib I/O                                                      */
/* ------------------------------------------------------------------ */

void osc_print(osc_str s);
void osc_println(osc_str s);
void osc_print_i32(int32_t n);
void osc_print_i64(int64_t n);
void osc_print_f64(double n);
void osc_print_bool(uint8_t b);
osc_result_str_str osc_read_line(osc_arena *arena);

/* ------------------------------------------------------------------ */
/*  Conversion functions                                               */
/* ------------------------------------------------------------------ */

osc_str osc_i32_to_str(osc_arena *arena, int32_t n);

/* ------------------------------------------------------------------ */
/*  Math helpers (micro-lib)                                           */
/* ------------------------------------------------------------------ */

int32_t osc_abs_i32(int32_t n);
double  osc_abs_f64(double n);

/* ------------------------------------------------------------------ */
/*  Arena reset (micro-lib convenience)                                */
/* ------------------------------------------------------------------ */

void osc_arena_reset_global(void);

/* Global arena — created by generated main, used by micro-lib fns */
extern osc_arena *osc_global_arena;

#ifdef __cplusplus
}
#endif

#endif /* OSC_RUNTIME_H */
