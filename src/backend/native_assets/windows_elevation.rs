//! Windows-only process-elevation detection (Finding 4c, security review;
//! signature updated to fail-closed `Result` in the 2026-07-15 follow-up
//! review -- see [`is_elevated`]'s doc comment).
//!
//! Used by [`super`]'s fail-closed elevation policy so an elevated `oscan`
//! process refuses a native final link/`--run` entirely, rather than
//! trusting or reusing the standard per-user native-asset cache: a
//! non-elevated process running as the *same* Windows user could have
//! pre-populated or tampered with it before this process elevated
//! (elevation changes the process token/privileges, not the user
//! account), so that shared cache is not a safe boundary for an elevated
//! process even though its ACL is otherwise per-user-private.

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Elevation check: `Ok(true)`/`Ok(false)` only when the OS-level check
/// genuinely succeeds; `Err(reason)` when the underlying
/// `OpenProcessToken`/`GetTokenInformation` calls themselves fail.
///
/// Security review 2026-07-15 (findings 2 & 3): this used to return a bare
/// `bool`, folding any detection failure into `false` ("not elevated").
/// That was acceptable under the old "elevated = extra caution only"
/// model (a false negative there only cost performance -- re-extracting
/// instead of reusing a cache -- never safety), but is a fail-*open* bug
/// under the new fail-*closed* policy (refuse a final link/`--run`
/// entirely while elevated): silently treating "we couldn't tell" as "not
/// elevated" would let a detection failure bypass the refusal. Callers
/// must route a detection error through
/// [`super::check_elevation_policy`], which treats `Err` the same as
/// `Ok(true)` for [`super::NativeLinkOperation::FinalLink`].
pub(super) fn is_elevated() -> Result<bool, String> {
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(format!(
                "OpenProcessToken failed (GetLastError={})",
                GetLastError()
            ));
        }

        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned_len: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_len,
        );
        let last_error = if ok == 0 { Some(GetLastError()) } else { None };
        CloseHandle(token);

        match last_error {
            Some(code) => Err(format!("GetTokenInformation failed (GetLastError={code})")),
            None => Ok(elevation.TokenIsElevated != 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_elevated_never_panics_and_returns_a_result() {
        // We cannot control whether the test runner itself is elevated, so
        // this only proves the FFI call plumbing works end-to-end on this
        // host without crashing; the elevated branch of
        // `check_elevation_policy` is exercised with an explicit `Result`
        // instead (see `native_assets`'s own tests).
        let _ = is_elevated();
    }
}
