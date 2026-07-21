//! Pure, unit-testable description of a final native link, and the exact
//! per-[`LinkerFlavor`] renderer that turns it into an argv.
//!
//! See `docs/design/native-link-embedding.md` §2.2/§2.4 for the contract
//! this module implements. [`LinkPlan::render`] does no I/O: every path it
//! emits must already be resolved by the caller (`super` orchestration,
//! `super::archive`, `super::driver`, `super::super::native_assets`).
//! This separation is what makes exact-argv snapshot tests possible (see
//! this module's `tests`), which is the primary "did we build the right
//! command" surface for this change.
//!
//! # Deviation from the design doc's illustrative `LinkPlan` (documented)
//!
//! §2.2 shows `LinkPlan`'s fields as the primary contract for the new
//! `MingwDirect` flavor; the legacy `CompilerDriver` flavor's exact argv
//! shape was intentionally left unspecified there (`CompilerDriver` is
//! "legacy... transitional", per §2.2's own doc comment on
//! [`LinkerFlavor`]). To render `CompilerDriver` without losing any
//! existing behavior (extra user `.c` sources compiled inline with the
//! link, arbitrary passthrough `-l`/`-D` style flags, extra `-I` include
//! dirs, `-fuse-ld=lld`, warning suppression), this struct adds a handful
//! of fields beyond §2.2's listing: [`LinkPlan::use_lld_driver_flag`],
//! [`LinkPlan::show_warnings`], [`LinkPlan::extra_c_sources`],
//! [`LinkPlan::include_dirs`], [`LinkPlan::passthrough_cflags`]. None of
//! §2.2's named fields are renamed or removed. See
//! `.squad/decisions/inbox/bishop-native-link-impl.md` for the recorded
//! rationale.

use std::ffi::OsString;
use std::path::PathBuf;

use super::super::target::NativeTarget;
use super::super::RuntimeMode;
use crate::CompilerSource;

/// How a [`LinkPlan`] renders to a concrete linker invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkerFlavor {
    /// Direct MinGW-flavor `ld.lld` (`-m i386pep`), the Windows freestanding
    /// default when embedded assets are present. No compiler driver.
    MingwDirect,
    /// Legacy: invoke GCC/Clang as a linker driver (`cc obj shim runtime.a`).
    /// Transitional; hosted mode; explicit `.c` files; dev builds without
    /// embedded assets.
    CompilerDriver,
    /// Direct GNU ld invocation targeting ELF x86-64 (`-m elf_x86_64`), the
    /// Linux freestanding default when embedded assets are present. No
    /// compiler driver. See `docs/design/native-link-embedding.md` §10.
    ElfDirect,
}

/// Where the linker binary came from — drives diagnostics and the
/// no-silent-fallback rule (design §7.3).
#[derive(Debug, Clone)]
pub enum LinkerExecutable {
    /// Extracted from this oscan binary's embedded assets to `path`.
    Embedded { path: PathBuf },
    /// User override via `OSCAN_NATIVE_LINKER` (+ `OSCAN_NATIVE_LINKER_FLAVOR`).
    Override { command: String },
    /// Legacy compiler driver discovered on host / recorded in archive manifest.
    CompilerDriver {
        command: String,
        source: CompilerSource,
    },
}

impl LinkerExecutable {
    /// The invocable command/path, suitable for `Command::new`.
    pub fn as_os_str(&self) -> &std::ffi::OsStr {
        match self {
            LinkerExecutable::Embedded { path } => path.as_os_str(),
            LinkerExecutable::Override { command } => command.as_ref(),
            LinkerExecutable::CompilerDriver { command, .. } => command.as_ref(),
        }
    }

    /// A display-friendly label for "Linking ... with <command> (<label>)"
    /// style user-facing messages.
    pub fn display_command(&self) -> String {
        match self {
            LinkerExecutable::Embedded { path } => path.display().to_string(),
            LinkerExecutable::Override { command } => command.clone(),
            LinkerExecutable::CompilerDriver { command, .. } => command.clone(),
        }
    }
}

/// A resolved system/import library input (absolute path form preferred for
/// direct flavors; `-l` name form for the compiler driver).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemLib {
    /// e.g. "kernel32"; used to render `-lkernel32` for CompilerDriver.
    pub name: &'static str,
    /// Absolute staged path (Some for MingwDirect: passed positionally).
    pub archive_path: Option<PathBuf>,
}

/// User-supplied `--extra-lib` input: either an explicit archive/import-library
/// path or a safe system-library name rendered as `-l<name>` for compiler
/// drivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtraLib {
    Path(PathBuf),
    SystemName(String),
}

impl ExtraLib {
    fn push_arg(&self, args: &mut Vec<OsString>) {
        match self {
            ExtraLib::Path(path) => args.push(path.clone().into_os_string()),
            ExtraLib::SystemName(name) => args.push(format!("-l{name}").into()),
        }
    }
}

/// Pure, unit-testable description of a final native link. Construction does
/// no I/O; all paths are already resolved by the caller.
#[derive(Debug, Clone)]
pub struct LinkPlan {
    pub flavor: LinkerFlavor,
    pub linker: LinkerExecutable,
    pub target: NativeTarget,
    pub runtime_mode: RuntimeMode,
    pub output: PathBuf,
    /// program.obj, shim.o (when compiled locally, hosted-legacy-archive path only)
    pub objects: Vec<PathBuf>,
    /// runtime archive(s), user `.a`
    pub archives: Vec<PathBuf>,
    /// kernel32 + optional ws2_32/user32/gdi32/secur32/crypt32
    pub system_libs: Vec<SystemLib>,
    /// `libclang_rt.builtins-x86_64.a` (embedded) or `-print-libgcc-file-name` (driver)
    pub builtins: Option<PathBuf>,
    /// empty for MingwDirect (absolute inputs); reserved for a future
    /// `-L`-based flavor (e.g. `ElfDirect`).
    pub search_paths: Vec<PathBuf>,
    /// user `.c` compiled outputs (driver only)
    pub extra_objects: Vec<PathBuf>,
    /// None today (archive supplies `_start`)
    pub entry: Option<String>,
    pub gc_sections: bool,
    pub strip: bool,
    pub build_id_none: bool,
    /// false; `-no-pie` only rendered on non-Windows, CompilerDriver flavor only
    pub pie: bool,
    /// Some("i386pep") for MingwDirect
    pub emulation: Option<&'static str>,

    // --- CompilerDriver-only fields (documented deviation above) ---
    /// Render `-fuse-ld=lld` first (matches the pre-split
    /// `driver.linker_family == LinkerFamily::Lld` branch).
    pub use_lld_driver_flag: bool,
    /// Mirrors `NativeLinkOptions::show_warnings`; only affects rendering
    /// when `extra_c_sources` is non-empty (`-w` suppression).
    pub show_warnings: bool,
    /// Raw, uncompiled user `.c` sources passed straight to the compiler
    /// driver so it compiles *and* links them in one invocation (today's
    /// exact `--extra-c-file` behavior).
    pub extra_c_sources: Vec<PathBuf>,
    /// `-I<dir>` search dirs, only rendered when `extra_c_sources` is
    /// non-empty.
    pub include_dirs: Vec<PathBuf>,
    /// Arbitrary passthrough flags (`--extra-cflags`), appended verbatim
    /// after all inputs/libs, matching prior ordering.
    pub passthrough_cflags: Vec<String>,
    /// `-static` (non-Windows freestanding/freestanding_core only).
    ///
    /// Security review 2026-07-15 (finding 1, `link_flags` injection): this
    /// crate used to render a runtime archive manifest's raw `link_flags`
    /// JSON array verbatim into this `Command`'s argv
    /// (`manifest_link_flags`, now removed). That let anything reachable
    /// via `OSCAN_RUNTIME_ARCHIVE_DIR` (or any other archive discovery
    /// path) inject arbitrary linker-driver arguments — `-B`, `-fplugin=`,
    /// `-Wl,-plugin,`, `@response-files`, `-o`/`--entry` overrides, etc.
    /// `link_flags` is no longer read for this purpose at all (see
    /// `archive.rs`'s doc comment on `read_manifest`, security review
    /// 2026-07-15). The only two behaviors
    /// that actually depended on the manifest's `link_flags` in practice
    /// (`packaging/toolchains/runtime-archive-contract.json`'s
    /// `targets.linux-x86_64.{hosted,freestanding,freestanding_core}` —
    /// Windows' own `link_flags` were already fully redundant with
    /// `needed_names`/`-nostdlib` elsewhere in this module) are now
    /// hardcoded here instead: Linux hosted's `-lm` (via the ordinary
    /// `system_libs`/`SystemLib` mechanism, see `mod.rs`) and this field,
    /// Linux freestanding's `-static`. This is a **deliberate, closed,
    /// security-motivated duplication** of the contract's own static
    /// values — never re-derive it by reading the manifest again.
    pub static_link: bool,
    /// User-supplied precompiled static libraries (`--extra-lib`). Rendered
    /// before the runtime archive(s), so undefined runtime references from
    /// selected user archive members are resolved by the later runtime
    /// archive scan. Design §12.4.
    pub extra_libs: Vec<ExtraLib>,
}

impl LinkPlan {
    /// Pure: renders argv (excluding argv[0], the linker). Unit-tested with
    /// snapshot/exact-vector assertions per flavor — this is the primary
    /// "did we build the right command" test surface.
    pub fn render(&self) -> Vec<OsString> {
        match self.flavor {
            LinkerFlavor::MingwDirect => self.render_mingw_direct(),
            LinkerFlavor::CompilerDriver => self.render_compiler_driver(),
            LinkerFlavor::ElfDirect => self.render_elf_direct(),
        }
    }

    /// Exact order locked by design §2.4:
    /// `-s -m i386pep -Bdynamic --gc-sections --build-id=none -o <output>
    /// <objects...> <archives...> <system_libs...> <builtins>`.
    /// No `-nostdlib`/`-no-pie`/`-fuse-ld`/`-l`/`-L`: import libs and
    /// builtins are absolute positional archive inputs.
    fn render_mingw_direct(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = Vec::new();
        if self.strip {
            args.push("-s".into());
        }
        if let Some(emulation) = self.emulation {
            args.push("-m".into());
            args.push(emulation.into());
        }
        args.push("-Bdynamic".into());
        if self.gc_sections {
            args.push("--gc-sections".into());
        }
        if self.build_id_none {
            args.push("--build-id=none".into());
        }
        if let Some(entry) = &self.entry {
            args.push(format!("--entry={entry}").into());
        }
        args.push("-o".into());
        args.push(self.output.clone().into_os_string());
        for obj in &self.objects {
            args.push(obj.clone().into_os_string());
        }
        for obj in &self.extra_objects {
            args.push(obj.clone().into_os_string());
        }
        for lib in &self.extra_libs {
            lib.push_arg(&mut args);
        }
        for archive in &self.archives {
            args.push(archive.clone().into_os_string());
        }
        for lib in &self.system_libs {
            if let Some(path) = &lib.archive_path {
                args.push(path.clone().into_os_string());
            }
        }
        if let Some(builtins) = &self.builtins {
            args.push(builtins.clone().into_os_string());
        }
        args
    }

    /// Exact order locked by design §10.3:
    /// `-s -m elf_x86_64 -static --gc-sections --build-id=none -o <output>
    /// <objects...> <archives...> <system_libs...> <builtins>`.
    /// No `-nostdlib`/`-no-pie`/`-fuse-ld`/`-l`/`-L`/`--entry`: the runtime
    /// archive's `_start` resolves as ld's default entry symbol automatically.
    fn render_elf_direct(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = Vec::new();
        if self.strip {
            args.push("-s".into());
        }
        if let Some(emulation) = self.emulation {
            args.push("-m".into());
            args.push(emulation.into());
        }
        args.push("-static".into());
        if self.gc_sections {
            args.push("--gc-sections".into());
        }
        if self.build_id_none {
            args.push("--build-id=none".into());
        }
        args.push("-o".into());
        args.push(self.output.clone().into_os_string());
        for obj in &self.objects {
            args.push(obj.clone().into_os_string());
        }
        for obj in &self.extra_objects {
            args.push(obj.clone().into_os_string());
        }
        for lib in &self.extra_libs {
            lib.push_arg(&mut args);
        }
        for archive in &self.archives {
            args.push(archive.clone().into_os_string());
        }
        for lib in &self.system_libs {
            if let Some(path) = &lib.archive_path {
                args.push(path.clone().into_os_string());
            }
        }
        if let Some(builtins) = &self.builtins {
            args.push(builtins.clone().into_os_string());
        }
        args
    }

    /// Legacy compiler-driver rendering. Preserves the exact prior
    /// `link_executable` argument order/content (see `git blame` on the
    /// pre-split `link.rs` for the reference implementation this mirrors).
    fn render_compiler_driver(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = Vec::new();
        if self.use_lld_driver_flag {
            args.push("-fuse-ld=lld".into());
        }
        if self.runtime_mode == RuntimeMode::Freestanding {
            // The runtime archive supplies `_start`/`mainCRTStartup`; do not
            // admit the toolchain's own CRT/default libraries.
            args.push("-nostdlib".into());
        }
        if self.strip {
            args.push("-s".into());
        }
        if self.gc_sections {
            if self.build_id_none {
                args.push("-Wl,--gc-sections,--build-id=none".into());
            } else {
                args.push("-Wl,--gc-sections".into());
            }
        }
        if !self.pie && self.target != NativeTarget::WindowsX64 {
            args.push("-no-pie".into());
        }
        if self.static_link {
            args.push("-static".into());
        }
        if !self.extra_c_sources.is_empty() {
            match self.runtime_mode {
                RuntimeMode::Freestanding => {
                    args.push("-std=gnu11".into());
                    args.push("-ffreestanding".into());
                    args.push("-fno-builtin".into());
                }
                RuntimeMode::Hosted => {
                    args.push("-std=c99".into());
                    args.push("-O2".into());
                }
            }
        }
        if !self.show_warnings && !self.extra_c_sources.is_empty() {
            args.push("-w".into());
        }
        if !self.extra_c_sources.is_empty() {
            for dir in &self.include_dirs {
                args.push(format!("-I{}", dir.display()).into());
            }
        }
        for obj in &self.objects {
            args.push(obj.clone().into_os_string());
        }
        for obj in &self.extra_objects {
            args.push(obj.clone().into_os_string());
        }
        // User C translation units precede the runtime archive so any
        // runtime symbols they call are resolved in the same left-to-right
        // static link.
        for src in &self.extra_c_sources {
            args.push(src.clone().into_os_string());
        }
        for lib in &self.extra_libs {
            lib.push_arg(&mut args);
        }
        for archive in &self.archives {
            args.push(archive.clone().into_os_string());
        }
        for path in &self.search_paths {
            args.push(format!("-L{}", path.display()).into());
        }
        args.push("-o".into());
        args.push(self.output.clone().into_os_string());
        for lib in &self.system_libs {
            args.push(format!("-l{}", lib.name).into());
        }
        for flag in &self.passthrough_cflags {
            args.push(flag.into());
        }
        // Re-link the compiler's own support library after -nostdlib
        // dropped it. Must come after the archive/objects that reference
        // symbols from it.
        if self.runtime_mode == RuntimeMode::Freestanding && self.target == NativeTarget::WindowsX64
        {
            if let Some(builtins) = &self.builtins {
                args.push(builtins.clone().into_os_string());
            }
        }
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mingw_plan(system_libs: Vec<SystemLib>) -> LinkPlan {
        LinkPlan {
            flavor: LinkerFlavor::MingwDirect,
            linker: LinkerExecutable::Embedded {
                path: PathBuf::from(r"C:\cache\native-assets\abc123\linker\ld.lld.exe"),
            },
            target: NativeTarget::WindowsX64,
            runtime_mode: RuntimeMode::Freestanding,
            output: PathBuf::from(r"C:\out\hello.exe"),
            objects: vec![PathBuf::from(r"C:\obj\program.obj")],
            archives: vec![PathBuf::from(
                r"C:\archives\libosc_runtime_freestanding_core.a",
            )],
            system_libs,
            builtins: Some(PathBuf::from(
                r"C:\cache\native-assets\abc123\lib\clang\libclang_rt.builtins-x86_64.a",
            )),
            search_paths: vec![],
            extra_objects: vec![],
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
            extra_libs: vec![],
        }
    }

    fn all_five_optional_libs() -> Vec<SystemLib> {
        vec![
            SystemLib {
                name: "kernel32",
                archive_path: Some(PathBuf::from(
                    r"C:\cache\native-assets\abc123\lib\libkernel32.a",
                )),
            },
            SystemLib {
                name: "ws2_32",
                archive_path: Some(PathBuf::from(
                    r"C:\cache\native-assets\abc123\lib\libws2_32.a",
                )),
            },
            SystemLib {
                name: "user32",
                archive_path: Some(PathBuf::from(
                    r"C:\cache\native-assets\abc123\lib\libuser32.a",
                )),
            },
            SystemLib {
                name: "gdi32",
                archive_path: Some(PathBuf::from(
                    r"C:\cache\native-assets\abc123\lib\libgdi32.a",
                )),
            },
            SystemLib {
                name: "secur32",
                archive_path: Some(PathBuf::from(
                    r"C:\cache\native-assets\abc123\lib\libsecur32.a",
                )),
            },
            SystemLib {
                name: "crypt32",
                archive_path: Some(PathBuf::from(
                    r"C:\cache\native-assets\abc123\lib\libcrypt32.a",
                )),
            },
        ]
    }

    fn render_strings(plan: &LinkPlan) -> Vec<String> {
        plan.render()
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    /// Exact-argv snapshot per design §2.4 — the primary "did we build the
    /// right command" surface. Any drift here (wrong order, an accidental
    /// `-nostdlib`/`-no-pie`/`-fuse-ld`/`-l`/`-L`) is a byte-parity
    /// regression against the proven 6,656-byte `hello.osc` output.
    #[test]
    fn mingw_direct_renders_exact_argv_in_locked_order() {
        let plan = mingw_plan(all_five_optional_libs());
        let rendered = render_strings(&plan);
        assert_eq!(
            rendered,
            vec![
                "-s",
                "-m",
                "i386pep",
                "-Bdynamic",
                "--gc-sections",
                "--build-id=none",
                "-o",
                r"C:\out\hello.exe",
                r"C:\obj\program.obj",
                r"C:\archives\libosc_runtime_freestanding_core.a",
                r"C:\cache\native-assets\abc123\lib\libkernel32.a",
                r"C:\cache\native-assets\abc123\lib\libws2_32.a",
                r"C:\cache\native-assets\abc123\lib\libuser32.a",
                r"C:\cache\native-assets\abc123\lib\libgdi32.a",
                r"C:\cache\native-assets\abc123\lib\libsecur32.a",
                r"C:\cache\native-assets\abc123\lib\libcrypt32.a",
                r"C:\cache\native-assets\abc123\lib\clang\libclang_rt.builtins-x86_64.a",
            ]
        );
    }

    #[test]
    fn mingw_direct_never_emits_forbidden_flags() {
        let plan = mingw_plan(all_five_optional_libs());
        let rendered = render_strings(&plan);
        for forbidden in ["-nostdlib", "-no-pie", "-fuse-ld=lld"] {
            assert!(
                !rendered.iter().any(|a| a == forbidden),
                "MingwDirect must never render {forbidden:?}, got {rendered:?}"
            );
        }
        assert!(
            !rendered
                .iter()
                .any(|a| a.starts_with("-l") || a.starts_with("-L")),
            "MingwDirect must pass import libs/builtins as absolute positional \
             archive inputs, never -l/-L, got {rendered:?}"
        );
    }

    #[test]
    fn mingw_direct_requests_all_five_optional_import_libs_when_present() {
        // The LLD-sees-all-optional-imports rule (design §2.4): MingwDirect
        // always requests every optional import lib regardless of what the
        // compiled program actually references.
        let plan = mingw_plan(all_five_optional_libs());
        let rendered = render_strings(&plan);
        for lib in ["ws2_32", "user32", "gdi32", "secur32", "crypt32"] {
            assert!(
                rendered.iter().any(|a| a.contains(lib)),
                "expected {lib} import library path in {rendered:?}"
            );
        }
    }

    #[test]
    fn mingw_direct_omits_a_system_lib_with_no_archive_path() {
        // A SystemLib without a resolved archive_path (e.g. an
        // override-linker path missing an optional asset) must not render
        // an empty/placeholder argument.
        let mut libs = all_five_optional_libs();
        libs.push(SystemLib {
            name: "unresolved",
            archive_path: None,
        });
        let plan = mingw_plan(libs);
        let rendered = render_strings(&plan);
        assert_eq!(
            rendered.iter().filter(|a| a.contains("unresolved")).count(),
            0
        );
    }

    fn compiler_driver_plan(runtime_mode: RuntimeMode, target: NativeTarget) -> LinkPlan {
        LinkPlan {
            flavor: LinkerFlavor::CompilerDriver,
            linker: LinkerExecutable::CompilerDriver {
                command: "clang".to_string(),
                source: CompilerSource::Host,
            },
            target,
            runtime_mode,
            output: PathBuf::from("/out/hello"),
            objects: vec![
                PathBuf::from("/obj/program.o"),
                PathBuf::from("/obj/shim.o"),
            ],
            archives: vec![PathBuf::from("/archives/libosc_runtime_freestanding.a")],
            system_libs: vec![SystemLib {
                name: "kernel32",
                archive_path: None,
            }],
            builtins: Some(PathBuf::from("/lib/libclang_rt.builtins-x86_64.a")),
            search_paths: vec![],
            extra_objects: vec![],
            entry: None,
            gc_sections: true,
            strip: runtime_mode == RuntimeMode::Freestanding,
            build_id_none: runtime_mode == RuntimeMode::Freestanding,
            pie: false,
            emulation: None,
            use_lld_driver_flag: false,
            show_warnings: false,
            extra_c_sources: vec![],
            include_dirs: vec![],
            passthrough_cflags: vec![],
            static_link: false,
            extra_libs: vec![],
        }
    }

    #[test]
    fn compiler_driver_freestanding_windows_renders_expected_argv() {
        let plan = compiler_driver_plan(RuntimeMode::Freestanding, NativeTarget::WindowsX64);
        let rendered = render_strings(&plan);
        assert_eq!(
            rendered,
            vec![
                "-nostdlib",
                "-s",
                "-Wl,--gc-sections,--build-id=none",
                "/obj/program.o",
                "/obj/shim.o",
                "/archives/libosc_runtime_freestanding.a",
                "-o",
                "/out/hello",
                "-lkernel32",
                "/lib/libclang_rt.builtins-x86_64.a",
            ]
        );
    }

    #[test]
    fn compiler_driver_hosted_non_windows_adds_no_pie_and_omits_build_id_and_strip() {
        let plan = compiler_driver_plan(RuntimeMode::Hosted, NativeTarget::LinuxX64);
        let rendered = render_strings(&plan);
        assert_eq!(
            rendered,
            vec![
                "-Wl,--gc-sections",
                "-no-pie",
                "/obj/program.o",
                "/obj/shim.o",
                "/archives/libosc_runtime_freestanding.a",
                "-o",
                "/out/hello",
                "-lkernel32",
            ]
        );
        // Hosted mode's compiler-builtins re-link is Windows-only.
        assert!(!rendered.iter().any(|a| a.contains("libclang_rt")));
    }

    #[test]
    fn compiler_driver_with_extra_c_sources_matches_prior_flag_set() {
        let mut plan = compiler_driver_plan(RuntimeMode::Freestanding, NativeTarget::WindowsX64);
        plan.extra_c_sources = vec![PathBuf::from("/user/extra.c")];
        plan.include_dirs = vec![PathBuf::from("/runtime")];
        plan.show_warnings = false;
        let rendered = render_strings(&plan);
        assert_eq!(
            rendered,
            vec![
                "-nostdlib",
                "-s",
                "-Wl,--gc-sections,--build-id=none",
                "-std=gnu11",
                "-ffreestanding",
                "-fno-builtin",
                "-w",
                "-I/runtime",
                "/obj/program.o",
                "/obj/shim.o",
                "/user/extra.c",
                "/archives/libosc_runtime_freestanding.a",
                "-o",
                "/out/hello",
                "-lkernel32",
                "/lib/libclang_rt.builtins-x86_64.a",
            ]
        );
    }

    #[test]
    fn compiler_driver_lld_flag_renders_first() {
        let mut plan = compiler_driver_plan(RuntimeMode::Freestanding, NativeTarget::WindowsX64);
        plan.use_lld_driver_flag = true;
        let rendered = render_strings(&plan);
        assert_eq!(rendered.first().map(String::as_str), Some("-fuse-ld=lld"));
    }

    /// Security review 2026-07-15 (finding 1): `manifest_link_flags` (a
    /// runtime archive manifest's raw, untrusted `link_flags` JSON array)
    /// no longer exists as a `LinkPlan` field at all, so there is no longer
    /// any way for archive-manifest content to reach this renderer's
    /// output — this test locks that structural guarantee in place by
    /// asserting `render()`'s output for a representative Linux freestanding
    /// plan contains only expected, code-computed tokens (in particular,
    /// none of the injection strings a malicious manifest used to be able
    /// to smuggle in verbatim: `-B`, an attacker directory, `-fplugin=`,
    /// `-Wl,-plugin,`, an `@response-file`, or `-o`/`--entry` overrides).
    #[test]
    fn compiler_driver_render_never_contains_former_manifest_injection_strings() {
        let plan = compiler_driver_plan(RuntimeMode::Freestanding, NativeTarget::LinuxX64);
        let rendered = render_strings(&plan);
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
                "expected {forbidden:?} to never appear (there is no field left to carry it), got {rendered:?}"
            );
        }
        // Exactly one "-o", contributed by the code-computed output path —
        // never a second, manifest-supplied one.
        assert_eq!(rendered.iter().filter(|a| a.as_str() == "-o").count(), 1);
    }

    /// Regression-proof for the `static_link` replacement (security review
    /// 2026-07-15, finding 1): Linux freestanding must still get `-static`
    /// (previously sourced from the manifest's `link_flags`, now hardcoded
    /// — see `LinkPlan::static_link`'s doc comment) after removing
    /// `manifest_link_flags` entirely.
    #[test]
    fn compiler_driver_static_link_renders_static_for_linux_freestanding() {
        let mut plan = compiler_driver_plan(RuntimeMode::Freestanding, NativeTarget::LinuxX64);
        plan.static_link = true;
        let rendered = render_strings(&plan);
        assert!(
            rendered.iter().any(|a| a == "-static"),
            "expected -static in {rendered:?}"
        );
    }

    #[test]
    fn compiler_driver_static_link_false_never_renders_static() {
        let plan = compiler_driver_plan(RuntimeMode::Hosted, NativeTarget::LinuxX64);
        assert!(!plan.static_link);
        let rendered = render_strings(&plan);
        assert!(!rendered.iter().any(|a| a == "-static"));
    }

    // --- ElfDirect tests (design §10.3) ---

    fn elf_direct_plan() -> LinkPlan {
        LinkPlan {
            flavor: LinkerFlavor::ElfDirect,
            linker: LinkerExecutable::Embedded {
                path: PathBuf::from("/cache/native-assets/abc123/linker/x86_64-linux-musl-ld"),
            },
            target: NativeTarget::LinuxX64,
            runtime_mode: RuntimeMode::Freestanding,
            output: PathBuf::from("/out/hello"),
            objects: vec![PathBuf::from("/obj/program.o")],
            archives: vec![PathBuf::from(
                "/archives/libosc_runtime_freestanding_core.a",
            )],
            system_libs: vec![],
            builtins: None,
            search_paths: vec![],
            extra_objects: vec![],
            entry: None,
            gc_sections: true,
            strip: true,
            build_id_none: true,
            pie: false,
            emulation: Some("elf_x86_64"),
            use_lld_driver_flag: false,
            show_warnings: false,
            extra_c_sources: vec![],
            include_dirs: vec![],
            passthrough_cflags: vec![],
            static_link: false,
            extra_libs: vec![],
        }
    }

    /// Exact-argv snapshot per design §10.3 — the primary "did we build the
    /// right command" surface for ElfDirect. Any drift here is a byte-parity
    /// regression against the proven 4,744-byte `hello.osc` Linux output.
    #[test]
    fn elf_direct_renders_exact_argv_in_locked_order() {
        let plan = elf_direct_plan();
        let rendered = render_strings(&plan);
        assert_eq!(
            rendered,
            vec![
                "-s",
                "-m",
                "elf_x86_64",
                "-static",
                "--gc-sections",
                "--build-id=none",
                "-o",
                "/out/hello",
                "/obj/program.o",
                "/archives/libosc_runtime_freestanding_core.a",
            ]
        );
    }

    #[test]
    fn user_static_libraries_precede_runtime_archives_in_every_linker_flavor() {
        let user_lib = ExtraLib::Path(if cfg!(windows) {
            PathBuf::from(r"C:\libs\user.lib")
        } else {
            PathBuf::from("/libs/libuser.a")
        });
        let user_lib_rendered = match &user_lib {
            ExtraLib::Path(path) => path.to_string_lossy().into_owned(),
            ExtraLib::SystemName(name) => format!("-l{name}"),
        };

        let mut plans = vec![
            mingw_plan(all_five_optional_libs()),
            compiler_driver_plan(RuntimeMode::Freestanding, NativeTarget::LinuxX64),
            elf_direct_plan(),
        ];
        for plan in &mut plans {
            plan.extra_libs.push(user_lib.clone());
            let runtime = plan.archives[0].to_string_lossy().into_owned();
            let rendered = render_strings(plan);
            let user_index = rendered
                .iter()
                .position(|arg| arg == &user_lib_rendered)
                .expect("user library must be rendered");
            let runtime_index = rendered
                .iter()
                .position(|arg| arg == &runtime)
                .expect("runtime archive must be rendered");
            assert!(
                user_index < runtime_index,
                "{:?} rendered the user library after the runtime archive: {rendered:?}",
                plan.flavor
            );
        }
    }

    #[test]
    fn compiler_driver_renders_extra_system_library_names_before_runtime_archive() {
        let mut plan = compiler_driver_plan(RuntimeMode::Hosted, NativeTarget::WindowsX64);
        plan.extra_libs = vec![
            ExtraLib::SystemName("winhttp".to_string()),
            ExtraLib::SystemName("ws2_32".to_string()),
        ];
        let rendered = render_strings(&plan);
        let winhttp_index = rendered
            .iter()
            .position(|arg| arg == "-lwinhttp")
            .expect("winhttp system library must render as -lwinhttp");
        let ws2_index = rendered
            .iter()
            .position(|arg| arg == "-lws2_32")
            .expect("ws2_32 system library must render as -lws2_32");
        let runtime_index = rendered
            .iter()
            .position(|arg| arg == "/archives/libosc_runtime_freestanding.a")
            .expect("runtime archive must be rendered");
        assert!(winhttp_index < runtime_index);
        assert!(ws2_index < runtime_index);
    }

    #[test]
    fn elf_direct_never_emits_forbidden_flags() {
        let plan = elf_direct_plan();
        let rendered = render_strings(&plan);
        for forbidden in ["-nostdlib", "-no-pie", "-fuse-ld=lld"] {
            assert!(
                !rendered.iter().any(|a| a == forbidden),
                "ElfDirect must never render {forbidden:?}, got {rendered:?}"
            );
        }
        assert!(
            !rendered
                .iter()
                .any(|a| a.starts_with("-l") || a.starts_with("-L")),
            "ElfDirect must never render -l/-L flags, got {rendered:?}"
        );
        assert!(
            !rendered
                .iter()
                .any(|a| a.starts_with("--entry") || a == "-e"),
            "ElfDirect must never render --entry/-e flags, got {rendered:?}"
        );
    }
}
