use std::borrow::Cow;
use std::ffi::{c_char, CString};
use std::fs;
use std::path::{Path, PathBuf};

use super::capability::freestanding_profile_from_bytes;
#[cfg(test)]
use super::capability::FreestandingProfile;
use super::{is_system_library_name, NativeLinkOptions, NativeTarget, RuntimeMode};

mod assets {
    include!(concat!(env!("OUT_DIR"), "/inprocess_link_assets.rs"));
}

#[repr(C)]
struct RawMemoryInput {
    name: *const c_char,
    data: *const u8,
    size: usize,
}

#[repr(C)]
struct RawLinkResult {
    return_code: i32,
    can_run_again: u8,
    stdout_data: *mut c_char,
    stdout_size: usize,
    stderr_data: *mut c_char,
    stderr_size: usize,
}

unsafe extern "C" {
    fn oscan_lld_link(
        args: *const *const c_char,
        arg_count: usize,
        inputs: *const RawMemoryInput,
        input_count: usize,
        result: *mut RawLinkResult,
    ) -> i32;
    fn oscan_lld_dispose_result(result: *mut RawLinkResult);
}

struct MemoryInput<'a> {
    name: String,
    bytes: Cow<'a, [u8]>,
}

pub(super) fn link_executable(
    target: NativeTarget,
    runtime_mode: RuntimeMode,
    object_bytes: &[u8],
    output: &Path,
    options: &NativeLinkOptions<'_>,
) -> Result<(), String> {
    if target != NativeTarget::WindowsX64 {
        return Err(format!(
            "the in-process linker currently supports windows-x86_64 only, not {}",
            target
        ));
    }
    if runtime_mode != RuntimeMode::Freestanding {
        return Err(
            "the strict in-process linker supports freestanding runtime mode only".to_string(),
        );
    }
    if !options.extra_c_files.is_empty() || !options.extra_cflags.is_empty() {
        return Err(format!(
            "C inputs and C compiler flags are incompatible with the strict in-process linker"
        ));
    }
    if assets::TARGET != "windows-x86_64" {
        return Err("the compiler does not contain windows-x86_64 link assets".to_string());
    }

    let profile = freestanding_profile_from_bytes("program.o", object_bytes)?;
    let mut inputs = vec![MemoryInput {
        name: "__oscan/program.o".to_string(),
        bytes: Cow::Borrowed(object_bytes),
    }];
    let mut positional_names = vec!["__oscan/program.o".to_string()];

    for (index, path) in options.extra_objects.iter().enumerate() {
        let path = Path::new(path);
        let bytes = read_strict_input(path)?;
        let name = format!("__oscan/extra/object-{index:04}.o");
        positional_names.push(name.clone());
        inputs.push(MemoryInput {
            name,
            bytes: Cow::Owned(bytes),
        });
    }
    for (index, library) in options.extra_libs.iter().enumerate() {
        if is_system_library_name(library) {
            return Err(format!(
                "named library '{library}' is incompatible with the strict in-process linker; \
                 pass the archive's path"
            ));
        }
        let path = Path::new(library);
        let bytes = read_strict_input(path)?;
        let name = format!("__oscan/extra/library-{index:04}.a");
        positional_names.push(name.clone());
        inputs.push(MemoryInput {
            name,
            bytes: Cow::Owned(bytes),
        });
    }

    let runtime_profile = profile.build_mode_str();
    let runtime = assets::ASSETS
        .iter()
        .find(|asset| asset.role == "runtime_archive" && asset.profile == Some(runtime_profile))
        .ok_or_else(|| {
            format!("the compiler does not contain the '{runtime_profile}' runtime archive")
        })?;
    let runtime_name = format!("__oscan/runtime/{}", runtime.name);
    positional_names.push(runtime_name.clone());
    inputs.push(MemoryInput {
        name: runtime_name,
        bytes: Cow::Borrowed(runtime.bytes),
    });

    for asset in assets::ASSETS
        .iter()
        .filter(|asset| asset.role == "import_lib")
    {
        let name = format!("__oscan/import/{}", asset.name);
        positional_names.push(name.clone());
        inputs.push(MemoryInput {
            name,
            bytes: Cow::Borrowed(asset.bytes),
        });
    }
    let builtins = assets::ASSETS
        .iter()
        .find(|asset| asset.role == "compiler_builtins")
        .ok_or_else(|| "the compiler does not contain compiler builtins".to_string())?;
    let builtins_name = format!("__oscan/builtins/{}", builtins.name);
    positional_names.push(builtins_name.clone());
    inputs.push(MemoryInput {
        name: builtins_name,
        bytes: Cow::Borrowed(builtins.bytes),
    });

    let output = absolute_path(output)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create output directory '{}': {error}",
                parent.display()
            )
        })?;
    }
    remove_stale_output(&output)?;

    let mut args = vec![
        "ld.lld".to_string(),
        "-s".to_string(),
        "-m".to_string(),
        "i386pep".to_string(),
        "-Bdynamic".to_string(),
        "--gc-sections".to_string(),
        "--build-id=none".to_string(),
        "-o".to_string(),
        output.to_string_lossy().into_owned(),
    ];
    args.extend(positional_names);
    invoke_lld(&args, &inputs).inspect_err(|_| {
        let _ = fs::remove_file(&output);
    })?;

    let metadata = fs::metadata(&output).map_err(|error| {
        format!(
            "in-process LLD reported success but did not create '{}': {error}",
            output.display()
        )
    })?;
    if metadata.len() == 0 {
        let _ = fs::remove_file(&output);
        return Err(format!(
            "in-process LLD created an empty output '{}'",
            output.display()
        ));
    }
    Ok(())
}

fn read_strict_input(path: &Path) -> Result<Vec<u8>, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    if bytes.starts_with(b"!<thin>\n") {
        return Err(format!(
            "thin archive '{}' is not self-contained and cannot be used by the strict linker",
            path.display()
        ));
    }
    Ok(bytes)
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|error| {
            format!(
                "failed to resolve output path '{}': {error}",
                path.display()
            )
        })
}

fn remove_stale_output(output: &Path) -> Result<(), String> {
    match fs::remove_file(output) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to remove stale output '{}': {error}",
            output.display()
        )),
    }
}

fn invoke_lld(args: &[String], inputs: &[MemoryInput<'_>]) -> Result<(), String> {
    let argument_strings = args
        .iter()
        .map(|argument| {
            CString::new(argument.as_str())
                .map_err(|_| format!("linker argument contains a NUL byte: {argument:?}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let argument_ptrs = argument_strings
        .iter()
        .map(|argument| argument.as_ptr())
        .collect::<Vec<_>>();
    let input_names = inputs
        .iter()
        .map(|input| {
            CString::new(input.name.as_str())
                .map_err(|_| format!("virtual input name contains a NUL byte: {:?}", input.name))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let raw_inputs = inputs
        .iter()
        .zip(&input_names)
        .map(|(input, name)| RawMemoryInput {
            name: name.as_ptr(),
            data: input.bytes.as_ptr(),
            size: input.bytes.len(),
        })
        .collect::<Vec<_>>();
    let mut result = RawLinkResult {
        return_code: -1,
        can_run_again: 0,
        stdout_data: std::ptr::null_mut(),
        stdout_size: 0,
        stderr_data: std::ptr::null_mut(),
        stderr_size: 0,
    };
    let bridge_status = unsafe {
        oscan_lld_link(
            argument_ptrs.as_ptr(),
            argument_ptrs.len(),
            raw_inputs.as_ptr(),
            raw_inputs.len(),
            &mut result,
        )
    };
    if bridge_status != 0 {
        return Err(format!(
            "the in-process LLD bridge failed with status {bridge_status}"
        ));
    }

    let stdout = unsafe { copy_bridge_output(result.stdout_data, result.stdout_size) };
    let stderr = unsafe { copy_bridge_output(result.stderr_data, result.stderr_size) };
    if result.can_run_again == 0 {
        eprintln!(
            "fatal: in-process LLD cannot safely run again\n{}{}",
            stdout, stderr
        );
        std::process::exit(70);
    }
    let return_code = result.return_code;
    unsafe {
        oscan_lld_dispose_result(&mut result);
    }
    if return_code != 0 {
        let diagnostics = format!("{stdout}{stderr}");
        return Err(format!(
            "in-process LLD failed with exit code {return_code}{}",
            if diagnostics.trim().is_empty() {
                String::new()
            } else {
                format!(":\n{}", diagnostics.trim_end())
            }
        ));
    }
    Ok(())
}

unsafe fn copy_bridge_output(data: *const c_char, size: usize) -> String {
    if data.is_null() || size == 0 {
        return String::new();
    }
    String::from_utf8_lossy(std::slice::from_raw_parts(data.cast::<u8>(), size)).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thin_archives_are_rejected_before_lld_can_follow_members() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("thin.a");
        fs::write(&path, b"!<thin>\n").unwrap();
        let error = read_strict_input(&path).unwrap_err();
        assert!(error.contains("thin archive"), "{error}");
    }

    #[test]
    fn all_embedded_assets_have_nonempty_digests() {
        for asset in assets::ASSETS {
            assert_eq!(asset.sha256.len(), 64, "{asset:?}");
            assert!(!asset.bytes.is_empty(), "{asset:?}");
        }
    }

    #[test]
    fn profile_names_match_embedded_runtime_assets() {
        for profile in [
            FreestandingProfile::Full,
            FreestandingProfile::Graphics,
            FreestandingProfile::Core,
        ] {
            let name = profile.build_mode_str();
            assert!(assets::ASSETS
                .iter()
                .any(|asset| { asset.role == "runtime_archive" && asset.profile == Some(name) }));
        }
    }
}
