//! Executes a rendered [`LinkPlan`] via `Command` (no shell), with post-link
//! validation and cleanup-on-failure (design §6.5).

use std::fs;
use std::process::Command;

use super::is_verbose;
#[cfg(windows)]
use super::plan::LinkerFlavor;
use super::plan::{LinkPlan, LinkerExecutable};

/// Run `plan` to completion. On any failure — spawn failure, non-zero exit,
/// or a linker that "succeeds" but leaves no (or an empty) output file —
/// removes any partial `plan.output` so a failed link never leaves a
/// success-shaped executable behind, then returns the failure as `Err`.
pub(super) fn run(plan: &LinkPlan) -> Result<(), String> {
    let mut cmd = Command::new(plan.linker.as_os_str());
    cmd.args(plan.render());
    apply_windows_dll_search_hardening(&mut cmd, plan);

    eprintln!(
        "Linking {mode} executable with {} ({})...",
        plan.linker.display_command(),
        linker_source_label(&plan.linker),
        mode = plan.runtime_mode,
    );
    if is_verbose() {
        eprintln!("[verbose] {:?}", cmd);
    }

    let output = match cmd.output() {
        Ok(output) => output,
        Err(e) => {
            cleanup_partial_output(plan);
            return Err(format!(
                "failed to run '{}': {e}",
                plan.linker.display_command()
            ));
        }
    };

    if !output.status.success() {
        cleanup_partial_output(plan);
        return Err(format!(
            "linking failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    if let Err(e) = validate_link_output(plan) {
        cleanup_partial_output(plan);
        return Err(e);
    }

    Ok(())
}

/// Windows-only defense-in-depth, `MingwDirect` flavor only (not
/// `CompilerDriver`, which needs its normal environment/CWD to find its own
/// support files): Windows always searches the *loading executable's own
/// directory* first when resolving a binary's static/implicit imports —
/// which is exactly why the embedded sibling DLLs already work with no
/// extra `PATH` wiring at all — but a delay-loaded import, or the linker
/// itself performing a dynamic `LoadLibrary("name")`-style lookup, can
/// still fall through to searching the current working directory and
/// `PATH`. Neither of those should ever need to resolve to anything other
/// than this linker's own bin directory, so both are pinned there
/// explicitly. This is not a fix for any proven exploit in `ld.lld` itself
/// — it is narrowing an edge case a third-party binary *could* exercise,
/// on the general Windows DLL search-order principle above. Every other
/// inherited environment variable is left alone (no `env_clear()`, which
/// risks breaking something the linker legitimately needs, e.g.
/// `SystemRoot`/`TMP`).
#[cfg(windows)]
fn apply_windows_dll_search_hardening(cmd: &mut Command, plan: &LinkPlan) {
    if plan.flavor != LinkerFlavor::MingwDirect {
        return;
    }
    if let Some(bin_dir) = std::path::Path::new(plan.linker.as_os_str()).parent() {
        cmd.current_dir(bin_dir);
        cmd.env("PATH", bin_dir);
    }
}

#[cfg(not(windows))]
fn apply_windows_dll_search_hardening(_cmd: &mut Command, _plan: &LinkPlan) {}

/// A linker that exits 0 must actually have produced a non-empty output
/// file — otherwise treat it as a link failure rather than silently
/// reporting success for a missing/truncated executable.
fn validate_link_output(plan: &LinkPlan) -> Result<(), String> {
    let meta = fs::metadata(&plan.output).map_err(|e| {
        format!(
            "linker exited successfully but '{}' does not exist: {e}",
            plan.output.display()
        )
    })?;
    if meta.len() == 0 {
        return Err(format!(
            "linker exited successfully but '{}' is empty",
            plan.output.display()
        ));
    }
    Ok(())
}

/// A failed link must never leave a success-shaped executable behind
/// (design §6.5). Best-effort: if the file never existed (the common case),
/// removal simply no-ops.
fn cleanup_partial_output(plan: &LinkPlan) {
    let _ = fs::remove_file(&plan.output);
}

fn linker_source_label(linker: &LinkerExecutable) -> &'static str {
    match linker {
        LinkerExecutable::Embedded { .. } => "embedded",
        LinkerExecutable::Override { .. } => "override",
        LinkerExecutable::CompilerDriver { source, .. } => crate::compiler_source_label(*source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::link::plan::{LinkerFlavor, SystemLib};
    use crate::backend::target::NativeTarget;
    use crate::backend::RuntimeMode;
    use std::path::PathBuf;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oscan-execute-rs-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    fn minimal_plan(output: PathBuf) -> LinkPlan {
        LinkPlan {
            flavor: LinkerFlavor::MingwDirect,
            linker: LinkerExecutable::Embedded {
                path: PathBuf::from("ld.lld.exe"),
            },
            target: NativeTarget::WindowsX64,
            runtime_mode: RuntimeMode::Freestanding,
            output,
            objects: vec![],
            archives: vec![],
            system_libs: Vec::<SystemLib>::new(),
            builtins: None,
            search_paths: vec![],
            extra_objects: vec![],
            entry: None,
            gc_sections: true,
            strip: true,
            build_id_none: true,
            pie: false,
            emulation: Some("i386pep"),
            use_lld_driver_flag: false,
            show_warnings: false,
            extra_c_sources: vec![],
            include_dirs: vec![],
            passthrough_cflags: vec![],
            static_link: false,
            extra_libs: vec![],
        }
    }

    #[test]
    fn validate_link_output_rejects_a_missing_file() {
        let dir = scratch_dir("missing");
        let plan = minimal_plan(dir.join("nonexistent.exe"));
        let err = validate_link_output(&plan).expect_err("missing output must be rejected");
        assert!(err.contains("does not exist"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_link_output_rejects_an_empty_file() {
        let dir = scratch_dir("empty");
        let exe = dir.join("empty.exe");
        fs::write(&exe, []).expect("write empty file");
        let plan = minimal_plan(exe);
        let err = validate_link_output(&plan).expect_err("empty output must be rejected");
        assert!(err.contains("is empty"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_link_output_accepts_a_non_empty_file() {
        let dir = scratch_dir("nonempty");
        let exe = dir.join("hello.exe");
        fs::write(&exe, b"not really an exe, just non-empty").expect("write file");
        let plan = minimal_plan(exe);
        assert!(validate_link_output(&plan).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_partial_output_removes_an_existing_file() {
        let dir = scratch_dir("cleanup");
        let exe = dir.join("partial.exe");
        fs::write(&exe, b"partial").expect("write file");
        let plan = minimal_plan(exe.clone());
        cleanup_partial_output(&plan);
        assert!(!exe.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_partial_output_is_a_no_op_when_nothing_exists() {
        let dir = scratch_dir("cleanup-noop");
        let exe = dir.join("never-existed.exe");
        let plan = minimal_plan(exe);
        // Must not panic even though the file was never created.
        cleanup_partial_output(&plan);
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Windows DLL search hardening (MingwDirect only). ---

    #[cfg(windows)]
    #[test]
    fn dll_search_hardening_pins_cwd_and_path_for_mingw_direct() {
        let dir = scratch_dir("dll-hardening-mingw");
        let linker_path = dir.join("ld.lld.exe");
        fs::write(&linker_path, b"fake linker").unwrap();

        let mut plan = minimal_plan(dir.join("out.exe"));
        plan.flavor = LinkerFlavor::MingwDirect;
        plan.linker = LinkerExecutable::Embedded {
            path: linker_path.clone(),
        };

        let mut cmd = Command::new(plan.linker.as_os_str());
        apply_windows_dll_search_hardening(&mut cmd, &plan);

        assert_eq!(cmd.get_current_dir(), Some(dir.as_path()));
        let path_env = cmd
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new("PATH"))
            .and_then(|(_, v)| v)
            .expect("PATH must be explicitly set for MingwDirect");
        assert_eq!(path_env, dir.as_os_str());

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn dll_search_hardening_leaves_compiler_driver_untouched() {
        let dir = scratch_dir("dll-hardening-driver");

        let mut plan = minimal_plan(dir.join("out.exe"));
        plan.flavor = LinkerFlavor::CompilerDriver;
        plan.linker = LinkerExecutable::CompilerDriver {
            command: "clang".to_string(),
            source: crate::CompilerSource::Host,
        };

        let mut cmd = Command::new(plan.linker.as_os_str());
        apply_windows_dll_search_hardening(&mut cmd, &plan);

        assert_eq!(
            cmd.get_current_dir(),
            None,
            "CompilerDriver must keep inheriting the parent's CWD, untouched"
        );
        assert!(
            cmd.get_envs()
                .find(|(k, _)| *k == std::ffi::OsStr::new("PATH"))
                .is_none(),
            "CompilerDriver must keep inheriting PATH, untouched"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
