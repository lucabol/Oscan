//! Deterministic textual LLVM IR emitter: the `--backend llvm`
//! implementation of [`crate::backend::lir`]'s `LirModule`/`LirBuilder`.
//!
//! # No C, ever
//!
//! Nothing in this file (or anything it calls) writes a `.c`/`.h` file,
//! spawns a compiler driver, or consults `crate::codegen`. The shared
//! translator in `src/backend/func.rs` produces LIR, this module turns
//! that LIR into LLVM IR text, and `super::provider` hands the text to
//! the bundled `libLLVM` in-process.
//!
//! # Representation choices
//!
//! * **Addresses are `i64`, not `ptr`.** Every [`LType::Ptr`] value is
//!   emitted as an `i64`, and `inttoptr` appears only immediately at the
//!   `load`/`store`/`call` that dereferences it. Oscan addresses are
//!   genuinely computed as integers (arena offsets, `osc_array_get`
//!   results, embedded-aggregate field arithmetic), so this is both the
//!   honest model and the conservative one: LLVM gets no pointer
//!   provenance to reason with, which is exactly the "no unjustified
//!   strong pointer attributes / no `inbounds`" policy this backend
//!   wants. Function *signatures* still use `ptr` where the C ABI does,
//!   with the conversion at the boundary.
//! * **Variables and block parameters are `alloca` slots.** The shared
//!   translator hands us Cranelift-style `def_var`/`use_var` and block
//!   parameters; both are realized as an entry-block `alloca` plus
//!   `store`/`load`. `mem2reg`/SROA in the optimization pipeline rebuild
//!   real SSA and phis from that, so the emitted IR stays simple and
//!   obviously correct while the optimized IR is fully register
//!   promoted.
//! * **`bool` is `i8`; a branch condition is "non-zero".** `brif` emits
//!   `icmp ne i8 %c, 0`, matching the Cranelift backend's semantics and
//!   the C backend's `if (x)`.
//! * **No poison-generating flags.** No `nsw`/`nuw`/`exact`/`inbounds`/
//!   fast-math is ever emitted. Oscan's arithmetic is checked by real
//!   runtime calls, so promising signed-overflow UB would be actively
//!   wrong.
//! * **Determinism.** Every emitted name is a monotonically-allocated
//!   integer or a stable symbol-derived string, module-level items are
//!   emitted in first-declaration order, and floating-point literals use
//!   LLVM's exact hexadecimal form. Compiling the same program twice
//!   produces byte-identical IR.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::backend::lir::{
    copy_chunks, FloatCmp, IntCmp, LBlock, LData, LFunc, LLinkage, LSig, LType, LValue, LVar,
    LirArtifact, LirBody, LirBuilder, LirError, LirModule,
};

/// LLVM type text for a LIR type in the *value* world. See module docs
/// for why `Ptr` is `i64`.
fn llvm_ty(ty: LType) -> &'static str {
    match ty {
        LType::I8 => "i8",
        LType::I32 => "i32",
        LType::I64 => "i64",
        LType::F64 => "double",
        LType::Ptr => "i64",
    }
}

/// The LLVM type used for a signature slot. Function parameters and
/// returns that carry an address use `ptr`, so emitted declarations match
/// the C prototypes in `runtime/osc_runtime.h` exactly.
fn llvm_abi_ty(ty: LType) -> &'static str {
    match ty {
        LType::Ptr => "ptr",
        other => llvm_ty(other),
    }
}

fn llvm_sig_text(sig: &LSig) -> String {
    let ret = match sig.ret {
        Some(t) => llvm_abi_ty(t),
        None => "void",
    };
    let params = sig
        .params
        .iter()
        .map(|t| llvm_abi_ty(*t))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{ret} ({params})")
}

/// LLVM's exact hexadecimal double literal syntax (`0x` + big-endian IEEE
/// 754 bits). Used instead of a decimal rendering so no value is ever
/// perturbed by formatting and the output is bit-for-bit reproducible.
fn llvm_double(value: f64) -> String {
    format!("0x{:016X}", value.to_bits())
}

/// Escape a byte string for LLVM's `c"..."` literal syntax.
fn llvm_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 8);
    for &b in bytes {
        if b == b'"' || b == b'\\' || !(0x20..0x7f).contains(&b) {
            let _ = write!(out, "\\{b:02X}");
        } else {
            out.push(b as char);
        }
    }
    out
}

/// Quote a symbol for LLVM. Oscan symbols are C identifiers, but user
/// `extern` names go through `c_name::mangle_c_name`, so be defensive.
fn llvm_symbol(symbol: &str) -> String {
    let plain = !symbol.is_empty()
        && symbol
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'$');
    if plain {
        format!("@{symbol}")
    } else {
        format!("@\"{}\"", llvm_bytes(symbol.as_bytes()))
    }
}

struct FuncDecl {
    symbol: String,
    sig: LSig,
    linkage: LLinkage,
    defined: bool,
}

enum DataDecl {
    /// A deduplicated string literal: `{ ptr data; i32 len; i32 pad; }`
    /// plus its NUL-terminated backing bytes.
    StringLiteral { index: u32, value: String },
    /// An imported runtime-owned global (`osc_global_argc`, ...).
    Import { symbol: String },
}

/// Module-level state, mirroring `lir_cranelift::ModuleState`.
struct ModuleState {
    target_triple: String,
    data_layout: String,
    funcs: Vec<FuncDecl>,
    func_handles: HashMap<String, LFunc>,
    datas: Vec<DataDecl>,
    data_handles: HashMap<String, LData>,
    string_handles: HashMap<String, LData>,
    next_string: u32,
    /// Emitted function definitions, in definition order.
    bodies: Vec<String>,
}

impl ModuleState {
    fn declare_function(
        &mut self,
        symbol: &str,
        sig: &LSig,
        linkage: LLinkage,
    ) -> Result<LFunc, String> {
        if let Some(existing) = self.func_handles.get(symbol) {
            let decl = &self.funcs[existing.index()];
            if &decl.sig != sig {
                return Err(format!(
                    "conflicting signatures for symbol '{symbol}': {} vs {}",
                    llvm_sig_text(&decl.sig),
                    llvm_sig_text(sig)
                ));
            }
            return Ok(*existing);
        }
        let handle = LFunc(self.funcs.len() as u32);
        self.funcs.push(FuncDecl {
            symbol: symbol.to_string(),
            sig: sig.clone(),
            linkage,
            defined: false,
        });
        self.func_handles.insert(symbol.to_string(), handle);
        Ok(handle)
    }

    fn string_literal_data(&mut self, value: &str) -> LData {
        if let Some(existing) = self.string_handles.get(value) {
            return *existing;
        }
        let index = self.next_string;
        self.next_string += 1;
        let handle = LData(self.datas.len() as u32);
        self.datas.push(DataDecl::StringLiteral {
            index,
            value: value.to_string(),
        });
        self.string_handles.insert(value.to_string(), handle);
        handle
    }

    fn declare_import_data(&mut self, symbol: &str) -> LData {
        if let Some(existing) = self.data_handles.get(symbol) {
            return *existing;
        }
        let handle = LData(self.datas.len() as u32);
        self.datas.push(DataDecl::Import {
            symbol: symbol.to_string(),
        });
        self.data_handles.insert(symbol.to_string(), handle);
        handle
    }

    /// The LLVM global name a data handle addresses.
    fn data_symbol(&self, data: LData) -> String {
        match &self.datas[data.index()] {
            DataDecl::StringLiteral { index, .. } => format!("@__osc_str_{index}"),
            DataDecl::Import { symbol } => llvm_symbol(symbol),
        }
    }
}

pub struct LlvmEmitter {
    state: ModuleState,
}

impl LlvmEmitter {
    pub fn new(target_triple: &str, data_layout: &str) -> Self {
        LlvmEmitter {
            state: ModuleState {
                target_triple: target_triple.to_string(),
                data_layout: data_layout.to_string(),
                funcs: Vec::new(),
                func_handles: HashMap::new(),
                datas: Vec::new(),
                data_handles: HashMap::new(),
                string_handles: HashMap::new(),
                next_string: 0,
                bodies: Vec::new(),
            },
        }
    }
}

impl LirModule for LlvmEmitter {
    fn declare_function(
        &mut self,
        symbol: &str,
        sig: &LSig,
        linkage: LLinkage,
    ) -> Result<LFunc, String> {
        self.state.declare_function(symbol, sig, linkage)
    }

    fn define_function(
        &mut self,
        func: LFunc,
        sig: &LSig,
        body: &mut LirBody<'_>,
    ) -> Result<(), LirError> {
        if self.state.funcs[func.index()].defined {
            return Err(LirError::Backend(format!(
                "function '{}' defined twice",
                self.state.funcs[func.index()].symbol
            )));
        }
        self.state.funcs[func.index()].defined = true;

        let mut fb = LlvmFuncBuilder::new(&mut self.state, func, sig);
        let params = fb.entry_params.clone();
        let result = body(&mut fb, &params);
        let text = fb.finish();
        // Report a translator diagnostic ahead of any emitter complaint:
        // an incomplete body is the *cause* of a malformed function, not
        // an independent failure.
        result?;
        self.state.bodies.push(text?);
        Ok(())
    }

    fn finish(&mut self) -> Result<LirArtifact, String> {
        let mut out = String::new();
        out.push_str("; ModuleID = 'oscan_program'\n");
        out.push_str("source_filename = \"oscan_program\"\n");
        let _ = writeln!(out, "target datalayout = \"{}\"", self.state.data_layout);
        let _ = writeln!(out, "target triple = \"{}\"", self.state.target_triple);
        out.push('\n');

        // --- globals -----------------------------------------------------
        for data in &self.state.datas {
            match data {
                DataDecl::StringLiteral { index, value } => {
                    let mut bytes = value.as_bytes().to_vec();
                    // Always append a trailing NUL, matching the C backend's
                    // `osc_str_from_cstr("...")` (a real C string literal).
                    // `len` below is the *Oscan* string length (excludes the
                    // NUL), same as `osc_str { data, len }`.
                    bytes.push(0);
                    let _ = writeln!(
                        out,
                        "@__osc_str_bytes_{index} = private unnamed_addr constant [{} x i8] c\"{}\", align 1",
                        bytes.len(),
                        llvm_bytes(&bytes)
                    );
                    // { const char* data; int32_t len; } — 16 bytes with the
                    // same trailing padding `layout.rs` computes for `str`.
                    let _ = writeln!(
                        out,
                        "@__osc_str_{index} = private unnamed_addr constant {{ ptr, i32, i32 }} {{ ptr @__osc_str_bytes_{index}, i32 {}, i32 0 }}, align 8",
                        value.len()
                    );
                }
                DataDecl::Import { symbol } => {
                    // Declared as `i8` rather than a concrete type: these
                    // are runtime-owned storage cells whose real size and
                    // type live in `runtime/osc_runtime.h`, and every
                    // access below goes through an explicitly-typed
                    // `load`/`store`, so an inaccurate declared type here
                    // would be a way to lie without any benefit.
                    let _ = writeln!(out, "{} = external global i8", llvm_symbol(symbol));
                }
            }
        }
        if !self.state.datas.is_empty() {
            out.push('\n');
        }

        // --- declarations ------------------------------------------------
        for decl in &self.state.funcs {
            if decl.defined {
                continue;
            }
            if decl.linkage == LLinkage::Export {
                return Err(format!(
                    "internal error: exported function '{}' was declared but never defined",
                    decl.symbol
                ));
            }
            let ret = match decl.sig.ret {
                Some(t) => llvm_abi_ty(t),
                None => "void",
            };
            let params = decl
                .sig
                .params
                .iter()
                .map(|t| llvm_abi_ty(*t))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "declare {ret} {}({params})", llvm_symbol(&decl.symbol));
        }
        out.push('\n');

        // --- definitions -------------------------------------------------
        for body in &self.state.bodies {
            out.push_str(body);
            out.push('\n');
        }

        Ok(LirArtifact::LlvmIr(out))
    }
}

/// One emitted basic block.
struct BlockBuf {
    /// `b<N>` label index.
    index: u32,
    body: String,
    /// Whether a terminator has already been written into `body`.
    terminated: bool,
    /// The value handle and backing slot of each block parameter, in
    /// append order.
    params: Vec<(LValue, LVar)>,
    /// How many `jump`/`brif` edges target this block.
    predecessors: u32,
}

pub struct LlvmFuncBuilder<'a> {
    state: &'a mut ModuleState,
    func: LFunc,
    sig: LSig,
    entry_params: Vec<LValue>,

    /// Rendered operand text (`%v3`, `42`) and type, per value.
    value_text: Vec<String>,
    value_types: Vec<LType>,

    /// Per-variable `alloca` slot name and type.
    var_slot: Vec<String>,
    var_types: Vec<LType>,

    blocks: Vec<BlockBuf>,
    current: usize,

    /// All `alloca`s, hoisted into the entry block so `mem2reg` can
    /// promote them.
    allocas: String,

    next_temp: u32,
    next_label: u32,
    /// A hard error discovered while emitting (e.g. a type mismatch that
    /// would produce invalid IR), surfaced by `finish`.
    error: Option<String>,
}

impl<'a> LlvmFuncBuilder<'a> {
    fn new(state: &'a mut ModuleState, func: LFunc, sig: &LSig) -> Self {
        let mut fb = LlvmFuncBuilder {
            state,
            func,
            sig: sig.clone(),
            entry_params: Vec::new(),
            value_text: Vec::new(),
            value_types: Vec::new(),
            var_slot: Vec::new(),
            var_types: Vec::new(),
            blocks: Vec::new(),
            current: 0,
            allocas: String::new(),
            next_temp: 0,
            next_label: 0,
            error: None,
        };
        fb.new_block();
        fb.current = 0;

        // Function parameters arrive as LLVM SSA values `%arg0..`; a `ptr`
        // parameter is immediately converted to the `i64` the value world
        // uses (see module docs).
        let params: Vec<LType> = sig.params.clone();
        for (i, ty) in params.into_iter().enumerate() {
            let handle = match ty {
                LType::Ptr => {
                    let tmp = fb.fresh_temp();
                    fb.emit(format!("{tmp} = ptrtoint ptr %arg{i} to i64"));
                    fb.push_value(tmp, LType::Ptr)
                }
                other => fb.push_value(format!("%arg{i}"), other),
            };
            fb.entry_params.push(handle);
        }
        fb
    }

    fn fail(&mut self, message: String) {
        if self.error.is_none() {
            self.error = Some(message);
        }
    }

    fn fresh_temp(&mut self) -> String {
        let name = format!("%v{}", self.next_temp);
        self.next_temp += 1;
        name
    }

    fn new_block(&mut self) -> LBlock {
        let index = self.next_label;
        self.next_label += 1;
        let handle = LBlock(self.blocks.len() as u32);
        self.blocks.push(BlockBuf {
            index,
            body: String::new(),
            terminated: false,
            params: Vec::new(),
            predecessors: 0,
        });
        handle
    }

    fn push_value(&mut self, text: String, ty: LType) -> LValue {
        let handle = LValue(self.value_text.len() as u32);
        self.value_text.push(text);
        self.value_types.push(ty);
        handle
    }

    fn text(&self, value: LValue) -> &str {
        &self.value_text[value.index()]
    }

    fn ty(&self, value: LValue) -> LType {
        self.value_types[value.index()]
    }

    /// `<type> <operand>`, the form every LLVM instruction operand takes.
    fn operand(&self, value: LValue) -> String {
        format!("{} {}", llvm_ty(self.ty(value)), self.text(value))
    }

    /// Append one instruction to the current block. Instructions after a
    /// terminator are dropped: the shared translator can legitimately keep
    /// lowering into a block it has already terminated (dead code after a
    /// `break`/`return` inside a nested expression), and LLVM rejects a
    /// block with instructions past its terminator.
    fn emit(&mut self, instruction: String) {
        let block = &mut self.blocks[self.current];
        if block.terminated {
            return;
        }
        block.body.push_str("  ");
        block.body.push_str(&instruction);
        block.body.push('\n');
    }

    fn emit_terminator(&mut self, instruction: String) {
        let block = &mut self.blocks[self.current];
        if block.terminated {
            return;
        }
        block.body.push_str("  ");
        block.body.push_str(&instruction);
        block.body.push('\n');
        block.terminated = true;
    }

    fn label(&self, block: LBlock) -> String {
        format!("%b{}", self.blocks[block.index()].index)
    }

    /// Whether two LIR types are the same *machine* type. [`LType::Ptr`]
    /// and [`LType::I64`] both live in an `i64` register in the value
    /// world (see module docs), and the language deliberately lets them
    /// mix — `handle as i64`, `str` byte addressing (`data_ptr + index`),
    /// and every other place an address is used arithmetically. The
    /// distinction only matters at an ABI boundary, which
    /// [`Self::as_abi_operand`] handles.
    fn same_machine_type(a: LType, b: LType) -> bool {
        llvm_ty(a) == llvm_ty(b)
    }

    /// A binary instruction over two same-typed operands.
    fn binop(&mut self, op: &str, a: LValue, b: LValue) -> LValue {
        let ty = self.ty(a);
        if !Self::same_machine_type(self.ty(b), ty) {
            self.fail(format!(
                "internal error: '{op}' operands have different types ({} vs {})",
                llvm_ty(ty),
                llvm_ty(self.ty(b))
            ));
        }
        let (x, y) = (self.text(a).to_string(), self.text(b).to_string());
        let tmp = self.fresh_temp();
        self.emit(format!("{tmp} = {op} {} {x}, {y}", llvm_ty(ty)));
        self.push_value(tmp, ty)
    }

    /// Materialize `value` as a `ptr` operand for a load/store/call.
    fn as_pointer(&mut self, value: LValue) -> String {
        match self.ty(value) {
            LType::Ptr | LType::I64 => {
                let src = self.text(value).to_string();
                let tmp = self.fresh_temp();
                self.emit(format!("{tmp} = inttoptr i64 {src} to ptr"));
                tmp
            }
            other => {
                self.fail(format!(
                    "internal error: a '{}' value is not usable as an address",
                    llvm_ty(other)
                ));
                "null".to_string()
            }
        }
    }

    /// Coerce `value` to the declared ABI type of a call argument/return.
    ///
    /// The only real conversion is address-shaped: the value world holds
    /// every address as `i64` (see module docs) while the ABI world spells
    /// the same thing `ptr`.
    fn as_abi_operand(&mut self, value: LValue, want: LType) -> String {
        let have = self.ty(value);
        if want == LType::Ptr {
            if !matches!(have, LType::Ptr | LType::I64) {
                self.fail(format!(
                    "internal error: cannot pass a '{}' value where 'ptr' is required",
                    llvm_ty(have)
                ));
                return "ptr poison".to_string();
            }
            let p = self.as_pointer(value);
            return format!("ptr {p}");
        }
        // Everything else must already agree in the value world; `Ptr` and
        // `I64` are both spelled `i64` there, which is exactly the
        // `handle`/`i64` cast pair the language allows.
        if llvm_ty(have) != llvm_ty(want) {
            self.fail(format!(
                "internal error: cannot pass a '{}' value where '{}' is required",
                llvm_ty(have),
                llvm_abi_ty(want)
            ));
            return format!("{} poison", llvm_abi_ty(want));
        }
        format!("{} {}", llvm_abi_ty(want), self.text(value))
    }

    /// Emit a call to `callee_text` (a global symbol or an SSA `ptr`).
    fn emit_call(&mut self, callee_text: &str, sig: &LSig, args: &[LValue]) -> Option<LValue> {
        if args.len() != sig.params.len() {
            self.fail(format!(
                "internal error: call to {callee_text} passes {} arguments, its signature declares {}",
                args.len(),
                sig.params.len()
            ));
            return sig.ret.map(|t| {
                let text = if t == LType::F64 { "0.0" } else { "0" };
                self.push_value(text.to_string(), t)
            });
        }
        let rendered: Vec<String> = args
            .iter()
            .zip(sig.params.iter())
            .map(|(a, want)| self.as_abi_operand(*a, *want))
            .collect();
        let joined = rendered.join(", ");
        match sig.ret {
            Some(ret) => {
                let tmp = self.fresh_temp();
                self.emit(format!(
                    "{tmp} = call {} {callee_text}({joined})",
                    llvm_abi_ty(ret)
                ));
                match ret {
                    LType::Ptr => {
                        let converted = self.fresh_temp();
                        self.emit(format!("{converted} = ptrtoint ptr {tmp} to i64"));
                        Some(self.push_value(converted, LType::Ptr))
                    }
                    other => Some(self.push_value(tmp, other)),
                }
            }
            None => {
                self.emit(format!("call void {callee_text}({joined})"));
                None
            }
        }
    }

    /// An `alloca` in the entry block. Hoisted out of the block stream so
    /// `mem2reg` always sees it in the first basic block, which is what
    /// makes it promotable.
    fn alloca(&mut self, ty_text: &str, align: u32) -> String {
        let name = format!("%s{}", self.next_temp);
        self.next_temp += 1;
        let _ = writeln!(
            self.allocas,
            "  {name} = alloca {ty_text}, align {}",
            align.max(1)
        );
        name
    }

    fn finish(self) -> Result<String, LirError> {
        if let Some(message) = self.error {
            return Err(LirError::Backend(message));
        }
        let decl = &self.state.funcs[self.func.index()];
        let ret = match self.sig.ret {
            Some(t) => llvm_abi_ty(t),
            None => "void",
        };
        let params = self
            .sig
            .params
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{} %arg{i}", llvm_abi_ty(*t)))
            .collect::<Vec<_>>()
            .join(", ");
        let mut out = String::new();
        // `nounwind`: Oscan has no exceptions and the runtime never
        // unwinds — it aborts. `minsize`/`optsize` match the C backend's
        // `-Oz` policy and guide both IR and machine-level optimization.
        // `"no-builtins"` is Clang's `-ffreestanding` marker: LLVM must not
        // assume libc entry points exist. Nothing stronger (no `nofree`, no
        // `willreturn`, no argument attributes) is promised.
        let _ = writeln!(
            out,
            "define {ret} {}({params}) minsize nounwind optsize \"no-builtins\" {{",
            llvm_symbol(&decl.symbol)
        );
        for (i, block) in self.blocks.iter().enumerate() {
            let _ = writeln!(out, "b{}:", block.index);
            if i == 0 {
                out.push_str(&self.allocas);
            }
            out.push_str(&block.body);
            if !block.terminated {
                // Every LLVM block needs a terminator. A block the shared
                // translator left open is unreachable by construction —
                // it only happens for a dead merge block whose every
                // predecessor already returned (see
                // `LirBuilder::is_unreachable`'s single use).
                out.push_str("  unreachable\n");
            }
        }
        out.push_str("}\n");
        Ok(out)
    }

    /// Store `args` into `target`'s block-parameter slots. Runs in the
    /// predecessor, before the branch, exactly like a phi's incoming
    /// value.
    fn write_block_args(&mut self, target: LBlock, args: &[LValue]) {
        let params = self.blocks[target.index()].params.clone();
        if params.len() != args.len() {
            self.fail(format!(
                "internal error: a branch passes {} arguments to a block with {} parameters",
                args.len(),
                params.len()
            ));
            return;
        }
        for ((_, var), value) in params.into_iter().zip(args.iter()) {
            self.def_var(var, *value);
        }
    }
}

impl LirBuilder for LlvmFuncBuilder<'_> {
    fn declare_function(
        &mut self,
        symbol: &str,
        sig: &LSig,
        linkage: LLinkage,
    ) -> Result<LFunc, String> {
        self.state.declare_function(symbol, sig, linkage)
    }

    fn string_literal_data(&mut self, value: &str) -> LData {
        self.state.string_literal_data(value)
    }

    fn declare_import_data(&mut self, symbol: &str) -> LData {
        self.state.declare_import_data(symbol)
    }

    fn create_block(&mut self) -> LBlock {
        self.new_block()
    }

    fn append_block_param(&mut self, block: LBlock, ty: LType) -> LValue {
        // Block parameters become memory: the predecessor stores the
        // argument into the slot, and the block reads it back on entry.
        // `mem2reg` turns the pair back into a real phi.
        let slot = self.alloca(llvm_ty(ty), ty.size());
        let var = LVar(self.var_slot.len() as u32);
        self.var_slot.push(slot);
        self.var_types.push(ty);
        // Placeholder text: `switch_to_block` rebinds this handle to the
        // load it emits at the top of `block`.
        let handle = self.push_value(String::new(), ty);
        self.blocks[block.index()].params.push((handle, var));
        handle
    }

    fn block_param(&self, block: LBlock, index: usize) -> LValue {
        self.blocks[block.index()].params[index].0
    }

    fn switch_to_block(&mut self, block: LBlock) {
        self.current = block.index();
        let params = self.blocks[block.index()].params.clone();
        for (handle, var) in params {
            let ty = self.var_types[var.index()];
            let slot = self.var_slot[var.index()].clone();
            let tmp = self.fresh_temp();
            self.emit(format!(
                "{tmp} = load {}, ptr {slot}, align {}",
                llvm_ty(ty),
                ty.size()
            ));
            self.value_text[handle.index()] = tmp;
        }
    }

    fn seal_block(&mut self, _block: LBlock) {
        // Sealing is a Cranelift SSA-construction concept; the
        // memory-based block parameters used here need no equivalent.
    }

    fn is_unreachable(&self) -> bool {
        self.current != 0 && self.blocks[self.current].predecessors == 0
    }

    fn declare_var(&mut self, ty: LType) -> LVar {
        let slot = self.alloca(llvm_ty(ty), ty.size());
        let handle = LVar(self.var_slot.len() as u32);
        self.var_slot.push(slot);
        self.var_types.push(ty);
        handle
    }

    fn def_var(&mut self, var: LVar, value: LValue) {
        let ty = self.var_types[var.index()];
        let slot = self.var_slot[var.index()].clone();
        if !Self::same_machine_type(self.ty(value), ty) {
            self.fail(format!(
                "internal error: storing a '{}' value into a '{}' variable",
                llvm_ty(self.ty(value)),
                llvm_ty(ty)
            ));
        }
        let operand = self.operand(value);
        self.emit(format!("store {operand}, ptr {slot}, align {}", ty.size()));
    }

    fn use_var(&mut self, var: LVar) -> LValue {
        let ty = self.var_types[var.index()];
        let slot = self.var_slot[var.index()].clone();
        let tmp = self.fresh_temp();
        self.emit(format!(
            "{tmp} = load {}, ptr {slot}, align {}",
            llvm_ty(ty),
            ty.size()
        ));
        self.push_value(tmp, ty)
    }

    fn value_type(&self, value: LValue) -> LType {
        self.ty(value)
    }

    fn iconst(&mut self, ty: LType, imm: i64) -> LValue {
        // Truncate to the declared width so the textual literal is always
        // in range for its type (LLVM rejects `i8 256`).
        let text = match ty {
            LType::I8 => (imm as i8).to_string(),
            LType::I32 => (imm as i32).to_string(),
            LType::I64 | LType::Ptr => imm.to_string(),
            LType::F64 => {
                self.fail("internal error: iconst with a floating type".to_string());
                "0".to_string()
            }
        };
        self.push_value(text, ty)
    }

    fn f64const(&mut self, imm: f64) -> LValue {
        self.push_value(llvm_double(imm), LType::F64)
    }

    fn iadd(&mut self, a: LValue, b: LValue) -> LValue {
        self.binop("add", a, b)
    }

    fn iadd_imm(&mut self, a: LValue, imm: i64) -> LValue {
        let ty = self.ty(a);
        let x = self.text(a).to_string();
        let tmp = self.fresh_temp();
        self.emit(format!("{tmp} = add {} {x}, {imm}", llvm_ty(ty)));
        self.push_value(tmp, ty)
    }

    fn band(&mut self, a: LValue, b: LValue) -> LValue {
        self.binop("and", a, b)
    }

    fn bor(&mut self, a: LValue, b: LValue) -> LValue {
        self.binop("or", a, b)
    }

    fn bxor(&mut self, a: LValue, b: LValue) -> LValue {
        self.binop("xor", a, b)
    }

    fn bxor_imm(&mut self, a: LValue, imm: i64) -> LValue {
        let ty = self.ty(a);
        let x = self.text(a).to_string();
        let tmp = self.fresh_temp();
        self.emit(format!("{tmp} = xor {} {x}, {imm}", llvm_ty(ty)));
        self.push_value(tmp, ty)
    }

    fn bnot(&mut self, a: LValue) -> LValue {
        let ty = self.ty(a);
        let x = self.text(a).to_string();
        let tmp = self.fresh_temp();
        self.emit(format!("{tmp} = xor {} {x}, -1", llvm_ty(ty)));
        self.push_value(tmp, ty)
    }

    fn ishl(&mut self, a: LValue, b: LValue) -> LValue {
        self.binop("shl", a, b)
    }

    fn ushr(&mut self, a: LValue, b: LValue) -> LValue {
        self.binop("lshr", a, b)
    }

    fn fadd(&mut self, a: LValue, b: LValue) -> LValue {
        self.binop("fadd", a, b)
    }

    fn fsub(&mut self, a: LValue, b: LValue) -> LValue {
        self.binop("fsub", a, b)
    }

    fn fmul(&mut self, a: LValue, b: LValue) -> LValue {
        self.binop("fmul", a, b)
    }

    fn fdiv(&mut self, a: LValue, b: LValue) -> LValue {
        self.binop("fdiv", a, b)
    }

    fn fneg(&mut self, a: LValue) -> LValue {
        let x = self.text(a).to_string();
        let tmp = self.fresh_temp();
        self.emit(format!("{tmp} = fneg double {x}"));
        self.push_value(tmp, LType::F64)
    }

    fn icmp(&mut self, cc: IntCmp, a: LValue, b: LValue) -> LValue {
        let pred = match cc {
            IntCmp::Eq => "eq",
            IntCmp::Ne => "ne",
            IntCmp::Slt => "slt",
            IntCmp::Sgt => "sgt",
            IntCmp::Sle => "sle",
            IntCmp::Sge => "sge",
        };
        let ty = self.ty(a);
        if !Self::same_machine_type(self.ty(b), ty) {
            self.fail(format!(
                "internal error: icmp operands have different types ({} vs {})",
                llvm_ty(ty),
                llvm_ty(self.ty(b))
            ));
        }
        let (x, y) = (self.text(a).to_string(), self.text(b).to_string());
        let bit = self.fresh_temp();
        self.emit(format!("{bit} = icmp {pred} {} {x}, {y}", llvm_ty(ty)));
        let byte = self.fresh_temp();
        self.emit(format!("{byte} = zext i1 {bit} to i8"));
        self.push_value(byte, LType::I8)
    }

    fn fcmp(&mut self, cc: FloatCmp, a: LValue, b: LValue) -> LValue {
        // Ordered predicates for everything but `!=`, which C (and the
        // Cranelift backend's `FloatCC::NotEqual`) define as "unordered or
        // not equal".
        let pred = match cc {
            FloatCmp::Eq => "oeq",
            FloatCmp::Ne => "une",
            FloatCmp::Lt => "olt",
            FloatCmp::Gt => "ogt",
            FloatCmp::Le => "ole",
            FloatCmp::Ge => "oge",
        };
        let (x, y) = (self.text(a).to_string(), self.text(b).to_string());
        let bit = self.fresh_temp();
        self.emit(format!("{bit} = fcmp {pred} double {x}, {y}"));
        let byte = self.fresh_temp();
        self.emit(format!("{byte} = zext i1 {bit} to i8"));
        self.push_value(byte, LType::I8)
    }

    fn uextend(&mut self, ty: LType, value: LValue) -> LValue {
        let from = self.ty(value);
        if from.size() >= ty.size() {
            self.fail(format!(
                "internal error: uextend from '{}' to '{}' does not widen",
                llvm_ty(from),
                llvm_ty(ty)
            ));
        }
        let x = self.text(value).to_string();
        let tmp = self.fresh_temp();
        self.emit(format!(
            "{tmp} = zext {} {x} to {}",
            llvm_ty(from),
            llvm_ty(ty)
        ));
        self.push_value(tmp, ty)
    }

    fn load(&mut self, ty: LType, addr: LValue, offset: i32) -> LValue {
        let base = if offset == 0 {
            addr
        } else {
            self.iadd_imm(addr, offset as i64)
        };
        let p = self.as_pointer(base);
        let tmp = self.fresh_temp();
        // `align 1`: no alignment is assumed anywhere (arena blocks are
        // 8-byte aligned, but an embedded aggregate field can sit at any
        // offset), matching the Cranelift backend's plain memory flags.
        self.emit(format!("{tmp} = load {}, ptr {p}, align 1", llvm_ty(ty)));
        self.push_value(tmp, ty)
    }

    fn store(&mut self, value: LValue, addr: LValue, offset: i32) {
        let base = if offset == 0 {
            addr
        } else {
            self.iadd_imm(addr, offset as i64)
        };
        let p = self.as_pointer(base);
        let operand = self.operand(value);
        self.emit(format!("store {operand}, ptr {p}, align 1"));
    }

    fn stack_slot_addr(&mut self, size: u32, align: u32) -> LValue {
        let slot = self.alloca(&format!("[{size} x i8]"), align);
        let tmp = self.fresh_temp();
        self.emit(format!("{tmp} = ptrtoint ptr {slot} to i64"));
        self.push_value(tmp, LType::Ptr)
    }

    fn mem_copy(&mut self, dest: LValue, src: LValue, size: u32, align: u32) {
        if size == 0 {
            return;
        }
        let _ = align;
        // Read every chunk before writing any of them, so an overlapping
        // copy behaves like `memmove` rather than a forward `memcpy` (see
        // `LirBuilder::mem_copy`'s contract). Only plain integer
        // load/store instructions are emitted — never `llvm.memcpy` —
        // because the freestanding runtime exports no linkable
        // `memcpy`/`memmove` symbol for the backend to fall back to.
        let chunks = copy_chunks(size);
        let mut loaded = Vec::with_capacity(chunks.len());
        for (offset, chunk) in &chunks {
            let ty_text = format!("i{}", chunk * 8);
            let addr = if *offset == 0 {
                src
            } else {
                self.iadd_imm(src, *offset as i64)
            };
            let p = self.as_pointer(addr);
            let tmp = self.fresh_temp();
            self.emit(format!("{tmp} = load {ty_text}, ptr {p}, align 1"));
            loaded.push((*offset, ty_text, tmp));
        }
        for (offset, ty_text, tmp) in loaded {
            let addr = if offset == 0 {
                dest
            } else {
                self.iadd_imm(dest, offset as i64)
            };
            let p = self.as_pointer(addr);
            self.emit(format!("store {ty_text} {tmp}, ptr {p}, align 1"));
        }
    }

    fn global_addr(&mut self, data: LData) -> LValue {
        let symbol = self.state.data_symbol(data);
        let tmp = self.fresh_temp();
        self.emit(format!("{tmp} = ptrtoint ptr {symbol} to i64"));
        self.push_value(tmp, LType::Ptr)
    }

    fn func_addr(&mut self, func: LFunc) -> LValue {
        let symbol = llvm_symbol(&self.state.funcs[func.index()].symbol);
        let tmp = self.fresh_temp();
        self.emit(format!("{tmp} = ptrtoint ptr {symbol} to i64"));
        self.push_value(tmp, LType::Ptr)
    }

    fn call(&mut self, func: LFunc, args: &[LValue]) -> Option<LValue> {
        let decl = &self.state.funcs[func.index()];
        let sig = decl.sig.clone();
        let symbol = llvm_symbol(&decl.symbol);
        self.emit_call(&symbol, &sig, args)
    }

    fn call_indirect(&mut self, sig: &LSig, callee: LValue, args: &[LValue]) -> Option<LValue> {
        let callee_ptr = self.as_pointer(callee);
        self.emit_call(&callee_ptr, sig, args)
    }

    fn jump(&mut self, target: LBlock, args: &[LValue]) {
        if self.blocks[self.current].terminated {
            return;
        }
        self.write_block_args(target, args);
        let label = self.label(target);
        self.blocks[target.index()].predecessors += 1;
        self.emit_terminator(format!("br label {label}"));
    }

    fn brif(
        &mut self,
        cond: LValue,
        then_block: LBlock,
        then_args: &[LValue],
        else_block: LBlock,
        else_args: &[LValue],
    ) {
        if self.blocks[self.current].terminated {
            return;
        }
        // A branch with block arguments on *both* edges would need the
        // stores to happen after the branch is decided; the shared
        // translator only ever passes arguments on one edge at a time
        // (`lower_short_circuit`), so this is checked rather than
        // supported.
        if !then_args.is_empty() && !else_args.is_empty() {
            self.fail(
                "internal error: conditional branch with arguments on both edges".to_string(),
            );
        }

        let cond_ty = self.ty(cond);
        let cond_text = self.text(cond).to_string();
        let bit = self.fresh_temp();
        self.emit(format!(
            "{bit} = icmp ne {} {cond_text}, 0",
            llvm_ty(cond_ty)
        ));

        if !then_args.is_empty() || !else_args.is_empty() {
            // Materialize the one-sided arguments in a dedicated landing
            // block so the stores only execute on the taken edge.
            let args: Vec<LValue> = if then_args.is_empty() {
                else_args.to_vec()
            } else {
                then_args.to_vec()
            };
            let arg_target = if then_args.is_empty() {
                else_block
            } else {
                then_block
            };
            let plain_target = if then_args.is_empty() {
                then_block
            } else {
                else_block
            };
            let landing = self.new_block();
            let landing_label = self.label(landing);
            let plain_label = self.label(plain_target);
            let (t_label, f_label) = if then_args.is_empty() {
                (plain_label, landing_label)
            } else {
                (landing_label, plain_label)
            };
            self.blocks[plain_target.index()].predecessors += 1;
            self.emit_terminator(format!("br i1 {bit}, label {t_label}, label {f_label}"));

            let saved = self.current;
            self.current = landing.index();
            self.write_block_args(arg_target, &args);
            let target_label = self.label(arg_target);
            self.blocks[arg_target.index()].predecessors += 1;
            self.emit_terminator(format!("br label {target_label}"));
            self.current = saved;
            return;
        }

        let t_label = self.label(then_block);
        let f_label = self.label(else_block);
        self.blocks[then_block.index()].predecessors += 1;
        self.blocks[else_block.index()].predecessors += 1;
        self.emit_terminator(format!("br i1 {bit}, label {t_label}, label {f_label}"));
    }

    fn ret(&mut self, value: Option<LValue>) {
        match (value, self.sig.ret) {
            (Some(v), Some(want)) => {
                let operand = self.as_abi_operand(v, want);
                self.emit_terminator(format!("ret {operand}"));
            }
            (None, None) => self.emit_terminator("ret void".to_string()),
            (Some(_), None) => {
                self.fail("internal error: value returned from a void function".to_string());
            }
            (None, Some(want)) => {
                // The shared translator only reaches this in provably dead
                // code (see `is_unreachable`); emit well-formed IR rather
                // than an invalid `ret void`.
                self.emit_terminator(format!("ret {} poison", llvm_abi_ty(want)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ir_of(build: impl FnOnce(&mut LlvmEmitter)) -> String {
        let mut emitter = LlvmEmitter::new("x86_64-unknown-linux-gnu", "e-m:e-p:64:64-i64:64");
        build(&mut emitter);
        match emitter.finish().expect("finish") {
            LirArtifact::LlvmIr(text) => text,
            LirArtifact::Object(_) => panic!("LLVM emitter must produce IR, not an object"),
        }
    }

    #[test]
    fn doubles_use_llvm_exact_hex_form() {
        assert_eq!(llvm_double(0.0), "0x0000000000000000");
        assert_eq!(llvm_double(1.0), "0x3FF0000000000000");
        assert_eq!(llvm_double(-0.5), "0xBFE0000000000000");
    }

    #[test]
    fn string_bytes_escape_quotes_backslashes_and_non_ascii() {
        assert_eq!(llvm_bytes(b"ok"), "ok");
        assert_eq!(llvm_bytes(b"a\"b"), "a\\22b");
        assert_eq!(llvm_bytes(b"a\\b"), "a\\5Cb");
        assert_eq!(llvm_bytes(b"a\nb"), "a\\0Ab");
        assert_eq!(llvm_bytes("é".as_bytes()), "\\C3\\A9");
    }

    #[test]
    fn symbols_are_quoted_only_when_they_need_it() {
        assert_eq!(llvm_symbol("oscan_main"), "@oscan_main");
        assert_eq!(llvm_symbol("osc.thing$1"), "@osc.thing$1");
        assert_eq!(llvm_symbol("weird name"), "@\"weird name\"");
    }

    #[test]
    fn pointer_values_are_i64_but_abi_slots_are_ptr() {
        assert_eq!(llvm_ty(LType::Ptr), "i64");
        assert_eq!(llvm_abi_ty(LType::Ptr), "ptr");
        assert_eq!(llvm_ty(LType::I8), "i8");
        assert_eq!(llvm_abi_ty(LType::I8), "i8");
    }

    #[test]
    fn conflicting_signatures_for_one_symbol_are_rejected() {
        let mut emitter = LlvmEmitter::new("x86_64-unknown-linux-gnu", "e-m:e-p:64:64");
        let a = LSig::new(vec![LType::I32], Some(LType::I32));
        let b = LSig::new(vec![LType::I64], Some(LType::I32));
        assert!(emitter.declare_function("f", &a, LLinkage::Import).is_ok());
        assert!(emitter.declare_function("f", &a, LLinkage::Import).is_ok());
        let err = emitter
            .declare_function("f", &b, LLinkage::Import)
            .unwrap_err();
        assert!(err.contains("conflicting signatures"), "{err}");
    }

    #[test]
    fn emitted_module_is_deterministic_and_carries_no_poison_flags() {
        let build = || {
            ir_of(|emitter| {
                let sig = LSig::new(vec![LType::Ptr], Some(LType::I32));
                let f = emitter
                    .declare_function("oscan_main", &sig, LLinkage::Export)
                    .expect("declare");
                emitter
                    .define_function(f, &sig, &mut |b, params| {
                        let s = b.string_literal_data("hi");
                        let addr = b.global_addr(s);
                        let loaded = b.load(LType::I32, addr, 8);
                        let one = b.iconst(LType::I32, 1);
                        let sum = b.iadd(loaded, one);
                        let _ = params;
                        b.ret(Some(sum));
                        Ok(())
                    })
                    .unwrap_or_else(|_| panic!("define failed"));
            })
        };

        let first = build();
        assert_eq!(first, build(), "IR emission must be deterministic");
        assert!(first.contains(
            "define i32 @oscan_main(ptr %arg0) minsize nounwind optsize \"no-builtins\" {"
        ));
        assert!(first
            .contains("@__osc_str_bytes_0 = private unnamed_addr constant [3 x i8] c\"hi\\00\""));
        assert!(!first.contains("nsw"), "no poison-generating flags");
        assert!(!first.contains("nuw"), "no poison-generating flags");
        assert!(!first.contains("inbounds"), "no inbounds");
        assert!(!first.contains("llvm.memcpy"), "no memcpy intrinsics");
    }

    #[test]
    fn aggregate_copies_read_every_chunk_before_writing_any() {
        let text = ir_of(|emitter| {
            let sig = LSig::new(vec![LType::Ptr, LType::Ptr], None);
            let f = emitter
                .declare_function("copy", &sig, LLinkage::Export)
                .expect("declare");
            emitter
                .define_function(f, &sig, &mut |b, params| {
                    b.mem_copy(params[0], params[1], 40, 8);
                    b.ret(None);
                    Ok(())
                })
                .unwrap_or_else(|_| panic!("define failed"));
        });
        let last_load = text.rfind("= load i").expect("loads emitted");
        let first_store = text.find("  store i").expect("stores emitted");
        assert!(
            last_load < first_store,
            "every load must precede every store so overlapping copies behave like memmove:\n{text}"
        );
        assert!(!text.contains("llvm.memcpy"));
        assert!(!text.contains("llvm.memmove"));
    }

    #[test]
    fn block_parameters_round_trip_through_a_slot() {
        let text = ir_of(|emitter| {
            let sig = LSig::new(vec![LType::I8], Some(LType::I32));
            let f = emitter
                .declare_function("pick", &sig, LLinkage::Export)
                .expect("declare");
            emitter
                .define_function(f, &sig, &mut |b, params| {
                    let then_blk = b.create_block();
                    let else_blk = b.create_block();
                    let merge = b.create_block();
                    b.append_block_param(merge, LType::I32);
                    b.brif(params[0], then_blk, &[], else_blk, &[]);

                    b.switch_to_block(then_blk);
                    let one = b.iconst(LType::I32, 1);
                    b.jump(merge, &[one]);

                    b.switch_to_block(else_blk);
                    let two = b.iconst(LType::I32, 2);
                    b.jump(merge, &[two]);

                    b.switch_to_block(merge);
                    let result = b.block_param(merge, 0);
                    b.ret(Some(result));
                    Ok(())
                })
                .unwrap_or_else(|_| panic!("define failed"));
        });
        assert!(text.contains("store i32 1, ptr %s0"), "{text}");
        assert!(text.contains("store i32 2, ptr %s0"), "{text}");
        assert!(text.contains("= load i32, ptr %s0"), "{text}");
        assert!(text.contains("br i1 "), "{text}");
    }

    #[test]
    fn one_sided_branch_arguments_get_a_landing_block() {
        let text = ir_of(|emitter| {
            let sig = LSig::new(vec![LType::I8], Some(LType::I8));
            let f = emitter
                .declare_function("shortcircuit", &sig, LLinkage::Export)
                .expect("declare");
            emitter
                .define_function(f, &sig, &mut |b, params| {
                    let rhs = b.create_block();
                    let merge = b.create_block();
                    b.append_block_param(merge, LType::I8);
                    b.brif(params[0], rhs, &[], merge, &[params[0]]);

                    b.switch_to_block(rhs);
                    let t = b.iconst(LType::I8, 1);
                    b.jump(merge, &[t]);

                    b.switch_to_block(merge);
                    let result = b.block_param(merge, 0);
                    b.ret(Some(result));
                    Ok(())
                })
                .unwrap_or_else(|_| panic!("define failed"));
        });
        // The `else` edge carries an argument, so it must route through a
        // landing block that performs the store before branching.
        assert!(text.contains("b3:"), "landing block emitted:\n{text}");
    }

    #[test]
    fn every_emitted_block_ends_in_a_terminator() {
        let text = ir_of(|emitter| {
            let sig = LSig::new(vec![], Some(LType::I32));
            let f = emitter
                .declare_function("dead", &sig, LLinkage::Export)
                .expect("declare");
            emitter
                .define_function(f, &sig, &mut |b, _params| {
                    let orphan = b.create_block();
                    let zero = b.iconst(LType::I32, 0);
                    b.ret(Some(zero));
                    b.switch_to_block(orphan);
                    assert!(b.is_unreachable());
                    Ok(())
                })
                .unwrap_or_else(|_| panic!("define failed"));
        });
        assert!(text.contains("unreachable"), "{text}");
    }
}
