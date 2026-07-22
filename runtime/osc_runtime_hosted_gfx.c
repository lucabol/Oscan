/*
 * Windows hosted graphics runtime.
 *
 * Native hosted programs link through the CRT, so osc_runtime.c must remain in
 * libc mode. The canvas/image/SVG/TrueType implementation depends on
 * laststanding's l_os.h definitions, which must stay isolated from the hosted
 * CRT-heavy runtime core.
 */

#ifdef _WIN32

#include "osc_runtime.h"

static void osc_hosted_gfx_str_to_cstr_buf(osc_str s, char *buf, int32_t bufsz)
{
    int32_t n;
    int32_t i;

    if (!buf || bufsz <= 0) return;
    n = s.len;
    if (n > bufsz - 1) n = bufsz - 1;
    for (i = 0; i < n; i++) buf[i] = s.data[i];
    buf[n] = '\0';
}

#define osc_str_to_cstr_buf osc_hosted_gfx_str_to_cstr_buf

#define L_WITHDEFS
#define L_WITHSNPRINTF

/* Enable every bundled bitmap-font tier used by gfx_draw_text_* and UI code. */
#define L_FONT_PROPORTIONAL
#define L_FONT_LATIN1_SUPPLEMENT
#define L_FONT_BOX_DRAWING
#define L_UI_WITH_CUSTOM_FONT

#include "l_gfx.h"
#define OSC_HAS_GFX

#include "l_img.h"
#define OSC_HAS_IMG

#include "l_svg.h"
#define OSC_HAS_SVG

#include "l_tt.h"
#define OSC_HAS_TT

#include "osc_runtime_graphics.inc"

#endif /* _WIN32 */
