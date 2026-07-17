mod ast;
mod backend;
mod codegen;
mod error;
mod ir;
mod lexer;
mod lower;
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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub(crate) fn is_verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// Log a Command's full command line when --verbose is active.
fn verbose_command(label: &str, cmd: &Command) {
    if is_verbose() {
        let prog = cmd.get_program().to_string_lossy();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        eprintln!("[verbose] {label}: {prog} {}", args.join(" "));
    }
}

const EMBEDDED_RUNTIME_H: &str = include_str!("../runtime/osc_runtime.h");
const EMBEDDED_RUNTIME_C: &str = include_str!("../runtime/osc_runtime.c");
const EMBEDDED_L_OS_H: &str = include_str!("../deps/laststanding/l_os.h");
const EMBEDDED_L_GFX_H: &str = include_str!("../deps/laststanding/l_gfx.h");
const EMBEDDED_L_IMG_H: &str = include_str!("../deps/laststanding/l_img.h");
const EMBEDDED_STB_IMAGE_H: &str = include_str!("../deps/laststanding/stb_image.h");
const EMBEDDED_L_SVG_H: &str = include_str!("../deps/laststanding/l_svg.h");
const EMBEDDED_COMPAT_MATH_H: &str = include_str!("../deps/laststanding/compat/math.h");
const EMBEDDED_NANOSVG_H: &str = include_str!("../deps/laststanding/compat/nanosvg/nanosvg.h");
const EMBEDDED_NANOSVGRAST_H: &str =
    include_str!("../deps/laststanding/compat/nanosvg/nanosvgrast.h");
const EMBEDDED_L_TLS_H: &str = include_str!("../deps/laststanding/l_tls.h");
const EMBEDDED_L_TT_H: &str = include_str!("../deps/laststanding/l_tt.h");
const EMBEDDED_STB_TRUETYPE_H: &str = include_str!("../deps/laststanding/stb_truetype.h");

// BearSSL public headers (for l_tls.h on Linux)
const EMBEDDED_BEARSSL_H: &str = include_str!("../deps/laststanding/bearssl/inc/bearssl.h");
const EMBEDDED_BEARSSL_HASH_H: &str =
    include_str!("../deps/laststanding/bearssl/inc/bearssl_hash.h");
const EMBEDDED_BEARSSL_HMAC_H: &str =
    include_str!("../deps/laststanding/bearssl/inc/bearssl_hmac.h");
const EMBEDDED_BEARSSL_KDF_H: &str = include_str!("../deps/laststanding/bearssl/inc/bearssl_kdf.h");
const EMBEDDED_BEARSSL_RAND_H: &str =
    include_str!("../deps/laststanding/bearssl/inc/bearssl_rand.h");
const EMBEDDED_BEARSSL_PRF_H: &str = include_str!("../deps/laststanding/bearssl/inc/bearssl_prf.h");
const EMBEDDED_BEARSSL_BLOCK_H: &str =
    include_str!("../deps/laststanding/bearssl/inc/bearssl_block.h");
const EMBEDDED_BEARSSL_AEAD_H: &str =
    include_str!("../deps/laststanding/bearssl/inc/bearssl_aead.h");
const EMBEDDED_BEARSSL_RSA_H: &str = include_str!("../deps/laststanding/bearssl/inc/bearssl_rsa.h");
const EMBEDDED_BEARSSL_EC_H: &str = include_str!("../deps/laststanding/bearssl/inc/bearssl_ec.h");
const EMBEDDED_BEARSSL_SSL_H: &str = include_str!("../deps/laststanding/bearssl/inc/bearssl_ssl.h");
const EMBEDDED_BEARSSL_X509_H: &str =
    include_str!("../deps/laststanding/bearssl/inc/bearssl_x509.h");
const EMBEDDED_BEARSSL_PEM_H: &str = include_str!("../deps/laststanding/bearssl/inc/bearssl_pem.h");

const BEARSSL_HEADERS: &[(&str, &str)] = &[
    ("bearssl.h", EMBEDDED_BEARSSL_H),
    ("bearssl_hash.h", EMBEDDED_BEARSSL_HASH_H),
    ("bearssl_hmac.h", EMBEDDED_BEARSSL_HMAC_H),
    ("bearssl_kdf.h", EMBEDDED_BEARSSL_KDF_H),
    ("bearssl_rand.h", EMBEDDED_BEARSSL_RAND_H),
    ("bearssl_prf.h", EMBEDDED_BEARSSL_PRF_H),
    ("bearssl_block.h", EMBEDDED_BEARSSL_BLOCK_H),
    ("bearssl_aead.h", EMBEDDED_BEARSSL_AEAD_H),
    ("bearssl_rsa.h", EMBEDDED_BEARSSL_RSA_H),
    ("bearssl_ec.h", EMBEDDED_BEARSSL_EC_H),
    ("bearssl_ssl.h", EMBEDDED_BEARSSL_SSL_H),
    ("bearssl_x509.h", EMBEDDED_BEARSSL_X509_H),
    ("bearssl_pem.h", EMBEDDED_BEARSSL_PEM_H),
];

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
            let after_ok = i + word_len >= bytes.len() || !is_ident_char(bytes[i + word_len]);
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
        if i + ns_dot_len < bytes.len() && bytes[i..i + ns_dot_len] == *ns_dot_bytes {
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
        "usage: oscan [--help] [-h] [--version] [-V] [--verbose] [--warnings] [-W] [--dump-tokens] [--dump-ast] [--run] [--emit-c] [--libc] [--backend c|native] [--native-target <tag>] [--target <arch>] [--extra-c <file.c>] [--extra-cflags <flag>] [--extra-obj <file.o|.obj>] [--extra-lib <file.a|.lib>] [-o output] <file.osc>",
    );
    print_line("  --emit-c        Emit C-backend source to stdout (or use -o file.c)");
    print_line("  --libc           Use the hosted libc runtime (including with --backend native)");
    print_line("  --target <arch>  Cross-compile for target (riscv64, wasi) — C backend only");
    print_line("  --backend c|native  Backend (default: native on supported hosts; c otherwise)");
    print_line("    c       Portability/reference, C source, macOS, and WASI backend");
    print_line("    native  Direct object code for supported Windows and Linux hosts/targets");
    print_line(&format!(
        "  --native-target <tag>  Native backend target: {} (default: host)",
        backend::NativeTarget::accepted_values()
    ));
    print_line("  --extra-c <file> Extra C source file to compile and link (repeatable)");
    print_line("  --extra-cflags <flag>  Extra flag passed to the C compiler (repeatable)");
    print_line("  --extra-obj <file>  Precompiled object file to link (.o/.obj, repeatable)");
    print_line("  --extra-lib <file>  Precompiled static library to link (.a/.lib, repeatable)");
    print_line("  OSCAN_CC         Override the detected C compiler command or path");
    print_line(&format!(
        "  OSCAN_TOOLCHAIN_DIR  Bundled toolchain root (default: {})",
        toolchain_search_hint()
    ));
    print_line(
        "  OSCAN_NATIVE_LINKER  Override the linker used by --backend native (Windows freestanding: a ld.lld-compatible binary; Linux freestanding: a GNU ld-compatible binary; otherwise a compiler driver)",
    );
    print_line(
        "  OSCAN_NATIVE_LINKER_FLAVOR  How to invoke OSCAN_NATIVE_LINKER/the default native linker: 'mingw' (direct ld.lld, Windows), 'elf' (direct GNU ld, Linux), or 'compiler-driver' (legacy)",
    );
    print_line(
        "  OSCAN_NATIVE_ASSET_CACHE_DIR  Override where extracted embedded native-link assets are cached (default: %LOCALAPPDATA%\\oscan\\native-assets\\ on Windows, $XDG_CACHE_HOME/oscan/native-assets on Linux)",
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    C,
    Native,
}

fn resolve_backend(
    explicit_backend: Option<Backend>,
    c_source_output: bool,
    c_cross_target: bool,
    native_target_requested: bool,
    native_host_supported: bool,
) -> Backend {
    explicit_backend.unwrap_or_else(|| {
        if c_source_output || c_cross_target {
            Backend::C
        } else if native_target_requested || native_host_supported {
            Backend::Native
        } else {
            Backend::C
        }
    })
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
    let mut verbose = false;
    let mut target: Option<CrossTarget> = None;
    let mut explicit_backend = None;
    let mut native_target_arg: Option<String> = None;
    let mut program_args: Vec<String> = Vec::new();
    let mut extra_c_files: Vec<String> = Vec::new();
    let mut extra_cflags: Vec<String> = Vec::new();
    let mut extra_obj_files: Vec<String> = Vec::new();
    let mut extra_lib_files: Vec<String> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--dump-tokens" => dump_tokens = true,
            "--dump-ast" => dump_ast = true,
            "--run" => run_mode = true,
            "--emit-c" => emit_c = true,
            "--libc" => use_libc = true,
            "--warnings" | "-W" => show_warnings = true,
            "--verbose" => verbose = true,
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
            "--backend" => {
                i += 1;
                let val = args.get(i).cloned().unwrap_or_else(|| {
                    eprintln!("error: --backend requires an argument (c, native)");
                    process::exit(1);
                });
                explicit_backend = Some(match val.as_str() {
                    "c" => Backend::C,
                    "native" => Backend::Native,
                    other => {
                        eprintln!("error: unknown backend '{other}' (supported: c, native)");
                        process::exit(1);
                    }
                });
            }
            "--native-target" => {
                i += 1;
                let val = args.get(i).cloned().unwrap_or_else(|| {
                    eprintln!(
                        "error: --native-target requires an argument ({})",
                        backend::NativeTarget::accepted_values()
                    );
                    process::exit(1);
                });
                native_target_arg = Some(val);
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
            "--extra-c" => {
                i += 1;
                if i < args.len() {
                    extra_c_files.push(args[i].clone());
                } else {
                    eprintln!("--extra-c requires a C source file path");
                    process::exit(1);
                }
            }
            "--extra-cflags" => {
                i += 1;
                if i < args.len() {
                    extra_cflags.push(args[i].clone());
                } else {
                    eprintln!("--extra-cflags requires a compiler flag");
                    process::exit(1);
                }
            }
            "--extra-obj" => {
                i += 1;
                if i < args.len() {
                    extra_obj_files.push(args[i].clone());
                } else {
                    eprintln!("--extra-obj requires an object file path (.o/.obj)");
                    process::exit(1);
                }
            }
            "--extra-lib" => {
                i += 1;
                if i < args.len() {
                    extra_lib_files.push(args[i].clone());
                } else {
                    eprintln!("--extra-lib requires a static library path (.a/.lib)");
                    process::exit(1);
                }
            }
            _ if !args[i].starts_with('-') => {
                if file_path.is_none() {
                    file_path = Some(args[i].clone());
                } else {
                    // Extra positional args → program arguments for --run
                    program_args.push(args[i].clone());
                }
            }
            other => {
                eprintln!("unknown flag: {other}");
                process::exit(1);
            }
        }
        i += 1;
    }

    let output_ends_in_c = output_path
        .as_ref()
        .map(|path| path.ends_with(".c"))
        .unwrap_or(false);
    let backend_kind = resolve_backend(
        explicit_backend,
        emit_c || output_ends_in_c,
        target.is_some(),
        native_target_arg.is_some(),
        backend::NativeTarget::try_host().is_ok(),
    );

    // Backend-selection validation. `--target` (C-backend cross-compile
    // to riscv64/wasi) and `--native-target` (native-backend target
    // selection) are deliberately separate flags with non-overlapping
    // meanings — see `print_usage` — so using either with the wrong
    // backend is a user error, not something to silently ignore.
    if backend_kind == Backend::Native {
        if target.is_some() {
            eprintln!("error: --target is only supported with --backend c; use --native-target for the native backend");
            process::exit(1);
        }
        if emit_c {
            eprintln!(
                "error: --emit-c requires the C portability/reference backend (--backend c); \
                 the native backend produces object code"
            );
            process::exit(1);
        }
        if output_ends_in_c {
            eprintln!(
                "error: C source output (-o *.c) requires the C portability/reference backend \
                 (--backend c); the native backend produces object code"
            );
            process::exit(1);
        }
    } else if native_target_arg.is_some() {
        eprintln!("error: --native-target requires --backend native");
        process::exit(1);
    }
    let native_target = if backend_kind == Backend::Native {
        Some(match native_target_arg.as_deref().unwrap_or("host") {
            // Both "no --native-target given" and an explicit
            // "--native-target host" mean the same thing — auto-detect
            // this machine — and must fail the same clean way (naming
            // the unsupported host, never silently defaulting to some
            // other target) when this host's OS/architecture isn't one
            // the native backend supports; see `NativeTarget::try_host`.
            "host" => backend::NativeTarget::try_host().unwrap_or_else(|e| {
                eprintln!("error: {e}");
                process::exit(1);
            }),
            val => backend::NativeTarget::parse(val).unwrap_or_else(|| {
                eprintln!(
                    "error: unknown --native-target '{val}' (supported: {})",
                    backend::NativeTarget::accepted_values()
                );
                process::exit(1);
            }),
        })
    } else {
        None
    };

    let path = match file_path {
        Some(p) => p,
        None => {
            print_usage(true);
            process::exit(1);
        }
    };

    // Set global verbose flag so all functions can use is_verbose()
    VERBOSE.store(verbose, Ordering::Relaxed);

    let source = {
        if is_verbose() {
            eprintln!("[verbose] Resolving imports for {}", path);
        }
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
    if is_verbose() {
        eprintln!("[verbose] Lexing...");
    }
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
    if is_verbose() {
        eprintln!("[verbose] Parsing...");
    }
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
    if is_verbose() {
        eprintln!("[verbose] Semantic analysis...");
    }
    let info = match semantic::SemanticAnalyzer::analyze(&program) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("{}", e.with_file(&path));
            process::exit(1);
        }
    };

    // Code generation
    let ir_program = lower::lower_program(&program, &info);
    if let Err(errors) = ir::verify(&ir_program) {
        for e in &errors {
            eprintln!("internal compiler error: {}", e);
        }
        process::exit(1);
    }

    if let Some(native_target) = native_target {
        let runtime_mode = if use_libc {
            backend::RuntimeMode::Hosted
        } else {
            backend::RuntimeMode::Freestanding
        };
        run_native_backend(
            &ir_program,
            native_target,
            runtime_mode,
            &path,
            output_path,
            run_mode,
            show_warnings,
            &program_args,
            &extra_c_files,
            &extra_cflags,
            &extra_obj_files,
            &extra_lib_files,
        );
        return;
    }

    // macOS: l_os.h uses Linux syscalls for Unix; freestanding is not possible on macOS
    let freestanding = match &target {
        Some(CrossTarget::RiscV64) => true, // RISC-V always freestanding
        Some(CrossTarget::Wasi) => false,   // WASI always libc mode
        None => {
            if !use_libc && cfg!(target_os = "macos") {
                eprintln!("note: freestanding mode not supported on macOS; using libc mode");
                false
            } else {
                !use_libc
            }
        }
    };

    let c_code = codegen::CodeGenerator::generate(&ir_program, freestanding);
    if is_verbose() {
        eprintln!(
            "[verbose] Generated C code ({} bytes, freestanding={})",
            c_code.len(),
            freestanding
        );
    }

    // Determine the output mode:
    //   --run           → compile and execute
    //   --emit-c        → output C code (to stdout or -o file)
    //   -o foo.c        → output C code to foo.c (extension-based detection)
    //   otherwise       → compile to executable
    if run_mode {
        if target.is_some() {
            eprintln!("error: --run cannot be used with --target (cross-compiled binaries cannot be executed directly)");
            process::exit(1);
        }
        run_program(
            &path,
            &c_code,
            freestanding,
            show_warnings,
            &program_args,
            &extra_c_files,
            &extra_cflags,
            &extra_obj_files,
            &extra_lib_files,
        );
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
        compile_to_executable(
            &c_code,
            &exe_path,
            freestanding,
            target.as_ref(),
            show_warnings,
            &extra_c_files,
            &extra_cflags,
            &extra_obj_files,
            &extra_lib_files,
        );
    }
}

/// Drives the Cranelift native backend end to end: object emission,
/// with `-o *.o`/`-o *.obj` short-circuiting to just the object file,
/// otherwise linking into a standalone executable (and, for
/// `--run`, executing it). Never touches `crate::codegen`/the C backend —
/// see `src/backend/mod.rs`'s module docs on why that would be a "silent
/// fallback" this backend deliberately never performs.
/// Create a private, unpredictable scratch directory for one `--backend
/// native` compile invocation (Finding 3 hardening, security review): the
/// old `env::temp_dir().join(format!("oscan_native_{}", process::id()))`
/// was predictable (bare PID), and on multi-user Unix systems
/// `env::temp_dir()` is typically world-writable (`/tmp`) — a local
/// attacker could pre-create/symlink that exact path ahead of time, and in
/// `--run` mode race to substitute the compiled executable before it's
/// launched. `tempfile::Builder` instead creates an atomically-named
/// directory with a cryptographically-random suffix.
///
/// Security review 2026-07-15 (finding 3): on Unix, permissions are no
/// longer left to `tempfile`'s own internal default alone — this
/// explicitly calls [`harden_native_scratch_dir_unix`] immediately after
/// creation and **fails the build** (propagates the `io::Error`, handled
/// by this function's one caller as a fatal `error creating temp
/// directory: ...` + `process::exit(1)`) if the mode cannot be set to
/// exactly `0700`, rather than silently trusting a best-effort default
/// (verified explicitly by `native_scratch_dir_has_0700_permissions_on_unix`
/// below).
fn create_native_scratch_dir() -> std::io::Result<tempfile::TempDir> {
    let dir = tempfile::Builder::new().prefix("oscan_native_").tempdir()?;
    #[cfg(unix)]
    harden_native_scratch_dir_unix(dir.path())?;
    Ok(dir)
}

/// Unix-only: explicitly set the native-backend scratch directory's mode
/// to `0700` (owner-only), and propagate any failure to do so as a hard
/// error rather than a silent best-effort attempt (security review
/// 2026-07-15, finding 3).
#[cfg(unix)]
fn harden_native_scratch_dir_unix(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn run_native_backend(
    ir_program: &ir::Program,
    native_target: backend::NativeTarget,
    runtime_mode: backend::RuntimeMode,
    source_path: &str,
    output_path: Option<String>,
    run_mode: bool,
    show_warnings: bool,
    program_args: &[String],
    extra_c_files: &[String],
    extra_cflags: &[String],
    extra_obj_files: &[String],
    extra_lib_files: &[String],
) {
    if is_verbose() {
        eprintln!("[verbose] Native backend target: {native_target}, runtime: {runtime_mode}");
    }
    let object_bytes = match backend::compile_object(ir_program, native_target, runtime_mode) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("{}", e.with_file(source_path));
            process::exit(1);
        }
    };
    if is_verbose() {
        eprintln!(
            "[verbose] Native object emitted ({} bytes)",
            object_bytes.len()
        );
    }

    let output_is_object = output_path
        .as_ref()
        .map(|p| {
            let lower = p.to_ascii_lowercase();
            lower.ends_with(".o") || lower.ends_with(".obj")
        })
        .unwrap_or(false);

    if output_is_object {
        if run_mode {
            eprintln!(
                "error: --run cannot be combined with an object-file output path (-o *.o/*.obj)"
            );
            process::exit(1);
        }
        let out_path = output_path.unwrap();
        if let Err(e) = backend::link::write_object_file(&object_bytes, Path::new(&out_path)) {
            eprintln!("error: {e}");
            process::exit(1);
        }
        eprintln!("Wrote {out_path}");
        return;
    }

    // Security review 2026-07-15 (findings 2 & 3): refuse a native final
    // link (or `--run`, which requires one) entirely while this process is
    // running elevated on Windows, *before* creating any scratch directory
    // or touching the native-asset cache. This replaces last round's
    // "route an elevated process to a fresh, isolated directory" mitigation
    // — Windows handle-based TOCTOU races between a path check and a
    // subsequent open/rename are not fully closed by re-checking paths,
    // however carefully, so this no longer tries to sandbox an elevated
    // process, it refuses the operation outright. `output_is_object` is
    // never gated (handled and returned above): that path never extracts
    // or executes an embedded asset, or writes a final linked executable.
    // Unix gets a separate, unrelated fix instead (explicit scratch-dir
    // permissions, see `harden_native_scratch_dir_unix`) — elevation in the
    // Windows Administrator sense has no Unix equivalent gated here.
    #[cfg(windows)]
    {
        let elevation = backend::native_assets::is_elevated();
        if let Err(reason) = backend::native_assets::check_elevation_policy(
            elevation,
            backend::native_assets::NativeLinkOperation::FinalLink,
        ) {
            eprintln!("error: {reason}");
            process::exit(1);
        }
    }

    #[cfg(unix)]
    {
        let elevation = backend::native_assets::is_setuid_elevated();
        if let Err(reason) = backend::native_assets::check_elevation_policy(
            elevation,
            backend::native_assets::NativeLinkOperation::FinalLink,
        ) {
            eprintln!("error: {reason}");
            process::exit(1);
        }
    }

    let link_options = backend::link::NativeLinkOptions {
        runtime_mode,
        show_warnings,
        extra_c_files: &extra_c_files,
        extra_cflags: &extra_cflags,
        extra_objects: &extra_obj_files,
        extra_libs: &extra_lib_files,
    };

    // Validate --extra-obj files (design §12.2)
    for obj_file in extra_obj_files {
        let path = std::path::Path::new(obj_file);
        if !path.exists() {
            eprintln!("error: --extra-obj file does not exist: {}", obj_file);
            process::exit(1);
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if !ext.eq_ignore_ascii_case("o") && !ext.eq_ignore_ascii_case("obj") {
            eprintln!(
                "warning: --extra-obj file does not have .o or .obj extension: {}",
                obj_file
            );
        }
    }

    // Validate --extra-lib files (design §12.2)
    for lib_file in extra_lib_files {
        let path = std::path::Path::new(lib_file);
        if !path.exists() {
            eprintln!("error: --extra-lib file does not exist: {}", lib_file);
            process::exit(1);
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if !ext.eq_ignore_ascii_case("a") && !ext.eq_ignore_ascii_case("lib") {
            eprintln!(
                "warning: --extra-lib file does not have .a or .lib extension: {}",
                lib_file
            );
        }
    }

    let temp_dir = match create_native_scratch_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("error creating temp directory: {e}");
            process::exit(1);
        }
    };
    let obj_path = temp_dir
        .path()
        .join(format!("program{}", native_target.obj_suffix()));
    if let Err(e) = backend::link::write_object_file(&object_bytes, &obj_path) {
        eprintln!("error: {e}");
        // `process::exit` skips `Drop`, so the `TempDir` guard must be
        // dropped explicitly first to still clean up (Finding 3).
        drop(temp_dir);
        process::exit(1);
    }

    if run_mode {
        let exe_path = temp_dir
            .path()
            .join(format!("program{}", native_target.exe_suffix()));
        if let Err(e) =
            backend::link::link_executable(&obj_path, &exe_path, native_target, &link_options)
        {
            eprintln!("error: {e}");
            drop(temp_dir);
            process::exit(1);
        }
        eprintln!("\n=== Running {} ===\n", source_path);
        // Keep the `TempDir` guard alive through the run itself — the
        // compiled executable lives inside it.
        let status = Command::new(&exe_path).args(program_args).status();
        drop(temp_dir);
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
        return;
    }

    let exe_path = match output_path {
        Some(p) => {
            let pb = PathBuf::from(p);
            if pb.extension().is_none() {
                pb.with_extension(native_target.exe_suffix().trim_start_matches('.'))
            } else {
                pb
            }
        }
        None => {
            let stem = Path::new(source_path)
                .file_stem()
                .unwrap_or_else(|| std::ffi::OsStr::new("output"))
                .to_os_string();
            let mut pb = PathBuf::from(stem);
            let suffix = native_target.exe_suffix().trim_start_matches('.');
            if !suffix.is_empty() {
                pb.set_extension(suffix);
            }
            pb
        }
    };
    let result = backend::link::link_executable(&obj_path, &exe_path, native_target, &link_options);
    drop(temp_dir);
    if let Err(e) = result {
        eprintln!("error: {e}");
        process::exit(1);
    }
    eprintln!("Compiled {}", exe_path.display());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompilerSource {
    Override,
    Bundled,
    Host,
}

/// Represents a discovered C compiler with enough info to invoke it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CCompiler {
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

pub(crate) fn command_exists(cmd: &str) -> bool {
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

pub(crate) fn compiler_source_label(source: CompilerSource) -> &'static str {
    match source {
        CompilerSource::Override => "override",
        CompilerSource::Bundled => "bundled",
        CompilerSource::Host => "host",
    }
}

/// The invocable command for `compiler`, only when it is GCC or Clang
/// (`None` for MSVC): used by the native (Cranelift) backend's linker
/// discovery in `src/backend/link.rs`, which drives GCC/Clang as a linker
/// front-end for `.o`/`.a` inputs rather than `cl.exe`/`link.exe` (see that
/// module for why).
pub(crate) fn gcc_or_clang_cmd(compiler: &CCompiler) -> Option<(&str, CompilerSource)> {
    match compiler {
        CCompiler::Gcc { cmd, source } | CCompiler::Clang { cmd, source } => {
            Some((cmd.as_str(), *source))
        }
        CCompiler::Msvc { .. } => None,
    }
}

/// Finding 2b (security review): the bundled toolchain lookup no longer
/// has a bare CWD-relative fallback (only `<exe-dir>/toolchain`, or the
/// explicit `OSCAN_TOOLCHAIN_DIR` override) — an executed compiler binary
/// must never be resolved against whatever directory `oscan` happened to
/// be launched from.
fn toolchain_search_hint() -> &'static str {
    if cfg!(windows) {
        "<exe-dir>\\toolchain (never the current directory)"
    } else {
        "<exe-dir>/toolchain (never the current directory)"
    }
}

fn bundled_toolchain_platform() -> Option<&'static str> {
    match env::consts::OS {
        "windows" => Some("windows"),
        "linux" => Some("linux"),
        _ => None,
    }
}

/// `include_cwd` gates the bare `PathBuf::from(resource_name)` candidate,
/// which resolves against this process's current working directory: it
/// must be `false` for any resource that ends up **executed** (e.g. the
/// bundled-toolchain compiler binary looked up by [`find_toolchain_dir`]) —
/// a malicious/incidental CWD must never be able to substitute a planted
/// executable there. It remains `true` for resources that are only ever
/// read as **data** (e.g. [`find_runtime_dir`]'s `runtime/` C sources),
/// which is out of scope for this hardening pass.
fn resource_dir_candidates(
    resource_name: &str,
    explicit: Option<PathBuf>,
    exe_path: Option<&Path>,
    include_cwd: bool,
) -> Vec<PathBuf> {
    if let Some(path) = explicit {
        return vec![path];
    }

    let mut candidates = Vec::new();
    if let Some(exe_dir) = exe_path.and_then(|path| path.parent()) {
        candidates.push(exe_dir.join(resource_name));
    }
    if include_cwd {
        candidates.push(PathBuf::from(resource_name));
    }
    candidates
}

fn find_first_existing_dir(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|candidate| candidate.is_dir())
        .cloned()
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
            // Verify the binary can actually execute (musl-native binaries
            // can't run on glibc systems — they hang instead of erroring).
            let cmd_str = candidate.to_string_lossy().into_owned();
            if !compiler_can_execute(&cmd_str) {
                if is_verbose() {
                    eprintln!(
                        "[verbose] Bundled compiler {} exists but can't execute — skipping",
                        cmd_str
                    );
                }
                continue;
            }
            return Some(compiler_from_command(cmd_str, CompilerSource::Bundled));
        }
    }
    None
}

/// Quick sanity check: can the compiler actually run on this system?
/// Returns false if the binary exists but can't execute (e.g. musl-native
/// binary on a glibc system).
fn compiler_can_execute(cmd: &str) -> bool {
    use std::time::Duration;
    // Use a short timeout — a working compiler responds to --version instantly.
    // A musl binary on glibc hangs forever, so we kill it after 3 seconds.
    let child = Command::new(cmd)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match child {
        Ok(mut child) => {
            // Wait up to 3 seconds for the process to complete
            let start = std::time::Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => return status.success(),
                    Ok(None) => {
                        if start.elapsed() > Duration::from_secs(3) {
                            let _ = child.kill();
                            let _ = child.wait();
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => return false,
                }
            }
        }
        Err(_) => false,
    }
}

pub(crate) fn find_toolchain_dir() -> Option<PathBuf> {
    let exe = env::current_exe().ok();
    let explicit = env_var_nonempty("OSCAN_TOOLCHAIN_DIR").map(PathBuf::from);
    // include_cwd = false: this resolves to an *executed* compiler binary
    // (see `resource_dir_candidates`'s docs) -- a bare `./toolchain`
    // relative to whatever directory `oscan` happened to be launched from
    // must never be trusted (Finding 2b).
    let candidates = resource_dir_candidates("toolchain", explicit, exe.as_deref(), false);
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
pub(crate) fn find_c_compiler() -> Option<CCompiler> {
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
    // include_cwd = true: unchanged, out-of-scope behavior for this pass —
    // `runtime/` is only ever read as data, never executed (see
    // `resource_dir_candidates`'s docs).
    let candidates = resource_dir_candidates("runtime", None, exe.as_deref(), true);
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

/// Search for libbearssl.a in multiple locations:
/// 1. Include dirs (deps/laststanding/bearssl/build/ for dev-built copy)
/// 2. packaging/prebuilt/<host-triple>/ (committed prebuilt for dev/CI)
/// 3. <exe-dir>/toolchain/lib/ (release bundle layout)
/// 4. <exe-dir>/lib/ (minimal install layout)
fn find_bearssl_lib(include_dirs: &[PathBuf]) -> Option<String> {
    // Dev mode: locally built copy under deps/laststanding/bearssl/build/
    for dir in include_dirs {
        let lib = dir.join("bearssl").join("build").join("libbearssl.a");
        if lib.exists() {
            return Some(lib.display().to_string());
        }
    }
    // Dev/CI mode: committed prebuilt under packaging/prebuilt/<host-triple>/
    // include_dirs typically contains <project_root>/deps/laststanding; walk up
    // two levels to reach the project root.
    let prebuilt_subdir = format!("{}-{}", env::consts::OS, env::consts::ARCH);
    for dir in include_dirs {
        if let Some(project_root) = dir.parent().and_then(|p| p.parent()) {
            let lib = project_root
                .join("packaging")
                .join("prebuilt")
                .join(&prebuilt_subdir)
                .join("libbearssl.a");
            if lib.exists() {
                return Some(lib.display().to_string());
            }
        }
    }
    // Release bundle: search relative to the oscan binary
    if let Ok(exe) = env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            for sub in &["toolchain/lib", "lib"] {
                let lib = exe_dir.join(sub).join("libbearssl.a");
                if lib.exists() {
                    return Some(lib.display().to_string());
                }
            }
        }
    }
    None
}

/// A temp-directory path unique to this invocation. Combining the process
/// ID with a monotonic per-process counter and a high-resolution
/// timestamp avoids a real (if narrow) collision window: Windows reuses a
/// just-exited process's PID for a newly spawned process almost
/// immediately, so `oscan_temp_<pid>` alone can collide when many short-lived
/// `oscan` invocations run back-to-back in quick succession (as the
/// differential backend-oracle test harness does) — one process's
/// `fs::remove_dir_all` cleanup racing a same-PID successor's
/// `fs::create_dir_all` of the identical path intermittently manifests as
/// spurious "path not found"/"access denied" I/O errors.
fn unique_temp_dir_name() -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("oscan_temp_{}_{}_{}", process::id(), nanos, seq)
}

/// Write C code to a temp file, compile it to `exe_path`, and clean up the temp C file.
fn compile_to_executable(
    c_code: &str,
    exe_path: &Path,
    freestanding: bool,
    target: Option<&CrossTarget>,
    show_warnings: bool,
    extra_c_files: &[String],
    extra_cflags: &[String],
    extra_obj_files: &[String],
    extra_lib_files: &[String],
) {
    let temp_dir = env::temp_dir().join(unique_temp_dir_name());
    if is_verbose() {
        eprintln!("[verbose] Creating temp dir: {}", temp_dir.display());
    }
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
        ("l_svg.h", EMBEDDED_L_SVG_H),
        ("l_tls.h", EMBEDDED_L_TLS_H),
        ("l_tt.h", EMBEDDED_L_TT_H),
        ("stb_truetype.h", EMBEDDED_STB_TRUETYPE_H),
    ] {
        if let Err(e) = fs::write(temp_dir.join(name), content) {
            eprintln!("error writing embedded runtime file {name}: {e}");
            process::exit(1);
        }
    }

    // Write compat/ files for l_svg.h (math shims + NanoSVG fork)
    {
        let compat_dir = temp_dir.join("compat");
        let _ = fs::create_dir_all(&compat_dir);
        if let Err(e) = fs::write(compat_dir.join("math.h"), EMBEDDED_COMPAT_MATH_H) {
            eprintln!("error writing compat/math.h: {e}");
            process::exit(1);
        }
        let nanosvg_dir = compat_dir.join("nanosvg");
        let _ = fs::create_dir_all(&nanosvg_dir);
        for (name, content) in [
            ("nanosvg.h", EMBEDDED_NANOSVG_H),
            ("nanosvgrast.h", EMBEDDED_NANOSVGRAST_H),
        ] {
            if let Err(e) = fs::write(nanosvg_dir.join(name), content) {
                eprintln!("error writing compat/nanosvg/{name}: {e}");
                process::exit(1);
            }
        }
    }

    // Write BearSSL public headers to bearssl/inc/ subdirectory (for l_tls.h)
    let bearssl_inc_dir = temp_dir.join("bearssl").join("inc");
    let _ = fs::create_dir_all(&bearssl_inc_dir);
    for (name, content) in BEARSSL_HEADERS {
        if let Err(e) = fs::write(bearssl_inc_dir.join(name), content) {
            eprintln!("error writing BearSSL header {name}: {e}");
            process::exit(1);
        }
    }

    // On-disk runtime dir (dev mode) takes precedence; temp dir is always a fallback
    let mut include_dirs: Vec<PathBuf> = Vec::new();
    let runtime_c = if let Some(ref rd) = find_runtime_dir() {
        if is_verbose() {
            eprintln!("[verbose] Using on-disk runtime: {}", rd.display());
        }
        include_dirs.push(rd.clone());
        include_dirs.extend(find_extra_include_dirs(rd));
        rd.join("osc_runtime.c")
    } else {
        if is_verbose() {
            eprintln!("[verbose] Using embedded runtime files");
        }
        temp_dir.join("osc_runtime.c")
    };
    include_dirs.push(temp_dir.clone());
    if is_verbose() {
        eprintln!(
            "[verbose] Include dirs: {:?}",
            include_dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
        );
        if let Some(lib) = find_bearssl_lib(&include_dirs) {
            eprintln!("[verbose] Found libbearssl.a: {}", lib);
        } else {
            eprintln!("[verbose] libbearssl.a not found (TLS will use stubs on Linux)");
        }
    }

    if is_verbose() {
        eprintln!("[verbose] Invoking C compiler...");
    }
    let compiled = invoke_c_compiler(
        &c_file,
        exe_path,
        &runtime_c,
        &include_dirs,
        freestanding,
        target,
        show_warnings,
        extra_c_files,
        extra_cflags,
        extra_obj_files,
        extra_lib_files,
    );
    // Clean up temp files and directory
    let _ = fs::remove_dir_all(&temp_dir);

    if !compiled {
        eprintln!("\nerror: compilation failed");
        process::exit(1);
    }

    eprintln!("Compiled {}", exe_path.display());
}

fn run_program(
    source_path: &str,
    c_code: &str,
    freestanding: bool,
    show_warnings: bool,
    program_args: &[String],
    extra_c_files: &[String],
    extra_cflags: &[String],
    extra_obj_files: &[String],
    extra_lib_files: &[String],
) {
    let temp_dir = env::temp_dir().join(unique_temp_dir_name());
    if is_verbose() {
        eprintln!("[verbose] Creating temp dir: {}", temp_dir.display());
    }
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
        ("l_svg.h", EMBEDDED_L_SVG_H),
        ("l_tls.h", EMBEDDED_L_TLS_H),
        ("l_tt.h", EMBEDDED_L_TT_H),
        ("stb_truetype.h", EMBEDDED_STB_TRUETYPE_H),
    ] {
        if let Err(e) = fs::write(temp_dir.join(name), content) {
            eprintln!("error writing embedded runtime file {name}: {e}");
            process::exit(1);
        }
    }

    // Write compat/ files for l_svg.h (math shims + NanoSVG fork)
    let compat_dir = temp_dir.join("compat");
    let _ = fs::create_dir_all(&compat_dir);
    if let Err(e) = fs::write(compat_dir.join("math.h"), EMBEDDED_COMPAT_MATH_H) {
        eprintln!("error writing compat/math.h: {e}");
        process::exit(1);
    }
    let nanosvg_dir = compat_dir.join("nanosvg");
    let _ = fs::create_dir_all(&nanosvg_dir);
    for (name, content) in [
        ("nanosvg.h", EMBEDDED_NANOSVG_H),
        ("nanosvgrast.h", EMBEDDED_NANOSVGRAST_H),
    ] {
        if let Err(e) = fs::write(nanosvg_dir.join(name), content) {
            eprintln!("error writing compat/nanosvg/{name}: {e}");
            process::exit(1);
        }
    }

    // Write BearSSL public headers to bearssl/inc/ subdirectory (for l_tls.h)
    let bearssl_inc_dir = temp_dir.join("bearssl").join("inc");
    let _ = fs::create_dir_all(&bearssl_inc_dir);
    for (name, content) in BEARSSL_HEADERS {
        if let Err(e) = fs::write(bearssl_inc_dir.join(name), content) {
            eprintln!("error writing BearSSL header {name}: {e}");
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
        if is_verbose() {
            eprintln!("[verbose] Using on-disk runtime: {}", rd.display());
        }
        include_dirs.push(rd.clone());
        include_dirs.extend(find_extra_include_dirs(rd));
        rd.join("osc_runtime.c")
    } else {
        if is_verbose() {
            eprintln!("[verbose] Using embedded runtime files");
        }
        temp_dir.join("osc_runtime.c")
    };
    include_dirs.push(temp_dir.clone());
    if is_verbose() {
        eprintln!(
            "[verbose] Include dirs: {:?}",
            include_dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
        );
    }

    if is_verbose() {
        eprintln!("[verbose] Invoking C compiler (run mode)...");
    }
    let compiled = invoke_c_compiler(
        &c_file,
        &exe_file,
        &runtime_c,
        &include_dirs,
        freestanding,
        None,
        show_warnings,
        extra_c_files,
        extra_cflags,
        extra_obj_files,
        extra_lib_files,
    );

    if !compiled {
        eprintln!("\nerror: compilation failed");
        process::exit(1);
    }

    eprintln!("\n=== Running {} ===\n", source_path);
    let status = Command::new(&exe_file).args(program_args).status();

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
    extra_c_files: &[String],
    extra_cflags: &[String],
    extra_obj_files: &[String],
    extra_lib_files: &[String],
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
                return compile_cross_riscv64(
                    cmd,
                    c_file,
                    exe_file,
                    include_dirs,
                    show_warnings,
                    extra_c_files,
                    extra_cflags,
                    extra_obj_files,
                    extra_lib_files,
                );
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
                return compile_cross_wasi(
                    cmd,
                    c_file,
                    exe_file,
                    runtime_c,
                    include_dirs,
                    &sysroot,
                    show_warnings,
                    extra_c_files,
                    extra_cflags,
                    extra_obj_files,
                    extra_lib_files,
                );
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
                eprintln!("  2. bundled toolchain under {}", toolchain_search_hint());
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
                        extra_c_files,
                        extra_cflags,
                        extra_obj_files,
                        extra_lib_files,
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
                extra_c_files,
                extra_cflags,
                extra_obj_files,
                extra_lib_files,
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
                        extra_c_files,
                        extra_cflags,
                        extra_obj_files,
                        extra_lib_files,
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
                extra_c_files,
                extra_cflags,
                extra_obj_files,
                extra_lib_files,
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
                            extra_c_files,
                            extra_cflags,
                            extra_obj_files,
                            extra_lib_files,
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
                extra_c_files,
                extra_cflags,
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
    extra_c_files: &[String],
    extra_cflags: &[String],
    extra_obj_files: &[String],
    extra_lib_files: &[String],
) -> bool {
    let mut command = Command::new(cmd);

    if freestanding {
        // Freestanding: single TU (runtime is #included), no libc
        // Use gnu11 for GNU extensions required by l_os.h (register asm, etc.)
        // Size optimization flags matching laststanding build scripts
        command.arg("-std=gnu11").arg("-ffreestanding");
        if !show_warnings {
            command.arg("-w");
        }
        // BearSSL headers use memcpy in inline functions after l_tls.h undefs
        // the macro alias. Clang C99+ treats implicit function declarations as
        // errors. Downgrade to warning so the __asm__ linker shims still work.
        if cmd.contains("clang") {
            command.arg("-Wno-error=implicit-function-declaration");
        }
        // -Oz is Clang-only; GCC uses -Os for size optimization
        let size_opt = if cmd.contains("clang") { "-Oz" } else { "-Os" };
        command
            .arg(size_opt)
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
        // Link libbearssl.a AFTER source file — linker processes left-to-right,
        // so the archive must come after the object that references its symbols.
        if !cfg!(windows) {
            if let Some(lib) = find_bearssl_lib(include_dirs) {
                command.arg(&lib);
            }
        }
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
        if cfg!(windows) {
            // The hosted runtime still exposes sockets, graphics, and TLS.
            // Unlike the freestanding Laststanding translation unit, these
            // wrappers are compiled separately and their Win32 imports must
            // be supplied explicitly to Clang/GCC.
            command
                .arg("-lws2_32")
                .arg("-luser32")
                .arg("-lgdi32")
                .arg("-lsecur32")
                .arg("-lcrypt32");
        } else {
            // On Windows, math functions live in ucrt (no separate libm).
            command.arg("-lm");
        }
    }

    // Add user-supplied extra C source files and compiler flags
    for f in extra_c_files {
        command.arg(f);
    }
    // Add user-supplied precompiled object files (design §12.6)
    for obj in extra_obj_files {
        command.arg(obj);
    }
    for flag in extra_cflags {
        command.arg(flag);
    }
    // Add user-supplied precompiled static libraries (design §12.6)
    // These go last per spec: after all other inputs
    for lib in extra_lib_files {
        command.arg(lib);
    }

    verbose_command("C compiler", &command);
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
    extra_c_files: &[String],
    extra_cflags: &[String],
    extra_obj_files: &[String],
    extra_lib_files: &[String],
) -> bool {
    let mut command = Command::new(cmd);
    // Freestanding single-TU build for RISC-V 64-bit
    command.arg("-std=gnu11").arg("-ffreestanding");
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

    for f in extra_c_files {
        command.arg(f);
    }
    // Add user-supplied precompiled object files (design §12.6)
    for obj in extra_obj_files {
        command.arg(obj);
    }
    for flag in extra_cflags {
        command.arg(flag);
    }
    // Add user-supplied precompiled static libraries (design §12.6)
    for lib in extra_lib_files {
        command.arg(lib);
    }

    verbose_command("RISC-V cross-compile", &command);
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
    extra_c_files: &[String],
    extra_cflags: &[String],
    extra_obj_files: &[String],
    extra_lib_files: &[String],
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
    command.arg(c_file).arg(runtime_c);
    for dir in include_dirs {
        command.arg(format!("-I{}", dir.display()));
    }
    command.arg("-o").arg(exe_file);

    for f in extra_c_files {
        command.arg(f);
    }
    // Add user-supplied precompiled object files (design §12.6)
    for obj in extra_obj_files {
        command.arg(obj);
    }
    for flag in extra_cflags {
        command.arg(flag);
    }
    // Add user-supplied precompiled static libraries (design §12.6)
    for lib in extra_lib_files {
        command.arg(lib);
    }

    verbose_command("WASI cross-compile", &command);
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
    extra_c_files: &[String],
    extra_cflags: &[String],
) -> bool {
    // When codegen emitted dual-mode headers but we're compiling with MSVC (libc),
    // define OSC_NOFREESTANDING to select the libc path.
    let nofree_flag = if needs_nofreestanding {
        " /DOSC_NOFREESTANDING"
    } else {
        ""
    };
    let warn_flag = if show_warnings { "" } else { " /w" };

    // Build extra files/flags strings for MSVC
    let extra_c_str: String = extra_c_files
        .iter()
        .map(|f| format!(" \"{}\"", f))
        .collect();
    let extra_flags_str: String = extra_cflags.iter().map(|f| format!(" {}", f)).collect();

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
                "@echo off\r\ncall \"{}\" x64 >nul 2>&1\r\n\"{}\" /nologo{} /std:c11 /Os /GS-{}  \"{}\"{}{} /Fe:\"{}\" /link /NODEFAULTLIB kernel32.lib ws2_32.lib secur32.lib crypt32.lib\r\n",
                vcvars_path, cl_path, warn_flag,
                all_includes, c_file.display(), extra_c_str, extra_flags_str, exe_file.display(),
            )
        } else {
            // libc mode: two TUs, default CRT
            format!(
                "@echo off\r\ncall \"{}\" x64 >nul 2>&1\r\n\"{}\" /nologo{} /std:c11{}{}  \"{}\" \"{}\"{}{} /Fe:\"{}\" /link\r\n",
                vcvars_path, cl_path, warn_flag, nofree_flag,
                all_includes, c_file.display(), runtime_c.display(), extra_c_str, extra_flags_str, exe_file.display(),
            )
        };

        if let Err(e) = fs::write(&bat_file, &bat_content) {
            eprintln!("failed to write compile script: {e}");
            return false;
        }
        if is_verbose() {
            eprintln!("[verbose] MSVC bat compile: {}", bat_content.trim());
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

        // Add user-supplied extra C source files and compiler flags
        for f in extra_c_files {
            command.arg(f);
        }
        for flag in extra_cflags {
            command.arg(flag);
        }

        verbose_command("MSVC cl.exe", &command);
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
    fn backend_resolution_covers_implicit_policy_and_explicit_overrides() {
        assert_eq!(
            resolve_backend(Some(Backend::C), false, false, false, true),
            Backend::C
        );
        assert_eq!(
            resolve_backend(Some(Backend::Native), true, true, false, false),
            Backend::Native
        );
        assert_eq!(resolve_backend(None, true, false, false, true), Backend::C);
        assert_eq!(resolve_backend(None, false, true, false, true), Backend::C);
        assert_eq!(
            resolve_backend(None, false, false, true, false),
            Backend::Native
        );
        assert_eq!(
            resolve_backend(None, false, false, false, true),
            Backend::Native
        );
        assert_eq!(
            resolve_backend(None, false, false, false, false),
            Backend::C
        );
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
            resource_dir_candidates("toolchain", Some(override_dir.clone()), Some(&exe), false),
            vec![override_dir]
        );
    }

    #[test]
    fn toolchain_candidates_never_include_a_bare_cwd_relative_path() {
        // Finding 2b: an executed resource (the bundled toolchain) must
        // never fall back to a bare `./toolchain` relative to whatever
        // directory `oscan` was launched from.
        let exe = PathBuf::from("install").join("oscan");
        let candidates = resource_dir_candidates("toolchain", None, Some(&exe), false);

        assert_eq!(candidates, vec![PathBuf::from("install").join("toolchain")]);
    }

    #[test]
    fn runtime_candidates_still_include_a_bare_cwd_relative_path() {
        // Unchanged, out-of-scope behavior: `runtime/` is only ever read as
        // data, never executed.
        let exe = PathBuf::from("install").join("oscan");
        let candidates = resource_dir_candidates("runtime", None, Some(&exe), true);

        assert_eq!(
            candidates,
            vec![
                PathBuf::from("install").join("runtime"),
                PathBuf::from("runtime")
            ]
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
        // With the execution health check, dummy files won't pass.
        // Verify that non-executable files are correctly skipped.
        let test_dir = TestDir::new("bundled-platform");
        let toolchain = test_dir.path.join("toolchain");
        let platform_clang = toolchain.join("windows").join("bin").join("clang.exe");
        let generic_gcc = toolchain.join("bin").join("gcc.exe");

        fs::create_dir_all(platform_clang.parent().unwrap()).unwrap();
        fs::create_dir_all(generic_gcc.parent().unwrap()).unwrap();
        fs::write(&platform_clang, []).unwrap();
        fs::write(&generic_gcc, []).unwrap();

        // Non-executable dummy files should be skipped by the health check
        assert_eq!(find_bundled_c_compiler_in_dir(&toolchain, "windows"), None);
    }

    #[test]
    fn bundled_compiler_detection_falls_back_to_generic_bin() {
        // With the execution health check, dummy files won't pass.
        let test_dir = TestDir::new("bundled-generic");
        let toolchain = test_dir.path.join("toolchain");
        let generic_gcc = toolchain.join("bin").join("gcc");

        fs::create_dir_all(generic_gcc.parent().unwrap()).unwrap();
        fs::write(&generic_gcc, []).unwrap();

        // Non-executable dummy file should be skipped
        assert_eq!(find_bundled_c_compiler_in_dir(&toolchain, "linux"), None);
    }

    #[test]
    fn compiler_source_labels_are_stable_for_release_smoke_tests() {
        assert_eq!(compiler_source_label(CompilerSource::Override), "override");
        assert_eq!(compiler_source_label(CompilerSource::Bundled), "bundled");
        assert_eq!(compiler_source_label(CompilerSource::Host), "host");
    }

    // --- Finding 3: unpredictable, private native-backend scratch dir. ---

    #[test]
    fn native_scratch_dir_names_are_unpredictable_across_repeated_calls() {
        let dirs: Vec<_> = (0..8)
            .map(|_| create_native_scratch_dir().expect("create scratch dir"))
            .collect();
        let names: Vec<String> = dirs
            .iter()
            .map(|d| d.path().file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        let old_predictable_name = format!("oscan_native_{}", process::id());
        for name in &names {
            assert!(name.starts_with("oscan_native_"));
            assert_ne!(
                name, &old_predictable_name,
                "must never reproduce the old bare-PID predictable name"
            );
        }
        let unique: HashSet<&String> = names.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "each call must produce a distinct, non-sequential random suffix"
        );
    }

    #[cfg(unix)]
    #[test]
    fn native_scratch_dir_has_0700_permissions_on_unix() {
        // Security review 2026-07-15 (finding 3): asserts the *explicit*
        // `harden_native_scratch_dir_unix` call took effect, not just
        // `tempfile`'s own internal default.
        use std::os::unix::fs::PermissionsExt;
        let dir = create_native_scratch_dir().expect("create scratch dir");
        let mode = fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "native scratch dir must be private (0700) on Unix"
        );
    }

    #[cfg(unix)]
    #[test]
    fn harden_native_scratch_dir_unix_sets_exactly_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .prefix("oscan_native_harden_test_")
            .tempdir()
            .expect("create a plain temp dir to harden");
        // Start from a deliberately looser mode so the assertion below
        // proves this function actively narrows it, rather than merely
        // reading back a mode some other mechanism already set.
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755))
            .expect("set a loose starting mode");
        harden_native_scratch_dir_unix(dir.path()).expect("must succeed against a real directory");
        let mode = fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn harden_native_scratch_dir_unix_fails_hard_when_permissions_cannot_be_set() {
        // Security review 2026-07-15 (finding 3): a permission-setting
        // failure must be a hard error, not a silently-ignored best-effort
        // attempt. A path that does not exist is a portable, reliable way
        // to make `fs::set_permissions` fail (`ENOENT`) without depending
        // on any particular privileged/unprivileged user setup.
        let missing = env::temp_dir().join(format!(
            "oscan-native-harden-missing-{}-{}",
            process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        assert!(!missing.exists(), "the probe path must not already exist");
        let err = harden_native_scratch_dir_unix(&missing).expect_err(
            "setting permissions on a nonexistent path must fail, not succeed silently",
        );
        // `create_native_scratch_dir`'s caller (`run_native_backend`)
        // propagates this `io::Error` as a fatal
        // "error creating temp directory: ..." + `process::exit(1)` — this
        // just proves the error itself is produced, not swallowed.
        let _ = err;
    }

    #[test]
    fn native_scratch_dir_never_reuses_the_old_predictable_planted_path() {
        let old_style = env::temp_dir().join(format!("oscan_native_{}", process::id()));
        fs::create_dir_all(&old_style).expect("pre-create the old-style vulnerable path");
        fs::write(old_style.join("planted"), b"attacker-controlled content")
            .expect("plant attacker content");

        let dir = create_native_scratch_dir().expect("create scratch dir");
        assert_ne!(
            dir.path(),
            old_style.as_path(),
            "must never reuse the predictable oscan_native_<pid> path"
        );
        assert!(
            !dir.path().join("planted").exists(),
            "must never pick up attacker-planted content from the old predictable path"
        );

        let _ = fs::remove_dir_all(&old_style);
    }

    #[test]
    fn native_scratch_dir_is_removed_once_the_guard_drops() {
        let path = {
            let dir = create_native_scratch_dir().expect("create scratch dir");
            let p = dir.path().to_path_buf();
            assert!(
                p.is_dir(),
                "scratch dir must exist while the guard is alive"
            );
            p
            // `dir` (the `TempDir` guard) drops here.
        };
        assert!(
            !path.exists(),
            "scratch dir must be removed once the TempDir guard drops"
        );
    }
}
