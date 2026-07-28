// The distribution-stamp contract.
//
// `OSCAN_DISTRIBUTION_BACKEND=llvm|cranelift|c` marks a build as *the*
// packaged compiler for one backend. Two places have to agree on exactly
// what that means, so they share this one dependency-free file:
//
// * `build.rs` `include!`s it and enforces the contract at build time, so
//   a mismatched pair fails the build with a named reason instead of
//   producing a compiler that cannot run its own default backend; and
// * the compiler itself (`crate::backend::select`) compiles it as a normal
//   module, reads the stamp through it, and unit-tests the rules below.
//
// Because `build.rs` includes this file verbatim, nothing here may refer
// to the crate, to cargo features, or to any dependency.

/// Every backend name a stamp may carry, in canonical order.
pub const DISTRIBUTION_BACKEND_NAMES: [&str; 3] = ["llvm", "cranelift", "c"];

/// Normalize a raw stamp value. `None` means "unset": an ordinary
/// development build with no forced default. Surrounding whitespace and
/// letter case are not meaningful.
pub fn normalize_distribution_stamp(raw: &str) -> Option<String> {
    let trimmed = raw.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Validate a raw stamp against the backends this build actually enables.
///
/// * `Ok(None)` — no stamp: an ordinary development build.
/// * `Ok(Some(name))` — a valid single-backend distribution build.
/// * `Err(message)` — an actionable reason the pair is not a distribution
///   build at all.
///
/// A distribution build must enable **exactly one** backend, and it must
/// be the stamped one. "At least one" is not enough: a stamp that merely
/// picks a default out of several compiled-in backends would produce an
/// artifact whose name promises a single-backend package while still
/// carrying the others, which is precisely the confusion the stamp exists
/// to prevent.
#[allow(dead_code)] // `build.rs` is the primary caller; the crate unit-tests it.
pub fn validate_distribution_stamp(raw: &str, enabled: &[&str]) -> Result<Option<String>, String> {
    if enabled.is_empty() {
        return Err(
            "no backend feature is enabled: build with at least one of backend-llvm, \
             backend-cranelift, backend-c (the default feature set enables all three)"
                .to_string(),
        );
    }
    let requested = match normalize_distribution_stamp(raw) {
        None => return Ok(None),
        Some(requested) => requested,
    };
    if !DISTRIBUTION_BACKEND_NAMES.contains(&requested.as_str()) {
        return Err(format!(
            "OSCAN_DISTRIBUTION_BACKEND='{requested}' is not a valid backend name (expected one \
             of: {})",
            DISTRIBUTION_BACKEND_NAMES.join(", ")
        ));
    }
    if !enabled.contains(&requested.as_str()) {
        return Err(format!(
            "OSCAN_DISTRIBUTION_BACKEND='{requested}' names a backend that is not enabled in \
             this build (enabled: {}); build it with --no-default-features --features \
             backend-{requested}",
            enabled.join(", ")
        ));
    }
    if enabled.len() != 1 {
        return Err(format!(
            "OSCAN_DISTRIBUTION_BACKEND='{requested}' marks a single-backend distribution build, \
             but this build enables {} backends ({}); a distribution build must enable exactly \
             one: build it with --no-default-features --features backend-{requested}",
            enabled.len(),
            enabled.join(", ")
        ));
    }
    Ok(Some(requested))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [&str; 3] = ["llvm", "cranelift", "c"];

    #[test]
    fn an_unset_stamp_is_an_ordinary_development_build() {
        for raw in ["", "   ", "\t\n"] {
            assert_eq!(normalize_distribution_stamp(raw), None);
            assert_eq!(validate_distribution_stamp(raw, &ALL), Ok(None));
            assert_eq!(validate_distribution_stamp(raw, &["llvm"]), Ok(None));
        }
    }

    #[test]
    fn a_stamp_is_normalized_before_it_is_matched() {
        assert_eq!(
            normalize_distribution_stamp("  Cranelift \n"),
            Some("cranelift".to_string())
        );
        assert_eq!(
            validate_distribution_stamp(" LLVM ", &["llvm"]),
            Ok(Some("llvm".to_string()))
        );
    }

    #[test]
    fn exactly_one_enabled_backend_matching_the_stamp_is_a_distribution_build() {
        for name in DISTRIBUTION_BACKEND_NAMES {
            assert_eq!(
                validate_distribution_stamp(name, &[name]),
                Ok(Some(name.to_string()))
            );
        }
    }

    /// The regression this rule exists for: stamping a default-feature
    /// build (every backend enabled) must fail, not quietly produce a
    /// "distribution" that still contains every backend.
    #[test]
    fn a_stamped_all_features_build_is_rejected() {
        let err = validate_distribution_stamp("llvm", &ALL)
            .expect_err("an all-features build may not be stamped");
        assert!(err.contains("exactly one"), "{err}");
        assert!(err.contains("llvm, cranelift, c"), "{err}");
        assert!(
            err.contains("--no-default-features --features backend-llvm"),
            "{err}"
        );
    }

    #[test]
    fn a_stamp_with_more_than_one_enabled_backend_is_rejected_even_in_pairs() {
        let err = validate_distribution_stamp("cranelift", &["cranelift", "c"])
            .expect_err("two enabled backends may not be stamped");
        assert!(err.contains("exactly one"), "{err}");
        assert!(err.contains("2 backends"), "{err}");
    }

    #[test]
    fn a_stamp_naming_a_disabled_backend_is_rejected() {
        let err = validate_distribution_stamp("llvm", &["cranelift"])
            .expect_err("a stamp must name an enabled backend");
        assert!(err.contains("not enabled in this build"), "{err}");
        assert!(err.contains("enabled: cranelift"), "{err}");
        assert!(
            err.contains("--no-default-features --features backend-llvm"),
            "{err}"
        );
    }

    #[test]
    fn an_unknown_stamp_is_rejected_before_anything_else() {
        let err = validate_distribution_stamp("cranelifty", &ALL)
            .expect_err("an unknown backend name must be rejected");
        assert!(err.contains("is not a valid backend name"), "{err}");
        assert!(err.contains("llvm, cranelift, c"), "{err}");
    }

    #[test]
    fn a_build_with_no_backend_at_all_is_rejected_stamped_or_not() {
        for raw in ["", "llvm"] {
            let err = validate_distribution_stamp(raw, &[])
                .expect_err("a build with no backend feature is never valid");
            assert!(err.contains("no backend feature is enabled"), "{err}");
        }
    }
}
