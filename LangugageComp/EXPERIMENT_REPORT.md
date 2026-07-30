# Corrected BuildGraph language experiment

**Research date:** 2026-07-30  
**Repository revision:** [`c620ecb8ffaa28ce6e445db2d54474fcbbb24a90`](https://github.com/lucabol/Oscan/tree/c620ecb8ffaa28ce6e445db2d54474fcbbb24a90)  
**Compared implementations:** Oscan, Rust, TypeScript/Node.js, and C# .NET 9 Native AOT  
**Correction:** all earlier framework-dependent C# size and timing results are invalidated

## Executive conclusion

The corrected experiment establishes three useful facts and one important
non-result:

1. **The four reviewed reference implementations are behaviorally equivalent
   under the current oracle.** Each passed all 47 executable cases, for 188/188
   language-case checks.
2. **Oscan has the strongest self-contained native artifact-size result.** Its
   27,136-byte executable is 7.13 times smaller than Rust's 193,536-byte
   executable and 56.98 times smaller than C# Native AOT's 1,546,240-byte
   executable. TypeScript's 9,958-byte JavaScript file is smaller as an
   application payload, but that number excludes Node.js and is therefore not a
   self-contained deployment comparison.
3. **Oscan has near-Rust cold process time and materially lower cold process
   time than C# Native AOT and TypeScript/Node on this host.** The 1,000-task
   analysis medians were 21.820 ms for Oscan, 20.683 ms for Rust, 27.526 ms for
   C# Native AOT, and 77.742 ms for TypeScript.
4. **The experiment does not measure whether an LLM writes Oscan more
   successfully.** These programs were produced and repaired interactively
   using repository access, compiler feedback, tests, and multiple edits. No
   repeated, independent, tool-free one-shot generations were collected, so
   all-tests pass@1 is unobserved.

The defensible conclusion is narrow: **for this reviewed BuildGraph
implementation and toolchain, Oscan combines the smallest self-contained native
artifact with process time close to Rust, but it requires the largest source
file. There is not yet evidence that Oscan is superior to Rust, TypeScript, or
C# when programmed by an LLM.**

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
- [`harness/suite.py`](harness/suite.py#L259-L281) invokes
  `dotnet publish -c Release -r win-x64 --self-contained true`.
- [`harness/suite.py`](harness/suite.py#L289-L297) runs `BuildGraph.exe`
  directly, with no `dotnet` host in the measured command.
- The harness deletes the old C# publish directory before each publish, so a
  stale DLL cannot satisfy the artifact check
  ([`harness/suite.py`](harness/suite.py#L259-L286)).

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
| Native AOT, corrected | `BuildGraph.exe` | 1,546,240 B | 27.526 ms | 23.161 ms |

The corrected analysis median is 70.31% below the old value and the startup
median is 70.76% below it. The honest self-contained executable is 86.29 times
larger than the old application-only DLL. The timing difference is descriptive,
not a causal estimate of AOT alone: the invalid run used 20 iterations, two
warmups, and a .NET 10 RC SDK, while the corrected run used 50 iterations, five
warmups, and the pinned .NET 9 SDK. The size correction is the omitted
self-contained deployment cost, not a regression in equivalent artifacts.

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

No implementation uses an application package dependency. Build products,
temporary inputs, the isolated Oscan compiler checkout, and TypeScript tooling
remain below `LangugageComp`, as documented in
[`README.md`](README.md#L1-L42).

## 3. Experimental method

### 3.1 Correctness oracle

The Python harness executes the compiled command for each language and compares:

- exact process exit code;
- exact stdout;
- exact stderr;
- timeout behavior.

The runner is external to all four implementations
([`harness/suite.py`](harness/suite.py#L304-L426)). The 47 cases are defined
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
- C# with .NET 9 Native AOT for `win-x64`, executed directly.

Exact build and command selection are centralized in
[`harness/suite.py`](harness/suite.py#L203-L297). This avoids accidentally
testing one command and benchmarking another.

The primary artifact means:

- Oscan: deployed executable;
- Rust: release executable;
- TypeScript: emitted JavaScript only, explicitly excluding Node.js;
- C#: self-contained Native AOT executable, excluding the optional PDB.

Because Node.js was not copied and measured, the TypeScript artifact-size value
is an application payload metric, not a deployment-size metric. The C# Native
AOT executable contains the required trimmed runtime code, as Microsoft
documents. Those values must not be ranked as though they had identical
boundaries.

### 3.3 Timing method

The benchmark creates a 1,000-task dependency chain and measures:

1. five warmup executions of both `--help` and `analyze`;
2. 50 fresh-process `--help` executions;
3. 50 fresh-process `analyze chain.bg` executions.

Elapsed time is measured around each child process with
`time.perf_counter`; stdout is discarded, stderr is captured, and nonzero exit
status fails the run
([`harness/suite.py`](harness/suite.py#L438-L548)).

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
| Repository | `c620ecb8ffaa28ce6e445db2d54474fcbbb24a90` |
| Python | 3.13.14 |
| Rust | rustc 1.94.1 |
| Node.js | 22.14.0 |
| TypeScript | 5.9.3 |
| .NET SDK | 9.0.316 |
| Native AOT compiler package | Microsoft.DotNet.ILCompiler 9.0.18 |
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
| **Combined** | **188** | **188** | **100%** |

This result confirms semantic parity under the current oracle. It does not
distinguish language quality because these are repaired reference
implementations, not unedited model samples.

### 4.2 Source and artifact size

| Language | Physical lines | Source bytes | Primary artifact | Artifact boundary |
|---|---:|---:|---:|---|
| Oscan | 542 | 16,229 | 27,136 B | Native executable |
| Rust | 335 | 10,600 | 193,536 B | Release executable |
| TypeScript | 357 | 10,687 | 9,958 B | JavaScript only; Node excluded |
| C# Native AOT | 381 | 11,448 | 1,546,240 B | Self-contained executable |

C# also emitted an 8,015,872-byte optional PDB. The complete publish directory
was 9,562,112 bytes with that debug file, but the executable runs without it.

Normalized to Oscan:

| Comparison | Result |
|---|---:|
| Oscan artifact / Rust artifact | 0.140x |
| Oscan artifact / C# Native AOT artifact | 0.0175x |
| Oscan artifact / TypeScript JS payload | 2.725x |
| Oscan source bytes / Rust source bytes | 1.531x |
| Oscan source bytes / TypeScript source bytes | 1.519x |
| Oscan source bytes / C# source bytes | 1.418x |

Oscan's artifact result is excellent among the self-contained/native outputs.
Its source-size result points in the opposite direction: this reference
implementation is 42-53% larger in bytes and 42-62% longer in physical lines
than the three baselines. Source length is not maintainability or generation
difficulty, but it gives no support to a claim that this task is more concise in
Oscan.

### 4.3 Cold process timing

| Language | Analyze median | Analyze min-max | `--help` median | Approximate difference |
|---|---:|---:|---:|---:|
| Oscan | 21.820 ms | 16.390-35.205 ms | 14.444 ms | 7.376 ms |
| Rust | 20.683 ms | 15.575-41.198 ms | 17.834 ms | 2.849 ms |
| TypeScript | 77.742 ms | 73.899-111.086 ms | 61.843 ms | 15.899 ms |
| C# Native AOT | 27.526 ms | 23.427-55.603 ms | 23.161 ms | 4.365 ms |

Interpretation:

- Rust had the lowest analysis median. Oscan was 5.5% slower, a small
  difference relative to the observed process-level ranges.
- Oscan's analysis median was 20.7% lower than C# Native AOT's; equivalently,
  C# was 1.26 times Oscan.
- Oscan's analysis median was 71.9% lower than TypeScript/Node's; equivalently,
  TypeScript was 3.56 times Oscan.
- Oscan had the lowest `--help` median, but this is still a whole-process
  measurement rather than pure loader time.
- The approximate difference column should not be used to rank the graph
  algorithm implementations.

## 5. Parameter-by-parameter verdict

| Parameter | Finding | Does it show Oscan superiority? |
|---|---|---|
| Reference functional correctness | Four-way tie at 47/47 | No; it establishes feasibility and parity |
| Source concision | Oscan is largest by lines and bytes | No |
| Self-contained native artifact | Oscan is 7.13x smaller than Rust and 56.98x smaller than C# AOT | Yes, narrowly for this build |
| Cold analysis process time | Rust 20.683 ms, Oscan 21.820 ms | No advantage over Rust; near parity |
| Cold time vs C# AOT | Oscan is 20.7% lower | Yes, on this host and workload |
| Cold time vs TypeScript/Node | Oscan is 71.9% lower | Yes, on this host and workload |
| Required runtime accounting | Oscan and C# are represented by deployed executables; Node is omitted | TypeScript size ranking is unresolved |
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
- Cold process behavior is in the same practical band as Rust for this small
  CLI and better than the measured C# AOT and Node invocations.

### Unsupported claims

- That frontier models are more likely to produce a compiling Oscan program.
- That an unedited one-shot Oscan program is more likely to pass the oracle.
- That Oscan needs fewer output tokens or less generation cost.
- That Oscan is more maintainable.
- That the result generalizes beyond BuildGraph, Windows x64, this compiler
  revision, or these reference implementations.

The observed Oscan source is substantially longer than the baseline sources,
and several issues were repaired with compiler and test feedback during
implementation. That history is evidence that this run is not a one-shot sample,
not evidence for or against Oscan's eventual pass@1.

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

The first command rebuilds all four implementations and executes 188
language-case checks. The second uses those exact artifacts for the 50-sample
benchmark. To rerun C# alone:

```powershell
python .\harness\suite.py test --language csharp `
  --json .\.build\test-results-csharp-aot.json
python .\harness\suite.py benchmark --language csharp --tasks 1000 `
  --warmup 5 --iterations 50 --no-build `
  --json .\.build\benchmark-results-csharp-aot.json
```

## 11. Final answer

The Native AOT correction is decisive for C#: the old 17.9 KB / 92.7 ms row was
not a valid native deployment comparison. The corrected C# result is a 1.55 MB
self-contained executable with a 27.5 ms median process time.

Under the corrected reference experiment, Oscan's strongest advantages are
deployed native size and cold process behavior versus C# AOT and Node. Rust is
slightly faster, and all three baseline sources are shorter. All four references
are equally correct under the 47-case oracle.

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
- [`global.json`](global.json)
- [`REFERENCE_RESULTS.md`](REFERENCE_RESULTS.md)
- [`prompts/one-shot.txt`](prompts/one-shot.txt)

### External methodology and toolchain

- Microsoft, [Native AOT deployment overview](https://learn.microsoft.com/en-us/dotnet/core/deploying/native-aot/)
- Chen et al., [Evaluating Large Language Models Trained on Code](https://arxiv.org/abs/2107.03374)
- OpenAI, [HumanEval evaluation harness](https://github.com/openai/human-eval)
- Liu et al., [Is Your Code Generated by ChatGPT Really Correct?](https://arxiv.org/abs/2305.01210)
- [EvalPlus official repository](https://github.com/evalplus/evalplus)
- Cassano et al., [MultiPL-E](https://arxiv.org/abs/2208.08227)
- [MultiPL-E official repository](https://github.com/nuprl/MultiPL-E)
