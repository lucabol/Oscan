//! Native target selection.
//!
//! Isolates every "which triple / which ISA / which linker family" decision
//! behind one small enum so the rest of the backend never has to reason
//! about `target_lexicon::Triple` directly. Only [`NativeTarget::WindowsX64`]
//! is validated end-to-end (object emission + linking + running) today; the
//! Linux variants produce verified relocatable objects via Cranelift's
//! cross-codegen support but report a clear error at link time when this
//! host has no matching cross linker/runtime archive available (see
//! `super::link`). That is a tooling gap, not a code-generation one.

use std::sync::Arc;

use cranelift_codegen::isa::TargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use target_lexicon::Triple;

/// A native compilation target the Cranelift backend knows how to select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeTarget {
    WindowsX64,
    LinuxX64,
    LinuxAarch64,
    LinuxRiscv64,
}

impl NativeTarget {
    /// Parse a `--native-target` value. Accepts the same tags used by the
    /// runtime-archive contract (`packaging/toolchains/runtime-archive-contract.json`)
    /// for the x86-64 targets, plus two Linux cross-codegen-only targets.
    /// Returns `None` both for a tag this backend has never heard of *and*
    /// for `"host"` on a machine [`NativeTarget::try_host`] cannot
    /// identify — callers that need to tell those two cases apart (e.g.
    /// to print a more specific error for the latter) should check for
    /// `"host"` and call [`NativeTarget::try_host`] directly instead of
    /// going through this method, the way `main.rs` does.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "host" => Self::try_host().ok(),
            "windows-x86_64" => Some(Self::WindowsX64),
            "linux-x86_64" => Some(Self::LinuxX64),
            "linux-aarch64" => Some(Self::LinuxAarch64),
            "linux-riscv64" => Some(Self::LinuxRiscv64),
            _ => None,
        }
    }

    /// All target tags accepted by [`NativeTarget::parse`], for error messages.
    pub fn accepted_values() -> &'static str {
        "host, windows-x86_64, linux-x86_64, linux-aarch64, linux-riscv64"
    }

    /// The host's own native target, used as the default when no
    /// `--native-target` is given.
    ///
    /// Panics with the same message [`NativeTarget::try_host`] would
    /// return as an `Err` if this host's OS/architecture isn't one this
    /// backend supports. This exists only for the few callers that
    /// already have no fallible path of their own ([`NativeTarget::is_host`]
    /// below, and `super::link`'s host-mismatch error message, which only
    /// ever runs once a target has already been resolved some other way).
    /// Anything selecting *the* native-backend target for this process —
    /// e.g. `main.rs`'s default-target selection — must call
    /// [`NativeTarget::try_host`] directly instead, so an unsupported host
    /// is a clean CLI error rather than a panic.
    pub fn host() -> Self {
        Self::try_host().unwrap_or_else(|e| panic!("{e}"))
    }

    /// Fallibly detect the host's own native target from
    /// `std::env::consts::OS`/`std::env::consts::ARCH`. Returns a
    /// descriptive `Err` — never a default target — when this host's
    /// OS/architecture combination isn't one this backend has a
    /// [`NativeTarget`] variant for (e.g. macOS on any architecture,
    /// 32-bit x86, or any other OS/architecture pair). Silently guessing
    /// a *different* target in that situation (this backend used to
    /// default to [`NativeTarget::WindowsX64`] unconditionally) would let
    /// `--backend native` with no explicit `--native-target` emit an
    /// object file for the wrong machine — a miscompile a user might not
    /// notice until the resulting object fails to link or run — instead
    /// of failing loudly at the point the mismatch is actually known.
    pub fn try_host() -> Result<Self, String> {
        Self::detect(env_os(), env_arch())
    }

    /// The OS/architecture matching behind [`NativeTarget::try_host`],
    /// factored out so it can be unit-tested against arbitrary
    /// `std::env::consts::OS`/`ARCH`-shaped strings (macOS, an unknown
    /// host, ...) without needing to actually run this build on each one.
    fn detect(os: &str, arch: &str) -> Result<Self, String> {
        match (os, arch) {
            ("windows", "x86_64") => Ok(Self::WindowsX64),
            ("linux", "x86_64") => Ok(Self::LinuxX64),
            ("linux", "aarch64") => Ok(Self::LinuxAarch64),
            ("linux", "riscv64") => Ok(Self::LinuxRiscv64),
            _ => Err(format!(
                "unsupported host OS/architecture '{os}/{arch}' for the native backend's \
                 default (\"host\") target detection — supported hosts: windows/x86_64, \
                 linux/x86_64, linux/aarch64, linux/riscv64; pass --native-target explicitly \
                 to select one of those targets instead of relying on host auto-detection \
                 (accepted tags: {})",
                Self::accepted_values()
            )),
        }
    }

    /// Whether this target matches the machine oscan itself is running on
    /// (i.e. whether `--run` / direct linking without a cross toolchain is
    /// expected to work). `false` — never a panic — when the host itself
    /// isn't one this backend can identify as any [`NativeTarget`]: no
    /// explicit target can equal an undetectable host, so cross-linking
    /// tooling errors (see `super::link`) still apply rather than a panic
    /// replacing them.
    pub fn is_host(&self) -> bool {
        Self::try_host().map(|h| h == *self).unwrap_or(false)
    }

    /// The archive/tooling tag, matching `packaging/toolchains/runtime-archive-contract.json`'s
    /// `{target}` naming (`detect_host_target()` in `scripts/release_tools.py`).
    pub fn archive_tag(&self) -> &'static str {
        match self {
            Self::WindowsX64 => "windows-x86_64",
            Self::LinuxX64 => "linux-x86_64",
            Self::LinuxAarch64 => "linux-aarch64",
            Self::LinuxRiscv64 => "linux-riscv64",
        }
    }

    /// The `target_lexicon` triple used to configure Cranelift. Windows uses
    /// the `gnu` environment because the validated linking path in
    /// `super::link` drives GCC/Clang (MinGW-w64 on Windows) as the linker,
    /// not `link.exe`/MSVC.
    pub fn triple(&self) -> Triple {
        let raw = match self {
            Self::WindowsX64 => "x86_64-pc-windows-gnu",
            Self::LinuxX64 => "x86_64-unknown-linux-gnu",
            Self::LinuxAarch64 => "aarch64-unknown-linux-gnu",
            Self::LinuxRiscv64 => "riscv64gc-unknown-linux-gnu",
        };
        raw.parse()
            .unwrap_or_else(|e| panic!("internal error: invalid built-in triple '{raw}': {e}"))
    }

    pub fn exe_suffix(&self) -> &'static str {
        match self {
            Self::WindowsX64 => ".exe",
            _ => "",
        }
    }

    pub fn obj_suffix(&self) -> &'static str {
        match self {
            Self::WindowsX64 => ".obj",
            _ => ".o",
        }
    }
}

impl std::fmt::Display for NativeTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.archive_tag())
    }
}

fn env_os() -> &'static str {
    std::env::consts::OS
}

fn env_arch() -> &'static str {
    std::env::consts::ARCH
}

/// Build a Cranelift `TargetIsa` for `target`, with settings suitable for
/// emitting relocatable, statically-linkable object code:
///
/// * PIC is left off — Oscan produces standalone, non-relocatable-at-runtime
///   executables (no dynamic loader relocations expected), matching the
///   static-archive linking model in `super::link`.
/// * Stack-probing is disabled: Oscan's generated functions keep modest
///   stack frames (aggregates are arena-allocated, not stack-allocated —
///   see `super::layout`), so the inline-probe/`__cranelift_probestack`
///   distinction does not matter for the supported corpus. This is a
///   documented simplification, not a silent correctness gap: pathological
///   functions with many kilobytes of scalar locals on Windows could in
///   principle skip a guard page.
/// * `opt_level` is `speed_and_size` rather than plain `speed`: Cranelift
///   documents it as "like speed, but also perform transformations aimed
///   at reducing code size" with no downside versus `speed`, so it is
///   strictly the better release default whenever both are available.
///   As of cranelift-codegen 0.133, `Speed` and `SpeedAndSize` are in fact
///   handled identically by the x86-64 backend and its optimizer passes
///   (only `OptLevel::None` takes a different, unoptimized path) — verified
///   by diffing every linkable `examples/*.osc` program's emitted object
///   bytes under both settings (all byte-for-byte identical, including the
///   two largest, `led.osc` and `sh.osc`). `speed_and_size` is kept anyway
///   as the setting whose documented behavior actually matches the native
///   backend's goal, so a future Cranelift upgrade that adds a real
///   `SpeedAndSize`-specific size optimization is picked up automatically
///   without another change here.
pub fn build_isa(target: NativeTarget) -> Result<Arc<dyn TargetIsa>, String> {
    let triple = target.triple();
    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", "speed_and_size")
        .map_err(|e| format!("internal error configuring Cranelift settings: {e}"))?;
    flag_builder
        .set("is_pic", "false")
        .map_err(|e| format!("internal error configuring Cranelift settings: {e}"))?;
    flag_builder
        .set("enable_probestack", "false")
        .map_err(|e| format!("internal error configuring Cranelift settings: {e}"))?;

    let isa_builder = cranelift_codegen::isa::lookup(triple.clone()).map_err(|e| {
        format!(
            "target '{}' ({triple}) is not supported by this build of the Cranelift backend: {e}",
            target.archive_tag()
        )
    })?;
    isa_builder
        .finish(settings::Flags::new(flag_builder))
        .map_err(|e| {
            format!(
                "failed to configure Cranelift ISA for target '{}': {e}",
                target.archive_tag()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cranelift_codegen::settings::OptLevel;

    /// Locks in the size-oriented ISA settings `build_isa` configures — a
    /// regression here would silently regress release native-backend
    /// binary size (e.g. reverting to `opt_level = "speed"`, or turning PIC
    /// or stack probes back on) without any compile error to catch it.
    #[test]
    fn build_isa_uses_size_oriented_release_settings() {
        for target in [
            NativeTarget::WindowsX64,
            NativeTarget::LinuxX64,
            NativeTarget::LinuxAarch64,
            NativeTarget::LinuxRiscv64,
        ] {
            let isa = build_isa(target).expect("every built-in target must configure an ISA");
            let flags = isa.flags();
            assert_eq!(
                flags.opt_level(),
                OptLevel::SpeedAndSize,
                "{target} must use the speed_and_size opt level"
            );
            assert!(!flags.is_pic(), "{target} must not enable PIC");
            assert!(
                !flags.enable_probestack(),
                "{target} must not enable stack probes"
            );
        }
    }

    #[test]
    fn detect_recognizes_every_supported_host() {
        assert_eq!(
            NativeTarget::detect("windows", "x86_64"),
            Ok(NativeTarget::WindowsX64)
        );
        assert_eq!(
            NativeTarget::detect("linux", "x86_64"),
            Ok(NativeTarget::LinuxX64)
        );
        assert_eq!(
            NativeTarget::detect("linux", "aarch64"),
            Ok(NativeTarget::LinuxAarch64)
        );
        assert_eq!(
            NativeTarget::detect("linux", "riscv64"),
            Ok(NativeTarget::LinuxRiscv64)
        );
    }

    /// macOS has no `NativeTarget` variant at all today (see module docs:
    /// only Windows x86-64 is validated end-to-end, and the Linux variants
    /// are cross-codegen-only) — on *either* Apple Silicon or Intel Macs,
    /// host auto-detection must fail with a clear, actionable error, never
    /// silently pick some other target (this backend previously defaulted
    /// unconditionally to Windows x86-64/COFF for exactly this case).
    #[test]
    fn detect_reports_a_clean_error_for_macos_x86_64() {
        let err = NativeTarget::detect("macos", "x86_64").expect_err("macOS is not supported");
        assert!(
            err.contains("macos/x86_64"),
            "error should name the unsupported host: {err}"
        );
        assert!(
            err.contains("--native-target"),
            "error should point at the escape hatch: {err}"
        );
        assert!(
            !err.contains("windows-x86_64") || err.contains("accepted tags"),
            "error must not read as silently selecting Windows: {err}"
        );
    }

    #[test]
    fn detect_reports_a_clean_error_for_macos_aarch64() {
        let err = NativeTarget::detect("macos", "aarch64").expect_err("macOS is not supported");
        assert!(
            err.contains("macos/aarch64"),
            "error should name the unsupported host: {err}"
        );
        assert!(
            err.contains("--native-target"),
            "error should point at the escape hatch: {err}"
        );
    }

    /// A completely unrecognized OS/architecture pair (as opposed to a
    /// merely-uncovered *known* OS like macOS) must fail the same way —
    /// there is no OS-specific special case that could let an unknown
    /// host slip through to a default target.
    #[test]
    fn detect_reports_a_clean_error_for_an_unknown_host() {
        let err =
            NativeTarget::detect("plan9", "mips").expect_err("plan9/mips is not a known host");
        assert!(
            err.contains("plan9/mips"),
            "error should name the unrecognized host: {err}"
        );
        assert!(
            err.contains("--native-target"),
            "error should point at the escape hatch: {err}"
        );
    }

    /// Windows on a non-x86_64 architecture (e.g. ARM64) must not be
    /// folded into [`NativeTarget::WindowsX64`] just because the OS
    /// matches — the architecture also has to match a real variant.
    #[test]
    fn detect_reports_a_clean_error_for_windows_aarch64() {
        let err = NativeTarget::detect("windows", "aarch64").expect_err("no Windows ARM64 variant");
        assert!(
            err.contains("windows/aarch64"),
            "error should name the unsupported host: {err}"
        );
    }

    #[test]
    fn try_host_matches_detect_of_the_real_environment() {
        assert_eq!(
            NativeTarget::try_host(),
            NativeTarget::detect(env_os(), env_arch())
        );
    }

    /// `parse("host")` must fold a failed auto-detection into `None` —
    /// the same "not usable" signal an unrecognized tag produces — rather
    /// than panicking, even though the infallible [`NativeTarget::host`]
    /// (used only by call sites with no fallible path of their own) still
    /// panics in that situation.
    #[test]
    fn parse_host_is_none_when_detection_is_impossible() {
        // This process' own host is always one `NativeTarget::parse` can
        // resolve (the test suite only runs on supported CI hosts), so
        // assert the *shape* of the contract instead of forcing a
        // specific unsupported host through the real `parse` entry
        // point: `parse("host")` must never panic, and must agree with
        // `try_host().ok()`.
        assert_eq!(NativeTarget::parse("host"), NativeTarget::try_host().ok());
    }

    #[test]
    fn is_host_never_panics_even_when_compared_against_every_target() {
        // Regardless of which target this actually is on the current CI
        // host, `is_host` must return a plain bool for every variant
        // without panicking.
        for target in [
            NativeTarget::WindowsX64,
            NativeTarget::LinuxX64,
            NativeTarget::LinuxAarch64,
            NativeTarget::LinuxRiscv64,
        ] {
            let _ = target.is_host();
        }
    }
}
