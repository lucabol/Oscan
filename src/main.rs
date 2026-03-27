mod ast;
mod error;
mod lexer;
mod parser;
mod token;

use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut dump_tokens = false;
    let mut dump_ast = false;
    let mut file_path = None;

    for arg in &args[1..] {
        match arg.as_str() {
            "--dump-tokens" => dump_tokens = true,
            "--dump-ast" => dump_ast = true,
            _ if !arg.starts_with('-') => file_path = Some(arg.clone()),
            other => {
                eprintln!("unknown flag: {other}");
                process::exit(1);
            }
        }
    }

    let path = match file_path {
        Some(p) => p,
        None => {
            eprintln!("usage: babelc [--dump-tokens] [--dump-ast] <file.bc>");
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

    let mut lex = lexer::Lexer::new(&source);
    let tokens = match lex.tokenize() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            process::exit(1);
        }
    };

    if dump_tokens {
        for tok in &tokens {
            println!("{:?}", tok);
        }
    }

    let mut par = parser::Parser::new(tokens);
    let program = match par.parse_program() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            process::exit(1);
        }
    };

    if dump_ast {
        println!("{:#?}", program);
    }

    if !dump_tokens && !dump_ast {
        println!("Parse successful");
    }
}
