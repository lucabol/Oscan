//! Direct LLVM object backend.
//!
//! `--backend llvm` runs the same shared semantic lowering the Cranelift
//! backend does (`super::func`, via `super::lir`), emits deterministic
//! typed LLVM IR (`emit`), and hands that IR to Oscan's own packaged
//! `libLLVM`, loaded in-process (`provider`), which parses, verifies,
//! optimizes, and emits a relocatable object. Final linking is handled by
//! [`super::link`] exactly as it is for Cranelift.
//!
//! # Invariants this module enforces
//!
//! * **No C.** No `.c`/`.h` file is written, `crate::codegen` is never
//!   consulted, and no compiler driver (`clang`/`gcc`/`cc`) is involved.
//! * **No code-generation subprocess.** `llvm-as`, `opt`, and `llc` are
//!   never spawned; only the existing embedded/direct linker may be, and
//!   that happens later, in `super::link`.
//! * **No installed LLVM SDK.** The provider is a packaged artifact
//!   resolved along an executable-relative path (see
//!   [`provider::search_candidates`]); a plain `cargo build` needs no LLVM
//!   at all.
//! * **Gated targets.** A target is only offered when the *packaged*
//!   library exports that back end's initializers. The Windows library
//!   Oscan ships today has X86 and AArch64 but no RISC-V, and the backend
//!   says so rather than emitting a broken object.
//! * **Freestanding-safe output.** Functions carry LLVM's `"no-builtins"`
//!   marker (the equivalent of Clang's `-ffreestanding`), and the emitted
//!   object is audited for unresolvable libc references before it is handed
//!   to the linker.

pub mod emit;
pub mod provider;

use object::{Architecture, BinaryFormat, Object, ObjectKind, ObjectSymbol, SymbolKind};

use crate::error::CompileError;
use crate::ir;
use crate::token::Span;

use super::ctx::BackendContext;
use super::lir::{LirArtifact, LirModule};
use super::{NativeCompileOutput, NativeTarget, RuntimeMode};

pub use provider::{LlvmProvider, TargetArch};

/// The LLVM target triple for each native target.
///
/// Windows uses the `-gnu` (MinGW-w64) environment because that is the
/// ABI Oscan's packaged Windows runtime archive and `ld.lld` link plan are
/// built against (see `src/backend/link/`).
pub fn target_triple(target: NativeTarget) -> &'static str {
    match target {
        NativeTarget::WindowsX64 => "x86_64-w64-windows-gnu",
        NativeTarget::LinuxX64 => "x86_64-unknown-linux-gnu",
        NativeTarget::LinuxAarch64 => "aarch64-unknown-linux-gnu",
        NativeTarget::LinuxRiscv64 => "riscv64-unknown-linux-gnu",
    }
}

/// The LLVM back end a native target needs.
pub fn target_arch(target: NativeTarget) -> TargetArch {
    match target {
        NativeTarget::WindowsX64 | NativeTarget::LinuxX64 => TargetArch::X86_64,
        NativeTarget::LinuxAarch64 => TargetArch::Aarch64,
        NativeTarget::LinuxRiscv64 => TargetArch::Riscv64,
    }
}

/// A loaded provider plus the checks the backend performs once per
/// invocation.
pub struct LlvmBackend {
    provider: LlvmProvider,
}

impl LlvmBackend {
    /// Load the packaged code generator. Fails with an actionable
    /// diagnostic (never a silent fallback to another backend).
    pub fn load() -> Result<Self, String> {
        LlvmProvider::load().map(|provider| LlvmBackend { provider })
    }

    pub fn describe(&self) -> String {
        format!(
            "{} (LLVM {}, targets: {})",
            self.provider.path().display(),
            self.provider.version_string(),
            self.provider.capabilities().describe()
        )
    }

    /// Whether this provider can emit object code for `target`.
    pub fn supports(&self, target: NativeTarget) -> bool {
        self.provider.capabilities().supports(target_arch(target))
    }

    fn require_target(&self, target: NativeTarget) -> Result<(), CompileError> {
        if self.supports(target) {
            return Ok(());
        }
        Err(compile_error(format!(
            "the LLVM backend cannot target {target}: Oscan's packaged LLVM code generator ({}) \
             has no {} back end (it provides: {}). Use --backend cranelift for this target, or a \
             release whose packaged code generator includes it.",
            self.provider.path().display(),
            target_arch(target).as_str(),
            self.provider.capabilities().describe()
        )))
    }
}

/// The IR text plus, when requested, the emitted object bytes.
pub struct LlvmCompileOutput {
    pub ir_text: String,
    pub object_bytes: Option<Vec<u8>>,
    /// Generated C shim source for user `extern` functions whose
    /// signature contains `str`. Identical to the Cranelift backend's:
    /// the *runtime* C ABI shim is still a C translation unit compiled by
    /// the final link step, which is a link-time concern, not an LLVM
    /// code-generation one. `None` for every program that declares no such
    /// extern.
    pub generated_extern_shim_c: Option<String>,
}

/// Lower `program` all the way to LLVM IR and (unless `emit_object` is
/// false) a relocatable object, without generating C or spawning anything.
pub fn compile_object(
    backend: &LlvmBackend,
    program: &ir::Program,
    target: NativeTarget,
    runtime_mode: RuntimeMode,
    emit_object: bool,
    debug_info: crate::debuginfo::DebugInfo,
    source_map: &crate::debuginfo::SourceMap,
) -> Result<LlvmCompileOutput, CompileError> {
    backend.require_target(target)?;
    let triple = target_triple(target);

    // Ask the *actual* TargetMachine for its data layout so the module's
    // `target datalayout` can never disagree with the machine that
    // compiles it (a mismatch silently changes struct offsets).
    let data_layout = backend
        .provider
        .data_layout_for(triple)
        .map_err(compile_error)?;

    let mut emitter = emit::LlvmEmitter::new(triple, &data_layout, debug_info, source_map);
    let mut ctx = BackendContext::new(program, runtime_mode, debug_info, source_map);
    let generated_extern_shim_c = super::translate_program(&mut ctx, &mut emitter)?;

    let ir_text = match emitter.finish() {
        Ok(LirArtifact::LlvmIr(text)) => text,
        Ok(LirArtifact::Object(_)) => {
            return Err(compile_error(
                "internal error: the LLVM emitter produced an object instead of IR".to_string(),
            ))
        }
        Err(e) => return Err(compile_error(e)),
    };

    let object_bytes = if emit_object {
        let bytes = backend
            .provider
            .compile_ir_to_object(
                &ir_text,
                triple,
                provider::OptimizationLevel::FreestandingSafe,
            )
            .map_err(compile_error)?;
        validate_object(&bytes, target)?;
        if runtime_mode == RuntimeMode::Freestanding {
            audit_freestanding_symbols(&bytes)?;
        }
        Some(bytes)
    } else {
        // `--emit-llvm-ir` still runs the parser and the module verifier,
        // so textual output is never something LLVM would reject: the IR
        // the user sees is exactly the IR the object path would compile.
        backend
            .provider
            .verify_ir(&ir_text, triple)
            .map_err(compile_error)?;
        None
    };

    Ok(LlvmCompileOutput {
        ir_text,
        object_bytes,
        generated_extern_shim_c,
    })
}

fn compile_error(message: impl Into<String>) -> CompileError {
    CompileError::new(Span::new(1, 1), message.into())
}

/// Check that the emitted bytes really are a relocatable object of the
/// expected format and architecture. A code generator that quietly
/// produced the wrong thing must fail here, not at link time.
fn validate_object(bytes: &[u8], target: NativeTarget) -> Result<(), CompileError> {
    if bytes.is_empty() {
        return Err(compile_error("LLVM emitted an empty object file"));
    }
    let file = object::File::parse(bytes)
        .map_err(|e| compile_error(format!("LLVM emitted an invalid object file: {e}")))?;
    if file.kind() != ObjectKind::Relocatable {
        return Err(compile_error(format!(
            "LLVM emitted {:?}, expected a relocatable object",
            file.kind()
        )));
    }
    let expected_format = match target {
        NativeTarget::WindowsX64 => BinaryFormat::Coff,
        _ => BinaryFormat::Elf,
    };
    if file.format() != expected_format {
        return Err(compile_error(format!(
            "LLVM emitted {:?} for target {}, expected {:?}",
            file.format(),
            target,
            expected_format
        )));
    }
    let expected_arch = match target {
        NativeTarget::WindowsX64 | NativeTarget::LinuxX64 => Architecture::X86_64,
        NativeTarget::LinuxAarch64 => Architecture::Aarch64,
        NativeTarget::LinuxRiscv64 => Architecture::Riscv64,
    };
    if file.architecture() != expected_arch {
        return Err(compile_error(format!(
            "LLVM emitted architecture {:?} for target {}, expected {:?}",
            file.architecture(),
            target,
            expected_arch
        )));
    }
    Ok(())
}

/// Symbols a freestanding Oscan object must never reference.
///
/// The freestanding runtime archive deliberately exports no libc: it has
/// only `static inline` equivalents private to its own translation units
/// (see `deps/laststanding/l_os.h`). If the optimizer ever synthesizes a
/// call to one of these — the classic failure mode of `loop-idiom` and
/// `memcpyopt` turning a hand-emitted load/store sequence back into a
/// `memcpy` — the link fails with an unhelpful "undefined reference".
/// Catching it here names the real cause instead.
const FREESTANDING_FORBIDDEN_SYMBOLS: [&str; 6] =
    ["memcpy", "memmove", "memset", "memcmp", "bcmp", "strlen"];

/// Whether an undefined symbol name is one of the libc entry points a
/// freestanding Oscan link cannot resolve.
///
/// Names are compared with leading underscores stripped (COFF/Mach-O
/// decoration and LLVM's `__`-prefixed variants) and with a trailing
/// `_suffix` allowed, so `_memcpy`, `__memcpy_chk`, and
/// `__memmove_avx_unaligned` are all recognized.
fn is_forbidden_freestanding_symbol(name: &str) -> bool {
    let bare = name.trim_start_matches('_');
    FREESTANDING_FORBIDDEN_SYMBOLS.iter().any(|forbidden| {
        bare == *forbidden
            || bare
                .strip_prefix(forbidden)
                .is_some_and(|rest| rest.starts_with('_'))
    })
}

fn audit_freestanding_symbols(bytes: &[u8]) -> Result<(), CompileError> {
    let file = object::File::parse(bytes)
        .map_err(|e| compile_error(format!("LLVM emitted an invalid object file: {e}")))?;
    let mut found: Vec<String> = Vec::new();
    for symbol in file.symbols() {
        if !symbol.is_undefined() || symbol.kind() == SymbolKind::Section {
            continue;
        }
        let Ok(name) = symbol.name() else { continue };
        if is_forbidden_freestanding_symbol(name) && !found.iter().any(|f| f == name) {
            found.push(name.to_string());
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    found.sort();
    Err(compile_error(format!(
        "the LLVM optimizer produced references to libc symbols the freestanding runtime does not \
         provide ({}); this is an Oscan code-generation bug, not a program error — please report \
         it. Use --libc for a hosted build, or --backend cranelift, as a workaround.",
        found.join(", ")
    )))
}

/// Convert an LLVM backend result into the shape the shared object
/// orchestration in `main.rs` consumes.
impl From<LlvmCompileOutput> for Option<NativeCompileOutput> {
    fn from(value: LlvmCompileOutput) -> Self {
        value.object_bytes.map(|object_bytes| NativeCompileOutput {
            object_bytes,
            generated_extern_shim_c: value.generated_extern_shim_c,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_triples_cover_every_native_target() {
        assert_eq!(
            target_triple(NativeTarget::WindowsX64),
            "x86_64-w64-windows-gnu"
        );
        assert_eq!(
            target_triple(NativeTarget::LinuxX64),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            target_triple(NativeTarget::LinuxAarch64),
            "aarch64-unknown-linux-gnu"
        );
        assert_eq!(
            target_triple(NativeTarget::LinuxRiscv64),
            "riscv64-unknown-linux-gnu"
        );
    }

    #[test]
    fn target_arch_maps_every_native_target_to_an_llvm_back_end() {
        assert_eq!(target_arch(NativeTarget::WindowsX64), TargetArch::X86_64);
        assert_eq!(target_arch(NativeTarget::LinuxX64), TargetArch::X86_64);
        assert_eq!(target_arch(NativeTarget::LinuxAarch64), TargetArch::Aarch64);
        assert_eq!(target_arch(NativeTarget::LinuxRiscv64), TargetArch::Riscv64);
    }

    #[test]
    fn an_empty_object_is_rejected() {
        let err = validate_object(&[], NativeTarget::LinuxX64).expect_err("empty must fail");
        assert!(err.message.contains("empty object file"), "{}", err.message);
    }

    #[test]
    fn a_non_object_payload_is_rejected() {
        let err = validate_object(b"not an object at all", NativeTarget::LinuxX64)
            .expect_err("garbage must fail");
        assert!(
            err.message.contains("invalid object file"),
            "{}",
            err.message
        );
    }

    #[test]
    fn the_freestanding_symbol_audit_covers_the_classic_libcall_regressions() {
        // These are exactly the symbols `loop-idiom-recognize`/`memcpyopt`
        // would introduce if they ever made it into the pipeline.
        for symbol in ["memcpy", "memmove", "memset"] {
            assert!(FREESTANDING_FORBIDDEN_SYMBOLS.contains(&symbol));
        }
    }

    #[test]
    fn forbidden_symbol_matching_handles_platform_decoration_and_variants() {
        for name in [
            "memcpy",
            "_memcpy",
            "__memcpy_chk",
            "__memmove_avx_unaligned",
            "memset",
            "_bcmp",
            "strlen",
        ] {
            assert!(
                is_forbidden_freestanding_symbol(name),
                "'{name}' must be recognized"
            );
        }
        // Oscan's own runtime symbols must never be mistaken for libc,
        // even when they contain a forbidden name as a substring.
        for name in [
            "osc_arena_alloc",
            "oscan_main",
            "osc_memcpy_helper",
            "memcpying",
            "memcpyX",
            "strlength",
        ] {
            assert!(
                !is_forbidden_freestanding_symbol(name),
                "'{name}' must not be flagged"
            );
        }
    }
}
