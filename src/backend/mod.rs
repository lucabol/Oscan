//! Object backends and the native link path.
//!
//! Two code generators live under this module and share everything above
//! the instruction selector: the Cranelift AOT backend (`--backend
//! cranelift`, compiled in with the `backend-cranelift` feature) and the
//! direct in-process LLVM backend (`--backend llvm`, `backend-llvm`; see
//! [`llvm`]). Both consume the same [`crate::ir::Program`] the C backend
//! (`crate::codegen`) does, and produce a relocatable object file ready to
//! link against a static Oscan runtime archive (see [`link`]).
//!
//! [`lir_cranelift`] is the *only* place Cranelift/`cranelift-object`
//! types are visible; `main.rs` only ever calls [`compile_object`]/
//! `link`'s public entry points, never touches Cranelift directly, and the
//! C backend (`crate::codegen`) is untouched by any of this — selecting
//! `--backend cranelift` never runs `crate::codegen` at all (see module
//! docs on "no silent fallback" in `main.rs`). A build without the
//! `backend-cranelift` feature contains no Cranelift code or dependencies
//! whatsoever.
//!
//! # Coverage
//!
//! Implemented: the full language surface exercised by the positive
//! integration corpus, including checked scalar arithmetic, structured
//! control flow, `defer`, user/recursive/indirect calls, scalar-signature
//! `extern` calls, user `extern` calls with `str` parameters/returns through
//! generated C shims, strings/interpolation, arrays, structs, enums,
//! `Result`/`try`/`match`, maps, sockets/TLS, terminal/environment/process
//! builtins, graphics, interactive `canvas_*`/`clipboard_*`, and
//! `img_load`/`svg_load`/`tt_*`. Aggregate-returning runtime calls cross
//! the ABI through `runtime/osc_native_shim.c`.
//!
//! The remaining source-level limitation is user-declared `extern`
//! functions with aggregate types other than `str` (struct, payload enum, or
//! `Result`) as a parameter or return type. Those require an explicit C ABI
//! shim; the object backends report a compile error rather than trying to
//! classify platform aggregate ABI layouts directly. Nested enum payload
//! subpatterns are rejected by the language grammar, so they are not a
//! backend gap.
//!
//! # Runtime modes
//!
//! An object backend selects [`RuntimeMode::Freestanding`] by default:
//! no libc/UCRT/glibc dependency, only the small per-target system import
//! libraries documented by the runtime-archive contract. `--libc`
//! explicitly selects [`RuntimeMode::Hosted`] instead, using the hosted
//! archive plus the toolchain's normal CRT/libm/system libraries. Neither
//! mode ever falls back to the other.
//!
//! In freestanding mode, `osc_runtime.c`'s `osc_tls_connect` is the real
//! implementation (BearSSL on Linux, Schannel via secur32/crypt32 on
//! Windows) rather than the hosted stub, so `tls_fetch` behaves identically
//! under `--backend cranelift` and the freestanding C oracle.
//! `synthesize_main_entry` emits the same real `main(argc, argv)` the C
//! backend's `emit_main_wrapper` does (see `src/codegen.rs`). The hosted
//! CRT calls it normally; the freestanding archive's `_start`/
//! `mainCRTStartup` calls it without a CRT. Only the freestanding object
//! calls the exported `osc_freestanding_env_init` wrapper, because hosted
//! environment access uses libc directly. See `link.rs` for the distinct
//! final-link plans.

// A build with no object backend (`--no-default-features --features
// backend-c`) still compiles this tree for the handful of pieces the C
// path shares with it (e.g. `link::is_system_library_name`); the rest —
// runtime archives, linker discovery, embedded native-link assets — has
// no caller in that configuration by construction, not by accident.
#![cfg_attr(
    not(any(feature = "backend-cranelift", feature = "backend-llvm")),
    allow(dead_code)
)]

// The shared object-backend machinery (semantic lowering onto the
// backend-independent `lir` interface) exists only when at least one
// object backend does.
#[cfg(feature = "backend-cranelift")]
mod cranelift_debug;
#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
mod ctx;
pub mod distribution_contract;
#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
mod extern_shim;
#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
mod func;
#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
mod layout;
pub mod link;
#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
pub mod lir;
#[cfg(feature = "backend-cranelift")]
mod lir_cranelift;
#[cfg(feature = "backend-llvm")]
pub mod llvm;
pub mod native_assets;
pub mod no_toolchain;
pub mod select;
pub mod target;

#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
use crate::error::CompileError;
#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
use crate::ir;

#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
use ctx::BackendContext;
#[cfg(feature = "backend-cranelift")]
use lir::LirArtifact;
#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
use lir::{LLinkage, LSig, LType, LirBuilder, LirError, LirModule};
pub use target::NativeTarget;

/// Runtime and final-link environment for a native-backend artifact.
///
/// This is deliberately an enum rather than a `use_libc`/`freestanding`
/// boolean so object generation, runtime archive selection, shim compilation,
/// and final linking cannot accidentally interpret the same flag differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    Freestanding,
    Hosted,
}

#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
pub struct NativeCompileOutput {
    pub object_bytes: Vec<u8>,
    pub generated_extern_shim_c: Option<String>,
}

impl RuntimeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Freestanding => "freestanding",
            Self::Hosted => "hosted",
        }
    }
}

impl std::fmt::Display for RuntimeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compile `program` for `target` into a relocatable object file's raw
/// bytes using the Cranelift code generator. Never falls back to the C
/// backend: any construct this backend cannot lower is reported here as a
/// [`CompileError`] naming the unsupported construct and its source
/// location.
#[cfg(feature = "backend-cranelift")]
pub fn compile_object(
    program: &ir::Program,
    target: NativeTarget,
    runtime_mode: RuntimeMode,
    debug_info: crate::debuginfo::DebugInfo,
    source_map: &crate::debuginfo::SourceMap,
) -> Result<NativeCompileOutput, CompileError> {
    let mut lir = lir_cranelift::CraneliftLir::new(target, debug_info, source_map)
        .map_err(|e| CompileError::new(crate::token::Span::new(1, 1), e))?;
    let mut ctx = BackendContext::new(program, runtime_mode, debug_info, source_map);
    let generated_extern_shim_c = translate_program(&mut ctx, &mut lir)?;
    let object_bytes = match lir.finish() {
        Ok(LirArtifact::Object(bytes)) => bytes,
        Ok(LirArtifact::LlvmIr(_)) => {
            return Err(CompileError::new(
                crate::token::Span::new(1, 1),
                "internal error: Cranelift backend produced LLVM IR".to_string(),
            ))
        }
        Err(e) => return Err(CompileError::new(crate::token::Span::new(1, 1), e)),
    };
    Ok(NativeCompileOutput {
        object_bytes,
        generated_extern_shim_c,
    })
}

/// Run the shared semantic lowering (`func.rs`) plus the shared entry
/// wrapper over `lir`, and return any generated C extern shim source.
///
/// This is the single point every object backend goes through, so the
/// Cranelift and direct-LLVM paths cannot diverge on language semantics.
#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
pub(crate) fn translate_program(
    ctx: &mut BackendContext,
    lir: &mut dyn LirModule,
) -> Result<Option<String>, CompileError> {
    func::declare_and_translate_all(ctx, lir)?;
    synthesize_main_entry(ctx, lir)?;
    ctx.generated_extern_shim_source().map_err(|e| {
        CompileError::new(
            crate::token::Span::new(1, 1),
            format!("internal error generating native extern shims: {e}"),
        )
    })
}

/// Emit the real, C-ABI `int main(int argc, char** argv)`: stash `argc`/
/// `argv` into runtime globals, initialize environment access when the
/// selected mode is freestanding, create the top-level arena, call
/// `oscan_main`, tear the arena down, and translate a `Result` return into
/// a process exit code. Mirrors `src/codegen.rs`'s `emit_main_wrapper`.
#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
fn synthesize_main_entry(
    ctx: &mut BackendContext,
    lir: &mut dyn LirModule,
) -> Result<(), CompileError> {
    let span = crate::token::Span::new(1, 1);
    let internal = |message: String| CompileError::new(span, message);

    let (oscan_main, main_return_ty) = {
        let f = program_main(ctx.program)?;
        (ctx.functions["main"], f.return_type.clone())
    };
    let runtime_mode = ctx.runtime_mode;

    let sig = LSig::new(vec![LType::I32, LType::Ptr], Some(LType::I32));
    let main_id = lir
        .declare_function("main", &sig, LLinkage::Export)
        .map_err(|e| internal(format!("internal error declaring main: {e}")))?;

    lir.define_function(main_id, &sig, &mut |b, params| {
        let argc = params[0];
        let argv = params[1];

        let argc_data = b.declare_import_data("osc_global_argc");
        let argc_addr = b.global_addr(argc_data);
        b.store(argc, argc_addr, 0);

        let argv_data = b.declare_import_data("osc_global_argv");
        let argv_addr = b.global_addr(argv_data);
        b.store(argv, argv_addr, 0);

        if runtime_mode == RuntimeMode::Freestanding {
            // Initialize the freestanding runtime's argv-derived
            // environment table. Hosted mode uses the process CRT
            // environment directly and intentionally does not export this
            // freestanding-only wrapper.
            let env_init = declare_runtime(
                b,
                "osc_freestanding_env_init",
                &[LType::I32, LType::Ptr],
                None,
            )?;
            b.call(env_init, &[argc, argv]);
        }

        let create = declare_runtime(b, "osc_arena_create", &[LType::I64], Some(LType::Ptr))?;
        let cap = b.iconst(LType::I64, 1_048_576);
        let arena_ptr = b
            .call(create, &[cap])
            .expect("osc_arena_create returns a pointer");

        let arena_data = b.declare_import_data("osc_global_arena");
        let arena_addr = b.global_addr(arena_data);
        b.store(arena_ptr, arena_addr, 0);

        let main_result = b.call(oscan_main, &[arena_ptr]);

        // Exit code: 0 normally, or (for a `Result`-returning `main`) 0 on
        // `Ok`/1 on `Err` — matches `src/codegen.rs`'s `emit_main_wrapper`.
        // This must be computed *before* the arena is destroyed below. A
        // `Result`-returning `oscan_main` returns a *pointer* into arena
        // memory (see `src/backend/func.rs` module docs on inline
        // aggregates), unlike the C backend's
        // `osc_result_xxx _result = oscan_main(_arena);`, whose real C
        // struct-return ABI already copies the whole `Result` — including
        // its `is_ok` discriminator — out into a local variable before
        // `osc_arena_destroy` ever runs. Reading `is_ok` through that
        // pointer *after* `osc_arena_destroy` — which unmaps (freestanding)
        // or frees (hosted) every block the arena ever allocated,
        // including the one backing this very `Result` — would be a
        // use-after-free/use-after-unmap read of already-released memory.
        // Computing the exit code first, while the arena is still alive,
        // reduces it to a plain scalar SSA value that remains perfectly
        // valid to return after the arena underneath it is gone.
        let exit_code = match &main_return_ty {
            crate::types::BcType::Result(_, _) => {
                let ptr = main_result.expect("Result-returning main has a pointer value");
                let is_ok = b.load(LType::I8, ptr, 0);
                let ok_blk = b.create_block();
                let err_blk = b.create_block();
                let done_blk = b.create_block();
                b.append_block_param(done_blk, LType::I32);
                b.brif(is_ok, ok_blk, &[], err_blk, &[]);

                b.switch_to_block(ok_blk);
                b.seal_block(ok_blk);
                let zero = b.iconst(LType::I32, 0);
                b.jump(done_blk, &[zero]);

                b.switch_to_block(err_blk);
                b.seal_block(err_blk);
                let one = b.iconst(LType::I32, 1);
                b.jump(done_blk, &[one]);

                b.seal_block(done_blk);
                b.switch_to_block(done_blk);
                b.block_param(done_blk, 0)
            }
            _ => b.iconst(LType::I32, 0),
        };

        let destroy = declare_runtime(b, "osc_arena_destroy", &[LType::Ptr], None)?;
        b.call(destroy, &[arena_ptr]);

        b.ret(Some(exit_code));
        Ok(())
    })
    .map_err(|e| match e {
        LirError::Body(err) => err,
        LirError::Backend(message) => {
            internal(format!("internal error compiling entry point: {message}"))
        }
    })
}

#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
fn declare_runtime(
    b: &mut dyn LirBuilder,
    symbol: &str,
    params: &[LType],
    ret: Option<LType>,
) -> Result<lir::LFunc, CompileError> {
    b.declare_function(symbol, &LSig::new(params.to_vec(), ret), LLinkage::Import)
        .map_err(|e| {
            CompileError::new(
                crate::token::Span::new(1, 1),
                format!("internal error declaring {symbol}: {e}"),
            )
        })
}

#[cfg(any(feature = "backend-cranelift", feature = "backend-llvm"))]
fn program_main(program: &ir::Program) -> Result<&ir::FnDef, CompileError> {
    program
        .fn_defs
        .iter()
        .find(|f| f.name == "main")
        .ok_or_else(|| {
            CompileError::new(
                crate::token::Span::new(1, 1),
                "no 'main' function found".to_string(),
            )
        })
}
