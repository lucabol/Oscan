use std::collections::HashSet;

use crate::ast::*;
use crate::error::CompileError;
use crate::token::{Span, Token, TokenKind};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Known struct/enum names for disambiguating `Name { ... }` as struct literal vs block.
    known_type_names: HashSet<String>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            known_type_names: HashSet::new(),
        }
    }

    // ─── Helpers ───────────────────────────────────────────────

    fn peek(&self) -> &TokenKind {
        self.tokens
            .get(self.pos)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    fn peek_span(&self) -> Span {
        self.tokens
            .get(self.pos)
            .map(|t| t.span)
            .unwrap_or(Span::new(0, 0))
    }

    fn peek_ahead(&self, offset: usize) -> &TokenKind {
        self.tokens
            .get(self.pos + offset)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos];
        self.pos += 1;
        tok
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<Span, CompileError> {
        let span = self.peek_span();
        if self.peek() == kind {
            self.advance();
            Ok(span)
        } else {
            Err(CompileError::new(
                span,
                format!("expected '{}', found '{}'", kind, self.peek()),
            ))
        }
    }

    fn expect_ident(&mut self) -> Result<(String, Span), CompileError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok((name, span))
            }
            _ => Err(CompileError::new(
                span,
                format!("expected identifier, found '{}'", self.peek()),
            )),
        }
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.peek() == kind
    }

    fn at_eof(&self) -> bool {
        self.at(&TokenKind::Eof)
    }

    // ─── Pass 1: Collect type names ──────────────────────────

    fn collect_type_names(&mut self) {
        let saved = self.pos;
        self.pos = 0;
        while !self.at_eof() {
            match self.peek() {
                TokenKind::Struct | TokenKind::Enum => {
                    self.advance();
                    if let TokenKind::Ident(name) = self.peek().clone() {
                        self.known_type_names.insert(name);
                    }
                    self.advance();
                }
                _ => {
                    self.advance();
                }
            }
        }
        // Also add "Result" as a known type name
        self.known_type_names.insert("Result".to_string());
        self.pos = saved;
    }

    // ─── Program ─────────────────────────────────────────────

    pub fn parse_program(&mut self) -> Result<Program, CompileError> {
        self.collect_type_names();
        let mut decls = Vec::new();
        while !self.at_eof() {
            decls.push(self.parse_top_decl()?);
        }
        Ok(Program { decls })
    }

    fn parse_top_decl(&mut self) -> Result<TopDecl, CompileError> {
        match self.peek() {
            TokenKind::Fn | TokenKind::FnBang => Ok(TopDecl::Fn(self.parse_fn_decl()?)),
            TokenKind::Struct => Ok(TopDecl::Struct(self.parse_struct_decl()?)),
            TokenKind::Enum => Ok(TopDecl::Enum(self.parse_enum_decl()?)),
            TokenKind::Let => Ok(TopDecl::Let(self.parse_top_let_decl()?)),
            TokenKind::Extern => Ok(TopDecl::Extern(self.parse_extern_block()?)),
            TokenKind::Use => self.parse_use_decl(),
            _ => Err(CompileError::new(
                self.peek_span(),
                format!(
                    "expected top-level declaration (fn, struct, enum, let, extern, use), found '{}'",
                    self.peek()
                ),
            )),
        }
    }

    fn parse_use_decl(&mut self) -> Result<TopDecl, CompileError> {
        self.advance(); // consume `use`
        let span = self.peek_span();
        match self.peek().clone() {
            TokenKind::StringLit(path) => {
                let path = path.clone();
                self.advance();
                let ns = if *self.peek() == TokenKind::As {
                    self.advance(); // consume `as`
                    match self.peek().clone() {
                        TokenKind::Ident(name) => {
                            let name = name.clone();
                            self.advance();
                            Some(name)
                        }
                        _ => {
                            return Err(CompileError::new(
                                self.peek_span(),
                                format!(
                                    "expected identifier after 'as', found '{}'",
                                    self.peek()
                                ),
                            ))
                        }
                    }
                } else {
                    None
                };
                Ok(TopDecl::Use(path, ns))
            }
            _ => Err(CompileError::new(
                span,
                format!(
                    "expected string literal after 'use', found '{}'",
                    self.peek()
                ),
            )),
        }
    }

    // ─── Function Declaration ────────────────────────────────

    fn parse_fn_decl(&mut self) -> Result<FnDecl, CompileError> {
        let span = self.peek_span();
        let is_pure = match self.peek() {
            TokenKind::Fn => {
                self.advance();
                true
            }
            TokenKind::FnBang => {
                self.advance();
                false
            }
            _ => {
                return Err(CompileError::new(
                    span,
                    format!("expected 'fn' or 'fn!', found '{}'", self.peek()),
                ));
            }
        };

        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_param_list()?;
        self.expect(&TokenKind::RParen)?;

        let return_type = if self.at(&TokenKind::Arrow) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        let body = self.parse_block()?;

        Ok(FnDecl {
            name,
            params,
            return_type,
            body,
            is_pure,
            span,
        })
    }

    fn parse_param_list(&mut self) -> Result<Vec<Param>, CompileError> {
        let mut params = Vec::new();
        if self.at(&TokenKind::RParen) {
            return Ok(params);
        }
        params.push(self.parse_param()?);
        while self.at(&TokenKind::Comma) {
            self.advance();
            if self.at(&TokenKind::RParen) {
                break; // trailing comma
            }
            params.push(self.parse_param()?);
        }
        Ok(params)
    }

    fn parse_param(&mut self) -> Result<Param, CompileError> {
        let (name, span) = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.parse_type()?;
        Ok(Param { name, ty, span })
    }

    // ─── Struct Declaration ──────────────────────────────────

    fn parse_struct_decl(&mut self) -> Result<StructDecl, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::Struct)?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let fields = self.parse_field_list()?;
        self.expect(&TokenKind::RBrace)?;
        Ok(StructDecl { name, fields, span })
    }

    fn parse_field_list(&mut self) -> Result<Vec<Field>, CompileError> {
        let mut fields = Vec::new();
        if self.at(&TokenKind::RBrace) {
            return Ok(fields);
        }
        fields.push(self.parse_field()?);
        while self.at(&TokenKind::Comma) {
            self.advance();
            if self.at(&TokenKind::RBrace) {
                break;
            }
            fields.push(self.parse_field()?);
        }
        Ok(fields)
    }

    fn parse_field(&mut self) -> Result<Field, CompileError> {
        let (name, span) = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.parse_type()?;
        Ok(Field { name, ty, span })
    }

    // ─── Enum Declaration ────────────────────────────────────

    fn parse_enum_decl(&mut self) -> Result<EnumDecl, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::Enum)?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let variants = self.parse_variant_list()?;
        self.expect(&TokenKind::RBrace)?;
        Ok(EnumDecl {
            name,
            variants,
            span,
        })
    }

    fn parse_variant_list(&mut self) -> Result<Vec<Variant>, CompileError> {
        let mut variants = Vec::new();
        if self.at(&TokenKind::RBrace) {
            return Ok(variants);
        }
        variants.push(self.parse_variant()?);
        while self.at(&TokenKind::Comma) {
            self.advance();
            if self.at(&TokenKind::RBrace) {
                break;
            }
            variants.push(self.parse_variant()?);
        }
        Ok(variants)
    }

    fn parse_variant(&mut self) -> Result<Variant, CompileError> {
        let (name, span) = self.expect_ident()?;
        let mut payload_types = Vec::new();
        if self.at(&TokenKind::LParen) {
            self.advance();
            if !self.at(&TokenKind::RParen) {
                payload_types.push(self.parse_type()?);
                while self.at(&TokenKind::Comma) {
                    self.advance();
                    if self.at(&TokenKind::RParen) {
                        break;
                    }
                    payload_types.push(self.parse_type()?);
                }
            }
            self.expect(&TokenKind::RParen)?;
        }
        Ok(Variant {
            name,
            payload_types,
            span,
        })
    }

    // ─── Top-level Let Declaration ───────────────────────────

    fn parse_top_let_decl(&mut self) -> Result<LetDecl, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::Let)?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.parse_type()?;
        self.expect(&TokenKind::Eq)?;
        let value = self.parse_expr()?;
        self.expect(&TokenKind::Semicolon)?;
        Ok(LetDecl {
            name,
            ty,
            value,
            span,
        })
    }

    // ─── Extern Block ────────────────────────────────────────

    fn parse_extern_block(&mut self) -> Result<ExternBlock, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::Extern)?;
        self.expect(&TokenKind::LBrace)?;
        let mut decls = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            decls.push(self.parse_extern_fn_decl()?);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(ExternBlock { decls, span })
    }

    fn parse_extern_fn_decl(&mut self) -> Result<ExternFnDecl, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::FnBang)?;
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_param_list()?;
        self.expect(&TokenKind::RParen)?;
        let return_type = if self.at(&TokenKind::Arrow) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&TokenKind::Semicolon)?;
        Ok(ExternFnDecl {
            name,
            params,
            return_type,
            span,
        })
    }

    // ─── Types ───────────────────────────────────────────────

    fn parse_type(&mut self) -> Result<Type, CompileError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokenKind::LBracket => self.parse_array_type(),
            TokenKind::Fn => self.parse_fn_ptr_type(),
            TokenKind::Ident(name) => {
                let name = name.clone();
                match name.as_str() {
                    "i32" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::I32, span))
                    }
                    "i64" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::I64, span))
                    }
                    "f64" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::F64, span))
                    }
                    "bool" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::Bool, span))
                    }
                    "str" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::Str, span))
                    }
                    "unit" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::Unit, span))
                    }
                    "handle" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::Handle, span))
                    }
                    "map" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::Map, span))
                    }
                    "map_str_i32" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::MapStrI32, span))
                    }
                    "map_str_i64" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::MapStrI64, span))
                    }
                    "map_str_f64" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::MapStrF64, span))
                    }
                    "map_i32_str" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::MapI32Str, span))
                    }
                    "map_i32_i32" => {
                        self.advance();
                        Ok(Type::Primitive(PrimitiveType::MapI32I32, span))
                    }
                    "Result" => {
                        self.advance();
                        self.expect(&TokenKind::Lt)?;
                        let ok_type = self.parse_type()?;
                        self.expect(&TokenKind::Comma)?;
                        let err_type = self.parse_type()?;
                        self.expect(&TokenKind::Gt)?;
                        Ok(Type::Result(Box::new(ok_type), Box::new(err_type), span))
                    }
                    _ => {
                        self.advance();
                        Ok(Type::Named(name, span))
                    }
                }
            }
            _ => Err(CompileError::new(
                span,
                format!("expected type, found '{}'", self.peek()),
            )),
        }
    }

    fn parse_array_type(&mut self) -> Result<Type, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::LBracket)?;
        let elem_type = self.parse_type()?;
        if self.at(&TokenKind::Semicolon) {
            // Fixed-size: [type; N]
            self.advance();
            let size_span = self.peek_span();
            if let TokenKind::IntLit(size) = self.peek().clone() {
                self.advance();
                self.expect(&TokenKind::RBracket)?;
                Ok(Type::FixedArray(Box::new(elem_type), size, span))
            } else {
                Err(CompileError::new(
                    size_span,
                    format!(
                        "expected integer literal for array size, found '{}'",
                        self.peek()
                    ),
                ))
            }
        } else {
            // Dynamic: [type]
            self.expect(&TokenKind::RBracket)?;
            Ok(Type::DynamicArray(Box::new(elem_type), span))
        }
    }

    fn parse_fn_ptr_type(&mut self) -> Result<Type, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::Fn)?;
        self.expect(&TokenKind::LParen)?;
        let mut param_types = Vec::new();
        while !self.at(&TokenKind::RParen) && !self.at_eof() {
            if !param_types.is_empty() {
                self.expect(&TokenKind::Comma)?;
            }
            param_types.push(self.parse_type()?);
        }
        self.expect(&TokenKind::RParen)?;
        let ret = if self.at(&TokenKind::Arrow) {
            self.advance();
            self.parse_type()?
        } else {
            Type::Primitive(PrimitiveType::Unit, span)
        };
        Ok(Type::FnPtr(param_types, Box::new(ret), span))
    }

    // ─── Block ───────────────────────────────────────────────

    fn parse_block(&mut self) -> Result<Block, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::LBrace)?;

        let mut stmts = Vec::new();
        let mut tail_expr = None;

        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            // Try to determine if this is a statement or a tail expression.
            // Statements that start with keywords are unambiguous.
            if self.is_at_statement_start() {
                stmts.push(self.parse_stmt()?);
            } else {
                // Parse an expression. Then check if followed by `;` (stmt) or `}` (tail).
                let expr = self.parse_expr()?;
                if self.at(&TokenKind::Semicolon) {
                    let expr_span = expr.span();
                    self.advance();
                    stmts.push(Stmt::Expr(ExprStmt {
                        expr,
                        span: expr_span,
                    }));
                } else if self.at(&TokenKind::RBrace) {
                    tail_expr = Some(Box::new(expr));
                } else {
                    return Err(CompileError::new(
                        self.peek_span(),
                        format!(
                            "expected ';' or '}}' after expression, found '{}'",
                            self.peek()
                        ),
                    ));
                }
            }
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(Block {
            stmts,
            tail_expr,
            span,
        })
    }

    fn is_at_statement_start(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Let
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Return
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Defer
        ) || self.is_at_assign()
    }

    /// Check if we're at an assignment: `ident (.field | [expr])* = expr ;`
    /// We need lookahead to distinguish from expression statements.
    fn is_at_assign(&self) -> bool {
        if !matches!(self.peek(), TokenKind::Ident(_)) {
            return false;
        }
        // Scan ahead past the place expression
        let mut i = 1;
        loop {
            match self.peek_ahead(i) {
                TokenKind::Dot => {
                    i += 1;
                    // Expect an identifier after dot
                    if matches!(self.peek_ahead(i), TokenKind::Ident(_)) {
                        i += 1;
                    } else {
                        return false;
                    }
                }
                TokenKind::LBracket => {
                    i += 1;
                    // Skip over bracket contents — find matching ]
                    let mut depth = 1;
                    while depth > 0 {
                        match self.peek_ahead(i) {
                            TokenKind::LBracket => depth += 1,
                            TokenKind::RBracket => depth -= 1,
                            TokenKind::Eof => return false,
                            _ => {}
                        }
                        i += 1;
                    }
                }
                TokenKind::Eq
                | TokenKind::PlusEq
                | TokenKind::MinusEq
                | TokenKind::StarEq
                | TokenKind::SlashEq
                | TokenKind::PercentEq => {
                    return true;
                }
                _ => return false,
            }
        }
    }

    // ─── Statements ──────────────────────────────────────────

    fn parse_stmt(&mut self) -> Result<Stmt, CompileError> {
        match self.peek() {
            TokenKind::Let => self.parse_let_stmt(),
            TokenKind::While => self.parse_while_stmt(),
            TokenKind::For => self.parse_for_stmt(),
            TokenKind::Return => self.parse_return_stmt(),
            TokenKind::Defer => self.parse_defer_stmt(),
            TokenKind::Break => {
                let span = self.peek_span();
                self.advance();
                self.expect(&TokenKind::Semicolon)?;
                Ok(Stmt::Break(span))
            }
            TokenKind::Continue => {
                let span = self.peek_span();
                self.advance();
                self.expect(&TokenKind::Semicolon)?;
                Ok(Stmt::Continue(span))
            }
            _ if self.is_at_assign() => self.parse_assign_stmt(),
            _ => {
                let expr = self.parse_expr()?;
                let span = expr.span();
                self.expect(&TokenKind::Semicolon)?;
                Ok(Stmt::Expr(ExprStmt { expr, span }))
            }
        }
    }

    fn parse_let_stmt(&mut self) -> Result<Stmt, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::Let)?;
        let is_mut = if self.at(&TokenKind::Mut) {
            self.advance();
            true
        } else {
            false
        };
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.parse_type()?;
        self.expect(&TokenKind::Eq)?;
        let value = self.parse_expr()?;
        self.expect(&TokenKind::Semicolon)?;
        Ok(Stmt::Let(LetStmt {
            name,
            is_mut,
            ty,
            value,
            span,
        }))
    }

    fn parse_assign_stmt(&mut self) -> Result<Stmt, CompileError> {
        let span = self.peek_span();
        let target = self.parse_place()?;
        // Check for compound assignment operators
        let compound_op = match self.peek() {
            TokenKind::PlusEq => Some(BinOp::Add),
            TokenKind::MinusEq => Some(BinOp::Sub),
            TokenKind::StarEq => Some(BinOp::Mul),
            TokenKind::SlashEq => Some(BinOp::Div),
            TokenKind::PercentEq => Some(BinOp::Mod),
            _ => None,
        };
        if let Some(op) = compound_op {
            self.advance();
            let value = self.parse_expr()?;
            self.expect(&TokenKind::Semicolon)?;
            Ok(Stmt::CompoundAssign(CompoundAssignStmt {
                target,
                op,
                value,
                span,
            }))
        } else {
            self.expect(&TokenKind::Eq)?;
            let value = self.parse_expr()?;
            self.expect(&TokenKind::Semicolon)?;
            Ok(Stmt::Assign(AssignStmt {
                target,
                value,
                span,
            }))
        }
    }

    fn parse_place(&mut self) -> Result<Place, CompileError> {
        let (name, span) = self.expect_ident()?;
        let mut accessors = Vec::new();
        loop {
            if self.at(&TokenKind::Dot) {
                self.advance();
                let (field, _) = self.expect_ident()?;
                accessors.push(PlaceAccessor::Field(field));
            } else if self.at(&TokenKind::LBracket) {
                self.advance();
                let index = self.parse_expr()?;
                self.expect(&TokenKind::RBracket)?;
                accessors.push(PlaceAccessor::Index(index));
            } else {
                break;
            }
        }
        Ok(Place {
            name,
            accessors,
            span,
        })
    }

    fn parse_while_stmt(&mut self) -> Result<Stmt, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::While)?;
        let condition = self.parse_expr()?;
        let body = self.parse_block()?;
        // Consume optional trailing semicolon (e.g. `while cond { ... };`)
        if self.at(&TokenKind::Semicolon) {
            self.advance();
        }
        Ok(Stmt::While(WhileStmt {
            condition,
            body,
            span,
        }))
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::For)?;
        let (var, _) = self.expect_ident()?;
        self.expect(&TokenKind::In)?;
        let start = self.parse_expr()?;
        // Distinguish range `start..end` from array iteration
        if self.at(&TokenKind::DotDot) {
            self.advance();
            let end = self.parse_expr()?;
            let body = self.parse_block()?;
            if self.at(&TokenKind::Semicolon) {
                self.advance();
            }
            Ok(Stmt::For(ForStmt {
                var,
                start,
                end,
                body,
                span,
            }))
        } else {
            // for x in arr { body }
            let body = self.parse_block()?;
            if self.at(&TokenKind::Semicolon) {
                self.advance();
            }
            Ok(Stmt::ForIn(ForInStmt {
                var,
                iterable: start,
                body,
                span,
            }))
        }
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::Return)?;
        let value = if self.at(&TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(&TokenKind::Semicolon)?;
        Ok(Stmt::Return(ReturnStmt { value, span }))
    }

    fn parse_defer_stmt(&mut self) -> Result<Stmt, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::Defer)?;
        let expr = self.parse_expr()?;
        self.expect(&TokenKind::Semicolon)?;
        Ok(Stmt::Defer(DeferStmt { expr, span }))
    }

    // ─── Expressions (precedence climbing) ───────────────────

    fn parse_expr(&mut self) -> Result<Expr, CompileError> {
        self.parse_or_expr()
    }

    fn parse_or_expr(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.parse_and_expr()?;
        while self.at(&TokenKind::Or) {
            let span = self.peek_span();
            self.advance();
            let right = self.parse_and_expr()?;
            left = Expr::BinaryOp {
                op: BinOp::Or,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn parse_and_expr(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.parse_equality_expr()?;
        while self.at(&TokenKind::And) {
            let span = self.peek_span();
            self.advance();
            let right = self.parse_equality_expr()?;
            left = Expr::BinaryOp {
                op: BinOp::And,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn parse_equality_expr(&mut self) -> Result<Expr, CompileError> {
        let left = self.parse_relational_expr()?;
        let op = match self.peek() {
            TokenKind::EqEq => BinOp::Eq,
            TokenKind::BangEq => BinOp::Neq,
            _ => return Ok(left),
        };
        let span = self.peek_span();
        self.advance();
        let right = self.parse_relational_expr()?;
        Ok(Expr::BinaryOp {
            op,
            left: Box::new(left),
            right: Box::new(right),
            span,
        })
    }

    fn parse_relational_expr(&mut self) -> Result<Expr, CompileError> {
        let left = self.parse_additive_expr()?;
        let op = match self.peek() {
            TokenKind::Lt => BinOp::Lt,
            TokenKind::Gt => BinOp::Gt,
            TokenKind::LtEq => BinOp::LtEq,
            TokenKind::GtEq => BinOp::GtEq,
            _ => return Ok(left),
        };
        let span = self.peek_span();
        self.advance();
        let right = self.parse_additive_expr()?;
        Ok(Expr::BinaryOp {
            op,
            left: Box::new(left),
            right: Box::new(right),
            span,
        })
    }

    fn parse_additive_expr(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.parse_mult_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            let span = self.peek_span();
            self.advance();
            let right = self.parse_mult_expr()?;
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn parse_mult_expr(&mut self) -> Result<Expr, CompileError> {
        let mut left = self.parse_unary_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Mod,
                _ => break,
            };
            let span = self.peek_span();
            self.advance();
            let right = self.parse_unary_expr()?;
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn parse_unary_expr(&mut self) -> Result<Expr, CompileError> {
        match self.peek() {
            TokenKind::Not => {
                let span = self.peek_span();
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                    span,
                })
            }
            TokenKind::Minus => {
                let span = self.peek_span();
                self.advance();
                let operand = self.parse_unary_expr()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Neg,
                    operand: Box::new(operand),
                    span,
                })
            }
            _ => self.parse_cast_expr(),
        }
    }

    fn parse_cast_expr(&mut self) -> Result<Expr, CompileError> {
        let expr = self.parse_postfix_expr()?;
        if self.at(&TokenKind::As) {
            let span = self.peek_span();
            self.advance();
            let ty = self.parse_type()?;
            Ok(Expr::Cast {
                expr: Box::new(expr),
                ty,
                span,
            })
        } else {
            Ok(expr)
        }
    }

    fn parse_postfix_expr(&mut self) -> Result<Expr, CompileError> {
        let mut expr = self.parse_primary_expr()?;
        loop {
            if self.at(&TokenKind::LParen) {
                let span = expr.span();
                self.advance();
                let args = self.parse_arg_list()?;
                self.expect(&TokenKind::RParen)?;
                expr = Expr::Call {
                    callee: Box::new(expr),
                    args,
                    span,
                };
            } else if self.at(&TokenKind::LBracket) {
                let span = expr.span();
                self.advance();
                let index = self.parse_expr()?;
                self.expect(&TokenKind::RBracket)?;
                expr = Expr::Index {
                    expr: Box::new(expr),
                    index: Box::new(index),
                    span,
                };
            } else if self.at(&TokenKind::Dot) {
                let span = expr.span();
                self.advance();
                let (field, _) = self.expect_ident()?;
                expr = Expr::FieldAccess {
                    expr: Box::new(expr),
                    field,
                    span,
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_arg_list(&mut self) -> Result<Vec<Expr>, CompileError> {
        let mut args = Vec::new();
        if self.at(&TokenKind::RParen) {
            return Ok(args);
        }
        args.push(self.parse_expr()?);
        while self.at(&TokenKind::Comma) {
            self.advance();
            if self.at(&TokenKind::RParen) {
                break;
            }
            args.push(self.parse_expr()?);
        }
        Ok(args)
    }

    // ─── Primary Expressions ─────────────────────────────────

    fn parse_primary_expr(&mut self) -> Result<Expr, CompileError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokenKind::IntLit(v) => {
                self.advance();
                Ok(Expr::IntLit(v, span))
            }
            TokenKind::FloatLit(v) => {
                self.advance();
                Ok(Expr::FloatLit(v, span))
            }
            TokenKind::StringLit(s) => {
                let s = s.clone();
                self.advance();
                Ok(Expr::StringLit(s, span))
            }
            TokenKind::InterpStringStart(s) => {
                let s = s.clone();
                self.advance();
                self.parse_interpolated_string_expr(s, span)
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr::BoolLit(true, span))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr::BoolLit(false, span))
            }
            TokenKind::If => self.parse_if_expr(),
            TokenKind::Match => self.parse_match_expr(),
            TokenKind::Try => self.parse_try_expr(),
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                Ok(Expr::Block(block))
            }
            TokenKind::LBracket => self.parse_array_literal(),
            TokenKind::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(expr)
            }
            TokenKind::Arena => {
                self.advance();
                let body = self.parse_block()?;
                Ok(Expr::Arena { body, span })
            }
            TokenKind::Ident(name) => {
                let name = name.clone();
                // Check for enum constructor: Ident::Ident(...)
                if self.peek_ahead(1) == &TokenKind::ColonColon {
                    return self.parse_enum_constructor();
                }
                // Check for struct literal: Ident { ... }
                // Only if the name is a known type name
                if self.peek_ahead(1) == &TokenKind::LBrace && self.known_type_names.contains(&name)
                {
                    return self.parse_struct_literal();
                }
                // Plain identifier
                self.advance();
                Ok(Expr::Ident(name, span))
            }
            _ => Err(CompileError::new(
                span,
                format!("expected expression, found '{}'", self.peek()),
            )),
        }
    }

    fn parse_interpolated_string_expr(
        &mut self,
        start_text: String,
        span: Span,
    ) -> Result<Expr, CompileError> {
        let mut parts = Vec::new();
        if !start_text.is_empty() {
            parts.push(InterpolatedStringPart::Text(start_text));
        }

        loop {
            let expr = self.parse_expr()?;
            parts.push(InterpolatedStringPart::Expr(expr));

            match self.peek().clone() {
                TokenKind::InterpStringMiddle(text) => {
                    let text = text.clone();
                    self.advance();
                    if !text.is_empty() {
                        parts.push(InterpolatedStringPart::Text(text));
                    }
                }
                TokenKind::InterpStringEnd(text) => {
                    let text = text.clone();
                    self.advance();
                    if !text.is_empty() {
                        parts.push(InterpolatedStringPart::Text(text));
                    }
                    break;
                }
                _ => {
                    return Err(CompileError::new(
                        self.peek_span(),
                        "unterminated interpolated string",
                    ));
                }
            }
        }

        Ok(Expr::InterpolatedString { parts, span })
    }

    fn parse_if_expr(&mut self) -> Result<Expr, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::If)?;
        let condition = self.parse_expr()?;
        let then_block = self.parse_block()?;
        let else_branch = if self.at(&TokenKind::Else) {
            self.advance();
            if self.at(&TokenKind::If) {
                Some(Box::new(self.parse_if_expr()?))
            } else {
                let block = self.parse_block()?;
                Some(Box::new(Expr::Block(block)))
            }
        } else {
            None
        };
        Ok(Expr::If {
            condition: Box::new(condition),
            then_block,
            else_branch,
            span,
        })
    }

    fn parse_match_expr(&mut self) -> Result<Expr, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::Match)?;
        let scrutinee = self.parse_expr()?;
        self.expect(&TokenKind::LBrace)?;
        let mut arms = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            arms.push(self.parse_match_arm()?);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            span,
        })
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, CompileError> {
        let span = self.peek_span();
        let pattern = self.parse_pattern()?;
        self.expect(&TokenKind::FatArrow)?;
        let body = self.parse_expr()?;
        self.expect(&TokenKind::Comma)?;
        Ok(MatchArm {
            pattern,
            body,
            span,
        })
    }

    fn parse_try_expr(&mut self) -> Result<Expr, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::Try)?;
        // Parse the call expression (postfix handles call suffix)
        let call = self.parse_postfix_expr()?;
        Ok(Expr::Try {
            call: Box::new(call),
            span,
        })
    }

    fn parse_array_literal(&mut self) -> Result<Expr, CompileError> {
        let span = self.peek_span();
        self.expect(&TokenKind::LBracket)?;
        let mut elements = Vec::new();
        if !self.at(&TokenKind::RBracket) {
            elements.push(self.parse_expr()?);
            while self.at(&TokenKind::Comma) {
                self.advance();
                if self.at(&TokenKind::RBracket) {
                    break;
                }
                elements.push(self.parse_expr()?);
            }
        }
        self.expect(&TokenKind::RBracket)?;
        Ok(Expr::ArrayLit { elements, span })
    }

    fn parse_struct_literal(&mut self) -> Result<Expr, CompileError> {
        let span = self.peek_span();
        let (name, _) = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        if !self.at(&TokenKind::RBrace) {
            fields.push(self.parse_field_init()?);
            while self.at(&TokenKind::Comma) {
                self.advance();
                if self.at(&TokenKind::RBrace) {
                    break;
                }
                fields.push(self.parse_field_init()?);
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Expr::StructLit { name, fields, span })
    }

    fn parse_field_init(&mut self) -> Result<FieldInit, CompileError> {
        let (name, span) = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let value = self.parse_expr()?;
        Ok(FieldInit { name, value, span })
    }

    fn parse_enum_constructor(&mut self) -> Result<Expr, CompileError> {
        let span = self.peek_span();
        let (enum_name, _) = self.expect_ident()?;
        self.expect(&TokenKind::ColonColon)?;
        let (variant, _) = self.expect_ident()?;
        let args = if self.at(&TokenKind::LParen) {
            self.advance();
            let args = self.parse_arg_list()?;
            self.expect(&TokenKind::RParen)?;
            args
        } else {
            Vec::new()
        };
        Ok(Expr::EnumConstructor {
            enum_name,
            variant,
            args,
            span,
        })
    }

    // ─── Patterns ────────────────────────────────────────────

    fn parse_pattern(&mut self) -> Result<Pattern, CompileError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokenKind::Underscore => {
                self.advance();
                Ok(Pattern::Wildcard(span))
            }
            TokenKind::True => {
                self.advance();
                Ok(Pattern::BoolLit(true, span))
            }
            TokenKind::False => {
                self.advance();
                Ok(Pattern::BoolLit(false, span))
            }
            TokenKind::IntLit(v) => {
                self.advance();
                Ok(Pattern::IntLit(v, span))
            }
            TokenKind::FloatLit(v) => {
                self.advance();
                Ok(Pattern::FloatLit(v, span))
            }
            TokenKind::StringLit(s) => {
                let s = s.clone();
                self.advance();
                Ok(Pattern::StringLit(s, span))
            }
            TokenKind::Ident(_) if self.peek_ahead(1) == &TokenKind::ColonColon => {
                self.parse_enum_pattern()
            }
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(Pattern::Ident(name, span))
            }
            TokenKind::Minus => {
                // Negative numeric literal in pattern
                self.advance();
                let lit_span = self.peek_span();
                match self.peek().clone() {
                    TokenKind::IntLit(v) => {
                        self.advance();
                        Ok(Pattern::IntLit(-v, lit_span))
                    }
                    TokenKind::FloatLit(v) => {
                        self.advance();
                        Ok(Pattern::FloatLit(-v, lit_span))
                    }
                    _ => Err(CompileError::new(
                        lit_span,
                        format!(
                            "expected numeric literal after '-' in pattern, found '{}'",
                            self.peek()
                        ),
                    )),
                }
            }
            _ => Err(CompileError::new(
                span,
                format!("expected pattern, found '{}'", self.peek()),
            )),
        }
    }

    fn parse_enum_pattern(&mut self) -> Result<Pattern, CompileError> {
        let span = self.peek_span();
        let (enum_name, _) = self.expect_ident()?;
        self.expect(&TokenKind::ColonColon)?;
        let (variant, _) = self.expect_ident()?;
        let bindings = if self.at(&TokenKind::LParen) {
            self.advance();
            let mut patterns = Vec::new();
            if !self.at(&TokenKind::RParen) {
                patterns.push(self.parse_pattern()?);
                while self.at(&TokenKind::Comma) {
                    self.advance();
                    if self.at(&TokenKind::RParen) {
                        break;
                    }
                    patterns.push(self.parse_pattern()?);
                }
            }
            self.expect(&TokenKind::RParen)?;
            patterns
        } else {
            Vec::new()
        };
        Ok(Pattern::Enum {
            enum_name,
            variant,
            bindings,
            span,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn parse(input: &str) -> Program {
        let mut lexer = Lexer::new(input);
        let tokens = lexer.tokenize().expect("lexer failed");
        let mut parser = Parser::new(tokens);
        parser.parse_program().expect("parser failed")
    }

    fn parse_err(input: &str) -> String {
        let mut lexer = Lexer::new(input);
        let tokens = lexer.tokenize().expect("lexer failed");
        let mut parser = Parser::new(tokens);
        parser.parse_program().unwrap_err().message
    }

    #[test]
    fn test_empty_program() {
        let program = parse("");
        assert!(program.decls.is_empty());
    }

    #[test]
    fn test_simple_fn() {
        let program = parse("fn add(a: i32, b: i32) -> i32 { a + b }");
        assert_eq!(program.decls.len(), 1);
        match &program.decls[0] {
            TopDecl::Fn(f) => {
                assert_eq!(f.name, "add");
                assert!(f.is_pure);
                assert_eq!(f.params.len(), 2);
                assert!(f.return_type.is_some());
            }
            _ => panic!("expected function declaration"),
        }
    }

    #[test]
    fn test_fn_bang() {
        let program = parse("fn! main() { }");
        match &program.decls[0] {
            TopDecl::Fn(f) => {
                assert_eq!(f.name, "main");
                assert!(!f.is_pure);
            }
            _ => panic!("expected fn!"),
        }
    }

    #[test]
    fn test_struct_decl() {
        let program = parse("struct Point { x: f64, y: f64, }");
        match &program.decls[0] {
            TopDecl::Struct(s) => {
                assert_eq!(s.name, "Point");
                assert_eq!(s.fields.len(), 2);
                assert_eq!(s.fields[0].name, "x");
                assert_eq!(s.fields[1].name, "y");
            }
            _ => panic!("expected struct"),
        }
    }

    #[test]
    fn test_enum_decl() {
        let program = parse("enum Shape { Circle(f64), Rectangle(f64, f64), Empty, }");
        match &program.decls[0] {
            TopDecl::Enum(e) => {
                assert_eq!(e.name, "Shape");
                assert_eq!(e.variants.len(), 3);
                assert_eq!(e.variants[0].name, "Circle");
                assert_eq!(e.variants[0].payload_types.len(), 1);
                assert_eq!(e.variants[1].name, "Rectangle");
                assert_eq!(e.variants[1].payload_types.len(), 2);
                assert_eq!(e.variants[2].name, "Empty");
                assert_eq!(e.variants[2].payload_types.len(), 0);
            }
            _ => panic!("expected enum"),
        }
    }

    #[test]
    fn test_top_level_let() {
        let program = parse("let PI: f64 = 3.14;");
        match &program.decls[0] {
            TopDecl::Let(l) => {
                assert_eq!(l.name, "PI");
            }
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn test_extern_block() {
        let program = parse(r#"extern { fn! printf(fmt: str) -> i32; }"#);
        match &program.decls[0] {
            TopDecl::Extern(e) => {
                assert_eq!(e.decls.len(), 1);
                assert_eq!(e.decls[0].name, "printf");
            }
            _ => panic!("expected extern"),
        }
    }

    #[test]
    fn test_precedence() {
        let program = parse("fn f() -> i32 { 1 + 2 * 3 }");
        match &program.decls[0] {
            TopDecl::Fn(f) => {
                let tail = f.body.tail_expr.as_ref().unwrap();
                // Should be Add(1, Mul(2, 3)) due to precedence
                match tail.as_ref() {
                    Expr::BinaryOp {
                        op: BinOp::Add,
                        right,
                        ..
                    } => {
                        assert!(matches!(
                            right.as_ref(),
                            Expr::BinaryOp { op: BinOp::Mul, .. }
                        ));
                    }
                    other => panic!("expected add, got {:?}", other),
                }
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn test_if_else() {
        let program = parse("fn f(x: i32) -> i32 { if x > 0 { x } else { 0 } }");
        match &program.decls[0] {
            TopDecl::Fn(f) => {
                let tail = f.body.tail_expr.as_ref().unwrap();
                assert!(matches!(tail.as_ref(), Expr::If { .. }));
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn test_match_expr() {
        let program = parse(
            "enum Opt { Some(i32), None, } fn f(o: Opt) -> i32 { match o { Opt::Some(v) => v, Opt::None => 0, } }",
        );
        assert_eq!(program.decls.len(), 2);
        match &program.decls[1] {
            TopDecl::Fn(f) => {
                let tail = f.body.tail_expr.as_ref().unwrap();
                match tail.as_ref() {
                    Expr::Match { arms, .. } => {
                        assert_eq!(arms.len(), 2);
                    }
                    other => panic!("expected match, got {:?}", other),
                }
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn test_for_loop() {
        let program = parse("fn! f() { for i in 0..10 { print(i); } }");
        match &program.decls[0] {
            TopDecl::Fn(f) => {
                assert_eq!(f.body.stmts.len(), 1);
                assert!(matches!(&f.body.stmts[0], Stmt::For(_)));
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn test_while_loop() {
        let program = parse("fn! f() { let mut x: i32 = 0; while x < 10 { x = x + 1; } }");
        match &program.decls[0] {
            TopDecl::Fn(f) => {
                assert_eq!(f.body.stmts.len(), 2);
                assert!(matches!(&f.body.stmts[0], Stmt::Let(_)));
                assert!(matches!(&f.body.stmts[1], Stmt::While(_)));
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn test_struct_literal() {
        let program =
            parse("struct Point { x: f64, y: f64 } fn f() -> Point { Point { x: 1.0, y: 2.0 } }");
        match &program.decls[1] {
            TopDecl::Fn(f) => {
                let tail = f.body.tail_expr.as_ref().unwrap();
                match tail.as_ref() {
                    Expr::StructLit { name, fields, .. } => {
                        assert_eq!(name, "Point");
                        assert_eq!(fields.len(), 2);
                    }
                    other => panic!("expected struct literal, got {:?}", other),
                }
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn test_error_missing_semicolon() {
        let msg = parse_err("fn f() { let x: i32 = 1 }");
        assert!(msg.contains("expected ';'"), "got: {msg}");
    }

    #[test]
    fn test_error_unexpected_token() {
        // Parser error: a bare literal is not a valid top-level decl
        let msg = parse_err("42");
        assert!(msg.contains("expected top-level declaration"), "got: {msg}");
    }

    #[test]
    fn test_try_expr() {
        let program =
            parse("fn f() -> Result<i32, str> { let x: i32 = try parse(\"42\"); Result::Ok(x) }");
        match &program.decls[0] {
            TopDecl::Fn(f) => {
                assert!(!f.body.stmts.is_empty());
                match &f.body.stmts[0] {
                    Stmt::Let(l) => {
                        assert!(matches!(&l.value, Expr::Try { .. }));
                    }
                    other => panic!("expected let with try, got {:?}", other),
                }
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn test_array_literal() {
        let program = parse("fn f() -> [i32] { [1, 2, 3] }");
        match &program.decls[0] {
            TopDecl::Fn(f) => {
                let tail = f.body.tail_expr.as_ref().unwrap();
                match tail.as_ref() {
                    Expr::ArrayLit { elements, .. } => {
                        assert_eq!(elements.len(), 3);
                    }
                    other => panic!("expected array literal, got {:?}", other),
                }
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn test_cast_expr() {
        let program = parse("fn f() -> f64 { 42 as f64 }");
        match &program.decls[0] {
            TopDecl::Fn(f) => {
                let tail = f.body.tail_expr.as_ref().unwrap();
                assert!(matches!(tail.as_ref(), Expr::Cast { .. }));
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn test_interpolated_string_expr() {
        let program = parse(r#"fn! main() { println("hello {name}!"); }"#);
        match &program.decls[0] {
            TopDecl::Fn(f) => match &f.body.stmts[0] {
                Stmt::Expr(expr_stmt) => match &expr_stmt.expr {
                    Expr::Call { args, .. } => match &args[0] {
                        Expr::InterpolatedString { parts, .. } => {
                            assert_eq!(parts.len(), 3);
                            assert!(
                                matches!(&parts[0], InterpolatedStringPart::Text(text) if text == "hello ")
                            );
                            assert!(
                                matches!(&parts[1], InterpolatedStringPart::Expr(Expr::Ident(name, _)) if name == "name")
                            );
                            assert!(
                                matches!(&parts[2], InterpolatedStringPart::Text(text) if text == "!")
                            );
                        }
                        other => panic!("expected interpolated string, got {:?}", other),
                    },
                    other => panic!("expected call, got {:?}", other),
                },
                other => panic!("expected expr stmt, got {:?}", other),
            },
            _ => panic!("expected fn"),
        }
    }
}
