use std::process::Command;

fn oscan_binary_path() -> String {
    std::env::var("CARGO_BIN_EXE_oscan").expect("CARGO_BIN_EXE_oscan should be set for integration tests")
}

#[test]
fn long_help_flag_prints_usage_and_succeeds() {
    let output = Command::new(oscan_binary_path())
        .arg("--help")
        .output()
        .expect("failed to run oscan --help");

    assert!(output.status.success(), "expected --help to exit successfully");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("usage: oscan"));
    assert!(stdout.contains("--target <arch>"));
}
