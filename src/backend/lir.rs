//! Oscan-owned low-level IR (LIR): the shared semantic lowering layer
//! every object backend consumes.
//!
//! # Why this exists
//!
//! `crate::ir` is a *typed, structured* IR: blocks, `if`/`while`/`match`,
//! `defer`, `try`, aggregates-by-type. Turning that into machine code
//! requires a large body of genuinely language-specific decisions —
//! the implicit arena parameter, the pointer/scalar aggregate
//! representation, `bool` stored as `i8` but branched on as a condition,
//! copy-on-bind value semantics, checked arithmetic through runtime
//! calls, `Result` layout and `try` early-return, string interpolation,
//! the C-ABI entry wrapper's lifecycle, and so on. Duplicating that body
//! of decisions per backend is how two backends silently diverge.
//!
//! So `src/backend/func.rs` (the translator) is written *once*, against
//! this module's [`LirModule`] trait, and every backend supplies an
//! implementation:
//!
//! * `lir_cranelift::CraneliftLir` — Cranelift SSA + `cranelift-object`
//!   (`--backend native`).
//! * `llvm::emit::LlvmEmitter` — deterministic textual LLVM IR handed to
//!   the in-process bundled `libLLVM` (`--backend llvm`).
//!
//! The trait is deliberately *low level and total*: it has no notion of
//! Oscan types, arenas, aggregates, or `Result`. Everything above the
//! line "load 4 bytes at `ptr + 12`" lives in `func.rs`, so a new backend
//! can only differ in how it encodes those primitives, never in what the
//! language means.
//!
//! # Design notes
//!
//! * **Handles, not types.** `LValue`/`LBlock`/`LVar`/`LFunc`/`LData` are
//!   opaque `u32` newtypes minted by the implementation. The translator
//!   only ever passes them back, so no backend type ever leaks into the
//!   shared layer.
//! * **`Option<LValue>` is the Unit convention.** Mirrors the existing
//!   Cranelift translator exactly: `None` means "no value at all".
//! * **Block parameters, not phis.** The translator produces
//!   Cranelift-style block parameters (see [`LirBuilder::append_block_param`]);
//!   a backend without them (LLVM) is free to implement them with
//!   memory (`alloca` + `store`/`load`) and let `mem2reg`/SROA rebuild
//!   the phis. That is strictly sound and keeps the translator free of
//!   SSA-construction bookkeeping.
//! * **`bool` is `i8` in memory, and a condition is "non-zero".**
//!   [`LirBuilder::brif`] takes an `i8`-repr value; the implementation is
//!   responsible for the truncation/compare its own IR needs.
//! * **No poison.** Nothing here is allowed to carry `nsw`/`nuw`/
//!   `inbounds`/`exact`/fast-math semantics. Oscan's checked arithmetic
//!   is implemented by real runtime calls, and pointer arithmetic is
//!   plain integer arithmetic on addresses.

use std::fmt;

macro_rules! lir_handle {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            #[allow(dead_code)]
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

lir_handle!(
    /// An SSA value produced by an instruction, block parameter, or
    /// function parameter.
    LValue
);
lir_handle!(
    /// A basic block within the function currently being defined.
    LBlock
);
lir_handle!(
    /// A mutable local slot (the translator's Oscan-level bindings and
    /// loop counters). Backends may implement this as real SSA variables
    /// (Cranelift) or as an `alloca` (LLVM).
    LVar
);
lir_handle!(
    /// A module-level function: declared once, referenced by every call
    /// site and `func_addr`.
    LFunc
);
lir_handle!(
    /// A module-level data object (string-literal headers, imported
    /// runtime globals).
    LData
);

/// The machine-level type of an LIR value.
///
/// Deliberately tiny: Oscan's entire value model is `i8` (`bool`, tags,
/// raw bytes), `i32`, `i64`, `f64`, and a 64-bit address. Every target
/// this backend family supports is LP64/64-bit-pointer (see
/// [`crate::backend::layout::POINTER_SIZE`]), so `Ptr` is exactly 8 bytes
/// and is freely interconvertible with `I64` at the ABI level — which is
/// what the runtime's `handle` type relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LType {
    I8,
    I32,
    I64,
    F64,
    Ptr,
}

impl LType {
    /// Size in bytes.
    pub fn size(self) -> u32 {
        match self {
            LType::I8 => 1,
            LType::I32 => 4,
            LType::I64 | LType::F64 | LType::Ptr => 8,
        }
    }

    #[cfg(test)]
    pub fn is_float(self) -> bool {
        matches!(self, LType::F64)
    }

    /// Whether this type is carried in an integer register (everything
    /// but `f64`). `Ptr` counts: Oscan addresses are plain 64-bit
    /// integers at this level, never provenance-carrying pointers.
    #[cfg(test)]
    pub fn is_integral(self) -> bool {
        !self.is_float()
    }
}

impl fmt::Display for LType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LType::I8 => "i8",
            LType::I32 => "i32",
            LType::I64 => "i64",
            LType::F64 => "f64",
            LType::Ptr => "ptr",
        })
    }
}

/// A function signature at the LIR level: zero or more parameters and at
/// most one result (the `Option<LValue>`/Unit convention).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct LSig {
    pub params: Vec<LType>,
    pub ret: Option<LType>,
}

impl LSig {
    pub fn new(params: Vec<LType>, ret: Option<LType>) -> Self {
        LSig { params, ret }
    }

    pub fn push_param(&mut self, ty: LType) {
        self.params.push(ty);
    }
}

/// Module-level symbol visibility. Oscan objects only ever export the
/// program's own functions and import runtime/extern symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LLinkage {
    /// Defined by this object and visible to the linker.
    Export,
    /// Declared here, defined by the runtime archive / user C.
    Import,
}

/// Integer comparison predicates. Every Oscan integer comparison is
/// signed (`i32`/`i64`/`bool`/`handle` all compare with signed
/// predicates today — see `func.rs`'s `lower_binop`), so only signed
/// orderings are exposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntCmp {
    Eq,
    Ne,
    Slt,
    Sgt,
    Sle,
    Sge,
}

/// Floating-point comparison predicates. These are the *ordered*
/// comparisons (`Ne` is the unordered-or-not-equal form, matching
/// Cranelift's `FloatCC::NotEqual` and C's `!=`), and no backend may
/// attach fast-math flags to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatCmp {
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

/// What a finished [`LirModule`] produced.
pub enum LirArtifact {
    /// A relocatable object file's raw bytes (Cranelift).
    Object(Vec<u8>),
    /// Textual LLVM IR, ready for the in-process provider.
    LlvmIr(String),
}

/// Failure of a function definition: either the shared translator
/// rejected the program (a real user-facing diagnostic) or the backend
/// itself failed (an internal compiler error the caller re-spans).
pub enum LirError {
    Body(crate::error::CompileError),
    Backend(String),
}

impl From<crate::error::CompileError> for LirError {
    fn from(value: crate::error::CompileError) -> Self {
        LirError::Body(value)
    }
}

/// The body of one function definition, as handed to
/// [`LirModule::define_function`]: receives the open builder and the
/// entry block's parameter values (one per signature parameter, in
/// order).
pub type LirBody<'f> =
    dyn FnMut(&mut dyn LirBuilder, &[LValue]) -> Result<(), crate::error::CompileError> + 'f;

/// Module-level operations.
///
/// Kept separate from [`LirBuilder`] because a Cranelift-style backend
/// cannot hold its `FunctionBuilder` across calls (it borrows the
/// in-progress `Function`), so function bodies are supplied as a
/// callback rather than bracketed by begin/end calls.
pub trait LirModule {
    /// Declare (or look up) a function symbol. Calling this twice with
    /// the same symbol must return the same [`LFunc`]; implementations
    /// may assume the signature agrees (the translator caches by name
    /// and never varies a symbol's signature between call sites).
    fn declare_function(
        &mut self,
        symbol: &str,
        sig: &LSig,
        linkage: LLinkage,
    ) -> Result<LFunc, String>;

    /// Define `func`: create its entry block, run `body` against an open
    /// builder, then finalize and install the result.
    fn define_function(
        &mut self,
        func: LFunc,
        sig: &LSig,
        body: &mut LirBody<'_>,
    ) -> Result<(), LirError>;

    /// Finish the module and hand back its artifact. Called exactly once.
    fn finish(&mut self) -> Result<LirArtifact, String>;
}

/// The complete set of primitives `src/backend/func.rs` needs while
/// lowering one function body.
pub trait LirBuilder {
    // -- Module level (reachable from inside a body) ---------------------

    /// Same contract as [`LirModule::declare_function`]; used for the
    /// lazily-declared runtime/extern imports a body discovers as it is
    /// lowered.
    fn declare_function(
        &mut self,
        symbol: &str,
        sig: &LSig,
        linkage: LLinkage,
    ) -> Result<LFunc, String>;

    /// Get-or-create the deduplicated 16-byte `{ const char* data;
    /// int32_t len; }` header cell for a string literal, plus its
    /// NUL-terminated backing bytes. Returns the header's handle.
    fn string_literal_data(&mut self, value: &str) -> LData;

    /// Declare an imported (runtime-owned) module-level global by symbol
    /// name. Repeat calls with the same symbol return the same handle.
    fn declare_import_data(&mut self, symbol: &str) -> LData;

    // -- Blocks ----------------------------------------------------------

    fn create_block(&mut self) -> LBlock;
    /// Append a parameter of `ty` to `block` and return its value.
    fn append_block_param(&mut self, block: LBlock, ty: LType) -> LValue;
    /// The `index`-th parameter previously appended to `block`.
    fn block_param(&self, block: LBlock, index: usize) -> LValue;
    /// Make `block` the block subsequent instructions are appended to.
    fn switch_to_block(&mut self, block: LBlock);
    /// Declare that `block` will gain no further predecessors.
    fn seal_block(&mut self, block: LBlock);

    /// Whether the block currently being appended to is provably
    /// unreachable (not the entry block, sealed, and with no
    /// predecessors). Used exactly once — to decide whether a function
    /// body still needs its implicit tail `return` after every path
    /// already returned — where emitting a value-less `return` into a
    /// dead merge block of a value-returning function would otherwise
    /// produce a signature mismatch.
    fn is_unreachable(&self) -> bool;

    // -- Variables -------------------------------------------------------

    fn declare_var(&mut self, ty: LType) -> LVar;
    fn def_var(&mut self, var: LVar, value: LValue);
    fn use_var(&mut self, var: LVar) -> LValue;

    // -- Values ----------------------------------------------------------

    fn value_type(&self, value: LValue) -> LType;

    fn iconst(&mut self, ty: LType, imm: i64) -> LValue;
    fn f64const(&mut self, imm: f64) -> LValue;

    fn iadd(&mut self, a: LValue, b: LValue) -> LValue;
    /// `a + imm`, with `a` an address or integer of any integral width.
    fn iadd_imm(&mut self, a: LValue, imm: i64) -> LValue;

    fn band(&mut self, a: LValue, b: LValue) -> LValue;
    fn bor(&mut self, a: LValue, b: LValue) -> LValue;
    fn bxor(&mut self, a: LValue, b: LValue) -> LValue;
    fn bxor_imm(&mut self, a: LValue, imm: i64) -> LValue;
    fn bnot(&mut self, a: LValue) -> LValue;
    fn ishl(&mut self, a: LValue, b: LValue) -> LValue;
    /// Logical (zero-filling) right shift.
    fn ushr(&mut self, a: LValue, b: LValue) -> LValue;

    fn fadd(&mut self, a: LValue, b: LValue) -> LValue;
    fn fsub(&mut self, a: LValue, b: LValue) -> LValue;
    fn fmul(&mut self, a: LValue, b: LValue) -> LValue;
    fn fdiv(&mut self, a: LValue, b: LValue) -> LValue;
    fn fneg(&mut self, a: LValue) -> LValue;

    /// Integer comparison; the result is an `i8` 0/1 value (the same
    /// representation Oscan `bool` uses in memory).
    fn icmp(&mut self, cc: IntCmp, a: LValue, b: LValue) -> LValue;
    /// Floating comparison; the result is an `i8` 0/1 value.
    fn fcmp(&mut self, cc: FloatCmp, a: LValue, b: LValue) -> LValue;

    /// Zero-extend `value` to the wider integral type `ty`.
    fn uextend(&mut self, ty: LType, value: LValue) -> LValue;

    // -- Memory ----------------------------------------------------------

    fn load(&mut self, ty: LType, addr: LValue, offset: i32) -> LValue;
    fn store(&mut self, value: LValue, addr: LValue, offset: i32);

    /// Allocate `size` bytes of function-local scratch and return its
    /// address. Only used for handing a *scalar's* address to a runtime
    /// call that copies immediately (`osc_array_push`), never for
    /// anything whose lifetime escapes the call.
    fn stack_slot_addr(&mut self, size: u32, align: u32) -> LValue;

    /// Copy exactly `size` bytes from `src` to `dest`.
    ///
    /// Implementations MUST:
    /// * emit real load/store instructions, never a call to a libc
    ///   `memcpy`/`memmove` symbol (the freestanding runtime exports
    ///   neither — see `func.rs`'s `copy_bytes` docs), and
    /// * be safe for overlapping ranges (read-before-write), because
    ///   `write_at` can legitimately be handed a source that lives
    ///   inside the destination aggregate.
    fn mem_copy(&mut self, dest: LValue, src: LValue, size: u32, align: u32);

    /// The address of a module-level data object.
    fn global_addr(&mut self, data: LData) -> LValue;
    /// The address of a module-level function.
    fn func_addr(&mut self, func: LFunc) -> LValue;

    // -- Calls & terminators ---------------------------------------------

    fn call(&mut self, func: LFunc, args: &[LValue]) -> Option<LValue>;
    fn call_indirect(&mut self, sig: &LSig, callee: LValue, args: &[LValue]) -> Option<LValue>;

    fn jump(&mut self, target: LBlock, args: &[LValue]);
    fn brif(
        &mut self,
        cond: LValue,
        then_block: LBlock,
        then_args: &[LValue],
        else_block: LBlock,
        else_args: &[LValue],
    );
    fn ret(&mut self, value: Option<LValue>);
}

/// Split a byte copy of `size` bytes into a deterministic sequence of
/// power-of-two chunks no larger than `MAX_COPY_CHUNK`, greedily
/// largest-first.
///
/// Shared by every backend so aggregate copies have identical shape
/// regardless of code generator. Each chunk is small enough that a
/// backend can always realize it as plain load/store instructions,
/// which is what keeps freestanding links free of `memcpy`/`memmove`
/// references (see [`LirBuilder::mem_copy`]).
pub const MAX_COPY_CHUNK: u32 = 32;

/// Yields `(offset, chunk_size)` pairs covering `[0, size)`.
pub fn copy_chunks(size: u32) -> Vec<(u32, u32)> {
    let mut chunks = Vec::new();
    let mut offset = 0;
    while offset < size {
        let remaining = size - offset;
        let mut chunk = MAX_COPY_CHUNK;
        while chunk > remaining {
            chunk /= 2;
        }
        chunks.push((offset, chunk));
        offset += chunk;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_chunks_cover_the_exact_range_with_power_of_two_pieces() {
        for size in 0..=200u32 {
            let chunks = copy_chunks(size);
            let mut expected_offset = 0;
            for (offset, chunk) in &chunks {
                assert_eq!(*offset, expected_offset, "size {size} has a gap");
                assert!(chunk.is_power_of_two(), "size {size} chunk {chunk}");
                assert!(*chunk <= MAX_COPY_CHUNK, "size {size} chunk {chunk}");
                expected_offset += chunk;
            }
            assert_eq!(expected_offset, size, "size {size} is not fully covered");
        }
    }

    #[test]
    fn copy_chunks_are_greedy_largest_first() {
        assert_eq!(copy_chunks(0), vec![]);
        assert_eq!(copy_chunks(1), vec![(0, 1)]);
        assert_eq!(copy_chunks(40), vec![(0, 32), (32, 8)]);
        assert_eq!(copy_chunks(12), vec![(0, 8), (8, 4)]);
    }

    #[test]
    fn ltype_sizes_match_the_lp64_model() {
        assert_eq!(LType::I8.size(), 1);
        assert_eq!(LType::I32.size(), 4);
        assert_eq!(LType::I64.size(), 8);
        assert_eq!(LType::F64.size(), 8);
        assert_eq!(LType::Ptr.size(), 8);
        assert!(LType::F64.is_float());
        assert!(LType::Ptr.is_integral());
    }
}
