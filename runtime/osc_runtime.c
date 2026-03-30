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

/* _POSIX_C_SOURCE must be defined before ANY includes so that glibc's
   <features.h> (pulled in by <stdint.h> etc.) sees it on first pass. */
#ifndef OSC_FREESTANDING
#if !defined(_WIN32) && !defined(_POSIX_C_SOURCE)
#define _POSIX_C_SOURCE 200809L
#endif
#endif

#include "osc_runtime.h"

#ifndef OSC_FREESTANDING
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <ctype.h>
#include <time.h>
#include <errno.h>
#include <sys/stat.h>
#ifdef _WIN32
#include <windows.h>
#include <direct.h>
#include <io.h>
#include <conio.h>
#else
#include <unistd.h>
#include <dirent.h>
#include <sys/wait.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <fcntl.h>
#endif
#endif

/* Global arena pointer — set by generated main() */
osc_arena *osc_global_arena = NULL;

/* Global argc/argv — set by generated main() */
int osc_global_argc = 0;
char **osc_global_argv = NULL;

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

int osc_str_compare(osc_str a, osc_str b)
{
    int32_t min_len = a.len < b.len ? a.len : b.len;
    if (min_len > 0) {
        int cmp = memcmp(a.data, b.data, (size_t)min_len);
        if (cmp != 0) return cmp;
    }
    if (a.len < b.len) return -1;
    if (a.len > b.len) return 1;
    return 0;
}

int32_t osc_str_find(osc_str haystack, osc_str needle)
{
    if (needle.len == 0) return 0;
    if (needle.len > haystack.len) return -1;
    int32_t limit = haystack.len - needle.len;
    for (int32_t i = 0; i <= limit; i++) {
        if (memcmp(haystack.data + i, needle.data, (size_t)needle.len) == 0) {
            return i;
        }
    }
    return -1;
}

osc_str osc_str_from_i32(osc_arena *arena, int32_t n)
{
    /* -2147483648 is 11 chars + NUL */
    char tmp[12];
    int pos = 0;
    uint32_t u;
    osc_str result;
    char *buf;

    if (n < 0) {
        tmp[pos++] = '-';
        u = (uint32_t)(-(int64_t)n);
    } else {
        u = (uint32_t)n;
    }
    /* write digits in reverse into tmp */
    {
        int start = pos;
        do {
            tmp[pos++] = (char)('0' + (u % 10));
            u /= 10;
        } while (u > 0);
        /* reverse the digit portion */
        for (int i = start, j = pos - 1; i < j; i++, j--) {
            char c = tmp[i]; tmp[i] = tmp[j]; tmp[j] = c;
        }
    }
    buf = (char *)osc_arena_alloc(arena, (size_t)pos);
    memcpy(buf, tmp, (size_t)pos);
    result.data = buf;
    result.len  = (int32_t)pos;
    return result;
}

osc_str osc_str_slice(osc_arena *arena, osc_str s, int32_t start, int32_t end)
{
    osc_str result;
    char *buf;
    int32_t slice_len;

    if (start < 0) start = 0;
    if (end > s.len) end = s.len;
    if (start >= end) {
        result.data = "";
        result.len  = 0;
        return result;
    }
    slice_len = end - start;
    buf = (char *)osc_arena_alloc(arena, (size_t)slice_len);
    memcpy(buf, s.data + start, (size_t)slice_len);
    result.data = buf;
    result.len  = slice_len;
    return result;
}

int32_t osc_str_check_index(osc_str s, int32_t idx)
{
    if (idx < 0 || idx >= s.len) {
        OSC_PANIC("string index out of bounds");
    }
    return idx;
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
/*  File I/O                                                           */
/* ================================================================== */

/* Helper: null-terminate an osc_str into a stack buffer */
static void osc_path_to_cstr(osc_str path, char *buf, size_t bufsz)
{
    size_t len = (size_t)path.len;
    if (len >= bufsz) len = bufsz - 1;
    {
        size_t i;
        for (i = 0; i < len; i++) buf[i] = path.data[i];
    }
    buf[len] = '\0';
}

#ifdef OSC_FREESTANDING

int32_t osc_file_open_read(osc_str path)
{
    char buf[4096];
    osc_path_to_cstr(path, buf, sizeof(buf));
    int32_t fd = (int32_t)open_read(buf);
    return fd < 0 ? -1 : fd;
}

int32_t osc_file_open_write(osc_str path)
{
    char buf[4096];
    osc_path_to_cstr(path, buf, sizeof(buf));
    int32_t fd = (int32_t)open_write(buf);
    return fd < 0 ? -1 : fd;
}

int32_t osc_read_byte(int32_t fd)
{
    unsigned char c;
    ssize_t n = read((L_FD)fd, &c, 1);
    return n == 1 ? (int32_t)c : -1;
}

void osc_write_byte(int32_t fd, int32_t b)
{
    unsigned char c = (unsigned char)b;
    write((L_FD)fd, &c, 1);
}

void osc_write_str(int32_t fd, osc_str s)
{
    if (s.len > 0) write((L_FD)fd, s.data, (size_t)s.len);
}

void osc_file_close(int32_t fd)
{
    close((L_FD)fd);
}

int32_t osc_file_delete(osc_str path)
{
    char buf[4096];
    osc_path_to_cstr(path, buf, sizeof(buf));
    int32_t r = (int32_t)unlink(buf);
    return r < 0 ? -1 : 0;
}

#else /* libc mode */

#ifdef _WIN32
#include <io.h>
#include <fcntl.h>
#include <sys/stat.h>
#define OSC_OPEN   _open
#define OSC_READ   _read
#define OSC_WRITE  _write
#define OSC_CLOSE  _close
#define OSC_UNLINK _unlink
#define OSC_O_RDONLY _O_RDONLY
#define OSC_O_BINARY _O_BINARY
#define OSC_O_WRONLY _O_WRONLY
#define OSC_O_CREAT  _O_CREAT
#define OSC_O_TRUNC  _O_TRUNC
#define OSC_S_IREAD  _S_IREAD
#define OSC_S_IWRITE _S_IWRITE
#else
#include <unistd.h>
#include <fcntl.h>
#include <sys/stat.h>
#define OSC_OPEN   open
#define OSC_READ   read
#define OSC_WRITE  write
#define OSC_CLOSE  close
#define OSC_UNLINK unlink
#define OSC_O_RDONLY O_RDONLY
#define OSC_O_BINARY 0
#define OSC_O_WRONLY O_WRONLY
#define OSC_O_CREAT  O_CREAT
#define OSC_O_TRUNC  O_TRUNC
#define OSC_S_IREAD  (S_IRUSR | S_IRGRP | S_IROTH)
#define OSC_S_IWRITE (S_IWUSR | S_IWGRP)
#endif

int32_t osc_file_open_read(osc_str path)
{
    char buf[4096];
    osc_path_to_cstr(path, buf, sizeof(buf));
    return (int32_t)OSC_OPEN(buf, OSC_O_RDONLY | OSC_O_BINARY);
}

int32_t osc_file_open_write(osc_str path)
{
    char buf[4096];
    osc_path_to_cstr(path, buf, sizeof(buf));
    return (int32_t)OSC_OPEN(buf, OSC_O_WRONLY | OSC_O_CREAT | OSC_O_TRUNC | OSC_O_BINARY,
                             OSC_S_IREAD | OSC_S_IWRITE);
}

int32_t osc_read_byte(int32_t fd)
{
    unsigned char c;
    int n = OSC_READ(fd, &c, 1);
    return n == 1 ? (int32_t)c : -1;
}

void osc_write_byte(int32_t fd, int32_t b)
{
    unsigned char c = (unsigned char)b;
    OSC_WRITE(fd, &c, 1);
}

void osc_write_str(int32_t fd, osc_str s)
{
    if (s.len > 0) OSC_WRITE(fd, s.data, (unsigned)s.len);
}

void osc_file_close(int32_t fd)
{
    OSC_CLOSE(fd);
}

int32_t osc_file_delete(osc_str path)
{
    char buf[4096];
    osc_path_to_cstr(path, buf, sizeof(buf));
    return (int32_t)OSC_UNLINK(buf);
}

#endif /* OSC_FREESTANDING — file I/O */

/* ================================================================== */
/*  Type-cast functions                                                */
/* ==================================================================*/

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

/* ================================================================== */
/*  Command-line argument access                                       */
/* ================================================================== */

int32_t osc_arg_count(void)
{
    return (int32_t)osc_global_argc;
}

osc_str osc_arg_get(osc_arena *arena, int32_t i)
{
    (void)arena;
    if (i < 0 || i >= (int32_t)osc_global_argc) {
        OSC_PANIC("arg_get: index out of bounds");
    }
    return osc_str_from_cstr(osc_global_argv[i]);
}

/* ================================================================== */
/*  Character classification & conversion                              */
/* ================================================================== */

#ifdef OSC_FREESTANDING
uint8_t osc_char_is_alpha(int32_t c)  { return l_isalpha(c) ? 1 : 0; }
uint8_t osc_char_is_digit(int32_t c)  { return l_isdigit(c) ? 1 : 0; }
uint8_t osc_char_is_alnum(int32_t c)  { return l_isalnum(c) ? 1 : 0; }
uint8_t osc_char_is_space(int32_t c)  { return l_isspace(c) ? 1 : 0; }
uint8_t osc_char_is_upper(int32_t c)  { return l_isupper(c) ? 1 : 0; }
uint8_t osc_char_is_lower(int32_t c)  { return l_islower(c) ? 1 : 0; }
uint8_t osc_char_is_print(int32_t c)  { return l_isprint(c) ? 1 : 0; }
uint8_t osc_char_is_xdigit(int32_t c) { return l_isxdigit(c) ? 1 : 0; }
int32_t osc_char_to_upper(int32_t c)  { return (int32_t)l_toupper(c); }
int32_t osc_char_to_lower(int32_t c)  { return (int32_t)l_tolower(c); }
#else
#include <ctype.h>
uint8_t osc_char_is_alpha(int32_t c)  { return isalpha(c) ? 1 : 0; }
uint8_t osc_char_is_digit(int32_t c)  { return isdigit(c) ? 1 : 0; }
uint8_t osc_char_is_alnum(int32_t c)  { return isalnum(c) ? 1 : 0; }
uint8_t osc_char_is_space(int32_t c)  { return isspace(c) ? 1 : 0; }
uint8_t osc_char_is_upper(int32_t c)  { return isupper(c) ? 1 : 0; }
uint8_t osc_char_is_lower(int32_t c)  { return islower(c) ? 1 : 0; }
uint8_t osc_char_is_print(int32_t c)  { return isprint(c) ? 1 : 0; }
uint8_t osc_char_is_xdigit(int32_t c) { return isxdigit(c) ? 1 : 0; }
int32_t osc_char_to_upper(int32_t c)  { return (int32_t)toupper(c); }
int32_t osc_char_to_lower(int32_t c)  { return (int32_t)tolower(c); }
#endif

/* ================================================================== */
/*  abs_i64                                                            */
/* ================================================================== */

int64_t osc_abs_i64(int64_t n)
{
    if (n == INT64_MIN) {
        OSC_PANIC("abs_i64: MIN_VALUE has no positive counterpart");
    }
    return n < 0 ? -n : n;
}

/* ================================================================== */
/*  Number parsing                                                     */
/* ================================================================== */

osc_result_i32_str osc_parse_i32(osc_str s)
{
    osc_result_i32_str result;
    char buf[64];
    int32_t len = s.len < 63 ? s.len : 63;
    int i;
    int sign = 0;
    int32_t val = 0;
    
    /* Copy to null-terminated buffer */
    for (i = 0; i < len; i++) buf[i] = s.data[i];
    buf[len] = '\0';

    i = 0;
    /* Skip whitespace */
    while (i < len && osc_char_is_space((int32_t)(unsigned char)buf[i])) i++;
    if (i == len) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("parse_i32: empty or whitespace-only string");
        return result;
    }
    /* Sign */
    if (buf[i] == '-') { sign = 1; i++; }
    else if (buf[i] == '+') { i++; }
    
    if (i == len || !osc_char_is_digit((int32_t)(unsigned char)buf[i])) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("parse_i32: invalid integer string");
        return result;
    }
    
    while (i < len && osc_char_is_digit((int32_t)(unsigned char)buf[i])) {
        int32_t digit = buf[i] - '0';
        if (val > (INT32_MAX - digit) / 10) {
            result.is_ok = 0;
            result.value.err = osc_str_from_cstr("parse_i32: overflow");
            return result;
        }
        val = val * 10 + digit;
        i++;
    }
    
    result.is_ok = 1;
    result.value.ok = sign ? -val : val;
    return result;
}

osc_result_i64_str osc_parse_i64(osc_str s)
{
    osc_result_i64_str result;
    char buf[64];
    int32_t len = s.len < 63 ? s.len : 63;
    int i;
    int sign = 0;
    int64_t val = 0;
    
    for (i = 0; i < len; i++) buf[i] = s.data[i];
    buf[len] = '\0';

    i = 0;
    while (i < len && osc_char_is_space((int32_t)(unsigned char)buf[i])) i++;
    if (i == len) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("parse_i64: empty or whitespace-only string");
        return result;
    }
    if (buf[i] == '-') { sign = 1; i++; }
    else if (buf[i] == '+') { i++; }
    
    if (i == len || !osc_char_is_digit((int32_t)(unsigned char)buf[i])) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("parse_i64: invalid integer string");
        return result;
    }
    
    while (i < len && osc_char_is_digit((int32_t)(unsigned char)buf[i])) {
        int64_t digit = buf[i] - '0';
        if (val > (INT64_MAX - digit) / 10) {
            result.is_ok = 0;
            result.value.err = osc_str_from_cstr("parse_i64: overflow");
            return result;
        }
        val = val * 10 + digit;
        i++;
    }
    
    result.is_ok = 1;
    result.value.ok = sign ? -val : val;
    return result;
}

osc_str osc_str_from_i64(osc_arena *arena, int64_t n)
{
    char buf[32];
#ifdef OSC_FREESTANDING
    int len = l_snprintf(buf, sizeof(buf), "%lld", (long long)n);
#else
    int len = snprintf(buf, sizeof(buf), "%lld", (long long)n);
#endif
    char *copy = (char *)osc_arena_alloc(arena, (size_t)len + 1);
    for (int i = 0; i <= len; i++) copy[i] = buf[i];
    osc_str result;
    result.data = copy;
    result.len = (int32_t)len;
    return result;
}

osc_str osc_str_from_f64(osc_arena *arena, double n)
{
    char buf[64];
#ifdef OSC_FREESTANDING
    int len = l_snprintf(buf, sizeof(buf), "%.6f", n);
    char *dot = l_strchr(buf, '.');
#else
    int len = snprintf(buf, sizeof(buf), "%.6f", n);
    char *dot = strchr(buf, '.');
#endif
    if (dot) {
        while (len > 1 && buf[len - 1] == '0') len--;
        if (len > 0 && buf[len - 1] == '.') len--;
    }
    buf[len] = '\0';
    char *copy = (char *)osc_arena_alloc(arena, (size_t)len + 1);
    for (int i = 0; i <= len; i++) copy[i] = buf[i];
    osc_str result;
    result.data = copy;
    result.len = (int32_t)len;
    return result;
}

osc_str osc_str_from_bool(uint8_t b)
{
    return b ? osc_str_from_cstr("true") : osc_str_from_cstr("false");
}

/* ================================================================== */
/*  Environment & error                                                */
/* ================================================================== */

osc_result_str_str osc_env_get(osc_arena *arena, osc_str name)
{
    osc_result_str_str result;
    char buf[256];
    int32_t len = name.len < 255 ? name.len : 255;
    int i;
    char *val;
    
    for (i = 0; i < len; i++) buf[i] = name.data[i];
    buf[len] = '\0';
    
#ifdef OSC_FREESTANDING
    val = l_getenv(buf);
#else
    val = getenv(buf);
#endif
    if (!val) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("env_get: variable not found");
        return result;
    }
    
    {
        int32_t vlen = (int32_t)strlen(val);
        char *copy = (char *)osc_arena_alloc(arena, (size_t)vlen + 1);
        for (i = 0; i < vlen; i++) copy[i] = val[i];
        copy[vlen] = '\0';
        result.is_ok = 1;
        result.value.ok.data = copy;
        result.value.ok.len = vlen;
    }
    return result;
}

int32_t osc_errno_get(void)
{
#ifdef OSC_FREESTANDING
    return (int32_t)l_errno();
#else
    return (int32_t)errno;
#endif
}

osc_str osc_errno_str(int32_t code)
{
#ifdef OSC_FREESTANDING
    return osc_str_from_cstr((char *)l_strerror(code));
#else
    return osc_str_from_cstr(strerror(code));
#endif
}

/* ================================================================== */
/*  System: random, time, sleep, exit                                  */
/* ================================================================== */

void osc_rand_seed(int32_t seed)
{
#ifdef OSC_FREESTANDING
    l_srand((unsigned int)seed);
#else
    srand((unsigned int)seed);
#endif
}

int32_t osc_rand_i32(void)
{
#ifdef OSC_FREESTANDING
    return (int32_t)(l_rand() & 0x7FFFFFFF);
#else
    return (int32_t)(rand() & 0x7FFFFFFF);
#endif
}

int64_t osc_time_now(void)
{
#ifdef OSC_FREESTANDING
    return (int64_t)l_time(0);
#else
    return (int64_t)time(0);
#endif
}

void osc_sleep_ms(int32_t ms)
{
#ifdef OSC_FREESTANDING
    l_sleep_ms((unsigned int)ms);
#else
#ifdef _WIN32
    Sleep((unsigned int)ms);
#else
    {
        struct timespec ts;
        ts.tv_sec = ms / 1000;
        ts.tv_nsec = (ms % 1000) * 1000000L;
        nanosleep(&ts, NULL);
    }
#endif
#endif
}

void osc_exit(int32_t code)
{
#ifdef OSC_FREESTANDING
    l_exit(code);
#else
    exit(code);
#endif
}

/* ================================================================== */
/*  Tier 5: Filesystem operations                                      */
/* ================================================================== */

static void osc_str_to_cstr_buf(osc_str s, char *buf, int32_t bufsz)
{
    int32_t len = s.len < (bufsz - 1) ? s.len : (bufsz - 1);
    int i;
    for (i = 0; i < len; i++) buf[i] = s.data[i];
    buf[len] = '\0';
}

int32_t osc_file_rename(osc_str old_path, osc_str new_path)
{
    char obuf[1024], nbuf[1024];
    osc_str_to_cstr_buf(old_path, obuf, 1024);
    osc_str_to_cstr_buf(new_path, nbuf, 1024);
#ifdef OSC_FREESTANDING
    return l_rename(obuf, nbuf);
#else
    return rename(obuf, nbuf);
#endif
}

uint8_t osc_file_exists(osc_str path)
{
    char buf[1024];
    osc_str_to_cstr_buf(path, buf, 1024);
#ifdef OSC_FREESTANDING
    return l_access(buf, 0) == 0 ? 1 : 0;
#else
#ifdef _WIN32
    return _access(buf, 0) == 0 ? 1 : 0;
#else
    return access(buf, 0) == 0 ? 1 : 0;
#endif
#endif
}

int32_t osc_dir_create(osc_str path)
{
    char buf[1024];
    osc_str_to_cstr_buf(path, buf, 1024);
#ifdef OSC_FREESTANDING
    return l_mkdir(buf, 0755);
#else
#ifdef _WIN32
    return _mkdir(buf);
#else
    return mkdir(buf, 0755);
#endif
#endif
}

int32_t osc_dir_remove(osc_str path)
{
    char buf[1024];
    osc_str_to_cstr_buf(path, buf, 1024);
#ifdef OSC_FREESTANDING
    return l_rmdir(buf);
#else
#ifdef _WIN32
    return _rmdir(buf);
#else
    return rmdir(buf);
#endif
#endif
}

osc_str osc_dir_current(osc_arena *arena)
{
    char buf[1024];
#ifdef OSC_FREESTANDING
    if (!l_getcwd(buf, sizeof(buf))) {
#else
#ifdef _WIN32
    if (!_getcwd(buf, (int)sizeof(buf))) {
#else
    if (!getcwd(buf, sizeof(buf))) {
#endif
#endif
        return osc_str_from_cstr("");
    }
    int32_t len = (int32_t)strlen(buf);
    char *copy = (char *)osc_arena_alloc(arena, (size_t)len + 1);
    int i;
    for (i = 0; i <= len; i++) copy[i] = buf[i];
    osc_str result;
    result.data = copy;
    result.len = len;
    return result;
}

int32_t osc_dir_change(osc_str path)
{
    char buf[1024];
    osc_str_to_cstr_buf(path, buf, 1024);
#ifdef OSC_FREESTANDING
    return l_chdir(buf);
#else
#ifdef _WIN32
    return _chdir(buf);
#else
    return chdir(buf);
#endif
#endif
}

int32_t osc_file_open_append(osc_str path)
{
    char buf[1024];
    osc_str_to_cstr_buf(path, buf, 1024);
#ifdef OSC_FREESTANDING
    return (int32_t)l_open_append(buf);
#else
    FILE *f = fopen(buf, "a");
    if (!f) return -1;
    fclose(f);
    return 0;
#endif
}

int64_t osc_file_size(osc_str path)
{
    char buf[1024];
    osc_str_to_cstr_buf(path, buf, 1024);
#ifdef OSC_FREESTANDING
    L_Stat st;
    if (l_stat(buf, &st) != 0) return -1;
    return (int64_t)st.st_size;
#else
    struct stat st;
    if (stat(buf, &st) != 0) return -1;
    return (int64_t)st.st_size;
#endif
}

/* ================================================================== */
/*  Tier 6: String operations                                          */
/* ================================================================== */

uint8_t osc_str_contains(osc_str s, osc_str sub)
{
    if (sub.len == 0) return 1;
    if (sub.len > s.len) return 0;
    int32_t i, j;
    for (i = 0; i <= s.len - sub.len; i++) {
        for (j = 0; j < sub.len; j++) {
            if (s.data[i + j] != sub.data[j]) break;
        }
        if (j == sub.len) return 1;
    }
    return 0;
}

uint8_t osc_str_starts_with(osc_str s, osc_str prefix)
{
    if (prefix.len > s.len) return 0;
    int32_t i;
    for (i = 0; i < prefix.len; i++) {
        if (s.data[i] != prefix.data[i]) return 0;
    }
    return 1;
}

uint8_t osc_str_ends_with(osc_str s, osc_str suffix)
{
    if (suffix.len > s.len) return 0;
    int32_t offset = s.len - suffix.len;
    int32_t i;
    for (i = 0; i < suffix.len; i++) {
        if (s.data[offset + i] != suffix.data[i]) return 0;
    }
    return 1;
}

osc_str osc_str_trim(osc_arena *arena, osc_str s)
{
    int32_t start = 0, end = s.len;
    while (start < end && osc_char_is_space((int32_t)(unsigned char)s.data[start])) start++;
    while (end > start && osc_char_is_space((int32_t)(unsigned char)s.data[end - 1])) end--;
    int32_t len = end - start;
    char *copy = (char *)osc_arena_alloc(arena, (size_t)len + 1);
    int32_t i;
    for (i = 0; i < len; i++) copy[i] = s.data[start + i];
    copy[len] = '\0';
    osc_str result;
    result.data = copy;
    result.len = len;
    return result;
}

osc_array *osc_str_split(osc_arena *arena, osc_str s, osc_str delim)
{
    osc_array *arr = osc_array_new(arena, sizeof(osc_str), 4);
    if (delim.len == 0 || s.len == 0) {
        osc_str copy;
        copy.data = s.data;
        copy.len = s.len;
        osc_array_push(arena, arr, &copy);
        return arr;
    }
    int32_t start = 0;
    int32_t i;
    for (i = 0; i <= s.len - delim.len; i++) {
        int32_t j;
        int match = 1;
        for (j = 0; j < delim.len; j++) {
            if (s.data[i + j] != delim.data[j]) { match = 0; break; }
        }
        if (match) {
            int32_t len = i - start;
            char *copy = (char *)osc_arena_alloc(arena, (size_t)len + 1);
            int32_t k;
            for (k = 0; k < len; k++) copy[k] = s.data[start + k];
            copy[len] = '\0';
            osc_str part;
            part.data = copy;
            part.len = len;
            osc_array_push(arena, arr, &part);
            start = i + delim.len;
            i = start - 1; /* loop will increment */
        }
    }
    /* Last segment */
    {
        int32_t len = s.len - start;
        char *copy = (char *)osc_arena_alloc(arena, (size_t)len + 1);
        int32_t k;
        for (k = 0; k < len; k++) copy[k] = s.data[start + k];
        copy[len] = '\0';
        osc_str part;
        part.data = copy;
        part.len = len;
        osc_array_push(arena, arr, &part);
    }
    return arr;
}

osc_str osc_str_to_upper(osc_arena *arena, osc_str s)
{
    char *copy = (char *)osc_arena_alloc(arena, (size_t)s.len + 1);
    int32_t i;
    for (i = 0; i < s.len; i++) copy[i] = (char)osc_char_to_upper((int32_t)(unsigned char)s.data[i]);
    copy[s.len] = '\0';
    osc_str result;
    result.data = copy;
    result.len = s.len;
    return result;
}

osc_str osc_str_to_lower(osc_arena *arena, osc_str s)
{
    char *copy = (char *)osc_arena_alloc(arena, (size_t)s.len + 1);
    int32_t i;
    for (i = 0; i < s.len; i++) copy[i] = (char)osc_char_to_lower((int32_t)(unsigned char)s.data[i]);
    copy[s.len] = '\0';
    osc_str result;
    result.data = copy;
    result.len = s.len;
    return result;
}

osc_str osc_str_replace(osc_arena *arena, osc_str s, osc_str old_s, osc_str new_s)
{
    if (old_s.len == 0) {
        /* Return copy */
        char *copy = (char *)osc_arena_alloc(arena, (size_t)s.len + 1);
        int32_t i;
        for (i = 0; i < s.len; i++) copy[i] = s.data[i];
        copy[s.len] = '\0';
        osc_str result;
        result.data = copy;
        result.len = s.len;
        return result;
    }
    /* Count occurrences to compute output size */
    int32_t count = 0;
    int32_t i;
    for (i = 0; i <= s.len - old_s.len; i++) {
        int32_t j;
        int match = 1;
        for (j = 0; j < old_s.len; j++) {
            if (s.data[i + j] != old_s.data[j]) { match = 0; break; }
        }
        if (match) { count++; i += old_s.len - 1; }
    }
    int32_t new_len = s.len + count * (new_s.len - old_s.len);
    char *out = (char *)osc_arena_alloc(arena, (size_t)new_len + 1);
    int32_t oi = 0;
    for (i = 0; i < s.len; ) {
        if (i <= s.len - old_s.len) {
            int32_t j;
            int match = 1;
            for (j = 0; j < old_s.len; j++) {
                if (s.data[i + j] != old_s.data[j]) { match = 0; break; }
            }
            if (match) {
                int32_t k;
                for (k = 0; k < new_s.len; k++) out[oi++] = new_s.data[k];
                i += old_s.len;
                continue;
            }
        }
        out[oi++] = s.data[i++];
    }
    out[oi] = '\0';
    osc_str result;
    result.data = out;
    result.len = oi;
    return result;
}


/* ================================================================== */
/*  Tier 7: Directory listing & process control                        */
/* ================================================================== */

osc_array *osc_dir_list(osc_arena *arena, osc_str path)
{
    osc_array *arr = osc_array_new(arena, sizeof(osc_str), 8);
    char buf[1024];
    osc_str_to_cstr_buf(path, buf, 1024);
#ifdef OSC_FREESTANDING
    L_Dir dir;
    L_DirEntry *ent;
    if (l_opendir(buf, &dir) != 0) return arr;
    while ((ent = l_readdir(&dir)) != 0) {
        const char *name = ent->d_name;
        if (name[0] == '.' && (name[1] == '\0' || (name[1] == '.' && name[2] == '\0')))
            continue;
        int32_t elen = (int32_t)l_strlen(name);
        char *copy = (char *)osc_arena_alloc(arena, (size_t)elen + 1);
        int i;
        for (i = 0; i <= elen; i++) copy[i] = name[i];
        osc_str aentry;
        aentry.data = copy;
        aentry.len = elen;
        osc_array_push(arena, arr, &aentry);
    }
    l_closedir(&dir);
#else
#ifdef _WIN32
    char pattern[1040];
    WIN32_FIND_DATAA fd;
    HANDLE hFind;
    snprintf(pattern, sizeof(pattern), "%s\\*", buf);
    hFind = FindFirstFileA(pattern, &fd);
    if (hFind == INVALID_HANDLE_VALUE) return arr;
    do {
        const char *name = fd.cFileName;
        if (name[0] == '.' && (name[1] == '\0' || (name[1] == '.' && name[2] == '\0')))
            continue;
        {
            int32_t elen = (int32_t)strlen(name);
            char *copy = (char *)osc_arena_alloc(arena, (size_t)elen + 1);
            int i;
            for (i = 0; i <= elen; i++) copy[i] = name[i];
            {
                osc_str aentry;
                aentry.data = copy;
                aentry.len = elen;
                osc_array_push(arena, arr, &aentry);
            }
        }
    } while (FindNextFileA(hFind, &fd));
    FindClose(hFind);
#else
    {
        DIR *d = opendir(buf);
        struct dirent *ent;
        if (!d) return arr;
        while ((ent = readdir(d)) != NULL) {
            const char *name = ent->d_name;
            if (name[0] == '.' && (name[1] == '\0' || (name[1] == '.' && name[2] == '\0')))
                continue;
            {
                int32_t elen = (int32_t)strlen(name);
                char *copy = (char *)osc_arena_alloc(arena, (size_t)elen + 1);
                int i;
                for (i = 0; i <= elen; i++) copy[i] = name[i];
                {
                    osc_str aentry;
                    aentry.data = copy;
                    aentry.len = elen;
                    osc_array_push(arena, arr, &aentry);
                }
            }
        }
        closedir(d);
    }
#endif
#endif
    return arr;
}

int32_t osc_proc_run(osc_str cmd, osc_array *args)
{
    char cbuf[1024];
    osc_str_to_cstr_buf(cmd, cbuf, 1024);
    int32_t nargs = args->len;
    char *argv_ptrs[66];
    static char abuf[64][256];
    int32_t max_args = nargs < 64 ? nargs : 64;
    int32_t i;

    argv_ptrs[0] = cbuf;
    for (i = 0; i < max_args; i++) {
        osc_str *s = (osc_str *)((char *)args->data + (size_t)i * sizeof(osc_str));
        osc_str_to_cstr_buf(*s, abuf[i], 256);
        argv_ptrs[i + 1] = abuf[i];
    }
    argv_ptrs[max_args + 1] = 0;

#ifdef OSC_FREESTANDING
    {
        L_PID pid = l_spawn(cbuf, argv_ptrs, 0);
        int exitcode = 0;
        if (pid < 0) return -1;
        l_wait(pid, &exitcode);
        return (int32_t)exitcode;
    }
#else
#ifdef _WIN32
    (void)argv_ptrs;
    return (int32_t)system(cbuf);
#else
    {
        pid_t pid = fork();
        int status = 0;
        if (pid < 0) return -1;
        if (pid == 0) {
            execvp(cbuf, argv_ptrs);
            _exit(127);
        }
        waitpid(pid, &status, 0);
        return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
    }
#endif
#endif
}

int32_t osc_term_width(void)
{
#ifdef OSC_FREESTANDING
    int rows = 0, cols = 0;
    l_term_size(&rows, &cols);
    return (int32_t)cols;
#else
#ifdef _WIN32
    CONSOLE_SCREEN_BUFFER_INFO csbi;
    if (GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &csbi))
        return (int32_t)(csbi.srWindow.Right - csbi.srWindow.Left + 1);
    return 0;
#else
    {
        struct winsize ws;
        if (ioctl(1, TIOCGWINSZ, &ws) == 0) return (int32_t)ws.ws_col;
        return 0;
    }
#endif
#endif
}

int32_t osc_term_height(void)
{
#ifdef OSC_FREESTANDING
    int rows = 0, cols = 0;
    l_term_size(&rows, &cols);
    return (int32_t)rows;
#else
#ifdef _WIN32
    CONSOLE_SCREEN_BUFFER_INFO csbi;
    if (GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &csbi))
        return (int32_t)(csbi.srWindow.Bottom - csbi.srWindow.Top + 1);
    return 0;
#else
    {
        struct winsize ws;
        if (ioctl(1, TIOCGWINSZ, &ws) == 0) return (int32_t)ws.ws_row;
        return 0;
    }
#endif
#endif
}

/* ================================================================== */
/*  Tier 8: Raw terminal I/O                                           */
/* ================================================================== */

#ifdef OSC_FREESTANDING
static unsigned long osc_saved_term_mode = 0;
#else
#ifdef _WIN32
static DWORD osc_saved_console_mode = 0;
#else
static struct termios osc_saved_termios;
static int osc_termios_saved = 0;
#endif
#endif

int32_t osc_term_raw(void)
{
#ifdef OSC_FREESTANDING
    osc_saved_term_mode = l_term_raw();
    return 0;
#else
#ifdef _WIN32
    {
        HANDLE h = GetStdHandle(STD_INPUT_HANDLE);
        if (h == INVALID_HANDLE_VALUE) return -1;
        if (!GetConsoleMode(h, &osc_saved_console_mode)) return -1;
        if (!SetConsoleMode(h, ENABLE_PROCESSED_INPUT)) return -1;
        return 0;
    }
#else
    {
        struct termios raw;
        if (tcgetattr(STDIN_FILENO, &osc_saved_termios) < 0) return -1;
        osc_termios_saved = 1;
        raw = osc_saved_termios;
        raw.c_lflag &= (tcflag_t)~(ECHO | ICANON | ISIG | IEXTEN);
        raw.c_iflag &= (tcflag_t)~(IXON | ICRNL | BRKINT | INPCK | ISTRIP);
        raw.c_oflag &= (tcflag_t)~(OPOST);
        raw.c_cc[VMIN] = 0;
        raw.c_cc[VTIME] = 0;
        if (tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw) < 0) return -1;
        return 0;
    }
#endif
#endif
}

int32_t osc_term_restore(void)
{
#ifdef OSC_FREESTANDING
    l_term_restore(osc_saved_term_mode);
    return 0;
#else
#ifdef _WIN32
    {
        HANDLE h = GetStdHandle(STD_INPUT_HANDLE);
        if (h == INVALID_HANDLE_VALUE) return -1;
        if (!SetConsoleMode(h, osc_saved_console_mode)) return -1;
        return 0;
    }
#else
    {
        if (!osc_termios_saved) return -1;
        if (tcsetattr(STDIN_FILENO, TCSAFLUSH, &osc_saved_termios) < 0) return -1;
        return 0;
    }
#endif
#endif
}

int32_t osc_read_nonblock(void)
{
#ifdef OSC_FREESTANDING
    {
        char buf[1];
        long n = l_read_nonblock(L_STDIN, buf, 1);
        if (n <= 0) return -1;
        return (int32_t)(unsigned char)buf[0];
    }
#else
#ifdef _WIN32
    {
        if (_kbhit()) return (int32_t)_getch();
        return -1;
    }
#else
    {
        int flags = fcntl(STDIN_FILENO, F_GETFL, 0);
        char buf[1];
        ssize_t n;
        if (flags < 0) return -1;
        fcntl(STDIN_FILENO, F_SETFL, flags | O_NONBLOCK);
        n = read(STDIN_FILENO, buf, 1);
        fcntl(STDIN_FILENO, F_SETFL, flags);
        if (n <= 0) return -1;
        return (int32_t)(unsigned char)buf[0];
    }
#endif
#endif
}

/* ================================================================== */
/*  Tier 9: Environment iteration                                      */
/* ================================================================== */

#ifndef OSC_FREESTANDING
#ifndef _WIN32
extern char **environ;
#endif
#endif

int32_t osc_env_count(void)
{
#ifdef OSC_FREESTANDING
    {
        int32_t count = 0;
        void *handle = l_env_start();
        void *iter = handle;
        char buf[4096];
        while (l_env_next(&iter, buf, sizeof(buf)) != NULL) {
            count++;
        }
        l_env_end(handle);
        return count;
    }
#else
#ifdef _WIN32
    {
        int32_t count = 0;
        LPCH env = GetEnvironmentStringsA();
        LPCH p;
        if (!env) return 0;
        p = env;
        while (*p) {
            count++;
            while (*p) p++;
            p++;
        }
        FreeEnvironmentStringsA(env);
        return count;
    }
#else
    {
        int32_t count = 0;
        char **e = environ;
        if (!e) return 0;
        while (*e) { count++; e++; }
        return count;
    }
#endif
#endif
}

osc_str osc_env_key(osc_arena *arena, int32_t i)
{
#ifdef OSC_FREESTANDING
    {
        void *handle = l_env_start();
        void *iter = handle;
        char buf[4096];
        int32_t idx = 0;
        const char *entry;
        while ((entry = l_env_next(&iter, buf, sizeof(buf))) != NULL) {
            if (idx == i) {
                const char *eq = entry;
                int32_t klen;
                char *copy;
                osc_str result;
                while (*eq && *eq != '=') eq++;
                klen = (int32_t)(eq - entry);
                copy = (char *)osc_arena_alloc(arena, (size_t)klen + 1);
                for (int32_t j = 0; j < klen; j++) copy[j] = entry[j];
                copy[klen] = '\0';
                result.data = copy;
                result.len = klen;
                l_env_end(handle);
                return result;
            }
            idx++;
        }
        l_env_end(handle);
        return osc_str_from_cstr("");
    }
#else
    {
        const char *entry = NULL;
#ifdef _WIN32
        {
            LPCH env = GetEnvironmentStringsA();
            LPCH p;
            int32_t idx = 0;
            if (!env) return osc_str_from_cstr("");
            p = env;
            while (*p) {
                if (idx == i) { entry = p; break; }
                idx++;
                while (*p) p++;
                p++;
            }
            if (entry) {
                const char *eq = entry;
                int32_t klen;
                char *copy;
                osc_str result;
                while (*eq && *eq != '=') eq++;
                klen = (int32_t)(eq - entry);
                copy = (char *)osc_arena_alloc(arena, (size_t)klen + 1);
                for (int32_t j = 0; j < klen; j++) copy[j] = entry[j];
                copy[klen] = '\0';
                result.data = copy;
                result.len = klen;
                FreeEnvironmentStringsA(env);
                return result;
            }
            FreeEnvironmentStringsA(env);
            return osc_str_from_cstr("");
        }
#else
        if (environ && i >= 0) {
            int32_t idx = 0;
            char **e = environ;
            while (*e) {
                if (idx == i) { entry = *e; break; }
                idx++; e++;
            }
        }
        if (entry) {
            const char *eq = entry;
            int32_t klen;
            char *copy;
            osc_str result;
            while (*eq && *eq != '=') eq++;
            klen = (int32_t)(eq - entry);
            copy = (char *)osc_arena_alloc(arena, (size_t)klen + 1);
            for (int32_t j = 0; j < klen; j++) copy[j] = entry[j];
            copy[klen] = '\0';
            result.data = copy;
            result.len = klen;
            return result;
        }
        return osc_str_from_cstr("");
#endif
    }
#endif
}

osc_str osc_env_value(osc_arena *arena, int32_t i)
{
#ifdef OSC_FREESTANDING
    {
        void *handle = l_env_start();
        void *iter = handle;
        char buf[4096];
        int32_t idx = 0;
        const char *entry;
        while ((entry = l_env_next(&iter, buf, sizeof(buf))) != NULL) {
            if (idx == i) {
                const char *eq = entry;
                const char *val;
                int32_t vlen;
                char *copy;
                osc_str result;
                while (*eq && *eq != '=') eq++;
                val = (*eq == '=') ? eq + 1 : eq;
                vlen = (int32_t)strlen(val);
                copy = (char *)osc_arena_alloc(arena, (size_t)vlen + 1);
                for (int32_t j = 0; j < vlen; j++) copy[j] = val[j];
                copy[vlen] = '\0';
                result.data = copy;
                result.len = vlen;
                l_env_end(handle);
                return result;
            }
            idx++;
        }
        l_env_end(handle);
        return osc_str_from_cstr("");
    }
#else
    {
        const char *entry = NULL;
#ifdef _WIN32
        {
            LPCH env = GetEnvironmentStringsA();
            LPCH p;
            int32_t idx = 0;
            if (!env) return osc_str_from_cstr("");
            p = env;
            while (*p) {
                if (idx == i) { entry = p; break; }
                idx++;
                while (*p) p++;
                p++;
            }
            if (entry) {
                const char *eq = entry;
                const char *val;
                int32_t vlen;
                char *copy;
                osc_str result;
                while (*eq && *eq != '=') eq++;
                val = (*eq == '=') ? eq + 1 : eq;
                vlen = (int32_t)strlen(val);
                copy = (char *)osc_arena_alloc(arena, (size_t)vlen + 1);
                for (int32_t j = 0; j < vlen; j++) copy[j] = val[j];
                copy[vlen] = '\0';
                result.data = copy;
                result.len = vlen;
                FreeEnvironmentStringsA(env);
                return result;
            }
            FreeEnvironmentStringsA(env);
            return osc_str_from_cstr("");
        }
#else
        if (environ && i >= 0) {
            int32_t idx = 0;
            char **e = environ;
            while (*e) {
                if (idx == i) { entry = *e; break; }
                idx++; e++;
            }
        }
        if (entry) {
            const char *eq = entry;
            const char *val;
            int32_t vlen;
            char *copy;
            osc_str result;
            while (*eq && *eq != '=') eq++;
            val = (*eq == '=') ? eq + 1 : eq;
            vlen = (int32_t)strlen(val);
            copy = (char *)osc_arena_alloc(arena, (size_t)vlen + 1);
            for (int32_t j = 0; j < vlen; j++) copy[j] = val[j];
            copy[vlen] = '\0';
            result.data = copy;
            result.len = vlen;
            return result;
        }
        return osc_str_from_cstr("");
#endif
    }
#endif
}

/* ================================================================== */
/*  Tier 10: Hex formatting                                            */
/* ================================================================== */

osc_str osc_str_from_i32_hex(osc_arena *arena, int32_t n)
{
    char buf[16];
#ifdef OSC_FREESTANDING
    int len = l_snprintf(buf, sizeof(buf), "%x", (unsigned int)(uint32_t)n);
#else
    int len = snprintf(buf, sizeof(buf), "%x", (unsigned int)(uint32_t)n);
#endif
    char *copy = (char *)osc_arena_alloc(arena, (size_t)len + 1);
    int i;
    for (i = 0; i <= len; i++) copy[i] = buf[i];
    osc_str result;
    result.data = copy;
    result.len = (int32_t)len;
    return result;
}

osc_str osc_str_from_i64_hex(osc_arena *arena, int64_t n)
{
    char buf[32];
#ifdef OSC_FREESTANDING
    int len = l_snprintf(buf, sizeof(buf), "%llx", (unsigned long long)(uint64_t)n);
#else
    int len = snprintf(buf, sizeof(buf), "%llx", (unsigned long long)(uint64_t)n);
#endif
    char *copy = (char *)osc_arena_alloc(arena, (size_t)len + 1);
    int i;
    for (i = 0; i <= len; i++) copy[i] = buf[i];
    osc_str result;
    result.data = copy;
    result.len = (int32_t)len;
    return result;
}

/* ================================================================== */
/*  Tier 11: Array sort (shellsort — no function pointers)             */
/* ================================================================== */

void osc_sort_i32(osc_array *arr)
{
    int32_t *d;
    int32_t n, gap, i, j;
    int32_t tmp;
    if (!arr) OSC_PANIC("sort_i32: array is NULL");
    d = (int32_t *)arr->data;
    n = arr->len;
    for (gap = n / 2; gap > 0; gap /= 2) {
        for (i = gap; i < n; i++) {
            tmp = d[i];
            for (j = i; j >= gap && d[j - gap] > tmp; j -= gap)
                d[j] = d[j - gap];
            d[j] = tmp;
        }
    }
}

void osc_sort_i64(osc_array *arr)
{
    int64_t *d;
    int32_t n, gap, i, j;
    int64_t tmp;
    if (!arr) OSC_PANIC("sort_i64: array is NULL");
    d = (int64_t *)arr->data;
    n = arr->len;
    for (gap = n / 2; gap > 0; gap /= 2) {
        for (i = gap; i < n; i++) {
            tmp = d[i];
            for (j = i; j >= gap && d[j - gap] > tmp; j -= gap)
                d[j] = d[j - gap];
            d[j] = tmp;
        }
    }
}

void osc_sort_f64(osc_array *arr)
{
    double *d;
    int32_t n, gap, i, j;
    double tmp;
    if (!arr) OSC_PANIC("sort_f64: array is NULL");
    d = (double *)arr->data;
    n = arr->len;
    for (gap = n / 2; gap > 0; gap /= 2) {
        for (i = gap; i < n; i++) {
            tmp = d[i];
            for (j = i; j >= gap && d[j - gap] > tmp; j -= gap)
                d[j] = d[j - gap];
            d[j] = tmp;
        }
    }
}

static int osc_str_cmp(osc_str a, osc_str b)
{
    int32_t min_len = a.len < b.len ? a.len : b.len;
    int32_t i;
    for (i = 0; i < min_len; i++) {
        if ((unsigned char)a.data[i] < (unsigned char)b.data[i]) return -1;
        if ((unsigned char)a.data[i] > (unsigned char)b.data[i]) return 1;
    }
    if (a.len < b.len) return -1;
    if (a.len > b.len) return 1;
    return 0;
}

void osc_sort_str(osc_array *arr)
{
    osc_str *d;
    int32_t n, gap, i, j;
    osc_str tmp;
    if (!arr) OSC_PANIC("sort_str: array is NULL");
    d = (osc_str *)arr->data;
    n = arr->len;
    for (gap = n / 2; gap > 0; gap /= 2) {
        for (i = gap; i < n; i++) {
            tmp = d[i];
            for (j = i; j >= gap && osc_str_cmp(d[j - gap], tmp) > 0; j -= gap)
                d[j] = d[j - gap];
            d[j] = tmp;
        }
    }
}
