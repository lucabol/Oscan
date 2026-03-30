#![allow(dead_code)]

use crate::token::Span;

/// A complete Oscan program.
#[derive(Debug)]
pub struct Program {
    pub decls: Vec<TopDecl>,
}

/// Top-level declaration.
#[derive(Debug)]
pub enum TopDecl {
    Fn(FnDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Let(LetDecl),
    Extern(ExternBlock),
}

/// Function declaration (both `fn` and `fn!`).
#[derive(Debug)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Block,
    pub is_pure: bool, // true = fn, false = fn!
    pub span: Span,
}

/// Function parameter.
#[derive(Debug)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

/// Struct declaration.
#[derive(Debug)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<Field>,
    pub span: Span,
}

/// Struct field.
#[derive(Debug)]
pub struct Field {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

/// Enum declaration.
#[derive(Debug)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<Variant>,
    pub span: Span,
}

/// Enum variant with optional payload types.
#[derive(Debug)]
pub struct Variant {
    pub name: String,
    pub payload_types: Vec<Type>,
    pub span: Span,
}

/// Top-level let declaration (constant).
#[derive(Debug)]
pub struct LetDecl {
    pub name: String,
    pub ty: Type,
    pub value: Expr,
    pub span: Span,
}

/// Extern block containing C-FFI function declarations.
#[derive(Debug)]
pub struct ExternBlock {
    pub decls: Vec<ExternFnDecl>,
    pub span: Span,
}

/// Extern function declaration.
#[derive(Debug)]
pub struct ExternFnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub span: Span,
}

/// A block: `{ stmt* expr? }`
#[derive(Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail_expr: Option<Box<Expr>>,
    pub span: Span,
}

/// Statement types.
#[derive(Debug)]
pub enum Stmt {
    Let(LetStmt),
    Assign(AssignStmt),
    CompoundAssign(CompoundAssignStmt),
    Expr(ExprStmt),
    While(WhileStmt),
    For(ForStmt),
    ForIn(ForInStmt),
    Return(ReturnStmt),
    Break(Span),
    Continue(Span),
}

/// Local let binding.
#[derive(Debug)]
pub struct LetStmt {
    pub name: String,
    pub is_mut: bool,
    pub ty: Type,
    pub value: Expr,
    pub span: Span,
}

/// Assignment statement.
#[derive(Debug)]
pub struct AssignStmt {
    pub target: Place,
    pub value: Expr,
    pub span: Span,
}

/// Place expression (lvalue).
#[derive(Debug)]
pub struct Place {
    pub name: String,
    pub accessors: Vec<PlaceAccessor>,
    pub span: Span,
}

#[derive(Debug)]
pub enum PlaceAccessor {
    Field(String),
    Index(Expr),
}

/// Expression used as statement (followed by `;`).
#[derive(Debug)]
pub struct ExprStmt {
    pub expr: Expr,
    pub span: Span,
}

/// While loop.
#[derive(Debug)]
pub struct WhileStmt {
    pub condition: Expr,
    pub body: Block,
    pub span: Span,
}

/// For-in loop.
#[derive(Debug)]
pub struct ForStmt {
    pub var: String,
    pub start: Expr,
    pub end: Expr,
    pub body: Block,
    pub span: Span,
}

/// Return statement.
#[derive(Debug)]
pub struct ReturnStmt {
    pub value: Option<Expr>,
    pub span: Span,
}

/// Compound assignment statement: `x += expr;`
#[derive(Debug)]
pub struct CompoundAssignStmt {
    pub target: Place,
    pub op: BinOp,
    pub value: Expr,
    pub span: Span,
}

/// For-in loop over an array: `for x in arr { body }`
#[derive(Debug)]
pub struct ForInStmt {
    pub var: String,
    pub iterable: Expr,
    pub body: Block,
    pub span: Span,
}

/// Expression types.
#[derive(Debug)]
pub enum Expr {
    /// Integer literal
    IntLit(i64, Span),
    /// Float literal
    FloatLit(f64, Span),
    /// String literal
    StringLit(String, Span),
    /// Boolean literal
    BoolLit(bool, Span),
    /// Variable reference
    Ident(String, Span),
    /// Binary operation: `a op b`
    BinaryOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    /// Unary operation: `not x` or `-x`
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
        span: Span,
    },
    /// Type cast: `expr as type`
    Cast {
        expr: Box<Expr>,
        ty: Type,
        span: Span,
    },
    /// Function call: `f(args)`
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    /// Field access: `expr.field`
    FieldAccess {
        expr: Box<Expr>,
        field: String,
        span: Span,
    },
    /// Array index: `expr[index]`
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    /// Block expression: `{ stmts; tail_expr }`
    Block(Block),
    /// If expression: `if cond { } else { }`
    If {
        condition: Box<Expr>,
        then_block: Block,
        else_branch: Option<Box<Expr>>, // Block or another If
        span: Span,
    },
    /// Match expression
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    /// Try expression: `try f(args)`
    Try {
        call: Box<Expr>, // Must be a Call expression
        span: Span,
    },
    /// Array literal: `[1, 2, 3]`
    ArrayLit {
        elements: Vec<Expr>,
        span: Span,
    },
    /// Struct literal: `Point { x: 1.0, y: 2.0 }`
    StructLit {
        name: String,
        fields: Vec<FieldInit>,
        span: Span,
    },
    /// Enum constructor: `Option::Some(42)`
    EnumConstructor {
        enum_name: String,
        variant: String,
        args: Vec<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::IntLit(_, s)
            | Expr::FloatLit(_, s)
            | Expr::StringLit(_, s)
            | Expr::BoolLit(_, s)
            | Expr::Ident(_, s) => *s,
            Expr::BinaryOp { span, .. }
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
            | Expr::EnumConstructor { span, .. } => *span,
            Expr::Block(block) => block.span,
        }
    }
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
}

/// Field initializer in struct literal.
#[derive(Debug)]
pub struct FieldInit {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}

/// Match arm: `pattern => expr,`
#[derive(Debug)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub span: Span,
}

/// Pattern for match arms.
#[derive(Debug)]
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

/// Type annotations.
#[derive(Debug, Clone)]
pub enum Type {
    /// Primitive: i32, i64, f64, bool, str, unit
    Primitive(PrimitiveType, Span),
    /// Fixed-size array: [type; size]
    FixedArray(Box<Type>, i64, Span),
    /// Dynamic array: [type]
    DynamicArray(Box<Type>, Span),
    /// Result<T, E>
    Result(Box<Type>, Box<Type>, Span),
    /// Named type (struct or enum reference)
    Named(String, Span),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrimitiveType {
    I32,
    I64,
    F64,
    Bool,
    Str,
    Unit,
}
