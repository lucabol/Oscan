use object::Object;
use std::fs;
use std::path::Path;
use std::process::{self, Command};

fn oscan_binary_path() -> String {
    std::env::var("CARGO_BIN_EXE_oscan")
        .expect("CARGO_BIN_EXE_oscan should be set for integration tests")
}

#[test]
fn long_help_flag_prints_usage_and_succeeds() {
    let output = Command::new(oscan_binary_path())
        .arg("--help")
        .output()
        .expect("failed to run oscan --help");

    assert!(
        output.status.success(),
        "expected --help to exit successfully"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("usage: oscan"));
    assert!(stdout.contains("--target <arch>"));
    assert!(stdout.contains("--libc"));
    assert!(stdout.contains("including with LLVM/Cranelift"));
    assert!(stdout.contains("--allow-elevated-native-link"));
    assert!(stdout.contains("Trusted CI/release only"));
}

#[test]
fn short_help_flag_prints_usage_and_succeeds() {
    let output = Command::new(oscan_binary_path())
        .arg("-h")
        .output()
        .expect("failed to run oscan -h");

    assert!(output.status.success(), "expected -h to exit successfully");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("usage: oscan"));
    assert!(stdout.contains("--target <arch>"));
}

#[test]
fn help_mentions_extra_obj() {
    let output = Command::new(oscan_binary_path())
        .arg("--help")
        .output()
        .expect("failed to run oscan --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--extra-obj"));
}

#[test]
fn help_mentions_extra_lib() {
    let output = Command::new(oscan_binary_path())
        .arg("--help")
        .output()
        .expect("failed to run oscan --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--extra-lib"));
    assert!(stdout.contains("system library name"));
}

#[test]
fn elevated_native_link_opt_in_is_rejected_for_c_backend() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output = Command::new(oscan_binary_path())
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
    let output = Command::new(oscan_binary_path())
        .arg("--help")
        .output()
        .expect("failed to run oscan --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("default: LLVM when its packaged code generator is available"));
    assert!(
        stdout.contains("Direct LLVM object code through Oscan's own packaged LLVM code generator")
    );
    assert!(stdout.contains("no C, no toolchain"));
    assert!(stdout.contains("Portability/reference"));
    assert!(stdout.contains("Cranelift object code"));
    assert!(stdout.contains("--emit-c"));
    assert!(stdout.contains("--emit-llvm-ir"));
    assert!(stdout.contains("C-backend source"));
    assert!(stdout.contains("OSCAN_LLVM_LIB"));
    assert!(stdout.contains("OSCAN_LLVM_DIR"));
    // The C-mediated path is gone: nothing in the help may suggest the
    // LLVM backend needs Clang or an installed LLVM toolchain.
    assert!(!stdout.contains("OSCAN_LLVM_CLANG"));
    assert!(!stdout.contains("OSCAN_LLVM_TOOLCHAIN_DIR"));
}

#[test]
fn implicit_emit_c_matches_explicit_c_source_output() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let default_output = Command::new(oscan_binary_path())
        .arg(&source)
        .arg("--emit-c")
        .output()
        .expect("failed to run default backend");
    let explicit_c_output = Command::new(oscan_binary_path())
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
#[test]
fn default_backend_emits_an_object_on_supported_hosts() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output_path =
        std::env::temp_dir().join(format!("oscan-default-object-{}.obj", process::id()));
    let output = Command::new(oscan_binary_path())
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
#[test]
fn unavailable_implicit_llvm_falls_back_to_cranelift() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let missing_lib = std::env::temp_dir().join(format!("oscan-missing-llvm-{}", process::id()));
    let output_path =
        std::env::temp_dir().join(format!("oscan-llvm-fallback-{}.obj", process::id()));
    let _ = fs::remove_file(&output_path);
    let output = Command::new(oscan_binary_path())
        .arg(&source)
        .args(["--verbose", "-o"])
        .arg(&output_path)
        .env("OSCAN_LLVM_LIB", &missing_lib)
        .output()
        .expect("failed to run LLVM fallback validation");

    assert!(
        output.status.success(),
        "implicit LLVM fallback failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[verbose] native backend target:"));
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
    let output = Command::new(oscan_binary_path())
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
    assert!(!stderr.contains("[verbose] native backend target:"));
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
#[test]
fn elevated_native_link_opt_in_is_harmless_for_native_object_only_output() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output_path =
        std::env::temp_dir().join(format!("oscan-allow-elevated-object-{}.obj", process::id()));
    let output = Command::new(oscan_binary_path())
        .arg(&source)
        .args(["--backend", "native", "--allow-elevated-native-link", "-o"])
        .arg(&output_path)
        .output()
        .expect("failed to run native object-only validation");

    assert!(
        output.status.success(),
        "native object-only output with opt-in failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(&output_path).expect("native object output should exist");
    fs::remove_file(&output_path).expect("failed to remove native object output");
    let object = object::File::parse(bytes.as_slice()).expect("output should be an object file");
    assert_eq!(object.kind(), object::ObjectKind::Relocatable);
}

#[test]
fn c_output_extension_selects_the_c_backend_implicitly() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let implicit_path = std::env::temp_dir().join(format!("oscan-implicit-c-{}.c", process::id()));
    let explicit_path = std::env::temp_dir().join(format!("oscan-explicit-c-{}.c", process::id()));
    let implicit_output = Command::new(oscan_binary_path())
        .arg(&source)
        .arg("-o")
        .arg(&implicit_path)
        .output()
        .expect("failed to run implicit C backend");
    let explicit_output = Command::new(oscan_binary_path())
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

#[test]
fn native_backend_rejects_c_source_emission() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output = Command::new(oscan_binary_path())
        .arg(&source)
        .args(["--backend", "native", "--emit-c"])
        .output()
        .expect("failed to run native backend validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--emit-c requires the C portability/reference backend"));
    assert!(stderr.contains("native backend produces object code"));
}

#[test]
fn native_backend_rejects_a_c_output_extension() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output_path =
        std::env::temp_dir().join(format!("oscan-rejected-native-c-{}.c", process::id()));
    let output = Command::new(oscan_binary_path())
        .arg(&source)
        .args(["--backend", "native", "-o"])
        .arg(&output_path)
        .output()
        .expect("failed to run native backend validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("C source output (-o *.c) requires"));
    assert!(stderr.contains("native backend produces object code"));
    assert!(!output_path.exists());
}

#[test]
fn llvm_backend_rejects_c_source_emission() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output = Command::new(oscan_binary_path())
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
    for backend in ["c", "native"] {
        let output = Command::new(oscan_binary_path())
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

#[test]
fn llvm_ir_extension_rejects_run_mode() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output_path = std::env::temp_dir().join(format!("oscan-run-llvm-ir-{}.ll", process::id()));
    let _ = fs::remove_file(&output_path);
    let output = Command::new(oscan_binary_path())
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
fn llvm_provider_configured() -> bool {
    let configured = std::env::var_os("OSCAN_LLVM_LIB").is_some()
        || std::env::var_os("OSCAN_LLVM_DIR").is_some();
    assert!(
        configured || std::env::var_os("OSCAN_LLVM_TEST_REQUIRED").is_none(),
        "OSCAN_LLVM_TEST_REQUIRED is set, but OSCAN_LLVM_LIB/OSCAN_LLVM_DIR is not configured"
    );
    configured
}

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
    let output = Command::new(oscan_binary_path())
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

#[test]
fn llvm_ir_output_is_deterministic_and_direct() {
    if !llvm_provider_configured() {
        return;
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let emit = || {
        Command::new(oscan_binary_path())
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
    assert!(ir.contains("define void @oscan_main("), "{ir}");
    assert!(ir.contains("define i32 @main("), "{ir}");
    // Conservative poison policy.
    assert!(!ir.contains(" nsw "), "no nsw: {ir}");
    assert!(!ir.contains(" nuw "), "no nuw: {ir}");
    assert!(!ir.contains("inbounds"), "no inbounds: {ir}");
    assert!(!ir.contains("llvm.memcpy"), "no memcpy intrinsic: {ir}");
}

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
    let output = Command::new(oscan_binary_path())
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

    let output = Command::new(oscan_binary_path())
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
