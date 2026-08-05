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
use std::path::Path;

use crate::backend::lir::{
    copy_chunks, FloatCmp, IntCmp, LBlock, LData, LFunc, LLinkage, LSig, LType, LValue, LVar,
    LirArtifact, LirBody, LirBuilder, LirError, LirModule,
};
use crate::backend::OptimizationProfile;
use crate::debuginfo::{DebugInfo, SourceFileId, SourceLocation, SourceMap};

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
    debug_subprogram: Option<u32>,
    debug_fallback_location: Option<u32>,
}

enum DataDecl {
    /// A deduplicated string literal: `{ ptr data; i32 len; i32 pad; }`
    /// plus its NUL-terminated backing bytes.
    StringLiteral { index: u32, value: String },
    /// An imported runtime-owned global (`osc_global_argc`, ...).
    Import { symbol: String },
}

struct DebugMetadata {
    source_filename: String,
    files: HashMap<SourceFileId, u32>,
    primary_file: u32,
    compile_unit: u32,
    subroutine_type: u32,
    dwarf_version_flag: u32,
    debug_info_version_flag: u32,
    locations: HashMap<(u32, SourceLocation), u32>,
    definitions: Vec<String>,
}

impl DebugMetadata {
    fn new(source_map: &SourceMap) -> Self {
        let source_filename = source_map
            .primary_path()
            .map(path_text)
            .unwrap_or_else(|| "oscan_program".to_string());
        let mut metadata = DebugMetadata {
            source_filename,
            files: HashMap::new(),
            primary_file: 0,
            compile_unit: 0,
            subroutine_type: 0,
            dwarf_version_flag: 0,
            debug_info_version_flag: 0,
            locations: HashMap::new(),
            definitions: Vec::new(),
        };

        let compile_unit = metadata.reserve_definition();
        metadata.compile_unit = compile_unit;

        let mut primary_file = None;
        for (source_file, path) in source_map.files() {
            let (filename, directory) = split_debug_path(path);
            let file = metadata.add_definition(format!(
                "!DIFile(filename: \"{}\", directory: \"{}\")",
                llvm_bytes(filename.as_bytes()),
                llvm_bytes(directory.as_bytes())
            ));
            primary_file.get_or_insert(file);
            metadata.files.insert(source_file, file);
        }
        let primary_file = match primary_file {
            Some(file) => file,
            None => metadata.add_definition(
                "!DIFile(filename: \"oscan_program\", directory: \"\")".to_string(),
            ),
        };
        metadata.primary_file = primary_file;

        let empty_types = metadata.add_definition("!{}".to_string());
        metadata.subroutine_type =
            metadata.add_definition(format!("!DISubroutineType(types: !{empty_types})"));
        metadata.dwarf_version_flag =
            metadata.add_definition("!{i32 7, !\"Dwarf Version\", i32 4}".to_string());
        metadata.debug_info_version_flag =
            metadata.add_definition("!{i32 2, !\"Debug Info Version\", i32 3}".to_string());
        metadata.set_definition(
            compile_unit,
            format!(
                "distinct !DICompileUnit(language: DW_LANG_C, file: !{primary_file}, producer: \
                 \"Oscan\", isOptimized: true, runtimeVersion: 0, emissionKind: LineTablesOnly)"
            ),
        );
        metadata
    }

    fn reserve_definition(&mut self) -> u32 {
        let id = self.definitions.len() as u32;
        self.definitions.push(String::new());
        id
    }

    fn set_definition(&mut self, id: u32, definition: String) {
        self.definitions[id as usize] = definition;
    }

    fn add_definition(&mut self, definition: String) -> u32 {
        let id = self.reserve_definition();
        self.set_definition(id, definition);
        id
    }

    fn add_subprogram(
        &mut self,
        source_name: &str,
        linkage_name: &str,
        location: SourceLocation,
    ) -> Option<u32> {
        let file = *self.files.get(&location.file)?;
        Some(self.add_definition(format!(
            "distinct !DISubprogram(name: \"{}\", linkageName: \"{}\", scope: !{file}, file: \
             !{file}, line: {}, type: !{}, scopeLine: {}, spFlags: DISPFlagDefinition | \
             DISPFlagOptimized, unit: !{})",
            llvm_bytes(source_name.as_bytes()),
            llvm_bytes(linkage_name.as_bytes()),
            location.line,
            self.subroutine_type,
            location.line,
            self.compile_unit
        )))
    }

    fn add_artificial_subprogram(&mut self, linkage_name: &str) -> (u32, u32) {
        let subprogram = self.add_definition(format!(
            "distinct !DISubprogram(name: \"{}\", linkageName: \"{}\", scope: !{}, file: !{}, \
             line: 0, type: !{}, scopeLine: 0, flags: DIFlagArtificial, spFlags: \
             DISPFlagDefinition | DISPFlagOptimized, unit: !{})",
            llvm_bytes(linkage_name.as_bytes()),
            llvm_bytes(linkage_name.as_bytes()),
            self.primary_file,
            self.primary_file,
            self.subroutine_type,
            self.compile_unit
        ));
        let location = self.add_definition(format!(
            "!DILocation(line: 0, column: 0, scope: !{subprogram})"
        ));
        (subprogram, location)
    }

    fn location(&mut self, scope: u32, location: SourceLocation) -> u32 {
        if let Some(existing) = self.locations.get(&(scope, location)) {
            return *existing;
        }
        let column = if location.column <= u32::from(u16::MAX) {
            location.column
        } else {
            0
        };
        let id = self.add_definition(format!(
            "!DILocation(line: {}, column: {}, scope: !{scope})",
            location.line, column
        ));
        self.locations.insert((scope, location), id);
        id
    }

    fn write_to(&self, out: &mut String) {
        let _ = writeln!(out, "!llvm.dbg.cu = !{{!{}}}", self.compile_unit);
        let _ = writeln!(
            out,
            "!llvm.module.flags = !{{!{}, !{}}}",
            self.dwarf_version_flag, self.debug_info_version_flag
        );
        out.push('\n');
        for (id, definition) in self.definitions.iter().enumerate() {
            debug_assert!(!definition.is_empty());
            let _ = writeln!(out, "!{id} = {definition}");
        }
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn split_debug_path(path: &Path) -> (String, String) {
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path_text(path));
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(path_text)
        .unwrap_or_default();
    (filename, directory)
}

/// Module-level state, mirroring `lir_cranelift::ModuleState`.
struct ModuleState {
    target_triple: String,
    data_layout: String,
    optimization: OptimizationProfile,
    debug: Option<DebugMetadata>,
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
            if decl.linkage != linkage {
                return Err(format!(
                    "conflicting linkage for symbol '{symbol}': {:?} vs {:?}",
                    decl.linkage, linkage
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
            debug_subprogram: None,
            debug_fallback_location: None,
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
    pub fn new(
        target_triple: &str,
        data_layout: &str,
        optimization: OptimizationProfile,
        debug_info: DebugInfo,
        source_map: &SourceMap,
    ) -> Self {
        LlvmEmitter {
            state: ModuleState {
                target_triple: target_triple.to_string(),
                data_layout: data_layout.to_string(),
                optimization,
                debug: debug_info
                    .is_enabled()
                    .then(|| DebugMetadata::new(source_map)),
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

    fn set_function_source(
        &mut self,
        func: LFunc,
        source_name: &str,
        linkage_name: &str,
        location: SourceLocation,
    ) {
        if self.state.funcs[func.index()].debug_subprogram.is_some() {
            return;
        }
        let subprogram = self
            .state
            .debug
            .as_mut()
            .and_then(|debug| debug.add_subprogram(source_name, linkage_name, location));
        self.state.funcs[func.index()].debug_subprogram = subprogram;
        self.state.funcs[func.index()].debug_fallback_location = None;
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
        if self.state.funcs[func.index()].debug_subprogram.is_none() {
            let symbol = self.state.funcs[func.index()].symbol.clone();
            let generated_debug = self
                .state
                .debug
                .as_mut()
                .map(|debug| debug.add_artificial_subprogram(&symbol));
            if let Some((subprogram, fallback_location)) = generated_debug {
                self.state.funcs[func.index()].debug_subprogram = Some(subprogram);
                self.state.funcs[func.index()].debug_fallback_location = Some(fallback_location);
            }
        }

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
        match &self.state.debug {
            Some(debug) => {
                let _ = writeln!(
                    out,
                    "source_filename = \"{}\"",
                    llvm_bytes(debug.source_filename.as_bytes())
                );
            }
            None => out.push_str("source_filename = \"oscan_program\"\n"),
        }
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
            if decl.linkage != LLinkage::Import {
                return Err(format!(
                    "internal error: defined function '{}' was declared but never defined",
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

        if let Some(debug) = &self.state.debug {
            debug.write_to(&mut out);
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
    debug_subprogram: Option<u32>,
    debug_fallback_location: Option<u32>,
    source_location: Option<SourceLocation>,
    /// A hard error discovered while emitting (e.g. a type mismatch that
    /// would produce invalid IR), surfaced by `finish`.
    error: Option<String>,
}

impl<'a> LlvmFuncBuilder<'a> {
    fn new(state: &'a mut ModuleState, func: LFunc, sig: &LSig) -> Self {
        let debug_subprogram = state.funcs[func.index()].debug_subprogram;
        let debug_fallback_location = state.funcs[func.index()].debug_fallback_location;
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
            debug_subprogram,
            debug_fallback_location,
            source_location: None,
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
        if self.blocks[self.current].terminated {
            return;
        }
        let debug_location = self.debug_location();
        let block = &mut self.blocks[self.current];
        block.body.push_str("  ");
        block.body.push_str(&instruction);
        if let Some(debug_location) = debug_location {
            let _ = write!(block.body, ", !dbg !{debug_location}");
        }
        block.body.push('\n');
    }

    fn emit_terminator(&mut self, instruction: String) {
        if self.blocks[self.current].terminated {
            return;
        }
        let debug_location = self.debug_location();
        let block = &mut self.blocks[self.current];
        block.body.push_str("  ");
        block.body.push_str(&instruction);
        if let Some(debug_location) = debug_location {
            let _ = write!(block.body, ", !dbg !{debug_location}");
        }
        block.body.push('\n');
        block.terminated = true;
    }

    fn debug_location(&mut self) -> Option<u32> {
        let scope = self.debug_subprogram?;
        match self.source_location {
            Some(location) => self
                .state
                .debug
                .as_mut()
                .map(|debug| debug.location(scope, location)),
            None => self.debug_fallback_location,
        }
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
        let linkage = match decl.linkage {
            LLinkage::Export => "",
            LLinkage::Local => "internal ",
            LLinkage::Import => {
                return Err(LirError::Backend(format!(
                    "imported function '{}' cannot have a body",
                    decl.symbol
                )));
            }
        };
        let optimization_attrs = match self.state.optimization {
            OptimizationProfile::Size => "minsize nounwind optsize",
            OptimizationProfile::Speed => "nounwind",
        };
        // `nounwind`: Oscan has no exceptions and the runtime never
        // unwinds — it aborts. The size profile's `minsize`/`optsize`
        // attributes guide both IR and machine-level optimization; the
        // speed profile deliberately omits both.
        // `"no-builtins"` is Clang's `-ffreestanding` marker: LLVM must not
        // assume libc entry points exist. Nothing stronger (no `nofree`, no
        // `willreturn`, no argument attributes) is promised.
        let line_tables = self.state.debug.is_some();
        let unwind_table_attribute =
            if line_tables && self.state.target_triple.contains("-windows-") {
                // Win64 stack walkers consume the mandatory PE unwind records.
                // LLVM emits them for nounwind functions only when requested.
                " uwtable"
            } else {
                ""
            };
        let line_table_attributes = if line_tables {
            " \"frame-pointer\"=\"all\""
        } else {
            ""
        };
        let debug_subprogram = decl
            .debug_subprogram
            .map(|id| format!(" !dbg !{id}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "define {linkage}{ret} {}({params}) {optimization_attrs}{unwind_table_attribute} \"no-builtins\"{line_table_attributes}{debug_subprogram} {{",
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

    fn set_source_location(&mut self, location: Option<SourceLocation>) {
        self.source_location = location;
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
    use crate::debuginfo::SourceMapBuilder;
    use std::path::PathBuf;

    fn ir_of_with_debug(
        debug_info: DebugInfo,
        source_map: &SourceMap,
        build: impl FnOnce(&mut LlvmEmitter),
    ) -> String {
        ir_of_for_target_with_debug("x86_64-unknown-linux-gnu", debug_info, source_map, build)
    }

    fn ir_of_for_target_with_debug(
        target_triple: &str,
        debug_info: DebugInfo,
        source_map: &SourceMap,
        build: impl FnOnce(&mut LlvmEmitter),
    ) -> String {
        ir_of_for_target_profile_with_debug(
            target_triple,
            OptimizationProfile::Size,
            debug_info,
            source_map,
            build,
        )
    }

    fn ir_of_for_target_profile_with_debug(
        target_triple: &str,
        optimization: OptimizationProfile,
        debug_info: DebugInfo,
        source_map: &SourceMap,
        build: impl FnOnce(&mut LlvmEmitter),
    ) -> String {
        let mut emitter = LlvmEmitter::new(
            target_triple,
            "e-m:e-p:64:64-i64:64",
            optimization,
            debug_info,
            source_map,
        );
        build(&mut emitter);
        match emitter.finish().expect("finish") {
            LirArtifact::LlvmIr(text) => text,
            LirArtifact::Object(_) => panic!("LLVM emitter must produce IR, not an object"),
        }
    }

    fn ir_of_profile(
        optimization: OptimizationProfile,
        build: impl FnOnce(&mut LlvmEmitter),
    ) -> String {
        let source_map = SourceMap::default();
        ir_of_for_target_profile_with_debug(
            "x86_64-unknown-linux-gnu",
            optimization,
            DebugInfo::None,
            &source_map,
            build,
        )
    }

    fn ir_of(build: impl FnOnce(&mut LlvmEmitter)) -> String {
        ir_of_profile(OptimizationProfile::Size, build)
    }

    #[test]
    fn line_tables_request_frame_pointers_and_windows_unwind_tables() {
        let source_map = SourceMap::default();
        for (target, debug_info, frame_pointer, unwind_table) in [
            (
                "x86_64-unknown-linux-gnu",
                DebugInfo::LineTables,
                true,
                false,
            ),
            ("x86_64-w64-windows-gnu", DebugInfo::LineTables, true, true),
            ("x86_64-w64-windows-gnu", DebugInfo::None, false, false),
        ] {
            let text = ir_of_for_target_with_debug(target, debug_info, &source_map, |emitter| {
                let sig = LSig::new(vec![], None);
                let wrapper = emitter
                    .declare_function("main", &sig, LLinkage::Export)
                    .expect("declare wrapper");
                emitter
                    .define_function(wrapper, &sig, &mut |b, _params| {
                        b.ret(None);
                        Ok(())
                    })
                    .unwrap_or_else(|_| panic!("define wrapper failed"));
            });
            let header = text
                .lines()
                .find(|line| line.starts_with("define "))
                .expect("function definition");
            assert_eq!(
                header.contains("\"frame-pointer\"=\"all\""),
                frame_pointer,
                "{header}"
            );
            assert_eq!(header.contains(" uwtable "), unwind_table, "{header}");
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
        let source_map = SourceMap::default();
        let mut emitter = LlvmEmitter::new(
            "x86_64-unknown-linux-gnu",
            "e-m:e-p:64:64",
            OptimizationProfile::Size,
            DebugInfo::None,
            &source_map,
        );
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
    fn debug_info_none_preserves_the_metadata_free_module_shape() {
        let mut source_map = SourceMapBuilder::default();
        let file = source_map.intern_file(PathBuf::from("ignored").join("actual.osc"));
        let source_map = source_map.finish(Vec::new());
        let text = ir_of_with_debug(DebugInfo::None, &source_map, |emitter| {
            let sig = LSig::new(vec![], Some(LType::I32));
            let func = emitter
                .declare_function("plain", &sig, LLinkage::Export)
                .expect("declare");
            emitter.set_function_source(
                func,
                "plain",
                "plain",
                SourceLocation {
                    file,
                    line: 3,
                    column: 1,
                },
            );
            emitter
                .define_function(func, &sig, &mut |b, _params| {
                    b.set_source_location(Some(SourceLocation {
                        file,
                        line: 4,
                        column: 2,
                    }));
                    let one = b.iconst(LType::I32, 1);
                    let two = b.iconst(LType::I32, 2);
                    let sum = b.iadd(one, two);
                    b.ret(Some(sum));
                    Ok(())
                })
                .unwrap_or_else(|_| panic!("define failed"));
        });

        assert!(
            text.contains("source_filename = \"oscan_program\""),
            "{text}"
        );
        for marker in [
            "!llvm.dbg.cu",
            "!llvm.module.flags",
            "!DI",
            "!dbg",
            "\"frame-pointer\"",
        ] {
            assert!(
                !text.contains(marker),
                "DebugInfo::None must not emit '{marker}':\n{text}"
            );
        }
    }

    #[test]
    fn line_tables_emit_multi_file_function_and_location_metadata() {
        let root_path = PathBuf::from("project").join("src").join("root.osc");
        let imported_path = PathBuf::from("project").join("libs").join("math\"core.osc");
        let mut source_map = SourceMapBuilder::default();
        let root = source_map.intern_file(root_path.clone());
        let imported = source_map.intern_file(imported_path.clone());
        let source_map = source_map.finish(Vec::new());

        let text = ir_of_with_debug(DebugInfo::LineTables, &source_map, |emitter| {
            let wrapper_sig = LSig::new(vec![], Some(LType::I32));
            let wrapper = emitter
                .declare_function("main", &wrapper_sig, LLinkage::Export)
                .expect("declare wrapper");
            emitter
                .define_function(wrapper, &wrapper_sig, &mut |b, _params| {
                    let one = b.iconst(LType::I32, 1);
                    let two = b.iconst(LType::I32, 2);
                    let sum = b.iadd(one, two);
                    b.ret(Some(sum));
                    Ok(())
                })
                .unwrap_or_else(|_| panic!("define wrapper failed"));

            let foo_sig = LSig::new(vec![LType::Ptr], Some(LType::I32));
            let foo = emitter
                .declare_function("__osc_foo", &foo_sig, LLinkage::Export)
                .expect("declare foo");
            emitter.set_function_source(
                foo,
                "foo",
                "__osc_foo",
                SourceLocation {
                    file: root,
                    line: 3,
                    column: 4,
                },
            );
            emitter
                .define_function(foo, &foo_sig, &mut |b, _params| {
                    let slot = b.declare_var(LType::I32);
                    b.set_source_location(Some(SourceLocation {
                        file: root,
                        line: 4,
                        column: 2,
                    }));
                    let one = b.iconst(LType::I32, 1);
                    let two = b.iconst(LType::I32, 2);
                    let sum = b.iadd(one, two);
                    b.def_var(slot, sum);
                    b.set_source_location(Some(SourceLocation {
                        file: root,
                        line: 8,
                        column: 7,
                    }));
                    let result = b.use_var(slot);
                    b.ret(Some(result));
                    Ok(())
                })
                .unwrap_or_else(|_| panic!("define foo failed"));

            let bar_sig = LSig::new(vec![], Some(LType::I32));
            let bar = emitter
                .declare_function("__osc_bar", &bar_sig, LLinkage::Export)
                .expect("declare bar");
            emitter.set_function_source(
                bar,
                "bar",
                "__osc_bar",
                SourceLocation {
                    file: imported,
                    line: 12,
                    column: 3,
                },
            );
            emitter
                .define_function(bar, &bar_sig, &mut |b, _params| {
                    b.set_source_location(Some(SourceLocation {
                        file: imported,
                        line: 13,
                        column: u32::MAX,
                    }));
                    let one = b.iconst(LType::I32, 1);
                    let two = b.iconst(LType::I32, 2);
                    let sum = b.iadd(one, two);
                    b.ret(Some(sum));
                    Ok(())
                })
                .unwrap_or_else(|_| panic!("define bar failed"));
        });

        let source_filename = llvm_bytes(root_path.to_string_lossy().as_bytes());
        assert!(
            text.contains(&format!("source_filename = \"{source_filename}\"")),
            "{text}"
        );
        assert!(text.contains("!llvm.dbg.cu = !{!"), "{text}");
        assert!(text.contains("!llvm.module.flags = !{!"), "{text}");
        assert!(
            text.contains("!{i32 7, !\"Dwarf Version\", i32 4}"),
            "{text}"
        );
        assert!(
            text.contains("!{i32 2, !\"Debug Info Version\", i32 3}"),
            "{text}"
        );
        assert_eq!(text.matches("!DIFile(").count(), 2, "{text}");
        assert!(text.contains("filename: \"root.osc\""), "{text}");
        assert!(
            text.contains("filename: \"math\\22core.osc\""),
            "LLVM strings must escape quotes:\n{text}"
        );
        assert_eq!(
            text.matches("distinct !DICompileUnit(").count(),
            1,
            "{text}"
        );
        assert!(text.contains("emissionKind: LineTablesOnly"), "{text}");
        assert_eq!(text.matches("!DISubroutineType(").count(), 1, "{text}");
        for symbol in ["main", "__osc_foo", "__osc_bar"] {
            let marker = format!("@{symbol}(");
            let header = text
                .lines()
                .find(|line| line.starts_with("define ") && line.contains(&marker))
                .expect("function definition");
            assert!(
                header.contains("\"frame-pointer\"=\"all\""),
                "line-table definition lacks a frame pointer attribute: {header}"
            );
        }

        let root_file = text
            .lines()
            .find(|line| line.contains("!DIFile(filename: \"root.osc\""))
            .expect("root DIFile");
        let imported_file = text
            .lines()
            .find(|line| line.contains("!DIFile(filename: \"math\\22core.osc\""))
            .expect("imported DIFile");
        let root_directory = llvm_bytes(
            root_path
                .parent()
                .expect("root directory")
                .to_string_lossy()
                .as_bytes(),
        );
        let imported_directory = llvm_bytes(
            imported_path
                .parent()
                .expect("imported directory")
                .to_string_lossy()
                .as_bytes(),
        );
        assert!(
            root_file.contains(&format!("directory: \"{root_directory}\"")),
            "{root_file}"
        );
        assert!(
            imported_file.contains(&format!("directory: \"{imported_directory}\"")),
            "{imported_file}"
        );
        let root_file_id = root_file
            .strip_prefix('!')
            .and_then(|line| line.split_once(" = "))
            .map(|(id, _)| id)
            .expect("root metadata id");
        let imported_file_id = imported_file
            .strip_prefix('!')
            .and_then(|line| line.split_once(" = "))
            .map(|(id, _)| id)
            .expect("imported metadata id");

        let subprograms: Vec<&str> = text
            .lines()
            .filter(|line| line.contains("distinct !DISubprogram("))
            .collect();
        assert_eq!(subprograms.len(), 3, "{text}");
        let wrapper_subprogram = subprograms
            .iter()
            .copied()
            .find(|line| line.contains("linkageName: \"main\""))
            .expect("wrapper subprogram");
        let foo_subprogram = subprograms
            .iter()
            .copied()
            .find(|line| line.contains("name: \"foo\""))
            .expect("foo subprogram");
        let bar_subprogram = subprograms
            .iter()
            .copied()
            .find(|line| line.contains("name: \"bar\""))
            .expect("bar subprogram");
        assert!(
            wrapper_subprogram.contains("line: 0")
                && wrapper_subprogram.contains("flags: DIFlagArtificial"),
            "{wrapper_subprogram}"
        );
        assert!(
            foo_subprogram.contains(&format!(
                "linkageName: \"__osc_foo\", scope: !{root_file_id}, file: !{root_file_id}, line: 3"
            )),
            "{foo_subprogram}"
        );
        assert!(
            bar_subprogram.contains(&format!(
                "linkageName: \"__osc_bar\", scope: !{imported_file_id}, file: !{imported_file_id}, line: 12"
            )),
            "{bar_subprogram}"
        );
        for subprogram in &subprograms {
            assert!(
                subprogram.contains("spFlags: DISPFlagDefinition | DISPFlagOptimized, unit: !"),
                "{subprogram}"
            );
            for obsolete in ["isLocal:", "isDefinition:", "isOptimized:"] {
                assert!(
                    !subprogram.contains(obsolete),
                    "obsolete DISubprogram field '{obsolete}': {subprogram}"
                );
            }
        }
        let foo_type = foo_subprogram
            .split("type: !")
            .nth(1)
            .and_then(|tail| tail.split(',').next())
            .expect("foo type");
        let bar_type = bar_subprogram
            .split("type: !")
            .nth(1)
            .and_then(|tail| tail.split(',').next())
            .expect("bar type");
        assert_eq!(foo_type, bar_type, "subroutine type must be shared");

        assert_eq!(text.matches("!DILocation(").count(), 4, "{text}");
        assert!(
            text.contains("!DILocation(line: 0, column: 0, scope: !"),
            "{text}"
        );
        assert!(
            text.contains("!DILocation(line: 13, column: 0, scope: !"),
            "{text}"
        );
        assert!(!text.contains("column: 4294967295"), "{text}");
        let foo_subprogram_id = foo_subprogram
            .strip_prefix('!')
            .and_then(|line| line.split_once(" = "))
            .map(|(id, _)| id)
            .expect("foo subprogram id");
        let bar_subprogram_id = bar_subprogram
            .strip_prefix('!')
            .and_then(|line| line.split_once(" = "))
            .map(|(id, _)| id)
            .expect("bar subprogram id");
        assert_eq!(
            text.lines()
                .filter(|line| {
                    line.contains("!DILocation(")
                        && line.contains(&format!("scope: !{foo_subprogram_id})"))
                })
                .count(),
            2,
            "{text}"
        );
        assert_eq!(
            text.lines()
                .filter(|line| {
                    line.contains("!DILocation(")
                        && line.contains(&format!("scope: !{bar_subprogram_id})"))
                })
                .count(),
            1,
            "{text}"
        );

        let function_text = |symbol: &str| {
            let marker = format!("@{symbol}(");
            let start = text
                .lines()
                .position(|line| line.starts_with("define ") && line.contains(&marker))
                .expect("function definition");
            text.lines()
                .skip(start)
                .take_while(|line| *line != "}")
                .chain(std::iter::once("}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let wrapper = function_text("main");
        assert!(
            wrapper
                .lines()
                .next()
                .unwrap_or_default()
                .contains("!dbg !"),
            "{wrapper}"
        );
        for instruction in wrapper.lines().filter(|line| line.starts_with("  ")) {
            assert!(
                instruction.contains("!dbg !"),
                "generated wrapper instruction lacks its artificial location: {instruction}"
            );
        }
        let foo = function_text("__osc_foo");
        let foo_header = foo.lines().next().expect("foo header");
        assert!(foo_header.contains("!dbg !"), "{foo_header}");
        let alloca = foo
            .lines()
            .find(|line| line.contains(" = alloca "))
            .expect("hoisted alloca");
        let parameter_setup = foo
            .lines()
            .find(|line| line.contains("ptrtoint ptr %arg0"))
            .expect("parameter setup");
        assert!(!alloca.contains("!dbg"), "{alloca}");
        assert!(!parameter_setup.contains("!dbg"), "{parameter_setup}");
        let add = foo
            .lines()
            .find(|line| line.contains(" = add i32 "))
            .expect("add instruction");
        let store = foo
            .lines()
            .find(|line| line.contains("store i32 "))
            .expect("store instruction");
        let add_location = add.split("!dbg !").nth(1).expect("add location");
        let store_location = store.split("!dbg !").nth(1).expect("store location");
        assert_eq!(
            add_location, store_location,
            "equal source locations must share one DILocation"
        );
        for instruction in foo.lines().filter(|line| {
            line.starts_with("  ")
                && !line.contains(" = alloca ")
                && !line.contains("ptrtoint ptr %arg")
        }) {
            assert!(
                instruction.contains("!dbg !"),
                "normal user instruction lacks debug location: {instruction}"
            );
        }
        let bar = function_text("__osc_bar");
        assert!(bar.lines().next().unwrap_or_default().contains("!dbg !"));
        for instruction in bar.lines().filter(|line| line.starts_with("  ")) {
            assert!(
                instruction.contains("!dbg !"),
                "bar instruction lacks debug location: {instruction}"
            );
        }
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
    fn optimization_profiles_control_size_attributes_and_keep_safety_attributes() {
        let emit = |optimization| {
            ir_of_profile(optimization, |emitter| {
                let sig = LSig::new(Vec::new(), None);
                let f = emitter
                    .declare_function("worker", &sig, LLinkage::Local)
                    .expect("declare");
                emitter
                    .define_function(f, &sig, &mut |b, _| {
                        b.ret(None);
                        Ok(())
                    })
                    .unwrap_or_else(|_| panic!("define failed"));
            })
        };

        let size = emit(OptimizationProfile::Size);
        assert!(size
            .contains("define internal void @worker() minsize nounwind optsize \"no-builtins\" {"));

        let speed = emit(OptimizationProfile::Speed);
        assert!(speed.contains("define internal void @worker() nounwind \"no-builtins\" {"));
        assert!(!speed.contains("minsize"));
        assert!(!speed.contains("optsize"));
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
