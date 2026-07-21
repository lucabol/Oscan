//! Shared per-compilation context for the Cranelift backend: owns the
//! `ObjectModule` and every cross-function lookup table (declared user
//! functions, declared extern functions, a dedup/declare-on-first-use
//! cache for runtime/shim entry points, and string-literal/top-level
//! constant data objects). `func.rs` borrows this mutably while
//! translating each function body; `program.rs` drives the two
//! declare-then-define passes that fill it in.

use std::collections::HashMap;

use cranelift_codegen::ir::Signature;
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::ObjectModule;

use crate::ir;

use super::extern_shim::NativeExternShim;
use super::layout::cl_pointer_type;
use super::RuntimeMode;

#[derive(Clone, Copy)]
pub enum ExternDeclKind {
    Direct,
    NativeShim,
}

pub struct BackendContext<'a> {
    pub module: ObjectModule,
    pub program: &'a ir::Program,
    pub runtime_mode: RuntimeMode,
    /// Oscan `fn`/`fn!` name -> declared `FuncId` (the IR's `main` is
    /// declared under the C-backend-compatible symbol `oscan_main`).
    pub functions: HashMap<String, FuncId>,
    /// User `extern` block function name -> declared `FuncId` plus whether
    /// the native object imports the real C symbol directly or a generated
    /// per-program shim.
    pub externs: HashMap<String, (FuncId, ExternDeclKind)>,
    /// Per-program generated C shims for used user externs whose signature
    /// contains `str`.
    extern_shims: Vec<NativeExternShim>,
    /// Runtime/shim symbol name -> declared `FuncId`, filled in lazily the
    /// first time a given runtime entry point is actually called.
    runtime_funcs: HashMap<&'static str, FuncId>,
    /// Deduplicated string literal content -> the `DataId` of its 16-byte
    /// `{ptr, len}` header cell.
    string_literals: HashMap<String, DataId>,
    next_anon_data: u32,
}

impl<'a> BackendContext<'a> {
    pub fn new(module: ObjectModule, program: &'a ir::Program, runtime_mode: RuntimeMode) -> Self {
        BackendContext {
            module,
            program,
            runtime_mode,
            functions: HashMap::new(),
            externs: HashMap::new(),
            extern_shims: Vec::new(),
            runtime_funcs: HashMap::new(),
            string_literals: HashMap::new(),
            next_anon_data: 0,
        }
    }

    #[allow(dead_code)]
    pub fn pointer_type(&self) -> cranelift_codegen::ir::Type {
        cl_pointer_type()
    }

    /// The mangled symbol name for a user function definition (`main`
    /// becomes `oscan_main`, matching `src/codegen.rs`'s C backend so both
    /// backends' object/asm dumps stay easy to cross-reference).
    pub fn user_fn_symbol(name: &str) -> String {
        if name == "main" {
            "oscan_main".to_string()
        } else {
            name.to_string()
        }
    }

    /// Get-or-declare a runtime/shim function by its exact C symbol name.
    /// `build_sig` is only invoked the first time `symbol` is requested;
    /// later calls with the same `symbol` reuse the cached `FuncId`
    /// (signatures for a fixed runtime symbol never vary between call
    /// sites, so no consistency check is needed beyond that name-based
    /// cache).
    pub fn runtime_func(
        &mut self,
        symbol: &'static str,
        build_sig: impl FnOnce(&mut Signature),
    ) -> FuncId {
        if let Some(id) = self.runtime_funcs.get(symbol) {
            return *id;
        }
        let mut sig = self.module.make_signature();
        build_sig(&mut sig);
        let id = self
            .module
            .declare_function(symbol, Linkage::Import, &sig)
            .unwrap_or_else(|e| {
                panic!("internal error: failed to declare runtime function '{symbol}': {e}")
            });
        self.runtime_funcs.insert(symbol, id);
        id
    }

    pub fn add_extern_shim(&mut self, shim: NativeExternShim) {
        self.extern_shims.push(shim);
    }

    pub fn generated_extern_shim_source(&self) -> Result<Option<String>, String> {
        if self.extern_shims.is_empty() {
            return Ok(None);
        }
        super::extern_shim::generate_source(&self.extern_shims, self.program).map(Some)
    }

    /// Get-or-create the `DataId` for a (deduplicated) string literal's
    /// 16-byte `{ const char* data; int32_t len; }` header cell. The
    /// backing character bytes are a second, anonymous data object that
    /// the header stores a relocated pointer to (see module docs in
    /// `layout.rs` for why `osc_str` is laid out this way).
    pub fn string_literal_data(&mut self, s: &str) -> DataId {
        if let Some(id) = self.string_literals.get(s) {
            return *id;
        }

        let bytes_id = self.declare_anonymous_data(false);
        let mut bytes_desc = DataDescription::new();
        // Always append a trailing NUL, matching the C backend's `osc_str_from_cstr("...")`
        // (a real C string literal), and incidentally avoiding a zero-sized
        // data object for `""`. `len` below is the *Oscan* string length
        // (excludes the NUL), same as the C backend's `osc_str { data, len }`.
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        bytes_desc.define(bytes.into_boxed_slice());
        self.module
            .define_data(bytes_id, &bytes_desc)
            .unwrap_or_else(|e| {
                panic!("internal error: failed to define string literal bytes: {e}")
            });

        let header_id = self.declare_anonymous_data(false);
        let mut header_desc = DataDescription::new();
        // { data: ptr (8 bytes, relocated to `bytes_id`), len: i32, pad: i32 }
        let mut initial_bytes = vec![0u8; 16];
        initial_bytes[8..12].copy_from_slice(&(s.len() as i32).to_le_bytes());
        header_desc.define(initial_bytes.into_boxed_slice());
        let gv = self.module.declare_data_in_data(bytes_id, &mut header_desc);
        header_desc.write_data_addr(0, gv, 0);
        self.module
            .define_data(header_id, &header_desc)
            .unwrap_or_else(|e| {
                panic!("internal error: failed to define string literal header: {e}")
            });

        self.string_literals.insert(s.to_string(), header_id);
        header_id
    }

    fn declare_anonymous_data(&mut self, writable: bool) -> DataId {
        let name = format!("__osc_data_{}", self.next_anon_data);
        self.next_anon_data += 1;
        self.module
            .declare_data(&name, Linkage::Local, writable, false)
            .unwrap_or_else(|e| {
                panic!("internal error: failed to declare anonymous data '{name}': {e}")
            })
    }

    /// Declare a fresh, zero-initialized, writable anonymous data object of
    /// `size` bytes (used for arena-independent scratch such as the
    /// process-lifetime `_arena`/argc/argv globals are *not* — those are
    /// runtime-owned — but is handy for other module-level scratch storage
    /// should a future feature need it).
    #[allow(dead_code)]
    pub fn declare_zeroed_data(&mut self, size: usize) -> DataId {
        let id = self.declare_anonymous_data(true);
        let mut desc = DataDescription::new();
        desc.define_zeroinit(size);
        self.module
            .define_data(id, &desc)
            .unwrap_or_else(|e| panic!("internal error: failed to define zeroed data: {e}"));
        id
    }
}
