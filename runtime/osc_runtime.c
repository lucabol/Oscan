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

#ifdef _WIN32
#ifndef _CRT_SECURE_NO_WARNINGS
#define _CRT_SECURE_NO_WARNINGS
#endif
#ifndef _WINSOCK_DEPRECATED_NO_WARNINGS
#define _WINSOCK_DEPRECATED_NO_WARNINGS
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
#include <winsock2.h>
#include <ws2tcpip.h>
#pragma comment(lib, "ws2_32.lib")
#include <windows.h>
#include <direct.h>
#include <io.h>
#include <conio.h>
#else
#include <unistd.h>
#include <dirent.h>
#if !defined(__wasi__)
#include <netdb.h>
#include <sys/wait.h>
#include <sys/ioctl.h>
#include <termios.h>
#endif
#include <fcntl.h>
#include <fnmatch.h>
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

void* osc_array_pop(osc_array *arr)
{
    if (!arr) {
        OSC_PANIC("array is NULL");
    }
    if (arr->len <= 0) {
        OSC_PANIC("pop on empty array");
    }
    arr->len--;
    return (char*)arr->data + (size_t)arr->len * (size_t)arr->elem_size;
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
    return osc_i32_to_str(arena, n);
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

#ifdef OSC_FREESTANDING
    pos += (size_t)l_snprintf(buf + pos, bufsz - pos, "%llu", ipart);
#else
    pos += (size_t)snprintf(buf + pos, bufsz - pos, "%llu", ipart);
#endif

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
    char buf[64];
    osc_format_f64(buf, sizeof(buf), n);
    printf("%s", buf);
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

osc_result_i32_str osc_file_open_read(osc_str path)
{
    osc_result_i32_str result;
    char buf[4096];
    osc_path_to_cstr(path, buf, sizeof(buf));
    int32_t fd = (int32_t)open_read(buf);
    if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("file_open_read: cannot open file"); return result; }
    result.is_ok = 1; result.value.ok = fd; return result;
}

osc_result_i32_str osc_file_open_write(osc_str path)
{
    osc_result_i32_str result;
    char buf[4096];
    osc_path_to_cstr(path, buf, sizeof(buf));
    int32_t fd = (int32_t)open_write(buf);
    if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("file_open_write: cannot open file"); return result; }
    result.is_ok = 1; result.value.ok = fd; return result;
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

osc_result_str_str osc_file_delete(osc_str path)
{
    osc_result_str_str result;
    char buf[4096];
    osc_path_to_cstr(path, buf, sizeof(buf));
    int32_t r = (int32_t)unlink(buf);
    if (r < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("file_delete: cannot delete file"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_str_str osc_read_file(osc_arena *arena, osc_str path)
{
    osc_result_str_str result;
    char pbuf[4096];
    osc_path_to_cstr(path, pbuf, sizeof(pbuf));

    L_FD fd = open_read(pbuf);
    if ((int)fd < 0) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("read_file: cannot open file");
        return result;
    }
    L_Stat st;
    if (l_fstat(fd, &st) != 0 || st.st_size < 0) {
        close(fd);
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("read_file: cannot stat file");
        return result;
    }
    size_t file_len = (size_t)st.st_size;
    char *buf = (char *)osc_arena_alloc(arena, file_len + 1);
    size_t total = 0;
    while (total < file_len) {
        ssize_t n = read(fd, buf + total, file_len - total);
        if (n <= 0) break;
        total += (size_t)n;
    }
    close(fd);
    buf[total] = '\0';
    result.is_ok = 1;
    result.value.ok.data = buf;
    result.value.ok.len = (int32_t)total;
    return result;
}

osc_result_str_str osc_write_file(osc_str path, osc_str data)
{
    osc_result_str_str result;
    char pbuf[4096];
    osc_path_to_cstr(path, pbuf, sizeof(pbuf));

    L_FD fd = open_write(pbuf);
    if ((int)fd < 0) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("write_file: cannot open file");
        return result;
    }
    if (data.len > 0) {
        size_t total = 0;
        while (total < (size_t)data.len) {
            ssize_t n = write(fd, data.data + total, (size_t)data.len - total);
            if (n <= 0) {
                close(fd);
                result.is_ok = 0;
                result.value.err = osc_str_from_cstr("write_file: write failed");
                return result;
            }
            total += (size_t)n;
        }
    }
    close(fd);
    result.is_ok = 1;
    result.value.ok.data = "";
    result.value.ok.len = 0;
    return result;
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

osc_result_i32_str osc_file_open_read(osc_str path)
{
    osc_result_i32_str result;
    char buf[4096];
    osc_path_to_cstr(path, buf, sizeof(buf));
    int32_t fd = (int32_t)OSC_OPEN(buf, OSC_O_RDONLY | OSC_O_BINARY);
    if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("file_open_read: cannot open file"); return result; }
    result.is_ok = 1; result.value.ok = fd; return result;
}

osc_result_i32_str osc_file_open_write(osc_str path)
{
    osc_result_i32_str result;
    char buf[4096];
    osc_path_to_cstr(path, buf, sizeof(buf));
    int32_t fd = (int32_t)OSC_OPEN(buf, OSC_O_WRONLY | OSC_O_CREAT | OSC_O_TRUNC | OSC_O_BINARY,
                             OSC_S_IREAD | OSC_S_IWRITE);
    if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("file_open_write: cannot open file"); return result; }
    result.is_ok = 1; result.value.ok = fd; return result;
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

osc_result_str_str osc_file_delete(osc_str path)
{
    osc_result_str_str result;
    char buf[4096];
    osc_path_to_cstr(path, buf, sizeof(buf));
    if (OSC_UNLINK(buf) != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("file_delete: cannot delete file"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_str_str osc_read_file(osc_arena *arena, osc_str path)
{
    osc_result_str_str result;
    char pbuf[4096];
    osc_path_to_cstr(path, pbuf, sizeof(pbuf));

    int fd = OSC_OPEN(pbuf, OSC_O_RDONLY | OSC_O_BINARY);
    if (fd < 0) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("read_file: cannot open file");
        return result;
    }
#ifdef _WIN32
    struct _stat st;
    if (_fstat(fd, &st) != 0 || st.st_size < 0) {
#else
    struct stat st;
    if (fstat(fd, &st) != 0 || st.st_size < 0) {
#endif
        OSC_CLOSE(fd);
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("read_file: cannot stat file");
        return result;
    }
    size_t file_len = (size_t)st.st_size;
    char *buf = (char *)osc_arena_alloc(arena, file_len + 1);
    size_t total = 0;
    while (total < file_len) {
        int n = OSC_READ(fd, buf + total, (unsigned)(file_len - total));
        if (n <= 0) break;
        total += (size_t)n;
    }
    OSC_CLOSE(fd);
    buf[total] = '\0';
    result.is_ok = 1;
    result.value.ok.data = buf;
    result.value.ok.len = (int32_t)total;
    return result;
}

osc_result_str_str osc_write_file(osc_str path, osc_str data)
{
    osc_result_str_str result;
    char pbuf[4096];
    osc_path_to_cstr(path, pbuf, sizeof(pbuf));

    int fd = OSC_OPEN(pbuf, OSC_O_WRONLY | OSC_O_CREAT | OSC_O_TRUNC | OSC_O_BINARY,
                      OSC_S_IREAD | OSC_S_IWRITE);
    if (fd < 0) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("write_file: cannot open file");
        return result;
    }
    if (data.len > 0) {
        size_t total = 0;
        while (total < (size_t)data.len) {
            int n = OSC_WRITE(fd, data.data + total, (unsigned)((size_t)data.len - total));
            if (n <= 0) {
                OSC_CLOSE(fd);
                result.is_ok = 0;
                result.value.err = osc_str_from_cstr("write_file: write failed");
                return result;
            }
            total += (size_t)n;
        }
    }
    OSC_CLOSE(fd);
    result.is_ok = 1;
    result.value.ok.data = "";
    result.value.ok.len = 0;
    return result;
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

double osc_math_sin(double x)
{
#ifdef OSC_FREESTANDING
    static const double PI  = 3.14159265358979323846;
    static const double PI2 = 6.28318530717958647692;
    int neg = 0;
    x = x - PI2 * (double)(int)(x / PI2);
    if (x < -PI) x += PI2;
    if (x >  PI) x -= PI2;
    if (x < 0) { x = -x; neg = 1; }
    if (x > PI) { x -= PI; neg = !neg; }
    double x2 = x * x;
    double r = x;
    double term = x;
    int i;
    for (i = 1; i <= 8; i++) {
        term *= -x2 / (double)(2*i * (2*i + 1));
        r += term;
    }
    return neg ? -r : r;
#else
    return sin(x);
#endif
}

double osc_math_cos(double x)
{
#ifdef OSC_FREESTANDING
    static const double PI_2 = 1.57079632679489661923;
    return osc_math_sin(x + PI_2);
#else
    return cos(x);
#endif
}

double osc_math_sqrt(double x)
{
#ifdef OSC_FREESTANDING
    if (x < 0.0) return 0.0 / 0.0;
    if (x == 0.0) return 0.0;
    double g = x * 0.5;
    int i;
    for (i = 0; i < 60; i++) {
        g = 0.5 * (g + x / g);
    }
    return g;
#else
    return sqrt(x);
#endif
}

double osc_math_pow(double base, double exponent)
{
#ifdef OSC_FREESTANDING
    if (exponent == 0.0) return 1.0;
    if (base == 0.0) return 0.0;
    if (exponent == (double)(int)exponent && exponent > 0 && exponent < 1024) {
        double r = 1.0;
        int n = (int)exponent;
        double b = base;
        while (n > 0) {
            if (n & 1) r *= b;
            b *= b;
            n >>= 1;
        }
        return r;
    }
    return osc_math_exp(exponent * osc_math_log(base));
#else
    return pow(base, exponent);
#endif
}

double osc_math_exp(double x)
{
#ifdef OSC_FREESTANDING
    static const double LN2 = 0.69314718055994530942;
    if (x > 709.0) return 1.0 / 0.0;
    if (x < -709.0) return 0.0;
    int k = (int)(x / LN2 + (x >= 0 ? 0.5 : -0.5));
    double r = x - (double)k * LN2;
    double sum = 1.0, term = 1.0;
    int i;
    for (i = 1; i <= 20; i++) {
        term *= r / (double)i;
        sum += term;
    }
    while (k > 0) { sum *= 2.0; k--; }
    while (k < 0) { sum *= 0.5; k++; }
    return sum;
#else
    return exp(x);
#endif
}

double osc_math_log(double x)
{
#ifdef OSC_FREESTANDING
    if (x <= 0.0) return -1.0 / 0.0;
    static const double LN2 = 0.69314718055994530942;
    int exp2 = 0;
    while (x >= 2.0) { x *= 0.5; exp2++; }
    while (x <  0.5) { x *= 2.0; exp2--; }
    double y = (x - 1.0) / (x + 1.0);
    double y2 = y * y;
    double sum = 0.0, term = y;
    int i;
    for (i = 0; i < 30; i++) {
        sum += term / (double)(2 * i + 1);
        term *= y2;
    }
    return 2.0 * sum + (double)exp2 * LN2;
#else
    return log(x);
#endif
}

double osc_math_atan2(double y, double x)
{
#ifdef OSC_FREESTANDING
    static const double PI   = 3.14159265358979323846;
    static const double PI_2 = 1.57079632679489661923;
    if (x == 0.0 && y == 0.0) return 0.0;
    if (x == 0.0) return y > 0.0 ? PI_2 : -PI_2;
    double t = y / x;
    int flip = 0;
    if (t < 0) { t = -t; flip = 1; }
    int inv = 0;
    if (t > 1.0) { t = 1.0 / t; inv = 1; }
    double t2 = t * t;
    double sum = 0.0, term = t;
    int i;
    for (i = 0; i < 30; i++) {
        sum += (i % 2 == 0 ? 1.0 : -1.0) * term / (double)(2 * i + 1);
        term *= t2;
    }
    if (inv) sum = PI_2 - sum;
    if (flip) sum = -sum;
    if (x < 0.0) sum += (y >= 0.0 ? PI : -PI);
    return sum;
#else
    return atan2(y, x);
#endif
}

double osc_math_floor(double x)
{
#ifdef OSC_FREESTANDING
    double i = (double)(long long)x;
    return (x < i) ? i - 1.0 : i;
#else
    return floor(x);
#endif
}

double osc_math_ceil(double x)
{
#ifdef OSC_FREESTANDING
    double i = (double)(long long)x;
    return (x > i) ? i + 1.0 : i;
#else
    return ceil(x);
#endif
}

double osc_math_fmod(double x, double y)
{
#ifdef OSC_FREESTANDING
    if (y == 0.0) return 0.0 / 0.0;
    return x - (double)(long long)(x / y) * y;
#else
    return fmod(x, y);
#endif
}

double osc_math_abs(double x)
{
#ifdef OSC_FREESTANDING
    return x < 0.0 ? -x : x;
#else
    return fabs(x);
#endif
}

double osc_math_pi(void)  { return 3.14159265358979323846; }
double osc_math_e(void)   { return 2.71828182845904523536; }
double osc_math_ln2(void) { return 0.69314718055994530942; }
double osc_math_sqrt2(void) { return 1.41421356237309504880; }

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
/*  Min / Max / Clamp                                                  */
/* ================================================================== */

int32_t osc_min_i32(int32_t a, int32_t b) { return a < b ? a : b; }
int32_t osc_max_i32(int32_t a, int32_t b) { return a > b ? a : b; }
int32_t osc_clamp_i32(int32_t v, int32_t lo, int32_t hi) { return v < lo ? lo : (v > hi ? hi : v); }

int64_t osc_min_i64(int64_t a, int64_t b) { return a < b ? a : b; }
int64_t osc_max_i64(int64_t a, int64_t b) { return a > b ? a : b; }
int64_t osc_clamp_i64(int64_t v, int64_t lo, int64_t hi) { return v < lo ? lo : (v > hi ? hi : v); }

double osc_min_f64(double a, double b) { return a < b ? a : b; }
double osc_max_f64(double a, double b) { return a > b ? a : b; }
double osc_clamp_f64(double v, double lo, double hi) { return v < lo ? lo : (v > hi ? hi : v); }

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

osc_result_str_str osc_file_rename(osc_str old_path, osc_str new_path)
{
    osc_result_str_str result;
    char obuf[1024], nbuf[1024];
    int rc;
    osc_str_to_cstr_buf(old_path, obuf, 1024);
    osc_str_to_cstr_buf(new_path, nbuf, 1024);
#ifdef OSC_FREESTANDING
    rc = l_rename(obuf, nbuf);
#else
    rc = rename(obuf, nbuf);
#endif
    if (rc != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("file_rename: cannot rename file"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
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

osc_result_str_str osc_dir_create(osc_str path)
{
    osc_result_str_str result;
    char buf[1024];
    int rc;
    osc_str_to_cstr_buf(path, buf, 1024);
#ifdef OSC_FREESTANDING
    rc = l_mkdir(buf, 0755);
#else
#ifdef _WIN32
    rc = _mkdir(buf);
#else
    rc = mkdir(buf, 0755);
#endif
#endif
    if (rc != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("dir_create: cannot create directory"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_str_str osc_dir_remove(osc_str path)
{
    osc_result_str_str result;
    char buf[1024];
    int rc;
    osc_str_to_cstr_buf(path, buf, 1024);
#ifdef OSC_FREESTANDING
    rc = l_rmdir(buf);
#else
#ifdef _WIN32
    rc = _rmdir(buf);
#else
    rc = rmdir(buf);
#endif
#endif
    if (rc != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("dir_remove: cannot remove directory"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
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

osc_result_str_str osc_dir_change(osc_str path)
{
    osc_result_str_str result;
    char buf[1024];
    int rc;
    osc_str_to_cstr_buf(path, buf, 1024);
#ifdef OSC_FREESTANDING
    rc = l_chdir(buf);
#else
#ifdef _WIN32
    rc = _chdir(buf);
#else
    rc = chdir(buf);
#endif
#endif
    if (rc != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("dir_change: cannot change directory"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_i32_str osc_file_open_append(osc_str path)
{
    osc_result_i32_str result;
    char buf[1024];
    osc_str_to_cstr_buf(path, buf, 1024);
#ifdef OSC_FREESTANDING
    {
        int32_t fd = (int32_t)l_open_append(buf);
        if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("file_open_append: cannot open file"); return result; }
        result.is_ok = 1; result.value.ok = fd; return result;
    }
#else
    {
#ifdef _WIN32
        int32_t fd = (int32_t)_open(buf, _O_WRONLY | _O_CREAT | _O_APPEND | _O_BINARY, _S_IREAD | _S_IWRITE);
#else
        int32_t fd = (int32_t)open(buf, O_WRONLY | O_CREAT | O_APPEND, S_IRUSR | S_IWUSR | S_IRGRP | S_IWGRP);
#endif
        if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("file_open_append: cannot open file"); return result; }
        result.is_ok = 1; result.value.ok = fd; return result;
    }
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
/*  Path utilities                                                     */
/* ================================================================== */

osc_str osc_path_join(osc_arena *arena, osc_str dir, osc_str file)
{
    char buf[4096];
    char dir_buf[4096], file_buf[4096];
    int32_t len;
    char *copy;
    osc_str result;

    osc_str_to_cstr_buf(dir, dir_buf, 4096);
    osc_str_to_cstr_buf(file, file_buf, 4096);

#ifdef OSC_FREESTANDING
    l_path_join(buf, sizeof(buf), dir_buf, file_buf);
#else
    snprintf(buf, sizeof(buf), "%s/%s", dir_buf, file_buf);
#endif

    len = (int32_t)strlen(buf);
    copy = (char *)osc_arena_alloc(arena, (size_t)len + 1);
    memcpy(copy, buf, (size_t)len + 1);
    result.data = copy;
    result.len = len;
    return result;
}

osc_str osc_path_ext(osc_str path)
{
    int32_t i;
    int32_t last_dot = -1;
    int32_t last_sep = -1;
    osc_str result;

    for (i = 0; i < path.len; i++) {
        if (path.data[i] == '/' || path.data[i] == '\\') last_sep = i;
        if (path.data[i] == '.') last_dot = i;
    }
    /* No dot, or dot before last separator, or dot is first char of basename */
    if (last_dot <= 0 || last_dot <= last_sep || last_dot == last_sep + 1) {
        result.data = "";
        result.len = 0;
        return result;
    }
    result.data = path.data + last_dot;
    result.len = path.len - last_dot;
    return result;
}

uint8_t osc_path_exists(osc_str path)
{
    char buf[4096];
    osc_str_to_cstr_buf(path, buf, 4096);
#ifdef OSC_FREESTANDING
    return l_path_exists(buf) ? 1 : 0;
#else
    struct stat st;
    return stat(buf, &st) == 0 ? 1 : 0;
#endif
}

uint8_t osc_path_is_dir(osc_str path)
{
    char buf[4096];
    osc_str_to_cstr_buf(path, buf, 4096);
#ifdef OSC_FREESTANDING
    return l_path_isdir(buf) ? 1 : 0;
#else
    struct stat st;
    if (stat(buf, &st) != 0) return 0;
#ifdef _WIN32
    return (st.st_mode & _S_IFDIR) ? 1 : 0;
#else
    return S_ISDIR(st.st_mode) ? 1 : 0;
#endif
#endif
}

osc_str osc_path_basename(osc_str path)
{
    osc_str result;
    int32_t i;
    int32_t last_sep = -1;

    if (path.len == 0) {
        result.data = "";
        result.len = 0;
        return result;
    }
    for (i = 0; i < path.len; i++) {
        if (path.data[i] == '/' || path.data[i] == '\\') last_sep = i;
    }
    if (last_sep < 0) {
        /* No separator — entire path is the basename */
        return path;
    }
    result.data = path.data + last_sep + 1;
    result.len = path.len - last_sep - 1;
    return result;
}

osc_str osc_path_dirname(osc_arena *arena, osc_str path)
{
    osc_str result;
    int32_t i;
    int32_t last_sep = -1;
    char *copy;

    if (path.len == 0) {
        result.data = ".";
        result.len = 1;
        return result;
    }
    for (i = 0; i < path.len; i++) {
        if (path.data[i] == '/' || path.data[i] == '\\') last_sep = i;
    }
    if (last_sep < 0) {
        result.data = ".";
        result.len = 1;
        return result;
    }
    if (last_sep == 0) {
        /* Root directory */
        copy = (char *)osc_arena_alloc(arena, 2);
        copy[0] = path.data[0];
        copy[1] = '\0';
        result.data = copy;
        result.len = 1;
        return result;
    }
    copy = (char *)osc_arena_alloc(arena, (size_t)last_sep + 1);
    memcpy(copy, path.data, (size_t)last_sep);
    copy[last_sep] = '\0';
    result.data = copy;
    result.len = last_sep;
    return result;
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

osc_str osc_str_from_chars(osc_arena *arena, osc_array *arr)
{
    if (!arr) {
        OSC_PANIC("str_from_chars: array is NULL");
    }
    char *buf = (char *)osc_arena_alloc(arena, (size_t)arr->len + 1);
    int32_t i;
    for (i = 0; i < arr->len; i++) {
        int32_t ch = *(int32_t *)osc_array_get(arr, i);
        buf[i] = (char)(unsigned char)ch;
    }
    buf[arr->len] = '\0';
    osc_str result;
    result.data = buf;
    result.len = arr->len;
    return result;
}

osc_array *osc_str_to_chars(osc_arena *arena, osc_str s)
{
    osc_array *arr = osc_array_new(arena, sizeof(int32_t), s.len > 0 ? s.len : 1);
    int32_t i;
    for (i = 0; i < s.len; i++) {
        int32_t ch = (int32_t)(unsigned char)s.data[i];
        osc_array_push(arena, arr, &ch);
    }
    return arr;
}

osc_str osc_str_join(osc_arena *arena, osc_array *arr, osc_str sep)
{
    osc_str result;
    int32_t i;
    int32_t total_len;
    char *buf;
    int32_t pos;

    if (!arr) {
        OSC_PANIC("str_join: array is NULL");
    }
    if (arr->len == 0) {
        result.data = "";
        result.len = 0;
        return result;
    }
    /* Compute total length */
    total_len = 0;
    for (i = 0; i < arr->len; i++) {
        osc_str *s = (osc_str *)osc_array_get(arr, i);
        total_len += s->len;
    }
    total_len += sep.len * (arr->len - 1);

    buf = (char *)osc_arena_alloc(arena, (size_t)total_len + 1);
    pos = 0;
    for (i = 0; i < arr->len; i++) {
        osc_str *s = (osc_str *)osc_array_get(arr, i);
        if (i > 0 && sep.len > 0) {
            memcpy(buf + pos, sep.data, (size_t)sep.len);
            pos += sep.len;
        }
        if (s->len > 0) {
            memcpy(buf + pos, s->data, (size_t)s->len);
            pos += s->len;
        }
    }
    buf[pos] = '\0';
    result.data = buf;
    result.len = pos;
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

osc_result_str_str osc_term_raw(void)
{
    osc_result_str_str result;
#ifdef OSC_FREESTANDING
    osc_saved_term_mode = l_term_raw();
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
#else
#ifdef _WIN32
    {
        HANDLE h = GetStdHandle(STD_INPUT_HANDLE);
        if (h == INVALID_HANDLE_VALUE) { result.is_ok = 0; result.value.err = osc_str_from_cstr("term_raw: invalid console handle"); return result; }
        if (!GetConsoleMode(h, &osc_saved_console_mode)) { result.is_ok = 0; result.value.err = osc_str_from_cstr("term_raw: cannot get console mode"); return result; }
        if (!SetConsoleMode(h, ENABLE_PROCESSED_INPUT)) { result.is_ok = 0; result.value.err = osc_str_from_cstr("term_raw: cannot set console mode"); return result; }
        result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
    }
#else
    {
        struct termios raw;
        if (tcgetattr(STDIN_FILENO, &osc_saved_termios) < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("term_raw: tcgetattr failed"); return result; }
        osc_termios_saved = 1;
        raw = osc_saved_termios;
        raw.c_lflag &= (tcflag_t)~(ECHO | ICANON | ISIG | IEXTEN);
        raw.c_iflag &= (tcflag_t)~(IXON | ICRNL | BRKINT | INPCK | ISTRIP);
        raw.c_oflag &= (tcflag_t)~(OPOST);
        raw.c_cc[VMIN] = 0;
        raw.c_cc[VTIME] = 0;
        if (tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw) < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("term_raw: tcsetattr failed"); return result; }
        result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
    }
#endif
#endif
}

osc_result_str_str osc_term_restore(void)
{
    osc_result_str_str result;
#ifdef OSC_FREESTANDING
    l_term_restore(osc_saved_term_mode);
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
#else
#ifdef _WIN32
    {
        HANDLE h = GetStdHandle(STD_INPUT_HANDLE);
        if (h == INVALID_HANDLE_VALUE) { result.is_ok = 0; result.value.err = osc_str_from_cstr("term_restore: invalid console handle"); return result; }
        if (!SetConsoleMode(h, osc_saved_console_mode)) { result.is_ok = 0; result.value.err = osc_str_from_cstr("term_restore: cannot restore console mode"); return result; }
        result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
    }
#else
    {
        if (!osc_termios_saved) { result.is_ok = 0; result.value.err = osc_str_from_cstr("term_restore: no saved state"); return result; }
        if (tcsetattr(STDIN_FILENO, TCSAFLUSH, &osc_saved_termios) < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("term_restore: tcsetattr failed"); return result; }
        result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
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

/* ================================================================== */
/*  Hash map (string→string, open addressing, FNV-1a)                  */
/* ================================================================== */

typedef struct {
    osc_str   key;
    osc_str   value;
    uint32_t  hash;
    int8_t    state;  /* 0=empty, 1=occupied, 2=tombstone */
} osc_map_slot;

struct osc_map {
    osc_map_slot *slots;
    int32_t       cap;
    int32_t       len;
    osc_arena    *arena;
};

static uint32_t osc_map_fnv(const char *data, int32_t len)
{
    uint32_t h = 2166136261u;
    int32_t i;
    for (i = 0; i < len; i++) {
        h ^= (unsigned char)data[i];
        h *= 16777619u;
    }
    return h;
}

static int osc_map_keys_eq(osc_str a, const char *bdata, int32_t blen)
{
    int32_t i;
    if (a.len != blen) return 0;
    for (i = 0; i < blen; i++) {
        if (a.data[i] != bdata[i]) return 0;
    }
    return 1;
}

static void osc_map_grow(osc_arena *arena, osc_map *m)
{
    int32_t old_cap = m->cap;
    osc_map_slot *old_slots = m->slots;
    int32_t new_cap = old_cap * 2;
    int32_t i;
    uint32_t mask;

    m->cap = new_cap;
    m->len = 0;
    m->slots = (osc_map_slot *)osc_arena_alloc(arena,
                    (size_t)new_cap * sizeof(osc_map_slot));
    memset(m->slots, 0, (size_t)new_cap * sizeof(osc_map_slot));
    mask = (uint32_t)(new_cap - 1);

    for (i = 0; i < old_cap; i++) {
        if (old_slots[i].state == 1) {
            uint32_t idx = old_slots[i].hash & mask;
            while (m->slots[idx].state != 0)
                idx = (idx + 1) & mask;
            m->slots[idx] = old_slots[i];
            m->len++;
        }
    }
}

osc_map *osc_map_new(osc_arena *arena)
{
    osc_map *m = (osc_map *)osc_arena_alloc(arena, sizeof(osc_map));
    m->arena = arena;
    m->cap = 16;
    m->len = 0;
    m->slots = (osc_map_slot *)osc_arena_alloc(arena,
                    (size_t)m->cap * sizeof(osc_map_slot));
    memset(m->slots, 0, (size_t)m->cap * sizeof(osc_map_slot));
    return m;
}

void osc_map_set(osc_arena *arena, osc_map *m, osc_str key, osc_str value)
{
    uint32_t h, mask, idx, first_tomb;
    int tomb_found = 0;

    if (!m) OSC_PANIC("map_set: map is NULL");

    /* Grow if >75% full */
    if (m->len * 4 >= m->cap * 3)
        osc_map_grow(arena, m);

    h = osc_map_fnv(key.data, key.len);
    mask = (uint32_t)(m->cap - 1);
    idx = h & mask;
    first_tomb = 0;

    {
        uint32_t i;
        for (i = 0; i < (uint32_t)m->cap; i++) {
            uint32_t si = (idx + i) & mask;
            osc_map_slot *s = &m->slots[si];
            if (s->state == 0) {
                uint32_t target = tomb_found ? first_tomb : si;
                osc_map_slot *t = &m->slots[target];
                t->key = key;
                t->value = value;
                t->hash = h;
                t->state = 1;
                m->len++;
                return;
            }
            if (s->state == 2 && !tomb_found) {
                first_tomb = si;
                tomb_found = 1;
            }
            if (s->state == 1 && s->hash == h &&
                osc_map_keys_eq(s->key, key.data, key.len)) {
                s->value = value;
                return;
            }
        }
    }
    OSC_PANIC("map_set: map is full");
}

osc_str osc_map_get(osc_map *m, osc_str key)
{
    uint32_t h, mask, idx;
    osc_str empty;

    if (!m) OSC_PANIC("map_get: map is NULL");

    h = osc_map_fnv(key.data, key.len);
    mask = (uint32_t)(m->cap - 1);
    idx = h & mask;

    {
        uint32_t i;
        for (i = 0; i < (uint32_t)m->cap; i++) {
            uint32_t si = (idx + i) & mask;
            osc_map_slot *s = &m->slots[si];
            if (s->state == 0) break;
            if (s->state == 1 && s->hash == h &&
                osc_map_keys_eq(s->key, key.data, key.len))
                return s->value;
        }
    }
    empty.data = ""; empty.len = 0;
    OSC_PANIC("map_get: key not found");
    return empty;
}

uint8_t osc_map_has(osc_map *m, osc_str key)
{
    uint32_t h, mask, idx;

    if (!m) return 0;

    h = osc_map_fnv(key.data, key.len);
    mask = (uint32_t)(m->cap - 1);
    idx = h & mask;

    {
        uint32_t i;
        for (i = 0; i < (uint32_t)m->cap; i++) {
            uint32_t si = (idx + i) & mask;
            osc_map_slot *s = &m->slots[si];
            if (s->state == 0) return 0;
            if (s->state == 1 && s->hash == h &&
                osc_map_keys_eq(s->key, key.data, key.len))
                return 1;
        }
    }
    return 0;
}

void osc_map_delete(osc_map *m, osc_str key)
{
    uint32_t h, mask, idx;

    if (!m) OSC_PANIC("map_delete: map is NULL");

    h = osc_map_fnv(key.data, key.len);
    mask = (uint32_t)(m->cap - 1);
    idx = h & mask;

    {
        uint32_t i;
        for (i = 0; i < (uint32_t)m->cap; i++) {
            uint32_t si = (idx + i) & mask;
            osc_map_slot *s = &m->slots[si];
            if (s->state == 0) return;
            if (s->state == 1 && s->hash == h &&
                osc_map_keys_eq(s->key, key.data, key.len)) {
                s->state = 2;
                m->len--;
                return;
            }
        }
    }
}

int32_t osc_map_len(osc_map *m)
{
    if (!m) return 0;
    return m->len;
}

/* ================================================================== */
/*  Typed hash maps — wrappers around osc_map with type conversions    */
/* ================================================================== */

/* Internal: find a slot by key, returns pointer or NULL */
static osc_map_slot *osc_map_find_slot(osc_map *m, const char *kdata, int32_t klen)
{
    uint32_t h, mask, idx;
    if (!m) return NULL;
    h = osc_map_fnv(kdata, klen);
    mask = (uint32_t)(m->cap - 1);
    idx = h & mask;
    {
        uint32_t i;
        for (i = 0; i < (uint32_t)m->cap; i++) {
            uint32_t si = (idx + i) & mask;
            osc_map_slot *s = &m->slots[si];
            if (s->state == 0) return NULL;
            if (s->state == 1 && s->hash == h &&
                osc_map_keys_eq(s->key, kdata, klen))
                return s;
        }
    }
    return NULL;
}

/* Stack-buffer i32→osc_str (no arena allocation) */
static osc_str osc_map_i32_key(char *buf, size_t bufsz, int32_t n)
{
    osc_str s;
#ifdef OSC_FREESTANDING
    int len = snprintf(buf, bufsz, "%d", (int)n);
#else
    int len = snprintf(buf, bufsz, "%" PRId32, n);
#endif
    s.data = buf;
    s.len = (int32_t)len;
    return s;
}

/* Arena-allocated i32→osc_str for persistent map key storage */
static osc_str osc_map_i32_key_arena(osc_arena *arena, int32_t n)
{
    return osc_i32_to_str(arena, n);
}

/* Store a raw int32_t as osc_str value (binary, 4 bytes) */
static osc_str osc_map_val_from_i32(osc_arena *arena, int32_t v)
{
    return osc_str_from_i32(arena, v);
}

/* Store a raw int64_t as osc_str value (binary, 8 bytes) */
static osc_str osc_map_val_from_i64(osc_arena *arena, int64_t v)
{
    return osc_str_from_i64(arena, v);
}

/* Store a double as text string value */
static osc_str osc_map_val_from_f64(osc_arena *arena, double v)
{
    return osc_str_from_f64(arena, v);
}

/* Read raw int32_t from osc_str value */
static int32_t osc_map_val_to_i32(osc_str s)
{
    return osc_parse_i32(s).value.ok;
}

/* Read raw int64_t from osc_str value */
static int64_t osc_map_val_to_i64(osc_str s)
{
    return osc_parse_i64(s).value.ok;
}

/* Read double from text string value */
static double osc_map_val_to_f64(osc_str s)
{
    /* Simple manual parse: atof-style */
    char buf[64];
    int32_t len = s.len < 63 ? s.len : 63;
    int i;
    for (i = 0; i < len; i++) buf[i] = s.data[i];
    buf[len] = '\0';
    double result = 0.0;
    int neg = 0, j = 0;
    if (buf[0] == '-') { neg = 1; j = 1; }
    while (buf[j] >= '0' && buf[j] <= '9') { result = result * 10.0 + (buf[j] - '0'); j++; }
    if (buf[j] == '.') {
        j++;
        double frac = 0.1;
        while (buf[j] >= '0' && buf[j] <= '9') { result += (buf[j] - '0') * frac; frac *= 0.1; j++; }
    }
    return neg ? -result : result;
}

/* --- map_str_i32 --- */

osc_map *osc_map_str_i32_new(osc_arena *arena)
{
    return osc_map_new(arena);
}

void osc_map_str_i32_set(osc_arena *arena, osc_map *m, osc_str key, int32_t value)
{
    osc_map_set(arena, m, key, osc_map_val_from_i32(arena, value));
}

int32_t osc_map_str_i32_get(osc_map *m, osc_str key)
{
    osc_map_slot *s = osc_map_find_slot(m, key.data, key.len);
    if (!s) return 0;
    return osc_map_val_to_i32(s->value);
}

uint8_t osc_map_str_i32_has(osc_map *m, osc_str key)
{
    return osc_map_has(m, key);
}

void osc_map_str_i32_delete(osc_map *m, osc_str key)
{
    osc_map_delete(m, key);
}

int32_t osc_map_str_i32_len(osc_map *m)
{
    return osc_map_len(m);
}

/* --- map_str_i64 --- */

osc_map *osc_map_str_i64_new(osc_arena *arena)
{
    return osc_map_new(arena);
}

void osc_map_str_i64_set(osc_arena *arena, osc_map *m, osc_str key, int64_t value)
{
    osc_map_set(arena, m, key, osc_map_val_from_i64(arena, value));
}

int64_t osc_map_str_i64_get(osc_map *m, osc_str key)
{
    osc_map_slot *s = osc_map_find_slot(m, key.data, key.len);
    if (!s) return 0;
    return osc_map_val_to_i64(s->value);
}

uint8_t osc_map_str_i64_has(osc_map *m, osc_str key)
{
    return osc_map_has(m, key);
}

void osc_map_str_i64_delete(osc_map *m, osc_str key)
{
    osc_map_delete(m, key);
}

int32_t osc_map_str_i64_len(osc_map *m)
{
    return osc_map_len(m);
}

/* --- map_str_f64 --- */

osc_map *osc_map_str_f64_new(osc_arena *arena)
{
    return osc_map_new(arena);
}

void osc_map_str_f64_set(osc_arena *arena, osc_map *m, osc_str key, double value)
{
    osc_map_set(arena, m, key, osc_map_val_from_f64(arena, value));
}

double osc_map_str_f64_get(osc_map *m, osc_str key)
{
    osc_map_slot *s = osc_map_find_slot(m, key.data, key.len);
    if (!s) return 0.0;
    return osc_map_val_to_f64(s->value);
}

uint8_t osc_map_str_f64_has(osc_map *m, osc_str key)
{
    return osc_map_has(m, key);
}

void osc_map_str_f64_delete(osc_map *m, osc_str key)
{
    osc_map_delete(m, key);
}

int32_t osc_map_str_f64_len(osc_map *m)
{
    return osc_map_len(m);
}

/* --- map_i32_str --- */

osc_map *osc_map_i32_str_new(osc_arena *arena)
{
    return osc_map_new(arena);
}

void osc_map_i32_str_set(osc_arena *arena, osc_map *m, int32_t key, osc_str value)
{
    osc_map_set(arena, m, osc_map_i32_key_arena(arena, key), value);
}

osc_str osc_map_i32_str_get(osc_map *m, int32_t key)
{
    char buf[16];
    osc_str ks = osc_map_i32_key(buf, sizeof(buf), key);
    osc_map_slot *s = osc_map_find_slot(m, ks.data, ks.len);
    if (!s) { osc_str empty; empty.data = ""; empty.len = 0; return empty; }
    return s->value;
}

uint8_t osc_map_i32_str_has(osc_map *m, int32_t key)
{
    char buf[16];
    osc_str ks = osc_map_i32_key(buf, sizeof(buf), key);
    return osc_map_find_slot(m, ks.data, ks.len) != NULL;
}

void osc_map_i32_str_delete(osc_map *m, int32_t key)
{
    char buf[16];
    osc_str ks = osc_map_i32_key(buf, sizeof(buf), key);
    osc_map_delete(m, ks);
}

int32_t osc_map_i32_str_len(osc_map *m)
{
    return osc_map_len(m);
}

/* --- map_i32_i32 --- */

osc_map *osc_map_i32_i32_new(osc_arena *arena)
{
    return osc_map_new(arena);
}

void osc_map_i32_i32_set(osc_arena *arena, osc_map *m, int32_t key, int32_t value)
{
    osc_map_set(arena, m, osc_map_i32_key_arena(arena, key),
                osc_map_val_from_i32(arena, value));
}

int32_t osc_map_i32_i32_get(osc_map *m, int32_t key)
{
    char buf[16];
    osc_str ks = osc_map_i32_key(buf, sizeof(buf), key);
    osc_map_slot *s = osc_map_find_slot(m, ks.data, ks.len);
    if (!s) return 0;
    return osc_map_val_to_i32(s->value);
}

uint8_t osc_map_i32_i32_has(osc_map *m, int32_t key)
{
    char buf[16];
    osc_str ks = osc_map_i32_key(buf, sizeof(buf), key);
    return osc_map_find_slot(m, ks.data, ks.len) != NULL;
}

void osc_map_i32_i32_delete(osc_map *m, int32_t key)
{
    char buf[16];
    osc_str ks = osc_map_i32_key(buf, sizeof(buf), key);
    osc_map_delete(m, ks);
}

int32_t osc_map_i32_i32_len(osc_map *m)
{
    return osc_map_len(m);
}

/* ================================================================== */
/*  Tier 13: Date/Time                                                 */
/* ================================================================== */

osc_str osc_time_format(osc_arena *arena, int64_t timestamp, osc_str fmt)
{
    osc_str result;
    char fmtbuf[256];
    char buf[256];
    int len;

    osc_str_to_cstr_buf(fmt, fmtbuf, 256);

#ifdef OSC_FREESTANDING
    {
        L_Tm tm = l_gmtime((long long)timestamp);
        len = l_strftime(buf, sizeof(buf), fmtbuf, &tm);
    }
#else
    {
        time_t t = (time_t)timestamp;
        struct tm *tm;
#ifdef _WIN32
        struct tm tm_buf;
        gmtime_s(&tm_buf, &t);
        tm = &tm_buf;
#else
        tm = gmtime(&t);
#endif
        len = (int)strftime(buf, sizeof(buf), fmtbuf, tm);
    }
#endif
    if (len <= 0) { result.data = ""; result.len = 0; return result; }
    {
        char *copy = (char *)osc_arena_alloc(arena, (size_t)len + 1);
        int i;
        for (i = 0; i <= len; i++) copy[i] = buf[i];
        result.data = copy;
        result.len = (int32_t)len;
    }
    return result;
}

int32_t osc_time_utc_year(int64_t timestamp)
{
#ifdef OSC_FREESTANDING
    L_Tm tm = l_gmtime((long long)timestamp);
    return (int32_t)(tm.year + 1900);
#else
    time_t t = (time_t)timestamp;
    struct tm *tm;
#ifdef _WIN32
    struct tm tm_buf;
    gmtime_s(&tm_buf, &t);
    tm = &tm_buf;
#else
    tm = gmtime(&t);
#endif
    return (int32_t)(tm->tm_year + 1900);
#endif
}

int32_t osc_time_utc_month(int64_t timestamp)
{
#ifdef OSC_FREESTANDING
    L_Tm tm = l_gmtime((long long)timestamp);
    return (int32_t)(tm.mon + 1);
#else
    time_t t = (time_t)timestamp;
    struct tm *tm;
#ifdef _WIN32
    struct tm tm_buf;
    gmtime_s(&tm_buf, &t);
    tm = &tm_buf;
#else
    tm = gmtime(&t);
#endif
    return (int32_t)(tm->tm_mon + 1);
#endif
}

int32_t osc_time_utc_day(int64_t timestamp)
{
#ifdef OSC_FREESTANDING
    L_Tm tm = l_gmtime((long long)timestamp);
    return (int32_t)tm.mday;
#else
    time_t t = (time_t)timestamp;
    struct tm *tm;
#ifdef _WIN32
    struct tm tm_buf;
    gmtime_s(&tm_buf, &t);
    tm = &tm_buf;
#else
    tm = gmtime(&t);
#endif
    return (int32_t)tm->tm_mday;
#endif
}

int32_t osc_time_utc_hour(int64_t timestamp)
{
#ifdef OSC_FREESTANDING
    L_Tm tm = l_gmtime((long long)timestamp);
    return (int32_t)tm.hour;
#else
    time_t t = (time_t)timestamp;
    struct tm *tm;
#ifdef _WIN32
    struct tm tm_buf;
    gmtime_s(&tm_buf, &t);
    tm = &tm_buf;
#else
    tm = gmtime(&t);
#endif
    return (int32_t)tm->tm_hour;
#endif
}

int32_t osc_time_utc_min(int64_t timestamp)
{
#ifdef OSC_FREESTANDING
    L_Tm tm = l_gmtime((long long)timestamp);
    return (int32_t)tm.min;
#else
    time_t t = (time_t)timestamp;
    struct tm *tm;
#ifdef _WIN32
    struct tm tm_buf;
    gmtime_s(&tm_buf, &t);
    tm = &tm_buf;
#else
    tm = gmtime(&t);
#endif
    return (int32_t)tm->tm_min;
#endif
}

int32_t osc_time_utc_sec(int64_t timestamp)
{
#ifdef OSC_FREESTANDING
    L_Tm tm = l_gmtime((long long)timestamp);
    return (int32_t)tm.sec;
#else
    time_t t = (time_t)timestamp;
    struct tm *tm;
#ifdef _WIN32
    struct tm tm_buf;
    gmtime_s(&tm_buf, &t);
    tm = &tm_buf;
#else
    tm = gmtime(&t);
#endif
    return (int32_t)tm->tm_sec;
#endif
}

/* ================================================================== */
/*  Tier 13: Glob matching                                             */
/* ================================================================== */

uint8_t osc_glob_match(osc_str pattern, osc_str text)
{
    char pbuf[1024], tbuf[1024];
    osc_str_to_cstr_buf(pattern, pbuf, 1024);
    osc_str_to_cstr_buf(text, tbuf, 1024);
#ifdef OSC_FREESTANDING
    return l_fnmatch(pbuf, tbuf) == 0 ? 1 : 0;
#else
#ifdef _WIN32
    /* Minimal glob match for libc mode on Windows */
    {
        const char *p = pbuf, *t = tbuf;
        const char *star_p = NULL, *star_t = NULL;
        while (*t) {
            if (*p == *t || *p == '?') { p++; t++; }
            else if (*p == '*') { star_p = p++; star_t = t; }
            else if (star_p) { p = star_p + 1; t = ++star_t; }
            else return 0;
        }
        while (*p == '*') p++;
        return *p == '\0' ? 1 : 0;
    }
#else
    {
        return fnmatch(pbuf, tbuf, 0) == 0 ? 1 : 0;
    }
#endif
#endif
}

/* ================================================================== */
/*  Tier 13: SHA-256                                                   */
/* ================================================================== */

osc_str osc_sha256(osc_arena *arena, osc_str data)
{
    osc_str result;
    unsigned char hash[32];
    char *hex;
    int i;
    static const char hexchars[] = "0123456789abcdef";

#ifdef OSC_FREESTANDING
    l_sha256(data.data, (size_t)data.len, hash);
#else
    /* Self-contained SHA-256 for libc mode */
    {
        static const unsigned int sha256_k[64] = {
            0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
            0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
            0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
            0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
            0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
            0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
            0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
            0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2
        };
        unsigned int h0=0x6a09e667, h1=0xbb67ae85, h2=0x3c6ef372, h3=0xa54ff53a;
        unsigned int h4=0x510e527f, h5=0x9b05688c, h6=0x1f83d9ab, h7=0x5be0cd19;
        unsigned long long total_bits = (unsigned long long)data.len * 8;
        const unsigned char *msg = (const unsigned char *)data.data;
        size_t msg_len = (size_t)data.len;
        /* Process message in 64-byte chunks including padding */
        size_t padded_len = ((msg_len + 8) / 64 + 1) * 64;
        unsigned char *padded = (unsigned char *)osc_arena_alloc(arena, padded_len);
        size_t ci;
        for (ci = 0; ci < msg_len; ci++) padded[ci] = msg[ci];
        padded[msg_len] = 0x80;
        for (ci = msg_len + 1; ci < padded_len - 8; ci++) padded[ci] = 0;
        for (i = 0; i < 8; i++)
            padded[padded_len - 1 - i] = (unsigned char)(total_bits >> (i * 8));

        for (ci = 0; ci < padded_len; ci += 64) {
            unsigned int w[64], a, b, c, d, e, f, g, hh, t1, t2;
            int j;
            for (j = 0; j < 16; j++)
                w[j] = ((unsigned int)padded[ci+j*4]<<24) | ((unsigned int)padded[ci+j*4+1]<<16) |
                       ((unsigned int)padded[ci+j*4+2]<<8) | ((unsigned int)padded[ci+j*4+3]);
            for (j = 16; j < 64; j++) {
                unsigned int s0 = ((w[j-15]>>7)|(w[j-15]<<25)) ^ ((w[j-15]>>18)|(w[j-15]<<14)) ^ (w[j-15]>>3);
                unsigned int s1 = ((w[j-2]>>17)|(w[j-2]<<15)) ^ ((w[j-2]>>19)|(w[j-2]<<13)) ^ (w[j-2]>>10);
                w[j] = w[j-16] + s0 + w[j-7] + s1;
            }
            a=h0; b=h1; c=h2; d=h3; e=h4; f=h5; g=h6; hh=h7;
            for (j = 0; j < 64; j++) {
                unsigned int S1 = ((e>>6)|(e<<26)) ^ ((e>>11)|(e<<21)) ^ ((e>>25)|(e<<7));
                unsigned int ch = (e & f) ^ ((~e) & g);
                t1 = hh + S1 + ch + sha256_k[j] + w[j];
                {
                unsigned int S0 = ((a>>2)|(a<<30)) ^ ((a>>13)|(a<<19)) ^ ((a>>22)|(a<<10));
                unsigned int maj = (a & b) ^ (a & c) ^ (b & c);
                t2 = S0 + maj;
                }
                hh=g; g=f; f=e; e=d+t1; d=c; c=b; b=a; a=t1+t2;
            }
            h0+=a; h1+=b; h2+=c; h3+=d; h4+=e; h5+=f; h6+=g; h7+=hh;
        }
        hash[0]=(unsigned char)(h0>>24); hash[1]=(unsigned char)(h0>>16); hash[2]=(unsigned char)(h0>>8); hash[3]=(unsigned char)h0;
        hash[4]=(unsigned char)(h1>>24); hash[5]=(unsigned char)(h1>>16); hash[6]=(unsigned char)(h1>>8); hash[7]=(unsigned char)h1;
        hash[8]=(unsigned char)(h2>>24); hash[9]=(unsigned char)(h2>>16); hash[10]=(unsigned char)(h2>>8); hash[11]=(unsigned char)h2;
        hash[12]=(unsigned char)(h3>>24); hash[13]=(unsigned char)(h3>>16); hash[14]=(unsigned char)(h3>>8); hash[15]=(unsigned char)h3;
        hash[16]=(unsigned char)(h4>>24); hash[17]=(unsigned char)(h4>>16); hash[18]=(unsigned char)(h4>>8); hash[19]=(unsigned char)h4;
        hash[20]=(unsigned char)(h5>>24); hash[21]=(unsigned char)(h5>>16); hash[22]=(unsigned char)(h5>>8); hash[23]=(unsigned char)h5;
        hash[24]=(unsigned char)(h6>>24); hash[25]=(unsigned char)(h6>>16); hash[26]=(unsigned char)(h6>>8); hash[27]=(unsigned char)h6;
        hash[28]=(unsigned char)(h7>>24); hash[29]=(unsigned char)(h7>>16); hash[30]=(unsigned char)(h7>>8); hash[31]=(unsigned char)h7;
    }
#endif

    hex = (char *)osc_arena_alloc(arena, 65);
    for (i = 0; i < 32; i++) {
        hex[i * 2]     = hexchars[(hash[i] >> 4) & 0x0f];
        hex[i * 2 + 1] = hexchars[hash[i] & 0x0f];
    }
    hex[64] = '\0';
    result.data = hex;
    result.len = 64;
    return result;
}

/* ================================================================== */
/*  Tier 13: Terminal detection                                        */
/* ================================================================== */

uint8_t osc_is_tty(void)
{
#ifdef OSC_FREESTANDING
    return l_isatty(L_STDOUT) ? 1 : 0;
#else
#ifdef _WIN32
    return _isatty(_fileno(stdout)) ? 1 : 0;
#else
    return isatty(STDOUT_FILENO) ? 1 : 0;
#endif
#endif
}

/* ================================================================== */
/*  Tier 13: Environment modification                                  */
/* ================================================================== */

osc_result_str_str osc_env_set(osc_str name, osc_str value)
{
    osc_result_str_str result;
    char nbuf[256], vbuf[4096];
    int rc;
    osc_str_to_cstr_buf(name, nbuf, 256);
    osc_str_to_cstr_buf(value, vbuf, 4096);
#ifdef OSC_FREESTANDING
    rc = (int)l_setenv(nbuf, vbuf);
#else
#ifdef _WIN32
    rc = _putenv_s(nbuf, vbuf) == 0 ? 0 : -1;
#else
    rc = setenv(nbuf, vbuf, 1) == 0 ? 0 : -1;
#endif
#endif
    if (rc != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("env_set: cannot set environment variable"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_str_str osc_env_delete(osc_str name)
{
    osc_result_str_str result;
    char nbuf[256];
    int rc;
    osc_str_to_cstr_buf(name, nbuf, 256);
#ifdef OSC_FREESTANDING
    rc = (int)l_unsetenv(nbuf);
#else
#ifdef _WIN32
    rc = _putenv_s(nbuf, "") == 0 ? 0 : -1;
#else
    rc = unsetenv(nbuf) == 0 ? 0 : -1;
#endif
#endif
    if (rc != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("env_delete: cannot delete environment variable"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

/* ================================================================== */
/*  Graphics wrappers (requires l_gfx.h)                               */
/* ================================================================== */

#ifdef OSC_HAS_GFX

static L_Canvas osc_gfx_canvas;

osc_result_str_str osc_canvas_open(int32_t width, int32_t height, osc_str title) {
    osc_result_str_str result;
    char buf[256];
    osc_str_to_cstr_buf(title, buf, 256);
    int rc = (int)l_canvas_open(&osc_gfx_canvas, width, height, buf);
    if (rc != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("canvas_open: cannot open canvas"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

void osc_canvas_close(void) { l_canvas_close(&osc_gfx_canvas); }
uint8_t osc_canvas_alive(void) { return l_canvas_alive(&osc_gfx_canvas) ? 1 : 0; }
void osc_canvas_flush(void) { l_canvas_flush(&osc_gfx_canvas); }
void osc_canvas_clear(int32_t color) { l_canvas_clear(&osc_gfx_canvas, (uint32_t)color); }

void osc_gfx_pixel(int32_t x, int32_t y, int32_t color) { l_pixel(&osc_gfx_canvas, x, y, (uint32_t)color); }
int32_t osc_gfx_get_pixel(int32_t x, int32_t y) { return (int32_t)l_get_pixel(&osc_gfx_canvas, x, y); }
void osc_gfx_line(int32_t x0, int32_t y0, int32_t x1, int32_t y1, int32_t color) { l_line(&osc_gfx_canvas, x0, y0, x1, y1, (uint32_t)color); }
void osc_gfx_rect(int32_t x, int32_t y, int32_t w, int32_t h, int32_t color) { l_rect(&osc_gfx_canvas, x, y, w, h, (uint32_t)color); }
void osc_gfx_fill_rect(int32_t x, int32_t y, int32_t w, int32_t h, int32_t color) { l_fill_rect(&osc_gfx_canvas, x, y, w, h, (uint32_t)color); }
void osc_gfx_circle(int32_t cx, int32_t cy, int32_t r, int32_t color) { l_circle(&osc_gfx_canvas, cx, cy, r, (uint32_t)color); }
void osc_gfx_fill_circle(int32_t cx, int32_t cy, int32_t r, int32_t color) { l_fill_circle(&osc_gfx_canvas, cx, cy, r, (uint32_t)color); }

void osc_gfx_draw_text(int32_t x, int32_t y, osc_str text, int32_t color) {
    char buf[1024];
    osc_str_to_cstr_buf(text, buf, 1024);
    l_draw_text(&osc_gfx_canvas, x, y, buf, (uint32_t)color);
}

void osc_gfx_draw_text_scaled(int32_t x, int32_t y, osc_str text, int32_t color, int32_t sx, int32_t sy) {
    char buf[4096];
    osc_str_to_cstr_buf(text, buf, sizeof(buf));
    l_draw_text_scaled(&osc_gfx_canvas, x, y, buf, (uint32_t)color, sx, sy);
}

void osc_gfx_blit(int32_t dx, int32_t dy, int32_t w, int32_t h, osc_array *pixels) {
    if (!pixels || w <= 0 || h <= 0 || pixels->len < w * h) return;
    l_blit(&osc_gfx_canvas, dx, dy, w, h, (const uint32_t *)pixels->data, w * 4);
}

void osc_gfx_blit_alpha(int32_t dx, int32_t dy, int32_t w, int32_t h, osc_array *pixels) {
    if (!pixels || w <= 0 || h <= 0 || pixels->len < w * h) return;
    l_blit_alpha(&osc_gfx_canvas, dx, dy, w, h, (const uint32_t *)pixels->data, w * 4);
}

int32_t osc_canvas_key(void) { return (int32_t)l_canvas_key(&osc_gfx_canvas); }
int32_t osc_canvas_mouse_x(void) { return (int32_t)osc_gfx_canvas.mouse_x; }
int32_t osc_canvas_mouse_y(void) { return (int32_t)osc_gfx_canvas.mouse_y; }
int32_t osc_canvas_mouse_btn(void) { return (int32_t)osc_gfx_canvas.mouse_btn; }

int32_t osc_rgb(int32_t r, int32_t g, int32_t b) { return (int32_t)L_RGB(r, g, b); }
int32_t osc_rgba(int32_t r, int32_t g, int32_t b, int32_t a) { return (int32_t)L_RGBA(r, g, b, a); }

#else /* !OSC_HAS_GFX — stub implementations for platforms without graphics */

osc_result_str_str osc_canvas_open(int32_t width, int32_t height, osc_str title) {
    (void)width; (void)height; (void)title;
    osc_result_str_str r; r.is_ok = 0; r.value.err = osc_str_from_cstr("canvas_open: not supported on this platform"); return r;
}
void    osc_canvas_close(void) {}
uint8_t osc_canvas_alive(void) { return 0; }
void    osc_canvas_flush(void) {}
void    osc_canvas_clear(int32_t color) { (void)color; }
void    osc_gfx_pixel(int32_t x, int32_t y, int32_t color) { (void)x; (void)y; (void)color; }
int32_t osc_gfx_get_pixel(int32_t x, int32_t y) { (void)x; (void)y; return 0; }
void    osc_gfx_line(int32_t x0, int32_t y0, int32_t x1, int32_t y1, int32_t color) { (void)x0; (void)y0; (void)x1; (void)y1; (void)color; }
void    osc_gfx_rect(int32_t x, int32_t y, int32_t w, int32_t h, int32_t color) { (void)x; (void)y; (void)w; (void)h; (void)color; }
void    osc_gfx_fill_rect(int32_t x, int32_t y, int32_t w, int32_t h, int32_t color) { (void)x; (void)y; (void)w; (void)h; (void)color; }
void    osc_gfx_circle(int32_t cx, int32_t cy, int32_t r, int32_t color) { (void)cx; (void)cy; (void)r; (void)color; }
void    osc_gfx_fill_circle(int32_t cx, int32_t cy, int32_t r, int32_t color) { (void)cx; (void)cy; (void)r; (void)color; }
void    osc_gfx_draw_text(int32_t x, int32_t y, osc_str text, int32_t color) { (void)x; (void)y; (void)text; (void)color; }
void    osc_gfx_draw_text_scaled(int32_t x, int32_t y, osc_str text, int32_t color, int32_t sx, int32_t sy) { (void)x; (void)y; (void)text; (void)color; (void)sx; (void)sy; }
void    osc_gfx_blit(int32_t dx, int32_t dy, int32_t w, int32_t h, osc_array *pixels) { (void)dx; (void)dy; (void)w; (void)h; (void)pixels; }
void    osc_gfx_blit_alpha(int32_t dx, int32_t dy, int32_t w, int32_t h, osc_array *pixels) { (void)dx; (void)dy; (void)w; (void)h; (void)pixels; }
int32_t osc_canvas_key(void) { return 0; }
int32_t osc_canvas_mouse_x(void) { return 0; }
int32_t osc_canvas_mouse_y(void) { return 0; }
int32_t osc_canvas_mouse_btn(void) { return 0; }
int32_t osc_rgb(int32_t r, int32_t g, int32_t b) { return (int32_t)(0xFF000000u | ((uint32_t)r<<16) | ((uint32_t)g<<8) | (uint32_t)b); }
int32_t osc_rgba(int32_t r, int32_t g, int32_t b, int32_t a) { return (int32_t)(((uint32_t)a<<24) | ((uint32_t)r<<16) | ((uint32_t)g<<8) | (uint32_t)b); }

#endif /* OSC_HAS_GFX */

/* ================================================================== */
/*  Socket wrappers                                                    */
/* ================================================================== */

/* Shared helper for libc modes: resolve hostname or IPv4 address to sockaddr_in.
   Uses getaddrinfo() for DNS/hostname resolution (requires libc). */
#if !defined(OSC_FREESTANDING) && !defined(__wasi__)
static int osc_socket_lookup_ipv4(osc_str addr, int32_t port, struct sockaddr_in *sa)
{
    char host[256];
    struct addrinfo hints;
    struct addrinfo *result = NULL;
    struct addrinfo *it;

    if (!sa) return -1;
    if (port < 0 || port > 65535) return -1;

    osc_path_to_cstr(addr, host, sizeof(host));
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;

    if (getaddrinfo(host, NULL, &hints, &result) != 0) {
        return -1;
    }

    for (it = result; it != NULL; it = it->ai_next) {
        if (it->ai_family == AF_INET && (size_t)it->ai_addrlen >= sizeof(*sa)) {
            memcpy(sa, it->ai_addr, sizeof(*sa));
            sa->sin_port = htons((unsigned short)port);
            freeaddrinfo(result);
            return 0;
        }
    }

    freeaddrinfo(result);
    return -1;
}
#endif

#ifdef OSC_HAS_SOCKETS

static int osc_socket_port_is_valid(int32_t port)
{
    return port >= 0 && port <= 65535;
}

osc_result_i32_str osc_socket_tcp(void)
{
    osc_result_i32_str result;
    int32_t fd = (int32_t)l_socket_tcp();
    if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_tcp: cannot create socket"); return result; }
    result.is_ok = 1; result.value.ok = fd; return result;
}

osc_result_str_str osc_socket_connect(int32_t sock, osc_str addr, int32_t port)
{
    osc_result_str_str result;
    char host[256];
    char resolved[16];

    if (!osc_socket_port_is_valid(port)) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_connect: invalid port"); return result; }
    osc_path_to_cstr(addr, host, sizeof(host));
    if (l_resolve(host, resolved) < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_connect: cannot resolve address"); return result; }
    if (l_socket_connect((L_SOCKET)sock, resolved, (int)port) != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_connect: connection failed"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_str_str osc_socket_bind(int32_t sock, int32_t port)
{
    osc_result_str_str result;
    if (l_socket_bind((L_SOCKET)sock, (int)port) != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_bind: cannot bind socket"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_str_str osc_socket_listen(int32_t sock, int32_t backlog)
{
    osc_result_str_str result;
    if (l_socket_listen((L_SOCKET)sock, (int)backlog) != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_listen: listen failed"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_i32_str osc_socket_accept(int32_t sock)
{
    osc_result_i32_str result;
    int32_t fd = (int32_t)l_socket_accept((L_SOCKET)sock);
    if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_accept: accept failed"); return result; }
    result.is_ok = 1; result.value.ok = fd; return result;
}

osc_result_i32_str osc_socket_send(int32_t sock, osc_str data)
{
    osc_result_i32_str result;
    int32_t n = (int32_t)l_socket_send((L_SOCKET)sock, data.data, (size_t)data.len);
    if (n < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_send: send failed"); return result; }
    result.is_ok = 1; result.value.ok = n; return result;
}

osc_str osc_socket_recv(osc_arena *arena, int32_t sock, int32_t max_len)
{
    osc_str result;
    char *buf;
    ptrdiff_t n;
    if (max_len <= 0 || max_len > 65536) max_len = 4096;
    buf = (char *)osc_arena_alloc(arena, (size_t)max_len);
    if (!buf) { result.data = ""; result.len = 0; return result; }
    n = l_socket_recv((L_SOCKET)sock, buf, (size_t)max_len);
    if (n <= 0) { result.data = ""; result.len = 0; return result; }
    result.data = buf;
    result.len = (int32_t)n;
    return result;
}

void osc_socket_close(int32_t sock)
{
    l_socket_close((L_SOCKET)sock);
}

osc_result_i32_str osc_socket_udp(void)
{
    osc_result_i32_str result;
    int32_t fd = (int32_t)l_socket_udp();
    if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_udp: cannot create socket"); return result; }
    result.is_ok = 1; result.value.ok = fd; return result;
}

int32_t osc_socket_sendto(int32_t sock, osc_str data, osc_str addr, int32_t port)
{
    char host[256];
    char resolved[16];

    if (!osc_socket_port_is_valid(port)) return -1;
    osc_path_to_cstr(addr, host, sizeof(host));
    if (l_resolve(host, resolved) < 0) return -1;
    return (int32_t)l_socket_sendto((L_SOCKET)sock, data.data, (size_t)data.len, resolved, (int)port);
}

osc_str osc_socket_recvfrom(osc_arena *arena, int32_t sock, int32_t max_len)
{
    osc_str result;
    char *buf;
    ptrdiff_t n;
    char addr_buf[16];
    int port_out;
    if (max_len <= 0 || max_len > 65536) max_len = 4096;
    buf = (char *)osc_arena_alloc(arena, (size_t)max_len);
    if (!buf) { result.data = ""; result.len = 0; return result; }
    n = l_socket_recvfrom((L_SOCKET)sock, buf, (size_t)max_len, addr_buf, &port_out);
    if (n <= 0) { result.data = ""; result.len = 0; return result; }
    result.data = buf;
    result.len = (int32_t)n;
    return result;
}

osc_result_i32_str osc_socket_unix_connect(osc_str path)
{
    osc_result_i32_str result;
#ifdef _WIN32
    (void)path;
    result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_unix_connect: not supported on Windows");
#else
    char buf[256];
    osc_str_to_cstr_buf(path, buf, sizeof(buf));
    int32_t fd = (int32_t)l_socket_unix_connect(buf);
    if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_unix_connect: connection failed"); return result; }
    result.is_ok = 1; result.value.ok = fd;
#endif
    return result;
}

osc_result_i32_str osc_tls_connect(osc_str host, int32_t port)
{
    osc_result_i32_str result;
    char hostname[256];
    osc_str_to_cstr_buf(host, hostname, sizeof(hostname));
    int h = l_tls_connect(hostname, (int)port);
    if (h < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("tls_connect: connection failed"); return result; }
    result.is_ok = 1; result.value.ok = (int32_t)h; return result;
}

osc_result_i32_str osc_tls_send(int32_t handle, osc_str data)
{
    osc_result_i32_str result;
    int n = l_tls_send((int)handle, data.data, (int)data.len);
    if (n < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("tls_send: send failed"); return result; }
    result.is_ok = 1; result.value.ok = (int32_t)n; return result;
}

osc_str osc_tls_recv(osc_arena *arena, int32_t handle, int32_t max_len)
{
    osc_str result;
    char *buf;
    if (max_len <= 0 || max_len > 65536) max_len = 4096;
    buf = (char *)osc_arena_alloc(arena, (size_t)max_len);
    if (!buf) { result.data = ""; result.len = 0; return result; }
    int n = l_tls_recv((int)handle, buf, (int)max_len);
    if (n <= 0) { result.data = ""; result.len = 0; return result; }
    result.data = buf; result.len = (int32_t)n; return result;
}

int32_t osc_tls_recv_byte(int32_t handle)
{
    return (int32_t)l_tls_recv_byte((int)handle);
}

void osc_tls_close(int32_t handle)
{
    l_tls_close((int)handle);
}

void osc_tls_cleanup(void)
{
    l_tls_cleanup();
}

#elif defined(_WIN32) /* Windows libc mode */

#pragma comment(lib, "ws2_32.lib")

static void osc_wsa_init(void)
{
    static int done = 0;
    if (!done) {
        WSADATA wsa;
        WSAStartup(MAKEWORD(2, 2), &wsa);
        done = 1;
    }
}

osc_result_i32_str osc_socket_tcp(void)
{
    osc_result_i32_str result;
    osc_wsa_init();
    SOCKET s = socket(AF_INET, SOCK_STREAM, 0);
    if (s == INVALID_SOCKET) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_tcp: cannot create socket"); return result; }
    result.is_ok = 1; result.value.ok = (int32_t)s; return result;
}

osc_result_str_str osc_socket_connect(int32_t sock, osc_str addr, int32_t port)
{
    osc_result_str_str result;
    struct sockaddr_in sa;
    if (osc_socket_lookup_ipv4(addr, port, &sa) < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_connect: cannot resolve address"); return result; }
    if (connect((SOCKET)sock, (struct sockaddr *)&sa, sizeof(sa)) != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_connect: connection failed"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_str_str osc_socket_bind(int32_t sock, int32_t port)
{
    osc_result_str_str result;
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons((unsigned short)port);
    sa.sin_addr.s_addr = INADDR_ANY;
    if (bind((SOCKET)sock, (struct sockaddr *)&sa, sizeof(sa)) != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_bind: cannot bind socket"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_str_str osc_socket_listen(int32_t sock, int32_t backlog)
{
    osc_result_str_str result;
    if (listen((SOCKET)sock, (int)backlog) != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_listen: listen failed"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_i32_str osc_socket_accept(int32_t sock)
{
    osc_result_i32_str result;
    SOCKET s = accept((SOCKET)sock, NULL, NULL);
    if (s == INVALID_SOCKET) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_accept: accept failed"); return result; }
    result.is_ok = 1; result.value.ok = (int32_t)s; return result;
}

osc_result_i32_str osc_socket_send(int32_t sock, osc_str data)
{
    osc_result_i32_str result;
    int32_t n = (int32_t)send((SOCKET)sock, data.data, data.len, 0);
    if (n < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_send: send failed"); return result; }
    result.is_ok = 1; result.value.ok = n; return result;
}

osc_str osc_socket_recv(osc_arena *arena, int32_t sock, int32_t max_len)
{
    osc_str result;
    char *buf;
    int n;
    if (max_len <= 0 || max_len > 65536) max_len = 4096;
    buf = (char *)osc_arena_alloc(arena, (size_t)max_len);
    if (!buf) { result.data = ""; result.len = 0; return result; }
    n = recv((SOCKET)sock, buf, max_len, 0);
    if (n <= 0) { result.data = ""; result.len = 0; return result; }
    result.data = buf;
    result.len = (int32_t)n;
    return result;
}

void osc_socket_close(int32_t sock)
{
    closesocket((SOCKET)sock);
}

osc_result_i32_str osc_socket_udp(void)
{
    osc_result_i32_str result;
    osc_wsa_init();
    SOCKET s = socket(AF_INET, SOCK_DGRAM, 0);
    if (s == INVALID_SOCKET) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_udp: cannot create socket"); return result; }
    result.is_ok = 1; result.value.ok = (int32_t)s; return result;
}

int32_t osc_socket_sendto(int32_t sock, osc_str data, osc_str addr, int32_t port)
{
    struct sockaddr_in sa;
    if (osc_socket_lookup_ipv4(addr, port, &sa) < 0) return -1;
    return (int32_t)sendto((SOCKET)sock, data.data, data.len, 0,
                           (struct sockaddr *)&sa, sizeof(sa));
}

osc_str osc_socket_recvfrom(osc_arena *arena, int32_t sock, int32_t max_len)
{
    osc_str result;
    char *buf;
    int n;
    struct sockaddr_in sa;
    int sa_len = sizeof(sa);
    if (max_len <= 0 || max_len > 65536) max_len = 4096;
    buf = (char *)osc_arena_alloc(arena, (size_t)max_len);
    if (!buf) { result.data = ""; result.len = 0; return result; }
    n = recvfrom((SOCKET)sock, buf, max_len, 0,
                 (struct sockaddr *)&sa, &sa_len);
    if (n <= 0) { result.data = ""; result.len = 0; return result; }
    result.data = buf;
    result.len = (int32_t)n;
    return result;
}

osc_result_i32_str osc_socket_unix_connect(osc_str path)
{
    (void)path;
    osc_result_i32_str result;
    result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_unix_connect: not supported on Windows");
    return result;
}

osc_result_i32_str osc_tls_connect(osc_str host, int32_t port)
{
    (void)host; (void)port;
    osc_result_i32_str result;
    result.is_ok = 0; result.value.err = osc_str_from_cstr("tls_connect: not supported in libc mode");
    return result;
}

osc_result_i32_str osc_tls_send(int32_t handle, osc_str data)
{
    (void)handle; (void)data;
    osc_result_i32_str result;
    result.is_ok = 0; result.value.err = osc_str_from_cstr("tls_send: not supported in libc mode");
    return result;
}

osc_str osc_tls_recv(osc_arena *arena, int32_t handle, int32_t max_len)
{
    (void)arena; (void)handle; (void)max_len;
    osc_str result; result.data = ""; result.len = 0; return result;
}

int32_t osc_tls_recv_byte(int32_t handle)
{
    (void)handle; return -1;
}

void osc_tls_close(int32_t handle) { (void)handle; }
void osc_tls_cleanup(void) {}

#else /* POSIX libc */
#include <sys/socket.h>
#include <sys/un.h>
#include <netinet/in.h>
#include <arpa/inet.h>

osc_result_i32_str osc_socket_tcp(void)
{
    osc_result_i32_str result;
    int32_t fd = (int32_t)socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_tcp: cannot create socket"); return result; }
    result.is_ok = 1; result.value.ok = fd; return result;
}

osc_result_str_str osc_socket_connect(int32_t sock, osc_str addr, int32_t port)
{
    osc_result_str_str result;
    struct sockaddr_in sa;
    if (osc_socket_lookup_ipv4(addr, port, &sa) < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_connect: cannot resolve address"); return result; }
    if (connect(sock, (struct sockaddr *)&sa, sizeof(sa)) != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_connect: connection failed"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_str_str osc_socket_bind(int32_t sock, int32_t port)
{
    osc_result_str_str result;
    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons((unsigned short)port);
    sa.sin_addr.s_addr = INADDR_ANY;
    if (bind(sock, (struct sockaddr *)&sa, sizeof(sa)) != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_bind: cannot bind socket"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_str_str osc_socket_listen(int32_t sock, int32_t backlog)
{
    osc_result_str_str result;
    if (listen(sock, (int)backlog) != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_listen: listen failed"); return result; }
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_i32_str osc_socket_accept(int32_t sock)
{
    osc_result_i32_str result;
    int32_t fd = (int32_t)accept(sock, NULL, NULL);
    if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_accept: accept failed"); return result; }
    result.is_ok = 1; result.value.ok = fd; return result;
}

osc_result_i32_str osc_socket_send(int32_t sock, osc_str data)
{
    osc_result_i32_str result;
    int32_t n = (int32_t)send(sock, data.data, (size_t)data.len, 0);
    if (n < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_send: send failed"); return result; }
    result.is_ok = 1; result.value.ok = n; return result;
}

osc_str osc_socket_recv(osc_arena *arena, int32_t sock, int32_t max_len)
{
    osc_str result;
    char *buf;
    ssize_t n;
    if (max_len <= 0 || max_len > 65536) max_len = 4096;
    buf = (char *)osc_arena_alloc(arena, (size_t)max_len);
    if (!buf) { result.data = ""; result.len = 0; return result; }
    n = recv(sock, buf, (size_t)max_len, 0);
    if (n <= 0) { result.data = ""; result.len = 0; return result; }
    result.data = buf;
    result.len = (int32_t)n;
    return result;
}

void osc_socket_close(int32_t sock)
{
    close(sock);
}

osc_result_i32_str osc_socket_udp(void)
{
    osc_result_i32_str result;
    int32_t fd = (int32_t)socket(AF_INET, SOCK_DGRAM, 0);
    if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_udp: cannot create socket"); return result; }
    result.is_ok = 1; result.value.ok = fd; return result;
}

int32_t osc_socket_sendto(int32_t sock, osc_str data, osc_str addr, int32_t port)
{
    struct sockaddr_in sa;
    if (osc_socket_lookup_ipv4(addr, port, &sa) < 0) return -1;
    return (int32_t)sendto(sock, data.data, (size_t)data.len, 0,
                           (struct sockaddr *)&sa, sizeof(sa));
}

osc_str osc_socket_recvfrom(osc_arena *arena, int32_t sock, int32_t max_len)
{
    osc_str result;
    char *buf;
    ssize_t n;
    struct sockaddr_in sa;
    socklen_t sa_len = sizeof(sa);
    if (max_len <= 0 || max_len > 65536) max_len = 4096;
    buf = (char *)osc_arena_alloc(arena, (size_t)max_len);
    if (!buf) { result.data = ""; result.len = 0; return result; }
    n = recvfrom(sock, buf, (size_t)max_len, 0,
                 (struct sockaddr *)&sa, &sa_len);
    if (n <= 0) { result.data = ""; result.len = 0; return result; }
    result.data = buf;
    result.len = (int32_t)n;
    return result;
}

osc_result_i32_str osc_socket_unix_connect(osc_str path)
{
    osc_result_i32_str result;
    char buf[256];
    struct sockaddr_un sa;
    osc_str_to_cstr_buf(path, buf, sizeof(buf));
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_unix_connect: cannot create socket"); return result; }
    memset(&sa, 0, sizeof(sa));
    sa.sun_family = AF_UNIX;
    strncpy(sa.sun_path, buf, sizeof(sa.sun_path) - 1);
    if (connect(fd, (struct sockaddr *)&sa, sizeof(sa)) != 0) { close(fd); result.is_ok = 0; result.value.err = osc_str_from_cstr("socket_unix_connect: connection failed"); return result; }
    result.is_ok = 1; result.value.ok = (int32_t)fd; return result;
}

osc_result_i32_str osc_tls_connect(osc_str host, int32_t port)
{
    (void)host; (void)port;
    osc_result_i32_str result;
    result.is_ok = 0; result.value.err = osc_str_from_cstr("tls_connect: not supported in libc mode");
    return result;
}

osc_result_i32_str osc_tls_send(int32_t handle, osc_str data)
{
    (void)handle; (void)data;
    osc_result_i32_str result;
    result.is_ok = 0; result.value.err = osc_str_from_cstr("tls_send: not supported in libc mode");
    return result;
}

osc_str osc_tls_recv(osc_arena *arena, int32_t handle, int32_t max_len)
{
    (void)arena; (void)handle; (void)max_len;
    osc_str result; result.data = ""; result.len = 0; return result;
}

int32_t osc_tls_recv_byte(int32_t handle)
{
    (void)handle; return -1;
}

void osc_tls_close(int32_t handle) { (void)handle; }
void osc_tls_cleanup(void) {}

#endif /* OSC_HAS_SOCKETS / _WIN32 / POSIX */

/* ================================================================== */
/*  Image decoding wrappers (requires l_img.h)                         */
/* ================================================================== */

#ifdef OSC_HAS_IMG

osc_result_arr_i32_str osc_img_load(osc_arena *arena, osc_str data)
{
    osc_result_arr_i32_str result;
    int w, h;
    uint32_t *pixels = l_img_load_mem((const unsigned char *)data.data, data.len, &w, &h);
    if (!pixels) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("img_load: failed to decode image");
        return result;
    }
    if (w <= 0 || h <= 0 || (int64_t)w * h > 0x7FFFFFFE) {
        l_img_free_pixels(pixels, w, h);
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("img_load: invalid image dimensions");
        return result;
    }
    int total = w * h + 2;
    osc_array *arr = osc_array_new(arena, (int32_t)sizeof(int32_t), total);
    int32_t *d = (int32_t *)arr->data;
    d[0] = (int32_t)w;
    d[1] = (int32_t)h;
    for (int i = 0; i < w * h; i++) d[i + 2] = (int32_t)pixels[i];
    arr->len = total;
    l_img_free_pixels(pixels, w, h);
    result.is_ok = 1;
    result.value.ok = arr;
    return result;
}

#else /* no image support */

osc_result_arr_i32_str osc_img_load(osc_arena *arena, osc_str data)
{
    (void)arena; (void)data;
    osc_result_arr_i32_str result;
    result.is_ok = 0;
    result.value.err = osc_str_from_cstr("img_load: not supported in this build");
    return result;
}

#endif /* OSC_HAS_IMG */
