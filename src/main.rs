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
use std::path::Path;
use std::process::{self, Command};

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut dump_tokens = false;
    let mut dump_ast = false;
    let mut output_path = None;
    let mut file_path = None;
    let mut run_mode = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--dump-tokens" => dump_tokens = true,
            "--dump-ast" => dump_ast = true,
            "--run" => run_mode = true,
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
            eprintln!("usage: babelc [--dump-tokens] [--dump-ast] [--run] [-o output.c] <file.bc>");
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
        if output_path.is_none() && !run_mode {
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

    if run_mode {
        // Run mode: compile and execute
        run_program(&path, &c_code);
    } else if let Some(out_path) = output_path {
        match fs::write(&out_path, &c_code) {
            Ok(_) => {
                eprintln!("Wrote {}", out_path);
            }
            Err(e) => {
                eprintln!("error writing {out_path}: {e}");
                process::exit(1);
            }
        }
    } else {
        println!("{}", c_code);
    }
}

fn run_program(source_path: &str, c_code: &str) {
    // Create a temporary directory in the current workspace
    let temp_dir = Path::new("target").join("babelc_temp");
    if let Err(e) = fs::create_dir_all(&temp_dir) {
        eprintln!("error creating temp directory: {e}");
        process::exit(1);
    }

    // Write the generated C code to a temp file
    let c_file = temp_dir.join("program.c");
    if let Err(e) = fs::write(&c_file, c_code) {
        eprintln!("error writing temporary C file: {e}");
        process::exit(1);
    }

    // Determine the executable name
    let exe_file = if cfg!(windows) {
        temp_dir.join("program.exe")
    } else {
        temp_dir.join("program")
    };

    // Get the runtime directory
    let runtime_dir = Path::new("runtime");
    if !runtime_dir.exists() {
        eprintln!("error: runtime directory not found");
        eprintln!("make sure you're running from the project root");
        process::exit(1);
    }

    let runtime_c = runtime_dir.join("bc_runtime.c");
    let runtime_h = runtime_dir.join("bc_runtime.h");

    if !runtime_c.exists() || !runtime_h.exists() {
        eprintln!("error: runtime files not found in runtime/");
        process::exit(1);
    }

    // Try to compile with gcc
    // On Windows, try WSL first, then native gcc
    let (compiled, use_wsl) = if cfg!(windows) {
        // Try WSL gcc first
        let wsl_result = compile_with_wsl(&c_file, &exe_file, &runtime_c, runtime_dir);
        if wsl_result {
            (true, true)
        } else {
            // Fall back to native gcc if available
            (compile_with_native_gcc(&c_file, &exe_file, &runtime_c, runtime_dir), false)
        }
    } else {
        // Unix: use gcc directly
        (compile_with_native_gcc(&c_file, &exe_file, &runtime_c, runtime_dir), false)
    };

    if !compiled {
        eprintln!("\nerror: failed to compile");
        eprintln!("on Windows, gcc must be available via WSL or in PATH");
        process::exit(1);
    }

    // Run the compiled executable
    eprintln!("\n=== Running {} ===\n", source_path);
    let status = if cfg!(windows) && use_wsl {
        // If compiled with WSL, run with WSL
        let to_wsl_path = |p: &Path| -> String {
            let s = p.to_str().unwrap().replace("\\", "/");
            let lower = s.to_lowercase();
            if lower.starts_with("c:/") {
                format!("/mnt/c/{}", &s[3..])
            } else if lower.starts_with("c:\\") {
                format!("/mnt/c/{}", &s[3..].replace("\\", "/"))
            } else {
                s
            }
        };
        let wsl_exe = to_wsl_path(&exe_file);
        Command::new("wsl")
            .arg("bash")
            .arg("-c")
            .arg(&wsl_exe)
            .status()
    } else {
        // Native execution
        Command::new(&exe_file).status()
    };

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

fn compile_with_wsl(c_file: &Path, exe_file: &Path, runtime_c: &Path, runtime_dir: &Path) -> bool {
    eprintln!("Compiling with WSL gcc...");
    
    // Convert Windows paths to WSL paths properly
    let to_wsl_path = |p: &Path| -> String {
        let s = p.to_str().unwrap().replace("\\", "/");
        // Handle C:/... or c:/...
        let lower = s.to_lowercase();
        if lower.starts_with("c:/") {
            format!("/mnt/c/{}", &s[3..])
        } else if lower.starts_with("c:\\") {
            format!("/mnt/c/{}", &s[3..].replace("\\", "/"))
        } else {
            // Relative path - convert backslashes
            s
        }
    };
    
    let wsl_c_file = to_wsl_path(c_file);
    let wsl_runtime_c = to_wsl_path(runtime_c);
    let wsl_runtime_dir = to_wsl_path(runtime_dir);
    let wsl_exe = to_wsl_path(exe_file);
    
    let compile_cmd = format!(
        "gcc {} {} -I{} -o {} -lm -std=c99",
        wsl_c_file, wsl_runtime_c, wsl_runtime_dir, wsl_exe
    );
    
    let output = Command::new("wsl")
        .arg("bash")
        .arg("-c")
        .arg(&compile_cmd)
        .output();
    
    match output {
        Ok(out) => {
            if !out.status.success() {
                eprintln!("WSL gcc compilation failed:");
                std::io::stderr().write_all(&out.stderr).ok();
                false
            } else {
                true
            }
        }
        Err(_) => {
            eprintln!("WSL not available");
            false
        }
    }
}

fn compile_with_native_gcc(c_file: &Path, exe_file: &Path, runtime_c: &Path, runtime_dir: &Path) -> bool {
    eprintln!("Compiling with native gcc...");
    
    let output = Command::new("gcc")
        .arg(c_file)
        .arg(runtime_c)
        .arg(format!("-I{}", runtime_dir.display()))
        .arg("-o")
        .arg(exe_file)
        .arg("-lm")
        .arg("-std=c99")
        .output();
    
    match output {
        Ok(out) => {
            if !out.status.success() {
                eprintln!("gcc compilation failed:");
                std::io::stderr().write_all(&out.stderr).ok();
                false
            } else {
                true
            }
        }
        Err(_) => {
            eprintln!("gcc not found in PATH");
            false
        }
    }
}
