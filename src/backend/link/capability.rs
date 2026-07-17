//! Capability analysis: which optional Windows import libraries and which
//! freestanding runtime archive profile a compiled program actually needs.
//!
//! Moved verbatim (modulo visibility) from the pre-split `link.rs`; see
//! `super::mod` for the "Windows import-library minimization" and
//! "Freestanding runtime profiles" module docs these functions implement.

use std::fs;
use std::path::Path;

/// Which freestanding runtime archive to link against. Hosted mode only
/// ever has one archive; freestanding mode has two (see `super::mod`'s
/// "Freestanding runtime profiles" docs): [`Full`](Self::Full)
/// (`libosc_runtime_freestanding.a`, everything, including graphics/
/// image/SVG/TrueType) and [`Core`](Self::Core)
/// (`libosc_runtime_freestanding_core.a`, the same runtime minus those
/// feature libraries). [`program_needs_graphics_runtime`] decides between
/// them per program; hosted mode ignores this entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FreestandingProfile {
    Full,
    Core,
}

impl FreestandingProfile {
    /// The `--mode` value `scripts/release_tools.py build-runtime-archive`
    /// expects (see `packaging/toolchains/runtime-archive-contract.json`'s
    /// `modes` map) — distinct from `RuntimeMode::as_str`, which is a
    /// user-facing "freestanding"/"hosted" label, not an archive variant.
    pub(super) fn build_mode_str(self) -> &'static str {
        match self {
            Self::Full => "freestanding",
            Self::Core => "freestanding_core",
        }
    }
}

/// Optional Windows import libraries a *freestanding* program's compiled
/// object might need, beyond the always-linked `-lkernel32`, determined by
/// scanning `object_path`'s own undefined symbols for the runtime entry
/// points each optional feature area calls (see this module's docs,
/// "Windows import-library minimization", for why this must be decided
/// here rather than left for `--gc-sections` to sort out on its own, and
/// `src/backend/func.rs` for the exhaustive set of `osc_*`/`osc_*_shim`
/// names the Cranelift backend ever declares/calls). Names are matched by
/// prefix against the runtime's own naming convention:
///
/// - `osc_socket_*` (TCP/UDP/Unix-domain sockets, including
///   `osc_socket_close`) and `osc_tls_*` (TLS is itself socket-based, see
///   `deps/laststanding/l_tls.h`'s Windows `l_tls_connect`, which calls
///   `socket`/`connect`/`closesocket` directly) need `-lws2_32`.
/// - `osc_tls_*` additionally needs `-lsecur32 -lcrypt32` (Schannel).
/// - `osc_canvas_*` (real OS window) and `osc_clipboard_*` (desktop
///   clipboard) need `-luser32 -lgdi32`. The non-interactive drawing
///   primitives (`osc_gfx_*`, `osc_rgb`/`osc_rgba`) and the image/SVG/
///   TrueType decoders (`osc_img_*`/`osc_svg_*`/`osc_tt_*`) are pure
///   in-memory pixel-buffer code with no Win32 dependency of their own,
///   so they are deliberately *not* matched here.
///
/// Falls back to requesting every optional library (the previous,
/// unconditional behavior) if `object_path` cannot be read or parsed as an
/// object file, so a scanning failure degrades to "link everything" rather
/// than risking an unresolved-symbol link error.
///
/// Only used for the [`super::plan::LinkerFlavor::CompilerDriver`] flavor:
/// `MingwDirect` always requests every optional import library regardless
/// of this scan (the "LLD-sees-all-optional-imports rule", design §2.4).
pub(super) fn detect_windows_feature_libs(object_path: &Path) -> Vec<&'static str> {
    let all = vec!["ws2_32", "user32", "gdi32", "secur32", "crypt32"];
    let Ok(data) = fs::read(object_path) else {
        return all;
    };
    let Ok(file) = object::File::parse(&*data) else {
        return all;
    };

    let (mut needs_sockets, mut needs_tls, mut needs_windowing) = (false, false, false);
    for symbol in object::Object::symbols(&file) {
        if !object::ObjectSymbol::is_undefined(&symbol) {
            continue;
        }
        let Ok(name) = object::ObjectSymbol::name(&symbol) else {
            continue;
        };
        if name.starts_with("osc_socket_") || name.starts_with("osc_tls_") {
            needs_sockets = true;
        }
        if name.starts_with("osc_tls_") {
            needs_tls = true;
        }
        if name.starts_with("osc_canvas_") || name.starts_with("osc_clipboard_") {
            needs_windowing = true;
        }
    }

    let mut libs = Vec::new();
    if needs_sockets {
        libs.push("ws2_32");
    }
    if needs_tls {
        libs.push("secur32");
        libs.push("crypt32");
    }
    if needs_windowing {
        libs.push("user32");
        libs.push("gdi32");
    }
    libs
}

/// Whether a *freestanding* program needs the full
/// (`libosc_runtime_freestanding.a`) runtime archive rather than the
/// smaller `libosc_runtime_freestanding_core.a` sibling that omits
/// graphics/image/SVG/TrueType (see `super::mod`'s "Freestanding runtime
/// profiles" docs and `runtime/osc_runtime_freestanding_core.c`).
///
/// Scans `object_path`'s own undefined symbols (the same technique
/// [`detect_windows_feature_libs`] uses, and for the same reason: this
/// must be decided before/independent of `--gc-sections`, which cannot
/// partially discard the graphics feature libraries' shared constant
/// pool once *any* part of it is reachable) for the prefixes the
/// graphics-adjacent runtime surface is exclusively defined under —
/// `osc_gfx_` (`src/backend/func.rs`'s non-interactive drawing
/// primitives, e.g. `gfx_pixel`/`gfx_text_width`), `osc_canvas_`/
/// `osc_clipboard_` (the interactive window/clipboard builtins — not
/// currently reachable from this backend at all per its "not
/// implemented" list in `src/backend/mod.rs`, but matched anyway in case
/// that ever changes), and `osc_img_`/`osc_svg_`/`osc_tt_` (image/SVG/
/// TrueType decoding — likewise not yet reachable here). `osc_rgb`/
/// `osc_rgba` are deliberately *not* matched: they are plain integer
/// packing helpers present identically in both archives (see
/// `osc_runtime.c`'s `OSC_HAS_GFX` stub branch), so referencing them
/// alone never requires the full archive.
///
/// Returns `true` (the conservative, always-correct choice — the full
/// archive is a strict superset of the core one) when `object_path`
/// cannot be read/parsed, mirroring [`detect_windows_feature_libs`]'s
/// "degrade to link everything" fallback.
pub(super) fn program_needs_graphics_runtime(object_path: &Path) -> bool {
    const GRAPHICS_PREFIXES: [&str; 6] = [
        "osc_gfx_",
        "osc_canvas_",
        "osc_clipboard_",
        "osc_img_",
        "osc_svg_",
        "osc_tt_",
    ];
    let Ok(data) = fs::read(object_path) else {
        return true;
    };
    let Ok(file) = object::File::parse(&*data) else {
        return true;
    };
    object::Object::symbols(&file).any(|symbol| {
        object::ObjectSymbol::is_undefined(&symbol)
            && object::ObjectSymbol::name(&symbol).is_ok_and(|name| {
                GRAPHICS_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freestanding_profile_build_mode_str_matches_contract_modes() {
        // Must match packaging/toolchains/runtime-archive-contract.json's
        // "modes" keys exactly — this is the --mode value passed to
        // scripts/release_tools.py build-runtime-archive.
        assert_eq!(FreestandingProfile::Full.build_mode_str(), "freestanding");
        assert_eq!(
            FreestandingProfile::Core.build_mode_str(),
            "freestanding_core"
        );
    }

    #[test]
    fn program_needs_graphics_runtime_defaults_true_when_unreadable() {
        // Conservative fallback: an object that can't be read/parsed must
        // resolve to the full archive, never the smaller core one.
        let missing = Path::new("this/path/does/not/exist.o");
        assert!(program_needs_graphics_runtime(missing));
    }

    #[test]
    fn detect_windows_feature_libs_defaults_to_all_when_unreadable() {
        let missing = Path::new("this/path/does/not/exist.o");
        let libs = detect_windows_feature_libs(missing);
        assert_eq!(
            libs,
            vec!["ws2_32", "user32", "gdi32", "secur32", "crypt32"]
        );
    }
}
