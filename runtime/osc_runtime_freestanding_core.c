/*
 * osc_runtime_freestanding_core.c — Freestanding "core" Compilation Unit
 *
 * A leaner sibling of osc_runtime_freestanding.c: the same freestanding
 * Oscan runtime (arena, strings, checked arithmetic, panic, print family,
 * file/process/env I/O, maps, sockets, TLS) but WITHOUT the graphics/
 * image/SVG/TrueType feature libraries (l_gfx.h/l_img.h/l_svg.h/l_tt.h).
 *
 * Why this file exists (see src/backend/link.rs's "Freestanding runtime
 * profiles" module docs for the full rationale): osc_runtime.c compiles
 * gfx/img/svg/tt as one giant translation unit alongside core/sockets/TLS
 * so that --gc-sections can drop the object *code* a given program never
 * calls. That works for functions (compiled with -ffunction-sections),
 * but the graphics/font/curve-flattening code's own floating-point
 * constant pool is emitted by the compiler backend into a single, non-
 * COMDAT `.rdata`/`.rodata` blob shared by every function in the
 * translation unit — section GC cannot partially discard it, so it
 * survives whole in *every* freestanding native executable even when
 * nothing calls into gfx/img/svg/tt (e.g. `hello.osc`), costing several
 * KB of pure dead weight. Splitting gfx/img/svg/tt into their own archive
 * member/translation unit (rather than only relying on section GC) lets
 * ordinary static-archive member selection — a program only pulls in an
 * archive member if it references an undefined symbol defined there —
 * exclude that whole constant pool for programs that never touch
 * graphics, without touching osc_runtime.c itself.
 *
 * `src/backend/link.rs` selects this archive
 * (`libosc_runtime_freestanding_core.a`) instead of the full
 * `libosc_runtime_freestanding.a` only when the compiled program's own
 * object file has no undefined `osc_gfx_*`/`osc_canvas_*`/
 * `osc_clipboard_*`/`osc_img_*`/`osc_svg_*`/`osc_tt_*` symbol (and no
 * extra user C sources, which cannot be scanned) — see
 * `program_needs_graphics_runtime` there. `osc_runtime.c`'s own
 * `#ifdef OSC_HAS_GFX` stub branch (compiled here, since OSC_HAS_GFX is
 * left undefined) still provides `osc_rgb`/`osc_rgba` and no-op
 * canvas/gfx stubs for API completeness/link-compatibility; `OSC_HAS_IMG`/
 * `OSC_HAS_SVG`/`OSC_HAS_TT` have no stub branch at all, so this object
 * simply does not define `osc_img_load`/`osc_svg_load`/`osc_tt_*` — a
 * program that needs them must resolve to the full archive instead.
 *
 * Sockets/TLS (`l_tls.h`, `OSC_HAS_SOCKETS`) are unaffected and identical
 * to osc_runtime_freestanding.c: they are not part of the gfx/img/svg/tt
 * constant-pool problem (verified: no cross-references either way), and
 * splitting them out was not needed to close the native/C size gap, so
 * this keeps that surface unchanged to minimize risk.
 *
 * IMPORTANT: keep this in sync with osc_runtime_freestanding.c (this file
 * should always be that file's preamble minus the l_gfx.h/l_img.h/
 * l_svg.h/l_tt.h block and its OSC_HAS_GFX/IMG/SVG/TT defines) and with
 * packaging/toolchains/runtime-archive-contract.json's
 * `freestanding_core` mode, which compiles this file into the sibling
 * archive.
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

#include "l_os.h"

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
