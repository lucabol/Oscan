# Direct LLVM Backend

**Status:** Implemented and preferred on supported packaged targets.

## 1. Purpose

Oscan's LLVM backend is a real, C-independent code generator:

```text
typed ir::Program
  -> shared Oscan LIR lowering
  -> deterministic typed LLVM IR
  -> packaged libLLVM loaded in-process
     (parse -> verify -> default<Oz> -> verify -> TargetMachine object)
  -> relocatable COFF/ELF object
  -> shared Oscan runtime/archive/link pipeline
  -> executable
```

Normal freestanding `--backend llvm` compilation does **not** generate C and
does not invoke Clang, GCC, `cc`, `cl`, `llvm-as`, `opt`, or `llc`. Users do not
need an installed C or LLVM toolchain. Release bundles carry the exact LLVM
shared library, precompiled runtime archives, and direct linker assets.

The other backends remain supported:

- `--backend native` / `--backend cranelift`: direct Cranelift object emission.
- `--backend c`: portable C99/source-emission, macOS, WASI, and the
  differential reference implementation.

An explicit backend is strict. In particular, an explicit LLVM failure is
reported and is never retried through Cranelift or C.

## 2. Shared semantic lowering

LLVM and Cranelift consume the same backend-neutral low-level interface:

- `src/backend/func.rs` contains the single semantic lowering.
- `src/backend/lir.rs` defines `LirModule`/`LirBuilder`.
- `src/backend/lir_cranelift.rs` implements that interface with Cranelift.
- `src/backend/llvm/emit.rs` implements it as deterministic textual LLVM IR.

This keeps control flow, aggregate copies, checked arithmetic, match/defer
behavior, arena rules, runtime calls, and builtin mappings in one place. LLVM
is therefore independent of the C backend without duplicating Oscan semantics
between the two object backends.

The LLVM emitter uses explicit types and conservative semantics. It does not
invent `nsw`, `nuw`, `exact`, `inbounds`, fast-math, or stronger lifetime
promises. Oscan addresses stay as `i64` values and convert to LLVM `ptr` only at
ABI and dereference boundaries.

## 3. In-process LLVM provider

`src/backend/llvm/provider.rs` is a small dynamically loaded binding to the LLVM
C API. A plain `cargo build` has no LLVM headers, SDK, `llvm-config`, Inkwell,
or `llvm-sys` dependency.

For object emission, Oscan:

1. obtains the target data layout from the actual `TargetMachine`;
2. parses emitted IR with `LLVMParseIRInContext`;
3. verifies it with `LLVMVerifyModule`;
4. runs LLVM's `default<Oz>` pipeline with `LLVMRunPasses`;
5. verifies the optimized module again;
6. obtains object bytes from `LLVMTargetMachineEmitToMemoryBuffer`.

No code-generation temporary files or subprocesses are used. `--emit-llvm-ir`
still parses and verifies the IR before exposing it.

### Provider discovery and security

LLVM major version 22 is required exactly. Search order is:

1. `OSCAN_LLVM_LIB` — an absolute shared-library path;
2. `OSCAN_LLVM_DIR` — an absolute packaged-provider directory;
3. `OSCAN_TOOLCHAIN_DIR` — an absolute packaged-toolchain root;
4. executable-relative `toolchain/` and the executable directory.

Relative overrides, the current working directory, `PATH`, and the platform's
bare library-loader search path are not used. The provider is executable code,
so loading an arbitrary `libLLVM` found near user input would be unsafe.

Target support is probed from exported target initializers rather than assumed.
The current packaged providers are:

| Host bundle | LLVM | Provider targets |
|---|---:|---|
| Windows x86-64 | 22.1.2 | x86-64, AArch64 |
| Linux x86-64 | 22.1.8 | x86-64, AArch64, RISC-V64 |

The Windows provider intentionally rejects RISC-V because that library does not
export the RISC-V initializers.

RISC-V TargetMachines explicitly use `generic-rv64` with
`+m,+a,+f,+d,+c,+zicsr,+zifencei`. A generic triple alone means RV64I/`lp64`
to LLVM, which is ABI-incompatible with Oscan's packaged RV64GC/`lp64d` musl
runtime. CI checks the ELF double-float/RVC flags and QEMU-runs arithmetic.

The Linux provider is redistributed from apt.llvm.org's Ubuntu 22.04 build. It
requires glibc 2.34 or newer plus `libedit2`, `libffi8`, `libxml2`, `libz3-4`,
`libzstd1`, and `zlib1g` host runtime packages. These are shared-library
dependencies, not a C/Clang/LLVM toolchain. Release smoke installs the
manifest-declared packages and must load the staged provider successfully.

## 4. Object validation and freestanding safety

Every emitted object is checked with the Rust `object` crate:

- it must be non-empty and relocatable;
- Windows targets must be COFF and Linux targets ELF;
- the architecture must match the requested target.

Generated functions carry `minsize`, `optsize`, `nounwind`, and LLVM's
`"no-builtins"` marker. The latter is the IR-level equivalent of a
freestanding/no-builtin contract. After optimization, freestanding objects are
audited for unresolved `memcpy`, `memmove`, `memset`, `memcmp`, `bcmp`, and
`strlen`, so a future LLVM pass cannot silently add an unavailable libc
dependency.

## 5. Runtime and final linking

LLVM and Cranelift objects use the same `src/backend/link/` implementation:

- precompiled hosted or freestanding runtime archives;
- object-driven runtime-profile and Windows import-library selection;
- embedded Windows `ld.lld` or Linux GNU `ld`;
- target-matched cross-linker/runtime sidecars;
- extra object/library inputs and elevation policy.

Freestanding runtime selection has three profiles:

| Profile | Contents | Selected when |
|---|---|---|
| `freestanding_core` | core, sockets, TLS | no graphics-adjacent undefined symbol |
| `freestanding_gfx` | core plus graphics/canvas/built-in fonts | `osc_gfx_*`, `osc_canvas_*`, or `osc_clipboard_*`, but no advanced decoder |
| `freestanding` | full graphics, image, SVG, and TrueType | `osc_img_*`, `osc_svg_*`, or `osc_tt_*`, or when inputs cannot be safely scanned |

These are release-time C artifacts, like any precompiled runtime library. Their
implementation language does not create an end-user C-toolchain dependency.

## 6. CLI and default policy

Backend resolution is:

1. explicit `--backend llvm|native|c`;
2. C source or C-only cross-target requests select C;
3. LLVM IR requests select LLVM;
4. explicit `--native-target` without `--backend` selects Cranelift for
   compatibility;
5. LLVM when the packaged provider loads and supports the host;
6. Cranelift on a supported object host;
7. C otherwise.

Both Windows and Linux full bundles package LLVM and therefore default to LLVM.
Source-built compilers default to LLVM only when an executable-relative or
explicit provider is available. Capability fallback applies only during
implicit selection; an explicit LLVM request never falls back.

## 7. Exact no-toolchain boundary

| Operation | End-user C/Clang toolchain required? |
|---|---|
| Packaged freestanding LLVM executable/object/`--run` | **No** |
| `--emit-llvm-ir` with a packaged provider | **No** |
| Packaged freestanding Cranelift | **No** |
| C backend | Yes |
| Hosted `--libc` final link | Yes; a driver supplies CRT/libc |
| `--extra-c` / `--extra-cflags` | Yes |
| User `extern` signatures containing `str` | May require the generated C ABI shim |
| Missing runtime archive in a development build | Auto-building it requires C |

`OSCAN_NO_TOOLCHAIN=1` turns every C-toolchain escape hatch into a hard error:
C selection, C inputs, runtime/shim auto-build, and compiler-driver linking are
refused. This is the release/CI proof mode for the first three rows.

## 8. Tradeoffs

| Backend | Strengths | Costs |
|---|---|---|
| LLVM (default) | Best optimizer and code density; direct typed lowering; broad target machinery; no installed toolchain in packaged freestanding builds | `libLLVM` materially increases release size; optimization is slower than Cranelift; exact-major provider packaging |
| Cranelift | Fast compilation; no provider shared library; independent differential implementation | Generally larger/slower generated programs; narrower optimization |
| C | Maximum portability; inspectable C; mature correctness oracle | Requires a C toolchain to build executables; not self-contained for normal compilation |

Shipping all three is intentional. LLVM gives the best default output,
Cranelift provides a fast independent object backend and fallback, and C keeps
source portability and a high-value differential oracle. Removing either
alternative would reduce failure isolation and target coverage more than it
would simplify the compiler.

## 9. Size and validation contract

The pinned Windows release build currently produces:

- `hello.osc`: LLVM 6,144 bytes; C 8,704 bytes;
- all 37 recursive examples: LLVM 814,080 bytes; C 875,520 bytes
  (LLVM is 61,440 bytes / 7.02% smaller in aggregate).

The aggregate result uses `scripts/sample-backend-matrix.ps1`. Size equality is
not expected per program because LLVM/Cranelift call a separately compiled
runtime ABI while C compiles generated program and runtime together.

Required gates include:

- Rust and release-tooling unit tests;
- C-vs-LLVM and C-vs-Cranelift differential execution;
- hosted, FFI, negative, panic, graphics, and cross-target coverage;
- provider version/capability, IR verification, object-format, and forbidden
  symbol checks;
- empty-`PATH`, unusable-`OSCAN_CC`, `OSCAN_NO_TOOLCHAIN=1` release smoke;
- recursive three-backend example compilation and size reporting;
- packaged release smoke on Windows and Linux.
