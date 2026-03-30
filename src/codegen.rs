use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::types::*;

// ---------------------------------------------------------------------------
// Code Generator — Typed AST → C99 source
// ---------------------------------------------------------------------------

pub struct CodeGenerator {
    out: String,
    indent: usize,
    temp_counter: usize,
    structs: HashMap<String, StructInfo>,
    enums: HashMap<String, EnumInfo>,
    functions: HashMap<String, FunctionInfo>,
    constants: HashMap<String, ConstInfo>,
    scopes: Vec<HashMap<String, BcType>>,
    mut_vars: HashSet<String>,
    current_fn_return_type: Option<BcType>,
    result_types: Vec<(BcType, BcType)>,
    expected_array_elem_type: Option<BcType>,
    freestanding: bool,
}

impl CodeGenerator {
    pub fn generate(program: &Program, info: &SemanticInfo, freestanding: bool) -> String {
        let mut cg = Self {
            out: String::new(),
            indent: 0,
            temp_counter: 0,
            structs: info.structs.clone(),
            enums: info.enums.clone(),
            functions: info.functions.clone(),
            constants: info.constants.clone(),
            scopes: Vec::new(),
            mut_vars: HashSet::new(),
            current_fn_return_type: None,
            result_types: Vec::new(),
            expected_array_elem_type: None,
            freestanding,
        };

        // Collect all unique Result types used in the program
        cg.collect_result_types(program);

        cg.emit_includes();
        cg.emit_result_typedefs();
        cg.emit_struct_defs(program);
        cg.emit_enum_defs(program);
        cg.emit_forward_decls(program);
        cg.emit_top_level_constants(program);
        cg.emit_function_defs(program);
        cg.emit_main_wrapper();

        cg.out
    }

    // -----------------------------------------------------------------------
    // Result type collection
    // -----------------------------------------------------------------------

    fn collect_result_types(&mut self, program: &Program) {
        let mut seen = HashSet::new();
        for decl in &program.decls {
            self.collect_result_types_decl(decl, &mut seen);
        }
        // Also collect from function return types
        for fi in self.functions.values() {
            if let BcType::Result(ok, err) = &fi.return_type {
                let key = ((**ok).clone(), (**err).clone());
                if seen.insert(key.clone()) {
                    self.result_types.push(key);
                }
            }
        }
        // Always include Result<str, str> for read_line
        let str_str = (BcType::Str, BcType::Str);
        if !seen.contains(&str_str) {
            // Already defined in runtime, skip
        }
    }

    fn collect_result_types_decl(&mut self, decl: &TopDecl, seen: &mut HashSet<(BcType, BcType)>) {
        match decl {
            TopDecl::Fn(f) => {
                if let Some(ty) = &f.return_type {
                    self.collect_result_types_from_ast_type(ty, seen);
                }
                for p in &f.params {
                    self.collect_result_types_from_ast_type(&p.ty, seen);
                }
                self.collect_result_types_block(&f.body, seen);
            }
            TopDecl::Let(l) => {
                self.collect_result_types_from_ast_type(&l.ty, seen);
            }
            TopDecl::Struct(s) => {
                for field in &s.fields {
                    self.collect_result_types_from_ast_type(&field.ty, seen);
                }
            }
            _ => {}
        }
    }

    fn collect_result_types_from_ast_type(&mut self, ty: &Type, seen: &mut HashSet<(BcType, BcType)>) {
        if let Type::Result(ok, err, _) = ty {
            let ok_bc = self.resolve_ast_type(ok);
            let err_bc = self.resolve_ast_type(err);
            let key = (ok_bc, err_bc);
            if seen.insert(key.clone()) {
                self.result_types.push(key);
            }
            self.collect_result_types_from_ast_type(ok, seen);
            self.collect_result_types_from_ast_type(err, seen);
        }
    }

    fn collect_result_types_block(&mut self, block: &Block, seen: &mut HashSet<(BcType, BcType)>) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let(ls) => {
                    self.collect_result_types_from_ast_type(&ls.ty, seen);
                }
                Stmt::For(f) => self.collect_result_types_block(&f.body, seen),
                Stmt::ForIn(fi) => self.collect_result_types_block(&fi.body, seen),
                Stmt::While(w) => self.collect_result_types_block(&w.body, seen),
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Output helpers
    // -----------------------------------------------------------------------

    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn blank(&mut self) {
        self.out.push('\n');
    }

    fn fresh_tmp(&mut self) -> String {
        let n = self.temp_counter;
        self.temp_counter += 1;
        format!("_tmp_{}", n)
    }

    /// Mangle identifiers that clash with C keywords or standard type names.
    fn mangle_c_name(name: &str) -> String {
        const C_RESERVED: &[&str] = &[
            "auto", "break", "case", "char", "const", "continue", "default", "do",
            "double", "else", "enum", "extern", "float", "for", "goto", "if",
            "int", "long", "register", "return", "short", "signed", "sizeof",
            "static", "struct", "switch", "typedef", "union", "unsigned", "void",
            "volatile", "while", "inline", "restrict",
        ];
        if C_RESERVED.contains(&name) {
            format!("{}_", name)
        } else {
            name.to_string()
        }
    }

    // -----------------------------------------------------------------------
    // Includes
    // -----------------------------------------------------------------------

    fn emit_includes(&mut self) {
        if self.freestanding {
            // Dual-mode header block: freestanding by default, libc if
            // the compiler doesn't define OSC_FREESTANDING (e.g. MSVC fallback)
            self.line("#ifndef OSC_NOFREESTANDING");
            self.line("#define OSC_FREESTANDING");
            self.line("#define L_MAINFILE");
            self.line("#define L_WITHSNPRINTF");
            self.line("#include \"l_os.h\"");
            self.line("#include \"osc_runtime.h\"");
            self.line("#include \"osc_runtime.c\"");
            self.line("#else");
            self.line("#include <stdint.h>");
            self.line("#include <stdio.h>");
            self.line("#include <stdlib.h>");
            self.line("#include <math.h>");
            self.line("#include \"osc_runtime.h\"");
            self.line("#endif");
        } else {
            // libc mode: standard headers, runtime linked separately
            self.line("#include <stdint.h>");
            self.line("#include <stdio.h>");
            self.line("#include <stdlib.h>");
            self.line("#include <math.h>");
            self.line("#include \"osc_runtime.h\"");
        }
        self.blank();
        // Global argc/argv for command-line argument access
        // osc_global_argc and osc_global_argv are declared in osc_runtime.h
        // and defined in osc_runtime.c — no need to redefine here
    }

    // -----------------------------------------------------------------------
    // Result typedefs
    // -----------------------------------------------------------------------

    fn emit_result_typedefs(&mut self) {
        for (ok, err) in &self.result_types.clone() {
            let name = self.result_type_name(ok, err);
            if name == "osc_result_str_str"
                || name == "osc_result_i32_str"
                || name == "osc_result_i64_str"
            {
                continue; // Already in runtime
            }
            let ok_c = self.type_to_c(ok);
            let err_c = self.type_to_c(err);
            let ok_c_field = if *ok == BcType::Unit { "uint8_t".to_string() } else { ok_c };
            self.line(&format!("OSC_RESULT_DECL({}, {}, {});", ok_c_field, err_c, name));
        }
        self.blank();
    }

    // -----------------------------------------------------------------------
    // Enum definitions
    // -----------------------------------------------------------------------

    fn emit_enum_defs(&mut self, program: &Program) {
        for decl in &program.decls {
            if let TopDecl::Enum(e) = decl {
                self.emit_enum_def(e);
            }
        }
    }

    fn emit_enum_def(&mut self, e: &EnumDecl) {
        // Tag constants
        for (i, v) in e.variants.iter().enumerate() {
            self.line(&format!("#define {}_TAG_{} {}", e.name, v.name, i));
        }
        self.blank();

        let has_payload = e.variants.iter().any(|v| !v.payload_types.is_empty());

        if has_payload {
            self.line(&format!("typedef struct {{"));
            self.indent += 1;
            self.line("int tag;");
            self.line("union {");
            self.indent += 1;
            for v in &e.variants {
                if !v.payload_types.is_empty() {
                    self.line(&format!("struct {{"));
                    self.indent += 1;
                    for (i, pt) in v.payload_types.iter().enumerate() {
                        let ct = self.type_to_c(&self.resolve_ast_type(pt));
                        self.line(&format!("{} _{};", ct, i));
                    }
                    self.indent -= 1;
                    self.line(&format!("}} {};", v.name));
                }
            }
            self.indent -= 1;
            self.line("} data;");
            self.indent -= 1;
            self.line(&format!("}} {};", e.name));
        } else {
            // Simple int enum
            self.line(&format!("typedef int {};", e.name));
        }
        self.blank();
    }

    // -----------------------------------------------------------------------
    // Struct definitions
    // -----------------------------------------------------------------------

    fn emit_struct_defs(&mut self, program: &Program) {
        for decl in &program.decls {
            if let TopDecl::Struct(s) = decl {
                self.emit_struct_def(s);
            }
        }
    }

    fn emit_struct_def(&mut self, s: &StructDecl) {
        self.line(&format!("typedef struct {{"));
        self.indent += 1;
        for f in &s.fields {
            let ct = self.type_to_c(&self.resolve_ast_type(&f.ty));
            self.line(&format!("{} {};", ct, f.name));
        }
        self.indent -= 1;
        self.line(&format!("}} {};", s.name));
        self.blank();
    }

    // -----------------------------------------------------------------------
    // Forward declarations
    // -----------------------------------------------------------------------

    fn emit_forward_decls(&mut self, program: &Program) {
        for decl in &program.decls {
            if let TopDecl::Fn(f) = decl {
                if f.name == "main" {
                    let ret = self.fn_return_c(f);
                    let params = self.fn_params_c(f);
                    self.line(&format!("{} oscan_main({});", ret, params));
                } else {
                    let ret = self.fn_return_c(f);
                    let params = self.fn_params_c(f);
                    let cname = Self::mangle_c_name(&f.name);
                    self.line(&format!("{} {}({});", ret, cname, params));
                }
            }
        }
        self.blank();
    }

    // -----------------------------------------------------------------------
    // Top-level constants
    // -----------------------------------------------------------------------

    fn emit_top_level_constants(&mut self, program: &Program) {
        for decl in &program.decls {
            if let TopDecl::Let(l) = decl {
                let ty = self.resolve_ast_type(&l.ty);
                let c_ty = self.type_to_c(&ty);
                match &l.value {
                    Expr::StringLit(s, _) => {
                        let escaped = self.escape_c_string(s);
                        self.line(&format!(
                            "static const osc_str {} = {{ \"{}\", {} }};",
                            l.name, escaped, s.len()
                        ));
                    }
                    _ => {
                        // Try compile-time constant evaluation first (C99 requires
                        // static initializers to be constant expressions).
                        if let Some(const_val) = Self::try_const_eval(&l.value) {
                            self.line(&format!("static const {} {} = {};", c_ty, l.name, const_val));
                        } else {
                            self.scopes.push(HashMap::new());
                            let val = self.emit_expr(&l.value);
                            self.scopes.pop();
                            self.line(&format!("static const {} {} = {};", c_ty, l.name, val));
                        }
                    }
                }
            }
        }
        self.blank();
    }

    /// Attempt to evaluate a constant expression at compile time.
    fn try_const_eval(expr: &Expr) -> Option<String> {
        match expr {
            Expr::IntLit(v, _) => Some(format!("{}", v)),
            Expr::FloatLit(v, _) => Some(format!("{:?}", v)),
            Expr::BoolLit(b, _) => Some(if *b { "1".to_string() } else { "0".to_string() }),
            Expr::BinaryOp { op, left, right, .. } => {
                let l = Self::try_const_eval(left)?;
                let r = Self::try_const_eval(right)?;
                let lv = l.parse::<i64>().ok()?;
                let rv = r.parse::<i64>().ok()?;
                match op {
                    BinOp::Add => Some(format!("{}", lv.checked_add(rv)?)),
                    BinOp::Sub => Some(format!("{}", lv.checked_sub(rv)?)),
                    BinOp::Mul => Some(format!("{}", lv.checked_mul(rv)?)),
                    BinOp::Div if rv != 0 => Some(format!("{}", lv / rv)),
                    BinOp::Mod if rv != 0 => Some(format!("{}", lv % rv)),
                    _ => None,
                }
            }
            Expr::UnaryOp { op: UnaryOp::Neg, operand, .. } => {
                let v = Self::try_const_eval(operand)?;
                let iv = v.parse::<i64>().ok()?;
                Some(format!("{}", iv.checked_neg()?))
            }
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Function definitions
    // -----------------------------------------------------------------------

    fn emit_function_defs(&mut self, program: &Program) {
        for decl in &program.decls {
            if let TopDecl::Fn(f) = decl {
                self.emit_function(f);
            }
        }
    }

    fn emit_function(&mut self, f: &FnDecl) {
        let ret = self.fn_return_c(f);
        let params = self.fn_params_c(f);
        let name = if f.name == "main" { "oscan_main".to_string() } else { Self::mangle_c_name(&f.name) };

        self.current_fn_return_type = match &f.return_type {
            Some(t) => Some(self.resolve_ast_type(t)),
            None => Some(BcType::Unit),
        };

        self.line(&format!("{} {}({}) {{", ret, name, params));
        self.indent += 1;

        // Set up scope with parameters
        self.push_scope();
        for p in &f.params {
            let ty = self.resolve_ast_type(&p.ty);
            self.scopes.last_mut().unwrap().insert(p.name.clone(), ty);
        }

        // Emit body
        let body_val = self.emit_block_body(&f.body);

        // If the body has a tail expression and function returns non-unit, emit return
        if f.body.tail_expr.is_some() {
            let fn_ret = self.current_fn_return_type.clone().unwrap_or(BcType::Unit);
            if fn_ret != BcType::Unit {
                self.line(&format!("return {};", body_val));
            } else if body_val != "(void)0" {
                self.line(&format!("{};", body_val));
            }
        }

        self.pop_scope();
        self.indent -= 1;
        self.line("}");
        self.blank();
        self.current_fn_return_type = None;
    }

    // -----------------------------------------------------------------------
    // Main wrapper
    // -----------------------------------------------------------------------

    fn emit_main_wrapper(&mut self) {
        self.line("int main(int argc, char *argv[]) {");
        self.indent += 1;
        self.line("osc_global_argc = argc;");
        self.line("osc_global_argv = argv;");
        if self.freestanding {
            self.line("#ifdef OSC_FREESTANDING");
            self.line("l_getenv_init(argc, argv);");
            self.line("#endif");
        }
        self.line("osc_arena* _arena = osc_arena_create(1048576);");
        self.line("osc_global_arena = _arena;");

        // Check if oscan_main returns Result
        if let Some(fi) = self.functions.get("main") {
            if let BcType::Result(_, _) = &fi.return_type {
                let ret_c = self.type_to_c(&fi.return_type);
                self.line(&format!("{} _result = oscan_main(_arena);", ret_c));
                self.line("osc_arena_destroy(_arena);");
                self.line("if (!_result.is_ok) {");
                self.indent += 1;
                self.line("return 1;");
                self.indent -= 1;
                self.line("}");
                self.line("return 0;");
            } else {
                self.line("oscan_main(_arena);");
                self.line("osc_arena_destroy(_arena);");
                self.line("return 0;");
            }
        } else {
            self.line("oscan_main(_arena);");
            self.line("osc_arena_destroy(_arena);");
            self.line("return 0;");
        }

        self.indent -= 1;
        self.line("}");
    }

    // -----------------------------------------------------------------------
    // Block emission (statements + optional tail)
    // -----------------------------------------------------------------------

    /// Emits all statements in the block. Returns the C expression for the
    /// tail expression, or "(void)0" for unit blocks.
    fn emit_block_body(&mut self, block: &Block) -> String {
        for stmt in &block.stmts {
            self.emit_stmt(stmt);
        }
        if let Some(tail) = &block.tail_expr {
            self.emit_expr(tail)
        } else {
            "(void)0".to_string()
        }
    }

    // -----------------------------------------------------------------------
    // Statement emission
    // -----------------------------------------------------------------------

    fn emit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let(ls) => {
                let ty = self.resolve_ast_type(&ls.ty);
                let c_ty = self.type_to_c(&ty);
                // Propagate element type for empty array literals
                if let BcType::Array(ref elem_ty) = ty {
                    self.expected_array_elem_type = Some((**elem_ty).clone());
                }
                let val = self.emit_expr(&ls.value);
                self.expected_array_elem_type = None;
                if ls.is_mut {
                    self.line(&format!("{} {} = {};", c_ty, ls.name, val));
                    self.mut_vars.insert(ls.name.clone());
                } else {
                    // For structs/enums/arrays, skip const qualifier to avoid C issues
                    match &ty {
                        BcType::Struct(_) | BcType::Enum(_) | BcType::Array(_) | BcType::FixedArray(_, _) | BcType::Result(_, _) => {
                            self.line(&format!("{} {} = {};", c_ty, ls.name, val));
                        }
                        _ => {
                            self.line(&format!("const {} {} = {};", c_ty, ls.name, val));
                        }
                    }
                }
                self.scopes.last_mut().unwrap().insert(ls.name.clone(), ty);
            }
            Stmt::Assign(a) => {
                let val = self.emit_expr(&a.value);
                if a.target.accessors.is_empty() {
                    self.line(&format!("{} = {};", a.target.name, val));
                } else {
                    let target = self.emit_place(&a.target);
                    self.line(&format!("{} = {};", target, val));
                }
            }
            Stmt::CompoundAssign(ca) => {
                let c_op = match ca.op {
                    BinOp::Add => "+=",
                    BinOp::Sub => "-=",
                    BinOp::Mul => "*=",
                    BinOp::Div => "/=",
                    BinOp::Mod => "%=",
                    _ => unreachable!(),
                };
                let val = self.emit_expr(&ca.value);
                if ca.target.accessors.is_empty() {
                    self.line(&format!("{} {} {};", ca.target.name, c_op, val));
                } else {
                    let target = self.emit_place(&ca.target);
                    self.line(&format!("{} {} {};", target, c_op, val));
                }
            }
            Stmt::Expr(es) => {
                let val = self.emit_expr(&es.expr);
                if val != "(void)0" {
                    self.line(&format!("{};", val));
                }
            }
            Stmt::While(w) => {
                let cond = self.emit_expr(&w.condition);
                self.line(&format!("while ({}) {{", cond));
                self.indent += 1;
                self.push_scope();
                for s in &w.body.stmts {
                    self.emit_stmt(s);
                }
                if let Some(tail) = &w.body.tail_expr {
                    let v = self.emit_expr(tail);
                    if v != "(void)0" {
                        self.line(&format!("{};", v));
                    }
                }
                self.pop_scope();
                self.indent -= 1;
                self.line("}");
            }
            Stmt::For(f) => {
                let start = self.emit_expr(&f.start);
                let end = self.emit_expr(&f.end);
                self.line(&format!("for (int32_t {} = {}; {} < {}; {}++) {{",
                    f.var, start, f.var, end, f.var));
                self.indent += 1;
                self.push_scope();
                self.scopes.last_mut().unwrap().insert(f.var.clone(), BcType::I32);
                for s in &f.body.stmts {
                    self.emit_stmt(s);
                }
                if let Some(tail) = &f.body.tail_expr {
                    let v = self.emit_expr(tail);
                    if v != "(void)0" {
                        self.line(&format!("{};", v));
                    }
                }
                self.pop_scope();
                self.indent -= 1;
                self.line("}");
            }
            Stmt::ForIn(fi) => {
                let iter_idx = self.fresh_tmp();
                let arr_expr = self.emit_expr(&fi.iterable);
                // Determine array element type from the iterable
                let arr_ty = self.type_of(&fi.iterable);
                let (elem_ty, is_fixed, fixed_size) = match &arr_ty {
                    BcType::FixedArray(e, n) => ((**e).clone(), true, *n),
                    BcType::Array(e) => ((**e).clone(), false, 0),
                    _ => (BcType::I32, false, 0),
                };
                let c_elem_ty = self.type_to_c(&elem_ty);
                if is_fixed {
                    self.line(&format!("for (int32_t {} = 0; {} < {}; {}++) {{",
                        iter_idx, iter_idx, fixed_size, iter_idx));
                    self.indent += 1;
                    self.push_scope();
                    self.scopes.last_mut().unwrap().insert(fi.var.clone(), elem_ty);
                    self.line(&format!("const {} {} = {}[{}];",
                        c_elem_ty, fi.var, arr_expr, iter_idx));
                } else {
                    self.line(&format!("for (int32_t {} = 0; {} < {}->len; {}++) {{",
                        iter_idx, iter_idx, arr_expr, iter_idx));
                    self.indent += 1;
                    self.push_scope();
                    self.scopes.last_mut().unwrap().insert(fi.var.clone(), elem_ty);
                    self.line(&format!("const {} {} = (({}*){}->data)[{}];",
                        c_elem_ty, fi.var, c_elem_ty, arr_expr, iter_idx));
                }
                for s in &fi.body.stmts {
                    self.emit_stmt(s);
                }
                if let Some(tail) = &fi.body.tail_expr {
                    let v = self.emit_expr(tail);
                    if v != "(void)0" {
                        self.line(&format!("{};", v));
                    }
                }
                self.pop_scope();
                self.indent -= 1;
                self.line("}");
            }
            Stmt::Break(_) => {
                self.line("break;");
            }
            Stmt::Continue(_) => {
                self.line("continue;");
            }
            Stmt::Return(r) => {
                if let Some(val) = &r.value {
                    let v = self.emit_expr(val);
                    self.line(&format!("return {};", v));
                } else {
                    self.line("return;");
                }
            }
        }
    }

    fn emit_place(&mut self, place: &Place) -> String {
        let mut s = place.name.clone();
        for acc in &place.accessors {
            match acc {
                PlaceAccessor::Field(f) => {
                    s = format!("{}.{}", s, f);
                }
                PlaceAccessor::Index(idx) => {
                    let idx_c = self.emit_expr(idx);
                    // For array set, we need the element type
                    let arr_ty = self.lookup_type(&place.name).unwrap_or(BcType::Unit);
                    let elem_ty = match &arr_ty {
                        BcType::Array(e) | BcType::FixedArray(e, _) => (**e).clone(),
                        _ => BcType::I32,
                    };
                    let elem_c = self.type_to_c(&elem_ty);
                    // Use a special marker — we handle array set in emit_stmt for Assign
                    s = format!("(*({}*)osc_array_get({}, {}))", elem_c, s, idx_c);
                }
            }
        }
        s
    }

    // -----------------------------------------------------------------------
    // Expression emission — returns a C expression string
    // May emit supporting statements to `self.out`.
    // -----------------------------------------------------------------------

    fn emit_expr(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::IntLit(v, _) => format!("{}", v),
            Expr::FloatLit(v, _) => {
                let s = format!("{}", v);
                if s.contains('.') || s.contains('e') || s.contains('E') {
                    s
                } else {
                    format!("{}.0", s)
                }
            }
            Expr::StringLit(s, _) => {
                let escaped = self.escape_c_string(s);
                format!("osc_str_from_cstr(\"{}\")", escaped)
            }
            Expr::BoolLit(b, _) => if *b { "1".to_string() } else { "0".to_string() },

            Expr::Ident(name, _) => name.clone(),

            Expr::BinaryOp { op, left, right, .. } => {
                self.emit_binary_op(*op, left, right)
            }

            Expr::UnaryOp { op, operand, .. } => {
                let val = self.emit_expr(operand);
                match op {
                    UnaryOp::Not => format!("(!{})", val),
                    UnaryOp::Neg => {
                        let ty = self.type_of(operand);
                        match ty {
                            BcType::I32 => format!("osc_neg_i32({})", val),
                            BcType::I64 => format!("osc_neg_i64({})", val),
                            BcType::F64 => format!("(-{})", val),
                            _ => format!("(-{})", val),
                        }
                    }
                }
            }

            Expr::Cast { expr: inner, ty, .. } => {
                let val = self.emit_expr(inner);
                let from = self.type_of(inner);
                let to = self.resolve_ast_type(ty);
                self.emit_cast(&val, &from, &to)
            }

            Expr::Call { callee, args, .. } => {
                let name = match callee.as_ref() {
                    Expr::Ident(n, _) => n.clone(),
                    _ => "unknown".to_string(),
                };
                self.emit_call(&name, args)
            }

            Expr::FieldAccess { expr: obj, field, .. } => {
                let obj_c = self.emit_expr(obj);
                format!("{}.{}", obj_c, field)
            }

            Expr::Index { expr: arr, index, .. } => {
                let arr_c = self.emit_expr(arr);
                let idx_c = self.emit_expr(index);
                let arr_ty = self.type_of(arr);
                match &arr_ty {
                    BcType::Str => {
                        format!("((int32_t)(unsigned char)(({}).data[osc_str_check_index({}, {})]))", arr_c, arr_c, idx_c)
                    }
                    _ => {
                        let elem_ty = match &arr_ty {
                            BcType::Array(e) | BcType::FixedArray(e, _) => (**e).clone(),
                            _ => BcType::I32,
                        };
                        let elem_c = self.type_to_c(&elem_ty);
                        format!("(*({}*)osc_array_get({}, {}))", elem_c, arr_c, idx_c)
                    }
                }
            }

            Expr::Block(block) => {
                // Pre-scan let stmts to populate a temporary scope so
                // block_type can resolve identifiers defined inside the block.
                let ty = if block.tail_expr.is_some() {
                    self.push_scope();
                    for stmt in &block.stmts {
                        if let Stmt::Let(ls) = stmt {
                            let ty = self.resolve_ast_type(&ls.ty);
                            self.scopes.last_mut().unwrap().insert(ls.name.clone(), ty);
                        }
                    }
                    let ty = self.block_type(block);
                    self.pop_scope();
                    ty
                } else {
                    BcType::Unit
                };

                if ty == BcType::Unit {
                    self.line("{");
                    self.indent += 1;
                    self.push_scope();
                    for s in &block.stmts {
                        self.emit_stmt(s);
                    }
                    if let Some(tail) = &block.tail_expr {
                        let v = self.emit_expr(tail);
                        if v != "(void)0" {
                            self.line(&format!("{};", v));
                        }
                    }
                    self.pop_scope();
                    self.indent -= 1;
                    self.line("}");
                    "(void)0".to_string()
                } else {
                    let tmp = self.fresh_tmp();
                    let c_ty = self.type_to_c(&ty);
                    self.line(&format!("{} {};", c_ty, tmp));
                    self.line("{");
                    self.indent += 1;
                    self.push_scope();
                    for s in &block.stmts {
                        self.emit_stmt(s);
                    }
                    if let Some(tail) = &block.tail_expr {
                        let v = self.emit_expr(tail);
                        self.line(&format!("{} = {};", tmp, v));
                    }
                    self.pop_scope();
                    self.indent -= 1;
                    self.line("}");
                    tmp
                }
            }

            Expr::If { condition, then_block, else_branch, .. } => {
                self.emit_if(condition, then_block, else_branch.as_deref())
            }

            Expr::Match { scrutinee, arms, .. } => {
                self.emit_match(scrutinee, arms)
            }

            Expr::Try { call, span } => {
                self.emit_try(call, *span)
            }

            Expr::ArrayLit { elements, .. } => {
                self.emit_array_lit(elements)
            }

            Expr::StructLit { name, fields, .. } => {
                self.emit_struct_lit(name, fields)
            }

            Expr::EnumConstructor { enum_name, variant, args, .. } => {
                self.emit_enum_constructor(enum_name, variant, args)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Binary operations
    // -----------------------------------------------------------------------

    fn emit_binary_op(&mut self, op: BinOp, left: &Expr, right: &Expr) -> String {
        let lv = self.emit_expr(left);
        let rv = self.emit_expr(right);
        let ty = self.type_of(left);

        match op {
            BinOp::Add => match ty {
                BcType::I32 => format!("osc_add_i32({}, {})", lv, rv),
                BcType::I64 => format!("osc_add_i64({}, {})", lv, rv),
                BcType::F64 => format!("({} + {})", lv, rv),
                BcType::Str => format!("osc_str_concat(_arena, {}, {})", lv, rv),
                _ => format!("({} + {})", lv, rv),
            },
            BinOp::Sub => match ty {
                BcType::I32 => format!("osc_sub_i32({}, {})", lv, rv),
                BcType::I64 => format!("osc_sub_i64({}, {})", lv, rv),
                _ => format!("({} - {})", lv, rv),
            },
            BinOp::Mul => match ty {
                BcType::I32 => format!("osc_mul_i32({}, {})", lv, rv),
                BcType::I64 => format!("osc_mul_i64({}, {})", lv, rv),
                _ => format!("({} * {})", lv, rv),
            },
            BinOp::Div => match ty {
                BcType::I32 => format!("osc_div_i32({}, {})", lv, rv),
                BcType::I64 => format!("osc_div_i64({}, {})", lv, rv),
                _ => format!("({} / {})", lv, rv),
            },
            BinOp::Mod => match ty {
                BcType::I32 => format!("osc_mod_i32({}, {})", lv, rv),
                BcType::I64 => format!("osc_mod_i64({}, {})", lv, rv),
                _ => format!("({} %% {})", lv, rv),
            },
            BinOp::Eq => {
                match ty {
                    BcType::Str => format!("osc_str_eq({}, {})", lv, rv),
                    BcType::Enum(_) => format!("({}.tag == {}.tag)", lv, rv),
                    _ => format!("({} == {})", lv, rv),
                }
            }
            BinOp::Neq => {
                match ty {
                    BcType::Str => format!("(!osc_str_eq({}, {}))", lv, rv),
                    BcType::Enum(_) => format!("({}.tag != {}.tag)", lv, rv),
                    _ => format!("({} != {})", lv, rv),
                }
            }
            BinOp::Lt => match ty {
                BcType::Str => format!("(osc_str_compare({}, {}) < 0)", lv, rv),
                _ => format!("({} < {})", lv, rv),
            },
            BinOp::Gt => match ty {
                BcType::Str => format!("(osc_str_compare({}, {}) > 0)", lv, rv),
                _ => format!("({} > {})", lv, rv),
            },
            BinOp::LtEq => match ty {
                BcType::Str => format!("(osc_str_compare({}, {}) <= 0)", lv, rv),
                _ => format!("({} <= {})", lv, rv),
            },
            BinOp::GtEq => match ty {
                BcType::Str => format!("(osc_str_compare({}, {}) >= 0)", lv, rv),
                _ => format!("({} >= {})", lv, rv),
            },
            BinOp::And => format!("({} && {})", lv, rv),
            BinOp::Or => format!("({} || {})", lv, rv),
        }
    }

    // -----------------------------------------------------------------------
    // Cast
    // -----------------------------------------------------------------------

    fn emit_cast(&self, val: &str, from: &BcType, to: &BcType) -> String {
        match (from, to) {
            (BcType::I32, BcType::I64) => format!("osc_i32_to_i64({})", val),
            (BcType::I64, BcType::I32) => format!("osc_i64_to_i32({})", val),
            (BcType::I32, BcType::F64) => format!("osc_i32_to_f64({})", val),
            (BcType::I64, BcType::F64) => format!("osc_i64_to_f64({})", val),
            (BcType::F64, BcType::I32) => format!("osc_f64_to_i32({})", val),
            (BcType::F64, BcType::I64) => format!("osc_f64_to_i64({})", val),
            _ => format!("(({}){})", self.type_to_c(to), val),
        }
    }

    // -----------------------------------------------------------------------
    // Function calls
    // -----------------------------------------------------------------------

    fn emit_call(&mut self, name: &str, args: &[Expr]) -> String {
        let mut arg_strs: Vec<String> = args.iter().map(|a| self.emit_expr(a)).collect();

        match name {
            "print" => format!("osc_print({})", arg_strs[0]),
            "println" => format!("osc_println({})", arg_strs[0]),
            "print_i32" => format!("osc_print_i32({})", arg_strs[0]),
            "print_i64" => format!("osc_print_i64({})", arg_strs[0]),
            "print_f64" => format!("osc_print_f64({})", arg_strs[0]),
            "print_bool" => format!("osc_print_bool({})", arg_strs[0]),
            "read_line" => "osc_read_line(_arena)".to_string(),
            "str_len" => format!("osc_str_len({})", arg_strs[0]),
            "str_eq" => format!("osc_str_eq({}, {})", arg_strs[0], arg_strs[1]),
            "str_concat" => format!("osc_str_concat(_arena, {}, {})", arg_strs[0], arg_strs[1]),
            "str_to_cstr" => format!("osc_str_to_cstr(_arena, {})", arg_strs[0]),
            "str_find" => format!("osc_str_find({}, {})", arg_strs[0], arg_strs[1]),
            "str_from_i32" => format!("osc_str_from_i32(_arena, {})", arg_strs[0]),
            "str_slice" => format!("osc_str_slice(_arena, {}, {}, {})", arg_strs[0], arg_strs[1], arg_strs[2]),
            "abs_i32" => format!("osc_abs_i32({})", arg_strs[0]),
            "abs_f64" => format!("osc_abs_f64({})", arg_strs[0]),
            "mod_i32" => format!("osc_mod_i32({}, {})", arg_strs[0], arg_strs[1]),
            "band" => format!("((int32_t)((uint32_t)({}) & (uint32_t)({})))", arg_strs[0], arg_strs[1]),
            "bor" => format!("((int32_t)((uint32_t)({}) | (uint32_t)({})))", arg_strs[0], arg_strs[1]),
            "bxor" => format!("((int32_t)((uint32_t)({}) ^ (uint32_t)({})))", arg_strs[0], arg_strs[1]),
            "bshl" => format!("((int32_t)((uint32_t)({}) << ({})))", arg_strs[0], arg_strs[1]),
            "bshr" => format!("((int32_t)((uint32_t)({}) >> ({})))", arg_strs[0], arg_strs[1]),
            "bnot" => format!("((int32_t)(~(uint32_t)({})))", arg_strs[0]),
            "i32_to_str" => format!("osc_i32_to_str(_arena, {})", arg_strs[0]),
            "arena_reset" => "osc_arena_reset_global()".to_string(),
            "file_open_read" => format!("osc_file_open_read({})", arg_strs[0]),
            "file_open_write" => format!("osc_file_open_write({})", arg_strs[0]),
            "read_byte" => format!("osc_read_byte({})", arg_strs[0]),
            "write_byte" => format!("osc_write_byte({}, {})", arg_strs[0], arg_strs[1]),
            "write_str" => format!("osc_write_str({}, {})", arg_strs[0], arg_strs[1]),
            "file_close" => format!("osc_file_close({})", arg_strs[0]),
            "file_delete" => format!("osc_file_delete({})", arg_strs[0]),
            "arg_count" => "osc_arg_count()".to_string(),
            "arg_get" => format!("osc_arg_get(_arena, {})", arg_strs[0]),
            // Tier 1: Character classification
            "char_is_alpha" => format!("osc_char_is_alpha({})", arg_strs[0]),
            "char_is_digit" => format!("osc_char_is_digit({})", arg_strs[0]),
            "char_is_alnum" => format!("osc_char_is_alnum({})", arg_strs[0]),
            "char_is_space" => format!("osc_char_is_space({})", arg_strs[0]),
            "char_is_upper" => format!("osc_char_is_upper({})", arg_strs[0]),
            "char_is_lower" => format!("osc_char_is_lower({})", arg_strs[0]),
            "char_is_print" => format!("osc_char_is_print({})", arg_strs[0]),
            "char_is_xdigit" => format!("osc_char_is_xdigit({})", arg_strs[0]),
            "char_to_upper" => format!("osc_char_to_upper({})", arg_strs[0]),
            "char_to_lower" => format!("osc_char_to_lower({})", arg_strs[0]),
            "abs_i64" => format!("osc_abs_i64({})", arg_strs[0]),
            // Tier 2: Number parsing & conversion
            "parse_i32" => format!("osc_parse_i32({})", arg_strs[0]),
            "parse_i64" => format!("osc_parse_i64({})", arg_strs[0]),
            "str_from_i64" => format!("osc_str_from_i64(_arena, {})", arg_strs[0]),
            "str_from_f64" => format!("osc_str_from_f64(_arena, {})", arg_strs[0]),
            "str_from_bool" => format!("osc_str_from_bool({})", arg_strs[0]),
            // Tier 3: Random, time, sleep, exit
            "rand_seed" => format!("osc_rand_seed({})", arg_strs[0]),
            "rand_i32" => "osc_rand_i32()".to_string(),
            "time_now" => "osc_time_now()".to_string(),
            "sleep_ms" => format!("osc_sleep_ms({})", arg_strs[0]),
            "exit" => format!("osc_exit({})", arg_strs[0]),
            // Tier 4: Environment & error
            "env_get" => format!("osc_env_get(_arena, {})", arg_strs[0]),
            "errno_get" => "osc_errno_get()".to_string(),
            "errno_str" => format!("osc_errno_str({})", arg_strs[0]),
            // Tier 5: Filesystem operations
            "file_rename" => format!("osc_file_rename({}, {})", arg_strs[0], arg_strs[1]),
            "file_exists" => format!("osc_file_exists({})", arg_strs[0]),
            "dir_create" => format!("osc_dir_create({})", arg_strs[0]),
            "dir_remove" => format!("osc_dir_remove({})", arg_strs[0]),
            "dir_current" => "osc_dir_current(_arena)".to_string(),
            "dir_change" => format!("osc_dir_change({})", arg_strs[0]),
            "file_open_append" => format!("osc_file_open_append({})", arg_strs[0]),
            "file_size" => format!("osc_file_size({})", arg_strs[0]),
            // Tier 6: String operations
            "str_contains" => format!("osc_str_contains({}, {})", arg_strs[0], arg_strs[1]),
            "str_starts_with" => format!("osc_str_starts_with({}, {})", arg_strs[0], arg_strs[1]),
            "str_ends_with" => format!("osc_str_ends_with({}, {})", arg_strs[0], arg_strs[1]),
            "str_trim" => format!("osc_str_trim(_arena, {})", arg_strs[0]),
            "str_split" => format!("osc_str_split(_arena, {}, {})", arg_strs[0], arg_strs[1]),
            "str_to_upper" => format!("osc_str_to_upper(_arena, {})", arg_strs[0]),
            "str_to_lower" => format!("osc_str_to_lower(_arena, {})", arg_strs[0]),
            "str_replace" => format!("osc_str_replace(_arena, {}, {}, {})", arg_strs[0], arg_strs[1], arg_strs[2]),
            "str_compare" => format!("osc_str_compare({}, {})", arg_strs[0], arg_strs[1]),
            // Tier 7: Directory listing & process control
            "dir_list" => format!("osc_dir_list(_arena, {})", arg_strs[0]),
            "proc_run" => format!("osc_proc_run({}, {})", arg_strs[0], arg_strs[1]),
            "term_width" => "osc_term_width()".to_string(),
            "term_height" => "osc_term_height()".to_string(),
            // Tier 8: Raw terminal I/O
            "term_raw" => "osc_term_raw()".to_string(),
            "term_restore" => "osc_term_restore()".to_string(),
            "read_nonblock" => "osc_read_nonblock()".to_string(),
            // Tier 9: Environment iteration
            "env_count" => "osc_env_count()".to_string(),
            "env_key" => format!("osc_env_key(_arena, {})", arg_strs[0]),
            "env_value" => format!("osc_env_value(_arena, {})", arg_strs[0]),
            // Tier 10: Hex formatting
            "str_from_i32_hex" => format!("osc_str_from_i32_hex(_arena, {})", arg_strs[0]),
            "str_from_i64_hex" => format!("osc_str_from_i64_hex(_arena, {})", arg_strs[0]),
            // Tier 11: Array sort
            "sort_i32" => format!("osc_sort_i32({})", arg_strs[0]),
            "sort_i64" => format!("osc_sort_i64({})", arg_strs[0]),
            "sort_str" => format!("osc_sort_str({})", arg_strs[0]),
            "sort_f64" => format!("osc_sort_f64({})", arg_strs[0]),
            "len" => format!("osc_array_len({})", arg_strs[0]),
            "push" => {
                // Need to get element type for the &val
                let arr_ty = self.type_of(&args[0]);
                let elem_ty = match &arr_ty {
                    BcType::Array(e) => (**e).clone(),
                    _ => BcType::I32,
                };
                let elem_c = self.type_to_c(&elem_ty);
                let tmp = self.fresh_tmp();
                self.line(&format!("{} {} = {};", elem_c, tmp, arg_strs[1]));
                format!("osc_array_push(_arena, {}, &{})", arg_strs[0], tmp)
            }
            _ => {
                // User-defined or extern function
                let fi = self.functions.get(name).cloned();
                let cname = Self::mangle_c_name(name);
                if let Some(info) = fi {
                    if info.is_extern {
                        format!("{}({})", cname, arg_strs.join(", "))
                    } else {
                        arg_strs.insert(0, "_arena".to_string());
                        format!("{}({})", cname, arg_strs.join(", "))
                    }
                } else {
                    arg_strs.insert(0, "_arena".to_string());
                    format!("{}({})", cname, arg_strs.join(", "))
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // If expression
    // -----------------------------------------------------------------------

    fn emit_if(&mut self, condition: &Expr, then_block: &Block, else_branch: Option<&Expr>) -> String {
        // Pre-scan then_block let stmts to determine type correctly
        let ty = if then_block.tail_expr.is_some() {
            self.push_scope();
            for stmt in &then_block.stmts {
                if let Stmt::Let(ls) = stmt {
                    let ty = self.resolve_ast_type(&ls.ty);
                    self.scopes.last_mut().unwrap().insert(ls.name.clone(), ty);
                }
            }
            let ty = self.block_type(then_block);
            self.pop_scope();
            ty
        } else {
            BcType::Unit
        };
        let cond = self.emit_expr(condition);

        if ty == BcType::Unit {
            self.line(&format!("if ({}) {{", cond));
            self.indent += 1;
            self.push_scope();
            for s in &then_block.stmts {
                self.emit_stmt(s);
            }
            if let Some(tail) = &then_block.tail_expr {
                let v = self.emit_expr(tail);
                if v != "(void)0" {
                    self.line(&format!("{};", v));
                }
            }
            self.pop_scope();
            self.indent -= 1;
            if let Some(else_expr) = else_branch {
                match else_expr {
                    Expr::If { condition: ec, then_block: et, else_branch: ee, .. } => {
                        // else if
                        let ec_c = self.emit_expr(ec);
                        self.line(&format!("}} else if ({}) {{", ec_c));
                        self.indent += 1;
                        self.push_scope();
                        for s in &et.stmts {
                            self.emit_stmt(s);
                        }
                        if let Some(tail) = &et.tail_expr {
                            let v = self.emit_expr(tail);
                            if v != "(void)0" {
                                self.line(&format!("{};", v));
                            }
                        }
                        self.pop_scope();
                        self.indent -= 1;
                        if let Some(ee) = ee {
                            self.emit_else_unit(ee);
                        }
                        self.line("}");
                    }
                    Expr::Block(blk) => {
                        self.line("} else {");
                        self.indent += 1;
                        self.push_scope();
                        for s in &blk.stmts {
                            self.emit_stmt(s);
                        }
                        if let Some(tail) = &blk.tail_expr {
                            let v = self.emit_expr(tail);
                            if v != "(void)0" {
                                self.line(&format!("{};", v));
                            }
                        }
                        self.pop_scope();
                        self.indent -= 1;
                        self.line("}");
                    }
                    _ => {
                        self.line("} else {");
                        self.indent += 1;
                        let v = self.emit_expr(else_expr);
                        if v != "(void)0" {
                            self.line(&format!("{};", v));
                        }
                        self.indent -= 1;
                        self.line("}");
                    }
                }
            } else {
                self.line("}");
            }
            "(void)0".to_string()
        } else {
            // Expression if — use temp variable
            let tmp = self.fresh_tmp();
            let c_ty = self.type_to_c(&ty);
            self.line(&format!("{} {};", c_ty, tmp));
            self.line(&format!("if ({}) {{", cond));
            self.indent += 1;
            self.push_scope();
            let then_val = self.emit_block_body(then_block);
            self.line(&format!("{} = {};", tmp, then_val));
            self.pop_scope();
            self.indent -= 1;
            if let Some(else_expr) = else_branch {
                self.line("} else {");
                self.indent += 1;
                self.push_scope();
                let else_val = self.emit_expr(else_expr);
                self.line(&format!("{} = {};", tmp, else_val));
                self.pop_scope();
                self.indent -= 1;
            }
            self.line("}");
            tmp
        }
    }

    fn emit_else_unit(&mut self, expr: &Expr) {
        match expr {
            Expr::If { condition, then_block, else_branch, .. } => {
                let cond = self.emit_expr(condition);
                self.line(&format!("}} else if ({}) {{", cond));
                self.indent += 1;
                self.push_scope();
                for s in &then_block.stmts {
                    self.emit_stmt(s);
                }
                if let Some(tail) = &then_block.tail_expr {
                    let v = self.emit_expr(tail);
                    if v != "(void)0" {
                        self.line(&format!("{};", v));
                    }
                }
                self.pop_scope();
                self.indent -= 1;
                if let Some(ee) = else_branch {
                    self.emit_else_unit(ee);
                }
            }
            Expr::Block(blk) => {
                self.line("} else {");
                self.indent += 1;
                self.push_scope();
                for s in &blk.stmts {
                    self.emit_stmt(s);
                }
                if let Some(tail) = &blk.tail_expr {
                    let v = self.emit_expr(tail);
                    if v != "(void)0" {
                        self.line(&format!("{};", v));
                    }
                }
                self.pop_scope();
                self.indent -= 1;
            }
            _ => {
                self.line("} else {");
                self.indent += 1;
                let v = self.emit_expr(expr);
                if v != "(void)0" {
                    self.line(&format!("{};", v));
                }
                self.indent -= 1;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Match expression
    // -----------------------------------------------------------------------

    fn emit_match(&mut self, scrutinee: &Expr, arms: &[MatchArm]) -> String {
        let scrut_ty = self.type_of(scrutinee);
        let scrut_c = self.emit_expr(scrutinee);

        // Determine result type from first arm, pre-scanning pattern bindings
        let result_ty = self.match_arm_result_type(&scrut_ty, &arms[0]);

        if result_ty == BcType::Unit {
            self.emit_match_unit(&scrut_c, &scrut_ty, arms);
            "(void)0".to_string()
        } else {
            let tmp = self.fresh_tmp();
            let c_ty = self.type_to_c(&result_ty);
            self.line(&format!("{} {};", c_ty, tmp));
            self.emit_match_valued(&tmp, &scrut_c, &scrut_ty, arms);
            tmp
        }
    }

    fn emit_match_unit(&mut self, scrut_c: &str, scrut_ty: &BcType, arms: &[MatchArm]) {
        match scrut_ty {
            BcType::Enum(ename) => {
                let has_payload = self.enums.get(ename)
                    .map(|i| i.variants.iter().any(|v| !v.1.is_empty()))
                    .unwrap_or(false);
                if has_payload {
                    self.line(&format!("switch ({}.tag) {{", scrut_c));
                } else {
                    self.line(&format!("switch ({}) {{", scrut_c));
                }
                self.indent += 1;
                for arm in arms {
                    self.emit_enum_arm_unit(scrut_c, ename, arm);
                }
                self.indent -= 1;
                self.line("}");
            }
            BcType::Result(ok_ty, err_ty) => {
                self.emit_result_match_unit(scrut_c, ok_ty, err_ty, arms);
            }
            _ => {
                // Literal match using if/else
                self.emit_literal_match_unit(scrut_c, scrut_ty, arms);
            }
        }
    }

    fn emit_match_valued(&mut self, tmp: &str, scrut_c: &str, scrut_ty: &BcType, arms: &[MatchArm]) {
        match scrut_ty {
            BcType::Enum(ename) => {
                let has_payload = self.enums.get(ename)
                    .map(|i| i.variants.iter().any(|v| !v.1.is_empty()))
                    .unwrap_or(false);
                if has_payload {
                    self.line(&format!("switch ({}.tag) {{", scrut_c));
                } else {
                    self.line(&format!("switch ({}) {{", scrut_c));
                }
                self.indent += 1;
                for arm in arms {
                    self.emit_enum_arm_valued(tmp, scrut_c, ename, arm);
                }
                self.indent -= 1;
                self.line("}");
            }
            BcType::Result(ok_ty, err_ty) => {
                self.emit_result_match_valued(tmp, scrut_c, ok_ty, err_ty, arms);
            }
            _ => {
                self.emit_literal_match_valued(tmp, scrut_c, scrut_ty, arms);
            }
        }
    }

    fn emit_enum_arm_unit(&mut self, scrut_c: &str, ename: &str, arm: &MatchArm) {
        match &arm.pattern {
            Pattern::Wildcard(_) => {
                self.line("default: {");
            }
            Pattern::Ident(name, _) => {
                self.line("default: {");
                self.indent += 1;
                let _info = self.enums.get(ename).unwrap().clone();
                // Can't easily bind the whole value for default
                let ty = BcType::Enum(ename.to_string());
                let c_ty = self.type_to_c(&ty);
                self.line(&format!("{} {} = {};", c_ty, name, scrut_c));
                self.push_scope();
                self.scopes.last_mut().unwrap().insert(name.clone(), ty);
                let v = self.emit_expr(&arm.body);
                if v != "(void)0" {
                    self.line(&format!("{};", v));
                }
                self.pop_scope();
                self.indent -= 1;
                self.line("    break;");
                self.line("}");
                return;
            }
            Pattern::Enum { variant, bindings: _, .. } => {
                self.line(&format!("case {}_TAG_{}: {{", ename, variant));
            }
            _ => { self.line("default: {"); }
        }
        self.indent += 1;
        self.push_scope();
        self.bind_enum_payload(scrut_c, ename, &arm.pattern);
        let v = self.emit_expr(&arm.body);
        if v != "(void)0" {
            self.line(&format!("{};", v));
        }
        self.pop_scope();
        self.line("break;");
        self.indent -= 1;
        self.line("}");
    }

    fn emit_enum_arm_valued(&mut self, tmp: &str, scrut_c: &str, ename: &str, arm: &MatchArm) {
        match &arm.pattern {
            Pattern::Wildcard(_) => {
                self.line("default: {");
            }
            Pattern::Enum { variant, .. } => {
                self.line(&format!("case {}_TAG_{}: {{", ename, variant));
            }
            _ => { self.line("default: {"); }
        }
        self.indent += 1;
        self.push_scope();
        self.bind_enum_payload(scrut_c, ename, &arm.pattern);
        let v = self.emit_expr(&arm.body);
        self.line(&format!("{} = {};", tmp, v));
        self.pop_scope();
        self.line("break;");
        self.indent -= 1;
        self.line("}");
    }

    fn bind_enum_payload(&mut self, scrut_c: &str, ename: &str, pattern: &Pattern) {
        if let Pattern::Enum { variant, bindings, .. } = pattern {
            let info = self.enums.get(ename).unwrap().clone();
            let var_info = info.variants.iter().find(|(n, _)| n == variant);
            if let Some((_, payload_types)) = var_info {
                for (i, binding) in bindings.iter().enumerate() {
                    if let Pattern::Ident(name, _) = binding {
                        if i < payload_types.len() {
                            let ty = &payload_types[i];
                            let c_ty = self.type_to_c(ty);
                            self.line(&format!("{} {} = {}.data.{}._{};",
                                c_ty, name, scrut_c, variant, i));
                            self.scopes.last_mut().unwrap().insert(name.clone(), ty.clone());
                        }
                    }
                }
            }
        }
    }

    fn emit_result_match_unit(&mut self, scrut_c: &str, ok_ty: &BcType, err_ty: &BcType, arms: &[MatchArm]) {
        let mut ok_arm = None;
        let mut err_arm = None;
        let mut wildcard_arm = None;
        for arm in arms {
            match &arm.pattern {
                Pattern::Enum { variant, .. } if variant == "Ok" => ok_arm = Some(arm),
                Pattern::Enum { variant, .. } if variant == "Err" => err_arm = Some(arm),
                Pattern::Wildcard(_) => wildcard_arm = Some(arm),
                _ => wildcard_arm = Some(arm),
            }
        }

        self.line(&format!("if ({}.is_ok) {{", scrut_c));
        self.indent += 1;
        self.push_scope();
        if let Some(arm) = ok_arm {
            if let Pattern::Enum { bindings, .. } = &arm.pattern {
                if let Some(Pattern::Ident(name, _)) = bindings.first() {
                    let c_ty = self.type_to_c(ok_ty);
                    self.line(&format!("{} {} = {}.value.ok;", c_ty, name, scrut_c));
                    self.scopes.last_mut().unwrap().insert(name.clone(), ok_ty.clone());
                }
            }
            let v = self.emit_expr(&arm.body);
            if v != "(void)0" { self.line(&format!("{};", v)); }
        } else if let Some(arm) = wildcard_arm {
            let v = self.emit_expr(&arm.body);
            if v != "(void)0" { self.line(&format!("{};", v)); }
        }
        self.pop_scope();
        self.indent -= 1;
        self.line("} else {");
        self.indent += 1;
        self.push_scope();
        if let Some(arm) = err_arm {
            if let Pattern::Enum { bindings, .. } = &arm.pattern {
                if let Some(Pattern::Ident(name, _)) = bindings.first() {
                    let c_ty = self.type_to_c(err_ty);
                    self.line(&format!("{} {} = {}.value.err;", c_ty, name, scrut_c));
                    self.scopes.last_mut().unwrap().insert(name.clone(), err_ty.clone());
                }
            }
            let v = self.emit_expr(&arm.body);
            if v != "(void)0" { self.line(&format!("{};", v)); }
        } else if let Some(arm) = wildcard_arm {
            let v = self.emit_expr(&arm.body);
            if v != "(void)0" { self.line(&format!("{};", v)); }
        }
        self.pop_scope();
        self.indent -= 1;
        self.line("}");
    }

    fn emit_result_match_valued(&mut self, tmp: &str, scrut_c: &str, ok_ty: &BcType, err_ty: &BcType, arms: &[MatchArm]) {
        let mut ok_arm = None;
        let mut err_arm = None;
        let mut wildcard_arm = None;
        for arm in arms {
            match &arm.pattern {
                Pattern::Enum { variant, .. } if variant == "Ok" => ok_arm = Some(arm),
                Pattern::Enum { variant, .. } if variant == "Err" => err_arm = Some(arm),
                Pattern::Wildcard(_) => wildcard_arm = Some(arm),
                _ => wildcard_arm = Some(arm),
            }
        }

        self.line(&format!("if ({}.is_ok) {{", scrut_c));
        self.indent += 1;
        self.push_scope();
        if let Some(arm) = ok_arm {
            if let Pattern::Enum { bindings, .. } = &arm.pattern {
                if let Some(Pattern::Ident(name, _)) = bindings.first() {
                    let c_ty = self.type_to_c(ok_ty);
                    self.line(&format!("{} {} = {}.value.ok;", c_ty, name, scrut_c));
                    self.scopes.last_mut().unwrap().insert(name.clone(), ok_ty.clone());
                }
            }
            let v = self.emit_expr(&arm.body);
            self.line(&format!("{} = {};", tmp, v));
        } else if let Some(arm) = wildcard_arm {
            let v = self.emit_expr(&arm.body);
            self.line(&format!("{} = {};", tmp, v));
        }
        self.pop_scope();
        self.indent -= 1;
        self.line("} else {");
        self.indent += 1;
        self.push_scope();
        if let Some(arm) = err_arm {
            if let Pattern::Enum { bindings, .. } = &arm.pattern {
                if let Some(Pattern::Ident(name, _)) = bindings.first() {
                    let c_ty = self.type_to_c(err_ty);
                    self.line(&format!("{} {} = {}.value.err;", c_ty, name, scrut_c));
                    self.scopes.last_mut().unwrap().insert(name.clone(), err_ty.clone());
                }
            }
            let v = self.emit_expr(&arm.body);
            self.line(&format!("{} = {};", tmp, v));
        } else if let Some(arm) = wildcard_arm {
            let v = self.emit_expr(&arm.body);
            self.line(&format!("{} = {};", tmp, v));
        }
        self.pop_scope();
        self.indent -= 1;
        self.line("}");
    }

    fn emit_literal_match_unit(&mut self, scrut_c: &str, scrut_ty: &BcType, arms: &[MatchArm]) {
        let mut first = true;
        for arm in arms {
            match &arm.pattern {
                Pattern::Wildcard(_) => {
                    if first { self.line("{"); } else { self.line("} else {"); }
                    self.indent += 1;
                    self.push_scope();
                    let v = self.emit_expr(&arm.body);
                    if v != "(void)0" { self.line(&format!("{};", v)); }
                    self.pop_scope();
                    self.indent -= 1;
                    self.line("}");
                    return;
                }
                Pattern::Ident(name, _) => {
                    if first { self.line("{"); } else { self.line("} else {"); }
                    self.indent += 1;
                    self.push_scope();
                    let c_ty = self.type_to_c(scrut_ty);
                    self.line(&format!("{} {} = {};", c_ty, name, scrut_c));
                    self.scopes.last_mut().unwrap().insert(name.clone(), scrut_ty.clone());
                    let v = self.emit_expr(&arm.body);
                    if v != "(void)0" { self.line(&format!("{};", v)); }
                    self.pop_scope();
                    self.indent -= 1;
                    self.line("}");
                    return;
                }
                _ => {
                    let pat_val = self.pattern_to_c(&arm.pattern);
                    let cmp = self.emit_comparison(scrut_c, scrut_ty, &pat_val);
                    if first {
                        self.line(&format!("if ({}) {{", cmp));
                        first = false;
                    } else {
                        self.line(&format!("}} else if ({}) {{", cmp));
                    }
                    self.indent += 1;
                    self.push_scope();
                    let v = self.emit_expr(&arm.body);
                    if v != "(void)0" { self.line(&format!("{};", v)); }
                    self.pop_scope();
                    self.indent -= 1;
                }
            }
        }
        self.line("}");
    }

    fn emit_literal_match_valued(&mut self, tmp: &str, scrut_c: &str, scrut_ty: &BcType, arms: &[MatchArm]) {
        let mut first = true;
        for arm in arms {
            match &arm.pattern {
                Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                    if first { self.line("{"); } else { self.line("} else {"); }
                    self.indent += 1;
                    self.push_scope();
                    if let Pattern::Ident(name, _) = &arm.pattern {
                        let c_ty = self.type_to_c(scrut_ty);
                        self.line(&format!("{} {} = {};", c_ty, name, scrut_c));
                        self.scopes.last_mut().unwrap().insert(name.clone(), scrut_ty.clone());
                    }
                    let v = self.emit_expr(&arm.body);
                    self.line(&format!("{} = {};", tmp, v));
                    self.pop_scope();
                    self.indent -= 1;
                    self.line("}");
                    return;
                }
                _ => {
                    let pat_val = self.pattern_to_c(&arm.pattern);
                    let cmp = self.emit_comparison(scrut_c, scrut_ty, &pat_val);
                    if first {
                        self.line(&format!("if ({}) {{", cmp));
                        first = false;
                    } else {
                        self.line(&format!("}} else if ({}) {{", cmp));
                    }
                    self.indent += 1;
                    self.push_scope();
                    let v = self.emit_expr(&arm.body);
                    self.line(&format!("{} = {};", tmp, v));
                    self.pop_scope();
                    self.indent -= 1;
                }
            }
        }
        self.line("}");
    }

    fn pattern_to_c(&self, pat: &Pattern) -> String {
        match pat {
            Pattern::IntLit(v, _) => format!("{}", v),
            Pattern::FloatLit(v, _) => format!("{}", v),
            Pattern::StringLit(s, _) => format!("osc_str_from_cstr(\"{}\")", self.escape_c_string(s)),
            Pattern::BoolLit(b, _) => if *b { "1".to_string() } else { "0".to_string() },
            _ => "0".to_string(),
        }
    }

    fn emit_comparison(&self, lhs: &str, ty: &BcType, rhs: &str) -> String {
        match ty {
            BcType::Str => format!("osc_str_eq({}, {})", lhs, rhs),
            _ => format!("({} == {})", lhs, rhs),
        }
    }

    // -----------------------------------------------------------------------
    // Try expression
    // -----------------------------------------------------------------------

    fn emit_try(&mut self, call: &Expr, _span: crate::token::Span) -> String {
        let call_ty = self.type_of(call);
        let (_ok_ty, _err_ty) = match &call_ty {
            BcType::Result(o, e) => ((**o).clone(), (**e).clone()),
            _ => (BcType::Unit, BcType::Str),
        };

        let call_c = self.emit_expr(call);
        let call_tmp = self.fresh_tmp();
        let call_c_ty = self.type_to_c(&call_ty);
        self.line(&format!("{} {} = {};", call_c_ty, call_tmp, call_c));

        // Early return on error
        let fn_ret = self.current_fn_return_type.clone().unwrap_or(BcType::Unit);
        let fn_ret_c = self.type_to_c(&fn_ret);
        self.line(&format!("if (!{}.is_ok) {{", call_tmp));
        self.indent += 1;
        self.line(&format!("{} _err_ret;", fn_ret_c));
        self.line("_err_ret.is_ok = 0;");
        self.line(&format!("_err_ret.value.err = {}.value.err;", call_tmp));
        self.line("return _err_ret;");
        self.indent -= 1;
        self.line("}");

        format!("{}.value.ok", call_tmp)
    }

    // -----------------------------------------------------------------------
    // Array literal
    // -----------------------------------------------------------------------

    fn emit_array_lit(&mut self, elements: &[Expr]) -> String {
        if elements.is_empty() {
            let size_expr = if let Some(ref elem_ty) = self.expected_array_elem_type {
                self.c_sizeof(elem_ty)
            } else {
                // Fallback: should not happen with correct type annotations
                "1".to_string()
            };
            return format!("osc_array_new(_arena, {}, 0)", size_expr);
        }

        let elem_ty = self.type_of(&elements[0]);
        let elem_c = self.type_to_c(&elem_ty);
        let tmp = self.fresh_tmp();
        let size_expr = self.c_sizeof(&elem_ty);
        self.line(&format!("osc_array* {} = osc_array_new(_arena, {}, {});",
            tmp, size_expr, elements.len()));
        for elem in elements {
            let v = self.emit_expr(elem);
            let push_tmp = self.fresh_tmp();
            self.line(&format!("{} {} = {};", elem_c, push_tmp, v));
            self.line(&format!("osc_array_push(_arena, {}, &{});", tmp, push_tmp));
        }
        tmp
    }

    // -----------------------------------------------------------------------
    // Struct literal
    // -----------------------------------------------------------------------

    fn emit_struct_lit(&mut self, name: &str, fields: &[FieldInit]) -> String {
        let mut parts = Vec::new();
        for fi in fields {
            let v = self.emit_expr(&fi.value);
            parts.push(format!(".{} = {}", fi.name, v));
        }
        format!("({}){{ {} }}", name, parts.join(", "))
    }

    // -----------------------------------------------------------------------
    // Enum constructor
    // -----------------------------------------------------------------------

    fn emit_enum_constructor(&mut self, enum_name: &str, variant: &str, args: &[Expr]) -> String {
        if enum_name == "Result" {
            return self.emit_result_constructor(variant, args);
        }

        let info = self.enums.get(enum_name).cloned();
        let has_payload = info.as_ref()
            .map(|i| i.variants.iter().any(|v| !v.1.is_empty()))
            .unwrap_or(false);

        if !has_payload {
            // Simple int enum
            return format!("{}_TAG_{}", enum_name, variant);
        }

        if args.is_empty() {
            format!("({}){{ .tag = {}_TAG_{} }}", enum_name, enum_name, variant)
        } else {
            let mut payload_parts = Vec::new();
            for (i, arg) in args.iter().enumerate() {
                let v = self.emit_expr(arg);
                payload_parts.push(format!("._{} = {}", i, v));
            }
            format!("({}){{ .tag = {}_TAG_{}, .data.{} = {{ {} }} }}",
                enum_name, enum_name, variant, variant, payload_parts.join(", "))
        }
    }

    fn emit_result_constructor(&mut self, variant: &str, args: &[Expr]) -> String {
        let fn_ret = self.current_fn_return_type.clone().unwrap_or(BcType::Unit);
        let result_c_ty = self.type_to_c(&fn_ret);

        match variant {
            "Ok" => {
                let val = self.emit_expr(&args[0]);
                let (ok_ty, _) = match &fn_ret {
                    BcType::Result(o, _) => ((**o).clone(), BcType::Unit),
                    _ => (BcType::Unit, BcType::Unit),
                };
                if ok_ty == BcType::Unit {
                    format!("({}){{ .is_ok = 1 }}", result_c_ty)
                } else {
                    format!("({}){{ .is_ok = 1, .value = {{ .ok = {} }} }}", result_c_ty, val)
                }
            }
            "Err" => {
                let val = self.emit_expr(&args[0]);
                format!("({}){{ .is_ok = 0, .value = {{ .err = {} }} }}", result_c_ty, val)
            }
            _ => "0".to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // Type helpers
    // -----------------------------------------------------------------------

    fn type_to_c(&self, ty: &BcType) -> String {
        match ty {
            BcType::I32 => "int32_t".to_string(),
            BcType::I64 => "int64_t".to_string(),
            BcType::F64 => "double".to_string(),
            BcType::Bool => "uint8_t".to_string(),
            BcType::Str => "osc_str".to_string(),
            BcType::Unit => "void".to_string(),
            BcType::Struct(name) => name.clone(),
            BcType::Enum(name) => name.clone(),
            BcType::Array(_) | BcType::FixedArray(_, _) => "osc_array*".to_string(),
            BcType::Result(ok, err) => self.result_type_name(ok, err),
        }
    }

    fn result_type_name(&self, ok: &BcType, err: &BcType) -> String {
        format!("osc_result_{}_{}", self.type_tag(ok), self.type_tag(err))
    }

    fn type_tag(&self, ty: &BcType) -> String {
        match ty {
            BcType::I32 => "i32".to_string(),
            BcType::I64 => "i64".to_string(),
            BcType::F64 => "f64".to_string(),
            BcType::Bool => "bool".to_string(),
            BcType::Str => "str".to_string(),
            BcType::Unit => "unit".to_string(),
            BcType::Struct(name) => name.to_lowercase(),
            BcType::Enum(name) => name.to_lowercase(),
            BcType::Array(e) => format!("arr_{}", self.type_tag(e)),
            BcType::FixedArray(e, n) => format!("arr_{}_{}", self.type_tag(e), n),
            BcType::Result(o, e) => format!("result_{}_{}", self.type_tag(o), self.type_tag(e)),
        }
    }

    fn c_sizeof(&self, ty: &BcType) -> String {
        match ty {
            BcType::I32 => "sizeof(int32_t)".to_string(),
            BcType::I64 => "sizeof(int64_t)".to_string(),
            BcType::F64 => "sizeof(double)".to_string(),
            BcType::Bool => "sizeof(uint8_t)".to_string(),
            BcType::Str => "sizeof(osc_str)".to_string(),
            BcType::Struct(name) => format!("sizeof({})", name),
            BcType::Enum(name) => format!("sizeof({})", name),
            BcType::Array(_) | BcType::FixedArray(_, _) => "sizeof(osc_array*)".to_string(),
            BcType::Result(ok, err) => format!("sizeof({})", self.result_type_name(ok, err)),
            BcType::Unit => "1".to_string(),
        }
    }

    fn resolve_ast_type(&self, ty: &Type) -> BcType {
        match ty {
            Type::Primitive(p, _) => match p {
                PrimitiveType::I32 => BcType::I32,
                PrimitiveType::I64 => BcType::I64,
                PrimitiveType::F64 => BcType::F64,
                PrimitiveType::Bool => BcType::Bool,
                PrimitiveType::Str => BcType::Str,
                PrimitiveType::Unit => BcType::Unit,
            },
            Type::Named(name, _) => {
                if self.structs.contains_key(name) {
                    BcType::Struct(name.clone())
                } else {
                    BcType::Enum(name.clone())
                }
            }
            Type::FixedArray(elem, size, _) => {
                BcType::FixedArray(Box::new(self.resolve_ast_type(elem)), *size)
            }
            Type::DynamicArray(elem, _) => {
                BcType::Array(Box::new(self.resolve_ast_type(elem)))
            }
            Type::Result(ok, err, _) => {
                BcType::Result(
                    Box::new(self.resolve_ast_type(ok)),
                    Box::new(self.resolve_ast_type(err)),
                )
            }
        }
    }

    /// Determine the type of an expression (for codegen, uses scope + symbol tables)
    fn type_of(&self, expr: &Expr) -> BcType {
        match expr {
            Expr::IntLit(_, _) => BcType::I32,
            Expr::FloatLit(_, _) => BcType::F64,
            Expr::StringLit(_, _) => BcType::Str,
            Expr::BoolLit(_, _) => BcType::Bool,
            Expr::Ident(name, _) => {
                self.lookup_type(name).unwrap_or(BcType::Unit)
            }
            Expr::BinaryOp { op, left, .. } => {
                match op {
                    BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Gt
                    | BinOp::LtEq | BinOp::GtEq | BinOp::And | BinOp::Or => BcType::Bool,
                    _ => self.type_of(left),
                }
            }
            Expr::UnaryOp { op, operand, .. } => {
                match op {
                    UnaryOp::Not => BcType::Bool,
                    UnaryOp::Neg => self.type_of(operand),
                }
            }
            Expr::Cast { ty, .. } => self.resolve_ast_type(ty),
            Expr::Call { callee, args: _, .. } => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    if name == "len" { return BcType::I32; }
                    if name == "push" { return BcType::Unit; }
                    self.functions.get(name)
                        .map(|f| f.return_type.clone())
                        .unwrap_or(BcType::Unit)
                } else {
                    BcType::Unit
                }
            }
            Expr::FieldAccess { expr: obj, field, .. } => {
                if let BcType::Struct(name) = self.type_of(obj) {
                    self.structs.get(&name)
                        .and_then(|s| s.fields.iter().find(|(n, _)| n == field))
                        .map(|(_, t)| t.clone())
                        .unwrap_or(BcType::Unit)
                } else {
                    BcType::Unit
                }
            }
            Expr::Index { expr: arr, .. } => {
                match self.type_of(arr) {
                    BcType::Array(e) | BcType::FixedArray(e, _) => *e,
                    _ => BcType::Unit,
                }
            }
            Expr::Block(block) => self.block_type(block),
            Expr::If { then_block, else_branch, .. } => {
                if else_branch.is_some() {
                    self.block_type(then_block)
                } else {
                    BcType::Unit
                }
            }
            Expr::Match { arms, .. } => {
                if arms.is_empty() {
                    BcType::Unit
                } else {
                    self.arm_body_type(&arms[0].body)
                }
            }
            Expr::Try { call, .. } => {
                match self.type_of(call) {
                    BcType::Result(ok, _) => *ok,
                    _ => BcType::Unit,
                }
            }
            Expr::ArrayLit { elements, .. } => {
                if elements.is_empty() {
                    BcType::Array(Box::new(BcType::Unit))
                } else {
                    BcType::Array(Box::new(self.type_of(&elements[0])))
                }
            }
            Expr::StructLit { name, .. } => BcType::Struct(name.clone()),
            Expr::EnumConstructor { enum_name, .. } => {
                if enum_name == "Result" {
                    self.current_fn_return_type.clone().unwrap_or(BcType::Unit)
                } else {
                    BcType::Enum(enum_name.clone())
                }
            }
        }
    }

    fn block_type(&self, block: &Block) -> BcType {
        if let Some(tail) = &block.tail_expr {
            self.type_of(tail)
        } else {
            BcType::Unit
        }
    }

    fn arm_body_type(&self, body: &Expr) -> BcType {
        self.type_of(body)
    }

    /// Determine the result type of a match arm by temporarily pushing
    /// pattern bindings into scope so identifiers resolve correctly.
    fn match_arm_result_type(&mut self, scrut_ty: &BcType, arm: &MatchArm) -> BcType {
        self.push_scope();
        // Register pattern bindings from enum/result patterns
        match (&arm.pattern, scrut_ty) {
            (Pattern::Enum { enum_name, variant, bindings, .. }, BcType::Enum(ename)) => {
                let lookup = if enum_name.is_empty() { ename.as_str() } else { enum_name.as_str() };
                if let Some(info) = self.enums.get(lookup).cloned() {
                    if let Some((_, payload_types)) = info.variants.iter().find(|(n, _)| n == variant) {
                        for (i, binding) in bindings.iter().enumerate() {
                            if let Pattern::Ident(name, _) = binding {
                                if i < payload_types.len() {
                                    self.scopes.last_mut().unwrap().insert(name.clone(), payload_types[i].clone());
                                }
                            }
                        }
                    }
                }
            }
            (Pattern::Enum { variant, bindings, .. }, BcType::Result(ok_ty, err_ty)) => {
                if let Some(Pattern::Ident(name, _)) = bindings.first() {
                    match variant.as_str() {
                        "Ok" => { self.scopes.last_mut().unwrap().insert(name.clone(), (**ok_ty).clone()); }
                        "Err" => { self.scopes.last_mut().unwrap().insert(name.clone(), (**err_ty).clone()); }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        // Also pre-scan let stmts in arm body if it's a block
        if let Expr::Block(block) = &arm.body {
            for stmt in &block.stmts {
                if let Stmt::Let(ls) = stmt {
                    let ty = self.resolve_ast_type(&ls.ty);
                    self.scopes.last_mut().unwrap().insert(ls.name.clone(), ty);
                }
            }
        }
        let ty = self.type_of(&arm.body);
        self.pop_scope();
        ty
    }

    fn lookup_type(&self, name: &str) -> Option<BcType> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        if let Some(ci) = self.constants.get(name) {
            return Some(ci.ty.clone());
        }
        None
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn fn_return_c(&self, f: &FnDecl) -> String {
        match &f.return_type {
            Some(t) => self.type_to_c(&self.resolve_ast_type(t)),
            None => "void".to_string(),
        }
    }

    fn fn_params_c(&self, f: &FnDecl) -> String {
        let mut parts = vec!["osc_arena* _arena".to_string()];
        for p in &f.params {
            let ty = self.resolve_ast_type(&p.ty);
            let c_ty = self.type_to_c(&ty);
            parts.push(format!("{} {}", c_ty, p.name));
        }
        parts.join(", ")
    }

    fn escape_c_string(&self, s: &str) -> String {
        let mut out = String::new();
        for c in s.chars() {
            match c {
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\r' => out.push_str("\\r"),
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\0' => out.push_str("\\0"),
                _ => out.push(c),
            }
        }
        out
    }
}
