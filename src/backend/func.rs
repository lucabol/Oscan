//! Translates one `ir::FnDef` body into a Cranelift `Function`.
//!
//! # Value representation (see `layout.rs` for the full rationale)
//!
//! Every Oscan value flowing through this translator is `Option<Value>`:
//! `None` means `Unit` (no value at all — Cranelift calls/blocks with zero
//! results); `Some(v)` is the type's single Cranelift SSA value, per
//! [`Repr::of`]: a direct scalar for `i32`/`i64`/`f64`/`bool`/payload-less
//! enums, or a pointer for everything else. Pointers come in two flavours
//! that matter when reading/writing through them:
//!
//! * **Inline aggregates** (`str`, `struct`, payload-bearing `enum`,
//!   `Result`) — the pointer addresses a memory block laid out by
//!   `layout.rs`; accessing a *field* of inline-aggregate type is pure
//!   address arithmetic (no load), because the field's bytes are the
//!   aggregate embedded in place, exactly like a C struct field.
//! * **Opaque pointers** (`Array`/`FixedArray`/`Map*`/`Handle`/`FnPtr`) —
//!   the pointer *is* the value (an `osc_array*` etc.); accessing a field
//!   of this type is a real load/store of the 8-byte pointer.
//!
//! Inline-aggregate pointers always address arena memory (a Cranelift
//! stack slot would dangle the moment the owning function returns), so
//! every aggregate-producing expression (`StructLit`, `EnumConstructor`,
//! `Result::Ok`/`Err`, string/array-of-aggregate reads that must copy out,
//! ...) allocates via `osc_arena_alloc`. To keep true copy-on-bind value
//! semantics (`let b = a;` must not let a later `a.field = x` observably
//! change `b`) while avoiding redundant copies of values that are already
//! freshly allocated, [`FuncTranslator::bind_value`] only re-materializes
//! (fresh alloc + byte copy) inline-aggregate values whose source
//! expression *isn't* itself a `StructLit`/`EnumConstructor`/`ArrayLit`
//! (see that method's doc comment for the full argument). Every other
//! place an inline-aggregate value crosses into a new, independently-named
//! binding applies that same copy, matching what a real C ABI/compiler
//! gives for free: a by-value function parameter (`translate_function`,
//! mirroring a C callee's own copy of its by-value argument), a
//! function's own implicit tail-expression `return` (`translate_function`,
//! matching the explicit-`return` case's `bind_value` call exactly, since
//! an implicit return is otherwise indistinguishable from one that
//! happened to omit the `return` keyword), and a `match` arm's
//! identifier/wildcard-catch-all or enum/`Result` payload binding
//! (`bind_pattern`, mirroring `src/codegen.rs`'s `T name = scrutinee...;`).
//! Skipping any of these would let the *caller's* (or scrutinee's/
//! payload's) still-mutable backing storage retroactively change an
//! already-bound name after the fact — see the differential regressions
//! in `tests/positive/param_tail_return_copy_semantics.osc` and
//! `tests/positive/match_binding_copy_semantics.osc`.

use std::collections::HashMap;

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    AbiParam, Function, InstBuilder, MemFlagsData, Signature, StackSlotData, StackSlotKind,
    UserFuncName, Value,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{FuncId, Linkage, Module};

use crate::ast::{BinOp, UnaryOp};
use crate::error::CompileError;
use crate::ir as oir;
use crate::token::Span;
use crate::types::BcType;

use super::ctx::{BackendContext, ExternDeclKind};
use super::extern_shim::{self, NativeExternAbi};
use super::layout::{
    self, cl_pointer_type, enum_layout, layout_of, result_layout, struct_field_offset, Repr,
};

type CResult<T> = Result<T, CompileError>;
type CBlock = cranelift_codegen::ir::Block;

fn unsupported(span: Span, what: impl std::fmt::Display) -> CompileError {
    CompileError::new(
        span,
        format!(
            "native backend: {what} is not supported (use --backend c when portable C lowering supports this construct)"
        ),
    )
}

/// Build the `&[BlockArg]` a `jump`/`brif` call needs from our
/// `Option<Value>` convention (`None`/`Unit` contributes no block args).
fn block_args(value: Option<Value>) -> Vec<cranelift_codegen::ir::BlockArg> {
    value
        .into_iter()
        .map(cranelift_codegen::ir::BlockArg::Value)
        .collect()
}

/// The flags used for every plain scalar/pointer load and store this
/// backend emits. Not `trusted()`: Oscan values can indeed be read at
/// attacker/user-influenced offsets (e.g. array/string indexing), which
/// osc_array/osc_str bounds-check at the *runtime call* level (see
/// `osc_array_get`/`osc_str_check_index`) before a raw pointer ever
/// reaches one of these loads/stores, but a plain `MemFlagsData::new()`
/// (no `notrap`/`aligned` assumptions) is the conservative, always-correct
/// choice for a first implementation.
pub(super) fn mem_flags() -> MemFlagsData {
    MemFlagsData::new()
}

/// Whether `ty` is an inline aggregate (embedded-by-value pointee) as
/// opposed to an opaque runtime pointer. See module docs.
fn is_inline_aggregate(ty: &BcType, program: &oir::Program) -> bool {
    match ty {
        BcType::Str | BcType::Struct(_) | BcType::Result(_, _) => true,
        BcType::Enum(name) => layout::enum_has_payload(name, program),
        _ => false,
    }
}

/// A single Oscan-level binding: its Cranelift `Variable` and static type.
#[derive(Clone)]
struct Binding {
    var: Variable,
    ty: BcType,
}

struct LoopTargets {
    continue_block: CBlock,
    break_block: CBlock,
}

pub struct FuncTranslator<'a, 'b> {
    ctx: &'a mut BackendContext<'b>,
    builder: FunctionBuilder<'a>,
    scopes: Vec<HashMap<String, Binding>>,
    loops: Vec<LoopTargets>,
    /// Deferred expressions for the *current* function, innermost-last;
    /// replayed (in reverse) at every `return` and at the implicit
    /// fall-through end of the function body, mirroring `src/codegen.rs`'s
    /// `deferred_exprs` handling exactly.
    defers: Vec<&'a oir::Expr>,
    fn_return_ty: BcType,
    arena_value: Value,
    /// Whether the current Cranelift block already ends in a terminator
    /// (`jump`/`brif`/`return`). Tracked ourselves rather than via
    /// `FunctionBuilder::is_unreachable` — that method answers a
    /// different question ("does this block have zero predecessors and
    /// is it sealed", i.e. genuinely dead code), not "did I already emit
    /// a terminator into the block I'm currently appending to" (which is
    /// what deciding whether a structured-control-flow construct needs
    /// its own fallthrough jump after lowering a nested block/expression
    /// that might itself end in `break`/`continue`/`return` requires).
    /// Reset to `false` by [`Self::goto`] (our `switch_to_block` wrapper)
    /// and set to `true` by every terminator-emitting helper.
    terminated: bool,
}

/// Build the Cranelift `Signature` for a user-defined (non-extern) Oscan
/// function: an implicit leading `osc_arena*` parameter, then each
/// declared parameter, then zero or one return value per [`Repr::of`].
fn oscan_fn_signature(
    module: &impl Module,
    program: &oir::Program,
    params: &[(String, BcType)],
    return_type: &BcType,
) -> Signature {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(cl_pointer_type()));
    for (_, ty) in params {
        if let Some(t) = Repr::of(ty, program).cl_type() {
            sig.params.push(AbiParam::new(t));
        }
    }
    if let Some(t) = Repr::of(return_type, program).cl_type() {
        sig.returns.push(AbiParam::new(t));
    }
    sig
}

/// Build the Cranelift `Signature` for a real C-ABI extern function with no
/// implicit arena and no generated shim. Signatures containing `str` use
/// `extern_shim_signature` instead so the C compiler, not Cranelift, lowers
/// the by-value `osc_str` ABI.
fn direct_extern_fn_signature(
    module: &impl Module,
    program: &oir::Program,
    name: &str,
    params: &[(String, BcType)],
    return_type: &BcType,
    span: Span,
) -> CResult<Signature> {
    let mut sig = module.make_signature();
    for (_, ty) in params {
        if is_inline_aggregate(ty, program) {
            return Err(unsupported(
                span,
                format!("extern function '{name}' parameter of type '{ty}'"),
            ));
        }
        if let Some(t) = Repr::of(ty, program).cl_type() {
            sig.params.push(AbiParam::new(t));
        }
    }
    if is_inline_aggregate(return_type, program) {
        return Err(unsupported(
            span,
            format!("extern function '{name}' return type '{return_type}'"),
        ));
    }
    if let Some(t) = Repr::of(return_type, program).cl_type() {
        sig.returns.push(AbiParam::new(t));
    }
    Ok(sig)
}

fn extern_shim_signature(
    module: &impl Module,
    program: &oir::Program,
    params: &[(String, BcType)],
    return_type: &BcType,
) -> Signature {
    let mut sig = module.make_signature();
    if *return_type == BcType::Str {
        sig.params.push(AbiParam::new(cl_pointer_type()));
    }
    for (_, ty) in params {
        if *ty == BcType::Str {
            sig.params.push(AbiParam::new(cl_pointer_type()));
        } else if let Some(t) = Repr::of(ty, program).cl_type() {
            sig.params.push(AbiParam::new(t));
        }
    }
    if *return_type != BcType::Str {
        if let Some(t) = Repr::of(return_type, program).cl_type() {
            sig.returns.push(AbiParam::new(t));
        }
    }
    sig
}

/// Declare every user function up front (so forward references — calling
/// a function declared later in the source file — resolve correctly
/// regardless of translation order), then translate each user function
/// body. Mirrors the two-pass structure `src/codegen.rs` gets "for free"
/// from C's own forward declarations.
///
/// `extern` block functions are deliberately *not* declared here — see
/// `FuncTranslator::resolve_extern`'s docs for why they're resolved
/// lazily, on first actual call, instead.
pub fn declare_and_translate_all(ctx: &mut BackendContext) -> CResult<()> {
    for f in &ctx.program.fn_defs {
        let sig = oscan_fn_signature(&ctx.module, ctx.program, &f.params, &f.return_type);
        let symbol = BackendContext::user_fn_symbol(&f.name);
        let id = ctx
            .module
            .declare_function(&symbol, Linkage::Export, &sig)
            .map_err(|e| {
                CompileError::new(
                    f.span,
                    format!("internal error declaring function '{}': {e}", f.name),
                )
            })?;
        ctx.functions.insert(f.name.clone(), id);
    }

    for f in &ctx.program.fn_defs {
        let func_id = ctx.functions[&f.name];
        translate_function(ctx, f, func_id)?;
    }
    Ok(())
}

fn translate_function(ctx: &mut BackendContext, f: &oir::FnDef, func_id: FuncId) -> CResult<()> {
    let sig = oscan_fn_signature(&ctx.module, ctx.program, &f.params, &f.return_type);
    let symbol = BackendContext::user_fn_symbol(&f.name);
    let mut func = Function::with_name_signature(UserFuncName::user(0, func_id.as_u32()), sig);
    let mut fb_ctx = FunctionBuilderContext::new();

    {
        let mut builder = FunctionBuilder::new(&mut func, &mut fb_ctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        let arena_value = builder.block_params(entry)[0];

        let mut translator = FuncTranslator {
            ctx,
            builder,
            scopes: vec![HashMap::new()],
            loops: Vec::new(),
            defers: Vec::new(),
            fn_return_ty: f.return_type.clone(),
            arena_value,
            terminated: false,
        };

        // Bind parameters: block param index 0 is `_arena`, then one
        // Cranelift block param per declared parameter that isn't Unit.
        let mut next_block_param = 1;
        for (name, ty) in &f.params {
            let repr = Repr::of(ty, translator.ctx.program);
            let var = translator.fresh_var(repr.cl_type());
            if let Some(_t) = repr.cl_type() {
                let bp = translator.builder.block_params(entry)[next_block_param];
                // A by-value inline-aggregate parameter is, at the ABI
                // level, just the caller's own pointer (see module docs):
                // materialize an owned copy here, exactly like a real C
                // callee's own copy of a by-value struct argument, so a
                // later mutation of the *caller's* source (which the
                // callee cannot itself perform — parameters are always
                // immutable bindings — but which the caller can perform
                // once the call returns, e.g. via a bare tail-returned
                // alias) can never retroactively change this binding.
                let value = if is_inline_aggregate(ty, translator.ctx.program) {
                    translator.materialize_owned(ty, bp)
                } else {
                    bp
                };
                translator.builder.def_var(var, value);
                next_block_param += 1;
            }
            translator.scopes.last_mut().unwrap().insert(
                name.clone(),
                Binding {
                    var,
                    ty: ty.clone(),
                },
            );
        }

        let body_val = translator.lower_function_body(&f.body)?;
        if !translator.builder.is_unreachable() {
            // An implicit (bare tail-expression) `return` must copy an
            // inline-aggregate result exactly like the explicit
            // `return expr;` case does (see `Stmt::Return` below) —
            // otherwise a function whose body happens to omit the
            // `return` keyword would silently skip the copy an
            // otherwise-identical explicit `return` performs, letting a
            // returned parameter/local keep aliasing mutable storage the
            // caller can change after the call returns.
            let body_val = match f.body.tail_expr.as_deref() {
                Some(tail) => translator.bind_value(&f.return_type, body_val, tail)?,
                None => body_val,
            };
            translator.emit_return(body_val)?;
        }

        translator.builder.finalize();
    }

    let mut ctx_obj = ctx.module.make_context();
    ctx_obj.func = func;
    ctx.module
        .define_function(func_id, &mut ctx_obj)
        .map_err(|e| {
            CompileError::new(
                f.span,
                format!("internal error compiling function '{symbol}': {e}"),
            )
        })?;
    Ok(())
}

impl<'a, 'b> FuncTranslator<'a, 'b> {
    fn fresh_var(&mut self, ty: Option<cranelift_codegen::ir::Type>) -> Variable {
        // Every Variable needs *some* declared type even when it will
        // never be def_var'd/use_var'd (Unit bindings): use I8 as an
        // unobserved placeholder (see module docs on `Option<Value>`).
        self.builder
            .declare_var(ty.unwrap_or(cranelift_codegen::ir::types::I8))
    }

    /// `switch_to_block` wrapper that also resets `self.terminated` — see
    /// that field's doc comment for why this (not
    /// `FunctionBuilder::is_unreachable`) is what tracks whether a
    /// fallthrough jump is still needed after lowering a nested
    /// block/expression.
    fn goto(&mut self, block: CBlock) {
        self.builder.switch_to_block(block);
        self.terminated = false;
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn lookup(&self, name: &str) -> Option<Binding> {
        for scope in self.scopes.iter().rev() {
            if let Some(b) = scope.get(name) {
                return Some(b.clone());
            }
        }
        None
    }

    fn program(&self) -> &'b oir::Program {
        self.ctx.program
    }

    // -----------------------------------------------------------------
    // Blocks / statements
    // -----------------------------------------------------------------

    fn lower_block(&mut self, block: &'a oir::Block) -> CResult<Option<Value>> {
        self.push_scope();
        for stmt in &block.stmts {
            self.lower_stmt(stmt)?;
            if self.terminated {
                break;
            }
        }
        let result = if !self.terminated {
            match &block.tail_expr {
                Some(tail) => self.lower_expr(tail)?,
                None => None,
            }
        } else {
            None
        };
        self.pop_scope();
        Ok(result)
    }

    fn lower_function_body(&mut self, block: &'a oir::Block) -> CResult<Option<Value>> {
        for stmt in &block.stmts {
            self.lower_stmt(stmt)?;
            if self.terminated {
                break;
            }
        }
        if !self.terminated {
            match &block.tail_expr {
                Some(tail) => self.lower_expr(tail),
                None => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    fn lower_stmt(&mut self, stmt: &'a oir::Stmt) -> CResult<()> {
        match stmt {
            oir::Stmt::Let(ls) => {
                let raw = self.lower_expr(&ls.value)?;
                let value = self.bind_value(&ls.ty, raw, &ls.value)?;
                let repr = Repr::of(&ls.ty, self.program());
                let var = self.fresh_var(repr.cl_type());
                if let Some(v) = value {
                    self.builder.def_var(var, v);
                }
                self.scopes.last_mut().unwrap().insert(
                    ls.name.clone(),
                    Binding {
                        var,
                        ty: ls.ty.clone(),
                    },
                );
                Ok(())
            }
            oir::Stmt::Assign(a) => {
                let raw = self.lower_expr(&a.value)?;
                if a.target.accessors.is_empty() {
                    let binding = self.lookup(&a.target.name).unwrap_or_else(|| {
                        panic!("internal error: assignment to unknown '{}'", a.target.name)
                    });
                    let value = self.bind_value(&binding.ty, raw, &a.value)?;
                    if let Some(v) = value {
                        self.builder.def_var(binding.var, v);
                    }
                } else {
                    self.store_place(&a.target, raw, &a.value)?;
                }
                Ok(())
            }
            oir::Stmt::CompoundAssign(ca) => {
                let place_expr = self.place_as_expr(&ca.target);
                let current = self.lower_expr(place_expr)?;
                let rhs = self.lower_expr(&ca.value)?;
                let final_ty = self.place_final_type(&ca.target)?;
                let combined = self.lower_binop(ca.op, current, rhs, &final_ty, ca.span)?;
                if ca.target.accessors.is_empty() {
                    let binding = self.lookup(&ca.target.name).unwrap();
                    if let Some(v) = combined {
                        self.builder.def_var(binding.var, v);
                    }
                } else {
                    self.store_place_value(&ca.target, combined)?;
                }
                Ok(())
            }
            oir::Stmt::Expr(es) => {
                self.lower_expr(&es.expr)?;
                Ok(())
            }
            oir::Stmt::While(w) => self.lower_while(w),
            oir::Stmt::For(f) => self.lower_for(f),
            oir::Stmt::ForIn(fi) => self.lower_for_in(fi),
            oir::Stmt::Return(r) => {
                let value = match &r.value {
                    Some(v) => {
                        let raw = self.lower_expr(v)?;
                        self.bind_value(&self.fn_return_ty.clone(), raw, v)?
                    }
                    None => None,
                };
                self.emit_return(value)
            }
            oir::Stmt::Defer(d) => {
                self.defers.push(&d.expr);
                Ok(())
            }
            oir::Stmt::Break(span) => {
                let target = self
                    .loops
                    .last()
                    .map(|l| l.break_block)
                    .ok_or_else(|| unsupported(*span, "'break' outside of a loop"))?;
                self.builder.ins().jump(target, &[]);
                self.terminated = true;
                Ok(())
            }
            oir::Stmt::Continue(span) => {
                let target = self
                    .loops
                    .last()
                    .map(|l| l.continue_block)
                    .ok_or_else(|| unsupported(*span, "'continue' outside of a loop"))?;
                self.builder.ins().jump(target, &[]);
                self.terminated = true;
                Ok(())
            }
        }
    }

    /// Emit every deferred expression (LIFO) and a `return`, exactly like
    /// `src/codegen.rs`'s `emit_deferred_before_return`.
    fn emit_return(&mut self, value: Option<Value>) -> CResult<()> {
        for expr in self.defers.clone().into_iter().rev() {
            self.lower_expr(expr)?;
        }
        match value {
            Some(v) => {
                self.builder.ins().return_(&[v]);
            }
            None => {
                self.builder.ins().return_(&[]);
            }
        }
        self.terminated = true;
        Ok(())
    }

    /// Copy-on-bind semantics for a value about to be stored into a new
    /// (or reassigned) binding: inline-aggregate values are re-materialized
    /// (fresh arena block + byte copy) unless `source` syntactically
    /// *just* constructed a fresh value (`StructLit`/`EnumConstructor`/
    /// `ArrayLit`), in which case the pointer is already exclusively owned
    /// and copying would be a pure waste. See module docs for why this is
    /// sound: every other place that could introduce aliasing (function
    /// parameters, `return`) either shares immutable data or is itself
    /// guarded by the *next* binding's copy.
    fn bind_value(
        &mut self,
        ty: &BcType,
        value: Option<Value>,
        source: &oir::Expr,
    ) -> CResult<Option<Value>> {
        let Some(v) = value else { return Ok(None) };
        if !is_inline_aggregate(ty, self.program()) {
            return Ok(Some(v));
        }
        if matches!(
            source,
            oir::Expr::StructLit { .. } | oir::Expr::EnumConstructor { .. }
        ) {
            return Ok(Some(v));
        }
        Ok(Some(self.materialize_owned(ty, v)))
    }

    fn materialize_owned(&mut self, ty: &BcType, src_ptr: Value) -> Value {
        let layout = layout_of(ty, self.program());
        let dest_ptr = self.arena_alloc(layout.size);
        self.copy_bytes(dest_ptr, 0, src_ptr, 0, layout.size, layout.align);
        dest_ptr
    }

    // -----------------------------------------------------------------
    // Control flow
    // -----------------------------------------------------------------

    fn lower_while(&mut self, w: &'a oir::WhileStmt) -> CResult<()> {
        let header = self.builder.create_block();
        let body = self.builder.create_block();
        let exit = self.builder.create_block();

        self.builder.ins().jump(header, &[]);
        self.goto(header);
        let cond = self.lower_expr(&w.condition)?.expect("bool condition");
        self.builder.ins().brif(cond, body, &[], exit, &[]);

        self.goto(body);
        self.loops.push(LoopTargets {
            continue_block: header,
            break_block: exit,
        });
        self.lower_block(&w.body)?;
        self.loops.pop();
        if !self.terminated {
            self.builder.ins().jump(header, &[]);
        }
        self.builder.seal_block(header);
        self.builder.seal_block(body);

        self.goto(exit);
        self.builder.seal_block(exit);
        Ok(())
    }

    fn lower_for(&mut self, f: &'a oir::ForStmt) -> CResult<()> {
        let start = self.lower_expr(&f.start)?.expect("i32 start");
        let end = self.lower_expr(&f.end)?.expect("i32 end");

        let header = self.builder.create_block();
        let body = self.builder.create_block();
        let incr = self.builder.create_block();
        let exit = self.builder.create_block();

        let ivar = self.fresh_var(Some(cranelift_codegen::ir::types::I32));
        self.builder.def_var(ivar, start);
        self.builder.ins().jump(header, &[]);

        self.goto(header);
        let i = self.builder.use_var(ivar);
        let cond = self.builder.ins().icmp(IntCC::SignedLessThan, i, end);
        self.builder.ins().brif(cond, body, &[], exit, &[]);

        self.goto(body);
        self.push_scope();
        self.scopes.last_mut().unwrap().insert(
            f.var.clone(),
            Binding {
                var: ivar,
                ty: BcType::I32,
            },
        );
        self.loops.push(LoopTargets {
            continue_block: incr,
            break_block: exit,
        });
        self.lower_block_no_scope(&f.body)?;
        self.loops.pop();
        self.pop_scope();
        if !self.terminated {
            self.builder.ins().jump(incr, &[]);
        }

        self.goto(incr);
        let i2 = self.builder.use_var(ivar);
        let one = self
            .builder
            .ins()
            .iconst(cranelift_codegen::ir::types::I32, 1);
        let next = self.builder.ins().iadd(i2, one);
        self.builder.def_var(ivar, next);
        self.builder.ins().jump(header, &[]);
        self.builder.seal_block(header);
        self.builder.seal_block(body);
        self.builder.seal_block(incr);

        self.goto(exit);
        self.builder.seal_block(exit);
        Ok(())
    }

    fn lower_for_in(&mut self, fi: &'a oir::ForInStmt) -> CResult<()> {
        let arr_ty = fi.iterable.ty();
        let elem_ty = match &arr_ty {
            BcType::FixedArray(e, _) | BcType::Array(e) => (**e).clone(),
            other => {
                return Err(unsupported(
                    fi.span,
                    format!("for-in over non-array type '{other}'"),
                ))
            }
        };
        let arr_ptr = self.lower_expr(&fi.iterable)?.expect("array pointer");
        let len = self.array_len(arr_ptr);

        let header = self.builder.create_block();
        let body = self.builder.create_block();
        let incr = self.builder.create_block();
        let exit = self.builder.create_block();

        let ivar = self.fresh_var(Some(cranelift_codegen::ir::types::I32));
        let zero = self
            .builder
            .ins()
            .iconst(cranelift_codegen::ir::types::I32, 0);
        self.builder.def_var(ivar, zero);
        self.builder.ins().jump(header, &[]);

        self.goto(header);
        let i = self.builder.use_var(ivar);
        let cond = self.builder.ins().icmp(IntCC::SignedLessThan, i, len);
        self.builder.ins().brif(cond, body, &[], exit, &[]);

        self.goto(body);
        let elem_val = self.array_read(arr_ptr, i, &elem_ty);
        // Materialize an owned copy for inline-aggregate elements (str,
        // struct, payload-enum, Result) *before* binding the loop
        // variable: `array_read` (via `read_at`) hands back the address
        // *inside the array's own backing storage* for these types (see
        // that function's docs — field/element access of an inline
        // aggregate is pure address arithmetic, no load), so binding it
        // directly would alias the loop variable to the array's storage
        // instead of copying it, unlike the C oracle's
        // `const T x = arr->data[i];`, which is a real value copy (C
        // struct assignment always copies). Without this, mutating the
        // array during iteration (`arr[i] = ...`, `push`/`pop`, ...)
        // would retroactively change what the *already-bound* loop
        // variable reads, and a `push`-triggered reallocation could even
        // leave it pointing at memory the array has moved away from.
        // Scalars/opaque pointers (Array/Map/Handle/FnPtr) are unaffected
        // — those are already real values, never aliased storage.
        let elem_val = match elem_val {
            Some(v) if is_inline_aggregate(&elem_ty, self.program()) => {
                Some(self.materialize_owned(&elem_ty, v))
            }
            other => other,
        };
        self.push_scope();
        let evar = self.fresh_var(Repr::of(&elem_ty, self.program()).cl_type());
        if let Some(v) = elem_val {
            self.builder.def_var(evar, v);
        }
        self.scopes.last_mut().unwrap().insert(
            fi.var.clone(),
            Binding {
                var: evar,
                ty: elem_ty,
            },
        );
        self.loops.push(LoopTargets {
            continue_block: incr,
            break_block: exit,
        });
        self.lower_block_no_scope(&fi.body)?;
        self.loops.pop();
        self.pop_scope();
        if !self.terminated {
            self.builder.ins().jump(incr, &[]);
        }

        self.goto(incr);
        let i2 = self.builder.use_var(ivar);
        let one = self
            .builder
            .ins()
            .iconst(cranelift_codegen::ir::types::I32, 1);
        let next = self.builder.ins().iadd(i2, one);
        self.builder.def_var(ivar, next);
        self.builder.ins().jump(header, &[]);
        self.builder.seal_block(header);
        self.builder.seal_block(body);
        self.builder.seal_block(incr);

        self.goto(exit);
        self.builder.seal_block(exit);
        Ok(())
    }

    /// Like `lower_block`, but does not push/pop its own scope (used by
    /// loop bodies, whose loop-variable binding must stay in scope
    /// alongside the block's own locals in one frame that the loop
    /// construct itself pushes/pops).
    fn lower_block_no_scope(&mut self, block: &'a oir::Block) -> CResult<Option<Value>> {
        for stmt in &block.stmts {
            self.lower_stmt(stmt)?;
            if self.terminated {
                return Ok(None);
            }
        }
        match &block.tail_expr {
            Some(tail) => self.lower_expr(tail),
            None => Ok(None),
        }
    }
}

/// Core scalar/control-flow/call expression lowering.
impl<'a, 'b> FuncTranslator<'a, 'b> {
    fn lower_expr(&mut self, expr: &'a oir::Expr) -> CResult<Option<Value>> {
        use cranelift_codegen::ir::types;
        match expr {
            oir::Expr::IntLit(v, _) => Ok(Some(self.builder.ins().iconst(types::I32, *v))),
            oir::Expr::FloatLit(v, _) => {
                Ok(Some(self.builder.ins().f64const(
                    cranelift_codegen::ir::immediates::Ieee64::with_float(*v),
                )))
            }
            oir::Expr::BoolLit(b, _) => Ok(Some(
                self.builder.ins().iconst(types::I8, if *b { 1 } else { 0 }),
            )),
            oir::Expr::StringLit(s, _) => Ok(Some(self.string_literal_ptr(s))),
            oir::Expr::InterpolatedString { parts, span } => {
                self.lower_interpolated_string(parts, *span)
            }

            oir::Expr::Ident {
                name,
                kind,
                ty,
                span,
            } => self.lower_ident(name, *kind, ty, *span),

            oir::Expr::BinaryOp {
                op, left, right, ..
            } if matches!(op, BinOp::And | BinOp::Or) => self.lower_short_circuit(*op, left, right),
            oir::Expr::BinaryOp {
                op,
                left,
                right,
                span,
                ..
            } => {
                let lv = self.lower_expr(left)?;
                let rv = self.lower_expr(right)?;
                self.lower_binop(*op, lv, rv, &left.ty(), *span)
            }
            oir::Expr::UnaryOp {
                op, operand, ty, ..
            } => {
                let v = self
                    .lower_expr(operand)?
                    .expect("unary operand has a value");
                match op {
                    UnaryOp::Not => Ok(Some(self.builder.ins().bxor_imm(v, 1))),
                    UnaryOp::Neg => match ty {
                        BcType::I32 => Ok(Some(self.call_runtime_scalar(
                            "osc_neg_i32",
                            &[v],
                            types::I32,
                        ))),
                        BcType::I64 => Ok(Some(self.call_runtime_scalar(
                            "osc_neg_i64",
                            &[v],
                            types::I64,
                        ))),
                        BcType::F64 => Ok(Some(self.builder.ins().fneg(v))),
                        other => Err(unsupported(
                            operand.span(),
                            format!("unary '-' on type '{other}'"),
                        )),
                    },
                }
            }
            oir::Expr::Cast {
                expr: inner,
                to_ty,
                span,
            } => {
                let v = self.lower_expr(inner)?.expect("cast operand has a value");
                self.lower_cast(v, &inner.ty(), to_ty, *span)
            }
            oir::Expr::Call {
                callee,
                args,
                ty,
                span,
            } => self.lower_call(callee, args, ty, *span),

            oir::Expr::Block(block) => self.lower_block(block),
            oir::Expr::If {
                condition,
                then_block,
                else_branch,
                ty,
                ..
            } => self.lower_if(condition, then_block, else_branch.as_deref(), ty),
            oir::Expr::Arena { body, .. } => self.lower_arena(body),

            oir::Expr::FieldAccess {
                expr: obj,
                field,
                ty,
                span,
            } => self.lower_field_access(obj, field, ty, *span),
            oir::Expr::Index {
                expr: arr,
                index,
                ty,
                span,
            } => self.lower_index(arr, index, ty, *span),
            oir::Expr::ArrayLit {
                elements,
                elem_ty,
                span,
                ..
            } => self.lower_array_lit(elements, elem_ty, *span),
            oir::Expr::StructLit { name, fields, .. } => self.lower_struct_lit(name, fields),
            oir::Expr::EnumConstructor {
                enum_name,
                variant,
                args,
                ty,
                span,
            } => self.lower_enum_constructor(enum_name, variant, args, ty, *span),
            oir::Expr::Match {
                scrutinee,
                arms,
                ty,
                span,
            } => self.lower_match(scrutinee, arms, ty, *span),
            oir::Expr::Try { call, ty, span } => self.lower_try(call, ty, *span),
        }
    }

    fn lower_ident(
        &mut self,
        name: &str,
        kind: oir::IdentKind,
        ty: &BcType,
        span: Span,
    ) -> CResult<Option<Value>> {
        match kind {
            oir::IdentKind::FnRef => {
                let func_id = *self.ctx.functions.get(name).ok_or_else(|| {
                    unsupported(span, format!("reference to function '{name}' as a value"))
                })?;
                let func_ref = self
                    .ctx
                    .module
                    .declare_func_in_func(func_id, self.builder.func);
                Ok(Some(
                    self.builder.ins().func_addr(cl_pointer_type(), func_ref),
                ))
            }
            oir::IdentKind::Value => {
                if matches!(ty, BcType::Unit) {
                    return Ok(None);
                }
                if let Some(binding) = self.lookup(name) {
                    return Ok(Some(self.builder.use_var(binding.var)));
                }
                // Top-level constant (not a local/param): materialize it as
                // module data, matching `src/codegen.rs`'s
                // `emit_top_level_constants` (`static const ...`).
                let value = self.top_level_const_ptr(name, ty)?;
                Ok(Some(value))
            }
        }
    }

    fn lower_cast(
        &mut self,
        v: Value,
        from: &BcType,
        to: &BcType,
        span: Span,
    ) -> CResult<Option<Value>> {
        use cranelift_codegen::ir::types;
        let out = match (from, to) {
            (BcType::I32, BcType::I64) => {
                self.call_runtime_scalar("osc_i32_to_i64", &[v], types::I64)
            }
            (BcType::I64, BcType::I32) => {
                self.call_runtime_scalar("osc_i64_to_i32", &[v], types::I32)
            }
            (BcType::I32, BcType::F64) => {
                self.call_runtime_scalar("osc_i32_to_f64", &[v], types::F64)
            }
            (BcType::I64, BcType::F64) => {
                self.call_runtime_scalar("osc_i64_to_f64", &[v], types::F64)
            }
            (BcType::F64, BcType::I32) => {
                self.call_runtime_scalar("osc_f64_to_i32", &[v], types::I32)
            }
            (BcType::F64, BcType::I64) => {
                self.call_runtime_scalar("osc_f64_to_i64", &[v], types::I64)
            }
            (BcType::Handle, BcType::I64) | (BcType::I64, BcType::Handle) => v,
            (a, b) if a == b => v,
            (a, b) => return Err(unsupported(span, format!("cast from '{a}' to '{b}'"))),
        };
        Ok(Some(out))
    }

    /// Call a runtime function with purely scalar/pointer parameters and a
    /// single scalar/pointer return, declaring it on first use. This is
    /// the direct (no-shim) path used for arithmetic/math/cast/etc. helpers
    /// whose C signature never involves an aggregate-by-value type.
    fn call_runtime_scalar(
        &mut self,
        symbol: &'static str,
        args: &[Value],
        ret: cranelift_codegen::ir::Type,
    ) -> Value {
        let arg_types: Vec<_> = args
            .iter()
            .map(|v| self.builder.func.dfg.value_type(*v))
            .collect();
        let func_id = self.ctx.runtime_func(symbol, |sig| {
            for t in &arg_types {
                sig.params.push(AbiParam::new(*t));
            }
            sig.returns.push(AbiParam::new(ret));
        });
        let func_ref = self
            .ctx
            .module
            .declare_func_in_func(func_id, self.builder.func);
        let call = self.builder.ins().call(func_ref, args);
        self.builder.inst_results(call)[0]
    }

    /// Like `call_runtime_scalar`, but for a `void`-returning runtime call.
    fn call_runtime_void(&mut self, symbol: &'static str, args: &[Value]) {
        let arg_types: Vec<_> = args
            .iter()
            .map(|v| self.builder.func.dfg.value_type(*v))
            .collect();
        let func_id = self.ctx.runtime_func(symbol, |sig| {
            for t in &arg_types {
                sig.params.push(AbiParam::new(*t));
            }
        });
        let func_ref = self
            .ctx
            .module
            .declare_func_in_func(func_id, self.builder.func);
        self.builder.ins().call(func_ref, args);
    }

    fn lower_if(
        &mut self,
        condition: &'a oir::Expr,
        then_block: &'a oir::Block,
        else_branch: Option<&'a oir::Expr>,
        ty: &BcType,
    ) -> CResult<Option<Value>> {
        let cond = self
            .lower_expr(condition)?
            .expect("if condition has a value");
        let then_blk = self.builder.create_block();
        let merge_blk = self.builder.create_block();
        let repr_ty = Repr::of(ty, self.program()).cl_type();
        if let Some(t) = repr_ty {
            self.builder.append_block_param(merge_blk, t);
        }

        match else_branch {
            Some(else_expr) => {
                let else_blk = self.builder.create_block();
                self.builder.ins().brif(cond, then_blk, &[], else_blk, &[]);

                self.goto(then_blk);
                self.builder.seal_block(then_blk);
                let then_val = self.lower_block(then_block)?;
                if !self.terminated {
                    let args = block_args(then_val);
                    self.builder.ins().jump(merge_blk, &args);
                }

                self.goto(else_blk);
                self.builder.seal_block(else_blk);
                let else_val = self.lower_expr(else_expr)?;
                if !self.terminated {
                    let args = block_args(else_val);
                    self.builder.ins().jump(merge_blk, &args);
                }
            }
            None => {
                self.builder.ins().brif(cond, then_blk, &[], merge_blk, &[]);
                self.goto(then_blk);
                self.builder.seal_block(then_blk);
                self.lower_block(then_block)?;
                if !self.terminated {
                    self.builder.ins().jump(merge_blk, &[]);
                }
            }
        }

        self.builder.seal_block(merge_blk);
        self.goto(merge_blk);
        if repr_ty.is_some() {
            Ok(Some(self.builder.block_params(merge_blk)[0]))
        } else {
            Ok(None)
        }
    }

    fn lower_arena(&mut self, body: &'a oir::Block) -> CResult<Option<Value>> {
        // A nested arena scope: `osc_arena_create`/`osc_arena_destroy`
        // bracket the body, and the body's `_arena` shadows the enclosing
        // one for any calls it makes. Matches `src/codegen.rs`'s
        // `emit_arena` exactly, including that a valued arena body's
        // result must already be safe to read after the arena is
        // destroyed (checked by semantic analysis, not re-verified here).
        let parent_arena = self.arena_value;
        let zero = self
            .builder
            .ins()
            .iconst(cranelift_codegen::ir::types::I64, 0);
        let new_arena = self.call_runtime_scalar("osc_arena_create", &[zero], cl_pointer_type());
        self.arena_value = new_arena;
        let result = self.lower_block(body)?;
        self.call_runtime_void("osc_arena_destroy", &[new_arena]);
        self.arena_value = parent_arena;
        Ok(result)
    }
}

/// Arena allocation, byte copies, and string/global-constant addressing.
impl<'a, 'b> FuncTranslator<'a, 'b> {
    fn arena_alloc(&mut self, size: u32) -> Value {
        let size_val = self
            .builder
            .ins()
            .iconst(cranelift_codegen::ir::types::I64, size as i64);
        self.call_runtime_scalar(
            "osc_arena_alloc",
            &[self.arena_value, size_val],
            cl_pointer_type(),
        )
    }

    /// Copy `size` bytes from `src+src_off` to `dest+dest_off`. Used for
    /// every inline-aggregate value/field/element copy (see module docs).
    fn copy_bytes(
        &mut self,
        dest: Value,
        dest_off: i32,
        src: Value,
        src_off: i32,
        size: u32,
        align: u32,
    ) {
        if size == 0 {
            return;
        }
        let dest_addr = if dest_off == 0 {
            dest
        } else {
            self.builder.ins().iadd_imm(dest, dest_off as i64)
        };
        let src_addr = if src_off == 0 {
            src
        } else {
            self.builder.ins().iadd_imm(src, src_off as i64)
        };
        let config = self.ctx.module.target_config();
        let align_u8 = align.min(8) as u8;

        // `FunctionBuilder::emit_small_memory_copy` only *conditionally*
        // inlines: past cranelift-frontend's own threshold (more than 4
        // aligned load/store pairs — e.g. any size over 32 bytes, but
        // also any *smaller* size whose largest power-of-two divisor is
        // itself small, like this backend's 40-byte two-`str`-field
        // struct, whose largest power-of-two divisor is 8 and so needs
        // 5 pairs), it instead lowers to a call to the libc
        // `memcpy`/`memmove` symbol. The freestanding runtime never
        // exports a linkable symbol for either (only a `static inline`
        // equivalent private to its own translation unit — see
        // `deps/laststanding/l_os.h`), so a freestanding link would fail
        // with "undefined reference to `memcpy`" the moment *any*
        // inline-aggregate copy (parameter, tail return, match binding,
        // struct/enum/`Result` construction, ...) needed to move that
        // many bytes. Splitting the copy into a greedy sequence of
        // power-of-two chunks no larger than 32 bytes sidesteps this
        // entirely: each chunk is, by construction, at or under
        // cranelift-frontend's own inlining threshold, so every call
        // here always emits real load/store instructions and never a
        // libcall, regardless of the aggregate's total size.
        let mut offset: u32 = 0;
        while offset < size {
            let remaining = size - offset;
            let mut chunk: u32 = 32;
            while chunk > remaining {
                chunk /= 2;
            }
            let d = if offset == 0 {
                dest_addr
            } else {
                self.builder.ins().iadd_imm(dest_addr, offset as i64)
            };
            let s = if offset == 0 {
                src_addr
            } else {
                self.builder.ins().iadd_imm(src_addr, offset as i64)
            };
            let chunk_align = align_u8.min(chunk as u8).max(1);
            self.builder.emit_small_memory_copy(
                config,
                d,
                s,
                chunk as u64,
                chunk_align,
                chunk_align,
                true,
                mem_flags(),
            );
            offset += chunk;
        }
    }

    fn load_scalar(&mut self, ty: cranelift_codegen::ir::Type, addr: Value, offset: i32) -> Value {
        self.builder.ins().load(ty, mem_flags(), addr, offset)
    }

    fn store_scalar(&mut self, value: Value, addr: Value, offset: i32) {
        self.builder.ins().store(mem_flags(), value, addr, offset);
    }

    /// The pointer-repr value of a string literal: the address of its
    /// (deduplicated) 16-byte `{ptr, len}` header data object.
    fn string_literal_ptr(&mut self, s: &str) -> Value {
        let data_id = self.ctx.string_literal_data(s);
        let gv = self
            .ctx
            .module
            .declare_data_in_func(data_id, self.builder.func);
        self.builder.ins().global_value(cl_pointer_type(), gv)
    }

    /// The value of a top-level `let` constant referenced by name (never a
    /// local/param — those are resolved via `self.lookup` first). Mirrors
    /// `src/codegen.rs`'s `emit_top_level_constants`: string constants
    /// become static data; scalar constants must be foldable to literals
    /// and/or other top-level constants at this point (semantic analysis
    /// restricts top-level `let` to constant-foldable expressions — see
    /// `top_level_const.osc`/`global_mut.osc`).
    fn top_level_const_ptr(&mut self, name: &str, ty: &BcType) -> CResult<Value> {
        self.top_level_const_value(name, ty, &mut Vec::new())
    }

    fn top_level_const_value(
        &mut self,
        name: &str,
        ty: &BcType,
        stack: &mut Vec<String>,
    ) -> CResult<Value> {
        enum FoldedConst {
            Str(String),
            F64(f64),
            I64(i64),
            Alias(String, BcType),
            Unsupported(Span),
        }

        if stack.iter().any(|n| n == name) {
            return Err(unsupported(
                Span::new(1, 1),
                format!("cyclic top-level constant alias involving '{name}'"),
            ));
        }
        let const_def = self
            .program()
            .const_defs
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("internal error: unknown top-level constant '{name}'"));
        stack.push(name.to_string());
        let folded = {
            match &const_def.value {
                oir::Expr::StringLit(s, _) => FoldedConst::Str(s.clone()),
                oir::Expr::FloatLit(v, _) => FoldedConst::F64(*v),
                oir::Expr::Ident {
                    name: alias,
                    kind: oir::IdentKind::Value,
                    ..
                } => FoldedConst::Alias(alias.clone(), const_def.ty.clone()),
                other => match self.const_eval_i64(other, &mut Vec::new()) {
                    Some(value) => FoldedConst::I64(value),
                    None => FoldedConst::Unsupported(const_def.span),
                },
            }
        };
        let result = match folded {
            FoldedConst::Str(value) => Ok(self.string_literal_ptr(&value)),
            FoldedConst::F64(value) => Ok(self
                .builder
                .ins()
                .f64const(cranelift_codegen::ir::immediates::Ieee64::with_float(value))),
            FoldedConst::I64(value) => {
                let cl_ty = Repr::of(ty, self.program())
                    .cl_type()
                    .unwrap_or(cranelift_codegen::ir::types::I32);
                Ok(self.builder.ins().iconst(cl_ty, value))
            }
            FoldedConst::Alias(alias, alias_ty) => self.top_level_const_value(&alias, &alias_ty, stack),
            FoldedConst::Unsupported(span) => Err(unsupported(
                span,
                format!("top-level constant '{name}' initializer (only literals, aliases, and +-*/% of literals/aliases fold at this point)"),
            )),
        };
        stack.pop();
        result
    }

    fn const_eval_i64(&self, expr: &oir::Expr, stack: &mut Vec<String>) -> Option<i64> {
        match expr {
            oir::Expr::IntLit(v, _) => Some(*v),
            oir::Expr::BoolLit(b, _) => Some(if *b { 1 } else { 0 }),
            oir::Expr::FloatLit(v, _) => Some(*v as i64),
            oir::Expr::Ident {
                name,
                kind: oir::IdentKind::Value,
                ..
            } => {
                if stack.iter().any(|n| n == name) {
                    return None;
                }
                let const_def = self.program().const_defs.iter().find(|c| c.name == *name)?;
                stack.push(name.clone());
                let value = self.const_eval_i64(&const_def.value, stack);
                stack.pop();
                value
            }
            oir::Expr::BinaryOp {
                op, left, right, ..
            } => {
                let l = self.const_eval_i64(left, stack)?;
                let r = self.const_eval_i64(right, stack)?;
                match op {
                    BinOp::Add => l.checked_add(r),
                    BinOp::Sub => l.checked_sub(r),
                    BinOp::Mul => l.checked_mul(r),
                    BinOp::Div if r != 0 => Some(l / r),
                    BinOp::Mod if r != 0 => Some(l % r),
                    _ => None,
                }
            }
            oir::Expr::UnaryOp {
                op: UnaryOp::Neg,
                operand,
                ..
            } => self.const_eval_i64(operand, stack)?.checked_neg(),
            _ => None,
        }
    }
}

/// Field access, array element access, and `Place` (lvalue) handling.
impl<'a, 'b> FuncTranslator<'a, 'b> {
    /// Read a value of type `ty` stored at `base + offset`: a real load
    /// for scalars and opaque pointers, or pure address arithmetic (no
    /// load at all) for inline aggregates, whose bytes live embedded in
    /// place — see module docs.
    fn read_at(&mut self, base: Value, offset: u32, ty: &BcType) -> Option<Value> {
        match Repr::of(ty, self.program()) {
            Repr::Unit => None,
            Repr::Scalar(t) => Some(self.load_scalar(t, base, offset as i32)),
            Repr::Pointer => {
                if is_inline_aggregate(ty, self.program()) {
                    Some(if offset == 0 {
                        base
                    } else {
                        self.builder.ins().iadd_imm(base, offset as i64)
                    })
                } else {
                    Some(self.load_scalar(cl_pointer_type(), base, offset as i32))
                }
            }
        }
    }

    /// Write a value of type `ty` into `base + offset`: a real store for
    /// scalars/opaque pointers, or a byte copy from `value`'s own memory
    /// for inline aggregates (never just storing the pointer — the bytes
    /// must end up embedded at `base + offset`, matching C struct-field
    /// assignment semantics exactly).
    fn write_at(&mut self, base: Value, offset: u32, ty: &BcType, value: Option<Value>) {
        match Repr::of(ty, self.program()) {
            Repr::Unit => {}
            Repr::Scalar(_) => self.store_scalar(
                value.expect("scalar write needs a value"),
                base,
                offset as i32,
            ),
            Repr::Pointer => {
                let v = value.expect("pointer write needs a value");
                if is_inline_aggregate(ty, self.program()) {
                    let layout = layout_of(ty, self.program());
                    self.copy_bytes(base, offset as i32, v, 0, layout.size, layout.align);
                } else {
                    self.store_scalar(v, base, offset as i32);
                }
            }
        }
    }

    fn struct_field_offset_of(
        &self,
        ty: &BcType,
        field: &str,
        span: Span,
    ) -> CResult<(u32, BcType)> {
        match ty {
            BcType::Struct(name) => Ok(struct_field_offset(name, field, self.program())),
            other => Err(unsupported(
                span,
                format!("field access on non-struct type '{other}'"),
            )),
        }
    }

    fn array_elem_ty(&self, ty: &BcType, span: Span) -> CResult<BcType> {
        match ty {
            BcType::Array(e) | BcType::FixedArray(e, _) => Ok((**e).clone()),
            other => Err(unsupported(
                span,
                format!("indexing non-array type '{other}'"),
            )),
        }
    }

    fn array_len(&mut self, arr_ptr: Value) -> Value {
        self.call_runtime_scalar(
            "osc_array_len",
            &[arr_ptr],
            cranelift_codegen::ir::types::I32,
        )
    }

    fn array_elem_ptr(&mut self, arr_ptr: Value, idx: Value) -> Value {
        self.call_runtime_scalar("osc_array_get", &[arr_ptr, idx], cl_pointer_type())
    }

    fn array_read(&mut self, arr_ptr: Value, idx: Value, elem_ty: &BcType) -> Option<Value> {
        let elem_ptr = self.array_elem_ptr(arr_ptr, idx);
        self.read_at(elem_ptr, 0, elem_ty)
    }

    fn array_write(&mut self, arr_ptr: Value, idx: Value, elem_ty: &BcType, value: Option<Value>) {
        let elem_ptr = self.array_elem_ptr(arr_ptr, idx);
        self.write_at(elem_ptr, 0, elem_ty, value);
    }

    fn lower_field_access(
        &mut self,
        obj: &'a oir::Expr,
        field: &str,
        field_ty: &BcType,
        span: Span,
    ) -> CResult<Option<Value>> {
        let obj_ty = obj.ty();
        let obj_ptr = self.lower_expr(obj)?.expect("struct value is a pointer");
        let (offset, _) = self.struct_field_offset_of(&obj_ty, field, span)?;
        Ok(self.read_at(obj_ptr, offset, field_ty))
    }

    fn lower_index(
        &mut self,
        arr: &'a oir::Expr,
        index: &'a oir::Expr,
        ty: &BcType,
        span: Span,
    ) -> CResult<Option<Value>> {
        let arr_ty = arr.ty();
        if arr_ty == BcType::Str {
            // `s[i]` yields the i-th UTF-8 byte as an i32, bounds-checked
            // by the runtime (`osc_str_check_index` panics out of bounds),
            // matching `src/codegen.rs`'s `Expr::Index` on `Str` exactly.
            let s_ptr = self.lower_expr(arr)?.expect("str value is a pointer");
            let idx_val = self.lower_expr(index)?.expect("index has a value");
            let data_ptr = self.load_scalar(cl_pointer_type(), s_ptr, 0);
            let checked = self.call_runtime_scalar(
                "osc_str_check_index_shim",
                &[s_ptr, idx_val],
                cranelift_codegen::ir::types::I32,
            );
            let checked64 = self
                .builder
                .ins()
                .uextend(cranelift_codegen::ir::types::I64, checked);
            let byte_ptr = self.builder.ins().iadd(data_ptr, checked64);
            let byte = self.load_scalar(cranelift_codegen::ir::types::I8, byte_ptr, 0);
            return Ok(Some(
                self.builder
                    .ins()
                    .uextend(cranelift_codegen::ir::types::I32, byte),
            ));
        }
        let arr_ptr = self.lower_expr(arr)?.expect("array value is a pointer");
        let idx_val = self.lower_expr(index)?.expect("index has a value");
        let elem_ty = self.array_elem_ty(&arr_ty, span)?;
        let _ = ty; // `ty` (the Index node's cached type) always matches `elem_ty` — verified by `ir::verify`.
        Ok(self.array_read(arr_ptr, idx_val, &elem_ty))
    }

    /// An address containing exactly `layout_of(ty).size` bytes
    /// representing `value`: the pointer itself for inline aggregates
    /// (already such an address), or a fresh stack slot holding the
    /// scalar/opaque-pointer value otherwise. Used by `push`/array-literal
    /// construction, which hand a source address to `osc_array_push`.
    fn value_source_addr(&mut self, ty: &BcType, value: Option<Value>) -> Value {
        match Repr::of(ty, self.program()) {
            Repr::Unit => self.arena_alloc(0),
            Repr::Pointer if is_inline_aggregate(ty, self.program()) => {
                value.expect("aggregate has a pointer value")
            }
            _ => {
                let size = layout_of(ty, self.program()).size.max(1);
                let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    size,
                    0,
                ));
                let addr = self.builder.ins().stack_addr(cl_pointer_type(), slot, 0);
                if let Some(v) = value {
                    self.store_scalar(v, addr, 0);
                }
                addr
            }
        }
    }

    fn lower_array_lit(
        &mut self,
        elements: &'a [oir::Expr],
        elem_ty: &BcType,
        span: Span,
    ) -> CResult<Option<Value>> {
        let elem_size = layout_of(elem_ty, self.program()).size;
        let size_val = self
            .builder
            .ins()
            .iconst(cranelift_codegen::ir::types::I32, elem_size as i64);
        let cap_val = self
            .builder
            .ins()
            .iconst(cranelift_codegen::ir::types::I32, elements.len() as i64);
        let arr_ptr = self.call_runtime_scalar(
            "osc_array_new",
            &[self.arena_value, size_val, cap_val],
            cl_pointer_type(),
        );
        for el in elements {
            let raw = self.lower_expr(el)?;
            let addr = self.value_source_addr(elem_ty, raw);
            self.call_runtime_void("osc_array_push", &[self.arena_value, arr_ptr, addr]);
        }
        let _ = span;
        Ok(Some(arr_ptr))
    }

    /// Synthesize the `Expr` a `Place` would read as an rvalue (used by
    /// `CompoundAssign` to read the current value before combining it with
    /// the RHS via the operator). Spans are copied from the place/its
    /// sub-expressions; they are only ever used for backend diagnostics on
    /// an already-semantically-checked program.
    fn place_as_expr(&self, place: &oir::Place) -> &'a oir::Expr {
        let mut cur = oir::Expr::Ident {
            name: place.name.clone(),
            kind: oir::IdentKind::Value,
            ty: place.base_ty.clone(),
            span: place.span,
        };
        let mut cur_ty = place.base_ty.clone();
        for acc in &place.accessors {
            match acc {
                oir::PlaceAccessor::Field(f) => {
                    let (_, field_ty) = struct_field_offset(
                        match &cur_ty {
                            BcType::Struct(n) => n,
                            _ => "",
                        },
                        f,
                        self.program(),
                    );
                    cur = oir::Expr::FieldAccess {
                        expr: Box::new(cur),
                        field: f.clone(),
                        ty: field_ty.clone(),
                        span: place.span,
                    };
                    cur_ty = field_ty;
                }
                oir::PlaceAccessor::Index(idx) => {
                    let elem_ty = match &cur_ty {
                        BcType::Array(e) | BcType::FixedArray(e, _) => (**e).clone(),
                        _ => BcType::I32,
                    };
                    cur = oir::Expr::Index {
                        expr: Box::new(cur),
                        index: Box::new(clone_expr_shallow(idx)),
                        ty: elem_ty.clone(),
                        span: place.span,
                    };
                    cur_ty = elem_ty;
                }
            }
        }
        // Leaked deliberately: this synthesizes a small `Expr` tree that
        // does not live inside the original `ir::Program`, but every
        // lowering method takes `&'a Expr` (borrowed from that program's
        // lifetime) so it can stash references in `self.defers` etc.
        // Compilation is a short-lived, one-shot process, so leaking one
        // small tree per `CompoundAssign` statement is an acceptable,
        // deliberate trade-off rather than threading a second lifetime
        // through every lowering method just for this one case.
        Box::leak(Box::new(cur))
    }

    /// Store `value` (already lowered) into `place`, whose accessor list
    /// is non-empty (whole-variable reassignment is handled directly by
    /// the `Assign`/`CompoundAssign` statement cases without calling this).
    fn store_place(
        &mut self,
        place: &'a oir::Place,
        raw_value: Option<Value>,
        value_expr: &'a oir::Expr,
    ) -> CResult<()> {
        let final_ty = self.place_final_type(place)?;
        let value = self.bind_value(&final_ty, raw_value, value_expr)?;
        self.store_place_value(place, value)
    }

    fn store_place_value(&mut self, place: &'a oir::Place, value: Option<Value>) -> CResult<()> {
        let binding = self
            .lookup(&place.name)
            .unwrap_or_else(|| panic!("internal error: unknown place root '{}'", place.name));
        let mut addr = self.builder.use_var(binding.var);
        let mut ty = binding.ty;
        let n = place.accessors.len();
        for (i, acc) in place.accessors.iter().enumerate() {
            let is_last = i + 1 == n;
            match acc {
                oir::PlaceAccessor::Field(f) => {
                    let (offset, field_ty) = self.struct_field_offset_of(&ty, f, place.span)?;
                    if is_last {
                        self.write_at(addr, offset, &field_ty, value);
                        return Ok(());
                    }
                    addr = self
                        .read_at(addr, offset, &field_ty)
                        .expect("container field is a pointer");
                    ty = field_ty;
                }
                oir::PlaceAccessor::Index(idx) => {
                    let elem_ty = self.array_elem_ty(&ty, place.span)?;
                    let idx_val = self.lower_expr(idx)?.expect("index has a value");
                    if is_last {
                        self.array_write(addr, idx_val, &elem_ty, value);
                        return Ok(());
                    }
                    addr = self.array_elem_ptr(addr, idx_val);
                    ty = elem_ty;
                }
            }
        }
        Ok(())
    }

    /// The static type of `place` after applying every accessor (i.e. the
    /// type actually being written), computed independently of
    /// `place.base_ty` (which — see `ir.rs` — is always the *root*
    /// variable's type, even for chained accessors).
    fn place_final_type(&self, place: &oir::Place) -> CResult<BcType> {
        let mut ty = place.base_ty.clone();
        for acc in &place.accessors {
            match acc {
                oir::PlaceAccessor::Field(f) => {
                    let (_, field_ty) = self.struct_field_offset_of(&ty, f, place.span)?;
                    ty = field_ty;
                }
                oir::PlaceAccessor::Index(_) => {
                    ty = self.array_elem_ty(&ty, place.span)?;
                }
            }
        }
        Ok(ty)
    }
}

/// A shallow structural clone of an `Expr`, used only by `place_as_expr`
/// to re-embed a `Place`'s index sub-expression into a synthesized `Expr`
/// tree (the IR has no `Clone` derive since ordinary lowering never needs
/// to duplicate a sub-expression — this one narrow case does).
fn clone_expr_shallow(expr: &oir::Expr) -> oir::Expr {
    match expr {
        oir::Expr::IntLit(v, s) => oir::Expr::IntLit(*v, *s),
        oir::Expr::FloatLit(v, s) => oir::Expr::FloatLit(*v, *s),
        oir::Expr::BoolLit(v, s) => oir::Expr::BoolLit(*v, *s),
        oir::Expr::StringLit(v, s) => oir::Expr::StringLit(v.clone(), *s),
        oir::Expr::Ident {
            name,
            kind,
            ty,
            span,
        } => oir::Expr::Ident {
            name: name.clone(),
            kind: *kind,
            ty: ty.clone(),
            span: *span,
        },
        oir::Expr::BinaryOp {
            op,
            left,
            right,
            ty,
            span,
        } => oir::Expr::BinaryOp {
            op: *op,
            left: Box::new(clone_expr_shallow(left)),
            right: Box::new(clone_expr_shallow(right)),
            ty: ty.clone(),
            span: *span,
        },
        oir::Expr::FieldAccess {
            expr,
            field,
            ty,
            span,
        } => oir::Expr::FieldAccess {
            expr: Box::new(clone_expr_shallow(expr)),
            field: field.clone(),
            ty: ty.clone(),
            span: *span,
        },
        oir::Expr::Index {
            expr,
            index,
            ty,
            span,
        } => oir::Expr::Index {
            expr: Box::new(clone_expr_shallow(expr)),
            index: Box::new(clone_expr_shallow(index)),
            ty: ty.clone(),
            span: *span,
        },
        // Index expressions inside a `Place` are restricted by the parser
        // to simple arithmetic on locals/literals (`arr[i + 1]`), never a
        // call/block/match/etc — see `ast::Expr` construction in
        // `parser.rs`'s index-expression grammar — so this fallback is
        // unreachable in practice; reproducing it verbatim (rather than
        // panicking) still gives a correct, if inefficiently re-evaluated,
        // translation if that assumption is ever wrong for a corner case.
        other => panic!(
            "internal error: unexpected expression kind inside a Place index (variant #{})",
            expr_variant_tag(other)
        ),
    }
}

/// A small numeric tag distinguishing `Expr` variants for the panic
/// message above, since `ir::Expr` intentionally does not derive `Debug`
/// (it is never printed/dumped elsewhere in the compiler).
fn expr_variant_tag(expr: &oir::Expr) -> u32 {
    match expr {
        oir::Expr::IntLit(..) => 0,
        oir::Expr::FloatLit(..) => 1,
        oir::Expr::StringLit(..) => 2,
        oir::Expr::InterpolatedString { .. } => 3,
        oir::Expr::BoolLit(..) => 4,
        oir::Expr::Ident { .. } => 5,
        oir::Expr::BinaryOp { .. } => 6,
        oir::Expr::UnaryOp { .. } => 7,
        oir::Expr::Cast { .. } => 8,
        oir::Expr::Call { .. } => 9,
        oir::Expr::FieldAccess { .. } => 10,
        oir::Expr::Index { .. } => 11,
        oir::Expr::Block(_) => 12,
        oir::Expr::If { .. } => 13,
        oir::Expr::Match { .. } => 14,
        oir::Expr::Try { .. } => 15,
        oir::Expr::ArrayLit { .. } => 16,
        oir::Expr::StructLit { .. } => 17,
        oir::Expr::EnumConstructor { .. } => 18,
        oir::Expr::Arena { .. } => 19,
    }
}

/// Arithmetic/comparison/string-op lowering.
impl<'a, 'b> FuncTranslator<'a, 'b> {
    /// `and`/`or` with true short-circuit control flow (the naive
    /// eager-evaluate-both-operands approach `lower_binop` uses for every
    /// other operator is *not* correct here: the right operand must not
    /// even be evaluated when it would be redundant, exactly like C's `&&`/`||`
    /// that `src/codegen.rs` relies on for the same short-circuiting).
    fn lower_short_circuit(
        &mut self,
        op: BinOp,
        left: &'a oir::Expr,
        right: &'a oir::Expr,
    ) -> CResult<Option<Value>> {
        use cranelift_codegen::ir::types;
        let lv = self.lower_expr(left)?.expect("bool operand has a value");
        let rhs_blk = self.builder.create_block();
        let merge_blk = self.builder.create_block();
        self.builder.append_block_param(merge_blk, types::I8);

        match op {
            BinOp::And => {
                self.builder
                    .ins()
                    .brif(lv, rhs_blk, &[], merge_blk, &block_args(Some(lv)));
            }
            BinOp::Or => {
                self.builder
                    .ins()
                    .brif(lv, merge_blk, &block_args(Some(lv)), rhs_blk, &[]);
            }
            _ => unreachable!("lower_short_circuit only handles And/Or"),
        }
        self.builder.seal_block(rhs_blk);

        self.goto(rhs_blk);
        let rv = self.lower_expr(right)?.expect("bool operand has a value");
        if !self.terminated {
            let args = block_args(Some(rv));
            self.builder.ins().jump(merge_blk, &args);
        }

        self.builder.seal_block(merge_blk);
        self.goto(merge_blk);
        Ok(Some(self.builder.block_params(merge_blk)[0]))
    }

    fn lower_binop(
        &mut self,
        op: BinOp,
        lv: Option<Value>,
        rv: Option<Value>,
        operand_ty: &BcType,
        span: Span,
    ) -> CResult<Option<Value>> {
        use cranelift_codegen::ir::types;
        let l = lv.expect("binop operand has a value");
        let r = rv.expect("binop operand has a value");
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => match operand_ty {
                BcType::I32 => {
                    let sym = match op {
                        BinOp::Add => "osc_add_i32",
                        BinOp::Sub => "osc_sub_i32",
                        BinOp::Mul => "osc_mul_i32",
                        BinOp::Div => "osc_div_i32",
                        BinOp::Mod => "osc_mod_i32",
                        _ => unreachable!(),
                    };
                    Ok(Some(self.call_runtime_scalar(sym, &[l, r], types::I32)))
                }
                BcType::I64 => {
                    let sym = match op {
                        BinOp::Add => "osc_add_i64",
                        BinOp::Sub => "osc_sub_i64",
                        BinOp::Mul => "osc_mul_i64",
                        BinOp::Div => "osc_div_i64",
                        BinOp::Mod => "osc_mod_i64",
                        _ => unreachable!(),
                    };
                    Ok(Some(self.call_runtime_scalar(sym, &[l, r], types::I64)))
                }
                BcType::F64 => Ok(Some(match op {
                    BinOp::Add => self.builder.ins().fadd(l, r),
                    BinOp::Sub => self.builder.ins().fsub(l, r),
                    BinOp::Mul => self.builder.ins().fmul(l, r),
                    BinOp::Div => self.builder.ins().fdiv(l, r),
                    BinOp::Mod => self.call_runtime_scalar("osc_math_fmod", &[l, r], types::F64),
                    _ => unreachable!(),
                })),
                BcType::Str if matches!(op, BinOp::Add) => Ok(Some(self.call_shim_out(
                    "osc_str_concat_shim",
                    &BcType::Str,
                    &[self.arena_value, l, r],
                ))),
                other => Err(unsupported(span, format!("'{op:?}' on type '{other}'"))),
            },
            BinOp::Eq | BinOp::Neq => {
                let (lt, rt) = self.tag_or_scalar_for_eq(operand_ty, l, r);
                match operand_ty {
                    BcType::Str => {
                        let eq = self.call_runtime_scalar("osc_str_eq_shim", &[l, r], types::I8);
                        Ok(Some(if matches!(op, BinOp::Eq) {
                            eq
                        } else {
                            self.builder.ins().bxor_imm(eq, 1)
                        }))
                    }
                    BcType::F64 => {
                        let cc = if matches!(op, BinOp::Eq) {
                            FloatCC::Equal
                        } else {
                            FloatCC::NotEqual
                        };
                        Ok(Some(self.builder.ins().fcmp(cc, lt, rt)))
                    }
                    _ => {
                        let cc = if matches!(op, BinOp::Eq) {
                            IntCC::Equal
                        } else {
                            IntCC::NotEqual
                        };
                        Ok(Some(self.builder.ins().icmp(cc, lt, rt)))
                    }
                }
            }
            BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => match operand_ty {
                BcType::Str => {
                    let cmp = self.call_runtime_scalar("osc_str_compare_shim", &[l, r], types::I32);
                    let zero = self.builder.ins().iconst(types::I32, 0);
                    let cc = match op {
                        BinOp::Lt => IntCC::SignedLessThan,
                        BinOp::Gt => IntCC::SignedGreaterThan,
                        BinOp::LtEq => IntCC::SignedLessThanOrEqual,
                        BinOp::GtEq => IntCC::SignedGreaterThanOrEqual,
                        _ => unreachable!(),
                    };
                    Ok(Some(self.builder.ins().icmp(cc, cmp, zero)))
                }
                BcType::F64 => {
                    let cc = match op {
                        BinOp::Lt => FloatCC::LessThan,
                        BinOp::Gt => FloatCC::GreaterThan,
                        BinOp::LtEq => FloatCC::LessThanOrEqual,
                        BinOp::GtEq => FloatCC::GreaterThanOrEqual,
                        _ => unreachable!(),
                    };
                    Ok(Some(self.builder.ins().fcmp(cc, l, r)))
                }
                BcType::I32 | BcType::I64 | BcType::Bool | BcType::Handle => {
                    let cc = match op {
                        BinOp::Lt => IntCC::SignedLessThan,
                        BinOp::Gt => IntCC::SignedGreaterThan,
                        BinOp::LtEq => IntCC::SignedLessThanOrEqual,
                        BinOp::GtEq => IntCC::SignedGreaterThanOrEqual,
                        _ => unreachable!(),
                    };
                    Ok(Some(self.builder.ins().icmp(cc, l, r)))
                }
                other => Err(unsupported(span, format!("'{op:?}' on type '{other}'"))),
            },
            BinOp::And | BinOp::Or => {
                unreachable!("handled by lower_short_circuit before reaching lower_binop")
            }
        }
    }

    /// For `==`/`!=`: payload-bearing enums compare by tag only (loaded
    /// from the pointer), matching `src/codegen.rs`'s `{}.tag == {}.tag`;
    /// every other type's Cranelift value already *is* the thing to
    /// compare directly.
    fn tag_or_scalar_for_eq(&mut self, ty: &BcType, l: Value, r: Value) -> (Value, Value) {
        if let BcType::Enum(name) = ty {
            if layout::enum_has_payload(name, self.program()) {
                let lt = self.load_scalar(cranelift_codegen::ir::types::I32, l, 0);
                let rt = self.load_scalar(cranelift_codegen::ir::types::I32, r, 0);
                return (lt, rt);
            }
        }
        (l, r)
    }

    /// Call a shim whose C signature is `void sym(RetC* out, ...args)`,
    /// allocating `out` in the arena and returning its pointer as the
    /// call's resulting (pointer-repr) value.
    fn call_shim_out(&mut self, symbol: &'static str, ret_ty: &BcType, args: &[Value]) -> Value {
        let layout = layout_of(ret_ty, self.program());
        let out_ptr = self.arena_alloc(layout.size);
        let mut all_args = Vec::with_capacity(args.len() + 1);
        all_args.push(out_ptr);
        all_args.extend_from_slice(args);
        self.call_runtime_void(symbol, &all_args);
        out_ptr
    }

    fn lower_interpolated_string(
        &mut self,
        parts: &'a [oir::InterpolatedStringPart],
        span: Span,
    ) -> CResult<Option<Value>> {
        let mut acc: Option<Value> = None;
        for part in parts {
            let piece = match part {
                oir::InterpolatedStringPart::Text(text) => {
                    if text.is_empty() {
                        continue;
                    }
                    self.string_literal_ptr(text)
                }
                oir::InterpolatedStringPart::Expr(e) => {
                    let v = self.lower_expr(e)?.expect("interpolated expr has a value");
                    match e.ty() {
                        BcType::Str => v,
                        BcType::I32 => self.call_shim_out(
                            "osc_str_from_i32_shim",
                            &BcType::Str,
                            &[self.arena_value, v],
                        ),
                        BcType::I64 => self.call_shim_out(
                            "osc_str_from_i64_shim",
                            &BcType::Str,
                            &[self.arena_value, v],
                        ),
                        BcType::F64 => self.call_shim_out(
                            "osc_str_from_f64_shim",
                            &BcType::Str,
                            &[self.arena_value, v],
                        ),
                        BcType::Bool => {
                            self.call_shim_out("osc_str_from_bool_shim", &BcType::Str, &[v])
                        }
                        other => {
                            return Err(unsupported(
                                span,
                                format!("string interpolation of type '{other}'"),
                            ))
                        }
                    }
                }
            };
            acc = Some(match acc {
                None => piece,
                Some(prev) => self.call_shim_out(
                    "osc_str_concat_shim",
                    &BcType::Str,
                    &[self.arena_value, prev, piece],
                ),
            });
        }
        Ok(Some(acc.unwrap_or_else(|| self.string_literal_ptr(""))))
    }
}

/// Call lowering: builtins (direct-scalar and shimmed), user functions,
/// extern functions, and indirect calls through a function-pointer value.
impl<'a, 'b> FuncTranslator<'a, 'b> {
    fn lower_call(
        &mut self,
        callee: &'a oir::Callee,
        args: &'a [oir::Expr],
        ty: &BcType,
        span: Span,
    ) -> CResult<Option<Value>> {
        use cranelift_codegen::ir::types;
        let name: &str = match callee {
            oir::Callee::Named(n) | oir::Callee::Var(n) => n.as_str(),
        };
        let mut a: Vec<Option<Value>> = Vec::with_capacity(args.len());
        for arg in args {
            a.push(self.lower_expr(arg)?);
        }
        let arena = self.arena_value;
        macro_rules! v {
            ($i:expr) => {
                a[$i].expect("builtin argument has a value")
            };
        }

        Ok(match name {
            // -- I/O --------------------------------------------------
            "print" => {
                self.call_runtime_void("osc_print_shim", &[v!(0)]);
                None
            }
            "println" => {
                self.call_runtime_void("osc_println_shim", &[v!(0)]);
                None
            }
            "print_i32" => {
                self.call_runtime_void("osc_print_i32", &[v!(0)]);
                None
            }
            "print_i64" => {
                self.call_runtime_void("osc_print_i64", &[v!(0)]);
                None
            }
            "print_f64" => {
                self.call_runtime_void("osc_print_f64", &[v!(0)]);
                None
            }
            "print_bool" => {
                self.call_runtime_void("osc_print_bool", &[v!(0)]);
                None
            }
            "read_line" => {
                Some(self.call_shim_out("osc_read_line_shim", &result_ty("str", "str"), &[arena]))
            }
            "write_str" => {
                self.call_runtime_void("osc_write_str_shim", &[v!(0), v!(1)]);
                None
            }
            "file_open_read" => Some(self.call_shim_out(
                "osc_file_open_read_shim",
                &result_ty("i32", "str"),
                &[v!(0)],
            )),
            "file_open_write" => Some(self.call_shim_out(
                "osc_file_open_write_shim",
                &result_ty("i32", "str"),
                &[v!(0)],
            )),
            "read_byte" => Some(self.call_runtime_scalar("osc_read_byte", &[v!(0)], types::I32)),
            "write_byte" => {
                self.call_runtime_void("osc_write_byte", &[v!(0), v!(1)]);
                None
            }
            "file_close" => {
                self.call_runtime_void("osc_file_close", &[v!(0)]);
                None
            }
            "read_file" => Some(self.call_shim_out(
                "osc_read_file_shim",
                &result_ty("str", "str"),
                &[arena, v!(0)],
            )),
            "write_file" => Some(self.call_shim_out(
                "osc_write_file_shim",
                &result_ty("str", "str"),
                &[v!(0), v!(1)],
            )),
            "file_exists" => {
                Some(self.call_runtime_scalar("osc_file_exists_shim", &[v!(0)], types::I8))
            }
            "path_exists" => {
                Some(self.call_runtime_scalar("osc_path_exists_shim", &[v!(0)], types::I8))
            }
            "path_is_dir" => {
                Some(self.call_runtime_scalar("osc_path_is_dir_shim", &[v!(0)], types::I8))
            }
            "dir_current" => {
                Some(self.call_shim_out("osc_dir_current_shim", &BcType::Str, &[arena]))
            }
            "env_get" => Some(self.call_shim_out(
                "osc_env_get_shim",
                &result_ty("str", "str"),
                &[arena, v!(0)],
            )),
            "errno_get" => Some(self.call_runtime_scalar("osc_errno_get", &[], types::I32)),
            "errno_str" => Some(self.call_shim_out("osc_errno_str_shim", &BcType::Str, &[v!(0)])),
            "sha256" => Some(self.call_shim_out("osc_sha256_shim", &BcType::Str, &[arena, v!(0)])),
            "arg_count" => Some(self.call_runtime_scalar("osc_arg_count", &[], types::I32)),
            "arg_get" => {
                Some(self.call_shim_out("osc_arg_get_shim", &BcType::Str, &[arena, v!(0)]))
            }

            // -- Strings ------------------------------------------------
            "str_len" => Some(self.call_runtime_scalar("osc_str_len_shim", &[v!(0)], types::I32)),
            "str_eq" => {
                Some(self.call_runtime_scalar("osc_str_eq_shim", &[v!(0), v!(1)], types::I8))
            }
            "str_concat" => Some(self.call_shim_out(
                "osc_str_concat_shim",
                &BcType::Str,
                &[arena, v!(0), v!(1)],
            )),
            "str_to_cstr" => {
                Some(self.call_shim_out("osc_str_to_cstr_shim", &BcType::Str, &[arena, v!(0)]))
            }
            "str_compare" => {
                Some(self.call_runtime_scalar("osc_str_compare_shim", &[v!(0), v!(1)], types::I32))
            }
            "str_find" => {
                Some(self.call_runtime_scalar("osc_str_find_shim", &[v!(0), v!(1)], types::I32))
            }
            "str_contains" => {
                Some(self.call_runtime_scalar("osc_str_contains_shim", &[v!(0), v!(1)], types::I8))
            }
            "str_starts_with" => Some(self.call_runtime_scalar(
                "osc_str_starts_with_shim",
                &[v!(0), v!(1)],
                types::I8,
            )),
            "str_ends_with" => {
                Some(self.call_runtime_scalar("osc_str_ends_with_shim", &[v!(0), v!(1)], types::I8))
            }
            "str_trim" => {
                Some(self.call_shim_out("osc_str_trim_shim", &BcType::Str, &[arena, v!(0)]))
            }
            "str_to_upper" => {
                Some(self.call_shim_out("osc_str_to_upper_shim", &BcType::Str, &[arena, v!(0)]))
            }
            "str_to_lower" => {
                Some(self.call_shim_out("osc_str_to_lower_shim", &BcType::Str, &[arena, v!(0)]))
            }
            "str_replace" => Some(self.call_shim_out(
                "osc_str_replace_shim",
                &BcType::Str,
                &[arena, v!(0), v!(1), v!(2)],
            )),
            "str_slice" => Some(self.call_shim_out(
                "osc_str_slice_shim",
                &BcType::Str,
                &[arena, v!(0), v!(1), v!(2)],
            )),
            "str_from_i32" | "i32_to_str" => {
                Some(self.call_shim_out("osc_str_from_i32_shim", &BcType::Str, &[arena, v!(0)]))
            }
            "str_from_i64" => {
                Some(self.call_shim_out("osc_str_from_i64_shim", &BcType::Str, &[arena, v!(0)]))
            }
            "str_from_f64" => {
                Some(self.call_shim_out("osc_str_from_f64_shim", &BcType::Str, &[arena, v!(0)]))
            }
            "str_from_bool" => {
                Some(self.call_shim_out("osc_str_from_bool_shim", &BcType::Str, &[v!(0)]))
            }
            "str_from_i32_hex" => {
                Some(self.call_shim_out("osc_str_from_i32_hex_shim", &BcType::Str, &[arena, v!(0)]))
            }
            "str_from_i64_hex" => {
                Some(self.call_shim_out("osc_str_from_i64_hex_shim", &BcType::Str, &[arena, v!(0)]))
            }
            "parse_i32" => {
                Some(self.call_shim_out("osc_parse_i32_shim", &result_ty("i32", "str"), &[v!(0)]))
            }
            "parse_i64" => {
                Some(self.call_shim_out("osc_parse_i64_shim", &result_ty("i64", "str"), &[v!(0)]))
            }
            "str_split" => Some(self.call_runtime_scalar(
                "osc_str_split_shim",
                &[arena, v!(0), v!(1)],
                cl_pointer_type(),
            )),
            "str_join" => {
                Some(self.call_shim_out("osc_str_join_shim", &BcType::Str, &[arena, v!(0), v!(1)]))
            }
            "str_from_chars" => {
                Some(self.call_shim_out("osc_str_from_chars_shim", &BcType::Str, &[arena, v!(0)]))
            }
            "str_to_chars" => Some(self.call_runtime_scalar(
                "osc_str_to_chars_shim",
                &[arena, v!(0)],
                cl_pointer_type(),
            )),
            "path_join" => {
                Some(self.call_shim_out("osc_path_join_shim", &BcType::Str, &[arena, v!(0), v!(1)]))
            }
            "path_basename" => {
                Some(self.call_shim_out("osc_path_basename_shim", &BcType::Str, &[v!(0)]))
            }
            "path_dirname" => {
                Some(self.call_shim_out("osc_path_dirname_shim", &BcType::Str, &[arena, v!(0)]))
            }
            "file_delete" => {
                Some(self.call_shim_out("osc_file_delete_shim", &result_ty("str", "str"), &[v!(0)]))
            }
            "file_size" => {
                Some(self.call_runtime_scalar("osc_file_size_shim", &[v!(0)], types::I64))
            }
            "file_rename" => Some(self.call_shim_out(
                "osc_file_rename_shim",
                &result_ty("str", "str"),
                &[v!(0), v!(1)],
            )),
            "path_ext" => Some(self.call_shim_out("osc_path_ext_shim", &BcType::Str, &[v!(0)])),
            "dir_create" => {
                Some(self.call_shim_out("osc_dir_create_shim", &result_ty("str", "str"), &[v!(0)]))
            }
            "dir_remove" => {
                Some(self.call_shim_out("osc_dir_remove_shim", &result_ty("str", "str"), &[v!(0)]))
            }
            "sort_i32" => {
                self.call_runtime_void("osc_sort_i32", &[v!(0)]);
                None
            }
            "sort_i64" => {
                self.call_runtime_void("osc_sort_i64", &[v!(0)]);
                None
            }
            "sort_str" => {
                self.call_runtime_void("osc_sort_str", &[v!(0)]);
                None
            }
            "sort_f64" => {
                self.call_runtime_void("osc_sort_f64", &[v!(0)]);
                None
            }
            "time_utc_year" => {
                Some(self.call_runtime_scalar("osc_time_utc_year", &[v!(0)], types::I32))
            }
            "time_utc_month" => {
                Some(self.call_runtime_scalar("osc_time_utc_month", &[v!(0)], types::I32))
            }
            "time_utc_day" => {
                Some(self.call_runtime_scalar("osc_time_utc_day", &[v!(0)], types::I32))
            }
            "time_utc_hour" => {
                Some(self.call_runtime_scalar("osc_time_utc_hour", &[v!(0)], types::I32))
            }
            "time_utc_min" => {
                Some(self.call_runtime_scalar("osc_time_utc_min", &[v!(0)], types::I32))
            }
            "time_utc_sec" => {
                Some(self.call_runtime_scalar("osc_time_utc_sec", &[v!(0)], types::I32))
            }
            "time_format" => Some(self.call_shim_out(
                "osc_time_format_shim",
                &BcType::Str,
                &[arena, v!(0), v!(1)],
            )),
            "glob_match" => {
                Some(self.call_runtime_scalar("osc_glob_match_shim", &[v!(0), v!(1)], types::I8))
            }
            "is_tty" => Some(self.call_runtime_scalar("osc_is_tty", &[], types::I8)),
            "env_set" => Some(self.call_shim_out(
                "osc_env_set_shim",
                &result_ty("str", "str"),
                &[v!(0), v!(1)],
            )),
            "env_delete" => {
                Some(self.call_shim_out("osc_env_delete_shim", &result_ty("str", "str"), &[v!(0)]))
            }

            // -- Arithmetic / math helpers also callable as functions ---
            "abs_i32" => Some(self.call_runtime_scalar("osc_abs_i32", &[v!(0)], types::I32)),
            "abs_i64" => Some(self.call_runtime_scalar("osc_abs_i64", &[v!(0)], types::I64)),
            "abs_f64" => Some(self.call_runtime_scalar("osc_abs_f64", &[v!(0)], types::F64)),
            "mod_i32" => Some(self.call_runtime_scalar("osc_mod_i32", &[v!(0), v!(1)], types::I32)),
            "min_i32" => Some(self.call_runtime_scalar("osc_min_i32", &[v!(0), v!(1)], types::I32)),
            "max_i32" => Some(self.call_runtime_scalar("osc_max_i32", &[v!(0), v!(1)], types::I32)),
            "clamp_i32" => {
                Some(self.call_runtime_scalar("osc_clamp_i32", &[v!(0), v!(1), v!(2)], types::I32))
            }
            "min_i64" => Some(self.call_runtime_scalar("osc_min_i64", &[v!(0), v!(1)], types::I64)),
            "max_i64" => Some(self.call_runtime_scalar("osc_max_i64", &[v!(0), v!(1)], types::I64)),
            "clamp_i64" => {
                Some(self.call_runtime_scalar("osc_clamp_i64", &[v!(0), v!(1), v!(2)], types::I64))
            }
            "min_f64" => Some(self.call_runtime_scalar("osc_min_f64", &[v!(0), v!(1)], types::F64)),
            "max_f64" => Some(self.call_runtime_scalar("osc_max_f64", &[v!(0), v!(1)], types::F64)),
            "clamp_f64" => {
                Some(self.call_runtime_scalar("osc_clamp_f64", &[v!(0), v!(1), v!(2)], types::F64))
            }
            "math_sin" => Some(self.call_runtime_scalar("osc_math_sin", &[v!(0)], types::F64)),
            "math_cos" => Some(self.call_runtime_scalar("osc_math_cos", &[v!(0)], types::F64)),
            "math_sqrt" => Some(self.call_runtime_scalar("osc_math_sqrt", &[v!(0)], types::F64)),
            "math_pow" => {
                Some(self.call_runtime_scalar("osc_math_pow", &[v!(0), v!(1)], types::F64))
            }
            "math_exp" => Some(self.call_runtime_scalar("osc_math_exp", &[v!(0)], types::F64)),
            "math_log" => Some(self.call_runtime_scalar("osc_math_log", &[v!(0)], types::F64)),
            "math_atan2" => {
                Some(self.call_runtime_scalar("osc_math_atan2", &[v!(0), v!(1)], types::F64))
            }
            "math_floor" => Some(self.call_runtime_scalar("osc_math_floor", &[v!(0)], types::F64)),
            "math_ceil" => Some(self.call_runtime_scalar("osc_math_ceil", &[v!(0)], types::F64)),
            "math_fmod" => {
                Some(self.call_runtime_scalar("osc_math_fmod", &[v!(0), v!(1)], types::F64))
            }
            "math_abs" => Some(self.call_runtime_scalar("osc_math_abs", &[v!(0)], types::F64)),
            "math_pi" => Some(self.call_runtime_scalar("osc_math_pi", &[], types::F64)),
            "math_e" => Some(self.call_runtime_scalar("osc_math_e", &[], types::F64)),
            "math_ln2" => Some(self.call_runtime_scalar("osc_math_ln2", &[], types::F64)),
            "math_sqrt2" => Some(self.call_runtime_scalar("osc_math_sqrt2", &[], types::F64)),

            // -- Bitwise (inlined, matching src/codegen.rs's raw C exprs) --
            "band" => Some(self.builder.ins().band(v!(0), v!(1))),
            "bor" => Some(self.builder.ins().bor(v!(0), v!(1))),
            "bxor" => Some(self.builder.ins().bxor(v!(0), v!(1))),
            "bshl" => Some(self.builder.ins().ishl(v!(0), v!(1))),
            "bshr" => Some(self.builder.ins().ushr(v!(0), v!(1))),
            "bnot" => Some(self.builder.ins().bnot(v!(0))),

            // -- Character classification / conversion -------------------
            "char_is_alpha" => {
                Some(self.call_runtime_scalar("osc_char_is_alpha", &[v!(0)], types::I8))
            }
            "char_is_digit" => {
                Some(self.call_runtime_scalar("osc_char_is_digit", &[v!(0)], types::I8))
            }
            "char_is_alnum" => {
                Some(self.call_runtime_scalar("osc_char_is_alnum", &[v!(0)], types::I8))
            }
            "char_is_space" => {
                Some(self.call_runtime_scalar("osc_char_is_space", &[v!(0)], types::I8))
            }
            "char_is_upper" => {
                Some(self.call_runtime_scalar("osc_char_is_upper", &[v!(0)], types::I8))
            }
            "char_is_lower" => {
                Some(self.call_runtime_scalar("osc_char_is_lower", &[v!(0)], types::I8))
            }
            "char_is_print" => {
                Some(self.call_runtime_scalar("osc_char_is_print", &[v!(0)], types::I8))
            }
            "char_is_xdigit" => {
                Some(self.call_runtime_scalar("osc_char_is_xdigit", &[v!(0)], types::I8))
            }
            "char_to_upper" => {
                Some(self.call_runtime_scalar("osc_char_to_upper", &[v!(0)], types::I32))
            }
            "char_to_lower" => {
                Some(self.call_runtime_scalar("osc_char_to_lower", &[v!(0)], types::I32))
            }

            // -- System ---------------------------------------------------
            "rand_seed" => {
                self.call_runtime_void("osc_rand_seed", &[v!(0)]);
                None
            }
            "rand_i32" => Some(self.call_runtime_scalar("osc_rand_i32", &[], types::I32)),
            "time_now" => Some(self.call_runtime_scalar("osc_time_now", &[], types::I64)),
            "sleep_ms" => {
                self.call_runtime_void("osc_sleep_ms", &[v!(0)]);
                None
            }
            "exit" => {
                self.call_runtime_void("osc_exit", &[v!(0)]);
                None
            }

            // -- Arrays ---------------------------------------------------
            "len" => Some(self.array_len(v!(0))),
            "push" => {
                let elem_ty = array_elem_ty_of(&args[0].ty());
                let addr = self.value_source_addr(&elem_ty, a[1]);
                self.call_runtime_void("osc_array_push", &[arena, v!(0), addr]);
                None
            }
            "pop" => {
                let elem_ty = array_elem_ty_of(&args[0].ty());
                let ptr = self.call_runtime_scalar("osc_array_pop", &[v!(0)], cl_pointer_type());
                self.read_at(ptr, 0, &elem_ty)
            }

            // -- Directory listing, process control, pipes ---------------
            "dir_list" => Some(self.call_runtime_scalar(
                "osc_dir_list_shim",
                &[arena, v!(0)],
                cl_pointer_type(),
            )),
            "dir_change" => {
                Some(self.call_shim_out("osc_dir_change_shim", &result_ty("str", "str"), &[v!(0)]))
            }
            "file_open_append" => Some(self.call_shim_out(
                "osc_file_open_append_shim",
                &result_ty("i32", "str"),
                &[v!(0)],
            )),
            "fd_dup" => Some(self.call_runtime_scalar("osc_fd_dup", &[v!(0)], types::I32)),
            "fd_dup2" => Some(self.call_runtime_scalar("osc_fd_dup2", &[v!(0), v!(1)], types::I32)),
            "proc_run" => {
                Some(self.call_runtime_scalar("osc_proc_run_shim", &[v!(0), v!(1)], types::I32))
            }
            "proc_spawn" => {
                Some(self.call_runtime_scalar("osc_proc_spawn_shim", &[v!(0), v!(1)], types::I32))
            }
            "proc_wait" => Some(self.call_runtime_scalar("osc_proc_wait", &[v!(0)], types::I32)),
            "pipe_create" => {
                Some(self.call_runtime_scalar("osc_pipe_create", &[arena], cl_pointer_type()))
            }
            "path_find_exec" => Some(self.call_shim_out(
                "osc_path_find_exec_shim",
                &result_ty("str", "str"),
                &[arena, v!(0)],
            )),

            // -- Terminal --------------------------------------------------
            "term_width" => Some(self.call_runtime_scalar("osc_term_width", &[], types::I32)),
            "term_height" => Some(self.call_runtime_scalar("osc_term_height", &[], types::I32)),
            "term_raw" => {
                Some(self.call_shim_out("osc_term_raw_shim", &result_ty("str", "str"), &[]))
            }
            "term_restore" => {
                Some(self.call_shim_out("osc_term_restore_shim", &result_ty("str", "str"), &[]))
            }
            "read_nonblock" => Some(self.call_runtime_scalar("osc_read_nonblock", &[], types::I32)),

            // -- Environment iteration --------------------------------------
            "env_count" => Some(self.call_runtime_scalar("osc_env_count", &[], types::I32)),
            "env_key" => {
                Some(self.call_shim_out("osc_env_key_shim", &BcType::Str, &[arena, v!(0)]))
            }
            "env_value" => {
                Some(self.call_shim_out("osc_env_value_shim", &BcType::Str, &[arena, v!(0)]))
            }

            // -- TCP sockets -------------------------------------------------
            "socket_tcp" => {
                Some(self.call_shim_out("osc_socket_tcp_shim", &result_ty("i32", "str"), &[]))
            }
            "socket_connect" => Some(self.call_shim_out(
                "osc_socket_connect_shim",
                &result_ty("str", "str"),
                &[v!(0), v!(1), v!(2)],
            )),
            "socket_bind" => Some(self.call_shim_out(
                "osc_socket_bind_shim",
                &result_ty("str", "str"),
                &[v!(0), v!(1), v!(2)],
            )),
            "socket_listen" => Some(self.call_shim_out(
                "osc_socket_listen_shim",
                &result_ty("str", "str"),
                &[v!(0), v!(1)],
            )),
            "socket_accept" => Some(self.call_shim_out(
                "osc_socket_accept_shim",
                &result_ty("i32", "str"),
                &[v!(0)],
            )),
            "socket_send" => Some(self.call_shim_out(
                "osc_socket_send_shim",
                &result_ty("i32", "str"),
                &[v!(0), v!(1)],
            )),
            "socket_recv" => Some(self.call_shim_out(
                "osc_socket_recv_shim",
                &BcType::Str,
                &[arena, v!(0), v!(1)],
            )),
            "socket_close" => {
                self.call_runtime_void("osc_socket_close", &[v!(0)]);
                None
            }

            // -- UDP sockets -------------------------------------------------
            "socket_udp" => {
                Some(self.call_shim_out("osc_socket_udp_shim", &result_ty("i32", "str"), &[]))
            }
            "socket_sendto" => Some(self.call_runtime_scalar(
                "osc_socket_sendto_shim",
                &[v!(0), v!(1), v!(2), v!(3)],
                types::I32,
            )),
            "socket_recvfrom" => Some(self.call_shim_out(
                "osc_socket_recvfrom_shim",
                &BcType::Str,
                &[arena, v!(0), v!(1)],
            )),

            // -- Unix domain sockets ------------------------------------------
            "socket_unix_connect" => Some(self.call_shim_out(
                "osc_socket_unix_connect_shim",
                &result_ty("i32", "str"),
                &[v!(0)],
            )),

            // -- TLS (encrypted sockets) --------------------------------------
            "tls_connect" => Some(self.call_shim_out(
                "osc_tls_connect_shim",
                &result_ty("i32", "str"),
                &[v!(0), v!(1)],
            )),
            "tls_send" => Some(self.call_shim_out(
                "osc_tls_send_shim",
                &result_ty("i32", "str"),
                &[v!(0), v!(1)],
            )),
            "tls_recv" => {
                Some(self.call_shim_out("osc_tls_recv_shim", &BcType::Str, &[arena, v!(0), v!(1)]))
            }
            "tls_recv_byte" => {
                Some(self.call_runtime_scalar("osc_tls_recv_byte", &[v!(0)], types::I32))
            }
            "tls_close" => {
                self.call_runtime_void("osc_tls_close", &[v!(0)]);
                None
            }
            "tls_cleanup" => {
                self.call_runtime_void("osc_tls_cleanup", &[]);
                None
            }

            // -- Graphics: drawing primitives (plain scalars / an already-
            // pointer osc_array*, so called directly — only the three
            // text-measuring/drawing builtins below cross an osc_str and
            // need a shim) ------------------------------------------------
            "gfx_pixel" => {
                self.call_runtime_void("osc_gfx_pixel", &[v!(0), v!(1), v!(2)]);
                None
            }
            "gfx_get_pixel" => {
                Some(self.call_runtime_scalar("osc_gfx_get_pixel", &[v!(0), v!(1)], types::I32))
            }
            "gfx_line" => {
                self.call_runtime_void("osc_gfx_line", &[v!(0), v!(1), v!(2), v!(3), v!(4)]);
                None
            }
            "gfx_rect" => {
                self.call_runtime_void("osc_gfx_rect", &[v!(0), v!(1), v!(2), v!(3), v!(4)]);
                None
            }
            "gfx_fill_rect" => {
                self.call_runtime_void("osc_gfx_fill_rect", &[v!(0), v!(1), v!(2), v!(3), v!(4)]);
                None
            }
            "gfx_circle" => {
                self.call_runtime_void("osc_gfx_circle", &[v!(0), v!(1), v!(2), v!(3)]);
                None
            }
            "gfx_fill_circle" => {
                self.call_runtime_void("osc_gfx_fill_circle", &[v!(0), v!(1), v!(2), v!(3)]);
                None
            }
            "gfx_draw_text" => Some(self.call_runtime_scalar(
                "osc_gfx_draw_text_shim",
                &[v!(0), v!(1), v!(2), v!(3), v!(4)],
                types::I32,
            )),
            "gfx_draw_text_scaled" => Some(self.call_runtime_scalar(
                "osc_gfx_draw_text_scaled_shim",
                &[v!(0), v!(1), v!(2), v!(3), v!(4), v!(5), v!(6)],
                types::I32,
            )),
            "gfx_text_width" => Some(self.call_runtime_scalar(
                "osc_gfx_text_width_shim",
                &[v!(0), v!(1)],
                types::I32,
            )),
            "gfx_blit" => {
                self.call_runtime_void("osc_gfx_blit", &[v!(0), v!(1), v!(2), v!(3), v!(4)]);
                None
            }
            "gfx_blit_alpha" => {
                self.call_runtime_void("osc_gfx_blit_alpha", &[v!(0), v!(1), v!(2), v!(3), v!(4)]);
                None
            }

            // -- Graphics: color -----------------------------------------
            "rgb" => Some(self.call_runtime_scalar("osc_rgb", &[v!(0), v!(1), v!(2)], types::I32)),
            "rgba" => Some(self.call_runtime_scalar(
                "osc_rgba",
                &[v!(0), v!(1), v!(2), v!(3)],
                types::I32,
            )),

            // -- HashMap (untyped str->str) --------------------------------
            "map_new" => Some(self.call_runtime_scalar("osc_map_new", &[arena], cl_pointer_type())),
            "map_set" => {
                self.call_runtime_void("osc_map_set_shim", &[arena, v!(0), v!(1), v!(2)]);
                None
            }
            "map_get" => {
                Some(self.call_shim_out("osc_map_get_shim", &BcType::Str, &[v!(0), v!(1)]))
            }
            "map_has" => {
                Some(self.call_runtime_scalar("osc_map_has_shim", &[v!(0), v!(1)], types::I8))
            }
            "map_delete" => {
                self.call_runtime_void("osc_map_delete_shim", &[v!(0), v!(1)]);
                None
            }
            "map_len" => Some(self.call_runtime_scalar("osc_map_len", &[v!(0)], types::I32)),

            // -- Typed HashMap: map_str_i32 ---------------------------------
            "map_str_i32_new" => {
                Some(self.call_runtime_scalar("osc_map_str_i32_new", &[arena], cl_pointer_type()))
            }
            "map_str_i32_set" => {
                self.call_runtime_void("osc_map_str_i32_set_shim", &[arena, v!(0), v!(1), v!(2)]);
                None
            }
            "map_str_i32_get" => Some(self.call_runtime_scalar(
                "osc_map_str_i32_get_shim",
                &[v!(0), v!(1)],
                types::I32,
            )),
            "map_str_i32_has" => Some(self.call_runtime_scalar(
                "osc_map_str_i32_has_shim",
                &[v!(0), v!(1)],
                types::I8,
            )),
            "map_str_i32_delete" => {
                self.call_runtime_void("osc_map_str_i32_delete_shim", &[v!(0), v!(1)]);
                None
            }
            "map_str_i32_len" => {
                Some(self.call_runtime_scalar("osc_map_str_i32_len", &[v!(0)], types::I32))
            }

            // -- Typed HashMap: map_str_i64 ---------------------------------
            "map_str_i64_new" => {
                Some(self.call_runtime_scalar("osc_map_str_i64_new", &[arena], cl_pointer_type()))
            }
            "map_str_i64_set" => {
                self.call_runtime_void("osc_map_str_i64_set_shim", &[arena, v!(0), v!(1), v!(2)]);
                None
            }
            "map_str_i64_get" => Some(self.call_runtime_scalar(
                "osc_map_str_i64_get_shim",
                &[v!(0), v!(1)],
                types::I64,
            )),
            "map_str_i64_has" => Some(self.call_runtime_scalar(
                "osc_map_str_i64_has_shim",
                &[v!(0), v!(1)],
                types::I8,
            )),
            "map_str_i64_delete" => {
                self.call_runtime_void("osc_map_str_i64_delete_shim", &[v!(0), v!(1)]);
                None
            }
            "map_str_i64_len" => {
                Some(self.call_runtime_scalar("osc_map_str_i64_len", &[v!(0)], types::I32))
            }

            // -- Typed HashMap: map_str_f64 ---------------------------------
            "map_str_f64_new" => {
                Some(self.call_runtime_scalar("osc_map_str_f64_new", &[arena], cl_pointer_type()))
            }
            "map_str_f64_set" => {
                self.call_runtime_void("osc_map_str_f64_set_shim", &[arena, v!(0), v!(1), v!(2)]);
                None
            }
            "map_str_f64_get" => Some(self.call_runtime_scalar(
                "osc_map_str_f64_get_shim",
                &[v!(0), v!(1)],
                types::F64,
            )),
            "map_str_f64_has" => Some(self.call_runtime_scalar(
                "osc_map_str_f64_has_shim",
                &[v!(0), v!(1)],
                types::I8,
            )),
            "map_str_f64_delete" => {
                self.call_runtime_void("osc_map_str_f64_delete_shim", &[v!(0), v!(1)]);
                None
            }
            "map_str_f64_len" => {
                Some(self.call_runtime_scalar("osc_map_str_f64_len", &[v!(0)], types::I32))
            }

            // -- Typed HashMap: map_i32_str ---------------------------------
            "map_i32_str_new" => {
                Some(self.call_runtime_scalar("osc_map_i32_str_new", &[arena], cl_pointer_type()))
            }
            "map_i32_str_set" => {
                self.call_runtime_void("osc_map_i32_str_set_shim", &[arena, v!(0), v!(1), v!(2)]);
                None
            }
            "map_i32_str_get" => {
                Some(self.call_shim_out("osc_map_i32_str_get_shim", &BcType::Str, &[v!(0), v!(1)]))
            }
            "map_i32_str_has" => {
                Some(self.call_runtime_scalar("osc_map_i32_str_has", &[v!(0), v!(1)], types::I8))
            }
            "map_i32_str_delete" => {
                self.call_runtime_void("osc_map_i32_str_delete", &[v!(0), v!(1)]);
                None
            }
            "map_i32_str_len" => {
                Some(self.call_runtime_scalar("osc_map_i32_str_len", &[v!(0)], types::I32))
            }

            // -- Typed HashMap: map_i32_i32 (never carries an osc_str, so
            // every operation calls the real runtime entry point directly) --
            "map_i32_i32_new" => {
                Some(self.call_runtime_scalar("osc_map_i32_i32_new", &[arena], cl_pointer_type()))
            }
            "map_i32_i32_set" => {
                self.call_runtime_void("osc_map_i32_i32_set", &[arena, v!(0), v!(1), v!(2)]);
                None
            }
            "map_i32_i32_get" => {
                Some(self.call_runtime_scalar("osc_map_i32_i32_get", &[v!(0), v!(1)], types::I32))
            }
            "map_i32_i32_has" => {
                Some(self.call_runtime_scalar("osc_map_i32_i32_has", &[v!(0), v!(1)], types::I8))
            }
            "map_i32_i32_delete" => {
                self.call_runtime_void("osc_map_i32_i32_delete", &[v!(0), v!(1)]);
                None
            }
            "map_i32_i32_len" => {
                Some(self.call_runtime_scalar("osc_map_i32_i32_len", &[v!(0)], types::I32))
            }

            // -- Canvas: lifecycle/state/input (plain scalars, called
            // directly) --------------------------------------------------
            "canvas_close" => {
                self.call_runtime_void("osc_canvas_close", &[]);
                None
            }
            "canvas_alive" => Some(self.call_runtime_scalar("osc_canvas_alive", &[], types::I8)),
            "canvas_flush" => {
                self.call_runtime_void("osc_canvas_flush", &[]);
                None
            }
            "canvas_clear" => {
                self.call_runtime_void("osc_canvas_clear", &[v!(0)]);
                None
            }
            "canvas_width" => Some(self.call_runtime_scalar("osc_canvas_width", &[], types::I32)),
            "canvas_height" => Some(self.call_runtime_scalar("osc_canvas_height", &[], types::I32)),
            "canvas_scale" => Some(self.call_runtime_scalar("osc_canvas_scale", &[], types::I32)),
            "canvas_resized" => {
                Some(self.call_runtime_scalar("osc_canvas_resized", &[], types::I8))
            }
            "canvas_key" => Some(self.call_runtime_scalar("osc_canvas_key", &[], types::I32)),
            "canvas_mouse_x" => {
                Some(self.call_runtime_scalar("osc_canvas_mouse_x", &[], types::I32))
            }
            "canvas_mouse_y" => {
                Some(self.call_runtime_scalar("osc_canvas_mouse_y", &[], types::I32))
            }
            "canvas_mouse_btn" => {
                Some(self.call_runtime_scalar("osc_canvas_mouse_btn", &[], types::I32))
            }
            "canvas_wheel" => Some(self.call_runtime_scalar("osc_canvas_wheel", &[], types::I32)),

            // -- Canvas: calls that cross an osc_str and/or return a
            // Result ------------------------------------------------------
            "canvas_open" => Some(self.call_shim_out(
                "osc_canvas_open_shim",
                &result_ty("str", "str"),
                &[v!(0), v!(1), v!(2)],
            )),
            "canvas_set_icon" => Some(self.call_shim_out(
                "osc_canvas_set_icon_shim",
                &result_ty("str", "str"),
                &[v!(0), v!(1), v!(2)],
            )),

            // -- Clipboard --------------------------------------------------
            "clipboard_set" => {
                Some(self.call_runtime_scalar("osc_clipboard_set_shim", &[v!(0)], types::I32))
            }
            "clipboard_get" => Some(self.call_shim_out(
                "osc_clipboard_get_shim",
                &result_ty("str", "str"),
                &[arena],
            )),

            // -- Image decoding -----------------------------------------------
            "img_load" => Some(self.call_shim_out(
                "osc_img_load_shim",
                &result_ty("arr_i32", "str"),
                &[arena, v!(0)],
            )),

            // -- SVG rasterization ----------------------------------------------
            "svg_load" => Some(self.call_shim_out(
                "osc_svg_load_shim",
                &result_ty("arr_i32", "str"),
                &[arena, v!(0), v!(1), v!(2)],
            )),

            // -- TrueType: font handle lifecycle/metrics (plain scalars —
            // `handle` is already a bare pointer-sized value, called
            // directly) ----------------------------------------------------
            "tt_free" => {
                self.call_runtime_void("osc_tt_free", &[v!(0)]);
                None
            }
            "tt_ascent" => {
                Some(self.call_runtime_scalar("osc_tt_ascent", &[v!(0), v!(1)], types::I32))
            }
            "tt_descent" => {
                Some(self.call_runtime_scalar("osc_tt_descent", &[v!(0), v!(1)], types::I32))
            }
            "tt_line_gap" => {
                Some(self.call_runtime_scalar("osc_tt_line_gap", &[v!(0), v!(1)], types::I32))
            }
            "tt_line_height" => {
                Some(self.call_runtime_scalar("osc_tt_line_height", &[v!(0), v!(1)], types::I32))
            }

            // -- TrueType: calls that cross an osc_str and/or return a
            // Result ------------------------------------------------------
            "tt_load" => Some(self.call_shim_out(
                "osc_tt_load_shim",
                &result_ty("handle", "str"),
                &[arena, v!(0)],
            )),
            "tt_text_width" => Some(self.call_runtime_scalar(
                "osc_tt_text_width_shim",
                &[v!(0), v!(1), v!(2)],
                types::I32,
            )),
            "tt_draw_text" => Some(self.call_runtime_scalar(
                "osc_tt_draw_text_shim",
                &[v!(0), v!(1), v!(2), v!(3), v!(4), v!(5)],
                types::I32,
            )),

            _ => return self.lower_user_or_extern_call(callee, name, args, &a, ty, span),
        })
    }

    fn lower_user_or_extern_call(
        &mut self,
        callee: &oir::Callee,
        name: &str,
        args: &[oir::Expr],
        arg_vals: &[Option<Value>],
        ty: &BcType,
        span: Span,
    ) -> CResult<Option<Value>> {
        let _ = ty;
        match callee {
            oir::Callee::Var(_) => {
                let binding = self.lookup(name).unwrap_or_else(|| {
                    panic!("internal error: unknown function-pointer variable '{name}'")
                });
                let (param_tys, ret_ty) = match &binding.ty {
                    BcType::FnPtr(p, r) => (p.clone(), (**r).clone()),
                    other => panic!("internal error: '{name}' used as a call target has non-fn-ptr type '{other}'"),
                };
                let fn_addr = self.builder.use_var(binding.var);
                let mut sig = self.ctx.module.make_signature();
                sig.params.push(AbiParam::new(cl_pointer_type()));
                for pty in &param_tys {
                    if let Some(t) = Repr::of(pty, self.program()).cl_type() {
                        sig.params.push(AbiParam::new(t));
                    }
                }
                if let Some(t) = Repr::of(&ret_ty, self.program()).cl_type() {
                    sig.returns.push(AbiParam::new(t));
                }
                let sig_ref = self.builder.import_signature(sig);
                let mut call_args = vec![self.arena_value];
                call_args.extend(arg_vals.iter().filter_map(|v| *v));
                let call = self
                    .builder
                    .ins()
                    .call_indirect(sig_ref, fn_addr, &call_args);
                Ok(self.builder.inst_results(call).first().copied())
            }
            oir::Callee::Named(_) => {
                if let Some(&func_id) = self.ctx.functions.get(name) {
                    let func_ref = self
                        .ctx
                        .module
                        .declare_func_in_func(func_id, self.builder.func);
                    let mut call_args = vec![self.arena_value];
                    call_args.extend(arg_vals.iter().filter_map(|v| *v));
                    let call = self.builder.ins().call(func_ref, &call_args);
                    Ok(self.builder.inst_results(call).first().copied())
                } else if let Some((func_id, kind)) = self.resolve_extern(name, span)? {
                    let func_ref = self
                        .ctx
                        .module
                        .declare_func_in_func(func_id, self.builder.func);
                    let call_args: Vec<Value> = arg_vals.iter().filter_map(|v| *v).collect();
                    if matches!(kind, ExternDeclKind::NativeShim) && *ty == BcType::Str {
                        let out_ptr = self.arena_alloc(layout_of(ty, self.program()).size);
                        let mut shim_args = Vec::with_capacity(call_args.len() + 1);
                        shim_args.push(out_ptr);
                        shim_args.extend(call_args);
                        self.builder.ins().call(func_ref, &shim_args);
                        return Ok(Some(out_ptr));
                    }
                    let call = self.builder.ins().call(func_ref, &call_args);
                    Ok(self.builder.inst_results(call).first().copied())
                } else {
                    let _ = args;
                    Err(unsupported(
                        span,
                        format!("call to builtin function '{name}'"),
                    ))
                }
            }
        }
    }

    /// Resolve `name` to a declared `extern` function's `FuncId`,
    /// declaring it as a Cranelift import *lazily* — the first time it is
    /// actually called — rather than unconditionally for every `extern`
    /// block declaration up front (the old behavior). This matters
    /// because an `extern` block may declare functions a program never
    /// calls, e.g. purely to name a `handle`-typed API without needing a
    /// real implementation to link against (see
    /// `tests/positive/handle_type.osc`, which declares `fake_create`/
    /// `fake_destroy` only to exercise the `handle` type itself and never
    /// calls either): unconditionally declaring every one of them as a
    /// hard `Linkage::Import` symbol would demand the final link resolve
    /// a symbol the program never references, unlike `src/codegen.rs`'s
    /// C prototypes — an unused C `extern` declaration is inert, never
    /// becoming an object-file symbol reference unless something actually
    /// calls it.
    ///
    /// Returns `Ok(None)` if `name` isn't a declared extern at all (the
    /// caller then reports its own "unknown function" error). Returns
    /// `Err` if it *is* declared but its signature can't cross a real C
    /// ABI boundary (see `extern_fn_signature`) — using the call's own
    /// `span`, so an unused, ABI-incompatible declaration still never
    /// errors, matching C exactly, and a used one is reported at the
    /// call site that actually needs the unsupported ABI.
    fn resolve_extern(
        &mut self,
        name: &str,
        span: Span,
    ) -> CResult<Option<(FuncId, ExternDeclKind)>> {
        if let Some(&(func_id, kind)) = self.ctx.externs.get(name) {
            return Ok(Some((func_id, kind)));
        }
        let Some(ef) = self
            .program()
            .extern_blocks
            .iter()
            .flat_map(|b| &b.decls)
            .find(|ef| ef.name == name)
        else {
            return Ok(None);
        };
        let abi = extern_shim::classify(self.program(), &ef.name, &ef.params, &ef.return_type)
            .map_err(|reason| unsupported(span, reason))?;
        let (symbol, sig, kind) = match abi {
            NativeExternAbi::Direct => (
                crate::c_name::mangle_c_name(&ef.name),
                direct_extern_fn_signature(
                    &self.ctx.module,
                    self.program(),
                    &ef.name,
                    &ef.params,
                    &ef.return_type,
                    span,
                )?,
                ExternDeclKind::Direct,
            ),
            NativeExternAbi::Shim(shim) => {
                let symbol = shim.shim_symbol.clone();
                let sig = extern_shim_signature(
                    &self.ctx.module,
                    self.program(),
                    &ef.params,
                    &ef.return_type,
                );
                self.ctx.add_extern_shim(shim);
                (symbol, sig, ExternDeclKind::NativeShim)
            }
        };
        let func_id = self
            .ctx
            .module
            .declare_function(&symbol, Linkage::Import, &sig)
            .map_err(|e| {
                CompileError::new(
                    span,
                    format!("internal error declaring extern '{name}': {e}"),
                )
            })?;
        self.ctx.externs.insert(name.to_string(), (func_id, kind));
        Ok(Some((func_id, kind)))
    }
}

fn array_elem_ty_of(ty: &BcType) -> BcType {
    match ty {
        BcType::Array(e) | BcType::FixedArray(e, _) => (**e).clone(),
        _ => BcType::I32,
    }
}

fn result_ty(ok: &str, err: &str) -> BcType {
    let to_ty = |s: &str| match s {
        "i32" => BcType::I32,
        "i64" => BcType::I64,
        "str" => BcType::Str,
        "arr_i32" => BcType::Array(Box::new(BcType::I32)),
        "handle" => BcType::Handle,
        other => panic!("internal error: result_ty helper does not know type '{other}'"),
    };
    BcType::Result(Box::new(to_ty(ok)), Box::new(to_ty(err)))
}

/// Struct literals, enum construction (including `Result::Ok`/`Err`),
/// `match`, and `try` (`?`-style early return on `Result::Err`).
impl<'a, 'b> FuncTranslator<'a, 'b> {
    fn lower_struct_lit(
        &mut self,
        name: &str,
        fields: &'a [oir::FieldInit],
    ) -> CResult<Option<Value>> {
        let layout = layout_of(&BcType::Struct(name.to_string()), self.program());
        let ptr = self.arena_alloc(layout.size);
        for fi in fields {
            let raw = self.lower_expr(&fi.value)?;
            let (offset, field_ty) = struct_field_offset(name, &fi.name, self.program());
            let value = self.bind_value(&field_ty, raw, &fi.value)?;
            self.write_at(ptr, offset, &field_ty, value);
        }
        Ok(Some(ptr))
    }

    fn lower_enum_constructor(
        &mut self,
        enum_name: &str,
        variant: &str,
        args: &'a [oir::Expr],
        ty: &BcType,
        span: Span,
    ) -> CResult<Option<Value>> {
        if enum_name == "Result" {
            return self.lower_result_constructor(variant, args, ty, span);
        }
        if !layout::enum_has_payload(enum_name, self.program()) {
            // Simple int enum: the value *is* the tag.
            let tag = self.enum_variant_tag(enum_name, variant);
            return Ok(Some(
                self.builder
                    .ins()
                    .iconst(cranelift_codegen::ir::types::I32, tag as i64),
            ));
        }
        let el = enum_layout(enum_name, self.program());
        let ptr = self.arena_alloc(el.total.size);
        let tag = self.enum_variant_tag(enum_name, variant);
        let tag_val = self
            .builder
            .ins()
            .iconst(cranelift_codegen::ir::types::I32, tag as i64);
        self.store_scalar(tag_val, ptr, 0);

        let variant_info = self
            .program()
            .enums
            .get(enum_name)
            .and_then(|info| info.variants.iter().find(|(n, _)| n == variant))
            .map(|(_, tys)| tys.clone())
            .unwrap_or_default();
        let offsets = el
            .variant_field_offsets
            .get(variant)
            .cloned()
            .unwrap_or_default();
        for (i, arg) in args.iter().enumerate() {
            let raw = self.lower_expr(arg)?;
            let field_ty = variant_info.get(i).cloned().unwrap_or_else(|| arg.ty());
            let value = self.bind_value(&field_ty, raw, arg)?;
            self.write_at(ptr, offsets[i], &field_ty, value);
        }
        let _ = span;
        Ok(Some(ptr))
    }

    fn enum_variant_tag(&self, enum_name: &str, variant: &str) -> u32 {
        self.program()
            .enums
            .get(enum_name)
            .and_then(|info| info.variants.iter().position(|(n, _)| n == variant))
            .unwrap_or_else(|| panic!("internal error: unknown variant '{enum_name}::{variant}'"))
            as u32
    }

    /// `Result::Ok(v)` / `Result::Err(e)`, using the constructor's own
    /// resolved `ty` (the contextual `Result<T, E>`, which may differ from
    /// the enclosing function's return type) — mirrors
    /// `src/codegen.rs`'s `emit_result_constructor` exactly.
    fn lower_result_constructor(
        &mut self,
        variant: &str,
        args: &'a [oir::Expr],
        ty: &BcType,
        span: Span,
    ) -> CResult<Option<Value>> {
        let (ok_ty, err_ty) = match ty {
            BcType::Result(o, e) => ((**o).clone(), (**e).clone()),
            other => {
                return Err(unsupported(
                    span,
                    format!("Result constructor with non-Result type '{other}'"),
                ))
            }
        };
        let rl = result_layout(&ok_ty, &err_ty, self.program());
        let ptr = self.arena_alloc(rl.total.size);
        match variant {
            "Ok" => {
                let one = self
                    .builder
                    .ins()
                    .iconst(cranelift_codegen::ir::types::I8, 1);
                self.store_scalar(one, ptr, 0);
                if ok_ty != BcType::Unit {
                    let raw = self.lower_expr(&args[0])?;
                    let value = self.bind_value(&ok_ty, raw, &args[0])?;
                    self.write_at(ptr, rl.ok_offset, &ok_ty, value);
                }
            }
            "Err" => {
                let zero = self
                    .builder
                    .ins()
                    .iconst(cranelift_codegen::ir::types::I8, 0);
                self.store_scalar(zero, ptr, 0);
                let raw = self.lower_expr(&args[0])?;
                let value = self.bind_value(&err_ty, raw, &args[0])?;
                self.write_at(ptr, rl.err_offset, &err_ty, value);
            }
            other => {
                return Err(unsupported(
                    span,
                    format!("unknown Result variant '{other}'"),
                ))
            }
        }
        Ok(Some(ptr))
    }

    /// `try <call>`: evaluate `call` (a `Result<T, E>`-returning
    /// expression), and if it is `Err`, immediately return a fresh
    /// `Result<fn_ret_ok, E>::Err` built from the same error payload
    /// (byte-copied, since `E` is identical); otherwise the expression's
    /// value is the `Ok` payload. Mirrors `src/codegen.rs`'s `emit_try`.
    fn lower_try(
        &mut self,
        call: &'a oir::Expr,
        ty: &BcType,
        span: Span,
    ) -> CResult<Option<Value>> {
        let call_ty = call.ty();
        let (call_ok_ty, call_err_ty) = match &call_ty {
            BcType::Result(o, e) => ((**o).clone(), (**e).clone()),
            other => {
                return Err(unsupported(
                    span,
                    format!("'try' applied to non-Result type '{other}'"),
                ))
            }
        };
        let call_ptr = self.lower_expr(call)?.expect("Result value is a pointer");
        let call_rl = result_layout(&call_ok_ty, &call_err_ty, self.program());

        let is_ok = self.load_scalar(cranelift_codegen::ir::types::I8, call_ptr, 0);
        let err_blk = self.builder.create_block();
        let ok_blk = self.builder.create_block();
        self.builder.ins().brif(is_ok, ok_blk, &[], err_blk, &[]);

        self.goto(err_blk);
        self.builder.seal_block(err_blk);
        let fn_ret_ty = self.fn_return_ty.clone();
        let (fn_ok_ty, fn_err_ty) = match &fn_ret_ty {
            BcType::Result(o, e) => ((**o).clone(), (**e).clone()),
            other => {
                return Err(unsupported(
                    span,
                    format!("'try' inside a function not returning Result (returns '{other}')"),
                ))
            }
        };
        let fn_rl = result_layout(&fn_ok_ty, &fn_err_ty, self.program());
        let err_ret_ptr = self.arena_alloc(fn_rl.total.size);
        let zero = self
            .builder
            .ins()
            .iconst(cranelift_codegen::ir::types::I8, 0);
        self.store_scalar(zero, err_ret_ptr, 0);
        let err_layout = layout_of(&call_err_ty, self.program());
        self.copy_bytes(
            err_ret_ptr,
            fn_rl.err_offset as i32,
            call_ptr,
            call_rl.err_offset as i32,
            err_layout.size,
            err_layout.align,
        );
        self.emit_return(Some(err_ret_ptr))?;

        self.goto(ok_blk);
        self.builder.seal_block(ok_blk);
        let _ = ty;
        Ok(self.read_at(call_ptr, call_rl.ok_offset, &call_ok_ty))
    }

    fn lower_match(
        &mut self,
        scrutinee: &'a oir::Expr,
        arms: &'a [oir::MatchArm],
        ty: &BcType,
        span: Span,
    ) -> CResult<Option<Value>> {
        let scrut_ty = scrutinee.ty();
        let scrut_val = self.lower_expr(scrutinee)?;

        let merge_blk = self.builder.create_block();
        let repr_ty = Repr::of(ty, self.program()).cl_type();
        if let Some(t) = repr_ty {
            self.builder.append_block_param(merge_blk, t);
        }

        let n = arms.len();
        for (i, arm) in arms.iter().enumerate() {
            let is_last = i + 1 == n;
            if is_last {
                // Exhaustiveness was already verified by semantic analysis
                // (see `non_exhaustive_match.osc`): the last syntactic arm
                // is always reachable unconditionally once every earlier
                // arm's test has failed, so it is evaluated directly in
                // the current block rather than behind its own test.
                self.push_scope();
                self.bind_pattern(&arm.pattern, scrut_val, &scrut_ty)?;
                let arm_val = self.lower_expr(&arm.body)?;
                self.pop_scope();
                if !self.terminated {
                    let args = block_args(arm_val);
                    self.builder.ins().jump(merge_blk, &args);
                }
            } else {
                let body_blk = self.builder.create_block();
                let next_blk = self.builder.create_block();
                self.lower_pattern_test(
                    &arm.pattern,
                    scrut_val,
                    &scrut_ty,
                    body_blk,
                    next_blk,
                    span,
                )?;
                // Both blocks have exactly one predecessor — the brif
                // `lower_pattern_test` just emitted in the (now former)
                // current block — so both can be sealed immediately.
                self.builder.seal_block(body_blk);
                self.builder.seal_block(next_blk);

                self.goto(body_blk);
                self.push_scope();
                self.bind_pattern(&arm.pattern, scrut_val, &scrut_ty)?;
                let arm_val = self.lower_expr(&arm.body)?;
                self.pop_scope();
                if !self.terminated {
                    let args = block_args(arm_val);
                    self.builder.ins().jump(merge_blk, &args);
                }

                // `next_blk` becomes the current block for testing the
                // next arm's pattern (or, if this was the second-to-last
                // arm, for evaluating the last arm unconditionally).
                self.goto(next_blk);
            }
        }

        self.builder.seal_block(merge_blk);
        self.goto(merge_blk);
        Ok(if repr_ty.is_some() {
            Some(self.builder.block_params(merge_blk)[0])
        } else {
            None
        })
    }

    fn lower_pattern_test(
        &mut self,
        pattern: &oir::Pattern,
        scrut_val: Option<Value>,
        scrut_ty: &BcType,
        then_blk: CBlock,
        else_blk: CBlock,
        span: Span,
    ) -> CResult<()> {
        use cranelift_codegen::ir::types;
        match pattern {
            oir::Pattern::Wildcard(_) | oir::Pattern::Ident(_, _) => {
                self.builder.ins().jump(then_blk, &[]);
            }
            oir::Pattern::IntLit(v, _) => {
                let s = scrut_val.expect("int scrutinee has a value");
                let c = self.builder.ins().iconst(types::I32, *v);
                let cond = self.builder.ins().icmp(IntCC::Equal, s, c);
                self.builder.ins().brif(cond, then_blk, &[], else_blk, &[]);
            }
            oir::Pattern::FloatLit(v, _) => {
                let s = scrut_val.expect("float scrutinee has a value");
                let c = self
                    .builder
                    .ins()
                    .f64const(cranelift_codegen::ir::immediates::Ieee64::with_float(*v));
                let cond = self.builder.ins().fcmp(FloatCC::Equal, s, c);
                self.builder.ins().brif(cond, then_blk, &[], else_blk, &[]);
            }
            oir::Pattern::BoolLit(v, _) => {
                let s = scrut_val.expect("bool scrutinee has a value");
                let c = self.builder.ins().iconst(types::I8, if *v { 1 } else { 0 });
                let cond = self.builder.ins().icmp(IntCC::Equal, s, c);
                self.builder.ins().brif(cond, then_blk, &[], else_blk, &[]);
            }
            oir::Pattern::StringLit(v, _) => {
                let s = scrut_val.expect("str scrutinee has a value");
                let c = self.string_literal_ptr(v);
                let eq = self.call_runtime_scalar("osc_str_eq_shim", &[s, c], types::I8);
                self.builder.ins().brif(eq, then_blk, &[], else_blk, &[]);
            }
            oir::Pattern::Enum {
                enum_name, variant, ..
            } if enum_name == "Result" => {
                let ptr = scrut_val.expect("Result scrutinee has a value");
                let is_ok = self.load_scalar(types::I8, ptr, 0);
                let cond = if variant == "Ok" {
                    is_ok
                } else {
                    self.builder.ins().bxor_imm(is_ok, 1)
                };
                self.builder.ins().brif(cond, then_blk, &[], else_blk, &[]);
            }
            oir::Pattern::Enum {
                enum_name, variant, ..
            } => {
                let want = self.enum_variant_tag(enum_name, variant);
                let want_val = self.builder.ins().iconst(types::I32, want as i64);
                let tag = if layout::enum_has_payload(enum_name, self.program()) {
                    let ptr = scrut_val.expect("enum scrutinee has a value");
                    self.load_scalar(types::I32, ptr, 0)
                } else {
                    scrut_val.expect("enum scrutinee has a value")
                };
                let cond = self.builder.ins().icmp(IntCC::Equal, tag, want_val);
                self.builder.ins().brif(cond, then_blk, &[], else_blk, &[]);
            }
        }
        let _ = (scrut_ty, span);
        Ok(())
    }

    /// Bind whatever names `pattern` introduces (only `Ident` and
    /// `Enum { bindings, .. }` patterns introduce any) into the current
    /// (already-pushed) scope. Every inline-aggregate binding introduced
    /// here is materialized into its own fresh arena block (matching
    /// `src/codegen.rs`'s `T name = scrutinee...;`, a real C value copy),
    /// never a raw pointer into the scrutinee's own backing storage:
    /// otherwise a mutation of that backing storage (e.g. reassigning the
    /// array slot or struct field the scrutinee came from) later in the
    /// same arm's body would retroactively change an already-bound name
    /// — see `lower_for_in`'s identical reasoning for the analogous
    /// loop-variable binding, and `tests/positive/match_binding_copy_semantics.osc`.
    fn bind_pattern(
        &mut self,
        pattern: &oir::Pattern,
        scrut_val: Option<Value>,
        scrut_ty: &BcType,
    ) -> CResult<()> {
        match pattern {
            oir::Pattern::Wildcard(_)
            | oir::Pattern::IntLit(..)
            | oir::Pattern::FloatLit(..)
            | oir::Pattern::StringLit(..)
            | oir::Pattern::BoolLit(..) => Ok(()),
            oir::Pattern::Ident(name, _) => {
                let repr = Repr::of(scrut_ty, self.program());
                let var = self.fresh_var(repr.cl_type());
                if let Some(v) = scrut_val {
                    let v = if is_inline_aggregate(scrut_ty, self.program()) {
                        self.materialize_owned(scrut_ty, v)
                    } else {
                        v
                    };
                    self.builder.def_var(var, v);
                }
                self.scopes.last_mut().unwrap().insert(
                    name.clone(),
                    Binding {
                        var,
                        ty: scrut_ty.clone(),
                    },
                );
                Ok(())
            }
            oir::Pattern::Enum {
                enum_name,
                variant,
                bindings,
                ..
            } if enum_name == "Result" => {
                let (ok_ty, err_ty) = match scrut_ty {
                    BcType::Result(o, e) => ((**o).clone(), (**e).clone()),
                    other => panic!("internal error: Result pattern against non-Result scrutinee type '{other}'"),
                };
                let ptr = scrut_val.expect("Result scrutinee has a value");
                let rl = result_layout(&ok_ty, &err_ty, self.program());
                let (payload_ty, offset) = if variant == "Ok" {
                    (ok_ty, rl.ok_offset)
                } else {
                    (err_ty, rl.err_offset)
                };
                if let Some(oir::Pattern::Ident(name, _)) = bindings.first() {
                    let value = self.read_at(ptr, offset, &payload_ty);
                    let value = match value {
                        Some(v) if is_inline_aggregate(&payload_ty, self.program()) => {
                            Some(self.materialize_owned(&payload_ty, v))
                        }
                        other => other,
                    };
                    let repr = Repr::of(&payload_ty, self.program());
                    let var = self.fresh_var(repr.cl_type());
                    if let Some(v) = value {
                        self.builder.def_var(var, v);
                    }
                    self.scopes.last_mut().unwrap().insert(
                        name.clone(),
                        Binding {
                            var,
                            ty: payload_ty,
                        },
                    );
                }
                Ok(())
            }
            oir::Pattern::Enum {
                enum_name,
                variant,
                bindings,
                ..
            } => {
                let el = enum_layout(enum_name, self.program());
                let offsets = el
                    .variant_field_offsets
                    .get(variant)
                    .cloned()
                    .unwrap_or_default();
                let variant_field_tys = self
                    .program()
                    .enums
                    .get(enum_name)
                    .and_then(|info| info.variants.iter().find(|(n, _)| n == variant))
                    .map(|(_, tys)| tys.clone())
                    .unwrap_or_default();
                let ptr = scrut_val.expect("payload-bearing enum scrutinee has a value");
                for (i, sub) in bindings.iter().enumerate() {
                    if let oir::Pattern::Ident(name, _) = sub {
                        let field_ty = variant_field_tys[i].clone();
                        let value = self.read_at(ptr, offsets[i], &field_ty);
                        let value = match value {
                            Some(v) if is_inline_aggregate(&field_ty, self.program()) => {
                                Some(self.materialize_owned(&field_ty, v))
                            }
                            other => other,
                        };
                        let repr = Repr::of(&field_ty, self.program());
                        let var = self.fresh_var(repr.cl_type());
                        if let Some(v) = value {
                            self.builder.def_var(var, v);
                        }
                        self.scopes
                            .last_mut()
                            .unwrap()
                            .insert(name.clone(), Binding { var, ty: field_ty });
                    }
                    // Wildcard payload bindings need nothing; nested
                    // literal/enum sub-patterns inside a payload are
                    // rejected by the language grammar today (enum
                    // payload bindings are always plain identifiers or
                    // `_`), so there is nothing further to bind here.
                }
                Ok(())
            }
        }
    }
}
