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
    size_t   alloc_size; /* total allocation size (for munmap in freestanding) */
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
int       osc_str_compare(osc_str a, osc_str b);
int32_t  osc_str_find(osc_str haystack, osc_str needle);
osc_str   osc_str_from_i32(osc_arena *arena, int32_t n);
osc_str   osc_str_slice(osc_arena *arena, osc_str s, int32_t start, int32_t end);
int32_t  osc_str_check_index(osc_str s, int32_t idx);

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
void     *osc_array_pop(osc_array *arr);
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

/* File I/O */
int32_t osc_file_open_read(osc_str path);
int32_t osc_file_open_write(osc_str path);
int32_t osc_read_byte(int32_t fd);
void    osc_write_byte(int32_t fd, int32_t b);
void    osc_write_str(int32_t fd, osc_str s);
void    osc_file_close(int32_t fd);
int32_t osc_file_delete(osc_str path);

/* Socket I/O */
int32_t osc_socket_tcp(void);
int32_t osc_socket_connect(int32_t sock, osc_str addr, int32_t port);
int32_t osc_socket_bind(int32_t sock, int32_t port);
int32_t osc_socket_listen(int32_t sock, int32_t backlog);
int32_t osc_socket_accept(int32_t sock);
int32_t osc_socket_send(int32_t sock, osc_str data);
osc_str osc_socket_recv(osc_arena *arena, int32_t sock, int32_t max_len);
void    osc_socket_close(int32_t sock);

/* UDP Socket I/O */
int32_t osc_socket_udp(void);
int32_t osc_socket_sendto(int32_t sock, osc_str data, osc_str addr, int32_t port);
osc_str osc_socket_recvfrom(osc_arena *arena, int32_t sock, int32_t max_len);

/* ------------------------------------------------------------------ */
/*  Conversion functions                                               */
/* ------------------------------------------------------------------ */

osc_str osc_i32_to_str(osc_arena *arena, int32_t n);

/* ------------------------------------------------------------------ */
/*  Math helpers (micro-lib)                                           */
/* ------------------------------------------------------------------ */

int32_t osc_abs_i32(int32_t n);
double  osc_abs_f64(double n);
int64_t osc_abs_i64(int64_t n);

double osc_math_sin(double x);
double osc_math_cos(double x);
double osc_math_sqrt(double x);
double osc_math_pow(double base, double exponent);
double osc_math_exp(double x);
double osc_math_log(double x);
double osc_math_atan2(double y, double x);
double osc_math_floor(double x);
double osc_math_ceil(double x);
double osc_math_fmod(double x, double y);
double osc_math_abs(double x);
double osc_math_pi(void);
double osc_math_e(void);
double osc_math_ln2(void);
double osc_math_sqrt2(void);

/* ------------------------------------------------------------------ */
/*  Character classification & conversion                              */
/* ------------------------------------------------------------------ */

uint8_t osc_char_is_alpha(int32_t c);
uint8_t osc_char_is_digit(int32_t c);
uint8_t osc_char_is_alnum(int32_t c);
uint8_t osc_char_is_space(int32_t c);
uint8_t osc_char_is_upper(int32_t c);
uint8_t osc_char_is_lower(int32_t c);
uint8_t osc_char_is_print(int32_t c);
uint8_t osc_char_is_xdigit(int32_t c);
int32_t osc_char_to_upper(int32_t c);
int32_t osc_char_to_lower(int32_t c);

/* ------------------------------------------------------------------ */
/*  Number parsing & conversion                                        */
/* ------------------------------------------------------------------ */

OSC_RESULT_DECL(int32_t, osc_str, osc_result_i32_str);
OSC_RESULT_DECL(int64_t, osc_str, osc_result_i64_str);

osc_result_i32_str osc_parse_i32(osc_str s);
osc_result_i64_str osc_parse_i64(osc_str s);
osc_str osc_str_from_i64(osc_arena *arena, int64_t n);
osc_str osc_str_from_f64(osc_arena *arena, double n);
osc_str osc_str_from_bool(uint8_t b);

/* ------------------------------------------------------------------ */
/*  Environment & error                                                */
/* ------------------------------------------------------------------ */

osc_result_str_str osc_env_get(osc_arena *arena, osc_str name);
int32_t osc_errno_get(void);
osc_str osc_errno_str(int32_t code);

/* ------------------------------------------------------------------ */
/*  System: random, time, sleep, exit                                  */
/* ------------------------------------------------------------------ */

void    osc_rand_seed(int32_t seed);
int32_t osc_rand_i32(void);
int64_t osc_time_now(void);
void    osc_sleep_ms(int32_t ms);
void    osc_exit(int32_t code);

/* ------------------------------------------------------------------ */
/*  Filesystem operations                                              */
/* ------------------------------------------------------------------ */

int32_t  osc_file_rename(osc_str old_path, osc_str new_path);
uint8_t  osc_file_exists(osc_str path);
int32_t  osc_dir_create(osc_str path);
int32_t  osc_dir_remove(osc_str path);
osc_str  osc_dir_current(osc_arena *arena);
int32_t  osc_dir_change(osc_str path);
int32_t  osc_file_open_append(osc_str path);
int64_t  osc_file_size(osc_str path);

/* ------------------------------------------------------------------ */
/*  Path utilities                                                     */
/* ------------------------------------------------------------------ */

osc_str   osc_path_join(osc_arena *arena, osc_str dir, osc_str file);
osc_str   osc_path_ext(osc_str path);
uint8_t   osc_path_exists(osc_str path);
uint8_t   osc_path_is_dir(osc_str path);

/* ------------------------------------------------------------------ */
/*  String operations                                                  */
/* ------------------------------------------------------------------ */

uint8_t   osc_str_contains(osc_str s, osc_str sub);
uint8_t   osc_str_starts_with(osc_str s, osc_str prefix);
uint8_t   osc_str_ends_with(osc_str s, osc_str suffix);
osc_str   osc_str_trim(osc_arena *arena, osc_str s);
osc_array *osc_str_split(osc_arena *arena, osc_str s, osc_str delim);
osc_str   osc_str_to_upper(osc_arena *arena, osc_str s);
osc_str   osc_str_to_lower(osc_arena *arena, osc_str s);
osc_str   osc_str_replace(osc_arena *arena, osc_str s, osc_str old_s, osc_str new_s);
osc_str   osc_str_from_chars(osc_arena *arena, osc_array *arr);
osc_array *osc_str_to_chars(osc_arena *arena, osc_str s);

/* ------------------------------------------------------------------ */
/*  Directory listing & process control                                */
/* ------------------------------------------------------------------ */

osc_array *osc_dir_list(osc_arena *arena, osc_str path);
int32_t    osc_proc_run(osc_str cmd, osc_array *args);
int32_t    osc_term_width(void);
int32_t    osc_term_height(void);

/* ------------------------------------------------------------------ */
/*  Raw terminal I/O                                                   */
/* ------------------------------------------------------------------ */

int32_t osc_term_raw(void);
int32_t osc_term_restore(void);
int32_t osc_read_nonblock(void);

/* ------------------------------------------------------------------ */
/*  Environment iteration                                              */
/* ------------------------------------------------------------------ */

int32_t osc_env_count(void);
osc_str osc_env_key(osc_arena *arena, int32_t i);
osc_str osc_env_value(osc_arena *arena, int32_t i);

/* ------------------------------------------------------------------ */
/*  Hex formatting                                                     */
/* ------------------------------------------------------------------ */

osc_str osc_str_from_i32_hex(osc_arena *arena, int32_t n);
osc_str osc_str_from_i64_hex(osc_arena *arena, int64_t n);

/* ------------------------------------------------------------------ */
/*  Date/Time                                                          */
/* ------------------------------------------------------------------ */

osc_str   osc_time_format(osc_arena *arena, int64_t timestamp, osc_str fmt);
int32_t   osc_time_utc_year(int64_t timestamp);
int32_t   osc_time_utc_month(int64_t timestamp);
int32_t   osc_time_utc_day(int64_t timestamp);
int32_t   osc_time_utc_hour(int64_t timestamp);
int32_t   osc_time_utc_min(int64_t timestamp);
int32_t   osc_time_utc_sec(int64_t timestamp);

/* ------------------------------------------------------------------ */
/*  Glob matching                                                      */
/* ------------------------------------------------------------------ */

uint8_t   osc_glob_match(osc_str pattern, osc_str text);

/* ------------------------------------------------------------------ */
/*  SHA-256                                                            */
/* ------------------------------------------------------------------ */

osc_str   osc_sha256(osc_arena *arena, osc_str data);

/* ------------------------------------------------------------------ */
/*  Terminal detection                                                 */
/* ------------------------------------------------------------------ */

uint8_t   osc_is_tty(void);

/* ------------------------------------------------------------------ */
/*  Environment modification                                           */
/* ------------------------------------------------------------------ */

int32_t   osc_env_set(osc_str name, osc_str value);
int32_t   osc_env_delete(osc_str name);

/* ------------------------------------------------------------------ */
/*  Array sort                                                         */
/* ------------------------------------------------------------------ */

void osc_sort_i32(osc_array *arr);
void osc_sort_i64(osc_array *arr);
void osc_sort_str(osc_array *arr);
void osc_sort_f64(osc_array *arr);

/* ------------------------------------------------------------------ */
/*  Hash map (string→string)                                           */
/* ------------------------------------------------------------------ */

typedef struct osc_map osc_map;

osc_map  *osc_map_new(osc_arena *arena);
void      osc_map_set(osc_arena *arena, osc_map *m, osc_str key, osc_str value);
osc_str   osc_map_get(osc_map *m, osc_str key);
uint8_t   osc_map_has(osc_map *m, osc_str key);
void      osc_map_delete(osc_map *m, osc_str key);
int32_t   osc_map_len(osc_map *m);

/* ------------------------------------------------------------------ */
/*  Arena reset (micro-lib convenience)                                */
/* ------------------------------------------------------------------ */

void osc_arena_reset_global(void);

/* ------------------------------------------------------------------ */
/*  Command-line argument access                                       */
/* ------------------------------------------------------------------ */

int32_t osc_arg_count(void);
osc_str osc_arg_get(osc_arena *arena, int32_t i);

/* Global argc/argv — set by generated main */
extern int osc_global_argc;
extern char **osc_global_argv;

/* Global arena — created by generated main, used by micro-lib fns */
extern osc_arena *osc_global_arena;

/* ------------------------------------------------------------------ */
/*  Graphics — Canvas lifecycle                                        */
/* ------------------------------------------------------------------ */

int32_t osc_canvas_open(int32_t width, int32_t height, osc_str title);
void    osc_canvas_close(void);
uint8_t osc_canvas_alive(void);
void    osc_canvas_flush(void);
void    osc_canvas_clear(int32_t color);

/* ------------------------------------------------------------------ */
/*  Graphics — Drawing primitives                                      */
/* ------------------------------------------------------------------ */

void    osc_gfx_pixel(int32_t x, int32_t y, int32_t color);
int32_t osc_gfx_get_pixel(int32_t x, int32_t y);
void    osc_gfx_line(int32_t x0, int32_t y0, int32_t x1, int32_t y1, int32_t color);
void    osc_gfx_rect(int32_t x, int32_t y, int32_t w, int32_t h, int32_t color);
void    osc_gfx_fill_rect(int32_t x, int32_t y, int32_t w, int32_t h, int32_t color);
void    osc_gfx_circle(int32_t cx, int32_t cy, int32_t r, int32_t color);
void    osc_gfx_fill_circle(int32_t cx, int32_t cy, int32_t r, int32_t color);
void    osc_gfx_draw_text(int32_t x, int32_t y, osc_str text, int32_t color);

/* ------------------------------------------------------------------ */
/*  Graphics — Input                                                   */
/* ------------------------------------------------------------------ */

int32_t osc_canvas_key(void);
int32_t osc_canvas_mouse_x(void);
int32_t osc_canvas_mouse_y(void);
int32_t osc_canvas_mouse_btn(void);

/* ------------------------------------------------------------------ */
/*  Graphics — Color                                                   */
/* ------------------------------------------------------------------ */

int32_t osc_rgb(int32_t r, int32_t g, int32_t b);
int32_t osc_rgba(int32_t r, int32_t g, int32_t b, int32_t a);

#ifdef __cplusplus
}
#endif

#endif /* OSC_RUNTIME_H */
