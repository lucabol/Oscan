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
    assert!(stdout.contains("including with --backend native"));
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
}

#[test]
fn help_describes_backend_roles_and_default() {
    let output = Command::new(oscan_binary_path())
        .arg("--help")
        .output()
        .expect("failed to run oscan --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("default: native on supported hosts; c otherwise"));
    assert!(stdout.contains("Portability/reference"));
    assert!(stdout.contains("Direct object code"));
    assert!(stdout.contains("--emit-c"));
    assert!(stdout.contains("C-backend source"));
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
fn default_backend_emits_a_native_object_on_supported_hosts() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("hello.osc");
    let output_path =
        std::env::temp_dir().join(format!("oscan-default-native-{}.obj", process::id()));
    let output = Command::new(oscan_binary_path())
        .arg(&source)
        .arg("-o")
        .arg(&output_path)
        .output()
        .expect("failed to run implicit native backend");

    assert!(
        output.status.success(),
        "implicit native backend failed: {}",
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
