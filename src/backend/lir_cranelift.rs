//! Cranelift implementation of [`super::lir`]'s `LirModule`/`LirBuilder`.
//!
//! Owns the `cranelift-object` `ObjectModule` (in [`CraneliftLir`]) and,
//! for the duration of one `define_function` call, the in-progress
//! `Function` plus its `FunctionBuilder` (in [`CraneliftBuilder`]).
//! Everything that used to live inline in `func.rs` — memory flags, the
//! `emit_small_memory_copy` chunking that avoids libc `memcpy` calls, the
//! string-literal data layout — is preserved verbatim here; this file is
//! a mechanical re-homing of that code, not a rewrite.

use std::collections::HashMap;

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    types, AbiParam, Function, InstBuilder, MemFlagsData, Signature, StackSlotData, StackSlotKind,
    UserFuncName, Value,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use super::lir::{
    FloatCmp, IntCmp, LBlock, LData, LFunc, LLinkage, LSig, LType, LValue, LVar, LirArtifact,
    LirBody, LirBuilder, LirError, LirModule,
};
use super::target::{self, NativeTarget};
use super::OptimizationProfile;

/// The flags used for every plain scalar/pointer load and store this
/// backend emits. Not `trusted()`: Oscan values can indeed be read at
/// attacker/user-influenced offsets (e.g. array/string indexing), which
/// osc_array/osc_str bounds-check at the *runtime call* level (see
/// `osc_array_get`/`osc_str_check_index`) before a raw pointer ever
/// reaches one of these loads/stores, but a plain `MemFlagsData::new()`
/// (no `notrap`/`aligned` assumptions) is the conservative,
/// always-correct choice.
fn mem_flags() -> MemFlagsData {
    MemFlagsData::new()
}

fn cl_type(ty: LType) -> cranelift_codegen::ir::Type {
    match ty {
        LType::I8 => types::I8,
        LType::I32 => types::I32,
        LType::I64 | LType::Ptr => types::I64,
        LType::F64 => types::F64,
    }
}

fn cl_signature(module: &impl Module, sig: &LSig) -> Signature {
    let mut out = module.make_signature();
    for p in &sig.params {
        out.params.push(AbiParam::new(cl_type(*p)));
    }
    if let Some(r) = sig.ret {
        out.returns.push(AbiParam::new(cl_type(r)));
    }
    out
}

fn int_cc(cc: IntCmp) -> IntCC {
    match cc {
        IntCmp::Eq => IntCC::Equal,
        IntCmp::Ne => IntCC::NotEqual,
        IntCmp::Slt => IntCC::SignedLessThan,
        IntCmp::Sgt => IntCC::SignedGreaterThan,
        IntCmp::Sle => IntCC::SignedLessThanOrEqual,
        IntCmp::Sge => IntCC::SignedGreaterThanOrEqual,
    }
}

fn float_cc(cc: FloatCmp) -> FloatCC {
    match cc {
        FloatCmp::Eq => FloatCC::Equal,
        FloatCmp::Ne => FloatCC::NotEqual,
        FloatCmp::Lt => FloatCC::LessThan,
        FloatCmp::Gt => FloatCC::GreaterThan,
        FloatCmp::Le => FloatCC::LessThanOrEqual,
        FloatCmp::Ge => FloatCC::GreaterThanOrEqual,
    }
}

/// Module-level state: the object module plus every cross-function
/// lookup table. Split out from [`CraneliftLir`] so a
/// [`CraneliftBuilder`] can borrow it for the lifetime of one function
/// body while the in-progress `Function` lives on `define_function`'s
/// own stack frame (a `FunctionBuilder` borrows that `Function`, so it
/// can never be stored alongside the module it is built into).
struct ModuleState {
    module: Option<ObjectModule>,
    /// Dense `LFunc` -> Cranelift id / return type, plus a symbol cache
    /// so repeat declarations share one handle.
    func_ids: Vec<FuncId>,
    func_rets: Vec<Option<LType>>,
    func_handles: HashMap<String, LFunc>,
    /// Dense `LData` -> Cranelift id, plus the symbol/string-content
    /// caches that dedupe declarations.
    data_ids: Vec<DataId>,
    data_handles: HashMap<String, LData>,
    string_handles: HashMap<String, LData>,
    next_anon_data: u32,
}

impl ModuleState {
    fn module(&mut self) -> &mut ObjectModule {
        self.module
            .as_mut()
            .expect("internal error: Cranelift module already finished")
    }

    fn module_ref(&self) -> &ObjectModule {
        self.module
            .as_ref()
            .expect("internal error: Cranelift module already finished")
    }

    fn declare_function(
        &mut self,
        symbol: &str,
        sig: &LSig,
        linkage: LLinkage,
    ) -> Result<LFunc, String> {
        if let Some(existing) = self.func_handles.get(symbol) {
            return Ok(*existing);
        }
        let cl_sig = cl_signature(self.module_ref(), sig);
        let cl_linkage = match linkage {
            LLinkage::Export => Linkage::Export,
            LLinkage::Local => Linkage::Local,
            LLinkage::Import => Linkage::Import,
        };
        let id = self
            .module()
            .declare_function(symbol, cl_linkage, &cl_sig)
            .map_err(|e| format!("failed to declare function '{symbol}': {e}"))?;
        let handle = LFunc(self.func_ids.len() as u32);
        self.func_ids.push(id);
        self.func_rets.push(sig.ret);
        self.func_handles.insert(symbol.to_string(), handle);
        Ok(handle)
    }

    fn declare_anonymous_data(&mut self, writable: bool) -> DataId {
        let name = format!("__osc_data_{}", self.next_anon_data);
        self.next_anon_data += 1;
        self.module()
            .declare_data(&name, Linkage::Local, writable, false)
            .unwrap_or_else(|e| {
                panic!("internal error: failed to declare anonymous data '{name}': {e}")
            })
    }

    /// Get-or-create the `DataId` for a (deduplicated) string literal's
    /// 16-byte `{ const char* data; int32_t len; }` header cell. The
    /// backing character bytes are a second, anonymous data object that
    /// the header stores a relocated pointer to (see `layout.rs` for why
    /// `osc_str` is laid out this way).
    fn string_literal_data(&mut self, value: &str) -> LData {
        if let Some(existing) = self.string_handles.get(value) {
            return *existing;
        }

        let bytes_id = self.declare_anonymous_data(false);
        let mut bytes_desc = DataDescription::new();
        // Always append a trailing NUL, matching the C backend's
        // `osc_str_from_cstr("...")` (a real C string literal), and
        // incidentally avoiding a zero-sized data object for `""`. `len`
        // below is the *Oscan* string length (excludes the NUL).
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        bytes_desc.define(bytes.into_boxed_slice());
        self.module()
            .define_data(bytes_id, &bytes_desc)
            .unwrap_or_else(|e| {
                panic!("internal error: failed to define string literal bytes: {e}")
            });

        let header_id = self.declare_anonymous_data(false);
        let mut header_desc = DataDescription::new();
        // { data: ptr (8 bytes, relocated to `bytes_id`), len: i32, pad: i32 }
        let mut initial_bytes = vec![0u8; 16];
        initial_bytes[8..12].copy_from_slice(&(value.len() as i32).to_le_bytes());
        header_desc.define(initial_bytes.into_boxed_slice());
        let gv = self
            .module()
            .declare_data_in_data(bytes_id, &mut header_desc);
        header_desc.write_data_addr(0, gv, 0);
        self.module()
            .define_data(header_id, &header_desc)
            .unwrap_or_else(|e| {
                panic!("internal error: failed to define string literal header: {e}")
            });

        let handle = LData(self.data_ids.len() as u32);
        self.data_ids.push(header_id);
        self.string_handles.insert(value.to_string(), handle);
        handle
    }

    fn declare_import_data(&mut self, symbol: &str) -> LData {
        if let Some(existing) = self.data_handles.get(symbol) {
            return *existing;
        }
        let id = self
            .module()
            .declare_data(symbol, Linkage::Import, true, false)
            .unwrap_or_else(|e| panic!("internal error: failed to declare data '{symbol}': {e}"));
        let handle = LData(self.data_ids.len() as u32);
        self.data_ids.push(id);
        self.data_handles.insert(symbol.to_string(), handle);
        handle
    }
}

pub struct CraneliftLir {
    state: ModuleState,
}

impl CraneliftLir {
    pub fn new(target: NativeTarget, optimization: OptimizationProfile) -> Result<Self, String> {
        let isa = target::build_isa(target, optimization)?;
        let builder = ObjectBuilder::new(
            isa,
            "oscan_program",
            cranelift_module::default_libcall_names(),
        )
        .map_err(|e| format!("internal error configuring object writer: {e}"))?;
        Ok(CraneliftLir {
            state: ModuleState {
                module: Some(ObjectModule::new(builder)),
                func_ids: Vec::new(),
                func_rets: Vec::new(),
                func_handles: HashMap::new(),
                data_ids: Vec::new(),
                data_handles: HashMap::new(),
                string_handles: HashMap::new(),
                next_anon_data: 0,
            },
        })
    }
}

impl LirModule for CraneliftLir {
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
        let func_id = self.state.func_ids[func.index()];
        let cl_sig = cl_signature(self.state.module_ref(), sig);
        let mut function =
            Function::with_name_signature(UserFuncName::user(0, func_id.as_u32()), cl_sig);
        let mut fb_ctx = FunctionBuilderContext::new();

        {
            let mut builder = FunctionBuilder::new(&mut function, &mut fb_ctx);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);
            let raw_params = builder.block_params(entry).to_vec();

            let mut cb = CraneliftBuilder {
                state: &mut self.state,
                builder,
                values: Vec::new(),
                value_types: Vec::new(),
                blocks: vec![entry],
                block_params: vec![Vec::new()],
                vars: Vec::new(),
                var_types: Vec::new(),
            };
            let mut params = Vec::with_capacity(raw_params.len());
            for (value, ty) in raw_params.into_iter().zip(sig.params.iter()) {
                let handle = cb.push_value(value, *ty);
                cb.block_params[0].push(handle);
                params.push(handle);
            }

            let result = body(&mut cb, &params);
            let CraneliftBuilder { builder, .. } = cb;
            result?;
            builder.finalize();
        }

        let mut ctx_obj = self.state.module().make_context();
        ctx_obj.func = function;
        self.state
            .module()
            .define_function(func_id, &mut ctx_obj)
            .map_err(|e| LirError::Backend(format!("{e}")))?;
        Ok(())
    }

    fn finish(&mut self) -> Result<LirArtifact, String> {
        let module = self
            .state
            .module
            .take()
            .ok_or_else(|| "internal error: Cranelift module already finished".to_string())?;
        let product = module.finish();
        let bytes = product
            .emit()
            .map_err(|e| format!("internal error emitting object file: {e}"))?;
        Ok(LirArtifact::Object(bytes))
    }
}

pub struct CraneliftBuilder<'a> {
    state: &'a mut ModuleState,
    builder: FunctionBuilder<'a>,
    values: Vec<Value>,
    /// Declared type per [`LValue`]; `Ptr` is preserved rather than
    /// collapsed to `I64` so `value_type` answers identically across
    /// backends.
    value_types: Vec<LType>,
    blocks: Vec<cranelift_codegen::ir::Block>,
    block_params: Vec<Vec<LValue>>,
    vars: Vec<Variable>,
    var_types: Vec<LType>,
}

impl CraneliftBuilder<'_> {
    fn push_value(&mut self, value: Value, ty: LType) -> LValue {
        let handle = LValue(self.values.len() as u32);
        self.values.push(value);
        self.value_types.push(ty);
        handle
    }

    fn value(&self, handle: LValue) -> Value {
        self.values[handle.index()]
    }

    fn cl_values(&self, handles: &[LValue]) -> Vec<Value> {
        handles.iter().map(|h| self.value(*h)).collect()
    }

    fn block(&self, handle: LBlock) -> cranelift_codegen::ir::Block {
        self.blocks[handle.index()]
    }

    fn block_args(&self, handles: &[LValue]) -> Vec<cranelift_codegen::ir::BlockArg> {
        handles
            .iter()
            .map(|h| cranelift_codegen::ir::BlockArg::Value(self.value(*h)))
            .collect()
    }
}

impl LirBuilder for CraneliftBuilder<'_> {
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
        let block = self.builder.create_block();
        let handle = LBlock(self.blocks.len() as u32);
        self.blocks.push(block);
        self.block_params.push(Vec::new());
        handle
    }

    fn append_block_param(&mut self, block: LBlock, ty: LType) -> LValue {
        let cl_block = self.block(block);
        let value = self.builder.append_block_param(cl_block, cl_type(ty));
        let handle = self.push_value(value, ty);
        self.block_params[block.index()].push(handle);
        handle
    }

    fn block_param(&self, block: LBlock, index: usize) -> LValue {
        self.block_params[block.index()][index]
    }

    fn switch_to_block(&mut self, block: LBlock) {
        let cl_block = self.block(block);
        self.builder.switch_to_block(cl_block);
    }

    fn seal_block(&mut self, block: LBlock) {
        let cl_block = self.block(block);
        self.builder.seal_block(cl_block);
    }

    fn is_unreachable(&self) -> bool {
        self.builder.is_unreachable()
    }

    fn declare_var(&mut self, ty: LType) -> LVar {
        let var = self.builder.declare_var(cl_type(ty));
        let handle = LVar(self.vars.len() as u32);
        self.vars.push(var);
        self.var_types.push(ty);
        handle
    }

    fn def_var(&mut self, var: LVar, value: LValue) {
        let cl_var = self.vars[var.index()];
        let cl_value = self.value(value);
        self.builder.def_var(cl_var, cl_value);
    }

    fn use_var(&mut self, var: LVar) -> LValue {
        let cl_var = self.vars[var.index()];
        let ty = self.var_types[var.index()];
        let value = self.builder.use_var(cl_var);
        self.push_value(value, ty)
    }

    fn value_type(&self, value: LValue) -> LType {
        self.value_types[value.index()]
    }

    fn iconst(&mut self, ty: LType, imm: i64) -> LValue {
        let value = self.builder.ins().iconst(cl_type(ty), imm);
        self.push_value(value, ty)
    }

    fn f64const(&mut self, imm: f64) -> LValue {
        let value = self
            .builder
            .ins()
            .f64const(cranelift_codegen::ir::immediates::Ieee64::with_float(imm));
        self.push_value(value, LType::F64)
    }

    fn iadd(&mut self, a: LValue, b: LValue) -> LValue {
        let ty = self.value_type(a);
        let (x, y) = (self.value(a), self.value(b));
        let value = self.builder.ins().iadd(x, y);
        self.push_value(value, ty)
    }

    fn iadd_imm(&mut self, a: LValue, imm: i64) -> LValue {
        let ty = self.value_type(a);
        let x = self.value(a);
        let value = self.builder.ins().iadd_imm(x, imm);
        self.push_value(value, ty)
    }

    fn band(&mut self, a: LValue, b: LValue) -> LValue {
        let ty = self.value_type(a);
        let (x, y) = (self.value(a), self.value(b));
        let value = self.builder.ins().band(x, y);
        self.push_value(value, ty)
    }

    fn bor(&mut self, a: LValue, b: LValue) -> LValue {
        let ty = self.value_type(a);
        let (x, y) = (self.value(a), self.value(b));
        let value = self.builder.ins().bor(x, y);
        self.push_value(value, ty)
    }

    fn bxor(&mut self, a: LValue, b: LValue) -> LValue {
        let ty = self.value_type(a);
        let (x, y) = (self.value(a), self.value(b));
        let value = self.builder.ins().bxor(x, y);
        self.push_value(value, ty)
    }

    fn bxor_imm(&mut self, a: LValue, imm: i64) -> LValue {
        let ty = self.value_type(a);
        let x = self.value(a);
        let value = self.builder.ins().bxor_imm(x, imm);
        self.push_value(value, ty)
    }

    fn bnot(&mut self, a: LValue) -> LValue {
        let ty = self.value_type(a);
        let x = self.value(a);
        let value = self.builder.ins().bnot(x);
        self.push_value(value, ty)
    }

    fn ishl(&mut self, a: LValue, b: LValue) -> LValue {
        let ty = self.value_type(a);
        let (x, y) = (self.value(a), self.value(b));
        let value = self.builder.ins().ishl(x, y);
        self.push_value(value, ty)
    }

    fn ushr(&mut self, a: LValue, b: LValue) -> LValue {
        let ty = self.value_type(a);
        let (x, y) = (self.value(a), self.value(b));
        let value = self.builder.ins().ushr(x, y);
        self.push_value(value, ty)
    }

    fn fadd(&mut self, a: LValue, b: LValue) -> LValue {
        let (x, y) = (self.value(a), self.value(b));
        let value = self.builder.ins().fadd(x, y);
        self.push_value(value, LType::F64)
    }

    fn fsub(&mut self, a: LValue, b: LValue) -> LValue {
        let (x, y) = (self.value(a), self.value(b));
        let value = self.builder.ins().fsub(x, y);
        self.push_value(value, LType::F64)
    }

    fn fmul(&mut self, a: LValue, b: LValue) -> LValue {
        let (x, y) = (self.value(a), self.value(b));
        let value = self.builder.ins().fmul(x, y);
        self.push_value(value, LType::F64)
    }

    fn fdiv(&mut self, a: LValue, b: LValue) -> LValue {
        let (x, y) = (self.value(a), self.value(b));
        let value = self.builder.ins().fdiv(x, y);
        self.push_value(value, LType::F64)
    }

    fn fneg(&mut self, a: LValue) -> LValue {
        let x = self.value(a);
        let value = self.builder.ins().fneg(x);
        self.push_value(value, LType::F64)
    }

    fn icmp(&mut self, cc: IntCmp, a: LValue, b: LValue) -> LValue {
        let (x, y) = (self.value(a), self.value(b));
        let value = self.builder.ins().icmp(int_cc(cc), x, y);
        self.push_value(value, LType::I8)
    }

    fn fcmp(&mut self, cc: FloatCmp, a: LValue, b: LValue) -> LValue {
        let (x, y) = (self.value(a), self.value(b));
        let value = self.builder.ins().fcmp(float_cc(cc), x, y);
        self.push_value(value, LType::I8)
    }

    fn uextend(&mut self, ty: LType, value: LValue) -> LValue {
        let x = self.value(value);
        let extended = self.builder.ins().uextend(cl_type(ty), x);
        self.push_value(extended, ty)
    }

    fn load(&mut self, ty: LType, addr: LValue, offset: i32) -> LValue {
        let a = self.value(addr);
        let value = self.builder.ins().load(cl_type(ty), mem_flags(), a, offset);
        self.push_value(value, ty)
    }

    fn store(&mut self, value: LValue, addr: LValue, offset: i32) {
        let (v, a) = (self.value(value), self.value(addr));
        self.builder.ins().store(mem_flags(), v, a, offset);
    }

    fn stack_slot_addr(&mut self, size: u32, align: u32) -> LValue {
        let _ = align;
        let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            size,
            0,
        ));
        let addr = self.builder.ins().stack_addr(cl_type(LType::Ptr), slot, 0);
        self.push_value(addr, LType::Ptr)
    }

    fn mem_copy(&mut self, dest: LValue, src: LValue, size: u32, align: u32) {
        if size == 0 {
            return;
        }
        let _ = align;
        // Load every scalar chunk before storing any of them. Besides avoiding
        // libc memcpy/memmove calls, this gives the LIR operation its promised
        // memmove semantics when the source lies inside the destination.
        let mut loaded = Vec::new();
        let mut offset = 0u32;
        let src_value = self.value(src);
        while offset < size {
            let remaining = size - offset;
            let (chunk, ty) = if remaining >= 8 {
                (8, types::I64)
            } else if remaining >= 4 {
                (4, types::I32)
            } else if remaining >= 2 {
                (2, types::I16)
            } else {
                (1, types::I8)
            };
            let value = self
                .builder
                .ins()
                .load(ty, mem_flags(), src_value, offset as i32);
            loaded.push((offset, value));
            offset += chunk;
        }
        let dest_value = self.value(dest);
        for (offset, value) in loaded {
            self.builder
                .ins()
                .store(mem_flags(), value, dest_value, offset as i32);
        }
    }

    fn global_addr(&mut self, data: LData) -> LValue {
        let data_id = self.state.data_ids[data.index()];
        let gv = self
            .state
            .module()
            .declare_data_in_func(data_id, self.builder.func);
        let value = self.builder.ins().global_value(cl_type(LType::Ptr), gv);
        self.push_value(value, LType::Ptr)
    }

    fn func_addr(&mut self, func: LFunc) -> LValue {
        let func_id = self.state.func_ids[func.index()];
        let func_ref = self
            .state
            .module()
            .declare_func_in_func(func_id, self.builder.func);
        let value = self.builder.ins().func_addr(cl_type(LType::Ptr), func_ref);
        self.push_value(value, LType::Ptr)
    }

    fn call(&mut self, func: LFunc, args: &[LValue]) -> Option<LValue> {
        let func_id = self.state.func_ids[func.index()];
        let ret = self.state.func_rets[func.index()];
        let cl_args = self.cl_values(args);
        let func_ref = self
            .state
            .module()
            .declare_func_in_func(func_id, self.builder.func);
        let call = self.builder.ins().call(func_ref, &cl_args);
        let result = self.builder.inst_results(call).first().copied();
        match (result, ret) {
            (Some(value), Some(ty)) => Some(self.push_value(value, ty)),
            _ => None,
        }
    }

    fn call_indirect(&mut self, sig: &LSig, callee: LValue, args: &[LValue]) -> Option<LValue> {
        let cl_sig = cl_signature(self.state.module_ref(), sig);
        let cl_args = self.cl_values(args);
        let callee_value = self.value(callee);
        let sig_ref = self.builder.import_signature(cl_sig);
        let call = self
            .builder
            .ins()
            .call_indirect(sig_ref, callee_value, &cl_args);
        let result = self.builder.inst_results(call).first().copied();
        match (result, sig.ret) {
            (Some(value), Some(ty)) => Some(self.push_value(value, ty)),
            _ => None,
        }
    }

    fn jump(&mut self, target: LBlock, args: &[LValue]) {
        let block = self.block(target);
        let block_args = self.block_args(args);
        self.builder.ins().jump(block, &block_args);
    }

    fn brif(
        &mut self,
        cond: LValue,
        then_block: LBlock,
        then_args: &[LValue],
        else_block: LBlock,
        else_args: &[LValue],
    ) {
        let c = self.value(cond);
        let t = self.block(then_block);
        let e = self.block(else_block);
        let ta = self.block_args(then_args);
        let ea = self.block_args(else_args);
        self.builder.ins().brif(c, t, &ta, e, &ea);
    }

    fn ret(&mut self, value: Option<LValue>) {
        match value {
            Some(v) => {
                let cl = self.value(v);
                self.builder.ins().return_(&[cl]);
            }
            None => {
                self.builder.ins().return_(&[]);
            }
        }
    }
}
