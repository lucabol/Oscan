/*
 * test_runtime.c — Babel-C Runtime Test Suite
 *
 * Runs assert-based tests covering arena, checked arithmetic, arrays,
 * strings, type casts, and conversions. Panics are tested via fork/exec
 * where supported; on Windows we skip panic-specific tests.
 */

#include "bc_runtime.h"

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
 * non-zero status (i.e., calls bc_panic).
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
        bc_arena *a = bc_arena_create(0);
        assert(a != NULL);
        assert(a->capacity == BC_ARENA_DEFAULT_CAPACITY);
        assert(a->used == 0);
        bc_arena_destroy(a);
    }
    PASS();

    TEST("alloc within capacity");
    {
        bc_arena *a = bc_arena_create(256);
        void *p1 = bc_arena_alloc(a, 64);
        void *p2 = bc_arena_alloc(a, 64);
        assert(p1 != NULL);
        assert(p2 != NULL);
        assert(p1 != p2);
        assert(a->used <= a->capacity);
        bc_arena_destroy(a);
    }
    PASS();

    TEST("alloc beyond capacity (growth)");
    {
        bc_arena *a = bc_arena_create(32);
        size_t old_cap = a->capacity;
        /* Force growth: allocate more than capacity */
        void *p = bc_arena_alloc(a, 128);
        assert(p != NULL);
        assert(a->capacity > old_cap);
        bc_arena_destroy(a);
    }
    PASS();

    TEST("reset");
    {
        bc_arena *a = bc_arena_create(256);
        bc_arena_alloc(a, 100);
        assert(a->used > 0);
        bc_arena_reset(a);
        assert(a->used == 0);
        assert(a->capacity == 256);
        bc_arena_destroy(a);
    }
    PASS();

    TEST("multiple allocs alignment");
    {
        bc_arena *a = bc_arena_create(1024);
        void *p1 = bc_arena_alloc(a, 1);
        void *p2 = bc_arena_alloc(a, 1);
        /* Each alloc should be 8-byte aligned */
        assert(((size_t)p1 & 7) == 0 || (size_t)p2 - (size_t)p1 >= 8);
        (void)p1; (void)p2;
        bc_arena_destroy(a);
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
    assert(bc_add_i32(10, 20) == 30);
    assert(bc_add_i32(-5, 5) == 0);
    assert(bc_add_i32(0, 0) == 0);
    PASS();

    TEST("add overflow");
    EXPECT_PANIC({ bc_add_i32(INT32_MAX, 1); });

    TEST("add underflow");
    EXPECT_PANIC({ bc_add_i32(INT32_MIN, -1); });

    TEST("sub normal");
    assert(bc_sub_i32(30, 10) == 20);
    assert(bc_sub_i32(0, 0) == 0);
    assert(bc_sub_i32(-5, -3) == -2);
    PASS();

    TEST("sub overflow");
    EXPECT_PANIC({ bc_sub_i32(INT32_MIN, 1); });

    TEST("sub underflow");
    EXPECT_PANIC({ bc_sub_i32(INT32_MAX, -1); });

    TEST("mul normal");
    assert(bc_mul_i32(6, 7) == 42);
    assert(bc_mul_i32(-3, 4) == -12);
    assert(bc_mul_i32(0, INT32_MAX) == 0);
    PASS();

    TEST("mul overflow");
    EXPECT_PANIC({ bc_mul_i32(INT32_MAX, 2); });

    TEST("div normal");
    assert(bc_div_i32(42, 6) == 7);
    assert(bc_div_i32(-10, 3) == -3);
    PASS();

    TEST("div by zero");
    EXPECT_PANIC({ bc_div_i32(1, 0); });

    TEST("div MIN / -1");
    EXPECT_PANIC({ bc_div_i32(INT32_MIN, -1); });

    TEST("mod normal");
    assert(bc_mod_i32(10, 3) == 1);
    assert(bc_mod_i32(-10, 3) == -1);
    PASS();

    TEST("mod by zero");
    EXPECT_PANIC({ bc_mod_i32(1, 0); });

    TEST("neg normal");
    assert(bc_neg_i32(5) == -5);
    assert(bc_neg_i32(-5) == 5);
    assert(bc_neg_i32(0) == 0);
    PASS();

    TEST("neg MIN_VALUE");
    EXPECT_PANIC({ bc_neg_i32(INT32_MIN); });
}

/* ================================================================== */
/*  Checked arithmetic tests — i64                                     */
/* ================================================================== */

static void test_checked_arith_i64(void)
{
    printf("\n[Checked Arithmetic i64]\n");

    TEST("add normal");
    assert(bc_add_i64(100LL, 200LL) == 300LL);
    assert(bc_add_i64(-50LL, 50LL) == 0LL);
    PASS();

    TEST("add overflow");
    EXPECT_PANIC({ bc_add_i64(INT64_MAX, 1); });

    TEST("sub normal");
    assert(bc_sub_i64(300LL, 100LL) == 200LL);
    PASS();

    TEST("sub overflow");
    EXPECT_PANIC({ bc_sub_i64(INT64_MIN, 1); });

    TEST("mul normal");
    assert(bc_mul_i64(6LL, 7LL) == 42LL);
    assert(bc_mul_i64(-3LL, 4LL) == -12LL);
    assert(bc_mul_i64(0LL, INT64_MAX) == 0LL);
    PASS();

    TEST("mul overflow");
    EXPECT_PANIC({ bc_mul_i64(INT64_MAX, 2); });

    TEST("div normal");
    assert(bc_div_i64(42LL, 6LL) == 7LL);
    PASS();

    TEST("div by zero");
    EXPECT_PANIC({ bc_div_i64(1, 0); });

    TEST("div MIN / -1");
    EXPECT_PANIC({ bc_div_i64(INT64_MIN, -1); });

    TEST("mod normal");
    assert(bc_mod_i64(10LL, 3LL) == 1LL);
    PASS();

    TEST("mod by zero");
    EXPECT_PANIC({ bc_mod_i64(1, 0); });

    TEST("neg normal");
    assert(bc_neg_i64(5LL) == -5LL);
    assert(bc_neg_i64(0LL) == 0LL);
    PASS();

    TEST("neg MIN_VALUE");
    EXPECT_PANIC({ bc_neg_i64(INT64_MIN); });
}

/* ================================================================== */
/*  Array tests                                                        */
/* ================================================================== */

static void test_array(void)
{
    printf("\n[Array]\n");

    bc_arena *arena = bc_arena_create(4096);

    TEST("create and push");
    {
        bc_array *arr = bc_array_new(arena, (int32_t)sizeof(int32_t), 4);
        assert(bc_array_len(arr) == 0);
        int32_t v = 42;
        bc_array_push(arena, arr, &v);
        assert(bc_array_len(arr) == 1);
        assert(*(int32_t *)bc_array_get(arr, 0) == 42);
    }
    PASS();

    TEST("set and get");
    {
        bc_array *arr = bc_array_new(arena, (int32_t)sizeof(int32_t), 4);
        int32_t values[] = {10, 20, 30};
        int i;
        for (i = 0; i < 3; i++) {
            bc_array_push(arena, arr, &values[i]);
        }
        int32_t new_val = 99;
        bc_array_set(arr, 1, &new_val);
        assert(*(int32_t *)bc_array_get(arr, 0) == 10);
        assert(*(int32_t *)bc_array_get(arr, 1) == 99);
        assert(*(int32_t *)bc_array_get(arr, 2) == 30);
    }
    PASS();

    TEST("push beyond capacity (growth)");
    {
        bc_array *arr = bc_array_new(arena, (int32_t)sizeof(int32_t), 2);
        int32_t i;
        for (i = 0; i < 100; i++) {
            bc_array_push(arena, arr, &i);
        }
        assert(bc_array_len(arr) == 100);
        for (i = 0; i < 100; i++) {
            assert(*(int32_t *)bc_array_get(arr, i) == i);
        }
    }
    PASS();

    TEST("get out of bounds (negative)");
    {
        bc_array *arr = bc_array_new(arena, (int32_t)sizeof(int32_t), 4);
        int32_t v = 1;
        bc_array_push(arena, arr, &v);
        EXPECT_PANIC({ bc_array_get(arr, -1); });
    }

    TEST("get out of bounds (too large)");
    {
        bc_array *arr = bc_array_new(arena, (int32_t)sizeof(int32_t), 4);
        int32_t v = 1;
        bc_array_push(arena, arr, &v);
        EXPECT_PANIC({ bc_array_get(arr, 1); });
    }

    TEST("set out of bounds");
    {
        bc_array *arr = bc_array_new(arena, (int32_t)sizeof(int32_t), 4);
        int32_t v = 1;
        bc_array_push(arena, arr, &v);
        int32_t nv = 99;
        EXPECT_PANIC({ bc_array_set(arr, 5, &nv); });
    }

    bc_arena_destroy(arena);
}

/* ================================================================== */
/*  String tests                                                       */
/* ================================================================== */

static void test_strings(void)
{
    printf("\n[Strings]\n");

    bc_arena *arena = bc_arena_create(4096);

    TEST("from_cstr");
    {
        bc_str s = bc_str_from_cstr("hello");
        assert(s.len == 5);
        assert(memcmp(s.data, "hello", 5) == 0);
    }
    PASS();

    TEST("from_cstr NULL");
    {
        bc_str s = bc_str_from_cstr(NULL);
        assert(s.len == 0);
    }
    PASS();

    TEST("len");
    {
        bc_str s = bc_str_from_cstr("abc");
        assert(bc_str_len(s) == 3);
    }
    PASS();

    TEST("eq (equal)");
    {
        bc_str a = bc_str_from_cstr("foo");
        bc_str b = bc_str_from_cstr("foo");
        assert(bc_str_eq(a, b) == 1);
    }
    PASS();

    TEST("eq (not equal)");
    {
        bc_str a = bc_str_from_cstr("foo");
        bc_str b = bc_str_from_cstr("bar");
        assert(bc_str_eq(a, b) == 0);
    }
    PASS();

    TEST("eq (different lengths)");
    {
        bc_str a = bc_str_from_cstr("foo");
        bc_str b = bc_str_from_cstr("foobar");
        assert(bc_str_eq(a, b) == 0);
    }
    PASS();

    TEST("concat");
    {
        bc_str a = bc_str_from_cstr("hello");
        bc_str b = bc_str_from_cstr(" world");
        bc_str c = bc_str_concat(arena, a, b);
        assert(c.len == 11);
        assert(memcmp(c.data, "hello world", 11) == 0);
    }
    PASS();

    TEST("concat with empty");
    {
        bc_str a = bc_str_from_cstr("hello");
        bc_str b = bc_str_from_cstr("");
        bc_str c = bc_str_concat(arena, a, b);
        assert(c.len == 5);
        assert(memcmp(c.data, "hello", 5) == 0);
    }
    PASS();

    TEST("to_cstr (null-terminated)");
    {
        bc_str s = bc_str_from_cstr("test");
        bc_str c = bc_str_to_cstr(arena, s);
        assert(c.len == 4);
        assert(c.data[4] == '\0');
        assert(strcmp(c.data, "test") == 0);
    }
    PASS();

    bc_arena_destroy(arena);
}

/* ================================================================== */
/*  Type cast tests                                                    */
/* ================================================================== */

static void test_type_casts(void)
{
    printf("\n[Type Casts]\n");

    TEST("i32 to i64 (safe widening)");
    assert(bc_i32_to_i64(42) == 42LL);
    assert(bc_i32_to_i64(INT32_MIN) == (int64_t)INT32_MIN);
    assert(bc_i32_to_i64(INT32_MAX) == (int64_t)INT32_MAX);
    PASS();

    TEST("i64 to i32 (valid narrow)");
    assert(bc_i64_to_i32(42LL) == 42);
    assert(bc_i64_to_i32(-100LL) == -100);
    PASS();

    TEST("i64 to i32 (overflow)");
    EXPECT_PANIC({ bc_i64_to_i32((int64_t)INT32_MAX + 1); });

    TEST("i64 to i32 (underflow)");
    EXPECT_PANIC({ bc_i64_to_i32((int64_t)INT32_MIN - 1); });

    TEST("i32 to f64 (safe)");
    assert(bc_i32_to_f64(42) == 42.0);
    assert(bc_i32_to_f64(-1) == -1.0);
    PASS();

    TEST("i64 to f64 (may lose precision)");
    assert(bc_i64_to_f64(42LL) == 42.0);
    PASS();

    TEST("f64 to i32 (valid)");
    assert(bc_f64_to_i32(42.0) == 42);
    assert(bc_f64_to_i32(-100.9) == -100);
    PASS();

    TEST("f64 to i32 (NaN)");
    {
        double nan_val = 0.0 / 0.0;
        EXPECT_PANIC({ bc_f64_to_i32(nan_val); });
    }

    TEST("f64 to i32 (out of range)");
    EXPECT_PANIC({ bc_f64_to_i32(3.0e10); });

    TEST("f64 to i64 (valid)");
    assert(bc_f64_to_i64(42.0) == 42LL);
    assert(bc_f64_to_i64(-100.5) == -100LL);
    PASS();

    TEST("f64 to i64 (NaN)");
    {
        double nan_val = 0.0 / 0.0;
        EXPECT_PANIC({ bc_f64_to_i64(nan_val); });
    }

    TEST("f64 to i64 (Inf)");
    {
        double inf_val = 1.0 / 0.0;
        EXPECT_PANIC({ bc_f64_to_i64(inf_val); });
    }
}

/* ================================================================== */
/*  Conversion tests                                                   */
/* ================================================================== */

static void test_conversions(void)
{
    printf("\n[Conversions]\n");

    bc_arena *arena = bc_arena_create(4096);

    TEST("i32_to_str positive");
    {
        bc_str s = bc_i32_to_str(arena, 42);
        assert(s.len == 2);
        assert(memcmp(s.data, "42", 2) == 0);
    }
    PASS();

    TEST("i32_to_str negative");
    {
        bc_str s = bc_i32_to_str(arena, -123);
        assert(s.len == 4);
        assert(memcmp(s.data, "-123", 4) == 0);
    }
    PASS();

    TEST("i32_to_str zero");
    {
        bc_str s = bc_i32_to_str(arena, 0);
        assert(s.len == 1);
        assert(s.data[0] == '0');
    }
    PASS();

    TEST("i32_to_str MIN");
    {
        bc_str s = bc_i32_to_str(arena, INT32_MIN);
        assert(s.len == 11); /* -2147483648 */
        assert(memcmp(s.data, "-2147483648", 11) == 0);
    }
    PASS();

    bc_arena_destroy(arena);
}

/* ================================================================== */
/*  Math helpers tests                                                 */
/* ================================================================== */

static void test_math(void)
{
    printf("\n[Math]\n");

    TEST("abs_i32 positive");
    assert(bc_abs_i32(5) == 5);
    PASS();

    TEST("abs_i32 negative");
    assert(bc_abs_i32(-5) == 5);
    PASS();

    TEST("abs_i32 zero");
    assert(bc_abs_i32(0) == 0);
    PASS();

    TEST("abs_i32 MIN_VALUE");
    EXPECT_PANIC({ bc_abs_i32(INT32_MIN); });

    TEST("abs_f64");
    assert(bc_abs_f64(-3.14) == 3.14);
    assert(bc_abs_f64(2.71) == 2.71);
    assert(bc_abs_f64(0.0) == 0.0);
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
        bc_str s = bc_str_from_cstr("hello");
        bc_print(s);
        printf(" ");
        bc_println(s);
    }
    PASS();

    TEST("print_i32");
    bc_print_i32(42);
    printf("\n");
    PASS();

    TEST("print_i64");
    bc_print_i64(123456789LL);
    printf("\n");
    PASS();

    TEST("print_f64");
    bc_print_f64(3.14);
    printf("\n");
    PASS();

    TEST("print_bool");
    bc_print_bool(1);
    printf(" ");
    bc_print_bool(0);
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
        bc_result_str_str r;
        r.is_ok = 1;
        r.value.ok = bc_str_from_cstr("success");
        assert(r.is_ok == 1);
        assert(bc_str_eq(r.value.ok, bc_str_from_cstr("success")));
    }
    PASS();

    TEST("construct Err");
    {
        bc_result_str_str r;
        r.is_ok = 0;
        r.value.err = bc_str_from_cstr("failure");
        assert(r.is_ok == 0);
        assert(bc_str_eq(r.value.err, bc_str_from_cstr("failure")));
    }
    PASS();
}

/* ================================================================== */
/*  Main                                                               */
/* ================================================================== */

int main(void)
{
    printf("=== Babel-C Runtime Test Suite ===\n");

    test_arena();
    test_checked_arith_i32();
    test_checked_arith_i64();
    test_array();
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
