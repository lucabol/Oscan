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

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut dump_tokens = false;
    let mut dump_ast = false;
    let mut output_path = None;
    let mut file_path = None;
    let mut run_mode = false;
    let mut emit_c = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--dump-tokens" => dump_tokens = true,
            "--dump-ast" => dump_ast = true,
            "--run" => run_mode = true,
            "--emit-c" => emit_c = true,
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
            eprintln!("usage: oscan [--dump-tokens] [--dump-ast] [--run] [--emit-c] [-o output] <file.osc>");
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
    let c_code = codegen::CodeGenerator::generate(&program, &info);

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
        run_program(&path, &c_code);
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
        compile_to_executable(&c_code, &exe_path);
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
/// gcc → clang → cl.exe (PATH) → cl.exe (Visual Studio installation).
fn find_c_compiler() -> Option<CCompiler> {
    if command_exists("gcc") {
        return Some(CCompiler::Gcc("gcc".to_string()));
    }
    if command_exists("clang") {
        return Some(CCompiler::Clang("clang".to_string()));
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
fn find_runtime_dir() -> PathBuf {
    // Try next to the executable first (for installed usage)
    if let Ok(exe) = env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let candidate = exe_dir.join("runtime");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    // Fallback: cwd-relative (dev / project-root usage)
    let cwd = PathBuf::from("runtime");
    if cwd.exists() {
        return cwd;
    }
    eprintln!("error: runtime directory not found");
    eprintln!("make sure you're running from the project root or that runtime/ is next to the binary");
    process::exit(1);
}

/// Write C code to a temp file, compile it to `exe_path`, and clean up the temp C file.
fn compile_to_executable(c_code: &str, exe_path: &Path) {
    let temp_dir = env::temp_dir().join("oscan_temp");
    if let Err(e) = fs::create_dir_all(&temp_dir) {
        eprintln!("error creating temp directory: {e}");
        process::exit(1);
    }

    let c_file = temp_dir.join("program.c");
    if let Err(e) = fs::write(&c_file, c_code) {
        eprintln!("error writing temporary C file: {e}");
        process::exit(1);
    }

    let runtime_dir = find_runtime_dir();
    let runtime_c = runtime_dir.join("osc_runtime.c");
    let runtime_h = runtime_dir.join("osc_runtime.h");
    if !runtime_c.exists() || !runtime_h.exists() {
        eprintln!("error: runtime files not found in {}", runtime_dir.display());
        process::exit(1);
    }

    let compiled = invoke_c_compiler(&c_file, exe_path, &runtime_c, &runtime_dir);
    // Clean up temp C file
    let _ = fs::remove_file(&c_file);

    if !compiled {
        eprintln!("\nerror: compilation failed");
        process::exit(1);
    }

    eprintln!("Compiled {}", exe_path.display());
}

fn run_program(source_path: &str, c_code: &str) {
    let temp_dir = env::temp_dir().join("oscan_temp");
    if let Err(e) = fs::create_dir_all(&temp_dir) {
        eprintln!("error creating temp directory: {e}");
        process::exit(1);
    }

    let c_file = temp_dir.join("program.c");
    if let Err(e) = fs::write(&c_file, c_code) {
        eprintln!("error writing temporary C file: {e}");
        process::exit(1);
    }

    let exe_file = if cfg!(windows) {
        temp_dir.join("program.exe")
    } else {
        temp_dir.join("program")
    };

    let runtime_dir = find_runtime_dir();
    let runtime_c = runtime_dir.join("osc_runtime.c");
    let runtime_h = runtime_dir.join("osc_runtime.h");
    if !runtime_c.exists() || !runtime_h.exists() {
        eprintln!("error: runtime files not found in {}", runtime_dir.display());
        process::exit(1);
    }

    let compiled = invoke_c_compiler(&c_file, &exe_file, &runtime_c, &runtime_dir);

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
fn invoke_c_compiler(c_file: &Path, exe_file: &Path, runtime_c: &Path, runtime_dir: &Path) -> bool {
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
            eprintln!("Compiling with gcc...");
            compile_with_gcc_or_clang(cmd, c_file, exe_file, runtime_c, runtime_dir)
        }
        CCompiler::Clang(cmd) => {
            eprintln!("Compiling with clang...");
            compile_with_gcc_or_clang(cmd, c_file, exe_file, runtime_c, runtime_dir)
        }
        CCompiler::Msvc { cl_path, vcvars } => {
            eprintln!("Compiling with MSVC cl.exe...");
            compile_with_msvc(cl_path, vcvars.as_deref(), c_file, exe_file, runtime_c, runtime_dir)
        }
    }
}

fn compile_with_gcc_or_clang(
    cmd: &str, c_file: &Path, exe_file: &Path, runtime_c: &Path, runtime_dir: &Path,
) -> bool {
    let output = Command::new(cmd)
        .arg("-std=c99")
        .arg(c_file)
        .arg(runtime_c)
        .arg(format!("-I{}", runtime_dir.display()))
        .arg("-o")
        .arg(exe_file)
        .arg("-lm")
        .output();

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
    c_file: &Path, exe_file: &Path, runtime_c: &Path, runtime_dir: &Path,
) -> bool {
    if let Some(vcvars_path) = vcvars {
        // cl.exe was found outside PATH – use a temporary .bat file so that
        // vcvarsall.bat can set up the environment in the same cmd session.
        // (Embedding quotes inside `cmd /c "..."` from Rust is unreliable
        //  because Rust escapes `"` as `\"` which cmd.exe does not understand.)
        let bat_file = exe_file.with_extension("bat");
        let bat_content = format!(
            "@echo off\r\ncall \"{}\" x64 >nul 2>&1\r\n\"{}\" /nologo /std:c11 /I\"{}\" \"{}\" \"{}\" /Fe:\"{}\" /link\r\n",
            vcvars_path, cl_path,
            runtime_dir.display(), c_file.display(), runtime_c.display(), exe_file.display(),
        );
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
        let output = Command::new(cl_path)
            .arg("/nologo")
            .arg("/std:c11")
            .arg(format!("/I{}", runtime_dir.display()))
            .arg(c_file)
            .arg(runtime_c)
            .arg(format!("/Fe:{}", exe_file.display()))
            .arg("/link")
            .output();
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
