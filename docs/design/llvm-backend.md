# LLVM Backend

**Status:** Implemented

## 1. Purpose

LLVM is Oscan's preferred backend for ordinary executable, object, and
`--run` requests on supported Windows/Linux hosts when a compatible Clang is
available. The C and Cranelift backends remain supported options:

- `--backend llvm`: optimized object code through Clang/LLVM.
- `--backend native` (or `--backend cranelift`): direct Cranelift lowering.
- `--backend c`: portable/reference C output and C-based cross targets.

An explicit backend selection is strict. In particular, an explicit LLVM
failure is reported to the user and never retried through Cranelift or C.

## 2. Architecture

The production pipeline is:

```text
parsed and type-checked ir::Program
    |
    v
CodeGenerator::generate_object_source
    |  deterministic C99, declarations only, no embedded runtime body
    v
clang -S -emit-llvm -Oz
    |
    v
textual LLVM IR (program.ll)
    |
    v
clang -x ir -c -Oz
    |
    v
relocatable COFF/ELF object
    |
    v
backend::link + existing runtime archive
    |
    v
freestanding or hosted executable
```

The LLVM module owns Clang discovery, LLVM IR/object emission, subprocess
diagnostics, and object validation. It does not own final linking. LLVM and
Cranelift both use the existing `backend::link` pipeline, including runtime
archive selection, object capability scanning, embedded linkers, external
objects/libraries, and the elevated-process policy.

### External-runtime C profile

The ordinary C backend remains a standalone single-translation-unit emitter:
it embeds the selected runtime implementation exactly as before.

The LLVM object profile emits only the program and public runtime declarations:

- freestanding output defines `OSC_FREESTANDING`;
- the generated entry calls the public `osc_freestanding_env_init`;
- hosted output includes the public runtime header;
- neither mode embeds runtime definitions.

The separately compiled runtime archive therefore remains the single runtime
definition in an LLVM executable.

For an ordinary packaged freestanding build, Clang is the LLVM provider rather
than a final-link driver. `OSCAN_CC` is not consulted: after LLVM object
emission, Oscan invokes its embedded target linker directly. Hosted mode,
explicit `--extra-c` inputs, and development builds without embedded link
assets retain the documented compiler-driver requirement.

## 3. Why this design

| Approach | Advantages | Costs | Decision |
|---|---|---|---|
| `llvm-sys` or Inkwell | Typed, in-process LLVM API | LLVM development libraries become a build prerequisite; LLVM ABI/version coupling; unsafe FFI; larger compiler | Rejected |
| Direct textual LLVM emitter | Independent semantic lowering; small runtime tool surface | Duplicates the complete Cranelift lowering and builtin/ABI logic; highest initial miscompile risk | Future direction |
| Existing C lowering plus Clang | Immediate full-language parity; inspectable IR; no LLVM Cargo dependency; reuses pinned Windows Clang | Not semantically independent from C; requires a Clang executable | Implemented |
| Compile generated C directly | Smallest implementation | No stable LLVM IR surface and an ambiguous backend boundary | Rejected |
| Unconditional LLVM default | Simple policy | Breaks source builds and installations without Clang | Rejected |
| Capability-gated LLVM default | Release/toolchain-aware; contributors retain a working compiler | Implicit backend can differ by installed capability | Implemented and exposed by `--verbose` |

The important limitation is intentional: LLVM currently shares the C
backend's source-level lowering and ABI choices. It is an LLVM optimization and
machine-code backend, but not yet an independent implementation of Oscan
semantics. Documentation and bug reports must preserve that distinction.

## 4. CLI and selection policy

LLVM-specific output:

```text
--emit-llvm
--emit-llvm-ir
-o file.ll
```

`--emit-llvm` is an alias. LLVM IR output stops before object emission and
linking.

Backend resolution, in order:

1. An explicit `--backend llvm|native|c`.
2. C source output (`--emit-c` or `-o *.c`) and C-only `--target` requests.
3. LLVM IR output (`--emit-llvm-ir` or `-o *.ll`).
4. An explicit `--native-target` without a backend selects Cranelift for
   compatibility with the pre-LLVM CLI.
5. LLVM when the host object target is supported and Clang can be resolved.
6. Cranelift when the host object target is supported.
7. C otherwise.

`--native-target` is shared by explicit LLVM and Cranelift requests. The legacy
`--target riscv64|wasi` flag remains C-only.

## 5. Clang discovery

Discovery is deterministic and probes each candidate with `clang --version`:

1. `OSCAN_LLVM_CLANG`
2. `OSCAN_LLVM_TOOLCHAIN_DIR`
3. `OSCAN_TOOLCHAIN_DIR` or the existing discovered bundled `toolchain/`
4. `clang-22` and `clang` on `PATH`
5. Visual Studio Clang on Windows

An executable is accepted only when its version output identifies Clang. The
backend never searches for a bare executable in the current working directory;
a configured/discovered `toolchain/` root is a deliberate directory contract,
not a PATH-style current-directory lookup.

The Windows full release retains the pinned llvm-mingw Clang and therefore
selects LLVM by default. The current Linux full release contains the pinned
musl GCC/binutils toolchain rather than Clang, so it selects LLVM only when a
host or override Clang is available and otherwise selects Cranelift.

## 6. Targets and object validation

| Oscan target | Clang triple | Required object |
|---|---|---|
| `windows-x86_64` | `x86_64-w64-windows-gnu` | COFF x86-64 |
| `linux-x86_64` | `x86_64-unknown-linux-gnu` | ELF x86-64 |
| `linux-aarch64` | `aarch64-unknown-linux-gnu` | ELF AArch64 |
| `linux-riscv64` | `riscv64-unknown-linux-gnu` | ELF RISC-V64 |

The GNU Linux triples describe object ABI/code generation; freestanding final
linking still uses Oscan's target-matched runtime archives and direct linker.

After Clang reports success, Oscan parses the output with the `object` crate and
requires:

- non-empty bytes;
- `ObjectKind::Relocatable`;
- COFF for Windows or ELF for Linux;
- the architecture matching the requested target.

Target availability remains a Clang capability. The pinned Windows
llvm-mingw Clang emits Windows x86-64 and Linux AArch64 objects but does not
register RISC-V; an explicit RISC-V LLVM request therefore reports Clang's
target error. Cranelift and C target support is unaffected.

## 7. Emission flags and determinism

The C-to-IR stage uses C99, `-Oz`, function/data sections, no stack protector,
no asynchronous unwind tables, and an explicit target triple. Freestanding
mode also uses `-ffreestanding -fno-builtin`.

The scratch source is invoked as the relative name `program.c` with
`-fdebug-compilation-dir=.` and `-fno-ident`. This prevents random temporary
directory names and compiler identity records from making otherwise identical
`.ll` output differ between invocations.

Scratch directories come from `tempfile`; Unix permissions are set to `0700`.
Commands are executed directly without a shell. A failed process includes the
stage, exit status, executable path, discovery source, version, stderr, and
stdout in the compile error.

## 8. Linking and runtime modes

LLVM objects use exactly the same final-link inputs as Cranelift:

- freestanding or hosted runtime archive selected from the program request;
- freestanding profile selection based on object/runtime capabilities;
- extra C, object, and library inputs;
- embedded Windows `ld.lld` or Linux GNU `ld` where packaged;
- compiler-driver paths for hosted builds and cases that require C inputs;
- target-matched cross-linker/runtime sidecars.

This separation keeps LLVM out of native-link asset extraction and release
archive logic. It also means fixes to link planning, runtime manifests, or
security policy apply to both object backends.

## 9. Validation contract

Required gates:

- Rust tests with and without a configured Clang;
- deterministic and parseable textual LLVM IR;
- relocatable object format/architecture checks;
- C-vs-LLVM differential execution for positive, negative, hosted, and example
  corpora;
- focused panic, extern-string ABI, hosted runtime, and object-only tests;
- Windows and WSL x86-64 cross-link execution;
- explicit C and Cranelift regression suites, including the AArch64 Cranelift
  object link/execution gate under QEMU (LLVM AArch64 emission is a Clang
  capability and is not gated under QEMU today);
- packaged implicit-default smoke with host compiler names blocked on `PATH`;
- LLVM toolchain isolation with an empty `PATH`, an unusable `OSCAN_CC`, an
  absolute LLVM provider, and an embedded direct linker;
- pinned release-toolchain size comparison requiring LLVM output to be no
  larger than equivalent C-backend output.

## 10. Future direct emitter

A future `ir::Program -> LLVM IR` implementation should replace only the first
stage inside `backend::llvm`. It must preserve:

- the CLI and strict fallback rules;
- Clang/toolchain discovery (or provide a compatible replacement);
- target and object validation;
- external runtime ABI;
- shared object/link orchestration;
- all differential and packaging gates.

That migration should be staged behind explicit tests until every aggregate,
match, defer, arena, FFI, builtin, and panic behavior matches the current
production path.
