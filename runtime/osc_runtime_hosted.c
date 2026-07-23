/*
 * Hosted runtime core.
 *
 * On Windows, native hosted archives provide canvas/gfx/image/SVG/TrueType
 * symbols from osc_runtime_hosted_gfx.c. Keeping laststanding's l_os.h in that
 * separate translation unit avoids collisions with the hosted CRT headers used
 * by osc_runtime.c.
 */
#ifdef _WIN32
#define OSC_RUNTIME_EXTERNAL_GRAPHICS
#endif

#define OSC_RUNTIME_EXTERNAL_TLS

#include "osc_runtime.c"
