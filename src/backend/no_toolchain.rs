//! The strict **no-toolchain profile**.
//!
//! Oscan's whole direct-LLVM proposition is that a release carries its
//! own code generator and its own linker, and never needs a C compiler
//! on the machine. That is a claim worth being able to *prove* rather
//! than merely believe, so `OSCAN_NO_TOOLCHAIN=1` turns every remaining
//! escape hatch that would reach for a host C toolchain into a hard,
//! named error instead of a silent detour:
//!
//! | Escape hatch | Strict-profile behavior |
//! |---|---|
//! | `--backend c` / `--emit-c` / `-o *.c` / `--target` | refused (the C backend *is* a toolchain dependency) |
//! | `--libc` | refused (the hosted runtime links the host CRT/libm) |
//! | `--extra-c` / `--extra-cflags` | refused (needs a C compiler at link time) |
//! | Generated C shims for `extern` functions taking/returning `str` | refused |
//! | Runtime-archive auto-build from `scripts/release_tools.py` | refused |
//! | Locally compiling `runtime/osc_native_shim.c` | refused |
//! | Compiler-driver linker flavor (`gcc`/`clang` as the linker) | refused |
//!
//! The profile has two sources, and both enforce exactly the same policy:
//!
//! * **The environment.** `OSCAN_NO_TOOLCHAIN=1` in a build that *does*
//!   contain the C backend. Nothing changes unless the variable is set:
//!   it is a verification profile for CI and release smoke tests, not a
//!   new default. A build which passes with it set provably used only
//!   Oscan's own packaged artifacts.
//! * **The build itself.** A backend-specific LLVM or Cranelift
//!   distribution is compiled without `backend-c` at all (see
//!   [`super::select`]), so there is no C code generator to reach for and
//!   no honest way to offer the C-toolchain escape hatches. Such a build
//!   is strict intrinsically — there is no environment variable to unset,
//!   and no `PATH` search that could resurrect what was compiled out.

use std::env;

/// The environment variable that selects the strict profile.
pub const ENV_VAR: &str = "OSCAN_NO_TOOLCHAIN";

/// Whether the C backend was compiled out of this build entirely, which
/// makes the strict profile intrinsic rather than opt-in.
pub const TOOLCHAIN_FREE_BUILD: bool = !cfg!(feature = "backend-c");

/// Why the strict profile is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrictSource {
    /// A single-backend LLVM/Cranelift build with no C backend at all.
    Build,
    /// `OSCAN_NO_TOOLCHAIN=1` in the environment.
    Environment,
}

/// What (if anything) makes this invocation strict. The build-level
/// source is checked first: a compiler that has no C backend cannot stop
/// being strict, whatever the environment says.
pub fn strict_source() -> Option<StrictSource> {
    if TOOLCHAIN_FREE_BUILD {
        return Some(StrictSource::Build);
    }
    if env::var(ENV_VAR)
        .map(|value| parse_flag(&value))
        .unwrap_or(false)
    {
        return Some(StrictSource::Environment);
    }
    None
}

/// Whether the strict no-toolchain profile is active.
pub fn is_strict() -> bool {
    strict_source().is_some()
}

/// Interpret the variable's value. Anything other than an explicit
/// affirmative (`1`/`true`/`yes`/`on`, case-insensitive) leaves the
/// profile off — including an empty value, so `OSCAN_NO_TOOLCHAIN=`
/// behaves like "unset" rather than accidentally arming a strict build.
pub fn parse_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// The diagnostic for a refused operation. `what` names the operation
/// and `alternative` names what to do instead.
pub fn refusal(what: &str, alternative: &str) -> String {
    refusal_for(
        strict_source().unwrap_or(StrictSource::Environment),
        what,
        alternative,
    )
}

/// The wording for each strict source. They refuse the same operations
/// for the same reason, but only the environment profile can be turned
/// off by the person reading the message.
fn refusal_for(source: StrictSource, what: &str, alternative: &str) -> String {
    match source {
        StrictSource::Environment => format!(
            "{ENV_VAR}=1 (strict no-toolchain profile) refuses {what}, because it would require a \
             C toolchain on this machine; {alternative}, or unset {ENV_VAR} to allow it"
        ),
        StrictSource::Build => format!(
            "this compiler build does not include the C backend (it includes: {}), so it refuses \
             {what}, because it would require a C toolchain on this machine; {alternative}, or \
             install a package that includes the C backend",
            super::select::compiled_in_list()
        ),
    }
}

/// `Err(refusal(...))` when the strict profile is active, `Ok(())`
/// otherwise.
pub fn refuse_if_strict(what: &str, alternative: &str) -> Result<(), String> {
    if is_strict() {
        return Err(refusal(what, alternative));
    }
    Ok(())
}

/// Test-only serialization for the strict-profile environment variable.
///
/// [`ENV_VAR`] is process-global: a test that sets it changes what *every*
/// other test thread in this binary observes through [`is_strict`]. Every
/// test that writes it — or that exercises a code path whose result
/// depends on it — takes this one lock, so those tests cannot interleave.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Mutex, MutexGuard};

    static STRICT_PROFILE_ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Hold this for the whole critical section, through the final
    /// assertion and any `remove_var` cleanup.
    pub(crate) fn lock_strict_profile_env() -> MutexGuard<'static, ()> {
        STRICT_PROFILE_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_affirmatives_arm_the_strict_profile() {
        for value in ["1", "true", "TRUE", " yes ", "On"] {
            assert!(parse_flag(value), "{value:?} should arm the profile");
        }
        for value in ["", " ", "0", "false", "no", "off", "maybe"] {
            assert!(!parse_flag(value), "{value:?} should not arm the profile");
        }
    }

    #[test]
    fn the_refusal_names_the_operation_the_alternative_and_the_variable() {
        let message = refusal_for(
            StrictSource::Environment,
            "--extra-c",
            "precompile it to an object and pass --extra-obj",
        );
        assert!(message.contains("--extra-c"));
        assert!(message.contains("--extra-obj"));
        assert!(message.contains(ENV_VAR));
        assert!(message.contains("C toolchain"));
    }

    /// A build without the C backend refuses the same operations, but
    /// must not tell the user to unset a variable that would not help.
    #[test]
    fn a_toolchain_free_build_refuses_without_naming_an_escape_hatch() {
        let message = refusal_for(
            StrictSource::Build,
            "--extra-c",
            "precompile it to an object and pass --extra-obj",
        );
        assert!(message.contains("--extra-c"));
        assert!(message.contains("--extra-obj"));
        assert!(message.contains("does not include the C backend"));
        assert!(message.contains("C toolchain"));
        assert!(
            !message.contains(ENV_VAR),
            "an intrinsically strict build has no variable to unset: {message}"
        );
    }

    /// The build-level source cannot be switched off, and is reported in
    /// preference to the environment one.
    #[test]
    fn the_build_source_outranks_the_environment_source() {
        assert_eq!(TOOLCHAIN_FREE_BUILD, !cfg!(feature = "backend-c"));
        if TOOLCHAIN_FREE_BUILD {
            assert_eq!(strict_source(), Some(StrictSource::Build));
            assert!(is_strict());
        }
    }

    /// The environment profile arms (and disarms) a build that has a C
    /// backend to refuse in the first place; a build without one is
    /// already strict and stays strict whatever the variable says.
    #[test]
    fn the_environment_variable_arms_a_build_that_has_a_c_backend() {
        let _lock = testing::lock_strict_profile_env();
        let restore = env::var(ENV_VAR).ok();

        env::remove_var(ENV_VAR);
        assert_eq!(is_strict(), TOOLCHAIN_FREE_BUILD);

        env::set_var(ENV_VAR, "1");
        assert!(is_strict());
        assert_eq!(
            strict_source(),
            Some(if TOOLCHAIN_FREE_BUILD {
                StrictSource::Build
            } else {
                StrictSource::Environment
            })
        );

        env::set_var(ENV_VAR, "0");
        assert_eq!(is_strict(), TOOLCHAIN_FREE_BUILD);

        match restore {
            Some(value) => env::set_var(ENV_VAR, value),
            None => env::remove_var(ENV_VAR),
        }
    }
}
