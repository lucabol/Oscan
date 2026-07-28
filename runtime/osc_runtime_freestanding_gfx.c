/*
 * osc_runtime_freestanding_gfx.c — Freestanding graphics profile
 *
 * A size-tuned sibling of osc_runtime_freestanding.c. It includes the core
 * runtime and l_gfx, including the built-in fonts and canvas/window support,
 * but leaves the image, SVG, and TrueType decoders disabled. Programs that
 * reference osc_img_*, osc_svg_*, or osc_tt_* use the full runtime archive;
 * programs that only reference osc_gfx_*, osc_canvas_*, or osc_clipboard_*
 * use this profile.
 *
 * This split matters even with -ffunction-sections/-fdata-sections: Clang
 * emits floating-point constants shared by a translation unit into one
 * non-COMDAT .rdata/.rodata section. Keeping l_img/l_svg/l_tt out of this
 * translation unit lets the linker discard their entire constant pools.
 *
 * Keep the common preamble in sync with osc_runtime_freestanding.c and
 * osc_runtime_freestanding_core.c.
 */

#define OSC_FREESTANDING
#define L_MAINFILE
#define L_WITHSNPRINTF
#define L_WITHSOCKETS
#define L_MEMFUNCS_DONE

#define L_FONT_PROPORTIONAL
#define L_FONT_LATIN1_SUPPLEMENT
#define L_FONT_BOX_DRAWING
#define L_UI_WITH_CUSTOM_FONT
#include "l_gfx.h"
#define OSC_HAS_GFX

#define OSC_HAS_SOCKETS

#include "l_tls.h"
#define memcpy l_memcpy
#define memcmp l_memcmp
#define memmove l_memmove
#define memset l_memset
#define strlen l_strlen

#include "osc_runtime.h"
#include "osc_runtime.c"
