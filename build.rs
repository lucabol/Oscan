use std::process::Command;

fn main() {
    // Priority: OSCAN_VERSION env var (set by CI) > git describe > "unknown"
    let version = std::env::var("OSCAN_VERSION").ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            Command::new("git")
                .args(["describe", "--tags", "--always", "--dirty"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "unknown".to_string())
        });

    println!("cargo:rustc-env=GIT_VERSION={version}");
    println!("cargo:rerun-if-env-changed=OSCAN_VERSION");
    // Rebuild when git HEAD changes (new commits, tags, etc.)
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");
}
