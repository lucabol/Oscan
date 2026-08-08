// The packaged-distribution contract.
//
// A distribution has two independent facts:
//
// * OSCAN_DISTRIBUTION_PROFILE says whether this is a packaged build and
//   which package profile it belongs to (`full`, `llvm`, `cranelift`, or `c`);
// * OSCAN_DEFAULT_BACKEND says which compiled-in backend an invocation uses
//   when no CLI/output flag selects one.
//
// OSCAN_DISTRIBUTION_BACKEND remains a compatibility input for existing
// single-backend build scripts. It is normalized to the equivalent profile
// and default. Two places share this dependency-free file:
//
// * build.rs validates and exports the normalized values at build time; and
// * crate::backend::select reads those exported values and unit-tests the
//   selection contract.
//
// Because build.rs includes this file verbatim, nothing here may refer to the
// crate, Cargo features, or any dependency.

/// Every backend name, in canonical order.
pub const DISTRIBUTION_BACKEND_NAMES: [&str; 3] = ["llvm", "cranelift", "c"];

/// Every packaged profile, in canonical order.
pub const DISTRIBUTION_PROFILE_NAMES: [&str; 4] = ["full", "llvm", "cranelift", "c"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DistributionConfig {
    pub profile: Option<String>,
    pub default_backend: Option<String>,
}

/// Normalize an environment value. `None` means unset; surrounding whitespace
/// and letter case are not meaningful.
pub fn normalize_distribution_stamp(raw: &str) -> Option<String> {
    let trimmed = raw.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn validate_backend_name(value: &str, variable: &str) -> Result<(), String> {
    if DISTRIBUTION_BACKEND_NAMES.contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "{variable}='{value}' is not a valid backend name (expected one of: {})",
            DISTRIBUTION_BACKEND_NAMES.join(", ")
        ))
    }
}

/// Validate and normalize the complete distribution configuration.
///
/// An empty configuration is an ordinary development build. A default backend
/// without a profile is also allowed for deterministic custom/development
/// builds, but it does not enable strict packaged lookup. Packaged profiles
/// always require a default:
///
/// * `full` enables all three backends and defaults to LLVM;
/// * `llvm`, `cranelift`, and `c` enable exactly the named backend.
///
/// The legacy `OSCAN_DISTRIBUTION_BACKEND` input is accepted only for a
/// matching single-backend build and must agree with any new variables.
#[allow(dead_code)] // build.rs is the primary caller; the crate unit-tests it.
pub fn validate_distribution_config(
    profile_raw: &str,
    default_raw: &str,
    legacy_backend_raw: &str,
    enabled: &[&str],
) -> Result<DistributionConfig, String> {
    if enabled.is_empty() {
        return Err(
            "no backend feature is enabled: build with at least one of backend-llvm, \
             backend-cranelift, backend-c (the default feature set enables all three)"
                .to_string(),
        );
    }

    let mut profile = normalize_distribution_stamp(profile_raw);
    let mut default_backend = normalize_distribution_stamp(default_raw);
    let legacy_backend = normalize_distribution_stamp(legacy_backend_raw);

    if let Some(legacy) = legacy_backend.as_deref() {
        validate_backend_name(legacy, "OSCAN_DISTRIBUTION_BACKEND")?;
        if enabled != [legacy] {
            return Err(format!(
                "OSCAN_DISTRIBUTION_BACKEND='{legacy}' marks a single-backend distribution \
                 build, but this build enables {} backends ({}); build it with \
                 --no-default-features --features backend-{legacy}",
                enabled.len(),
                enabled.join(", ")
            ));
        }
        if let Some(configured) = profile.as_deref() {
            if configured != legacy {
                return Err(format!(
                    "OSCAN_DISTRIBUTION_PROFILE='{configured}' conflicts with legacy \
                     OSCAN_DISTRIBUTION_BACKEND='{legacy}'"
                ));
            }
        } else {
            profile = Some(legacy.to_string());
        }
        if let Some(configured) = default_backend.as_deref() {
            if configured != legacy {
                return Err(format!(
                    "OSCAN_DEFAULT_BACKEND='{configured}' conflicts with legacy \
                     OSCAN_DISTRIBUTION_BACKEND='{legacy}'"
                ));
            }
        } else {
            default_backend = Some(legacy.to_string());
        }
    }

    if let Some(default) = default_backend.as_deref() {
        validate_backend_name(default, "OSCAN_DEFAULT_BACKEND")?;
        if !enabled.contains(&default) {
            return Err(format!(
                "OSCAN_DEFAULT_BACKEND='{default}' names a backend that is not enabled in this \
                 build (enabled: {})",
                enabled.join(", ")
            ));
        }
    }

    let Some(profile_name) = profile.as_deref() else {
        return Ok(DistributionConfig {
            profile: None,
            default_backend,
        });
    };

    if !DISTRIBUTION_PROFILE_NAMES.contains(&profile_name) {
        return Err(format!(
            "OSCAN_DISTRIBUTION_PROFILE='{profile_name}' is not a valid profile name (expected \
             one of: {})",
            DISTRIBUTION_PROFILE_NAMES.join(", ")
        ));
    }
    let default = default_backend.as_deref().ok_or_else(|| {
        format!("OSCAN_DISTRIBUTION_PROFILE='{profile_name}' requires OSCAN_DEFAULT_BACKEND")
    })?;

    if profile_name == "full" {
        if enabled != DISTRIBUTION_BACKEND_NAMES {
            return Err(format!(
                "OSCAN_DISTRIBUTION_PROFILE='full' must enable every backend in canonical order \
                 (expected: {}; enabled: {})",
                DISTRIBUTION_BACKEND_NAMES.join(", "),
                enabled.join(", ")
            ));
        }
        if default != "llvm" {
            return Err(format!(
                "OSCAN_DISTRIBUTION_PROFILE='full' must default to 'llvm', not '{default}'"
            ));
        }
    } else {
        if enabled != [profile_name] {
            return Err(format!(
                "OSCAN_DISTRIBUTION_PROFILE='{profile_name}' is a slim profile and must enable \
                 exactly backend-{profile_name} (enabled: {})",
                enabled.join(", ")
            ));
        }
        if default != profile_name {
            return Err(format!(
                "OSCAN_DISTRIBUTION_PROFILE='{profile_name}' must default to the same backend, \
                 not '{default}'"
            ));
        }
    }

    Ok(DistributionConfig {
        profile,
        default_backend,
    })
}

/// Backward-compatible validator for callers that only provide the legacy
/// single-backend stamp.
#[allow(dead_code)]
pub fn validate_distribution_stamp(raw: &str, enabled: &[&str]) -> Result<Option<String>, String> {
    validate_distribution_config("", "", raw, enabled).map(|config| config.profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [&str; 3] = ["llvm", "cranelift", "c"];

    #[test]
    fn an_unset_configuration_is_an_ordinary_development_build() {
        for raw in ["", "   ", "\t\n"] {
            assert_eq!(normalize_distribution_stamp(raw), None);
            assert_eq!(
                validate_distribution_config(raw, "", "", &ALL),
                Ok(DistributionConfig {
                    profile: None,
                    default_backend: None
                })
            );
        }
    }

    #[test]
    fn the_legacy_stamp_still_defines_a_slim_distribution() {
        for name in DISTRIBUTION_BACKEND_NAMES {
            assert_eq!(
                validate_distribution_config("", "", name, &[name]),
                Ok(DistributionConfig {
                    profile: Some(name.to_string()),
                    default_backend: Some(name.to_string())
                })
            );
            assert_eq!(
                validate_distribution_stamp(name, &[name]),
                Ok(Some(name.to_string()))
            );
        }
    }

    #[test]
    fn a_full_distribution_contains_every_backend_and_has_a_fixed_default() {
        assert_eq!(
            validate_distribution_config(" FULL ", " LLVM ", "", &ALL),
            Ok(DistributionConfig {
                profile: Some("full".to_string()),
                default_backend: Some("llvm".to_string())
            })
        );
    }

    #[test]
    fn a_full_profile_rejects_a_partial_backend_set() {
        let err = validate_distribution_config("full", "llvm", "", &["llvm", "cranelift"])
            .expect_err("full must mean every backend");
        assert!(err.contains("must enable every backend"), "{err}");
    }

    #[test]
    fn a_full_profile_rejects_a_non_llvm_default() {
        let err = validate_distribution_config("full", "cranelift", "", &ALL)
            .expect_err("full has one stable cross-package default");
        assert!(err.contains("must default to 'llvm'"), "{err}");
    }

    #[test]
    fn a_slim_profile_rejects_extra_backends() {
        let err = validate_distribution_config("llvm", "llvm", "", &["llvm", "cranelift", "c"])
            .expect_err("a slim profile must stay slim");
        assert!(err.contains("must enable exactly backend-llvm"), "{err}");
    }

    #[test]
    fn a_profile_requires_an_enabled_default() {
        let err = validate_distribution_config("full", "", "", &ALL)
            .expect_err("packaged profiles need deterministic defaults");
        assert!(err.contains("requires OSCAN_DEFAULT_BACKEND"), "{err}");

        let err = validate_distribution_config("full", "swift", "", &ALL)
            .expect_err("unknown defaults must be rejected");
        assert!(err.contains("not a valid backend name"), "{err}");

        let err = validate_distribution_config("", "llvm", "", &["cranelift"])
            .expect_err("a default must be compiled in");
        assert!(err.contains("not enabled"), "{err}");
    }

    #[test]
    fn a_custom_unpacked_build_may_have_a_deterministic_default() {
        assert_eq!(
            validate_distribution_config("", "cranelift", "", &ALL),
            Ok(DistributionConfig {
                profile: None,
                default_backend: Some("cranelift".to_string())
            })
        );
    }

    #[test]
    fn legacy_and_new_variables_must_agree() {
        let err = validate_distribution_config("c", "c", "llvm", &["llvm"])
            .expect_err("conflicting profile inputs must fail");
        assert!(err.contains("conflicts"), "{err}");
    }

    #[test]
    fn a_build_with_no_backend_is_always_rejected() {
        for values in [("", "", ""), ("full", "llvm", ""), ("", "", "llvm")] {
            let err = validate_distribution_config(values.0, values.1, values.2, &[])
                .expect_err("a backendless build is invalid");
            assert!(err.contains("no backend feature is enabled"), "{err}");
        }
    }
}
