//! Shared per-compilation context for every object backend: the
//! backend-neutral cross-function lookup tables (declared user
//! functions, declared extern functions, and the generated C shims a
//! user `extern` with `str` in its signature needs).
//!
//! The code generator itself lives behind [`super::lir::LirModule`] /
//! [`super::lir::LirBuilder`], so this struct holds no Cranelift or LLVM
//! types at all. `func.rs` borrows it mutably while translating each
//! function body.

use std::collections::HashMap;

use crate::debuginfo::{DebugInfo, SourceLocation, SourceMap};
use crate::ir;
use crate::token::Span;

use super::extern_shim::NativeExternShim;
use super::lir::LFunc;
use super::RuntimeMode;

#[derive(Clone, Copy)]
pub enum ExternDeclKind {
    Direct,
    NativeShim,
}

pub struct BackendContext<'a> {
    pub program: &'a ir::Program,
    pub runtime_mode: RuntimeMode,
    debug_info: DebugInfo,
    source_map: &'a SourceMap,
    /// Oscan `fn`/`fn!` name -> declared function (the IR's `main` is
    /// declared under the C-backend-compatible symbol `oscan_main`).
    pub functions: HashMap<String, LFunc>,
    /// User `extern` block function name -> declared function plus
    /// whether the object imports the real C symbol directly or a
    /// generated per-program shim.
    pub externs: HashMap<String, (LFunc, ExternDeclKind)>,
    /// Per-program generated C shims for used user externs whose
    /// signature contains `str`.
    extern_shims: Vec<NativeExternShim>,
}

impl<'a> BackendContext<'a> {
    pub fn new(
        program: &'a ir::Program,
        runtime_mode: RuntimeMode,
        debug_info: DebugInfo,
        source_map: &'a SourceMap,
    ) -> Self {
        BackendContext {
            program,
            runtime_mode,
            debug_info,
            source_map,
            functions: HashMap::new(),
            externs: HashMap::new(),
            extern_shims: Vec::new(),
        }
    }

    pub fn source_location(&self, span: Span) -> Option<SourceLocation> {
        self.debug_info
            .is_enabled()
            .then(|| self.source_map.location(span))
            .flatten()
    }

    /// The mangled symbol name for a user function definition (`main`
    /// becomes `oscan_main`, matching `src/codegen.rs`'s C backend so all
    /// backends' object/asm dumps stay easy to cross-reference).
    pub fn user_fn_symbol(name: &str) -> String {
        if name == "main" {
            "oscan_main".to_string()
        } else {
            name.to_string()
        }
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
}
