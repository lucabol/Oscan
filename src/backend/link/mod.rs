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
//! reported tooling gap. This is the legacy
//! [`plan::LinkerFlavor::CompilerDriver`] flavor's story; see "Direct linker
//! & embedding" below for the freestanding direct-link paths.
//!
//! # Explicit runtime modes
//!
//! [`link_executable`] receives an explicit [`RuntimeMode`]. The default
//! CLI path passes `Freestanding`, keeping `--backend native` standalone
//! and libc-free; only `--libc --backend native` passes `Hosted`, which
//! selects the hosted archive and normal CRT/libm/system linking.
//! [`archive::find_or_build_runtime_archive`] never substitutes one mode
//! for the other: an unsupported `--native-target` or a missing toolchain
//! is a reported error (see
//! `packaging/toolchains/runtime-archive-contract.json`'s mode-specific
//! `supported_targets`).
//! Freestanding linking additionally needs, beyond `-lkernel32`/`-lm`
//! (hardcoded per-target/mode in `build_compiler_driver_plan`, deliberately
//! *not* read from the archive manifest's `link_flags` — see
//! `LinkPlan::static_link`'s doc comment, security review 2026-07-15):
//! `-nostdlib` (the archive itself provides the platform entry point —
//! `_start`/`mainCRTStartup`, see `deps/laststanding/l_os.h` — so the
//! toolchain's own default CRT startup/libraries must not also be linked,
//! which would at best conflict with a duplicate `main`-calling entry
//! symbol and at worst reintroduce a libc dependency; only rendered for
//! the `CompilerDriver` flavor — `ld.lld` never links a CRT anyway);
//! `--gc-sections` (each freestanding archive is one large translation
//! unit covering every runtime feature it includes — sockets, TLS, etc. —
//! so discarding the object code a given program never calls mostly
//! depends on section-level garbage collection rather than archive-member
//! selection; see "Freestanding runtime profiles" below for the one
//! exception); and re-linking the compiler's own support library
//! (`-print-libgcc-file-name`, portable across GCC and Clang, or the
//! embedded `libclang_rt.builtins-x86_64.a` for `MingwDirect`) since
//! `-nostdlib` also drops it, and a handful of runtime helpers (e.g.
//! Windows' stack-probing `__chkstk_ms`) live there rather than in the
//! Oscan runtime itself.
//!
//! # Windows import-library minimization
//!
//! `--gc-sections` alone is not sufficient to keep a freestanding
//! Windows executable's DLL imports minimal, for two separate reasons
//! (see [`capability::detect_windows_feature_libs`]'s docs for the second
//! one):
//!
//! 1. GCC/MinGW can lower a `switch` into a jump table stored in a
//!    generic, shared `.rdata`/`.rodata` section rather than one scoped
//!    to the owning function's own COMDAT, even under `-ffunction-sections
//!    -fdata-sections`. Once *any* live code references *any* entry in
//!    that shared blob, the whole blob — including jump-table entries
//!    for entirely unrelated, otherwise-dead switch statements (e.g. the
//!    Win32 window procedure's message dispatch) — is kept live, which
//!    transitively re-anchors those unrelated functions' own Win32 calls.
//!    [`driver::compile_shim_object`] and `scripts/release_tools.py`'s
//!    freestanding compile flags both pass `-fno-jump-tables` to avoid
//!    this.
//! 2. Even with (1) fixed, GNU ld resolves the `-l`-specified import
//!    libraries against undefined symbols from the objects/archive
//!    members it has *already* decided to include, and does so before
//!    `--gc-sections` finalizes which of those objects' sections survive
//!    into the output. So a dead function that merely *mentions* e.g.
//!    `SelectObject` still causes `libgdi32.a`'s stub for it to be pulled
//!    in and kept, even though the calling code is later stripped —
//!    `--gc-sections` does not retroactively "un-pull" an import-library
//!    member once resolved. [`capability::detect_windows_feature_libs`]
//!    avoids this for the `CompilerDriver` flavor by only ever requesting
//!    the optional `ws2_32`/`user32`/`gdi32`/`secur32`/`crypt32` libraries
//!    when the compiled program's object actually references a runtime
//!    symbol from that feature area; `kernel32` is unconditional (every
//!    freestanding program needs it). LLD has the complementary
//!    constraint: it diagnoses undefined imports in dead sections before
//!    section GC, so every optional import library must be present while
//!    resolving a Clang-built runtime archive — the `MingwDirect` flavor
//!    therefore *always* requests all five optional libraries (the
//!    "LLD-sees-all-optional-imports rule", design §2.4), and so does the
//!    `CompilerDriver` flavor whenever its discovered driver is
//!    Clang/LLD. LLD then garbage-collects the unused import thunks,
//!    preserving the same minimal final DLL dependency set.
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
//! of any executable linked against it. [`capability::program_needs_graphics_runtime`]
//! selects between the two archives per program by scanning the
//! compiled object's own undefined symbols for the graphics-only
//! `osc_gfx_*`/`osc_canvas_*`/`osc_clipboard_*`/`osc_img_*`/`osc_svg_*`/
//! `osc_tt_*` prefixes, exactly as [`capability::detect_windows_feature_libs`]
//! already does for import libraries; core (arena/strings/panic/print/
//! file/process/env/maps), sockets, and TLS are unaffected and identical
//! in both archives, since they neither call into nor are called from
//! the graphics feature libraries (verified: no cross-references either
//! way). `FreestandingProfile::Core` is only ever chosen when that scan
//! is both possible and negative — an unparseable object, or any
//! unscanned `extra_c_files`, conservatively falls back to
//! `FreestandingProfile::Full`, the strict superset, so this can never
//! omit a symbol a program actually needs, including one reached only
//! indirectly through another runtime function.
//!
//! # Direct linker & embedding (freestanding native targets)
//!
//! Implements `docs/design/native-link-embedding.md`. In freestanding mode,
//! with no explicit user `.c` files, [`link_executable`] selects a direct
//! linker when the target has matching packaged assets:
//!
//! * Windows x86-64 uses [`plan::LinkerFlavor::MingwDirect`] with `ld.lld`
//!   plus embedded import libraries and compiler-builtins.
//! * Linux x86-64 uses [`plan::LinkerFlavor::ElfDirect`] with the embedded
//!   static musl GNU linker.
//! * Linux AArch64/RISC-V64 use [`plan::LinkerFlavor::ElfDirect`] with a
//!   target-matched sidecar linker and runtime archive.
//!
//! The native shim (`runtime/osc_native_shim.c`) is precompiled into each
//! runtime archive (`archive::ShimSource::ArchiveMember`), so these direct
//! paths need no C frontend during downstream compilation.
//!
//! Hosted mode, explicit `.c` files, and development builds without matching
//! direct-link assets keep the diagnosed [`plan::LinkerFlavor::CompilerDriver`]
//! path. A standard release embeds only its own host linker; cross-target
//! direct linking therefore requires the documented sidecar assets rather
//! than silently falling back to a C compiler.
//!
//! [`driver::resolve_linker_selection`] implements the exact
//! `OSCAN_NATIVE_LINKER`/`OSCAN_NATIVE_LINKER_FLAVOR` migration/selection
//! table (design §7.2), and [`driver::no_silent_fallback_error`] is the
//! **only** message produced when an embedded-assets build's extraction or
//! link fails — this module never falls back to `CompilerDriver` in that
//! situation except via an explicit `OSCAN_NATIVE_LINKER_FLAVOR=compiler-driver`
//! override (design §7.3), which is a distinct, already-resolved branch of
//! [`driver::resolve_linker_selection`] rather than a fallback reached
//! from a failure path.

mod archive;
mod capability;
mod driver;
pub mod plan;

mod execute;

use std::path::Path;

pub use plan::{ExtraLib, LinkPlan, LinkerExecutable, LinkerFlavor, SystemLib};

use super::native_assets;
use super::target::NativeTarget;
use super::RuntimeMode;

/// Inputs that affect final native linking but not Cranelift object emission.
pub struct NativeLinkOptions<'a> {
    pub runtime_mode: RuntimeMode,
    pub show_warnings: bool,
    pub allow_elevated_native_link: bool,
    pub extra_c_files: &'a [String],
    pub extra_cflags: &'a [String],
    pub extra_objects: &'a [String],
    pub extra_libs: &'a [String],
}

pub(self) fn is_verbose() -> bool {
    crate::is_verbose()
}

pub fn is_system_library_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes[0] != b'-'
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(*b, b'_' | b'+' | b'-'))
}

fn extra_lib_input(value: &str) -> ExtraLib {
    if is_system_library_name(value) {
        ExtraLib::SystemName(value.to_string())
    } else {
        ExtraLib::Path(absolute_path_from_cwd(Path::new(value)))
    }
}

/// Design §1.1/§1.2/§7.1: MingwDirect is only ever eligible for the Windows
/// x86-64 **freestanding** target with no explicit user `.c` files. Hosted
/// (`--libc`) links always keep the external compiler-driver flavor: the
/// embedded asset set's import libraries are the freestanding Win32 subset
/// only (kernel32/ws2_32/user32/gdi32/secur32/crypt32) and never include a
/// CRT (msvcrt/ucrt), so a hosted program linked directly against them would
/// fail with undefined libc symbols (fwrite/printf/malloc/...) instead of
/// getting the compiler driver's CRT. Pulled out as its own pure function so
/// this exact eligibility rule is unit-testable without needing a real
/// runtime archive or embedded assets.
pub(self) fn is_mingw_eligible(
    target: NativeTarget,
    runtime_mode: RuntimeMode,
    extra_c_files_empty: bool,
) -> bool {
    target == NativeTarget::WindowsX64
        && runtime_mode == RuntimeMode::Freestanding
        && extra_c_files_empty
}

/// Design §10.6/§11.3: ElfDirect is eligible for Linux x86-64, AArch64,
/// and RISC-V64 **freestanding** targets with no explicit user `.c` files.
/// Hosted (`--libc`) links always keep the external compiler-driver flavor:
/// the embedded asset set has no CRT/libc, so a hosted link against it would
/// fail with undefined libc symbols.
pub(self) fn is_elf_eligible(
    target: NativeTarget,
    runtime_mode: RuntimeMode,
    extra_c_files_empty: bool,
) -> bool {
    matches!(
        target,
        NativeTarget::LinuxX64 | NativeTarget::LinuxAarch64 | NativeTarget::LinuxRiscv64
    ) && runtime_mode == RuntimeMode::Freestanding
        && extra_c_files_empty
}

/// Design §11.4/bugfix: is a non-host cross-link permitted, given whether an
/// explicit `OSCAN_NATIVE_LINKER` override is set together with a
/// target-matching `OSCAN_NATIVE_LINKER_FLAVOR`, or (absent an override)
/// whether this build has matching embedded ELF linker assets for `target`.
///
/// Pure/testable: callers resolve the env-var and `native_assets` state
/// first (see [`link_executable`]) and pass the resolved values in. An
/// explicit override for the *wrong* flavor (e.g. `elf` set while the
/// target is only `mingw_eligible`) does **not** count — only a
/// target-matching override bypasses the embedded-asset requirement;
/// anything else falls through to the embedded-asset check unchanged.
fn cross_link_permitted(
    elf_eligible: bool,
    mingw_eligible: bool,
    has_native_linker_override: bool,
    flavor_override: Option<&str>,
    embedded_assets_present: bool,
    embedded_target_matches: bool,
) -> bool {
    let explicit_cross_override = has_native_linker_override
        && ((elf_eligible && flavor_override == Some("elf"))
            || (mingw_eligible && flavor_override == Some("mingw")));
    explicit_cross_override || (elf_eligible && embedded_assets_present && embedded_target_matches)
}

/// Design §11.5: returns the target-appropriate GNU ld emulation name for
/// ELF targets. Emulation names are verified from the respective toolchains'
/// `lib/ldscripts/*.x` files.
fn elf_emulation(target: NativeTarget) -> &'static str {
    match target {
        NativeTarget::LinuxX64 => "elf_x86_64",
        NativeTarget::LinuxAarch64 => "aarch64linux",
        NativeTarget::LinuxRiscv64 => "elf64lriscv",
        _ => unreachable!("elf_emulation called for non-ELF target {target}"),
    }
}

pub fn write_object_file(bytes: &[u8], path: &Path) -> Result<(), String> {
    std::fs::write(path, bytes)
        .map_err(|e| format!("error writing object file '{}': {e}", path.display()))
}

/// Resolve `path` to an absolute path against the process's current
/// working directory when it is relative; returns it unchanged when it is
/// already absolute. Falls back to returning the original relative path if
/// the current directory cannot be read (best-effort; extremely unlikely,
/// and no worse than the pre-existing behavior).
fn absolute_path_from_cwd(path: &Path) -> std::path::PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// The five optional Windows import libraries, in the fixed order every
/// caller (capability analysis, embedded-asset lookup, manifest
/// dedup) must agree on.
const OPTIONAL_WINDOWS_LIBS: [&str; 5] = ["ws2_32", "user32", "gdi32", "secur32", "crypt32"];

/// Link `object_path`, the native shim, optional user C sources, and the
/// explicitly selected runtime archive into an executable at `exe_path`.
pub fn link_executable(
    object_path: &Path,
    exe_path: &Path,
    target: NativeTarget,
    options: &NativeLinkOptions<'_>,
) -> Result<(), String> {
    // Resolve to an absolute path up front (harmless for the legacy
    // `CompilerDriver` flavor, which never changes its child's CWD, but
    // required for `MingwDirect`: `execute::run`'s Windows DLL-search
    // hardening sets the embedded linker child's CWD to its own bin
    // directory, which would otherwise reinterpret a relative `-o <output>`
    // path against that directory instead of this process's real CWD).
    // `LinkPlan::render` is documented as doing no I/O of its own and
    // expecting every path already resolved by its caller — this is where
    // that resolution happens.
    let exe_path_buf = absolute_path_from_cwd(exe_path);
    let exe_path = exe_path_buf.as_path();

    let profile = if options.runtime_mode == RuntimeMode::Freestanding
        && options.extra_c_files.is_empty()
        && !capability::program_needs_graphics_runtime(object_path)
    {
        // No extra (unscannable) user C sources, and the compiled program
        // itself has no undefined graphics/image/SVG/TrueType symbol: the
        // smaller core archive is a complete, correct link for it.
        capability::FreestandingProfile::Core
    } else {
        capability::FreestandingProfile::Full
    };
    let archive_path =
        archive::find_or_build_runtime_archive(target, options.runtime_mode, profile)?;
    let manifest = archive::read_manifest(&archive_path);
    let shim_source =
        archive::resolve_shim_source(options.runtime_mode, &archive_path, manifest.as_ref())?;

    let mingw_eligible = is_mingw_eligible(
        target,
        options.runtime_mode,
        options.extra_c_files.is_empty(),
    );
    let elf_eligible = is_elf_eligible(
        target,
        options.runtime_mode,
        options.extra_c_files.is_empty(),
    );

    // Design §11.4: target-aware embedded-asset gate. Allow cross-linking for
    // ELF targets when this build has embedded assets FOR THAT TARGET (not just
    // "some" assets — the embedded target must match the requested target).
    //
    // Bugfix (found during coordinator live-validation of the cross-linker
    // sidecar mechanism): an explicit `OSCAN_NATIVE_LINKER` +
    // `OSCAN_NATIVE_LINKER_FLAVOR=elf`/`mingw` override must bypass this
    // embedded-asset check entirely — that is exactly the documented escape
    // hatch this error message itself advertises ("...or set
    // OSCAN_NATIVE_LINKER + OSCAN_NATIVE_LINKER_FLAVOR=elf to use an external
    // cross-linker"), and `driver::resolve_linker_selection` below already
    // fully implements it (`ElfLinkerSource::Override`/`MingwLinkerSource::Override`)
    // regardless of `native_assets::EMBEDDED_ASSETS_PRESENT`. Previously this
    // gate ran *before* that override was ever consulted, so the documented
    // sidecar workflow (release binary + a standalone cross-linker binary,
    // e.g. from `cross-linkers/<target>/` in a packaged release bundle)
    // failed with this "not the host target" error even when the override
    // was set correctly. Only a target-matching override counts: an
    // `elf` override only unblocks `elf_eligible` targets, a `mingw`
    // override only unblocks `mingw_eligible` targets — an override typoed
    // for the wrong flavor still falls through to this error, and
    // `resolve_linker_selection` is left to report any override-specific
    // failure (e.g. `mingw` override without embedded import libs).
    if !target.is_host() {
        let native_linker_override = driver::env_var_nonempty("OSCAN_NATIVE_LINKER");
        let flavor_override = driver::env_var_nonempty("OSCAN_NATIVE_LINKER_FLAVOR");
        let can_cross = cross_link_permitted(
            elf_eligible,
            mingw_eligible,
            native_linker_override.is_some(),
            flavor_override.as_deref(),
            native_assets::EMBEDDED_ASSETS_PRESENT,
            native_assets::embedded_target().as_deref() == Some(target.archive_tag()),
        );
        if !can_cross {
            let reason = if native_assets::EMBEDDED_ASSETS_PRESENT {
                if let Some(embedded_target) = native_assets::embedded_target() {
                    if embedded_target != target.archive_tag() {
                        format!(
                            "this oscan build has embedded assets for '{}', not '{}'",
                            embedded_target,
                            target.archive_tag()
                        )
                    } else {
                        "no matching embedded linker assets".to_string()
                    }
                } else {
                    "embedded asset manifest is missing 'target' field".to_string()
                }
            } else {
                "no embedded assets in this build".to_string()
            };
            return Err(format!(
                "'{}' is not the host target ({}); cross-linking requires a matching \
                 embedded ELF linker asset in this oscan build ({}) \
                 (build with OSCAN_EMBED_ASSETS_DIR staged for '{}', or set \
                 OSCAN_NATIVE_LINKER + OSCAN_NATIVE_LINKER_FLAVOR=elf to use an external \
                 cross-linker)",
                target,
                NativeTarget::host(),
                reason,
                target.archive_tag(),
            ));
        }
    }

    let selection = driver::resolve_linker_selection(
        target,
        options.runtime_mode,
        mingw_eligible,
        elf_eligible,
        &archive_path,
        manifest.as_ref(),
        native_assets::EMBEDDED_ASSETS_PRESENT,
    )?;

    let plan = match selection {
        driver::LinkerSelection::Mingw(mingw_source) => build_mingw_plan(
            object_path,
            exe_path,
            target,
            options,
            &archive_path,
            manifest.as_ref(),
            mingw_source,
        )?,
        driver::LinkerSelection::Elf(elf_source) => build_elf_plan(
            object_path,
            exe_path,
            target,
            options,
            &archive_path,
            elf_source,
        )?,
        driver::LinkerSelection::CompilerDriver(linker_driver) => build_compiler_driver_plan(
            object_path,
            exe_path,
            target,
            options,
            &archive_path,
            manifest.as_ref(),
            linker_driver,
            shim_source,
        )?,
    };

    // Design §7.3/§10.10: "either extraction or the direct link fails" both
    // fall under the no-silent-fallback rule. Extraction/asset-resolution
    // failures are already wrapped inside `build_mingw_plan`/`build_elf_plan`
    // above; this covers the remaining case — the actual linker invocation
    // itself (`execute::run`) failing (spawn failure, non-zero exit, or an
    // empty/missing output) — for the direct flavors specifically, so every
    // failure mode of the embedded-linker path surfaces the same "will not
    // silently fall back" diagnostic, not a bare tool error.
    match (execute::run(&plan), plan.flavor) {
        (Err(reason), LinkerFlavor::MingwDirect) => Err(driver::no_silent_fallback_error(&reason)),
        (Err(reason), LinkerFlavor::ElfDirect) => Err(driver::no_silent_fallback_error(&reason)),
        (result, _) => result,
    }
}

/// Build a [`LinkPlan`] for the [`LinkerFlavor::MingwDirect`] flavor: the
/// Windows x86-64 freestanding, embedded-assets default (design §2.4/§4.3).
fn build_mingw_plan(
    object_path: &Path,
    exe_path: &Path,
    target: NativeTarget,
    options: &NativeLinkOptions<'_>,
    archive_path: &Path,
    manifest: Option<&archive::RuntimeArchiveManifest>,
    mingw_source: driver::MingwLinkerSource,
) -> Result<LinkPlan, String> {
    // Toolchain-version cross-check (design §4.3): no silent drift between
    // the embedded linker's toolchain and the runtime archive's toolchain.
    if let (Some(embedded_version), Some(archive_version)) = (
        native_assets::embedded_toolchain_version(),
        manifest.and_then(|m| m.toolchain_version.clone()),
    ) {
        if embedded_version != archive_version {
            return Err(format!(
                "embedded linker toolchain (llvm-mingw {embedded_version}) does not match runtime \
                 archive toolchain (llvm-mingw {archive_version}); this oscan build and its runtime \
                 archives were produced from different toolchains"
            ));
        }
    }

    let extracted = native_assets::ensure_extracted(options.allow_elevated_native_link)
        .map_err(|reason| driver::no_silent_fallback_error(&reason))?;

    let linker = match &mingw_source {
        driver::MingwLinkerSource::Embedded => {
            let linker_asset = extracted.linker().ok_or_else(|| {
                driver::no_silent_fallback_error(&format!(
                    "this build's embedded asset set has no 'linker' role entry (cache set dir: '{}')",
                    extracted.dir.display()
                ))
            })?;
            LinkerExecutable::Embedded {
                path: linker_asset.path.clone(),
            }
        }
        driver::MingwLinkerSource::Override { command } => LinkerExecutable::Override {
            command: command.clone(),
        },
    };

    // LLD-sees-all-optional-imports rule (design §2.4): MingwDirect always
    // requests every optional import library, regardless of program
    // scanning.
    let mut system_libs = vec![resolve_embedded_system_lib(&extracted, "kernel32")
        .map_err(|reason| driver::no_silent_fallback_error(&reason))?];
    for name in OPTIONAL_WINDOWS_LIBS {
        system_libs.push(
            resolve_embedded_system_lib(&extracted, name)
                .map_err(|reason| driver::no_silent_fallback_error(&reason))?,
        );
    }

    let builtins = extracted.compiler_builtins().map(|a| a.path.clone());
    if builtins.is_none() {
        return Err(driver::no_silent_fallback_error(&format!(
            "this build's embedded asset set has no 'compiler_builtins' role entry (cache set dir: '{}')",
            extracted.dir.display()
        )));
    }

    Ok(LinkPlan {
        flavor: LinkerFlavor::MingwDirect,
        linker,
        target,
        runtime_mode: options.runtime_mode,
        output: exe_path.to_path_buf(),
        objects: vec![object_path.to_path_buf()],
        archives: vec![archive_path.to_path_buf()],
        system_libs,
        builtins,
        search_paths: vec![],
        extra_objects: options
            .extra_objects
            .iter()
            .map(|path| absolute_path_from_cwd(Path::new(path)))
            .collect(),
        entry: None,
        gc_sections: true,
        strip: true,
        build_id_none: true,
        pie: false,
        emulation: Some("i386pep"),
        use_lld_driver_flag: false,
        show_warnings: false,
        extra_c_sources: vec![],
        include_dirs: vec![],
        passthrough_cflags: vec![],
        static_link: false,
        extra_libs: options
            .extra_libs
            .iter()
            .map(|path| extra_lib_input(path))
            .collect(),
    })
}

/// Build a [`LinkPlan`] for the [`LinkerFlavor::ElfDirect`] flavor: the
/// Linux x86-64 freestanding, embedded-assets default (design §10.3/§10.5).
///
/// Deliberately does NOT perform a toolchain-version cross-check (unlike
/// `build_mingw_plan`): the Linux toolchain manifest's `version` field is a
/// fixed distribution name (e.g. "musl-cross-make 2023-08-28"), not a real
/// toolchain version number that can meaningfully drift between the linker
/// and the runtime archive builds. This is a documented, deliberate
/// simplification, not an oversight.
fn build_elf_plan(
    object_path: &Path,
    exe_path: &Path,
    target: NativeTarget,
    options: &NativeLinkOptions<'_>,
    archive_path: &Path,
    elf_source: driver::ElfLinkerSource,
) -> Result<LinkPlan, String> {
    let linker = match &elf_source {
        driver::ElfLinkerSource::Embedded => {
            let extracted = native_assets::ensure_extracted(options.allow_elevated_native_link)
                .map_err(|reason| driver::no_silent_fallback_error(&reason))?;
            let linker_asset = extracted.linker().ok_or_else(|| {
                driver::no_silent_fallback_error(&format!(
                    "this build's embedded asset set has no 'linker' role entry (cache set dir: '{}')",
                    extracted.dir.display()
                ))
            })?;
            LinkerExecutable::Embedded {
                path: linker_asset.path.clone(),
            }
        }
        driver::ElfLinkerSource::Override { command } => LinkerExecutable::Override {
            command: command.clone(),
        },
    };

    // Design §10.3: no system_libs, no builtins for Linux freestanding.
    let system_libs: Vec<SystemLib> = vec![];
    let builtins: Option<std::path::PathBuf> = None;

    Ok(LinkPlan {
        flavor: LinkerFlavor::ElfDirect,
        linker,
        target,
        runtime_mode: options.runtime_mode,
        output: exe_path.to_path_buf(),
        objects: vec![object_path.to_path_buf()],
        archives: vec![archive_path.to_path_buf()],
        system_libs,
        builtins,
        search_paths: vec![],
        extra_objects: options
            .extra_objects
            .iter()
            .map(|path| absolute_path_from_cwd(Path::new(path)))
            .collect(),
        entry: None,
        gc_sections: true,
        strip: true,
        build_id_none: true,
        pie: false,
        emulation: Some(elf_emulation(target)),
        use_lld_driver_flag: false,
        show_warnings: false,
        extra_c_sources: vec![],
        include_dirs: vec![],
        passthrough_cflags: vec![],
        static_link: false,
        extra_libs: options
            .extra_libs
            .iter()
            .map(|path| extra_lib_input(path))
            .collect(),
    })
}

fn resolve_embedded_system_lib(
    extracted: &native_assets::ExtractedAssetSet,
    name: &'static str,
) -> Result<SystemLib, String> {
    let path = extracted
        .import_lib(name)
        .map(|a| a.path.clone())
        .ok_or_else(|| {
            format!("this build's embedded asset set is missing the '{name}' import library")
        })?;
    Ok(SystemLib {
        name,
        archive_path: Some(path),
    })
}

/// Build a [`LinkPlan`] for the legacy [`LinkerFlavor::CompilerDriver`]
/// flavor, preserving the pre-split `link_executable`'s exact behavior.
fn build_compiler_driver_plan(
    object_path: &Path,
    exe_path: &Path,
    target: NativeTarget,
    options: &NativeLinkOptions<'_>,
    archive_path: &Path,
    manifest: Option<&archive::RuntimeArchiveManifest>,
    linker_driver: driver::LinkerDriver,
    shim_source: archive::ShimSource,
) -> Result<LinkPlan, String> {
    let mut objects = vec![object_path.to_path_buf()];
    if let archive::ShimSource::CompileLocally = shim_source {
        let shim_obj =
            driver::compile_shim_object(&linker_driver.cmd, target, options.runtime_mode)?;
        objects.push(shim_obj);
    }

    // Security review 2026-07-15 (finding 1): the archive manifest's raw
    // `link_flags` JSON array is *never* rendered into this (or any)
    // `Command`'s argv anymore — doing so let an attacker-controlled
    // runtime archive (reachable via `OSCAN_RUNTIME_ARCHIVE_DIR`, or any
    // other archive discovery path) inject arbitrary linker-driver
    // arguments (`-B`, `-fplugin=`, `-Wl,-plugin,`, `@response-files`,
    // `-o`/`--entry` overrides, ...). The two behaviors that actually
    // depended on `link_flags` in practice are now hardcoded below instead
    // (see `LinkPlan::static_link`'s doc comment for why this hardcoding is
    // deliberate and closed rather than manifest-driven). We still parse
    // the manifest's `link_flags` (via `manifest`, already read by the
    // caller) purely to surface a diagnostic if an archive still carries
    // some — printing only the *count*, never the flag text itself, since
    // that text is exactly the untrusted content we must not echo or use.
    if let Some(n) = manifest.map(|m| m.link_flags.len()).filter(|n| *n > 0) {
        eprintln!(
            "note: ignoring {n} link_flags entries recorded in this archive's manifest \
             (they are no longer used for security reasons; if your archive relied on \
             custom link flags, please open an issue — required flags are now derived \
             internally)"
        );
    }

    let mut needed_names: Vec<&'static str> = Vec::new();
    if target == NativeTarget::WindowsX64 {
        needed_names.push("kernel32");
        match options.runtime_mode {
            // Hosted mode always links every optional Win32 import
            // library (see capability::detect_windows_feature_libs's
            // docs for why this is not scanned for).
            RuntimeMode::Hosted => needed_names.extend(OPTIONAL_WINDOWS_LIBS),
            // Freestanding executables *are* dependency-checked; only
            // request the optional libraries this program's object
            // actually needs, unless extra user C files (unscanned) are
            // present, or the discovered driver is Clang/LLD (which needs
            // every optional import library present during resolution,
            // per the LLD-sees-all-optional-imports rule).
            RuntimeMode::Freestanding => {
                if options.extra_c_files.is_empty()
                    && linker_driver.linker_family != driver::LinkerFamily::Lld
                {
                    needed_names.extend(capability::detect_windows_feature_libs(object_path));
                } else {
                    needed_names.extend(OPTIONAL_WINDOWS_LIBS);
                }
            }
        }
    } else if options.runtime_mode == RuntimeMode::Hosted {
        // Security review 2026-07-15 (finding 1): this used to arrive via
        // the runtime archive manifest's `link_flags`
        // (`packaging/toolchains/runtime-archive-contract.json`'s
        // `targets.linux-x86_64.hosted` == `["-lm"]`); hardcoded here now
        // that manifest content is never trusted for argv construction.
        needed_names.push("m");
    }

    let mut system_libs = Vec::new();
    for name in needed_names {
        system_libs.push(SystemLib {
            name,
            archive_path: None,
        });
    }

    let include_dirs = if options.extra_c_files.is_empty() {
        Vec::new()
    } else if let Some(runtime_dir) = archive::find_runtime_source_dir() {
        let mut dirs = vec![runtime_dir.clone()];
        dirs.extend(crate::find_extra_include_dirs(&runtime_dir));
        dirs
    } else {
        Vec::new()
    };

    let builtins = if options.runtime_mode == RuntimeMode::Freestanding
        && target == NativeTarget::WindowsX64
    {
        driver::find_compiler_builtins_lib(&linker_driver.cmd)
    } else {
        None
    };

    // Security review 2026-07-15 (finding 1): hardcoded, deliberate
    // duplication of `packaging/toolchains/runtime-archive-contract.json`'s
    // `targets.linux-x86_64.{freestanding,freestanding_core}.link_flags`
    // (which includes `-static`, alongside `-nostdlib`/`-Wl,--gc-sections,
    // --build-id=none` that are already rendered elsewhere in this module
    // independently of the manifest) — see `LinkPlan::static_link`'s doc
    // comment. Never re-derive this by reading the manifest.
    let static_link =
        target != NativeTarget::WindowsX64 && options.runtime_mode == RuntimeMode::Freestanding;

    Ok(LinkPlan {
        flavor: LinkerFlavor::CompilerDriver,
        linker: LinkerExecutable::CompilerDriver {
            command: linker_driver.cmd.clone(),
            source: linker_driver.source,
        },
        target,
        runtime_mode: options.runtime_mode,
        output: exe_path.to_path_buf(),
        objects,
        archives: vec![archive_path.to_path_buf()],
        system_libs,
        builtins,
        search_paths: vec![],
        extra_objects: options
            .extra_objects
            .iter()
            .map(|path| absolute_path_from_cwd(Path::new(path)))
            .collect(),
        entry: None,
        gc_sections: true,
        strip: options.runtime_mode == RuntimeMode::Freestanding,
        build_id_none: options.runtime_mode == RuntimeMode::Freestanding,
        pie: false,
        emulation: None,
        use_lld_driver_flag: linker_driver.linker_family == driver::LinkerFamily::Lld,
        show_warnings: options.show_warnings,
        extra_c_sources: options
            .extra_c_files
            .iter()
            .map(std::path::PathBuf::from)
            .collect(),
        include_dirs,
        passthrough_cflags: options.extra_cflags.to_vec(),
        static_link,
        extra_libs: options
            .extra_libs
            .iter()
            .map(|path| extra_lib_input(path))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression test for a real bug found during Vasquez's native-link
    // embedding validation pass: `is_mingw_eligible` (formerly an inline
    // `mingw_eligible` computation in `link_executable`) did not check
    // `runtime_mode`, so `--libc --backend native` (Hosted) on Windows
    // incorrectly qualified for MingwDirect whenever embedded assets were
    // present. MingwDirect's embedded import libraries are the freestanding
    // Win32 subset only (no msvcrt/ucrt), so every hosted link failed with
    // undefined CRT symbols (fwrite/printf/malloc/sqrt/...). See
    // `.squad/decisions/inbox/vasquez-native-link-validation.md`.
    #[test]
    fn hosted_mode_is_never_mingw_eligible_even_on_windows_with_no_extra_c_files() {
        assert!(!is_mingw_eligible(
            NativeTarget::WindowsX64,
            RuntimeMode::Hosted,
            true
        ));
    }

    #[test]
    fn freestanding_windows_with_no_extra_c_files_is_mingw_eligible() {
        assert!(is_mingw_eligible(
            NativeTarget::WindowsX64,
            RuntimeMode::Freestanding,
            true
        ));
    }

    #[test]
    fn freestanding_windows_with_extra_c_files_is_not_mingw_eligible() {
        assert!(!is_mingw_eligible(
            NativeTarget::WindowsX64,
            RuntimeMode::Freestanding,
            false
        ));
    }

    #[test]
    fn freestanding_non_windows_target_is_not_mingw_eligible() {
        assert!(!is_mingw_eligible(
            NativeTarget::LinuxX64,
            RuntimeMode::Freestanding,
            true
        ));
    }

    // Regression test for a bug this pass's Windows DLL-search hardening
    // introduced and then fixed: `execute::run` sets the `MingwDirect`
    // child's CWD to its own bin directory, which would silently
    // reinterpret a *relative* `-o <output>` argument against that
    // directory instead of the oscan process's real CWD, causing the
    // linker to "succeed" while writing the executable to the wrong place.
    #[test]
    fn absolute_path_from_cwd_leaves_an_absolute_path_unchanged() {
        let abs = if cfg!(windows) {
            Path::new(r"C:\some\abs\path.exe")
        } else {
            Path::new("/some/abs/path")
        };
        assert_eq!(absolute_path_from_cwd(abs), abs);
    }

    #[test]
    fn absolute_path_from_cwd_resolves_a_relative_path_against_the_real_cwd() {
        let real_cwd = std::env::current_dir().expect("current dir must be readable in tests");
        let resolved = absolute_path_from_cwd(Path::new("hello.exe"));
        assert!(resolved.is_absolute());
        assert_eq!(resolved, real_cwd.join("hello.exe"));
    }

    #[test]
    fn relative_extra_link_inputs_are_resolved_before_linker_cwd_hardening() {
        let real_cwd = std::env::current_dir().expect("current dir must be readable in tests");
        for path in ["objects/helper.obj", "libs/helper.lib"] {
            assert_eq!(absolute_path_from_cwd(Path::new(path)), real_cwd.join(path));
        }
    }

    /// Security review 2026-07-15 (finding 1), full pipeline test: build a
    /// real [`LinkPlan`] via [`build_compiler_driver_plan`] — the exact
    /// function that used to call `archive::read_link_flags` and render its
    /// result verbatim — from a manifest containing the full injection set
    /// from the finding (`-B`, an attacker directory, `-fplugin=`,
    /// `-Wl,-plugin,`, an `@response-file`, and `-o`/`--entry` overrides),
    /// and prove none of it appears anywhere in `render()`'s output. Also
    /// asserts the positive/regression side: Linux hosted still gets `-lm`
    /// (the one behavior that genuinely depended on the manifest's
    /// `link_flags` for this target/mode, now hardcoded instead — see
    /// `LinkPlan::static_link`'s doc comment).
    #[test]
    fn build_compiler_driver_plan_never_leaks_manifest_link_flags_into_argv() {
        let malicious_manifest = archive::RuntimeArchiveManifest {
            link_flags: vec![
                "-B".to_string(),
                "/attacker/dir".to_string(),
                "-fplugin=/attacker/evil.so".to_string(),
                "-Wl,-plugin,/attacker/evil.so".to_string(),
                "@attacker.rsp".to_string(),
                "-o".to_string(),
                "/attacker/overwrite/target".to_string(),
                "--entry".to_string(),
                "evil_symbol".to_string(),
            ],
            contains_native_shim: true,
            ..Default::default()
        };
        let options = NativeLinkOptions {
            runtime_mode: RuntimeMode::Hosted,
            show_warnings: false,
            allow_elevated_native_link: false,
            extra_c_files: &[],
            extra_cflags: &[],
            extra_objects: &[],
            extra_libs: &[],
        };
        let linker_driver = driver::LinkerDriver {
            cmd: "cc".to_string(),
            source: crate::CompilerSource::Host,
            linker_family: driver::LinkerFamily::GnuLd,
        };
        let plan = build_compiler_driver_plan(
            Path::new("/obj/program.o"),
            Path::new("/out/hello"),
            NativeTarget::LinuxX64,
            &options,
            Path::new("/archives/libosc_runtime_hosted.a"),
            Some(&malicious_manifest),
            linker_driver,
            archive::ShimSource::ArchiveMember,
        )
        .expect("plan construction never fails on manifest content alone");

        let rendered: Vec<String> = plan
            .render()
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        for forbidden in [
            "-B",
            "/attacker/dir",
            "-fplugin=/attacker/evil.so",
            "-Wl,-plugin,/attacker/evil.so",
            "@attacker.rsp",
            "/attacker/overwrite/target",
            "--entry",
            "evil_symbol",
        ] {
            assert!(
                !rendered.iter().any(|a| a == forbidden),
                "manifest-injected flag {forbidden:?} leaked into rendered argv: {rendered:?}"
            );
        }
        assert_eq!(
            rendered.iter().filter(|a| a.as_str() == "-o").count(),
            1,
            "exactly one -o (ours), never a manifest-supplied second one, got {rendered:?}"
        );
        assert!(
            rendered.iter().any(|a| a == "-lm"),
            "Linux hosted must still get -lm (now hardcoded, not manifest-derived), got {rendered:?}"
        );
    }

    /// Companion regression test: Linux **freestanding** still gets
    /// `-static` after the same change.
    #[test]
    fn build_compiler_driver_plan_linux_freestanding_still_gets_static() {
        let options = NativeLinkOptions {
            runtime_mode: RuntimeMode::Freestanding,
            show_warnings: false,
            allow_elevated_native_link: false,
            extra_c_files: &[],
            extra_cflags: &[],
            extra_objects: &[],
            extra_libs: &[],
        };
        let linker_driver = driver::LinkerDriver {
            cmd: "cc".to_string(),
            source: crate::CompilerSource::Host,
            linker_family: driver::LinkerFamily::GnuLd,
        };
        let plan = build_compiler_driver_plan(
            Path::new("/obj/program.o"),
            Path::new("/out/hello"),
            NativeTarget::LinuxX64,
            &options,
            Path::new("/archives/libosc_runtime_freestanding.a"),
            None,
            linker_driver,
            archive::ShimSource::ArchiveMember,
        )
        .expect("plan construction succeeds with no manifest");
        assert!(plan.static_link);
        let rendered: Vec<String> = plan
            .render()
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(
            rendered.iter().any(|a| a == "-static"),
            "expected -static in {rendered:?}"
        );
    }

    // --- is_elf_eligible tests (design §10.6) ---

    #[test]
    fn hosted_linux_is_never_elf_eligible() {
        assert!(!is_elf_eligible(
            NativeTarget::LinuxX64,
            RuntimeMode::Hosted,
            true
        ));
    }

    #[test]
    fn freestanding_linux_x64_with_no_extra_c_files_is_elf_eligible() {
        assert!(is_elf_eligible(
            NativeTarget::LinuxX64,
            RuntimeMode::Freestanding,
            true
        ));
    }

    #[test]
    fn freestanding_linux_x64_with_extra_c_files_is_not_elf_eligible() {
        assert!(!is_elf_eligible(
            NativeTarget::LinuxX64,
            RuntimeMode::Freestanding,
            false
        ));
    }

    #[test]
    fn freestanding_windows_is_not_elf_eligible() {
        assert!(!is_elf_eligible(
            NativeTarget::WindowsX64,
            RuntimeMode::Freestanding,
            true
        ));
    }

    #[test]
    fn freestanding_linux_aarch64_with_no_extra_c_files_is_elf_eligible() {
        assert!(is_elf_eligible(
            NativeTarget::LinuxAarch64,
            RuntimeMode::Freestanding,
            true
        ));
    }

    #[test]
    fn freestanding_linux_riscv64_with_no_extra_c_files_is_elf_eligible() {
        assert!(is_elf_eligible(
            NativeTarget::LinuxRiscv64,
            RuntimeMode::Freestanding,
            true
        ));
    }

    // --- cross_link_permitted tests (bugfix: OSCAN_NATIVE_LINKER override
    // must bypass the embedded-asset gate for a non-host cross-link target;
    // this is the exact escape hatch the gate's own error message
    // advertises, and it silently didn't work before this fix — verified
    // live via a real x86_64 oscan release binary + an extracted
    // linux-aarch64 cross-linker-sidecar binary cross-linking and
    // qemu-aarch64-executing a real program) ---

    #[test]
    fn cross_link_denied_with_no_override_and_no_embedded_assets() {
        assert!(!cross_link_permitted(
            true,  // elf_eligible
            false, // mingw_eligible
            false, // has_native_linker_override
            None,  // flavor_override
            false, // embedded_assets_present
            false, // embedded_target_matches
        ));
    }

    #[test]
    fn cross_link_permitted_with_elf_override_even_without_embedded_assets() {
        assert!(cross_link_permitted(
            true,  // elf_eligible
            false, // mingw_eligible
            true,  // has_native_linker_override
            Some("elf"),
            false, // embedded_assets_present
            false, // embedded_target_matches
        ));
    }

    #[test]
    fn cross_link_permitted_with_mingw_override_even_without_embedded_assets() {
        assert!(cross_link_permitted(
            false, // elf_eligible
            true,  // mingw_eligible
            true,  // has_native_linker_override
            Some("mingw"),
            false, // embedded_assets_present
            false, // embedded_target_matches
        ));
    }

    #[test]
    fn cross_link_denied_when_override_flavor_does_not_match_eligibility() {
        // OSCAN_NATIVE_LINKER_FLAVOR=elf set, but this target is only
        // mingw_eligible (not elf_eligible) — must NOT bypass the gate.
        assert!(!cross_link_permitted(
            false, // elf_eligible
            true,  // mingw_eligible
            true,  // has_native_linker_override
            Some("elf"),
            false, // embedded_assets_present
            false, // embedded_target_matches
        ));
    }

    #[test]
    fn cross_link_denied_when_linker_override_unset_even_with_flavor_set() {
        // OSCAN_NATIVE_LINKER_FLAVOR alone (no OSCAN_NATIVE_LINKER) must not
        // count as an explicit cross-linker override for a foreign target.
        assert!(!cross_link_permitted(
            true,  // elf_eligible
            false, // mingw_eligible
            false, // has_native_linker_override
            Some("elf"),
            false, // embedded_assets_present
            false, // embedded_target_matches
        ));
    }

    #[test]
    fn cross_link_permitted_via_matching_embedded_assets_without_any_override() {
        assert!(cross_link_permitted(
            true,  // elf_eligible
            false, // mingw_eligible
            false, // has_native_linker_override
            None, true, // embedded_assets_present
            true, // embedded_target_matches
        ));
    }

    #[test]
    fn cross_link_denied_via_embedded_assets_for_a_different_target() {
        assert!(!cross_link_permitted(
            true,  // elf_eligible
            false, // mingw_eligible
            false, // has_native_linker_override
            None, true,  // embedded_assets_present
            false, // embedded_target_matches (mismatched target)
        ));
    }

    // --- elf_emulation tests (design §11.5) ---

    #[test]
    fn elf_emulation_linuxx64_returns_elf_x86_64() {
        assert_eq!(elf_emulation(NativeTarget::LinuxX64), "elf_x86_64");
    }

    #[test]
    fn elf_emulation_linuxaarch64_returns_aarch64linux() {
        assert_eq!(elf_emulation(NativeTarget::LinuxAarch64), "aarch64linux");
    }

    #[test]
    fn elf_emulation_linuxriscv64_returns_elf64lriscv() {
        assert_eq!(elf_emulation(NativeTarget::LinuxRiscv64), "elf64lriscv");
    }
}
