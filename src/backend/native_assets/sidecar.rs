//! Verified, executable-relative **sidecar** native-link assets.
//!
//! A backend-specific release package (an LLVM- or Cranelift-only build,
//! see [`crate::backend::select`]) does not embed its linker/runtime files
//! in the compiler binary. Embedding them there and *also* shipping them
//! next to the executable — which Windows needs anyway, because
//! `libLLVM-22.dll` is both `ld.lld.exe`'s runtime dependency and the
//! packaged LLVM code generator — would duplicate ~90 MB per package. So
//! such a package stages exactly the same
//! [`super::EMBEDDED_ASSET_MANIFEST_JSON`] format
//! (`native-link-assets.json`, written by `scripts/release_tools.py
//! prepare-embed-assets`) plus its asset files under one fixed directory
//! beside the executable:
//!
//! ```text
//! <exe-dir>/
//!   oscan(.exe)
//!   native-link/
//!     native-link-assets.json
//!     bin/ld.lld.exe, bin/libLLVM-22.dll, ...   (install_subpath layout)
//!     lib/libkernel32.a, ...
//!   build/runtime-archives/<target>/libosc_runtime_*.a
//! ```
//!
//! # Trust rules
//!
//! * **One fixed, absolute, executable-relative location.** The manifest is
//!   only ever read from `<exe-dir>/native-link/native-link-assets.json`.
//!   Never the current directory, never `PATH`, never an ancestor search —
//!   these files are *executed* (the linker) and *loaded* (the LLVM code
//!   generator).
//! * **Absent means absent.** If the manifest file does not exist, this
//!   module reports `Ok(None)` and the caller keeps its existing behavior
//!   (embedded assets in a full release, a C toolchain in a dev build).
//! * **Present means verified.** If it exists, every failure — malformed
//!   JSON, unknown schema, unsafe `install_subpath`, a file that is not a
//!   regular file, a symlink/junction escape out of the sidecar root, a
//!   missing file, a duplicate entry, an unknown role, or a SHA-256
//!   mismatch — is a hard, named error. There is no fallback to embedded
//!   assets, to a compiler driver, or to anything found on the host.
//! * **Used in place.** Verified sidecar files are consumed where they are
//!   staged; they are never copied into the extraction cache, because
//!   copying them is exactly the duplication this mechanism removes. The
//!   embedded-asset cache and its hardening are untouched.

use std::fs;
use std::path::{Component, Path, PathBuf};

use super::{sha256_hex_of_file, AssetSource, ExtractedAsset, ExtractedAssetSet};

/// The one directory name a package may stage native-link assets under.
pub const SIDECAR_DIR_NAME: &str = "native-link";

/// The manifest file name, identical to the staged/embedded one.
pub const MANIFEST_FILE_NAME: &str = "native-link-assets.json";

/// The fixed, executable-relative runtime-archive root a package stages
/// its prebuilt runtime archives under: `<exe-dir>/build/runtime-archives/
/// <target>/libosc_runtime_*.a`. The current `native-link-assets.json`
/// schema does not describe runtime archives, so they remain a separate
/// package component with this fixed layout (matching
/// `packaging/toolchains/release-contract.json`'s
/// `archive-root/build/runtime-archives/{target}`); see
/// `super::super::link::archive::packaged_runtime_archive_root`.
pub const RUNTIME_ARCHIVE_SUBPATH: [&str; 2] = ["build", "runtime-archives"];

const SUPPORTED_SCHEMA_VERSION: u64 = 1;

/// The roles `prepare-embed-assets` emits. An unknown role is a hard
/// error rather than an ignored entry: a package that describes something
/// this compiler does not understand is not a package it may half-use.
const KNOWN_ROLES: [&str; 4] = [
    "linker",
    "linker_runtime",
    "import_lib",
    "compiler_builtins",
];

/// The minimum asset set a package must declare for a given target, so an
/// incomplete package fails on the manifest rather than at link time (or,
/// worse, at DLL-load time inside `ld.lld.exe`).
///
/// The import-library list is not restated here: it is taken from
/// [`crate::backend::link::required_import_libs`], the same list the link
/// plan itself requests, so the two cannot drift. The Windows runtime-DLL
/// list mirrors the pinned manifest
/// (`_WINDOWS_X86_64_EMBED_LINKER_RUNTIME` in
/// `scripts/release_tools.py`): `ld.lld.exe` is not statically linked and
/// Windows resolves its imports from its own directory first, so each of
/// these must be present *and* verified before it is executed. A package
/// that ships additional `linker_runtime` entries is fine — every declared
/// asset is verified regardless — this list only defines what may not be
/// *missing*.
struct RequiredAssets {
    /// `(role, name)` pairs that must be declared.
    named: &'static [(&'static str, &'static str)],
    /// Whether a `compiler_builtins` asset is required.
    compiler_builtins: bool,
    /// Whether this target's import libraries are required.
    import_libs: bool,
    /// Whether every `linker_runtime` asset must sit in the *same*
    /// directory as the linker. On Windows that placement is what makes
    /// the package work at all: the loader resolves an EXE's imports from
    /// the directory containing the EXE, so a DLL staged anywhere else
    /// would either not be found or be picked up from somewhere on the
    /// system instead.
    sibling_runtime: bool,
}

const WINDOWS_X86_64_REQUIRED: RequiredAssets = RequiredAssets {
    named: &[
        ("linker", "ld.lld.exe"),
        ("linker_runtime", "libLLVM-22.dll"),
        ("linker_runtime", "libc++.dll"),
        ("linker_runtime", "libunwind.dll"),
        ("linker_runtime", "libwinpthread-1.dll"),
        ("linker_runtime", "libffi-8.dll"),
    ],
    compiler_builtins: true,
    import_libs: true,
    sibling_runtime: true,
};

/// Linux packages ship a statically-linked `ld` from the musl-cross
/// toolchain and link freestanding objects with neither import libraries
/// nor compiler builtins (see `crate::backend::link`'s ELF plan), so the
/// linker itself is the whole required set.
const LINUX_REQUIRED: RequiredAssets = RequiredAssets {
    named: &[],
    compiler_builtins: false,
    import_libs: false,
    sibling_runtime: false,
};

fn required_assets_for(target: &str) -> &'static RequiredAssets {
    match target {
        "windows-x86_64" => &WINDOWS_X86_64_REQUIRED,
        _ => &LINUX_REQUIRED,
    }
}

/// One manifest entry, resolved to its absolute staged path (not yet
/// hashed — see [`SidecarPackage::verify_all`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarEntry {
    pub role: String,
    pub name: String,
    pub lib: Option<String>,
    pub install_subpath: String,
    pub sha256: String,
    pub path: PathBuf,
}

/// A structurally-validated sidecar package: every declared file exists as
/// a regular file inside the sidecar root. Contents are verified by
/// [`SidecarPackage::verify_all`] / [`SidecarPackage::verify_entry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarPackage {
    pub root: PathBuf,
    pub target: String,
    pub toolchain_version: Option<String>,
    pub entries: Vec<SidecarEntry>,
}

/// `<exe-dir>/native-link`.
pub fn root_for(exe_dir: &Path) -> PathBuf {
    exe_dir.join(SIDECAR_DIR_NAME)
}

/// `<exe-dir>/native-link/native-link-assets.json`.
pub fn manifest_path_for(exe_dir: &Path) -> PathBuf {
    root_for(exe_dir).join(MANIFEST_FILE_NAME)
}

/// The directory containing this process's executable, canonicalized.
/// Never the current directory: an unresolvable executable path is an
/// error, not a reason to look somewhere more convenient.
pub fn exe_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot resolve this executable's own path: {e}"))?;
    let exe = fs::canonicalize(&exe).unwrap_or(exe);
    exe.parent().map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "executable path '{}' has no parent directory",
            exe.display()
        )
    })
}

/// Whether a sidecar manifest *exists* beside this executable, without
/// validating it. A malformed one must still be a hard error at use time,
/// so this deliberately answers "is there a package here" rather than "is
/// there a usable package here".
pub fn is_present() -> bool {
    match exe_dir() {
        Ok(dir) => fs::symlink_metadata(manifest_path_for(&dir)).is_ok(),
        Err(_) => false,
    }
}

/// Load and structurally validate the sidecar package beside this
/// executable. `Ok(None)` only when no manifest file exists at all.
pub fn load() -> Result<Option<SidecarPackage>, String> {
    load_from(&exe_dir()?)
}

/// [`load`], parameterized over the executable directory so tests can
/// build a package layout in a temporary directory without touching this
/// process's own executable.
pub fn load_from(exe_dir: &Path) -> Result<Option<SidecarPackage>, String> {
    if !exe_dir.is_absolute() {
        return Err(format!(
            "refusing to look for native-link sidecar assets under relative path '{}': the \
             sidecar root is always an absolute, executable-relative directory",
            exe_dir.display()
        ));
    }
    let root = root_for(exe_dir);
    let manifest_path = manifest_path_for(exe_dir);
    // The sidecar root itself must be a real directory beside the
    // executable. A symlink/junction there could redirect the whole
    // package — every per-asset check below would then be checking files
    // inside somebody else's directory.
    match fs::symlink_metadata(&root) {
        Ok(meta) if meta.file_type().is_symlink() => return Err(sidecar_error(
            &root,
            "it is a symlink/reparse point (junction on Windows); the native-link directory is \
                 never followed through one",
        )),
        Ok(meta) if !meta.is_dir() => return Err(sidecar_error(&root, "it is not a directory")),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(sidecar_error(
                &root,
                &format!("cannot read its metadata: {e}"),
            ))
        }
    }
    let manifest_meta = match fs::symlink_metadata(&manifest_path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(sidecar_error(
                &manifest_path,
                &format!("cannot read its metadata: {e}"),
            ))
        }
    };
    if manifest_meta.file_type().is_symlink() {
        return Err(sidecar_error(
            &manifest_path,
            "it is a symlink/reparse point; the sidecar manifest is never followed through one",
        ));
    }
    if !manifest_meta.is_file() {
        return Err(sidecar_error(&manifest_path, "it is not a regular file"));
    }

    let canonical_root = fs::canonicalize(&root)
        .map_err(|e| sidecar_error(&root, &format!("cannot canonicalize it: {e}")))?;

    let text = fs::read_to_string(&manifest_path)
        .map_err(|e| sidecar_error(&manifest_path, &format!("cannot read it: {e}")))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| sidecar_error(&manifest_path, &format!("it is not valid JSON: {e}")))?;

    match value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
    {
        Some(SUPPORTED_SCHEMA_VERSION) => {}
        Some(other) => {
            return Err(sidecar_error(
                &manifest_path,
                &format!(
                    "it declares schema_version {other}, but this compiler only understands \
                     version {SUPPORTED_SCHEMA_VERSION}"
                ),
            ))
        }
        None => {
            return Err(sidecar_error(
                &manifest_path,
                "it has no numeric 'schema_version' field",
            ))
        }
    }

    let target = value
        .get("target")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .ok_or_else(|| sidecar_error(&manifest_path, "it has no non-empty 'target' field"))?
        .to_string();

    let toolchain_version = value
        .get("toolchain")
        .and_then(|toolchain| toolchain.get("version"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    let mut raw_entries: Vec<&serde_json::Value> = Vec::new();
    let linker = value
        .get("linker")
        .ok_or_else(|| sidecar_error(&manifest_path, "it has no 'linker' entry"))?;
    raw_entries.push(linker);
    let assets = value
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| sidecar_error(&manifest_path, "it has no 'assets' array"))?;
    raw_entries.extend(assets.iter());

    let mut entries: Vec<SidecarEntry> = Vec::with_capacity(raw_entries.len());
    for raw in raw_entries {
        let entry = read_entry(&manifest_path, raw)?;
        if entries
            .iter()
            .any(|seen| seen.install_subpath == entry.install_subpath)
        {
            return Err(sidecar_error(
                &manifest_path,
                &format!(
                    "it lists '{}' more than once; every asset must appear exactly once",
                    entry.install_subpath
                ),
            ));
        }
        if entries.iter().any(|seen| {
            seen.role == entry.role && seen.lib == entry.lib && seen.role != "linker_runtime"
        }) {
            return Err(sidecar_error(
                &manifest_path,
                &format!(
                    "it lists two '{}' entries for the same library ({:?}); the role/lib pair must \
                     identify exactly one asset",
                    entry.role, entry.lib
                ),
            ));
        }
        entries.push(entry);
    }
    if !entries.iter().any(|entry| entry.role == "linker") {
        return Err(sidecar_error(
            &manifest_path,
            "it declares no 'linker' role asset",
        ));
    }

    let package = SidecarPackage {
        root,
        target,
        toolchain_version,
        entries,
    };
    for entry in &package.entries {
        package.check_staged_file(&canonical_root, entry)?;
    }
    package.check_required_assets(&manifest_path)?;
    Ok(Some(package))
}

impl SidecarPackage {
    /// Hash-verify every declared asset and return them in the same shape
    /// the embedded-asset path produces, so the link plan consumes both
    /// sources through one type.
    pub fn verify_all(&self) -> Result<ExtractedAssetSet, String> {
        let mut assets = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            self.verify_entry(entry)?;
            assets.push(ExtractedAsset {
                role: entry.role.clone(),
                name: entry.name.clone(),
                lib: entry.lib.clone(),
                path: entry.path.clone(),
            });
        }
        Ok(ExtractedAssetSet {
            source: AssetSource::Sidecar,
            dir: self.root.clone(),
            assets,
        })
    }

    /// Hash-verify one declared asset.
    pub fn verify_entry(&self, entry: &SidecarEntry) -> Result<(), String> {
        let actual = sha256_hex_of_file(&entry.path)?;
        if !actual.eq_ignore_ascii_case(&entry.sha256) {
            return Err(sidecar_error(
                &entry.path,
                &format!(
                    "its SHA-256 is {actual}, but native-link-assets.json declares {}; this \
                     package is corrupt or has been tampered with",
                    entry.sha256
                ),
            ));
        }
        Ok(())
    }

    /// Hash-verify every file that will be *implicitly* loaded alongside
    /// `entry` — on Windows, `ld.lld.exe` and `libLLVM-22.dll` both pull in
    /// their sibling DLLs from their own directory, and the operating
    /// system never asks this compiler whether those bytes are the ones
    /// the manifest describes. Called immediately before a linker is
    /// executed or a code generator is loaded, not only when the primary
    /// file is resolved.
    pub fn verify_runtime_closure(&self) -> Result<(), String> {
        for entry in self
            .entries
            .iter()
            .filter(|entry| entry.role == "linker_runtime")
        {
            self.verify_entry(entry)?;
        }
        Ok(())
    }

    /// The directory the linker and every implicitly-loaded runtime
    /// library share, canonicalized. This is the only directory a
    /// packaged code generator may be loaded from.
    pub fn runtime_dir(&self) -> Option<PathBuf> {
        let linker = self.entries.iter().find(|entry| entry.role == "linker")?;
        let parent = linker.path.parent()?;
        fs::canonicalize(parent).ok()
    }

    /// Placement check: the linker and every `linker_runtime` asset must
    /// live in one canonical directory. Import libraries and compiler
    /// builtins are inputs the linker is *given* by path, so they keep
    /// their own `lib/` layout and are not covered here.
    fn check_runtime_placement(&self, manifest_path: &Path) -> Result<(), String> {
        let linker = self
            .entries
            .iter()
            .find(|entry| entry.role == "linker")
            .ok_or_else(|| sidecar_error(manifest_path, "it declares no 'linker' role asset"))?;
        let linker_dir = canonical_parent(&linker.path)?;
        for entry in self
            .entries
            .iter()
            .filter(|entry| entry.role == "linker_runtime")
        {
            let entry_dir = canonical_parent(&entry.path)?;
            if entry_dir != linker_dir {
                return Err(sidecar_error(
                    manifest_path,
                    &format!(
                        "it stages '{}' in '{}' but the linker '{}' in '{}'; every runtime library \
                         the linker loads implicitly must be its sibling in one directory, or the \
                         operating system will resolve it from somewhere else entirely",
                        entry.name,
                        entry.install_subpath,
                        linker.name,
                        linker.install_subpath
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The minimum-set check for this package's declared target: an
    /// omitted runtime DLL, import library, or compiler-builtins archive
    /// is a package defect, caught here rather than at load/link time.
    fn check_required_assets(&self, manifest_path: &Path) -> Result<(), String> {
        let required = required_assets_for(&self.target);
        for (role, name) in required.named {
            if !self
                .entries
                .iter()
                .any(|entry| entry.role == *role && entry.name == *name)
            {
                return Err(sidecar_error(
                    manifest_path,
                    &format!(
                        "it does not declare the required '{role}' asset '{name}' for target \
                         '{}'; this package is incomplete",
                        self.target
                    ),
                ));
            }
        }
        if required.sibling_runtime {
            self.check_runtime_placement(manifest_path)?;
        }
        if required.compiler_builtins
            && !self
                .entries
                .iter()
                .any(|entry| entry.role == "compiler_builtins")
        {
            return Err(sidecar_error(
                manifest_path,
                &format!(
                    "it declares no 'compiler_builtins' asset, which target '{}' requires; this \
                     package is incomplete",
                    self.target
                ),
            ));
        }
        if required.import_libs {
            for lib in crate::backend::link::required_import_libs() {
                if !self
                    .entries
                    .iter()
                    .any(|entry| entry.role == "import_lib" && entry.lib.as_deref() == Some(lib))
                {
                    return Err(sidecar_error(
                        manifest_path,
                        &format!(
                            "it does not declare the '{lib}' import library, which every link for \
                             target '{}' requests; this package is incomplete",
                            self.target
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// The entry a given absolute path corresponds to, if this package
    /// declares it.
    #[cfg_attr(not(feature = "backend-llvm"), allow(dead_code))]
    pub fn entry_for_path(&self, path: &Path) -> Option<&SidecarEntry> {
        let canonical = fs::canonicalize(path).ok()?;
        self.entries.iter().find(|entry| {
            fs::canonicalize(&entry.path)
                .map(|declared| declared == canonical)
                .unwrap_or(false)
        })
    }

    /// Structural checks for one staged file: it exists, is a regular file
    /// (never a symlink/junction), and stays inside the sidecar root after
    /// canonicalization.
    fn check_staged_file(&self, canonical_root: &Path, entry: &SidecarEntry) -> Result<(), String> {
        let meta = fs::symlink_metadata(&entry.path).map_err(|e| {
            sidecar_error(
                &entry.path,
                &format!(
                    "native-link-assets.json declares it as '{}', but it cannot be read: {e}",
                    entry.install_subpath
                ),
            )
        })?;
        if meta.file_type().is_symlink() {
            return Err(sidecar_error(
                &entry.path,
                "it is a symlink/reparse point; sidecar assets are never followed through one",
            ));
        }
        if !meta.is_file() {
            return Err(sidecar_error(&entry.path, "it is not a regular file"));
        }
        let canonical = fs::canonicalize(&entry.path)
            .map_err(|e| sidecar_error(&entry.path, &format!("cannot canonicalize it: {e}")))?;
        if !canonical.starts_with(canonical_root) {
            return Err(sidecar_error(
                &entry.path,
                &format!(
                    "it resolves to '{}', which is outside the sidecar root '{}'",
                    canonical.display(),
                    canonical_root.display()
                ),
            ));
        }
        Ok(())
    }
}

/// Guard for a path some *other* discovery mechanism produced (today: the
/// packaged LLVM provider search, which may find `libLLVM-22.dll` inside
/// the sidecar root because Windows shares one copy between the code
/// generator and `ld.lld.exe`).
///
/// * `Ok(())` when `path` is not inside the sidecar root — nothing here
///   claims anything about it.
/// * `Ok(())` when it *is* inside and the package declares that exact file
///   and its SHA-256 matches.
/// * `Err(..)` otherwise: a file inside the sidecar root that the manifest
///   does not vouch for is never loaded.
///
/// Only the packaged LLVM provider consults this today, so a build without
/// the LLVM backend has no non-test caller.
#[cfg_attr(not(feature = "backend-llvm"), allow(dead_code))]
pub fn require_verified_if_inside(path: &Path) -> Result<(), String> {
    let exe_dir = match exe_dir() {
        Ok(dir) => dir,
        // No executable path means no sidecar root to be inside of.
        Err(_) => return Ok(()),
    };
    require_verified_if_inside_from(&exe_dir, path)
}

/// [`require_verified_if_inside`], parameterized over the executable
/// directory for tests.
///
/// Containment is decided **lexically against the fixed root first**, and
/// only then cross-checked against canonical paths. Deciding it the other
/// way round would let a junction/symlink *inside* the sidecar directory
/// answer "this canonicalizes somewhere else, so it is not a sidecar
/// candidate" — exactly the redirect this guard exists to stop.
#[cfg_attr(not(feature = "backend-llvm"), allow(dead_code))]
pub fn require_verified_if_inside_from(exe_dir: &Path, path: &Path) -> Result<(), String> {
    let root = root_for(exe_dir);
    let lexically_inside = path.starts_with(&root);
    let canonical_root = fs::canonicalize(&root).ok();
    let canonical_path = fs::canonicalize(path).ok();
    let canonically_inside = match (&canonical_root, &canonical_path) {
        (Some(root), Some(path)) => path.starts_with(root),
        _ => false,
    };

    if !lexically_inside && !canonically_inside {
        // Not a sidecar candidate at all: this module makes no claim
        // about it either way.
        return Ok(());
    }
    if lexically_inside && canonical_path.is_some() && !canonically_inside {
        return Err(sidecar_error(
            path,
            "it lies inside the native-link sidecar directory but resolves outside it (a \
             symlink/junction redirect); such a redirect is never followed",
        ));
    }
    // A candidate that is itself a symlink/reparse point is refused even
    // when its target happens to stay inside the root: the manifest
    // describes files, not links to files.
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(sidecar_error(
                path,
                "it is a symlink/reparse point (junction on Windows); sidecar assets are never \
                 loaded through one",
            ));
        }
    }

    let package = load_from(exe_dir)?.ok_or_else(|| {
        sidecar_error(
            path,
            "it lies inside the native-link sidecar directory, but that directory has no \
             native-link-assets.json to verify it against",
        )
    })?;
    let entry = package.entry_for_path(path).ok_or_else(|| {
        sidecar_error(
            path,
            "it lies inside the native-link sidecar directory, but native-link-assets.json does \
             not declare it; unverified files there are never loaded",
        )
    })?;
    // The packaged code generator is one of the runtime libraries staged
    // beside the linker (that shared copy is the whole point of the
    // sidecar layout). A declared file with any other role — an import
    // library, the compiler-builtins archive, the linker itself — is not
    // something this process may load as a shared library.
    if entry.role != "linker_runtime" {
        return Err(sidecar_error(
            path,
            &format!(
                "native-link-assets.json declares it with role '{}', not 'linker_runtime'; only \
                 the runtime libraries staged beside the linker are ever loaded as shared \
                 libraries",
                entry.role
            ),
        ));
    }
    if let Some(runtime_dir) = package.runtime_dir() {
        let entry_dir = canonical_parent(&entry.path)?;
        if entry_dir != runtime_dir {
            return Err(sidecar_error(
                path,
                "it is not staged in the same directory as the packaged linker; the code \
                 generator and the linker must share one directory",
            ));
        }
    }
    package.verify_entry(entry)?;
    // Everything this file will implicitly pull in from the same
    // directory is verified in the same breath (finding: verifying only
    // the primary library leaves its siblings unchecked).
    package.verify_runtime_closure()
}

/// The canonical directory containing `path`.
fn canonical_parent(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| sidecar_error(path, "it has no parent directory"))?;
    fs::canonicalize(parent)
        .map_err(|e| sidecar_error(parent, &format!("cannot canonicalize it: {e}")))
}

fn read_entry(manifest_path: &Path, raw: &serde_json::Value) -> Result<SidecarEntry, String> {
    let field = |key: &str| -> Result<String, String> {
        raw.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                sidecar_error(
                    manifest_path,
                    &format!("one of its entries has no non-empty '{key}' field"),
                )
            })
    };
    let role = field("role")?;
    if !KNOWN_ROLES.contains(&role.as_str()) {
        return Err(sidecar_error(
            manifest_path,
            &format!(
                "it declares an asset with unknown role '{role}' (known roles: {})",
                KNOWN_ROLES.join(", ")
            ),
        ));
    }
    let name = field("name")?;
    let install_subpath = field("install_subpath")?;
    let sha256 = field("sha256")?;
    if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(sidecar_error(
            manifest_path,
            &format!("asset '{name}' has a malformed sha256 value '{sha256}'"),
        ));
    }
    let lib = raw
        .get("lib")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    let relative = validated_relative_subpath(&install_subpath).map_err(|reason| {
        sidecar_error(
            manifest_path,
            &format!("asset '{name}' has an unusable install_subpath: {reason}"),
        )
    })?;
    let root = manifest_path
        .parent()
        .expect("the manifest path always has the sidecar root as its parent")
        .to_path_buf();
    Ok(SidecarEntry {
        role,
        name,
        lib,
        install_subpath,
        sha256,
        path: root.join(relative),
    })
}

/// Reject absolute paths, drive letters/prefixes, and any `.`/`..`
/// component: only a strict relative path of plain segments is ever joined
/// onto the sidecar root. Same rule the embedded cache applies to its own
/// `install_subpath`s (see [`super::validated_dest`]).
fn validated_relative_subpath(install_subpath: &str) -> Result<PathBuf, String> {
    let raw = Path::new(install_subpath);
    for component in raw.components() {
        match component {
            Component::Normal(_) => {}
            other => {
                return Err(format!(
                    "'{install_subpath}' contains a disallowed path component ({other:?}); only a \
                     strict relative path with plain segments is permitted"
                ))
            }
        }
    }
    if raw.components().next().is_none() {
        return Err(format!("'{install_subpath}' is empty"));
    }
    Ok(raw.to_path_buf())
}

fn sidecar_error(path: &Path, reason: &str) -> String {
    format!(
        "packaged native-link assets are unusable: '{}' {reason}. This compiler only uses \
         native-link assets it can verify against '{SIDECAR_DIR_NAME}/{MANIFEST_FILE_NAME}' \
         beside its own executable, and never falls back to a C toolchain or to unverified \
         files; reinstall this package",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    struct Package {
        _dir: tempfile::TempDir,
        exe_dir: PathBuf,
    }

    impl Package {
        /// A temporary `<exe-dir>` with a `native-link/` sidecar staged in
        /// it — the same shape a release package has, without touching
        /// this process's own executable.
        fn new() -> Self {
            let dir = tempfile::Builder::new()
                .prefix("oscan-sidecar-test-")
                .tempdir()
                .expect("create temp package dir");
            let exe_dir = dir.path().to_path_buf();
            fs::create_dir_all(root_for(&exe_dir).join("bin")).expect("create sidecar bin dir");
            fs::create_dir_all(root_for(&exe_dir).join("lib")).expect("create sidecar lib dir");
            Package { _dir: dir, exe_dir }
        }

        fn stage(&self, install_subpath: &str, contents: &[u8]) -> String {
            let path = root_for(&self.exe_dir).join(install_subpath);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create staged asset dir");
            }
            fs::write(&path, contents).expect("write staged asset");
            hex(&Sha256::digest(contents))
        }

        fn write_manifest(&self, json: &str) {
            fs::write(manifest_path_for(&self.exe_dir), json).expect("write manifest");
        }

        /// A complete, valid Windows package: linker + the runtime DLLs
        /// `ld.lld.exe` loads from its own directory + every import
        /// library a link requests + compiler builtins.
        fn valid(target: &str) -> Self {
            let package = Package::new();
            let linker_name = if target == "windows-x86_64" {
                "ld.lld.exe"
            } else {
                "ld"
            };
            let linker = package.stage(&format!("bin/{linker_name}"), b"linker bytes");
            let mut assets = Vec::new();
            if target == "windows-x86_64" {
                for dll in [
                    "libLLVM-22.dll",
                    "libc++.dll",
                    "libunwind.dll",
                    "libwinpthread-1.dll",
                    "libffi-8.dll",
                ] {
                    let sha = package.stage(&format!("bin/{dll}"), dll.as_bytes());
                    assets.push(format!(
                        r#"    {{ "role": "linker_runtime", "name": "{dll}", "install_subpath": "bin/{dll}", "sha256": "{sha}" }}"#
                    ));
                }
                for lib in [
                    "kernel32", "ws2_32", "user32", "gdi32", "secur32", "crypt32",
                ] {
                    let sha = package.stage(&format!("lib/lib{lib}.a"), lib.as_bytes());
                    assets.push(format!(
                        r#"    {{ "role": "import_lib", "name": "lib{lib}.a", "lib": "{lib}", "install_subpath": "lib/lib{lib}.a", "sha256": "{sha}" }}"#
                    ));
                }
                let builtins =
                    package.stage("lib/clang/libclang_rt.builtins-x86_64.a", b"builtins");
                assets.push(format!(
                    r#"    {{ "role": "compiler_builtins", "name": "libclang_rt.builtins-x86_64.a", "install_subpath": "lib/clang/libclang_rt.builtins-x86_64.a", "sha256": "{builtins}" }}"#
                ));
            }
            package.write_manifest(&format!(
                r#"{{
  "schema_version": 1,
  "target": "{target}",
  "toolchain": {{ "vendor": "llvm-mingw", "version": "20250910" }},
  "linker": {{ "role": "linker", "name": "{linker_name}", "install_subpath": "bin/{linker_name}", "sha256": "{linker}" }},
  "assets": [
{}
  ]
}}"#,
                assets.join(",\n")
            ));
            package
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn load_err(package: &Package) -> String {
        load_from(&package.exe_dir).expect_err("this package must be rejected")
    }

    #[test]
    fn a_valid_package_is_discovered_and_verified_in_place() {
        let package = Package::valid("windows-x86_64");
        let loaded = load_from(&package.exe_dir)
            .expect("a valid package must load")
            .expect("a staged manifest must be found");

        assert_eq!(loaded.target, "windows-x86_64");
        assert_eq!(loaded.toolchain_version.as_deref(), Some("20250910"));
        assert_eq!(loaded.entries.len(), 13);
        assert_eq!(loaded.root, root_for(&package.exe_dir));

        let set = loaded.verify_all().expect("every hash must match");
        assert_eq!(set.source, AssetSource::Sidecar);
        // Used in place: the paths are the staged ones, not cache copies.
        assert_eq!(set.dir, root_for(&package.exe_dir));
        assert_eq!(
            set.linker().expect("linker role").path,
            root_for(&package.exe_dir).join("bin/ld.lld.exe")
        );
        assert!(set.compiler_builtins().is_some());
        assert_eq!(
            set.import_lib("kernel32").expect("import lib").path,
            root_for(&package.exe_dir).join("lib/libkernel32.a")
        );
        // Every import library the link plan requests is present...
        for lib in crate::backend::link::required_import_libs() {
            assert!(set.import_lib(lib).is_some(), "missing {lib}");
        }
        // ...and a library the package does not declare is simply absent.
        assert!(set.import_lib("advapi32").is_none());
    }

    #[test]
    fn an_absent_manifest_is_not_an_error_so_embedded_assets_still_apply() {
        let package = Package::new();
        assert_eq!(
            load_from(&package.exe_dir).expect("an absent sidecar is not a failure"),
            None
        );
    }

    #[test]
    fn a_malformed_manifest_is_a_hard_error() {
        let package = Package::new();
        package.write_manifest("{ this is not json");
        let err = load_err(&package);
        assert!(err.contains("not valid JSON"), "{err}");
        assert!(err.contains("never falls back"), "{err}");
    }

    #[test]
    fn an_unknown_schema_version_is_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        let text = fs::read_to_string(manifest_path_for(&package.exe_dir)).unwrap();
        package.write_manifest(&text.replace("\"schema_version\": 1", "\"schema_version\": 2"));
        let err = load_err(&package);
        assert!(err.contains("schema_version 2"), "{err}");
    }

    #[test]
    fn a_missing_target_is_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        let text = fs::read_to_string(manifest_path_for(&package.exe_dir)).unwrap();
        package.write_manifest(&text.replace("\"target\": \"windows-x86_64\",", ""));
        let err = load_err(&package);
        assert!(err.contains("'target' field"), "{err}");
    }

    /// The target is carried through so the caller can reject a package
    /// staged for another machine (the same gate embedded assets get).
    #[test]
    fn the_declared_target_is_reported_for_the_callers_target_gate() {
        let package = Package::valid("linux-aarch64");
        let loaded = load_from(&package.exe_dir).unwrap().unwrap();
        assert_eq!(loaded.target, "linux-aarch64");
        assert_ne!(loaded.target, "windows-x86_64");
    }

    #[test]
    fn a_traversing_install_subpath_is_a_hard_error() {
        for subpath in ["../escape.exe", "bin/../../escape.exe", "./bin/ld.lld.exe"] {
            let package = Package::valid("windows-x86_64");
            let text = fs::read_to_string(manifest_path_for(&package.exe_dir)).unwrap();
            package.write_manifest(&text.replace("bin/libLLVM-22.dll", subpath));
            let err = load_err(&package);
            assert!(
                err.contains("disallowed path component"),
                "{subpath}: {err}"
            );
        }
    }

    #[test]
    fn an_absolute_install_subpath_is_a_hard_error() {
        let absolute = if cfg!(windows) {
            r"C:\\Windows\\System32\\evil.dll"
        } else {
            "/etc/evil"
        };
        let package = Package::valid("windows-x86_64");
        let text = fs::read_to_string(manifest_path_for(&package.exe_dir)).unwrap();
        package.write_manifest(&text.replace("bin/libLLVM-22.dll", absolute));
        let err = load_err(&package);
        assert!(err.contains("disallowed path component"), "{err}");
    }

    #[test]
    fn a_missing_staged_file_is_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        fs::remove_file(root_for(&package.exe_dir).join("bin/libLLVM-22.dll")).unwrap();
        let err = load_err(&package);
        assert!(err.contains("libLLVM-22.dll"), "{err}");
        assert!(err.contains("cannot be read"), "{err}");
    }

    #[test]
    fn a_directory_in_place_of_an_asset_is_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        let path = root_for(&package.exe_dir).join("bin/libLLVM-22.dll");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let err = load_err(&package);
        assert!(err.contains("not a regular file"), "{err}");
    }

    #[test]
    fn a_checksum_mismatch_is_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        // Same length, different content: nothing memoizes a hash here.
        fs::write(
            root_for(&package.exe_dir).join("bin/ld.lld.exe"),
            b"LINKER BYTES",
        )
        .unwrap();
        let loaded = load_from(&package.exe_dir)
            .expect("structure is still valid")
            .unwrap();
        let err = loaded
            .verify_all()
            .expect_err("a content swap must be caught");
        assert!(err.contains("SHA-256"), "{err}");
        assert!(err.contains("corrupt or has been tampered with"), "{err}");
    }

    #[test]
    fn a_duplicate_entry_is_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        let text = fs::read_to_string(manifest_path_for(&package.exe_dir)).unwrap();
        let duplicated = text.replace(
            "\"assets\": [",
            "\"assets\": [\n    { \"role\": \"import_lib\", \"name\": \"libkernel32.a\", \"lib\": \"kernel32\", \"install_subpath\": \"lib/libkernel32.a\", \"sha256\": \"0000000000000000000000000000000000000000000000000000000000000000\" },",
        );
        package.write_manifest(&duplicated);
        let err = load_err(&package);
        assert!(err.contains("more than once"), "{err}");
    }

    #[test]
    fn an_unknown_role_is_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        let text = fs::read_to_string(manifest_path_for(&package.exe_dir)).unwrap();
        package
            .write_manifest(&text.replace("\"role\": \"linker_runtime\"", "\"role\": \"payload\""));
        let err = load_err(&package);
        assert!(err.contains("unknown role 'payload'"), "{err}");
    }

    #[test]
    fn a_manifest_without_a_linker_is_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        let text = fs::read_to_string(manifest_path_for(&package.exe_dir)).unwrap();
        package
            .write_manifest(&text.replace("\"role\": \"linker\"", "\"role\": \"linker_runtime\""));
        let err = load_err(&package);
        assert!(err.contains("no 'linker' role asset"), "{err}");
    }

    #[test]
    fn a_malformed_sha256_is_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        let text = fs::read_to_string(manifest_path_for(&package.exe_dir)).unwrap();
        let broken = text.replacen("\"sha256\": \"", "\"sha256\": \"zz", 1);
        package.write_manifest(&broken);
        let err = load_err(&package);
        assert!(err.contains("malformed sha256"), "{err}");
    }

    /// Discovery is executable-relative and absolute by construction: a
    /// relative root (which would resolve against the process CWD) is
    /// refused outright rather than searched.
    #[test]
    fn discovery_never_resolves_against_the_current_directory() {
        let err = load_from(Path::new("relative-dir"))
            .expect_err("a relative executable directory must be refused");
        assert!(err.contains("relative path"), "{err}");

        let package = Package::valid("windows-x86_64");
        assert!(manifest_path_for(&package.exe_dir).is_absolute());
        // A different (valid) package elsewhere is never picked up for
        // this executable directory.
        let other = Package::new();
        assert_eq!(load_from(&other.exe_dir).unwrap(), None);
    }

    #[test]
    fn a_file_inside_the_sidecar_root_must_be_declared_and_verified() {
        let package = Package::valid("windows-x86_64");
        let declared = root_for(&package.exe_dir).join("bin/libLLVM-22.dll");
        require_verified_if_inside_from(&package.exe_dir, &declared)
            .expect("a declared, matching file is verified");

        let undeclared = root_for(&package.exe_dir).join("bin/rogue.dll");
        fs::write(&undeclared, b"rogue").unwrap();
        let err = require_verified_if_inside_from(&package.exe_dir, &undeclared)
            .expect_err("an undeclared file inside the sidecar is never loaded");
        assert!(err.contains("does not declare it"), "{err}");

        // Outside the sidecar root: this module makes no claim either way.
        let outside = package.exe_dir.join("oscan.exe");
        fs::write(&outside, b"exe").unwrap();
        require_verified_if_inside_from(&package.exe_dir, &outside)
            .expect("paths outside the sidecar root are not this module's business");
    }

    #[test]
    fn a_corrupt_package_fails_the_provider_guard_too() {
        let package = Package::valid("windows-x86_64");
        let declared = root_for(&package.exe_dir).join("bin/libLLVM-22.dll");
        fs::write(&declared, b"swapped bytes!!").unwrap();
        let err = require_verified_if_inside_from(&package.exe_dir, &declared)
            .expect_err("a hash mismatch must reject the provider candidate");
        assert!(err.contains("SHA-256"), "{err}");
    }

    /// Every runtime DLL `ld.lld.exe` implicitly loads from its own
    /// directory must be declared: omitting one would surface as an
    /// opaque `STATUS_DLL_NOT_FOUND` at link time (or, worse, load a
    /// stray copy from elsewhere).
    #[test]
    fn an_omitted_windows_runtime_dll_is_a_hard_error() {
        for omitted in [
            "libLLVM-22.dll",
            "libc++.dll",
            "libunwind.dll",
            "libwinpthread-1.dll",
            "libffi-8.dll",
        ] {
            let package = Package::valid("windows-x86_64");
            let text = fs::read_to_string(manifest_path_for(&package.exe_dir)).unwrap();
            let stripped: String = text
                .lines()
                .filter(|line| !line.contains(&format!("\"name\": \"{omitted}\"")))
                .collect::<Vec<_>>()
                .join("\n")
                .replace(",\n  ]", "\n  ]");
            package.write_manifest(&stripped);
            let err = load_err(&package);
            assert!(
                err.contains(&format!("required 'linker_runtime' asset '{omitted}'")),
                "{omitted}: {err}"
            );
            assert!(err.contains("incomplete"), "{omitted}: {err}");
        }
    }

    /// The import-library requirement is taken from the link plan's own
    /// list, so a package that omits any of them is rejected up front.
    #[test]
    fn an_omitted_import_library_is_a_hard_error() {
        for omitted in crate::backend::link::required_import_libs() {
            let package = Package::valid("windows-x86_64");
            let text = fs::read_to_string(manifest_path_for(&package.exe_dir)).unwrap();
            let stripped: String = text
                .lines()
                .filter(|line| !line.contains(&format!("\"lib\": \"{omitted}\"")))
                .collect::<Vec<_>>()
                .join("\n")
                .replace(",\n  ]", "\n  ]");
            package.write_manifest(&stripped);
            let err = load_err(&package);
            assert!(
                err.contains(&format!("does not declare the '{omitted}' import library")),
                "{omitted}: {err}"
            );
        }
    }

    #[test]
    fn omitted_windows_compiler_builtins_are_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        let text = fs::read_to_string(manifest_path_for(&package.exe_dir)).unwrap();
        let stripped: String = text
            .lines()
            .filter(|line| !line.contains("compiler_builtins"))
            .collect::<Vec<_>>()
            .join("\n")
            .replace(",\n  ]", "\n  ]");
        package.write_manifest(&stripped);
        let err = load_err(&package);
        assert!(err.contains("no 'compiler_builtins' asset"), "{err}");
    }

    /// A corrupt *dependency* is caught by the closure check even when the
    /// primary asset (the linker, or the code generator the provider is
    /// about to load) is untouched.
    #[test]
    fn a_corrupt_dependent_dll_fails_the_runtime_closure_check() {
        let package = Package::valid("windows-x86_64");
        fs::write(
            root_for(&package.exe_dir).join("bin/libunwind.dll"),
            b"swapped!!!!!",
        )
        .unwrap();
        let loaded = load_from(&package.exe_dir).unwrap().unwrap();
        let err = loaded
            .verify_runtime_closure()
            .expect_err("a corrupt sibling DLL must be caught");
        assert!(err.contains("libunwind.dll"), "{err}");
        assert!(err.contains("SHA-256"), "{err}");

        // ...and the provider guard, asked only about libLLVM, still
        // refuses because a sibling it will implicitly load is corrupt.
        let llvm = root_for(&package.exe_dir).join("bin/libLLVM-22.dll");
        let err = require_verified_if_inside_from(&package.exe_dir, &llvm)
            .expect_err("the provider guard must verify the whole runtime closure");
        assert!(err.contains("libunwind.dll"), "{err}");
    }

    /// A Linux package needs only its linker: no import libraries, no
    /// compiler builtins (the ELF plan requests neither).
    #[test]
    fn a_linux_package_requires_only_its_linker() {
        let package = Package::valid("linux-x86_64");
        let loaded = load_from(&package.exe_dir)
            .expect("a linker-only Linux package is complete")
            .expect("manifest present");
        assert_eq!(loaded.entries.len(), 1);
        loaded.verify_all().expect("hashes match");
        loaded
            .verify_runtime_closure()
            .expect("no runtime closure to verify");
    }

    /// Windows resolves an EXE's imports from the directory containing the
    /// EXE, so a runtime DLL staged anywhere but beside the linker would be
    /// looked up somewhere else entirely — that is a package defect, not a
    /// layout preference.
    #[test]
    fn a_misplaced_windows_runtime_dll_is_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        let text = fs::read_to_string(manifest_path_for(&package.exe_dir)).unwrap();
        // Restage libLLVM one directory over, and point the manifest there.
        let bytes = b"libLLVM-22.dll";
        package.stage("lib/libLLVM-22.dll", bytes);
        package.write_manifest(&text.replace(
            "\"install_subpath\": \"bin/libLLVM-22.dll\"",
            "\"install_subpath\": \"lib/libLLVM-22.dll\"",
        ));
        let err = load_err(&package);
        assert!(err.contains("libLLVM-22.dll"), "{err}");
        assert!(
            err.contains("must be its sibling in one directory"),
            "{err}"
        );
    }

    /// The linker, every runtime DLL, and therefore the packaged code
    /// generator all share one canonical directory; import libraries and
    /// compiler builtins keep their own `lib/` layout.
    #[test]
    fn the_runtime_directory_is_the_linkers_own_directory() {
        let package = Package::valid("windows-x86_64");
        let loaded = load_from(&package.exe_dir).unwrap().unwrap();
        let runtime_dir = loaded.runtime_dir().expect("a linker defines the dir");
        assert_eq!(
            runtime_dir,
            fs::canonicalize(root_for(&package.exe_dir).join("bin")).unwrap()
        );
        // Import libraries and builtins live elsewhere and are still fine.
        let import = loaded
            .entries
            .iter()
            .find(|entry| entry.role == "import_lib")
            .expect("import lib declared");
        assert_ne!(canonical_parent(&import.path).unwrap(), runtime_dir);
    }

    /// The provider may only load a runtime library staged beside the
    /// linker: a declared file with any other role is refused even though
    /// the manifest vouches for its bytes.
    #[test]
    fn only_a_runtime_library_beside_the_linker_may_be_loaded_as_the_provider() {
        let package = Package::valid("windows-x86_64");
        let llvm = root_for(&package.exe_dir).join("bin/libLLVM-22.dll");
        require_verified_if_inside_from(&package.exe_dir, &llvm)
            .expect("the packaged code generator is a runtime sibling of the linker");

        let import_lib = root_for(&package.exe_dir).join("lib/libkernel32.a");
        let err = require_verified_if_inside_from(&package.exe_dir, &import_lib)
            .expect_err("an import library is not loadable as a shared library");
        assert!(err.contains("not 'linker_runtime'"), "{err}");

        let linker = root_for(&package.exe_dir).join("bin/ld.lld.exe");
        let err = require_verified_if_inside_from(&package.exe_dir, &linker)
            .expect_err("the linker is executed, never loaded as a library");
        assert!(err.contains("not 'linker_runtime'"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_escaping_the_sidecar_root_is_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        let outside = package.exe_dir.join("outside.bin");
        fs::write(&outside, b"llvm bytes").unwrap();
        let staged = root_for(&package.exe_dir).join("bin/libLLVM-22.dll");
        fs::remove_file(&staged).unwrap();
        std::os::unix::fs::symlink(&outside, &staged).unwrap();
        let err = load_err(&package);
        assert!(err.contains("symlink/reparse point"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_manifest_is_a_hard_error() {
        let package = Package::valid("windows-x86_64");
        let manifest = manifest_path_for(&package.exe_dir);
        let real = package.exe_dir.join("elsewhere.json");
        fs::rename(&manifest, &real).unwrap();
        std::os::unix::fs::symlink(&real, &manifest).unwrap();
        let err = load_err(&package);
        assert!(err.contains("symlink/reparse point"), "{err}");
    }

    /// A directory symlink/junction *inside* the root that redirects an
    /// asset out of the package is caught by canonical containment even
    /// though the asset file itself is a regular file.
    #[cfg(unix)]
    #[test]
    fn a_directory_symlink_escape_is_a_hard_error() {
        let package = Package::new();
        let outside_dir = package.exe_dir.join("outside");
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join("ld.lld"), b"linker bytes").unwrap();
        let sha = hex(&Sha256::digest(b"linker bytes"));
        std::os::unix::fs::symlink(&outside_dir, root_for(&package.exe_dir).join("redirect"))
            .unwrap();
        package.write_manifest(&format!(
            r#"{{
  "schema_version": 1,
  "target": "linux-x86_64",
  "linker": {{ "role": "linker", "name": "ld.lld", "install_subpath": "redirect/ld.lld", "sha256": "{sha}" }},
  "assets": []
}}"#
        ));
        let err = load_err(&package);
        assert!(err.contains("outside the sidecar root"), "{err}");
    }

    /// Windows junctions need no special privileges, so this runs for
    /// real on Windows CI/dev machines; if the OS refuses to create one
    /// anyway (locked-down policy, non-NTFS temp), the test skips rather
    /// than failing for an unrelated reason.
    #[cfg(windows)]
    fn try_create_junction(link: &Path, target: &Path) -> bool {
        std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|out| out.status.success() && fs::symlink_metadata(link).is_ok())
            .unwrap_or(false)
    }

    /// A junction *as* the sidecar root redirects the entire package.
    #[cfg(windows)]
    #[test]
    fn a_junctioned_sidecar_root_is_a_hard_error() {
        let package = Package::new();
        let elsewhere = package.exe_dir.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join(MANIFEST_FILE_NAME), "{}").unwrap();
        fs::remove_dir_all(root_for(&package.exe_dir)).unwrap();
        if !try_create_junction(&root_for(&package.exe_dir), &elsewhere) {
            return;
        }
        let err = load_err(&package);
        assert!(err.contains("symlink/reparse point"), "{err}");
    }

    /// A junction *inside* the sidecar root that redirects an asset out of
    /// the package is caught by canonical containment.
    #[cfg(windows)]
    #[test]
    fn a_junction_inside_the_sidecar_root_is_a_hard_error() {
        let package = Package::new();
        let outside_dir = package.exe_dir.join("outside");
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join("ld.lld.exe"), b"linker bytes").unwrap();
        let sha = hex(&Sha256::digest(b"linker bytes"));
        if !try_create_junction(&root_for(&package.exe_dir).join("redirect"), &outside_dir) {
            return;
        }
        package.write_manifest(&format!(
            r#"{{
  "schema_version": 1,
  "target": "linux-x86_64",
  "linker": {{ "role": "linker", "name": "ld.lld.exe", "install_subpath": "redirect/ld.lld.exe", "sha256": "{sha}" }},
  "assets": []
}}"#
        ));
        let err = load_err(&package);
        assert!(err.contains("outside the sidecar root"), "{err}");
    }

    /// The provider guard decides containment lexically *first*: a
    /// candidate that sits under the fixed sidecar path but canonicalizes
    /// somewhere else is refused, instead of being waved through as "not a
    /// sidecar candidate".
    #[cfg(any(unix, windows))]
    #[test]
    fn a_candidate_redirected_out_of_the_sidecar_root_is_refused() {
        let package = Package::valid("windows-x86_64");
        let outside_dir = package.exe_dir.join("outside");
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join("libLLVM-22.dll"), b"planted").unwrap();
        let redirect = root_for(&package.exe_dir).join("redirect");
        #[cfg(unix)]
        let created = { std::os::unix::fs::symlink(&outside_dir, &redirect).is_ok() };
        #[cfg(windows)]
        let created = try_create_junction(&redirect, &outside_dir);
        if !created {
            return;
        }
        let candidate = redirect.join("libLLVM-22.dll");
        let err = require_verified_if_inside_from(&package.exe_dir, &candidate)
            .expect_err("a redirected candidate must be refused, not ignored");
        assert!(err.contains("resolves outside it"), "{err}");
    }

    /// A candidate that *is* a symlink/junction is refused even when its
    /// target stays inside the package: the manifest describes files.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_candidate_inside_the_root_is_refused() {
        let package = Package::valid("windows-x86_64");
        let real = root_for(&package.exe_dir).join("bin/libLLVM-22.dll");
        let link = root_for(&package.exe_dir).join("bin/aliased.dll");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = require_verified_if_inside_from(&package.exe_dir, &link)
            .expect_err("a symlinked candidate must be refused");
        assert!(err.contains("symlink/reparse point"), "{err}");
    }
}
