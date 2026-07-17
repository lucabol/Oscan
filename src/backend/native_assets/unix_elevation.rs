//! Unix-only setuid/setgid elevation detection (design §10.7).
//!
//! Used by [`super`]'s fail-closed elevation policy so a setuid- or
//! setgid-elevated `oscan` process refuses a native final link/`--run`
//! entirely, rather than trusting or reusing the standard per-user
//! native-asset cache: a non-elevated process running as the *same* Unix
//! user could have pre-populated or tampered with it before this process
//! acquired elevated privileges via setuid/setgid (elevation changes the
//! effective UID/GID, not the real UID/GID), so that shared cache is not
//! a safe boundary for an elevated process.

// Hand-rolled extern declarations — no new `libc` crate dependency,
// mirrors this codebase's existing "hand-rolled, no `dirs` dependency"
// convention for `cache_root()`.
extern "C" {
    fn geteuid() -> u32;
    fn getuid() -> u32;
    fn getegid() -> u32;
    fn getgid() -> u32;
}

/// Detects setuid or setgid elevation (euid != uid or egid != gid).
///
/// Returns `Ok(true)` when elevated (effective UID differs from real UID,
/// or effective GID differs from real GID), `Ok(false)` when not elevated.
/// These syscalls cannot fail on any POSIX system, but the `Result` shape
/// is kept for symmetry with the Windows policy function
/// ([`super::check_elevation_policy`] takes `Result<bool, String>`) and to
/// allow a future extension.
pub(super) fn is_setuid_elevated() -> Result<bool, String> {
    let euid = unsafe { geteuid() };
    let uid = unsafe { getuid() };
    let egid = unsafe { getegid() };
    let gid = unsafe { getgid() };
    Ok(euid != uid || egid != gid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_setuid_elevated_never_panics_and_returns_a_result() {
        // We cannot control whether the test runner itself is setuid, so
        // this only proves the FFI call plumbing works end-to-end on this
        // host without crashing; the elevated branch of
        // `check_elevation_policy` is exercised with an explicit `Result`
        // instead (see `native_assets`'s own tests).
        let result = is_setuid_elevated();
        assert!(result.is_ok());
    }
}
