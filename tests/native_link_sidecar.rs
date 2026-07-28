//! Executable-relative **sidecar** native-link asset discovery, end to end.
//!
//! The unit tests in `src/backend/native_assets/sidecar.rs` cover the
//! manifest/validation rules against a synthetic package directory. These
//! tests cover the part that only a real process can prove: that discovery
//! is anchored to the *running executable's own directory*, that a package
//! whose sidecar is corrupt fails hard instead of falling back to anything
//! on the host, and that a file inside the sidecar directory is never
//! loaded unless the manifest vouches for it.
//!
//! Each test copies the built `oscan` binary into a temporary directory and
//! stages a `native-link/` package beside that copy, so nothing depends on
//! (or disturbs) the real build tree.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

fn oscan_binary_path() -> PathBuf {
    PathBuf::from(
        std::env::var("CARGO_BIN_EXE_oscan")
            .expect("CARGO_BIN_EXE_oscan should be set for integration tests"),
    )
}

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(name)
}

/// Every environment variable that steers discovery of the LLVM provider,
/// the native linker, the runtime archive, or the strict no-toolchain
/// profile. These tests assert what a *packaged* compiler discovers from
/// its own directory, so the child process starts without all of them
/// unless the test sets one deliberately.
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

fn scrubbed(command: &mut Command) -> &mut Command {
    for name in DISCOVERY_ENV {
        command.env_remove(name);
    }
    command
}

/// A temporary directory holding a copy of `oscan` plus a `native-link/`
/// sidecar directory — the shape a backend-specific release package has.
struct PackageLayout {
    dir: PathBuf,
    exe: PathBuf,
}

impl PackageLayout {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oscan-sidecar-pkg-{tag}-{}-{}",
            process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(dir.join("native-link").join("bin")).expect("create sidecar dir");
        let exe = dir.join(if cfg!(windows) { "oscan.exe" } else { "oscan" });
        fs::copy(oscan_binary_path(), &exe).expect("copy the compiler into the package");
        PackageLayout { dir, exe }
    }

    #[cfg(feature = "backend-llvm")]
    fn sidecar(&self) -> PathBuf {
        self.dir.join("native-link")
    }

    #[cfg(feature = "backend-llvm")]
    fn write_manifest(&self, json: &str) {
        fs::write(self.sidecar().join("native-link-assets.json"), json)
            .expect("write sidecar manifest");
    }

    #[cfg(feature = "backend-llvm")]
    fn stage(&self, install_subpath: &str, contents: &[u8]) {
        let path = self.sidecar().join(install_subpath);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create staged asset dir");
        }
        fs::write(path, contents).expect("stage asset");
    }

    /// Run the packaged copy of the compiler with a scrubbed environment.
    #[cfg(feature = "backend-llvm")]
    fn run(&self, args: &[&str]) -> std::process::Output {
        scrubbed(&mut Command::new(&self.exe))
            .args(args)
            .current_dir(&self.dir)
            .output()
            .expect("failed to run the packaged compiler")
    }
}

impl Drop for PackageLayout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A `libLLVM` file name this platform's provider search looks for.
#[cfg(feature = "backend-llvm")]
fn provider_library_name() -> &'static str {
    if cfg!(windows) {
        "libLLVM-22.dll"
    } else {
        "libLLVM.so.22.1"
    }
}

/// The packaged LLVM provider search reaches into `<exe-dir>/native-link/`
/// (Windows shares one `libLLVM-22.dll` between the code generator and
/// `ld.lld.exe`), but a corrupt sidecar manifest must make that candidate
/// a named failure rather than something the compiler loads anyway.
#[cfg(feature = "backend-llvm")]
#[test]
fn a_corrupt_sidecar_manifest_never_yields_a_provider_candidate() {
    let package = PackageLayout::new("corrupt-manifest");
    package.write_manifest("{ this is not json");
    package.stage(
        &format!("bin/{}", provider_library_name()),
        b"not a real library",
    );

    let output = package.run(&[
        "--backend",
        "llvm",
        example("hello.osc").to_str().expect("utf-8 path"),
        "-o",
        "out.obj",
    ]);

    assert!(!output.status.success(), "a corrupt package must not link");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("packaged native-link assets are unusable"),
        "{stderr}"
    );
    assert!(stderr.contains("not valid JSON"), "{stderr}");
    // No silent fallback to a host toolchain or an unverified file.
    assert!(stderr.contains("never falls back"), "{stderr}");
    assert!(!package.dir.join("out.obj").exists());
}

/// A file sitting inside the sidecar directory that the manifest does not
/// declare is never loaded, even though it has exactly the name the
/// provider search looks for.
#[cfg(feature = "backend-llvm")]
#[test]
fn an_undeclared_file_in_the_sidecar_is_never_loaded() {
    let package = PackageLayout::new("undeclared");
    // A structurally complete package (a Linux package needs only its
    // linker) that simply does not declare the library below...
    let linker_bytes = b"linker bytes";
    package.stage("bin/ld", linker_bytes);
    package.write_manifest(&format!(
        r#"{{
  "schema_version": 1,
  "target": "linux-x86_64",
  "linker": {{ "role": "linker", "name": "ld", "install_subpath": "bin/ld", "sha256": "{}" }},
  "assets": []
}}"#,
        sha256_hex(linker_bytes)
    ));
    // ...next to an undeclared library with the provider's own name.
    package.stage(&format!("bin/{}", provider_library_name()), b"planted");

    let output = package.run(&[
        "--backend",
        "llvm",
        example("hello.osc").to_str().expect("utf-8 path"),
        "-o",
        "out.obj",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not declare it"),
        "an undeclared sidecar file must be refused by name: {stderr}"
    );
    assert!(!package.dir.join("out.obj").exists());
}

/// Discovery is anchored to the executable's directory, not the process
/// working directory: a corrupt package staged in the *current* directory
/// of a compiler whose own directory has no sidecar is ignored entirely.
#[test]
fn a_sidecar_in_the_current_directory_is_ignored() {
    let package = PackageLayout::new("cwd-only");
    // The copied compiler's own directory has an (empty) native-link
    // directory but no manifest, so it has no sidecar package at all.
    let cwd = package.dir.join("workdir");
    fs::create_dir_all(cwd.join("native-link")).expect("create cwd sidecar");
    fs::write(
        cwd.join("native-link").join("native-link-assets.json"),
        "{ not json either",
    )
    .expect("write cwd manifest");

    // Run the packaged copy from that directory: the corrupt CWD package
    // must never be consulted.
    let version = scrubbed(&mut Command::new(&package.exe))
        .arg("--version")
        .current_dir(&cwd)
        .output()
        .expect("failed to run the packaged compiler");
    assert!(version.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr)
    );
    assert!(
        !combined.contains("native-link assets are unusable"),
        "the current directory must never be searched for sidecar assets: {combined}"
    );

    let emit = scrubbed(&mut Command::new(&package.exe))
        .arg(example("hello.osc"))
        .args(["-o", "cwd-probe.obj"])
        .current_dir(&cwd)
        .output()
        .expect("failed to run object emission");
    let emit_stderr = String::from_utf8_lossy(&emit.stderr);
    assert!(
        !emit_stderr.contains("native-link assets are unusable"),
        "compilation must not consult a CWD sidecar: {emit_stderr}"
    );
}

/// A complete, explicit direct-linker override consumes no packaged
/// assets, so it must not parse — let alone fail on — an unrelated corrupt
/// sidecar package. (`OSCAN_RUNTIME_ARCHIVE_DIR` and the linker override
/// are set deliberately here; everything else is scrubbed.)
#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
#[test]
fn an_explicit_direct_linker_override_ignores_a_corrupt_sidecar() {
    let package = PackageLayout::new("override-bypass");
    fs::write(
        package
            .dir
            .join("native-link")
            .join("native-link-assets.json"),
        "{ definitely not json",
    )
    .expect("write corrupt manifest");

    // A runtime archive the link step can find, so the run reaches
    // linker selection rather than stopping at archive discovery.
    let archives = package.dir.join("archives");
    fs::create_dir_all(&archives).expect("create archive dir");
    fs::write(
        archives.join("libosc_runtime_freestanding.a"),
        b"not a real archive",
    )
    .expect("stage a placeholder archive");
    // A "linker" that exists but cannot link anything: reaching it at all
    // is the point.
    let fake_linker = package.dir.join(if cfg!(windows) {
        "fake-ld.exe"
    } else {
        "fake-ld"
    });
    fs::write(&fake_linker, b"not a linker").expect("stage a fake linker");

    let backend = if cfg!(feature = "backend-cranelift") {
        "cranelift"
    } else {
        "llvm"
    };
    let output = scrubbed(&mut Command::new(&package.exe))
        .arg(example("hello.osc"))
        .args(["--backend", backend, "-o"])
        .arg(package.dir.join("out.exe"))
        .env("OSCAN_RUNTIME_ARCHIVE_DIR", &archives)
        .env("OSCAN_NATIVE_LINKER", &fake_linker)
        .env("OSCAN_NATIVE_LINKER_FLAVOR", "elf")
        .current_dir(&package.dir)
        .output()
        .expect("failed to run the packaged compiler");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("packaged native-link assets are unusable"),
        "a complete direct-linker override must not consult the sidecar: {stderr}"
    );
}

#[cfg(feature = "backend-llvm")]
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
