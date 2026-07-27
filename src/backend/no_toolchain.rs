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
//! | `--extra-c` | refused (needs a C compiler at link time) |
//! | Runtime-archive auto-build from `scripts/release_tools.py` | refused |
//! | Locally compiling `runtime/osc_native_shim.c` | refused |
//! | Compiler-driver linker flavor (`gcc`/`clang` as the linker) | refused |
//!
//! Nothing here changes behavior unless the variable is set: it is a
//! verification profile for CI and release smoke tests, not a new
//! default. The point is that a build which passes with
//! `OSCAN_NO_TOOLCHAIN=1` provably used only Oscan's own packaged
//! artifacts.

use std::env;

/// The environment variable that selects the strict profile.
pub const ENV_VAR: &str = "OSCAN_NO_TOOLCHAIN";

/// Whether the strict no-toolchain profile is active.
pub fn is_strict() -> bool {
    env::var(ENV_VAR)
        .map(|value| parse_flag(&value))
        .unwrap_or(false)
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
    format!(
        "{ENV_VAR}=1 (strict no-toolchain profile) refuses {what}, because it would require a C \
         toolchain on this machine; {alternative}, or unset {ENV_VAR} to allow it"
    )
}

/// `Err(refusal(...))` when the strict profile is active, `Ok(())`
/// otherwise.
pub fn refuse_if_strict(what: &str, alternative: &str) -> Result<(), String> {
    if is_strict() {
        return Err(refusal(what, alternative));
    }
    Ok(())
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
        let message = refusal(
            "--extra-c",
            "precompile it to an object and pass --extra-obj",
        );
        assert!(message.contains("--extra-c"));
        assert!(message.contains("--extra-obj"));
        assert!(message.contains(ENV_VAR));
        assert!(message.contains("C toolchain"));
    }
}
