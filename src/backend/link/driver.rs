//! Legacy compiler-driver flavor: discovery, validation, and (hosted-only
//! legacy fallback) local shim compilation. Also the
//! `OSCAN_NATIVE_LINKER`/`OSCAN_NATIVE_LINKER_FLAVOR` migration/selection
//! logic (design §7) and the mechanically-checkable no-silent-fallback rule
//! (design §7.3).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::super::target::NativeTarget;
use super::super::RuntimeMode;
use super::archive::{find_runtime_source_dir, RuntimeArchiveManifest};
use super::is_verbose;
use crate::CompilerSource;

/// A linker driver discovered on this host: GCC/Clang only (see `super::mod`
/// module docs for why MSVC is not supported here). Its identity matters
/// beyond "does it work": it must be toolchain-*compatible* with whatever
/// built the runtime archive (a MinGW-GCC-built static archive's object
/// members expect MinGW's CRT/import-library naming, e.g.
/// `__mingw_vfprintf`, `_open`/`_unlink`, `___chkstk_ms`, and WinSock
/// imports resolved via `-lws2_32` — linking those against an MSVC-mode
/// Clang/`link.exe` fails with dozens of unresolved externals). So this
/// prefers, in order: an explicit override, the *exact* compiler recorded
/// in the archive's own build manifest (guaranteeing a match), and only
/// then falls back to normal compiler discovery after checking its family,
/// version, and target against the archive provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LinkerFamily {
    GnuLd,
    Lld,
}

pub(super) struct LinkerDriver {
    pub(super) cmd: String,
    pub(super) source: CompilerSource,
    pub(super) linker_family: LinkerFamily,
}

fn compiler_family_from_command(cmd: &str) -> &'static str {
    let name = Path::new(cmd)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(cmd)
        .to_ascii_lowercase();
    if name.contains("clang") {
        "clang"
    } else {
        "gcc"
    }
}

/// Whether `cc` — the manifest's raw, **untrusted** `cc` field (read from
/// `<archive>.json`, which ships alongside a runtime archive and can come
/// from an arbitrary project directory) — is trustworthy enough to ever be
/// executed directly as a linker-driver fallback (Finding 2a, security
/// review).
///
/// Requires `cc` to be an **absolute** path whose **canonicalized** form
/// (`Path::canonicalize`, which resolves symlinks) is a descendant of one
/// of exactly two trusted roots:
/// 1. [`crate::find_toolchain_dir`]'s result — the bundled/installed
///    toolchain root (exe-relative, or an explicit `OSCAN_TOOLCHAIN_DIR`
///    override; never CWD, per that function's own hardening).
/// 2. Only in a dev build (`!native_assets::EMBEDDED_ASSETS_PRESENT`),
///    `CARGO_MANIFEST_DIR` — this deliberately preserves this repo's own
///    local dev/CI workflow, where the pinned local toolchain lives at
///    `build/toolchain-windows-x86_64` (a descendant of
///    `CARGO_MANIFEST_DIR`, *not* of `find_toolchain_dir()`'s `./toolchain`
///    convention) and the runtime archive manifests built here legitimately
///    record its absolute `clang.exe` path.
///
/// A relative path, or an absolute path canonicalizing to neither root
/// (e.g. one recorded by an unrelated/foreign project), returns `None` —
/// callers must treat that as provenance/diagnostic text only, and must
/// never execute it.
pub(super) fn trusted_manifest_cc(cc: &str) -> Option<String> {
    let path = Path::new(cc);
    if !path.is_absolute() {
        return None;
    }
    let canonical = path.canonicalize().ok()?;

    let mut trusted_roots: Vec<PathBuf> = Vec::new();
    if let Some(toolchain_dir) = crate::find_toolchain_dir() {
        if let Ok(canonical_root) = toolchain_dir.canonicalize() {
            trusted_roots.push(canonical_root);
        }
    }
    if !crate::backend::native_assets::EMBEDDED_ASSETS_PRESENT {
        if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
            if let Ok(canonical_root) = Path::new(manifest_dir).canonicalize() {
                trusted_roots.push(canonical_root);
            }
        }
    }

    trusted_roots
        .iter()
        .any(|root| canonical.starts_with(root))
        .then(|| canonical.to_string_lossy().into_owned())
}

fn validate_manifest_driver(cmd: &str, manifest: &RuntimeArchiveManifest) -> Result<(), String> {
    if let Some(expected) = manifest.cc_family.as_deref() {
        let actual = compiler_family_from_command(cmd);
        if expected != actual {
            return Err(format!(
                "runtime archive requires compiler family '{expected}', but '{cmd}' is '{actual}'"
            ));
        }
    }
    if let Some(expected) = manifest.cc_target.as_deref() {
        let output = Command::new(cmd)
            .arg("-dumpmachine")
            .output()
            .map_err(|e| format!("failed to probe linker driver '{cmd}' target: {e}"))?;
        let actual = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() || !actual.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "runtime archive was built for compiler target '{expected}', but '{cmd}' reports '{}'",
                if actual.is_empty() { "<none>" } else { &actual }
            ));
        }
    }
    if let Some(expected) = manifest.cc_version.as_deref() {
        let output = Command::new(cmd)
            .arg("--version")
            .output()
            .map_err(|e| format!("failed to probe linker driver '{cmd}' version: {e}"))?;
        let actual = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() || !actual.contains(expected) {
            return Err(format!(
                "runtime archive was built with compiler version '{expected}', but '{cmd}' reports '{}'",
                actual.lines().next().unwrap_or("<none>")
            ));
        }
    }
    Ok(())
}

pub(super) fn linker_family_for(
    cmd: &str,
    target: NativeTarget,
    manifest: Option<&RuntimeArchiveManifest>,
) -> LinkerFamily {
    if manifest.and_then(|m| m.linker_family.as_deref()) == Some("lld")
        || (target == NativeTarget::WindowsX64 && compiler_family_from_command(cmd) == "clang")
    {
        LinkerFamily::Lld
    } else {
        LinkerFamily::GnuLd
    }
}

/// Discover a GCC/Clang linker driver for the legacy `CompilerDriver`
/// flavor. Does **not** read `OSCAN_NATIVE_LINKER`/`OSCAN_NATIVE_LINKER_FLAVOR`
/// itself — see [`resolve_linker_selection`] in this module, which applies
/// the design §7.2 migration table before ever reaching this function.
///
/// Discovery order (Finding 2a, security review — rewritten so the
/// manifest's own untrusted `cc` field is never the *first* thing tried):
/// 1. **Preferred, trusted path**: [`crate::find_c_compiler`] (`OSCAN_CC`
///    override → bundled toolchain resolved relative to the oscan
///    executable → host `PATH` → MSVC), validated against the manifest's
///    recorded family/version/target via [`validate_manifest_driver`] when
///    a manifest is available.
/// 2. **Only if that's absent, or fails manifest validation**: the
///    manifest's own recorded `cc`, but exclusively via
///    [`trusted_manifest_cc`] — never executed unless it canonicalizes to
///    a descendant of a trusted root. A relative path (which would
///    resolve against this process's CWD) or a foreign absolute path is
///    never executed; it is provenance/diagnostic text only.
pub(super) fn find_linker_driver(
    archive: &Path,
    target: NativeTarget,
    manifest: Option<&RuntimeArchiveManifest>,
) -> Result<LinkerDriver, String> {
    let discovered = crate::find_c_compiler().and_then(|compiler| {
        crate::gcc_or_clang_cmd(&compiler).map(|(cmd, source)| (cmd.to_string(), source))
    });

    if let Some((cmd, source)) = &discovered {
        let validated = match manifest {
            Some(m) => validate_manifest_driver(cmd, m).is_ok(),
            None => true,
        };
        if validated {
            return Ok(LinkerDriver {
                linker_family: linker_family_for(cmd, target, manifest),
                cmd: cmd.clone(),
                source: *source,
            });
        }
    }

    if let Some(manifest) = manifest {
        if let Some(cc) = manifest.cc.as_deref() {
            if let Some(trusted_cc) = trusted_manifest_cc(cc) {
                validate_manifest_driver(&trusted_cc, manifest).map_err(|error| {
                    format!(
                        "the compiler recorded in runtime archive '{}' ('{trusted_cc}') is a trusted \
                         path but incompatible: {error}; install/use the packaged matching toolchain \
                         or set OSCAN_NATIVE_LINKER explicitly",
                        archive.display()
                    )
                })?;
                return Ok(LinkerDriver {
                    linker_family: linker_family_for(&trusted_cc, target, Some(manifest)),
                    cmd: trusted_cc,
                    source: CompilerSource::Host,
                });
            }
        }
    }

    match crate::find_c_compiler() {
        Some(_) if discovered.is_none() => Err(
            "the native backend links object files with GCC or Clang (matching the toolchain used to \
             build the runtime archive), but only an MSVC (cl.exe) toolchain was found on this host; \
             install GCC or Clang (e.g. MinGW-w64, or LLVM), or set OSCAN_NATIVE_LINKER to a GCC/Clang \
             command"
                .to_string(),
        ),
        Some(_) => Err(format!(
            "the compiler recorded in runtime archive '{}' is untrusted or unavailable, and the \
             discovered replacement is incompatible with it (family/version/target mismatch); \
             install/use the packaged matching toolchain or set OSCAN_NATIVE_LINKER explicitly",
            archive.display()
        )),
        None => {
            let recorded = manifest
                .and_then(|m| m.cc.as_deref())
                .map(|cc| {
                    format!(
                        "; the archive records compiler '{cc}', which is not a trusted path and was \
                         never executed"
                    )
                })
                .unwrap_or_default();
            Err(format!(
                "no trusted C compiler found to act as the native backend's linker (searched the same \
                 way --backend c does){recorded}; install GCC or a GNU-ABI Clang toolchain"
            ))
        }
    }
}

/// Compile `runtime/osc_native_shim.c` with flags matching `runtime_mode`,
/// caching a separate object for every mode/compiler pair. Reachable
/// **only** from the hosted legacy-archive fallback path (design §3.4) —
/// the freestanding path hard-errors instead (see
/// `super::archive::resolve_shim_source`).
pub(super) fn compile_shim_object(
    cc: &str,
    target: NativeTarget,
    runtime_mode: RuntimeMode,
) -> Result<PathBuf, String> {
    let runtime_dir = find_runtime_source_dir().ok_or_else(|| {
        "osc_native_shim.c and osc_runtime.h were not found in native-runtime/ or runtime/ relative to the oscan binary or repository root".to_string()
    })?;
    let src = runtime_dir.join("osc_native_shim.c");
    let out_dir = runtime_dir
        .parent()
        .unwrap_or(&runtime_dir)
        .join("build")
        .join("runtime-archives")
        .join(target.archive_tag());
    fs::create_dir_all(&out_dir)
        .map_err(|e| format!("error creating '{}': {e}", out_dir.display()))?;
    // Switching modes or linkers must not reuse an ABI/toolchain-incompatible
    // object from another invocation.
    let cc_tag = Path::new(cc)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cc".to_string());
    let obj = out_dir.join(format!(
        "osc_native_shim.{}.{cc_tag}.o",
        runtime_mode.as_str()
    ));

    // Recompile only if missing/stale relative to the source.
    let needs_build = match (fs::metadata(&obj), fs::metadata(&src)) {
        (Ok(obj_meta), Ok(src_meta)) => match (obj_meta.modified(), src_meta.modified()) {
            (Ok(obj_t), Ok(src_t)) => src_t > obj_t,
            _ => true,
        },
        _ => true,
    };
    if !needs_build {
        return Ok(obj);
    }

    let mut cmd = Command::new(cc);
    match runtime_mode {
        RuntimeMode::Freestanding => {
            cmd.arg("-std=gnu11").arg("-ffreestanding").arg("-w");
            if cc.to_lowercase().contains("clang") {
                cmd.arg("-Wno-error=implicit-function-declaration");
            }
            let size_opt = if cc.to_lowercase().contains("clang") {
                "-Oz"
            } else {
                "-Os"
            };
            cmd.arg(size_opt)
                .arg("-fno-builtin")
                .arg("-fno-asynchronous-unwind-tables")
                .arg("-fomit-frame-pointer")
                // See `super::mod`'s "Windows import-library minimization"
                // docs: a switch's jump table can otherwise land in a
                // shared, non-function-scoped section that keeps unrelated
                // dead code (and its Win32 imports) alive.
                .arg("-fno-jump-tables");
        }
        RuntimeMode::Hosted => {
            cmd.arg("-std=c99").arg("-O2").arg("-w");
        }
    }
    cmd.arg("-ffunction-sections")
        .arg("-fdata-sections")
        .arg(format!("-I{}", runtime_dir.display()))
        .arg("-c")
        .arg(&src)
        .arg("-o")
        .arg(&obj);
    if is_verbose() {
        eprintln!("[verbose] {:?}", cmd);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("failed to run '{cc}': {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "compiling runtime/osc_native_shim.c failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(obj)
}

/// Ask `cc` (a flag both GCC and Clang understand) where its own compiler
/// support library lives — `libgcc.a` for GCC, `libclang_rt.builtins-*.a`
/// for Clang — so it can be explicitly re-linked after `-nostdlib` (which
/// drops it along with the default CRT/libraries). Returns `None` if the
/// compiler doesn't resolve this to a real, existing file.
pub(super) fn find_compiler_builtins_lib(cc: &str) -> Option<PathBuf> {
    let output = Command::new(cc)
        .arg("-print-libgcc-file-name")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    path.is_file().then_some(path)
}

/// The result of applying the design §7.1/§7.2 selection/migration policy:
/// either the `MingwDirect` flavor (embedded or an explicit override
/// binary), the `ElfDirect` flavor (design §10), or the legacy
/// `CompilerDriver` flavor.
pub(super) enum LinkerSelection {
    Mingw(MingwLinkerSource),
    Elf(ElfLinkerSource),
    CompilerDriver(LinkerDriver),
}

pub(super) enum MingwLinkerSource {
    /// Use this build's embedded/extracted `ld.lld`.
    Embedded,
    /// `OSCAN_NATIVE_LINKER` + `OSCAN_NATIVE_LINKER_FLAVOR=mingw`: invoke
    /// the override binary directly. Import libs/builtins still come from
    /// the embedded asset store.
    Override { command: String },
}

pub(super) enum ElfLinkerSource {
    /// Use this build's embedded/extracted `x86_64-linux-musl-ld`.
    Embedded,
    /// `OSCAN_NATIVE_LINKER` + `OSCAN_NATIVE_LINKER_FLAVOR=elf`: invoke the
    /// override binary directly (design §10.9).
    Override { command: String },
}

/// Applies the design §7.2 migration table. `mingw_eligible` gates whether
/// the default (no-override) path may ever choose `MingwDirect` at all
/// (Windows x86-64 target, freestanding mode, no explicit user `.c` files —
/// design §1.1/§1.2); `elf_eligible` gates `ElfDirect` similarly (Linux
/// x86-64 freestanding, no user `.c` files — design §10.6);
/// `embedded_assets_present` is `native_assets::EMBEDDED_ASSETS_PRESENT`.
pub(super) fn resolve_linker_selection(
    target: NativeTarget,
    runtime_mode: RuntimeMode,
    mingw_eligible: bool,
    elf_eligible: bool,
    archive: &Path,
    manifest: Option<&RuntimeArchiveManifest>,
    embedded_assets_present: bool,
) -> Result<LinkerSelection, String> {
    let native_linker = env_var_nonempty("OSCAN_NATIVE_LINKER");
    let flavor_override = env_var_nonempty("OSCAN_NATIVE_LINKER_FLAVOR");

    match (native_linker, flavor_override.as_deref()) {
        // set / unset: legacy compatibility — treated as a compiler driver,
        // plus a one-line migration diagnostic (design §7.2 row 1).
        (Some(cmd), None) => {
            eprintln!(
                "note: OSCAN_NATIVE_LINKER is being interpreted as a C compiler driver for backward \
                 compatibility; set OSCAN_NATIVE_LINKER_FLAVOR=mingw to invoke a direct ld.lld instead."
            );
            Ok(LinkerSelection::CompilerDriver(LinkerDriver {
                linker_family: linker_family_for(&cmd, target, manifest),
                cmd,
                source: CompilerSource::Override,
            }))
        }
        // set / compiler-driver: legacy driver, no diagnostic.
        (Some(cmd), Some("compiler-driver")) => Ok(LinkerSelection::CompilerDriver(LinkerDriver {
            linker_family: linker_family_for(&cmd, target, manifest),
            cmd,
            source: CompilerSource::Override,
        })),
        // set / mingw: invoke that binary directly (Windows).
        (Some(cmd), Some("mingw")) => {
            if !embedded_assets_present {
                return Err(format!(
                    "OSCAN_NATIVE_LINKER_FLAVOR=mingw requires this oscan build to embed its own native-link \
                     import libraries/compiler-builtins (even when OSCAN_NATIVE_LINKER overrides the linker \
                     binary itself); this build has none embedded (a dev build) — unset \
                     OSCAN_NATIVE_LINKER_FLAVOR to use '{cmd}' as a compiler driver instead"
                ));
            }
            Ok(LinkerSelection::Mingw(MingwLinkerSource::Override { command: cmd }))
        }
        // set / elf: invoke that binary directly (Linux). Unlike
        // MingwDirect, ElfDirect needs no embedded import libraries or
        // compiler builtins, so an explicit GNU ld override is complete by
        // itself and works in an ordinary dev build.
        (Some(cmd), Some("elf")) => Ok(LinkerSelection::Elf(ElfLinkerSource::Override {
            command: cmd,
        })),
        (Some(_), Some(other)) => Err(format!(
            "OSCAN_NATIVE_LINKER_FLAVOR='{other}' is not recognized (expected 'compiler-driver', 'mingw', or 'elf')"
        )),
        // unset / compiler-driver: FLAVOR alone selects compiler-driver for
        // the default-resolved linker.
        (None, Some("compiler-driver")) => {
            find_linker_driver(archive, target, manifest).map(LinkerSelection::CompilerDriver)
        }
        // unset / mingw: FLAVOR alone selects MingwDirect for the
        // default-resolved (embedded) linker.
        (None, Some("mingw")) => {
            if !embedded_assets_present {
                return Err(
                    "OSCAN_NATIVE_LINKER_FLAVOR=mingw was set, but this oscan build has no embedded \
                     native linker/import libraries (a dev build); unset OSCAN_NATIVE_LINKER_FLAVOR, or \
                     set OSCAN_NATIVE_LINKER to a ld.lld-compatible binary as well"
                        .to_string(),
                );
            }
            Ok(LinkerSelection::Mingw(MingwLinkerSource::Embedded))
        }
        // unset / elf: FLAVOR alone selects ElfDirect for the
        // default-resolved (embedded) linker.
        (None, Some("elf")) => {
            if !embedded_assets_present {
                return Err(
                    "OSCAN_NATIVE_LINKER_FLAVOR=elf was set, but this oscan build has no embedded \
                     native linker (a dev build); unset OSCAN_NATIVE_LINKER_FLAVOR, or set \
                     OSCAN_NATIVE_LINKER to a GNU ld-compatible binary as well"
                        .to_string(),
                );
            }
            Ok(LinkerSelection::Elf(ElfLinkerSource::Embedded))
        }
        (None, Some(other)) => Err(format!(
            "OSCAN_NATIVE_LINKER_FLAVOR='{other}' is not recognized (expected 'compiler-driver', 'mingw', or 'elf')"
        )),
        // unset / unset: the default selection policy (design §7.1/§10.11).
        (None, None) => {
            if mingw_eligible && embedded_assets_present {
                Ok(LinkerSelection::Mingw(MingwLinkerSource::Embedded))
            } else if elf_eligible && embedded_assets_present {
                Ok(LinkerSelection::Elf(ElfLinkerSource::Embedded))
            } else {
                if (mingw_eligible || elf_eligible) && runtime_mode == RuntimeMode::Freestanding {
                    // Design §5.3: the dev-build (no embedded assets) note.
                    eprintln!(
                        "note: this oscan build has no embedded native linker (dev build); using external \
                         C toolchain as linker driver"
                    );
                }
                find_linker_driver(archive, target, manifest).map(LinkerSelection::CompilerDriver)
            }
        }
    }
}

pub(super) fn env_var_nonempty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The no-silent-fallback rule (design §7.3), mechanically enforced: if this
/// build claims embedded assets (`EMBEDDED_ASSETS_PRESENT == true`) and
/// either extraction or the direct link fails, this is the **only** message
/// callers may return in that situation — there is no code path from here
/// back into [`LinkerSelection::CompilerDriver`] without an explicit
/// `OSCAN_NATIVE_LINKER_FLAVOR=compiler-driver` override, which is a
/// distinct, already-resolved branch in [`resolve_linker_selection`] above.
pub(super) fn no_silent_fallback_error(reason: &str) -> String {
    format!(
        "native link failed using this build's embedded linker: {reason}. This oscan build embeds \
         its own linker and will not silently fall back to an external C toolchain. To override, set \
         OSCAN_NATIVE_LINKER together with OSCAN_NATIVE_LINKER_FLAVOR (e.g. =mingw for a direct \
         ld.lld, or =compiler-driver for the legacy path)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes all tests in this module that touch
    /// `OSCAN_NATIVE_LINKER` / `OSCAN_NATIVE_LINKER_FLAVOR` env vars,
    /// both global process state shared by every test thread in this
    /// binary. Follows the same pattern as `archive.rs`'s
    /// `RUNTIME_BUILDER_ENV_TEST_LOCK`.
    static LINKER_ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn windows_clang_selects_lld_without_changing_linux_clang() {
        assert_eq!(
            linker_family_for("clang.exe", NativeTarget::WindowsX64, None),
            LinkerFamily::Lld
        );
        assert_eq!(
            linker_family_for("clang", NativeTarget::LinuxX64, None),
            LinkerFamily::GnuLd
        );
        assert_eq!(compiler_family_from_command("gcc.exe"), "gcc");
    }

    #[test]
    fn elf_flavor_is_now_accepted_by_resolve_linker_selection() {
        // Design §10.9: "elf" is no longer rejected; it selects ElfDirect.
        // We cannot fully test `resolve_linker_selection` here (it reads env
        // vars and may call `find_linker_driver`), but we can verify the
        // enum variant exists and is constructible.
        let _: LinkerSelection = LinkerSelection::Elf(ElfLinkerSource::Embedded);
        let _: LinkerSelection = LinkerSelection::Elf(ElfLinkerSource::Override {
            command: "/usr/bin/ld".to_string(),
        });
    }

    #[test]
    fn resolve_linker_selection_returns_elf_embedded_when_flavor_is_elf_and_assets_present() {
        let _lock = LINKER_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        env::remove_var("OSCAN_NATIVE_LINKER");
        env::set_var("OSCAN_NATIVE_LINKER_FLAVOR", "elf");

        let result = resolve_linker_selection(
            NativeTarget::LinuxX64,
            RuntimeMode::Freestanding,
            false,
            true,
            Path::new("dummy.a"),
            None,
            true, // embedded_assets_present
        );
        assert!(
            matches!(result, Ok(LinkerSelection::Elf(ElfLinkerSource::Embedded))),
            "FLAVOR=elf + assets present must yield Elf(Embedded)"
        );

        env::remove_var("OSCAN_NATIVE_LINKER_FLAVOR");
    }

    #[test]
    fn resolve_linker_selection_allows_elf_override_without_embedded_assets() {
        let _lock = LINKER_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        env::set_var("OSCAN_NATIVE_LINKER", "/usr/bin/ld");
        env::set_var("OSCAN_NATIVE_LINKER_FLAVOR", "elf");

        let result = resolve_linker_selection(
            NativeTarget::LinuxX64,
            RuntimeMode::Freestanding,
            false,
            true,
            Path::new("dummy.a"),
            None,
            false,
        );
        assert!(matches!(
            result,
            Ok(LinkerSelection::Elf(ElfLinkerSource::Override { command }))
                if command == "/usr/bin/ld"
        ));

        env::remove_var("OSCAN_NATIVE_LINKER");
        env::remove_var("OSCAN_NATIVE_LINKER_FLAVOR");
    }

    #[test]
    fn resolve_linker_selection_errors_when_flavor_is_elf_but_no_assets() {
        let _lock = LINKER_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        env::remove_var("OSCAN_NATIVE_LINKER");
        env::set_var("OSCAN_NATIVE_LINKER_FLAVOR", "elf");

        let result = resolve_linker_selection(
            NativeTarget::LinuxX64,
            RuntimeMode::Freestanding,
            false,
            true,
            Path::new("dummy.a"),
            None,
            false, // embedded_assets_present
        );
        assert!(result.is_err(), "FLAVOR=elf + no assets must error");

        env::remove_var("OSCAN_NATIVE_LINKER_FLAVOR");
    }

    #[test]
    fn no_silent_fallback_error_never_suggests_a_bare_compiler_driver_default() {
        let msg = no_silent_fallback_error("extraction failed: disk full");
        assert!(msg.contains("will not silently fall back"));
        assert!(msg.contains("OSCAN_NATIVE_LINKER_FLAVOR"));
        assert!(msg.contains("compiler-driver"));
    }

    // --- Finding 2: `trusted_manifest_cc` / `find_linker_driver` must
    // --- never execute an untrusted manifest-recorded `cc`. ---

    #[test]
    fn trusted_manifest_cc_rejects_a_relative_path() {
        // A relative path resolves against this process's CWD -- exactly
        // the original vulnerability (a malicious project directory
        // shipping a manifest with a relative `cc`).
        assert_eq!(trusted_manifest_cc("build/toolchain/clang.exe"), None);
        assert_eq!(trusted_manifest_cc("clang"), None);
    }

    #[test]
    fn trusted_manifest_cc_rejects_a_foreign_absolute_path() {
        // An absolute path that exists on this host but has nothing to do
        // with this build's trusted roots (bundled toolchain /
        // CARGO_MANIFEST_DIR in dev builds) must never be trusted.
        #[cfg(windows)]
        let foreign = r"C:\Windows\System32\cmd.exe";
        #[cfg(not(windows))]
        let foreign = "/bin/sh";

        assert!(
            Path::new(foreign).is_file(),
            "test fixture path must actually exist on this host: {foreign}"
        );
        assert_eq!(trusted_manifest_cc(foreign), None);
    }

    #[test]
    fn trusted_manifest_cc_accepts_a_path_under_a_trusted_root() {
        // In a dev/test build (EMBEDDED_ASSETS_PRESENT == false, per
        // build.rs's documented default), CARGO_MANIFEST_DIR is itself a
        // trusted root -- exactly this repo's own local dev/CI toolchain
        // story (build/toolchain-windows-x86_64 lives under it).
        assert!(
            !crate::backend::native_assets::EMBEDDED_ASSETS_PRESENT,
            "this test assumes a normal `cargo test` dev build with no embedded assets"
        );
        let trusted_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(trusted_path.is_file());

        let result = trusted_manifest_cc(&trusted_path.to_string_lossy());
        assert!(
            result.is_some(),
            "a path under CARGO_MANIFEST_DIR must be trusted in a dev build"
        );
    }

    #[test]
    fn find_linker_driver_never_executes_a_foreign_manifest_recorded_cc() {
        let dir = std::env::temp_dir().join(format!(
            "oscan-driver-rs-test-foreign-cc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("create scratch dir");
        let marker = dir.join("EXECUTED.marker");

        #[cfg(windows)]
        let fake_cc = {
            let path = dir.join("fake-clang.bat");
            fs::write(
                &path,
                format!("@echo off\r\necho executed> \"{}\"\r\n", marker.display()),
            )
            .expect("write fake compiler script");
            path
        };
        #[cfg(not(windows))]
        let fake_cc = {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join("fake-clang.sh");
            fs::write(
                &path,
                format!("#!/bin/sh\necho executed > \"{}\"\n", marker.display()),
            )
            .expect("write fake compiler script");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod +x");
            path
        };

        let manifest = RuntimeArchiveManifest {
            cc: Some(fake_cc.to_string_lossy().into_owned()),
            ..Default::default()
        };

        // Whether this returns Ok (a trusted compiler was discovered
        // instead) or Err (nothing trusted was found/validated), the
        // foreign path recorded in the manifest must never be executed.
        let _ = find_linker_driver(
            Path::new("fake-archive.a"),
            NativeTarget::WindowsX64,
            Some(&manifest),
        );

        assert!(
            !marker.exists(),
            "a foreign, untrusted manifest-recorded `cc` must never be executed"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
