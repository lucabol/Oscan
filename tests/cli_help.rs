//! CLI-surface coverage.
//!
//! These tests run against whatever backends the package was built with,
//! and against the distribution profile/default it was built under: cargo sets the
//! package's own feature cfgs for integration-test targets too, and
//! `build.rs` re-exports the normalized configuration
//! for every target in the package. So `cfg!(feature = "backend-c")` and
//! [`distribution_profile`] here describe exactly the `oscan` binary under
//! test. Tests that need a specific backend are gated on it; the rest
//! derive their expectations from [`compiled_in_backends`] /
//! [`distribution_profile`], so
//! `cargo test --no-default-features --features backend-<name>` — stamped
//! or not — is a first-class configuration rather than a build that merely
//! compiles.

#[cfg(any(feature = "backend-llvm", feature = "backend-cranelift"))]
use object::{Object, ObjectSymbol};
use std::fs;
use std::path::Path;
use std::process::{self, Command};

fn oscan_binary_path() -> String {
    std::env::var("CARGO_BIN_EXE_oscan")
        .expect("CARGO_BIN_EXE_oscan should be set for integration tests")
}

/// Every environment variable that steers discovery of the LLVM provider,
/// the native linker, the runtime archive, or the strict no-toolchain
/// profile. Child processes start without all of them, so an ambient
/// developer/CI setting can neither mask nor fake what these tests assert;
/// a test that needs one sets it back deliberately.
const DISCOVERY_ENV: [&str; 10] = [
    "OSCAN_LLVM_LIB",
    "OSCAN_LLVM_DIR",
    "OSCAN_TOOLCHAIN_DIR",
    "OSCAN_CC",
    "OSCAN_NATIVE_LINKER",
    "OSCAN_NATIVE_LINKER_FLAVOR",
    "OSCAN_NATIVE_ASSET_CACHE_DIR",
    "OSCAN_RUNTIME_ARCHIVE_DIR",
    "OSCAN_RUNTIME_BUILDER",
    "OSCAN_NO_TOOLCHAIN",
];

/// `oscan` with a scrubbed environment.
fn oscan_command() -> Command {
    let mut command = Command::new(oscan_binary_path());
    for name in DISCOVERY_ENV {
        command.env_remove(name);
    }
    command
}

/// `oscan` with a scrubbed environment plus this run's deliberately
/// configured packaged LLVM provider (see [`llvm_provider_configured`]),
/// for the tests that need the code generator to actually load.
#[cfg(feature = "backend-llvm")]
fn oscan_provider_command() -> Command {
    let mut command = oscan_command();
    for name in ["OSCAN_LLVM_LIB", "OSCAN_LLVM_DIR"] {
        if let Ok(value) = std::env::var(name) {
            command.env(name, value);
        }
    }
    command
}

/// The backends this build of `oscan` contains, in canonical order.
fn compiled_in_backends() -> Vec<&'static str> {
    let mut backends = Vec::new();
    if cfg!(feature = "backend-llvm") {
        backends.push("llvm");
    }
    if cfg!(feature = "backend-cranelift") {
        backends.push("cranelift");
    }
    if cfg!(feature = "backend-c") {
        backends.push("c");
    }
    backends
}

/// The packaged profile and deterministic default stamped by `build.rs`.
fn distribution_profile() -> Option<&'static str> {
    let raw = option_env!("OSCAN_DISTRIBUTION_PROFILE")?.trim();
    let profile = ["full", "llvm", "cranelift", "c"]
        .into_iter()
        .find(|name| raw.eq_ignore_ascii_case(name));
    assert!(
        raw.is_empty() || profile.is_some(),
        "OSCAN_DISTRIBUTION_PROFILE={raw:?} is not a valid profile name"
    );
    if let Some(profile) = profile {
        let expected = if profile == "full" {
            vec!["llvm", "cranelift", "c"]
        } else {
            vec![profile]
        };
        assert_eq!(
            compiled_in_backends(),
            expected,
            "a packaged build must contain exactly the backends promised by its profile"
        );
    }
    profile
}

fn configured_default_backend() -> Option<&'static str> {
    let raw = option_env!("OSCAN_DEFAULT_BACKEND")?.trim();
    let default = ["llvm", "cranelift", "c"]
        .into_iter()
        .find(|name| raw.eq_ignore_ascii_case(name));
    assert!(
        raw.is_empty() || default.is_some(),
        "OSCAN_DEFAULT_BACKEND={raw:?} is not a valid backend name"
    );
    if let Some(default) = default {
        assert!(
            compiled_in_backends().contains(&default),
            "the configured default must be compiled in"
        );
    }
    default
}

/// The backend an implicit (no `--backend`) invocation resolves to, when
/// that is decided at build time rather than by probing this host.
fn build_time_default_backend() -> Option<&'static str> {
    match (
        configured_default_backend(),
        compiled_in_backends().as_slice(),
    ) {
        (Some(configured), _) => Some(configured),
        (None, [only]) => Some(only),
        _ => None,
    }
}

/// A build without the C backend is intrinsically strict (see
/// `backend::no_toolchain`), so every C-toolchain flag is refused.
fn toolchain_free_build() -> bool {
    !cfg!(feature = "backend-c")
}

fn help_output() -> String {
    let output = oscan_command()
        .arg("--help")
        .output()
        .expect("failed to run oscan --help");
    assert!(output.status.success(), "expected --help to succeed");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn long_help_flag_prints_usage_and_succeeds() {
    let stdout = help_output();
    assert!(stdout.contains("usage: oscan"));
    assert!(stdout.contains("--extra-obj"));
    assert!(stdout.contains("--debuginfo <level>"));
    assert!(stdout.contains("none or line-tables (default: none)"));
    if cfg!(feature = "backend-c") {
        assert!(stdout.contains("--target <arch>"));
    }
    if !toolchain_free_build() {
        assert!(stdout.contains("--libc"));
        if cfg!(any(feature = "backend-llvm", feature = "backend-cranelift")) {
            assert!(stdout.contains("including with LLVM/Cranelift"));
        }
    }
    if cfg!(any(feature = "backend-llvm", feature = "backend-cranelift")) {
        assert!(stdout.contains("--allow-elevated-native-link"));
        assert!(stdout.contains("Trusted CI/release only"));
        assert!(stdout.contains("--opt-level size|speed"));
        assert!(stdout.contains("size (default) or speed"));
    }
}

/// A backend-specific build must not advertise flags it can only refuse:
/// the usage line and the option list are both built from what is
/// actually compiled in.
#[test]
fn help_only_advertises_flags_this_build_can_honor() {
    let stdout = help_output();
    let usage = stdout
        .lines()
        .next()
        .expect("help starts with a usage line");

    assert!(usage.contains(&format!("[--backend {}]", compiled_in_backends().join("|"))));
    assert_eq!(usage.contains("[--emit-c]"), cfg!(feature = "backend-c"));
    assert_eq!(
        usage.contains("[--target <arch>]"),
        cfg!(feature = "backend-c")
    );
    assert_eq!(
        usage.contains("[--emit-llvm-ir]"),
        cfg!(feature = "backend-llvm")
    );
    assert_eq!(usage.contains("[--libc]"), !toolchain_free_build());
    assert_eq!(
        usage.contains("[--extra-c <file.c>]"),
        !toolchain_free_build()
    );
    assert_eq!(
        usage.contains("[--opt-level size|speed]"),
        cfg!(any(feature = "backend-llvm", feature = "backend-cranelift"))
    );
    assert!(usage.contains("[--extra-obj <file.o|.obj>]"));

    if toolchain_free_build() {
        // The closing note deliberately *names* the refused routes; the
        // flag list itself must not offer them.
        let advertised: String = stdout
            .lines()
            .filter(|line| !line.trim_start().starts_with("(this build"))
            .collect::<Vec<_>>()
            .join("\n");
        for absent in [
            "--emit-c ",
            "--target <arch>",
            "--libc ",
            "--extra-cflags",
            "OSCAN_CC ",
            "OSCAN_TOOLCHAIN_DIR",
            "OSCAN_NO_TOOLCHAIN=1",
            // A build that refuses compiler-driver linking must not offer
            // it as a linker flavor, or describe it as a fallback.
            "'compiler-driver'",
            "otherwise a compiler driver",
        ] {
            assert!(
                !advertised.contains(absent),
                "a toolchain-free build must not advertise {absent}: {stdout}"
            );
        }

        assert!(
            stdout.contains("this build includes no C backend"),
            "{stdout}"
        );
    }
    if cfg!(any(feature = "backend-llvm", feature = "backend-cranelift")) {
        // Both kinds of build advertise the direct linker flavors; only a
        // build that still has a C toolchain advertises the legacy
        // compiler-driver one, and a toolchain-free build additionally
        // says an override must name a direct flavor explicitly.
        assert!(
            stdout.contains("'mingw' (direct ld.lld, Windows)"),
            "{stdout}"
        );
        assert!(stdout.contains("'elf' (direct GNU ld, Linux)"), "{stdout}");
        if toolchain_free_build() {
            assert!(
                stdout.contains("OSCAN_NATIVE_LINKER_FLAVOR=mingw|elf"),
                "{stdout}"
            );
            assert!(
                stdout.contains("this build never links through a C compiler driver"),
                "{stdout}"
            );
        } else {
            assert!(stdout.contains("'compiler-driver' (legacy)"), "{stdout}");
        }
    }
    if !cfg!(feature = "backend-llvm") {
        assert!(!stdout.contains("OSCAN_LLVM_LIB"), "{stdout}");
        assert!(!stdout.contains("--emit-llvm-ir"), "{stdout}");
    }
    if !cfg!(any(feature = "backend-llvm", feature = "backend-cranelift")) {
        assert!(!stdout.contains("--native-target"), "{stdout}");
        assert!(!stdout.contains("OSCAN_NATIVE_LINKER"), "{stdout}");
    }
}

#[test]
fn invalid_optimization_levels_are_rejected_before_compilation() {
    let missing = oscan_command()
        .arg("--opt-level")
        .output()
        .expect("failed to validate a missing optimization level");
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr)
        .contains("--opt-level requires an argument (size, speed)"));

    let unknown = oscan_command()
        .args(["--opt-level", "fast"])
        .output()
        .expect("failed to validate an unknown optimization level");
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr)
        .contains("unknown optimization level 'fast' (supported: size, speed)"));
}

#[cfg(feature = "backend-c")]
#[test]
fn c_backend_rejects_an_explicit_object_optimization_level() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output = oscan_command()
        .arg(&source)
        .args(["--backend", "c", "--opt-level", "speed", "--emit-c"])
        .output()
        .expect("failed to validate the C optimization-level rejection");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--opt-level is only supported with --backend llvm or --backend cranelift"));
}

#[test]
fn short_help_flag_prints_usage_and_succeeds() {
    let output = oscan_command()
        .arg("-h")
        .output()
        .expect("failed to run oscan -h");

    assert!(output.status.success(), "expected -h to exit successfully");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("usage: oscan"));
    assert!(stdout.contains(&format!("[--backend {}]", compiled_in_backends().join("|"))));
}

#[test]
fn help_mentions_extra_obj() {
    let output = oscan_command()
        .arg("--help")
        .output()
        .expect("failed to run oscan --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--extra-obj"));
}

#[test]
fn help_mentions_extra_lib() {
    let output = oscan_command()
        .arg("--help")
        .output()
        .expect("failed to run oscan --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--extra-lib"));
    assert!(stdout.contains("system library name"));
}

#[cfg(feature = "backend-c")]
#[test]
fn elevated_native_link_opt_in_is_rejected_for_c_backend() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output = oscan_command()
        .args(["--backend", "c", "--allow-elevated-native-link"])
        .arg(&source)
        .arg("--emit-c")
        .output()
        .expect("failed to run oscan validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-elevated-native-link is only meaningful"));
    assert!(stderr.contains("trusted CI/release inputs"));
}

#[test]
fn help_describes_backend_roles_and_default() {
    let stdout = help_output();
    if cfg!(feature = "backend-llvm") {
        assert!(stdout
            .contains("Direct LLVM object code through Oscan's own packaged LLVM code generator"));
        assert!(stdout.contains("no C, no toolchain"));
        assert!(stdout.contains("--emit-llvm-ir"));
        assert!(stdout.contains("OSCAN_LLVM_LIB"));
        assert!(stdout.contains("OSCAN_LLVM_DIR"));
    }
    if cfg!(feature = "backend-cranelift") {
        assert!(stdout.contains("Cranelift object code"));
    }
    if cfg!(feature = "backend-c") {
        assert!(stdout.contains("Portability/reference"));
        assert!(stdout.contains("--emit-c"));
        assert!(stdout.contains("C-backend source"));
    }
    // The "default:" clause must describe the build in hand, not the
    // all-backends policy.
    let expected_default = match (
        configured_default_backend(),
        compiled_in_backends().as_slice(),
    ) {
        (Some(configured), [_]) => {
            format!("default: {configured}; this build includes only the {configured} backend")
        }
        (Some(configured), _) => format!("default: {configured}; configured at build time"),
        (None, [only]) => format!("default: {only}; the only backend in this build"),
        (None, backends) if backends.contains(&"llvm") => {
            let rest: Vec<&str> = backends.iter().copied().filter(|b| *b != "llvm").collect();
            format!(
                "default: LLVM when its packaged code generator is available; {} otherwise",
                rest.join("/")
            )
        }
        _ => "default: cranelift when this host is a supported native target; c otherwise"
            .to_string(),
    };
    assert!(stdout.contains(&expected_default), "{stdout}");
    // The C-mediated path is gone: nothing in the help may suggest the
    // LLVM backend needs Clang or an installed LLVM toolchain.
    assert!(!stdout.contains("OSCAN_LLVM_CLANG"));
    assert!(!stdout.contains("OSCAN_LLVM_TOOLCHAIN_DIR"));
}

/// `cranelift` is the canonical spelling everywhere the compiler talks
/// about backends; `native` only survives as an alias, and the help says
/// exactly that rather than advertising it as a choice.
#[test]
fn help_advertises_the_canonical_backend_names() {
    let stdout = help_output();
    let choices = compiled_in_backends().join("|");
    assert!(
        stdout.contains(&format!("[--backend {choices}]")),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("  --backend {choices} ")),
        "{stdout}"
    );
    assert_eq!(
        stdout.contains("('native' is a deprecated alias for 'cranelift')"),
        cfg!(feature = "backend-cranelift"),
        "{stdout}"
    );
    // The old spelling must not be advertised as canonical anywhere.
    assert!(!stdout.contains("llvm|c|native"), "{stdout}");
    assert!(!stdout.contains("--backend native"), "{stdout}");
}

/// `--backend native` keeps working, warns exactly once, and resolves to
/// the Cranelift backend (proved by the diagnostic naming it).
#[cfg(feature = "backend-cranelift")]
#[test]
fn the_native_backend_alias_warns_once_and_selects_cranelift() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output = oscan_command()
        .arg(&source)
        .args(["--backend", "native", "--emit-c"])
        .output()
        .expect("failed to run backend alias validation");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("'--backend native' is deprecated").count(),
        1,
        "exactly one deprecation warning expected: {stderr}"
    );
    assert!(stderr.contains("use '--backend cranelift'"), "{stderr}");
    // Same rejection the canonical spelling produces, so the alias really
    // selected the Cranelift backend rather than something else.
    assert!(!output.status.success());
    assert!(
        stderr.contains("cranelift backend produces object code"),
        "{stderr}"
    );
}

#[cfg(feature = "backend-cranelift")]
#[test]
fn the_canonical_cranelift_spelling_is_not_deprecated() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output = oscan_command()
        .arg(&source)
        .args(["--backend", "cranelift", "--emit-c"])
        .output()
        .expect("failed to run canonical backend validation");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("deprecated"), "{stderr}");
    assert!(
        stderr.contains("cranelift backend produces object code"),
        "{stderr}"
    );
}

#[test]
fn an_unknown_backend_names_the_canonical_choices() {
    let output = oscan_command()
        .args(["--backend", "cranelifty"])
        .output()
        .expect("failed to run unknown-backend validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown backend 'cranelifty'"), "{stderr}");
    assert!(stderr.contains("llvm, cranelift, c"), "{stderr}");
    assert!(stderr.contains("deprecated alias"), "{stderr}");
}

/// Selecting a backend this build does not contain is a named error that
/// points at the package which does contain it.
#[test]
fn a_compiled_out_backend_is_refused_with_an_actionable_error() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    for backend in ["llvm", "cranelift", "c"] {
        if compiled_in_backends().contains(&backend) {
            continue;
        }
        let output = oscan_command()
            .arg(&source)
            .args(["--backend", backend])
            .output()
            .expect("failed to run compiled-out backend validation");

        assert!(!output.status.success(), "{backend} unexpectedly succeeded");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!(
                "the {backend} backend is not included in this compiler build"
            )),
            "{stderr}"
        );
        assert!(stderr.contains("ends in '-full'"), "{stderr}");
        assert!(stderr.contains(&format!("or '-{backend}'")), "{stderr}");
        assert!(
            stderr.contains(&format!(
                "this build includes: {}",
                compiled_in_backends().join(", ")
            )),
            "{stderr}"
        );
    }
}

/// `--version` carries the build's backend inventory so tests and release
/// smoke checks can assert what a packaged compiler contains and which
/// backend it defaults to — in every feature configuration.
#[test]
fn version_reports_available_and_default_backends() {
    let output = oscan_command()
        .arg("--version")
        .output()
        .expect("failed to run oscan --version");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    assert!(
        lines.next().unwrap_or_default().starts_with("oscan "),
        "{stdout}"
    );
    // Whole lines, not substrings: "default-backend: c" is a prefix of
    // "default-backend: cranelift".
    let line = |key: &str| {
        stdout
            .lines()
            .find(|line| line.starts_with(key))
            .unwrap_or_else(|| panic!("--version is missing '{key}': {stdout}"))
    };
    let backends = compiled_in_backends();
    assert_eq!(
        line("backends: "),
        format!("backends: {}", backends.join(", "))
    );
    let expected_default = build_time_default_backend()
        .map(str::to_string)
        .unwrap_or_else(|| "auto".to_string());
    assert_eq!(
        line("default-backend: "),
        format!("default-backend: {expected_default}")
    );
    assert_eq!(
        line("distribution: "),
        format!("distribution: {}", distribution_profile().unwrap_or("none"))
    );
    assert_eq!(
        line("toolchain-free: "),
        format!(
            "toolchain-free: {}",
            if toolchain_free_build() { "yes" } else { "no" }
        )
    );
}

#[cfg(feature = "backend-c")]
#[test]
fn implicit_emit_c_matches_explicit_c_source_output() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let default_output = oscan_command()
        .arg(&source)
        .arg("--emit-c")
        .output()
        .expect("failed to run default backend");
    let explicit_c_output = oscan_command()
        .arg(&source)
        .args(["--backend", "c", "--emit-c"])
        .output()
        .expect("failed to run explicit C backend");

    assert!(
        default_output.status.success(),
        "default backend failed: {}",
        String::from_utf8_lossy(&default_output.stderr)
    );
    assert!(
        explicit_c_output.status.success(),
        "explicit C backend failed: {}",
        String::from_utf8_lossy(&explicit_c_output.stderr)
    );
    assert_eq!(default_output.stdout, explicit_c_output.stdout);
}

#[cfg(feature = "backend-c")]
#[test]
fn c_line_tables_map_root_and_imported_sources_without_changing_the_default() {
    let dir = std::env::temp_dir().join(format!("oscan-c-debuginfo-{}", process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create source directory");
    let imported_path = dir.join("lib.osc");
    let root_path = dir.join("main.osc");
    fs::write(
        &imported_path,
        "fn! greet() {\n    println(\"from import\");\n}\n",
    )
    .expect("write imported source");
    fs::write(
        &root_path,
        "use \"lib.osc\"\nfn! main() {\n    greet();\n}\n",
    )
    .expect("write root source");

    let default = oscan_command()
        .arg(&root_path)
        .args(["--backend", "c", "--emit-c"])
        .output()
        .expect("emit default C");
    let explicit_none = oscan_command()
        .arg(&root_path)
        .args(["--backend", "c", "--debuginfo", "none", "--emit-c"])
        .output()
        .expect("emit explicit no-debug C");
    let debug = oscan_command()
        .arg(&root_path)
        .args(["--backend", "c", "--debuginfo", "line-tables", "--emit-c"])
        .output()
        .expect("emit debug C");
    assert!(
        default.status.success(),
        "{}",
        String::from_utf8_lossy(&default.stderr)
    );
    assert!(
        debug.status.success(),
        "{}",
        String::from_utf8_lossy(&debug.stderr)
    );
    assert!(
        explicit_none.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit_none.stderr)
    );
    assert_eq!(
        default.stdout, explicit_none.stdout,
        "the explicit default must not change generated C"
    );

    let default_c = String::from_utf8(default.stdout).expect("default C is UTF-8");
    let debug_c = String::from_utf8(debug.stdout).expect("debug C is UTF-8");
    assert!(!default_c.contains("#line "), "{default_c}");
    assert!(
        debug_c.contains("#line 1 \"<oscan-generated>\""),
        "compiler-generated C must not inherit the previous Oscan location\n{debug_c}"
    );
    for (path, line) in [
        (&imported_path, 1),
        (&imported_path, 2),
        (&root_path, 2),
        (&root_path, 3),
    ] {
        let path = path
            .canonicalize()
            .expect("canonical source")
            .to_string_lossy()
            .replace('\\', "/");
        let path = path.strip_prefix("//?/").unwrap_or(&path);
        let marker = format!("#line {line} \"{path}\"");
        assert!(debug_c.contains(&marker), "missing {marker:?}\n{debug_c}");
    }
    assert!(
        !debug_c.contains("program.c"),
        "source mappings must never point at the temporary C file\n{debug_c}"
    );
    fs::remove_dir_all(&dir).expect("remove source directory");
}

#[test]
fn rejects_unknown_debug_info_level() {
    let output = oscan_command()
        .args(["--debuginfo", "full"])
        .output()
        .expect("run oscan");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("unknown debug-info level 'full' (supported: none, line-tables)"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(
        target_os = "linux",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "riscv64"
        )
    )
))]
#[cfg(feature = "backend-cranelift")]
#[test]
fn default_backend_emits_an_object_on_supported_hosts() {
    // A configured LLVM default is intentionally not capability-probed. Its
    // object emission is covered by the provider-backed LLVM tests below.
    if configured_default_backend() == Some("llvm") {
        return;
    }

    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output_path =
        std::env::temp_dir().join(format!("oscan-default-object-{}.obj", process::id()));
    let output = oscan_command()
        .arg(&source)
        .arg("-o")
        .arg(&output_path)
        .output()
        .expect("failed to run implicit object backend");

    assert!(
        output.status.success(),
        "implicit object backend failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(&output_path).expect("object output should exist");
    fs::remove_file(&output_path).expect("failed to remove object output");
    let object = object::File::parse(bytes.as_slice()).expect("output should be an object file");
    assert_eq!(object.kind(), object::ObjectKind::Relocatable);
}

#[cfg(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(
        target_os = "linux",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "riscv64"
        )
    )
))]
#[cfg(feature = "backend-cranelift")]
#[test]
fn cranelift_speed_profile_makes_user_main_local_and_process_main_global() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output_path =
        std::env::temp_dir().join(format!("oscan-speed-linkage-{}.obj", process::id()));
    let output = oscan_command()
        .arg(&source)
        .args([
            "--backend",
            "cranelift",
            "--opt-level",
            "speed",
            "--verbose",
            "-o",
        ])
        .arg(&output_path)
        .output()
        .expect("failed to emit a Cranelift speed-profile object");

    assert!(
        output.status.success(),
        "Cranelift speed-profile emission failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("optimization: speed"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(&output_path).expect("object output should exist");
    fs::remove_file(&output_path).expect("failed to remove object output");
    let object = object::File::parse(bytes.as_slice()).expect("output should be an object file");
    let user_main = object
        .symbols()
        .find(|symbol| symbol.name().ok() == Some("oscan_main"))
        .expect("object must contain the reachable Oscan main function");
    let process_main = object
        .symbols()
        .find(|symbol| symbol.name().ok() == Some("main"))
        .expect("object must contain the process entry function");
    assert!(user_main.is_local(), "oscan_main must have local linkage");
    assert!(process_main.is_global(), "main must remain linker-visible");
}

#[cfg(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(
        target_os = "linux",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "riscv64"
        )
    )
))]
#[cfg(all(feature = "backend-llvm", feature = "backend-cranelift"))]
#[test]
fn unavailable_implicit_llvm_obeys_the_configured_default_or_falls_back() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let missing_lib = std::env::temp_dir().join(format!("oscan-missing-llvm-{}", process::id()));
    let output_path =
        std::env::temp_dir().join(format!("oscan-llvm-fallback-{}.obj", process::id()));
    let _ = fs::remove_file(&output_path);
    let output = oscan_command()
        .arg(&source)
        .args(["--verbose", "-o"])
        .arg(&output_path)
        .env("OSCAN_LLVM_LIB", &missing_lib)
        .output()
        .expect("failed to run LLVM fallback validation");

    let stderr = String::from_utf8_lossy(&output.stderr);
    if configured_default_backend() == Some("llvm") {
        assert!(!output.status.success(), "{stderr}");
        assert!(
            stderr.contains("the LLVM backend needs Oscan's packaged LLVM"),
            "{stderr}"
        );
        assert!(!output_path.exists());
        return;
    }

    assert!(
        output.status.success(),
        "implicit LLVM fallback failed: {stderr}"
    );
    assert!(stderr.contains("[verbose] cranelift backend target:"));
    let bytes = fs::read(&output_path).expect("Cranelift fallback object should exist");
    fs::remove_file(&output_path).expect("failed to remove fallback object");
    let object = object::File::parse(bytes.as_slice()).expect("output should be an object file");
    assert_eq!(object.kind(), object::ObjectKind::Relocatable);
}

#[cfg(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(
        target_os = "linux",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "riscv64"
        )
    )
))]
#[cfg(feature = "backend-llvm")]
#[cfg(not(feature = "static-llvm"))]
#[test]
fn unavailable_explicit_llvm_never_falls_back() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let missing_lib =
        std::env::temp_dir().join(format!("oscan-missing-explicit-llvm-{}", process::id()));
    let output_path =
        std::env::temp_dir().join(format!("oscan-explicit-llvm-failure-{}.obj", process::id()));
    let _ = fs::remove_file(&output_path);
    let output = oscan_command()
        .arg(&source)
        .args(["--backend", "llvm", "-o"])
        .arg(&output_path)
        .env("OSCAN_LLVM_LIB", &missing_lib)
        .output()
        .expect("failed to run explicit LLVM failure validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("packaged LLVM"),
        "unexpected diagnostic: {stderr}"
    );
    assert!(stderr.contains("OSCAN_LLVM_LIB"), "{stderr}");
    // No silent fallback to Cranelift or C, and no partial output.
    assert!(!stderr.contains("[verbose] cranelift backend target:"));
    assert!(!output_path.exists());
}

#[cfg(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(
        target_os = "linux",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "riscv64"
        )
    )
))]
#[cfg(feature = "backend-cranelift")]
#[test]
fn elevated_native_link_opt_in_is_harmless_for_cranelift_object_only_output() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output_path =
        std::env::temp_dir().join(format!("oscan-allow-elevated-object-{}.obj", process::id()));
    let output = oscan_command()
        .arg(&source)
        .args([
            "--backend",
            "cranelift",
            "--allow-elevated-native-link",
            "-o",
        ])
        .arg(&output_path)
        .output()
        .expect("failed to run cranelift object-only validation");

    assert!(
        output.status.success(),
        "cranelift object-only output with opt-in failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(&output_path).expect("cranelift object output should exist");
    fs::remove_file(&output_path).expect("failed to remove cranelift object output");
    let object = object::File::parse(bytes.as_slice()).expect("output should be an object file");
    assert_eq!(object.kind(), object::ObjectKind::Relocatable);
}

#[cfg(feature = "backend-c")]
#[test]
fn c_output_extension_selects_the_c_backend_implicitly() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let implicit_path = std::env::temp_dir().join(format!("oscan-implicit-c-{}.c", process::id()));
    let explicit_path = std::env::temp_dir().join(format!("oscan-explicit-c-{}.c", process::id()));
    let implicit_output = oscan_command()
        .arg(&source)
        .arg("-o")
        .arg(&implicit_path)
        .output()
        .expect("failed to run implicit C backend");
    let explicit_output = oscan_command()
        .arg(&source)
        .args(["--backend", "c", "-o"])
        .arg(&explicit_path)
        .output()
        .expect("failed to run explicit C backend");

    assert!(
        implicit_output.status.success(),
        "implicit C backend failed: {}",
        String::from_utf8_lossy(&implicit_output.stderr)
    );
    assert!(
        explicit_output.status.success(),
        "explicit C backend failed: {}",
        String::from_utf8_lossy(&explicit_output.stderr)
    );
    let implicit_c = fs::read(&implicit_path).expect("implicit C output should exist");
    let explicit_c = fs::read(&explicit_path).expect("explicit C output should exist");
    fs::remove_file(&implicit_path).expect("failed to remove implicit C output");
    fs::remove_file(&explicit_path).expect("failed to remove explicit C output");
    assert_eq!(implicit_c, explicit_c);
}

#[cfg(feature = "backend-cranelift")]
#[test]
fn cranelift_backend_rejects_c_source_emission() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output = oscan_command()
        .arg(&source)
        .args(["--backend", "cranelift", "--emit-c"])
        .output()
        .expect("failed to run cranelift backend validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--emit-c requires the C portability/reference backend"));
    assert!(stderr.contains("cranelift backend produces object code"));
}

#[cfg(feature = "backend-cranelift")]
#[test]
fn cranelift_backend_rejects_a_c_output_extension() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output_path =
        std::env::temp_dir().join(format!("oscan-rejected-native-c-{}.c", process::id()));
    let output = oscan_command()
        .arg(&source)
        .args(["--backend", "cranelift", "-o"])
        .arg(&output_path)
        .output()
        .expect("failed to run cranelift backend validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("C source output (-o *.c) requires"));
    assert!(stderr.contains("cranelift backend produces object code"));
    assert!(!output_path.exists());
}

#[cfg(feature = "backend-llvm")]
#[test]
fn llvm_backend_rejects_c_source_emission() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output = oscan_command()
        .arg(&source)
        .args(["--backend", "llvm", "--emit-c"])
        .output()
        .expect("failed to run LLVM backend validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--emit-c requires the C portability/reference backend"));
    assert!(stderr.contains("llvm backend produces object code"));
}

#[test]
fn non_llvm_backends_reject_llvm_ir_emission() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    for backend in compiled_in_backends()
        .into_iter()
        .filter(|backend| *backend != "llvm")
    {
        let output = oscan_command()
            .arg(&source)
            .args(["--backend", backend, "--emit-llvm-ir"])
            .output()
            .expect("failed to run LLVM IR validation");

        assert!(!output.status.success(), "{backend} unexpectedly succeeded");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("LLVM IR output requires --backend llvm"),
            "unexpected {backend} diagnostic: {stderr}"
        );
    }
}

#[cfg(feature = "backend-llvm")]
#[test]
fn llvm_ir_extension_rejects_run_mode() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output_path = std::env::temp_dir().join(format!("oscan-run-llvm-ir-{}.ll", process::id()));
    let _ = fs::remove_file(&output_path);
    let output = oscan_command()
        .arg(&source)
        .arg("--run")
        .arg("-o")
        .arg(&output_path)
        .output()
        .expect("failed to validate LLVM IR run conflict");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LLVM IR output cannot be combined with --run"));
    assert!(!output_path.exists());
}

/// Whether this machine has Oscan's packaged LLVM code generator
/// available (either staged next to the test binary or pointed at by
/// `OSCAN_LLVM_LIB`/`OSCAN_LLVM_DIR`). The direct LLVM tests below are
/// skipped without it; `unavailable_explicit_llvm_never_falls_back`
/// covers the missing-provider path itself.
#[cfg(feature = "backend-llvm")]
fn llvm_provider_configured() -> bool {
    let configured = std::env::var_os("OSCAN_LLVM_LIB").is_some()
        || std::env::var_os("OSCAN_LLVM_DIR").is_some();
    assert!(
        configured || std::env::var_os("OSCAN_LLVM_TEST_REQUIRED").is_none(),
        "OSCAN_LLVM_TEST_REQUIRED is set, but OSCAN_LLVM_LIB/OSCAN_LLVM_DIR is not configured"
    );
    configured
}

#[cfg(feature = "backend-llvm")]
#[test]
fn configured_llvm_provider_becomes_the_implicit_default() {
    if !llvm_provider_configured() {
        return;
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output_path =
        std::env::temp_dir().join(format!("oscan-default-llvm-{}.obj", process::id()));
    let output = oscan_provider_command()
        .arg(&source)
        .args(["--verbose", "-o"])
        .arg(&output_path)
        .output()
        .expect("failed to run configured LLVM default");

    assert!(
        output.status.success(),
        "configured LLVM default failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[verbose] llvm backend target:"));
    assert!(
        stderr.contains("[verbose] LLVM code generator:"),
        "{stderr}"
    );
    let bytes = fs::read(&output_path).expect("LLVM object output should exist");
    fs::remove_file(&output_path).expect("failed to remove LLVM object output");
    let object = object::File::parse(bytes.as_slice()).expect("output should be an object file");
    assert_eq!(object.kind(), object::ObjectKind::Relocatable);
}

#[cfg(feature = "backend-llvm")]
#[test]
fn llvm_ir_output_is_deterministic_and_direct() {
    if !llvm_provider_configured() {
        return;
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let emit = || {
        oscan_provider_command()
            .arg(&source)
            .args(["--backend", "llvm", "--emit-llvm-ir"])
            .output()
            .expect("failed to emit LLVM IR")
    };
    let first = emit();
    let second = emit();

    assert!(
        first.status.success(),
        "first LLVM emission failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "second LLVM emission failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(first.stdout, second.stdout);
    let ir = String::from_utf8_lossy(&first.stdout);
    assert!(ir.contains("target triple = "));
    assert!(ir.contains("target datalayout = "));
    // The IR is lowered directly from Oscan's own typed IR, never through
    // a generated C translation unit compiled by Clang.
    assert!(
        ir.contains("source_filename = \"oscan_program\""),
        "unexpected module identity: {ir}"
    );
    assert!(!ir.contains("program.c"), "no C source may appear: {ir}");
    assert!(!ir.contains(".c\""), "no C source may appear: {ir}");
    assert!(ir.contains("define internal void @oscan_main("), "{ir}");
    assert!(ir.contains("define i32 @main("), "{ir}");
    // Conservative poison policy.
    assert!(!ir.contains(" nsw "), "no nsw: {ir}");
    assert!(!ir.contains(" nuw "), "no nuw: {ir}");
    assert!(!ir.contains("inbounds"), "no inbounds: {ir}");
    assert!(!ir.contains("llvm.memcpy"), "no memcpy intrinsic: {ir}");
}

#[cfg(feature = "backend-llvm")]
#[test]
fn llvm_ir_extension_selects_llvm_when_provider_is_configured() {
    if !llvm_provider_configured() {
        return;
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output_path =
        std::env::temp_dir().join(format!("oscan-implicit-llvm-{}.ll", process::id()));
    let output = oscan_provider_command()
        .arg(&source)
        .arg("-o")
        .arg(&output_path)
        .output()
        .expect("failed to emit implicit LLVM IR");

    assert!(
        output.status.success(),
        "implicit LLVM IR emission failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ir = fs::read_to_string(&output_path).expect("LLVM IR output should exist");
    fs::remove_file(&output_path).expect("failed to remove LLVM IR output");
    assert!(ir.contains("target triple = "));
}

/// The LLVM backend must produce its object without writing a single
/// C/header file anywhere: no `.c`, `.h`, `.i`, `.ll`, `.bc`, or `.s`
/// artifact may survive an object-only compile, and the scratch
/// directory the old C-mediated path used must never be created.
#[cfg(feature = "backend-llvm")]
#[test]
fn llvm_object_emission_leaves_no_c_or_intermediate_artifacts() {
    if !llvm_provider_configured() {
        return;
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let work_dir = std::env::temp_dir().join(format!("oscan-llvm-artifacts-{}", process::id()));
    let _ = fs::remove_dir_all(&work_dir);
    fs::create_dir_all(&work_dir).expect("failed to create work directory");
    let output_path = work_dir.join("program.obj");

    let output = oscan_provider_command()
        .arg(&source)
        .args(["--backend", "llvm", "-o"])
        .arg(&output_path)
        .current_dir(&work_dir)
        .output()
        .expect("failed to run LLVM object emission");
    assert!(
        output.status.success(),
        "LLVM object emission failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let leftovers: Vec<String> = fs::read_dir(&work_dir)
        .expect("failed to read work directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "program.obj")
        .collect();
    let _ = fs::remove_dir_all(&work_dir);
    assert!(
        leftovers.is_empty(),
        "the direct LLVM backend must not create intermediate files: {leftovers:?}"
    );
}
