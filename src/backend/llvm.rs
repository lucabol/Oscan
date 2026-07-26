//! LLVM object backend.
//!
//! Oscan deliberately keeps LLVM out of its Cargo dependency graph. The
//! backend asks Clang to turn the mature typed-IR-to-C lowering into textual
//! LLVM IR, then asks the same toolchain to emit a relocatable object. Final
//! linking is handled by [`super::link`] exactly as it is for Cranelift.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use object::{Architecture, BinaryFormat, Object, ObjectKind};

use crate::error::CompileError;
use crate::token::Span;

use super::{NativeTarget, RuntimeMode};

const RUNTIME_HEADER: &str = include_str!("../../runtime/osc_runtime.h");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolchainSource {
    Override,
    Bundled,
    Path,
    VisualStudio,
}

impl ToolchainSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Bundled => "bundled",
            Self::Path => "PATH",
            Self::VisualStudio => "Visual Studio",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlvmToolchain {
    pub clang: PathBuf,
    pub version: String,
    pub source: ToolchainSource,
}

pub struct LlvmCompileOutput {
    pub ir_text: String,
    pub object_bytes: Option<Vec<u8>>,
    pub toolchain: LlvmToolchain,
}

pub fn target_triple(target: NativeTarget) -> &'static str {
    match target {
        NativeTarget::WindowsX64 => "x86_64-w64-windows-gnu",
        NativeTarget::LinuxX64 => "x86_64-unknown-linux-gnu",
        NativeTarget::LinuxAarch64 => "aarch64-unknown-linux-gnu",
        NativeTarget::LinuxRiscv64 => "riscv64-unknown-linux-gnu",
    }
}

pub fn discover_toolchain() -> Result<LlvmToolchain, String> {
    if let Some(value) = env_var_nonempty("OSCAN_LLVM_CLANG") {
        return probe_toolchain(PathBuf::from(value), ToolchainSource::Override).map_err(|e| {
            format!("OSCAN_LLVM_CLANG does not identify a usable Clang executable: {e}")
        });
    }

    if let Some(value) = env_var_nonempty("OSCAN_LLVM_TOOLCHAIN_DIR") {
        let root = PathBuf::from(value);
        return probe_toolchain_dir(&root, ToolchainSource::Override).ok_or_else(|| {
            format!(
                "OSCAN_LLVM_TOOLCHAIN_DIR '{}' does not contain a usable clang executable",
                root.display()
            )
        });
    }

    if let Some(root) = crate::find_toolchain_dir() {
        if let Some(toolchain) = probe_toolchain_dir(&root, ToolchainSource::Bundled) {
            return Ok(toolchain);
        }
    }

    for name in versioned_clang_names() {
        if let Some(path) = resolve_on_path(name) {
            if let Ok(toolchain) = probe_toolchain(path, ToolchainSource::Path) {
                return Ok(toolchain);
            }
        }
    }

    #[cfg(windows)]
    if let Some(path) = crate::find_vs_clang() {
        if let Ok(toolchain) = probe_toolchain(PathBuf::from(path), ToolchainSource::VisualStudio) {
            return Ok(toolchain);
        }
    }

    Err(
        "LLVM backend requires Clang, but no usable executable was found; set \
         OSCAN_LLVM_CLANG, set OSCAN_LLVM_TOOLCHAIN_DIR/OSCAN_TOOLCHAIN_DIR, \
         install clang on PATH, or select --backend native/--backend c"
            .to_string(),
    )
}

pub fn compile(
    c_source: &str,
    target: NativeTarget,
    runtime_mode: RuntimeMode,
    emit_object: bool,
    show_warnings: bool,
    discovered_toolchain: Option<&LlvmToolchain>,
) -> Result<LlvmCompileOutput, CompileError> {
    let toolchain = match discovered_toolchain {
        Some(toolchain) => toolchain.clone(),
        None => discover_toolchain().map_err(compile_error)?,
    };
    let temp_dir = create_scratch_dir().map_err(|e| {
        compile_error(format!(
            "failed to create LLVM backend scratch directory: {e}"
        ))
    })?;
    let source_path = temp_dir.path().join("program.c");
    let header_path = temp_dir.path().join("osc_runtime.h");
    let ir_path = temp_dir.path().join("program.ll");
    let object_path = temp_dir
        .path()
        .join(format!("program{}", target.obj_suffix()));

    fs::write(&source_path, c_source).map_err(|e| {
        compile_error(format!(
            "failed to write LLVM backend input '{}': {e}",
            source_path.display()
        ))
    })?;
    fs::write(&header_path, RUNTIME_HEADER).map_err(|e| {
        compile_error(format!(
            "failed to write LLVM runtime header '{}': {e}",
            header_path.display()
        ))
    })?;

    let mut emit_ir = Command::new(&toolchain.clang);
    push_common_clang_args(&mut emit_ir, target, runtime_mode, show_warnings);
    emit_ir
        .current_dir(temp_dir.path())
        .arg("-S")
        .arg("-emit-llvm")
        .arg("-I")
        .arg(".")
        .arg("-o")
        .arg("program.ll")
        .arg("program.c");
    run_clang(&mut emit_ir, "LLVM IR emission", &toolchain)?;

    let ir_text = fs::read_to_string(&ir_path).map_err(|e| {
        compile_error(format!(
            "Clang reported successful LLVM IR emission but '{}' could not be read: {e}",
            ir_path.display()
        ))
    })?;
    if ir_text.trim().is_empty() {
        return Err(compile_error(
            "Clang emitted an empty LLVM IR module".to_string(),
        ));
    }

    let object_bytes = if emit_object {
        let mut emit_obj = Command::new(&toolchain.clang);
        emit_obj
            .current_dir(temp_dir.path())
            .arg("-x")
            .arg("ir")
            .arg("-c")
            .arg("-Oz")
            .arg("-ffunction-sections")
            .arg("-fdata-sections")
            .arg("-fno-stack-protector")
            .arg("-fno-asynchronous-unwind-tables")
            .arg("-target")
            .arg(target_triple(target))
            .arg("-o")
            .arg(
                object_path
                    .file_name()
                    .expect("object path has a file name"),
            )
            .arg("program.ll");
        if !show_warnings {
            emit_obj.arg("-w");
        }
        run_clang(&mut emit_obj, "LLVM object emission", &toolchain)?;
        let bytes = fs::read(&object_path).map_err(|e| {
            compile_error(format!(
                "Clang reported successful object emission but '{}' could not be read: {e}",
                object_path.display()
            ))
        })?;
        validate_object(&bytes, target)?;
        Some(bytes)
    } else {
        None
    };

    Ok(LlvmCompileOutput {
        ir_text,
        object_bytes,
        toolchain,
    })
}

fn compile_error(message: impl Into<String>) -> CompileError {
    CompileError::new(Span::new(1, 1), message.into())
}

fn create_scratch_dir() -> std::io::Result<tempfile::TempDir> {
    let dir = tempfile::Builder::new().prefix("oscan_llvm_").tempdir()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

fn push_common_clang_args(
    command: &mut Command,
    target: NativeTarget,
    runtime_mode: RuntimeMode,
    show_warnings: bool,
) {
    command
        .arg("-std=c99")
        .arg("-Oz")
        .arg("-ffunction-sections")
        .arg("-fdata-sections")
        .arg("-fno-stack-protector")
        .arg("-fno-asynchronous-unwind-tables")
        .arg("-fno-ident")
        .arg("-fdebug-compilation-dir=.")
        .arg("-target")
        .arg(target_triple(target));
    if runtime_mode == RuntimeMode::Freestanding {
        command.arg("-ffreestanding").arg("-fno-builtin");
    }
    if !show_warnings {
        command.arg("-w");
    }
}

fn run_clang(
    command: &mut Command,
    stage: &str,
    toolchain: &LlvmToolchain,
) -> Result<(), CompileError> {
    crate::verbose_command(stage, command);
    let output = command.output().map_err(|e| {
        compile_error(format!(
            "failed to start Clang for {stage} using '{}': {e}",
            toolchain.clang.display()
        ))
    })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(compile_error(format!(
        "{stage} failed with {} using '{}' ({}, {}):\n{}{}",
        output.status,
        toolchain.clang.display(),
        toolchain.source.as_str(),
        toolchain.version,
        stderr,
        stdout
    )))
}

fn validate_object(bytes: &[u8], target: NativeTarget) -> Result<(), CompileError> {
    if bytes.is_empty() {
        return Err(compile_error("Clang emitted an empty object file"));
    }
    let file = object::File::parse(bytes)
        .map_err(|e| compile_error(format!("Clang emitted an invalid object file: {e}")))?;
    if file.kind() != ObjectKind::Relocatable {
        return Err(compile_error(format!(
            "Clang emitted {:?}, expected a relocatable object",
            file.kind()
        )));
    }
    let expected_format = match target {
        NativeTarget::WindowsX64 => BinaryFormat::Coff,
        _ => BinaryFormat::Elf,
    };
    if file.format() != expected_format {
        return Err(compile_error(format!(
            "Clang emitted {:?} for target {}, expected {:?}",
            file.format(),
            target,
            expected_format
        )));
    }
    let expected_arch = match target {
        NativeTarget::WindowsX64 | NativeTarget::LinuxX64 => Architecture::X86_64,
        NativeTarget::LinuxAarch64 => Architecture::Aarch64,
        NativeTarget::LinuxRiscv64 => Architecture::Riscv64,
    };
    if file.architecture() != expected_arch {
        return Err(compile_error(format!(
            "Clang emitted architecture {:?} for target {}, expected {:?}",
            file.architecture(),
            target,
            expected_arch
        )));
    }
    Ok(())
}

fn env_var_nonempty(name: &str) -> Option<String> {
    let value = env::var(name).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn probe_toolchain_dir(root: &Path, source: ToolchainSource) -> Option<LlvmToolchain> {
    clang_candidates(root)
        .into_iter()
        .filter(|candidate| candidate.is_file())
        .find_map(|candidate| probe_toolchain(candidate, source).ok())
}

fn clang_candidates(root: &Path) -> Vec<PathBuf> {
    let executable = if cfg!(windows) { "clang.exe" } else { "clang" };
    let mut candidates = Vec::new();
    if cfg!(windows) {
        candidates.push(root.join("windows").join("bin").join(executable));
    } else {
        candidates.push(root.join("linux").join("bin").join(executable));
    }
    candidates.push(root.join("bin").join(executable));
    candidates.push(root.join(executable));
    candidates
}

fn probe_toolchain(clang: PathBuf, source: ToolchainSource) -> Result<LlvmToolchain, String> {
    let output = Command::new(&clang)
        .arg("--version")
        .output()
        .map_err(|e| format!("failed to run '{} --version': {e}", clang.display()))?;
    if !output.status.success() {
        return Err(format!(
            "'{} --version' exited with {}",
            clang.display(),
            output.status
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    if !combined.to_ascii_lowercase().contains("clang") {
        return Err(format!(
            "'{} --version' did not identify an LLVM Clang toolchain",
            clang.display()
        ));
    }
    let version = combined
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("unknown Clang version")
        .trim()
        .to_string();
    Ok(LlvmToolchain {
        clang,
        version,
        source,
    })
}

fn versioned_clang_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["clang-22.exe", "clang.exe"]
    } else {
        &["clang-22", "clang"]
    }
}

fn resolve_on_path(name: &str) -> Option<PathBuf> {
    resolve_in_path(name, &env::var_os("PATH")?)
}

fn resolve_in_path(name: &str, path_value: &OsStr) -> Option<PathBuf> {
    env::split_paths(path_value)
        // Relative and empty PATH entries search the current directory. Do not
        // let an untrusted project provide the compiler executable.
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_triples_cover_every_native_target() {
        assert_eq!(
            target_triple(NativeTarget::WindowsX64),
            "x86_64-w64-windows-gnu"
        );
        assert_eq!(
            target_triple(NativeTarget::LinuxX64),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            target_triple(NativeTarget::LinuxAarch64),
            "aarch64-unknown-linux-gnu"
        );
        assert_eq!(
            target_triple(NativeTarget::LinuxRiscv64),
            "riscv64-unknown-linux-gnu"
        );
    }

    #[test]
    fn bundled_candidates_never_use_a_bare_cwd_path() {
        let root = Path::new("install").join("toolchain");
        let candidates = clang_candidates(&root);
        assert!(!candidates.is_empty());
        assert!(candidates.iter().all(|path| path.starts_with(&root)));
        assert!(candidates.iter().all(|path| path != Path::new("clang")));
        assert!(candidates.iter().all(|path| path != Path::new("clang.exe")));
    }

    #[test]
    fn path_resolution_ignores_relative_entries() {
        let absolute = tempfile::tempdir().expect("create PATH test directory");
        let executable_name = if cfg!(windows) {
            "clang-test.exe"
        } else {
            "clang-test"
        };
        fs::write(absolute.path().join(executable_name), b"test")
            .expect("write PATH test candidate");
        let path_value =
            env::join_paths([Path::new("."), absolute.path()]).expect("construct test PATH value");

        assert_eq!(
            resolve_in_path(executable_name, &path_value),
            Some(absolute.path().join(executable_name))
        );
    }
}
