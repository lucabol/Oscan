# Corrected BuildGraph five-language experiment

**Research date:** 2026-07-30  
**Repository base revision:** [`35fc3ab`](https://github.com/lucabol/Oscan/tree/35fc3ab); measurements include the collection-API changes described below<br>
**Compared implementations:** Oscan, Rust, TypeScript/Node.js, C# .NET 9 Native AOT, and Common Lisp on SBCL 2.6.7
**Correction:** all earlier framework-dependent C# size and timing results are invalidated

## Executive conclusion

The corrected experiment establishes three useful facts and one important
non-result:

1. **The five reviewed reference implementations are behaviorally equivalent
   under the current oracle.** Each passed all 47 executable cases, for 235/235
   language-case checks.
2. **Oscan has the strongest self-contained native artifact-size result.** Its
   28,672-byte executable is 6.75 times smaller than Rust's 193,536-byte
   executable and 53.93 times smaller than C# Native AOT's 1,546,240-byte
   executable. It is 1,378.43 times smaller than Common Lisp's 39,522,400-byte
   SBCL image. TypeScript's 9,958-byte JavaScript file is smaller as an
   application payload, but that number excludes Node.js and is therefore not a
   self-contained deployment comparison.
3. **Oscan had the lowest latest recorded cold-process median.** The 1,000-task
   analysis medians were 21.239 ms for Oscan, 37.820 ms for Rust, 35.016 ms for
   C# Native AOT, 73.429 ms for Common Lisp, and 87.384 ms for TypeScript. These
   process-level results differ materially across runs and should not be read
   as isolated algorithm speed.
4. **The experiment does not measure whether an LLM writes Oscan more
   successfully.** These programs were produced and repaired interactively
   using repository access, compiler feedback, tests, and multiple edits. No
   repeated, independent, tool-free one-shot generations were collected, so
   all-tests pass@1 is unobserved.

The defensible conclusion is narrow: **for this reviewed BuildGraph
implementation and toolchain, Oscan combines the smallest self-contained native
artifact with a low cold-process median, but it requires the largest source
file. The new collection API reduced that file by 56 lines and 993 bytes without
changing the algorithm. Common Lisp offers source length close to TypeScript and
beat Node's latest recorded cold-process median, at the cost of a 39.5 MB runtime
image. There is not yet evidence that Oscan is superior to Rust, TypeScript, C#,
or Common Lisp when programmed by an LLM.**

## 1. Why the C# experiment had to be redone

The previous C# build produced `BuildGraph.dll` and launched it through
`dotnet`. That is a framework-dependent deployment. Its 17,920-byte DLL omitted
the required .NET runtime, and its 92.715 ms process median included startup of
the managed `dotnet` host. Comparing those values directly with native Oscan
and Rust executables mixed deployment models.

The correction makes C# explicitly Native AOT:

- [`csharp/BuildGraph.csproj`](csharp/BuildGraph.csproj#L1-L13) targets `net9.0`
  and sets both `PublishAot` and `SelfContained` to `true`.
- [`global.json`](global.json) pins SDK 9.0.316 for this recorded experiment.
- [`harness/suite.py`](harness/suite.py#L280-L302) invokes
  `dotnet publish -c Release -r win-x64 --self-contained true`.
- [`harness/suite.py`](harness/suite.py#L326-L334) runs `BuildGraph.exe`
  directly, with no `dotnet` host in the measured command.
- The harness deletes the old C# publish directory before each publish, so a
  stale DLL cannot satisfy the artifact check
  ([`harness/suite.py`](harness/suite.py#L280-L323)).

This matches Microsoft's Native AOT contract: Native AOT publishing creates a
self-contained, runtime-specific native application; it does not use a JIT at
runtime; `<PublishAot>true</PublishAot>` enables it; and publishing requires a
specific runtime identifier. Windows publishing also requires the Visual
Studio C++ workload
([Microsoft, Native AOT deployment overview](https://learn.microsoft.com/en-us/dotnet/core/deploying/native-aot/)).

Additional checks on the corrected output found:

- `BuildGraph.exe` is a 64-bit PE32+ executable (`machine 0x8664`);
- its PE CLR Runtime Header directory has RVA 0 and size 0;
- it exits successfully when invoked directly with `DOTNET_ROOT` removed and
  `PATH` reduced to the Windows system directory;
- the resolved Native AOT compiler package is
  `Microsoft.DotNet.ILCompiler/9.0.18`.

The optional `BuildGraph.pdb` is not required to run the application. Microsoft
documents that Native AOT debug information is emitted separately by default on
Windows. The executable is therefore the relevant deployed artifact; the PDB is
reported separately rather than silently added to runtime size.

### Correction impact

| C# mode | Measured command | Primary artifact | Analyze median | `--help` median |
|---|---|---:|---:|---:|
| Framework-dependent, **invalidated** | `dotnet BuildGraph.dll` | 17,920 B | 92.715 ms | 79.220 ms |
| Native AOT, corrected, current rerun | `BuildGraph.exe` | 1,546,240 B | 35.016 ms | 33.148 ms |

The current Native AOT analysis median is 62.23% below the old value and the
startup median is 58.16% below it. The honest self-contained executable is 86.29 times
larger than the old application-only DLL. The timing difference is descriptive,
not a causal estimate of AOT alone: the invalid run used 20 iterations, two
warmups, and a .NET 10 RC SDK, while the corrected run used 50 iterations, five
warmups, and the pinned .NET 9 SDK. The size correction is the omitted
self-contained deployment cost, not a regression in equivalent artifacts.

### Common Lisp deployment mode

The Common Lisp application logic is in
[`common-lisp/main.lisp`](common-lisp/main.lisp). The fixed scaffold in
[`common-lisp/build.lisp`](common-lisp/build.lisp) uses
`sb-ext:save-lisp-and-die` with `:executable t` to save a directly runnable SBCL
image. `:save-runtime-options t` prevents SBCL's startup option parser from
claiming the application's `--help` flag. The harness invokes that image
directly rather than launching `sbcl main.lisp`.

The official Windows SBCL 2.6.7 binary used here was not built with zstd
support, so its attempt to save a compressed core failed. The recorded
39,522,400-byte artifact is therefore an uncompressed image that includes the
Lisp runtime. It still ran with `PATH` reduced to the Windows system directory,
so the measurement does not omit a separately installed SBCL runtime. The SBCL
manual documents executable image creation, runtime-option saving, and optional
compression in
[`save-lisp-and-die`](https://www.sbcl.org/manual/#Saving-a-Core-Image).

## 2. What was implemented

BuildGraph is a deterministic command-line dependency-graph analyzer. It:

- parses a line-oriented task format;
- validates identifiers, durations, dependencies, duplicates, and cycles;
- produces a stable Kahn topological order;
- computes a checked signed 64-bit critical path with deterministic tie-breaks;
- computes the reverse dependent closure for an `affected` query;
- emits exact stdout, stderr, and exit codes.

The normative contract is in [`SPEC.md`](SPEC.md#L1-L97). In particular, the
task declaration index is the universal tie-break, critical-path arithmetic is
checked, and all successful output is canonical
([`SPEC.md`](SPEC.md#L46-L75)).

The same application is implemented in:

| Language | Source |
|---|---|
| Oscan | [`oscan/main.osc`](oscan/main.osc) |
| Rust | [`rust/src/main.rs`](rust/src/main.rs) |
| TypeScript | [`typescript/src/main.ts`](typescript/src/main.ts) |
| C# | [`csharp/Program.cs`](csharp/Program.cs) |
| Common Lisp | [`common-lisp/main.lisp`](common-lisp/main.lisp) |

No implementation uses an application package dependency. Build products,
temporary inputs, the isolated Oscan compiler checkout, TypeScript tooling, and
the saved SBCL image remain below `LangugageComp`, as documented in
[`README.md`](README.md#L1-L42).

## 3. Experimental method

### 3.1 Correctness oracle

The Python harness executes the compiled command for each language and compares:

- exact process exit code;
- exact stdout;
- exact stderr;
- timeout behavior.

The runner is external to all five implementations
([`harness/suite.py`](harness/suite.py#L341-L463)). The 47 cases are defined
once in [`harness/cases.py`](harness/cases.py#L60-L355), rather than duplicated
inside language-specific unit tests.

Coverage includes:

- help, invalid CLI forms, and missing files;
- empty/comment-only input and malformed field counts;
- invalid ASCII identifiers and numeric boundaries;
- duplicate tasks and dependencies;
- empty, self, invalid, and unknown dependencies;
- cycle detection;
- forward references and disconnected graphs;
- stable root ordering and newly-ready ordering;
- shallow and deep critical-path ties;
- 64-bit totals;
- LF and CRLF;
- affected closures for roots, middle nodes, leaves, disconnected tasks, and
  unknown tasks;
- a 150-task chain and a 101-task wide DAG
  ([`harness/cases.py`](harness/cases.py#L243-L355)).

This is a strong executable regression suite for a reference experiment, but it
is not a proof of correctness. The suite has not yet been mutation-tested
against a systematic set of deliberately faulty implementations.

### 3.2 Build and deployment modes

The shared harness builds:

- Oscan with the repository compiler's C backend into `buildgraph.exe`;
- Rust with `cargo build --release`;
- TypeScript with its fixed compiler dependency into `main.js`, executed by
  Node.js;
- C# with .NET 9 Native AOT for `win-x64`, executed directly;
- Common Lisp with SBCL `save-lisp-and-die`, executed directly.

Exact build and command selection are centralized in
[`harness/suite.py`](harness/suite.py#L224-L335). This avoids accidentally
testing one command and benchmarking another.

The primary artifact means:

- Oscan: deployed executable;
- Rust: release executable;
- TypeScript: emitted JavaScript only, explicitly excluding Node.js;
- C#: self-contained Native AOT executable, excluding the optional PDB;
- Common Lisp: uncompressed SBCL executable image, including the Lisp runtime.

Because Node.js was not copied and measured, the TypeScript artifact-size value
is an application payload metric, not a deployment-size metric. The C# Native
AOT executable contains the required trimmed runtime code, as Microsoft
documents. The Common Lisp image likewise includes its runtime. Those values
must not be ranked as though they had identical boundaries.

### 3.3 Timing method

The benchmark creates a 1,000-task dependency chain and measures:

1. five warmup executions of both `--help` and `analyze`;
2. 50 fresh-process `--help` executions;
3. 50 fresh-process `analyze chain.bg` executions.

Elapsed time is measured around each child process with
`time.perf_counter`; stdout is discarded, stderr is captured, and nonzero exit
status fails the run
([`harness/suite.py`](harness/suite.py#L476-L586)).

The `analyze` median is the main operational measurement. The report also shows
`analyze median - help median` as an approximate diagnostic, but the
subtraction does not isolate algorithm time: the two medians come from separate
processes, and process scheduling, file I/O, output construction, loader work,
runtime initialization, and antivirus activity remain mixed together.

### 3.4 Host and toolchains

| Component | Recorded value |
|---|---|
| OS | Microsoft Windows 11 Enterprise 10.0.26200, x64 |
| CPU | Intel Core i9-13950HX |
| Repository base | `7291ebea65083da2c419e6466f802e0351bcac43` plus the measured collection-API working-tree changes |
| Python | 3.13.14 |
| Rust | rustc 1.94.1 |
| Node.js | 22.14.0 |
| TypeScript | 5.9.3 |
| .NET SDK | 9.0.316 |
| Native AOT compiler package | Microsoft.DotNet.ILCompiler 9.0.18 |
| Common Lisp | SBCL 2.6.7 |
| GCC visible to Oscan build | 15.2.0 |
| C# RID | `win-x64` |

Results are host-specific. They are not cross-platform estimates.

## 4. Results

### 4.1 Reference correctness

| Language | Passed | Total | Rate |
|---|---:|---:|---:|
| Oscan | 47 | 47 | 100% |
| Rust | 47 | 47 | 100% |
| TypeScript | 47 | 47 | 100% |
| C# Native AOT | 47 | 47 | 100% |
| Common Lisp (SBCL) | 47 | 47 | 100% |
| **Combined** | **235** | **235** | **100%** |

This result confirms semantic parity under the current oracle. It does not
distinguish language quality because these are repaired reference
implementations, not unedited model samples.

### 4.2 Source and artifact size

| Language | Physical lines | Source bytes | Primary artifact | Artifact boundary |
|---|---:|---:|---:|---|
| Oscan | 486 | 15,236 | 28,672 B | Native executable |
| Rust | 335 | 10,935 | 193,536 B | Release executable |
| TypeScript | 357 | 10,687 | 9,958 B | JavaScript only; Node excluded |
| C# Native AOT | 381 | 11,448 | 1,546,240 B | Self-contained executable |
| Common Lisp (SBCL) | 358 | 14,977 | 39,522,400 B | Self-contained SBCL image |

C# also emitted an 8,015,872-byte optional PDB. The complete publish directory
was 9,562,112 bytes with that debug file, but the executable runs without it.
The SBCL image has no required runtime sidecar.

Normalized to Oscan:

| Comparison | Result |
|---|---:|
| Oscan artifact / Rust artifact | 0.148x |
| Oscan artifact / C# Native AOT artifact | 0.0185x |
| Oscan artifact / Common Lisp artifact | 0.000725x |
| Oscan artifact / TypeScript JS payload | 2.879x |
| Oscan source bytes / Rust source bytes | 1.393x |
| Oscan source bytes / TypeScript source bytes | 1.426x |
| Oscan source bytes / C# source bytes | 1.331x |
| Oscan source bytes / Common Lisp source bytes | 1.017x |

Oscan's artifact result is excellent among the self-contained/native outputs.
Its source-size result points in the opposite direction: this reference
implementation is 2-43% larger in bytes than all four baselines and has the
largest physical line count. Common Lisp is 1.7% smaller than Oscan by source
bytes, while its 358 lines are close to TypeScript's 357. Source length is not
maintainability or generation difficulty, but it gives no support to a claim
that this task is more concise in Oscan.

#### Collection API impact

| Oscan source | Physical lines | Source bytes |
|---|---:|---:|
| Before collection refactor | 542 | 16,229 |
| After collection refactor | 486 | 15,236 |
| Change | **-56 (-10.33%)** | **-993 (-6.12%)** |

The refactor replaced manual copy/fill/reverse/search/comparison loops with
`array_clone`, `array_repeat`, `array_reverse`, `array_all_i32`,
`array_index_of_str`, and `array_compare_i32`. It did not change the BuildGraph
algorithm or oracle. The executable grew by 1,536 bytes (5.66%) because the
broader collection primitives and their runtime guard paths are now linked.

### 4.3 Cold process timing

| Language | Analyze median | Analyze min-max | `--help` median | Approximate difference |
|---|---:|---:|---:|---:|
| Oscan | 21.239 ms | 17.479-64.160 ms | 17.343 ms | 3.896 ms |
| Rust | 37.820 ms | 29.498-49.425 ms | 27.522 ms | 10.298 ms |
| TypeScript | 87.384 ms | 78.847-138.055 ms | 72.590 ms | 14.794 ms |
| C# Native AOT | 35.016 ms | 29.585-54.204 ms | 33.148 ms | 1.868 ms |
| Common Lisp (SBCL) | 73.429 ms | 67.826-819.723 ms | 53.271 ms | 20.158 ms |

Interpretation:

- Oscan had the lowest analysis median: 43.8% below Rust and 39.3% below C#
  Native AOT in the latest recorded measurements.
- Oscan's median was 71.1% below Common Lisp's.
- Oscan's analysis median was 75.7% below TypeScript/Node's; TypeScript was
  4.11 times Oscan.
- Oscan also had the lowest `--help` median, but this is still a whole-process
  measurement rather than pure loader time.
- These rankings changed from the prior run. Fixed language order, background
  host activity, and whole-process timing make small cross-run conclusions
  especially unsafe.
- The approximate difference column should not be used to rank the graph
  algorithm implementations.

## 5. Parameter-by-parameter verdict

| Parameter | Finding | Does it show Oscan superiority? |
|---|---|---|
| Reference functional correctness | Five-way tie at 47/47 | No; it establishes feasibility and parity |
| Source concision | Oscan is largest by lines and bytes | No |
| Self-contained artifact | Oscan is 6.75x smaller than Rust, 53.93x smaller than C# AOT, and 1,378.43x smaller than SBCL | Yes, narrowly for this build |
| Cold analysis process time | Oscan has the lowest latest recorded median, but cross-run stability is weak | No robust Oscan-versus-Rust conclusion |
| Cold time vs C# AOT | Oscan is 39.3% lower | Yes, on this host and workload |
| Cold time vs Common Lisp | Oscan is 71.1% lower | Yes, on this host and workload |
| Cold time vs TypeScript/Node | Oscan is 75.7% lower | Yes, on this host and workload |
| Required runtime accounting | Oscan, C#, and Common Lisp include their runtimes; Node is omitted | TypeScript size ranking is unresolved |
| One-shot build@1 | Not measured | No conclusion |
| One-shot all-tests pass@1 | Not measured | No conclusion |
| Model output tokens, cost, latency | Not measured | No conclusion |
| Maintainability of generated programs | Not measured | No conclusion |

There is no sound single "overall score." Weighting artifact size, source size,
runtime, and model correctness into one number would conceal the fact that they
answer different questions.

## 6. What this says about Oscan

### Supported findings

- BuildGraph is feasible at medium complexity in current Oscan.
- The language and runtime can express strict parsing, deterministic
  diagnostics, graph algorithms, checked 64-bit totals, and stress cases
  without an application dependency.
- The C backend produced a very small executable.
- Cold process behavior led the latest recorded Rust, C# AOT, SBCL, and Node
  invocations, although the Oscan-versus-Rust ordering is not stable across runs.

### Unsupported claims

- That frontier models are more likely to produce a compiling Oscan program.
- That an unedited one-shot Oscan program is more likely to pass the oracle.
- That Oscan needs fewer output tokens or less generation cost.
- That Oscan is more maintainable.
- That the result generalizes beyond BuildGraph, Windows x64, this compiler
  revision, or these reference implementations.

The observed Oscan source remains longer than all four baseline sources, although
collection primitives reduced it by 10.33% in lines and 6.12% in bytes. Several
issues were repaired with compiler and test feedback during implementation.
That history is evidence that this run is not a one-shot sample, not evidence
for or against Oscan's eventual pass@1.

## 7. Why reference parity is not an LLM experiment

The fixed prompt in [`prompts/one-shot.txt`](prompts/one-shot.txt#L1-L74)
requires one complete source file, no packages, exact diagnostics, deterministic
tie-breaks, and checked arithmetic. A valid one-shot trial would give that
contract to a model once and score its raw output without repair.

This implementation session instead used:

- repository and language-reference inspection;
- shell and build tools;
- compiler diagnostics;
- the visible executable oracle;
- iterative source edits;
- repeated builds and tests.

Those are appropriate for producing reviewed reference implementations, but
they measure an agentic software-development workflow. Treating the resulting
47/47 scores as pass@1 would be a category error.

The assistant used for this implementation session was GPT-5.6 Sol
(`gpt-5.6-sol`). That records who produced the references; it does not turn the
interactive, tool-using session into a valid one-shot experimental sample.

Execution-based grading and repeated samples are standard in code-model
evaluation. HumanEval evaluates generated completions through tests and reports
pass@k; its official harness retains a result for every completion
([Chen et al., 2021](https://arxiv.org/abs/2107.03374);
[official HumanEval harness](https://github.com/openai/human-eval)). EvalPlus
demonstrates why a larger adversarial test suite matters, providing far more
tests than the original HumanEval
([Liu et al., 2023](https://arxiv.org/abs/2305.01210);
[EvalPlus](https://github.com/evalplus/evalplus)). MultiPL-E provides the direct
multilingual precedent: equivalent benchmark semantics are translated and
executed with language-specific harnesses across 18 languages
([Cassano et al., 2022](https://arxiv.org/abs/2208.08227);
[MultiPL-E](https://github.com/nuprl/MultiPL-E)).

## 8. Required next experiment for an LLM-superiority claim

Use the completed references only to validate the toolchains and oracle. Then:

1. Freeze `SPEC.md`, the harness, compiler versions, OS image, resource limits,
   and per-language scaffolds.
2. Select exact, immutable snapshots of several frontier coding models
   available on the collection date. Treat model as a blocking factor.
3. Start every sample in a fresh conversation with tools, retrieval, shell,
   compiler feedback, and retries disabled.
4. Request exactly one source file. Build and test it without human edits.
5. Retain compilation failures, malformed responses, truncations, timeouts, and
   wrong programs in the denominator.
6. Use **all-tests pass@1** as the primary endpoint and build@1, test fraction,
   severe-defect rate, tokens, latency, and cost as diagnostics.
7. Run repeated independent samples. A reasonable confirmatory starting design
   is 50 samples per model-language cell, with final size set after a pilot.
8. Randomize and interleave model/language order to reduce service drift. Record
   exact model IDs, inference settings, timestamps, response hashes, and
   duplicate-output rates.
9. Keep runtime and artifact metrics separate from generation correctness.
   Benchmark only correct generated programs and retain these reviewed
   references as a toolchain baseline.
10. Mutation-test the oracle before collection, especially for unstable
    ordering, 32-bit overflow, incomplete cycle detection, direct-only affected
    queries, and wrong critical-path tie-breaks.
11. Execute generated code in a networkless security sandbox. The official
    HumanEval and MultiPL-E projects explicitly warn that model-generated code
    is untrusted and recommend isolated execution.
12. Replicate on unrelated application families before making a general claim
    about Oscan rather than a BuildGraph-specific claim.

For a strict one-shot product claim, report pass@1. Reporting the best of
multiple attempts as pass@k changes the user workflow and inference budget.

## 9. Threats to validity

| Threat | Effect on this report |
|---|---|
| One application | Results may reflect graph/parser fit rather than general language properties |
| Repaired references | 100% correctness cannot estimate model pass@1 |
| Visible tests during development | References can be tuned to the oracle |
| No mutation score | Unknown ability to reject some plausible defects |
| One Windows host | Timing and native artifact results may differ elsewhere |
| Fixed language order | Background system drift may bias small timing differences |
| Whole-process timer | Loader, scheduler, I/O, output, and algorithm work are mixed |
| Independent median subtraction | The "difference" is not isolated algorithm time |
| Node omitted from size | TypeScript deployment size cannot be ranked fairly |
| Optional PDB omitted | Correct for runtime deployment, but full debug-output size is larger |
| Uncompressed SBCL image | Official Windows SBCL lacked zstd; another SBCL build could change size and startup |
| Physical source size | Includes formatting/comments and is not a maintainability metric |
| No build time or peak memory | Operational comparison is incomplete |
| No model outputs | No build@1, pass@1, token, cost, or failure taxonomy exists |

## 10. Reproduction

From `LangugageComp` on a machine with the documented toolchains:

```powershell
.\test.ps1 -Json .\.build\test-results-final.json
.\benchmark.ps1 -Tasks 1000 -Iterations 50 -NoBuild `
  -Json .\.build\benchmark-results.json
```

The first command rebuilds all five implementations and executes 235
language-case checks. The second uses those exact artifacts for the 50-sample
benchmark. To rerun Common Lisp alone:

```powershell
python .\harness\suite.py test --language common-lisp `
  --json .\.build\test-results-common-lisp.json
python .\harness\suite.py benchmark --language common-lisp --tasks 1000 `
  --warmup 5 --iterations 50 --no-build `
  --json .\.build\benchmark-results-common-lisp.json
```

## 11. Final answer

The Native AOT correction is decisive for C#: the old 17.9 KB / 92.7 ms row was
not a valid native deployment comparison. The current C# result is a 1.55 MB
self-contained executable with a 35.0 ms
median process time.

Under the corrected reference experiment, Oscan's strongest advantages are
deployed native size and cold process behavior versus C# AOT, SBCL, and Node.
Its timing relationship with Rust is not stable across the two recorded runs,
and all four baseline sources are shorter. Common Lisp is more concise than
Oscan and starts faster than Node in this run, but its uncompressed 39.5 MB SBCL
image is by far the largest artifact. All five references are equally correct
under the 47-case oracle.

Therefore, **Oscan is operationally attractive for this application, but this
experiment does not show that Oscan is superior when used by an LLM**. That
claim remains a testable hypothesis whose required evidence is repeated,
independent, unedited one-shot pass@1 across pinned frontier models.

## Sources

### Local experiment

- [`SPEC.md`](SPEC.md)
- [`harness/cases.py`](harness/cases.py)
- [`harness/suite.py`](harness/suite.py)
- [`csharp/BuildGraph.csproj`](csharp/BuildGraph.csproj)
- [`common-lisp/main.lisp`](common-lisp/main.lisp)
- [`common-lisp/build.lisp`](common-lisp/build.lisp)
- [`global.json`](global.json)
- [`REFERENCE_RESULTS.md`](REFERENCE_RESULTS.md)
- [`prompts/one-shot.txt`](prompts/one-shot.txt)

### External methodology and toolchain

- Microsoft, [Native AOT deployment overview](https://learn.microsoft.com/en-us/dotnet/core/deploying/native-aot/)
- Steel Bank Common Lisp, [SBCL User Manual](https://www.sbcl.org/manual/)
- Chen et al., [Evaluating Large Language Models Trained on Code](https://arxiv.org/abs/2107.03374)
- OpenAI, [HumanEval evaluation harness](https://github.com/openai/human-eval)
- Liu et al., [Is Your Code Generated by ChatGPT Really Correct?](https://arxiv.org/abs/2305.01210)
- [EvalPlus official repository](https://github.com/evalplus/evalplus)
- Cassano et al., [MultiPL-E](https://arxiv.org/abs/2208.08227)
- [MultiPL-E official repository](https://github.com/nuprl/MultiPL-E)
