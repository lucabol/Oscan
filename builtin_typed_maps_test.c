#ifndef OSC_NOFREESTANDING
#define OSC_FREESTANDING
#define L_MAINFILE
#define L_WITHSNPRINTF
#define L_WITHSOCKETS
#ifndef __wasi__
#include "l_gfx.h"
#define OSC_HAS_GFX
#else
#include "l_os.h"
#endif
#define OSC_HAS_SOCKETS
#include "osc_runtime.h"
#include "osc_runtime.c"
#else
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <math.h>
#include "osc_runtime.h"
#endif


void oscan_main(osc_arena* _arena);


void oscan_main(osc_arena* _arena) {
    osc_map* counts = osc_map_str_i32_new(_arena);
    osc_map_str_i32_set(_arena, counts, osc_str_from_cstr("hello"), 1);
    osc_map_str_i32_set(_arena, counts, osc_str_from_cstr("world"), 2);
    osc_print_i32(osc_map_str_i32_get(counts, osc_str_from_cstr("hello")));
    osc_print(osc_str_from_cstr(" "));
    osc_print_i32(osc_map_str_i32_get(counts, osc_str_from_cstr("world")));
    osc_print(osc_str_from_cstr(" "));
    osc_print_i32(osc_map_str_i32_len(counts));
    osc_println(osc_str_from_cstr(""));
    osc_print_i32(osc_map_str_i32_get(counts, osc_str_from_cstr("missing")));
    osc_println(osc_str_from_cstr(""));
    osc_map_str_i32_set(_arena, counts, osc_str_from_cstr("hello"), 42);
    osc_print_i32(osc_map_str_i32_get(counts, osc_str_from_cstr("hello")));
    osc_println(osc_str_from_cstr(""));
    osc_print_bool(osc_map_str_i32_has(counts, osc_str_from_cstr("world")));
    osc_println(osc_str_from_cstr(""));
    osc_map_str_i32_delete(counts, osc_str_from_cstr("world"));
    osc_print_bool(osc_map_str_i32_has(counts, osc_str_from_cstr("world")));
    osc_println(osc_str_from_cstr(""));
    osc_print_i32(osc_map_str_i32_len(counts));
    osc_println(osc_str_from_cstr(""));
    osc_map* big = osc_map_str_i64_new(_arena);
    osc_map_str_i64_set(_arena, big, osc_str_from_cstr("x"), osc_i32_to_i64(1000000000));
    osc_print_i64(osc_map_str_i64_get(big, osc_str_from_cstr("x")));
    osc_println(osc_str_from_cstr(""));
    osc_print_i64(osc_map_str_i64_get(big, osc_str_from_cstr("missing")));
    osc_println(osc_str_from_cstr(""));
    osc_map* temps = osc_map_str_f64_new(_arena);
    osc_map_str_f64_set(_arena, temps, osc_str_from_cstr("pi"), 3.14159);
    osc_print_f64(osc_map_str_f64_get(temps, osc_str_from_cstr("pi")));
    osc_println(osc_str_from_cstr(""));
    osc_print_f64(osc_map_str_f64_get(temps, osc_str_from_cstr("missing")));
    osc_println(osc_str_from_cstr(""));
    osc_map* names = osc_map_i32_str_new(_arena);
    osc_map_i32_str_set(_arena, names, 1, osc_str_from_cstr("Alice"));
    osc_map_i32_str_set(_arena, names, 2, osc_str_from_cstr("Bob"));
    osc_println(osc_map_i32_str_get(names, 1));
    osc_println(osc_map_i32_str_get(names, 2));
    osc_println(osc_map_i32_str_get(names, 99));
    osc_print_bool(osc_map_i32_str_has(names, 1));
    osc_println(osc_str_from_cstr(""));
    osc_print_i32(osc_map_i32_str_len(names));
    osc_println(osc_str_from_cstr(""));
    osc_map* squares = osc_map_i32_i32_new(_arena);
    osc_map_i32_i32_set(_arena, squares, 3, 9);
    osc_map_i32_i32_set(_arena, squares, 4, 16);
    osc_print_i32(osc_map_i32_i32_get(squares, 3));
    osc_print(osc_str_from_cstr(" "));
    osc_print_i32(osc_map_i32_i32_get(squares, 4));
    osc_println(osc_str_from_cstr(""));
    osc_print_i32(osc_map_i32_i32_get(squares, 99));
    osc_println(osc_str_from_cstr(""));
    osc_map_i32_i32_delete(squares, 3);
    osc_print_bool(osc_map_i32_i32_has(squares, 3));
    osc_println(osc_str_from_cstr(""));
    osc_map_i32_i32_set(_arena, squares, 3, 27);
    osc_print_i32(osc_map_i32_i32_get(squares, 3));
    osc_println(osc_str_from_cstr(""));
}

int main(int argc, char *argv[]) {
    osc_global_argc = argc;
    osc_global_argv = argv;
    #ifdef OSC_FREESTANDING
    l_getenv_init(argc, argv);
    #endif
    osc_arena* _arena = osc_arena_create(1048576);
    osc_global_arena = _arena;
    oscan_main(_arena);
    osc_arena_destroy(_arena);
    return 0;
}
