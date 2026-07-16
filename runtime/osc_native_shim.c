/*
 * osc_native_shim.c — ABI-crossing shims for the Cranelift native backend
 * (src/backend/).
 *
 * The Cranelift backend never passes or returns C aggregates (`osc_str`,
 * `osc_result_*`) by value: on Windows x64 a 16-byte struct like `osc_str`
 * is passed/returned via a hidden pointer, and replicating that
 * classification (and the SysV equivalent) directly in hand-written
 * Cranelift IR is exactly the kind of bug-prone ABI work a shim avoids.
 * Every function here takes/returns only pointers and scalars — types
 * Cranelift's built-in calling-convention support already lowers
 * correctly for every target — and forwards to the real runtime entry
 * point using a plain by-value call, which the *C compiler* (not
 * Cranelift) lowers using its own correct struct ABI.
 *
 * Naming convention: `osc_<name>_shim`. `out` parameters (when the
 * wrapped function returns an aggregate) are always the first parameter.
 * This file is compiled and linked in alongside the static Oscan runtime
 * archive only when `oscan --backend native` produces a program that
 * calls one of these; see src/backend/link.rs.
 */

#include "osc_runtime.h"

void osc_print_shim(const osc_str *s) { osc_print(*s); }
void osc_println_shim(const osc_str *s) { osc_println(*s); }

int32_t osc_str_len_shim(const osc_str *s) { return osc_str_len(*s); }
uint8_t osc_str_eq_shim(const osc_str *a, const osc_str *b) { return osc_str_eq(*a, *b); }
int32_t osc_str_compare_shim(const osc_str *a, const osc_str *b) { return (int32_t)osc_str_compare(*a, *b); }
int32_t osc_str_find_shim(const osc_str *haystack, const osc_str *needle) { return osc_str_find(*haystack, *needle); }
uint8_t osc_str_contains_shim(const osc_str *s, const osc_str *sub) { return osc_str_contains(*s, *sub); }
uint8_t osc_str_starts_with_shim(const osc_str *s, const osc_str *prefix) { return osc_str_starts_with(*s, *prefix); }
uint8_t osc_str_ends_with_shim(const osc_str *s, const osc_str *suffix) { return osc_str_ends_with(*s, *suffix); }
int32_t osc_str_check_index_shim(const osc_str *s, int32_t idx) { return osc_str_check_index(*s, idx); }

void osc_str_concat_shim(osc_str *out, osc_arena *arena, const osc_str *a, const osc_str *b) {
    *out = osc_str_concat(arena, *a, *b);
}
void osc_str_to_cstr_shim(osc_str *out, osc_arena *arena, const osc_str *s) {
    *out = osc_str_to_cstr(arena, *s);
}
void osc_str_slice_shim(osc_str *out, osc_arena *arena, const osc_str *s, int32_t start, int32_t end) {
    *out = osc_str_slice(arena, *s, start, end);
}
void osc_str_trim_shim(osc_str *out, osc_arena *arena, const osc_str *s) {
    *out = osc_str_trim(arena, *s);
}
void osc_str_to_upper_shim(osc_str *out, osc_arena *arena, const osc_str *s) {
    *out = osc_str_to_upper(arena, *s);
}
void osc_str_to_lower_shim(osc_str *out, osc_arena *arena, const osc_str *s) {
    *out = osc_str_to_lower(arena, *s);
}
void osc_str_replace_shim(osc_str *out, osc_arena *arena, const osc_str *s, const osc_str *old_s, const osc_str *new_s) {
    *out = osc_str_replace(arena, *s, *old_s, *new_s);
}

void osc_str_from_i32_shim(osc_str *out, osc_arena *arena, int32_t n) { *out = osc_str_from_i32(arena, n); }
void osc_str_from_i64_shim(osc_str *out, osc_arena *arena, int64_t n) { *out = osc_str_from_i64(arena, n); }
void osc_str_from_f64_shim(osc_str *out, osc_arena *arena, double n) { *out = osc_str_from_f64(arena, n); }
void osc_str_from_bool_shim(osc_str *out, uint8_t b) { *out = osc_str_from_bool(b); }
void osc_i32_to_str_shim(osc_str *out, osc_arena *arena, int32_t n) { *out = osc_i32_to_str(arena, n); }
void osc_str_from_i32_hex_shim(osc_str *out, osc_arena *arena, int32_t n) { *out = osc_str_from_i32_hex(arena, n); }
void osc_str_from_i64_hex_shim(osc_str *out, osc_arena *arena, int64_t n) { *out = osc_str_from_i64_hex(arena, n); }

void osc_parse_i32_shim(osc_result_i32_str *out, const osc_str *s) { *out = osc_parse_i32(*s); }
void osc_parse_i64_shim(osc_result_i64_str *out, const osc_str *s) { *out = osc_parse_i64(*s); }
void osc_read_line_shim(osc_result_str_str *out, osc_arena *arena) { *out = osc_read_line(arena); }

void osc_write_str_shim(int32_t fd, const osc_str *s) { osc_write_str(fd, *s); }
void osc_file_open_read_shim(osc_result_i32_str *out, const osc_str *path) { *out = osc_file_open_read(*path); }
void osc_file_open_write_shim(osc_result_i32_str *out, const osc_str *path) { *out = osc_file_open_write(*path); }
void osc_read_file_shim(osc_result_str_str *out, osc_arena *arena, const osc_str *path) {
    *out = osc_read_file(arena, *path);
}
void osc_write_file_shim(osc_result_str_str *out, const osc_str *path, const osc_str *data) {
    *out = osc_write_file(*path, *data);
}

uint8_t osc_file_exists_shim(const osc_str *path) { return osc_file_exists(*path); }
uint8_t osc_path_exists_shim(const osc_str *path) { return osc_path_exists(*path); }
uint8_t osc_path_is_dir_shim(const osc_str *path) { return osc_path_is_dir(*path); }
void osc_dir_current_shim(osc_str *out, osc_arena *arena) { *out = osc_dir_current(arena); }

void osc_env_get_shim(osc_result_str_str *out, osc_arena *arena, const osc_str *name) {
    *out = osc_env_get(arena, *name);
}
void osc_errno_str_shim(osc_str *out, int32_t code) { *out = osc_errno_str(code); }
void osc_sha256_shim(osc_str *out, osc_arena *arena, const osc_str *data) { *out = osc_sha256(arena, *data); }
osc_array *osc_str_split_shim(osc_arena *arena, const osc_str *s, const osc_str *delim) {
    return osc_str_split(arena, *s, *delim);
}
void osc_str_join_shim(osc_str *out, osc_arena *arena, osc_array *arr, const osc_str *sep) {
    *out = osc_str_join(arena, arr, *sep);
}
void osc_path_join_shim(osc_str *out, osc_arena *arena, const osc_str *dir, const osc_str *file) {
    *out = osc_path_join(arena, *dir, *file);
}
void osc_path_basename_shim(osc_str *out, const osc_str *path) { *out = osc_path_basename(*path); }
void osc_path_dirname_shim(osc_str *out, osc_arena *arena, const osc_str *path) {
    *out = osc_path_dirname(arena, *path);
}
void osc_file_delete_shim(osc_result_str_str *out, const osc_str *path) { *out = osc_file_delete(*path); }
int64_t osc_file_size_shim(const osc_str *path) { return osc_file_size(*path); }
void osc_str_from_chars_shim(osc_str *out, osc_arena *arena, osc_array *arr) { *out = osc_str_from_chars(arena, arr); }
osc_array *osc_str_to_chars_shim(osc_arena *arena, const osc_str *s) { return osc_str_to_chars(arena, *s); }
void osc_file_rename_shim(osc_result_str_str *out, const osc_str *old_path, const osc_str *new_path) {
    *out = osc_file_rename(*old_path, *new_path);
}
void osc_path_ext_shim(osc_str *out, const osc_str *path) { *out = osc_path_ext(*path); }
void osc_dir_create_shim(osc_result_str_str *out, const osc_str *path) { *out = osc_dir_create(*path); }
void osc_dir_remove_shim(osc_result_str_str *out, const osc_str *path) { *out = osc_dir_remove(*path); }
void osc_time_format_shim(osc_str *out, osc_arena *arena, int64_t timestamp, const osc_str *fmt) {
    *out = osc_time_format(arena, timestamp, *fmt);
}
uint8_t osc_glob_match_shim(const osc_str *pattern, const osc_str *text) { return osc_glob_match(*pattern, *text); }
void osc_env_set_shim(osc_result_str_str *out, const osc_str *name, const osc_str *value) {
    *out = osc_env_set(*name, *value);
}
void osc_env_delete_shim(osc_result_str_str *out, const osc_str *name) { *out = osc_env_delete(*name); }
void osc_arg_get_shim(osc_str *out, osc_arena *arena, int32_t i) { *out = osc_arg_get(arena, i); }

/* -- Directory listing, process control, pipes ------------------------ */
osc_array *osc_dir_list_shim(osc_arena *arena, const osc_str *path) { return osc_dir_list(arena, *path); }
void osc_dir_change_shim(osc_result_str_str *out, const osc_str *path) { *out = osc_dir_change(*path); }
void osc_file_open_append_shim(osc_result_i32_str *out, const osc_str *path) {
    *out = osc_file_open_append(*path);
}
int32_t osc_proc_run_shim(const osc_str *cmd, osc_array *args) { return osc_proc_run(*cmd, args); }
int32_t osc_proc_spawn_shim(const osc_str *cmd, osc_array *args) { return osc_proc_spawn(*cmd, args); }
void osc_path_find_exec_shim(osc_result_str_str *out, osc_arena *arena, const osc_str *name) {
    *out = osc_path_find_exec(arena, *name);
}

/* -- Raw terminal I/O --------------------------------------------------- */
void osc_term_raw_shim(osc_result_str_str *out) { *out = osc_term_raw(); }
void osc_term_restore_shim(osc_result_str_str *out) { *out = osc_term_restore(); }

/* -- Environment iteration ----------------------------------------------- */
void osc_env_key_shim(osc_str *out, osc_arena *arena, int32_t i) { *out = osc_env_key(arena, i); }
void osc_env_value_shim(osc_str *out, osc_arena *arena, int32_t i) { *out = osc_env_value(arena, i); }

/* -- TCP sockets ---------------------------------------------------------*/
void osc_socket_tcp_shim(osc_result_i32_str *out) { *out = osc_socket_tcp(); }
void osc_socket_connect_shim(osc_result_str_str *out, int32_t sock, const osc_str *addr, int32_t port) {
    *out = osc_socket_connect(sock, *addr, port);
}
void osc_socket_bind_shim(osc_result_str_str *out, int32_t sock, const osc_str *addr, int32_t port) {
    *out = osc_socket_bind(sock, *addr, port);
}
void osc_socket_listen_shim(osc_result_str_str *out, int32_t sock, int32_t backlog) {
    *out = osc_socket_listen(sock, backlog);
}
void osc_socket_accept_shim(osc_result_i32_str *out, int32_t sock) { *out = osc_socket_accept(sock); }
void osc_socket_send_shim(osc_result_i32_str *out, int32_t sock, const osc_str *data) {
    *out = osc_socket_send(sock, *data);
}
void osc_socket_recv_shim(osc_str *out, osc_arena *arena, int32_t sock, int32_t max_len) {
    *out = osc_socket_recv(arena, sock, max_len);
}

/* -- UDP sockets -----------------------------------------------------------*/
void osc_socket_udp_shim(osc_result_i32_str *out) { *out = osc_socket_udp(); }
int32_t osc_socket_sendto_shim(int32_t sock, const osc_str *data, const osc_str *addr, int32_t port) {
    return osc_socket_sendto(sock, *data, *addr, port);
}
void osc_socket_recvfrom_shim(osc_str *out, osc_arena *arena, int32_t sock, int32_t max_len) {
    *out = osc_socket_recvfrom(arena, sock, max_len);
}

/* -- Unix domain sockets -----------------------------------------------------*/
void osc_socket_unix_connect_shim(osc_result_i32_str *out, const osc_str *path) {
    *out = osc_socket_unix_connect(*path);
}

/* -- TLS (encrypted sockets) -------------------------------------------------*/
void osc_tls_connect_shim(osc_result_i32_str *out, const osc_str *host, int32_t port) {
    *out = osc_tls_connect(*host, port);
}
void osc_tls_send_shim(osc_result_i32_str *out, int32_t handle, const osc_str *data) {
    *out = osc_tls_send(handle, *data);
}
void osc_tls_recv_shim(osc_str *out, osc_arena *arena, int32_t handle, int32_t max_len) {
    *out = osc_tls_recv(arena, handle, max_len);
}

/* -- Graphics: text measurement/drawing (the only gfx_* calls that cross
 * an osc_str parameter; every other gfx_* builtin is plain scalars/an
 * already-pointer osc_array* and is called directly, no shim needed) -- */
int32_t osc_gfx_draw_text_shim(int32_t x, int32_t y, const osc_str *text, int32_t color, int32_t font) {
    return osc_gfx_draw_text(x, y, *text, color, font);
}
int32_t osc_gfx_draw_text_scaled_shim(
    int32_t x, int32_t y, const osc_str *text, int32_t color, int32_t sx, int32_t sy, int32_t font
) {
    return osc_gfx_draw_text_scaled(x, y, *text, color, sx, sy, font);
}
int32_t osc_gfx_text_width_shim(const osc_str *text, int32_t font) { return osc_gfx_text_width(*text, font); }

/* -- HashMap (untyped str->str) ------------------------------------------*/
void osc_map_set_shim(osc_arena *arena, osc_map *m, const osc_str *key, const osc_str *value) {
    osc_map_set(arena, m, *key, *value);
}
void osc_map_get_shim(osc_str *out, osc_map *m, const osc_str *key) { *out = osc_map_get(m, *key); }
uint8_t osc_map_has_shim(osc_map *m, const osc_str *key) { return osc_map_has(m, *key); }
void osc_map_delete_shim(osc_map *m, const osc_str *key) { osc_map_delete(m, *key); }

/* -- Typed HashMap: map_str_i32 (map_i32_i32/map_i32_str's key/value-only-i32
 * operations need no shims at all — they never carry an osc_str across the
 * boundary — see the direct osc_map_i32_* calls in src/backend/func.rs) --- */
void osc_map_str_i32_set_shim(osc_arena *arena, osc_map *m, const osc_str *key, int32_t value) {
    osc_map_str_i32_set(arena, m, *key, value);
}
int32_t osc_map_str_i32_get_shim(osc_map *m, const osc_str *key) { return osc_map_str_i32_get(m, *key); }
uint8_t osc_map_str_i32_has_shim(osc_map *m, const osc_str *key) { return osc_map_str_i32_has(m, *key); }
void osc_map_str_i32_delete_shim(osc_map *m, const osc_str *key) { osc_map_str_i32_delete(m, *key); }

/* -- Typed HashMap: map_str_i64 ------------------------------------------*/
void osc_map_str_i64_set_shim(osc_arena *arena, osc_map *m, const osc_str *key, int64_t value) {
    osc_map_str_i64_set(arena, m, *key, value);
}
int64_t osc_map_str_i64_get_shim(osc_map *m, const osc_str *key) { return osc_map_str_i64_get(m, *key); }
uint8_t osc_map_str_i64_has_shim(osc_map *m, const osc_str *key) { return osc_map_str_i64_has(m, *key); }
void osc_map_str_i64_delete_shim(osc_map *m, const osc_str *key) { osc_map_str_i64_delete(m, *key); }

/* -- Typed HashMap: map_str_f64 ------------------------------------------*/
void osc_map_str_f64_set_shim(osc_arena *arena, osc_map *m, const osc_str *key, double value) {
    osc_map_str_f64_set(arena, m, *key, value);
}
double osc_map_str_f64_get_shim(osc_map *m, const osc_str *key) { return osc_map_str_f64_get(m, *key); }
uint8_t osc_map_str_f64_has_shim(osc_map *m, const osc_str *key) { return osc_map_str_f64_has(m, *key); }
void osc_map_str_f64_delete_shim(osc_map *m, const osc_str *key) { osc_map_str_f64_delete(m, *key); }

/* -- Typed HashMap: map_i32_str (only `set`/`get` carry an osc_str) ------*/
void osc_map_i32_str_set_shim(osc_arena *arena, osc_map *m, int32_t key, const osc_str *value) {
    osc_map_i32_str_set(arena, m, key, *value);
}
void osc_map_i32_str_get_shim(osc_str *out, osc_map *m, int32_t key) { *out = osc_map_i32_str_get(m, key); }

/* -- Canvas (only calls that carry an osc_str param and/or return a
 * Result need a shim; canvas_close/alive/flush/clear/width/height/scale/
 * resized/key/mouse_x/mouse_y/mouse_btn/wheel are plain scalars and are
 * called directly — see src/backend/func.rs) ---------------------------*/
void osc_canvas_open_shim(osc_result_str_str *out, int32_t width, int32_t height, const osc_str *title) {
    *out = osc_canvas_open(width, height, *title);
}
void osc_canvas_set_icon_shim(osc_result_str_str *out, osc_array *pixels, int32_t w, int32_t h) {
    *out = osc_canvas_set_icon(pixels, w, h);
}

/* -- Clipboard -------------------------------------------------------------*/
int32_t osc_clipboard_set_shim(const osc_str *text) { return osc_clipboard_set(*text); }
void osc_clipboard_get_shim(osc_result_str_str *out, osc_arena *arena) { *out = osc_clipboard_get(arena); }

/* -- Image decoding ----------------------------------------------------------*/
void osc_img_load_shim(osc_result_arr_i32_str *out, osc_arena *arena, const osc_str *data) {
    *out = osc_img_load(arena, *data);
}

/* -- SVG rasterization -------------------------------------------------------*/
void osc_svg_load_shim(osc_result_arr_i32_str *out, osc_arena *arena, const osc_str *data, int32_t width, int32_t height) {
    *out = osc_svg_load(arena, *data, width, height);
}

/* -- TrueType (only load/text_width/draw_text carry an osc_str param
 * and/or return a Result; free/ascent/descent/line_gap/line_height are
 * plain scalars — `handle` is already a bare pointer-sized value, no
 * different from int64_t/uintptr_t, so it never needs marshaling — and
 * are called directly, see src/backend/func.rs) -------------------------*/
void osc_tt_load_shim(osc_result_handle_str *out, osc_arena *arena, const osc_str *data) {
    *out = osc_tt_load(arena, *data);
}
int32_t osc_tt_text_width_shim(uintptr_t font, const osc_str *text, double pixel_height) {
    return osc_tt_text_width(font, *text, pixel_height);
}
int32_t osc_tt_draw_text_shim(int32_t x, int32_t y, const osc_str *text, uintptr_t font, double pixel_height, int32_t color) {
    return osc_tt_draw_text(x, y, *text, font, pixel_height, color);
}
