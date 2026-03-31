mod ast;
mod codegen;
mod error;
mod lexer;
mod parser;
mod semantic;
mod token;
mod types;

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

const EMBEDDED_RUNTIME_H: &str = include_str!("../runtime/osc_runtime.h");
const EMBEDDED_RUNTIME_C: &str = include_str!("../runtime/osc_runtime.c");
const EMBEDDED_L_OS_H: &str = include_str!("../deps/laststanding/l_os.h");
const EMBEDDED_L_GFX_H: &str = include_str!("../deps/laststanding/l_gfx.h");

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut dump_tokens = false;
    let mut dump_ast = false;
    let mut output_path = None;
    let mut file_path = None;
    let mut run_mode = false;
    let mut emit_c = false;
    let mut use_libc = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--dump-tokens" => dump_tokens = true,
            "--dump-ast" => dump_ast = true,
            "--run" => run_mode = true,
            "--emit-c" => emit_c = true,
            "--libc" => use_libc = true,
            "-o" => {
                i += 1;
                if i < args.len() {
                    output_path = Some(args[i].clone());
                } else {
                    eprintln!("-o requires an output file path");
                    process::exit(1);
                }
            }
            _ if !args[i].starts_with('-') => file_path = Some(args[i].clone()),
            other => {
                eprintln!("unknown flag: {other}");
                process::exit(1);
            }
        }
        i += 1;
    }

    let path = match file_path {
        Some(p) => p,
        None => {
            eprintln!("usage: oscan [--dump-tokens] [--dump-ast] [--run] [--emit-c] [--libc] [-o output] <file.osc>");
            process::exit(1);
        }
    };

    let source = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error reading {path}: {e}");
            process::exit(1);
        }
    };

    // Lex
    let mut lex = lexer::Lexer::new(&source);
    let tokens = match lex.tokenize() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e.with_file(&path));
            process::exit(1);
        }
    };

    if dump_tokens {
        for tok in &tokens {
            println!("{:?}", tok);
        }
    }

    // Parse
    let mut par = parser::Parser::new(tokens);
    let program = match par.parse_program() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e.with_file(&path));
            process::exit(1);
        }
    };

    if dump_ast {
        println!("{:#?}", program);
    }

    // If just dumping, stop here
    if dump_tokens || dump_ast {
        if output_path.is_none() && !run_mode && !emit_c {
            return;
        }
    }

    // Semantic analysis
    let info = match semantic::SemanticAnalyzer::analyze(&program) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("{}", e.with_file(&path));
            process::exit(1);
        }
    };

    // Code generation
    // macOS: l_os.h uses Linux syscalls for Unix; freestanding is not possible on macOS
    let freestanding = if !use_libc && cfg!(target_os = "macos") {
        eprintln!("note: freestanding mode not supported on macOS; using libc mode");
        false
    } else {
        !use_libc
    };
    let c_code = codegen::CodeGenerator::generate(&program, &info, freestanding);

    // Determine the output mode:
    //   --run           → compile and execute
    //   --emit-c        → output C code (to stdout or -o file)
    //   -o foo.c        → output C code to foo.c (extension-based detection)
    //   otherwise       → compile to executable
    let output_ends_in_c = output_path
        .as_ref()
        .map(|p| p.ends_with(".c"))
        .unwrap_or(false);

    if run_mode {
        run_program(&path, &c_code, freestanding);
    } else if emit_c || output_ends_in_c {
        // Emit C mode
        if let Some(out_path) = output_path {
            match fs::write(&out_path, &c_code) {
                Ok(_) => eprintln!("Wrote {}", out_path),
                Err(e) => {
                    eprintln!("error writing {out_path}: {e}");
                    process::exit(1);
                }
            }
        } else {
            println!("{}", c_code);
        }
    } else {
        // Default: compile to executable
        let exe_path = match output_path {
            Some(ref p) => {
                let pb = PathBuf::from(p);
                if cfg!(windows) && pb.extension().is_none() {
                    pb.with_extension("exe")
                } else {
                    pb
                }
            }
            None => {
                let stem = Path::new(&path)
                    .file_stem()
                    .unwrap_or_else(|| std::ffi::OsStr::new("output"))
                    .to_os_string();
                let mut pb = PathBuf::from(stem);
                if cfg!(windows) {
                    pb.set_extension("exe");
                }
                pb
            }
        };
        compile_to_executable(&c_code, &exe_path, freestanding);
    }
}

/// Represents a discovered C compiler with enough info to invoke it.
enum CCompiler {
    Gcc(String),
    Clang(String),
    /// cl.exe path and optional vcvarsall.bat path (needed when cl.exe is not already on PATH)
    Msvc { cl_path: String, vcvars: Option<String> },
}

fn command_exists(cmd: &str) -> bool {
    let check = if cfg!(windows) { "where.exe" } else { "which" };
    Command::new(check)
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Walk up from a cl.exe path to find the sibling vcvarsall.bat.
fn find_vcvarsall_from_cl(cl_path: &str) -> Option<String> {
    let mut dir = Path::new(cl_path);
    while let Some(parent) = dir.parent() {
        if parent.file_name().map(|n| n == "VC").unwrap_or(false) {
            let vcvars = parent.join(r"Auxiliary\Build\vcvarsall.bat");
            if vcvars.exists() {
                return vcvars.to_str().map(|s| s.to_string());
            }
        }
        dir = parent;
    }
    None
}

/// Try to find Clang bundled with Visual Studio.
fn find_vs_clang() -> Option<String> {
    let vswhere = r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe";
    if Path::new(vswhere).exists() {
        let output = Command::new(vswhere)
            .args(["-latest", "-products", "*",
                   "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                   "-property", "installationPath"])
            .output();
        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Some(line) = stdout.lines().filter(|l| !l.trim().is_empty()).last() {
                    let clang_path = format!(r"{}\VC\Tools\Llvm\x64\bin\clang.exe", line.trim());
                    if Path::new(&clang_path).exists() {
                        return Some(clang_path);
                    }
                }
            }
        }
    }
    None
}

/// Try to locate cl.exe via vswhere or well-known Visual Studio paths.
fn find_msvc_cl() -> Option<(String, Option<String>)> {
    // Try vswhere first
    let vswhere = r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe";
    if Path::new(vswhere).exists() {
        let output = Command::new(vswhere)
            .args([
                "-latest", "-products", "*",
                "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-find", r"VC\Tools\MSVC\*\bin\Hostx64\x64\cl.exe",
            ])
            .output();
        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Some(line) = stdout.lines().filter(|l| !l.trim().is_empty()).last() {
                    let cl = line.trim().to_string();
                    if Path::new(&cl).exists() {
                        let vcvars = find_vcvarsall_from_cl(&cl);
                        return Some((cl, vcvars));
                    }
                }
            }
        }
    }

    // Scan well-known installation directories
    let bases = [
        r"C:\Program Files\Microsoft Visual Studio",
        r"C:\Program Files (x86)\Microsoft Visual Studio",
    ];
    for base in &bases {
        for year in &["2022", "2019"] {
            for edition in &["Enterprise", "Professional", "Community", "BuildTools"] {
                let vc_tools = format!(r"{}\{}\{}\VC\Tools\MSVC", base, year, edition);
                if let Ok(entries) = fs::read_dir(&vc_tools) {
                    let mut versions: Vec<_> = entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                        .map(|e| e.path())
                        .collect();
                    versions.sort();
                    if let Some(latest) = versions.last() {
                        let cl = latest.join(r"bin\Hostx64\x64\cl.exe");
                        if cl.exists() {
                            let cl_str = cl.to_str().unwrap().to_string();
                            let vcvars = find_vcvarsall_from_cl(&cl_str);
                            return Some((cl_str, vcvars));
                        }
                    }
                }
            }
        }
    }

    None
}

/// Detect the first available C compiler in priority order:
/// clang (PATH) → VS-bundled clang → gcc → cl.exe (PATH) → cl.exe (VS installation).
/// Clang is preferred for smaller binaries and faster compilation.
fn find_c_compiler() -> Option<CCompiler> {
    if command_exists("clang") {
        return Some(CCompiler::Clang("clang".to_string()));
    }
    if cfg!(windows) {
        if let Some(clang_path) = find_vs_clang() {
            return Some(CCompiler::Clang(clang_path));
        }
    }
    if command_exists("gcc") {
        return Some(CCompiler::Gcc("gcc".to_string()));
    }
    if cfg!(windows) {
        if command_exists("cl.exe") {
            return Some(CCompiler::Msvc { cl_path: "cl.exe".to_string(), vcvars: None });
        }
        if let Some((cl_path, vcvars)) = find_msvc_cl() {
            return Some(CCompiler::Msvc { cl_path, vcvars });
        }
    }
    None
}

/// Find the runtime directory, trying the exe's sibling `runtime/` first,
/// then falling back to a `runtime/` in the current working directory.
/// Returns `None` when no on-disk runtime directory is found (embedded files
/// will be used as a fallback).
fn find_runtime_dir() -> Option<PathBuf> {
    // Try next to the executable first (for installed usage)
    if let Ok(exe) = env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let candidate = exe_dir.join("runtime");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    // Fallback: cwd-relative (dev / project-root usage)
    let cwd = PathBuf::from("runtime");
    if cwd.exists() {
        return Some(cwd);
    }
    None
}

/// Discover extra include directories (e.g. git submodule deps).
/// Returns paths that exist; silently skips missing ones.
fn find_extra_include_dirs(runtime_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // deps/laststanding is a sibling of runtime/
    if let Some(project_root) = runtime_dir.parent() {
        let laststanding = project_root.join("deps").join("laststanding");
        if laststanding.exists() {
            dirs.push(laststanding);
        }
    }
    dirs
}

/// Write C code to a temp file, compile it to `exe_path`, and clean up the temp C file.
fn compile_to_executable(c_code: &str, exe_path: &Path, freestanding: bool) {
    let temp_dir = env::temp_dir().join(format!("oscan_temp_{}", process::id()));
    if let Err(e) = fs::create_dir_all(&temp_dir) {
        eprintln!("error creating temp directory: {e}");
        process::exit(1);
    }

    let c_file = temp_dir.join("program.c");
    if let Err(e) = fs::write(&c_file, c_code) {
        eprintln!("error writing temporary C file: {e}");
        process::exit(1);
    }

    // Write embedded runtime files to temp dir (fallback for distribution mode)
    for (name, content) in [
        ("osc_runtime.h", EMBEDDED_RUNTIME_H),
        ("osc_runtime.c", EMBEDDED_RUNTIME_C),
        ("l_os.h", EMBEDDED_L_OS_H),
        ("l_gfx.h", EMBEDDED_L_GFX_H),
    ] {
        if let Err(e) = fs::write(temp_dir.join(name), content) {
            eprintln!("error writing embedded runtime file {name}: {e}");
            process::exit(1);
        }
    }

    // On-disk runtime dir (dev mode) takes precedence; temp dir is always a fallback
    let mut include_dirs: Vec<PathBuf> = Vec::new();
    let runtime_c = if let Some(ref rd) = find_runtime_dir() {
        include_dirs.push(rd.clone());
        include_dirs.extend(find_extra_include_dirs(rd));
        rd.join("osc_runtime.c")
    } else {
        temp_dir.join("osc_runtime.c")
    };
    include_dirs.push(temp_dir.clone());

    let compiled = invoke_c_compiler(&c_file, exe_path, &runtime_c, &include_dirs, freestanding);
    // Clean up temp files and directory
    let _ = fs::remove_dir_all(&temp_dir);

    if !compiled {
        eprintln!("\nerror: compilation failed");
        process::exit(1);
    }

    eprintln!("Compiled {}", exe_path.display());
}

fn run_program(source_path: &str, c_code: &str, freestanding: bool) {
    let temp_dir = env::temp_dir().join(format!("oscan_temp_{}", process::id()));
    if let Err(e) = fs::create_dir_all(&temp_dir) {
        eprintln!("error creating temp directory: {e}");
        process::exit(1);
    }

    let c_file = temp_dir.join("program.c");
    if let Err(e) = fs::write(&c_file, c_code) {
        eprintln!("error writing temporary C file: {e}");
        process::exit(1);
    }

    // Write embedded runtime files to temp dir (fallback for distribution mode)
    for (name, content) in [
        ("osc_runtime.h", EMBEDDED_RUNTIME_H),
        ("osc_runtime.c", EMBEDDED_RUNTIME_C),
        ("l_os.h", EMBEDDED_L_OS_H),
        ("l_gfx.h", EMBEDDED_L_GFX_H),
    ] {
        if let Err(e) = fs::write(temp_dir.join(name), content) {
            eprintln!("error writing embedded runtime file {name}: {e}");
            process::exit(1);
        }
    }

    let exe_file = if cfg!(windows) {
        temp_dir.join("program.exe")
    } else {
        temp_dir.join("program")
    };

    // On-disk runtime dir (dev mode) takes precedence; temp dir is always a fallback
    let mut include_dirs: Vec<PathBuf> = Vec::new();
    let runtime_c = if let Some(ref rd) = find_runtime_dir() {
        include_dirs.push(rd.clone());
        include_dirs.extend(find_extra_include_dirs(rd));
        rd.join("osc_runtime.c")
    } else {
        temp_dir.join("osc_runtime.c")
    };
    include_dirs.push(temp_dir.clone());

    let compiled = invoke_c_compiler(&c_file, &exe_file, &runtime_c, &include_dirs, freestanding);

    if !compiled {
        eprintln!("\nerror: compilation failed");
        process::exit(1);
    }

    eprintln!("\n=== Running {} ===\n", source_path);
    let status = Command::new(&exe_file).status();

    match status {
        Ok(exit_status) => {
            if !exit_status.success() {
                process::exit(exit_status.code().unwrap_or(1));
            }
        }
        Err(e) => {
            eprintln!("error running program: {e}");
            process::exit(1);
        }
    }
}

/// Detect a C compiler and invoke it. Returns true on success.
fn invoke_c_compiler(c_file: &Path, exe_file: &Path, runtime_c: &Path, include_dirs: &[PathBuf], freestanding: bool) -> bool {
    let compiler = match find_c_compiler() {
        Some(c) => c,
        None => {
            eprintln!("error: no C compiler found");
            eprintln!("please install one of the following:");
            eprintln!("  - GCC:   https://gcc.gnu.org/");
            eprintln!("  - Clang: https://releases.llvm.org/");
            if cfg!(windows) {
                eprintln!("  - Visual Studio Build Tools: https://visualstudio.microsoft.com/visual-cpp-build-tools/");
            }
            process::exit(1);
        }
    };

    match &compiler {
        CCompiler::Gcc(cmd) => {
            // On Windows, MinGW GCC from PATH may not work with l_os.h freestanding;
            // prefer VS-bundled clang which is MSVC-compatible
            if freestanding && cfg!(windows) {
                if let Some(clang_path) = find_vs_clang() {
                    eprintln!("Compiling with VS clang (freestanding)...");
                    return compile_with_gcc_or_clang(&clang_path, c_file, exe_file, runtime_c, include_dirs, freestanding);
                }
            }
            eprintln!("Compiling with gcc{}...", if freestanding { " (freestanding)" } else { "" });
            compile_with_gcc_or_clang(cmd, c_file, exe_file, runtime_c, include_dirs, freestanding)
        }
        CCompiler::Clang(cmd) => {
            // On Windows, clang from PATH (e.g. MSYS2) may lack MSVC compat;
            // prefer VS-bundled clang for freestanding
            if freestanding && cfg!(windows) {
                if let Some(clang_path) = find_vs_clang() {
                    eprintln!("Compiling with VS clang (freestanding)...");
                    return compile_with_gcc_or_clang(&clang_path, c_file, exe_file, runtime_c, include_dirs, freestanding);
                }
            }
            eprintln!("Compiling with clang{}...", if freestanding { " (freestanding)" } else { "" });
            compile_with_gcc_or_clang(cmd, c_file, exe_file, runtime_c, include_dirs, freestanding)
        }
        CCompiler::Msvc { cl_path, vcvars } => {
            if freestanding {
                // Try VS-bundled clang for true freestanding on Windows
                if cfg!(windows) {
                    if let Some(clang_path) = find_vs_clang() {
                        eprintln!("Compiling with clang (freestanding)...");
                        return compile_with_gcc_or_clang(&clang_path, c_file, exe_file, runtime_c, include_dirs, freestanding);
                    }
                }
                eprintln!("note: freestanding mode requires GCC/Clang; falling back to libc mode for MSVC");
            }
            eprintln!("Compiling with MSVC cl.exe...");
            // MSVC always uses libc mode; pass OSC_NOFREESTANDING to select libc headers in dual-mode code
            compile_with_msvc(cl_path, vcvars.as_deref(), c_file, exe_file, runtime_c, include_dirs, false, freestanding)
        }
    }
}

fn compile_with_gcc_or_clang(
    cmd: &str, c_file: &Path, exe_file: &Path, runtime_c: &Path,
    include_dirs: &[PathBuf], freestanding: bool,
) -> bool {
    let mut command = Command::new(cmd);

    if freestanding {
        // Freestanding: single TU (runtime is #included), no libc
        // Use gnu11 for GNU extensions required by l_os.h (register asm, etc.)
        // Size optimization flags matching laststanding build scripts
        command.arg("-std=gnu11").arg("-ffreestanding")
            .arg("-Oz")  // aggressive size optimization
            .arg("-fno-builtin")
            .arg("-fno-asynchronous-unwind-tables")
            .arg("-fomit-frame-pointer")
            .arg("-ffunction-sections")
            .arg("-fdata-sections")
            .arg("-s");  // strip symbols
        if cfg!(windows) {
            // Windows: link kernel32 for Win32 API (VirtualAlloc, WriteFile, etc.)
            command.arg("-lkernel32");
        } else {
            // Unix: fully standalone, no system libraries
            command.arg("-nostdlib").arg("-static")
                .arg("-Wno-builtin-declaration-mismatch")
                .arg("-Wl,--gc-sections,--build-id=none");
        }
        command.arg(c_file);
        for dir in include_dirs {
            command.arg(format!("-I{}", dir.display()));
        }
        command
            .arg("-o")
            .arg(exe_file);
    } else {
        // libc mode: two TUs (generated + runtime), link libc + libm
        command
            .arg("-std=c99")
            .arg(c_file)
            .arg(runtime_c);
        for dir in include_dirs {
            command.arg(format!("-I{}", dir.display()));
        }
        command
            .arg("-o")
            .arg(exe_file);
        // On Windows, math functions live in ucrt (no separate libm);
        // -lm is only needed on Unix.
        if !cfg!(windows) {
            command.arg("-lm");
        }
    }

    let output = command.output();

    match output {
        Ok(out) => {
            if !out.status.success() {
                eprintln!("{cmd} compilation failed:");
                std::io::stderr().write_all(&out.stderr).ok();
                false
            } else {
                true
            }
        }
        Err(e) => {
            eprintln!("failed to run {cmd}: {e}");
            false
        }
    }
}

fn compile_with_msvc(
    cl_path: &str, vcvars: Option<&str>,
    c_file: &Path, exe_file: &Path, runtime_c: &Path,
    include_dirs: &[PathBuf], freestanding: bool, needs_nofreestanding: bool,
) -> bool {
    // When codegen emitted dual-mode headers but we're compiling with MSVC (libc),
    // define OSC_NOFREESTANDING to select the libc path.
    let nofree_flag = if needs_nofreestanding { " /DOSC_NOFREESTANDING" } else { "" };

    if let Some(vcvars_path) = vcvars {
        // cl.exe was found outside PATH – use a temporary .bat file so that
        // vcvarsall.bat can set up the environment in the same cmd session.
        let bat_file = exe_file.with_extension("bat");
        let all_includes: String = include_dirs.iter()
            .map(|d| format!(" /I\"{}\"", d.display()))
            .collect();

        let bat_content = if freestanding {
            // Freestanding: single TU, no CRT, optimize for size
            format!(
                "@echo off\r\ncall \"{}\" x64 >nul 2>&1\r\n\"{}\" /nologo /std:c11 /Os /GS-{}  \"{}\" /Fe:\"{}\" /link /NODEFAULTLIB kernel32.lib\r\n",
                vcvars_path, cl_path,
                all_includes, c_file.display(), exe_file.display(),
            )
        } else {
            // libc mode: two TUs, default CRT
            format!(
                "@echo off\r\ncall \"{}\" x64 >nul 2>&1\r\n\"{}\" /nologo /std:c11{}{}  \"{}\" \"{}\" /Fe:\"{}\" /link\r\n",
                vcvars_path, cl_path, nofree_flag,
                all_includes, c_file.display(), runtime_c.display(), exe_file.display(),
            )
        };

        if let Err(e) = fs::write(&bat_file, &bat_content) {
            eprintln!("failed to write compile script: {e}");
            return false;
        }
        let output = Command::new("cmd").arg("/c").arg(&bat_file).output();
        let _ = fs::remove_file(&bat_file);
        match output {
            Ok(out) => {
                if !out.status.success() {
                    eprintln!("MSVC compilation failed:");
                    std::io::stderr().write_all(&out.stderr).ok();
                    std::io::stderr().write_all(&out.stdout).ok();
                    false
                } else {
                    true
                }
            }
            Err(e) => {
                eprintln!("failed to run cl.exe via vcvarsall: {e}");
                false
            }
        }
    } else {
        // cl.exe already on PATH (e.g. Developer Command Prompt)
        let mut command = Command::new(cl_path);

        if freestanding {
            command
                .arg("/nologo")
                .arg("/std:c11")
                .arg("/Os")  // optimize for size
                .arg("/GS-");
            for dir in include_dirs {
                command.arg(format!("/I{}", dir.display()));
            }
            command
                .arg(c_file)
                .arg(format!("/Fe:{}", exe_file.display()))
                .arg("/link")
                .arg("/NODEFAULTLIB")
                .arg("kernel32.lib");
        } else {
            command
                .arg("/nologo")
                .arg("/std:c11");
            if needs_nofreestanding {
                command.arg("/DOSC_NOFREESTANDING");
            }
            for dir in include_dirs {
                command.arg(format!("/I{}", dir.display()));
            }
            command
                .arg(c_file)
                .arg(runtime_c)
                .arg(format!("/Fe:{}", exe_file.display()))
                .arg("/link");
        }

        let output = command.output();
        match output {
            Ok(out) => {
                if !out.status.success() {
                    eprintln!("MSVC compilation failed:");
                    std::io::stderr().write_all(&out.stderr).ok();
                    std::io::stderr().write_all(&out.stdout).ok();
                    false
                } else {
                    true
                }
            }
            Err(e) => {
                eprintln!("failed to run cl.exe: {e}");
                false
            }
        }
    }
}
