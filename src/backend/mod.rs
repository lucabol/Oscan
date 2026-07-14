//! Cranelift AOT native backend.
//!
//! Consumes the same [`crate::ir::Program`] the C backend
//! (`crate::codegen`) does, and produces a relocatable object file ready
//! to link against a static Oscan runtime archive (see `link.rs`). This
//! module is the *only* place Cranelift/`cranelift-object`/`target-lexicon`
//! types are visible outside `src/backend/`; `main.rs` only ever calls
//! [`compile_object`]/`link.rs`'s public entry points, never touches
//! Cranelift directly, and the C backend (`crate::codegen`) is untouched
//! by any of this — selecting `--backend native` never runs `crate::codegen`
//! at all (see module docs on "no silent fallback" in `main.rs`).
//!
//! # Coverage
//!
//! Implemented: `i32`/`i64`/`f64`/`bool` arithmetic (via the same checked
//! `osc_*` runtime helpers the C backend calls, for bit-identical
//! overflow/panic behavior), comparisons, short-circuiting `and`/`or`,
//! casts, `if`/`while`/`for`/`for-in`/`break`/`continue`/`return`/blocks,
//! `defer`, user function definitions and recursive/forward calls,
//! `extern` calls with scalar signatures, indirect calls through
//! function-pointer values, string literals/interpolation and a curated
//! set of `str`-returning/accepting runtime builtins (via
//! `runtime/osc_native_shim.c`), dynamic/fixed arrays (`len`/`push`/`pop`/
//! literals/indexing), structs (literals/field access, including nested
//! structs and array/str fields), payload and non-payload enums,
//! `Result`/`try`/`match` (literal, wildcard/ident, bool, and enum
//! patterns with simple identifier/wildcard payload bindings), the
//! untyped `map` and all five typed `map_<k>_<v>` hashmap families,
//! TCP/UDP/Unix-domain sockets, TLS (`tls_connect`/`tls_send`/`tls_recv`/
//! `tls_recv_byte`/`tls_close`/`tls_cleanup`), terminal
//! (`term_width`/`term_height`/`term_raw`/`term_restore`/`read_nonblock`/
//! `is_tty`), environment iteration (`env_count`/`env_key`/`env_value`),
//! directory listing (`dir_list`/`dir_change`), process/pipes
//! (`proc_run`/`proc_spawn`/`proc_wait`/`pipe_create`/`fd_dup`/`fd_dup2`/
//! `path_find_exec`), and the non-interactive graphics primitives
//! (`gfx_pixel`..`gfx_text_width`/`gfx_blit`/`gfx_blit_alpha`, `rgb`/`rgba`).
//!
//! Not implemented (reported as a compile error naming the exact
//! construct, never a panic or silent miscompilation): `extern`
//! declarations with a `str`/struct/payload-enum/`Result` parameter or
//! return type; the interactive `canvas_*`/`clipboard_*` builtins (they
//! open a real OS window / touch the desktop clipboard — no headless
//! test coverage exists for either backend) and `img_load`/`svg_load`/
//! `tt_*` (image/SVG/TrueType asset decoding, which also need a
//! `Result<[i32], str>`/`Result<handle, str>` shim shape the curated list
//! above does not yet have); nested literal/enum sub-patterns inside an
//! enum payload binding. See `native-completeness` for the tracked plan
//! to close these.
//!
//! # Runtime modes
//!
//! `--backend native` selects [`RuntimeMode::Freestanding`] by default:
//! no libc/UCRT/glibc dependency, only the small per-target system import
//! libraries documented by the runtime-archive contract. `--libc
//! --backend native` explicitly selects [`RuntimeMode::Hosted`] instead,
//! using the hosted archive plus the toolchain's normal CRT/libm/system
//! libraries. Neither mode ever falls back to the other.
//!
//! In freestanding mode, `osc_runtime.c`'s `osc_tls_connect` is the real
//! implementation (BearSSL on Linux, Schannel via secur32/crypt32 on
//! Windows) rather than the hosted stub, so `tls_fetch` behaves identically
//! under `--backend native` and the freestanding C oracle.
//! `synthesize_main_entry` emits the same real `main(argc, argv)` the C
//! backend's `emit_main_wrapper` does (see `src/codegen.rs`). The hosted
//! CRT calls it normally; the freestanding archive's `_start`/
//! `mainCRTStartup` calls it without a CRT. Only the freestanding object
//! calls the exported `osc_freestanding_env_init` wrapper, because hosted
//! environment access uses libc directly. See `link.rs` for the distinct
//! final-link plans.

mod ctx;
mod func;
mod layout;
pub mod link;
pub mod target;

use cranelift_codegen::ir::{AbiParam, InstBuilder, UserFuncName};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::error::CompileError;
use crate::ir;

use ctx::BackendContext;
use layout::cl_pointer_type;
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
/// bytes. Never falls back to the C backend: any construct this backend
/// cannot lower is reported here as a [`CompileError`] naming the
/// unsupported construct and its source location.
pub fn compile_object(
    program: &ir::Program,
    target: NativeTarget,
    runtime_mode: RuntimeMode,
) -> Result<Vec<u8>, CompileError> {
    let isa = target::build_isa(target)
        .map_err(|e| CompileError::new(crate::token::Span::new(1, 1), e))?;
    let builder = ObjectBuilder::new(
        isa,
        "oscan_program",
        cranelift_module::default_libcall_names(),
    )
    .map_err(|e| {
        CompileError::new(
            crate::token::Span::new(1, 1),
            format!("internal error configuring object writer: {e}"),
        )
    })?;
    let module = ObjectModule::new(builder);

    let mut ctx = BackendContext::new(module, program, runtime_mode);
    func::declare_and_translate_all(&mut ctx)?;
    synthesize_main_entry(&mut ctx)?;

    let product = ctx.module.finish();
    product.emit().map_err(|e| {
        CompileError::new(
            crate::token::Span::new(1, 1),
            format!("internal error emitting object file: {e}"),
        )
    })
}

/// Emit the real, C-ABI `int main(int argc, char** argv)`: stash `argc`/
/// `argv` into runtime globals, initialize environment access when the
/// selected mode is freestanding, create the top-level arena, call
/// `oscan_main`, tear the arena down, and translate a `Result` return into
/// a process exit code. Mirrors `src/codegen.rs`'s `emit_main_wrapper`.
fn synthesize_main_entry(ctx: &mut BackendContext) -> Result<(), CompileError> {
    use cranelift_codegen::ir::types;

    let argc_data = ctx
        .module
        .declare_data("osc_global_argc", Linkage::Import, true, false)
        .expect("internal error declaring osc_global_argc");
    let argv_data = ctx
        .module
        .declare_data("osc_global_argv", Linkage::Import, true, false)
        .expect("internal error declaring osc_global_argv");
    let arena_data = ctx
        .module
        .declare_data("osc_global_arena", Linkage::Import, true, false)
        .expect("internal error declaring osc_global_arena");

    let mut sig = ctx.module.make_signature();
    sig.params.push(AbiParam::new(types::I32));
    sig.params.push(AbiParam::new(cl_pointer_type()));
    sig.returns.push(AbiParam::new(types::I32));
    let main_id = ctx
        .module
        .declare_function("main", Linkage::Export, &sig)
        .expect("internal error declaring main");

    let mut func = cranelift_codegen::ir::Function::with_name_signature(
        UserFuncName::user(0, main_id.as_u32()),
        sig,
    );
    let mut fb_ctx = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut func, &mut fb_ctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        let argc = b.block_params(entry)[0];
        let argv = b.block_params(entry)[1];

        let argc_gv = ctx.module.declare_data_in_func(argc_data, b.func);
        let argc_addr = b.ins().global_value(cl_pointer_type(), argc_gv);
        b.ins().store(func::mem_flags(), argc, argc_addr, 0);

        let argv_gv = ctx.module.declare_data_in_func(argv_data, b.func);
        let argv_addr = b.ins().global_value(cl_pointer_type(), argv_gv);
        b.ins().store(func::mem_flags(), argv, argv_addr, 0);

        if ctx.runtime_mode == RuntimeMode::Freestanding {
            // Initialize the freestanding runtime's argv-derived environment
            // table. Hosted mode uses the process CRT environment directly and
            // intentionally does not export this freestanding-only wrapper.
            let env_init_sig_id = {
                let mut s = ctx.module.make_signature();
                s.params.push(AbiParam::new(types::I32));
                s.params.push(AbiParam::new(cl_pointer_type()));
                ctx.module
                    .declare_function("osc_freestanding_env_init", Linkage::Import, &s)
                    .expect("internal error declaring osc_freestanding_env_init")
            };
            let env_init_ref = ctx.module.declare_func_in_func(env_init_sig_id, b.func);
            b.ins().call(env_init_ref, &[argc, argv]);
        }

        let create_sig_id = {
            let mut s = ctx.module.make_signature();
            s.params.push(AbiParam::new(types::I64));
            s.returns.push(AbiParam::new(cl_pointer_type()));
            ctx.module
                .declare_function("osc_arena_create", Linkage::Import, &s)
                .expect("internal error declaring osc_arena_create")
        };
        let create_ref = ctx.module.declare_func_in_func(create_sig_id, b.func);
        let cap = b.ins().iconst(types::I64, 1_048_576);
        let call = b.ins().call(create_ref, &[cap]);
        let arena_ptr = b.inst_results(call)[0];

        let arena_gv = ctx.module.declare_data_in_func(arena_data, b.func);
        let arena_addr = b.ins().global_value(cl_pointer_type(), arena_gv);
        b.ins().store(func::mem_flags(), arena_ptr, arena_addr, 0);

        let (oscan_main_id, main_return_ty) = {
            let f = program_main(ctx.program)?;
            (ctx.functions["main"], f.return_type.clone())
        };
        let main_ref = ctx.module.declare_func_in_func(oscan_main_id, b.func);
        let call = b.ins().call(main_ref, &[arena_ptr]);
        let main_result = b.inst_results(call).first().copied();

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
                let is_ok = b.ins().load(types::I8, func::mem_flags(), ptr, 0);
                let ok_blk = b.create_block();
                let err_blk = b.create_block();
                let done_blk = b.create_block();
                b.append_block_param(done_blk, types::I32);
                b.ins().brif(is_ok, ok_blk, &[], err_blk, &[]);

                b.switch_to_block(ok_blk);
                b.seal_block(ok_blk);
                let zero = b.ins().iconst(types::I32, 0);
                b.ins()
                    .jump(done_blk, &[cranelift_codegen::ir::BlockArg::Value(zero)]);

                b.switch_to_block(err_blk);
                b.seal_block(err_blk);
                let one = b.ins().iconst(types::I32, 1);
                b.ins()
                    .jump(done_blk, &[cranelift_codegen::ir::BlockArg::Value(one)]);

                b.seal_block(done_blk);
                b.switch_to_block(done_blk);
                b.block_params(done_blk)[0]
            }
            _ => b.ins().iconst(types::I32, 0),
        };

        let destroy_sig_id = {
            let mut s = ctx.module.make_signature();
            s.params.push(AbiParam::new(cl_pointer_type()));
            ctx.module
                .declare_function("osc_arena_destroy", Linkage::Import, &s)
                .expect("internal error declaring osc_arena_destroy")
        };
        let destroy_ref = ctx.module.declare_func_in_func(destroy_sig_id, b.func);
        b.ins().call(destroy_ref, &[arena_ptr]);

        b.ins().return_(&[exit_code]);
        b.finalize();
    }

    let mut ctx_obj = ctx.module.make_context();
    ctx_obj.func = func;
    ctx.module
        .define_function(main_id, &mut ctx_obj)
        .map_err(|e| {
            CompileError::new(
                crate::token::Span::new(1, 1),
                format!("internal error compiling entry point: {e}"),
            )
        })?;
    Ok(())
}

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
