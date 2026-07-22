/*
 * Shared graphics/image/SVG/TrueType runtime implementation.
 *
 * Included by osc_runtime.c for freestanding/C-backend-style single-TU builds
 * and by osc_runtime_hosted_gfx.c for Windows native hosted archives, where
 * laststanding's l_os.h must stay in a separate translation unit from the
 * hosted CRT runtime core.
 */

/* ================================================================== */
/*  Graphics wrappers (requires l_gfx.h)                               */
/* ================================================================== */

#ifdef OSC_HAS_GFX

static L_Canvas osc_gfx_canvas;

/* Default retro icon: 32x32 phosphor-green "O" inside a CRT-style bezel.
 * Pixel format: 0xAARRGGBB (matches l_canvas_set_icon expectations). */
#define OSC_DEFAULT_ICON_SZ 32
static void osc_build_default_icon(uint32_t *px) {
    /* 16x16 pixel-art glyph for "O", scaled 2x to 32x32. MSB is leftmost pixel. */
    static const uint16_t glyph[16] = {
        0x0000, 0x07E0, 0x1FF8, 0x3C3C,
        0x381C, 0x700E, 0x700E, 0x700E,
        0x700E, 0x700E, 0x700E, 0x381C,
        0x3C3C, 0x1FF8, 0x07E0, 0x0000,
    };
    const int N = OSC_DEFAULT_ICON_SZ;
    for (int y = 0; y < N; y++) {
        for (int x = 0; x < N; x++) {
            uint32_t color;
            int gy = y / 2;
            int gx = x / 2;
            int on = (glyph[gy] >> (15 - gx)) & 1;
            int border = (x < 2 || y < 2 || x >= N - 2 || y >= N - 2);
            if (on) {
                /* Phosphor green with subtle scanline darkening. */
                uint32_t g = (y & 1) ? 0xC8u : 0xFFu;
                color = 0xFF000000u | (0x10u << 16) | (g << 8) | 0x20u;
            } else if (border) {
                /* CRT bezel: light gray. */
                color = 0xFFB0B0B0u;
            } else {
                /* CRT screen background: very dark green-tinted. */
                color = 0xFF080C08u;
            }
            px[y * N + x] = color;
        }
    }
}

static void osc_apply_default_icon(void) {
    uint32_t icon[OSC_DEFAULT_ICON_SZ * OSC_DEFAULT_ICON_SZ];
    osc_build_default_icon(icon);
    (void)l_canvas_set_icon(&osc_gfx_canvas, icon, OSC_DEFAULT_ICON_SZ, OSC_DEFAULT_ICON_SZ);
}

osc_result_str_str osc_canvas_open(int32_t width, int32_t height, osc_str title) {
    osc_result_str_str result;
    char buf[256];
    osc_str_to_cstr_buf(title, buf, 256);
    int rc = (int)l_canvas_open(&osc_gfx_canvas, width, height, buf);
    if (rc != 0) { result.is_ok = 0; result.value.err = osc_str_from_cstr("canvas_open: cannot open canvas"); return result; }
    osc_apply_default_icon();
    result.is_ok = 1; result.value.ok = osc_str_from_cstr(""); return result;
}

osc_result_str_str osc_canvas_set_icon(osc_array *pixels, int32_t w, int32_t h) {
    osc_result_str_str result;
    if (!pixels || w <= 0 || h <= 0 || pixels->len < w * h) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("canvas_set_icon: invalid pixel buffer");
        return result;
    }
    int rc = l_canvas_set_icon(&osc_gfx_canvas, (const uint32_t *)pixels->data, w, h);
    if (rc != 0) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("canvas_set_icon: platform call failed");
        return result;
    }
    result.is_ok = 1;
    result.value.ok = osc_str_from_cstr("");
    return result;
}

void osc_canvas_close(void) { l_canvas_close(&osc_gfx_canvas); }
uint8_t osc_canvas_alive(void) { return l_canvas_alive(&osc_gfx_canvas) ? 1 : 0; }
void osc_canvas_flush(void) { l_canvas_flush(&osc_gfx_canvas); }
void osc_canvas_clear(int32_t color) { l_canvas_clear(&osc_gfx_canvas, (uint32_t)color); }
int32_t osc_canvas_width(void) { return (int32_t)osc_gfx_canvas.width; }
int32_t osc_canvas_height(void) { return (int32_t)osc_gfx_canvas.height; }
int32_t osc_canvas_scale(void) {
    int s = osc_gfx_canvas.scale;
    return s > 0 ? (int32_t)s : 1;
}
uint8_t osc_canvas_resized(void) { return l_canvas_resized(&osc_gfx_canvas) ? 1 : 0; }

void osc_gfx_pixel(int32_t x, int32_t y, int32_t color) { l_pixel(&osc_gfx_canvas, x, y, (uint32_t)color); }
int32_t osc_gfx_get_pixel(int32_t x, int32_t y) { return (int32_t)l_get_pixel(&osc_gfx_canvas, x, y); }
void osc_gfx_line(int32_t x0, int32_t y0, int32_t x1, int32_t y1, int32_t color) { l_line(&osc_gfx_canvas, x0, y0, x1, y1, (uint32_t)color); }
void osc_gfx_rect(int32_t x, int32_t y, int32_t w, int32_t h, int32_t color) { l_rect(&osc_gfx_canvas, x, y, w, h, (uint32_t)color); }
void osc_gfx_fill_rect(int32_t x, int32_t y, int32_t w, int32_t h, int32_t color) { l_fill_rect(&osc_gfx_canvas, x, y, w, h, (uint32_t)color); }
void osc_gfx_circle(int32_t cx, int32_t cy, int32_t r, int32_t color) { l_circle(&osc_gfx_canvas, cx, cy, r, (uint32_t)color); }
void osc_gfx_fill_circle(int32_t cx, int32_t cy, int32_t r, int32_t color) { l_fill_circle(&osc_gfx_canvas, cx, cy, r, (uint32_t)color); }

/* Font id → L_Font pointer lookup. Ids match libs/fonts.osc constants:
 *   0 = FONT_DEFAULT          (ASCII, 8×8 cell)
 *   1 = FONT_PROPORTIONAL     (ASCII with per-glyph widths, 8×8 cell)
 *   2 = FONT_LATIN1           (default + U+00A0..U+00FF, 8×8 cell)
 *   3 = FONT_BOX              (default + ~33 box-drawing / arrow codepoints, 8×8 cell)
 *   4 = FONT_DEFAULT_L        (ASCII, 8×12 cell — crisp 1.5× line-height tier)
 *   5 = FONT_PROPORTIONAL_L   (ASCII with per-glyph widths, 8×12 cell)
 *   6 = FONT_LATIN1_L         (default + U+00A0..U+00FF, 8×12 cell)
 *   7 = FONT_BOX_L            (default + box-drawing / arrows, 8×12 cell)
 * Out-of-range ids fall back to the default font. */
static const L_Font *osc_font_by_id(int32_t id) {
    switch (id) {
        case 1: return &l_font8x8_proportional;
        case 2: return &l_font8x8_latin1;
        case 3: return &l_font8x8_box;
        case 4: return &l_font8x12_default;
        case 5: return &l_font8x12_proportional;
        case 6: return &l_font8x12_latin1;
        case 7: return &l_font8x12_box;
        case 0:
        default: return &l_font8x8_default;
    }
}

int32_t osc_gfx_draw_text(int32_t x, int32_t y, osc_str text, int32_t color, int32_t font) {
    char buf[1024];
    osc_str_to_cstr_buf(text, buf, 1024);
    return (int32_t)l_draw_text_f(&osc_gfx_canvas, osc_font_by_id(font), x, y, buf, (uint32_t)color);
}

int32_t osc_gfx_draw_text_scaled(int32_t x, int32_t y, osc_str text, int32_t color, int32_t sx, int32_t sy, int32_t font) {
    char buf[4096];
    osc_str_to_cstr_buf(text, buf, sizeof(buf));
    return (int32_t)l_draw_text_scaled_f(&osc_gfx_canvas, osc_font_by_id(font), x, y, buf, (uint32_t)color, sx, sy);
}

int32_t osc_gfx_text_width(osc_str text, int32_t font) {
    char buf[4096];
    osc_str_to_cstr_buf(text, buf, sizeof(buf));
    return (int32_t)l_text_width_f(osc_font_by_id(font), buf);
}

void osc_gfx_blit(int32_t dx, int32_t dy, int32_t w, int32_t h, osc_array *pixels) {
    if (!pixels || w <= 0 || h <= 0 || pixels->len < w * h) return;
    l_blit(&osc_gfx_canvas, dx, dy, w, h, (const uint32_t *)pixels->data, w * 4);
}

void osc_gfx_blit_alpha(int32_t dx, int32_t dy, int32_t w, int32_t h, osc_array *pixels) {
    if (!pixels || w <= 0 || h <= 0 || pixels->len < w * h) return;
    l_blit_alpha(&osc_gfx_canvas, dx, dy, w, h, (const uint32_t *)pixels->data, w * 4);
}

int32_t osc_canvas_key(void) {
    if (!l_canvas_alive(&osc_gfx_canvas)) return 0;
    return (int32_t)l_canvas_key(&osc_gfx_canvas);
}
int32_t osc_canvas_mouse_x(void) { return (int32_t)osc_gfx_canvas.mouse_x; }
int32_t osc_canvas_mouse_y(void) { return (int32_t)osc_gfx_canvas.mouse_y; }
int32_t osc_canvas_mouse_btn(void) { return (int32_t)osc_gfx_canvas.mouse_btn; }
int32_t osc_canvas_wheel(void) { return (int32_t)l_canvas_wheel(&osc_gfx_canvas); }

int32_t osc_clipboard_set(osc_str text) {
    char buf[4096];
    osc_str_to_cstr_buf(text, buf, (int32_t)sizeof(buf));
    return l_clipboard_set(&osc_gfx_canvas, buf, (int)text.len) == 0 ? 0 : -1;
}

osc_result_str_str osc_clipboard_get(osc_arena *arena) {
    osc_result_str_str result;
    char buf[4096];
    int n = l_clipboard_get(&osc_gfx_canvas, buf, (int)sizeof(buf));
    if (n < 0) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("clipboard_get: failed to read clipboard");
        return result;
    }
    char *copy = (char *)osc_arena_alloc(arena, (size_t)n + 1);
    memcpy(copy, buf, (size_t)n + 1);
    result.is_ok = 1;
    result.value.ok.data = copy;
    result.value.ok.len = (int32_t)n;
    return result;
}

int32_t osc_rgb(int32_t r, int32_t g, int32_t b) { return (int32_t)L_RGB(r, g, b); }
int32_t osc_rgba(int32_t r, int32_t g, int32_t b, int32_t a) { return (int32_t)L_RGBA(r, g, b, a); }

#elif !defined(OSC_RUNTIME_EXTERNAL_GRAPHICS) /* !OSC_HAS_GFX */

/* Stub implementations for platforms without graphics. Windows native hosted
   archives define OSC_RUNTIME_EXTERNAL_GRAPHICS so these symbols can be
   provided by osc_runtime_hosted_gfx.c in a separate translation unit. */

osc_result_str_str osc_canvas_open(int32_t width, int32_t height, osc_str title) {
    (void)width; (void)height; (void)title;
    osc_result_str_str r; r.is_ok = 0; r.value.err = osc_str_from_cstr("canvas_open: not supported on this platform"); return r;
}
void    osc_canvas_close(void) {}
uint8_t osc_canvas_alive(void) { return 0; }
void    osc_canvas_flush(void) {}
void    osc_canvas_clear(int32_t color) { (void)color; }
int32_t osc_canvas_width(void) { return 0; }
int32_t osc_canvas_height(void) { return 0; }
int32_t osc_canvas_scale(void) { return 1; }
uint8_t osc_canvas_resized(void) { return 0; }
osc_result_str_str osc_canvas_set_icon(osc_array *pixels, int32_t w, int32_t h) {
    (void)pixels; (void)w; (void)h;
    osc_result_str_str r; r.is_ok = 0; r.value.err = osc_str_from_cstr("canvas_set_icon: not supported on this platform"); return r;
}
void    osc_gfx_pixel(int32_t x, int32_t y, int32_t color) { (void)x; (void)y; (void)color; }
int32_t osc_gfx_get_pixel(int32_t x, int32_t y) { (void)x; (void)y; return 0; }
void    osc_gfx_line(int32_t x0, int32_t y0, int32_t x1, int32_t y1, int32_t color) { (void)x0; (void)y0; (void)x1; (void)y1; (void)color; }
void    osc_gfx_rect(int32_t x, int32_t y, int32_t w, int32_t h, int32_t color) { (void)x; (void)y; (void)w; (void)h; (void)color; }
void    osc_gfx_fill_rect(int32_t x, int32_t y, int32_t w, int32_t h, int32_t color) { (void)x; (void)y; (void)w; (void)h; (void)color; }
void    osc_gfx_circle(int32_t cx, int32_t cy, int32_t r, int32_t color) { (void)cx; (void)cy; (void)r; (void)color; }
void    osc_gfx_fill_circle(int32_t cx, int32_t cy, int32_t r, int32_t color) { (void)cx; (void)cy; (void)r; (void)color; }
int32_t osc_gfx_draw_text(int32_t x, int32_t y, osc_str text, int32_t color, int32_t font) { (void)x; (void)y; (void)text; (void)color; (void)font; return 0; }
int32_t osc_gfx_draw_text_scaled(int32_t x, int32_t y, osc_str text, int32_t color, int32_t sx, int32_t sy, int32_t font) { (void)x; (void)y; (void)text; (void)color; (void)sx; (void)sy; (void)font; return 0; }
int32_t osc_gfx_text_width(osc_str text, int32_t font) { (void)text; (void)font; return 0; }
void    osc_gfx_blit(int32_t dx, int32_t dy, int32_t w, int32_t h, osc_array *pixels) { (void)dx; (void)dy; (void)w; (void)h; (void)pixels; }
void    osc_gfx_blit_alpha(int32_t dx, int32_t dy, int32_t w, int32_t h, osc_array *pixels) { (void)dx; (void)dy; (void)w; (void)h; (void)pixels; }
int32_t osc_canvas_key(void) { return 0; }
int32_t osc_canvas_mouse_x(void) { return 0; }
int32_t osc_canvas_mouse_y(void) { return 0; }
int32_t osc_canvas_mouse_btn(void) { return 0; }
int32_t osc_canvas_wheel(void) { return 0; }
int32_t osc_clipboard_set(osc_str text) { (void)text; return -1; }
osc_result_str_str osc_clipboard_get(osc_arena *arena) {
    (void)arena;
    osc_result_str_str r; r.is_ok = 0; r.value.err = osc_str_from_cstr("clipboard: not supported on this platform"); return r;
}
int32_t osc_rgb(int32_t r, int32_t g, int32_t b) { return (int32_t)(0xFF000000u | ((uint32_t)r<<16) | ((uint32_t)g<<8) | (uint32_t)b); }
int32_t osc_rgba(int32_t r, int32_t g, int32_t b, int32_t a) { return (int32_t)(((uint32_t)a<<24) | ((uint32_t)r<<16) | ((uint32_t)g<<8) | (uint32_t)b); }

#endif /* OSC_HAS_GFX */

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

#elif !defined(OSC_RUNTIME_EXTERNAL_GRAPHICS) /* no image support */

osc_result_arr_i32_str osc_img_load(osc_arena *arena, osc_str data)
{
    (void)arena; (void)data;
    osc_result_arr_i32_str result;
    result.is_ok = 0;
    result.value.err = osc_str_from_cstr("img_load: not supported in this build");
    return result;
}

#endif /* OSC_HAS_IMG */

/* ================================================================== */
/*  SVG rasterization wrappers (requires l_svg.h)                      */
/* ================================================================== */

#ifdef OSC_HAS_SVG

osc_result_arr_i32_str osc_svg_load(osc_arena *arena, osc_str data, int32_t width, int32_t height)
{
    osc_result_arr_i32_str result;
    L_SvgOptions opt;
    opt.width  = (int)width;
    opt.height = (int)height;
    opt.dpi    = 96.0f;
    int w, h;
    uint32_t *pixels = l_svg_load_mem((const unsigned char *)data.data, data.len, &opt, &w, &h);
    if (!pixels) {
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("svg_load: failed to rasterize SVG");
        return result;
    }
    if (w <= 0 || h <= 0 || (int64_t)w * h > 0x7FFFFFFE) {
        l_svg_free_pixels(pixels, w, h);
        result.is_ok = 0;
        result.value.err = osc_str_from_cstr("svg_load: invalid image dimensions");
        return result;
    }
    int total = w * h + 2;
    osc_array *arr = osc_array_new(arena, (int32_t)sizeof(int32_t), total);
    int32_t *d = (int32_t *)arr->data;
    d[0] = (int32_t)w;
    d[1] = (int32_t)h;
    for (int i = 0; i < w * h; i++) d[i + 2] = (int32_t)pixels[i];
    arr->len = total;
    l_svg_free_pixels(pixels, w, h);
    result.is_ok = 1;
    result.value.ok = arr;
    return result;
}

#elif !defined(OSC_RUNTIME_EXTERNAL_GRAPHICS) /* no SVG support */

osc_result_arr_i32_str osc_svg_load(osc_arena *arena, osc_str data, int32_t width, int32_t height)
{
    (void)arena; (void)data; (void)width; (void)height;
    osc_result_arr_i32_str result;
    result.is_ok = 0;
    result.value.err = osc_str_from_cstr("svg_load: not supported in this build");
    return result;
}

#endif /* OSC_HAS_SVG */

/* ================================================================== */
/*  TrueType font wrappers (requires l_tt.h)                           */
/* ================================================================== */

#ifdef OSC_HAS_TT

/* Font handle: stores stbtt_fontinfo plus a copy of the font bytes.
   stb_truetype reads from the original byte buffer on every glyph
   lookup, so we own a copy for the font's lifetime. Allocated via
   l_mmap so tt_free can release the pages cleanly. */
typedef struct {
    uint32_t       magic;
    stbtt_fontinfo info;
    unsigned char *bytes;
    size_t         bytes_size;
    size_t         alloc_size;
} osc_tt_font;

#define OSC_TT_MAGIC 0x4F54464Eu /* 'OTFN' */

static osc_tt_font *osc_tt__resolve(uintptr_t h) {
    if (h == 0) return 0;
    osc_tt_font *f = (osc_tt_font *)(uintptr_t)h;
    if (f->magic != OSC_TT_MAGIC) return 0;
    return f;
}

osc_result_handle_str osc_tt_load(osc_arena *arena, osc_str data) {
    (void)arena;
    osc_result_handle_str result;
    result.is_ok = 0;

    if (!data.data || data.len <= 0) {
        result.value.err = osc_str_from_cstr("tt_load: empty font data");
        return result;
    }

    size_t bytes_cap = ((size_t)data.len + 4095) & ~(size_t)4095;
    unsigned char *bytes = (unsigned char *)l_mmap(
        0, bytes_cap,
        L_PROT_READ | L_PROT_WRITE,
        L_MAP_PRIVATE | L_MAP_ANONYMOUS, -1, 0);
    if (bytes == (unsigned char *)L_MAP_FAILED) {
        result.value.err = osc_str_from_cstr("tt_load: out of memory (font buffer)");
        return result;
    }
    l_memcpy(bytes, data.data, (size_t)data.len);

    size_t struct_cap = (sizeof(osc_tt_font) + 4095) & ~(size_t)4095;
    osc_tt_font *f = (osc_tt_font *)l_mmap(
        0, struct_cap,
        L_PROT_READ | L_PROT_WRITE,
        L_MAP_PRIVATE | L_MAP_ANONYMOUS, -1, 0);
    if (f == (osc_tt_font *)L_MAP_FAILED) {
        l_munmap(bytes, bytes_cap);
        result.value.err = osc_str_from_cstr("tt_load: out of memory (font struct)");
        return result;
    }
    f->magic      = OSC_TT_MAGIC;
    f->bytes      = bytes;
    f->bytes_size = bytes_cap;
    f->alloc_size = struct_cap;

    if (!l_tt_init_font(bytes, data.len, &f->info)) {
        f->magic = 0;
        l_munmap(bytes, bytes_cap);
        l_munmap(f, struct_cap);
        result.value.err = osc_str_from_cstr("tt_load: invalid font (not TrueType/OpenType)");
        return result;
    }

    result.is_ok = 1;
    result.value.ok = (uintptr_t)f;
    return result;
}

void osc_tt_free(uintptr_t font) {
    osc_tt_font *f = osc_tt__resolve(font);
    if (!f) return;
    unsigned char *bytes = f->bytes;
    size_t bytes_size    = f->bytes_size;
    size_t alloc_size    = f->alloc_size;
    f->magic = 0;
    if (bytes) l_munmap(bytes, bytes_size);
    l_munmap(f, alloc_size);
}

int32_t osc_tt_ascent(uintptr_t font, double pixel_height) {
    osc_tt_font *f = osc_tt__resolve(font);
    if (!f) return 0;
    float scale = l_tt_scale_for_pixel_height(&f->info, (float)pixel_height);
    int a = 0, d = 0, g = 0;
    l_tt_vmetrics(&f->info, &a, &d, &g);
    return (int32_t)(a * scale);
}

int32_t osc_tt_descent(uintptr_t font, double pixel_height) {
    osc_tt_font *f = osc_tt__resolve(font);
    if (!f) return 0;
    float scale = l_tt_scale_for_pixel_height(&f->info, (float)pixel_height);
    int a = 0, d = 0, g = 0;
    l_tt_vmetrics(&f->info, &a, &d, &g);
    return (int32_t)(d * scale);
}

int32_t osc_tt_line_gap(uintptr_t font, double pixel_height) {
    osc_tt_font *f = osc_tt__resolve(font);
    if (!f) return 0;
    float scale = l_tt_scale_for_pixel_height(&f->info, (float)pixel_height);
    int a = 0, d = 0, g = 0;
    l_tt_vmetrics(&f->info, &a, &d, &g);
    return (int32_t)(g * scale);
}

int32_t osc_tt_line_height(uintptr_t font, double pixel_height) {
    osc_tt_font *f = osc_tt__resolve(font);
    if (!f) return 0;
    float scale = l_tt_scale_for_pixel_height(&f->info, (float)pixel_height);
    int a = 0, d = 0, g = 0;
    l_tt_vmetrics(&f->info, &a, &d, &g);
    return (int32_t)((a - d + g) * scale);
}

/* Decode a single UTF-8 codepoint from s starting at *i.
   Advances *i past the codepoint. On malformed UTF-8 returns the raw
   byte and advances by 1 (so callers still make progress). */
static int osc_tt__next_cp(const char *s, int len, int *i) {
    unsigned char c = (unsigned char)s[*i];
    int cp;
    if (c < 0x80) {
        cp = c;
        *i += 1;
    } else if ((c & 0xE0) == 0xC0 && *i + 1 < len) {
        cp = ((c & 0x1F) << 6)
           |  ((unsigned char)s[*i + 1] & 0x3F);
        *i += 2;
    } else if ((c & 0xF0) == 0xE0 && *i + 2 < len) {
        cp = ((c & 0x0F) << 12)
           | (((unsigned char)s[*i + 1] & 0x3F) << 6)
           |  ((unsigned char)s[*i + 2] & 0x3F);
        *i += 3;
    } else if ((c & 0xF8) == 0xF0 && *i + 3 < len) {
        cp = ((c & 0x07) << 18)
           | (((unsigned char)s[*i + 1] & 0x3F) << 12)
           | (((unsigned char)s[*i + 2] & 0x3F) << 6)
           |  ((unsigned char)s[*i + 3] & 0x3F);
        *i += 4;
    } else {
        cp = c;
        *i += 1;
    }
    return cp;
}

int32_t osc_tt_text_width(uintptr_t font, osc_str text, double pixel_height) {
    osc_tt_font *f = osc_tt__resolve(font);
    if (!f || !text.data || text.len <= 0) return 0;
    float scale = l_tt_scale_for_pixel_height(&f->info, (float)pixel_height);
    int pen = 0, prev = 0, i = 0;
    while (i < text.len) {
        int cp = osc_tt__next_cp(text.data, text.len, &i);
        if (prev) {
            int k = l_tt_kern_advance(&f->info, prev, cp);
            pen += (int)(k * scale);
        }
        int adv = 0, lsb = 0;
        l_tt_hmetrics(&f->info, cp, &adv, &lsb);
        pen += (int)(adv * scale);
        prev = cp;
    }
    return (int32_t)pen;
}

int32_t osc_tt_draw_text(int32_t x, int32_t y, osc_str text, uintptr_t font,
                         double pixel_height, int32_t color) {
    osc_tt_font *f = osc_tt__resolve(font);
    if (!f || !text.data || text.len <= 0) return x;
    float scale = l_tt_scale_for_pixel_height(&f->info, (float)pixel_height);
    int pen = x, prev = 0, i = 0;
    while (i < text.len) {
        int cp = osc_tt__next_cp(text.data, text.len, &i);
        if (prev) {
            int k = l_tt_kern_advance(&f->info, prev, cp);
            pen += (int)(k * scale);
        }
        int gw = 0, gh = 0, gxo = 0, gyo = 0;
        unsigned char *bm = l_tt_render_glyph(&f->info, cp, scale,
                                              &gw, &gh, &gxo, &gyo);
        if (bm) {
            l_tt_blit_alpha(osc_gfx_canvas.pixels,
                            osc_gfx_canvas.width, osc_gfx_canvas.height,
                            osc_gfx_canvas.stride,
                            pen + gxo, y + gyo,
                            bm, gw, gh, (uint32_t)color);
            l_tt_free_bitmap(bm, gw, gh);
        }
        int adv = 0, lsb = 0;
        l_tt_hmetrics(&f->info, cp, &adv, &lsb);
        pen += (int)(adv * scale);
        prev = cp;
    }
    return (int32_t)pen;
}

#elif !defined(OSC_RUNTIME_EXTERNAL_GRAPHICS) /* no TrueType support */

osc_result_handle_str osc_tt_load(osc_arena *arena, osc_str data) {
    (void)arena; (void)data;
    osc_result_handle_str result;
    result.is_ok = 0;
    result.value.err = osc_str_from_cstr("tt_load: not supported in this build");
    return result;
}
void    osc_tt_free(uintptr_t font) { (void)font; }
int32_t osc_tt_ascent(uintptr_t font, double ph)      { (void)font; (void)ph; return 0; }
int32_t osc_tt_descent(uintptr_t font, double ph)     { (void)font; (void)ph; return 0; }
int32_t osc_tt_line_gap(uintptr_t font, double ph)    { (void)font; (void)ph; return 0; }
int32_t osc_tt_line_height(uintptr_t font, double ph) { (void)font; (void)ph; return 0; }
int32_t osc_tt_text_width(uintptr_t font, osc_str text, double ph) {
    (void)font; (void)text; (void)ph; return 0;
}
int32_t osc_tt_draw_text(int32_t x, int32_t y, osc_str text, uintptr_t font,
                         double ph, int32_t color) {
    (void)y; (void)text; (void)font; (void)ph; (void)color;
    return x;
}

#endif /* OSC_HAS_TT */
