mod ast;
mod codegen;
mod error;
mod lexer;
mod parser;
mod semantic;
mod token;
mod types;

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

const EMBEDDED_RUNTIME_H: &str = include_str!("../runtime/osc_runtime.h");
const EMBEDDED_RUNTIME_C: &str = include_str!("../runtime/osc_runtime.c");
const EMBEDDED_L_OS_H: &str = include_str!("../deps/laststanding/l_os.h");
const EMBEDDED_L_GFX_H: &str = include_str!("../deps/laststanding/l_gfx.h");
const EMBEDDED_L_IMG_H: &str = include_str!("../deps/laststanding/l_img.h");
const EMBEDDED_STB_IMAGE_H: &str = include_str!("../deps/laststanding/stb_image.h");
const EMBEDDED_L_TLS_H: &str = include_str!("../deps/laststanding/l_tls.h");

fn resolve_imports(
    path: &Path,
    visited: &mut HashSet<PathBuf>,
    namespaces: &mut Vec<String>,
) -> Result<String, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {}", path.display(), e))?;
    if !visited.insert(canonical.clone()) {
        return Ok(String::new());
    }
    let source = fs::read_to_string(&canonical)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;

    let mut result = String::new();
    let base_dir = canonical.parent().unwrap_or(Path::new("."));

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("use \"") {
            if let Some(end) = trimmed[5..].find('"') {
                let import_path = &trimmed[5..5 + end];
                let after_quote = trimmed[5 + end + 1..].trim();
                let full_path = base_dir.join(import_path);

                if let Some(ns) = parse_as_clause(after_quote) {
                    // Namespaced import: use a cloned visited set so the same file
                    // can be imported under different namespaces.
                    let mut ns_visited = visited.clone();
                    let mut ns_namespaces = Vec::new();
                    let imported =
                        resolve_imports(&full_path, &mut ns_visited, &mut ns_namespaces)?;
                    let mangled = mangle_top_level_names(&imported, &ns);
                    result.push_str(&mangled);
                    result.push('\n');
                    namespaces.push(ns);
                    namespaces.extend(ns_namespaces);
                    continue;
                } else {
                    // Flat import (existing behavior)
                    let imported = resolve_imports(&full_path, visited, namespaces)?;
                    result.push_str(&imported);
                    result.push('\n');
                    continue;
                }
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    Ok(result)
}

/// Check if `after_quote` is `as <identifier>` and return the namespace name.
fn parse_as_clause(after_quote: &str) -> Option<String> {
    let s = after_quote.strip_prefix("as ")?;
    let ns = s.trim();
    if !ns.is_empty()
        && ns.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && ns.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        Some(ns.to_string())
    } else {
        None
    }
}

/// Find names of top-level declarations (fn, fn!, struct, enum, let) in source text.
/// Top-level declarations are identified by starting at column 0 (no leading whitespace).
fn find_top_level_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let keywords = ["fn ", "fn! ", "struct ", "enum ", "let "];
    for line in source.lines() {
        for kw in &keywords {
            if line.starts_with(kw) {
                let rest = &line[kw.len()..];
                let name_end = rest
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(rest.len());
                let name = &rest[..name_end];
                if !name.is_empty() {
                    names.push(name.to_string());
                }
                break;
            }
        }
    }
    names
}

/// Prefix all top-level declaration names (and all word-bounded references to them)
/// in `source` with `ns_`.
fn mangle_top_level_names(source: &str, ns: &str) -> String {
    let names = find_top_level_names(source);
    let mut result = source.to_string();
    // Sort names longest-first to avoid partial replacement issues
    let mut names_sorted = names;
    names_sorted.sort_by(|a, b| b.len().cmp(&a.len()));
    for name in &names_sorted {
        let new_name = format!("{}_{}", ns, name);
        result = replace_word_boundary(&result, name, &new_name);
    }
    result
}

/// Replace all word-bounded occurrences of `word` with `replacement`.
/// A word boundary means the character before/after is not an identifier character
/// (ASCII alphanumeric or underscore).
fn replace_word_boundary(source: &str, word: &str, replacement: &str) -> String {
    let bytes = source.as_bytes();
    let word_bytes = word.as_bytes();
    let word_len = word_bytes.len();
    let mut result = String::with_capacity(source.len() + source.len() / 4);
    let mut i = 0;

    while i < bytes.len() {
        if i + word_len <= bytes.len() && bytes[i..i + word_len] == *word_bytes {
            let before_ok = i == 0 || !is_ident_char(bytes[i - 1]);
            let after_ok =
                i + word_len >= bytes.len() || !is_ident_char(bytes[i + word_len]);
            if before_ok && after_ok {
                result.push_str(replacement);
                i += word_len;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

/// Replace `ns.ident` with `ns_ident` for namespace-qualified access.
/// Only replaces when `ns` appears as a standalone word followed by `.` and an identifier.
fn replace_ns_dot(source: &str, ns: &str) -> String {
    let ns_dot = format!("{}.", ns);
    let ns_under = format!("{}_", ns);
    let bytes = source.as_bytes();
    let ns_dot_bytes = ns_dot.as_bytes();
    let ns_dot_len = ns_dot_bytes.len();
    let mut result = String::with_capacity(source.len());
    let mut i = 0;

    while i < bytes.len() {
        if i + ns_dot_len < bytes.len()
            && bytes[i..i + ns_dot_len] == *ns_dot_bytes
        {
            let before_ok = i == 0 || !is_ident_char(bytes[i - 1]);
            let after_byte = bytes[i + ns_dot_len];
            let after_ok = after_byte.is_ascii_alphabetic() || after_byte == b'_';
            if before_ok && after_ok {
                result.push_str(&ns_under);
                i += ns_dot_len;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn print_usage(to_stderr: bool) {
    let print_line = |line: &str| {
        if to_stderr {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    };
    print_line(
        "usage: oscan [--help] [-h] [--version] [-V] [--warnings] [-W] [--dump-tokens] [--dump-ast] [--run] [--emit-c] [--libc] [--target <arch>] [-o output] <file.osc>",
    );
    print_line("  --target <arch>  Cross-compile for target (riscv64, wasi)");
    print_line("  OSCAN_CC         Override the detected C compiler command or path");
    print_line(&format!(
        "  OSCAN_TOOLCHAIN_DIR  Bundled toolchain root (default: {})",
        toolchain_search_hint()
    ));
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut dump_tokens = false;
    let mut dump_ast = false;
    let mut output_path = None;
    let mut file_path = None;
    let mut run_mode = false;
    let mut emit_c = false;
    let mut use_libc = false;
    let mut show_warnings = false;
    let mut target: Option<CrossTarget> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--dump-tokens" => dump_tokens = true,
            "--dump-ast" => dump_ast = true,
            "--run" => run_mode = true,
            "--emit-c" => emit_c = true,
            "--libc" => use_libc = true,
            "--warnings" | "-W" => show_warnings = true,
            "--version" | "-V" => {
                println!("oscan {}", env!("GIT_VERSION"));
                return;
            }
            "--help" | "-h" => {
                print_usage(false);
                return;
            }
            "--target" => {
                i += 1;
                let val = args.get(i).cloned().unwrap_or_else(|| {
                    eprintln!("error: --target requires an argument (riscv64, wasi)");
                    process::exit(1);
                });
                target = Some(match val.as_str() {
                    "riscv64" => CrossTarget::RiscV64,
                    "wasi" => CrossTarget::Wasi,
                    other => {
                        eprintln!("error: unknown target '{other}' (supported: riscv64, wasi)");
                        process::exit(1);
                    }
                });
            }
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
            print_usage(true);
            process::exit(1);
        }
    };

    let source = {
        let file_path_buf = PathBuf::from(&path);
        let mut visited = HashSet::new();
        let mut namespaces = Vec::new();
        match resolve_imports(&file_path_buf, &mut visited, &mut namespaces) {
            Ok(s) => {
                let mut s = s;
                for ns in &namespaces {
                    s = replace_ns_dot(&s, ns);
                }
                s
            }
            Err(e) => {
                eprintln!("error: {e}");
                process::exit(1);
            }
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
    let freestanding = match &target {
        Some(CrossTarget::RiscV64) => true,   // RISC-V always freestanding
        Some(CrossTarget::Wasi) => false,      // WASI always libc mode
        None => {
            if !use_libc && cfg!(target_os = "macos") {
                eprintln!("note: freestanding mode not supported on macOS; using libc mode");
                false
            } else {
                !use_libc
            }
        }
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
        if target.is_some() {
            eprintln!("error: --run cannot be used with --target (cross-compiled binaries cannot be executed directly)");
            process::exit(1);
        }
        run_program(&path, &c_code, freestanding, show_warnings);
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
                if pb.extension().is_none() {
                    if matches!(target, Some(CrossTarget::Wasi)) {
                        pb.with_extension("wasm")
                    } else if cfg!(windows) {
                        pb.with_extension("exe")
                    } else {
                        pb
                    }
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
                if matches!(target, Some(CrossTarget::Wasi)) {
                    pb.set_extension("wasm");
                } else if cfg!(windows) {
                    pb.set_extension("exe");
                }
                pb
            }
        };
        compile_to_executable(&c_code, &exe_path, freestanding, target.as_ref(), show_warnings);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompilerSource {
    Override,
    Bundled,
    Host,
}

/// Represents a discovered C compiler with enough info to invoke it.
#[derive(Debug, PartialEq, Eq)]
enum CCompiler {
    Gcc {
        cmd: String,
        source: CompilerSource,
    },
    Clang {
        cmd: String,
        source: CompilerSource,
    },
    /// cl.exe path and optional vcvarsall.bat path (needed when cl.exe is not already on PATH)
    Msvc {
        cl_path: String,
        vcvars: Option<String>,
        source: CompilerSource,
    },
}

#[derive(Clone, Debug)]
enum CrossTarget {
    RiscV64,
    Wasi,
}

fn find_wasi_sysroot() -> Option<String> {
    if let Ok(val) = env::var("WASI_SYSROOT") {
        if Path::new(&val).exists() {
            return Some(val);
        }
    }
    let candidates = [
        "/opt/wasi-sdk/share/wasi-sysroot",
        "/usr/share/wasi-sysroot",
    ];
    for c in &candidates {
        if Path::new(c).exists() {
            return Some(c.to_string());
        }
    }
    None
}

fn command_exists(cmd: &str) -> bool {
    let check = if cfg!(windows) { "where.exe" } else { "which" };
    Command::new(check)
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn env_var_nonempty(name: &str) -> Option<String> {
    let value = env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn compiler_source_label(source: CompilerSource) -> &'static str {
    match source {
        CompilerSource::Override => "override",
        CompilerSource::Bundled => "bundled",
        CompilerSource::Host => "host",
    }
}

fn toolchain_search_hint() -> &'static str {
    if cfg!(windows) {
        "<exe-dir>\\toolchain, then .\\toolchain"
    } else {
        "<exe-dir>/toolchain, then ./toolchain"
    }
}

fn bundled_toolchain_platform() -> Option<&'static str> {
    match env::consts::OS {
        "windows" => Some("windows"),
        "linux" => Some("linux"),
        _ => None,
    }
}

fn resource_dir_candidates(
    resource_name: &str,
    explicit: Option<PathBuf>,
    exe_path: Option<&Path>,
) -> Vec<PathBuf> {
    if let Some(path) = explicit {
        return vec![path];
    }

    let mut candidates = Vec::new();
    if let Some(exe_dir) = exe_path.and_then(|path| path.parent()) {
        candidates.push(exe_dir.join(resource_name));
    }
    candidates.push(PathBuf::from(resource_name));
    candidates
}

fn find_first_existing_dir(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|candidate| candidate.is_dir()).cloned()
}

fn bundled_toolchain_bin_dirs(toolchain_dir: &Path, platform: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if matches!(platform, "windows" | "linux") {
        dirs.push(toolchain_dir.join(platform).join("bin"));
    }
    dirs.push(toolchain_dir.join("bin"));
    dirs
}

fn bundled_compiler_names(platform: &str) -> &'static [&'static str] {
    if platform == "windows" {
        &["clang.exe", "gcc.exe", "cl.exe"]
    } else {
        &["clang", "gcc"]
    }
}

fn bundled_compiler_candidate_paths(toolchain_dir: &Path, platform: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for bin_dir in bundled_toolchain_bin_dirs(toolchain_dir, platform) {
        for compiler in bundled_compiler_names(platform) {
            candidates.push(bin_dir.join(compiler));
        }
    }
    candidates
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

fn compiler_from_command(cmd: String, source: CompilerSource) -> CCompiler {
    let file_name = Path::new(&cmd)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(cmd.as_str())
        .to_ascii_lowercase();

    if file_name == "cl" || file_name == "cl.exe" {
        let vcvars = if Path::new(&cmd).exists() {
            find_vcvarsall_from_cl(&cmd)
        } else {
            None
        };
        CCompiler::Msvc {
            cl_path: cmd,
            vcvars,
            source,
        }
    } else if file_name.contains("clang") {
        CCompiler::Clang { cmd, source }
    } else {
        CCompiler::Gcc { cmd, source }
    }
}

fn compiler_override(value: Option<String>) -> Option<CCompiler> {
    value.map(|cmd| compiler_from_command(cmd, CompilerSource::Override))
}

/// Try to find Clang bundled with Visual Studio.
fn find_vs_clang() -> Option<String> {
    let vswhere = r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe";
    if Path::new(vswhere).exists() {
        let output = Command::new(vswhere)
            .args([
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-property",
                "installationPath",
            ])
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
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-find",
                r"VC\Tools\MSVC\*\bin\Hostx64\x64\cl.exe",
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

fn find_bundled_c_compiler_in_dir(toolchain_dir: &Path, platform: &str) -> Option<CCompiler> {
    for candidate in bundled_compiler_candidate_paths(toolchain_dir, platform) {
        if candidate.is_file() {
            return Some(compiler_from_command(
                candidate.to_string_lossy().into_owned(),
                CompilerSource::Bundled,
            ));
        }
    }
    None
}

fn find_toolchain_dir() -> Option<PathBuf> {
    let exe = env::current_exe().ok();
    let explicit = env_var_nonempty("OSCAN_TOOLCHAIN_DIR").map(PathBuf::from);
    let candidates = resource_dir_candidates("toolchain", explicit, exe.as_deref());
    find_first_existing_dir(&candidates)
}

fn find_host_c_compiler() -> Option<CCompiler> {
    if command_exists("clang") {
        return Some(CCompiler::Clang {
            cmd: "clang".to_string(),
            source: CompilerSource::Host,
        });
    }
    if cfg!(windows) {
        if let Some(clang_path) = find_vs_clang() {
            return Some(CCompiler::Clang {
                cmd: clang_path,
                source: CompilerSource::Host,
            });
        }
    }
    if command_exists("gcc") {
        return Some(CCompiler::Gcc {
            cmd: "gcc".to_string(),
            source: CompilerSource::Host,
        });
    }
    if cfg!(windows) {
        if command_exists("cl.exe") {
            return Some(CCompiler::Msvc {
                cl_path: "cl.exe".to_string(),
                vcvars: None,
                source: CompilerSource::Host,
            });
        }
        if let Some((cl_path, vcvars)) = find_msvc_cl() {
            return Some(CCompiler::Msvc {
                cl_path,
                vcvars,
                source: CompilerSource::Host,
            });
        }
    }
    None
}

/// Detect the first available C compiler in priority order:
/// OSCAN_CC override → bundled toolchain → clang (PATH) → VS-bundled clang →
/// gcc → cl.exe (PATH) → cl.exe (VS installation).
/// Clang is preferred for smaller binaries and faster compilation.
fn find_c_compiler() -> Option<CCompiler> {
    if let Some(override_compiler) = compiler_override(env_var_nonempty("OSCAN_CC")) {
        return Some(override_compiler);
    }
    if let Some(platform) = bundled_toolchain_platform() {
        if let Some(toolchain_dir) = find_toolchain_dir() {
            if let Some(compiler) = find_bundled_c_compiler_in_dir(&toolchain_dir, platform) {
                return Some(compiler);
            }
        }
    }
    find_host_c_compiler()
}

/// Find the runtime directory, trying the exe's sibling `runtime/` first,
/// then falling back to a `runtime/` in the current working directory.
/// Returns `None` when no on-disk runtime directory is found (embedded files
/// will be used as a fallback).
fn find_runtime_dir() -> Option<PathBuf> {
    let exe = env::current_exe().ok();
    let candidates = resource_dir_candidates("runtime", None, exe.as_deref());
    find_first_existing_dir(&candidates)
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
fn compile_to_executable(c_code: &str, exe_path: &Path, freestanding: bool, target: Option<&CrossTarget>, show_warnings: bool) {
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
        ("l_img.h", EMBEDDED_L_IMG_H),
        ("stb_image.h", EMBEDDED_STB_IMAGE_H),
        ("l_tls.h", EMBEDDED_L_TLS_H),
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

    let compiled = invoke_c_compiler(&c_file, exe_path, &runtime_c, &include_dirs, freestanding, target, show_warnings);
    // Clean up temp files and directory
    let _ = fs::remove_dir_all(&temp_dir);

    if !compiled {
        eprintln!("\nerror: compilation failed");
        process::exit(1);
    }

    eprintln!("Compiled {}", exe_path.display());
}

fn run_program(source_path: &str, c_code: &str, freestanding: bool, show_warnings: bool) {
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
        ("l_img.h", EMBEDDED_L_IMG_H),
        ("stb_image.h", EMBEDDED_STB_IMAGE_H),
        ("l_tls.h", EMBEDDED_L_TLS_H),
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

    let compiled = invoke_c_compiler(&c_file, &exe_file, &runtime_c, &include_dirs, freestanding, None, show_warnings);

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
fn invoke_c_compiler(
    c_file: &Path,
    exe_file: &Path,
    runtime_c: &Path,
    include_dirs: &[PathBuf],
    freestanding: bool,
    target: Option<&CrossTarget>,
    show_warnings: bool,
) -> bool {
    // Cross-compilation targets bypass normal compiler detection
    if let Some(ct) = target {
        match ct {
            CrossTarget::RiscV64 => {
                let cmd = "riscv64-linux-gnu-gcc";
                if !command_exists(cmd) {
                    eprintln!("error: {cmd} not found");
                    eprintln!("install with: sudo apt install gcc-riscv64-linux-gnu");
                    process::exit(1);
                }
                eprintln!("Cross-compiling for RISC-V (freestanding)...");
                return compile_cross_riscv64(cmd, c_file, exe_file, include_dirs, show_warnings);
            }
            CrossTarget::Wasi => {
                let cmd = "clang";
                if !command_exists(cmd) {
                    eprintln!("error: clang not found (required for WASI target)");
                    eprintln!("install from: https://releases.llvm.org/");
                    process::exit(1);
                }
                let sysroot = match find_wasi_sysroot() {
                    Some(s) => s,
                    None => {
                        eprintln!("error: WASI sysroot not found");
                        eprintln!("set WASI_SYSROOT environment variable or install wasi-sdk to /opt/wasi-sdk");
                        process::exit(1);
                    }
                };
                eprintln!("Cross-compiling for WASI/WebAssembly...");
                return compile_cross_wasi(cmd, c_file, exe_file, runtime_c, include_dirs, &sysroot, show_warnings);
            }
        }
    }

    let compiler = match find_c_compiler() {
        Some(c) => c,
        None => {
            let bundled_supported = bundled_toolchain_platform().is_some();
            eprintln!("error: no C compiler found");
            eprintln!("searched in this order:");
            eprintln!("  1. OSCAN_CC override");
            if bundled_supported {
                eprintln!(
                    "  2. bundled toolchain under {}",
                    toolchain_search_hint()
                );
                if cfg!(windows) {
                    eprintln!("  3. host compiler detection (clang, VS clang, gcc, cl.exe)");
                } else {
                    eprintln!("  3. host compiler detection (clang, gcc)");
                }
            } else {
                eprintln!("  2. host compiler detection (clang, gcc)");
            }
            eprintln!(
                "configure {} or install one of the following:",
                if bundled_supported {
                    "OSCAN_CC/OSCAN_TOOLCHAIN_DIR"
                } else {
                    "OSCAN_CC"
                }
            );
            eprintln!("  - GCC:   https://gcc.gnu.org/");
            eprintln!("  - Clang: https://releases.llvm.org/");
            if cfg!(windows) {
                eprintln!("  - Visual Studio Build Tools: https://visualstudio.microsoft.com/visual-cpp-build-tools/");
            }
            process::exit(1);
        }
    };

    match &compiler {
        CCompiler::Gcc { cmd, source } => {
            // On Windows, MinGW GCC from PATH may not work with l_os.h freestanding;
            // prefer VS-bundled clang which is MSVC-compatible
            if freestanding && cfg!(windows) && *source == CompilerSource::Host {
                if let Some(clang_path) = find_vs_clang() {
                    eprintln!(
                        "Compiling with VS clang ({}, freestanding)...",
                        compiler_source_label(CompilerSource::Host)
                    );
                    return compile_with_gcc_or_clang(
                        &clang_path,
                        c_file,
                        exe_file,
                        runtime_c,
                        include_dirs,
                        freestanding,
                        CompilerSource::Host,
                        show_warnings,
                    );
                }
            }
            eprintln!(
                "Compiling with gcc ({}){}...",
                compiler_source_label(*source),
                if freestanding { ", freestanding" } else { "" }
            );
            compile_with_gcc_or_clang(
                cmd,
                c_file,
                exe_file,
                runtime_c,
                include_dirs,
                freestanding,
                *source,
                show_warnings,
            )
        }
        CCompiler::Clang { cmd, source } => {
            // On Windows, clang from PATH (e.g. MSYS2) may lack MSVC compat;
            // prefer VS-bundled clang for freestanding
            if freestanding && cfg!(windows) && *source == CompilerSource::Host {
                if let Some(clang_path) = find_vs_clang() {
                    eprintln!(
                        "Compiling with VS clang ({}, freestanding)...",
                        compiler_source_label(CompilerSource::Host)
                    );
                    return compile_with_gcc_or_clang(
                        &clang_path,
                        c_file,
                        exe_file,
                        runtime_c,
                        include_dirs,
                        freestanding,
                        CompilerSource::Host,
                        show_warnings,
                    );
                }
            }
            eprintln!(
                "Compiling with clang ({}){}...",
                compiler_source_label(*source),
                if freestanding { ", freestanding" } else { "" }
            );
            compile_with_gcc_or_clang(
                cmd,
                c_file,
                exe_file,
                runtime_c,
                include_dirs,
                freestanding,
                *source,
                show_warnings,
            )
        }
        CCompiler::Msvc {
            cl_path,
            vcvars,
            source,
        } => {
            if freestanding {
                // Try VS-bundled clang for true freestanding on Windows
                if cfg!(windows) && *source == CompilerSource::Host {
                    if let Some(clang_path) = find_vs_clang() {
                        eprintln!(
                            "Compiling with clang ({}, freestanding)...",
                            compiler_source_label(CompilerSource::Host)
                        );
                        return compile_with_gcc_or_clang(
                            &clang_path,
                            c_file,
                            exe_file,
                            runtime_c,
                            include_dirs,
                            freestanding,
                            CompilerSource::Host,
                            show_warnings,
                        );
                    }
                }
                eprintln!("note: freestanding mode requires GCC/Clang; falling back to libc mode for MSVC");
            }
            eprintln!(
                "Compiling with MSVC cl.exe ({})...",
                compiler_source_label(*source)
            );
            // MSVC always uses libc mode; pass OSC_NOFREESTANDING to select libc headers in dual-mode code
            compile_with_msvc(
                cl_path,
                vcvars.as_deref(),
                c_file,
                exe_file,
                runtime_c,
                include_dirs,
                false,
                freestanding,
                show_warnings,
            )
        }
    }
}

fn compile_with_gcc_or_clang(
    cmd: &str,
    c_file: &Path,
    exe_file: &Path,
    runtime_c: &Path,
    include_dirs: &[PathBuf],
    freestanding: bool,
    source: CompilerSource,
    show_warnings: bool,
) -> bool {
    let mut command = Command::new(cmd);

    if freestanding {
        // Freestanding: single TU (runtime is #included), no libc
        // Use gnu11 for GNU extensions required by l_os.h (register asm, etc.)
        // Size optimization flags matching laststanding build scripts
        command
            .arg("-std=gnu11")
            .arg("-ffreestanding");
        if !show_warnings {
            command.arg("-w");
        }
        command
            .arg("-Oz") // aggressive size optimization
            .arg("-fno-builtin")
            .arg("-fno-asynchronous-unwind-tables")
            .arg("-fomit-frame-pointer")
            .arg("-ffunction-sections")
            .arg("-fdata-sections")
            .arg("-s"); // strip symbols
        if cfg!(windows) {
            // Windows: link core Win32, sockets, GDI windowing, and TLS support.
            command
                .arg("-lkernel32")
                .arg("-lws2_32")
                .arg("-luser32")
                .arg("-lgdi32")
                .arg("-lsecur32")
                .arg("-lcrypt32");
            if source == CompilerSource::Bundled {
                command
                    .arg("-nostartfiles")
                    .arg("-Wl,--gc-sections,--build-id=none");
            }
        } else {
            // Unix: fully standalone, no system libraries
            command
                .arg("-nostdlib")
                .arg("-static")
                .arg("-Wl,--gc-sections,--build-id=none");
        }
        command.arg(c_file);
        for dir in include_dirs {
            command.arg(format!("-I{}", dir.display()));
        }
        command.arg("-o").arg(exe_file);
    } else {
        // libc mode: two TUs (generated + runtime), link libc + libm
        command.arg("-std=c99");
        if !show_warnings {
            command.arg("-w");
        }
        command.arg(c_file).arg(runtime_c);
        for dir in include_dirs {
            command.arg(format!("-I{}", dir.display()));
        }
        command.arg("-o").arg(exe_file);
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
                if show_warnings && !out.stderr.is_empty() {
                    std::io::stderr().write_all(&out.stderr).ok();
                }
                true
            }
        }
        Err(e) => {
            eprintln!("failed to run {cmd}: {e}");
            false
        }
    }
}

fn compile_cross_riscv64(
    cmd: &str,
    c_file: &Path,
    exe_file: &Path,
    include_dirs: &[PathBuf],
    show_warnings: bool,
) -> bool {
    let mut command = Command::new(cmd);
    // Freestanding single-TU build for RISC-V 64-bit
    command
        .arg("-std=gnu11")
        .arg("-ffreestanding");
    if !show_warnings {
        command.arg("-w");
    }
    command
        .arg("-Os") // RISC-V gcc doesn't support -Oz
        .arg("-fno-builtin")
        .arg("-fno-asynchronous-unwind-tables")
        .arg("-fomit-frame-pointer")
        .arg("-ffunction-sections")
        .arg("-fdata-sections")
        .arg("-s")
        .arg("-march=rv64gc")
        .arg("-mabi=lp64d")
        .arg("-nostdlib")
        .arg("-static")
        .arg("-Wl,--gc-sections,--build-id=none");
    command.arg(c_file);
    for dir in include_dirs {
        command.arg(format!("-I{}", dir.display()));
    }
    command.arg("-o").arg(exe_file);

    match command.output() {
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

fn compile_cross_wasi(
    cmd: &str,
    c_file: &Path,
    exe_file: &Path,
    runtime_c: &Path,
    include_dirs: &[PathBuf],
    sysroot: &str,
    show_warnings: bool,
) -> bool {
    let mut command = Command::new(cmd);
    // WASI libc mode: two TUs, wasm32 target
    command
        .arg("--target=wasm32-wasi")
        .arg(format!("--sysroot={}", sysroot))
        .arg("-std=c99");
    if !show_warnings {
        command.arg("-w");
    }
    command
        .arg(c_file)
        .arg(runtime_c);
    for dir in include_dirs {
        command.arg(format!("-I{}", dir.display()));
    }
    command.arg("-o").arg(exe_file);

    match command.output() {
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
    cl_path: &str,
    vcvars: Option<&str>,
    c_file: &Path,
    exe_file: &Path,
    runtime_c: &Path,
    include_dirs: &[PathBuf],
    freestanding: bool,
    needs_nofreestanding: bool,
    show_warnings: bool,
) -> bool {
    // When codegen emitted dual-mode headers but we're compiling with MSVC (libc),
    // define OSC_NOFREESTANDING to select the libc path.
    let nofree_flag = if needs_nofreestanding {
        " /DOSC_NOFREESTANDING"
    } else {
        ""
    };
    let warn_flag = if show_warnings { "" } else { " /w" };

    if let Some(vcvars_path) = vcvars {
        // cl.exe was found outside PATH – use a temporary .bat file so that
        // vcvarsall.bat can set up the environment in the same cmd session.
        let bat_file = exe_file.with_extension("bat");
        let all_includes: String = include_dirs
            .iter()
            .map(|d| format!(" /I\"{}\"", d.display()))
            .collect();

        let bat_content = if freestanding {
            // Freestanding: single TU, no CRT, optimize for size
            format!(
                "@echo off\r\ncall \"{}\" x64 >nul 2>&1\r\n\"{}\" /nologo{} /std:c11 /Os /GS-{}  \"{}\" /Fe:\"{}\" /link /NODEFAULTLIB kernel32.lib ws2_32.lib secur32.lib crypt32.lib\r\n",
                vcvars_path, cl_path, warn_flag,
                all_includes, c_file.display(), exe_file.display(),
            )
        } else {
            // libc mode: two TUs, default CRT
            format!(
                "@echo off\r\ncall \"{}\" x64 >nul 2>&1\r\n\"{}\" /nologo{} /std:c11{}{}  \"{}\" \"{}\" /Fe:\"{}\" /link\r\n",
                vcvars_path, cl_path, warn_flag, nofree_flag,
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
            command.arg("/nologo");
            if !show_warnings {
                command.arg("/w");
            }
            command
                .arg("/std:c11")
                .arg("/Os") // optimize for size
                .arg("/GS-");
            for dir in include_dirs {
                command.arg(format!("/I{}", dir.display()));
            }
            command
                .arg(c_file)
                .arg(format!("/Fe:{}", exe_file.display()))
                .arg("/link")
                .arg("/NODEFAULTLIB")
                .arg("kernel32.lib")
                .arg("ws2_32.lib")
                .arg("secur32.lib")
                .arg("crypt32.lib");
        } else {
            command.arg("/nologo");
            if !show_warnings {
                command.arg("/w");
            }
            command.arg("/std:c11");
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let id = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-scratch")
                .join(format!("{}_{}_{}", name, process::id(), id));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn compiler_override_detects_requested_compiler() {
        assert_eq!(
            compiler_override(Some("custom-clang".to_string())),
            Some(CCompiler::Clang {
                cmd: "custom-clang".to_string(),
                source: CompilerSource::Override,
            })
        );
        assert_eq!(
            compiler_override(Some("x86_64-linux-gnu-gcc".to_string())),
            Some(CCompiler::Gcc {
                cmd: "x86_64-linux-gnu-gcc".to_string(),
                source: CompilerSource::Override,
            })
        );
        assert_eq!(
            compiler_override(Some("cl.exe".to_string())),
            Some(CCompiler::Msvc {
                cl_path: "cl.exe".to_string(),
                vcvars: None,
                source: CompilerSource::Override,
            })
        );
    }

    #[test]
    fn resource_dir_candidates_prefer_explicit_override() {
        let override_dir = PathBuf::from("custom-toolchain");
        let exe = PathBuf::from("install").join("oscan");

        assert_eq!(
            resource_dir_candidates("toolchain", Some(override_dir.clone()), Some(&exe)),
            vec![override_dir]
        );
    }

    #[test]
    fn toolchain_candidates_mirror_runtime_search_order() {
        let exe = PathBuf::from("install").join("oscan");
        let candidates = resource_dir_candidates("toolchain", None, Some(&exe));

        assert_eq!(
            candidates,
            vec![PathBuf::from("install").join("toolchain"), PathBuf::from("toolchain")]
        );
    }

    #[test]
    fn bundled_windows_compiler_candidates_include_platform_and_generic_bins() {
        let root = PathBuf::from("toolchain");

        assert_eq!(
            bundled_compiler_candidate_paths(&root, "windows"),
            vec![
                root.join("windows").join("bin").join("clang.exe"),
                root.join("windows").join("bin").join("gcc.exe"),
                root.join("windows").join("bin").join("cl.exe"),
                root.join("bin").join("clang.exe"),
                root.join("bin").join("gcc.exe"),
                root.join("bin").join("cl.exe"),
            ]
        );
    }

    #[test]
    fn bundled_linux_compiler_candidates_include_platform_and_generic_bins() {
        let root = PathBuf::from("toolchain");

        assert_eq!(
            bundled_compiler_candidate_paths(&root, "linux"),
            vec![
                root.join("linux").join("bin").join("clang"),
                root.join("linux").join("bin").join("gcc"),
                root.join("bin").join("clang"),
                root.join("bin").join("gcc"),
            ]
        );
    }

    #[test]
    fn bundled_compiler_detection_prefers_platform_specific_bin() {
        let test_dir = TestDir::new("bundled-platform");
        let toolchain = test_dir.path.join("toolchain");
        let platform_clang = toolchain.join("windows").join("bin").join("clang.exe");
        let generic_gcc = toolchain.join("bin").join("gcc.exe");

        fs::create_dir_all(platform_clang.parent().unwrap()).unwrap();
        fs::create_dir_all(generic_gcc.parent().unwrap()).unwrap();
        fs::write(&platform_clang, []).unwrap();
        fs::write(&generic_gcc, []).unwrap();

        assert_eq!(
            find_bundled_c_compiler_in_dir(&toolchain, "windows"),
            Some(CCompiler::Clang {
                cmd: platform_clang.to_string_lossy().into_owned(),
                source: CompilerSource::Bundled,
            })
        );
    }

    #[test]
    fn bundled_compiler_detection_falls_back_to_generic_bin() {
        let test_dir = TestDir::new("bundled-generic");
        let toolchain = test_dir.path.join("toolchain");
        let generic_gcc = toolchain.join("bin").join("gcc");

        fs::create_dir_all(generic_gcc.parent().unwrap()).unwrap();
        fs::write(&generic_gcc, []).unwrap();

        assert_eq!(
            find_bundled_c_compiler_in_dir(&toolchain, "linux"),
            Some(CCompiler::Gcc {
                cmd: generic_gcc.to_string_lossy().into_owned(),
                source: CompilerSource::Bundled,
            })
        );
    }

    #[test]
    fn compiler_source_labels_are_stable_for_release_smoke_tests() {
        assert_eq!(compiler_source_label(CompilerSource::Override), "override");
        assert_eq!(compiler_source_label(CompilerSource::Bundled), "bundled");
        assert_eq!(compiler_source_label(CompilerSource::Host), "host");
    }
}
