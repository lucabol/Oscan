//! In-process LLVM code generator: a small, audited, dynamically-loaded
//! binding to the bundled `libLLVM` C API.
//!
//! # Why a dynamic binding rather than `llvm-sys`
//!
//! `llvm-sys` requires a full LLVM SDK (headers, static libraries, and a
//! working `llvm-config`) to be present *at Oscan build time*. Oscan must
//! stay buildable with a plain `cargo build` on a machine with no LLVM
//! installed at all, and its releases must carry the code generator as a
//! packaged artifact rather than depending on whatever the user happens
//! to have installed. So this module loads exactly the handful of
//! documented, ABI-stable LLVM-C entry points it needs, from a shared
//! library resolved along an explicit, executable-relative search path.
//!
//! # What is *not* here
//!
//! No subprocess is ever spawned: `clang`, `llvm-as`, `opt`, and `llc`
//! are never invoked, and no `.ll`/`.bc`/`.o` temporary file is written.
//! The IR text produced by [`super::emit`] goes straight into
//! `LLVMParseIRInContext`, and the object bytes come straight back out of
//! `LLVMTargetMachineEmitToMemoryBuffer`.
//!
//! # Search path (in priority order)
//!
//! 1. `OSCAN_LLVM_LIB` — an explicit path to the shared library.
//! 2. `OSCAN_LLVM_DIR` / `OSCAN_TOOLCHAIN_DIR` — a directory to search.
//! 3. Executable-relative roots, in order: `<exe dir>/toolchain/...`,
//!    `<exe dir>/native-link/...` (a schema-v2 package's verified sidecar,
//!    where Windows shares one `libLLVM-22.dll` between this code
//!    generator and `ld.lld.exe`), and `<exe dir>` itself.
//!
//! A candidate inside the sidecar directory is only used once the sidecar
//! manifest has verified it (see
//! [`crate::backend::native_assets::sidecar`]).
//!
//! The bare platform loader search path is deliberately **not** used: a
//! code generator is executed code, so it must never be picked up from
//! the current working directory or an arbitrary `PATH` entry (the same
//! rule `crate::find_toolchain_dir` applies to the bundled toolchain).
//!
//! # Version gating
//!
//! `LLVMGetVersion` must report the exact major version this binding was
//! written against ([`REQUIRED_LLVM_MAJOR`]). The LLVM C API is only
//! stable within a major release, and silently binding a different one is
//! how a code generator starts miscompiling. Target support is likewise
//! probed, not assumed: [`ProviderCapabilities`] records which target
//! initializers the loaded library actually exports, and the backend
//! refuses a target the packaged library cannot emit for.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::path::{Path, PathBuf};

/// The LLVM major version this binding is written against. The LLVM C API
/// is only guaranteed stable within one major release.
pub const REQUIRED_LLVM_MAJOR: u32 = 22;

// -- Opaque LLVM handles ---------------------------------------------------

#[repr(C)]
struct LlvmContextOpaque {
    _private: [u8; 0],
}
#[repr(C)]
struct LlvmModuleOpaque {
    _private: [u8; 0],
}
#[repr(C)]
struct LlvmMemoryBufferOpaque {
    _private: [u8; 0],
}
#[repr(C)]
struct LlvmTargetOpaque {
    _private: [u8; 0],
}
#[repr(C)]
struct LlvmTargetMachineOpaque {
    _private: [u8; 0],
}
#[repr(C)]
struct LlvmTargetDataOpaque {
    _private: [u8; 0],
}
#[repr(C)]
struct LlvmPassBuilderOptionsOpaque {
    _private: [u8; 0],
}
#[repr(C)]
struct LlvmErrorOpaque {
    _private: [u8; 0],
}

type LlvmContextRef = *mut LlvmContextOpaque;
type LlvmModuleRef = *mut LlvmModuleOpaque;
type LlvmMemoryBufferRef = *mut LlvmMemoryBufferOpaque;
type LlvmTargetRef = *mut LlvmTargetOpaque;
type LlvmTargetMachineRef = *mut LlvmTargetMachineOpaque;
type LlvmTargetDataRef = *mut LlvmTargetDataOpaque;
type LlvmPassBuilderOptionsRef = *mut LlvmPassBuilderOptionsOpaque;
type LlvmErrorRef = *mut LlvmErrorOpaque;

// `LLVMCodeGenOptLevel`. The IR pipeline is size-oriented and function
// definitions carry `minsize`; use the matching fully optimizing machine
// pipeline rather than LLVM's default codegen level.
const LLVM_CODEGEN_LEVEL_AGGRESSIVE: c_int = 3;
// `LLVMRelocMode`
const LLVM_RELOC_PIC: c_int = 2;
// `LLVMCodeModel`
const LLVM_CODEMODEL_DEFAULT: c_int = 0;
// `LLVMCodeGenFileType`
const LLVM_CODEGEN_OBJECT_FILE: c_int = 1;
// `LLVMVerifierFailureAction`
const LLVM_RETURN_STATUS_ACTION: c_int = 2;

type FnGetVersion = unsafe extern "C" fn(*mut c_uint, *mut c_uint, *mut c_uint);
type FnContextCreate = unsafe extern "C" fn() -> LlvmContextRef;
type FnContextDispose = unsafe extern "C" fn(LlvmContextRef);
type FnCreateMemoryBufferWithMemoryRange =
    unsafe extern "C" fn(*const c_char, usize, *const c_char, c_int) -> LlvmMemoryBufferRef;
type FnDisposeMemoryBuffer = unsafe extern "C" fn(LlvmMemoryBufferRef);
type FnGetBufferStart = unsafe extern "C" fn(LlvmMemoryBufferRef) -> *const c_char;
type FnGetBufferSize = unsafe extern "C" fn(LlvmMemoryBufferRef) -> usize;
type FnParseIRInContext = unsafe extern "C" fn(
    LlvmContextRef,
    LlvmMemoryBufferRef,
    *mut LlvmModuleRef,
    *mut *mut c_char,
) -> c_int;
type FnDisposeModule = unsafe extern "C" fn(LlvmModuleRef);
type FnDisposeMessage = unsafe extern "C" fn(*mut c_char);
type FnVerifyModule = unsafe extern "C" fn(LlvmModuleRef, c_int, *mut *mut c_char) -> c_int;
type FnGetTargetFromTriple =
    unsafe extern "C" fn(*const c_char, *mut LlvmTargetRef, *mut *mut c_char) -> c_int;
type FnCreateTargetMachine = unsafe extern "C" fn(
    LlvmTargetRef,
    *const c_char,
    *const c_char,
    *const c_char,
    c_int,
    c_int,
    c_int,
) -> LlvmTargetMachineRef;
type FnDisposeTargetMachine = unsafe extern "C" fn(LlvmTargetMachineRef);
type FnCreateTargetDataLayout = unsafe extern "C" fn(LlvmTargetMachineRef) -> LlvmTargetDataRef;
type FnCopyStringRepOfTargetData = unsafe extern "C" fn(LlvmTargetDataRef) -> *mut c_char;
type FnDisposeTargetData = unsafe extern "C" fn(LlvmTargetDataRef);
type FnSetTarget = unsafe extern "C" fn(LlvmModuleRef, *const c_char);
type FnSetModuleDataLayout = unsafe extern "C" fn(LlvmModuleRef, LlvmTargetDataRef);
type FnCreatePassBuilderOptions = unsafe extern "C" fn() -> LlvmPassBuilderOptionsRef;
type FnDisposePassBuilderOptions = unsafe extern "C" fn(LlvmPassBuilderOptionsRef);
type FnRunPasses = unsafe extern "C" fn(
    LlvmModuleRef,
    *const c_char,
    LlvmTargetMachineRef,
    LlvmPassBuilderOptionsRef,
) -> LlvmErrorRef;
type FnGetErrorMessage = unsafe extern "C" fn(LlvmErrorRef) -> *mut c_char;
type FnDisposeErrorMessage = unsafe extern "C" fn(*mut c_char);
type FnTargetMachineEmitToMemoryBuffer = unsafe extern "C" fn(
    LlvmTargetMachineRef,
    LlvmModuleRef,
    c_int,
    *mut *mut c_char,
    *mut LlvmMemoryBufferRef,
) -> c_int;
type FnInitializeTarget = unsafe extern "C" fn();

/// Which LLVM target back ends the loaded library actually exports
/// initializers for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub x86: bool,
    pub aarch64: bool,
    pub riscv: bool,
}

impl ProviderCapabilities {
    pub fn supports(&self, arch: TargetArch) -> bool {
        match arch {
            TargetArch::X86_64 => self.x86,
            TargetArch::Aarch64 => self.aarch64,
            TargetArch::Riscv64 => self.riscv,
        }
    }

    pub fn describe(&self) -> String {
        let mut names = Vec::new();
        if self.x86 {
            names.push("x86-64");
        }
        if self.aarch64 {
            names.push("aarch64");
        }
        if self.riscv {
            names.push("riscv64");
        }
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        }
    }
}

/// The LLVM target architecture family a native target needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetArch {
    X86_64,
    Aarch64,
    Riscv64,
}

impl TargetArch {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetArch::X86_64 => "x86-64",
            TargetArch::Aarch64 => "aarch64",
            TargetArch::Riscv64 => "riscv64",
        }
    }
}

// -- Platform shared-library loading ---------------------------------------

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }

    // Resolve the library's own dependencies from its own directory (the
    // packaged toolchain `bin/`), never from the process CWD or `PATH`.
    const LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR: u32 = 0x0000_0100;
    const LOAD_LIBRARY_SEARCH_DEFAULT_DIRS: u32 = 0x0000_1000;

    pub struct Library(*mut c_void);

    // SAFETY: a loaded module handle is just an opaque HMODULE; the LLVM
    // C API entry points this binding uses are called from one thread at
    // a time behind `&mut` access to the owning `LlvmProvider`.
    unsafe impl Send for Library {}

    impl Library {
        pub fn open(path: &Path) -> Result<Self, String> {
            let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
            wide.push(0);
            let handle = unsafe {
                LoadLibraryExW(
                    wide.as_ptr(),
                    std::ptr::null_mut(),
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                )
            };
            if handle.is_null() {
                let code = unsafe { GetLastError() };
                return Err(format!(
                    "LoadLibraryExW('{}') failed with error {code}",
                    path.display()
                ));
            }
            Ok(Library(handle))
        }

        pub fn symbol(&self, name: &str) -> Option<*mut c_void> {
            let mut bytes = name.as_bytes().to_vec();
            bytes.push(0);
            let address = unsafe { GetProcAddress(self.0, bytes.as_ptr()) };
            (!address.is_null()).then_some(address)
        }
    }

    impl Drop for Library {
        fn drop(&mut self) {
            unsafe {
                FreeLibrary(self.0);
            }
        }
    }

    pub fn library_file_names() -> &'static [&'static str] {
        &["libLLVM-22.dll", "LLVM-C.dll", "libLLVM.dll"]
    }
}

#[cfg(unix)]
mod sys {
    use std::ffi::{c_void, CString};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    extern "C" {
        fn dlopen(filename: *const i8, flag: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> i32;
        fn dlerror() -> *mut i8;
    }

    const RTLD_NOW: i32 = 2;
    const RTLD_LOCAL: i32 = 0;

    pub struct Library(*mut c_void);

    // SAFETY: see the Windows counterpart.
    unsafe impl Send for Library {}

    impl Library {
        pub fn open(path: &Path) -> Result<Self, String> {
            let c_path = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| format!("library path '{}' contains a NUL byte", path.display()))?;
            let handle = unsafe { dlopen(c_path.as_ptr() as *const i8, RTLD_NOW | RTLD_LOCAL) };
            if handle.is_null() {
                let message = unsafe {
                    let raw = dlerror();
                    if raw.is_null() {
                        "unknown dlopen failure".to_string()
                    } else {
                        std::ffi::CStr::from_ptr(raw).to_string_lossy().into_owned()
                    }
                };
                return Err(format!("dlopen('{}') failed: {message}", path.display()));
            }
            Ok(Library(handle))
        }

        pub fn symbol(&self, name: &str) -> Option<*mut c_void> {
            let c_name = CString::new(name).ok()?;
            let address = unsafe { dlsym(self.0, c_name.as_ptr() as *const i8) };
            (!address.is_null()).then_some(address)
        }
    }

    impl Drop for Library {
        fn drop(&mut self) {
            unsafe {
                dlclose(self.0);
            }
        }
    }

    pub fn library_file_names() -> &'static [&'static str] {
        &[
            "libLLVM.so.22.1",
            "libLLVM.so.22",
            "libLLVM-22.so",
            "libLLVM.so",
        ]
    }
}

// -- Provider --------------------------------------------------------------

macro_rules! bind {
    ($lib:expr, $missing:expr, $name:literal, $ty:ty) => {{
        match $lib.symbol($name) {
            Some(address) => Some(unsafe { std::mem::transmute::<*mut c_void, $ty>(address) }),
            None => {
                $missing.push($name);
                None
            }
        }
    }};
}

struct Api {
    get_version: FnGetVersion,
    context_create: FnContextCreate,
    context_dispose: FnContextDispose,
    create_memory_buffer: FnCreateMemoryBufferWithMemoryRange,
    dispose_memory_buffer: FnDisposeMemoryBuffer,
    get_buffer_start: FnGetBufferStart,
    get_buffer_size: FnGetBufferSize,
    parse_ir: FnParseIRInContext,
    dispose_module: FnDisposeModule,
    dispose_message: FnDisposeMessage,
    verify_module: FnVerifyModule,
    get_target_from_triple: FnGetTargetFromTriple,
    create_target_machine: FnCreateTargetMachine,
    dispose_target_machine: FnDisposeTargetMachine,
    create_target_data_layout: FnCreateTargetDataLayout,
    copy_string_rep_of_target_data: FnCopyStringRepOfTargetData,
    dispose_target_data: FnDisposeTargetData,
    set_target: FnSetTarget,
    set_module_data_layout: FnSetModuleDataLayout,
    create_pass_builder_options: FnCreatePassBuilderOptions,
    dispose_pass_builder_options: FnDisposePassBuilderOptions,
    run_passes: FnRunPasses,
    get_error_message: FnGetErrorMessage,
    dispose_error_message: FnDisposeErrorMessage,
    emit_to_memory_buffer: FnTargetMachineEmitToMemoryBuffer,
}

/// A loaded, validated LLVM code generator.
pub struct LlvmProvider {
    /// Kept alive for the provider's lifetime: every function pointer in
    /// `api` points into this module's image.
    _library: sys::Library,
    api: Api,
    path: PathBuf,
    version: (u32, u32, u32),
    capabilities: ProviderCapabilities,
}

impl LlvmProvider {
    /// Locate, load, and validate the packaged LLVM code generator.
    pub fn load() -> Result<Self, String> {
        let candidates = search_candidates();
        if candidates.is_empty() {
            return Err(no_provider_error());
        }
        let mut failures = Vec::new();
        for candidate in &candidates {
            // A candidate inside the executable-relative native-link
            // sidecar directory (Windows stores one `libLLVM-22.dll` there
            // and shares it between this code generator and `ld.lld.exe`)
            // is only ever loaded when the sidecar manifest declares it and
            // its SHA-256 matches. Anything else there is not "packaged",
            // it merely sits in a packaged directory.
            if let Err(reason) =
                crate::backend::native_assets::sidecar::require_verified_if_inside(candidate)
            {
                failures.push(format!("  {}: {reason}", candidate.display()));
                continue;
            }
            match Self::load_from(candidate) {
                Ok(provider) => return Ok(provider),
                Err(reason) => failures.push(format!("  {}: {reason}", candidate.display())),
            }
        }
        Err(format!(
            "{}\ntried:\n{}",
            no_provider_error(),
            failures.join("\n")
        ))
    }

    /// Load a specific library file. Public so tests (and
    /// `OSCAN_LLVM_LIB`) can exercise a precise path and get a precise
    /// diagnostic.
    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err(format!(
                "LLVM provider path '{}' must be absolute; relative paths could load code from the \
                 current working directory",
                path.display()
            ));
        }
        if !path.is_file() {
            return Err("not a file".to_string());
        }
        let library = sys::Library::open(path)?;

        let mut missing: Vec<&'static str> = Vec::new();
        let api = Api {
            get_version: bind!(library, missing, "LLVMGetVersion", FnGetVersion)
                .unwrap_or(unreachable_get_version),
            context_create: bind!(library, missing, "LLVMContextCreate", FnContextCreate)
                .unwrap_or(unreachable_context_create),
            context_dispose: bind!(library, missing, "LLVMContextDispose", FnContextDispose)
                .unwrap_or(unreachable_unit_ptr),
            create_memory_buffer: bind!(
                library,
                missing,
                "LLVMCreateMemoryBufferWithMemoryRange",
                FnCreateMemoryBufferWithMemoryRange
            )
            .unwrap_or(unreachable_create_buffer),
            dispose_memory_buffer: bind!(
                library,
                missing,
                "LLVMDisposeMemoryBuffer",
                FnDisposeMemoryBuffer
            )
            .unwrap_or(unreachable_unit_ptr),
            get_buffer_start: bind!(library, missing, "LLVMGetBufferStart", FnGetBufferStart)
                .unwrap_or(unreachable_buffer_start),
            get_buffer_size: bind!(library, missing, "LLVMGetBufferSize", FnGetBufferSize)
                .unwrap_or(unreachable_buffer_size),
            parse_ir: bind!(library, missing, "LLVMParseIRInContext", FnParseIRInContext)
                .unwrap_or(unreachable_parse_ir),
            dispose_module: bind!(library, missing, "LLVMDisposeModule", FnDisposeModule)
                .unwrap_or(unreachable_unit_ptr),
            dispose_message: bind!(library, missing, "LLVMDisposeMessage", FnDisposeMessage)
                .unwrap_or(unreachable_unit_char),
            verify_module: bind!(library, missing, "LLVMVerifyModule", FnVerifyModule)
                .unwrap_or(unreachable_verify),
            get_target_from_triple: bind!(
                library,
                missing,
                "LLVMGetTargetFromTriple",
                FnGetTargetFromTriple
            )
            .unwrap_or(unreachable_target_from_triple),
            create_target_machine: bind!(
                library,
                missing,
                "LLVMCreateTargetMachine",
                FnCreateTargetMachine
            )
            .unwrap_or(unreachable_create_target_machine),
            dispose_target_machine: bind!(
                library,
                missing,
                "LLVMDisposeTargetMachine",
                FnDisposeTargetMachine
            )
            .unwrap_or(unreachable_unit_ptr),
            create_target_data_layout: bind!(
                library,
                missing,
                "LLVMCreateTargetDataLayout",
                FnCreateTargetDataLayout
            )
            .unwrap_or(unreachable_create_target_data),
            copy_string_rep_of_target_data: bind!(
                library,
                missing,
                "LLVMCopyStringRepOfTargetData",
                FnCopyStringRepOfTargetData
            )
            .unwrap_or(unreachable_copy_string_rep),
            dispose_target_data: bind!(
                library,
                missing,
                "LLVMDisposeTargetData",
                FnDisposeTargetData
            )
            .unwrap_or(unreachable_unit_ptr),
            set_target: bind!(library, missing, "LLVMSetTarget", FnSetTarget)
                .unwrap_or(unreachable_set_target),
            set_module_data_layout: bind!(
                library,
                missing,
                "LLVMSetModuleDataLayout",
                FnSetModuleDataLayout
            )
            .unwrap_or(unreachable_set_data_layout),
            create_pass_builder_options: bind!(
                library,
                missing,
                "LLVMCreatePassBuilderOptions",
                FnCreatePassBuilderOptions
            )
            .unwrap_or(unreachable_create_pbo),
            dispose_pass_builder_options: bind!(
                library,
                missing,
                "LLVMDisposePassBuilderOptions",
                FnDisposePassBuilderOptions
            )
            .unwrap_or(unreachable_unit_ptr),
            run_passes: bind!(library, missing, "LLVMRunPasses", FnRunPasses)
                .unwrap_or(unreachable_run_passes),
            get_error_message: bind!(library, missing, "LLVMGetErrorMessage", FnGetErrorMessage)
                .unwrap_or(unreachable_get_error_message),
            dispose_error_message: bind!(
                library,
                missing,
                "LLVMDisposeErrorMessage",
                FnDisposeErrorMessage
            )
            .unwrap_or(unreachable_unit_char),
            emit_to_memory_buffer: bind!(
                library,
                missing,
                "LLVMTargetMachineEmitToMemoryBuffer",
                FnTargetMachineEmitToMemoryBuffer
            )
            .unwrap_or(unreachable_emit),
        };

        if !missing.is_empty() {
            return Err(format!(
                "missing required LLVM C API entry points: {}",
                missing.join(", ")
            ));
        }

        let mut version = (0u32, 0u32, 0u32);
        // SAFETY: `LLVMGetVersion` was resolved from the loaded module and
        // writes three `unsigned`s through the out-pointers.
        unsafe {
            (api.get_version)(&mut version.0, &mut version.1, &mut version.2);
        }
        if version.0 != REQUIRED_LLVM_MAJOR {
            return Err(format!(
                "reports LLVM {}.{}.{}, but this oscan build binds the LLVM {} C API exactly (the LLVM C API is only stable within a major release)",
                version.0, version.1, version.2, REQUIRED_LLVM_MAJOR
            ));
        }

        let capabilities = probe_capabilities(&library);
        if capabilities == ProviderCapabilities::default() {
            return Err(
                "exports no LLVM target initializers (X86/AArch64/RISCV), so it cannot emit object code"
                    .to_string(),
            );
        }
        initialize_targets(&library, capabilities);

        Ok(LlvmProvider {
            _library: library,
            api,
            path: path.to_path_buf(),
            version,
            capabilities,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn version_string(&self) -> String {
        format!("{}.{}.{}", self.version.0, self.version.1, self.version.2)
    }

    pub fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities
    }

    /// The exact target data layout string `triple` implies. Used so the
    /// emitted module's `target datalayout` always matches the
    /// `TargetMachine` that will consume it (a mismatch makes LLVM
    /// silently recompute struct offsets).
    pub fn data_layout_for(&self, triple: &str) -> Result<String, String> {
        let machine = TargetMachine::create(self, triple)?;
        machine.data_layout()
    }

    /// Parse and verify `ir_text` without emitting anything.
    ///
    /// Used by `--emit-llvm-ir`, so textual IR output is never silently
    /// something LLVM would reject: the IR the user sees is the IR the
    /// object path would have compiled.
    pub fn verify_ir(&self, ir_text: &str, triple: &str) -> Result<(), String> {
        let mut terminated = Vec::with_capacity(ir_text.len() + 1);
        terminated.extend_from_slice(ir_text.as_bytes());
        terminated.push(0);

        let context = Context::create(self)?;
        let module = context.parse_ir(&terminated, ir_text.len())?;
        let machine = TargetMachine::create(self, triple)?;
        let triple_c = cstring(triple)?;
        // SAFETY: live module handle and a valid NUL-terminated triple.
        unsafe {
            (self.api.set_target)(module.raw, triple_c.as_ptr());
        }
        let layout = machine.data_layout_handle()?;
        unsafe {
            (self.api.set_module_data_layout)(module.raw, layout.raw);
        }
        drop(layout);
        module.verify("emitted")?;
        drop(terminated);
        Ok(())
    }

    /// Parse, verify, optimize, and emit `ir_text` as a relocatable object
    /// for `triple`, entirely in this process.
    pub fn compile_ir_to_object(
        &self,
        ir_text: &str,
        triple: &str,
        optimize: OptimizationLevel,
    ) -> Result<Vec<u8>, String> {
        // LLVM's textual IR lexer reads a sentinel byte one past the end
        // of its buffer, so the buffer must be genuinely NUL-terminated
        // (`RequiresNullTerminator = 1`). This owns that terminated copy
        // for the whole parse.
        let mut terminated = Vec::with_capacity(ir_text.len() + 1);
        terminated.extend_from_slice(ir_text.as_bytes());
        terminated.push(0);

        let context = Context::create(self)?;
        let module = context.parse_ir(&terminated, ir_text.len())?;
        let machine = TargetMachine::create(self, triple)?;

        let triple_c = cstring(triple)?;
        // SAFETY: `module`/`machine` are live handles from this provider.
        unsafe {
            (self.api.set_target)(module.raw, triple_c.as_ptr());
        }
        let layout = machine.data_layout_handle()?;
        unsafe {
            (self.api.set_module_data_layout)(module.raw, layout.raw);
        }
        drop(layout);

        module.verify("emitted")?;
        module.run_passes(&machine, optimize)?;
        // Re-verify after optimization: a pass pipeline that produced
        // invalid IR must be reported here, not turned into a broken
        // object file.
        module.verify("optimized")?;
        let bytes = machine.emit_object(&module)?;
        drop(terminated);
        Ok(bytes)
    }
}

/// The optimization pipeline to run before object emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizationLevel {
    /// Optimize for minimum size. Emitted functions carry LLVM's
    /// `"no-builtins"` attribute, and the freestanding object audit rejects
    /// any libc symbol that a future LLVM pipeline might nevertheless form.
    FreestandingSafe,
}

impl OptimizationLevel {
    /// LLVM's standard minimum-size pipeline includes size-aware inlining
    /// and interprocedural cleanup that a local pass list cannot reproduce.
    /// Safety does not rely on the list staying frozen: `"no-builtins"`
    /// suppresses libc assumptions and the object is audited before linking.
    pub fn pipeline(self) -> &'static str {
        match self {
            OptimizationLevel::FreestandingSafe => "default<Oz>",
        }
    }
}

// -- RAII wrappers ---------------------------------------------------------

struct Context<'a> {
    provider: &'a LlvmProvider,
    raw: LlvmContextRef,
}

impl<'a> Context<'a> {
    fn create(provider: &'a LlvmProvider) -> Result<Self, String> {
        // SAFETY: resolved entry point, no arguments.
        let raw = unsafe { (provider.api.context_create)() };
        if raw.is_null() {
            return Err("LLVMContextCreate returned null".to_string());
        }
        Ok(Context { provider, raw })
    }

    /// Parse textual IR. `terminated` must hold `len` bytes of IR followed
    /// by a NUL, and must stay alive for the duration of the call.
    fn parse_ir(&self, terminated: &[u8], len: usize) -> Result<Module<'a>, String> {
        debug_assert_eq!(terminated.len(), len + 1);
        debug_assert_eq!(terminated[len], 0);
        let name = cstring("oscan_program")?;
        // `LLVMParseIRInContext` takes ownership of the `MemoryBuffer`
        // object, so it is *not* disposed here on the success path; the
        // byte range itself stays owned by the caller.
        //
        // SAFETY: `terminated` outlives the call and is NUL-terminated at
        // `len`, which is what `RequiresNullTerminator = 1` promises.
        let buffer = unsafe {
            (self.provider.api.create_memory_buffer)(
                terminated.as_ptr() as *const c_char,
                len,
                name.as_ptr(),
                1,
            )
        };
        if buffer.is_null() {
            return Err("LLVMCreateMemoryBufferWithMemoryRange returned null".to_string());
        }

        let mut module: LlvmModuleRef = std::ptr::null_mut();
        let mut message: *mut c_char = std::ptr::null_mut();
        // SAFETY: out-parameters are valid; the buffer is consumed.
        let failed =
            unsafe { (self.provider.api.parse_ir)(self.raw, buffer, &mut module, &mut message) };
        if failed != 0 || module.is_null() {
            let detail = take_message(self.provider, message);
            return Err(format!(
                "the emitted LLVM IR was rejected by LLVM {}: {detail}",
                self.provider.version_string()
            ));
        }
        // A diagnostic can be produced alongside a successful parse.
        let _ = take_message(self.provider, message);
        Ok(Module {
            provider: self.provider,
            raw: module,
        })
    }
}

impl Drop for Context<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` was created by `LLVMContextCreate` and every module
        // created inside it has already been disposed (module drop order is
        // enforced by `compile_ir_to_object`'s local scoping).
        unsafe { (self.provider.api.context_dispose)(self.raw) }
    }
}

struct Module<'a> {
    provider: &'a LlvmProvider,
    raw: LlvmModuleRef,
}

impl Module<'_> {
    fn verify(&self, stage: &str) -> Result<(), String> {
        let mut message: *mut c_char = std::ptr::null_mut();
        // SAFETY: live module handle, valid out-pointer.
        let broken = unsafe {
            (self.provider.api.verify_module)(self.raw, LLVM_RETURN_STATUS_ACTION, &mut message)
        };
        let detail = take_message(self.provider, message);
        if broken != 0 {
            return Err(format!(
                "the {stage} LLVM module failed verification: {detail}"
            ));
        }
        Ok(())
    }

    fn run_passes(
        &self,
        machine: &TargetMachine<'_>,
        level: OptimizationLevel,
    ) -> Result<(), String> {
        let pipeline = cstring(level.pipeline())?;
        // SAFETY: resolved entry point, no arguments.
        let options = unsafe { (self.provider.api.create_pass_builder_options)() };
        if options.is_null() {
            return Err("LLVMCreatePassBuilderOptions returned null".to_string());
        }
        // SAFETY: live module/machine/options handles.
        let error = unsafe {
            (self.provider.api.run_passes)(self.raw, pipeline.as_ptr(), machine.raw, options)
        };
        // SAFETY: `options` is still live and owned here.
        unsafe { (self.provider.api.dispose_pass_builder_options)(options) };
        if !error.is_null() {
            // SAFETY: a non-null `LLVMErrorRef` owns a message that
            // `LLVMGetErrorMessage` transfers to the caller.
            let raw = unsafe { (self.provider.api.get_error_message)(error) };
            let detail = if raw.is_null() {
                "unknown pass-pipeline failure".to_string()
            } else {
                // SAFETY: NUL-terminated string owned by us after the call.
                let text = unsafe { CStr::from_ptr(raw) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { (self.provider.api.dispose_error_message)(raw) };
                text
            };
            return Err(format!(
                "the LLVM optimization pipeline failed ('{}'): {detail}",
                level.pipeline()
            ));
        }
        Ok(())
    }
}

impl Drop for Module<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` is a live module this wrapper owns.
        unsafe { (self.provider.api.dispose_module)(self.raw) }
    }
}

struct TargetMachine<'a> {
    provider: &'a LlvmProvider,
    raw: LlvmTargetMachineRef,
}

/// CPU/features that must match the ABI of Oscan's packaged runtime archive.
///
/// LLVM does not infer `rv64gc` from a generic RISC-V triple. With empty
/// features it emits RV64I using the soft-float `lp64` ABI, while Oscan's
/// musl runtime and direct linker use RV64GC/`lp64d`. Enabling the complete
/// standard `G` set plus compressed instructions makes LLVM select the same
/// hard-float ABI and prevents compiler-rt libcalls for ordinary arithmetic.
fn target_machine_cpu_features(triple: &str) -> (&'static str, &'static str) {
    if triple.starts_with("riscv64-") {
        ("generic-rv64", "+m,+a,+f,+d,+c,+zicsr,+zifencei")
    } else {
        ("generic", "")
    }
}

impl<'a> TargetMachine<'a> {
    fn create(provider: &'a LlvmProvider, triple: &str) -> Result<Self, String> {
        let triple_c = cstring(triple)?;
        let mut target: LlvmTargetRef = std::ptr::null_mut();
        let mut message: *mut c_char = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated triple and out-pointers.
        let failed = unsafe {
            (provider.api.get_target_from_triple)(triple_c.as_ptr(), &mut target, &mut message)
        };
        if failed != 0 || target.is_null() {
            let detail = take_message(provider, message);
            return Err(format!(
                "the packaged LLVM code generator has no back end for target triple '{triple}' (available: {}): {detail}",
                provider.capabilities.describe()
            ));
        }
        let (cpu_name, feature_names) = target_machine_cpu_features(triple);
        let cpu = cstring(cpu_name)?;
        let features = cstring(feature_names)?;
        // PIC everywhere. On ELF this is what makes the object usable in
        // both a PIE and a non-PIE final link; on Windows COFF it is what
        // makes LLVM address globals RIP-relatively instead of with a
        // 32-bit absolute relocation the MinGW linker rejects
        // ("relocation truncated to fit: IMAGE_REL_AMD64_ADDR32").
        let reloc = LLVM_RELOC_PIC;
        // SAFETY: live target handle and valid NUL-terminated strings.
        let raw = unsafe {
            (provider.api.create_target_machine)(
                target,
                triple_c.as_ptr(),
                cpu.as_ptr(),
                features.as_ptr(),
                LLVM_CODEGEN_LEVEL_AGGRESSIVE,
                reloc,
                LLVM_CODEMODEL_DEFAULT,
            )
        };
        if raw.is_null() {
            return Err(format!(
                "LLVMCreateTargetMachine returned null for triple '{triple}'"
            ));
        }

        Ok(TargetMachine { provider, raw })
    }

    fn data_layout_handle(&self) -> Result<TargetData<'a>, String> {
        // SAFETY: live target machine handle.
        let raw = unsafe { (self.provider.api.create_target_data_layout)(self.raw) };
        if raw.is_null() {
            return Err("LLVMCreateTargetDataLayout returned null".to_string());
        }
        Ok(TargetData {
            provider: self.provider,
            raw,
        })
    }

    fn data_layout(&self) -> Result<String, String> {
        let data = self.data_layout_handle()?;
        // SAFETY: live target data handle; the returned string is owned by
        // the caller and freed with `LLVMDisposeMessage`.
        let raw = unsafe { (self.provider.api.copy_string_rep_of_target_data)(data.raw) };
        if raw.is_null() {
            return Err("LLVMCopyStringRepOfTargetData returned null".to_string());
        }
        // SAFETY: NUL-terminated, owned by us.
        let text = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.provider.api.dispose_message)(raw) };
        Ok(text)
    }

    fn emit_object(&self, module: &Module<'_>) -> Result<Vec<u8>, String> {
        let mut message: *mut c_char = std::ptr::null_mut();
        let mut buffer: LlvmMemoryBufferRef = std::ptr::null_mut();
        // SAFETY: live machine/module handles and valid out-pointers.
        let failed = unsafe {
            (self.provider.api.emit_to_memory_buffer)(
                self.raw,
                module.raw,
                LLVM_CODEGEN_OBJECT_FILE,
                &mut message,
                &mut buffer,
            )
        };
        if failed != 0 || buffer.is_null() {
            let detail = take_message(self.provider, message);
            return Err(format!("LLVM object emission failed: {detail}"));
        }
        let _ = take_message(self.provider, message);
        // SAFETY: `buffer` is a live memory buffer this scope owns.
        let start = unsafe { (self.provider.api.get_buffer_start)(buffer) };
        let size = unsafe { (self.provider.api.get_buffer_size)(buffer) };
        if start.is_null() || size == 0 {
            unsafe { (self.provider.api.dispose_memory_buffer)(buffer) };
            return Err("LLVM emitted an empty object buffer".to_string());
        }
        // SAFETY: `start`/`size` describe a valid, initialized region owned
        // by the memory buffer, which is still alive here.
        let bytes = unsafe { std::slice::from_raw_parts(start as *const u8, size) }.to_vec();
        unsafe { (self.provider.api.dispose_memory_buffer)(buffer) };
        Ok(bytes)
    }
}

impl Drop for TargetMachine<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` is a live target machine this wrapper owns.
        unsafe { (self.provider.api.dispose_target_machine)(self.raw) }
    }
}

struct TargetData<'a> {
    provider: &'a LlvmProvider,
    raw: LlvmTargetDataRef,
}

impl Drop for TargetData<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` is a live target data handle this wrapper owns.
        unsafe { (self.provider.api.dispose_target_data)(self.raw) }
    }
}

/// Consume an LLVM-allocated diagnostic string, freeing it.
fn take_message(provider: &LlvmProvider, message: *mut c_char) -> String {
    if message.is_null() {
        return "no diagnostic provided".to_string();
    }
    // SAFETY: LLVM out-parameter messages are NUL-terminated and owned by
    // the caller, who must free them with `LLVMDisposeMessage`.
    let text = unsafe { CStr::from_ptr(message) }
        .to_string_lossy()
        .into_owned();
    unsafe { (provider.api.dispose_message)(message) };
    text
}

fn cstring(value: &str) -> Result<CString, String> {
    CString::new(value).map_err(|_| format!("'{value}' contains an interior NUL byte"))
}

fn probe_capabilities(library: &sys::Library) -> ProviderCapabilities {
    // A target back end is only usable when *all four* of its
    // initializers are present; a library built with a partial target
    // (e.g. TargetInfo but no AsmPrinter) cannot emit object code.
    let has = |prefix: &str| {
        ["TargetInfo", "Target", "TargetMC", "AsmPrinter"]
            .iter()
            .all(|suffix| {
                library
                    .symbol(&format!("LLVMInitialize{prefix}{suffix}"))
                    .is_some()
            })
    };
    ProviderCapabilities {
        x86: has("X86"),
        aarch64: has("AArch64"),
        riscv: has("RISCV"),
    }
}

fn initialize_targets(library: &sys::Library, capabilities: ProviderCapabilities) {
    let init = |prefix: &str| {
        for suffix in ["TargetInfo", "Target", "TargetMC", "AsmPrinter"] {
            if let Some(address) = library.symbol(&format!("LLVMInitialize{prefix}{suffix}")) {
                // SAFETY: every `LLVMInitialize*` entry point is a
                // no-argument, no-result C function, and LLVM documents
                // repeat calls as idempotent.
                let f: FnInitializeTarget =
                    unsafe { std::mem::transmute::<*mut c_void, FnInitializeTarget>(address) };
                unsafe { f() };
            }
        }
    };
    if capabilities.x86 {
        init("X86");
    }
    if capabilities.aarch64 {
        init("AArch64");
    }
    if capabilities.riscv {
        init("RISCV");
    }
}

/// Every location the provider may be loaded from, in priority order.
pub fn search_candidates() -> Vec<PathBuf> {
    let exe = std::env::current_exe().ok();
    search_candidates_with(
        env_var_nonempty("OSCAN_LLVM_LIB"),
        env_var_nonempty("OSCAN_LLVM_DIR"),
        env_var_nonempty("OSCAN_TOOLCHAIN_DIR"),
        exe.as_deref(),
    )
}

/// Pure, testable core of [`search_candidates`].
///
/// Deliberately never includes a bare relative path or a `PATH`
/// directory: the code generator is executed code, so it must not be
/// loadable from whatever directory `oscan` happened to be launched in
/// (the same rule `crate::find_toolchain_dir` applies to the bundled
/// toolchain).
pub fn search_candidates_with(
    explicit_lib: Option<String>,
    explicit_dir: Option<String>,
    toolchain_dir: Option<String>,
    exe_path: Option<&Path>,
) -> Vec<PathBuf> {
    if let Some(lib) = explicit_lib {
        let path = PathBuf::from(lib);
        return path.is_absolute().then_some(path).into_iter().collect();
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(dir) = explicit_dir {
        roots.push(PathBuf::from(dir));
    }
    if let Some(dir) = toolchain_dir {
        roots.push(PathBuf::from(dir));
    }
    if let Some(exe_dir) = exe_path.and_then(|p| p.parent()) {
        roots.push(exe_dir.join("toolchain"));
        // A backend-specific package stages one copy of the code generator
        // under its native-link sidecar directory, where `ld.lld.exe` also
        // finds it as a sibling runtime dependency (Windows resolves a
        // loaded EXE's imports from its own directory first). Candidates
        // from here are only loaded after
        // `native_assets::sidecar::require_verified_if_inside` vouches for
        // them; see `LlvmProvider::load`.
        roots.push(exe_dir.join(crate::backend::native_assets::sidecar::SIDECAR_DIR_NAME));
        roots.push(exe_dir.to_path_buf());
    }

    let platform = if cfg!(windows) { "windows" } else { "linux" };
    let mut candidates = Vec::new();
    for root in roots {
        if !root.is_absolute() {
            // Refuse a relative root outright rather than silently
            // resolving it against the process CWD.
            continue;
        }
        for sub in [
            root.join(platform).join("bin"),
            root.join(platform).join("lib"),
            root.join("bin"),
            root.join("lib"),
            root.clone(),
        ] {
            for name in sys::library_file_names() {
                let candidate = sub.join(name);
                if !candidates.contains(&candidate) {
                    candidates.push(candidate);
                }
            }
        }
    }
    candidates
}

fn env_var_nonempty(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn no_provider_error() -> String {
    format!(
        "the LLVM backend needs Oscan's packaged LLVM {REQUIRED_LLVM_MAJOR} code generator \
         ({}), but no usable copy was found next to this executable; set OSCAN_LLVM_LIB to \
         its full path, set OSCAN_LLVM_DIR/OSCAN_TOOLCHAIN_DIR to the packaged toolchain, or \
         select --backend cranelift/--backend c",
        sys::library_file_names().join(" / ")
    )
}

// -- `unreachable` stubs ---------------------------------------------------
//
// `bind!` reports every missing entry point at once (a much better
// diagnostic than failing on the first one), which means the `Api` struct
// still has to be constructed even when symbols are absent. These stubs
// fill those slots and can never be called: `load_from` returns an error
// before the provider is handed to anyone.

fn missing_symbol() -> ! {
    unreachable!("internal error: a missing LLVM C API entry point was called")
}
unsafe extern "C" fn unreachable_get_version(_: *mut c_uint, _: *mut c_uint, _: *mut c_uint) {
    missing_symbol()
}
unsafe extern "C" fn unreachable_context_create() -> LlvmContextRef {
    missing_symbol()
}
unsafe extern "C" fn unreachable_unit_ptr<T>(_: *mut T) {
    missing_symbol()
}
unsafe extern "C" fn unreachable_unit_char(_: *mut c_char) {
    missing_symbol()
}
unsafe extern "C" fn unreachable_create_buffer(
    _: *const c_char,
    _: usize,
    _: *const c_char,
    _: c_int,
) -> LlvmMemoryBufferRef {
    missing_symbol()
}
unsafe extern "C" fn unreachable_buffer_start(_: LlvmMemoryBufferRef) -> *const c_char {
    missing_symbol()
}
unsafe extern "C" fn unreachable_buffer_size(_: LlvmMemoryBufferRef) -> usize {
    missing_symbol()
}
unsafe extern "C" fn unreachable_parse_ir(
    _: LlvmContextRef,
    _: LlvmMemoryBufferRef,
    _: *mut LlvmModuleRef,
    _: *mut *mut c_char,
) -> c_int {
    missing_symbol()
}
unsafe extern "C" fn unreachable_verify(_: LlvmModuleRef, _: c_int, _: *mut *mut c_char) -> c_int {
    missing_symbol()
}
unsafe extern "C" fn unreachable_target_from_triple(
    _: *const c_char,
    _: *mut LlvmTargetRef,
    _: *mut *mut c_char,
) -> c_int {
    missing_symbol()
}
unsafe extern "C" fn unreachable_create_target_machine(
    _: LlvmTargetRef,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
    _: c_int,
    _: c_int,
    _: c_int,
) -> LlvmTargetMachineRef {
    missing_symbol()
}
unsafe extern "C" fn unreachable_create_target_data(_: LlvmTargetMachineRef) -> LlvmTargetDataRef {
    missing_symbol()
}
unsafe extern "C" fn unreachable_copy_string_rep(_: LlvmTargetDataRef) -> *mut c_char {
    missing_symbol()
}
unsafe extern "C" fn unreachable_set_target(_: LlvmModuleRef, _: *const c_char) {
    missing_symbol()
}
unsafe extern "C" fn unreachable_set_data_layout(_: LlvmModuleRef, _: LlvmTargetDataRef) {
    missing_symbol()
}
unsafe extern "C" fn unreachable_create_pbo() -> LlvmPassBuilderOptionsRef {
    missing_symbol()
}
unsafe extern "C" fn unreachable_run_passes(
    _: LlvmModuleRef,
    _: *const c_char,
    _: LlvmTargetMachineRef,
    _: LlvmPassBuilderOptionsRef,
) -> LlvmErrorRef {
    missing_symbol()
}
unsafe extern "C" fn unreachable_get_error_message(_: LlvmErrorRef) -> *mut c_char {
    missing_symbol()
}
unsafe extern "C" fn unreachable_emit(
    _: LlvmTargetMachineRef,
    _: LlvmModuleRef,
    _: c_int,
    _: *mut *mut c_char,
    _: *mut LlvmMemoryBufferRef,
) -> c_int {
    missing_symbol()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_library_path_short_circuits_every_other_candidate() {
        let explicit = if cfg!(windows) {
            PathBuf::from(r"C:\llvm\bin\libLLVM-22.dll")
        } else {
            PathBuf::from("/opt/llvm/libLLVM.so")
        };
        let candidates = search_candidates_with(
            Some(explicit.to_string_lossy().into_owned()),
            Some("/ignored".to_string()),
            Some("/also-ignored".to_string()),
            Some(Path::new("/install/oscan")),
        );
        assert_eq!(candidates, vec![explicit]);
    }

    #[test]
    fn a_relative_explicit_library_path_is_refused() {
        let candidates = search_candidates_with(
            Some("libLLVM-22.dll".to_string()),
            None,
            None,
            None::<&Path>,
        );
        assert!(candidates.is_empty(), "{candidates:?}");

        let error = match LlvmProvider::load_from(Path::new("libLLVM-22.dll")) {
            Ok(_) => panic!("relative provider paths must be rejected"),
            Err(error) => error,
        };
        assert!(error.contains("must be absolute"), "{error}");
    }

    #[test]
    fn riscv_target_machine_matches_the_packaged_rv64gc_hard_float_runtime() {
        assert_eq!(
            target_machine_cpu_features("riscv64-unknown-linux-gnu"),
            ("generic-rv64", "+m,+a,+f,+d,+c,+zicsr,+zifencei")
        );
        assert_eq!(
            target_machine_cpu_features("x86_64-unknown-linux-gnu"),
            ("generic", "")
        );
    }

    #[test]
    fn candidates_are_executable_relative_and_never_cwd_relative() {
        let exe = if cfg!(windows) {
            PathBuf::from(r"C:\install\oscan.exe")
        } else {
            PathBuf::from("/install/oscan")
        };
        let candidates = search_candidates_with(None, None, None, Some(&exe));
        assert!(!candidates.is_empty());
        let exe_dir = exe.parent().expect("exe has a parent");
        assert!(
            candidates.iter().all(|c| c.starts_with(exe_dir)),
            "{candidates:?}"
        );
        assert!(candidates.iter().all(|c| c.is_absolute()), "{candidates:?}");
        assert!(candidates
            .iter()
            .any(|c| c.starts_with(exe_dir.join("toolchain"))));
    }

    #[test]
    fn relative_roots_are_refused_outright() {
        let candidates =
            search_candidates_with(None, Some("relative-dir".to_string()), None, None::<&Path>);
        assert!(candidates.is_empty(), "{candidates:?}");
    }

    #[test]
    fn capability_gate_answers_per_architecture() {
        let caps = ProviderCapabilities {
            x86: true,
            aarch64: true,
            riscv: false,
        };
        assert!(caps.supports(TargetArch::X86_64));
        assert!(caps.supports(TargetArch::Aarch64));
        assert!(!caps.supports(TargetArch::Riscv64));
        assert_eq!(caps.describe(), "x86-64, aarch64");
        assert_eq!(ProviderCapabilities::default().describe(), "none");
    }

    #[test]
    fn the_optimization_pipeline_uses_llvm_minimum_size_defaults() {
        let pipeline = OptimizationLevel::FreestandingSafe.pipeline();
        assert_eq!(pipeline, "default<Oz>");
    }

    #[test]
    fn the_missing_provider_error_names_the_recovery_options() {
        let message = no_provider_error();
        assert!(message.contains("OSCAN_LLVM_LIB"));
        assert!(message.contains("--backend cranelift"));
        assert!(message.contains("--backend c"));
        assert!(message.contains(&REQUIRED_LLVM_MAJOR.to_string()));
    }
}
