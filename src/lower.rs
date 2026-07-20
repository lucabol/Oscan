//! AST → IR lowering.
//!
//! This is the single place where "what type is this expression" and "is
//! this call direct or indirect" get decided. It runs once, after semantic
//! analysis has already accepted the program (so every lookup here is
//! expected to succeed — this module does not re-implement error
//! reporting; `crate::semantic` already rejected anything that would make
//! these lookups fail).
//!
//! The lowering keeps a small scope stack (`Vec<HashMap<String, BcType>>`),
//! matching the scope discipline the old `codegen.rs` used for its own type
//! re-derivation: one scope frame per function body/param list, and one
//! additional nested frame anywhere a `{ ... }` block, `if`/`else` branch,
//! `while`/`for`/`for-in` body, `arena` body, or `match` arm introduces new
//! bindings. Because lowering assigns every [`crate::ir::Expr`] node its
//! resolved type exactly once (immediately after lowering its children, by
//! reading `.ty()` off the already-lowered subexpressions), the resulting
//! IR carries all the type information the C backend used to re-derive on
//! every emission.

use std::collections::HashMap;

use crate::ast;
use crate::ir;
use crate::types::{BcType, SemanticInfo};

struct Lowering<'a> {
    info: &'a SemanticInfo,
    scopes: Vec<HashMap<String, BcType>>,
    current_fn_return_type: Option<BcType>,
    /// The full declared `Array`/`FixedArray` type of the enclosing `let`
    /// (or top-level `let`/const), if any. Consulted while lowering an
    /// array literal: an *empty* literal always takes this type's element
    /// type (mirroring the old codegen's `expected_array_elem_type` side
    /// channel), while a *non-empty* literal only becomes `FixedArray` when
    /// this context is itself a matching `FixedArray` (see `semantic.rs`'s
    /// `Expr::ArrayLit` check, which this mirrors exactly). Set once per
    /// `let`/`return`/call-argument/struct-field and not narrowed further
    /// for nested array literals — the same coarse, single-level side
    /// channel the old codegen used, not a fully recursive "expected type"
    /// threaded through every sub-expression.
    expected_array_type: Option<BcType>,
    /// The contextual `Result<T, E>` type that an `Result::Ok`/`Result::Err`
    /// constructor should adopt, if lowering is currently inside a position
    /// with a more specific expected type than the enclosing function's own
    /// return type (e.g. a `let` with an explicit `Result<T, E>` type, or a
    /// call argument/struct field whose declared type is `Result<T, E>`).
    /// Mirrors `semantic.rs`'s `expected` parameter, restricted to the one
    /// case (`Result` constructors) that actually depends on it; falls back
    /// to `current_fn_return_type` when unset, matching `semantic.rs`'s own
    /// fallback in `infer_result_type`.
    expected_result_type: Option<BcType>,
}

/// Lower a semantically-checked AST into the backend-neutral IR.
pub fn lower_program(program: &ast::Program, info: &SemanticInfo) -> ir::Program {
    let mut lw = Lowering {
        info,
        scopes: Vec::new(),
        current_fn_return_type: None,
        expected_array_type: None,
        expected_result_type: None,
    };

    let mut struct_defs = Vec::new();
    let mut enum_defs = Vec::new();
    let mut extern_blocks = Vec::new();
    let mut const_defs = Vec::new();
    let mut fn_defs = Vec::new();

    for decl in &program.decls {
        match decl {
            ast::TopDecl::Struct(s) => struct_defs.push(lw.lower_struct(s)),
            ast::TopDecl::Enum(e) => enum_defs.push(lw.lower_enum(e)),
            ast::TopDecl::Extern(eb) => extern_blocks.push(lw.lower_extern_block(eb)),
            ast::TopDecl::Let(l) => const_defs.push(lw.lower_const(l)),
            ast::TopDecl::Fn(f) => fn_defs.push(lw.lower_fn(f)),
            // `use` is a parse-time import mechanism; it carries no
            // runtime/codegen meaning by the time we reach the IR.
            ast::TopDecl::Use(_, _) => {}
        }
    }

    ir::Program {
        structs: info.structs.clone(),
        enums: info.enums.clone(),
        functions: info.functions.clone(),
        constants: info.constants.clone(),
        struct_defs,
        enum_defs,
        extern_blocks,
        const_defs,
        fn_defs,
    }
}

impl<'a> Lowering<'a> {
    // -----------------------------------------------------------------------
    // Scope / type-resolution helpers (ported from the old codegen's scope
    // stack; used only during lowering, never carried into the backend).
    // -----------------------------------------------------------------------

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn lookup_type(&self, name: &str) -> Option<BcType> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        if let Some(ci) = self.info.constants.get(name) {
            return Some(ci.ty.clone());
        }
        None
    }

    fn resolve_type(&self, ty: &ast::Type) -> BcType {
        match ty {
            ast::Type::Primitive(p, _) => match p {
                ast::PrimitiveType::I32 => BcType::I32,
                ast::PrimitiveType::I64 => BcType::I64,
                ast::PrimitiveType::F64 => BcType::F64,
                ast::PrimitiveType::Bool => BcType::Bool,
                ast::PrimitiveType::Str => BcType::Str,
                ast::PrimitiveType::Unit => BcType::Unit,
                ast::PrimitiveType::Handle => BcType::Handle,
                ast::PrimitiveType::Map => BcType::Map,
                ast::PrimitiveType::MapStrI32 => BcType::MapStrI32,
                ast::PrimitiveType::MapStrI64 => BcType::MapStrI64,
                ast::PrimitiveType::MapStrF64 => BcType::MapStrF64,
                ast::PrimitiveType::MapI32Str => BcType::MapI32Str,
                ast::PrimitiveType::MapI32I32 => BcType::MapI32I32,
            },
            ast::Type::Named(name, _) => {
                if self.info.structs.contains_key(name) {
                    BcType::Struct(name.clone())
                } else {
                    BcType::Enum(name.clone())
                }
            }
            ast::Type::FixedArray(elem, size, _) => {
                BcType::FixedArray(Box::new(self.resolve_type(elem)), *size)
            }
            ast::Type::DynamicArray(elem, _) => BcType::Array(Box::new(self.resolve_type(elem))),
            ast::Type::Result(ok, err, _) => BcType::Result(
                Box::new(self.resolve_type(ok)),
                Box::new(self.resolve_type(err)),
            ),
            ast::Type::FnPtr(params, ret, _) => {
                let param_types: Vec<BcType> =
                    params.iter().map(|p| self.resolve_type(p)).collect();
                BcType::FnPtr(param_types, Box::new(self.resolve_type(ret)))
            }
        }
    }

    /// How a call's callee resolves: a named function symbol, or an
    /// indirect call through a local/param/constant binding of `FnPtr`
    /// type. Mirrors the old codegen's `emit_call` fallback-arm check
    /// exactly (see module docs in `ir.rs` for why builtins still win by
    /// name at the backend layer regardless of this resolution).
    fn resolve_callee(&self, name: &str) -> ir::Callee {
        if let Some(ty) = self.lookup_type(name) {
            if let BcType::FnPtr(_, _) = ty {
                return ir::Callee::Var(name.to_string());
            }
        }
        ir::Callee::Named(name.to_string())
    }

    /// Parameter types for a call to `name`, when known (a direct call to a
    /// declared function, or an indirect call through a local `FnPtr`
    /// binding). Used to give each call argument its own contextual
    /// expected type — mirroring `semantic.rs`'s `check_call`, which checks
    /// each argument against its parameter's declared type — so a
    /// `Result::Ok`/`Result::Err` constructor passed directly as an
    /// argument resolves to that parameter's `Result<T, E>` type rather
    /// than the enclosing function's return type.
    fn param_types_for(&self, name: &str) -> Option<Vec<BcType>> {
        if let Some(BcType::FnPtr(params, _)) = self.lookup_type(name) {
            return Some(params);
        }
        self.info
            .functions
            .get(name)
            .map(|f| f.params.iter().map(|(_, t)| t.clone()).collect())
    }

    /// Sets the `expected_array_type`/`expected_result_type` context fields
    /// to match `ty`'s own shape (an `Array`/`FixedArray` type sets the
    /// array context, a `Result` type sets the result context, anything
    /// else clears both), returning the previous values so the caller can
    /// restore them once done lowering under this context. Mirrors
    /// `semantic.rs` passing `Some(&ty)` as `expected` into `check_expr`.
    fn push_expected_type(&mut self, ty: &BcType) -> (Option<BcType>, Option<BcType>) {
        let saved = (
            self.expected_array_type.take(),
            self.expected_result_type.take(),
        );
        match ty {
            BcType::Array(_) | BcType::FixedArray(_, _) => {
                self.expected_array_type = Some(ty.clone());
            }
            BcType::Result(_, _) => {
                self.expected_result_type = Some(ty.clone());
            }
            _ => {}
        }
        saved
    }

    fn pop_expected_type(&mut self, saved: (Option<BcType>, Option<BcType>)) {
        self.expected_array_type = saved.0;
        self.expected_result_type = saved.1;
    }

    /// The result type of calling `name`, mirroring the old codegen's
    /// `type_of(Expr::Call)` exactly (including the `len`/`push` special
    /// cases, which are never registered in `functions`).
    fn call_result_type(&self, name: &str) -> BcType {
        if name == "len" {
            return BcType::I32;
        }
        if name == "push" {
            return BcType::Unit;
        }
        if let Some(ty) = self.lookup_type(name) {
            if let BcType::FnPtr(_, ret) = ty {
                return *ret;
            }
        }
        self.info
            .functions
            .get(name)
            .map(|f| f.return_type.clone())
            .unwrap_or(BcType::Unit)
    }

    // -----------------------------------------------------------------------
    // Top-level declarations
    // -----------------------------------------------------------------------

    fn lower_struct(&self, s: &ast::StructDecl) -> ir::StructDef {
        let fields = s
            .fields
            .iter()
            .map(|f| (f.name.clone(), self.resolve_type(&f.ty)))
            .collect();
        ir::StructDef {
            name: s.name.clone(),
            fields,
            span: s.span,
        }
    }

    fn lower_enum(&self, e: &ast::EnumDecl) -> ir::EnumDef {
        let variants = e
            .variants
            .iter()
            .map(|v| {
                (
                    v.name.clone(),
                    v.payload_types
                        .iter()
                        .map(|t| self.resolve_type(t))
                        .collect(),
                )
            })
            .collect();
        ir::EnumDef {
            name: e.name.clone(),
            variants,
            span: e.span,
        }
    }

    fn lower_extern_block(&self, eb: &ast::ExternBlock) -> ir::ExternBlock {
        let decls = eb
            .decls
            .iter()
            .map(|ef| {
                let params = ef
                    .params
                    .iter()
                    .map(|p| (p.name.clone(), self.resolve_type(&p.ty)))
                    .collect();
                let return_type = ef
                    .return_type
                    .as_ref()
                    .map(|t| self.resolve_type(t))
                    .unwrap_or(BcType::Unit);
                ir::ExternFnDef {
                    name: ef.name.clone(),
                    params,
                    return_type,
                    span: ef.span,
                }
            })
            .collect();
        ir::ExternBlock {
            decls,
            span: eb.span,
        }
    }

    fn lower_const(&mut self, l: &ast::LetDecl) -> ir::ConstDef {
        let ty = self.resolve_type(&l.ty);
        let saved = self.push_expected_type(&ty);
        let value = self.lower_expr(&l.value);
        self.pop_expected_type(saved);
        ir::ConstDef {
            name: l.name.clone(),
            ty,
            value,
            span: l.span,
        }
    }

    fn lower_fn(&mut self, f: &ast::FnDecl) -> ir::FnDef {
        let return_type = f
            .return_type
            .as_ref()
            .map(|t| self.resolve_type(t))
            .unwrap_or(BcType::Unit);
        let params: Vec<(String, BcType)> = f
            .params
            .iter()
            .map(|p| (p.name.clone(), self.resolve_type(&p.ty)))
            .collect();

        let saved_ret = self.current_fn_return_type.take();
        self.current_fn_return_type = Some(return_type.clone());

        self.push_scope();
        for (name, ty) in &params {
            self.scopes
                .last_mut()
                .unwrap()
                .insert(name.clone(), ty.clone());
        }
        let body = self.lower_block(&f.body);
        self.pop_scope();

        self.current_fn_return_type = saved_ret;

        ir::FnDef {
            name: f.name.clone(),
            params,
            return_type,
            body,
            is_pure: f.is_pure,
            span: f.span,
        }
    }

    // -----------------------------------------------------------------------
    // Blocks & statements
    // -----------------------------------------------------------------------

    /// Lowers a block's statements and tail expression *in the current
    /// scope frame* — callers are responsible for pushing/popping a scope
    /// around this call wherever the AST construct they're lowering
    /// introduces one (function body, arena body, block-as-expression,
    /// while/for/for-in body, if/else branch, match arm), exactly mirroring
    /// where the old codegen called `push_scope`/`pop_scope`.
    fn lower_block(&mut self, block: &ast::Block) -> ir::Block {
        let stmts: Vec<ir::Stmt> = block.stmts.iter().map(|s| self.lower_stmt(s)).collect();
        let tail_expr = block
            .tail_expr
            .as_ref()
            .map(|e| Box::new(self.lower_expr(e)));
        ir::Block {
            stmts,
            tail_expr,
            span: block.span,
        }
    }

    fn lower_stmt(&mut self, stmt: &ast::Stmt) -> ir::Stmt {
        match stmt {
            ast::Stmt::Let(ls) => {
                let ty = self.resolve_type(&ls.ty);
                // Mirrors `semantic.rs`'s `check_expr(&ls.value, Some(&ty))`:
                // the let's own declared type is the expected type for its
                // initializer, regardless of whether that's an array type
                // (for empty-array-literal inference) or a `Result<T, E>`
                // type (for a contextual `Result::Ok`/`Result::Err` call).
                let saved = self.push_expected_type(&ty);
                let value = self.lower_expr(&ls.value);
                self.pop_expected_type(saved);
                self.scopes
                    .last_mut()
                    .unwrap()
                    .insert(ls.name.clone(), ty.clone());
                ir::Stmt::Let(ir::LetStmt {
                    name: ls.name.clone(),
                    is_mut: ls.is_mut,
                    ty,
                    value,
                    span: ls.span,
                })
            }
            ast::Stmt::Assign(a) => {
                let target_ty = self.place_final_type(&a.target);
                let saved = self.push_expected_type(&target_ty);
                let value = self.lower_expr(&a.value);
                self.pop_expected_type(saved);
                let target = self.lower_place(&a.target);
                ir::Stmt::Assign(ir::AssignStmt {
                    target,
                    value,
                    span: a.span,
                })
            }
            ast::Stmt::CompoundAssign(ca) => {
                let value = self.lower_expr(&ca.value);
                let target = self.lower_place(&ca.target);
                ir::Stmt::CompoundAssign(ir::CompoundAssignStmt {
                    target,
                    op: ca.op,
                    value,
                    span: ca.span,
                })
            }
            ast::Stmt::Expr(es) => {
                let expr = self.lower_expr(&es.expr);
                ir::Stmt::Expr(ir::ExprStmt {
                    expr,
                    span: es.span,
                })
            }
            ast::Stmt::While(w) => {
                let condition = self.lower_expr(&w.condition);
                self.push_scope();
                let body = self.lower_block(&w.body);
                self.pop_scope();
                ir::Stmt::While(ir::WhileStmt {
                    condition,
                    body,
                    span: w.span,
                })
            }
            ast::Stmt::For(f) => {
                let start = self.lower_expr(&f.start);
                let end = self.lower_expr(&f.end);
                self.push_scope();
                self.scopes
                    .last_mut()
                    .unwrap()
                    .insert(f.var.clone(), BcType::I32);
                let body = self.lower_block(&f.body);
                self.pop_scope();
                ir::Stmt::For(ir::ForStmt {
                    var: f.var.clone(),
                    start,
                    end,
                    body,
                    span: f.span,
                })
            }
            ast::Stmt::ForIn(fi) => {
                let iterable = self.lower_expr(&fi.iterable);
                let elem_ty = match iterable.ty() {
                    BcType::FixedArray(e, _) => *e,
                    BcType::Array(e) => *e,
                    _ => BcType::I32,
                };
                self.push_scope();
                self.scopes
                    .last_mut()
                    .unwrap()
                    .insert(fi.var.clone(), elem_ty);
                let body = self.lower_block(&fi.body);
                self.pop_scope();
                ir::Stmt::ForIn(ir::ForInStmt {
                    var: fi.var.clone(),
                    iterable,
                    body,
                    span: fi.span,
                })
            }
            ast::Stmt::Return(r) => {
                // Mirrors `semantic.rs`'s `check_expr(expr, Some(&fn_ret))`:
                // the enclosing function's own return type is the expected
                // type for the returned value.
                let saved = self
                    .current_fn_return_type
                    .clone()
                    .map(|ret| self.push_expected_type(&ret));
                let value = r.value.as_ref().map(|v| self.lower_expr(v));
                if let Some(saved) = saved {
                    self.pop_expected_type(saved);
                }
                ir::Stmt::Return(ir::ReturnStmt {
                    value,
                    span: r.span,
                })
            }
            ast::Stmt::Defer(d) => {
                let expr = self.lower_expr(&d.expr);
                ir::Stmt::Defer(ir::DeferStmt { expr, span: d.span })
            }
            ast::Stmt::Break(s) => ir::Stmt::Break(*s),
            ast::Stmt::Continue(s) => ir::Stmt::Continue(*s),
        }
    }

    fn lower_place(&mut self, place: &ast::Place) -> ir::Place {
        // Matches the old `emit_place`'s exact (root-variable-only) lookup:
        // this does not follow intervening `Field` accessors, so a chain
        // like `obj.arr[i]` resolves the element type of `obj`'s own type,
        // not of `obj.arr`. Preserved intentionally — see `ir.rs` docs.
        let base_ty = self.lookup_type(&place.name).unwrap_or(BcType::Unit);
        let accessors = place
            .accessors
            .iter()
            .map(|a| match a {
                ast::PlaceAccessor::Field(f) => ir::PlaceAccessor::Field(f.clone()),
                ast::PlaceAccessor::Index(idx) => ir::PlaceAccessor::Index(self.lower_expr(idx)),
            })
            .collect();
        ir::Place {
            name: place.name.clone(),
            accessors,
            base_ty,
            span: place.span,
        }
    }

    fn place_final_type(&self, place: &ast::Place) -> BcType {
        let mut ty = self.lookup_type(&place.name).unwrap_or(BcType::Unit);
        for accessor in &place.accessors {
            match accessor {
                ast::PlaceAccessor::Field(field) => {
                    ty = match &ty {
                        BcType::Struct(name) => self
                            .info
                            .structs
                            .get(name)
                            .and_then(|s| s.fields.iter().find(|(n, _)| n == field))
                            .map(|(_, field_ty)| field_ty.clone())
                            .unwrap_or(BcType::Unit),
                        _ => BcType::Unit,
                    };
                }
                ast::PlaceAccessor::Index(_) => {
                    ty = match ty {
                        BcType::Array(elem) | BcType::FixedArray(elem, _) => *elem,
                        _ => BcType::Unit,
                    };
                }
            }
        }
        ty
    }

    // -----------------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------------

    fn lower_expr(&mut self, expr: &ast::Expr) -> ir::Expr {
        match expr {
            ast::Expr::IntLit(v, s) => ir::Expr::IntLit(*v, *s),
            ast::Expr::FloatLit(v, s) => ir::Expr::FloatLit(*v, *s),
            ast::Expr::StringLit(v, s) => ir::Expr::StringLit(v.clone(), *s),
            ast::Expr::BoolLit(v, s) => ir::Expr::BoolLit(*v, *s),

            ast::Expr::InterpolatedString { parts, span } => {
                let parts = parts
                    .iter()
                    .map(|p| match p {
                        ast::InterpolatedStringPart::Text(t) => {
                            ir::InterpolatedStringPart::Text(t.clone())
                        }
                        ast::InterpolatedStringPart::Expr(e) => {
                            ir::InterpolatedStringPart::Expr(self.lower_expr(e))
                        }
                    })
                    .collect();
                ir::Expr::InterpolatedString { parts, span: *span }
            }

            ast::Expr::Ident(name, span) => {
                let (kind, ty) = if let Some(ty) = self.lookup_type(name) {
                    (ir::IdentKind::Value, ty)
                } else if let Some(fi) = self.info.functions.get(name) {
                    let param_types: Vec<BcType> =
                        fi.params.iter().map(|(_, t)| t.clone()).collect();
                    (
                        ir::IdentKind::FnRef,
                        BcType::FnPtr(param_types, Box::new(fi.return_type.clone())),
                    )
                } else {
                    (ir::IdentKind::Value, BcType::Unit)
                };
                ir::Expr::Ident {
                    name: name.clone(),
                    kind,
                    ty,
                    span: *span,
                }
            }

            ast::Expr::BinaryOp {
                op,
                left,
                right,
                span,
            } => {
                let l = self.lower_expr(left);
                let r = self.lower_expr(right);
                let ty = match op {
                    ast::BinOp::Eq
                    | ast::BinOp::Neq
                    | ast::BinOp::Lt
                    | ast::BinOp::Gt
                    | ast::BinOp::LtEq
                    | ast::BinOp::GtEq
                    | ast::BinOp::And
                    | ast::BinOp::Or => BcType::Bool,
                    _ => l.ty(),
                };
                ir::Expr::BinaryOp {
                    op: *op,
                    left: Box::new(l),
                    right: Box::new(r),
                    ty,
                    span: *span,
                }
            }

            ast::Expr::UnaryOp { op, operand, span } => {
                let o = self.lower_expr(operand);
                let ty = match op {
                    ast::UnaryOp::Not => BcType::Bool,
                    ast::UnaryOp::Neg => o.ty(),
                };
                ir::Expr::UnaryOp {
                    op: *op,
                    operand: Box::new(o),
                    ty,
                    span: *span,
                }
            }

            ast::Expr::Cast { expr, ty, span } => {
                let e = self.lower_expr(expr);
                let to_ty = self.resolve_type(ty);
                ir::Expr::Cast {
                    expr: Box::new(e),
                    to_ty,
                    span: *span,
                }
            }

            ast::Expr::Call { callee, args, span } => {
                // Semantic analysis guarantees the callee is always a plain
                // identifier for any program that reaches lowering.
                let name = match callee.as_ref() {
                    ast::Expr::Ident(n, _) => n.clone(),
                    _ => "unknown".to_string(),
                };
                // Mirrors `semantic.rs`'s `check_call`: each argument's
                // expected type is that parameter's own declared type, so a
                // `Result::Ok`/`Result::Err` (or an empty/fixed array
                // literal) passed directly as an argument resolves against
                // the parameter's type rather than any ambient context.
                let param_types = self.param_types_for(&name);
                let lowered_args: Vec<ir::Expr> = args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| match param_types.as_ref().and_then(|p| p.get(i)) {
                        Some(param_ty) => {
                            let saved = self.push_expected_type(param_ty);
                            let lowered = self.lower_expr(a);
                            self.pop_expected_type(saved);
                            lowered
                        }
                        None => self.lower_expr(a),
                    })
                    .collect();
                let callee_ir = self.resolve_callee(&name);
                let ty = self.call_result_type(&name);
                ir::Expr::Call {
                    callee: callee_ir,
                    args: lowered_args,
                    ty,
                    span: *span,
                }
            }

            ast::Expr::FieldAccess { expr, field, span } => {
                let e = self.lower_expr(expr);
                let ty = if let BcType::Struct(name) = e.ty() {
                    self.info
                        .structs
                        .get(&name)
                        .and_then(|s| s.fields.iter().find(|(n, _)| n == field))
                        .map(|(_, t)| t.clone())
                        .unwrap_or(BcType::Unit)
                } else {
                    BcType::Unit
                };
                ir::Expr::FieldAccess {
                    expr: Box::new(e),
                    field: field.clone(),
                    ty,
                    span: *span,
                }
            }

            ast::Expr::Index { expr, index, span } => {
                let e = self.lower_expr(expr);
                let idx = self.lower_expr(index);
                // Matches `semantic.rs`'s `Expr::Index` check exactly:
                // indexing a `str` yields the `i32` code point of that
                // byte, indexing an `Array`/`FixedArray` yields the
                // element type. (The old codegen's cached `type_of` only
                // resolved the array case and fell back to `Unit` for
                // strings, but that never mattered there because
                // `emit_expr`'s `Index` arm re-derived the base type
                // itself; here `.ty()` is trusted directly by callers such
                // as `BinaryOp`'s type promotion, so it must be correct.)
                let ty = match e.ty() {
                    BcType::Array(el) | BcType::FixedArray(el, _) => *el,
                    BcType::Str => BcType::I32,
                    _ => BcType::Unit,
                };
                ir::Expr::Index {
                    expr: Box::new(e),
                    index: Box::new(idx),
                    ty,
                    span: *span,
                }
            }

            ast::Expr::Block(block) => {
                self.push_scope();
                let b = self.lower_block(block);
                self.pop_scope();
                ir::Expr::Block(b)
            }

            ast::Expr::If {
                condition,
                then_block,
                else_branch,
                span,
            } => {
                let cond = self.lower_expr(condition);
                self.push_scope();
                let then_b = self.lower_block(then_block);
                self.pop_scope();
                let else_b = else_branch.as_ref().map(|e| Box::new(self.lower_expr(e)));
                let ty = if else_b.is_some() {
                    then_b.ty()
                } else {
                    BcType::Unit
                };
                ir::Expr::If {
                    condition: Box::new(cond),
                    then_block: then_b,
                    else_branch: else_b,
                    ty,
                    span: *span,
                }
            }

            ast::Expr::Match {
                scrutinee,
                arms,
                span,
            } => {
                let scrut = self.lower_expr(scrutinee);
                let scrut_ty = scrut.ty();
                let lowered_arms: Vec<ir::MatchArm> = arms
                    .iter()
                    .map(|a| self.lower_match_arm(&scrut_ty, a))
                    .collect();
                let ty = lowered_arms
                    .first()
                    .map(|a| a.body.ty())
                    .unwrap_or(BcType::Unit);
                ir::Expr::Match {
                    scrutinee: Box::new(scrut),
                    arms: lowered_arms,
                    ty,
                    span: *span,
                }
            }

            ast::Expr::Try { call, span } => {
                let c = self.lower_expr(call);
                let ty = match c.ty() {
                    BcType::Result(ok, _) => *ok,
                    _ => BcType::Unit,
                };
                ir::Expr::Try {
                    call: Box::new(c),
                    ty,
                    span: *span,
                }
            }

            ast::Expr::ArrayLit { elements, span } => {
                if elements.is_empty() {
                    // Mirrors `semantic.rs`: an *empty* literal is always
                    // `Array(elem)`, taking `elem` from the expected type
                    // whether that expected type is itself `Array` or
                    // `FixedArray` — never `FixedArray` for the empty case.
                    let elem_ty = match &self.expected_array_type {
                        Some(BcType::Array(elem)) | Some(BcType::FixedArray(elem, _)) => {
                            (**elem).clone()
                        }
                        _ => BcType::Unit,
                    };
                    let ty = BcType::Array(Box::new(elem_ty.clone()));
                    ir::Expr::ArrayLit {
                        elements: Vec::new(),
                        elem_ty,
                        ty,
                        span: *span,
                    }
                } else {
                    let lowered: Vec<ir::Expr> =
                        elements.iter().map(|e| self.lower_expr(e)).collect();
                    let elem_ty = lowered[0].ty();
                    // Mirrors `semantic.rs`: a non-empty literal becomes
                    // `FixedArray` only when the expected type is itself a
                    // `FixedArray` with a matching element type and length;
                    // otherwise (including no expected type at all) it's
                    // the dynamic `Array`.
                    let ty = match &self.expected_array_type {
                        Some(BcType::FixedArray(exp_elem, size))
                            if **exp_elem == elem_ty && *size == lowered.len() as i64 =>
                        {
                            BcType::FixedArray(Box::new(elem_ty.clone()), *size)
                        }
                        _ => BcType::Array(Box::new(elem_ty.clone())),
                    };
                    ir::Expr::ArrayLit {
                        elements: lowered,
                        elem_ty,
                        ty,
                        span: *span,
                    }
                }
            }

            ast::Expr::StructLit { name, fields, span } => {
                // Mirrors `semantic.rs`'s `Expr::StructLit` check: each
                // field's expected type is that field's own declared type
                // on the struct.
                let field_types = self.info.structs.get(name).cloned();
                let lowered_fields = fields
                    .iter()
                    .map(|fi| {
                        let field_ty = field_types.as_ref().and_then(|s| {
                            s.fields
                                .iter()
                                .find(|(n, _)| n == &fi.name)
                                .map(|(_, t)| t.clone())
                        });
                        let value = match &field_ty {
                            Some(t) => {
                                let saved = self.push_expected_type(t);
                                let v = self.lower_expr(&fi.value);
                                self.pop_expected_type(saved);
                                v
                            }
                            None => self.lower_expr(&fi.value),
                        };
                        ir::FieldInit {
                            name: fi.name.clone(),
                            value,
                            span: fi.span,
                        }
                    })
                    .collect();
                ir::Expr::StructLit {
                    name: name.clone(),
                    fields: lowered_fields,
                    span: *span,
                }
            }

            ast::Expr::EnumConstructor {
                enum_name,
                variant,
                args,
                span,
            } => {
                let payload_types = if enum_name == "Result" {
                    match self
                        .expected_result_type
                        .clone()
                        .or_else(|| self.current_fn_return_type.clone())
                    {
                        Some(BcType::Result(ok, err)) => match variant.as_str() {
                            "Ok" => vec![*ok],
                            "Err" => vec![*err],
                            _ => Vec::new(),
                        },
                        _ => Vec::new(),
                    }
                } else {
                    self.info
                        .enums
                        .get(enum_name)
                        .and_then(|info| info.variants.iter().find(|(n, _)| n == variant))
                        .map(|(_, tys)| tys.clone())
                        .unwrap_or_default()
                };
                let lowered_args: Vec<ir::Expr> = args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| match payload_types.get(i) {
                        Some(payload_ty) => {
                            let saved = self.push_expected_type(payload_ty);
                            let lowered = self.lower_expr(a);
                            self.pop_expected_type(saved);
                            lowered
                        }
                        None => self.lower_expr(a),
                    })
                    .collect();
                let ty = if enum_name == "Result" {
                    // Mirrors `semantic.rs`'s `infer_result_type`: prefer
                    // the contextual expected `Result<T, E>` type (from an
                    // enclosing `let`/`return`/call-argument/struct-field
                    // whose declared type is `Result<T, E>`), falling back
                    // to the enclosing function's own return type only
                    // when no more specific context is set. Using the
                    // enclosing function's return type unconditionally
                    // (the old bug) is wrong whenever the constructor
                    // builds a *local* `Result` value that differs from
                    // the function's own return type — e.g. a `Result`
                    // local inside a `Unit`-returning function.
                    self.expected_result_type
                        .clone()
                        .or_else(|| self.current_fn_return_type.clone())
                        .unwrap_or(BcType::Unit)
                } else {
                    BcType::Enum(enum_name.clone())
                };
                ir::Expr::EnumConstructor {
                    enum_name: enum_name.clone(),
                    variant: variant.clone(),
                    args: lowered_args,
                    ty,
                    span: *span,
                }
            }

            ast::Expr::Arena { body, span } => {
                self.push_scope();
                let b = self.lower_block(body);
                self.pop_scope();
                ir::Expr::Arena {
                    body: b,
                    span: *span,
                }
            }
        }
    }

    fn lower_match_arm(&mut self, scrut_ty: &BcType, arm: &ast::MatchArm) -> ir::MatchArm {
        self.push_scope();
        match (&arm.pattern, scrut_ty) {
            (
                ast::Pattern::Enum {
                    enum_name,
                    variant,
                    bindings,
                    ..
                },
                BcType::Enum(ename),
            ) => {
                let lookup = if enum_name.is_empty() {
                    ename.as_str()
                } else {
                    enum_name.as_str()
                };
                if let Some(info) = self.info.enums.get(lookup).cloned() {
                    if let Some((_, payload_types)) =
                        info.variants.iter().find(|(n, _)| n == variant)
                    {
                        for (i, binding) in bindings.iter().enumerate() {
                            if let ast::Pattern::Ident(name, _) = binding {
                                if i < payload_types.len() {
                                    self.scopes
                                        .last_mut()
                                        .unwrap()
                                        .insert(name.clone(), payload_types[i].clone());
                                }
                            }
                        }
                    }
                }
            }
            (
                ast::Pattern::Enum {
                    variant, bindings, ..
                },
                BcType::Result(ok_ty, err_ty),
            ) => {
                if let Some(ast::Pattern::Ident(name, _)) = bindings.first() {
                    match variant.as_str() {
                        "Ok" => {
                            self.scopes
                                .last_mut()
                                .unwrap()
                                .insert(name.clone(), (**ok_ty).clone());
                        }
                        "Err" => {
                            self.scopes
                                .last_mut()
                                .unwrap()
                                .insert(name.clone(), (**err_ty).clone());
                        }
                        _ => {}
                    }
                }
            }
            // A bare identifier pattern (`other => ...`) binds the *whole*
            // scrutinee value under that name — the catch-all arm form
            // used for e.g. `match n { 0 => ..., other => print_i32(other) }`.
            // Mirrors `semantic.rs`'s `bind_pattern_vars`'s `Pattern::Ident`
            // arm exactly (`self.add_binding(name, scrut_ty.clone(), ...)`),
            // which is why this compiled and type-checked fine already —
            // only this lowering step was missing the registration, so any
            // reference to `other` in the arm body fell through
            // `lower_expr`'s `ast::Expr::Ident` case's "not found in any
            // scope, not a function name either" fallback and silently
            // became `BcType::Unit` (see that match's final `else` arm),
            // which the native backend then represents as "no value at
            // all" and panics on first use (`Option::expect` on `None`).
            (ast::Pattern::Ident(name, _), _) => {
                self.scopes
                    .last_mut()
                    .unwrap()
                    .insert(name.clone(), scrut_ty.clone());
            }
            _ => {}
        }
        let body = self.lower_expr(&arm.body);
        self.pop_scope();
        ir::MatchArm {
            pattern: Self::lower_pattern(&arm.pattern),
            body,
            span: arm.span,
        }
    }

    fn lower_pattern(pat: &ast::Pattern) -> ir::Pattern {
        match pat {
            ast::Pattern::Wildcard(s) => ir::Pattern::Wildcard(*s),
            ast::Pattern::Ident(n, s) => ir::Pattern::Ident(n.clone(), *s),
            ast::Pattern::IntLit(v, s) => ir::Pattern::IntLit(*v, *s),
            ast::Pattern::FloatLit(v, s) => ir::Pattern::FloatLit(*v, *s),
            ast::Pattern::StringLit(v, s) => ir::Pattern::StringLit(v.clone(), *s),
            ast::Pattern::BoolLit(v, s) => ir::Pattern::BoolLit(*v, *s),
            ast::Pattern::Enum {
                enum_name,
                variant,
                bindings,
                span,
            } => ir::Pattern::Enum {
                enum_name: enum_name.clone(),
                variant: variant.clone(),
                bindings: bindings.iter().map(Self::lower_pattern).collect(),
                span: *span,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::semantic::SemanticAnalyzer;

    /// Lexes, parses, semantically analyzes, and lowers `src`, panicking with
    /// a useful message if any stage rejects the (assumed-valid) program.
    fn lower_source(src: &str) -> ir::Program {
        let mut lex = Lexer::new(src);
        let tokens = lex.tokenize().expect("lex failed");
        let mut par = Parser::new(tokens);
        let program = par.parse_program().expect("parse failed");
        let info = match SemanticAnalyzer::analyze(&program) {
            Ok(info) => info,
            Err(e) => panic!("semantic analysis failed: {}", e),
        };
        lower_program(&program, &info)
    }

    /// The statement at `idx` in `main`'s lowered body.
    fn main_stmt(prog: &ir::Program, idx: usize) -> &ir::Stmt {
        &prog
            .fn_defs
            .iter()
            .find(|f| f.name == "main")
            .expect("no 'main' fn in lowered program")
            .body
            .stmts[idx]
    }

    fn let_value(stmt: &ir::Stmt) -> &ir::Expr {
        match stmt {
            ir::Stmt::Let(ls) => &ls.value,
            _ => panic!("expected a let statement"),
        }
    }

    // --- Finding #1: string indexing must retain `i32`, not `unit` ---

    #[test]
    fn indexing_a_str_yields_i32() {
        let prog = lower_source(
            r#"
                fn! main() {
                    let s: str = "abc";
                    let c: i32 = s[0];
                }
                "#,
        );
        let value = let_value(main_stmt(&prog, 1));
        assert_eq!(value.ty(), BcType::I32);
    }

    #[test]
    fn indexing_an_array_still_yields_elem_type() {
        // Regression guard: fixing the `str` case must not disturb the
        // pre-existing `Array`/`FixedArray` element-type behavior.
        let prog = lower_source(
            r#"
                fn! main() {
                    let arr: [i32] = [1, 2, 3];
                    let x: i32 = arr[0];
                }
                "#,
        );
        let value = let_value(main_stmt(&prog, 1));
        assert_eq!(value.ty(), BcType::I32);
    }

    // --- Finding #2: `Result::Ok`/`Err` must retain their *contextual*
    // `Result<T, E>` type, not the enclosing function's return type ---

    #[test]
    fn result_ok_local_inside_unit_function_keeps_local_result_type() {
        // `main` returns `unit`, but `r`'s declared type is `Result<i32,
        // str>` — the constructor must resolve to that local type, not
        // `unit` (the old bug: it always used `current_fn_return_type`).
        let prog = lower_source(
            r#"
                fn! main() {
                    let r: Result<i32, str> = Result::Ok(5);
                }
                "#,
        );
        let value = let_value(main_stmt(&prog, 0));
        assert_eq!(
            value.ty(),
            BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str))
        );
    }

    #[test]
    fn result_err_local_inside_unit_function_keeps_local_result_type() {
        let prog = lower_source(
            r#"
                fn! main() {
                    let r: Result<i32, str> = Result::Err("bad");
                }
                "#,
        );
        let value = let_value(main_stmt(&prog, 0));
        assert_eq!(
            value.ty(),
            BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str))
        );
    }

    #[test]
    fn result_ok_in_return_position_still_uses_function_return_type() {
        // Non-regression: an early `return Result::Err(...)` (the common
        // case, already correct before this fix) must still resolve from
        // the enclosing function's own return type.
        let prog = lower_source(
            r#"
                fn f(x: i32) -> Result<i32, str> {
                    if x < 0 {
                        return Result::Err("negative");
                    };
                    Result::Ok(x)
                }
                fn! main() {
                    let r: Result<i32, str> = f(1);
                }
                "#,
        );
        let f_def = prog
            .fn_defs
            .iter()
            .find(|f| f.name == "f")
            .expect("no 'f' fn in lowered program");
        match &f_def.body.stmts[0] {
            ir::Stmt::Expr(es) => match &es.expr {
                ir::Expr::If { then_block, .. } => match &then_block.stmts[0] {
                    ir::Stmt::Return(r) => {
                        let value = r.value.as_ref().expect("return has a value");
                        assert_eq!(
                            value.ty(),
                            BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str))
                        );
                    }
                    _ => panic!("expected a return statement"),
                },
                _ => panic!("expected an if expression"),
            },
            _ => panic!("expected an if statement"),
        }
    }

    #[test]
    fn result_ok_as_call_argument_keeps_parameter_result_type() {
        // The contextual type must also flow into call arguments (per the
        // callee's own declared parameter type), not just `let`/`return`.
        let prog = lower_source(
            r#"
                fn takes(r: Result<i32, str>) -> i32 {
                    0
                }
                fn! main() {
                    let x: i32 = takes(Result::Ok(7));
                }
                "#,
        );
        let value = let_value(main_stmt(&prog, 0));
        match value {
            ir::Expr::Call { args, .. } => {
                assert_eq!(
                    args[0].ty(),
                    BcType::Result(Box::new(BcType::I32), Box::new(BcType::Str))
                );
            }
            _ => panic!("expected a call expression"),
        }
    }

    // --- Finding #3: `ArrayLit` must retain the full resolved type,
    // including `FixedArray`, not only the dynamic `Array` ---

    #[test]
    fn fixed_array_literal_retains_fixed_array_type() {
        let prog = lower_source(
            r#"
                fn! main() {
                    let a: [i32; 3] = [1, 2, 3];
                }
                "#,
        );
        let value = let_value(main_stmt(&prog, 0));
        assert_eq!(value.ty(), BcType::FixedArray(Box::new(BcType::I32), 3));
    }

    #[test]
    fn dynamic_array_literal_still_retains_dynamic_array_type() {
        // Non-regression: a `[T]`-typed let must still infer the dynamic
        // `Array`, not spuriously become `FixedArray`.
        let prog = lower_source(
            r#"
                fn! main() {
                    let a: [i32] = [1, 2, 3];
                }
                "#,
        );
        let value = let_value(main_stmt(&prog, 0));
        assert_eq!(value.ty(), BcType::Array(Box::new(BcType::I32)));
    }

    #[test]
    fn empty_array_literal_in_fixed_array_context_infers_dynamic_array_elem_type() {
        // Non-regression: mirrors `semantic.rs` exactly — an *empty*
        // literal always infers the dynamic `Array`, even when the
        // expected/declared type is `FixedArray`. (A `[i32; 3] = []` would
        // actually be rejected by semantic analysis as a size mismatch, so
        // this is only reachable via a dynamic `[i32]` declared type.)
        let prog = lower_source(
            r#"
                fn! main() {
                    let a: [i32] = [];
                }
                "#,
        );
        let value = let_value(main_stmt(&prog, 0));
        assert_eq!(value.ty(), BcType::Array(Box::new(BcType::I32)));
    }
}
