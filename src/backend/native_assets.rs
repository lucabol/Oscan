//! Embedded native-link asset store: generated-const access, extraction
//! into a content-addressed, concurrency-safe cache, and the cache-dir
//! resolver. See `docs/design/native-link-embedding.md` §6 for the
//! contract this module implements, and §5.2/§8.3 for the generated-symbol
//! shape this module consumes.
//!
//! # Generated-const access
//!
//! `build.rs` (owned by Hicks; see design §5) writes
//! `${OUT_DIR}/native_link_assets_generated.rs`, included verbatim below.
//! It defines, in that generated file:
//!
//! - `pub const EMBEDDED_ASSETS_PRESENT: bool` — `true` iff a full,
//!   digest-verified Windows asset set was embedded at compiler build time.
//! - `struct EmbeddedAsset { role, name, lib, install_subpath, sha256, len,
//!   bytes }` and `pub static EMBEDDED_ASSETS: &[EmbeddedAsset]` — the flat
//!   list of all embedded files (the `linker` role, `import_lib` roles, and
//!   the `compiler_builtins` role all live in this one slice).
//! - `pub static EMBEDDED_ASSET_MANIFEST_JSON: &str` — the verbatim staged
//!   `native-link-assets.json`, used for the toolchain-version cross-check
//!   (design §4.3).
//!
//! When no assets were staged (an ordinary `cargo build` with
//! `OSCAN_EMBED_ASSETS_DIR` unset), `build.rs` generates the same shape
//! with `EMBEDDED_ASSETS_PRESENT = false`, an empty `EMBEDDED_ASSETS`
//! slice, and an empty manifest string — this module never has to special-
//! case "no build.rs support" vs. "build.rs ran but embedded nothing".
//!
//! # Cache hardening (security review findings, this pass)
//!
//! - **No length-only memoization.** [`verify_existing`] re-reads and
//!   re-hashes the file's current on-disk bytes on **every** call, with no
//!   in-process cache. An earlier version memoized `(len, sha256)` and
//!   trusted the cached hash whenever a later call saw a matching file
//!   length — a same-length content swap between two calls went
//!   undetected. There is no cache to poison anymore.
//! - **Symlinks/reparse points are never followed.** [`reject_symlink`]
//!   (checked with `fs::symlink_metadata`, never `fs::metadata`, so this
//!   also covers Windows junctions — `is_symlink()` handles both) is
//!   applied to the cache root, the set directory, every asset's
//!   destination file, and the extraction temp file — both at directory-
//!   creation time ([`create_dir_all_no_symlinks`], which never calls the
//!   symlink-following `fs::create_dir_all`) and again immediately before
//!   the final `fs::rename`. This narrows, but does not eliminate, the
//!   TOCTOU window below.
//! - **Cache root privacy.** Unix: the cache root and set directories get
//!   `fs::set_permissions` mode `0o700` immediately after creation (see
//!   [`harden_dir_permissions_unix`]). Windows: `%LOCALAPPDATA%\oscan\
//!   native-assets` is per-user-private by the OS's default ACL
//!   inheritance for a *non-elevated* process — this module explicitly
//!   relies on that default ACL and does not attempt to further restrict
//!   it (a disclosed, accepted boundary — see below).
//!
//! ## Windows elevation: fail-closed refusal, not sandboxing (security
//! ## review 2026-07-15, findings 2 & 3)
//!
//! An earlier version of this module tried to make an *elevated* Windows
//! process safe to run anyway, by routing it to a freshly-created,
//! best-effort-permissioned scratch directory instead of the shared
//! per-user cache. A follow-up security review judged that insufficient:
//! Windows handle-based TOCTOU races between a path check and a subsequent
//! open/rename are not fully closed by re-checking paths, however
//! carefully, so "sandbox the elevated process" was pretending to solve a
//! problem it could not fully solve.
//!
//! The product policy is now **fail-closed refusal by default**: `main.rs`'s
//! `run_native_backend` refuses to perform a native final link or `--run`
//! at all while this process is elevated (see [`NativeLinkOperation`] and
//! [`check_elevation_policy`]), *before* ever creating a scratch directory
//! or calling into this module's extraction path. Detection failure
//! (`is_elevated()` returning `Err`) is treated the same as "elevated" —
//! fail closed, not fail open. Trusted CI/release builds can explicitly pass
//! `--allow-elevated-native-link` to acknowledge the risk for trusted inputs;
//! that opt-in only bypasses the elevated-token refusal and does not relax
//! any path validation, cache verification, canonicalization, or sandboxing
//! check. `-o *.o`/`-o *.obj` (object-only output,
//! [`NativeLinkOperation::ObjectOnly`]) is always allowed regardless of
//! elevation: it never extracts or executes an embedded asset, and never
//! writes a final linked executable, so it never touches the native-asset
//! cache or a shared scratch location in the first place.
//!
//! This module's own [`ensure_extracted`] additionally re-checks elevation
//! itself as belt-and-suspenders defense-in-depth, in case a future code
//! path reaches it without going through the `main.rs` gate — but that
//! `main.rs` gate is the primary, documented enforcement point; do not rely
//! on this module's internal re-check as the sole protection.
//!
//! ## The non-elevated threat boundary (do not overclaim)
//!
//! For a **non-elevated** Windows process, the per-user `%LOCALAPPDATA%`
//! cache this module reads and writes is at **equivalent privilege** to
//! any other process already running as that **same user** — this is not,
//! and cannot be, a security boundary this cache enforces. Hash-on-every-
//! use (no memoization, see [`verify_existing`]) and the random/atomic,
//! content-addressed cache layout are defense-in-depth: they raise the
//! bar/cost of *accidental* corruption or a casual same-directory
//! collision. They do **not** claim to stop a determined same-user
//! attacker, who already has many other, easier avenues to cause harm
//! (modifying `PATH`, environment variables oscan itself trusts, the
//! source file being compiled, etc. — see "Residual TOCTOU boundary"
//! below). Treat same-user isolation as out of scope, not as something
//! this module is quietly relying on.
//!
//! ## Residual TOCTOU boundary (disclosed, not fully closed)
//!
//! Hash-then-execute (verify a file's sha256, then later spawn it) has an
//! inherent, small time-of-check-to-time-of-use window on *any* OS — this
//! module does the practical best to minimize it (hashing immediately
//! before use, re-checking for symlinks immediately before the final
//! rename, never caching a verification result across calls) but does not
//! claim to fully close it. The window is only exploitable by an attacker
//! who already has local code-execution as the **same user** as the oscan
//! process — at which point they have many other, easier avenues to cause
//! harm (e.g. modifying `PATH`, environment variables oscan itself trusts,
//! or the source file being compiled). This is the accepted, disclosed
//! threat boundary for this module, not something it claims to fully
//! close. Similarly, a *non-elevated* process's per-user cache relies on
//! the OS's own per-user ACL on `%LOCALAPPDATA%`/`$HOME` rather than this
//! module adding its own redundant ACL — that is also a same-user boundary,
//! disclosed rather than hidden.
include!(concat!(env!("OUT_DIR"), "/native_link_assets_generated.rs"));

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

#[cfg(windows)]
mod windows_elevation;

#[cfg(unix)]
mod unix_elevation;

/// Which kind of native-backend operation is being gated by the fail-
/// closed Windows elevation policy (security review 2026-07-15, findings 2
/// & 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeLinkOperation {
    /// `-o *.o`/`-o *.obj`: never extracts/executes embedded assets, never
    /// writes a final linked executable. Always allowed, regardless of
    /// elevation.
    ObjectOnly,
    /// Produces (and possibly runs, via `--run`) a final linked
    /// executable: requires scratch-directory creation and/or embedded-
    /// asset extraction/execution. Refused while elevated.
    FinalLink,
}

/// Fail-closed elevation policy (security review 2026-07-15): a detection
/// *error* (`elevation` is `Err`) is treated as "assume elevated" for
/// [`NativeLinkOperation::FinalLink`] — refuse rather than risk it.
/// [`NativeLinkOperation::ObjectOnly`] is always allowed regardless of
/// elevation (it never touches the native-asset cache or a shared scratch
/// location). Pure function — no real elevation needed to unit-test both
/// branches (see this module's tests).
pub fn check_elevation_policy(
    elevation: Result<bool, String>,
    operation: NativeLinkOperation,
    allow_elevated_native_link: bool,
) -> Result<(), String> {
    if operation == NativeLinkOperation::ObjectOnly {
        return Ok(());
    }
    match elevation {
        Ok(false) => Ok(()),
        Ok(true) if allow_elevated_native_link => Ok(()),
        Ok(true) => Err(
            "refusing to perform a native final link (or --run) while this process is running \
             elevated (Administrator). Windows handle-based TOCTOU races between a path check \
             and a subsequent open/rename cannot be fully closed by re-checking paths alone, so \
             oscan refuses this operation entirely while elevated rather than attempt to sandbox \
             it. Please re-run this command from a non-elevated terminal. Trusted CI/release \
             builds with trusted inputs may pass --allow-elevated-native-link to bypass only this \
             elevated-process refusal; path validation, cache verification, canonicalization, and \
             sandboxing checks still apply."
                .to_string(),
        ),
        Err(reason) => Err(format!(
            "refusing to perform a native final link (or --run): could not determine whether \
             this process is running elevated ({reason}). Per this build's fail-closed policy, \
             elevation-detection failure is treated the same as \"elevated\" and the operation is \
             refused rather than risking an unclosable TOCTOU race. Please re-run this command \
             from a non-elevated terminal (or investigate why elevation could not be determined)."
        )),
    }
}

/// Re-exported so `main.rs` can gate a [`NativeLinkOperation::FinalLink`]
/// entirely before ever creating a scratch directory or calling into this
/// module's extraction path (security review 2026-07-15, findings 2 & 3).
/// `Ok(true)`/`Ok(false)` only when the OS-level check genuinely succeeds;
/// `Err(reason)` when the underlying `OpenProcessToken`/`GetTokenInformation`
/// calls themselves fail — see [`check_elevation_policy`] for why that is
/// *not* treated as "not elevated" anymore (fail-closed, not fail-open).
#[cfg(windows)]
pub fn is_elevated() -> Result<bool, String> {
    windows_elevation::is_elevated()
}

/// Re-exported so `main.rs` can gate a [`NativeLinkOperation::FinalLink`]
/// on Unix when the process is setuid-elevated (euid != uid). See design
/// §10.7.
#[cfg(unix)]
pub fn is_setuid_elevated() -> Result<bool, String> {
    unix_elevation::is_setuid_elevated()
}

/// One extracted, verified asset's absolute path plus the identifying
/// metadata a [`crate::backend::link`] plan needs to place it correctly
/// (role/lib name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedAsset {
    pub role: &'static str,
    pub name: &'static str,
    pub lib: Option<&'static str>,
    pub path: PathBuf,
}

/// A fully extracted and verified embedded asset set, rooted at one
/// content-addressed cache directory (design §6.2).
#[derive(Debug, Clone)]
pub struct ExtractedAssetSet {
    pub dir: PathBuf,
    pub assets: Vec<ExtractedAsset>,
}

impl ExtractedAssetSet {
    pub fn find(&self, role: &str, lib: Option<&str>) -> Option<&ExtractedAsset> {
        self.assets.iter().find(|a| a.role == role && a.lib == lib)
    }

    pub fn linker(&self) -> Option<&ExtractedAsset> {
        self.find("linker", None)
    }

    pub fn compiler_builtins(&self) -> Option<&ExtractedAsset> {
        self.find("compiler_builtins", None)
    }

    pub fn import_lib(&self, lib_name: &str) -> Option<&ExtractedAsset> {
        self.assets
            .iter()
            .find(|a| a.role == "import_lib" && a.lib == Some(lib_name))
    }
}

/// Resolve the cache root directory (design §6.1): `OSCAN_NATIVE_ASSET_CACHE_DIR`
/// override first (tests/CI always use this — never the real per-user
/// cache dir), else `%LOCALAPPDATA%\oscan\native-assets` on Windows, else
/// `$XDG_CACHE_HOME/oscan/native-assets` or `$HOME/.cache/oscan/native-assets`
/// on Unix. Hand-rolled (no `dirs` dependency).
pub fn cache_root() -> Result<PathBuf, String> {
    if let Some(dir) = env_var_nonempty("OSCAN_NATIVE_ASSET_CACHE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if cfg!(windows) {
        let local_app_data = env_var_nonempty("LOCALAPPDATA").ok_or_else(|| {
            "LOCALAPPDATA is not set; cannot resolve the native-asset cache directory (set \
             OSCAN_NATIVE_ASSET_CACHE_DIR to override)"
                .to_string()
        })?;
        Ok(PathBuf::from(local_app_data)
            .join("oscan")
            .join("native-assets"))
    } else {
        let base = env_var_nonempty("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| env_var_nonempty("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .ok_or_else(|| {
                "neither XDG_CACHE_HOME nor HOME is set; cannot resolve the native-asset cache \
                 directory (set OSCAN_NATIVE_ASSET_CACHE_DIR to override)"
                    .to_string()
            })?;
        Ok(base.join("oscan").join("native-assets"))
    }
}

fn env_var_nonempty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Content-addressed set directory: `sha256` over the sorted list of
/// `(install_subpath, sha256)` pairs from the embedded manifest (design
/// §6.2). Set-level addressing dedupes identical sets and makes a new
/// embedded toolchain a new directory (never clobbers a live one).
fn asset_set_digest(assets: &[EmbeddedAsset]) -> String {
    let mut pairs: Vec<(&str, &str)> = assets
        .iter()
        .map(|a| (a.install_subpath, a.sha256))
        .collect();
    pairs.sort_unstable();
    let mut hasher = Sha256::new();
    for (subpath, sha) in pairs {
        hasher.update(subpath.as_bytes());
        hasher.update([0u8]);
        hasher.update(sha.as_bytes());
        hasher.update([b'\n']);
    }
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

fn sha256_hex_of_bytes(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}

fn sha256_hex_of_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| {
        format!(
            "error reading '{}' to verify its checksum: {e}",
            path.display()
        )
    })?;
    Ok(sha256_hex_of_bytes(&bytes))
}

/// Path-traversal validation (design §6.3 step 1): reject absolute paths,
/// drive letters/prefixes, and any `.`/`..` components. Only a strict,
/// entirely-`Normal`-component relative path is ever joined onto the set
/// directory.
fn validated_dest(set_dir: &Path, install_subpath: &str) -> Result<PathBuf, String> {
    if install_subpath.trim().is_empty() {
        return Err("embedded asset manifest has an empty install_subpath".to_string());
    }
    let raw = Path::new(install_subpath);
    for component in raw.components() {
        match component {
            std::path::Component::Normal(_) => {}
            other => {
                return Err(format!(
                    "embedded asset install_subpath '{install_subpath}' contains a disallowed path \
                     component ({other:?}); only a strict relative path with plain segments is \
                     permitted"
                ));
            }
        }
    }
    Ok(set_dir.join(raw))
}

/// Hard-errors if `path` **exists** and is a symlink or reparse point
/// (Finding 4b, security review) — checked with `fs::symlink_metadata`,
/// which, unlike `fs::metadata`, never follows the link, so this also
/// rejects Windows junctions (`std::fs::FileType::is_symlink` covers both).
/// A path that does not exist at all is not an error here (`Ok(())`) —
/// only an *existing* symlink/junction is rejected, since this cache never
/// follows or reuses one, anywhere in its directory tree or at any
/// destination file.
fn reject_symlink(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(format!(
            "refusing to use native-asset cache path '{}': it is a symlink/reparse point (junction on \
             Windows), which this cache never follows or reuses (this is a defense-in-depth boundary \
             against a same-user attacker pre-planting a redirect under the cache directory)",
            path.display()
        )),
        _ => Ok(()),
    }
}

/// `fs::create_dir_all`-equivalent that walks `path` component-by-
/// component, checking each level with [`reject_symlink`] *before*
/// creating it (Finding 4b) — never blindly calling the real
/// `fs::create_dir_all`, which would silently follow/reuse an existing
/// symlinked or junctioned directory at any level.
fn create_dir_all_no_symlinks(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(format!(
                        "refusing to create native-asset cache directory: '{}' is a symlink/reparse \
                         point (junction on Windows); this cache never follows or reuses one",
                        current.display()
                    ));
                }
                if !meta.is_dir() {
                    return Err(format!(
                        "refusing to create native-asset cache directory: '{}' exists and is not a \
                         directory",
                        current.display()
                    ));
                }
            }
            Err(_) => match fs::create_dir(&current) {
                Ok(()) => {}
                // A concurrent process may have just created the same
                // level first; that is fine as long as it is a real
                // directory (re-checked, not blindly trusted).
                Err(e) if current.is_dir() => {
                    if let Err(symlink_err) = reject_symlink(&current) {
                        return Err(symlink_err);
                    }
                    let _ = e;
                }
                Err(e) => {
                    return Err(format!("error creating '{}': {e}", current.display()));
                }
            },
        }
    }
    Ok(())
}

/// Unix-only: tighten a cache-related directory to `0700` immediately
/// after creation (Finding 4c) — individual asset files already get
/// `0755`/`0644` in [`extract_one`] (unchanged by this pass; still correct
/// for files meant to be executed/read), but the *directories* themselves
/// (the cache root and each set directory) additionally get locked down to
/// owner-only.
#[cfg(unix)]
fn harden_dir_permissions_unix(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| {
        format!(
            "error restricting native-asset cache directory '{}' to mode 0700: {e}",
            path.display()
        )
    })
}

/// Verify an already-extracted destination against `asset`'s expected size
/// and sha256. **Finding 4a (security review)**: this re-reads and
/// re-hashes the file's current on-disk bytes on *every single call*, no
/// exceptions, no cache — an earlier version memoized `(len, sha256)` in a
/// process-global cache and trusted the cached hash whenever a later call
/// saw a matching file length, so a same-length content swap between two
/// calls went undetected. There is no such cache anymore: every call is an
/// independent, full re-verification. Returns `Ok(true)` only when both
/// size and hash match.
fn verify_existing(dest: &Path, asset: &EmbeddedAsset) -> Result<bool, String> {
    let meta = match fs::metadata(dest) {
        Ok(m) => m,
        Err(_) => return Ok(false),
    };
    if meta.len() != asset.len as u64 {
        return Ok(false);
    }
    let actual = sha256_hex_of_file(dest)?;
    Ok(actual == asset.sha256)
}

fn random_suffix() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    nanos ^ count.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// Extraction algorithm for one asset (design §6.3): reuse a verified
/// destination as-is; otherwise write to a sibling temp file (same
/// directory -> same filesystem -> atomic rename), verify size+sha256 of
/// the temp file *before* renaming, set the Unix exec bit for the `linker`
/// role, then atomically rename onto the destination. Handles the Windows
/// rename-onto-existing race by re-verifying the (possibly
/// concurrently-written) existing destination before giving up.
///
/// Finding 4b (security review): every path this function touches —
/// `dest`, and the temp file it writes through before the atomic rename —
/// is checked with [`reject_symlink`], both up front and again immediately
/// before the final `fs::rename`, so a pre-planted symlink/junction is
/// never followed or reused (this narrows, but does not eliminate, the
/// TOCTOU window documented in this module's top-level docs).
fn extract_one(asset: &EmbeddedAsset, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        create_dir_all_no_symlinks(parent)?;
    }

    reject_symlink(dest)?;
    if dest.is_file() && verify_existing(dest, asset)? {
        return Ok(());
    }

    let file_name = dest
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("asset")
        .to_string();
    let dir = dest.parent().map(Path::to_path_buf).unwrap_or_default();
    let tmp_path = dir.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        random_suffix()
    ));

    // An entry already existing at this exact random+PID temp path is
    // itself suspicious (it is never created by any other legitimate
    // caller); refuse to write through a symlink there rather than
    // following it.
    reject_symlink(&tmp_path)?;
    fs::write(&tmp_path, asset.bytes)
        .map_err(|e| format!("error writing temp file '{}': {e}", tmp_path.display()))?;

    let meta = fs::metadata(&tmp_path).map_err(|e| {
        format!(
            "error reading temp file metadata '{}': {e}",
            tmp_path.display()
        )
    })?;
    if meta.len() != asset.len as u64 {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!(
            "internal error: embedded asset '{}' wrote {} bytes, expected {}",
            asset.name,
            meta.len(),
            asset.len
        ));
    }
    let actual_sha = sha256_hex_of_file(&tmp_path)?;
    if actual_sha != asset.sha256 {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!(
            "internal error: embedded asset '{}' sha256 mismatch after writing temp file (expected \
             {}, got {actual_sha})",
            asset.name, asset.sha256
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if asset.role == "linker" { 0o755 } else { 0o644 };
        let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(mode));
    }

    // Re-check immediately before the final rename (Finding 4b): narrows,
    // but does not eliminate, the residual TOCTOU window between these
    // checks and the rename itself.
    reject_symlink(&tmp_path)?;
    reject_symlink(dest)?;

    match fs::rename(&tmp_path, dest) {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            // A concurrent process may have just written (and verified) the
            // same destination first. Re-verify rather than assume failure.
            if dest.exists() && verify_existing(dest, asset).unwrap_or(false) {
                let _ = fs::remove_file(&tmp_path);
                return Ok(());
            }
            // Existing destination doesn't verify (or the rename failed for
            // another reason): remove it and retry exactly once. Refuse to
            // blindly remove a symlink target left in its place.
            reject_symlink(dest)?;
            let _ = fs::remove_file(dest);
            reject_symlink(&tmp_path)?;
            match fs::rename(&tmp_path, dest) {
                Ok(()) => Ok(()),
                Err(e) => {
                    let _ = fs::remove_file(&tmp_path);
                    Err(format!(
                        "error installing extracted asset '{}' to '{}': {e} (original rename error: \
                         {rename_err})",
                        asset.name,
                        dest.display()
                    ))
                }
            }
        }
    }
}

/// Best-effort cleanup of stale `.tmp` files left behind by a crashed
/// extraction (design §6.5). Never satisfies a reuse check on its own (no
/// `.complete` marker refers to them); failures here are ignored, since
/// this is opportunistic housekeeping, not correctness-critical.
fn cleanup_stale_temp_files(set_dir: &Path, older_than: Duration) {
    let Ok(entries) = fs::read_dir(set_dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if !name.ends_with(".tmp") || !name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if now.duration_since(modified).unwrap_or(Duration::ZERO) >= older_than {
            let _ = fs::remove_file(&path);
        }
    }
}

fn complete_marker_path(set_dir: &Path) -> PathBuf {
    set_dir.join(".complete")
}

fn write_complete_marker(set_dir: &Path) -> Result<(), String> {
    let marker = complete_marker_path(set_dir);
    reject_symlink(&marker)?;
    let tmp = set_dir.join(format!(
        ".complete.{}.{}.tmp",
        std::process::id(),
        random_suffix()
    ));
    reject_symlink(&tmp)?;
    fs::write(&tmp, b"ok").map_err(|e| format!("error writing '{}': {e}", tmp.display()))?;
    reject_symlink(&tmp)?;
    reject_symlink(&marker)?;
    match fs::rename(&tmp, &marker) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            // A concurrent process may have already finished; that is fine.
            if marker.is_file() {
                Ok(())
            } else {
                Err(format!(
                    "error finalizing native-asset cache marker '{}': {e}",
                    marker.display()
                ))
            }
        }
    }
}

/// Security review 2026-07-15 (findings 2 & 3): the elevated-process
/// per-process scratch-directory routing this function used to implement
/// (extract into a fresh, best-effort-permissioned directory instead of
/// the shared per-user cache) was judged insufficient — Windows
/// handle-based TOCTOU races are not fully closed by re-checking paths,
/// however carefully. The product policy is now fail-closed *refusal*
/// (see this module's top-level docs, "Windows elevation" section):
/// `main.rs`'s `run_native_backend` refuses any
/// [`NativeLinkOperation::FinalLink`] while elevated *before* ever calling
/// [`ensure_extracted`], so this function should never observe an elevated
/// process in the first place. [`ensure_extracted`] still re-checks
/// elevation itself immediately below, as cheap belt-and-suspenders
/// defense-in-depth for a future caller that might skip the `main.rs`
/// gate — but that gate, not this re-check, is the primary, documented
/// enforcement point. There is deliberately no more per-process
/// alternate-directory machinery here to preserve: a refused operation
/// needs nowhere to extract to.
///
/// Extract (or reuse a verified, previously-extracted) full embedded asset
/// set, returning absolute paths for each asset. A hard error here (never a
/// silent fallback) is the caller's (`backend::link`) cue to apply the
/// no-silent-fallback rule (design §7.3) rather than quietly trying a
/// compiler driver.
///
/// Hash verification alone is not sufficient: a linker binary can have
/// every byte on disk correct and still fail to *launch* because a sibling
/// runtime dependency it dynamically links against (e.g. a DLL) is missing
/// from the asset set. So after extraction/reuse, and before the caller is
/// allowed to treat the linker as ready for use, this runs a cheap live
/// smoke-check (`--version`) against the extracted linker binary and turns
/// any launch failure into a hard error with a diagnostic distinct from a
/// hash-mismatch message (see [`smoke_check_linker`]).
pub fn ensure_extracted(allow_elevated_native_link: bool) -> Result<ExtractedAssetSet, String> {
    if !EMBEDDED_ASSETS_PRESENT || EMBEDDED_ASSETS.is_empty() {
        return Err("this oscan build has no embedded native-link assets".to_string());
    }
    #[cfg(windows)]
    check_elevation_policy(
        is_elevated(),
        NativeLinkOperation::FinalLink,
        allow_elevated_native_link,
    )?;
    #[cfg(unix)]
    check_elevation_policy(is_setuid_elevated(), NativeLinkOperation::FinalLink, false)?;

    let cache_root = cache_root()?;
    let set = ensure_extracted_in(EMBEDDED_ASSETS, &cache_root)?;
    if let Some(linker) = set.linker() {
        smoke_check_linker(&linker.path)?;
    }
    Ok(set)
}

/// Live launch smoke-check for the extracted `linker` asset (design §6.4
/// hardening): invokes it with `--version` and requires a clean, successful
/// exit. Files that are present and hash-correct but unable to launch
/// (classically: `STATUS_DLL_NOT_FOUND` on Windows, when a sibling runtime
/// DLL the linker dynamically links against was not co-staged alongside
/// it) are a distinct failure class from a hash mismatch, and this
/// produces a diagnostic that says so explicitly rather than surfacing as
/// a confusing crash from a much later linker invocation.
fn smoke_check_linker(linker_path: &Path) -> Result<(), String> {
    let result = std::process::Command::new(linker_path)
        .arg("--version")
        .output();
    smoke_check_result(&linker_path.display().to_string(), result)
}

fn smoke_check_result(
    exe_display: &str,
    spawn_result: std::io::Result<std::process::Output>,
) -> Result<(), String> {
    match spawn_result {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "native linker smoke-check failed: '{exe_display}' was extracted and its hash \
             verified correctly, but it exited with {status} when invoked as `--version`. This is \
             NOT a hash/content problem -- it almost always means a sibling runtime dependency \
             (e.g. a DLL this binary dynamically links against) is missing from the extracted \
             asset set. stderr: {stderr}",
            status = describe_exit_status(&output.status),
            stderr = String::from_utf8_lossy(&output.stderr).trim(),
        )),
        Err(e) => Err(format!(
            "native linker smoke-check failed: could not even launch extracted '{exe_display}' \
             ({e}). It was extracted and its hash verified correctly, but is unusable as-is -- \
             check that every required sibling runtime file is present alongside it in the same \
             directory."
        )),
    }
}

fn describe_exit_status(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => {
            let unsigned = code as u32;
            if unsigned == 0xC000_0135 {
                format!("exit code 0x{unsigned:08X} (STATUS_DLL_NOT_FOUND)")
            } else {
                format!("exit code {code} (0x{unsigned:08X})")
            }
        }
        None => "no exit code (terminated by signal)".to_string(),
    }
}

/// Testable core of [`ensure_extracted`], parameterized over both the
/// asset slice and the cache root directory so tests never depend on this
/// build's actual embedded assets, on `OSCAN_NATIVE_ASSET_CACHE_DIR` (a
/// single global env var that would otherwise race across parallel
/// `#[test]` threads), or on the real `%LOCALAPPDATA%` (design's testing
/// guidance: use a temp dir for these).
fn ensure_extracted_in(
    assets: &'static [EmbeddedAsset],
    root: &Path,
) -> Result<ExtractedAssetSet, String> {
    let set_dir = root.join(asset_set_digest(assets));
    // Finding 4b: never `fs::create_dir_all` blindly (it would silently
    // follow/reuse a symlinked/junctioned directory at any level); Finding
    // 4c: lock both the cache root and this set directory down to
    // owner-only (0700) on Unix immediately after creation.
    create_dir_all_no_symlinks(&set_dir)?;
    #[cfg(unix)]
    {
        harden_dir_permissions_unix(root)?;
        harden_dir_permissions_unix(&set_dir)?;
    }

    cleanup_stale_temp_files(&set_dir, Duration::from_secs(3600));

    let mut extracted = Vec::with_capacity(assets.len());
    for asset in assets {
        let dest = validated_dest(&set_dir, asset.install_subpath)?;
        extract_one(asset, &dest)?;
        extracted.push(ExtractedAsset {
            role: asset.role,
            name: asset.name,
            lib: asset.lib,
            path: dest,
        });
    }
    write_complete_marker(&set_dir)?;

    Ok(ExtractedAssetSet {
        dir: set_dir,
        assets: extracted,
    })
}

/// Parses `EMBEDDED_ASSET_MANIFEST_JSON`'s `toolchain.version` (design
/// §4.3's cross-check against the runtime archive manifest's own
/// `toolchain.version`). `None` when no assets are embedded, or the
/// manifest is malformed/missing the field.
pub fn embedded_toolchain_version() -> Option<String> {
    toolchain_version_from_manifest(EMBEDDED_ASSET_MANIFEST_JSON)
}

/// Parses `EMBEDDED_ASSET_MANIFEST_JSON`'s `"target"` field to return
/// the target architecture tag (e.g., `"linux-x86_64"`, `"linux-aarch64"`,
/// `"linux-riscv64"`, `"windows-x86_64"`) that the embedded assets are
/// intended for. Used for the target-matching gate in `link_executable`
/// (§11.4) to prevent a linux-x86_64-embedding oscan binary from incorrectly
/// attempting to cross-link to aarch64 using the wrong-arch linker. `None`
/// when no assets are embedded, or the manifest is malformed/missing the field.
pub fn embedded_target() -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(EMBEDDED_ASSET_MANIFEST_JSON).ok()?;
    value.get("target")?.as_str().map(str::to_owned)
}

fn toolchain_version_from_manifest(manifest_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(manifest_json).ok()?;
    value
        .get("toolchain")?
        .get("version")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oscan-native-assets-test-{tag}-{}-{}",
            std::process::id(),
            random_suffix()
        ));
        fs::create_dir_all(&dir).expect("create scratch dir");
        // macOS exposes /var as a system-managed symlink to /private/var.
        // Production cache roots still reject every symlink component, but
        // these tests own this freshly-created private directory and can
        // canonicalize the trusted OS temp alias before exercising that
        // policy beneath it.
        if cfg!(target_os = "macos") {
            fs::canonicalize(&dir).expect("canonicalize macOS scratch dir")
        } else {
            dir
        }
    }

    fn make_asset(
        role: &'static str,
        name: &'static str,
        lib: Option<&'static str>,
        install_subpath: &'static str,
        bytes: &'static [u8],
    ) -> EmbeddedAsset {
        EmbeddedAsset {
            role,
            name,
            lib,
            install_subpath,
            sha256: Box::leak(sha256_hex_of_bytes(bytes).into_boxed_str()),
            len: bytes.len(),
            bytes,
        }
    }

    #[test]
    fn validated_dest_rejects_absolute_and_traversal_paths() {
        let set_dir = PathBuf::from(r"C:\cache\abc123");

        // Portable bad paths (forward-slash): must be rejected on all hosts
        for bad in [
            "/evil/payload",
            "../../evil.exe",
            "lib/../../evil.exe",
            "./lib/evil.exe",
        ] {
            let err = validated_dest(&set_dir, bad);
            assert!(
                err.is_err(),
                "expected install_subpath {bad:?} to be rejected, got {err:?}"
            );
        }

        // Windows-only bad paths (backslash separator): only meaningful on Windows
        #[cfg(windows)]
        for bad in [
            r"C:\evil\payload.exe",
            r"..\..\evil.exe",
            r"lib\..\..\evil.exe",
            r".\lib\evil.exe",
        ] {
            let err = validated_dest(&set_dir, bad);
            assert!(
                err.is_err(),
                "expected install_subpath {bad:?} to be rejected, got {err:?}"
            );
        }
    }

    #[test]
    fn validated_dest_accepts_plain_relative_subpaths() {
        let set_dir = PathBuf::from(r"C:\cache\abc123");
        let dest = validated_dest(&set_dir, "lib/libkernel32.a").expect("plain subpath is valid");
        assert_eq!(dest, set_dir.join("lib").join("libkernel32.a"));
    }

    #[test]
    fn validated_dest_rejects_empty_subpath() {
        let set_dir = PathBuf::from(r"C:\cache\abc123");
        assert!(validated_dest(&set_dir, "").is_err());
        assert!(validated_dest(&set_dir, "   ").is_err());
    }

    #[test]
    fn extraction_writes_verifiable_files_and_a_complete_marker() {
        let dir = scratch_dir("happy-path");

        let linker_bytes: &'static [u8] = b"fake-ld-lld-bytes";
        let lib_bytes: &'static [u8] = b"fake-import-lib-bytes";
        let assets: &'static [EmbeddedAsset] = Box::leak(Box::new([
            make_asset("linker", "ld.lld.exe", None, "bin/ld.lld.exe", linker_bytes),
            make_asset(
                "import_lib",
                "libkernel32.a",
                Some("kernel32"),
                "lib/libkernel32.a",
                lib_bytes,
            ),
        ]));

        let set = ensure_extracted_in(assets, &dir).expect("extraction must succeed");
        assert!(set.dir.join(".complete").is_file());
        assert!(set.linker().is_some());
        assert!(set.import_lib("kernel32").is_some());
        let linker_path = &set.linker().unwrap().path;
        assert_eq!(fs::read(linker_path).unwrap(), linker_bytes);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(linker_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "linker role must be executable on Unix");
        }

        // Re-running must reuse the verified destination rather than erroring.
        let set2 = ensure_extracted_in(assets, &dir).expect("re-extraction reuses cache");
        assert_eq!(set2.linker().unwrap().path, set.linker().unwrap().path);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extraction_detects_and_repairs_a_corrupted_cached_file() {
        let dir = scratch_dir("corruption");

        let bytes: &'static [u8] = b"correct-bytes-for-this-asset";
        let assets: &'static [EmbeddedAsset] = Box::leak(Box::new([make_asset(
            "linker",
            "ld.lld.exe",
            None,
            "bin/ld.lld.exe",
            bytes,
        )]));

        let set = ensure_extracted_in(assets, &dir).expect("first extraction succeeds");
        let path = set.linker().unwrap().path.clone();

        // Corrupt the cached file after `.complete` was written.
        fs::write(&path, b"CORRUPTED-CONTENT-DIFFERENT-LENGTH-AND-HASH").unwrap();

        // A fresh process (no in-process memoization) must detect the
        // mismatch and repair it rather than trusting the stale bytes.
        let set2 = ensure_extracted_in(assets, &dir).expect("must repair, not error");
        assert_eq!(fs::read(&set2.linker().unwrap().path).unwrap(), bytes);

        let _ = fs::remove_dir_all(&dir);
    }

    /// Finding 4a (security review), distinct from the test above: that one
    /// corrupts the file to a *different* length, which even the old,
    /// removed length-only memoization would have caught on its own
    /// (`verify_existing`'s length check ran *before* consulting the
    /// cache). The actual vulnerability the memoization introduced was a
    /// **same-length** content swap within the *same process*, where the
    /// old code trusted a cached hash without ever re-reading the file.
    /// This proves `verify_existing` now re-hashes on every single call,
    /// with no cache to mask a same-length tamper.
    #[test]
    fn verify_existing_rehashes_every_call_even_for_a_same_length_content_swap_same_process() {
        let dir = scratch_dir("same-length-swap");

        let original: &'static [u8] = b"0000000000000000";
        let tampered: &'static [u8] = b"1111111111111111";
        assert_eq!(
            original.len(),
            tampered.len(),
            "test fixture must keep the byte length identical to exercise the same-length case"
        );

        let assets: &'static [EmbeddedAsset] = Box::leak(Box::new([make_asset(
            "linker",
            "ld.lld.exe",
            None,
            "bin/ld.lld.exe",
            original,
        )]));

        let set = ensure_extracted_in(assets, &dir).expect("first extraction succeeds");
        let path = set.linker().unwrap().path.clone();
        assert_eq!(fs::read(&path).unwrap(), original);

        // Same-length content swap, still within this very same process.
        fs::write(&path, tampered).unwrap();
        assert!(
            !verify_existing(&path, &assets[0]).unwrap(),
            "a same-length content swap must be detected every call -- no cache may mask it"
        );

        // And the higher-level extraction entry point must repair it too.
        let set2 = ensure_extracted_in(assets, &dir).expect("must repair, not error");
        assert_eq!(fs::read(&set2.linker().unwrap().path).unwrap(), original);

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn extraction_rejects_a_preplanted_symlinked_set_directory() {
        use std::os::unix::fs::symlink;

        let dir = scratch_dir("symlink-reject");
        let evil_target = scratch_dir("symlink-evil-target");

        let bytes: &'static [u8] = b"payload-bytes";
        let assets: &'static [EmbeddedAsset] = Box::leak(Box::new([make_asset(
            "linker",
            "ld.lld.exe",
            None,
            "bin/ld.lld.exe",
            bytes,
        )]));

        // Pre-plant a symlink exactly where the content-addressed set
        // directory would normally be created, pointing somewhere else
        // entirely.
        let set_dir_path = dir.join(asset_set_digest(assets));
        symlink(&evil_target, &set_dir_path).expect("create pre-planted symlink");

        let err = ensure_extracted_in(assets, &dir)
            .expect_err("a symlinked set directory must be rejected, never followed");
        assert!(
            err.to_lowercase().contains("symlink"),
            "error should name the symlink rejection: {err}"
        );
        assert!(
            !evil_target.join("bin").exists(),
            "nothing must ever be written through the symlink"
        );

        let _ = fs::remove_dir_all(&evil_target);
        let _ = fs::remove_file(&set_dir_path);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn extraction_rejects_a_preplanted_symlinked_set_directory() {
        let dir = scratch_dir("symlink-reject");
        let evil_target = scratch_dir("symlink-evil-target");

        let bytes: &'static [u8] = b"payload-bytes";
        let assets: &'static [EmbeddedAsset] = Box::leak(Box::new([make_asset(
            "linker",
            "ld.lld.exe",
            None,
            "bin/ld.lld.exe",
            bytes,
        )]));

        let set_dir_path = dir.join(asset_set_digest(assets));
        // Creating a directory symlink on Windows normally requires either
        // Administrator elevation or Developer Mode; skip cleanly rather
        // than failing the whole suite on a host that has neither.
        if std::os::windows::fs::symlink_dir(&evil_target, &set_dir_path).is_err() {
            eprintln!(
                "skipping extraction_rejects_a_preplanted_symlinked_set_directory: creating a \
                 directory symlink requires elevation/Developer Mode on this host"
            );
            let _ = fs::remove_dir_all(&dir);
            let _ = fs::remove_dir_all(&evil_target);
            return;
        }

        let err = ensure_extracted_in(assets, &dir)
            .expect_err("a symlinked set directory must be rejected, never followed");
        assert!(
            err.to_lowercase().contains("symlink") || err.to_lowercase().contains("reparse"),
            "error should name the symlink/reparse-point rejection: {err}"
        );
        assert!(
            !evil_target.join("bin").exists(),
            "nothing must ever be written through the symlink"
        );

        let _ = fs::remove_dir_all(&evil_target);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn set_directory_and_cache_root_get_0700_permissions_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch_dir("dir-permissions");
        let bytes: &'static [u8] = b"payload-bytes";
        let assets: &'static [EmbeddedAsset] = Box::leak(Box::new([make_asset(
            "linker",
            "ld.lld.exe",
            None,
            "bin/ld.lld.exe",
            bytes,
        )]));

        let set = ensure_extracted_in(assets, &dir).expect("extraction succeeds");

        let root_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        let set_dir_mode = fs::metadata(&set.dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(root_mode, 0o700, "cache root must be 0700");
        assert_eq!(set_dir_mode, 0o700, "set directory must be 0700");

        let _ = fs::remove_dir_all(&dir);
    }

    /// Security review 2026-07-15 (findings 2 & 3): fail-closed elevation
    /// policy, pure-function tests — no real elevation needed. Replaces
    /// the removed `resolve_extraction_root_for` elevated-routing tests
    /// (that machinery no longer exists; the product policy is now
    /// "refuse `FinalLink` entirely while elevated", enforced up in
    /// `main.rs`, not "sandbox it here").
    #[test]
    fn check_elevation_policy_allows_object_only_regardless_of_elevation() {
        assert!(check_elevation_policy(Ok(true), NativeLinkOperation::ObjectOnly, false).is_ok());
        assert!(check_elevation_policy(Ok(false), NativeLinkOperation::ObjectOnly, false).is_ok());
        assert!(check_elevation_policy(
            Err("token check failed".to_string()),
            NativeLinkOperation::ObjectOnly,
            false
        )
        .is_ok());
    }

    #[test]
    fn check_elevation_policy_allows_final_link_when_confirmed_not_elevated() {
        assert!(check_elevation_policy(Ok(false), NativeLinkOperation::FinalLink, false).is_ok());
    }

    #[test]
    fn check_elevation_policy_refuses_final_link_when_elevated() {
        let err = check_elevation_policy(Ok(true), NativeLinkOperation::FinalLink, false)
            .expect_err("must refuse a final link while elevated");
        assert!(err.to_lowercase().contains("elevated"));
        assert!(err.contains("--allow-elevated-native-link"));
    }

    #[test]
    fn check_elevation_policy_allows_elevated_final_link_with_explicit_trusted_opt_in() {
        assert!(check_elevation_policy(Ok(true), NativeLinkOperation::FinalLink, true).is_ok());
    }

    #[test]
    fn check_elevation_policy_fails_closed_on_detection_error() {
        // A detection error must be treated the same as "elevated" for
        // FinalLink -- fail closed, not fail open.
        let err = check_elevation_policy(
            Err("OpenProcessToken failed".to_string()),
            NativeLinkOperation::FinalLink,
            true,
        )
        .expect_err("a detection error must refuse FinalLink, not silently allow it");
        assert!(err.contains("OpenProcessToken failed"));
    }

    #[test]
    fn extraction_rejects_a_path_traversal_install_subpath() {
        let dir = scratch_dir("traversal");

        let bytes: &'static [u8] = b"payload";
        let assets: &'static [EmbeddedAsset] = Box::leak(Box::new([make_asset(
            "linker",
            "evil.exe",
            None,
            "../../evil.exe",
            bytes,
        )]));

        let err = ensure_extracted_in(assets, &dir)
            .expect_err("path-traversal install_subpath must be rejected");
        assert!(err.contains("disallowed path component"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_extraction_of_the_same_asset_set_converges() {
        use std::sync::Arc;
        use std::thread;

        let dir = scratch_dir("concurrent");

        let bytes: &'static [u8] = b"concurrently-extracted-bytes";
        let assets: &'static [EmbeddedAsset] = Box::leak(Box::new([
            make_asset("linker", "ld.lld.exe", None, "bin/ld.lld.exe", bytes),
            make_asset(
                "import_lib",
                "libkernel32.a",
                Some("kernel32"),
                "lib/libkernel32.a",
                bytes,
            ),
        ]));

        let barrier = Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let dir = dir.clone();
                thread::spawn(move || {
                    barrier.wait();
                    ensure_extracted_in(assets, &dir)
                })
            })
            .collect();

        for handle in handles {
            let result = handle.join().expect("thread must not panic");
            let set = result.expect("every racing extraction must succeed, never corrupt");
            assert_eq!(fs::read(&set.linker().unwrap().path).unwrap(), bytes);
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn asset_set_digest_is_order_independent_and_changes_with_content() {
        let a: &'static [EmbeddedAsset] = Box::leak(Box::new([
            make_asset("linker", "a", None, "bin/a", b"x"),
            make_asset("import_lib", "b", Some("b"), "lib/b", b"y"),
        ]));
        let a_reordered: &'static [EmbeddedAsset] = Box::leak(Box::new([
            make_asset("import_lib", "b", Some("b"), "lib/b", b"y"),
            make_asset("linker", "a", None, "bin/a", b"x"),
        ]));
        assert_eq!(asset_set_digest(a), asset_set_digest(a_reordered));

        let different: &'static [EmbeddedAsset] = Box::leak(Box::new([make_asset(
            "linker",
            "a",
            None,
            "bin/a",
            b"different",
        )]));
        assert_ne!(asset_set_digest(a), asset_set_digest(different));
    }

    #[test]
    fn cache_root_honors_the_override_env_var() {
        // The only test in this crate that touches OSCAN_NATIVE_ASSET_CACHE_DIR,
        // so there is no cross-test race from Rust's parallel test runner.
        let dir = scratch_dir("cache-root-override");
        std::env::set_var("OSCAN_NATIVE_ASSET_CACHE_DIR", &dir);
        assert_eq!(cache_root().unwrap(), dir);
        std::env::remove_var("OSCAN_NATIVE_ASSET_CACHE_DIR");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn toolchain_version_from_manifest_parses_the_expected_field() {
        let json = r#"{"schema_version":1,"target":"windows-x86_64","toolchain":{"vendor":"llvm-mingw","version":"20260324"}}"#;
        assert_eq!(
            toolchain_version_from_manifest(json).as_deref(),
            Some("20260324")
        );
        assert_eq!(toolchain_version_from_manifest("not json"), None);
        assert_eq!(toolchain_version_from_manifest("{}"), None);
    }

    /// Build a synthetic `ExitStatus` for a given raw OS exit code, without
    /// spawning a process. Used to simulate the exact real-world failure
    /// signature of a binary whose sibling DLL dependency is missing --
    /// Windows does not fail to *spawn* such a process, it spawns it and
    /// the loader immediately terminates it with `STATUS_DLL_NOT_FOUND`
    /// (0xC0000135) as the exit code. This lets the smoke-check's
    /// classification logic be tested precisely against a "deliberately
    /// incomplete synthetic asset set" failure mode without needing a real
    /// multi-hundred-megabyte `ld.lld.exe` + `libLLVM*.dll` fixture in the
    /// test tree.
    fn synthetic_exit_status(raw: u32) -> std::process::ExitStatus {
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(raw)
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(raw as i32)
        }
    }

    fn synthetic_output(raw_exit_code: u32, stderr: &str) -> std::process::Output {
        std::process::Output {
            status: synthetic_exit_status(raw_exit_code),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn smoke_check_result_passes_on_clean_success() {
        let result = smoke_check_result("ld.lld.exe", Ok(synthetic_output(0, "")));
        assert!(result.is_ok(), "expected success, got {result:?}");
    }

    #[test]
    #[cfg(windows)]
    fn smoke_check_result_reports_missing_sibling_dll_distinctly_from_a_hash_mismatch() {
        // STATUS_DLL_NOT_FOUND: exactly the failure this hardening exists
        // to catch -- files present and hash-correct, but the asset set is
        // missing a sibling runtime DLL the linker dynamically links
        // against, so the loader kills it on launch.
        let output = synthetic_output(0xC000_0135, "");
        let err = smoke_check_result(r"C:\cache\abc\bin\ld.lld.exe", Ok(output))
            .expect_err("a STATUS_DLL_NOT_FOUND exit must be a hard smoke-check failure");
        assert!(
            err.contains("STATUS_DLL_NOT_FOUND"),
            "error should name the specific failure: {err}"
        );
        assert!(
            err.contains("sibling runtime dependency"),
            "error should point at the actual root cause class: {err}"
        );
        assert!(
            !err.to_lowercase().contains("sha256") && !err.to_lowercase().contains("hash mismatch"),
            "error must read as a launch failure, not be confusable with a hash-verification \
             failure: {err}"
        );
    }

    #[test]
    fn smoke_check_result_reports_other_nonzero_exits_too() {
        let output = synthetic_output(1, "some other startup error");
        let err = smoke_check_result("ld.lld.exe", Ok(output))
            .expect_err("any non-success exit must be a hard smoke-check failure");
        assert!(err.contains("some other startup error"));
    }

    #[test]
    fn smoke_check_result_reports_a_failure_to_even_launch() {
        let spawn_err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let err = smoke_check_result("ld.lld.exe", Err(spawn_err))
            .expect_err("a spawn failure must be a hard smoke-check failure");
        assert!(err.contains("could not even launch"));
    }

    #[test]
    fn smoke_check_linker_end_to_end_against_a_real_process() {
        // Real (non-synthetic) end-to-end exercise of the whole
        // spawn-and-classify path (not just `smoke_check_result` in
        // isolation): a real child process that cleanly exits 0 is a pass,
        // and a real child process that exits non-zero is a hard failure
        // with the expected diagnostic.
        #[cfg(windows)]
        let shell_cmd = |code: i32| {
            let mut cmd = std::process::Command::new("cmd.exe");
            cmd.args(["/C", &format!("exit {code}")]);
            cmd
        };
        #[cfg(not(windows))]
        let shell_cmd = |code: i32| {
            let mut cmd = std::process::Command::new("sh");
            cmd.args(["-c", &format!("exit {code}")]);
            cmd
        };

        let ok_output = shell_cmd(0)
            .output()
            .expect("spawning a real shell must succeed");
        smoke_check_result("fake-linker-that-exits-0", Ok(ok_output))
            .expect("a clean exit 0 must pass the smoke-check");

        let fail_output = shell_cmd(1)
            .output()
            .expect("spawning a real shell must succeed");
        let err = smoke_check_result("fake-linker-that-exits-1", Ok(fail_output))
            .expect_err("a non-zero exit must fail the smoke-check");
        assert!(err.contains("extracted and its hash verified correctly"));
    }

    #[test]
    fn ensure_extracted_surfaces_a_smoke_check_failure_for_a_linker_that_cannot_launch() {
        // Deliberately incomplete synthetic asset set: the "linker" bytes
        // are not a real executable at all (analogous to `ld.lld.exe`
        // missing one of its required sibling DLLs -- either way, the
        // extracted, hash-verified file cannot be launched). Confirms
        // `ensure_extracted`'s wiring of the smoke-check catches this with
        // a clear diagnostic rather than letting a broken linker through to
        // a later, more confusing failure.
        let dir = scratch_dir("smoke-check-unlaunchable");

        let bytes: &'static [u8] = b"not-a-real-executable";
        let assets: &'static [EmbeddedAsset] = Box::leak(Box::new([make_asset(
            "linker",
            "ld.lld.exe",
            None,
            "bin/ld.lld.exe",
            bytes,
        )]));

        let set = ensure_extracted_in(assets, &dir).expect("extraction of the bytes must succeed");
        let err = smoke_check_linker(&set.linker().unwrap().path)
            .expect_err("a non-executable file must fail the launch smoke-check");
        assert!(
            err.contains("could not even launch") || err.contains("exited with"),
            "expected a launch-failure diagnostic, got: {err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
