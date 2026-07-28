//! Integration coverage for the **direct** LLVM backend.
//!
//! These tests assert the properties that make `--backend llvm` a real
//! replacement for the old C-mediated path rather than a rename:
//!
//! * it lowers Oscan's own typed IR, never a generated C translation
//!   unit, and leaves no C/intermediate artifacts behind;
//! * it runs the code generator in-process — no `clang`/`llvm-as`/`opt`/
//!   `llc` subprocess and no installed LLVM SDK;
//! * it gates targets on what the *packaged* code generator can actually
//!   emit, and says so precisely when it cannot;
//! * it validates its own object output; and
//! * it never silently falls back to another backend, while `--backend c`
//!   and `--backend cranelift` keep working exactly as before.
//!
//! Tests that need the packaged code generator skip themselves when it is
//! not configured (see [`llvm_provider_configured`]). CI sets
//! `OSCAN_LLVM_TEST_REQUIRED=1`, turning a missing provider into a hard
//! failure rather than a vacuous pass.

#[cfg(any(
    feature = "backend-llvm",
    all(feature = "backend-c", feature = "backend-cranelift")
))]
use object::{Object, ObjectKind};
#[cfg(any(feature = "backend-llvm", feature = "backend-cranelift"))]
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(feature = "backend-llvm", feature = "backend-cranelift"))]
use std::process;
use std::process::Command;

fn oscan_binary_path() -> String {
    std::env::var("CARGO_BIN_EXE_oscan")
        .expect("CARGO_BIN_EXE_oscan should be set for integration tests")
}

/// Every environment variable that steers discovery of the LLVM provider,
/// the native linker, the runtime archive, or the strict no-toolchain
/// profile. Child processes start without all of them so an ambient
/// developer/CI setting cannot mask (or fake) what these tests assert; a
/// test that needs one sets it back deliberately.
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
/// configured packaged LLVM provider (see [`llvm_provider_configured`]).
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

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(name)
}

/// Whether Oscan's packaged LLVM code generator is reachable for this
/// test run.
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

// Only the object-backend tests below need scratch space; the host gate
// on some of them means it can still go unused in a valid configuration.
#[cfg(any(feature = "backend-llvm", feature = "backend-cranelift"))]
#[allow(dead_code)]
fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oscan-llvm-{tag}-{}", process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("failed to create scratch directory");
    dir
}

#[cfg(feature = "backend-llvm")]
#[test]
fn llvm_ir_is_lowered_directly_from_oscan_ir() {
    if !llvm_provider_configured() {
        return;
    }
    let output = oscan_provider_command()
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("positive")
                .join("arithmetic.osc"),
        )
        .args(["--backend", "llvm", "--emit-llvm-ir"])
        .output()
        .expect("failed to emit LLVM IR");
    assert!(
        output.status.success(),
        "IR emission failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ir = String::from_utf8_lossy(&output.stdout);

    // Oscan's own module identity and entry-point shape, not Clang's.
    assert!(ir.contains("; ModuleID = 'oscan_program'"), "{ir}");
    assert!(ir.contains("source_filename = \"oscan_program\""), "{ir}");
    assert!(
        ir.contains("define i32 @main(i32 %arg0, ptr %arg1)"),
        "{ir}"
    );
    assert!(ir.contains("@oscan_main"), "{ir}");

    // The implicit arena ABI: every Oscan function takes a leading
    // `osc_arena*`, and the entry wrapper creates/destroys it around the
    // call.
    assert!(ir.contains("declare ptr @osc_arena_create(i64)"), "{ir}");
    assert!(ir.contains("declare void @osc_arena_destroy(ptr)"), "{ir}");
    assert!(
        ir.contains("call ptr @osc_arena_create(i64 1048576)"),
        "{ir}"
    );

    // Nothing Clang-shaped: no C source name, no `.c` file reference.
    assert!(!ir.contains("program.c"), "{ir}");
    assert!(!ir.contains("osc_runtime.h"), "{ir}");
}

#[cfg(feature = "backend-llvm")]
#[test]
fn llvm_ir_carries_no_poison_generating_flags() {
    if !llvm_provider_configured() {
        return;
    }
    // A program with arithmetic, indexing, control flow, and aggregates,
    // so the assertions below are not vacuous.
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("positive")
        .join("structs_enums.osc");
    if !source.exists() {
        panic!("the structs_enums corpus test is required for this assertion to be meaningful");
    }
    let output = oscan_provider_command()
        .arg(&source)
        .args(["--backend", "llvm", "--emit-llvm-ir"])
        .output()
        .expect("failed to emit LLVM IR");
    assert!(
        output.status.success(),
        "IR emission failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ir = String::from_utf8_lossy(&output.stdout);

    for banned in [
        " nsw ", " nuw ", "inbounds", "exact", "fast", "ninf", "nnan",
    ] {
        assert!(
            !ir.contains(banned),
            "'{banned}' must never appear: Oscan's arithmetic is runtime-checked, so promising \
             the corresponding UB would be wrong\n{ir}"
        );
    }
    // Aggregate copies are real loads/stores, never a memcpy intrinsic
    // the freestanding runtime cannot resolve.
    assert!(!ir.contains("llvm.memcpy"), "{ir}");
    assert!(!ir.contains("llvm.memmove"), "{ir}");
    assert!(!ir.contains("llvm.memset"), "{ir}");
}

#[cfg(feature = "backend-llvm")]
#[test]
fn llvm_object_emission_writes_only_the_requested_object() {
    if !llvm_provider_configured() {
        return;
    }
    let dir = scratch_dir("artifacts");
    let output_path = dir.join("program.obj");
    let output = oscan_provider_command()
        .arg(example("hello.osc"))
        .args(["--backend", "llvm", "-o"])
        .arg(&output_path)
        .current_dir(&dir)
        .output()
        .expect("failed to run LLVM object emission");
    assert!(
        output.status.success(),
        "object emission failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let leftovers: Vec<String> = fs::read_dir(&dir)
        .expect("failed to read scratch directory")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "program.obj")
        .collect();
    assert!(
        leftovers.is_empty(),
        "no C, header, .ll, .bc or .s intermediate may survive: {leftovers:?}"
    );

    let bytes = fs::read(&output_path).expect("object should exist");
    let object = object::File::parse(bytes.as_slice()).expect("output should be an object file");
    assert_eq!(object.kind(), ObjectKind::Relocatable);
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "backend-llvm")]
#[test]
fn llvm_cross_targets_emit_the_right_object_format_and_architecture() {
    if !llvm_provider_configured() {
        return;
    }
    let dir = scratch_dir("cross");
    let cases: [(&str, object::BinaryFormat, object::Architecture); 3] = [
        (
            "windows-x86_64",
            object::BinaryFormat::Coff,
            object::Architecture::X86_64,
        ),
        (
            "linux-x86_64",
            object::BinaryFormat::Elf,
            object::Architecture::X86_64,
        ),
        (
            "linux-aarch64",
            object::BinaryFormat::Elf,
            object::Architecture::Aarch64,
        ),
    ];
    for (tag, format, arch) in cases {
        let output_path = dir.join(format!("{tag}.o"));
        let output = oscan_provider_command()
            .arg(example("hello.osc"))
            .args(["--backend", "llvm", "--native-target", tag, "-o"])
            .arg(&output_path)
            .output()
            .expect("failed to run cross object emission");
        assert!(
            output.status.success(),
            "{tag} object emission failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = fs::read(&output_path).expect("object should exist");
        let object =
            object::File::parse(bytes.as_slice()).expect("output should be an object file");
        assert_eq!(object.kind(), ObjectKind::Relocatable, "{tag}");
        assert_eq!(object.format(), format, "{tag}");
        assert_eq!(object.architecture(), arch, "{tag}");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "backend-llvm")]
#[test]
fn llvm_refuses_a_target_the_packaged_code_generator_cannot_emit() {
    if !llvm_provider_configured() {
        return;
    }
    let dir = scratch_dir("riscv");
    let output_path = dir.join("riscv.o");
    let output = oscan_provider_command()
        .arg(example("hello.osc"))
        .args([
            "--backend",
            "llvm",
            "--native-target",
            "linux-riscv64",
            "-o",
        ])
        .arg(&output_path)
        .output()
        .expect("failed to run RISC-V gate validation");

    // The packaged Windows/Linux code generator Oscan ships today has X86
    // and AArch64 but no RISC-V. Either it is genuinely absent (the
    // expected case, and the error must say exactly why) or a future
    // packaged library gained it, in which case the object must be a
    // valid RISC-V object rather than a lie.
    if output.status.success() {
        let bytes = fs::read(&output_path).expect("object should exist");
        let object =
            object::File::parse(bytes.as_slice()).expect("output should be an object file");
        assert_eq!(object.architecture(), object::Architecture::Riscv64);
        match object.flags() {
            object::FileFlags::Elf { e_flags, .. } => {
                const EF_RISCV_RVC: u32 = 0x1;
                const EF_RISCV_FLOAT_ABI_MASK: u32 = 0x6;
                const EF_RISCV_FLOAT_ABI_DOUBLE: u32 = 0x4;
                assert_ne!(
                    e_flags & EF_RISCV_RVC,
                    0,
                    "the packaged runtime targets RV64GC, so LLVM must enable compressed instructions"
                );
                assert_eq!(
                    e_flags & EF_RISCV_FLOAT_ABI_MASK,
                    EF_RISCV_FLOAT_ABI_DOUBLE,
                    "LLVM must emit the same lp64d hard-float ABI as the packaged runtime"
                );
            }
            flags => panic!("RISC-V output must carry ELF flags, got {flags:?}"),
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cannot target linux-riscv64"),
            "unexpected diagnostic: {stderr}"
        );
        assert!(stderr.contains("no riscv64 back end"), "{stderr}");
        assert!(stderr.contains("--backend cranelift"), "{stderr}");
        assert!(!output_path.exists(), "no partial object may be written");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(all(feature = "backend-llvm", feature = "backend-cranelift"))]
#[test]
fn llvm_and_cranelift_backends_agree_on_hello_world_output() {
    if !llvm_provider_configured() {
        return;
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("positive")
        .join("hello_world.osc");
    if !source.exists() {
        return;
    }
    let expected = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("expected")
        .join("hello_world.expected");
    let expected = fs::read_to_string(&expected)
        .expect("expected output should exist")
        .replace("\r\n", "\n");
    let expected = expected.trim_end();

    let dir = scratch_dir("parity");
    for backend in ["llvm", "cranelift"] {
        let exe = dir.join(format!(
            "{backend}{}",
            if cfg!(windows) { ".exe" } else { "" }
        ));
        let build = oscan_provider_command()
            .arg(&source)
            .args(["--backend", backend, "-o"])
            .arg(&exe)
            .output()
            .expect("failed to build");
        if !build.status.success() {
            // A machine without a usable final linker cannot run this
            // comparison; the object-level tests above still apply.
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        let run = Command::new(&exe).output().expect("failed to run");
        let stdout = String::from_utf8_lossy(&run.stdout).replace("\r\n", "\n");
        assert_eq!(
            stdout.trim_end(),
            expected,
            "{backend} backend output differs from the expected corpus output"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The strict-profile refusal always names the operation and the reason.
/// Only a build that *has* a C backend is armed (and could be disarmed)
/// by `OSCAN_NO_TOOLCHAIN`; a single-backend LLVM/Cranelift build is
/// strict intrinsically and must not point at a variable that would not
/// help.
fn assert_strict_refusal(stderr: &str, refused: &str) {
    assert!(stderr.contains(refused), "{stderr}");
    assert!(stderr.contains("C toolchain"), "{stderr}");
    if cfg!(feature = "backend-c") {
        assert!(stderr.contains("OSCAN_NO_TOOLCHAIN=1"), "{stderr}");
    } else {
        assert!(
            stderr.contains("does not include the C backend"),
            "{stderr}"
        );
        assert!(!stderr.contains("OSCAN_NO_TOOLCHAIN"), "{stderr}");
    }
}

#[cfg(feature = "backend-c")]
#[test]
fn strict_no_toolchain_profile_refuses_the_c_backend() {
    let output = oscan_command()
        .arg(example("hello.osc"))
        .args(["--backend", "c", "--emit-c"])
        .env("OSCAN_NO_TOOLCHAIN", "1")
        .output()
        .expect("failed to run strict-profile validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_strict_refusal(&stderr, "refuses the C backend");
    assert!(stderr.contains("--backend llvm"), "{stderr}");
    // And no C actually reached stdout.
    assert!(output.stdout.is_empty(), "no C may be emitted");
}

/// A build without the C backend refuses `--backend c` before the strict
/// profile ever comes up: it is not a policy choice, the code generator
/// simply is not there.
#[cfg(not(feature = "backend-c"))]
#[test]
fn a_build_without_the_c_backend_refuses_it_by_name() {
    let output = oscan_command()
        .arg(example("hello.osc"))
        .args(["--backend", "c", "--emit-c"])
        .output()
        .expect("failed to run compiled-out C backend validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("the c backend is not included in this compiler build"),
        "{stderr}"
    );
    assert!(stderr.contains("ends in '-c'"), "{stderr}");
    assert!(output.stdout.is_empty(), "no C may be emitted");
}

#[cfg(feature = "backend-llvm")]
#[test]
fn strict_no_toolchain_profile_refuses_extra_c_sources() {
    if !llvm_provider_configured() {
        return;
    }
    let dir = scratch_dir("strict-extra-c");
    let extra = dir.join("helper.c");
    fs::write(&extra, "int helper(void) { return 0; }\n").expect("failed to write helper");
    let output_path = dir.join("out.obj");
    let output = oscan_provider_command()
        .arg(example("hello.osc"))
        .args(["--backend", "llvm", "--extra-c"])
        .arg(&extra)
        .arg("-o")
        .arg(&output_path)
        .env("OSCAN_NO_TOOLCHAIN", "1")
        .output()
        .expect("failed to run strict-profile validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_strict_refusal(&stderr, "refuses --extra-c");
    assert!(stderr.contains("--extra-obj"), "{stderr}");
    assert!(!output_path.exists());
    let _ = fs::remove_dir_all(&dir);
}

/// The hosted (`--libc`) runtime links this machine's CRT/libm through a
/// C toolchain, so the strict profile refuses it by name — the same
/// policy a single-backend LLVM/Cranelift distribution enforces
/// intrinsically, since such a build has no C backend at all.
#[cfg(any(feature = "backend-llvm", feature = "backend-cranelift"))]
#[test]
fn strict_no_toolchain_profile_refuses_the_hosted_libc_runtime() {
    let backends: Vec<&str> = ["llvm", "cranelift"]
        .into_iter()
        .filter(|backend| match *backend {
            "llvm" => cfg!(feature = "backend-llvm"),
            _ => cfg!(feature = "backend-cranelift"),
        })
        .collect();
    for backend in backends {
        let output = oscan_command()
            .arg(example("hello.osc"))
            .args(["--backend", backend, "--libc"])
            .env("OSCAN_NO_TOOLCHAIN", "1")
            .output()
            .expect("failed to run strict-profile validation");

        assert!(!output.status.success(), "{backend} unexpectedly succeeded");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_strict_refusal(&stderr, "refuses --libc");
        assert!(stderr.contains("freestanding"), "{stderr}");
    }
}

/// `extern` functions with `str` in their signature are linked through a
/// generated C shim that a host C compiler has to build, so the strict
/// profile refuses them too, rather than reaching for a compiler.
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
fn strict_no_toolchain_profile_refuses_generated_extern_str_shims() {
    let dir = scratch_dir("strict-shim");
    let source = dir.join("shim.osc");
    fs::write(
        &source,
        "extern {\n    fn! host_label(value: str) -> str;\n}\n\nfn! main() {\n    println(host_label(\"x\"));\n}\n",
    )
    .expect("failed to write shim source");
    let exe = dir.join(format!("shim{}", if cfg!(windows) { ".exe" } else { "" }));
    let output = oscan_command()
        .arg(&source)
        .args(["--backend", "cranelift", "-o"])
        .arg(&exe)
        .env("OSCAN_NO_TOOLCHAIN", "1")
        .output()
        .expect("failed to run strict-profile shim validation");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_strict_refusal(&stderr, "generated C shim");
    assert!(stderr.contains("--extra-obj"), "{stderr}");
    assert!(!exe.exists(), "no executable may be produced");
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "backend-llvm")]
#[test]
fn strict_no_toolchain_profile_still_allows_direct_object_emission() {
    if !llvm_provider_configured() {
        return;
    }
    let dir = scratch_dir("strict-object");
    let output_path = dir.join("program.obj");
    let output = oscan_provider_command()
        .arg(example("hello.osc"))
        .args(["--backend", "llvm", "-o"])
        .arg(&output_path)
        .env("OSCAN_NO_TOOLCHAIN", "1")
        .output()
        .expect("failed to run strict-profile object emission");

    assert!(
        output.status.success(),
        "strict-profile object emission must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(&output_path).expect("object should exist");
    let object = object::File::parse(bytes.as_slice()).expect("output should be an object file");
    assert_eq!(object.kind(), ObjectKind::Relocatable);
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(all(feature = "backend-c", feature = "backend-cranelift"))]
#[test]
fn c_and_cranelift_backends_are_unaffected_by_the_llvm_migration() {
    // `--backend c` still emits a standalone C translation unit...
    let c_output = oscan_command()
        .arg(example("hello.osc"))
        .args(["--backend", "c", "--emit-c"])
        .output()
        .expect("failed to run C backend");
    assert!(
        c_output.status.success(),
        "C backend failed: {}",
        String::from_utf8_lossy(&c_output.stderr)
    );
    let c_source = String::from_utf8_lossy(&c_output.stdout);
    assert!(c_source.contains("int main("), "{c_source}");
    assert!(c_source.contains("oscan_main"), "{c_source}");

    // ...and `--backend cranelift` still emits a Cranelift object. Use an
    // explicit Linux object target so this remains valid on macOS, where
    // native host auto-detection is intentionally unsupported.
    let native_target = match std::env::consts::ARCH {
        "aarch64" => "linux-aarch64",
        "riscv64" => "linux-riscv64",
        _ => "linux-x86_64",
    };
    let dir = scratch_dir("cranelift-object");
    let output_path = dir.join("program.o");
    let cranelift_output = oscan_command()
        .arg(example("hello.osc"))
        .args([
            "--backend",
            "cranelift",
            "--native-target",
            native_target,
            "-o",
        ])
        .arg(&output_path)
        .output()
        .expect("failed to run cranelift backend");
    assert!(
        cranelift_output.status.success(),
        "cranelift backend failed: {}",
        String::from_utf8_lossy(&cranelift_output.stderr)
    );
    let bytes = fs::read(&output_path).expect("object should exist");
    let object = object::File::parse(bytes.as_slice()).expect("output should be an object file");
    assert_eq!(object.kind(), ObjectKind::Relocatable);
    let _ = fs::remove_dir_all(&dir);
}
