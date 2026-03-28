/*
 * osc_runtime.c — Oscan Runtime Implementation
 *
 * Supports two modes:
 *   - Freestanding (OSC_FREESTANDING defined): uses l_os.h, no libc dependency.
 *     In this mode, l_os.h is already included by the main TU before this file.
 *     Its macro redirects (strlen→l_strlen, memcpy→l_memcpy, exit→l_exit, etc.)
 *     are active, so most code works unchanged.
 *   - libc mode (default): uses standard C library headers.
 */

#include "osc_runtime.h"

#ifndef OSC_FREESTANDING
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#endif

/* Global arena pointer — set by generated main() */
osc_arena *osc_global_arena = NULL;

/* ================================================================== */
/*  Panic handler                                                      */
/* ================================================================== */

void osc_panic(const char *message, const char *file, int line)
{
#ifdef OSC_FREESTANDING
    char buf[256];
    int n = snprintf(buf, sizeof(buf), "panic at %s:%d: %s\n", file, line, message);
    if (n > 0) {
        size_t wlen = (size_t)n;
        if (wlen > sizeof(buf) - 1) wlen = sizeof(buf) - 1;
        write(L_STDERR, buf, wlen);
    }
    exit(1);
#else
    fprintf(stderr, "panic at %s:%d: %s\n", file, line, message);
    exit(1);
#endif
}

/* ================================================================== */
/*  Arena allocator (linked-list of blocks — pointers never move)      */
/* ================================================================== */

#ifdef OSC_FREESTANDING
#define OSC_PAGE_SIZE ((size_t)4096)
#define OSC_PAGE_ROUND(n) (((n) + OSC_PAGE_SIZE - 1) & ~(OSC_PAGE_SIZE - 1))
#endif

static osc_arena_block *osc_arena_block_new(size_t capacity)
{
#ifdef OSC_FREESTANDING
    /* Single mmap: [osc_arena_block struct | data ...] */
    size_t total = OSC_PAGE_ROUND(sizeof(osc_arena_block) + capacity);
    void *mem = l_mmap(0, total,
                       L_PROT_READ | L_PROT_WRITE,
                       L_MAP_PRIVATE | L_MAP_ANONYMOUS, -1, 0);
    if (mem == L_MAP_FAILED) {
        osc_panic("failed to mmap arena block", __FILE__, __LINE__);
    }
    osc_arena_block *block = (osc_arena_block *)mem;
    block->data       = (uint8_t *)mem + sizeof(osc_arena_block);
    block->used       = 0;
    block->capacity   = total - sizeof(osc_arena_block);
    block->alloc_size = total;
    block->next       = NULL;
    return block;
#else
    osc_arena_block *block = (osc_arena_block *)malloc(sizeof(osc_arena_block));
    if (!block) {
        osc_panic("failed to allocate arena block struct", __FILE__, __LINE__);
    }
    block->data = (uint8_t *)malloc(capacity);
    if (!block->data) {
        free(block);
        osc_panic("failed to allocate arena block data", __FILE__, __LINE__);
    }
    block->used       = 0;
    block->capacity   = capacity;
    block->alloc_size = 0;
    block->next       = NULL;
    return block;
#endif
}

osc_arena *osc_arena_create(size_t initial_capacity)
{
    osc_arena *arena;

    if (initial_capacity == 0) {
        initial_capacity = OSC_ARENA_DEFAULT_CAPACITY;
    }
#ifdef OSC_FREESTANDING
    size_t arena_alloc = OSC_PAGE_ROUND(sizeof(osc_arena));
    void *mem = l_mmap(0, arena_alloc,
                       L_PROT_READ | L_PROT_WRITE,
                       L_MAP_PRIVATE | L_MAP_ANONYMOUS, -1, 0);
    if (mem == L_MAP_FAILED) {
        osc_panic("failed to mmap arena struct", __FILE__, __LINE__);
    }
    arena = (osc_arena *)mem;
#else
    arena = (osc_arena *)malloc(sizeof(osc_arena));
    if (!arena) {
        osc_panic("failed to allocate arena struct", __FILE__, __LINE__);
    }
#endif
    arena->head       = osc_arena_block_new(initial_capacity);
    arena->current    = arena->head;
    arena->block_size = initial_capacity;
    return arena;
}

void *osc_arena_alloc(osc_arena *arena, size_t size)
{
    size_t aligned;
    void  *ptr;

    if (!arena) {
        osc_panic("arena is NULL", __FILE__, __LINE__);
    }

    /* 8-byte alignment */
    aligned = (size + 7u) & ~(size_t)7u;

    /* Fast path: current block has room */
    if (arena->current->used + aligned <= arena->current->capacity) {
        ptr = arena->current->data + arena->current->used;
        arena->current->used += aligned;
        return ptr;
    }

    /* Slow path: allocate a new block, link it in.
       Size is max(block_size, aligned) so oversized requests still fit. */
    {
        size_t          new_cap   = arena->block_size;
        osc_arena_block *new_block;

        if (new_cap < aligned) {
            new_cap = aligned;
        }
        new_block = osc_arena_block_new(new_cap);
        arena->current->next = new_block;
        arena->current       = new_block;

        ptr = new_block->data;
        new_block->used = aligned;
        return ptr;
    }
}

void osc_arena_reset(osc_arena *arena)
{
    if (arena) {
        osc_arena_block *block = arena->head;
        while (block) {
            block->used = 0;
            block = block->next;
        }
        arena->current = arena->head;
    }
}

void osc_arena_destroy(osc_arena *arena)
{
    if (arena) {
        osc_arena_block *block = arena->head;
        while (block) {
            osc_arena_block *next = block->next;
#ifdef OSC_FREESTANDING
            l_munmap(block, block->alloc_size);
#else
            free(block->data);
            free(block);
#endif
            block = next;
        }
        arena->head    = NULL;
        arena->current = NULL;
#ifdef OSC_FREESTANDING
        l_munmap(arena, OSC_PAGE_ROUND(sizeof(osc_arena)));
#else
        free(arena);
#endif
    }
}

void osc_arena_reset_global(void)
{
    osc_arena_reset(osc_global_arena);
}

/* ================================================================== */
/*  Checked arithmetic — i32                                           */
/* ================================================================== */

int32_t osc_add_i32(int32_t a, int32_t b)
{
    if (b > 0 && a > INT32_MAX - b) {
        OSC_PANIC("i32 addition overflow");
    }
    if (b < 0 && a < INT32_MIN - b) {
        OSC_PANIC("i32 addition underflow");
    }
    return a + b;
}

int32_t osc_sub_i32(int32_t a, int32_t b)
{
    if (b < 0 && a > INT32_MAX + b) {
        OSC_PANIC("i32 subtraction overflow");
    }
    if (b > 0 && a < INT32_MIN + b) {
        OSC_PANIC("i32 subtraction underflow");
    }
    return a - b;
}

int32_t osc_mul_i32(int32_t a, int32_t b)
{
    int64_t wide = (int64_t)a * (int64_t)b;
    if (wide > INT32_MAX || wide < INT32_MIN) {
        OSC_PANIC("i32 multiplication overflow");
    }
    return (int32_t)wide;
}

int32_t osc_div_i32(int32_t a, int32_t b)
{
    if (b == 0) {
        OSC_PANIC("i32 division by zero");
    }
    if (a == INT32_MIN && b == -1) {
        OSC_PANIC("i32 division overflow (MIN / -1)");
    }
    return a / b;
}

int32_t osc_mod_i32(int32_t a, int32_t b)
{
    if (b == 0) {
        OSC_PANIC("i32 modulo by zero");
    }
    if (a == INT32_MIN && b == -1) {
        OSC_PANIC("i32 modulo overflow (MIN % -1)");
    }
    return a % b;
}

int32_t osc_neg_i32(int32_t a)
{
    if (a == INT32_MIN) {
        OSC_PANIC("i32 negation overflow (MIN_VALUE)");
    }
    return -a;
}

/* ================================================================== */
/*  Checked arithmetic — i64                                           */
/* ================================================================== */

int64_t osc_add_i64(int64_t a, int64_t b)
{
    if (b > 0 && a > INT64_MAX - b) {
        OSC_PANIC("i64 addition overflow");
    }
    if (b < 0 && a < INT64_MIN - b) {
        OSC_PANIC("i64 addition underflow");
    }
    return a + b;
}

int64_t osc_sub_i64(int64_t a, int64_t b)
{
    if (b < 0 && a > INT64_MAX + b) {
        OSC_PANIC("i64 subtraction overflow");
    }
    if (b > 0 && a < INT64_MIN + b) {
        OSC_PANIC("i64 subtraction underflow");
    }
    return a - b;
}

int64_t osc_mul_i64(int64_t a, int64_t b)
{
    /* For i64, we cannot widen to 128 bits portably in C99.
       Use careful case analysis instead. */
    if (a > 0) {
        if (b > 0) {
            if (a > INT64_MAX / b) {
                OSC_PANIC("i64 multiplication overflow");
            }
        } else if (b < 0) {
            if (b < INT64_MIN / a) {
                OSC_PANIC("i64 multiplication overflow");
            }
        }
    } else if (a < 0) {
        if (b > 0) {
            if (a < INT64_MIN / b) {
                OSC_PANIC("i64 multiplication overflow");
            }
        } else if (b < 0) {
            if (a != 0 && b < INT64_MAX / a) {
                OSC_PANIC("i64 multiplication overflow");
            }
        }
    }
    return a * b;
}

int64_t osc_div_i64(int64_t a, int64_t b)
{
    if (b == 0) {
        OSC_PANIC("i64 division by zero");
    }
    if (a == INT64_MIN && b == -1) {
        OSC_PANIC("i64 division overflow (MIN / -1)");
    }
    return a / b;
}

int64_t osc_mod_i64(int64_t a, int64_t b)
{
    if (b == 0) {
        OSC_PANIC("i64 modulo by zero");
    }
    if (a == INT64_MIN && b == -1) {
        OSC_PANIC("i64 modulo overflow (MIN % -1)");
    }
    return a % b;
}

int64_t osc_neg_i64(int64_t a)
{
    if (a == INT64_MIN) {
        OSC_PANIC("i64 negation overflow (MIN_VALUE)");
    }
    return -a;
}

/* ================================================================== */
/*  Dynamic array                                                      */
/* ================================================================== */

osc_array *osc_array_new(osc_arena *arena, int32_t elem_size,
                       int32_t initial_capacity)
{
    osc_array *arr;

    if (elem_size <= 0) {
        OSC_PANIC("array elem_size must be > 0");
    }
    if (initial_capacity < 0) {
        OSC_PANIC("array initial_capacity must be >= 0");
    }

    arr = (osc_array *)osc_arena_alloc(arena, sizeof(osc_array));
    arr->elem_size = elem_size;
    arr->len       = 0;
    arr->capacity  = initial_capacity > 0 ? initial_capacity : 4;
    arr->data      = osc_arena_alloc(arena, (size_t)arr->capacity *
                                           (size_t)arr->elem_size);
    return arr;
}

void *osc_array_get(osc_array *arr, int32_t index)
{
    if (!arr) {
        OSC_PANIC("array is NULL");
    }
    if (index < 0 || index >= arr->len) {
        OSC_PANIC("array index out of bounds");
    }
    return (uint8_t *)arr->data + (size_t)index * (size_t)arr->elem_size;
}

void osc_array_set(osc_array *arr, int32_t index, void *value)
{
    if (!arr) {
        OSC_PANIC("array is NULL");
    }
    if (index < 0 || index >= arr->len) {
        OSC_PANIC("array index out of bounds");
    }
    memcpy((uint8_t *)arr->data + (size_t)index * (size_t)arr->elem_size,
           value, (size_t)arr->elem_size);
}

void osc_array_push(osc_arena *arena, osc_array *arr, void *value)
{
    if (!arr) {
        OSC_PANIC("array is NULL");
    }
    if (arr->len >= arr->capacity) {
        int32_t  new_cap  = arr->capacity * 2;
        void    *new_data;

        if (new_cap < arr->capacity) {
            OSC_PANIC("array capacity overflow");
        }
        new_data = osc_arena_alloc(arena, (size_t)new_cap *
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

int32_t osc_array_len(osc_array *arr)
{
    if (!arr) {
        OSC_PANIC("array is NULL");
    }
    return arr->len;
}

/* ================================================================== */
/*  String operations                                                  */
/* ================================================================== */

osc_str osc_str_from_cstr(const char *s)
{
    osc_str result;
    if (!s) {
        result.data = "";
        result.len  = 0;
    } else {
        size_t slen = strlen(s);
        if (slen > (size_t)INT32_MAX) {
            OSC_PANIC("string length exceeds i32 range");
        }
        result.data = s;
        result.len  = (int32_t)slen;
    }
    return result;
}

osc_str osc_str_concat(osc_arena *arena, osc_str a, osc_str b)
{
    osc_str  result;
    int32_t total_len;
    char   *buf;

    /* Check for overflow in length addition */
    if (a.len > INT32_MAX - b.len) {
        OSC_PANIC("string concat length overflow");
    }
    total_len = a.len + b.len;

    buf = (char *)osc_arena_alloc(arena, (size_t)total_len + 1);
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

int32_t osc_str_len(osc_str s)
{
    return s.len;
}

uint8_t osc_str_eq(osc_str a, osc_str b)
{
    if (a.len != b.len) {
        return 0;
    }
    if (a.len == 0) {
        return 1;
    }
    return (uint8_t)(memcmp(a.data, b.data, (size_t)a.len) == 0);
}

osc_str osc_str_to_cstr(osc_arena *arena, osc_str s)
{
    osc_str result;
    char  *buf;

    buf = (char *)osc_arena_alloc(arena, (size_t)s.len + 1);
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

#ifdef OSC_FREESTANDING

/* Helper: write a buffer to stdout */
static void osc_write_stdout(const char *buf, size_t len)
{
    if (len > 0) write(L_STDOUT, buf, len);
}

void osc_print(osc_str s)
{
    if (s.len > 0) write(L_STDOUT, s.data, (size_t)s.len);
}

void osc_println(osc_str s)
{
    if (s.len > 0) write(L_STDOUT, s.data, (size_t)s.len);
    write(L_STDOUT, "\n", 1);
}

void osc_print_i32(int32_t n)
{
    char buf[16];
    int len = snprintf(buf, sizeof(buf), "%d", (int)n);
    if (len > 0) osc_write_stdout(buf, (size_t)len);
}

void osc_print_i64(int64_t n)
{
    char buf[24];
    int len = snprintf(buf, sizeof(buf), "%lld", (long long)n);
    if (len > 0) osc_write_stdout(buf, (size_t)len);
}

/* Fixed-point float formatting (6 decimal places, trailing zeros trimmed) */
static void osc_format_f64(char *buf, size_t bufsz, double n)
{
    size_t pos = 0;

    /* NaN */
    if (n != n) {
        buf[0] = 'N'; buf[1] = 'a'; buf[2] = 'N'; buf[3] = '\0';
        return;
    }

    /* Negative */
    if (n < 0.0) {
        buf[pos++] = '-';
        n = -n;
    }

    /* Very large — use integer-only representation */
    if (n >= 1e18) {
        /* Format as large integer (approximate) */
        unsigned long long ipart = (unsigned long long)n;
        pos += (size_t)snprintf(buf + pos, bufsz - pos, "%llu", ipart);
        buf[pos] = '\0';
        return;
    }

    /* Integer part */
    unsigned long long ipart = (unsigned long long)n;
    double frac = n - (double)ipart;

    pos += (size_t)snprintf(buf + pos, bufsz - pos, "%llu", ipart);

    /* Fractional part: 6 digits */
    buf[pos++] = '.';
    {
        int i;
        for (i = 0; i < 6 && pos < bufsz - 1; i++) {
            frac *= 10.0;
            int digit = (int)frac;
            if (digit > 9) digit = 9;
            buf[pos++] = (char)('0' + digit);
            frac -= (double)digit;
        }
    }
    buf[pos] = '\0';

    /* Trim trailing zeros (keep at least "X.0") */
    {
        int end = (int)pos - 1;
        while (end > 0 && buf[end] == '0') end--;
        if (buf[end] == '.') end++; /* keep one digit after dot */
        buf[end + 1] = '\0';
    }
}

void osc_print_f64(double n)
{
    char buf[64];
    osc_format_f64(buf, sizeof(buf), n);
    osc_write_stdout(buf, strlen(buf));
}

void osc_print_bool(uint8_t b)
{
    if (b) {
        write(L_STDOUT, "true", 4);
    } else {
        write(L_STDOUT, "false", 5);
    }
}

osc_result_str_str osc_read_line(osc_arena *arena)
{
    osc_result_str_str result;
    char              buf[4096];
    size_t            pos = 0;

    /* Read one byte at a time until newline or error */
    while (pos < sizeof(buf) - 1) {
        ssize_t n = read(L_STDIN, &buf[pos], 1);
        if (n <= 0) {
            if (pos == 0) {
                result.is_ok     = 0;
                result.value.err = osc_str_from_cstr("failed to read line from stdin");
                return result;
            }
            break;
        }
        if (buf[pos] == '\n') break;
        pos++;
    }
    buf[pos] = '\0';

    /* Strip trailing CR */
    if (pos > 0 && buf[pos - 1] == '\r') {
        buf[--pos] = '\0';
    }

    {
        char *copy = (char *)osc_arena_alloc(arena, pos + 1);
        memcpy(copy, buf, pos + 1);

        result.is_ok          = 1;
        result.value.ok.data  = copy;
        result.value.ok.len   = (int32_t)pos;
    }
    return result;
}

#else /* libc mode */

void osc_print(osc_str s)
{
    if (s.len > 0) {
        fwrite(s.data, 1, (size_t)s.len, stdout);
    }
    fflush(stdout);
}

void osc_println(osc_str s)
{
    if (s.len > 0) {
        fwrite(s.data, 1, (size_t)s.len, stdout);
    }
    putchar('\n');
    fflush(stdout);
}

void osc_print_i32(int32_t n)
{
    printf("%" PRId32, n);
    fflush(stdout);
}

void osc_print_i64(int64_t n)
{
    printf("%" PRId64, n);
    fflush(stdout);
}

void osc_print_f64(double n)
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

void osc_print_bool(uint8_t b)
{
    printf("%s", b ? "true" : "false");
    fflush(stdout);
}

osc_result_str_str osc_read_line(osc_arena *arena)
{
    osc_result_str_str result;
    char              buf[4096];
    size_t            slen;
    char             *copy;

    if (!fgets(buf, (int)sizeof(buf), stdin)) {
        result.is_ok      = 0;
        result.value.err  = osc_str_from_cstr("failed to read line from stdin");
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

    copy = (char *)osc_arena_alloc(arena, slen + 1);
    memcpy(copy, buf, slen + 1);

    result.is_ok          = 1;
    result.value.ok.data  = copy;
    result.value.ok.len   = (int32_t)slen;
    return result;
}

#endif /* OSC_FREESTANDING */

/* ================================================================== */
/*  Type-cast functions                                                */
/* ================================================================== */

int64_t osc_i32_to_i64(int32_t n)
{
    return (int64_t)n;
}

int32_t osc_i64_to_i32(int64_t n)
{
    if (n > INT32_MAX || n < INT32_MIN) {
        OSC_PANIC("i64 to i32 narrowing overflow");
    }
    return (int32_t)n;
}

double osc_i32_to_f64(int32_t n)
{
    return (double)n;
}

double osc_i64_to_f64(int64_t n)
{
    return (double)n;
}

int32_t osc_f64_to_i32(double n)
{
    if (n != n) { /* NaN check */
        OSC_PANIC("f64 to i32: NaN");
    }
    if (n > (double)INT32_MAX || n < (double)INT32_MIN) {
        OSC_PANIC("f64 to i32: out of range");
    }
    return (int32_t)n;
}

int64_t osc_f64_to_i64(double n)
{
    if (n != n) { /* NaN check */
        OSC_PANIC("f64 to i64: NaN");
    }
    /* INT64_MAX can't be represented exactly in double, so we check
       against the boundary values that are representable. */
    if (n >= 9.2233720368547758e+18 || n < -9.2233720368547758e+18) {
        OSC_PANIC("f64 to i64: out of range");
    }
    return (int64_t)n;
}

/* ================================================================== */
/*  Conversion functions                                               */
/* ================================================================== */

osc_str osc_i32_to_str(osc_arena *arena, int32_t n)
{
    osc_str result;
    char   buf[16]; /* "-2147483648" is 11 chars + NUL */
    int    written;
    char  *copy;

#ifdef OSC_FREESTANDING
    written = snprintf(buf, sizeof(buf), "%d", (int)n);
#else
    written = snprintf(buf, sizeof(buf), "%" PRId32, n);
#endif
    if (written < 0 || (size_t)written >= sizeof(buf)) {
        OSC_PANIC("i32_to_str: snprintf failed");
    }
    copy = (char *)osc_arena_alloc(arena, (size_t)written + 1);
    memcpy(copy, buf, (size_t)written + 1);

    result.data = copy;
    result.len  = (int32_t)written;
    return result;
}

/* ================================================================== */
/*  Math helpers                                                       */
/* ================================================================== */

int32_t osc_abs_i32(int32_t n)
{
    if (n == INT32_MIN) {
        OSC_PANIC("abs_i32: MIN_VALUE has no positive counterpart");
    }
    return n < 0 ? -n : n;
}

double osc_abs_f64(double n)
{
#ifdef OSC_FREESTANDING
    return n < 0.0 ? -n : n;
#else
    return fabs(n);
#endif
}
