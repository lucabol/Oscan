const WORKFLOW: &str = include_str!("../.github/workflows/ci.yml");

fn job_block(name: &str, next_name: &str) -> String {
    let workflow = WORKFLOW.replace("\r\n", "\n");
    let start_marker = format!("  {name}:\n");
    let end_marker = format!("  {next_name}:\n");
    let start = workflow
        .find(&start_marker)
        .unwrap_or_else(|| panic!("missing {name} CI job"));
    let rest = &workflow[start..];
    let end = rest
        .find(&end_marker)
        .unwrap_or_else(|| panic!("missing job after {name}"));
    rest[..end].to_owned()
}

fn assert_bash_exit_check(job: &str, command: &str) {
    assert!(
        job.contains(&format!("if actual=$({command} 2>&1); then")),
        "positive loop must capture the program status"
    );
    assert!(
        job.contains("actual_exit=$?"),
        "positive loop must retain a nonzero program status"
    );
    assert!(
        job.contains("tests/expected_exit/${name}.expected"),
        "positive loop must load an expected exit override"
    );
    assert!(
        job.contains(r#"[ "$actual_exit" = "$expected_exit" ]"#),
        "positive loop must compare actual and expected exit statuses"
    );
}

#[test]
fn positive_ci_loops_enforce_expected_exit_codes() {
    assert_bash_exit_check(&job_block("linux", "windows"), r#""./tests/build/${name}""#);

    let windows = job_block("windows", "native-link-embedding-smoke");
    assert!(
        windows.contains(r#"$actualExit = $LASTEXITCODE"#),
        "Windows positive loop must capture LASTEXITCODE"
    );
    assert!(
        windows.contains(r#"tests\expected_exit\$name.expected"#),
        "Windows positive loop must load an expected exit override"
    );
    assert!(
        windows.contains(r#"$actualExit -eq $expectedExit"#),
        "Windows positive loop must compare actual and expected exit statuses"
    );

    assert_bash_exit_check(
        &job_block("macos", "arm-qemu"),
        r#""./tests/build/${name}""#,
    );
    assert_bash_exit_check(
        &job_block("arm-qemu", "riscv-qemu"),
        r#"qemu-aarch64 "./tests/build/${name}""#,
    );
    assert_bash_exit_check(
        &job_block("riscv-qemu", "wasi"),
        r#"qemu-riscv64 "./tests/build/${name}""#,
    );
}
