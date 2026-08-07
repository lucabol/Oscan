//! Backend identity, compile-time availability, and default selection.
//!
//! Oscan has three code generators — `llvm`, `cranelift`, and `c` — and two
//! independent questions about them:
//!
//! 1. **Which backends exist in this binary at all?** A development build
//!    (the default feature set) compiles in all three. A slim distribution is
//!    built with exactly one (`--no-default-features --features backend-llvm`,
//!    say), so the other two are not merely disabled, their code and
//!    dependencies are absent. A `full` distribution deliberately contains
//!    all three.
//! 2. **Which backend runs when the user does not say?** A development
//!    build keeps the historical capability-based policy (prefer the direct
//!    LLVM code generator when it is actually loadable for this host, then
//!    Cranelift, then C). A distribution build defaults deterministically to
//!    a fixed backend (`llvm` for `full`, the included backend for a slim
//!    profile) — never to a probe result or a backend it does not contain.
//!
//! Both answers live here so `main.rs` never has to spell a `cfg!` out
//! inline, and so the policy itself is unit-testable against arbitrary
//! availability/distribution combinations rather than only against the
//! configuration this test binary happens to be built with.
//!
//! # Naming
//!
//! `cranelift` is the canonical spelling of the Cranelift backend
//! everywhere: `--backend cranelift`, help text, `--version` metadata, and
//! diagnostics. `--backend native` is retained as a compatibility alias
//! (with one deprecation warning) because it was the original spelling, but
//! it is never *displayed*. Note that "native" remains the right word for
//! the concepts LLVM and Cranelift share — [`super::NativeTarget`],
//! `native_assets`, the native runtime archives, and the `OSCAN_NATIVE_*`
//! environment variables all keep their names.

/// The backend a compilation runs through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Llvm,
    Cranelift,
    C,
}

/// Every backend, in the canonical order used by help text, `--version`
/// metadata, error messages, and the last-resort default.
pub const ALL: [Backend; 3] = [Backend::Llvm, Backend::Cranelift, Backend::C];

/// The deprecated spelling of `--backend cranelift`.
pub const NATIVE_ALIAS: &str = "native";

/// Normalized packaged-profile and default-backend values stamped by
/// `build.rs`. Empty means the corresponding property is not configured.
const DISTRIBUTION_PROFILE_RAW: &str = env!("OSCAN_DISTRIBUTION_PROFILE");
const DEFAULT_BACKEND_RAW: &str = env!("OSCAN_DEFAULT_BACKEND");

/// Compatibility metadata for legacy slim distributions. New code should use
/// `distribution_profile` or `configured_default_backend`.
const DISTRIBUTION_BACKEND_RAW: &str = env!("OSCAN_DISTRIBUTION_BACKEND");

impl Backend {
    /// The canonical, user-visible name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Llvm => "llvm",
            Self::Cranelift => "cranelift",
            Self::C => "c",
        }
    }

    /// Parse a canonical backend name (no aliases).
    pub fn parse(value: &str) -> Option<Self> {
        ALL.into_iter().find(|backend| backend.as_str() == value)
    }

    /// Parse a `--backend` value, accepting the deprecated `native` alias.
    /// The returned flag reports whether the alias spelling was used, so
    /// the caller can emit exactly one deprecation warning.
    pub fn parse_cli(value: &str) -> Option<(Self, bool)> {
        if value == NATIVE_ALIAS {
            return Some((Self::Cranelift, true));
        }
        Self::parse(value).map(|backend| (backend, false))
    }

    /// Whether this backend's code was compiled into this binary.
    pub const fn is_compiled_in(self) -> bool {
        match self {
            Self::Llvm => cfg!(feature = "backend-llvm"),
            Self::Cranelift => cfg!(feature = "backend-cranelift"),
            Self::C => cfg!(feature = "backend-c"),
        }
    }

    /// The suffix a backend-specific release archive carries, e.g.
    /// `oscan-v1.2.3-windows-x86_64-llvm.zip`. Slim packages use these
    /// suffixes; supported targets may also publish a `-full` archive.
    pub fn artifact_suffix(self) -> &'static str {
        match self {
            Self::Llvm => "-llvm",
            Self::Cranelift => "-cranelift",
            Self::C => "-c",
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which backends a build contains. A value rather than a bare `cfg!` so
/// the selection policy below can be tested against every combination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Availability {
    pub llvm: bool,
    pub cranelift: bool,
    pub c: bool,
}

impl Availability {
    /// What *this* binary was compiled with.
    pub const COMPILED_IN: Self = Self {
        llvm: Backend::Llvm.is_compiled_in(),
        cranelift: Backend::Cranelift.is_compiled_in(),
        c: Backend::C.is_compiled_in(),
    };

    pub fn has(self, backend: Backend) -> bool {
        match backend {
            Backend::Llvm => self.llvm,
            Backend::Cranelift => self.cranelift,
            Backend::C => self.c,
        }
    }

    /// The compiled-in backends, in canonical order.
    pub fn list(self) -> Vec<Backend> {
        ALL.into_iter().filter(|b| self.has(*b)).collect()
    }

    /// The first compiled-in backend in canonical order. `None` only for
    /// the degenerate "no backend at all" build, which `build.rs` rejects.
    fn first(self) -> Option<Backend> {
        self.list().into_iter().next()
    }
}

/// The CLI facts that steer implicit (no `--backend`) selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionInputs {
    /// `--emit-c` or `-o *.c`.
    pub c_source_output: bool,
    /// `--emit-llvm-ir` or `-o *.ll`.
    pub llvm_ir_output: bool,
    /// `--target` (the C backend's riscv64/wasi cross-compile).
    pub c_cross_target: bool,
    /// `--native-target` (an LLVM/Cranelift object target).
    pub native_target_requested: bool,
    /// This host has a [`super::NativeTarget`] at all.
    pub native_host_supported: bool,
    /// Oscan's packaged LLVM code generator loaded *and* supports this host.
    pub llvm_available: bool,
}

/// The backends compiled into this binary, in canonical order.
pub fn compiled_in() -> Vec<Backend> {
    Availability::COMPILED_IN.list()
}

/// `"llvm, cranelift, c"` — for diagnostics.
pub fn compiled_in_list() -> String {
    compiled_in()
        .iter()
        .map(|b| b.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `"llvm|cranelift|c"` — for `--backend`'s help/usage text.
pub fn cli_choices() -> String {
    compiled_in()
        .iter()
        .map(|b| b.as_str())
        .collect::<Vec<_>>()
        .join("|")
}

/// The package profile this build belongs to, if any.
pub fn distribution_profile() -> Option<&'static str> {
    match DISTRIBUTION_PROFILE_RAW {
        "" => None,
        "full" => Some("full"),
        "llvm" => Some("llvm"),
        "cranelift" => Some("cranelift"),
        "c" => Some("c"),
        _ => unreachable!("build.rs validates OSCAN_DISTRIBUTION_PROFILE"),
    }
}

/// Whether this binary belongs to any packaged profile. Packaged builds use
/// strict executable-relative asset lookup.
pub fn is_distribution() -> bool {
    distribution_profile().is_some()
}

/// The deterministic build-time default, if one was configured.
pub fn configured_default_backend() -> Option<Backend> {
    match DEFAULT_BACKEND_RAW {
        "" => None,
        "llvm" => Some(Backend::Llvm),
        "cranelift" => Some(Backend::Cranelift),
        "c" => Some(Backend::C),
        _ => unreachable!("build.rs validates OSCAN_DEFAULT_BACKEND"),
    }
}

/// Compatibility accessor for code that identifies slim profiles by backend.
/// Full distributions intentionally return `None`.
#[allow(dead_code)]
pub fn distribution_backend() -> Option<Backend> {
    match DISTRIBUTION_BACKEND_RAW {
        "" => None,
        "llvm" => Some(Backend::Llvm),
        "cranelift" => Some(Backend::Cranelift),
        "c" => Some(Backend::C),
        _ => unreachable!("build.rs validates OSCAN_DISTRIBUTION_BACKEND"),
    }
}

// Defense in depth: build.rs validates the same complete contract.
const _: () = assert!(
    distribution_config_matches_features(),
    "invalid packaged distribution profile/default for the enabled backends"
);

const fn distribution_config_matches_features() -> bool {
    let enabled = Backend::Llvm.is_compiled_in() as u8
        + Backend::Cranelift.is_compiled_in() as u8
        + Backend::C.is_compiled_in() as u8;
    if enabled == 0 {
        return false;
    }

    let default_enabled = if const_str_eq(DEFAULT_BACKEND_RAW, "") {
        true
    } else if const_str_eq(DEFAULT_BACKEND_RAW, "llvm") {
        Backend::Llvm.is_compiled_in()
    } else if const_str_eq(DEFAULT_BACKEND_RAW, "cranelift") {
        Backend::Cranelift.is_compiled_in()
    } else if const_str_eq(DEFAULT_BACKEND_RAW, "c") {
        Backend::C.is_compiled_in()
    } else {
        false
    };
    if !default_enabled {
        return false;
    }

    if const_str_eq(DISTRIBUTION_PROFILE_RAW, "") {
        const_str_eq(DISTRIBUTION_BACKEND_RAW, "")
    } else if const_str_eq(DISTRIBUTION_PROFILE_RAW, "full") {
        const_str_eq(DEFAULT_BACKEND_RAW, "llvm")
            && const_str_eq(DISTRIBUTION_BACKEND_RAW, "")
            && enabled == 3
    } else if const_str_eq(DISTRIBUTION_PROFILE_RAW, "llvm") {
        const_str_eq(DEFAULT_BACKEND_RAW, "llvm")
            && const_str_eq(DISTRIBUTION_BACKEND_RAW, "llvm")
            && Backend::Llvm.is_compiled_in()
            && enabled == 1
    } else if const_str_eq(DISTRIBUTION_PROFILE_RAW, "cranelift") {
        const_str_eq(DEFAULT_BACKEND_RAW, "cranelift")
            && const_str_eq(DISTRIBUTION_BACKEND_RAW, "cranelift")
            && Backend::Cranelift.is_compiled_in()
            && enabled == 1
    } else if const_str_eq(DISTRIBUTION_PROFILE_RAW, "c") {
        const_str_eq(DEFAULT_BACKEND_RAW, "c")
            && const_str_eq(DISTRIBUTION_BACKEND_RAW, "c")
            && Backend::C.is_compiled_in()
            && enabled == 1
    } else {
        false
    }
}

const fn const_str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Resolve the backend for this invocation against what this build
/// actually contains.
pub fn resolve_backend(explicit: Option<Backend>, inputs: SelectionInputs) -> Backend {
    resolve_backend_with(
        explicit,
        inputs,
        configured_default_backend(),
        Availability::COMPILED_IN,
    )
}

/// The selection policy itself, parameterized over the configured default
/// and the compiled-in set so every combination is testable.
///
/// An explicit `--backend` always wins — including when it names a backend
/// this build does not contain, so the caller can report *that* precisely
/// (see [`unavailable_error`]) instead of silently running something else.
/// Output-format flags come next, because `--emit-c`/`-o *.c`/`--target`
/// and `--emit-llvm-ir`/`-o *.ll` only have one possible producer each. A
/// configured build then pins the default, before any
/// host probing, so the same command line always selects the same backend
/// on every machine. Only an ordinary development build reaches the
/// historical capability-based policy.
pub fn resolve_backend_with(
    explicit: Option<Backend>,
    inputs: SelectionInputs,
    configured_default: Option<Backend>,
    available: Availability,
) -> Backend {
    if let Some(explicit) = explicit {
        return explicit;
    }
    if inputs.c_source_output || inputs.c_cross_target {
        return Backend::C;
    }
    if inputs.llvm_ir_output {
        return Backend::Llvm;
    }
    if let Some(configured_default) = configured_default {
        return configured_default;
    }
    if inputs.native_target_requested {
        // `--native-target` names an object target shared by both object
        // backends; prefer Cranelift (historical behavior), but a build
        // without it is still an object-backend request, not a C one.
        if available.has(Backend::Cranelift) {
            return Backend::Cranelift;
        }
        if available.has(Backend::Llvm) {
            return Backend::Llvm;
        }
    }
    if inputs.llvm_available && inputs.native_host_supported && available.has(Backend::Llvm) {
        return Backend::Llvm;
    }
    if inputs.native_host_supported && available.has(Backend::Cranelift) {
        return Backend::Cranelift;
    }
    if available.has(Backend::C) {
        return Backend::C;
    }
    // No policy branch applies to what this build contains (e.g. an
    // LLVM-only build on a host with no `NativeTarget`): pick the highest
    // priority backend that exists so the ensuing diagnostic comes from
    // the backend itself, rather than from a backend that isn't here.
    available.first().unwrap_or(Backend::C)
}

/// The one-line warning emitted when `--backend native` is used.
pub fn deprecated_alias_warning() -> String {
    format!(
        "warning: '--backend {NATIVE_ALIAS}' is deprecated; use '--backend {}' \
         ('{NATIVE_ALIAS}' remains a compatibility alias)",
        Backend::Cranelift
    )
}

/// The error for an explicitly requested backend this build does not
/// contain. Names what *is* here, and how to find a package that has what
/// isn't — without promising that this machine's platform publishes it
/// (macOS, for example, publishes only the C package today).
pub fn unavailable_error(requested: Backend) -> String {
    format!(
        "the {requested} backend is not included in this compiler build (this build includes: \
         {}); to use it, install a release artifact whose archive name ends in '-full' or '{}' \
         for a platform that publishes that profile (oscan-v<version>-<target>-full.zip / \
         .tar.xz / .tar.gz or oscan-v<version>-<target>{}.zip / .tar.xz / .tar.gz, or the \
         corresponding .msi installer where one is published) — not every platform publishes \
         every profile",
        compiled_in_list(),
        requested.artifact_suffix(),
        requested.artifact_suffix()
    )
}

/// How the help text describes what happens without `--backend`, in terms
/// of what this build actually contains: a stamped distribution names its
/// configured default, a single-backend build names the only one it has, and a
/// multi-backend build describes the capability-based policy over exactly
/// the backends it can choose between.
pub fn default_description() -> String {
    describe_default(configured_default_backend(), Availability::COMPILED_IN)
}

/// [`default_description`]'s policy, parameterized so every build shape is
/// testable.
fn describe_default(configured_default: Option<Backend>, available: Availability) -> String {
    if let Some(backend) = configured_default {
        if available.list().len() == 1 {
            return format!("{backend}; this build includes only the {backend} backend");
        }
        return format!("{backend}; configured at build time");
    }
    let available = available.list();
    if let [only] = available.as_slice() {
        return format!("{only}; the only backend in this build");
    }
    let rest: Vec<&str> = available
        .iter()
        .filter(|backend| **backend != Backend::Llvm)
        .map(|backend| backend.as_str())
        .collect();
    if available.contains(&Backend::Llvm) {
        format!(
            "LLVM when its packaged code generator is available; {} otherwise",
            rest.join("/")
        )
    } else {
        // cranelift + c: the object backend leads whenever this host has a
        // native target at all.
        "cranelift when this host is a supported native target; c otherwise".to_string()
    }
}

/// The build-metadata block appended to `--version`, so tests and release
/// smoke checks can assert which backends a packaged compiler contains and
/// which one it defaults to without parsing help text or compiling
/// anything.
pub fn version_metadata() -> String {
    let distribution = distribution_profile().unwrap_or("none");
    format!(
        "backends: {}\ndefault-backend: {}\ndistribution: {distribution}\ntoolchain-free: {}",
        compiled_in_list(),
        default_backend_label(configured_default_backend(), Availability::COMPILED_IN),
        if super::no_toolchain::TOOLCHAIN_FREE_BUILD {
            "yes"
        } else {
            "no"
        }
    )
}

/// The `default-backend:` value: a concrete backend name whenever the
/// default is decided at build time (a stamped distribution, or a build
/// with only one backend to choose from), `auto` when it still depends on
/// what this host can do.
fn default_backend_label(configured_default: Option<Backend>, available: Availability) -> String {
    match (configured_default, available.list().as_slice()) {
        (Some(backend), _) => backend.as_str().to_string(),
        (None, [only]) => only.as_str().to_string(),
        _ => "auto".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_BACKENDS: Availability = Availability {
        llvm: true,
        cranelift: true,
        c: true,
    };
    const LLVM_ONLY: Availability = Availability {
        llvm: true,
        cranelift: false,
        c: false,
    };
    const CRANELIFT_ONLY: Availability = Availability {
        llvm: false,
        cranelift: true,
        c: false,
    };
    const C_ONLY: Availability = Availability {
        llvm: false,
        cranelift: false,
        c: true,
    };

    fn inputs() -> SelectionInputs {
        SelectionInputs::default()
    }

    #[test]
    fn cranelift_is_the_canonical_spelling_and_native_is_an_alias() {
        assert_eq!(Backend::Cranelift.as_str(), "cranelift");
        assert_eq!(Backend::Cranelift.to_string(), "cranelift");
        assert_eq!(Backend::parse("cranelift"), Some(Backend::Cranelift));
        // The alias parses, but only through the CLI entry point, and it
        // reports itself so exactly one deprecation warning can be emitted.
        assert_eq!(Backend::parse(NATIVE_ALIAS), None);
        assert_eq!(
            Backend::parse_cli(NATIVE_ALIAS),
            Some((Backend::Cranelift, true))
        );
        assert_eq!(
            Backend::parse_cli("cranelift"),
            Some((Backend::Cranelift, false))
        );
        assert_eq!(Backend::parse_cli("llvm"), Some((Backend::Llvm, false)));
        assert_eq!(Backend::parse_cli("c"), Some((Backend::C, false)));
        assert_eq!(Backend::parse_cli("nonsense"), None);
    }

    #[test]
    fn the_alias_warning_names_both_spellings_once() {
        let warning = deprecated_alias_warning();
        assert!(warning.contains("--backend native"), "{warning}");
        assert!(warning.contains("--backend cranelift"), "{warning}");
        assert!(warning.starts_with("warning: "), "{warning}");
    }

    /// The help's "default:" clause must describe what the build in hand
    /// really does — a restricted build without a distribution stamp
    /// cannot claim the full capability-based policy.
    #[test]
    fn the_default_description_reflects_what_a_build_actually_contains() {
        assert_eq!(
            describe_default(None, ALL_BACKENDS),
            "LLVM when its packaged code generator is available; cranelift/c otherwise"
        );
        assert_eq!(
            describe_default(
                None,
                Availability {
                    llvm: true,
                    cranelift: true,
                    c: false
                }
            ),
            "LLVM when its packaged code generator is available; cranelift otherwise"
        );
        assert_eq!(
            describe_default(
                None,
                Availability {
                    llvm: true,
                    cranelift: false,
                    c: true
                }
            ),
            "LLVM when its packaged code generator is available; c otherwise"
        );
        assert_eq!(
            describe_default(
                None,
                Availability {
                    llvm: false,
                    cranelift: true,
                    c: true
                }
            ),
            "cranelift when this host is a supported native target; c otherwise"
        );
        assert_eq!(
            describe_default(None, C_ONLY),
            "c; the only backend in this build"
        );
        assert_eq!(
            describe_default(None, CRANELIFT_ONLY),
            "cranelift; the only backend in this build"
        );
        assert_eq!(
            describe_default(None, LLVM_ONLY),
            "llvm; the only backend in this build"
        );
        assert_eq!(
            describe_default(Some(Backend::Llvm), LLVM_ONLY),
            "llvm; this build includes only the llvm backend"
        );
        assert_eq!(
            describe_default(Some(Backend::Llvm), ALL_BACKENDS),
            "llvm; configured at build time"
        );
    }

    /// A single-backend build with no stamp still defaults to the backend
    /// it has — the description and the resolution must agree.
    #[test]
    fn an_unstamped_single_backend_build_describes_and_resolves_to_that_backend() {
        for (backend, available) in [
            (Backend::Llvm, LLVM_ONLY),
            (Backend::Cranelift, CRANELIFT_ONLY),
            (Backend::C, C_ONLY),
        ] {
            assert!(describe_default(None, available).starts_with(backend.as_str()));
            assert_eq!(
                resolve_backend_with(None, inputs(), None, available),
                backend
            );
        }
    }

    /// This build's own profile/default/feature combination must satisfy the shared
    /// contract `build.rs` enforces (the `const _: () = assert!(...)`
    /// above proves the same thing at compile time; this proves the
    /// runtime view agrees).
    #[test]
    fn this_builds_configuration_satisfies_the_distribution_contract() {
        let enabled: Vec<&str> = compiled_in().iter().map(|b| b.as_str()).collect();
        let validated = super::super::distribution_contract::validate_distribution_config(
            DISTRIBUTION_PROFILE_RAW,
            DEFAULT_BACKEND_RAW,
            DISTRIBUTION_BACKEND_RAW,
            &enabled,
        )
        .expect("this build's configuration must satisfy the distribution contract");
        assert_eq!(validated.profile.as_deref(), distribution_profile());
        assert_eq!(
            validated.default_backend.as_deref(),
            configured_default_backend().map(|backend| backend.as_str())
        );
        if distribution_profile() == Some("full") {
            assert_eq!(enabled.len(), 3, "the full profile has every backend");
        } else if is_distribution() {
            assert_eq!(
                enabled.len(),
                1,
                "a slim distribution has exactly one backend"
            );
        }
    }

    #[test]
    fn backend_resolution_covers_implicit_policy_and_explicit_overrides() {
        // Explicit selection always wins, whatever else is requested.
        assert_eq!(
            resolve_backend_with(Some(Backend::C), inputs(), None, ALL_BACKENDS),
            Backend::C
        );
        assert_eq!(
            resolve_backend_with(
                Some(Backend::Cranelift),
                SelectionInputs {
                    c_source_output: true,
                    llvm_ir_output: true,
                    c_cross_target: true,
                    ..inputs()
                },
                None,
                ALL_BACKENDS
            ),
            Backend::Cranelift
        );
        assert_eq!(
            resolve_backend_with(
                Some(Backend::Llvm),
                SelectionInputs {
                    c_source_output: true,
                    c_cross_target: true,
                    ..inputs()
                },
                None,
                ALL_BACKENDS
            ),
            Backend::Llvm
        );
        // Output format implies the only backend that can produce it.
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    c_source_output: true,
                    native_host_supported: true,
                    llvm_available: true,
                    ..inputs()
                },
                None,
                ALL_BACKENDS
            ),
            Backend::C
        );
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    c_cross_target: true,
                    native_host_supported: true,
                    llvm_available: true,
                    ..inputs()
                },
                None,
                ALL_BACKENDS
            ),
            Backend::C
        );
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    llvm_ir_output: true,
                    native_host_supported: true,
                    ..inputs()
                },
                None,
                ALL_BACKENDS
            ),
            Backend::Llvm
        );
        // `--native-target` selects the Cranelift object backend.
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    native_target_requested: true,
                    llvm_available: true,
                    ..inputs()
                },
                None,
                ALL_BACKENDS
            ),
            Backend::Cranelift
        );
        // Capability-based default: LLVM when it loaded, else Cranelift,
        // else C.
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    native_host_supported: true,
                    llvm_available: true,
                    ..inputs()
                },
                None,
                ALL_BACKENDS
            ),
            Backend::Llvm
        );
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    native_host_supported: true,
                    ..inputs()
                },
                None,
                ALL_BACKENDS
            ),
            Backend::Cranelift
        );
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    llvm_available: true,
                    ..inputs()
                },
                None,
                ALL_BACKENDS
            ),
            Backend::C
        );
        assert_eq!(
            resolve_backend_with(None, inputs(), None, ALL_BACKENDS),
            Backend::C
        );
    }

    #[test]
    fn a_configured_build_defaults_deterministically() {
        for (configured_default, available) in [
            (Backend::Llvm, LLVM_ONLY),
            (Backend::Cranelift, CRANELIFT_ONLY),
            (Backend::C, C_ONLY),
        ] {
            // Regardless of what the host can do, and regardless of whether
            // the packaged LLVM code generator loads.
            for probe in [
                inputs(),
                SelectionInputs {
                    native_host_supported: true,
                    llvm_available: true,
                    ..inputs()
                },
                SelectionInputs {
                    native_host_supported: true,
                    native_target_requested: true,
                    ..inputs()
                },
            ] {
                assert_eq!(
                    resolve_backend_with(None, probe, Some(configured_default), available),
                    configured_default,
                    "the configured default must be deterministic"
                );
            }
            // An explicit request still reaches the availability check
            // rather than being silently rewritten to the distribution's
            // own backend.
            for requested in ALL {
                assert_eq!(
                    resolve_backend_with(
                        Some(requested),
                        inputs(),
                        Some(configured_default),
                        available
                    ),
                    requested
                );
            }
        }
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    native_host_supported: true,
                    ..inputs()
                },
                Some(Backend::Llvm),
                ALL_BACKENDS
            ),
            Backend::Llvm,
            "a full build may fix a default while retaining every backend"
        );
    }

    #[test]
    fn output_format_flags_outrank_the_distribution_default() {
        // An `--emit-c` in an LLVM distribution must resolve to the C
        // backend and then fail with "not included in this build", rather
        // than quietly emitting something else.
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    c_source_output: true,
                    ..inputs()
                },
                Some(Backend::Llvm),
                LLVM_ONLY
            ),
            Backend::C
        );
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    llvm_ir_output: true,
                    ..inputs()
                },
                Some(Backend::Cranelift),
                CRANELIFT_ONLY
            ),
            Backend::Llvm
        );
    }

    #[test]
    fn implicit_selection_never_picks_a_backend_this_build_lacks() {
        // A feature-restricted build with no distribution stamp still must
        // not default to something that was compiled out.
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    native_host_supported: true,
                    ..inputs()
                },
                None,
                LLVM_ONLY
            ),
            Backend::Llvm
        );
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    native_host_supported: true,
                    llvm_available: true,
                    ..inputs()
                },
                None,
                CRANELIFT_ONLY
            ),
            Backend::Cranelift
        );
        assert_eq!(
            resolve_backend_with(
                None,
                SelectionInputs {
                    native_target_requested: true,
                    ..inputs()
                },
                None,
                LLVM_ONLY
            ),
            Backend::Llvm
        );
        // Nothing in the policy applies (unsupported host, no probe): the
        // only compiled-in backend is still the answer.
        assert_eq!(
            resolve_backend_with(None, inputs(), None, LLVM_ONLY),
            Backend::Llvm
        );
        assert_eq!(
            resolve_backend_with(None, inputs(), None, CRANELIFT_ONLY),
            Backend::Cranelift
        );
    }

    #[test]
    fn the_unavailable_error_names_the_build_and_how_to_find_the_backend() {
        let message = unavailable_error(Backend::Cranelift);
        assert!(
            message.contains("cranelift backend is not included"),
            "{message}"
        );
        assert!(message.contains("'-full' or '-cranelift'"), "{message}");
        assert!(message.contains(&compiled_in_list()), "{message}");
        assert!(message.contains("-full"), "{message}");
        // Every archive format a release can publish is named, so the
        // message is correct whichever platform the user is on...
        for format in [".zip", ".tar.xz", ".tar.gz", ".msi"] {
            assert!(message.contains(format), "{format} missing from: {message}");
        }
        // ...and it must never promise that *this* platform publishes the
        // requested backend (macOS ships only the C package today).
        assert!(
            message.contains("not every platform publishes every profile"),
            "{message}"
        );
        assert_eq!(Backend::Llvm.artifact_suffix(), "-llvm");
        assert_eq!(Backend::C.artifact_suffix(), "-c");
    }

    /// The C-only macOS package is the case that made the previous wording
    /// wrong: asking it for LLVM must not read as "download the macOS LLVM
    /// package", because there isn't one.
    #[test]
    fn the_unavailable_error_never_promises_a_package_for_this_platform() {
        let message = unavailable_error(Backend::Llvm);
        assert!(
            message.contains("for a platform that publishes that profile"),
            "{message}"
        );
        assert!(
            !message.contains("install the backend-specific package"),
            "{message}"
        );
        assert!(
            !message.contains("macos") && !message.contains("this platform's"),
            "{message}"
        );
    }

    /// An all-backend build can be either an ordinary developer build or the
    /// packaged full profile.
    #[cfg(all(
        feature = "backend-c",
        feature = "backend-cranelift",
        feature = "backend-llvm"
    ))]
    #[test]
    fn an_all_backend_build_reports_its_profile_and_default() {
        assert_eq!(Availability::COMPILED_IN, ALL_BACKENDS);
        assert_eq!(compiled_in_list(), "llvm, cranelift, c");
        assert_eq!(cli_choices(), "llvm|cranelift|c");

        let metadata = version_metadata();
        assert!(
            metadata.contains("backends: llvm, cranelift, c"),
            "{metadata}"
        );
        assert!(metadata.contains("toolchain-free: no"), "{metadata}");
        if is_distribution() {
            assert_eq!(distribution_profile(), Some("full"));
            assert!(configured_default_backend().is_some());
            assert!(metadata.contains("distribution: full"), "{metadata}");
            assert!(
                metadata.contains(&format!(
                    "default-backend: {}",
                    configured_default_backend().unwrap()
                )),
                "{metadata}"
            );
        } else {
            assert_eq!(distribution_profile(), None);
            assert!(metadata.contains("default-backend: auto"), "{metadata}");
            assert!(metadata.contains("distribution: none"), "{metadata}");
        }
    }

    /// A build with a restricted feature set must report itself honestly,
    /// whether or not it carries a distribution stamp.
    #[cfg(not(all(
        feature = "backend-c",
        feature = "backend-cranelift",
        feature = "backend-llvm"
    )))]
    #[test]
    fn a_restricted_build_reports_only_the_backends_it_has() {
        let available = compiled_in();
        assert!(!available.is_empty());
        assert!(available.len() < 3);

        let metadata = version_metadata();
        let line = |key: &str| {
            metadata
                .lines()
                .find(|line| line.starts_with(key))
                .unwrap_or_else(|| panic!("metadata is missing '{key}': {metadata}"))
                .to_string()
        };
        // Whole lines, not substrings: "default-backend: c" is a prefix of
        // "default-backend: cranelift".
        assert_eq!(
            line("backends: "),
            format!("backends: {}", compiled_in_list())
        );
        assert_eq!(
            line("default-backend: "),
            format!(
                "default-backend: {}",
                default_backend_label(configured_default_backend(), Availability::COMPILED_IN)
            )
        );
        for backend in ALL {
            if !backend.is_compiled_in() {
                assert_ne!(
                    line("default-backend: "),
                    format!("default-backend: {backend}")
                );
            }
        }
        assert_eq!(
            line("toolchain-free: "),
            if Backend::C.is_compiled_in() {
                "toolchain-free: no"
            } else {
                "toolchain-free: yes"
            }
        );
    }

    /// `default-backend:` is only `auto` when the choice really is made at
    /// run time; a build that can only ever pick one names it.
    #[test]
    fn the_default_backend_label_is_concrete_whenever_the_build_decides_it() {
        assert_eq!(default_backend_label(None, ALL_BACKENDS), "auto");
        assert_eq!(
            default_backend_label(
                None,
                Availability {
                    llvm: true,
                    cranelift: true,
                    c: false
                }
            ),
            "auto"
        );
        assert_eq!(default_backend_label(None, LLVM_ONLY), "llvm");
        assert_eq!(default_backend_label(None, CRANELIFT_ONLY), "cranelift");
        assert_eq!(default_backend_label(None, C_ONLY), "c");
        assert_eq!(
            default_backend_label(Some(Backend::Cranelift), CRANELIFT_ONLY),
            "cranelift"
        );
    }
}
