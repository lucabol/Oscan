use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::error::CompileError;
use crate::token::Span;
use crate::types::*;

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
}

impl SemanticAnalyzer {
    pub fn analyze(program: &Program) -> Result<SemanticInfo, CompileError> {
        let mut sa = Self {
            structs: HashMap::new(),
            enums: HashMap::new(),
            functions: HashMap::new(),
            constants: HashMap::new(),
            scopes: Vec::new(),
            current_fn_return_type: None,
            in_pure_fn: false,
            loop_depth: 0,
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
            params: params.into_iter().map(|(n, t)| (n.to_string(), t)).collect(),
            return_type: ret,
            is_pure: pure,
            is_extern: false,
        };

        // I/O (fn!)
        self.functions.insert("print".into(), builtin(vec![("s", BcType::Str)], BcType::Unit, false));
        self.functions.insert("println".into(), builtin(vec![("s", BcType::Str)], BcType::Unit, false));
        self.functions.insert("print_i32".into(), builtin(vec![("n", BcType::I32)], BcType::Unit, false));
        self.functions.insert("print_i64".into(), builtin(vec![("n", BcType::I64)], BcType::Unit, false));
        self.functions.insert("print_f64".into(), builtin(vec![("n", BcType::F64)], BcType::Unit, false));
        self.functions.insert("print_bool".into(), builtin(vec![("b", BcType::Bool)], BcType::Unit, false));
        self.functions.insert("read_line".into(), builtin(
            vec![],
            BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)),
            false,
        ));

        // String (pure except concat/to_cstr/slice/from_i32 which allocate)
        self.functions.insert("str_len".into(), builtin(vec![("s", BcType::Str)], BcType::I32, true));
        self.functions.insert("str_eq".into(), builtin(vec![("a", BcType::Str), ("b", BcType::Str)], BcType::Bool, true));
        self.functions.insert("str_concat".into(), builtin(vec![("a", BcType::Str), ("b", BcType::Str)], BcType::Str, false));
        self.functions.insert("str_to_cstr".into(), builtin(vec![("s", BcType::Str)], BcType::Str, false));
        self.functions.insert("str_find".into(), builtin(vec![("haystack", BcType::Str), ("needle", BcType::Str)], BcType::I32, true));
        self.functions.insert("str_from_i32".into(), builtin(vec![("n", BcType::I32)], BcType::Str, false));
        self.functions.insert("str_slice".into(), builtin(vec![("s", BcType::Str), ("start", BcType::I32), ("end", BcType::I32)], BcType::Str, false));

        // Math (pure)
        self.functions.insert("abs_i32".into(), builtin(vec![("n", BcType::I32)], BcType::I32, true));
        self.functions.insert("abs_f64".into(), builtin(vec![("n", BcType::F64)], BcType::F64, true));
        self.functions.insert("mod_i32".into(), builtin(vec![("a", BcType::I32), ("b", BcType::I32)], BcType::I32, true));
        self.functions.insert("math_sin".into(), builtin(vec![("x", BcType::F64)], BcType::F64, true));
        self.functions.insert("math_cos".into(), builtin(vec![("x", BcType::F64)], BcType::F64, true));
        self.functions.insert("math_sqrt".into(), builtin(vec![("x", BcType::F64)], BcType::F64, true));
        self.functions.insert("math_pow".into(), builtin(vec![("base", BcType::F64), ("exp", BcType::F64)], BcType::F64, true));
        self.functions.insert("math_exp".into(), builtin(vec![("x", BcType::F64)], BcType::F64, true));
        self.functions.insert("math_log".into(), builtin(vec![("x", BcType::F64)], BcType::F64, true));
        self.functions.insert("math_atan2".into(), builtin(vec![("y", BcType::F64), ("x", BcType::F64)], BcType::F64, true));
        self.functions.insert("math_floor".into(), builtin(vec![("x", BcType::F64)], BcType::F64, true));
        self.functions.insert("math_ceil".into(), builtin(vec![("x", BcType::F64)], BcType::F64, true));
        self.functions.insert("math_fmod".into(), builtin(vec![("x", BcType::F64), ("y", BcType::F64)], BcType::F64, true));
        self.functions.insert("math_abs".into(), builtin(vec![("x", BcType::F64)], BcType::F64, true));
        self.functions.insert("math_pi".into(), builtin(vec![], BcType::F64, true));
        self.functions.insert("math_e".into(), builtin(vec![], BcType::F64, true));
        self.functions.insert("math_ln2".into(), builtin(vec![], BcType::F64, true));
        self.functions.insert("math_sqrt2".into(), builtin(vec![], BcType::F64, true));

        // Bitwise (pure)
        self.functions.insert("band".into(), builtin(vec![("a", BcType::I32), ("b", BcType::I32)], BcType::I32, true));
        self.functions.insert("bor".into(), builtin(vec![("a", BcType::I32), ("b", BcType::I32)], BcType::I32, true));
        self.functions.insert("bxor".into(), builtin(vec![("a", BcType::I32), ("b", BcType::I32)], BcType::I32, true));
        self.functions.insert("bshl".into(), builtin(vec![("a", BcType::I32), ("n", BcType::I32)], BcType::I32, true));
        self.functions.insert("bshr".into(), builtin(vec![("a", BcType::I32), ("n", BcType::I32)], BcType::I32, true));
        self.functions.insert("bnot".into(), builtin(vec![("a", BcType::I32)], BcType::I32, true));

        // Conversion
        self.functions.insert("i32_to_str".into(), builtin(vec![("n", BcType::I32)], BcType::Str, false));

        // Memory
        self.functions.insert("arena_reset".into(), builtin(vec![], BcType::Unit, false));

        // File I/O (fn!)
        self.functions.insert("file_open_read".into(), builtin(vec![("path", BcType::Str)], BcType::I32, false));
        self.functions.insert("file_open_write".into(), builtin(vec![("path", BcType::Str)], BcType::I32, false));
        self.functions.insert("read_byte".into(), builtin(vec![("fd", BcType::I32)], BcType::I32, false));
        self.functions.insert("write_byte".into(), builtin(vec![("fd", BcType::I32), ("b", BcType::I32)], BcType::Unit, false));
        self.functions.insert("write_str".into(), builtin(vec![("fd", BcType::I32), ("s", BcType::Str)], BcType::Unit, false));
        self.functions.insert("file_close".into(), builtin(vec![("fd", BcType::I32)], BcType::Unit, false));
        self.functions.insert("file_delete".into(), builtin(vec![("path", BcType::Str)], BcType::I32, false));

        // Socket I/O (fn!)
        self.functions.insert("socket_tcp".into(), builtin(vec![], BcType::I32, false));
        self.functions.insert("socket_connect".into(), builtin(vec![("sock", BcType::I32), ("addr", BcType::Str), ("port", BcType::I32)], BcType::I32, false));
        self.functions.insert("socket_bind".into(), builtin(vec![("sock", BcType::I32), ("port", BcType::I32)], BcType::I32, false));
        self.functions.insert("socket_listen".into(), builtin(vec![("sock", BcType::I32), ("backlog", BcType::I32)], BcType::I32, false));
        self.functions.insert("socket_accept".into(), builtin(vec![("sock", BcType::I32)], BcType::I32, false));
        self.functions.insert("socket_send".into(), builtin(vec![("sock", BcType::I32), ("data", BcType::Str)], BcType::I32, false));
        self.functions.insert("socket_recv".into(), builtin(vec![("sock", BcType::I32), ("max_len", BcType::I32)], BcType::Str, false));
        self.functions.insert("socket_close".into(), builtin(vec![("sock", BcType::I32)], BcType::Unit, false));

        // UDP Socket I/O (fn!)
        self.functions.insert("socket_udp".into(), builtin(vec![], BcType::I32, false));
        self.functions.insert("socket_sendto".into(), builtin(vec![("sock", BcType::I32), ("data", BcType::Str), ("addr", BcType::Str), ("port", BcType::I32)], BcType::I32, false));
        self.functions.insert("socket_recvfrom".into(), builtin(vec![("sock", BcType::I32), ("max_len", BcType::I32)], BcType::Str, false));

        // Command-line arguments (fn!)
        self.functions.insert("arg_count".into(), builtin(vec![], BcType::I32, false));
        self.functions.insert("arg_get".into(), builtin(vec![("i", BcType::I32)], BcType::Str, false));

        // Tier 1: Character classification (pure)
        self.functions.insert("char_is_alpha".into(), builtin(vec![("c", BcType::I32)], BcType::Bool, true));
        self.functions.insert("char_is_digit".into(), builtin(vec![("c", BcType::I32)], BcType::Bool, true));
        self.functions.insert("char_is_alnum".into(), builtin(vec![("c", BcType::I32)], BcType::Bool, true));
        self.functions.insert("char_is_space".into(), builtin(vec![("c", BcType::I32)], BcType::Bool, true));
        self.functions.insert("char_is_upper".into(), builtin(vec![("c", BcType::I32)], BcType::Bool, true));
        self.functions.insert("char_is_lower".into(), builtin(vec![("c", BcType::I32)], BcType::Bool, true));
        self.functions.insert("char_is_print".into(), builtin(vec![("c", BcType::I32)], BcType::Bool, true));
        self.functions.insert("char_is_xdigit".into(), builtin(vec![("c", BcType::I32)], BcType::Bool, true));
        self.functions.insert("char_to_upper".into(), builtin(vec![("c", BcType::I32)], BcType::I32, true));
        self.functions.insert("char_to_lower".into(), builtin(vec![("c", BcType::I32)], BcType::I32, true));
        self.functions.insert("abs_i64".into(), builtin(vec![("n", BcType::I64)], BcType::I64, true));

        // Tier 2: Number parsing & conversion
        self.functions.insert("parse_i32".into(), builtin(
            vec![("s", BcType::Str)],
            BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str)),
            true,
        ));
        self.functions.insert("parse_i64".into(), builtin(
            vec![("s", BcType::Str)],
            BcType::Result(Box::new(BcType::I64), Box::new(BcType::Str)),
            true,
        ));
        self.functions.insert("str_from_i64".into(), builtin(vec![("n", BcType::I64)], BcType::Str, false));
        self.functions.insert("str_from_f64".into(), builtin(vec![("n", BcType::F64)], BcType::Str, false));
        self.functions.insert("str_from_bool".into(), builtin(vec![("b", BcType::Bool)], BcType::Str, true));

        // Tier 3: Random, time, sleep, exit (fn!)
        self.functions.insert("rand_seed".into(), builtin(vec![("seed", BcType::I32)], BcType::Unit, false));
        self.functions.insert("rand_i32".into(), builtin(vec![], BcType::I32, false));
        self.functions.insert("time_now".into(), builtin(vec![], BcType::I64, false));
        self.functions.insert("sleep_ms".into(), builtin(vec![("ms", BcType::I32)], BcType::Unit, false));
        self.functions.insert("exit".into(), builtin(vec![("code", BcType::I32)], BcType::Unit, false));

        // Tier 4: Environment & error (fn!)
        self.functions.insert("env_get".into(), builtin(
            vec![("name", BcType::Str)],
            BcType::Result(Box::new(BcType::Str), Box::new(BcType::Str)),
            false,
        ));
        self.functions.insert("errno_get".into(), builtin(vec![], BcType::I32, false));
        self.functions.insert("errno_str".into(), builtin(vec![("code", BcType::I32)], BcType::Str, false));

        // Tier 5: Filesystem operations (fn!)
        self.functions.insert("file_rename".into(), builtin(vec![("old", BcType::Str), ("new_path", BcType::Str)], BcType::I32, false));
        self.functions.insert("file_exists".into(), builtin(vec![("path", BcType::Str)], BcType::Bool, false));
        self.functions.insert("dir_create".into(), builtin(vec![("path", BcType::Str)], BcType::I32, false));
        self.functions.insert("dir_remove".into(), builtin(vec![("path", BcType::Str)], BcType::I32, false));
        self.functions.insert("dir_current".into(), builtin(vec![], BcType::Str, false));
        self.functions.insert("dir_change".into(), builtin(vec![("path", BcType::Str)], BcType::I32, false));
        self.functions.insert("file_open_append".into(), builtin(vec![("path", BcType::Str)], BcType::I32, false));
        self.functions.insert("file_size".into(), builtin(vec![("path", BcType::Str)], BcType::I64, false));

        // Path utilities
        self.functions.insert("path_join".into(), builtin(vec![("dir", BcType::Str), ("file", BcType::Str)], BcType::Str, false));
        self.functions.insert("path_ext".into(), builtin(vec![("path", BcType::Str)], BcType::Str, true));
        self.functions.insert("path_exists".into(), builtin(vec![("path", BcType::Str)], BcType::Bool, false));
        self.functions.insert("path_is_dir".into(), builtin(vec![("path", BcType::Str)], BcType::Bool, false));

        // Tier 6: String operations
        self.functions.insert("str_contains".into(), builtin(vec![("s", BcType::Str), ("sub", BcType::Str)], BcType::Bool, true));
        self.functions.insert("str_starts_with".into(), builtin(vec![("s", BcType::Str), ("prefix", BcType::Str)], BcType::Bool, true));
        self.functions.insert("str_ends_with".into(), builtin(vec![("s", BcType::Str), ("suffix", BcType::Str)], BcType::Bool, true));
        self.functions.insert("str_trim".into(), builtin(vec![("s", BcType::Str)], BcType::Str, false));
        self.functions.insert("str_split".into(), builtin(vec![("s", BcType::Str), ("delim", BcType::Str)], BcType::Array(Box::new(BcType::Str)), false));
        self.functions.insert("str_to_upper".into(), builtin(vec![("s", BcType::Str)], BcType::Str, false));
        self.functions.insert("str_to_lower".into(), builtin(vec![("s", BcType::Str)], BcType::Str, false));
        self.functions.insert("str_replace".into(), builtin(vec![("s", BcType::Str), ("old", BcType::Str), ("new_s", BcType::Str)], BcType::Str, false));
        self.functions.insert("str_compare".into(), builtin(vec![("a", BcType::Str), ("b", BcType::Str)], BcType::I32, true));

        // Tier 7: Directory listing & process control (fn!)
        self.functions.insert("dir_list".into(), builtin(vec![("path", BcType::Str)], BcType::Array(Box::new(BcType::Str)), false));
        self.functions.insert("proc_run".into(), builtin(vec![("cmd", BcType::Str), ("args", BcType::Array(Box::new(BcType::Str)))], BcType::I32, false));
        self.functions.insert("term_width".into(), builtin(vec![], BcType::I32, false));
        self.functions.insert("term_height".into(), builtin(vec![], BcType::I32, false));

        // Tier 8: Raw terminal I/O (fn!)
        self.functions.insert("term_raw".into(), builtin(vec![], BcType::I32, false));
        self.functions.insert("term_restore".into(), builtin(vec![], BcType::I32, false));
        self.functions.insert("read_nonblock".into(), builtin(vec![], BcType::I32, false));

        // Tier 9: Environment iteration (fn!)
        self.functions.insert("env_count".into(), builtin(vec![], BcType::I32, false));
        self.functions.insert("env_key".into(), builtin(vec![("i", BcType::I32)], BcType::Str, false));
        self.functions.insert("env_value".into(), builtin(vec![("i", BcType::I32)], BcType::Str, false));

        // Tier 10: Hex formatting (fn! — allocates)
        self.functions.insert("str_from_i32_hex".into(), builtin(vec![("n", BcType::I32)], BcType::Str, false));
        self.functions.insert("str_from_i64_hex".into(), builtin(vec![("n", BcType::I64)], BcType::Str, false));

        // Tier 11: Array sort (fn! — mutates)
        self.functions.insert("sort_i32".into(), builtin(vec![("arr", BcType::Array(Box::new(BcType::I32)))], BcType::Unit, false));
        self.functions.insert("sort_i64".into(), builtin(vec![("arr", BcType::Array(Box::new(BcType::I64)))], BcType::Unit, false));
        self.functions.insert("sort_str".into(), builtin(vec![("arr", BcType::Array(Box::new(BcType::Str)))], BcType::Unit, false));
        self.functions.insert("sort_f64".into(), builtin(vec![("arr", BcType::Array(Box::new(BcType::F64)))], BcType::Unit, false));

        // Tier 12: String <-> char array conversions
        self.functions.insert("str_from_chars".into(), builtin(vec![("arr", BcType::Array(Box::new(BcType::I32)))], BcType::Str, false));
        self.functions.insert("str_to_chars".into(), builtin(vec![("s", BcType::Str)], BcType::Array(Box::new(BcType::I32)), false));

        // Tier 13: Date/Time (fn! — allocates / reads system state)
        self.functions.insert("time_format".into(), builtin(vec![("timestamp", BcType::I64), ("fmt", BcType::Str)], BcType::Str, false));
        self.functions.insert("time_utc_year".into(), builtin(vec![("timestamp", BcType::I64)], BcType::I32, false));
        self.functions.insert("time_utc_month".into(), builtin(vec![("timestamp", BcType::I64)], BcType::I32, false));
        self.functions.insert("time_utc_day".into(), builtin(vec![("timestamp", BcType::I64)], BcType::I32, false));
        self.functions.insert("time_utc_hour".into(), builtin(vec![("timestamp", BcType::I64)], BcType::I32, false));
        self.functions.insert("time_utc_min".into(), builtin(vec![("timestamp", BcType::I64)], BcType::I32, false));
        self.functions.insert("time_utc_sec".into(), builtin(vec![("timestamp", BcType::I64)], BcType::I32, false));

        // Tier 13: Glob matching (pure)
        self.functions.insert("glob_match".into(), builtin(vec![("pattern", BcType::Str), ("text", BcType::Str)], BcType::Bool, true));

        // Tier 13: SHA-256 (fn! — allocates)
        self.functions.insert("sha256".into(), builtin(vec![("data", BcType::Str)], BcType::Str, false));

        // Tier 13: Terminal detection (pure-ish)
        self.functions.insert("is_tty".into(), builtin(vec![], BcType::Bool, true));

        // Tier 13: Environment modification (fn!)
        self.functions.insert("env_set".into(), builtin(vec![("name", BcType::Str), ("value", BcType::Str)], BcType::I32, false));
        self.functions.insert("env_delete".into(), builtin(vec![("name", BcType::Str)], BcType::I32, false));

        // Graphics: Canvas Lifecycle (fn!)
        self.functions.insert("canvas_open".into(), builtin(vec![("width", BcType::I32), ("height", BcType::I32), ("title", BcType::Str)], BcType::I32, false));
        self.functions.insert("canvas_close".into(), builtin(vec![], BcType::Unit, false));
        self.functions.insert("canvas_alive".into(), builtin(vec![], BcType::Bool, false));
        self.functions.insert("canvas_flush".into(), builtin(vec![], BcType::Unit, false));
        self.functions.insert("canvas_clear".into(), builtin(vec![("color", BcType::I32)], BcType::Unit, false));

        // Graphics: Drawing Primitives (fn!)
        self.functions.insert("gfx_pixel".into(), builtin(vec![("x", BcType::I32), ("y", BcType::I32), ("color", BcType::I32)], BcType::Unit, false));
        self.functions.insert("gfx_get_pixel".into(), builtin(vec![("x", BcType::I32), ("y", BcType::I32)], BcType::I32, false));
        self.functions.insert("gfx_line".into(), builtin(vec![("x0", BcType::I32), ("y0", BcType::I32), ("x1", BcType::I32), ("y1", BcType::I32), ("color", BcType::I32)], BcType::Unit, false));
        self.functions.insert("gfx_rect".into(), builtin(vec![("x", BcType::I32), ("y", BcType::I32), ("w", BcType::I32), ("h", BcType::I32), ("color", BcType::I32)], BcType::Unit, false));
        self.functions.insert("gfx_fill_rect".into(), builtin(vec![("x", BcType::I32), ("y", BcType::I32), ("w", BcType::I32), ("h", BcType::I32), ("color", BcType::I32)], BcType::Unit, false));
        self.functions.insert("gfx_circle".into(), builtin(vec![("cx", BcType::I32), ("cy", BcType::I32), ("r", BcType::I32), ("color", BcType::I32)], BcType::Unit, false));
        self.functions.insert("gfx_fill_circle".into(), builtin(vec![("cx", BcType::I32), ("cy", BcType::I32), ("r", BcType::I32), ("color", BcType::I32)], BcType::Unit, false));
        self.functions.insert("gfx_draw_text".into(), builtin(vec![("x", BcType::I32), ("y", BcType::I32), ("text", BcType::Str), ("color", BcType::I32)], BcType::Unit, false));

        // Graphics: Input (fn!)
        self.functions.insert("canvas_key".into(), builtin(vec![], BcType::I32, false));
        self.functions.insert("canvas_mouse_x".into(), builtin(vec![], BcType::I32, false));
        self.functions.insert("canvas_mouse_y".into(), builtin(vec![], BcType::I32, false));
        self.functions.insert("canvas_mouse_btn".into(), builtin(vec![], BcType::I32, false));

        // Graphics: Color (pure)
        self.functions.insert("rgb".into(), builtin(vec![("r", BcType::I32), ("g", BcType::I32), ("b", BcType::I32)], BcType::I32, true));
        self.functions.insert("rgba".into(), builtin(vec![("r", BcType::I32), ("g", BcType::I32), ("b", BcType::I32), ("a", BcType::I32)], BcType::I32, true));

        // HashMap builtins (fn! except map_has and map_len which are pure reads)
        self.functions.insert("map_new".into(), builtin(vec![], BcType::Map, false));
        self.functions.insert("map_set".into(), builtin(vec![("m", BcType::Map), ("key", BcType::Str), ("value", BcType::Str)], BcType::Unit, false));
        self.functions.insert("map_get".into(), builtin(vec![("m", BcType::Map), ("key", BcType::Str)], BcType::Str, false));
        self.functions.insert("map_has".into(), builtin(vec![("m", BcType::Map), ("key", BcType::Str)], BcType::Bool, true));
        self.functions.insert("map_delete".into(), builtin(vec![("m", BcType::Map), ("key", BcType::Str)], BcType::Unit, false));
        self.functions.insert("map_len".into(), builtin(vec![("m", BcType::Map)], BcType::I32, true));

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
                    return Err(CompileError::new(s.span, format!("duplicate struct '{}'", s.name)));
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
                    return Err(CompileError::new(e.span, format!("duplicate enum '{}'", e.name)));
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
                    return Err(CompileError::new(f.span, format!("duplicate function '{}'", f.name)));
                }
                self.functions.insert(f.name.clone(), FunctionInfo {
                    params,
                    return_type,
                    is_pure: f.is_pure,
                    is_extern: false,
                });
            }
            TopDecl::Let(l) => {
                let ty = self.resolve_type(&l.ty)?;
                if self.constants.contains_key(&l.name) {
                    return Err(CompileError::new(l.span, format!("duplicate constant '{}'", l.name)));
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
                        return Err(CompileError::new(ef.span, format!("duplicate extern function '{}'", ef.name)));
                    }
                    self.functions.insert(ef.name.clone(), FunctionInfo {
                        params,
                        return_type,
                        is_pure: false,
                        is_extern: true,
                    });
                }
            }
            TopDecl::Use(_) => {}
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
                    return Err(CompileError::new(l.span,
                        format!("constant '{}': expected {}, got {}", l.name, expected, actual)));
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
                return Err(CompileError::new(p.span,
                    format!("duplicate parameter '{}'", p.name)));
            }
            scope.insert(p.name.clone(), BindingInfo { ty, is_mut: false });
        }

        let body_ty = self.check_block(&func.body, Some(&return_type))?;

        if body_ty != return_type {
            return Err(CompileError::new(func.span,
                format!("function '{}': expected return type {}, got {}", func.name, return_type, body_ty)));
        }

        self.scopes.pop();
        self.current_fn_return_type = None;
        self.in_pure_fn = false;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Block & statement checking
    // -----------------------------------------------------------------------

    fn check_block(&mut self, block: &Block, expected: Option<&BcType>) -> Result<BcType, CompileError> {
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
                    return Err(CompileError::new(ls.span,
                        format!("let '{}': expected {}, got {}", ls.name, expected, actual)));
                }
                self.add_binding(&ls.name, expected, ls.is_mut, ls.span)?;
                Ok(())
            }
            Stmt::Assign(a) => {
                // Check target is mutable (skip for array element access — arrays are references)
                let target_info = self.lookup_var(&a.target.name)
                    .ok_or_else(|| CompileError::new(a.span,
                        format!("undefined variable '{}'", a.target.name)))?
                    .clone();
                let has_index = a.target.accessors.iter().any(|acc| matches!(acc, PlaceAccessor::Index(_)));
                if !target_info.is_mut && !has_index {
                    return Err(CompileError::new(a.span,
                        format!("cannot assign to immutable variable '{}'", a.target.name)));
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
                                return Err(CompileError::new(a.span,
                                    "cannot assign to string index: strings are immutable".to_string()));
                            }
                            let idx_ty = self.check_expr(idx_expr, None)?;
                            if idx_ty != BcType::I32 {
                                return Err(CompileError::new(a.span,
                                    format!("array index must be i32, got {}", idx_ty)));
                            }
                            ty = self.element_type(&ty, a.span)?;
                        }
                    }
                }

                let val_ty = self.check_expr(&a.value, Some(&ty))?;
                if val_ty != ty {
                    return Err(CompileError::new(a.span,
                        format!("assignment type mismatch: expected {}, got {}", ty, val_ty)));
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
                    return Err(CompileError::new(w.span,
                        format!("while condition must be bool, got {}", cond_ty)));
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
                    return Err(CompileError::new(f.span,
                        format!("for range start must be i32, got {}", start_ty)));
                }
                if end_ty != BcType::I32 {
                    return Err(CompileError::new(f.span,
                        format!("for range end must be i32, got {}", end_ty)));
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
                    _ => return Err(CompileError::new(fi.span,
                        format!("for-in requires an array type, got {}", arr_ty))),
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
                    return Err(CompileError::new(*span,
                        "break used outside of a loop".to_string()));
                }
                Ok(())
            }
            Stmt::Continue(span) => {
                if self.loop_depth == 0 {
                    return Err(CompileError::new(*span,
                        "continue used outside of a loop".to_string()));
                }
                Ok(())
            }
            Stmt::CompoundAssign(ca) => {
                // Check target is mutable (skip for array element access — arrays are references)
                let target_info = self.lookup_var(&ca.target.name)
                    .ok_or_else(|| CompileError::new(ca.span,
                        format!("undefined variable '{}'", ca.target.name)))?
                    .clone();
                let has_index = ca.target.accessors.iter().any(|acc| matches!(acc, PlaceAccessor::Index(_)));
                if !target_info.is_mut && !has_index {
                    return Err(CompileError::new(ca.span,
                        format!("cannot assign to immutable variable '{}'", ca.target.name)));
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
                                return Err(CompileError::new(ca.span,
                                    format!("array index must be i32, got {}", idx_ty)));
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
                let fn_ret = self.current_fn_return_type.clone()
                    .unwrap_or(BcType::Unit);
                match &r.value {
                    Some(expr) => {
                        let ty = self.check_expr(expr, Some(&fn_ret))?;
                        if ty != fn_ret {
                            return Err(CompileError::new(r.span,
                                format!("return type mismatch: expected {}, got {}", fn_ret, ty)));
                        }
                    }
                    None => {
                        if fn_ret != BcType::Unit {
                            return Err(CompileError::new(r.span,
                                format!("return without value in function returning {}", fn_ret)));
                        }
                    }
                }
                Ok(())
            }
        }
    }

    // -----------------------------------------------------------------------
    // Expression type checking
    // -----------------------------------------------------------------------

    fn check_expr(&mut self, expr: &Expr, expected: Option<&BcType>) -> Result<BcType, CompileError> {
        match expr {
            Expr::IntLit(_, _) => Ok(BcType::I32),
            Expr::FloatLit(_, _) => Ok(BcType::F64),
            Expr::StringLit(_, _) => Ok(BcType::Str),
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
                Err(CompileError::new(*span, format!("undefined variable '{}'", name)))
            }

            Expr::BinaryOp { op, left, right, span } => {
                let lt = self.check_expr(left, None)?;
                let rt = self.check_expr(right, None)?;
                match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                        if lt != rt {
                            return Err(CompileError::new(*span,
                                format!("binary op type mismatch: {} vs {}", lt, rt)));
                        }
                        match &lt {
                            BcType::I32 | BcType::I64 | BcType::F64 => Ok(lt),
                            _ => Err(CompileError::new(*span,
                                format!("arithmetic not supported for {}", lt))),
                        }
                    }
                    BinOp::Mod => {
                        if lt != rt {
                            return Err(CompileError::new(*span,
                                format!("modulo type mismatch: {} vs {}", lt, rt)));
                        }
                        match &lt {
                            BcType::I32 | BcType::I64 => Ok(lt),
                            _ => Err(CompileError::new(*span,
                                format!("modulo not supported for {}", lt))),
                        }
                    }
                    BinOp::Eq | BinOp::Neq => {
                        if lt != rt {
                            return Err(CompileError::new(*span,
                                format!("equality type mismatch: {} vs {}", lt, rt)));
                        }
                        Ok(BcType::Bool)
                    }
                    BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
                        if lt != rt {
                            return Err(CompileError::new(*span,
                                format!("comparison type mismatch: {} vs {}", lt, rt)));
                        }
                        match &lt {
                            BcType::I32 | BcType::I64 | BcType::F64 | BcType::Str => Ok(BcType::Bool),
                            _ => Err(CompileError::new(*span,
                                format!("comparison not supported for {}", lt))),
                        }
                    }
                    BinOp::And | BinOp::Or => {
                        if lt != BcType::Bool {
                            return Err(CompileError::new(*span,
                                format!("logical op expects bool, got {}", lt)));
                        }
                        if rt != BcType::Bool {
                            return Err(CompileError::new(*span,
                                format!("logical op expects bool, got {}", rt)));
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
                        _ => Err(CompileError::new(*span,
                            format!("negation not supported for {}", t))),
                    },
                    UnaryOp::Not => {
                        if t != BcType::Bool {
                            return Err(CompileError::new(*span,
                                format!("'not' expects bool, got {}", t)));
                        }
                        Ok(BcType::Bool)
                    }
                }
            }

            Expr::Cast { expr: inner, ty, span } => {
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
                    return Err(CompileError::new(*span,
                        format!("invalid cast: {} as {}", from, to)));
                }
                Ok(to)
            }

            Expr::Call { callee, args, span } => {
                self.check_call(callee, args, *span)
            }

            Expr::FieldAccess { expr: obj, field, span } => {
                let obj_ty = self.check_expr(obj, None)?;
                self.field_type(&obj_ty, field, *span)
            }

            Expr::Index { expr: arr, index, span } => {
                let arr_ty = self.check_expr(arr, None)?;
                let idx_ty = self.check_expr(index, None)?;
                if idx_ty != BcType::I32 {
                    return Err(CompileError::new(*span,
                        format!("array index must be i32, got {}", idx_ty)));
                }
                if arr_ty == BcType::Str {
                    Ok(BcType::I32)
                } else {
                    self.element_type(&arr_ty, *span)
                }
            }

            Expr::Block(block) => {
                self.check_block(block, expected)
            }

            Expr::If { condition, then_block, else_branch, span } => {
                let cond_ty = self.check_expr(condition, None)?;
                if cond_ty != BcType::Bool {
                    return Err(CompileError::new(condition.span(),
                        format!("if condition must be bool, got {}", cond_ty)));
                }
                let then_ty = self.check_block(then_block, expected)?;
                if let Some(else_expr) = else_branch {
                    let else_ty = self.check_expr(else_expr, expected)?;
                    if then_ty != else_ty {
                        return Err(CompileError::new(*span,
                            format!("if/else type mismatch: {} vs {}", then_ty, else_ty)));
                    }
                    Ok(then_ty)
                } else {
                    if then_ty != BcType::Unit {
                        return Err(CompileError::new(*span,
                            "if without else must have unit type"));
                    }
                    Ok(BcType::Unit)
                }
            }

            Expr::Match { scrutinee, arms, span } => {
                let scrut_ty = self.check_expr(scrutinee, None)?;
                self.check_match(&scrut_ty, arms, *span, expected)
            }

            Expr::Try { call, span } => {
                let call_ty = self.check_expr(call, None)?;
                match call_ty {
                    BcType::Result(ok_ty, err_ty) => {
                        match &self.current_fn_return_type {
                            Some(BcType::Result(_, fn_err)) => {
                                if *err_ty != **fn_err {
                                    return Err(CompileError::new(*span,
                                        format!("try error type mismatch: {} vs {}", err_ty, fn_err)));
                                }
                                Ok(*ok_ty)
                            }
                            _ => Err(CompileError::new(*span,
                                "try can only be used in a function returning Result")),
                        }
                    }
                    _ => Err(CompileError::new(*span,
                        format!("try requires Result, got {}", call_ty))),
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
                    return Err(CompileError::new(*span,
                        "cannot infer element type of empty array"));
                }
                let first_ty = self.check_expr(&elements[0], None)?;
                for elem in elements.iter().skip(1) {
                    let ty = self.check_expr(elem, None)?;
                    if ty != first_ty {
                        return Err(CompileError::new(elem.span(),
                            format!("array element type mismatch: {} vs {}", first_ty, ty)));
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
                let info = self.structs.get(name)
                    .ok_or_else(|| CompileError::new(*span,
                        format!("undefined struct '{}'", name)))?
                    .clone();

                // Check all required fields are present
                let mut provided: HashSet<String> = HashSet::new();
                for fi in fields {
                    if !provided.insert(fi.name.clone()) {
                        return Err(CompileError::new(fi.span,
                            format!("duplicate field '{}'", fi.name)));
                    }
                    let expected_field_ty = info.fields.iter()
                        .find(|(n, _)| n == &fi.name)
                        .map(|(_, t)| t)
                        .ok_or_else(|| CompileError::new(fi.span,
                            format!("struct '{}' has no field '{}'", name, fi.name)))?;
                    let actual = self.check_expr(&fi.value, Some(expected_field_ty))?;
                    if actual != *expected_field_ty {
                        return Err(CompileError::new(fi.span,
                            format!("field '{}': expected {}, got {}", fi.name, expected_field_ty, actual)));
                    }
                }
                let expected_fields: HashSet<String> = info.fields.iter().map(|(n, _)| n.clone()).collect();
                let missing: Vec<_> = expected_fields.difference(&provided).collect();
                if !missing.is_empty() {
                    return Err(CompileError::new(*span,
                        format!("missing fields: {:?}", missing)));
                }
                Ok(BcType::Struct(name.clone()))
            }

            Expr::EnumConstructor { enum_name, variant, args, span } => {
                if enum_name == "Result" {
                    return self.check_result_constructor(variant, args, *span, expected);
                }
                let info = self.enums.get(enum_name)
                    .ok_or_else(|| CompileError::new(*span,
                        format!("undefined enum '{}'", enum_name)))?
                    .clone();
                let var_info = info.variants.iter()
                    .find(|(n, _)| n == variant)
                    .ok_or_else(|| CompileError::new(*span,
                        format!("enum '{}' has no variant '{}'", enum_name, variant)))?;
                if args.len() != var_info.1.len() {
                    return Err(CompileError::new(*span,
                        format!("'{}::{}' expects {} args, got {}",
                            enum_name, variant, var_info.1.len(), args.len())));
                }
                for (arg, expected_ty) in args.iter().zip(var_info.1.iter()) {
                    let actual = self.check_expr(arg, Some(expected_ty))?;
                    if actual != *expected_ty {
                        return Err(CompileError::new(arg.span(),
                            format!("expected {}, got {}", expected_ty, actual)));
                    }
                }
                Ok(BcType::Enum(enum_name.clone()))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Function call checking (with special cases for len, push)
    // -----------------------------------------------------------------------

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Result<BcType, CompileError> {
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
                _ => Err(CompileError::new(span,
                    format!("len() expects array, got {}", arr_ty))),
            }
        }
        // Special: push(arr, val)
        else if name == "push" {
            if self.in_pure_fn {
                return Err(CompileError::new(span,
                    "pure function cannot call 'push'"));
            }
            if args.len() != 2 {
                return Err(CompileError::new(span, "push() takes 2 arguments"));
            }
            let arr_ty = self.check_expr(&args[0], None)?;
            let elem_ty = match &arr_ty {
                BcType::Array(e) => (**e).clone(),
                _ => return Err(CompileError::new(span,
                    format!("push() expects dynamic array, got {}", arr_ty))),
            };
            let val_ty = self.check_expr(&args[1], Some(&elem_ty))?;
            if val_ty != elem_ty {
                return Err(CompileError::new(args[1].span(),
                    format!("push element type mismatch: expected {}, got {}", elem_ty, val_ty)));
            }
            Ok(BcType::Unit)
        }
        // Special: pop(arr)
        else if name == "pop" {
            if self.in_pure_fn {
                return Err(CompileError::new(span,
                    "pure function cannot call 'pop'"));
            }
            if args.len() != 1 {
                return Err(CompileError::new(span, "pop() takes 1 argument"));
            }
            let arr_ty = self.check_expr(&args[0], None)?;
            match &arr_ty {
                BcType::Array(elem) => Ok((**elem).clone()),
                _ => Err(CompileError::new(span,
                    format!("pop() expects dynamic array, got {}", arr_ty))),
            }
        }
        else {
            let func = self.functions.get(&name)
                .ok_or_else(|| CompileError::new(span,
                    format!("undefined function '{}'", name)))?
                .clone();

            // Purity check
            if self.in_pure_fn && (!func.is_pure || func.is_extern) {
                return Err(CompileError::new(span,
                    format!("pure function cannot call impure function '{}'", name)));
            }

            if args.len() != func.params.len() {
                return Err(CompileError::new(span,
                    format!("'{}' expects {} args, got {}", name, func.params.len(), args.len())));
            }

            for (i, (arg, (_, expected_ty))) in args.iter().zip(func.params.iter()).enumerate() {
                let actual = self.check_expr(arg, Some(expected_ty))?;
                if actual != *expected_ty {
                    return Err(CompileError::new(arg.span(),
                        format!("arg {} of '{}': expected {}, got {}", i + 1, name, expected_ty, actual)));
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
                        return Err(CompileError::new(args[0].span(),
                            format!("Result::Ok: expected {}, got {}", ok_ty, actual)));
                    }
                    actual
                } else {
                    self.check_expr(&args[0], None)?
                };

                if let Some(BcType::Result(_, err_ty)) = result_ty {
                    Ok(BcType::Result(Box::new(val_ty), err_ty))
                } else {
                    // Fallback: infer from function return type
                    Err(CompileError::new(span,
                        "cannot infer Result error type"))
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
                        return Err(CompileError::new(args[0].span(),
                            format!("Result::Err: expected {}, got {}", err_ty, actual)));
                    }
                    actual
                } else {
                    self.check_expr(&args[0], None)?
                };

                if let Some(BcType::Result(ok_ty, _)) = result_ty {
                    Ok(BcType::Result(ok_ty, Box::new(err_val_ty)))
                } else {
                    Err(CompileError::new(span,
                        "cannot infer Result ok type"))
                }
            }
            _ => Err(CompileError::new(span,
                format!("Result has no variant '{}'", variant))),
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
                    return Err(CompileError::new(span,
                        format!("non-exhaustive match on '{}', missing: {}", name, missing.join(", "))));
                }
            }
        }
        // Exhaustiveness for Result
        if let BcType::Result(_, _) = scrut_ty {
            if !has_wildcard {
                let has_ok = seen_variants.contains("Ok");
                let has_err = seen_variants.contains("Err");
                if !has_ok || !has_err {
                    return Err(CompileError::new(span,
                        "non-exhaustive match on Result: need both Ok and Err"));
                }
            }
        }

        // All arms must have same type
        if arm_types.len() > 1 {
            for (i, ty) in arm_types.iter().enumerate().skip(1) {
                if ty != &arm_types[0] {
                    return Err(CompileError::new(arms[i].span,
                        format!("match arm type mismatch: {} vs {}", arm_types[0], ty)));
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
                    return Err(CompileError::new(span,
                        format!("integer pattern on non-integer type {}", scrut_ty)));
                }
                Ok(())
            }
            Pattern::FloatLit(_, _) => {
                if *scrut_ty != BcType::F64 {
                    return Err(CompileError::new(span,
                        format!("float pattern on non-float type {}", scrut_ty)));
                }
                Ok(())
            }
            Pattern::StringLit(_, _) => {
                if *scrut_ty != BcType::Str {
                    return Err(CompileError::new(span,
                        format!("string pattern on non-string type {}", scrut_ty)));
                }
                Ok(())
            }
            Pattern::BoolLit(_, _) => {
                if *scrut_ty != BcType::Bool {
                    return Err(CompileError::new(span,
                        format!("bool pattern on non-bool type {}", scrut_ty)));
                }
                Ok(())
            }
            Pattern::Enum { enum_name, variant, bindings, span: pat_span } => {
                if enum_name == "Result" {
                    seen_variants.insert(variant.clone());
                    // Verify bindings count
                    match variant.as_str() {
                        "Ok" | "Err" => {
                            if bindings.len() != 1 {
                                return Err(CompileError::new(*pat_span,
                                    format!("Result::{} pattern takes 1 binding", variant)));
                            }
                        }
                        _ => return Err(CompileError::new(*pat_span,
                            format!("Result has no variant '{}'", variant))),
                    }
                    return Ok(());
                }
                if let BcType::Enum(ename) = scrut_ty {
                    if enum_name != ename {
                        return Err(CompileError::new(*pat_span,
                            format!("pattern enum '{}' doesn't match scrutinee enum '{}'", enum_name, ename)));
                    }
                    let info = self.enums.get(ename)
                        .ok_or_else(|| CompileError::new(*pat_span,
                            format!("undefined enum '{}'", ename)))?;
                    let var_info = info.variants.iter()
                        .find(|(n, _)| n == variant)
                        .ok_or_else(|| CompileError::new(*pat_span,
                            format!("'{}' has no variant '{}'", ename, variant)))?;
                    if bindings.len() != var_info.1.len() {
                        return Err(CompileError::new(*pat_span,
                            format!("'{}::{}' has {} fields, got {} bindings",
                                ename, variant, var_info.1.len(), bindings.len())));
                    }
                    seen_variants.insert(variant.clone());
                } else {
                    return Err(CompileError::new(*pat_span,
                        format!("enum pattern on non-enum type {}", scrut_ty)));
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
            Pattern::IntLit(_, _) | Pattern::FloatLit(_, _)
            | Pattern::StringLit(_, _) | Pattern::BoolLit(_, _) => Ok(()),
            Pattern::Enum { enum_name, variant, bindings, .. } => {
                if enum_name == "Result" {
                    let (ok_ty, err_ty) = match scrut_ty {
                        BcType::Result(o, e) => (o, e),
                        _ => return Err(CompileError::new(span,
                            "Result pattern on non-Result type")),
                    };
                    let payload_ty = match variant.as_str() {
                        "Ok" => (**ok_ty).clone(),
                        "Err" => (**err_ty).clone(),
                        _ => return Err(CompileError::new(span,
                            format!("Result has no variant '{}'", variant))),
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
                    let var_info = info.variants.iter()
                        .find(|(n, _)| n == variant).unwrap();
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
            }),
            Type::Named(name, span) => {
                if self.structs.contains_key(name) {
                    Ok(BcType::Struct(name.clone()))
                } else if self.enums.contains_key(name) {
                    Ok(BcType::Enum(name.clone()))
                } else {
                    Err(CompileError::new(*span, format!("undefined type '{}'", name)))
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
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn check_binary_op_types(&self, op: BinOp, lt: &BcType, rt: &BcType, span: Span) -> Result<BcType, CompileError> {
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                if lt != rt {
                    return Err(CompileError::new(span,
                        format!("binary op type mismatch: {} vs {}", lt, rt)));
                }
                match lt {
                    BcType::I32 | BcType::I64 | BcType::F64 => Ok(lt.clone()),
                    _ => Err(CompileError::new(span,
                        format!("arithmetic not supported for {}", lt))),
                }
            }
            BinOp::Mod => {
                if lt != rt {
                    return Err(CompileError::new(span,
                        format!("modulo type mismatch: {} vs {}", lt, rt)));
                }
                match lt {
                    BcType::I32 | BcType::I64 => Ok(lt.clone()),
                    _ => Err(CompileError::new(span,
                        format!("modulo not supported for {}", lt))),
                }
            }
            _ => Err(CompileError::new(span,
                format!("compound assignment not supported for operator {:?}", op))),
        }
    }

    fn add_binding(&mut self, name: &str, ty: BcType, is_mut: bool, span: Span) -> Result<(), CompileError> {
        // Anti-shadowing: check ALL enclosing scopes
        for scope in &self.scopes {
            if scope.contains_key(name) {
                return Err(CompileError::new(span,
                    format!("'{}' shadows an existing binding (anti-shadowing rule)", name)));
            }
        }
        self.scopes.last_mut().unwrap().insert(
            name.to_string(),
            BindingInfo { ty, is_mut },
        );
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
                let info = self.structs.get(name)
                    .ok_or_else(|| CompileError::new(span,
                        format!("undefined struct '{}'", name)))?;
                info.fields.iter()
                    .find(|(n, _)| n == field)
                    .map(|(_, t)| t.clone())
                    .ok_or_else(|| CompileError::new(span,
                        format!("struct '{}' has no field '{}'", name, field)))
            }
            _ => Err(CompileError::new(span,
                format!("field access on non-struct type {}", ty))),
        }
    }

    fn element_type(&self, ty: &BcType, span: Span) -> Result<BcType, CompileError> {
        match ty {
            BcType::Array(e) | BcType::FixedArray(e, _) => Ok((**e).clone()),
            _ => Err(CompileError::new(span,
                format!("index on non-array type {}", ty))),
        }
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
        SemanticAnalyzer::analyze(&program)
    }

    fn expect_ok(src: &str) {
        if let Err(e) = analyze_source(src) {
            panic!("expected ok, got error: {}", e);
        }
    }

    fn expect_err(src: &str, needle: &str) {
        match analyze_source(src) {
            Ok(_) => panic!("expected error containing '{}', but analysis succeeded", needle),
            Err(e) => {
                let msg = format!("{}", e);
                assert!(msg.contains(needle),
                    "expected error containing '{}', got: {}", needle, msg);
            }
        }
    }

    // --- Name resolution / order independence ---

    #[test]
    fn test_order_independence() {
        // Function calls a function defined later
        expect_ok("
            fn! main() {
                print_i32(add(1, 2));
            }
            fn add(a: i32, b: i32) -> i32 {
                a + b
            }
        ");
    }

    #[test]
    fn test_struct_before_use() {
        expect_ok("
            fn! main() {
                let p: Point = Point { x: 1.0, y: 2.0 };
                print_f64(p.x);
            }
            struct Point { x: f64, y: f64 }
        ");
    }

    #[test]
    fn test_undefined_variable() {
        expect_err("fn! main() { print_i32(x); }", "undefined variable 'x'");
    }

    // --- Anti-shadowing ---

    #[test]
    fn test_anti_shadowing_nested_block() {
        expect_err("
            fn! main() {
                let x: i32 = 1;
                {
                    let x: i32 = 2;
                };
            }
        ", "shadows");
    }

    #[test]
    fn test_anti_shadowing_for_loop() {
        expect_err("
            fn! main() {
                let i: i32 = 0;
                for i in 0..10 {
                    print_i32(i);
                }
            }
        ", "shadows");
    }

    #[test]
    fn test_different_functions_same_name_ok() {
        expect_ok("
            fn foo(x: i32) -> i32 { x + 1 }
            fn bar(x: i32) -> i32 { x + 2 }
            fn! main() { print_i32(foo(1)); }
        ");
    }

    // --- Type mismatch ---

    #[test]
    fn test_type_mismatch_arithmetic() {
        expect_err("
            fn! main() {
                let x: i32 = 1 + 2.0;
            }
        ", "type mismatch");
    }

    #[test]
    fn test_type_mismatch_let() {
        expect_err("
            fn! main() {
                let x: i32 = true;
            }
        ", "expected i32, got bool");
    }

    // --- Purity ---

    #[test]
    fn test_purity_violation() {
        expect_err("
            fn pure_fn() -> i32 {
                println(\"hello\");
                42
            }
            fn! main() { print_i32(pure_fn()); }
        ", "pure function cannot call impure");
    }

    #[test]
    fn test_pure_calling_pure_ok() {
        expect_ok("
            fn add(a: i32, b: i32) -> i32 { a + b }
            fn double(x: i32) -> i32 { add(x, x) }
            fn! main() { print_i32(double(5)); }
        ");
    }

    // --- Exhaustive match ---

    #[test]
    fn test_non_exhaustive_match() {
        expect_err("
            enum Color { Red, Green, Blue }
            fn! main() {
                let c: Color = Color::Red;
                match c {
                    Color::Red => println(\"red\"),
                    Color::Green => println(\"green\"),
                };
            }
        ", "non-exhaustive");
    }

    #[test]
    fn test_exhaustive_match_with_wildcard() {
        expect_ok("
            enum Color { Red, Green, Blue }
            fn! main() {
                let c: Color = Color::Red;
                match c {
                    Color::Red => println(\"red\"),
                    _ => println(\"other\"),
                };
            }
        ");
    }

    // --- Mutability ---

    #[test]
    fn test_immutable_assignment() {
        expect_err("
            fn! main() {
                let x: i32 = 1;
                x = 2;
            }
        ", "cannot assign to immutable");
    }

    #[test]
    fn test_mutable_ok() {
        expect_ok("
            fn! main() {
                let mut x: i32 = 1;
                x = 2;
                print_i32(x);
            }
        ");
    }

    // --- Cast validation ---

    #[test]
    fn test_valid_cast() {
        expect_ok("
            fn! main() {
                let x: i64 = 42 as i64;
                print_i64(x);
            }
        ");
    }

    #[test]
    fn test_invalid_cast() {
        expect_err("
            fn! main() {
                let x: str = 42 as str;
            }
        ", "invalid cast");
    }

    // --- Hello world (smoke test) ---

    #[test]
    fn test_hello_world() {
        expect_ok("
            fn! main() {
                println(\"Hello, World!\");
            }
        ");
    }

    // --- Fibonacci ---

    #[test]
    fn test_fibonacci() {
        expect_ok("
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
        ");
    }

    // --- Result / try ---

    #[test]
    fn test_result_and_try() {
        expect_ok("
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
        ");
    }
}
