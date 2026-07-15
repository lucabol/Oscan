#![allow(dead_code)]
//! Backend-neutral typed IR (Intermediate Representation).
//!
//! This is produced once, by [`crate::lower::lower_program`], from the
//! semantically-checked AST (`crate::ast`) plus the [`SemanticInfo`] computed
//! by `crate::semantic`. It is consumed by every code-generation backend —
//! today that is only the C backend in `crate::codegen`, but the whole point
//! of this module is that a future backend (e.g. a Cranelift backend) can
//! consume the same tree without re-implementing type inference, call
//! resolution, or symbol-table bookkeeping.
//!
//! Design invariants that make this a real shared layer rather than a
//! cosmetic AST rename:
//!
//! * **Types are fully resolved.** Every expression node carries its own
//!   resolved [`BcType`] (attached once during lowering). Backends never
//!   need to walk scopes or re-derive "what type is this expression" — they
//!   just read `.ty()`.
//! * **Spans are preserved** on every statement and expression so any
//!   backend can produce source-accurate diagnostics.
//! * **Calls are pre-resolved.** [`Callee`] distinguishes a direct call to a
//!   named function (builtin/user/extern — a backend concern how each is
//!   implemented) from an indirect call through a function-pointer-typed
//!   variable. This resolution used to happen ad hoc, per emission, inside
//!   the C backend by re-walking scope tables; now it happens once.
//! * **Defer and arena semantics are explicit control-flow constructs**
//!   (`Stmt::Defer`, `Expr::Arena`) rather than being reconstructed from
//!   raw AST during code generation.
//! * **Aggregates (struct/enum/array literals) carry their resolved type**,
//!   including the element type of empty array literals (resolved from
//!   context during lowering), so backends never need a fallback/mutable
//!   "expected type" side channel.
//!
//! Control flow remains *structured* (blocks / if / while / for / match /
//! break / continue / return) rather than a basic-block CFG: this mirrors
//! Oscan's structured surface syntax and is exactly what the C backend wants
//! to emit directly. A future SSA/CFG-based backend can lower this
//! structured IR into its own basic-block form as part of its own backend
//! pipeline; that lowering is out of scope here.

use std::collections::HashMap;

use crate::ast::BinOp;
use crate::ast::UnaryOp;
use crate::token::Span;
pub use crate::types::{BcType, ConstInfo, EnumInfo, FunctionInfo, StructInfo};

// ---------------------------------------------------------------------------
// Program
// ---------------------------------------------------------------------------

/// A fully lowered, typed program ready for backend consumption.
pub struct Program {
    // Flat symbol tables (same shape as `SemanticInfo`), kept for backends
    // that need name -> signature lookups (e.g. deciding the C arena-passing
    // convention for a call, which depends on `FunctionInfo::is_extern`).
    pub structs: HashMap<String, StructInfo>,
    pub enums: HashMap<String, EnumInfo>,
    pub functions: HashMap<String, FunctionInfo>,
    pub constants: HashMap<String, ConstInfo>,

    // Declarations, in source order within each category.
    pub struct_defs: Vec<StructDef>,
    pub enum_defs: Vec<EnumDef>,
    pub extern_blocks: Vec<ExternBlock>,
    pub const_defs: Vec<ConstDef>,
    pub fn_defs: Vec<FnDef>,
}

pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, BcType)>,
    pub span: Span,
}

pub struct EnumDef {
    pub name: String,
    pub variants: Vec<(String, Vec<BcType>)>,
    pub span: Span,
}

pub struct ExternBlock {
    pub decls: Vec<ExternFnDef>,
    pub span: Span,
}

pub struct ExternFnDef {
    pub name: String,
    pub params: Vec<(String, BcType)>,
    /// `BcType::Unit` when the source declared no return type.
    pub return_type: BcType,
    pub span: Span,
}

pub struct ConstDef {
    pub name: String,
    pub ty: BcType,
    pub value: Expr,
    pub span: Span,
}

pub struct FnDef {
    pub name: String,
    pub params: Vec<(String, BcType)>,
    /// `BcType::Unit` when the source declared no return type.
    pub return_type: BcType,
    pub body: Block,
    pub is_pure: bool,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Blocks & statements
// ---------------------------------------------------------------------------

pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail_expr: Option<Box<Expr>>,
    pub span: Span,
}

impl Block {
    /// The block's resolved type: its tail expression's type, or `Unit`.
    pub fn ty(&self) -> BcType {
        match &self.tail_expr {
            Some(e) => e.ty().clone(),
            None => BcType::Unit,
        }
    }
}

pub enum Stmt {
    Let(LetStmt),
    Assign(AssignStmt),
    CompoundAssign(CompoundAssignStmt),
    Expr(ExprStmt),
    While(WhileStmt),
    For(ForStmt),
    ForIn(ForInStmt),
    Return(ReturnStmt),
    Defer(DeferStmt),
    Break(Span),
    Continue(Span),
}

pub struct LetStmt {
    pub name: String,
    pub is_mut: bool,
    pub ty: BcType,
    pub value: Expr,
    pub span: Span,
}

pub struct AssignStmt {
    pub target: Place,
    pub value: Expr,
    pub span: Span,
}

pub struct CompoundAssignStmt {
    pub target: Place,
    pub op: BinOp,
    pub value: Expr,
    pub span: Span,
}

/// Place expression (lvalue). `base_ty` is the resolved type of the named
/// root variable (matching the previous codegen's `lookup_type(place.name)`
/// behavior exactly, including for chained accessors — see `lower.rs`).
pub struct Place {
    pub name: String,
    pub accessors: Vec<PlaceAccessor>,
    pub base_ty: BcType,
    pub span: Span,
}

pub enum PlaceAccessor {
    Field(String),
    Index(Expr),
}

pub struct ExprStmt {
    pub expr: Expr,
    pub span: Span,
}

pub struct WhileStmt {
    pub condition: Expr,
    pub body: Block,
    pub span: Span,
}

pub struct ForStmt {
    pub var: String,
    pub start: Expr,
    pub end: Expr,
    pub body: Block,
    pub span: Span,
}

pub struct ForInStmt {
    pub var: String,
    pub iterable: Expr,
    pub body: Block,
    pub span: Span,
}

pub struct ReturnStmt {
    pub value: Option<Expr>,
    pub span: Span,
}

pub struct DeferStmt {
    pub expr: Expr,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// How an identifier reference resolved during lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentKind {
    /// A local variable, parameter, or top-level constant: emitted as a
    /// plain identifier.
    Value,
    /// A function name used as a value (i.e. a function pointer): emitted
    /// as the mangled C function name.
    FnRef,
}

/// How a call's callee resolved during lowering. Semantic analysis
/// guarantees a call's callee syntax is always a plain identifier, so this
/// is the only decision a backend needs: is it a named function symbol, or
/// an indirect call through a function-pointer-typed binding?
pub enum Callee {
    /// Direct call to a named function symbol — a runtime builtin, a
    /// user-defined `fn`/`fn!`, or an `extern` function. Backends still
    /// decide *how* to emit each name (builtins have hand-written call
    /// sequences; user/extern functions use the mangled-name + calling
    /// convention).
    Named(String),
    /// Indirect call through a local variable, parameter, or top-level
    /// constant holding a function-pointer value.
    Var(String),
}

pub enum Expr {
    IntLit(i64, Span),
    FloatLit(f64, Span),
    StringLit(String, Span),
    InterpolatedString {
        parts: Vec<InterpolatedStringPart>,
        span: Span,
    },
    BoolLit(bool, Span),
    Ident {
        name: String,
        kind: IdentKind,
        ty: BcType,
        span: Span,
    },
    BinaryOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        /// Result type (Bool for comparisons/logic; operand type otherwise).
        ty: BcType,
        span: Span,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
        ty: BcType,
        span: Span,
    },
    Cast {
        expr: Box<Expr>,
        to_ty: BcType,
        span: Span,
    },
    Call {
        callee: Callee,
        args: Vec<Expr>,
        ty: BcType,
        span: Span,
    },
    FieldAccess {
        expr: Box<Expr>,
        field: String,
        ty: BcType,
        span: Span,
    },
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
        ty: BcType,
        span: Span,
    },
    Block(Block),
    If {
        condition: Box<Expr>,
        then_block: Block,
        else_branch: Option<Box<Expr>>,
        ty: BcType,
        span: Span,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        ty: BcType,
        span: Span,
    },
    Try {
        call: Box<Expr>,
        ty: BcType,
        span: Span,
    },
    ArrayLit {
        elements: Vec<Expr>,
        elem_ty: BcType,
        /// The literal's own fully resolved static type: `Array(elem_ty)`
        /// for a dynamic array, or `FixedArray(elem_ty, size)` when the
        /// literal is non-empty and bound to a matching fixed-size array
        /// context (see `lower.rs`'s `ArrayLit` lowering for the exact
        /// empty/non-empty inference rule, mirrored from `semantic.rs`).
        /// Kept alongside `elem_ty` (rather than replacing it) because the
        /// C backend's array-literal codegen only ever needs the element
        /// type — both `Array` and `FixedArray` literals construct an
        /// identical runtime value.
        ty: BcType,
        span: Span,
    },
    StructLit {
        name: String,
        fields: Vec<FieldInit>,
        span: Span,
    },
    EnumConstructor {
        enum_name: String,
        variant: String,
        args: Vec<Expr>,
        ty: BcType,
        span: Span,
    },
    Arena {
        body: Block,
        span: Span,
    },
}

impl Expr {
    /// The expression's fully resolved type.
    pub fn ty(&self) -> BcType {
        match self {
            Expr::IntLit(..) => BcType::I32,
            Expr::FloatLit(..) => BcType::F64,
            Expr::StringLit(..) => BcType::Str,
            Expr::InterpolatedString { .. } => BcType::Str,
            Expr::BoolLit(..) => BcType::Bool,
            Expr::Ident { ty, .. } => ty.clone(),
            Expr::BinaryOp { ty, .. } => ty.clone(),
            Expr::UnaryOp { ty, .. } => ty.clone(),
            Expr::Cast { to_ty, .. } => to_ty.clone(),
            Expr::Call { ty, .. } => ty.clone(),
            Expr::FieldAccess { ty, .. } => ty.clone(),
            Expr::Index { ty, .. } => ty.clone(),
            Expr::Block(block) => block.ty(),
            Expr::If { ty, .. } => ty.clone(),
            Expr::Match { ty, .. } => ty.clone(),
            Expr::Try { ty, .. } => ty.clone(),
            Expr::ArrayLit { ty, .. } => ty.clone(),
            Expr::StructLit { name, .. } => BcType::Struct(name.clone()),
            Expr::EnumConstructor { ty, .. } => ty.clone(),
            Expr::Arena { body, .. } => body.ty(),
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Expr::IntLit(_, s)
            | Expr::FloatLit(_, s)
            | Expr::StringLit(_, s)
            | Expr::BoolLit(_, s) => *s,
            Expr::InterpolatedString { span, .. }
            | Expr::Ident { span, .. }
            | Expr::BinaryOp { span, .. }
            | Expr::UnaryOp { span, .. }
            | Expr::Cast { span, .. }
            | Expr::Call { span, .. }
            | Expr::FieldAccess { span, .. }
            | Expr::Index { span, .. }
            | Expr::If { span, .. }
            | Expr::Match { span, .. }
            | Expr::Try { span, .. }
            | Expr::ArrayLit { span, .. }
            | Expr::StructLit { span, .. }
            | Expr::EnumConstructor { span, .. }
            | Expr::Arena { span, .. } => *span,
            Expr::Block(block) => block.span,
        }
    }
}

pub enum InterpolatedStringPart {
    Text(String),
    Expr(Expr),
}

pub struct FieldInit {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}

pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub span: Span,
}

/// Match patterns are structurally identical to the AST's; they carry no
/// type of their own; typing of any bindings they introduce is resolved
/// during lowering and baked into the arm body's sub-expressions.
pub enum Pattern {
    Wildcard(Span),
    Ident(String, Span),
    IntLit(i64, Span),
    FloatLit(f64, Span),
    StringLit(String, Span),
    BoolLit(bool, Span),
    Enum {
        enum_name: String,
        variant: String,
        bindings: Vec<Pattern>,
        span: Span,
    },
}

// ---------------------------------------------------------------------------
// Verifier
// ---------------------------------------------------------------------------

/// Structural, backend-neutral invariants checked once on the lowered IR.
///
/// This intentionally does **not** re-implement full semantic analysis
/// (`crate::semantic` already rejects ill-typed programs before lowering
/// ever runs). Its job is to catch *lowering bugs* — cases where the IR
/// itself is inconsistent — independent of any single backend:
///
/// * `break`/`continue` only ever appear inside a loop body.
/// * Every `Callee::Named` call site with a known function signature passes
///   the right number of arguments.
/// * Every `BcType::Struct`/`BcType::Enum` reachable from the program
///   refers to a struct/enum that is actually defined — including types
///   reachable only through an enum variant's payload or an `extern`
///   function's signature, not just function/struct/const declarations.
/// * Every node's own cached `.ty()` is *structurally consistent* with its
///   children — e.g. a comparison/logical `BinaryOp`'s type is `Bool`, an
///   arithmetic one matches its operand type; `Try`'s type is the `Ok`
///   payload of the awaited call's `Result` type; an `If` with no `else`
///   has `Unit` type and one with an `else` has matching branch types; an
///   `ArrayLit`'s type agrees with its own `elem_ty`/element count.
///
/// A failure here indicates an internal compiler error in `lower.rs`, not a
/// problem with the user's source program (that would have already been
/// rejected by semantic analysis).
pub fn verify(program: &Program) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    for def in &program.fn_defs {
        verify_type(program, &def.return_type, &mut errors, &def.name);
        for (_, ty) in &def.params {
            verify_type(program, ty, &mut errors, &def.name);
        }
        verify_block(program, &def.body, 0, &mut errors, &def.name);
    }
    for def in &program.struct_defs {
        for (_, ty) in &def.fields {
            verify_type(program, ty, &mut errors, &def.name);
        }
    }
    for def in &program.enum_defs {
        for (_, payload_types) in &def.variants {
            for ty in payload_types {
                verify_type(program, ty, &mut errors, &def.name);
            }
        }
    }
    for block in &program.extern_blocks {
        for def in &block.decls {
            for (_, ty) in &def.params {
                verify_type(program, ty, &mut errors, &def.name);
            }
            verify_type(program, &def.return_type, &mut errors, &def.name);
        }
    }
    for def in &program.const_defs {
        verify_type(program, &def.ty, &mut errors, &def.name);
        verify_expr(program, &def.value, 0, &mut errors, &def.name);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn verify_type(program: &Program, ty: &BcType, errors: &mut Vec<String>, ctx: &str) {
    match ty {
        BcType::Struct(name) => {
            if !program.structs.contains_key(name) {
                errors.push(format!(
                    "internal error in '{}': reference to undefined struct '{}'",
                    ctx, name
                ));
            }
        }
        BcType::Enum(name) => {
            if !program.enums.contains_key(name) {
                errors.push(format!(
                    "internal error in '{}': reference to undefined enum '{}'",
                    ctx, name
                ));
            }
        }
        BcType::Array(e) | BcType::FixedArray(e, _) => verify_type(program, e, errors, ctx),
        BcType::Result(ok, err) => {
            verify_type(program, ok, errors, ctx);
            verify_type(program, err, errors, ctx);
        }
        BcType::FnPtr(params, ret) => {
            for p in params {
                verify_type(program, p, errors, ctx);
            }
            verify_type(program, ret, errors, ctx);
        }
        _ => {}
    }
}

fn verify_block(
    program: &Program,
    block: &Block,
    loop_depth: usize,
    errors: &mut Vec<String>,
    ctx: &str,
) {
    for stmt in &block.stmts {
        verify_stmt(program, stmt, loop_depth, errors, ctx);
    }
    if let Some(tail) = &block.tail_expr {
        verify_expr(program, tail, loop_depth, errors, ctx);
    }
}

fn verify_stmt(
    program: &Program,
    stmt: &Stmt,
    loop_depth: usize,
    errors: &mut Vec<String>,
    ctx: &str,
) {
    match stmt {
        Stmt::Let(ls) => {
            verify_type(program, &ls.ty, errors, ctx);
            verify_expr(program, &ls.value, loop_depth, errors, ctx);
        }
        Stmt::Assign(a) => {
            verify_place(program, &a.target, loop_depth, errors, ctx);
            verify_expr(program, &a.value, loop_depth, errors, ctx);
        }
        Stmt::CompoundAssign(ca) => {
            verify_place(program, &ca.target, loop_depth, errors, ctx);
            verify_expr(program, &ca.value, loop_depth, errors, ctx);
        }
        Stmt::Expr(es) => verify_expr(program, &es.expr, loop_depth, errors, ctx),
        Stmt::While(w) => {
            verify_expr(program, &w.condition, loop_depth, errors, ctx);
            verify_block(program, &w.body, loop_depth + 1, errors, ctx);
        }
        Stmt::For(f) => {
            verify_expr(program, &f.start, loop_depth, errors, ctx);
            verify_expr(program, &f.end, loop_depth, errors, ctx);
            verify_block(program, &f.body, loop_depth + 1, errors, ctx);
        }
        Stmt::ForIn(fi) => {
            verify_expr(program, &fi.iterable, loop_depth, errors, ctx);
            verify_block(program, &fi.body, loop_depth + 1, errors, ctx);
        }
        Stmt::Return(r) => {
            if let Some(v) = &r.value {
                verify_expr(program, v, loop_depth, errors, ctx);
            }
        }
        Stmt::Defer(d) => verify_expr(program, &d.expr, loop_depth, errors, ctx),
        Stmt::Break(_) => {
            if loop_depth == 0 {
                errors.push(format!(
                    "internal error in '{}': 'break' outside of a loop reached lowering",
                    ctx
                ));
            }
        }
        Stmt::Continue(_) => {
            if loop_depth == 0 {
                errors.push(format!(
                    "internal error in '{}': 'continue' outside of a loop reached lowering",
                    ctx
                ));
            }
        }
    }
}

fn verify_place(
    program: &Program,
    place: &Place,
    loop_depth: usize,
    errors: &mut Vec<String>,
    ctx: &str,
) {
    verify_type(program, &place.base_ty, errors, ctx);
    for acc in &place.accessors {
        if let PlaceAccessor::Index(idx) = acc {
            verify_expr(program, idx, loop_depth, errors, ctx);
        }
    }
}

fn verify_expr(
    program: &Program,
    expr: &Expr,
    loop_depth: usize,
    errors: &mut Vec<String>,
    ctx: &str,
) {
    verify_type(program, &expr.ty(), errors, ctx);
    match expr {
        Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::StringLit(..)
        | Expr::BoolLit(..)
        | Expr::Ident { .. } => {}
        Expr::InterpolatedString { parts, .. } => {
            for part in parts {
                if let InterpolatedStringPart::Expr(e) = part {
                    verify_expr(program, e, loop_depth, errors, ctx);
                }
            }
        }
        Expr::BinaryOp {
            op,
            left,
            right,
            ty,
            ..
        } => {
            verify_expr(program, left, loop_depth, errors, ctx);
            verify_expr(program, right, loop_depth, errors, ctx);
            let expected_ty = match op {
                BinOp::Eq
                | BinOp::Neq
                | BinOp::Lt
                | BinOp::Gt
                | BinOp::LtEq
                | BinOp::GtEq
                | BinOp::And
                | BinOp::Or => BcType::Bool,
                _ => left.ty(),
            };
            if *ty != expected_ty {
                errors.push(format!(
                    "internal error in '{}': binary op '{:?}' cached type {} does not match expected {}",
                    ctx, op, ty, expected_ty
                ));
            }
        }
        Expr::UnaryOp {
            op, operand, ty, ..
        } => {
            verify_expr(program, operand, loop_depth, errors, ctx);
            let expected_ty = match op {
                UnaryOp::Not => BcType::Bool,
                UnaryOp::Neg => operand.ty(),
            };
            if *ty != expected_ty {
                errors.push(format!(
                    "internal error in '{}': unary op '{:?}' cached type {} does not match expected {}",
                    ctx, op, ty, expected_ty
                ));
            }
        }
        Expr::Cast { expr, .. } => verify_expr(program, expr, loop_depth, errors, ctx),
        Expr::Call { callee, args, .. } => {
            if let Callee::Named(name) = callee {
                if let Some(fi) = program.functions.get(name) {
                    if fi.params.len() != args.len() {
                        errors.push(format!(
                            "internal error in '{}': call to '{}' passes {} argument(s), expected {}",
                            ctx,
                            name,
                            args.len(),
                            fi.params.len()
                        ));
                    }
                }
            }
            for arg in args {
                verify_expr(program, arg, loop_depth, errors, ctx);
            }
        }
        Expr::FieldAccess { expr, .. } => verify_expr(program, expr, loop_depth, errors, ctx),
        Expr::Index { expr, index, .. } => {
            verify_expr(program, expr, loop_depth, errors, ctx);
            verify_expr(program, index, loop_depth, errors, ctx);
        }
        Expr::Block(block) => verify_block(program, block, loop_depth, errors, ctx),
        Expr::If {
            condition,
            then_block,
            else_branch,
            ty,
            ..
        } => {
            verify_expr(program, condition, loop_depth, errors, ctx);
            verify_block(program, then_block, loop_depth, errors, ctx);
            match else_branch {
                Some(eb) => {
                    verify_expr(program, eb, loop_depth, errors, ctx);
                    let then_ty = then_block.ty();
                    let else_ty = eb.ty();
                    if then_ty != else_ty {
                        errors.push(format!(
                            "internal error in '{}': if/else branch type mismatch: {} vs {}",
                            ctx, then_ty, else_ty
                        ));
                    } else if *ty != then_ty {
                        errors.push(format!(
                            "internal error in '{}': if cached type {} does not match branch type {}",
                            ctx, ty, then_ty
                        ));
                    }
                }
                None => {
                    if *ty != BcType::Unit {
                        errors.push(format!(
                            "internal error in '{}': if without else has cached type {}, expected unit",
                            ctx, ty
                        ));
                    }
                }
            }
        }
        Expr::Match {
            scrutinee,
            arms,
            ty,
            ..
        } => {
            verify_expr(program, scrutinee, loop_depth, errors, ctx);
            for arm in arms {
                verify_expr(program, &arm.body, loop_depth, errors, ctx);
                let arm_ty = arm.body.ty();
                if arm_ty != *ty {
                    errors.push(format!(
                        "internal error in '{}': match arm cached type {} does not match match's cached type {}",
                        ctx, arm_ty, ty
                    ));
                }
            }
        }
        Expr::Try { call, ty, .. } => {
            verify_expr(program, call, loop_depth, errors, ctx);
            match call.ty() {
                BcType::Result(ok, _) => {
                    if *ty != *ok {
                        errors.push(format!(
                            "internal error in '{}': try cached type {} does not match Ok payload {}",
                            ctx, ty, ok
                        ));
                    }
                }
                other => errors.push(format!(
                    "internal error in '{}': try applied to non-Result cached type {}",
                    ctx, other
                )),
            }
        }
        Expr::ArrayLit {
            elements,
            elem_ty,
            ty,
            ..
        } => {
            for e in elements {
                verify_expr(program, e, loop_depth, errors, ctx);
            }
            if let Some(first) = elements.first() {
                let first_ty = first.ty();
                if first_ty != *elem_ty {
                    errors.push(format!(
                        "internal error in '{}': array literal elem_ty {} does not match first element's type {}",
                        ctx, elem_ty, first_ty
                    ));
                }
            }
            match ty {
                BcType::Array(inner) => {
                    if **inner != *elem_ty {
                        errors.push(format!(
                            "internal error in '{}': array literal cached type [{}] does not match elem_ty {}",
                            ctx, inner, elem_ty
                        ));
                    }
                }
                BcType::FixedArray(inner, size) => {
                    if **inner != *elem_ty {
                        errors.push(format!(
                            "internal error in '{}': array literal cached type [{}; {}] does not match elem_ty {}",
                            ctx, inner, size, elem_ty
                        ));
                    }
                    if *size != elements.len() as i64 {
                        errors.push(format!(
                            "internal error in '{}': fixed array literal cached size {} does not match element count {}",
                            ctx, size, elements.len()
                        ));
                    }
                }
                other => errors.push(format!(
                    "internal error in '{}': array literal has non-array cached type {}",
                    ctx, other
                )),
            }
        }
        Expr::StructLit { fields, .. } => {
            for fi in fields {
                verify_expr(program, &fi.value, loop_depth, errors, ctx);
            }
        }
        Expr::EnumConstructor { args, .. } => {
            for arg in args {
                verify_expr(program, arg, loop_depth, errors, ctx);
            }
        }
        Expr::Arena { body, .. } => verify_block(program, body, loop_depth, errors, ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_program() -> Program {
        Program {
            structs: HashMap::new(),
            enums: HashMap::new(),
            functions: HashMap::new(),
            constants: HashMap::new(),
            struct_defs: Vec::new(),
            enum_defs: Vec::new(),
            extern_blocks: Vec::new(),
            const_defs: Vec::new(),
            fn_defs: Vec::new(),
        }
    }

    fn errs(program: &Program) -> Vec<String> {
        match verify(program) {
            Ok(()) => panic!("expected verify() to reject this program"),
            Err(errors) => errors,
        }
    }

    // --- Finding #4: verifier must traverse enum payload / extern
    // signature types, not just fn/struct/const declarations ---

    #[test]
    fn verify_traverses_enum_variant_payload_types() {
        let mut program = empty_program();
        let span = Span::new(1, 1);
        program.enum_defs.push(EnumDef {
            name: "MyEnum".to_string(),
            variants: vec![(
                "V".to_string(),
                vec![BcType::Struct("MissingPayload".to_string())],
            )],
            span,
        });
        let errors = errs(&program);
        assert!(
            errors.iter().any(|e| e.contains("MissingPayload")),
            "expected an undefined-struct error, got: {:?}",
            errors
        );
    }

    #[test]
    fn verify_traverses_extern_fn_param_and_return_types() {
        let mut program = empty_program();
        let span = Span::new(1, 1);
        program.extern_blocks.push(ExternBlock {
            decls: vec![ExternFnDef {
                name: "foo".to_string(),
                params: vec![("x".to_string(), BcType::Struct("MissingParam".to_string()))],
                return_type: BcType::Struct("MissingReturn".to_string()),
                span,
            }],
            span,
        });
        let errors = errs(&program);
        assert!(
            errors.iter().any(|e| e.contains("MissingParam")),
            "expected an undefined param-type error, got: {:?}",
            errors
        );
        assert!(
            errors.iter().any(|e| e.contains("MissingReturn")),
            "expected an undefined return-type error, got: {:?}",
            errors
        );
    }

    // --- Finding #4: structural cached-type consistency ---

    fn fn_with_body(stmts: Vec<Stmt>) -> FnDef {
        let span = Span::new(1, 1);
        FnDef {
            name: "main".to_string(),
            params: Vec::new(),
            return_type: BcType::Unit,
            body: Block {
                stmts,
                tail_expr: None,
                span,
            },
            is_pure: false,
            span,
        }
    }

    #[test]
    fn verify_rejects_binary_op_with_inconsistent_cached_type() {
        let mut program = empty_program();
        let span = Span::new(1, 1);
        // `Add` should retain the operand type (`i32` here), not `bool`.
        let bad_add = Expr::BinaryOp {
            op: BinOp::Add,
            left: Box::new(Expr::IntLit(1, span)),
            right: Box::new(Expr::IntLit(2, span)),
            ty: BcType::Bool,
            span,
        };
        program.fn_defs.push(fn_with_body(vec![Stmt::Expr(ExprStmt {
            expr: bad_add,
            span,
        })]));
        let errors = errs(&program);
        assert!(
            errors.iter().any(|e| e.contains("binary op")),
            "expected a binary-op consistency error, got: {:?}",
            errors
        );
    }

    #[test]
    fn verify_rejects_if_without_else_typed_as_non_unit() {
        let span = Span::new(1, 1);
        let mut program = empty_program();
        let bad_if = Expr::If {
            condition: Box::new(Expr::BoolLit(true, span)),
            then_block: Block {
                stmts: Vec::new(),
                tail_expr: Some(Box::new(Expr::IntLit(1, span))),
                span,
            },
            else_branch: None,
            ty: BcType::I32,
            span,
        };
        program.fn_defs.push(fn_with_body(vec![Stmt::Expr(ExprStmt {
            expr: bad_if,
            span,
        })]));
        let errors = errs(&program);
        assert!(
            errors.iter().any(|e| e.contains("if without else")),
            "expected an if-without-else consistency error, got: {:?}",
            errors
        );
    }

    #[test]
    fn verify_rejects_fixed_array_literal_with_wrong_cached_size() {
        let span = Span::new(1, 1);
        let mut program = empty_program();
        let bad_array = Expr::ArrayLit {
            elements: vec![Expr::IntLit(1, span), Expr::IntLit(2, span)],
            elem_ty: BcType::I32,
            // Only 2 elements, but the cached type claims a fixed size of 3.
            ty: BcType::FixedArray(Box::new(BcType::I32), 3),
            span,
        };
        program.fn_defs.push(fn_with_body(vec![Stmt::Expr(ExprStmt {
            expr: bad_array,
            span,
        })]));
        let errors = errs(&program);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("fixed array literal cached size")),
            "expected a fixed-array-size consistency error, got: {:?}",
            errors
        );
    }
}
