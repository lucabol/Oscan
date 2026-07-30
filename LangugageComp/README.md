# LangugageComp

An isolated, executable comparison of the same medium-complexity BuildGraph CLI
implemented in Oscan, Rust, TypeScript, C#, and Common Lisp.

The directory name intentionally preserves the requested `LangugageComp`
spelling. Source, dependencies, compiler targets, executables, temporary test
files, and benchmark inputs remain below this directory.

## Contents

| Path | Purpose |
|---|---|
| `SPEC.md` | Normative language-neutral behavior |
| `REFERENCE_RESULTS.md` | Correctness, source/artifact size, and timing baseline |
| `EXPERIMENT_REPORT.md` | Corrected Native AOT experiment analysis and conclusions |
| `oscan/main.osc` | Oscan implementation |
| `rust/src/main.rs` | Dependency-free Rust implementation |
| `typescript/src/main.ts` | TypeScript implementation; TypeScript is a build-only dev dependency |
| `csharp/Program.cs` | Dependency-free C# implementation published with Native AOT |
| `common-lisp/main.lisp` | Dependency-free Common Lisp implementation saved as an SBCL executable image |
| `fixtures/sample.bg` | Example graph |
| `harness/suite.py` | Isolated builder, shared executable test oracle, and benchmark |
| `prompts/one-shot.txt` | Self-contained prompt template for future one-shot model runs |

Generated files are ignored under `.build`, `typescript/node_modules`, and
language-local build directories.

## Prerequisites

- Python 3.12+
- Git
- Rust/Cargo
- Node.js and npm
- .NET 9 SDK and the platform's Native AOT toolchain (Visual Studio C++ workload on Windows)
- SBCL 2.6+
- GCC for Oscan's C backend

`global.json` pins the C# publish to the .NET 9.0.316 SDK used for the recorded
experiment.

The harness exports the pinned repository revision and clones the exact
`deps/laststanding` gitlink into `LangugageComp/.build`, then builds the Oscan
compiler there. It does not initialize the repository's top-level submodule or
write build artifacts outside `LangugageComp`.

## Build and test

From this directory:

```powershell
.\build.ps1
.\test.ps1
```

Or directly:

```powershell
python .\harness\suite.py build
python .\harness\suite.py test
```

Select one implementation with
`--language oscan|rust|typescript|csharp|common-lisp`.
`test` runs the same executable-level cases against every selected language,
including CLI behavior, malformed records, deterministic validation, cycle
detection, stable ordering, critical-path ties, affected-task closure, CRLF,
64-bit totals, and deep/wide stress graphs.

Example after building on Windows:

```powershell
.\.build\oscan\buildgraph.exe analyze .\fixtures\sample.bg
.\.build\rust-target\release\buildgraph.exe affected .\fixtures\sample.bg parse
node .\.build\typescript\main.js analyze .\fixtures\sample.bg
.\.build\csharp\BuildGraph.exe analyze .\fixtures\sample.bg
.\.build\common-lisp\buildgraph.exe analyze .\fixtures\sample.bg
```

On Linux/macOS, use `/` path separators and omit `.exe`:

```sh
./.build/oscan/buildgraph analyze ./fixtures/sample.bg
./.build/rust-target/release/buildgraph affected ./fixtures/sample.bg parse
node ./.build/typescript/main.js analyze ./fixtures/sample.bg
./.build/csharp/BuildGraph analyze ./fixtures/sample.bg
./.build/common-lisp/buildgraph analyze ./fixtures/sample.bg
```

## Reference benchmark

```powershell
.\benchmark.ps1 -Tasks 1000 -Iterations 50
```

The runner reports median cold-process time, median `--help` startup time, their
difference as an approximate workload cost, source bytes, and primary artifact
bytes. The subtraction is diagnostic rather than an in-process algorithm
benchmark: process scheduling, file I/O, and runtime startup still introduce
noise. Node runtime requirements are not folded into the TypeScript artifact
byte column. C# is published as a self-contained Native AOT executable, so its
executable includes the required trimmed .NET runtime code. Common Lisp is an
SBCL executable image and includes the Lisp runtime. The official Windows SBCL
2.6.7 build lacks zstd support, so the recorded image is uncompressed.

These measurements characterize the reviewed reference implementations and
their toolchains. They do **not** measure one-shot LLM success. Use
`prompts/one-shot.txt` in fresh, tool-free model sessions and score every raw
sample with this unchanged executable oracle to conduct that experiment.

## Fair-comparison boundaries

- No implementation uses an application runtime package.
- Every implementation follows the same validation order and exact output.
- The fixed manifests are scaffolding; a one-shot trial should replace only the
  single language source file.
- All failed, malformed, truncated, and noncompiling generations must remain in
  a model's pass@1 denominator.
- Runtime comparisons should include only correct programs and should be
  reported separately from generation correctness.
