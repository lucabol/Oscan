/*
 * bc_runtime.c — Babel-C Runtime Implementation
 */

#include "bc_runtime.h"

#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

/* Global arena pointer — set by generated main() */
bc_arena *bc_global_arena = NULL;

/* ================================================================== */
/*  Panic handler                                                      */
/* ================================================================== */

void bc_panic(const char *message, const char *file, int line)
{
    fprintf(stderr, "panic at %s:%d: %s\n", file, line, message);
    exit(1);
}

/* ================================================================== */
/*  Arena allocator                                                    */
/* ================================================================== */

bc_arena *bc_arena_create(size_t initial_capacity)
{
    bc_arena *arena;

    if (initial_capacity == 0) {
        initial_capacity = BC_ARENA_DEFAULT_CAPACITY;
    }
    arena = (bc_arena *)malloc(sizeof(bc_arena));
    if (!arena) {
        bc_panic("failed to allocate arena struct", __FILE__, __LINE__);
    }
    arena->data = (uint8_t *)malloc(initial_capacity);
    if (!arena->data) {
        free(arena);
        bc_panic("failed to allocate arena data", __FILE__, __LINE__);
    }
    arena->used     = 0;
    arena->capacity = initial_capacity;
    return arena;
}

void *bc_arena_alloc(bc_arena *arena, size_t size)
{
    size_t aligned;
    void  *ptr;

    if (!arena) {
        bc_panic("arena is NULL", __FILE__, __LINE__);
    }

    /* 8-byte alignment */
    aligned = (size + 7u) & ~(size_t)7u;

    /* Grow if necessary */
    if (arena->used + aligned > arena->capacity) {
        size_t    new_cap  = arena->capacity * 2;
        uint8_t  *new_data;

        while (new_cap < arena->used + aligned) {
            new_cap *= 2;
        }
        new_data = (uint8_t *)malloc(new_cap);
        if (!new_data) {
            bc_panic("arena growth allocation failed", __FILE__, __LINE__);
        }
        memcpy(new_data, arena->data, arena->used);
        free(arena->data);
        arena->data     = new_data;
        arena->capacity = new_cap;
    }

    ptr = arena->data + arena->used;
    arena->used += aligned;
    return ptr;
}

void bc_arena_reset(bc_arena *arena)
{
    if (arena) {
        arena->used = 0;
    }
}

void bc_arena_destroy(bc_arena *arena)
{
    if (arena) {
        free(arena->data);
        arena->data     = NULL;
        arena->used     = 0;
        arena->capacity = 0;
        free(arena);
    }
}

void bc_arena_reset_global(void)
{
    bc_arena_reset(bc_global_arena);
}

/* ================================================================== */
/*  Checked arithmetic — i32                                           */
/* ================================================================== */

int32_t bc_add_i32(int32_t a, int32_t b)
{
    if (b > 0 && a > INT32_MAX - b) {
        BC_PANIC("i32 addition overflow");
    }
    if (b < 0 && a < INT32_MIN - b) {
        BC_PANIC("i32 addition underflow");
    }
    return a + b;
}

int32_t bc_sub_i32(int32_t a, int32_t b)
{
    if (b < 0 && a > INT32_MAX + b) {
        BC_PANIC("i32 subtraction overflow");
    }
    if (b > 0 && a < INT32_MIN + b) {
        BC_PANIC("i32 subtraction underflow");
    }
    return a - b;
}

int32_t bc_mul_i32(int32_t a, int32_t b)
{
    int64_t wide = (int64_t)a * (int64_t)b;
    if (wide > INT32_MAX || wide < INT32_MIN) {
        BC_PANIC("i32 multiplication overflow");
    }
    return (int32_t)wide;
}

int32_t bc_div_i32(int32_t a, int32_t b)
{
    if (b == 0) {
        BC_PANIC("i32 division by zero");
    }
    if (a == INT32_MIN && b == -1) {
        BC_PANIC("i32 division overflow (MIN / -1)");
    }
    return a / b;
}

int32_t bc_mod_i32(int32_t a, int32_t b)
{
    if (b == 0) {
        BC_PANIC("i32 modulo by zero");
    }
    if (a == INT32_MIN && b == -1) {
        BC_PANIC("i32 modulo overflow (MIN % -1)");
    }
    return a % b;
}

int32_t bc_neg_i32(int32_t a)
{
    if (a == INT32_MIN) {
        BC_PANIC("i32 negation overflow (MIN_VALUE)");
    }
    return -a;
}

/* ================================================================== */
/*  Checked arithmetic — i64                                           */
/* ================================================================== */

int64_t bc_add_i64(int64_t a, int64_t b)
{
    if (b > 0 && a > INT64_MAX - b) {
        BC_PANIC("i64 addition overflow");
    }
    if (b < 0 && a < INT64_MIN - b) {
        BC_PANIC("i64 addition underflow");
    }
    return a + b;
}

int64_t bc_sub_i64(int64_t a, int64_t b)
{
    if (b < 0 && a > INT64_MAX + b) {
        BC_PANIC("i64 subtraction overflow");
    }
    if (b > 0 && a < INT64_MIN + b) {
        BC_PANIC("i64 subtraction underflow");
    }
    return a - b;
}

int64_t bc_mul_i64(int64_t a, int64_t b)
{
    /* For i64, we cannot widen to 128 bits portably in C99.
       Use careful case analysis instead. */
    if (a > 0) {
        if (b > 0) {
            if (a > INT64_MAX / b) {
                BC_PANIC("i64 multiplication overflow");
            }
        } else if (b < 0) {
            if (b < INT64_MIN / a) {
                BC_PANIC("i64 multiplication overflow");
            }
        }
    } else if (a < 0) {
        if (b > 0) {
            if (a < INT64_MIN / b) {
                BC_PANIC("i64 multiplication overflow");
            }
        } else if (b < 0) {
            if (a != 0 && b < INT64_MAX / a) {
                BC_PANIC("i64 multiplication overflow");
            }
        }
    }
    return a * b;
}

int64_t bc_div_i64(int64_t a, int64_t b)
{
    if (b == 0) {
        BC_PANIC("i64 division by zero");
    }
    if (a == INT64_MIN && b == -1) {
        BC_PANIC("i64 division overflow (MIN / -1)");
    }
    return a / b;
}

int64_t bc_mod_i64(int64_t a, int64_t b)
{
    if (b == 0) {
        BC_PANIC("i64 modulo by zero");
    }
    if (a == INT64_MIN && b == -1) {
        BC_PANIC("i64 modulo overflow (MIN % -1)");
    }
    return a % b;
}

int64_t bc_neg_i64(int64_t a)
{
    if (a == INT64_MIN) {
        BC_PANIC("i64 negation overflow (MIN_VALUE)");
    }
    return -a;
}

/* ================================================================== */
/*  Dynamic array                                                      */
/* ================================================================== */

bc_array *bc_array_new(bc_arena *arena, int32_t elem_size,
                       int32_t initial_capacity)
{
    bc_array *arr;

    if (elem_size <= 0) {
        BC_PANIC("array elem_size must be > 0");
    }
    if (initial_capacity < 0) {
        BC_PANIC("array initial_capacity must be >= 0");
    }

    arr = (bc_array *)bc_arena_alloc(arena, sizeof(bc_array));
    arr->elem_size = elem_size;
    arr->len       = 0;
    arr->capacity  = initial_capacity > 0 ? initial_capacity : 4;
    arr->data      = bc_arena_alloc(arena, (size_t)arr->capacity *
                                           (size_t)arr->elem_size);
    return arr;
}

void *bc_array_get(bc_array *arr, int32_t index)
{
    if (!arr) {
        BC_PANIC("array is NULL");
    }
    if (index < 0 || index >= arr->len) {
        BC_PANIC("array index out of bounds");
    }
    return (uint8_t *)arr->data + (size_t)index * (size_t)arr->elem_size;
}

void bc_array_set(bc_array *arr, int32_t index, void *value)
{
    if (!arr) {
        BC_PANIC("array is NULL");
    }
    if (index < 0 || index >= arr->len) {
        BC_PANIC("array index out of bounds");
    }
    memcpy((uint8_t *)arr->data + (size_t)index * (size_t)arr->elem_size,
           value, (size_t)arr->elem_size);
}

void bc_array_push(bc_arena *arena, bc_array *arr, void *value)
{
    if (!arr) {
        BC_PANIC("array is NULL");
    }
    if (arr->len >= arr->capacity) {
        int32_t  new_cap  = arr->capacity * 2;
        void    *new_data;

        if (new_cap < arr->capacity) {
            BC_PANIC("array capacity overflow");
        }
        new_data = bc_arena_alloc(arena, (size_t)new_cap *
                                         (size_t)arr->elem_size);
        memcpy(new_data, arr->data, (size_t)arr->len *
                                    (size_t)arr->elem_size);
        arr->data     = new_data;
        arr->capacity = new_cap;
    }
    memcpy((uint8_t *)arr->data + (size_t)arr->len * (size_t)arr->elem_size,
           value, (size_t)arr->elem_size);
    arr->len++;
}

int32_t bc_array_len(bc_array *arr)
{
    if (!arr) {
        BC_PANIC("array is NULL");
    }
    return arr->len;
}

/* ================================================================== */
/*  String operations                                                  */
/* ================================================================== */

bc_str bc_str_from_cstr(const char *s)
{
    bc_str result;
    if (!s) {
        result.data = "";
        result.len  = 0;
    } else {
        size_t slen = strlen(s);
        if (slen > (size_t)INT32_MAX) {
            BC_PANIC("string length exceeds i32 range");
        }
        result.data = s;
        result.len  = (int32_t)slen;
    }
    return result;
}

bc_str bc_str_concat(bc_arena *arena, bc_str a, bc_str b)
{
    bc_str  result;
    int32_t total_len;
    char   *buf;

    /* Check for overflow in length addition */
    if (a.len > INT32_MAX - b.len) {
        BC_PANIC("string concat length overflow");
    }
    total_len = a.len + b.len;

    buf = (char *)bc_arena_alloc(arena, (size_t)total_len + 1);
    if (a.len > 0) {
        memcpy(buf, a.data, (size_t)a.len);
    }
    if (b.len > 0) {
        memcpy(buf + a.len, b.data, (size_t)b.len);
    }
    buf[total_len] = '\0';

    result.data = buf;
    result.len  = total_len;
    return result;
}

int32_t bc_str_len(bc_str s)
{
    return s.len;
}

uint8_t bc_str_eq(bc_str a, bc_str b)
{
    if (a.len != b.len) {
        return 0;
    }
    if (a.len == 0) {
        return 1;
    }
    return (uint8_t)(memcmp(a.data, b.data, (size_t)a.len) == 0);
}

bc_str bc_str_to_cstr(bc_arena *arena, bc_str s)
{
    bc_str result;
    char  *buf;

    buf = (char *)bc_arena_alloc(arena, (size_t)s.len + 1);
    if (s.len > 0) {
        memcpy(buf, s.data, (size_t)s.len);
    }
    buf[s.len] = '\0';

    result.data = buf;
    result.len  = s.len;
    return result;
}

/* ================================================================== */
/*  Micro-lib I/O                                                      */
/* ================================================================== */

void bc_print(bc_str s)
{
    if (s.len > 0) {
        fwrite(s.data, 1, (size_t)s.len, stdout);
    }
    fflush(stdout);
}

void bc_println(bc_str s)
{
    if (s.len > 0) {
        fwrite(s.data, 1, (size_t)s.len, stdout);
    }
    putchar('\n');
    fflush(stdout);
}

void bc_print_i32(int32_t n)
{
    printf("%" PRId32, n);
    fflush(stdout);
}

void bc_print_i64(int64_t n)
{
    printf("%" PRId64, n);
    fflush(stdout);
}

void bc_print_f64(double n)
{
    /* Shortest-representation: find the minimum precision that round-trips */
    char buf[32];
    int prec;
    for (prec = 1; prec <= 17; prec++) {
        double reparsed;
        snprintf(buf, sizeof(buf), "%.*g", prec, n);
        sscanf(buf, "%lf", &reparsed);
        if (reparsed == n) {
            printf("%s", buf);
            fflush(stdout);
            return;
        }
    }
    printf("%.17g", n);
    fflush(stdout);
}

void bc_print_bool(uint8_t b)
{
    printf("%s", b ? "true" : "false");
    fflush(stdout);
}

bc_result_str_str bc_read_line(bc_arena *arena)
{
    bc_result_str_str result;
    char              buf[4096];
    size_t            slen;
    char             *copy;

    if (!fgets(buf, (int)sizeof(buf), stdin)) {
        result.is_ok      = 0;
        result.value.err  = bc_str_from_cstr("failed to read line from stdin");
        return result;
    }

    /* Strip trailing newline */
    slen = strlen(buf);
    if (slen > 0 && buf[slen - 1] == '\n') {
        buf[--slen] = '\0';
    }
    if (slen > 0 && buf[slen - 1] == '\r') {
        buf[--slen] = '\0';
    }

    copy = (char *)bc_arena_alloc(arena, slen + 1);
    memcpy(copy, buf, slen + 1);

    result.is_ok          = 1;
    result.value.ok.data  = copy;
    result.value.ok.len   = (int32_t)slen;
    return result;
}

/* ================================================================== */
/*  Type-cast functions                                                */
/* ================================================================== */

int64_t bc_i32_to_i64(int32_t n)
{
    return (int64_t)n;
}

int32_t bc_i64_to_i32(int64_t n)
{
    if (n > INT32_MAX || n < INT32_MIN) {
        BC_PANIC("i64 to i32 narrowing overflow");
    }
    return (int32_t)n;
}

double bc_i32_to_f64(int32_t n)
{
    return (double)n;
}

double bc_i64_to_f64(int64_t n)
{
    return (double)n;
}

int32_t bc_f64_to_i32(double n)
{
    if (n != n) { /* NaN check */
        BC_PANIC("f64 to i32: NaN");
    }
    if (n > (double)INT32_MAX || n < (double)INT32_MIN) {
        BC_PANIC("f64 to i32: out of range");
    }
    return (int32_t)n;
}

int64_t bc_f64_to_i64(double n)
{
    if (n != n) { /* NaN check */
        BC_PANIC("f64 to i64: NaN");
    }
    /* INT64_MAX can't be represented exactly in double, so we check
       against the boundary values that are representable. */
    if (n >= 9.2233720368547758e+18 || n < -9.2233720368547758e+18) {
        BC_PANIC("f64 to i64: out of range");
    }
    return (int64_t)n;
}

/* ================================================================== */
/*  Conversion functions                                               */
/* ================================================================== */

bc_str bc_i32_to_str(bc_arena *arena, int32_t n)
{
    bc_str result;
    char   buf[16]; /* "-2147483648" is 11 chars + NUL */
    int    written;
    char  *copy;

    written = snprintf(buf, sizeof(buf), "%" PRId32, n);
    if (written < 0 || (size_t)written >= sizeof(buf)) {
        BC_PANIC("i32_to_str: snprintf failed");
    }
    copy = (char *)bc_arena_alloc(arena, (size_t)written + 1);
    memcpy(copy, buf, (size_t)written + 1);

    result.data = copy;
    result.len  = (int32_t)written;
    return result;
}

/* ================================================================== */
/*  Math helpers                                                       */
/* ================================================================== */

int32_t bc_abs_i32(int32_t n)
{
    if (n == INT32_MIN) {
        BC_PANIC("abs_i32: MIN_VALUE has no positive counterpart");
    }
    return n < 0 ? -n : n;
}

double bc_abs_f64(double n)
{
    return fabs(n);
}
