//! Object emission, runtime-archive discovery/build, and final linking
//! for the Cranelift native backend.
//!
//! Linking is deliberately driven through GCC/Clang as a linker front-end
//! (`cc obj.o shim.o runtime.a -o out.exe`) rather than raw `link.exe`:
//! the freestanding runtime archive is built with GCC/Clang (see
//! `scripts/release_tools.py`'s `default_cc_for_target`, which prefers
//! `gcc`/`clang` on every host this backend targets), and mixing a
//! MinGW-built static archive with MSVC's `link.exe` does not reliably
//! work (different CRT/ABI expectations for the archive's own object
//! members). The Cranelift object itself is generated for the matching
//! `*-gnu` target triple (see `target.rs`) for the same reason. This is
//! why [`link_executable`] returns a clear error instead of attempting a
//! link when only `cl.exe` is available — not a silent fallback, a
//! reported tooling gap (MSVC-only Windows support is tracked as a
//! `native-completeness` follow-up).
//!
//! # Explicit runtime modes
//!
//! [`link_executable`] receives an explicit [`RuntimeMode`]. The default
//! CLI path passes `Freestanding`, keeping `--backend native` standalone
//! and libc-free; only `--libc --backend native` passes `Hosted`, which
//! selects the hosted archive and normal CRT/libm/system linking.
//! [`find_or_build_runtime_archive`] never substitutes one mode for the
//! other: an unsupported `--native-target` or a missing toolchain is a
//! reported error (see
//! `packaging/toolchains/runtime-archive-contract.json`'s freestanding
//! `supported_targets`, currently `windows-x86_64`/`linux-x86_64`).
//! Freestanding linking additionally needs, beyond what
//! [`read_link_flags`] pulls from the archive's manifest: `-nostdlib`
//! (the archive itself provides the platform entry point — `_start`/
//! `mainCRTStartup`, see `deps/laststanding/l_os.h` — so the toolchain's
//! own default CRT startup/libraries must not also be linked, which
//! would at best conflict with a duplicate `main`-calling entry symbol
//! and at worst reintroduce a libc dependency); `--gc-sections` (each
//! freestanding archive is one large translation unit covering every
//! runtime feature it includes — sockets, TLS, etc. — so discarding the
//! object code a given program never calls mostly depends on
//! section-level garbage collection rather than archive-member
//! selection; see "Freestanding runtime profiles" below for the one
//! exception); and re-linking the compiler's own support library
//! (`-print-libgcc-file-name`, portable across GCC and Clang) since
//! `-nostdlib` also drops it, and a handful of runtime helpers (e.g.
//! Windows' stack-probing `__chkstk_ms`) live there rather than in the
//! Oscan runtime itself.
//!
//! # Windows import-library minimization
//!
//! `--gc-sections` alone is not sufficient to keep a freestanding
//! Windows executable's DLL imports minimal, for two separate reasons
//! (see `detect_windows_feature_libs`'s docs for the second one):
//!
//! 1. GCC/MinGW can lower a `switch` into a jump table stored in a
//!    generic, shared `.rdata`/`.rodata` section rather than one scoped
//!    to the owning function's own COMDAT, even under `-ffunction-sections
//!    -fdata-sections`. Once *any* live code references *any* entry in
//!    that shared blob, the whole blob — including jump-table entries
//!    for entirely unrelated, otherwise-dead switch statements (e.g. the
//!    Win32 window procedure's message dispatch) — is kept live, which
//!    transitively re-anchors those unrelated functions' own Win32 calls.
//!    `compile_shim_object` and `scripts/release_tools.py`'s freestanding
//!    compile flags both pass `-fno-jump-tables` to avoid this.
//! 2. Even with (1) fixed, GNU ld resolves the `-l`-specified import
//!    libraries against undefined symbols from the objects/archive
//!    members it has *already* decided to include, and does so before
//!    `--gc-sections` finalizes which of those objects' sections survive
//!    into the output. So a dead function that merely *mentions* e.g.
//!    `SelectObject` still causes `libgdi32.a`'s stub for it to be pulled
//!    in and kept, even though the calling code is later stripped —
//!    `--gc-sections` does not retroactively "un-pull" an import-library
//!    member once resolved. [`detect_windows_feature_libs`] avoids this
//!    by only ever passing the optional `-lws2_32`/`-luser32`/`-lgdi32`/
//!    `-lsecur32`/`-lcrypt32` flags when the compiled program's object
//!    actually references a runtime symbol from that feature area;
//!    `-lkernel32` is unconditional (every freestanding program needs it).
//!    LLD has the complementary constraint: it diagnoses undefined imports
//!    in dead sections before section GC, so every optional import library
//!    must be present while resolving a Clang-built runtime archive. LLD then
//!    garbage-collects the unused import thunks, preserving the same minimal
//!    final DLL dependency set. The linker-family branch in
//!    [`link_executable`] captures that validated difference explicitly.
//!
//! # Freestanding runtime profiles
//!
//! `--gc-sections` normally handles dead-code elimination for a given
//! freestanding program well enough on its own (see above) — with one
//! measured exception. `osc_runtime_freestanding.c` compiles the core
//! runtime *and* the graphics/image/SVG/TrueType feature libraries
//! (`l_gfx.h`/`l_img.h`/`l_svg.h`/`l_tt.h`) as one translation unit, and a
//! Clang `-Oz` build of it puts a floating-point constant pool — curve-
//! flattening/trig tables the graphics code needs — into a single,
//! non-COMDAT `.rdata` input section shared by the whole file, rather
//! than one scoped per function/global the way `-ffunction-sections
//! -fdata-sections` scopes ordinary code and data. `--gc-sections` can
//! only discard a section as a whole, so once *anything* in the archive
//! keeps that pool reachable, it survives whole in the final executable
//! — roughly 2 KiB dead weight in `hello.osc`, which never calls a
//! graphics builtin at all, and was the largest single contributor to
//! native's executable size versus the C backend's for that program
//! (see `native-size-profiles`'s measurements).
//!
//! Rather than a heuristic that only trims this specific pool,
//! `runtime/osc_runtime_freestanding_core.c` is a second, sibling
//! translation unit — the exact same preamble minus the `l_gfx.h`/
//! `l_img.h`/`l_svg.h`/`l_tt.h` block and its `OSC_HAS_GFX`/`OSC_HAS_IMG`/
//! `OSC_HAS_SVG`/`OSC_HAS_TT` defines — compiled into a second archive,
//! `libosc_runtime_freestanding_core.a` (see
//! `packaging/toolchains/runtime-archive-contract.json`'s
//! `freestanding_core` mode). Since it is a wholly separate archive
//! (rather than a second member of the same one — the two would define
//! the same core symbols, e.g. `osc_arena_create`, and the linker would
//! reject that as a duplicate), ordinary archive selection (not section
//! GC, and not a symbol-by-symbol capability system) keeps the pool out
//! of any executable linked against it. [`program_needs_graphics_runtime`]
//! selects between the two archives per program by scanning the
//! compiled object's own undefined symbols for the graphics-only
//! `osc_gfx_*`/`osc_canvas_*`/`osc_clipboard_*`/`osc_img_*`/`osc_svg_*`/
//! `osc_tt_*` prefixes, exactly as [`detect_windows_feature_libs`] already
//! does for import libraries; core (arena/strings/panic/print/file/
//! process/env/maps), sockets, and TLS are unaffected and identical in
//! both archives, since they neither call into nor are called from the
//! graphics feature libraries (verified: no cross-references either
//! way). [`FreestandingProfile::Core`] is only ever chosen when that scan
//! is both possible and negative — an unparseable object, or any
//! unscanned `extra_c_files`, conservatively falls back to
//! [`FreestandingProfile::Full`], the strict superset, so this can never
//! omit a symbol a program actually needs, including one reached only
//! indirectly through another runtime function.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use object::{Object, ObjectSymbol};

use crate::CompilerSource;

use super::target::NativeTarget;
use super::RuntimeMode;

/// Inputs that affect final native linking but not Cranelift object emission.
pub struct NativeLinkOptions<'a> {
    pub runtime_mode: RuntimeMode,
    pub show_warnings: bool,
    pub extra_c_files: &'a [String],
    pub extra_cflags: &'a [String],
}

fn is_verbose() -> bool {
    crate::is_verbose()
}

pub fn write_object_file(bytes: &[u8], path: &Path) -> Result<(), String> {
    fs::write(path, bytes)
        .map_err(|e| format!("error writing object file '{}': {e}", path.display()))
}

/// Candidate repository/checkout roots to search for `native-runtime/`,
/// `runtime/`, and `build/runtime-archives/`: ancestors of the running
/// `oscan` binary (covering both an installed release bundle and a
/// `target/{debug,release}/oscan(.exe)` dev layout), then the compile-time
/// `CARGO_MANIFEST_DIR`, then the current directory as a last resort.
///
/// Executable-relative roots come first so an installed release always uses
/// its packaged runtime assets rather than a checkout that merely happens to
/// remain at the path where that compiler binary was built.
fn repo_root_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(exe) = env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..4 {
            if let Some(d) = dir {
                v.push(d.clone());
                dir = d.parent().map(|p| p.to_path_buf());
            } else {
                break;
            }
        }
    }
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        v.push(PathBuf::from(manifest_dir));
    }
    v.push(PathBuf::from("."));
    v
}

fn find_runtime_source_dir() -> Option<PathBuf> {
    for base in repo_root_candidates() {
        for directory in ["native-runtime", "runtime"] {
            let candidate = base.join(directory);
            if candidate.join("osc_native_shim.c").is_file()
                && candidate.join("osc_runtime.h").is_file()
            {
                return Some(candidate);
            }
        }
    }
    None
}

fn find_release_tools_script() -> Option<PathBuf> {
    repo_root_candidates()
        .into_iter()
        .map(|base| base.join("scripts").join("release_tools.py"))
        .find(|p| p.is_file())
}

const FREESTANDING_ARCHIVE_NAME: &str = "libosc_runtime_freestanding.a";
const FREESTANDING_CORE_ARCHIVE_NAME: &str = "libosc_runtime_freestanding_core.a";
const HOSTED_ARCHIVE_NAME: &str = "libosc_runtime_hosted.a";

/// Which freestanding runtime archive to link against. Hosted mode only
/// ever has one archive; freestanding mode has two (see this module's
/// "Freestanding runtime profiles" docs above): [`Full`](Self::Full)
/// (`libosc_runtime_freestanding.a`, everything, including graphics/
/// image/SVG/TrueType) and [`Core`](Self::Core)
/// (`libosc_runtime_freestanding_core.a`, the same runtime minus those
/// feature libraries). [`program_needs_graphics_runtime`] decides between
/// them per program; hosted mode ignores this entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FreestandingProfile {
    Full,
    Core,
}

impl FreestandingProfile {
    /// The `--mode` value `scripts/release_tools.py build-runtime-archive`
    /// expects (see `packaging/toolchains/runtime-archive-contract.json`'s
    /// `modes` map) — distinct from [`RuntimeMode::as_str`], which is a
    /// user-facing "freestanding"/"hosted" label, not an archive variant.
    fn build_mode_str(self) -> &'static str {
        match self {
            Self::Full => "freestanding",
            Self::Core => "freestanding_core",
        }
    }
}

fn archive_name(runtime_mode: RuntimeMode, profile: FreestandingProfile) -> &'static str {
    match runtime_mode {
        RuntimeMode::Freestanding => match profile {
            FreestandingProfile::Full => FREESTANDING_ARCHIVE_NAME,
            FreestandingProfile::Core => FREESTANDING_CORE_ARCHIVE_NAME,
        },
        RuntimeMode::Hosted => HOSTED_ARCHIVE_NAME,
    }
}

/// Locate a pre-built runtime archive for `target`/`runtime_mode`, building
/// it on demand via `scripts/release_tools.py build-runtime-archive` (the
/// same tool `scripts/build-runtime-archive.ps1`/`.sh` wrap) when none is
/// found and a Python interpreter + runtime sources are available. It never
/// falls back to the other `RuntimeMode`. `profile` only affects
/// `RuntimeMode::Freestanding` (see [`FreestandingProfile`]); hosted mode
/// ignores it.
fn find_or_build_runtime_archive(
    target: NativeTarget,
    runtime_mode: RuntimeMode,
    profile: FreestandingProfile,
) -> Result<PathBuf, String> {
    let archive_name = archive_name(runtime_mode, profile);
    if let Ok(dir) = env::var("OSCAN_RUNTIME_ARCHIVE_DIR") {
        let p = PathBuf::from(&dir).join(archive_name);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!(
                "OSCAN_RUNTIME_ARCHIVE_DIR='{dir}' is set, but the requested {runtime_mode} runtime archive '{}' does not exist",
                p.display(),
            ))
        };
    }

    for base in repo_root_candidates() {
        let p = base
            .join("build")
            .join("runtime-archives")
            .join(target.archive_tag())
            .join(archive_name);
        if p.is_file() {
            if is_verbose() {
                eprintln!("[verbose] Using existing runtime archive: {}", p.display());
            }
            return Ok(p);
        }
    }

    let script = find_release_tools_script().ok_or_else(|| {
        format!(
            "no {runtime_mode} Oscan runtime archive found (build/runtime-archives/<target>/{archive_name}) \
             and scripts/release_tools.py was not found to build one; run scripts/build-runtime-archive.ps1|.sh \
             -Mode {runtime_mode} first, or set OSCAN_RUNTIME_ARCHIVE_DIR to a directory containing {archive_name}"
        )
    })?;
    let repo_root = script
        .parent()
        .and_then(|p| p.parent())
        .expect("scripts/release_tools.py always has a repo-root grandparent")
        .to_path_buf();
    let out_dir = repo_root
        .join("build")
        .join("runtime-archives")
        .join(target.archive_tag());

    let build_mode = match runtime_mode {
        RuntimeMode::Freestanding => profile.build_mode_str(),
        RuntimeMode::Hosted => runtime_mode.as_str(),
    };
    eprintln!(
        "note: building the {runtime_mode} Oscan runtime archive for '{}' (first native-backend build)...",
        target.archive_tag()
    );
    let python = find_python_interpreter()
        .ok_or_else(|| "no working Python interpreter found (tried python, python3) to build the runtime archive".to_string())?;
    let mut cmd = Command::new(python);
    cmd.arg(&script)
        .arg("build-runtime-archive")
        .arg("--mode")
        .arg(build_mode)
        .arg("--target")
        .arg(target.archive_tag())
        .arg("--out-dir")
        .arg(&out_dir);
    if is_verbose() {
        eprintln!("[verbose] {:?}", cmd);
    }
    let output = cmd.output().map_err(|e| {
        format!(
            "failed to run '{python} {}' to build the runtime archive: {e}",
            script.display()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "building the {runtime_mode} Oscan runtime archive for '{}' failed (this target may not be \
             supported by that runtime-archive mode — see \
             packaging/toolchains/runtime-archive-contract.json):\n{}{}",
            target.archive_tag(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let p = out_dir.join(archive_name);
    if p.is_file() {
        Ok(p)
    } else {
        Err(format!(
            "scripts/release_tools.py reported success but '{}' does not exist",
            p.display()
        ))
    }
}

/// Find a Python interpreter that actually runs `--version` successfully
/// — not just one that `where`/`which` reports as present. This matters
/// specifically on Windows, where `python3.exe`/`python.exe` can both
/// resolve via a PATH "app execution alias" stub that does nothing but
/// print a Microsoft Store prompt and exit non-zero, so existence alone
/// (`command_exists`) is not sufficient evidence the interpreter works.
fn find_python_interpreter() -> Option<&'static str> {
    for candidate in ["python", "python3"] {
        let ok = Command::new(candidate)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Some(candidate);
        }
    }
    None
}

/// A linker driver discovered on this host: GCC/Clang only (see module
/// docs for why MSVC is not supported here). Its identity matters beyond
/// "does it work": it must be toolchain-*compatible* with whatever built
/// the runtime archive (a MinGW-GCC-built static archive's object members
/// expect MinGW's CRT/import-library naming, e.g. `__mingw_vfprintf`,
/// `_open`/`_unlink`, `___chkstk_ms`, and WinSock imports resolved via
/// `-lws2_32` — linking those against an MSVC-mode Clang/`link.exe`
/// fails with dozens of unresolved externals). So this prefers, in
/// order: an explicit override, the *exact* compiler recorded in the
/// archive's own build manifest (guaranteeing a match), and only then
/// falls back to normal compiler discovery after checking its family,
/// version, and target against the archive provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkerFamily {
    GnuLd,
    Lld,
}

struct LinkerDriver {
    cmd: String,
    source: CompilerSource,
    linker_family: LinkerFamily,
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

fn command_is_available(cmd: &str) -> bool {
    Path::new(cmd).is_file() || command_exists(cmd)
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

fn linker_family_for(
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

fn find_linker_driver(archive: &Path, target: NativeTarget) -> Result<LinkerDriver, String> {
    let manifest = read_manifest(archive);
    if let Ok(over) = env::var("OSCAN_NATIVE_LINKER") {
        if !over.trim().is_empty() {
            return Ok(LinkerDriver {
                linker_family: linker_family_for(&over, target, None),
                cmd: over,
                source: CompilerSource::Override,
            });
        }
    }
    if let Some(cc) = manifest.as_ref().and_then(|m| m.cc.clone()) {
        if command_is_available(&cc) {
            validate_manifest_driver(&cc, manifest.as_ref().expect("manifest exists"))?;
            return Ok(LinkerDriver {
                linker_family: linker_family_for(&cc, target, manifest.as_ref()),
                cmd: cc,
                source: CompilerSource::Host,
            });
        }
    }
    match crate::find_c_compiler() {
        Some(compiler) => match crate::gcc_or_clang_cmd(&compiler) {
            Some((cmd, source)) => {
                if let Some(manifest) = manifest.as_ref() {
                    validate_manifest_driver(cmd, manifest).map_err(|error| {
                        format!(
                            "the compiler recorded in runtime archive '{}' is unavailable, and the discovered \
                             replacement is incompatible: {error}; install/use the packaged matching toolchain \
                             or set OSCAN_NATIVE_LINKER explicitly",
                            archive.display()
                        )
                    })?;
                }
                Ok(LinkerDriver {
                    cmd: cmd.to_string(),
                    source,
                    linker_family: linker_family_for(cmd, target, manifest.as_ref()),
                })
            }
            None => Err(
                "the native backend links object files with GCC or Clang (matching the toolchain used to \
                 build the runtime archive), but only an MSVC (cl.exe) toolchain was found on this host; \
                 install GCC or Clang (e.g. MinGW-w64, or LLVM), or set OSCAN_NATIVE_LINKER to a GCC/Clang \
                 command"
                    .to_string(),
            ),
        },
        None => {
            let recorded = manifest
                .as_ref()
                .and_then(|m| m.cc.as_deref())
                .map(|cc| format!("; the archive records unavailable compiler '{cc}'"))
                .unwrap_or_default();
            Err(format!(
                "no C compiler found to act as the native backend's linker (searched the same way --backend c does){recorded}; \
                 install GCC or a GNU-ABI Clang toolchain"
            ))
        }
    }
}

fn command_exists(cmd: &str) -> bool {
    crate::command_exists(cmd)
}

/// Compile `runtime/osc_native_shim.c` with flags matching `runtime_mode`,
/// caching a separate object for every mode/compiler pair.
fn compile_shim_object(
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
                // See this module's docs ("Windows import-library
                // minimization"): a switch's jump table can otherwise land
                // in a shared, non-function-scoped section that keeps
                // unrelated dead code (and its Win32 imports) alive.
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

#[derive(Debug, Default, PartialEq)]
struct RuntimeArchiveManifest {
    cc: Option<String>,
    cc_family: Option<String>,
    cc_version: Option<String>,
    cc_target: Option<String>,
    linker_family: Option<String>,
    link_flags: Vec<String>,
}

fn parse_runtime_manifest(text: &str) -> Option<RuntimeArchiveManifest> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;

    let cc = match object.get("cc") {
        Some(serde_json::Value::String(value)) if !value.is_empty() => Some(value.clone()),
        Some(serde_json::Value::String(_)) | None => None,
        Some(_) => return None,
    };
    let toolchain = object
        .get("toolchain")
        .and_then(serde_json::Value::as_object);
    let compiler = toolchain
        .and_then(|value| value.get("compiler"))
        .and_then(serde_json::Value::as_object);
    let linker = toolchain
        .and_then(|value| value.get("linker"))
        .and_then(serde_json::Value::as_object);
    let cc_family = compiler
        .and_then(|value| value.get("family"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let cc_version = compiler
        .and_then(|value| value.get("version"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let cc_target = compiler
        .and_then(|value| value.get("target"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| object.get("cc_target").and_then(serde_json::Value::as_str))
        .map(str::to_owned);
    let linker_family = linker
        .and_then(|value| value.get("family"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let link_flags = match object.get("link_flags") {
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .map(|value| value.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()?,
        None => Vec::new(),
        Some(_) => return None,
    };

    Some(RuntimeArchiveManifest {
        cc,
        cc_family,
        cc_version,
        cc_target,
        linker_family,
        link_flags,
    })
}

/// Read the `link_flags` out of the runtime archive's build manifest
/// (`<archive-stem>.json`, written by `scripts/release_tools.py`'s
/// `build_runtime_archive`), e.g. the Win32 import libraries on Windows.
/// Best-effort: a missing, unreadable, or malformed manifest contributes no
/// extra flags rather than failing the link.
fn read_link_flags(archive_path: &Path) -> Vec<String> {
    read_manifest(archive_path)
        .map(|manifest| manifest.link_flags)
        .unwrap_or_default()
}

/// The runtime archive's build manifest sits next to it with the same
/// stem (`libosc_runtime_freestanding.a` -> `libosc_runtime_freestanding.json`),
/// matching `scripts/release_tools.py`'s `build_runtime_archive` (its
/// `mode_spec["manifest_name"]` is always `{archive stem}.json`).
fn read_manifest(archive_path: &Path) -> Option<RuntimeArchiveManifest> {
    let manifest_path = archive_path.with_extension("json");
    let text = fs::read_to_string(&manifest_path).ok()?;
    parse_runtime_manifest(&text)
}

/// Ask `cc` (a flag both GCC and Clang understand) where its own compiler
/// support library lives — `libgcc.a` for GCC, `libclang_rt.builtins-*.a`
/// for Clang — so it can be explicitly re-linked after `-nostdlib` (which
/// drops it along with the default CRT/libraries). The freestanding
/// runtime archive's own object code can call low-level helpers that live
/// there rather than in the Oscan runtime itself (e.g. Windows' x86-64
/// stack-probing `__chkstk_ms`, needed for `mainCRTStartup`'s
/// variable-length command-line buffer). Returns `None` if the compiler
/// doesn't resolve this to a real, existing file (e.g. a Clang not
/// targeting a GNU/MinGW environment just echoes the literal fallback
/// name `libgcc.a` without ever finding one) — the caller then leaves it
/// off the link rather than passing a bogus path.
fn find_compiler_builtins_lib(cc: &str) -> Option<PathBuf> {
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
fn detect_windows_feature_libs(object_path: &Path) -> Vec<&'static str> {
    let all = vec!["-lws2_32", "-luser32", "-lgdi32", "-lsecur32", "-lcrypt32"];
    let Ok(data) = fs::read(object_path) else {
        return all;
    };
    let Ok(file) = object::File::parse(&*data) else {
        return all;
    };

    let (mut needs_sockets, mut needs_tls, mut needs_windowing) = (false, false, false);
    for symbol in file.symbols() {
        if !symbol.is_undefined() {
            continue;
        }
        let Ok(name) = symbol.name() else { continue };
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
        libs.push("-lws2_32");
    }
    if needs_tls {
        libs.push("-lsecur32");
        libs.push("-lcrypt32");
    }
    if needs_windowing {
        libs.push("-luser32");
        libs.push("-lgdi32");
    }
    libs
}

/// Whether a *freestanding* program needs the full
/// (`libosc_runtime_freestanding.a`) runtime archive rather than the
/// smaller `libosc_runtime_freestanding_core.a` sibling that omits
/// graphics/image/SVG/TrueType (see this module's "Freestanding runtime
/// profiles" docs above and `runtime/osc_runtime_freestanding_core.c`).
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
fn program_needs_graphics_runtime(object_path: &Path) -> bool {
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
    file.symbols().any(|symbol| {
        symbol.is_undefined()
            && symbol
                .name()
                .is_ok_and(|name| GRAPHICS_PREFIXES.iter().any(|prefix| name.starts_with(prefix)))
    })
}

/// Link `object_path`, the native shim, optional user C sources, and the
/// explicitly selected runtime archive into an executable at `exe_path`.
pub fn link_executable(
    object_path: &Path,
    exe_path: &Path,
    target: NativeTarget,
    options: &NativeLinkOptions<'_>,
) -> Result<(), String> {
    if !target.is_host() {
        return Err(format!(
            "'{}' is not the host target ({}); this backend only links locally for the host target today \
            (object emission for other targets works with -o *.o/*.obj, but finishing the link needs a \
             matching cross linker and runtime archive, which this environment does not have configured)",
            target,
            NativeTarget::host()
        ));
    }

    let profile = if options.runtime_mode == RuntimeMode::Freestanding
        && options.extra_c_files.is_empty()
        && !program_needs_graphics_runtime(object_path)
    {
        // No extra (unscannable) user C sources, and the compiled program
        // itself has no undefined graphics/image/SVG/TrueType symbol: the
        // smaller core archive is a complete, correct link for it.
        FreestandingProfile::Core
    } else {
        FreestandingProfile::Full
    };
    let archive = find_or_build_runtime_archive(target, options.runtime_mode, profile)?;
    let driver = find_linker_driver(&archive, target)?;
    let shim_obj = compile_shim_object(&driver.cmd, target, options.runtime_mode)?;
    let mut link_flags = read_link_flags(&archive);
    if target == NativeTarget::WindowsX64 {
        let mut needed = vec!["-lkernel32"];
        match options.runtime_mode {
            // Hosted mode always links every optional Win32 import library:
            // GCC does not honor MSVC's `#pragma comment(lib, ...)`, hosted
            // executables already carry a full CRT/UCRT dependency, and they
            // are not checked for minimal DLL imports (see test.ps1's
            // `Test-WindowsFreestanding`, which only runs against freestanding
            // builds), so there is nothing to gain by scanning for them here.
            RuntimeMode::Hosted => {
                needed.extend(["-lws2_32", "-luser32", "-lgdi32", "-lsecur32", "-lcrypt32"]);
            }
            // Freestanding executables *are* dependency-checked, so only
            // request the optional libraries this specific program's object
            // actually needs (see detect_windows_feature_libs's docs).
            // Extra user-supplied C sources aren't scanned (they're compiled
            // after this point, and could call Win32 APIs directly under
            // names this scan doesn't know about), so conservatively request
            // everything when any are present rather than risk an
            // unresolved-symbol link error. LLD likewise needs every import
            // library available during resolution; its later section GC still
            // removes unused import thunks and DLL dependencies.
            RuntimeMode::Freestanding => {
                if options.extra_c_files.is_empty() && driver.linker_family != LinkerFamily::Lld {
                    needed.extend(detect_windows_feature_libs(object_path));
                } else {
                    needed.extend(["-lws2_32", "-luser32", "-lgdi32", "-lsecur32", "-lcrypt32"]);
                }
            }
        }
        for library in needed {
            if !link_flags.iter().any(|flag| flag == library) {
                link_flags.push(library.to_string());
            }
        }
    }

    let mut cmd = Command::new(&driver.cmd);
    if driver.linker_family == LinkerFamily::Lld {
        // llvm-mingw's Clang normally defaults to LLD, but make the validated
        // archive/link contract explicit rather than inheriting host config.
        cmd.arg("-fuse-ld=lld");
    }
    match options.runtime_mode {
        RuntimeMode::Freestanding => {
            // Do not admit the toolchain's CRT/default libraries. The runtime
            // archive supplies `_start`/`mainCRTStartup`.
            cmd.arg("-nostdlib")
                // Match the C backend's release behavior: final executables do
                // not retain COFF/ELF symbols or debug sections. Object-only
                // output remains untouched for debugging and inspection.
                .arg("-s")
                .arg("-Wl,--gc-sections,--build-id=none");
            if target != NativeTarget::WindowsX64 {
                // Cranelift emits non-PIC objects (see target.rs). The
                // archive's own `-static` link flag is not sufficient on
                // every toolchain: e.g. the bundled musl-cross-make GCC is
                // itself built with `--enable-default-pie
                // --enable-static-pie`, so plain `-static` still produces a
                // static-PIE executable (verified: `file` reports
                // "static-pie linked" without this flag) — and linking
                // non-PIC objects into any PIE, static or not, fails with
                // "relocation ... in read-only section `.text'" /
                // "read-only segment has dynamic relocations". `-no-pie`
                // forces the traditional, non-relocatable ET_EXEC layout
                // Cranelift's absolute-addressed code actually needs,
                // regardless of a given toolchain's own PIE defaults.
                cmd.arg("-no-pie");
            }
            if !options.extra_c_files.is_empty() {
                cmd.arg("-std=gnu11")
                    .arg("-ffreestanding")
                    .arg("-fno-builtin");
            }
        }
        RuntimeMode::Hosted => {
            // Keep the normal driver-provided CRT and system libraries. Section
            // GC is still useful because the runtime archive is one large
            // translation unit compiled with per-function/data sections.
            cmd.arg("-Wl,--gc-sections");
            if target != NativeTarget::WindowsX64 {
                // Cranelift emits non-PIC objects (see target.rs); many Linux
                // distributions default GCC to PIE, which otherwise creates
                // text relocations in the executable.
                cmd.arg("-no-pie");
            }
            if !options.extra_c_files.is_empty() {
                cmd.arg("-std=c99").arg("-O2");
            }
        }
    }
    if !options.show_warnings && !options.extra_c_files.is_empty() {
        cmd.arg("-w");
    }
    if !options.extra_c_files.is_empty() {
        if let Some(runtime_dir) = find_runtime_source_dir() {
            cmd.arg(format!("-I{}", runtime_dir.display()));
            for include_dir in crate::find_extra_include_dirs(&runtime_dir) {
                cmd.arg(format!("-I{}", include_dir.display()));
            }
        }
    }

    cmd.arg(object_path).arg(&shim_obj);
    // User C translation units precede the runtime archive so any runtime
    // symbols they call are resolved in the same left-to-right static link.
    for source in options.extra_c_files {
        cmd.arg(source);
    }
    cmd.arg(&archive).arg("-o").arg(exe_path);
    for flag in &link_flags {
        cmd.arg(flag);
    }
    // Preserve the C backend's repeatable, one-argument-per-flag semantics;
    // appending also keeps user-provided `-l...` libraries after all objects.
    for flag in options.extra_cflags {
        cmd.arg(flag);
    }
    // Re-link the compiler's own support library after -nostdlib dropped
    // it (see find_compiler_builtins_lib's docs). Must come after the
    // archive/objects that reference symbols from it.
    if options.runtime_mode == RuntimeMode::Freestanding && target == NativeTarget::WindowsX64 {
        if let Some(builtins) = find_compiler_builtins_lib(&driver.cmd) {
            cmd.arg(&builtins);
        }
    }

    eprintln!(
        "Linking {mode} executable with {} ({})...",
        driver.cmd,
        crate::compiler_source_label(driver.source),
        mode = options.runtime_mode,
    );
    if is_verbose() {
        eprintln!("[verbose] {:?}", cmd);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("failed to run '{}': {e}", driver.cmd))?;
    if !output.status.success() {
        return Err(format!(
            "linking failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_modes_select_distinct_archives() {
        assert_eq!(
            archive_name(RuntimeMode::Freestanding, FreestandingProfile::Full),
            "libosc_runtime_freestanding.a"
        );
        assert_eq!(
            archive_name(RuntimeMode::Freestanding, FreestandingProfile::Core),
            "libosc_runtime_freestanding_core.a"
        );
        assert_eq!(
            archive_name(RuntimeMode::Hosted, FreestandingProfile::Full),
            "libosc_runtime_hosted.a"
        );
        // Hosted mode ignores the profile entirely — there is only one
        // hosted archive regardless of whether a program uses graphics.
        assert_eq!(
            archive_name(RuntimeMode::Hosted, FreestandingProfile::Core),
            "libosc_runtime_hosted.a"
        );
    }

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
    fn runtime_manifest_preserves_comma_containing_linker_flags() {
        let manifest = parse_runtime_manifest(
            r#"{
                "cc": "x86_64-linux-musl-gcc",
                "link_flags": [
                    "-nostdlib",
                    "-static",
                    "-Wl,--gc-sections,--build-id=none"
                ]
            }"#,
        )
        .expect("valid runtime manifest");

        assert_eq!(
            manifest.link_flags,
            vec!["-nostdlib", "-static", "-Wl,--gc-sections,--build-id=none"]
        );
    }

    #[test]
    fn runtime_manifest_is_parsed_as_json() {
        let manifest = parse_runtime_manifest(
            r#"{
                "cc":"C:\\toolchains\\clang.exe",
                "toolchain":{
                    "compiler":{
                        "family":"clang",
                        "version":"clang version 22.1.2",
                        "target":"x86_64-w64-windows-gnu"
                    },
                    "linker":{"family":"lld"}
                },
                "link_flags":["-Wl,\"quoted\""]
            }"#,
        )
        .expect("valid escaped JSON");

        assert_eq!(manifest.cc.as_deref(), Some(r"C:\toolchains\clang.exe"));
        assert_eq!(manifest.cc_family.as_deref(), Some("clang"));
        assert_eq!(manifest.cc_version.as_deref(), Some("clang version 22.1.2"));
        assert_eq!(
            manifest.cc_target.as_deref(),
            Some("x86_64-w64-windows-gnu")
        );
        assert_eq!(manifest.linker_family.as_deref(), Some("lld"));
        assert_eq!(manifest.link_flags, vec![r#"-Wl,"quoted""#]);
        assert!(parse_runtime_manifest(r#"{"link_flags":["unterminated]}"#).is_none());
    }

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
}
