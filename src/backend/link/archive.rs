//! Runtime-archive discovery/build and manifest parsing.
//!
//! Moved from the pre-split `link.rs`. See `super::mod` module docs for the
//! "Explicit runtime modes" and "Freestanding runtime profiles" rationale
//! behind [`find_or_build_runtime_archive`] and [`FreestandingProfile`].

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::super::target::NativeTarget;
use super::super::RuntimeMode;
use super::capability::FreestandingProfile;
use super::is_verbose;

const FREESTANDING_ARCHIVE_NAME: &str = "libosc_runtime_freestanding.a";
const FREESTANDING_CORE_ARCHIVE_NAME: &str = "libosc_runtime_freestanding_core.a";
const HOSTED_ARCHIVE_NAME: &str = "libosc_runtime_hosted.a";

/// Candidate repository/checkout roots to search for `native-runtime/`,
/// `runtime/`, and `build/runtime-archives/`: ancestors of the running
/// `oscan` binary (covering both an installed release bundle and a
/// `target/{debug,release}/oscan(.exe)` dev layout), then the compile-time
/// `CARGO_MANIFEST_DIR`, then the current directory as a last resort.
///
/// Executable-relative roots come first so an installed release always uses
/// its packaged runtime assets rather than a checkout that merely happens to
/// remain at the path where that compiler binary was built.
pub(super) fn repo_root_candidates() -> Vec<PathBuf> {
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

pub(super) fn find_runtime_source_dir() -> Option<PathBuf> {
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

/// Locate a *trusted* `scripts/release_tools.py` to auto-build a missing
/// runtime archive from. Deliberately **not** built from
/// [`repo_root_candidates`] — that helper's CWD/exe-ancestor search is fine
/// for locating `runtime/` *data* files, but this path is `Command`-
/// **executed**, and an attacker who gets a victim to run `oscan` from a
/// directory containing a planted `scripts/release_tools.py` must never
/// have it picked up (the original vulnerability this function replaces).
///
/// Exactly two trusted sources, in order:
/// 1. `OSCAN_RUNTIME_BUILDER` — an explicit opt-in env var pointing
///    directly at a `release_tools.py` file. If set but not a real file,
///    this is a hard error (never silently falls through to source 2).
/// 2. `CARGO_MANIFEST_DIR` (a compile-time constant baked in by the same
///    trusted process that built this binary — never influenced by
///    runtime CWD/env) joined with `scripts/release_tools.py`, but *only*
///    when this build did not embed release assets
///    (`embedded_assets_present == false`). An installed/embedded release
///    build fails closed here — no auto-build at all; it must rely on
///    `OSCAN_RUNTIME_ARCHIVE_DIR` or a shipped prebuilt archive instead.
fn find_release_tools_script() -> Result<Option<PathBuf>, String> {
    find_release_tools_script_with(crate::backend::native_assets::EMBEDDED_ASSETS_PRESENT)
}

/// Testable core of [`find_release_tools_script`], parameterized over the
/// embedded-assets flag so a test can simulate an installed/embedded
/// release build without needing to actually produce one (mirrors how
/// [`super::super::native_assets::ensure_extracted_in`] is parameterized
/// for testability).
fn find_release_tools_script_with(
    embedded_assets_present: bool,
) -> Result<Option<PathBuf>, String> {
    if let Some(explicit) = env_var_nonempty("OSCAN_RUNTIME_BUILDER") {
        let path = PathBuf::from(&explicit);
        return if path.is_file() {
            Ok(Some(path))
        } else {
            Err(format!(
                "OSCAN_RUNTIME_BUILDER='{explicit}' is set, but that path does not exist (it must point \
                 directly at a release_tools.py file)"
            ))
        };
    }
    if embedded_assets_present {
        // Installed/release build: fail closed, never auto-build from a
        // trusted-looking-but-actually-CWD-relative script.
        return Ok(None);
    }
    match option_env!("CARGO_MANIFEST_DIR") {
        Some(manifest_dir) => {
            let candidate = PathBuf::from(manifest_dir)
                .join("scripts")
                .join("release_tools.py");
            Ok(candidate.is_file().then_some(candidate))
        }
        None => Ok(None),
    }
}

fn env_var_nonempty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub(super) fn archive_name(
    runtime_mode: RuntimeMode,
    profile: FreestandingProfile,
) -> &'static str {
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
pub(super) fn find_or_build_runtime_archive(
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

    let script = match find_release_tools_script()? {
        Some(script) => script,
        None => {
            return Err(if crate::backend::native_assets::EMBEDDED_ASSETS_PRESENT {
                format!(
                    "no {runtime_mode} Oscan runtime archive found (build/runtime-archives/<target>/{archive_name}) \
                     and this is an installed/release oscan build (it embeds its own native-link assets), which \
                     never auto-builds a runtime archive from scripts/release_tools.py; set \
                     OSCAN_RUNTIME_ARCHIVE_DIR to a directory containing {archive_name}, or reinstall/update to a \
                     release that ships one"
                )
            } else {
                format!(
                    "no {runtime_mode} Oscan runtime archive found (build/runtime-archives/<target>/{archive_name}) \
                     and no trusted scripts/release_tools.py was found to build one (only CARGO_MANIFEST_DIR's own \
                     scripts/release_tools.py is trusted for a dev build — never the current directory); run \
                     scripts/build-runtime-archive.ps1|.sh -Mode {runtime_mode} first, set OSCAN_RUNTIME_ARCHIVE_DIR \
                     to a directory containing {archive_name}, or set OSCAN_RUNTIME_BUILDER to an explicit, \
                     trusted release_tools.py path"
                )
            });
        }
    };
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

/// Parsed subset of a runtime archive's build manifest
/// (`<archive-stem>.json`, written by `scripts/release_tools.py`'s
/// `build_runtime_archive`).
#[derive(Debug, Default, PartialEq)]
pub(super) struct RuntimeArchiveManifest {
    pub(super) cc: Option<String>,
    pub(super) cc_family: Option<String>,
    pub(super) cc_version: Option<String>,
    pub(super) cc_target: Option<String>,
    pub(super) linker_family: Option<String>,
    pub(super) link_flags: Vec<String>,
    /// Design §3.3: parsed from `"contains_native_shim"`; `false` if absent
    /// (a legacy, pre-schema-2 archive that never precompiled
    /// `osc_native_shim.c` into itself). See [`shim_source_for`] for the
    /// policy this drives.
    pub(super) contains_native_shim: bool,
    /// Design §3.2: the `ar` member name of the precompiled shim, e.g.
    /// `"osc_native_shim.o"`. Informational only today (the linker
    /// resolves it automatically via normal archive-member symbol
    /// resolution); kept for diagnostics.
    pub(super) native_shim_member: Option<String>,
    /// `toolchain.version` (design §3.3/§8.3) — cross-checked against the
    /// embedded asset manifest's own `toolchain.version` (design §4.3).
    pub(super) toolchain_version: Option<String>,
}

pub(super) fn parse_runtime_manifest(text: &str) -> Option<RuntimeArchiveManifest> {
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
    // Absent/missing -> false: a legacy pre-schema-2 archive (design §3.3).
    let contains_native_shim = match object.get("contains_native_shim") {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(_) => return None,
        None => false,
    };
    let native_shim_member = object
        .get("native_shim_member")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let toolchain_version = toolchain
        .and_then(|value| value.get("version"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);

    Some(RuntimeArchiveManifest {
        cc,
        cc_family,
        cc_version,
        cc_target,
        linker_family,
        link_flags,
        contains_native_shim,
        native_shim_member,
        toolchain_version,
    })
}

/// Security review 2026-07-15 (finding 1): `link_flags` is intentionally
/// **not** exposed via a standalone accessor anymore — it must never flow
/// into a `Command`'s argv again (see `LinkPlan::static_link`'s doc comment
/// for the replacement, hardcoded behavior). Callers that need to know
/// whether an archive's manifest still carries legacy `link_flags` (purely
/// to print a count-only diagnostic, never the content) read
/// [`RuntimeArchiveManifest::link_flags`] directly off [`read_manifest`]'s
/// result instead of through a dedicated function, so there is no
/// "convenient" accessor left lying around for a future change to
/// accidentally start trusting again.
///
/// The runtime archive's build manifest sits next to it with the same
/// stem (`libosc_runtime_freestanding.a` -> `libosc_runtime_freestanding.json`),
/// matching `scripts/release_tools.py`'s `build_runtime_archive` (its
/// `mode_spec["manifest_name"]` is always `{archive stem}.json`).
pub(super) fn read_manifest(archive_path: &Path) -> Option<RuntimeArchiveManifest> {
    let manifest_path = archive_path.with_extension("json");
    let text = fs::read_to_string(&manifest_path).ok()?;
    parse_runtime_manifest(&text)
}

/// Whether the native shim comes pre-baked in the runtime archive, or must
/// be compiled locally as a diagnosed, hosted-only legacy fallback.
/// Implements the exact policy table in design §3.4.
#[derive(Debug)]
pub(super) enum ShimSource {
    /// `contains_native_shim: true` — the archive already has an
    /// `osc_native_shim.o` member; normal archive-member symbol resolution
    /// pulls it in automatically. No separate shim object is added to
    /// `LinkPlan.objects`.
    ArchiveMember,
    /// Legacy (`contains_native_shim: false`/absent) archive, hosted mode
    /// only: diagnosed local `compile_shim_object` fallback (design §3.4).
    CompileLocally,
}

pub(super) fn resolve_shim_source(
    runtime_mode: RuntimeMode,
    archive_path: &Path,
    manifest: Option<&RuntimeArchiveManifest>,
) -> Result<ShimSource, String> {
    let contains_shim = manifest.map(|m| m.contains_native_shim).unwrap_or(false);
    if contains_shim {
        return Ok(ShimSource::ArchiveMember);
    }
    match runtime_mode {
        RuntimeMode::Freestanding => Err(format!(
            "runtime archive '{}' predates the precompiled native shim (manifest contains_native_shim \
             is false/absent); rebuild it with 'scripts/build-runtime-archive.ps1 -Mode freestanding' \
             (or fetch a current release). The freestanding native backend no longer compiles \
             osc_native_shim.c locally.",
            archive_path.display()
        )),
        RuntimeMode::Hosted => {
            eprintln!(
                "warning: runtime archive '{}' predates the precompiled native shim (manifest \
                 contains_native_shim is false/absent); falling back to compiling \
                 runtime/osc_native_shim.c locally for this hosted build (rebuild the archive with \
                 'scripts/build-runtime-archive.ps1 -Mode hosted' to avoid this every time)",
                archive_path.display()
            );
            Ok(ShimSource::CompileLocally)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

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
        assert!(!manifest.contains_native_shim);
        assert_eq!(manifest.native_shim_member, None);
    }

    #[test]
    fn runtime_manifest_is_parsed_as_json() {
        let manifest = parse_runtime_manifest(
            r#"{
                "cc":"C:\\toolchains\\clang.exe",
                "toolchain":{
                    "version": "20260324",
                    "compiler":{
                        "family":"clang",
                        "version":"clang version 22.1.2",
                        "target":"x86_64-w64-windows-gnu"
                    },
                    "linker":{"family":"lld"}
                },
                "link_flags":["-Wl,\"quoted\""],
                "contains_native_shim": true,
                "native_shim_member": "osc_native_shim.o"
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
        assert!(manifest.contains_native_shim);
        assert_eq!(
            manifest.native_shim_member.as_deref(),
            Some("osc_native_shim.o")
        );
        assert_eq!(manifest.toolchain_version.as_deref(), Some("20260324"));
        assert!(parse_runtime_manifest(r#"{"link_flags":["unterminated]}"#).is_none());
    }

    #[test]
    fn contains_native_shim_absent_parses_as_false() {
        // Legacy, pre-schema-2 archives never had this field at all.
        let manifest = parse_runtime_manifest(r#"{"cc":"gcc"}"#).expect("valid manifest");
        assert!(!manifest.contains_native_shim);
    }

    #[test]
    fn shim_policy_hard_errors_for_freestanding_legacy_archive() {
        let manifest = RuntimeArchiveManifest {
            contains_native_shim: false,
            ..Default::default()
        };
        let err = resolve_shim_source(
            RuntimeMode::Freestanding,
            Path::new("libosc_runtime_freestanding.a"),
            Some(&manifest),
        )
        .expect_err("legacy freestanding archive must hard error, never fall back to a compiler");
        assert!(err.contains("contains_native_shim"));
        assert!(err.contains("rebuild"));
    }

    #[test]
    fn shim_policy_falls_back_locally_for_hosted_legacy_archive() {
        let manifest = RuntimeArchiveManifest {
            contains_native_shim: false,
            ..Default::default()
        };
        let source = resolve_shim_source(
            RuntimeMode::Hosted,
            Path::new("libosc_runtime_hosted.a"),
            Some(&manifest),
        )
        .expect("hosted legacy archive is a diagnosed fallback, not an error");
        assert!(matches!(source, ShimSource::CompileLocally));
    }

    #[test]
    fn shim_policy_uses_archive_member_when_present_for_either_mode() {
        let manifest = RuntimeArchiveManifest {
            contains_native_shim: true,
            ..Default::default()
        };
        for mode in [RuntimeMode::Freestanding, RuntimeMode::Hosted] {
            let source = resolve_shim_source(mode, Path::new("archive.a"), Some(&manifest))
                .expect("contains_native_shim: true never errors");
            assert!(matches!(source, ShimSource::ArchiveMember));
        }
    }

    // --- Finding 1: find_release_tools_script must never execute a
    // --- CWD-planted script. ---

    /// Serializes *all four* tests below that touch `OSCAN_RUNTIME_BUILDER`
    /// and/or the process current directory, both global process state
    /// shared by every test thread in this binary. Two of the four tests
    /// also change CWD and go through `CwdGuard::enter`, which acquires
    /// this same lock; the other two only mutate the env var and acquire
    /// it directly. All must hold the lock for the full set/remove
    /// critical section (through their final assertion), otherwise one
    /// test's `env::set_var`/`remove_var` can race with another's, as
    /// happened before this lock covered all four tests. `CwdGuard` is
    /// the only place in the crate that changes the current directory;
    /// the guard restores it before returning. See
    /// `.squad/decisions/inbox/bishop-security-fixes.md` for the
    /// trade-off this accepts.
    static RUNTIME_BUILDER_ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct CwdGuard {
        original: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl CwdGuard {
        fn enter(dir: &Path) -> Self {
            let lock = RUNTIME_BUILDER_ENV_TEST_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let original = env::current_dir().expect("current dir must be readable");
            env::set_current_dir(dir).expect("must be able to chdir into the fixture dir");
            CwdGuard {
                original,
                _lock: lock,
            }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = env::set_current_dir(&self.original);
        }
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oscan-archive-rs-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// Plants a `scripts/release_tools.py` that would write a marker file
    /// if it were ever executed (it never should be, from this function).
    fn plant_malicious_release_tools_script(dir: &Path) -> PathBuf {
        let scripts_dir = dir.join("scripts");
        fs::create_dir_all(&scripts_dir).expect("create scripts dir");
        let marker = dir.join("EXECUTED.marker");
        let script = scripts_dir.join("release_tools.py");
        fs::write(
            &script,
            format!(
                "import pathlib\npathlib.Path(r'{}').write_text('executed')\n",
                marker.display()
            ),
        )
        .expect("write malicious script");
        script
    }

    #[test]
    fn find_release_tools_script_never_returns_a_cwd_planted_script_dev_build() {
        let dir = scratch_dir("malicious-cwd-dev");
        let malicious_script = plant_malicious_release_tools_script(&dir);
        let marker = dir.join("EXECUTED.marker");

        env::remove_var("OSCAN_RUNTIME_BUILDER");
        let _guard = CwdGuard::enter(&dir);

        let result = find_release_tools_script_with(false)
            .expect("no explicit builder override set — must not hard error");
        if let Some(found) = &result {
            assert_ne!(
                found, &malicious_script,
                "must never resolve to the CWD-planted scripts/release_tools.py"
            );
        }
        assert!(
            !marker.exists(),
            "the planted script must never have been executed"
        );

        drop(_guard);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_release_tools_script_returns_none_for_an_embedded_release_build_even_with_cwd_planted_script(
    ) {
        let dir = scratch_dir("malicious-cwd-embedded");
        let malicious_script = plant_malicious_release_tools_script(&dir);
        let marker = dir.join("EXECUTED.marker");

        env::remove_var("OSCAN_RUNTIME_BUILDER");
        let _guard = CwdGuard::enter(&dir);

        // Simulating EMBEDDED_ASSETS_PRESENT == true (installed/release
        // build): must fail closed with no auto-build at all, never falling
        // through to the CWD-planted script.
        let result = find_release_tools_script_with(true)
            .expect("an installed/release build must not hard error here, just find nothing");
        assert!(
            result.is_none(),
            "an embedded/release build must never auto-discover a release_tools.py, found: {result:?}"
        );
        let _ = malicious_script; // never touched
        assert!(
            !marker.exists(),
            "the planted script must never have been executed"
        );

        drop(_guard);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_release_tools_script_errors_clearly_on_a_bad_explicit_builder_override() {
        let _lock = RUNTIME_BUILDER_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = scratch_dir("bad-builder-override");
        let bogus = dir.join("does-not-exist").join("release_tools.py");
        env::set_var("OSCAN_RUNTIME_BUILDER", &bogus);

        let err = find_release_tools_script_with(false)
            .expect_err("a set-but-nonexistent OSCAN_RUNTIME_BUILDER must hard error");
        assert!(err.contains("OSCAN_RUNTIME_BUILDER"));

        env::remove_var("OSCAN_RUNTIME_BUILDER");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_release_tools_script_honors_a_valid_explicit_builder_override() {
        let _lock = RUNTIME_BUILDER_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = scratch_dir("good-builder-override");
        let script = plant_malicious_release_tools_script(&dir); // just needs to be *a* real file
        env::set_var("OSCAN_RUNTIME_BUILDER", &script);

        let found = find_release_tools_script_with(true)
            .expect("must not error")
            .expect("an explicit valid override must be honored even for an embedded build");
        assert_eq!(found, script);

        env::remove_var("OSCAN_RUNTIME_BUILDER");
        let _ = fs::remove_dir_all(&dir);
    }
}
