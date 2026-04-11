use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::error::CompileError;
use crate::token::Span;
use crate::types::*;

// ---------------------------------------------------------------------------
// Resource functions that should have a matching defer cleanup.
// Format: (acquire_fn, cleanup_fn)
// ---------------------------------------------------------------------------

const RESOURCE_PAIRS: &[(&str, &str)] = &[
    ("file_open_read", "file_close"),
    ("file_open_write", "file_close"),
    ("file_open_append", "file_close"),
    ("socket_tcp", "socket_close"),
    ("socket_udp", "socket_close"),
    ("socket_accept", "socket_close"),
    ("socket_unix_connect", "socket_close"),
    ("term_raw", "term_restore"),
];

// ---------------------------------------------------------------------------
// Binding info for variables in scope
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct BindingInfo {
    ty: BcType,
    is_mut: bool,
}

// ---------------------------------------------------------------------------
// Semantic Analyzer
// ---------------------------------------------------------------------------

pub struct SemanticAnalyzer {
    structs: HashMap<String, StructInfo>,
    enums: HashMap<String, EnumInfo>,
    functions: HashMap<String, FunctionInfo>,
    constants: HashMap<String, ConstInfo>,
    scopes: Vec<HashMap<String, BindingInfo>>,
    current_fn_return_type: Option<BcType>,
    in_pure_fn: bool,
    loop_depth: usize,
    show_warnings: bool,
}

impl SemanticAnalyzer {
    pub fn analyze(program: &Program, show_warnings: bool) -> Result<SemanticInfo, CompileError> {
        let mut sa = Self {
            structs: HashMap::new(),
            enums: HashMap::new(),
            functions: HashMap::new(),
            constants: HashMap::new(),
            scopes: Vec::new(),
            current_fn_return_type: None,
            in_pure_fn: false,
            loop_depth: 0,
            show_warnings,
        };
        sa.register_builtins();

        // Pass 1: collect all top-level declarations
        for decl in &program.decls {
            sa.collect_decl(decl)?;
        }

        // Pass 2: type-check everything
        for decl in &program.decls {
            sa.check_decl(decl)?;
        }

        Ok(SemanticInfo {
            structs: sa.structs,
            enums: sa.enums,
            functions: sa.functions,
            constants: sa.constants,
        })
    }

    // -----------------------------------------------------------------------
    // Built-in micro-lib registration
    // -----------------------------------------------------------------------

    fn register_builtins(&mut self) {
        let builtin = |params: Vec<(&str, BcType)>, ret: BcType, pure: bool| FunctionInfo {
            params: params
                .into_iter()
                .map(|(n, t)| (n.to_string(), t))
                .collect(),
            return_type: ret,
            is_pure: pure,
            is_extern: false,
        };

        // I/O (fn!)
        // @builtin category="I/O" name="print" sig="fn! print(s: str)" desc="Print string to stdout"
        self.functions.insert(
            "print".into(),
            builtin(vec![("s", BcType::Str)], BcType::Unit, false),
        );
        // @builtin category="I/O" name="println" sig="fn! println(s: str)" desc="Print string with newline"
        self.functions.insert(
            "println".into(),
            builtin(vec![("s", BcType::Str)], BcType::Unit, false),
        );
        // @builtin category="I/O" name="print_i32" sig="fn! print_i32(n: i32)" desc="Print i32 to stdout"
        self.functions.insert(
            "print_i32".into(),
            builtin(vec![("n", BcType::I32)], BcType::Unit, false),
        );
        // @builtin category="I/O" name="print_i64" sig="fn! print_i64(n: i64)" desc="Print i64 to stdout"
        self.functions.insert(
            "print_i64".into(),
            builtin(vec![("n", BcType::I64)], BcType::Unit, false),
        );
        // @builtin category="I/O" name="print_f64" sig="fn! print_f64(n: f64)" desc="Print f64 to stdout"
        self.functions.insert(
            "print_f64".into(),
            builtin(vec![("n", BcType::F64)], BcType::Unit, false),
        );
        // @builtin category="I/O" name="print_bool" sig="fn! print_bool(b: bool)" desc="Print bool to stdout"
        self.functions.insert(
            "print_bool".into(),
            builtin(vec![("b", BcType::Bool)], BcType::Unit, false),
        );
        // @builtin category="I/O" name="read_line" sig="fn! read_line() -> Result<str, str>" desc="Read a line from stdin"
        self.functions.insert(
            "read_line".into(),
            builtin(
                vec![],
                BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)),
                false,
            ),
        );

        // String (pure except concat/to_cstr/slice/from_i32 which allocate)
        // @builtin category="String" name="str_len" sig="fn str_len(s: str) -> i32" desc="Length of string in bytes"
        self.functions.insert(
            "str_len".into(),
            builtin(vec![("s", BcType::Str)], BcType::I32, true),
        );
        // @builtin category="String" name="str_eq" sig="fn str_eq(a: str, b: str) -> bool" desc="String equality check"
        self.functions.insert(
            "str_eq".into(),
            builtin(
                vec![("a", BcType::Str), ("b", BcType::Str)],
                BcType::Bool,
                true,
            ),
        );
        // @builtin category="String" name="str_concat" sig="fn! str_concat(a: str, b: str) -> str" desc="Concatenate two strings"
        self.functions.insert(
            "str_concat".into(),
            builtin(
                vec![("a", BcType::Str), ("b", BcType::Str)],
                BcType::Str,
                false,
            ),
        );
        // @builtin category="String" name="str_to_cstr" sig="fn! str_to_cstr(s: str) -> str" desc="Convert to null-terminated C string"
        self.functions.insert(
            "str_to_cstr".into(),
            builtin(vec![("s", BcType::Str)], BcType::Str, false),
        );
        // @builtin category="String" name="str_find" sig="fn str_find(haystack: str, needle: str) -> i32" desc="Find substring index or -1"
        self.functions.insert(
            "str_find".into(),
            builtin(
                vec![("haystack", BcType::Str), ("needle", BcType::Str)],
                BcType::I32,
                true,
            ),
        );
        // @builtin category="String" name="str_from_i32" sig="fn! str_from_i32(n: i32) -> str" desc="Convert i32 to string"
        self.functions.insert(
            "str_from_i32".into(),
            builtin(vec![("n", BcType::I32)], BcType::Str, false),
        );
        // @builtin category="String" name="str_slice" sig="fn! str_slice(s: str, start: i32, end: i32) -> str" desc="Extract substring by index range"
        self.functions.insert(
            "str_slice".into(),
            builtin(
                vec![
                    ("s", BcType::Str),
                    ("start", BcType::I32),
                    ("end", BcType::I32),
                ],
                BcType::Str,
                false,
            ),
        );

        // Math (pure)
        // @builtin category="Math" name="abs_i32" sig="fn abs_i32(n: i32) -> i32" desc="Absolute value of i32"
        self.functions.insert(
            "abs_i32".into(),
            builtin(vec![("n", BcType::I32)], BcType::I32, true),
        );
        // @builtin category="Math" name="abs_f64" sig="fn abs_f64(n: f64) -> f64" desc="Absolute value of f64"
        self.functions.insert(
            "abs_f64".into(),
            builtin(vec![("n", BcType::F64)], BcType::F64, true),
        );
        // @builtin category="Math" name="mod_i32" sig="fn mod_i32(a: i32, b: i32) -> i32" desc="Integer modulus"
        self.functions.insert(
            "mod_i32".into(),
            builtin(
                vec![("a", BcType::I32), ("b", BcType::I32)],
                BcType::I32,
                true,
            ),
        );
        // @builtin category="Math" name="math_sin" sig="fn math_sin(x: f64) -> f64" desc="Sine"
        self.functions.insert(
            "math_sin".into(),
            builtin(vec![("x", BcType::F64)], BcType::F64, true),
        );
        // @builtin category="Math" name="math_cos" sig="fn math_cos(x: f64) -> f64" desc="Cosine"
        self.functions.insert(
            "math_cos".into(),
            builtin(vec![("x", BcType::F64)], BcType::F64, true),
        );
        // @builtin category="Math" name="math_sqrt" sig="fn math_sqrt(x: f64) -> f64" desc="Square root"
        self.functions.insert(
            "math_sqrt".into(),
            builtin(vec![("x", BcType::F64)], BcType::F64, true),
        );
        // @builtin category="Math" name="math_pow" sig="fn math_pow(base: f64, exp: f64) -> f64" desc="Power"
        self.functions.insert(
            "math_pow".into(),
            builtin(
                vec![("base", BcType::F64), ("exp", BcType::F64)],
                BcType::F64,
                true,
            ),
        );
        // @builtin category="Math" name="math_exp" sig="fn math_exp(x: f64) -> f64" desc="Exponential (e^x)"
        self.functions.insert(
            "math_exp".into(),
            builtin(vec![("x", BcType::F64)], BcType::F64, true),
        );
        // @builtin category="Math" name="math_log" sig="fn math_log(x: f64) -> f64" desc="Natural logarithm"
        self.functions.insert(
            "math_log".into(),
            builtin(vec![("x", BcType::F64)], BcType::F64, true),
        );
        // @builtin category="Math" name="math_atan2" sig="fn math_atan2(y: f64, x: f64) -> f64" desc="Two-argument arctangent"
        self.functions.insert(
            "math_atan2".into(),
            builtin(
                vec![("y", BcType::F64), ("x", BcType::F64)],
                BcType::F64,
                true,
            ),
        );
        // @builtin category="Math" name="math_floor" sig="fn math_floor(x: f64) -> f64" desc="Floor"
        self.functions.insert(
            "math_floor".into(),
            builtin(vec![("x", BcType::F64)], BcType::F64, true),
        );
        // @builtin category="Math" name="math_ceil" sig="fn math_ceil(x: f64) -> f64" desc="Ceiling"
        self.functions.insert(
            "math_ceil".into(),
            builtin(vec![("x", BcType::F64)], BcType::F64, true),
        );
        // @builtin category="Math" name="math_fmod" sig="fn math_fmod(x: f64, y: f64) -> f64" desc="Floating-point modulus"
        self.functions.insert(
            "math_fmod".into(),
            builtin(
                vec![("x", BcType::F64), ("y", BcType::F64)],
                BcType::F64,
                true,
            ),
        );
        // @builtin category="Math" name="math_abs" sig="fn math_abs(x: f64) -> f64" desc="Absolute value of f64"
        self.functions.insert(
            "math_abs".into(),
            builtin(vec![("x", BcType::F64)], BcType::F64, true),
        );
        // @builtin category="Math" name="math_pi" sig="fn math_pi() -> f64" desc="Constant pi"
        self.functions
            .insert("math_pi".into(), builtin(vec![], BcType::F64, true));
        // @builtin category="Math" name="math_e" sig="fn math_e() -> f64" desc="Constant e"
        self.functions
            .insert("math_e".into(), builtin(vec![], BcType::F64, true));
        // @builtin category="Math" name="math_ln2" sig="fn math_ln2() -> f64" desc="Constant ln(2)"
        self.functions
            .insert("math_ln2".into(), builtin(vec![], BcType::F64, true));
        // @builtin category="Math" name="math_sqrt2" sig="fn math_sqrt2() -> f64" desc="Constant sqrt(2)"
        self.functions
            .insert("math_sqrt2".into(), builtin(vec![], BcType::F64, true));

        // Bitwise (pure)
        // @builtin category="Bitwise" name="band" sig="fn band(a: i32, b: i32) -> i32" desc="Bitwise AND"
        self.functions.insert(
            "band".into(),
            builtin(
                vec![("a", BcType::I32), ("b", BcType::I32)],
                BcType::I32,
                true,
            ),
        );
        // @builtin category="Bitwise" name="bor" sig="fn bor(a: i32, b: i32) -> i32" desc="Bitwise OR"
        self.functions.insert(
            "bor".into(),
            builtin(
                vec![("a", BcType::I32), ("b", BcType::I32)],
                BcType::I32,
                true,
            ),
        );
        // @builtin category="Bitwise" name="bxor" sig="fn bxor(a: i32, b: i32) -> i32" desc="Bitwise XOR"
        self.functions.insert(
            "bxor".into(),
            builtin(
                vec![("a", BcType::I32), ("b", BcType::I32)],
                BcType::I32,
                true,
            ),
        );
        // @builtin category="Bitwise" name="bshl" sig="fn bshl(a: i32, n: i32) -> i32" desc="Bitwise shift left"
        self.functions.insert(
            "bshl".into(),
            builtin(
                vec![("a", BcType::I32), ("n", BcType::I32)],
                BcType::I32,
                true,
            ),
        );
        // @builtin category="Bitwise" name="bshr" sig="fn bshr(a: i32, n: i32) -> i32" desc="Bitwise shift right"
        self.functions.insert(
            "bshr".into(),
            builtin(
                vec![("a", BcType::I32), ("n", BcType::I32)],
                BcType::I32,
                true,
            ),
        );
        // @builtin category="Bitwise" name="bnot" sig="fn bnot(a: i32) -> i32" desc="Bitwise NOT"
        self.functions.insert(
            "bnot".into(),
            builtin(vec![("a", BcType::I32)], BcType::I32, true),
        );

        // Conversion
        // @builtin category="Conversion" name="i32_to_str" sig="fn! i32_to_str(n: i32) -> str" desc="Convert i32 to string"
        self.functions.insert(
            "i32_to_str".into(),
            builtin(vec![("n", BcType::I32)], BcType::Str, false),
        );

        // File I/O (fn!)
        // @builtin category="File I/O" name="file_open_read" sig="fn! file_open_read(path: str) -> Result<i32, str>" desc="Open file for reading, returns fd"
        self.functions.insert(
            "file_open_read".into(),
            builtin(vec![("path", BcType::Str)], BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str)), false),
        );
        // @builtin category="File I/O" name="file_open_write" sig="fn! file_open_write(path: str) -> Result<i32, str>" desc="Open file for writing, returns fd"
        self.functions.insert(
            "file_open_write".into(),
            builtin(vec![("path", BcType::Str)], BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str)), false),
        );
        // @builtin category="File I/O" name="read_byte" sig="fn! read_byte(fd: i32) -> i32" desc="Read one byte from fd"
        self.functions.insert(
            "read_byte".into(),
            builtin(vec![("fd", BcType::I32)], BcType::I32, false),
        );
        // @builtin category="File I/O" name="write_byte" sig="fn! write_byte(fd: i32, b: i32)" desc="Write one byte to fd"
        self.functions.insert(
            "write_byte".into(),
            builtin(
                vec![("fd", BcType::I32), ("b", BcType::I32)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="File I/O" name="write_str" sig="fn! write_str(fd: i32, s: str)" desc="Write string to fd"
        self.functions.insert(
            "write_str".into(),
            builtin(
                vec![("fd", BcType::I32), ("s", BcType::Str)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="File I/O" name="file_close" sig="fn! file_close(fd: i32)" desc="Close file descriptor"
        self.functions.insert(
            "file_close".into(),
            builtin(vec![("fd", BcType::I32)], BcType::Unit, false),
        );
        // @builtin category="File I/O" name="file_delete" sig="fn! file_delete(path: str) -> Result<str, str>" desc="Delete a file"
        self.functions.insert(
            "file_delete".into(),
            builtin(vec![("path", BcType::Str)], BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)), false),
        );

        // Socket I/O (fn!)
        // @builtin category="Socket" name="socket_tcp" sig="fn! socket_tcp() -> Result<i32, str>" desc="Create TCP socket"
        self.functions
            .insert("socket_tcp".into(), builtin(vec![], BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str)), false));
        // @builtin category="Socket" name="socket_connect" sig="fn! socket_connect(sock: i32, addr: str, port: i32) -> Result<str, str>" desc="Connect to address and port"
        self.functions.insert(
            "socket_connect".into(),
            builtin(
                vec![
                    ("sock", BcType::I32),
                    ("addr", BcType::Str),
                    ("port", BcType::I32),
                ],
                BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)),
                false,
            ),
        );
        // @builtin category="Socket" name="socket_bind" sig="fn! socket_bind(sock: i32, port: i32) -> Result<str, str>" desc="Bind socket to port"
        self.functions.insert(
            "socket_bind".into(),
            builtin(
                vec![("sock", BcType::I32), ("port", BcType::I32)],
                BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)),
                false,
            ),
        );
        // @builtin category="Socket" name="socket_listen" sig="fn! socket_listen(sock: i32, backlog: i32) -> Result<str, str>" desc="Listen for connections"
        self.functions.insert(
            "socket_listen".into(),
            builtin(
                vec![("sock", BcType::I32), ("backlog", BcType::I32)],
                BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)),
                false,
            ),
        );
        // @builtin category="Socket" name="socket_accept" sig="fn! socket_accept(sock: i32) -> Result<i32, str>" desc="Accept incoming connection"
        self.functions.insert(
            "socket_accept".into(),
            builtin(vec![("sock", BcType::I32)], BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str)), false),
        );
        // @builtin category="Socket" name="socket_send" sig="fn! socket_send(sock: i32, data: str) -> Result<i32, str>" desc="Send data on socket"
        self.functions.insert(
            "socket_send".into(),
            builtin(
                vec![("sock", BcType::I32), ("data", BcType::Str)],
                BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str)),
                false,
            ),
        );
        // @builtin category="Socket" name="socket_recv" sig="fn! socket_recv(sock: i32, max_len: i32) -> str" desc="Receive data from socket"
        self.functions.insert(
            "socket_recv".into(),
            builtin(
                vec![("sock", BcType::I32), ("max_len", BcType::I32)],
                BcType::Str,
                false,
            ),
        );
        // @builtin category="Socket" name="socket_close" sig="fn! socket_close(sock: i32)" desc="Close socket"
        self.functions.insert(
            "socket_close".into(),
            builtin(vec![("sock", BcType::I32)], BcType::Unit, false),
        );

        // UDP Socket I/O (fn!)
        // @builtin category="Socket" name="socket_udp" sig="fn! socket_udp() -> Result<i32, str>" desc="Create UDP socket"
        self.functions
            .insert("socket_udp".into(), builtin(vec![], BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str)), false));
        // @builtin category="Socket" name="socket_sendto" sig="fn! socket_sendto(sock: i32, data: str, addr: str, port: i32) -> i32" desc="Send UDP data to address"
        self.functions.insert(
            "socket_sendto".into(),
            builtin(
                vec![
                    ("sock", BcType::I32),
                    ("data", BcType::Str),
                    ("addr", BcType::Str),
                    ("port", BcType::I32),
                ],
                BcType::I32,
                false,
            ),
        );
        // @builtin category="Socket" name="socket_recvfrom" sig="fn! socket_recvfrom(sock: i32, max_len: i32) -> str" desc="Receive UDP data"
        self.functions.insert(
            "socket_recvfrom".into(),
            builtin(
                vec![("sock", BcType::I32), ("max_len", BcType::I32)],
                BcType::Str,
                false,
            ),
        );

        // Unix domain sockets (fn!)
        // @builtin category="Socket" name="socket_unix_connect" sig="fn! socket_unix_connect(path: str) -> Result<i32, str>" desc="Connect to Unix domain socket"
        self.functions.insert(
            "socket_unix_connect".into(),
            builtin(
                vec![("path", BcType::Str)],
                BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str)),
                false,
            ),
        );

        // Command-line arguments (fn!)
        // @builtin category="System" name="arg_count" sig="fn! arg_count() -> i32" desc="Number of command-line arguments"
        self.functions
            .insert("arg_count".into(), builtin(vec![], BcType::I32, false));
        // @builtin category="System" name="arg_get" sig="fn! arg_get(i: i32) -> str" desc="Get command-line argument by index"
        self.functions.insert(
            "arg_get".into(),
            builtin(vec![("i", BcType::I32)], BcType::Str, false),
        );

        // Tier 1: Character classification (pure)
        // @builtin category="Character" name="char_is_alpha" sig="fn char_is_alpha(c: i32) -> bool" desc="Check if character is alphabetic"
        self.functions.insert(
            "char_is_alpha".into(),
            builtin(vec![("c", BcType::I32)], BcType::Bool, true),
        );
        // @builtin category="Character" name="char_is_digit" sig="fn char_is_digit(c: i32) -> bool" desc="Check if character is a digit"
        self.functions.insert(
            "char_is_digit".into(),
            builtin(vec![("c", BcType::I32)], BcType::Bool, true),
        );
        // @builtin category="Character" name="char_is_alnum" sig="fn char_is_alnum(c: i32) -> bool" desc="Check if character is alphanumeric"
        self.functions.insert(
            "char_is_alnum".into(),
            builtin(vec![("c", BcType::I32)], BcType::Bool, true),
        );
        // @builtin category="Character" name="char_is_space" sig="fn char_is_space(c: i32) -> bool" desc="Check if character is whitespace"
        self.functions.insert(
            "char_is_space".into(),
            builtin(vec![("c", BcType::I32)], BcType::Bool, true),
        );
        // @builtin category="Character" name="char_is_upper" sig="fn char_is_upper(c: i32) -> bool" desc="Check if character is uppercase"
        self.functions.insert(
            "char_is_upper".into(),
            builtin(vec![("c", BcType::I32)], BcType::Bool, true),
        );
        // @builtin category="Character" name="char_is_lower" sig="fn char_is_lower(c: i32) -> bool" desc="Check if character is lowercase"
        self.functions.insert(
            "char_is_lower".into(),
            builtin(vec![("c", BcType::I32)], BcType::Bool, true),
        );
        // @builtin category="Character" name="char_is_print" sig="fn char_is_print(c: i32) -> bool" desc="Check if character is printable"
        self.functions.insert(
            "char_is_print".into(),
            builtin(vec![("c", BcType::I32)], BcType::Bool, true),
        );
        // @builtin category="Character" name="char_is_xdigit" sig="fn char_is_xdigit(c: i32) -> bool" desc="Check if character is hex digit"
        self.functions.insert(
            "char_is_xdigit".into(),
            builtin(vec![("c", BcType::I32)], BcType::Bool, true),
        );
        // @builtin category="Character" name="char_to_upper" sig="fn char_to_upper(c: i32) -> i32" desc="Convert character to uppercase"
        self.functions.insert(
            "char_to_upper".into(),
            builtin(vec![("c", BcType::I32)], BcType::I32, true),
        );
        // @builtin category="Character" name="char_to_lower" sig="fn char_to_lower(c: i32) -> i32" desc="Convert character to lowercase"
        self.functions.insert(
            "char_to_lower".into(),
            builtin(vec![("c", BcType::I32)], BcType::I32, true),
        );
        // @builtin category="Math" name="abs_i64" sig="fn abs_i64(n: i64) -> i64" desc="Absolute value of i64"
        self.functions.insert(
            "abs_i64".into(),
            builtin(vec![("n", BcType::I64)], BcType::I64, true),
        );

        // Tier 2: Number parsing & conversion
        // @builtin category="Conversion" name="parse_i32" sig="fn parse_i32(s: str) -> Result<i32, str>" desc="Parse string to i32"
        self.functions.insert(
            "parse_i32".into(),
            builtin(
                vec![("s", BcType::Str)],
                BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str)),
                true,
            ),
        );
        // @builtin category="Conversion" name="parse_i64" sig="fn parse_i64(s: str) -> Result<i64, str>" desc="Parse string to i64"
        self.functions.insert(
            "parse_i64".into(),
            builtin(
                vec![("s", BcType::Str)],
                BcType::Result(Box::new(BcType::I64), Box::new(BcType::Str)),
                true,
            ),
        );
        // @builtin category="Conversion" name="str_from_i64" sig="fn! str_from_i64(n: i64) -> str" desc="Convert i64 to string"
        self.functions.insert(
            "str_from_i64".into(),
            builtin(vec![("n", BcType::I64)], BcType::Str, false),
        );
        // @builtin category="Conversion" name="str_from_f64" sig="fn! str_from_f64(n: f64) -> str" desc="Convert f64 to string"
        self.functions.insert(
            "str_from_f64".into(),
            builtin(vec![("n", BcType::F64)], BcType::Str, false),
        );
        // @builtin category="Conversion" name="str_from_bool" sig="fn str_from_bool(b: bool) -> str" desc="Convert bool to string"
        self.functions.insert(
            "str_from_bool".into(),
            builtin(vec![("b", BcType::Bool)], BcType::Str, true),
        );

        // Tier 3: Random, time, sleep, exit (fn!)
        // @builtin category="System" name="rand_seed" sig="fn! rand_seed(seed: i32)" desc="Seed the random number generator"
        self.functions.insert(
            "rand_seed".into(),
            builtin(vec![("seed", BcType::I32)], BcType::Unit, false),
        );
        // @builtin category="System" name="rand_i32" sig="fn! rand_i32() -> i32" desc="Generate random i32"
        self.functions
            .insert("rand_i32".into(), builtin(vec![], BcType::I32, false));
        // @builtin category="System" name="time_now" sig="fn! time_now() -> i64" desc="Current time as Unix timestamp"
        self.functions
            .insert("time_now".into(), builtin(vec![], BcType::I64, false));
        // @builtin category="System" name="sleep_ms" sig="fn! sleep_ms(ms: i32)" desc="Sleep for milliseconds"
        self.functions.insert(
            "sleep_ms".into(),
            builtin(vec![("ms", BcType::I32)], BcType::Unit, false),
        );
        // @builtin category="System" name="exit" sig="fn! exit(code: i32)" desc="Exit with status code"
        self.functions.insert(
            "exit".into(),
            builtin(vec![("code", BcType::I32)], BcType::Unit, false),
        );

        // Tier 4: Environment & error (fn!)
        // @builtin category="Environment" name="env_get" sig="fn! env_get(name: str) -> Result<str, str>" desc="Get environment variable"
        self.functions.insert(
            "env_get".into(),
            builtin(
                vec![("name", BcType::Str)],
                BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)),
                false,
            ),
        );
        // @builtin category="Environment" name="errno_get" sig="fn! errno_get() -> i32" desc="Get last error code"
        self.functions
            .insert("errno_get".into(), builtin(vec![], BcType::I32, false));
        // @builtin category="Environment" name="errno_str" sig="fn! errno_str(code: i32) -> str" desc="Convert error code to message"
        self.functions.insert(
            "errno_str".into(),
            builtin(vec![("code", BcType::I32)], BcType::Str, false),
        );

        // Tier 5: Filesystem operations (fn!)
        // @builtin category="File I/O" name="file_rename" sig="fn! file_rename(old: str, new_path: str) -> Result<str, str>" desc="Rename a file"
        self.functions.insert(
            "file_rename".into(),
            builtin(
                vec![("old", BcType::Str), ("new_path", BcType::Str)],
                BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)),
                false,
            ),
        );
        // @builtin category="File I/O" name="file_exists" sig="fn! file_exists(path: str) -> bool" desc="Check if file exists"
        self.functions.insert(
            "file_exists".into(),
            builtin(vec![("path", BcType::Str)], BcType::Bool, false),
        );
        // @builtin category="Filesystem" name="dir_create" sig="fn! dir_create(path: str) -> Result<str, str>" desc="Create a directory"
        self.functions.insert(
            "dir_create".into(),
            builtin(vec![("path", BcType::Str)], BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)), false),
        );
        // @builtin category="Filesystem" name="dir_remove" sig="fn! dir_remove(path: str) -> Result<str, str>" desc="Remove a directory"
        self.functions.insert(
            "dir_remove".into(),
            builtin(vec![("path", BcType::Str)], BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)), false),
        );
        // @builtin category="Filesystem" name="dir_current" sig="fn! dir_current() -> str" desc="Get current working directory"
        self.functions
            .insert("dir_current".into(), builtin(vec![], BcType::Str, false));
        // @builtin category="Filesystem" name="dir_change" sig="fn! dir_change(path: str) -> Result<str, str>" desc="Change working directory"
        self.functions.insert(
            "dir_change".into(),
            builtin(vec![("path", BcType::Str)], BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)), false),
        );
        // @builtin category="File I/O" name="file_open_append" sig="fn! file_open_append(path: str) -> Result<i32, str>" desc="Open file for appending, returns fd"
        self.functions.insert(
            "file_open_append".into(),
            builtin(vec![("path", BcType::Str)], BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str)), false),
        );
        // @builtin category="File I/O" name="file_size" sig="fn! file_size(path: str) -> i64" desc="Get file size in bytes"
        self.functions.insert(
            "file_size".into(),
            builtin(vec![("path", BcType::Str)], BcType::I64, false),
        );

        // Path utilities
        // @builtin category="Path" name="path_join" sig="fn! path_join(dir: str, file: str) -> str" desc="Join directory and filename"
        self.functions.insert(
            "path_join".into(),
            builtin(
                vec![("dir", BcType::Str), ("file", BcType::Str)],
                BcType::Str,
                false,
            ),
        );
        // @builtin category="Path" name="path_ext" sig="fn path_ext(path: str) -> str" desc="Get file extension"
        self.functions.insert(
            "path_ext".into(),
            builtin(vec![("path", BcType::Str)], BcType::Str, true),
        );
        // @builtin category="Path" name="path_exists" sig="fn! path_exists(path: str) -> bool" desc="Check if path exists"
        self.functions.insert(
            "path_exists".into(),
            builtin(vec![("path", BcType::Str)], BcType::Bool, false),
        );
        // @builtin category="Path" name="path_is_dir" sig="fn! path_is_dir(path: str) -> bool" desc="Check if path is a directory"
        self.functions.insert(
            "path_is_dir".into(),
            builtin(vec![("path", BcType::Str)], BcType::Bool, false),
        );

        // Tier 6: String operations
        // @builtin category="String" name="str_contains" sig="fn str_contains(s: str, sub: str) -> bool" desc="Check if string contains substring"
        self.functions.insert(
            "str_contains".into(),
            builtin(
                vec![("s", BcType::Str), ("sub", BcType::Str)],
                BcType::Bool,
                true,
            ),
        );
        // @builtin category="String" name="str_starts_with" sig="fn str_starts_with(s: str, prefix: str) -> bool" desc="Check if string starts with prefix"
        self.functions.insert(
            "str_starts_with".into(),
            builtin(
                vec![("s", BcType::Str), ("prefix", BcType::Str)],
                BcType::Bool,
                true,
            ),
        );
        // @builtin category="String" name="str_ends_with" sig="fn str_ends_with(s: str, suffix: str) -> bool" desc="Check if string ends with suffix"
        self.functions.insert(
            "str_ends_with".into(),
            builtin(
                vec![("s", BcType::Str), ("suffix", BcType::Str)],
                BcType::Bool,
                true,
            ),
        );
        // @builtin category="String" name="str_trim" sig="fn! str_trim(s: str) -> str" desc="Remove leading and trailing whitespace"
        self.functions.insert(
            "str_trim".into(),
            builtin(vec![("s", BcType::Str)], BcType::Str, false),
        );
        // @builtin category="String" name="str_split" sig="fn! str_split(s: str, delim: str) -> [str]" desc="Split string by delimiter"
        self.functions.insert(
            "str_split".into(),
            builtin(
                vec![("s", BcType::Str), ("delim", BcType::Str)],
                BcType::Array(Box::new(BcType::Str)),
                false,
            ),
        );
        // @builtin category="String" name="str_to_upper" sig="fn! str_to_upper(s: str) -> str" desc="Convert string to uppercase"
        self.functions.insert(
            "str_to_upper".into(),
            builtin(vec![("s", BcType::Str)], BcType::Str, false),
        );
        // @builtin category="String" name="str_to_lower" sig="fn! str_to_lower(s: str) -> str" desc="Convert string to lowercase"
        self.functions.insert(
            "str_to_lower".into(),
            builtin(vec![("s", BcType::Str)], BcType::Str, false),
        );
        // @builtin category="String" name="str_replace" sig="fn! str_replace(s: str, old: str, new_s: str) -> str" desc="Replace all occurrences of substring"
        self.functions.insert(
            "str_replace".into(),
            builtin(
                vec![
                    ("s", BcType::Str),
                    ("old", BcType::Str),
                    ("new_s", BcType::Str),
                ],
                BcType::Str,
                false,
            ),
        );
        // @builtin category="String" name="str_compare" sig="fn str_compare(a: str, b: str) -> i32" desc="Lexicographic comparison (-1, 0, 1)"
        self.functions.insert(
            "str_compare".into(),
            builtin(
                vec![("a", BcType::Str), ("b", BcType::Str)],
                BcType::I32,
                true,
            ),
        );

        // Tier 7: Directory listing & process control (fn!)
        // @builtin category="Filesystem" name="dir_list" sig="fn! dir_list(path: str) -> [str]" desc="List directory contents"
        self.functions.insert(
            "dir_list".into(),
            builtin(
                vec![("path", BcType::Str)],
                BcType::Array(Box::new(BcType::Str)),
                false,
            ),
        );
        // @builtin category="Process" name="proc_run" sig="fn! proc_run(cmd: str, args: [str]) -> i32" desc="Run external process"
        self.functions.insert(
            "proc_run".into(),
            builtin(
                vec![
                    ("cmd", BcType::Str),
                    ("args", BcType::Array(Box::new(BcType::Str))),
                ],
                BcType::I32,
                false,
            ),
        );
        // @builtin category="Terminal" name="term_width" sig="fn! term_width() -> i32" desc="Get terminal width in columns"
        self.functions
            .insert("term_width".into(), builtin(vec![], BcType::I32, false));
        // @builtin category="Terminal" name="term_height" sig="fn! term_height() -> i32" desc="Get terminal height in rows"
        self.functions
            .insert("term_height".into(), builtin(vec![], BcType::I32, false));

        // Tier 8: Raw terminal I/O (fn!)
        // @builtin category="Terminal" name="term_raw" sig="fn! term_raw() -> Result<str, str>" desc="Enter raw terminal mode"
        self.functions
            .insert("term_raw".into(), builtin(vec![], BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)), false));
        // @builtin category="Terminal" name="term_restore" sig="fn! term_restore() -> Result<str, str>" desc="Restore normal terminal mode"
        self.functions
            .insert("term_restore".into(), builtin(vec![], BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)), false));
        // @builtin category="Terminal" name="read_nonblock" sig="fn! read_nonblock() -> i32" desc="Non-blocking read from stdin"
        self.functions
            .insert("read_nonblock".into(), builtin(vec![], BcType::I32, false));

        // Tier 9: Environment iteration (fn!)
        // @builtin category="Environment" name="env_count" sig="fn! env_count() -> i32" desc="Number of environment variables"
        self.functions
            .insert("env_count".into(), builtin(vec![], BcType::I32, false));
        // @builtin category="Environment" name="env_key" sig="fn! env_key(i: i32) -> str" desc="Get env variable name by index"
        self.functions.insert(
            "env_key".into(),
            builtin(vec![("i", BcType::I32)], BcType::Str, false),
        );
        // @builtin category="Environment" name="env_value" sig="fn! env_value(i: i32) -> str" desc="Get env variable value by index"
        self.functions.insert(
            "env_value".into(),
            builtin(vec![("i", BcType::I32)], BcType::Str, false),
        );

        // Tier 10: Hex formatting (fn! — allocates)
        // @builtin category="Conversion" name="str_from_i32_hex" sig="fn! str_from_i32_hex(n: i32) -> str" desc="Convert i32 to hex string"
        self.functions.insert(
            "str_from_i32_hex".into(),
            builtin(vec![("n", BcType::I32)], BcType::Str, false),
        );
        // @builtin category="Conversion" name="str_from_i64_hex" sig="fn! str_from_i64_hex(n: i64) -> str" desc="Convert i64 to hex string"
        self.functions.insert(
            "str_from_i64_hex".into(),
            builtin(vec![("n", BcType::I64)], BcType::Str, false),
        );

        // Tier 11: Array sort (fn! — mutates)
        // @builtin category="Array" name="sort_i32" sig="fn! sort_i32(arr: [i32])" desc="Sort i32 array in place"
        self.functions.insert(
            "sort_i32".into(),
            builtin(
                vec![("arr", BcType::Array(Box::new(BcType::I32)))],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="Array" name="sort_i64" sig="fn! sort_i64(arr: [i64])" desc="Sort i64 array in place"
        self.functions.insert(
            "sort_i64".into(),
            builtin(
                vec![("arr", BcType::Array(Box::new(BcType::I64)))],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="Array" name="sort_str" sig="fn! sort_str(arr: [str])" desc="Sort string array in place"
        self.functions.insert(
            "sort_str".into(),
            builtin(
                vec![("arr", BcType::Array(Box::new(BcType::Str)))],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="Array" name="sort_f64" sig="fn! sort_f64(arr: [f64])" desc="Sort f64 array in place"
        self.functions.insert(
            "sort_f64".into(),
            builtin(
                vec![("arr", BcType::Array(Box::new(BcType::F64)))],
                BcType::Unit,
                false,
            ),
        );

        // Tier 12: String <-> char array conversions
        // @builtin category="String" name="str_from_chars" sig="fn! str_from_chars(arr: [i32]) -> str" desc="Build string from char code array"
        self.functions.insert(
            "str_from_chars".into(),
            builtin(
                vec![("arr", BcType::Array(Box::new(BcType::I32)))],
                BcType::Str,
                false,
            ),
        );
        // @builtin category="String" name="str_to_chars" sig="fn! str_to_chars(s: str) -> [i32]" desc="Convert string to char code array"
        self.functions.insert(
            "str_to_chars".into(),
            builtin(
                vec![("s", BcType::Str)],
                BcType::Array(Box::new(BcType::I32)),
                false,
            ),
        );

        // Tier 13: Date/Time (fn! — allocates / reads system state)
        // @builtin category="Date/Time" name="time_format" sig="fn! time_format(timestamp: i64, fmt: str) -> str" desc="Format timestamp as string"
        self.functions.insert(
            "time_format".into(),
            builtin(
                vec![("timestamp", BcType::I64), ("fmt", BcType::Str)],
                BcType::Str,
                false,
            ),
        );
        // @builtin category="Date/Time" name="time_utc_year" sig="fn! time_utc_year(timestamp: i64) -> i32" desc="Get UTC year from timestamp"
        self.functions.insert(
            "time_utc_year".into(),
            builtin(vec![("timestamp", BcType::I64)], BcType::I32, false),
        );
        // @builtin category="Date/Time" name="time_utc_month" sig="fn! time_utc_month(timestamp: i64) -> i32" desc="Get UTC month from timestamp"
        self.functions.insert(
            "time_utc_month".into(),
            builtin(vec![("timestamp", BcType::I64)], BcType::I32, false),
        );
        // @builtin category="Date/Time" name="time_utc_day" sig="fn! time_utc_day(timestamp: i64) -> i32" desc="Get UTC day from timestamp"
        self.functions.insert(
            "time_utc_day".into(),
            builtin(vec![("timestamp", BcType::I64)], BcType::I32, false),
        );
        // @builtin category="Date/Time" name="time_utc_hour" sig="fn! time_utc_hour(timestamp: i64) -> i32" desc="Get UTC hour from timestamp"
        self.functions.insert(
            "time_utc_hour".into(),
            builtin(vec![("timestamp", BcType::I64)], BcType::I32, false),
        );
        // @builtin category="Date/Time" name="time_utc_min" sig="fn! time_utc_min(timestamp: i64) -> i32" desc="Get UTC minute from timestamp"
        self.functions.insert(
            "time_utc_min".into(),
            builtin(vec![("timestamp", BcType::I64)], BcType::I32, false),
        );
        // @builtin category="Date/Time" name="time_utc_sec" sig="fn! time_utc_sec(timestamp: i64) -> i32" desc="Get UTC second from timestamp"
        self.functions.insert(
            "time_utc_sec".into(),
            builtin(vec![("timestamp", BcType::I64)], BcType::I32, false),
        );

        // Tier 13: Glob matching (pure)
        // @builtin category="System" name="glob_match" sig="fn glob_match(pattern: str, text: str) -> bool" desc="Match text against glob pattern"
        self.functions.insert(
            "glob_match".into(),
            builtin(
                vec![("pattern", BcType::Str), ("text", BcType::Str)],
                BcType::Bool,
                true,
            ),
        );

        // Tier 13: SHA-256 (fn! — allocates)
        // @builtin category="System" name="sha256" sig="fn! sha256(data: str) -> str" desc="Compute SHA-256 hash"
        self.functions.insert(
            "sha256".into(),
            builtin(vec![("data", BcType::Str)], BcType::Str, false),
        );

        // Tier 13: Terminal detection (pure-ish)
        // @builtin category="System" name="is_tty" sig="fn is_tty() -> bool" desc="Check if stdout is a terminal"
        self.functions
            .insert("is_tty".into(), builtin(vec![], BcType::Bool, true));

        // Tier 13: Environment modification (fn!)
        // @builtin category="Environment" name="env_set" sig="fn! env_set(name: str, value: str) -> Result<str, str>" desc="Set environment variable"
        self.functions.insert(
            "env_set".into(),
            builtin(
                vec![("name", BcType::Str), ("value", BcType::Str)],
                BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)),
                false,
            ),
        );
        // @builtin category="Environment" name="env_delete" sig="fn! env_delete(name: str) -> Result<str, str>" desc="Delete environment variable"
        self.functions.insert(
            "env_delete".into(),
            builtin(vec![("name", BcType::Str)], BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)), false),
        );

        // Graphics: Canvas Lifecycle (fn!)
        // @builtin category="Graphics" name="canvas_open" sig="fn! canvas_open(width: i32, height: i32, title: str) -> Result<str, str>" desc="Open graphics canvas"
        self.functions.insert(
            "canvas_open".into(),
            builtin(
                vec![
                    ("width", BcType::I32),
                    ("height", BcType::I32),
                    ("title", BcType::Str),
                ],
                BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)),
                false,
            ),
        );
        // @builtin category="Graphics" name="canvas_close" sig="fn! canvas_close()" desc="Close graphics canvas"
        self.functions
            .insert("canvas_close".into(), builtin(vec![], BcType::Unit, false));
        // @builtin category="Graphics" name="canvas_alive" sig="fn! canvas_alive() -> bool" desc="Check if canvas is still open"
        self.functions
            .insert("canvas_alive".into(), builtin(vec![], BcType::Bool, false));
        // @builtin category="Graphics" name="canvas_flush" sig="fn! canvas_flush()" desc="Flush canvas to screen"
        self.functions
            .insert("canvas_flush".into(), builtin(vec![], BcType::Unit, false));
        // @builtin category="Graphics" name="canvas_clear" sig="fn! canvas_clear(color: i32)" desc="Clear canvas with color"
        self.functions.insert(
            "canvas_clear".into(),
            builtin(vec![("color", BcType::I32)], BcType::Unit, false),
        );

        // Graphics: Drawing Primitives (fn!)
        // @builtin category="Graphics" name="gfx_pixel" sig="fn! gfx_pixel(x: i32, y: i32, color: i32)" desc="Draw a pixel"
        self.functions.insert(
            "gfx_pixel".into(),
            builtin(
                vec![
                    ("x", BcType::I32),
                    ("y", BcType::I32),
                    ("color", BcType::I32),
                ],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="Graphics" name="gfx_get_pixel" sig="fn! gfx_get_pixel(x: i32, y: i32) -> i32" desc="Get pixel color at position"
        self.functions.insert(
            "gfx_get_pixel".into(),
            builtin(
                vec![("x", BcType::I32), ("y", BcType::I32)],
                BcType::I32,
                false,
            ),
        );
        // @builtin category="Graphics" name="gfx_line" sig="fn! gfx_line(x0: i32, y0: i32, x1: i32, y1: i32, color: i32)" desc="Draw a line"
        self.functions.insert(
            "gfx_line".into(),
            builtin(
                vec![
                    ("x0", BcType::I32),
                    ("y0", BcType::I32),
                    ("x1", BcType::I32),
                    ("y1", BcType::I32),
                    ("color", BcType::I32),
                ],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="Graphics" name="gfx_rect" sig="fn! gfx_rect(x: i32, y: i32, w: i32, h: i32, color: i32)" desc="Draw rectangle outline"
        self.functions.insert(
            "gfx_rect".into(),
            builtin(
                vec![
                    ("x", BcType::I32),
                    ("y", BcType::I32),
                    ("w", BcType::I32),
                    ("h", BcType::I32),
                    ("color", BcType::I32),
                ],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="Graphics" name="gfx_fill_rect" sig="fn! gfx_fill_rect(x: i32, y: i32, w: i32, h: i32, color: i32)" desc="Draw filled rectangle"
        self.functions.insert(
            "gfx_fill_rect".into(),
            builtin(
                vec![
                    ("x", BcType::I32),
                    ("y", BcType::I32),
                    ("w", BcType::I32),
                    ("h", BcType::I32),
                    ("color", BcType::I32),
                ],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="Graphics" name="gfx_circle" sig="fn! gfx_circle(cx: i32, cy: i32, r: i32, color: i32)" desc="Draw circle outline"
        self.functions.insert(
            "gfx_circle".into(),
            builtin(
                vec![
                    ("cx", BcType::I32),
                    ("cy", BcType::I32),
                    ("r", BcType::I32),
                    ("color", BcType::I32),
                ],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="Graphics" name="gfx_fill_circle" sig="fn! gfx_fill_circle(cx: i32, cy: i32, r: i32, color: i32)" desc="Draw filled circle"
        self.functions.insert(
            "gfx_fill_circle".into(),
            builtin(
                vec![
                    ("cx", BcType::I32),
                    ("cy", BcType::I32),
                    ("r", BcType::I32),
                    ("color", BcType::I32),
                ],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="Graphics" name="gfx_draw_text" sig="fn! gfx_draw_text(x: i32, y: i32, text: str, color: i32)" desc="Draw text on canvas"
        self.functions.insert(
            "gfx_draw_text".into(),
            builtin(
                vec![
                    ("x", BcType::I32),
                    ("y", BcType::I32),
                    ("text", BcType::Str),
                    ("color", BcType::I32),
                ],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="Graphics" name="gfx_draw_text_scaled" sig="fn! gfx_draw_text_scaled(x: i32, y: i32, text: str, color: i32, sx: i32, sy: i32)" desc="Draw scaled text on canvas"
        self.functions.insert(
            "gfx_draw_text_scaled".into(),
            builtin(
                vec![
                    ("x", BcType::I32),
                    ("y", BcType::I32),
                    ("text", BcType::Str),
                    ("color", BcType::I32),
                    ("sx", BcType::I32),
                    ("sy", BcType::I32),
                ],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="Graphics" name="gfx_blit" sig="fn! gfx_blit(dx: i32, dy: i32, w: i32, h: i32, pixels: [i32])" desc="Blit pixel buffer to canvas"
        self.functions.insert(
            "gfx_blit".into(),
            builtin(
                vec![
                    ("dx", BcType::I32),
                    ("dy", BcType::I32),
                    ("w", BcType::I32),
                    ("h", BcType::I32),
                    ("pixels", BcType::Array(Box::new(BcType::I32))),
                ],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="Graphics" name="gfx_blit_alpha" sig="fn! gfx_blit_alpha(dx: i32, dy: i32, w: i32, h: i32, pixels: [i32])" desc="Alpha-blended blit to canvas"
        self.functions.insert(
            "gfx_blit_alpha".into(),
            builtin(
                vec![
                    ("dx", BcType::I32),
                    ("dy", BcType::I32),
                    ("w", BcType::I32),
                    ("h", BcType::I32),
                    ("pixels", BcType::Array(Box::new(BcType::I32))),
                ],
                BcType::Unit,
                false,
            ),
        );

        // Graphics: Input (fn!)
        // @builtin category="Graphics" name="canvas_key" sig="fn! canvas_key() -> i32" desc="Get last key press"
        self.functions
            .insert("canvas_key".into(), builtin(vec![], BcType::I32, false));
        // @builtin category="Graphics" name="canvas_mouse_x" sig="fn! canvas_mouse_x() -> i32" desc="Get mouse X position"
        self.functions
            .insert("canvas_mouse_x".into(), builtin(vec![], BcType::I32, false));
        // @builtin category="Graphics" name="canvas_mouse_y" sig="fn! canvas_mouse_y() -> i32" desc="Get mouse Y position"
        self.functions
            .insert("canvas_mouse_y".into(), builtin(vec![], BcType::I32, false));
        // @builtin category="Graphics" name="canvas_mouse_btn" sig="fn! canvas_mouse_btn() -> i32" desc="Get mouse button state"
        self.functions.insert(
            "canvas_mouse_btn".into(),
            builtin(vec![], BcType::I32, false),
        );

        // Graphics: Color (pure)
        // @builtin category="Graphics" name="rgb" sig="fn rgb(r: i32, g: i32, b: i32) -> i32" desc="Create RGB color value"
        self.functions.insert(
            "rgb".into(),
            builtin(
                vec![("r", BcType::I32), ("g", BcType::I32), ("b", BcType::I32)],
                BcType::I32,
                true,
            ),
        );
        // @builtin category="Graphics" name="rgba" sig="fn rgba(r: i32, g: i32, b: i32, a: i32) -> i32" desc="Create RGBA color value"
        self.functions.insert(
            "rgba".into(),
            builtin(
                vec![
                    ("r", BcType::I32),
                    ("g", BcType::I32),
                    ("b", BcType::I32),
                    ("a", BcType::I32),
                ],
                BcType::I32,
                true,
            ),
        );

        // Image (fn!)
        // @builtin category="Image" name="img_load" sig="fn! img_load(data: str) -> Result<[i32], str>" desc="Decode PNG/JPEG/BMP/GIF image from memory"
        self.functions.insert(
            "img_load".into(),
            builtin(
                vec![("data", BcType::Str)],
                BcType::Result(
                    Box::new(BcType::Array(Box::new(BcType::I32))),
                    Box::new(BcType::Str),
                ),
                false,
            ),
        );

        // HashMap builtins (fn! except map_has and map_len which are pure reads)
        // @builtin category="HashMap" name="map_new" sig="fn! map_new() -> map" desc="Create empty hash map"
        self.functions
            .insert("map_new".into(), builtin(vec![], BcType::Map, false));
        // @builtin category="HashMap" name="map_set" sig="fn! map_set(m: map, key: str, value: str)" desc="Set key-value pair"
        self.functions.insert(
            "map_set".into(),
            builtin(
                vec![
                    ("m", BcType::Map),
                    ("key", BcType::Str),
                    ("value", BcType::Str),
                ],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_get" sig="fn! map_get(m: map, key: str) -> str" desc="Get value by key"
        self.functions.insert(
            "map_get".into(),
            builtin(
                vec![("m", BcType::Map), ("key", BcType::Str)],
                BcType::Str,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_has" sig="fn map_has(m: map, key: str) -> bool" desc="Check if key exists"
        self.functions.insert(
            "map_has".into(),
            builtin(
                vec![("m", BcType::Map), ("key", BcType::Str)],
                BcType::Bool,
                true,
            ),
        );
        // @builtin category="HashMap" name="map_delete" sig="fn! map_delete(m: map, key: str)" desc="Delete key from map"
        self.functions.insert(
            "map_delete".into(),
            builtin(
                vec![("m", BcType::Map), ("key", BcType::Str)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_len" sig="fn map_len(m: map) -> i32" desc="Number of entries in map"
        self.functions.insert(
            "map_len".into(),
            builtin(vec![("m", BcType::Map)], BcType::I32, true),
        );

        // Typed HashMap builtins: map_str_i32
        // @builtin category="HashMap" name="map_str_i32_new" sig="fn! map_str_i32_new() -> map_str_i32" desc="Create empty str→i32 map"
        self.functions
            .insert("map_str_i32_new".into(), builtin(vec![], BcType::MapStrI32, false));
        // @builtin category="HashMap" name="map_str_i32_set" sig="fn! map_str_i32_set(m: map_str_i32, key: str, value: i32)" desc="Set key-value pair"
        self.functions.insert(
            "map_str_i32_set".into(),
            builtin(
                vec![("m", BcType::MapStrI32), ("key", BcType::Str), ("value", BcType::I32)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_str_i32_get" sig="fn! map_str_i32_get(m: map_str_i32, key: str) -> i32" desc="Get value by key (0 if missing)"
        self.functions.insert(
            "map_str_i32_get".into(),
            builtin(
                vec![("m", BcType::MapStrI32), ("key", BcType::Str)],
                BcType::I32,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_str_i32_has" sig="fn map_str_i32_has(m: map_str_i32, key: str) -> bool" desc="Check if key exists"
        self.functions.insert(
            "map_str_i32_has".into(),
            builtin(
                vec![("m", BcType::MapStrI32), ("key", BcType::Str)],
                BcType::Bool,
                true,
            ),
        );
        // @builtin category="HashMap" name="map_str_i32_delete" sig="fn! map_str_i32_delete(m: map_str_i32, key: str)" desc="Delete key from map"
        self.functions.insert(
            "map_str_i32_delete".into(),
            builtin(
                vec![("m", BcType::MapStrI32), ("key", BcType::Str)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_str_i32_len" sig="fn map_str_i32_len(m: map_str_i32) -> i32" desc="Number of entries"
        self.functions.insert(
            "map_str_i32_len".into(),
            builtin(vec![("m", BcType::MapStrI32)], BcType::I32, true),
        );

        // Typed HashMap builtins: map_str_i64
        // @builtin category="HashMap" name="map_str_i64_new" sig="fn! map_str_i64_new() -> map_str_i64" desc="Create empty str→i64 map"
        self.functions
            .insert("map_str_i64_new".into(), builtin(vec![], BcType::MapStrI64, false));
        // @builtin category="HashMap" name="map_str_i64_set" sig="fn! map_str_i64_set(m: map_str_i64, key: str, value: i64)" desc="Set key-value pair"
        self.functions.insert(
            "map_str_i64_set".into(),
            builtin(
                vec![("m", BcType::MapStrI64), ("key", BcType::Str), ("value", BcType::I64)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_str_i64_get" sig="fn! map_str_i64_get(m: map_str_i64, key: str) -> i64" desc="Get value by key (0 if missing)"
        self.functions.insert(
            "map_str_i64_get".into(),
            builtin(
                vec![("m", BcType::MapStrI64), ("key", BcType::Str)],
                BcType::I64,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_str_i64_has" sig="fn map_str_i64_has(m: map_str_i64, key: str) -> bool" desc="Check if key exists"
        self.functions.insert(
            "map_str_i64_has".into(),
            builtin(
                vec![("m", BcType::MapStrI64), ("key", BcType::Str)],
                BcType::Bool,
                true,
            ),
        );
        // @builtin category="HashMap" name="map_str_i64_delete" sig="fn! map_str_i64_delete(m: map_str_i64, key: str)" desc="Delete key from map"
        self.functions.insert(
            "map_str_i64_delete".into(),
            builtin(
                vec![("m", BcType::MapStrI64), ("key", BcType::Str)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_str_i64_len" sig="fn map_str_i64_len(m: map_str_i64) -> i32" desc="Number of entries"
        self.functions.insert(
            "map_str_i64_len".into(),
            builtin(vec![("m", BcType::MapStrI64)], BcType::I32, true),
        );

        // Typed HashMap builtins: map_str_f64
        // @builtin category="HashMap" name="map_str_f64_new" sig="fn! map_str_f64_new() -> map_str_f64" desc="Create empty str→f64 map"
        self.functions
            .insert("map_str_f64_new".into(), builtin(vec![], BcType::MapStrF64, false));
        // @builtin category="HashMap" name="map_str_f64_set" sig="fn! map_str_f64_set(m: map_str_f64, key: str, value: f64)" desc="Set key-value pair"
        self.functions.insert(
            "map_str_f64_set".into(),
            builtin(
                vec![("m", BcType::MapStrF64), ("key", BcType::Str), ("value", BcType::F64)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_str_f64_get" sig="fn! map_str_f64_get(m: map_str_f64, key: str) -> f64" desc="Get value by key (0.0 if missing)"
        self.functions.insert(
            "map_str_f64_get".into(),
            builtin(
                vec![("m", BcType::MapStrF64), ("key", BcType::Str)],
                BcType::F64,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_str_f64_has" sig="fn map_str_f64_has(m: map_str_f64, key: str) -> bool" desc="Check if key exists"
        self.functions.insert(
            "map_str_f64_has".into(),
            builtin(
                vec![("m", BcType::MapStrF64), ("key", BcType::Str)],
                BcType::Bool,
                true,
            ),
        );
        // @builtin category="HashMap" name="map_str_f64_delete" sig="fn! map_str_f64_delete(m: map_str_f64, key: str)" desc="Delete key from map"
        self.functions.insert(
            "map_str_f64_delete".into(),
            builtin(
                vec![("m", BcType::MapStrF64), ("key", BcType::Str)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_str_f64_len" sig="fn map_str_f64_len(m: map_str_f64) -> i32" desc="Number of entries"
        self.functions.insert(
            "map_str_f64_len".into(),
            builtin(vec![("m", BcType::MapStrF64)], BcType::I32, true),
        );

        // Typed HashMap builtins: map_i32_str
        // @builtin category="HashMap" name="map_i32_str_new" sig="fn! map_i32_str_new() -> map_i32_str" desc="Create empty i32→str map"
        self.functions
            .insert("map_i32_str_new".into(), builtin(vec![], BcType::MapI32Str, false));
        // @builtin category="HashMap" name="map_i32_str_set" sig="fn! map_i32_str_set(m: map_i32_str, key: i32, value: str)" desc="Set key-value pair"
        self.functions.insert(
            "map_i32_str_set".into(),
            builtin(
                vec![("m", BcType::MapI32Str), ("key", BcType::I32), ("value", BcType::Str)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_i32_str_get" sig="fn! map_i32_str_get(m: map_i32_str, key: i32) -> str" desc="Get value by key (empty string if missing)"
        self.functions.insert(
            "map_i32_str_get".into(),
            builtin(
                vec![("m", BcType::MapI32Str), ("key", BcType::I32)],
                BcType::Str,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_i32_str_has" sig="fn! map_i32_str_has(m: map_i32_str, key: i32) -> bool" desc="Check if key exists"
        self.functions.insert(
            "map_i32_str_has".into(),
            builtin(
                vec![("m", BcType::MapI32Str), ("key", BcType::I32)],
                BcType::Bool,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_i32_str_delete" sig="fn! map_i32_str_delete(m: map_i32_str, key: i32)" desc="Delete key from map"
        self.functions.insert(
            "map_i32_str_delete".into(),
            builtin(
                vec![("m", BcType::MapI32Str), ("key", BcType::I32)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_i32_str_len" sig="fn map_i32_str_len(m: map_i32_str) -> i32" desc="Number of entries"
        self.functions.insert(
            "map_i32_str_len".into(),
            builtin(vec![("m", BcType::MapI32Str)], BcType::I32, true),
        );

        // Typed HashMap builtins: map_i32_i32
        // @builtin category="HashMap" name="map_i32_i32_new" sig="fn! map_i32_i32_new() -> map_i32_i32" desc="Create empty i32→i32 map"
        self.functions
            .insert("map_i32_i32_new".into(), builtin(vec![], BcType::MapI32I32, false));
        // @builtin category="HashMap" name="map_i32_i32_set" sig="fn! map_i32_i32_set(m: map_i32_i32, key: i32, value: i32)" desc="Set key-value pair"
        self.functions.insert(
            "map_i32_i32_set".into(),
            builtin(
                vec![("m", BcType::MapI32I32), ("key", BcType::I32), ("value", BcType::I32)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_i32_i32_get" sig="fn! map_i32_i32_get(m: map_i32_i32, key: i32) -> i32" desc="Get value by key (0 if missing)"
        self.functions.insert(
            "map_i32_i32_get".into(),
            builtin(
                vec![("m", BcType::MapI32I32), ("key", BcType::I32)],
                BcType::I32,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_i32_i32_has" sig="fn! map_i32_i32_has(m: map_i32_i32, key: i32) -> bool" desc="Check if key exists"
        self.functions.insert(
            "map_i32_i32_has".into(),
            builtin(
                vec![("m", BcType::MapI32I32), ("key", BcType::I32)],
                BcType::Bool,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_i32_i32_delete" sig="fn! map_i32_i32_delete(m: map_i32_i32, key: i32)" desc="Delete key from map"
        self.functions.insert(
            "map_i32_i32_delete".into(),
            builtin(
                vec![("m", BcType::MapI32I32), ("key", BcType::I32)],
                BcType::Unit,
                false,
            ),
        );
        // @builtin category="HashMap" name="map_i32_i32_len" sig="fn map_i32_i32_len(m: map_i32_i32) -> i32" desc="Number of entries"
        self.functions.insert(
            "map_i32_i32_len".into(),
            builtin(vec![("m", BcType::MapI32I32)], BcType::I32, true),
        );

        // File I/O — whole-file convenience (fn!)
        // @builtin category="File I/O" name="read_file" sig="fn! read_file(path: str) -> Result<str, str>" desc="Read entire file as string"
        self.functions.insert(
            "read_file".into(),
            builtin(
                vec![("path", BcType::Str)],
                BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)),
                false,
            ),
        );
        // @builtin category="File I/O" name="write_file" sig="fn! write_file(path: str, data: str) -> Result<str, str>" desc="Write string to file"
        self.functions.insert(
            "write_file".into(),
            builtin(
                vec![("path", BcType::Str), ("data", BcType::Str)],
                BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)),
                false,
            ),
        );

        // Math min/max/clamp (pure)
        // @builtin category="Math" name="min_i32" sig="fn min_i32(a: i32, b: i32) -> i32" desc="Minimum of two i32"
        self.functions.insert(
            "min_i32".into(),
            builtin(
                vec![("a", BcType::I32), ("b", BcType::I32)],
                BcType::I32,
                true,
            ),
        );
        // @builtin category="Math" name="max_i32" sig="fn max_i32(a: i32, b: i32) -> i32" desc="Maximum of two i32"
        self.functions.insert(
            "max_i32".into(),
            builtin(
                vec![("a", BcType::I32), ("b", BcType::I32)],
                BcType::I32,
                true,
            ),
        );
        // @builtin category="Math" name="clamp_i32" sig="fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32" desc="Clamp i32 to range"
        self.functions.insert(
            "clamp_i32".into(),
            builtin(
                vec![
                    ("v", BcType::I32),
                    ("lo", BcType::I32),
                    ("hi", BcType::I32),
                ],
                BcType::I32,
                true,
            ),
        );
        // @builtin category="Math" name="min_i64" sig="fn min_i64(a: i64, b: i64) -> i64" desc="Minimum of two i64"
        self.functions.insert(
            "min_i64".into(),
            builtin(
                vec![("a", BcType::I64), ("b", BcType::I64)],
                BcType::I64,
                true,
            ),
        );
        // @builtin category="Math" name="max_i64" sig="fn max_i64(a: i64, b: i64) -> i64" desc="Maximum of two i64"
        self.functions.insert(
            "max_i64".into(),
            builtin(
                vec![("a", BcType::I64), ("b", BcType::I64)],
                BcType::I64,
                true,
            ),
        );
        // @builtin category="Math" name="clamp_i64" sig="fn clamp_i64(v: i64, lo: i64, hi: i64) -> i64" desc="Clamp i64 to range"
        self.functions.insert(
            "clamp_i64".into(),
            builtin(
                vec![
                    ("v", BcType::I64),
                    ("lo", BcType::I64),
                    ("hi", BcType::I64),
                ],
                BcType::I64,
                true,
            ),
        );
        // @builtin category="Math" name="min_f64" sig="fn min_f64(a: f64, b: f64) -> f64" desc="Minimum of two f64"
        self.functions.insert(
            "min_f64".into(),
            builtin(
                vec![("a", BcType::F64), ("b", BcType::F64)],
                BcType::F64,
                true,
            ),
        );
        // @builtin category="Math" name="max_f64" sig="fn max_f64(a: f64, b: f64) -> f64" desc="Maximum of two f64"
        self.functions.insert(
            "max_f64".into(),
            builtin(
                vec![("a", BcType::F64), ("b", BcType::F64)],
                BcType::F64,
                true,
            ),
        );
        // @builtin category="Math" name="clamp_f64" sig="fn clamp_f64(v: f64, lo: f64, hi: f64) -> f64" desc="Clamp f64 to range"
        self.functions.insert(
            "clamp_f64".into(),
            builtin(
                vec![
                    ("v", BcType::F64),
                    ("lo", BcType::F64),
                    ("hi", BcType::F64),
                ],
                BcType::F64,
                true,
            ),
        );

        // String join (fn! — allocates)
        // @builtin category="String" name="str_join" sig="fn! str_join(arr: [str], sep: str) -> str" desc="Join string array with separator"
        self.functions.insert(
            "str_join".into(),
            builtin(
                vec![
                    ("arr", BcType::Array(Box::new(BcType::Str))),
                    ("sep", BcType::Str),
                ],
                BcType::Str,
                false,
            ),
        );

        // Path utilities — basename (pure), dirname (fn! — allocates)
        // @builtin category="Path" name="path_basename" sig="fn path_basename(path: str) -> str" desc="Get filename from path"
        self.functions.insert(
            "path_basename".into(),
            builtin(vec![("path", BcType::Str)], BcType::Str, true),
        );
        // @builtin category="Path" name="path_dirname" sig="fn! path_dirname(path: str) -> str" desc="Get directory from path"
        self.functions.insert(
            "path_dirname".into(),
            builtin(vec![("path", BcType::Str)], BcType::Str, false),
        );

        // len and push are special-cased in check_call
    }

    // -----------------------------------------------------------------------
    // Pass 1: Name Collection
    // -----------------------------------------------------------------------

    fn collect_decl(&mut self, decl: &TopDecl) -> Result<(), CompileError> {
        match decl {
            TopDecl::Struct(s) => {
                let mut fields = Vec::new();
                for f in &s.fields {
                    fields.push((f.name.clone(), self.resolve_type(&f.ty)?));
                }
                if self.structs.contains_key(&s.name) {
                    return Err(CompileError::new(
                        s.span,
                        format!("duplicate struct '{}'", s.name),
                    ));
                }
                self.structs.insert(s.name.clone(), StructInfo { fields });
            }
            TopDecl::Enum(e) => {
                let mut variants = Vec::new();
                for v in &e.variants {
                    let mut payload = Vec::new();
                    for t in &v.payload_types {
                        payload.push(self.resolve_type(t)?);
                    }
                    variants.push((v.name.clone(), payload));
                }
                if self.enums.contains_key(&e.name) {
                    return Err(CompileError::new(
                        e.span,
                        format!("duplicate enum '{}'", e.name),
                    ));
                }
                self.enums.insert(e.name.clone(), EnumInfo { variants });
            }
            TopDecl::Fn(f) => {
                let mut params = Vec::new();
                for p in &f.params {
                    params.push((p.name.clone(), self.resolve_type(&p.ty)?));
                }
                let return_type = match &f.return_type {
                    Some(t) => self.resolve_type(t)?,
                    None => BcType::Unit,
                };
                if self.functions.contains_key(&f.name) {
                    return Err(CompileError::new(
                        f.span,
                        format!("duplicate function '{}'", f.name),
                    ));
                }
                self.functions.insert(
                    f.name.clone(),
                    FunctionInfo {
                        params,
                        return_type,
                        is_pure: f.is_pure,
                        is_extern: false,
                    },
                );
            }
            TopDecl::Let(l) => {
                let ty = self.resolve_type(&l.ty)?;
                if self.constants.contains_key(&l.name) {
                    return Err(CompileError::new(
                        l.span,
                        format!("duplicate constant '{}'", l.name),
                    ));
                }
                self.constants.insert(l.name.clone(), ConstInfo { ty });
            }
            TopDecl::Extern(eb) => {
                for ef in &eb.decls {
                    let mut params = Vec::new();
                    for p in &ef.params {
                        params.push((p.name.clone(), self.resolve_type(&p.ty)?));
                    }
                    let return_type = match &ef.return_type {
                        Some(t) => self.resolve_type(t)?,
                        None => BcType::Unit,
                    };
                    if self.functions.contains_key(&ef.name) {
                        return Err(CompileError::new(
                            ef.span,
                            format!("duplicate extern function '{}'", ef.name),
                        ));
                    }
                    self.functions.insert(
                        ef.name.clone(),
                        FunctionInfo {
                            params,
                            return_type,
                            is_pure: false,
                            is_extern: true,
                        },
                    );
                }
            }
            TopDecl::Use(..) => {}
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Pass 2: Type Checking
    // -----------------------------------------------------------------------

    fn check_decl(&mut self, decl: &TopDecl) -> Result<(), CompileError> {
        match decl {
            TopDecl::Fn(f) => self.check_fn(f),
            TopDecl::Let(l) => {
                let expected = self.resolve_type(&l.ty)?;
                // Top-level lets are checked without local scope
                self.scopes.push(HashMap::new());
                let actual = self.check_expr(&l.value, Some(&expected))?;
                self.scopes.pop();
                if actual != expected {
                    return Err(CompileError::new(
                        l.span,
                        format!(
                            "constant '{}': expected {}, got {}",
                            l.name, expected, actual
                        ),
                    ));
                }
                Ok(())
            }
            // Structs, enums, externs already fully collected in pass 1
            _ => Ok(()),
        }
    }

    fn check_fn(&mut self, func: &FnDecl) -> Result<(), CompileError> {
        let return_type = match &func.return_type {
            Some(t) => self.resolve_type(t)?,
            None => BcType::Unit,
        };

        self.current_fn_return_type = Some(return_type.clone());
        self.in_pure_fn = func.is_pure;

        // Function scope for parameters
        self.scopes.push(HashMap::new());
        for p in &func.params {
            let ty = self.resolve_type(&p.ty)?;
            // Parameters go into the function scope directly (no anti-shadow check
            // against global scope — anti-shadowing is within-function only)
            let scope = self.scopes.last_mut().unwrap();
            if scope.contains_key(&p.name) {
                return Err(CompileError::new(
                    p.span,
                    format!("duplicate parameter '{}'", p.name),
                ));
            }
            scope.insert(p.name.clone(), BindingInfo { ty, is_mut: false });
        }

        let body_ty = self.check_block(&func.body, Some(&return_type))?;

        if body_ty != return_type {
            return Err(CompileError::new(
                func.span,
                format!(
                    "function '{}': expected return type {}, got {}",
                    func.name, return_type, body_ty
                ),
            ));
        }

        // Check for resource-acquiring calls without matching defer cleanup
        if self.show_warnings {
            Self::check_resource_leaks(&func.body, &func.name);
        }

        self.scopes.pop();
        self.current_fn_return_type = None;
        self.in_pure_fn = false;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Resource-leak warnings: detect acquire calls without matching defer
    // -----------------------------------------------------------------------

    /// Extract the callee name from a Call expression, if it's a simple identifier.
    fn call_name(expr: &Expr) -> Option<&str> {
        if let Expr::Call { callee, .. } = expr {
            if let Expr::Ident(name, _) = callee.as_ref() {
                return Some(name.as_str());
            }
        }
        None
    }

    /// Collect all function-call names that appear anywhere in a block (recursively).
    fn collect_calls_in_block(block: &Block, out: &mut HashSet<String>) {
        for stmt in &block.stmts {
            Self::collect_calls_in_stmt(stmt, out);
        }
        if let Some(tail) = &block.tail_expr {
            Self::collect_calls_in_expr(tail, out);
        }
    }

    fn collect_calls_in_stmt(stmt: &Stmt, out: &mut HashSet<String>) {
        match stmt {
            Stmt::Let(ls) => Self::collect_calls_in_expr(&ls.value, out),
            Stmt::Assign(a) => {
                Self::collect_calls_in_place(&a.target, out);
                Self::collect_calls_in_expr(&a.value, out);
            }
            Stmt::CompoundAssign(ca) => {
                Self::collect_calls_in_place(&ca.target, out);
                Self::collect_calls_in_expr(&ca.value, out);
            }
            Stmt::Expr(es) => Self::collect_calls_in_expr(&es.expr, out),
            Stmt::While(w) => {
                Self::collect_calls_in_expr(&w.condition, out);
                Self::collect_calls_in_block(&w.body, out);
            }
            Stmt::For(f) => {
                Self::collect_calls_in_expr(&f.start, out);
                Self::collect_calls_in_expr(&f.end, out);
                Self::collect_calls_in_block(&f.body, out);
            }
            Stmt::ForIn(fi) => {
                Self::collect_calls_in_expr(&fi.iterable, out);
                Self::collect_calls_in_block(&fi.body, out);
            }
            Stmt::Return(r) => {
                if let Some(val) = &r.value {
                    Self::collect_calls_in_expr(val, out);
                }
            }
            Stmt::Defer(_) | Stmt::Break(_) | Stmt::Continue(_) => {}
        }
    }

    fn collect_calls_in_place(place: &Place, out: &mut HashSet<String>) {
        for acc in &place.accessors {
            if let PlaceAccessor::Index(idx_expr) = acc {
                Self::collect_calls_in_expr(idx_expr, out);
            }
        }
    }

    fn collect_calls_in_expr(expr: &Expr, out: &mut HashSet<String>) {
        match expr {
            Expr::Call { callee, args, .. } => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    out.insert(name.clone());
                }
                Self::collect_calls_in_expr(callee, out);
                for arg in args {
                    Self::collect_calls_in_expr(arg, out);
                }
            }
            Expr::BinaryOp { left, right, .. } => {
                Self::collect_calls_in_expr(left, out);
                Self::collect_calls_in_expr(right, out);
            }
            Expr::UnaryOp { operand, .. } => Self::collect_calls_in_expr(operand, out),
            Expr::Cast { expr, .. } => Self::collect_calls_in_expr(expr, out),
            Expr::FieldAccess { expr, .. } => Self::collect_calls_in_expr(expr, out),
            Expr::Index { expr, index, .. } => {
                Self::collect_calls_in_expr(expr, out);
                Self::collect_calls_in_expr(index, out);
            }
            Expr::Block(block) => Self::collect_calls_in_block(block, out),
            Expr::If { condition, then_block, else_branch, .. } => {
                Self::collect_calls_in_expr(condition, out);
                Self::collect_calls_in_block(then_block, out);
                if let Some(eb) = else_branch {
                    Self::collect_calls_in_expr(eb, out);
                }
            }
            Expr::Match { scrutinee, arms, .. } => {
                Self::collect_calls_in_expr(scrutinee, out);
                for arm in arms {
                    Self::collect_calls_in_expr(&arm.body, out);
                }
            }
            Expr::StructLit { fields, .. } => {
                for fi in fields {
                    Self::collect_calls_in_expr(&fi.value, out);
                }
            }
            Expr::ArrayLit { elements, .. } => {
                for el in elements {
                    Self::collect_calls_in_expr(el, out);
                }
            }
            Expr::EnumConstructor { args, .. } => {
                for arg in args {
                    Self::collect_calls_in_expr(arg, out);
                }
            }
            Expr::InterpolatedString { parts, .. } => {
                for part in parts {
                    if let InterpolatedStringPart::Expr(e) = part {
                        Self::collect_calls_in_expr(e, out);
                    }
                }
            }
            Expr::Try { call, .. } => Self::collect_calls_in_expr(call, out),
            Expr::Arena { body, .. } => Self::collect_calls_in_block(body, out),
            Expr::IntLit(..) | Expr::FloatLit(..) | Expr::StringLit(..)
            | Expr::BoolLit(..) | Expr::Ident(..) => {}
        }
    }

    /// Collect callee names from top-level defer statements in a block.
    fn collect_deferred_calls(block: &Block) -> HashSet<String> {
        let mut deferred = HashSet::new();
        for stmt in &block.stmts {
            if let Stmt::Defer(d) = stmt {
                if let Some(name) = Self::call_name(&d.expr) {
                    deferred.insert(name.to_string());
                }
            }
        }
        deferred
    }

    /// Emit warnings for resource-acquiring calls without a matching defer cleanup.
    fn check_resource_leaks(body: &Block, fn_name: &str) {
        let mut acquired_calls: HashSet<String> = HashSet::new();
        Self::collect_calls_in_block(body, &mut acquired_calls);
        let deferred = Self::collect_deferred_calls(body);

        for &(acquire_fn, cleanup_fn) in RESOURCE_PAIRS {
            if acquired_calls.contains(acquire_fn) && !deferred.contains(cleanup_fn) {
                eprintln!(
                    "warning: '{}' called in '{}' without a matching 'defer {}(...)' \
                     — resource may leak",
                    acquire_fn, fn_name, cleanup_fn
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Block & statement checking
    // -----------------------------------------------------------------------

    fn check_block(
        &mut self,
        block: &Block,
        expected: Option<&BcType>,
    ) -> Result<BcType, CompileError> {
        self.push_scope();
        for stmt in &block.stmts {
            self.check_stmt(stmt)?;
        }
        let ty = if let Some(tail) = &block.tail_expr {
            self.check_expr(tail, expected)?
        } else {
            BcType::Unit
        };
        self.pop_scope();
        Ok(ty)
    }

    fn check_stmt(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        match stmt {
            Stmt::Let(ls) => {
                let expected = self.resolve_type(&ls.ty)?;
                let actual = self.check_expr(&ls.value, Some(&expected))?;
                if actual != expected {
                    return Err(CompileError::new(
                        ls.span,
                        format!("let '{}': expected {}, got {}", ls.name, expected, actual),
                    ));
                }
                self.add_binding(&ls.name, expected, ls.is_mut, ls.span)?;
                Ok(())
            }
            Stmt::Assign(a) => {
                // Check target is mutable (skip for array element access — arrays are references)
                let target_info = self
                    .lookup_var(&a.target.name)
                    .ok_or_else(|| {
                        CompileError::new(a.span, format!("undefined variable '{}'", a.target.name))
                    })?
                    .clone();
                let has_index = a
                    .target
                    .accessors
                    .iter()
                    .any(|acc| matches!(acc, PlaceAccessor::Index(_)));
                if !target_info.is_mut && !has_index {
                    return Err(CompileError::new(
                        a.span,
                        format!("cannot assign to immutable variable '{}'", a.target.name),
                    ));
                }

                // Walk through accessors to find the target type
                let mut ty = target_info.ty.clone();
                for acc in &a.target.accessors {
                    match acc {
                        PlaceAccessor::Field(fname) => {
                            ty = self.field_type(&ty, fname, a.span)?;
                        }
                        PlaceAccessor::Index(idx_expr) => {
                            if ty == BcType::Str {
                                return Err(CompileError::new(
                                    a.span,
                                    "cannot assign to string index: strings are immutable"
                                        .to_string(),
                                ));
                            }
                            let idx_ty = self.check_expr(idx_expr, None)?;
                            if idx_ty != BcType::I32 {
                                return Err(CompileError::new(
                                    a.span,
                                    format!("array index must be i32, got {}", idx_ty),
                                ));
                            }
                            ty = self.element_type(&ty, a.span)?;
                        }
                    }
                }

                let val_ty = self.check_expr(&a.value, Some(&ty))?;
                if val_ty != ty {
                    return Err(CompileError::new(
                        a.span,
                        format!("assignment type mismatch: expected {}, got {}", ty, val_ty),
                    ));
                }
                Ok(())
            }
            Stmt::Expr(es) => {
                self.check_expr(&es.expr, None)?;
                Ok(())
            }
            Stmt::While(w) => {
                let cond_ty = self.check_expr(&w.condition, None)?;
                if cond_ty != BcType::Bool {
                    return Err(CompileError::new(
                        w.span,
                        format!("while condition must be bool, got {}", cond_ty),
                    ));
                }
                self.loop_depth += 1;
                self.check_block(&w.body, None)?;
                self.loop_depth -= 1;
                Ok(())
            }
            Stmt::For(f) => {
                let start_ty = self.check_expr(&f.start, None)?;
                let end_ty = self.check_expr(&f.end, None)?;
                if start_ty != BcType::I32 {
                    return Err(CompileError::new(
                        f.span,
                        format!("for range start must be i32, got {}", start_ty),
                    ));
                }
                if end_ty != BcType::I32 {
                    return Err(CompileError::new(
                        f.span,
                        format!("for range end must be i32, got {}", end_ty),
                    ));
                }
                // Create scope for loop variable + body
                self.push_scope();
                self.add_binding(&f.var, BcType::I32, false, f.span)?;
                self.loop_depth += 1;
                // Check body in a nested scope
                for stmt in &f.body.stmts {
                    self.check_stmt(stmt)?;
                }
                if let Some(tail) = &f.body.tail_expr {
                    self.check_expr(tail, None)?;
                }
                self.loop_depth -= 1;
                self.pop_scope();
                Ok(())
            }
            Stmt::ForIn(fi) => {
                let arr_ty = self.check_expr(&fi.iterable, None)?;
                let elem_ty = match &arr_ty {
                    BcType::Array(e) | BcType::FixedArray(e, _) => (**e).clone(),
                    _ => {
                        return Err(CompileError::new(
                            fi.span,
                            format!("for-in requires an array type, got {}", arr_ty),
                        ))
                    }
                };
                self.push_scope();
                self.add_binding(&fi.var, elem_ty, false, fi.span)?;
                self.loop_depth += 1;
                for stmt in &fi.body.stmts {
                    self.check_stmt(stmt)?;
                }
                if let Some(tail) = &fi.body.tail_expr {
                    self.check_expr(tail, None)?;
                }
                self.loop_depth -= 1;
                self.pop_scope();
                Ok(())
            }
            Stmt::Break(span) => {
                if self.loop_depth == 0 {
                    return Err(CompileError::new(
                        *span,
                        "break used outside of a loop".to_string(),
                    ));
                }
                Ok(())
            }
            Stmt::Continue(span) => {
                if self.loop_depth == 0 {
                    return Err(CompileError::new(
                        *span,
                        "continue used outside of a loop".to_string(),
                    ));
                }
                Ok(())
            }
            Stmt::CompoundAssign(ca) => {
                // Check target is mutable (skip for array element access — arrays are references)
                let target_info = self
                    .lookup_var(&ca.target.name)
                    .ok_or_else(|| {
                        CompileError::new(
                            ca.span,
                            format!("undefined variable '{}'", ca.target.name),
                        )
                    })?
                    .clone();
                let has_index = ca
                    .target
                    .accessors
                    .iter()
                    .any(|acc| matches!(acc, PlaceAccessor::Index(_)));
                if !target_info.is_mut && !has_index {
                    return Err(CompileError::new(
                        ca.span,
                        format!("cannot assign to immutable variable '{}'", ca.target.name),
                    ));
                }

                // Walk through accessors to find the target type
                let mut ty = target_info.ty.clone();
                for acc in &ca.target.accessors {
                    match acc {
                        PlaceAccessor::Field(fname) => {
                            ty = self.field_type(&ty, fname, ca.span)?;
                        }
                        PlaceAccessor::Index(idx_expr) => {
                            let idx_ty = self.check_expr(idx_expr, None)?;
                            if idx_ty != BcType::I32 {
                                return Err(CompileError::new(
                                    ca.span,
                                    format!("array index must be i32, got {}", idx_ty),
                                ));
                            }
                            ty = self.element_type(&ty, ca.span)?;
                        }
                    }
                }

                // Type-check the RHS value
                let val_ty = self.check_expr(&ca.value, Some(&ty))?;
                // Check that the operation is valid for these types
                self.check_binary_op_types(ca.op, &ty, &val_ty, ca.span)?;
                Ok(())
            }
            Stmt::Return(r) => {
                let fn_ret = self.current_fn_return_type.clone().unwrap_or(BcType::Unit);
                match &r.value {
                    Some(expr) => {
                        let ty = self.check_expr(expr, Some(&fn_ret))?;
                        if ty != fn_ret {
                            return Err(CompileError::new(
                                r.span,
                                format!("return type mismatch: expected {}, got {}", fn_ret, ty),
                            ));
                        }
                    }
                    None => {
                        if fn_ret != BcType::Unit {
                            return Err(CompileError::new(
                                r.span,
                                format!("return without value in function returning {}", fn_ret),
                            ));
                        }
                    }
                }
                Ok(())
            }
            Stmt::Defer(d) => {
                if self.in_pure_fn {
                    return Err(CompileError::new(
                        d.span,
                        "defer is only allowed in impure functions (fn!)",
                    ));
                }
                // The deferred expression must be a function call
                if !matches!(d.expr, Expr::Call { .. }) {
                    return Err(CompileError::new(
                        d.span,
                        "defer requires a function call expression",
                    ));
                }
                self.check_expr(&d.expr, None)?;
                Ok(())
            }
        }
    }

    fn check_expr(
        &mut self,
        expr: &Expr,
        expected: Option<&BcType>,
    ) -> Result<BcType, CompileError> {
        match expr {
            Expr::IntLit(value, span) => {
                if *value < i32::MIN as i64 || *value > i32::MAX as i64 {
                    return Err(CompileError::new(
                        *span,
                        "integer literal out of range for i32; build larger i64 values from in-range i32 values with explicit casts",
                    ));
                }
                Ok(BcType::I32)
            }
            Expr::FloatLit(_, _) => Ok(BcType::F64),
            Expr::StringLit(_, _) => Ok(BcType::Str),
            Expr::InterpolatedString { parts, span } => {
                let mut rendered_parts = 0usize;
                let mut needs_impure_ops = false;

                for part in parts {
                    match part {
                        InterpolatedStringPart::Text(text) => {
                            if !text.is_empty() {
                                rendered_parts += 1;
                            }
                        }
                        InterpolatedStringPart::Expr(expr) => {
                            self.ensure_interpolation_expr_pure(expr)?;
                            let ty = self.check_expr(expr, None)?;
                            match ty {
                                BcType::Str => {}
                                BcType::Bool => {}
                                BcType::I32 | BcType::I64 | BcType::F64 => {
                                    needs_impure_ops = true;
                                }
                                _ => {
                                    return Err(CompileError::new(
                                        expr.span(),
                                        format!(
                                            "interpolated string expressions must be str, i32, i64, f64, or bool, got {}",
                                            ty
                                        ),
                                    ));
                                }
                            }
                            rendered_parts += 1;
                        }
                    }
                }

                if rendered_parts > 1 {
                    needs_impure_ops = true;
                }

                if self.in_pure_fn && needs_impure_ops {
                    return Err(CompileError::new(
                        *span,
                        "pure function cannot use interpolated string that allocates",
                    ));
                }

                Ok(BcType::Str)
            }
            Expr::BoolLit(_, _) => Ok(BcType::Bool),

            Expr::Ident(name, span) => {
                // Local scopes first
                if let Some(info) = self.lookup_var(name) {
                    return Ok(info.ty.clone());
                }
                // Global constants
                if let Some(info) = self.constants.get(name) {
                    return Ok(info.ty.clone());
                }
                // Function name used as a value → function pointer
                if let Some(fi) = self.functions.get(name) {
                    if fi.is_extern {
                        return Err(CompileError::new(
                            *span,
                            format!("extern function '{}' cannot be used as a value", name),
                        ));
                    }
                    let param_types: Vec<BcType> = fi.params.iter().map(|(_, t)| t.clone()).collect();
                    let ret_type = fi.return_type.clone();
                    return Ok(BcType::FnPtr(param_types, Box::new(ret_type)));
                }
                Err(CompileError::new(
                    *span,
                    format!("undefined variable '{}'", name),
                ))
            }

            Expr::BinaryOp {
                op,
                left,
                right,
                span,
            } => {
                let lt = self.check_expr(left, None)?;
                let rt = self.check_expr(right, None)?;
                match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                        if lt != rt {
                            return Err(CompileError::new(
                                *span,
                                format!("binary op type mismatch: {} vs {}", lt, rt),
                            ));
                        }
                        match &lt {
                            BcType::I32 | BcType::I64 | BcType::F64 => Ok(lt),
                            _ => Err(CompileError::new(
                                *span,
                                format!("arithmetic not supported for {}", lt),
                            )),
                        }
                    }
                    BinOp::Mod => {
                        if lt != rt {
                            return Err(CompileError::new(
                                *span,
                                format!("modulo type mismatch: {} vs {}", lt, rt),
                            ));
                        }
                        match &lt {
                            BcType::I32 | BcType::I64 => Ok(lt),
                            _ => Err(CompileError::new(
                                *span,
                                format!("modulo not supported for {}", lt),
                            )),
                        }
                    }
                    BinOp::Eq | BinOp::Neq => {
                        if lt != rt {
                            return Err(CompileError::new(
                                *span,
                                format!("equality type mismatch: {} vs {}", lt, rt),
                            ));
                        }
                        Ok(BcType::Bool)
                    }
                    BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
                        if lt != rt {
                            return Err(CompileError::new(
                                *span,
                                format!("comparison type mismatch: {} vs {}", lt, rt),
                            ));
                        }
                        match &lt {
                            BcType::I32 | BcType::I64 | BcType::F64 | BcType::Str => {
                                Ok(BcType::Bool)
                            }
                            _ => Err(CompileError::new(
                                *span,
                                format!("comparison not supported for {}", lt),
                            )),
                        }
                    }
                    BinOp::And | BinOp::Or => {
                        if lt != BcType::Bool {
                            return Err(CompileError::new(
                                *span,
                                format!("logical op expects bool, got {}", lt),
                            ));
                        }
                        if rt != BcType::Bool {
                            return Err(CompileError::new(
                                *span,
                                format!("logical op expects bool, got {}", rt),
                            ));
                        }
                        Ok(BcType::Bool)
                    }
                }
            }

            Expr::UnaryOp { op, operand, span } => {
                let t = self.check_expr(operand, None)?;
                match op {
                    UnaryOp::Neg => match &t {
                        BcType::I32 | BcType::I64 | BcType::F64 => Ok(t),
                        _ => Err(CompileError::new(
                            *span,
                            format!("negation not supported for {}", t),
                        )),
                    },
                    UnaryOp::Not => {
                        if t != BcType::Bool {
                            return Err(CompileError::new(
                                *span,
                                format!("'not' expects bool, got {}", t),
                            ));
                        }
                        Ok(BcType::Bool)
                    }
                }
            }

            Expr::Cast {
                expr: inner,
                ty,
                span,
            } => {
                let from = self.check_expr(inner, None)?;
                let to = self.resolve_type(ty)?;
                let valid = matches!(
                    (&from, &to),
                    (BcType::I32, BcType::I64)
                        | (BcType::I64, BcType::I32)
                        | (BcType::I32, BcType::F64)
                        | (BcType::I64, BcType::F64)
                        | (BcType::F64, BcType::I32)
                        | (BcType::F64, BcType::I64)
                );
                if !valid {
                    return Err(CompileError::new(
                        *span,
                        format!("invalid cast: {} as {}", from, to),
                    ));
                }
                Ok(to)
            }

            Expr::Call { callee, args, span } => self.check_call(callee, args, *span),

            Expr::FieldAccess {
                expr: obj,
                field,
                span,
            } => {
                let obj_ty = self.check_expr(obj, None)?;
                self.field_type(&obj_ty, field, *span)
            }

            Expr::Index {
                expr: arr,
                index,
                span,
            } => {
                let arr_ty = self.check_expr(arr, None)?;
                let idx_ty = self.check_expr(index, None)?;
                if idx_ty != BcType::I32 {
                    return Err(CompileError::new(
                        *span,
                        format!("array index must be i32, got {}", idx_ty),
                    ));
                }
                if arr_ty == BcType::Str {
                    Ok(BcType::I32)
                } else {
                    self.element_type(&arr_ty, *span)
                }
            }

            Expr::Block(block) => self.check_block(block, expected),

            Expr::Arena { body, span } => {
                if self.in_pure_fn {
                    return Err(CompileError::new(
                        *span,
                        "arena blocks are only allowed in impure functions (fn!)",
                    ));
                }
                let ty = self.check_block(body, expected)?;
                // Escape analysis: only primitives can escape the arena block
                if !Self::is_arena_safe_type(&ty) {
                    return Err(CompileError::new(
                        *span,
                        format!(
                            "arena block cannot return arena-allocated type '{}' — only primitives can escape",
                            ty
                        ),
                    ));
                }
                Ok(ty)
            }

            Expr::If {
                condition,
                then_block,
                else_branch,
                span,
            } => {
                let cond_ty = self.check_expr(condition, None)?;
                if cond_ty != BcType::Bool {
                    return Err(CompileError::new(
                        condition.span(),
                        format!("if condition must be bool, got {}", cond_ty),
                    ));
                }
                let then_ty = self.check_block(then_block, expected)?;
                if let Some(else_expr) = else_branch {
                    let else_ty = self.check_expr(else_expr, expected)?;
                    if then_ty != else_ty {
                        return Err(CompileError::new(
                            *span,
                            format!("if/else type mismatch: {} vs {}", then_ty, else_ty),
                        ));
                    }
                    Ok(then_ty)
                } else {
                    if then_ty != BcType::Unit {
                        return Err(CompileError::new(
                            *span,
                            "if without else must have unit type",
                        ));
                    }
                    Ok(BcType::Unit)
                }
            }

            Expr::Match {
                scrutinee,
                arms,
                span,
            } => {
                let scrut_ty = self.check_expr(scrutinee, None)?;
                self.check_match(&scrut_ty, arms, *span, expected)
            }

            Expr::Try { call, span } => {
                let call_ty = self.check_expr(call, None)?;
                match call_ty {
                    BcType::Result(ok_ty, err_ty) => match &self.current_fn_return_type {
                        Some(BcType::Result(_, fn_err)) => {
                            if *err_ty != **fn_err {
                                return Err(CompileError::new(
                                    *span,
                                    format!("try error type mismatch: {} vs {}", err_ty, fn_err),
                                ));
                            }
                            Ok(*ok_ty)
                        }
                        _ => Err(CompileError::new(
                            *span,
                            "try can only be used in a function returning Result",
                        )),
                    },
                    _ => Err(CompileError::new(
                        *span,
                        format!("try requires Result, got {}", call_ty),
                    )),
                }
            }

            Expr::ArrayLit { elements, span } => {
                if elements.is_empty() {
                    // Infer from expected type
                    if let Some(BcType::Array(elem)) = expected {
                        return Ok(BcType::Array(elem.clone()));
                    }
                    if let Some(BcType::FixedArray(elem, _)) = expected {
                        return Ok(BcType::Array(elem.clone()));
                    }
                    return Err(CompileError::new(
                        *span,
                        "cannot infer element type of empty array",
                    ));
                }
                let first_ty = self.check_expr(&elements[0], None)?;
                for elem in elements.iter().skip(1) {
                    let ty = self.check_expr(elem, None)?;
                    if ty != first_ty {
                        return Err(CompileError::new(
                            elem.span(),
                            format!("array element type mismatch: {} vs {}", first_ty, ty),
                        ));
                    }
                }
                // If the expected type is a fixed-size array with matching length, return that.
                if let Some(BcType::FixedArray(elem, size)) = expected {
                    if **elem == first_ty && *size == elements.len() as i64 {
                        return Ok(BcType::FixedArray(Box::new(first_ty), *size));
                    }
                }
                Ok(BcType::Array(Box::new(first_ty)))
            }

            Expr::StructLit { name, fields, span } => {
                let info = self
                    .structs
                    .get(name)
                    .ok_or_else(|| {
                        CompileError::new(*span, format!("undefined struct '{}'", name))
                    })?
                    .clone();

                // Check all required fields are present
                let mut provided: HashSet<String> = HashSet::new();
                for fi in fields {
                    if !provided.insert(fi.name.clone()) {
                        return Err(CompileError::new(
                            fi.span,
                            format!("duplicate field '{}'", fi.name),
                        ));
                    }
                    let expected_field_ty = info
                        .fields
                        .iter()
                        .find(|(n, _)| n == &fi.name)
                        .map(|(_, t)| t)
                        .ok_or_else(|| {
                            CompileError::new(
                                fi.span,
                                format!("struct '{}' has no field '{}'", name, fi.name),
                            )
                        })?;
                    let actual = self.check_expr(&fi.value, Some(expected_field_ty))?;
                    if actual != *expected_field_ty {
                        return Err(CompileError::new(
                            fi.span,
                            format!(
                                "field '{}': expected {}, got {}",
                                fi.name, expected_field_ty, actual
                            ),
                        ));
                    }
                }
                let expected_fields: HashSet<String> =
                    info.fields.iter().map(|(n, _)| n.clone()).collect();
                let missing: Vec<_> = expected_fields.difference(&provided).collect();
                if !missing.is_empty() {
                    return Err(CompileError::new(
                        *span,
                        format!("missing fields: {:?}", missing),
                    ));
                }
                Ok(BcType::Struct(name.clone()))
            }

            Expr::EnumConstructor {
                enum_name,
                variant,
                args,
                span,
            } => {
                if enum_name == "Result" {
                    return self.check_result_constructor(variant, args, *span, expected);
                }
                let info = self
                    .enums
                    .get(enum_name)
                    .ok_or_else(|| {
                        CompileError::new(*span, format!("undefined enum '{}'", enum_name))
                    })?
                    .clone();
                let var_info = info
                    .variants
                    .iter()
                    .find(|(n, _)| n == variant)
                    .ok_or_else(|| {
                        CompileError::new(
                            *span,
                            format!("enum '{}' has no variant '{}'", enum_name, variant),
                        )
                    })?;
                if args.len() != var_info.1.len() {
                    return Err(CompileError::new(
                        *span,
                        format!(
                            "'{}::{}' expects {} args, got {}",
                            enum_name,
                            variant,
                            var_info.1.len(),
                            args.len()
                        ),
                    ));
                }
                for (arg, expected_ty) in args.iter().zip(var_info.1.iter()) {
                    let actual = self.check_expr(arg, Some(expected_ty))?;
                    if actual != *expected_ty {
                        return Err(CompileError::new(
                            arg.span(),
                            format!("expected {}, got {}", expected_ty, actual),
                        ));
                    }
                }
                Ok(BcType::Enum(enum_name.clone()))
            }
        }
    }

    fn ensure_interpolation_expr_pure(&self, expr: &Expr) -> Result<(), CompileError> {
        match expr {
            Expr::IntLit(_, _)
            | Expr::FloatLit(_, _)
            | Expr::StringLit(_, _)
            | Expr::BoolLit(_, _)
            | Expr::Ident(_, _) => Ok(()),
            Expr::InterpolatedString { parts, .. } => {
                for part in parts {
                    if let InterpolatedStringPart::Expr(expr) = part {
                        self.ensure_interpolation_expr_pure(expr)?;
                    }
                }
                Ok(())
            }
            Expr::BinaryOp { left, right, .. } => {
                self.ensure_interpolation_expr_pure(left)?;
                self.ensure_interpolation_expr_pure(right)
            }
            Expr::UnaryOp { operand, .. } => self.ensure_interpolation_expr_pure(operand),
            Expr::Cast { expr, .. } => self.ensure_interpolation_expr_pure(expr),
            Expr::Call { callee, args, span } => {
                let name = match callee.as_ref() {
                    Expr::Ident(name, _) => name,
                    _ => return Err(CompileError::new(*span, "expected function name")),
                };

                if name == "push" || name == "pop" {
                    return Err(CompileError::new(
                        *span,
                        format!(
                            "interpolated string expressions cannot call impure function '{}'",
                            name
                        ),
                    ));
                }

                if name != "len"
                    && self
                        .functions
                        .get(name)
                        .is_some_and(|func| !func.is_pure || func.is_extern)
                {
                    return Err(CompileError::new(
                        *span,
                        format!(
                            "interpolated string expressions cannot call impure function '{}'",
                            name
                        ),
                    ));
                }

                for arg in args {
                    self.ensure_interpolation_expr_pure(arg)?;
                }
                Ok(())
            }
            Expr::FieldAccess { expr, .. } => self.ensure_interpolation_expr_pure(expr),
            Expr::Index { expr, index, .. } => {
                self.ensure_interpolation_expr_pure(expr)?;
                self.ensure_interpolation_expr_pure(index)
            }
            Expr::Block(block) => self.ensure_interpolation_block_pure(block),
            Expr::If {
                condition,
                then_block,
                else_branch,
                ..
            } => {
                self.ensure_interpolation_expr_pure(condition)?;
                self.ensure_interpolation_block_pure(then_block)?;
                if let Some(else_expr) = else_branch {
                    self.ensure_interpolation_expr_pure(else_expr)?;
                }
                Ok(())
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.ensure_interpolation_expr_pure(scrutinee)?;
                for arm in arms {
                    self.ensure_interpolation_expr_pure(&arm.body)?;
                }
                Ok(())
            }
            Expr::Try { call, .. } => self.ensure_interpolation_expr_pure(call),
            Expr::ArrayLit { elements, .. } => {
                for element in elements {
                    self.ensure_interpolation_expr_pure(element)?;
                }
                Ok(())
            }
            Expr::StructLit { fields, .. } => {
                for field in fields {
                    self.ensure_interpolation_expr_pure(&field.value)?;
                }
                Ok(())
            }
            Expr::EnumConstructor { args, .. } => {
                for arg in args {
                    self.ensure_interpolation_expr_pure(arg)?;
                }
                Ok(())
            }
            Expr::Arena { body, .. } => self.ensure_interpolation_block_pure(body),
        }
    }

    fn ensure_interpolation_block_pure(&self, block: &Block) -> Result<(), CompileError> {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let(ls) => self.ensure_interpolation_expr_pure(&ls.value)?,
                Stmt::Assign(assign) => self.ensure_interpolation_expr_pure(&assign.value)?,
                Stmt::CompoundAssign(assign) => {
                    self.ensure_interpolation_expr_pure(&assign.value)?
                }
                Stmt::Expr(expr_stmt) => self.ensure_interpolation_expr_pure(&expr_stmt.expr)?,
                Stmt::While(while_stmt) => {
                    self.ensure_interpolation_expr_pure(&while_stmt.condition)?;
                    self.ensure_interpolation_block_pure(&while_stmt.body)?;
                }
                Stmt::For(for_stmt) => {
                    self.ensure_interpolation_expr_pure(&for_stmt.start)?;
                    self.ensure_interpolation_expr_pure(&for_stmt.end)?;
                    self.ensure_interpolation_block_pure(&for_stmt.body)?;
                }
                Stmt::ForIn(for_stmt) => {
                    self.ensure_interpolation_expr_pure(&for_stmt.iterable)?;
                    self.ensure_interpolation_block_pure(&for_stmt.body)?;
                }
                Stmt::Return(ret) => {
                    if let Some(value) = &ret.value {
                        self.ensure_interpolation_expr_pure(value)?;
                    }
                }
                Stmt::Break(_) | Stmt::Continue(_) => {}
                Stmt::Defer(d) => self.ensure_interpolation_expr_pure(&d.expr)?,
            }
        }

        if let Some(tail) = &block.tail_expr {
            self.ensure_interpolation_expr_pure(tail)?;
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Function call checking (with special cases for len, push)
    // -----------------------------------------------------------------------

    fn check_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        span: Span,
    ) -> Result<BcType, CompileError> {
        let name = match callee {
            Expr::Ident(n, _) => n.clone(),
            _ => return Err(CompileError::new(span, "expected function name")),
        };

        // Special: len(arr)
        if name == "len" {
            if args.len() != 1 {
                return Err(CompileError::new(span, "len() takes 1 argument"));
            }
            let arr_ty = self.check_expr(&args[0], None)?;
            match &arr_ty {
                BcType::Array(_) | BcType::FixedArray(_, _) => Ok(BcType::I32),
                _ => Err(CompileError::new(
                    span,
                    format!("len() expects array, got {}", arr_ty),
                )),
            }
        }
        // Special: push(arr, val)
        else if name == "push" {
            if self.in_pure_fn {
                return Err(CompileError::new(span, "pure function cannot call 'push'"));
            }
            if args.len() != 2 {
                return Err(CompileError::new(span, "push() takes 2 arguments"));
            }
            let arr_ty = self.check_expr(&args[0], None)?;
            let elem_ty = match &arr_ty {
                BcType::Array(e) => (**e).clone(),
                _ => {
                    return Err(CompileError::new(
                        span,
                        format!("push() expects dynamic array, got {}", arr_ty),
                    ))
                }
            };
            let val_ty = self.check_expr(&args[1], Some(&elem_ty))?;
            if val_ty != elem_ty {
                return Err(CompileError::new(
                    args[1].span(),
                    format!(
                        "push element type mismatch: expected {}, got {}",
                        elem_ty, val_ty
                    ),
                ));
            }
            Ok(BcType::Unit)
        }
        // Special: pop(arr)
        else if name == "pop" {
            if self.in_pure_fn {
                return Err(CompileError::new(span, "pure function cannot call 'pop'"));
            }
            if args.len() != 1 {
                return Err(CompileError::new(span, "pop() takes 1 argument"));
            }
            let arr_ty = self.check_expr(&args[0], None)?;
            match &arr_ty {
                BcType::Array(elem) => Ok((**elem).clone()),
                _ => Err(CompileError::new(
                    span,
                    format!("pop() expects dynamic array, got {}", arr_ty),
                )),
            }
        } else {
            // Check if callee is a local variable with FnPtr type
            if let Some(info) = self.lookup_var(&name) {
                if let BcType::FnPtr(param_types, ret_type) = &info.ty {
                    let param_types = param_types.clone();
                    let ret_type = (**ret_type).clone();
                    if args.len() != param_types.len() {
                        return Err(CompileError::new(
                            span,
                            format!(
                                "function pointer '{}' expects {} args, got {}",
                                name,
                                param_types.len(),
                                args.len()
                            ),
                        ));
                    }
                    for (i, (arg, expected_ty)) in args.iter().zip(param_types.iter()).enumerate() {
                        let actual = self.check_expr(arg, Some(expected_ty))?;
                        if actual != *expected_ty {
                            return Err(CompileError::new(
                                arg.span(),
                                format!(
                                    "arg {} of '{}': expected {}, got {}",
                                    i + 1,
                                    name,
                                    expected_ty,
                                    actual
                                ),
                            ));
                        }
                    }
                    return Ok(ret_type);
                } else {
                    return Err(CompileError::new(
                        span,
                        format!("'{}' is not a function", name),
                    ));
                }
            }

            let func = self
                .functions
                .get(&name)
                .ok_or_else(|| CompileError::new(span, format!("undefined function '{}'", name)))?
                .clone();

            // Purity check
            if self.in_pure_fn && (!func.is_pure || func.is_extern) {
                return Err(CompileError::new(
                    span,
                    format!("pure function cannot call impure function '{}'", name),
                ));
            }

            if args.len() != func.params.len() {
                return Err(CompileError::new(
                    span,
                    format!(
                        "'{}' expects {} args, got {}",
                        name,
                        func.params.len(),
                        args.len()
                    ),
                ));
            }

            for (i, (arg, (_, expected_ty))) in args.iter().zip(func.params.iter()).enumerate() {
                let actual = self.check_expr(arg, Some(expected_ty))?;
                if actual != *expected_ty {
                    return Err(CompileError::new(
                        arg.span(),
                        format!(
                            "arg {} of '{}': expected {}, got {}",
                            i + 1,
                            name,
                            expected_ty,
                            actual
                        ),
                    ));
                }
            }

            Ok(func.return_type.clone())
        }
    }

    // -----------------------------------------------------------------------
    // Result constructor checking
    // -----------------------------------------------------------------------

    fn check_result_constructor(
        &mut self,
        variant: &str,
        args: &[Expr],
        span: Span,
        expected: Option<&BcType>,
    ) -> Result<BcType, CompileError> {
        match variant {
            "Ok" => {
                if args.len() != 1 {
                    return Err(CompileError::new(span, "Result::Ok takes 1 argument"));
                }
                // Try to get the Result type from context
                let result_ty = self.infer_result_type(expected);
                let val_ty = if let Some(BcType::Result(ok_ty, _)) = &result_ty {
                    let actual = self.check_expr(&args[0], Some(ok_ty))?;
                    if actual != **ok_ty {
                        return Err(CompileError::new(
                            args[0].span(),
                            format!("Result::Ok: expected {}, got {}", ok_ty, actual),
                        ));
                    }
                    actual
                } else {
                    self.check_expr(&args[0], None)?
                };

                if let Some(BcType::Result(_, err_ty)) = result_ty {
                    Ok(BcType::Result(Box::new(val_ty), err_ty))
                } else {
                    // Fallback: infer from function return type
                    Err(CompileError::new(span, "cannot infer Result error type"))
                }
            }
            "Err" => {
                if args.len() != 1 {
                    return Err(CompileError::new(span, "Result::Err takes 1 argument"));
                }
                let result_ty = self.infer_result_type(expected);
                let err_val_ty = if let Some(BcType::Result(_, err_ty)) = &result_ty {
                    let actual = self.check_expr(&args[0], Some(err_ty))?;
                    if actual != **err_ty {
                        return Err(CompileError::new(
                            args[0].span(),
                            format!("Result::Err: expected {}, got {}", err_ty, actual),
                        ));
                    }
                    actual
                } else {
                    self.check_expr(&args[0], None)?
                };

                if let Some(BcType::Result(ok_ty, _)) = result_ty {
                    Ok(BcType::Result(ok_ty, Box::new(err_val_ty)))
                } else {
                    Err(CompileError::new(span, "cannot infer Result ok type"))
                }
            }
            _ => Err(CompileError::new(
                span,
                format!("Result has no variant '{}'", variant),
            )),
        }
    }

    fn infer_result_type(&self, expected: Option<&BcType>) -> Option<BcType> {
        // First try the explicit expected type
        if let Some(ty @ BcType::Result(_, _)) = expected {
            return Some(ty.clone());
        }
        // Then try the current function's return type
        if let Some(ty @ BcType::Result(_, _)) = &self.current_fn_return_type {
            return Some(ty.clone());
        }
        None
    }

    // -----------------------------------------------------------------------
    // Match checking
    // -----------------------------------------------------------------------

    fn check_match(
        &mut self,
        scrut_ty: &BcType,
        arms: &[MatchArm],
        span: Span,
        expected: Option<&BcType>,
    ) -> Result<BcType, CompileError> {
        let mut arm_types = Vec::new();
        let mut has_wildcard = false;
        let mut seen_variants: HashSet<String> = HashSet::new();

        for arm in arms {
            self.check_pattern_compat(&arm.pattern, scrut_ty, &mut seen_variants, span)?;
            if matches!(arm.pattern, Pattern::Wildcard(_)) {
                has_wildcard = true;
            }
            if let Pattern::Ident(_, _) = &arm.pattern {
                has_wildcard = true; // ident pattern is a catch-all
            }

            self.push_scope();
            self.bind_pattern_vars(&arm.pattern, scrut_ty, arm.span)?;
            let arm_ty = self.check_expr(&arm.body, expected)?;
            self.pop_scope();
            arm_types.push(arm_ty);
        }

        // Exhaustiveness for enums
        if let BcType::Enum(name) = scrut_ty {
            if !has_wildcard {
                let info = self.enums.get(name).unwrap();
                let all: HashSet<String> = info.variants.iter().map(|(n, _)| n.clone()).collect();
                let missing: Vec<_> = all.difference(&seen_variants).cloned().collect();
                if !missing.is_empty() {
                    return Err(CompileError::new(
                        span,
                        format!(
                            "non-exhaustive match on '{}', missing: {}",
                            name,
                            missing.join(", ")
                        ),
                    ));
                }
            }
        }
        // Exhaustiveness for Result
        if let BcType::Result(_, _) = scrut_ty {
            if !has_wildcard {
                let has_ok = seen_variants.contains("Ok");
                let has_err = seen_variants.contains("Err");
                if !has_ok || !has_err {
                    return Err(CompileError::new(
                        span,
                        "non-exhaustive match on Result: need both Ok and Err",
                    ));
                }
            }
        }

        // All arms must have same type
        if arm_types.len() > 1 {
            for (i, ty) in arm_types.iter().enumerate().skip(1) {
                if ty != &arm_types[0] {
                    return Err(CompileError::new(
                        arms[i].span,
                        format!("match arm type mismatch: {} vs {}", arm_types[0], ty),
                    ));
                }
            }
        }

        Ok(arm_types.into_iter().next().unwrap_or(BcType::Unit))
    }

    fn check_pattern_compat(
        &self,
        pat: &Pattern,
        scrut_ty: &BcType,
        seen_variants: &mut HashSet<String>,
        span: Span,
    ) -> Result<(), CompileError> {
        match pat {
            Pattern::Wildcard(_) | Pattern::Ident(_, _) => Ok(()),
            Pattern::IntLit(_, _) => {
                if !matches!(scrut_ty, BcType::I32 | BcType::I64) {
                    return Err(CompileError::new(
                        span,
                        format!("integer pattern on non-integer type {}", scrut_ty),
                    ));
                }
                Ok(())
            }
            Pattern::FloatLit(_, _) => {
                if *scrut_ty != BcType::F64 {
                    return Err(CompileError::new(
                        span,
                        format!("float pattern on non-float type {}", scrut_ty),
                    ));
                }
                Ok(())
            }
            Pattern::StringLit(_, _) => {
                if *scrut_ty != BcType::Str {
                    return Err(CompileError::new(
                        span,
                        format!("string pattern on non-string type {}", scrut_ty),
                    ));
                }
                Ok(())
            }
            Pattern::BoolLit(_, _) => {
                if *scrut_ty != BcType::Bool {
                    return Err(CompileError::new(
                        span,
                        format!("bool pattern on non-bool type {}", scrut_ty),
                    ));
                }
                Ok(())
            }
            Pattern::Enum {
                enum_name,
                variant,
                bindings,
                span: pat_span,
            } => {
                if enum_name == "Result" {
                    seen_variants.insert(variant.clone());
                    // Verify bindings count
                    match variant.as_str() {
                        "Ok" | "Err" => {
                            if bindings.len() != 1 {
                                return Err(CompileError::new(
                                    *pat_span,
                                    format!("Result::{} pattern takes 1 binding", variant),
                                ));
                            }
                        }
                        _ => {
                            return Err(CompileError::new(
                                *pat_span,
                                format!("Result has no variant '{}'", variant),
                            ))
                        }
                    }
                    return Ok(());
                }
                if let BcType::Enum(ename) = scrut_ty {
                    if enum_name != ename {
                        return Err(CompileError::new(
                            *pat_span,
                            format!(
                                "pattern enum '{}' doesn't match scrutinee enum '{}'",
                                enum_name, ename
                            ),
                        ));
                    }
                    let info = self.enums.get(ename).ok_or_else(|| {
                        CompileError::new(*pat_span, format!("undefined enum '{}'", ename))
                    })?;
                    let var_info = info
                        .variants
                        .iter()
                        .find(|(n, _)| n == variant)
                        .ok_or_else(|| {
                            CompileError::new(
                                *pat_span,
                                format!("'{}' has no variant '{}'", ename, variant),
                            )
                        })?;
                    if bindings.len() != var_info.1.len() {
                        return Err(CompileError::new(
                            *pat_span,
                            format!(
                                "'{}::{}' has {} fields, got {} bindings",
                                ename,
                                variant,
                                var_info.1.len(),
                                bindings.len()
                            ),
                        ));
                    }
                    seen_variants.insert(variant.clone());
                } else {
                    return Err(CompileError::new(
                        *pat_span,
                        format!("enum pattern on non-enum type {}", scrut_ty),
                    ));
                }
                Ok(())
            }
        }
    }

    fn bind_pattern_vars(
        &mut self,
        pat: &Pattern,
        scrut_ty: &BcType,
        span: Span,
    ) -> Result<(), CompileError> {
        match pat {
            Pattern::Wildcard(_) => Ok(()),
            Pattern::Ident(name, ispan) => {
                // Bind the whole scrutinee value
                self.add_binding(name, scrut_ty.clone(), false, *ispan)?;
                Ok(())
            }
            Pattern::IntLit(_, _)
            | Pattern::FloatLit(_, _)
            | Pattern::StringLit(_, _)
            | Pattern::BoolLit(_, _) => Ok(()),
            Pattern::Enum {
                enum_name,
                variant,
                bindings,
                ..
            } => {
                if enum_name == "Result" {
                    let (ok_ty, err_ty) = match scrut_ty {
                        BcType::Result(o, e) => (o, e),
                        _ => {
                            return Err(CompileError::new(
                                span,
                                "Result pattern on non-Result type",
                            ))
                        }
                    };
                    let payload_ty = match variant.as_str() {
                        "Ok" => (**ok_ty).clone(),
                        "Err" => (**err_ty).clone(),
                        _ => {
                            return Err(CompileError::new(
                                span,
                                format!("Result has no variant '{}'", variant),
                            ))
                        }
                    };
                    if let Some(binding) = bindings.first() {
                        if let Pattern::Ident(name, bspan) = binding {
                            self.add_binding(name, payload_ty, false, *bspan)?;
                        }
                    }
                    return Ok(());
                }

                if let BcType::Enum(ename) = scrut_ty {
                    let info = self.enums.get(ename).unwrap().clone();
                    let var_info = info.variants.iter().find(|(n, _)| n == variant).unwrap();
                    for (binding, payload_ty) in bindings.iter().zip(var_info.1.iter()) {
                        if let Pattern::Ident(name, bspan) = binding {
                            self.add_binding(name, payload_ty.clone(), false, *bspan)?;
                        }
                    }
                }
                Ok(())
            }
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn resolve_type(&self, ty: &Type) -> Result<BcType, CompileError> {
        match ty {
            Type::Primitive(p, _) => Ok(match p {
                PrimitiveType::I32 => BcType::I32,
                PrimitiveType::I64 => BcType::I64,
                PrimitiveType::F64 => BcType::F64,
                PrimitiveType::Bool => BcType::Bool,
                PrimitiveType::Str => BcType::Str,
                PrimitiveType::Unit => BcType::Unit,
                PrimitiveType::Map => BcType::Map,
                PrimitiveType::MapStrI32 => BcType::MapStrI32,
                PrimitiveType::MapStrI64 => BcType::MapStrI64,
                PrimitiveType::MapStrF64 => BcType::MapStrF64,
                PrimitiveType::MapI32Str => BcType::MapI32Str,
                PrimitiveType::MapI32I32 => BcType::MapI32I32,
            }),
            Type::Named(name, span) => {
                if self.structs.contains_key(name) {
                    Ok(BcType::Struct(name.clone()))
                } else if self.enums.contains_key(name) {
                    Ok(BcType::Enum(name.clone()))
                } else {
                    Err(CompileError::new(
                        *span,
                        format!("undefined type '{}'", name),
                    ))
                }
            }
            Type::FixedArray(elem, size, _) => {
                let e = self.resolve_type(elem)?;
                Ok(BcType::FixedArray(Box::new(e), *size))
            }
            Type::DynamicArray(elem, _) => {
                let e = self.resolve_type(elem)?;
                Ok(BcType::Array(Box::new(e)))
            }
            Type::Result(ok, err, _) => {
                let o = self.resolve_type(ok)?;
                let e = self.resolve_type(err)?;
                Ok(BcType::Result(Box::new(o), Box::new(e)))
            }
            Type::FnPtr(params, ret, _) => {
                let mut param_types = Vec::new();
                for p in params {
                    param_types.push(self.resolve_type(p)?);
                }
                let ret_type = self.resolve_type(ret)?;
                Ok(BcType::FnPtr(param_types, Box::new(ret_type)))
            }
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn check_binary_op_types(
        &self,
        op: BinOp,
        lt: &BcType,
        rt: &BcType,
        span: Span,
    ) -> Result<BcType, CompileError> {
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                if lt != rt {
                    return Err(CompileError::new(
                        span,
                        format!("binary op type mismatch: {} vs {}", lt, rt),
                    ));
                }
                match lt {
                    BcType::I32 | BcType::I64 | BcType::F64 => Ok(lt.clone()),
                    _ => Err(CompileError::new(
                        span,
                        format!("arithmetic not supported for {}", lt),
                    )),
                }
            }
            BinOp::Mod => {
                if lt != rt {
                    return Err(CompileError::new(
                        span,
                        format!("modulo type mismatch: {} vs {}", lt, rt),
                    ));
                }
                match lt {
                    BcType::I32 | BcType::I64 => Ok(lt.clone()),
                    _ => Err(CompileError::new(
                        span,
                        format!("modulo not supported for {}", lt),
                    )),
                }
            }
            _ => Err(CompileError::new(
                span,
                format!("compound assignment not supported for operator {:?}", op),
            )),
        }
    }

    fn add_binding(
        &mut self,
        name: &str,
        ty: BcType,
        is_mut: bool,
        span: Span,
    ) -> Result<(), CompileError> {
        // Anti-shadowing: check ALL enclosing scopes
        for scope in &self.scopes {
            if scope.contains_key(name) {
                return Err(CompileError::new(
                    span,
                    format!(
                        "'{}' shadows an existing binding (anti-shadowing rule)",
                        name
                    ),
                ));
            }
        }
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), BindingInfo { ty, is_mut });
        Ok(())
    }

    fn lookup_var(&self, name: &str) -> Option<&BindingInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(info) = scope.get(name) {
                return Some(info);
            }
        }
        None
    }

    fn field_type(&self, ty: &BcType, field: &str, span: Span) -> Result<BcType, CompileError> {
        match ty {
            BcType::Struct(name) => {
                let info = self.structs.get(name).ok_or_else(|| {
                    CompileError::new(span, format!("undefined struct '{}'", name))
                })?;
                info.fields
                    .iter()
                    .find(|(n, _)| n == field)
                    .map(|(_, t)| t.clone())
                    .ok_or_else(|| {
                        CompileError::new(
                            span,
                            format!("struct '{}' has no field '{}'", name, field),
                        )
                    })
            }
            _ => Err(CompileError::new(
                span,
                format!("field access on non-struct type {}", ty),
            )),
        }
    }

    fn element_type(&self, ty: &BcType, span: Span) -> Result<BcType, CompileError> {
        match ty {
            BcType::Array(e) | BcType::FixedArray(e, _) => Ok((**e).clone()),
            _ => Err(CompileError::new(
                span,
                format!("index on non-array type {}", ty),
            )),
        }
    }

    /// Returns true if a type is safe to escape from an arena block.
    /// Only primitives (i32, i64, f64, bool, unit) are safe.
    fn is_arena_safe_type(ty: &BcType) -> bool {
        matches!(ty, BcType::I32 | BcType::I64 | BcType::F64 | BcType::Bool | BcType::Unit | BcType::FnPtr(_, _))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn analyze_source(src: &str) -> Result<SemanticInfo, CompileError> {
        let mut lex = Lexer::new(src);
        let tokens = lex.tokenize().unwrap();
        let mut par = Parser::new(tokens);
        let program = par.parse_program().unwrap();
        SemanticAnalyzer::analyze(&program, false)
    }

    fn expect_ok(src: &str) {
        if let Err(e) = analyze_source(src) {
            panic!("expected ok, got error: {}", e);
        }
    }

    fn expect_err(src: &str, needle: &str) {
        match analyze_source(src) {
            Ok(_) => panic!(
                "expected error containing '{}', but analysis succeeded",
                needle
            ),
            Err(e) => {
                let msg = format!("{}", e);
                assert!(
                    msg.contains(needle),
                    "expected error containing '{}', got: {}",
                    needle,
                    msg
                );
            }
        }
    }

    // --- Name resolution / order independence ---

    #[test]
    fn test_order_independence() {
        // Function calls a function defined later
        expect_ok(
            "
            fn! main() {
                print_i32(add(1, 2));
            }
            fn add(a: i32, b: i32) -> i32 {
                a + b
            }
        ",
        );
    }

    #[test]
    fn test_struct_before_use() {
        expect_ok(
            "
            fn! main() {
                let p: Point = Point { x: 1.0, y: 2.0 };
                print_f64(p.x);
            }
            struct Point { x: f64, y: f64 }
        ",
        );
    }

    #[test]
    fn test_undefined_variable() {
        expect_err("fn! main() { print_i32(x); }", "undefined variable 'x'");
    }

    // --- Anti-shadowing ---

    #[test]
    fn test_anti_shadowing_nested_block() {
        expect_err(
            "
            fn! main() {
                let x: i32 = 1;
                {
                    let x: i32 = 2;
                };
            }
        ",
            "shadows",
        );
    }

    #[test]
    fn test_anti_shadowing_for_loop() {
        expect_err(
            "
            fn! main() {
                let i: i32 = 0;
                for i in 0..10 {
                    print_i32(i);
                }
            }
        ",
            "shadows",
        );
    }

    #[test]
    fn test_different_functions_same_name_ok() {
        expect_ok(
            "
            fn foo(x: i32) -> i32 { x + 1 }
            fn bar(x: i32) -> i32 { x + 2 }
            fn! main() { print_i32(foo(1)); }
        ",
        );
    }

    // --- Type mismatch ---

    #[test]
    fn test_type_mismatch_arithmetic() {
        expect_err(
            "
            fn! main() {
                let x: i32 = 1 + 2.0;
            }
        ",
            "type mismatch",
        );
    }

    #[test]
    fn test_type_mismatch_let() {
        expect_err(
            "
            fn! main() {
                let x: i32 = true;
            }
        ",
            "expected i32, got bool",
        );
    }

    // --- Purity ---

    #[test]
    fn test_purity_violation() {
        expect_err(
            "
            fn pure_fn() -> i32 {
                println(\"hello\");
                42
            }
            fn! main() { print_i32(pure_fn()); }
        ",
            "pure function cannot call impure",
        );
    }

    #[test]
    fn test_pure_calling_pure_ok() {
        expect_ok(
            "
            fn add(a: i32, b: i32) -> i32 { a + b }
            fn double(x: i32) -> i32 { add(x, x) }
            fn! main() { print_i32(double(5)); }
        ",
        );
    }

    #[test]
    fn test_interpolated_string_ok() {
        expect_ok(
            r#"
            fn! main() {
                let name: str = "Neo";
                println("hello {name} {42} {true}");
            }
        "#,
        );
    }

    #[test]
    fn test_interpolated_string_rejects_impure_call() {
        expect_err(
            r#"
            fn! next_id() -> i32 {
                7
            }

            fn! main() {
                println("id={next_id()}");
            }
        "#,
            "interpolated string expressions cannot call impure function 'next_id'",
        );
    }

    #[test]
    fn test_interpolated_string_rejects_unsupported_type() {
        expect_err(
            r#"
            struct Point { x: i32 }
            fn! main() {
                let p: Point = Point { x: 1 };
                println("{p}");
            }
        "#,
            "interpolated string expressions must be",
        );
    }

    #[test]
    fn test_pure_interpolated_string_allocation_rejected() {
        expect_err(
            r#"
            fn label(n: i32) -> str {
                "value {n}"
            }
            fn! main() {
                println(label(1));
            }
        "#,
            "pure function cannot use interpolated string that allocates",
        );
    }

    #[test]
    fn test_integer_literal_out_of_range_for_i32() {
        expect_err(
            r#"
            fn! main() {
                let big: i64 = 2147483648 as i64;
                println("{big}");
            }
        "#,
            "integer literal out of range for i32",
        );
    }

    // --- Exhaustive match ---

    #[test]
    fn test_non_exhaustive_match() {
        expect_err(
            "
            enum Color { Red, Green, Blue }
            fn! main() {
                let c: Color = Color::Red;
                match c {
                    Color::Red => println(\"red\"),
                    Color::Green => println(\"green\"),
                };
            }
        ",
            "non-exhaustive",
        );
    }

    #[test]
    fn test_exhaustive_match_with_wildcard() {
        expect_ok(
            "
            enum Color { Red, Green, Blue }
            fn! main() {
                let c: Color = Color::Red;
                match c {
                    Color::Red => println(\"red\"),
                    _ => println(\"other\"),
                };
            }
        ",
        );
    }

    // --- Mutability ---

    #[test]
    fn test_immutable_assignment() {
        expect_err(
            "
            fn! main() {
                let x: i32 = 1;
                x = 2;
            }
        ",
            "cannot assign to immutable",
        );
    }

    #[test]
    fn test_mutable_ok() {
        expect_ok(
            "
            fn! main() {
                let mut x: i32 = 1;
                x = 2;
                print_i32(x);
            }
        ",
        );
    }

    // --- Cast validation ---

    #[test]
    fn test_valid_cast() {
        expect_ok(
            "
            fn! main() {
                let x: i64 = 42 as i64;
                print_i64(x);
            }
        ",
        );
    }

    #[test]
    fn test_invalid_cast() {
        expect_err(
            "
            fn! main() {
                let x: str = 42 as str;
            }
        ",
            "invalid cast",
        );
    }

    // --- Hello world (smoke test) ---

    #[test]
    fn test_hello_world() {
        expect_ok(
            "
            fn! main() {
                println(\"Hello, World!\");
            }
        ",
        );
    }

    // --- Fibonacci ---

    #[test]
    fn test_fibonacci() {
        expect_ok(
            "
            fn fib(n: i32) -> i32 {
                if n <= 1 {
                    n
                } else {
                    fib(n - 1) + fib(n - 2)
                }
            }
            fn! main() {
                let result: i32 = fib(10);
                print_i32(result);
                println(\"\");
            }
        ",
        );
    }

    // --- Result / try ---

    #[test]
    fn test_result_and_try() {
        expect_ok(
            "
            fn divide(a: i32, b: i32) -> Result<i32, str> {
                if b == 0 {
                    Result::Err(\"division by zero\")
                } else {
                    Result::Ok(a / b)
                }
            }
            fn compute(x: i32, y: i32, z: i32) -> Result<i32, str> {
                let first: i32 = try divide(x, y);
                let second: i32 = try divide(first, z);
                Result::Ok(second + 1)
            }
            fn! main() {
                let r: Result<i32, str> = compute(100, 5, 2);
                match r {
                    Result::Ok(val) => print_i32(val),
                    Result::Err(msg) => println(msg),
                };
            }
        ",
        );
    }
}
