//! Capability analysis: which optional Windows import libraries and which
//! freestanding runtime archive profile a compiled program actually needs.
//!
//! Moved verbatim (modulo visibility) from the pre-split `link.rs`; see
//! `super::mod` for the "Windows import-library minimization" and
//! "Freestanding runtime profiles" module docs these functions implement.

use std::fs;
use std::path::Path;

/// Which freestanding runtime archive to link against. Hosted mode only
/// ever has one archive; freestanding mode has three (see `super::mod`'s
/// "Freestanding runtime profiles" docs): [`Full`](Self::Full) includes
/// graphics, image, SVG, and TrueType; [`Graphics`](Self::Graphics) includes
/// the core plus graphics/canvas support; and [`Core`](Self::Core) omits all
/// of those feature libraries. [`freestanding_profile`] selects the narrowest
/// complete profile per program; hosted mode ignores this entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FreestandingProfile {
    Full,
    Graphics,
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
            Self::Graphics => "freestanding_gfx",
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

/// Select the narrowest complete runtime archive for a freestanding program.
///
/// Scans `object_path`'s own undefined symbols (the same technique
/// [`detect_windows_feature_libs`] uses, and for the same reason: this
/// must be decided before/independent of `--gc-sections`, which cannot
/// partially discard feature libraries' shared constant pools):
///
/// - `osc_img_`, `osc_svg_`, or `osc_tt_` requires [`Full`](FreestandingProfile::Full);
/// - `osc_gfx_`, `osc_canvas_`, or `osc_clipboard_` requires
///   [`Graphics`](FreestandingProfile::Graphics);
/// - everything else uses [`Core`](FreestandingProfile::Core).
///
/// `osc_rgb`/`osc_rgba` deliberately do not select graphics: they are plain
/// integer packing helpers present in every profile.
///
/// Returns [`Full`](FreestandingProfile::Full), the conservative superset,
/// when `object_path` cannot be read or parsed.
pub(super) fn freestanding_profile(object_path: &Path) -> FreestandingProfile {
    let Ok(data) = fs::read(object_path) else {
        return FreestandingProfile::Full;
    };
    let Ok(file) = object::File::parse(&*data) else {
        return FreestandingProfile::Full;
    };
    profile_for_undefined_symbols(object::Object::symbols(&file).filter_map(|symbol| {
        object::ObjectSymbol::is_undefined(&symbol)
            .then(|| object::ObjectSymbol::name(&symbol).ok())
            .flatten()
    }))
}

#[cfg(feature = "inprocess-lld")]
pub(super) fn freestanding_profile_from_bytes(
    object_name: &str,
    data: &[u8],
) -> Result<FreestandingProfile, String> {
    let file = object::File::parse(data)
        .map_err(|error| format!("failed to parse object '{object_name}': {error}"))?;
    Ok(profile_for_undefined_symbols(
        object::Object::symbols(&file).filter_map(|symbol| {
            object::ObjectSymbol::is_undefined(&symbol)
                .then(|| object::ObjectSymbol::name(&symbol).ok())
                .flatten()
        }),
    ))
}

fn profile_for_undefined_symbols<'a>(
    symbols: impl IntoIterator<Item = &'a str>,
) -> FreestandingProfile {
    const FULL_PREFIXES: [&str; 3] = ["osc_img_", "osc_svg_", "osc_tt_"];
    const GRAPHICS_PREFIXES: [&str; 3] = ["osc_gfx_", "osc_canvas_", "osc_clipboard_"];

    let mut profile = FreestandingProfile::Core;
    for name in symbols {
        if FULL_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
            return FreestandingProfile::Full;
        }
        if GRAPHICS_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            profile = FreestandingProfile::Graphics;
        }
    }
    profile
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
            FreestandingProfile::Graphics.build_mode_str(),
            "freestanding_gfx"
        );
        assert_eq!(
            FreestandingProfile::Core.build_mode_str(),
            "freestanding_core"
        );
    }

    #[test]
    fn freestanding_profile_defaults_to_full_when_unreadable() {
        let missing = Path::new("this/path/does/not/exist.o");
        assert_eq!(freestanding_profile(missing), FreestandingProfile::Full);
    }

    #[test]
    fn freestanding_profile_uses_narrowest_complete_archive() {
        assert_eq!(
            profile_for_undefined_symbols(["osc_println", "osc_rgb"]),
            FreestandingProfile::Core
        );
        assert_eq!(
            profile_for_undefined_symbols(["osc_println", "osc_gfx_pixel"]),
            FreestandingProfile::Graphics
        );
        assert_eq!(
            profile_for_undefined_symbols(["osc_gfx_pixel", "osc_svg_load_shim"]),
            FreestandingProfile::Full
        );
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
