/*
 * test_runtime.c — Oscan Runtime Test Suite
 *
 * Runs assert-based tests covering arena, checked arithmetic, arrays,
 * strings, type casts, and conversions. Panics are tested via fork/exec
 * where supported; on Windows we skip panic-specific tests.
 */

#include "osc_runtime.h"

#include <assert.h>
#include <inttypes.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <process.h>
#include <windows.h>
#else
#include <sys/wait.h>
#include <unistd.h>
#endif

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

static int tests_run    = 0;
static int tests_passed = 0;

#define TEST(name)                                          \
    do {                                                    \
        printf("  %-50s", name);                            \
        tests_run++;                                        \
    } while (0)

#define PASS()                                              \
    do {                                                    \
        tests_passed++;                                     \
        printf("PASS\n");                                   \
    } while (0)

/*
 * EXPECT_PANIC: run a block of code and verify it exits with a
 * non-zero status (i.e., calls OSC_PANIC).
 * On POSIX we fork; on Windows we skip the test.
 */
#ifdef _WIN32
#define EXPECT_PANIC(code_block)                            \
    do {                                                    \
        printf("SKIP (no fork on Windows)\n");              \
        tests_passed++;                                     \
    } while (0)
#else
#define EXPECT_PANIC(code_block)                            \
    do {                                                    \
        fflush(stdout);                                     \
        fflush(stderr);                                     \
        pid_t pid = fork();                                 \
        if (pid == 0) {                                     \
            /* child — redirect stderr to /dev/null */      \
            freopen("/dev/null", "w", stderr);              \
            code_block;                                     \
            _exit(0); /* should NOT reach here */           \
        } else {                                            \
            int status;                                     \
            waitpid(pid, &status, 0);                       \
            assert(WIFEXITED(status));                      \
            assert(WEXITSTATUS(status) != 0);               \
            tests_passed++;                                 \
            printf("PASS\n");                               \
        }                                                   \
    } while (0)
#endif

/* ================================================================== */
/*  Arena tests                                                        */
/* ================================================================== */

static void test_arena(void)
{
    printf("\n[Arena]\n");

    TEST("create with default capacity");
    {
        osc_arena *a = osc_arena_create(0);
        assert(a != NULL);
        assert(a->head != NULL);
        assert(a->head->capacity == OSC_ARENA_DEFAULT_CAPACITY);
        assert(a->head->used == 0);
        assert(a->current == a->head);
        osc_arena_destroy(a);
    }
    PASS();

    TEST("alloc within capacity");
    {
        osc_arena *a = osc_arena_create(256);
        void *p1 = osc_arena_alloc(a, 64);
        void *p2 = osc_arena_alloc(a, 64);
        assert(p1 != NULL);
        assert(p2 != NULL);
        assert(p1 != p2);
        assert(a->head->used <= a->head->capacity);
        /* No new block should have been allocated */
        assert(a->head->next == NULL);
        osc_arena_destroy(a);
    }
    PASS();

    TEST("alloc beyond capacity (growth via new block)");
    {
        osc_arena *a = osc_arena_create(32);
        /* Force new block: allocate more than first block capacity */
        void *p1 = osc_arena_alloc(a, 16);
        void *p2 = osc_arena_alloc(a, 128);
        assert(p1 != NULL);
        assert(p2 != NULL);
        /* A second block must have been created */
        assert(a->head->next != NULL);
        assert(a->current == a->head->next);
        osc_arena_destroy(a);
    }
    PASS();

    TEST("alloc beyond capacity — old pointers remain valid");
    {
        osc_arena *a = osc_arena_create(64);
        int32_t *p1 = (int32_t *)osc_arena_alloc(a, sizeof(int32_t));
        *p1 = 0xDEAD;
        /* Fill up the first block */
        osc_arena_alloc(a, 48);
        /* This must spill into a new block */
        int32_t *p2 = (int32_t *)osc_arena_alloc(a, sizeof(int32_t));
        *p2 = 0xBEEF;
        /* p1 must still be readable from the old (unmoved) block */
        assert(*p1 == 0xDEAD);
        assert(*p2 == 0xBEEF);
        osc_arena_destroy(a);
    }
    PASS();

    TEST("reset");
    {
        osc_arena *a = osc_arena_create(256);
        osc_arena_alloc(a, 100);
        assert(a->head->used > 0);
        osc_arena_reset(a);
        assert(a->head->used == 0);
        assert(a->current == a->head);
        osc_arena_destroy(a);
    }
    PASS();

    TEST("reset with multiple blocks");
    {
        osc_arena *a = osc_arena_create(32);
        osc_arena_alloc(a, 16);
        osc_arena_alloc(a, 64); /* forces new block */
        assert(a->head->next != NULL);
        osc_arena_reset(a);
        assert(a->head->used == 0);
        assert(a->head->next->used == 0);
        assert(a->current == a->head);
        /* Blocks are kept for reuse */
        assert(a->head->next != NULL);
        osc_arena_destroy(a);
    }
    PASS();

    TEST("multiple allocs alignment");
    {
        osc_arena *a = osc_arena_create(1024);
        void *p1 = osc_arena_alloc(a, 1);
        void *p2 = osc_arena_alloc(a, 1);
        /* Each alloc should be 8-byte aligned */
        assert(((size_t)p1 & 7) == 0 || (size_t)p2 - (size_t)p1 >= 8);
        (void)p1; (void)p2;
        osc_arena_destroy(a);
    }
    PASS();
}

/* ================================================================== */
/*  Checked arithmetic tests — i32                                     */
/* ================================================================== */

static void test_checked_arith_i32(void)
{
    printf("\n[Checked Arithmetic i32]\n");

    TEST("add normal");
    assert(osc_add_i32(10, 20) == 30);
    assert(osc_add_i32(-5, 5) == 0);
    assert(osc_add_i32(0, 0) == 0);
    PASS();

    TEST("add overflow");
    EXPECT_PANIC({ osc_add_i32(INT32_MAX, 1); });

    TEST("add underflow");
    EXPECT_PANIC({ osc_add_i32(INT32_MIN, -1); });

    TEST("sub normal");
    assert(osc_sub_i32(30, 10) == 20);
    assert(osc_sub_i32(0, 0) == 0);
    assert(osc_sub_i32(-5, -3) == -2);
    PASS();

    TEST("sub overflow");
    EXPECT_PANIC({ osc_sub_i32(INT32_MIN, 1); });

    TEST("sub underflow");
    EXPECT_PANIC({ osc_sub_i32(INT32_MAX, -1); });

    TEST("mul normal");
    assert(osc_mul_i32(6, 7) == 42);
    assert(osc_mul_i32(-3, 4) == -12);
    assert(osc_mul_i32(0, INT32_MAX) == 0);
    PASS();

    TEST("mul overflow");
    EXPECT_PANIC({ osc_mul_i32(INT32_MAX, 2); });

    TEST("div normal");
    assert(osc_div_i32(42, 6) == 7);
    assert(osc_div_i32(-10, 3) == -3);
    PASS();

    TEST("div by zero");
    EXPECT_PANIC({ osc_div_i32(1, 0); });

    TEST("div MIN / -1");
    EXPECT_PANIC({ osc_div_i32(INT32_MIN, -1); });

    TEST("mod normal");
    assert(osc_mod_i32(10, 3) == 1);
    assert(osc_mod_i32(-10, 3) == -1);
    PASS();

    TEST("mod by zero");
    EXPECT_PANIC({ osc_mod_i32(1, 0); });

    TEST("neg normal");
    assert(osc_neg_i32(5) == -5);
    assert(osc_neg_i32(-5) == 5);
    assert(osc_neg_i32(0) == 0);
    PASS();

    TEST("neg MIN_VALUE");
    EXPECT_PANIC({ osc_neg_i32(INT32_MIN); });
}

/* ================================================================== */
/*  Checked arithmetic tests — i64                                     */
/* ================================================================== */

static void test_checked_arith_i64(void)
{
    printf("\n[Checked Arithmetic i64]\n");

    TEST("add normal");
    assert(osc_add_i64(100LL, 200LL) == 300LL);
    assert(osc_add_i64(-50LL, 50LL) == 0LL);
    PASS();

    TEST("add overflow");
    EXPECT_PANIC({ osc_add_i64(INT64_MAX, 1); });

    TEST("sub normal");
    assert(osc_sub_i64(300LL, 100LL) == 200LL);
    PASS();

    TEST("sub overflow");
    EXPECT_PANIC({ osc_sub_i64(INT64_MIN, 1); });

    TEST("mul normal");
    assert(osc_mul_i64(6LL, 7LL) == 42LL);
    assert(osc_mul_i64(-3LL, 4LL) == -12LL);
    assert(osc_mul_i64(0LL, INT64_MAX) == 0LL);
    PASS();

    TEST("mul overflow");
    EXPECT_PANIC({ osc_mul_i64(INT64_MAX, 2); });

    TEST("div normal");
    assert(osc_div_i64(42LL, 6LL) == 7LL);
    PASS();

    TEST("div by zero");
    EXPECT_PANIC({ osc_div_i64(1, 0); });

    TEST("div MIN / -1");
    EXPECT_PANIC({ osc_div_i64(INT64_MIN, -1); });

    TEST("mod normal");
    assert(osc_mod_i64(10LL, 3LL) == 1LL);
    PASS();

    TEST("mod by zero");
    EXPECT_PANIC({ osc_mod_i64(1, 0); });

    TEST("neg normal");
    assert(osc_neg_i64(5LL) == -5LL);
    assert(osc_neg_i64(0LL) == 0LL);
    PASS();

    TEST("neg MIN_VALUE");
    EXPECT_PANIC({ osc_neg_i64(INT64_MIN); });
}

/* ================================================================== */
/*  Array tests                                                        */
/* ================================================================== */

static osc_array *array_from_values(osc_arena *arena, int32_t elem_size,
                                    const void *values, int32_t count)
{
    osc_array *arr = osc_array_new(arena, elem_size, count);
    const uint8_t *bytes = (const uint8_t *)values;
    int32_t i;
    for (i = 0; i < count; i++) {
        osc_array_push(arena, arr, (void *)(bytes + (size_t)i * (size_t)elem_size));
    }
    return arr;
}

static int arena_owns_pointer(osc_arena *arena, const void *pointer)
{
    osc_arena_block *block = arena->head;
    uintptr_t address = (uintptr_t)pointer;
    while (block) {
        uintptr_t start = (uintptr_t)block->data;
        uintptr_t end = start + block->capacity;
        if (address >= start && address < end) return 1;
        block = block->next;
    }
    return 0;
}

static void test_array(void)
{
    printf("\n[Array]\n");

    osc_arena *arena = osc_arena_create(4096);

    TEST("create and push");
    {
        osc_array *arr = osc_array_new(arena, (int32_t)sizeof(int32_t), 4);
        assert(osc_array_len(arr) == 0);
        int32_t v = 42;
        osc_array_push(arena, arr, &v);
        assert(osc_array_len(arr) == 1);
        assert(*(int32_t *)osc_array_get(arr, 0) == 42);
    }
    PASS();

    TEST("set and get");
    {
        osc_array *arr = osc_array_new(arena, (int32_t)sizeof(int32_t), 4);
        int32_t values[] = {10, 20, 30};
        int i;
        for (i = 0; i < 3; i++) {
            osc_array_push(arena, arr, &values[i]);
        }
        int32_t new_val = 99;
        osc_array_set(arr, 1, &new_val);
        assert(*(int32_t *)osc_array_get(arr, 0) == 10);
        assert(*(int32_t *)osc_array_get(arr, 1) == 99);
        assert(*(int32_t *)osc_array_get(arr, 2) == 30);
    }
    PASS();

    TEST("push beyond capacity (growth)");
    {
        osc_array *arr = osc_array_new(arena, (int32_t)sizeof(int32_t), 2);
        int32_t i;
        for (i = 0; i < 100; i++) {
            osc_array_push(arena, arr, &i);
        }
        assert(osc_array_len(arr) == 100);
        for (i = 0; i < 100; i++) {
            assert(*(int32_t *)osc_array_get(arr, i) == i);
        }
    }
    PASS();

    TEST("growth retains the array's owner arena");
    {
        osc_arena *owner = osc_arena_create(128);
        osc_arena *child = osc_arena_create(128);
        osc_array *arr = osc_array_new(owner, (int32_t)sizeof(int32_t), 1);
        int32_t first = 1;
        int32_t second = 2;
        osc_array_push(owner, arr, &first);
        osc_array_push(child, arr, &second);
        assert(arr->owner == owner);
        assert(arena_owns_pointer(owner, arr->data));
        assert(!arena_owns_pointer(child, arr->data));
        osc_arena_destroy(child);
        assert(*(int32_t *)osc_array_get(arr, 0) == 1);
        assert(*(int32_t *)osc_array_get(arr, 1) == 2);
        osc_arena_destroy(owner);
    }
    PASS();

    TEST("arena-backed elements cannot enter an outer array");
    {
        osc_arena *owner = osc_arena_create(128);
        osc_arena *child = osc_arena_create(128);
        osc_array *outer = osc_array_new(
            owner, (int32_t)sizeof(osc_array *), 1);
        (void)child;
        (void)outer;
        EXPECT_PANIC({
            osc_array_check_arena_store(child, outer);
        });
        osc_arena_destroy(child);
        osc_arena_destroy(owner);
    }

    if (sizeof(size_t) == 4) {
        TEST("new rejects allocation-size overflow");
        EXPECT_PANIC({
            osc_array_new(
                arena, INT32_MAX, INT32_MAX);
        });

        TEST("new rejects alignment-size overflow");
        EXPECT_PANIC({
            osc_array_new(
                arena, 2, INT32_MAX);
        });
    }

    TEST("get out of bounds (negative)");
    {
        osc_array *arr = osc_array_new(arena, (int32_t)sizeof(int32_t), 4);
        int32_t v = 1;
        osc_array_push(arena, arr, &v);
        EXPECT_PANIC({ osc_array_get(arr, -1); });
    }

    TEST("get out of bounds (too large)");
    {
        osc_array *arr = osc_array_new(arena, (int32_t)sizeof(int32_t), 4);
        int32_t v = 1;
        osc_array_push(arena, arr, &v);
        EXPECT_PANIC({ osc_array_get(arr, 1); });
    }

    TEST("set out of bounds");
    {
        osc_array *arr = osc_array_new(arena, (int32_t)sizeof(int32_t), 4);
        int32_t v = 1;
        osc_array_push(arena, arr, &v);
        int32_t nv = 99;
        (void)nv;
        EXPECT_PANIC({ osc_array_set(arr, 5, &nv); });
    }

    osc_arena_destroy(arena);
}

static void test_array_collections(void)
{
    printf("\n[Array Collections]\n");

    osc_arena *arena = osc_arena_create(16384);

    TEST("clone, repeat, reverse, fill, swap, and slice");
    {
        int32_t values[] = {1, 2, 3};
        int32_t fill = 8;
        int32_t repeated_value = 4;
        osc_array *arr = array_from_values(
            arena, (int32_t)sizeof(int32_t), values, 3);
        osc_array *clone = osc_array_clone(arena, arr);
        osc_array *repeated = osc_array_repeat(
            arena, (int32_t)sizeof(int32_t), &repeated_value, 3);
        osc_array *slice;

        osc_array_swap(clone, 0, 2);
        assert(*(int32_t *)osc_array_get(clone, 0) == 3);
        assert(*(int32_t *)osc_array_get(arr, 0) == 1);
        osc_array_reverse(clone);
        assert(*(int32_t *)osc_array_get(clone, 0) == 1);
        osc_array_fill(clone, &fill);
        assert(*(int32_t *)osc_array_get(clone, 0) == 8);
        assert(*(int32_t *)osc_array_get(clone, 2) == 8);
        assert(osc_array_len(repeated) == 3);
        assert(*(int32_t *)osc_array_get(repeated, 2) == 4);

        slice = osc_array_slice(arena, arr, 1, 3);
        assert(osc_array_len(slice) == 2);
        assert(*(int32_t *)osc_array_get(slice, 0) == 2);
        assert(*(int32_t *)osc_array_get(slice, 1) == 3);
    }
    PASS();

    TEST("insert aliases, remove, self-extend, and clear");
    {
        int32_t values[] = {1, 2, 3};
        int32_t removed = 0;
        osc_array *arr = array_from_values(
            arena, (int32_t)sizeof(int32_t), values, 3);
        const void *aliased_value = osc_array_get(arr, 1);

        osc_array_insert(arena, arr, 0, aliased_value);
        assert(osc_array_len(arr) == 4);
        assert(*(int32_t *)osc_array_get(arr, 0) == 2);
        assert(*(int32_t *)osc_array_get(arr, 1) == 1);
        osc_array_remove_at(arr, 2, &removed);
        assert(removed == 2);
        assert(osc_array_len(arr) == 3);
        assert(*(int32_t *)osc_array_get(arr, 2) == 3);

        osc_array_extend(arena, arr, arr);
        assert(osc_array_len(arr) == 6);
        assert(*(int32_t *)osc_array_get(arr, 0) == 2);
        assert(*(int32_t *)osc_array_get(arr, 3) == 2);
        assert(*(int32_t *)osc_array_get(arr, 5) == 3);
        osc_array_clear(arr);
        assert(osc_array_len(arr) == 0);
    }
    PASS();

    TEST("collection bounds checks");
    {
        int32_t value = 1;
        osc_array *arr = osc_array_repeat(
            arena, (int32_t)sizeof(int32_t), &value, 1);
        (void)arr;
        EXPECT_PANIC({ osc_array_swap(arr, 0, 1); });
    }

    TEST("negative repeat count");
    {
        int32_t value = 1;
        (void)value;
        EXPECT_PANIC({
            osc_array_repeat(
                arena, (int32_t)sizeof(int32_t), &value, -1);
        });
    }

    TEST("invalid slice range");
    {
        int32_t value = 1;
        osc_array *arr = osc_array_repeat(
            arena, (int32_t)sizeof(int32_t), &value, 1);
        (void)arr;
        EXPECT_PANIC({ osc_array_slice(arena, arr, 1, 0); });
    }

    TEST("typed i32 search, comparison, and sort");
    {
        int32_t values[] = {3, 1, 2, 1};
        int32_t other_values[] = {3, 1, 2, 2};
        int32_t one = 1;
        int32_t absent = 9;
        osc_array *arr = array_from_values(
            arena, (int32_t)sizeof(int32_t), values, 4);
        osc_array *other = array_from_values(
            arena, (int32_t)sizeof(int32_t), other_values, 4);

        assert(osc_array_contains_i32(arr, &one));
        assert(!osc_array_contains_i32(arr, &absent));
        assert(osc_array_index_of_i32(arr, &one) == 1);
        assert(osc_array_last_index_of_i32(arr, &one) == 3);
        assert(osc_array_count_i32(arr, &one) == 2);
        assert(osc_array_compare_i32(arr, other) == -1);
        osc_array_sort_i32(arr);
        assert(*(int32_t *)osc_array_get(arr, 0) == 1);
        assert(*(int32_t *)osc_array_get(arr, 1) == 1);
        assert(*(int32_t *)osc_array_get(arr, 3) == 3);
    }
    PASS();

    TEST("typed bool, i64, and str operations");
    {
        uint8_t bool_values[] = {1, 0, 1};
        uint8_t true_value = 1;
        int64_t i64_values[] = {5, -1, -1};
        int64_t minus_one = -1;
        osc_str str_values[] = {
            osc_str_from_cstr("beta"),
            osc_str_from_cstr("alpha"),
            osc_str_from_cstr("beta")
        };
        osc_str beta = osc_str_from_cstr("beta");
        osc_array *bools = array_from_values(
            arena, (int32_t)sizeof(uint8_t), bool_values, 3);
        osc_array *i64s = array_from_values(
            arena, (int32_t)sizeof(int64_t), i64_values, 3);
        osc_array *strs = array_from_values(
            arena, (int32_t)sizeof(osc_str), str_values, 3);

        assert(osc_array_count_bool(bools, &true_value) == 2);
        osc_array_sort_bool(bools);
        assert(*(uint8_t *)osc_array_get(bools, 0) == 0);
        assert(osc_array_index_of_i64(i64s, &minus_one) == 1);
        assert(osc_array_last_index_of_i64(i64s, &minus_one) == 2);
        osc_array_sort_i64(i64s);
        assert(*(int64_t *)osc_array_get(i64s, 0) == -1);
        assert(osc_array_count_str(strs, &beta) == 2);
        osc_array_sort_str(strs);
        {
            osc_str first = *(osc_str *)osc_array_get(strs, 0);
            assert(first.len == 5);
            assert(memcmp(first.data, "alpha", 5) == 0);
        }
    }
    PASS();

    TEST("f64 NaNs and signed zeros use one total relation");
    {
        double values[] = {NAN, -0.0, 2.0, 0.0, NAN, -1.0};
        double nan_value = NAN;
        double zero = 0.0;
        double zero_values[] = {0.0};
        double neg_zero_values[] = {-0.0};
        double nan_values[] = {NAN};
        osc_array *arr = array_from_values(
            arena, (int32_t)sizeof(double), values, 6);
        osc_array *zeros = array_from_values(
            arena, (int32_t)sizeof(double), zero_values, 1);
        osc_array *neg_zeros = array_from_values(
            arena, (int32_t)sizeof(double), neg_zero_values, 1);
        osc_array *nans = array_from_values(
            arena, (int32_t)sizeof(double), nan_values, 1);

        assert(osc_array_index_of_f64(arr, &nan_value) == 0);
        assert(osc_array_last_index_of_f64(arr, &nan_value) == 4);
        assert(osc_array_count_f64(arr, &nan_value) == 2);
        assert(osc_array_count_f64(arr, &zero) == 2);
        assert(osc_array_compare_f64(zeros, neg_zeros) == 0);
        assert(osc_array_compare_f64(zeros, nans) == -1);
        assert(osc_array_compare_f64(nans, nans) == 0);

        osc_array_sort_f64(arr);
        assert(*(double *)osc_array_get(arr, 0) == -1.0);
        assert(*(double *)osc_array_get(arr, 1) == 0.0);
        assert(*(double *)osc_array_get(arr, 2) == 0.0);
        assert(*(double *)osc_array_get(arr, 3) == 2.0);
        assert(isnan(*(double *)osc_array_get(arr, 4)));
        assert(isnan(*(double *)osc_array_get(arr, 5)));
    }
    PASS();

    osc_arena_destroy(arena);
}

/* ================================================================== */
/*  String tests                                                       */
/* ================================================================== */

static void test_strings(void)
{
    printf("\n[Strings]\n");

    osc_arena *arena = osc_arena_create(4096);

    TEST("from_cstr");
    {
        osc_str s = osc_str_from_cstr("hello");
        assert(s.len == 5);
        assert(memcmp(s.data, "hello", 5) == 0);
    }
    PASS();

    TEST("from_cstr NULL");
    {
        osc_str s = osc_str_from_cstr(NULL);
        assert(s.len == 0);
    }
    PASS();

    TEST("len");
    {
        osc_str s = osc_str_from_cstr("abc");
        assert(osc_str_len(s) == 3);
    }
    PASS();

    TEST("eq (equal)");
    {
        osc_str a = osc_str_from_cstr("foo");
        osc_str b = osc_str_from_cstr("foo");
        assert(osc_str_eq(a, b) == 1);
    }
    PASS();

    TEST("eq (not equal)");
    {
        osc_str a = osc_str_from_cstr("foo");
        osc_str b = osc_str_from_cstr("bar");
        assert(osc_str_eq(a, b) == 0);
    }
    PASS();

    TEST("eq (different lengths)");
    {
        osc_str a = osc_str_from_cstr("foo");
        osc_str b = osc_str_from_cstr("foobar");
        assert(osc_str_eq(a, b) == 0);
    }
    PASS();

    TEST("concat");
    {
        osc_str a = osc_str_from_cstr("hello");
        osc_str b = osc_str_from_cstr(" world");
        osc_str c = osc_str_concat(arena, a, b);
        assert(c.len == 11);
        assert(memcmp(c.data, "hello world", 11) == 0);
    }
    PASS();

    TEST("concat with empty");
    {
        osc_str a = osc_str_from_cstr("hello");
        osc_str b = osc_str_from_cstr("");
        osc_str c = osc_str_concat(arena, a, b);
        assert(c.len == 5);
        assert(memcmp(c.data, "hello", 5) == 0);
    }
    PASS();

    TEST("to_cstr (null-terminated)");
    {
        osc_str s = osc_str_from_cstr("test");
        osc_str c = osc_str_to_cstr(arena, s);
        assert(c.len == 4);
        assert(c.data[4] == '\0');
        assert(strcmp(c.data, "test") == 0);
    }
    PASS();

    TEST("str_from_i32 aliases i32_to_str");
    {
        osc_str a = osc_str_from_i32(arena, -2048);
        osc_str b = osc_i32_to_str(arena, -2048);
        assert(osc_str_eq(a, b) == 1);
    }
    PASS();

    osc_arena_destroy(arena);
}

/* ================================================================== */
/*  Type cast tests                                                    */
/* ================================================================== */

static void test_type_casts(void)
{
    printf("\n[Type Casts]\n");

    TEST("i32 to i64 (safe widening)");
    assert(osc_i32_to_i64(42) == 42LL);
    assert(osc_i32_to_i64(INT32_MIN) == (int64_t)INT32_MIN);
    assert(osc_i32_to_i64(INT32_MAX) == (int64_t)INT32_MAX);
    PASS();

    TEST("i64 to i32 (valid narrow)");
    assert(osc_i64_to_i32(42LL) == 42);
    assert(osc_i64_to_i32(-100LL) == -100);
    PASS();

    TEST("i64 to i32 (overflow)");
    EXPECT_PANIC({ osc_i64_to_i32((int64_t)INT32_MAX + 1); });

    TEST("i64 to i32 (underflow)");
    EXPECT_PANIC({ osc_i64_to_i32((int64_t)INT32_MIN - 1); });

    TEST("i32 to f64 (safe)");
    assert(osc_i32_to_f64(42) == 42.0);
    assert(osc_i32_to_f64(-1) == -1.0);
    PASS();

    TEST("i64 to f64 (may lose precision)");
    assert(osc_i64_to_f64(42LL) == 42.0);
    PASS();

    TEST("f64 to i32 (valid)");
    assert(osc_f64_to_i32(42.0) == 42);
    assert(osc_f64_to_i32(-100.9) == -100);
    PASS();

    TEST("f64 to i32 (NaN)");
    {
        volatile double zero = 0.0;
        double nan_val = zero / zero;
        (void)nan_val;
        EXPECT_PANIC({ osc_f64_to_i32(nan_val); });
    }

    TEST("f64 to i32 (out of range)");
    EXPECT_PANIC({ osc_f64_to_i32(3.0e10); });

    TEST("f64 to i64 (valid)");
    assert(osc_f64_to_i64(42.0) == 42LL);
    assert(osc_f64_to_i64(-100.5) == -100LL);
    PASS();

    TEST("f64 to i64 (NaN)");
    {
        volatile double zero = 0.0;
        double nan_val = zero / zero;
        (void)nan_val;
        EXPECT_PANIC({ osc_f64_to_i64(nan_val); });
    }

    TEST("f64 to i64 (Inf)");
    {
        volatile double zero = 0.0;
        double inf_val = 1.0 / zero;
        (void)inf_val;
        EXPECT_PANIC({ osc_f64_to_i64(inf_val); });
    }
}

/* ================================================================== */
/*  Conversion tests                                                   */
/* ================================================================== */

static void test_conversions(void)
{
    printf("\n[Conversions]\n");

    osc_arena *arena = osc_arena_create(4096);

    TEST("i32_to_str positive");
    {
        osc_str s = osc_i32_to_str(arena, 42);
        assert(s.len == 2);
        assert(memcmp(s.data, "42", 2) == 0);
    }
    PASS();

    TEST("i32_to_str negative");
    {
        osc_str s = osc_i32_to_str(arena, -123);
        assert(s.len == 4);
        assert(memcmp(s.data, "-123", 4) == 0);
    }
    PASS();

    TEST("i32_to_str zero");
    {
        osc_str s = osc_i32_to_str(arena, 0);
        assert(s.len == 1);
        assert(s.data[0] == '0');
    }
    PASS();

    TEST("i32_to_str MIN");
    {
        osc_str s = osc_i32_to_str(arena, INT32_MIN);
        assert(s.len == 11); /* -2147483648 */
        assert(memcmp(s.data, "-2147483648", 11) == 0);
    }
    PASS();

    TEST("str_from_i64 positive");
    {
        osc_str s = osc_str_from_i64(arena, 9876543210LL);
        assert(s.len == 10);
        assert(memcmp(s.data, "9876543210", 10) == 0);
    }
    PASS();

    TEST("str_from_f64 trims trailing zeros");
    {
        osc_str s = osc_str_from_f64(arena, 12.340000);
        assert(s.len == 5);
        assert(memcmp(s.data, "12.34", 5) == 0);
    }
    PASS();

    TEST("str_from_bool true/false");
    {
        assert(osc_str_eq(osc_str_from_bool(1), osc_str_from_cstr("true")) == 1);
        assert(osc_str_eq(osc_str_from_bool(0), osc_str_from_cstr("false")) == 1);
    }
    PASS();

    osc_arena_destroy(arena);
}

/* ================================================================== */
/*  Math helpers tests                                                 */
/* ================================================================== */

static void test_math(void)
{
    printf("\n[Math]\n");

    TEST("abs_i32 positive");
    assert(osc_abs_i32(5) == 5);
    PASS();

    TEST("abs_i32 negative");
    assert(osc_abs_i32(-5) == 5);
    PASS();

    TEST("abs_i32 zero");
    assert(osc_abs_i32(0) == 0);
    PASS();

    TEST("abs_i32 MIN_VALUE");
    EXPECT_PANIC({ osc_abs_i32(INT32_MIN); });

    TEST("abs_f64");
    assert(osc_abs_f64(-3.14) == 3.14);
    assert(osc_abs_f64(2.71) == 2.71);
    assert(osc_abs_f64(0.0) == 0.0);
    PASS();
}

/* ================================================================== */
/*  I/O tests (basic smoke tests)                                      */
/* ================================================================== */

static void test_io(void)
{
    printf("\n[I/O]\n");

    TEST("print and println (smoke)");
    {
        osc_str s = osc_str_from_cstr("hello");
        osc_print(s);
        printf(" ");
        osc_println(s);
    }
    PASS();

    TEST("print_i32");
    osc_print_i32(42);
    printf("\n");
    PASS();

    TEST("print_i64");
    osc_print_i64(123456789LL);
    printf("\n");
    PASS();

    TEST("print_f64");
    osc_print_f64(3.14);
    printf("\n");
    PASS();

    TEST("print_bool");
    osc_print_bool(1);
    printf(" ");
    osc_print_bool(0);
    printf("\n");
    PASS();
}

/* ================================================================== */
/*  Result type tests                                                  */
/* ================================================================== */

static void test_result(void)
{
    printf("\n[Result]\n");

    TEST("construct Ok");
    {
        osc_result_str_str r;
        r.is_ok = 1;
        r.value.ok = osc_str_from_cstr("success");
        assert(r.is_ok == 1);
        assert(osc_str_eq(r.value.ok, osc_str_from_cstr("success")));
    }
    PASS();

    TEST("construct Err");
    {
        osc_result_str_str r;
        r.is_ok = 0;
        r.value.err = osc_str_from_cstr("failure");
        assert(r.is_ok == 0);
        assert(osc_str_eq(r.value.err, osc_str_from_cstr("failure")));
    }
    PASS();
}

/* ================================================================== */
/*  Main                                                               */
/* ================================================================== */

int main(void)
{
    printf("=== Oscan Runtime Test Suite ===\n");

    test_arena();
    test_checked_arith_i32();
    test_checked_arith_i64();
    test_array();
    test_array_collections();
    test_strings();
    test_type_casts();
    test_conversions();
    test_math();
    test_io();
    test_result();

    printf("\n=== Results: %d/%d passed ===\n", tests_passed, tests_run);
    if (tests_passed == tests_run) {
        printf("All tests passed.\n");
        return 0;
    } else {
        printf("SOME TESTS FAILED.\n");
        return 1;
    }
}
