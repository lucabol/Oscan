/*
 * osc_runtime_freestanding.c — Freestanding Runtime Compilation Unit
 *
 * Standalone translation unit that compiles the Oscan runtime, together
 * with the laststanding OS/graphics/image/SVG/TrueType/TLS feature
 * libraries it depends on in freestanding mode, into ONE object file with
 * no libc dependency.
 *
 * Every `oscan` freestanding build inlines this exact sequence of
 * defines/includes ahead of the *generated* program code (see
 * `emit_includes()` in src/codegen.rs, x86_64 / Windows branch) so that a
 * single translation unit can apply macro-level redirection (memcpy ->
 * l_memcpy, etc.) uniformly across program and runtime code. A future
 * native codegen backend that emits object/machine code directly (rather
 * than transpiled C) cannot rely on that per-program macro trick — it
 * needs the runtime's real, stable extern "C" symbols (as declared in
 * osc_runtime.h) precompiled once per target. This file exists to make
 * that precompilation possible: `oscan build/packaging/scripts` tooling
 * compiles it with `-c` into a freestanding object, which is archived by
 * scripts/build-runtime-archive.* (see runtime/Makefile and
 * scripts/release_tools.py's `build-runtime-archive` command).
 *
 * IMPORTANT: keep this preamble in sync with the freestanding branch of
 * `emit_includes()` in src/codegen.rs. It is intentionally duplicated
 * here (rather than shared) because this file lives in the
 * runtime/build/packaging area, while codegen.rs is compiler-owned.
 *
 * Scope: only the x86_64 / Windows feature set is supported, matching the
 * two "full" release targets (linux-x86_64 and windows-x86_64) that ship
 * a bundled C toolchain (see packaging/toolchains/*.json). RISC-V and
 * WASI freestanding builds use a narrower header chain and their own
 * compile paths (see main.rs::compile_cross_riscv64 /
 * compile_cross_wasi) and are out of scope for this archive.
 */

#define OSC_FREESTANDING
#define L_MAINFILE
#define L_WITHSNPRINTF
#define L_WITHSOCKETS
/* l_tls.h's __asm__ shims (L_WITHSTART) conflict with l_os.h's memcpy
 * function definitions; l_tls.h provides the linker symbols instead. The
 * macros are re-added below, after l_tls.h, exactly as emit_includes()
 * does for generated programs. */
#define L_MEMFUNCS_DONE

/* Enable all bundled fonts so gfx_draw_text_* can route through the
 * UTF-8 L_Font path. Must be defined before including l_gfx.h. */
#define L_FONT_PROPORTIONAL
#define L_FONT_LATIN1_SUPPLEMENT
#define L_FONT_BOX_DRAWING
#define L_UI_WITH_CUSTOM_FONT
#include "l_gfx.h" /* pulls in l_os.h */
#define OSC_HAS_GFX

#include "l_img.h"
#define OSC_HAS_IMG
#include "l_svg.h"
#define OSC_HAS_SVG
#include "l_tt.h"
#define OSC_HAS_TT

#define OSC_HAS_SOCKETS

#include "l_tls.h"
/* Restore macro redirects so osc_runtime.c can use bare libc-style names. */
#define memcpy l_memcpy
#define memcmp l_memcmp
#define memmove l_memmove
#define memset l_memset
#define strlen l_strlen

#include "osc_runtime.h"
#include "osc_runtime.c"
