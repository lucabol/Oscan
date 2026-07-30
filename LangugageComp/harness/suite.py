#!/usr/bin/env python3
"""Build, behavior-test, and benchmark all BuildGraph implementations."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import tarfile
import tempfile
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Sequence

from cases import TestCase, all_cases


ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = ROOT.parent
BUILD = ROOT / ".build"
EXE_SUFFIX = ".exe" if os.name == "nt" else ""
LANGUAGES = ("oscan", "rust", "typescript", "csharp", "common-lisp")
NPM = "npm.cmd" if os.name == "nt" else "npm"


@dataclass
class TestResult:
    language: str
    case: str
    passed: bool
    expected_code: int
    actual_code: int | None
    expected_stdout: str
    actual_stdout: str
    expected_stderr: str
    actual_stderr: str
    elapsed_ms: float
    error: str | None = None


def environment() -> dict[str, str]:
    result = os.environ.copy()
    result.update(
        {
            "DOTNET_CLI_TELEMETRY_OPTOUT": "1",
            "DOTNET_NOLOGO": "1",
            "NUGET_XMLDOC_MODE": "skip",
        }
    )
    if os.name == "nt":
        program_files_x86 = os.environ.get(
            "ProgramFiles(x86)", r"C:\Program Files (x86)"
        )
        visual_studio_installer = (
            Path(program_files_x86) / "Microsoft Visual Studio" / "Installer"
        )
        if (visual_studio_installer / "vswhere.exe").exists():
            result["PATH"] = (
                f"{visual_studio_installer}{os.pathsep}{result.get('PATH', '')}"
            )
    return result


def run(
    command: Sequence[str],
    cwd: Path = ROOT,
    *,
    capture_stdout: bool = True,
) -> subprocess.CompletedProcess[bytes]:
    completed = subprocess.run(
        list(command),
        cwd=cwd,
        env=environment(),
        stdout=subprocess.PIPE if capture_stdout else None,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        rendered = subprocess.list2cmdline(list(command))
        stdout = (completed.stdout or b"").decode("utf-8", errors="replace")
        stderr = completed.stderr.decode("utf-8", errors="replace")
        raise RuntimeError(
            f"command failed ({completed.returncode}): {rendered}\n{stdout}{stderr}"
        )
    return completed


def output(command: Sequence[str], cwd: Path = ROOT) -> str:
    return run(command, cwd).stdout.decode("utf-8").strip()


def prepare_isolated_oscan_source() -> Path:
    """Materialize compiler source and its submodule entirely below .build."""
    compiler_source = BUILD / "oscan-source"
    commit = output(["git", "rev-parse", "HEAD"], REPOSITORY)
    tree_entry = output(
        ["git", "ls-tree", commit, "deps/laststanding"], REPOSITORY
    ).split()
    if len(tree_entry) < 3:
        raise RuntimeError("could not resolve deps/laststanding gitlink")
    dependency_commit = tree_entry[2]
    marker_text = f"{commit}\n{dependency_commit}\n"
    marker = compiler_source / ".language-comp-source"

    if marker.exists() and marker.read_text(encoding="utf-8") == marker_text:
        return compiler_source

    if compiler_source.exists():
        shutil.rmtree(compiler_source)
    compiler_source.mkdir(parents=True)
    archive = run(["git", "archive", "--format=tar", commit], REPOSITORY).stdout
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as tar:
        tar.extractall(compiler_source, filter="data")

    dependency_cache = BUILD / "laststanding"
    if not (dependency_cache / ".git").exists():
        if dependency_cache.exists():
            shutil.rmtree(dependency_cache)
        run(
            [
                "git",
                "clone",
                "--quiet",
                "https://github.com/lucabol/laststanding",
                str(dependency_cache),
            ],
            BUILD,
        )
    else:
        run(["git", "fetch", "--quiet", "origin"], dependency_cache)
    run(["git", "checkout", "--quiet", "--detach", dependency_commit], dependency_cache)
    run(
        ["git", "submodule", "update", "--init", "--recursive", "--quiet"],
        dependency_cache,
    )

    dependency_destination = compiler_source / "deps" / "laststanding"
    if dependency_destination.exists():
        shutil.rmtree(dependency_destination)
    shutil.copytree(
        dependency_cache,
        dependency_destination,
        ignore=shutil.ignore_patterns(".git"),
    )
    marker.write_text(marker_text, encoding="utf-8", newline="\n")
    return compiler_source


def compiler_path() -> Path:
    return BUILD / "oscan-compiler-target" / "release" / f"oscan{EXE_SUFFIX}"


def native_aot_rid() -> str:
    override = os.environ.get("BUILDGRAPH_DOTNET_RID")
    if override:
        return override

    operating_system = platform.system()
    machine = platform.machine().lower()
    architecture = {
        "amd64": "x64",
        "x86_64": "x64",
        "arm64": "arm64",
        "aarch64": "arm64",
        "x86": "x86",
        "i386": "x86",
        "i686": "x86",
    }.get(machine)
    os_name = {
        "Windows": "win",
        "Linux": "linux",
        "Darwin": "osx",
    }.get(operating_system)
    if os_name is None or architecture is None:
        raise RuntimeError(
            "unsupported Native AOT host; set BUILDGRAPH_DOTNET_RID explicitly"
        )
    if os_name != "win" and architecture == "x86":
        raise RuntimeError(
            "unsupported Native AOT host; set BUILDGRAPH_DOTNET_RID explicitly"
        )
    return f"{os_name}-{architecture}"


def sbcl_path() -> str:
    override = os.environ.get("BUILDGRAPH_SBCL")
    if override:
        return override

    discovered = shutil.which("sbcl")
    if discovered:
        return discovered

    if os.name == "nt":
        program_files = os.environ.get("ProgramFiles", r"C:\Program Files")
        installed = Path(program_files) / "Steel Bank Common Lisp" / "sbcl.exe"
        if installed.exists():
            return str(installed)

    raise RuntimeError(
        "SBCL is required for Common Lisp; install it or set BUILDGRAPH_SBCL"
    )


def artifact_path(language: str) -> Path:
    paths = {
        "oscan": BUILD / "oscan" / f"buildgraph{EXE_SUFFIX}",
        "rust": BUILD / "rust-target" / "release" / f"buildgraph{EXE_SUFFIX}",
        "typescript": BUILD / "typescript" / "main.js",
        "csharp": BUILD / "csharp" / f"BuildGraph{EXE_SUFFIX}",
        "common-lisp": BUILD / "common-lisp" / f"buildgraph{EXE_SUFFIX}",
    }
    return paths[language]


def build(language: str) -> None:
    BUILD.mkdir(parents=True, exist_ok=True)
    if language == "oscan":
        compiler_source = prepare_isolated_oscan_source()
        run(
            [
                "cargo",
                "build",
                "--release",
                "--quiet",
                "--manifest-path",
                str(compiler_source / "Cargo.toml"),
                "--target-dir",
                str(BUILD / "oscan-compiler-target"),
            ],
            compiler_source,
        )
        artifact_path(language).parent.mkdir(parents=True, exist_ok=True)
        run(
            [
                str(compiler_path()),
                str(ROOT / "oscan" / "main.osc"),
                "--backend",
                "c",
                "-o",
                str(artifact_path(language)),
            ]
        )
    elif language == "rust":
        run(
            [
                "cargo",
                "build",
                "--release",
                "--quiet",
                "--manifest-path",
                str(ROOT / "rust" / "Cargo.toml"),
                "--target-dir",
                str(BUILD / "rust-target"),
            ]
        )
    elif language == "typescript":
        package_dir = ROOT / "typescript"
        if not (package_dir / "node_modules" / "typescript").exists():
            run(
                [
                    NPM,
                    "ci",
                    "--ignore-scripts",
                    "--no-audit",
                    "--no-fund",
                    "--silent",
                ],
                package_dir,
            )
        run([NPM, "run", "build", "--silent"], package_dir)
    elif language == "csharp":
        publish_directory = artifact_path(language).parent
        if publish_directory.exists():
            shutil.rmtree(publish_directory)
        run(
            [
                "dotnet",
                "publish",
                str(ROOT / "csharp" / "BuildGraph.csproj"),
                "-c",
                "Release",
                "-r",
                native_aot_rid(),
                "--self-contained",
                "true",
                "-o",
                str(publish_directory),
                "-p:PublishAot=true",
                "--nologo",
                "--verbosity",
                "quiet",
            ]
        )
    elif language == "common-lisp":
        artifact = artifact_path(language)
        artifact.parent.mkdir(parents=True, exist_ok=True)
        artifact.unlink(missing_ok=True)
        run(
            [
                sbcl_path(),
                "--noinform",
                "--no-sysinit",
                "--no-userinit",
                "--disable-debugger",
                "--load",
                str(ROOT / "common-lisp" / "build.lisp"),
                "--quit",
            ]
        )
    else:
        raise ValueError(f"unknown language: {language}")

    if not artifact_path(language).exists():
        raise RuntimeError(f"{language} build did not create {artifact_path(language)}")


def command_for(language: str) -> list[str]:
    artifact = artifact_path(language)
    if language in {"oscan", "rust", "common-lisp"}:
        return [str(artifact)]
    if language == "typescript":
        return ["node", str(artifact)]
    if language == "csharp":
        return [str(artifact)]
    raise ValueError(f"unknown language: {language}")


def selected(value: str) -> list[str]:
    return list(LANGUAGES) if value == "all" else [value]


def run_case(
    language: str, case: TestCase, work_parent: Path, timeout: float
) -> TestResult:
    case_dir = work_parent / f"{language}-{case.name}"
    case_dir.mkdir(parents=True)
    arguments = list(case.args)
    if case.content is not None:
        data = case.content if isinstance(case.content, bytes) else case.content.encode()
        (case_dir / "input.bg").write_bytes(data)
        arguments = ["input.bg" if value == "{file}" else value for value in arguments]

    started = time.perf_counter()
    try:
        completed = subprocess.run(
            command_for(language) + arguments,
            cwd=case_dir,
            env=environment(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            check=False,
        )
        elapsed_ms = (time.perf_counter() - started) * 1000
        stdout = completed.stdout.decode("utf-8", errors="replace")
        stderr = completed.stderr.decode("utf-8", errors="replace")
        passed = (
            completed.returncode == case.code
            and stdout == case.stdout
            and stderr == case.stderr
        )
        return TestResult(
            language,
            case.name,
            passed,
            case.code,
            completed.returncode,
            case.stdout,
            stdout,
            case.stderr,
            stderr,
            elapsed_ms,
        )
    except subprocess.TimeoutExpired as error:
        return TestResult(
            language,
            case.name,
            False,
            case.code,
            None,
            case.stdout,
            (error.stdout or b"").decode("utf-8", errors="replace"),
            case.stderr,
            (error.stderr or b"").decode("utf-8", errors="replace"),
            (time.perf_counter() - started) * 1000,
            f"timed out after {timeout:.1f}s",
        )


def build_action(args: argparse.Namespace) -> int:
    for language in selected(args.language):
        print(f"building {language}...", flush=True)
        build(language)
    return 0


def test_action(args: argparse.Namespace) -> int:
    languages = selected(args.language)
    if not args.no_build:
        for language in languages:
            print(f"building {language}...", flush=True)
            build(language)

    cases = all_cases()
    results: list[TestResult] = []
    BUILD.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="tests-", dir=BUILD) as temp:
        for language in languages:
            print(f"testing {language} ({len(cases)} cases)...", flush=True)
            for case in cases:
                result = run_case(language, case, Path(temp), args.timeout)
                results.append(result)
                if result.passed:
                    continue
                print(
                    f"FAIL {language}/{case.name}: code {result.actual_code!r}",
                    file=sys.stderr,
                )
                if result.error:
                    print(f"  {result.error}", file=sys.stderr)
                if result.actual_stdout != result.expected_stdout:
                    print(
                        f"  stdout expected={result.expected_stdout!r} "
                        f"actual={result.actual_stdout!r}",
                        file=sys.stderr,
                    )
                if result.actual_stderr != result.expected_stderr:
                    print(
                        f"  stderr expected={result.expected_stderr!r} "
                        f"actual={result.actual_stderr!r}",
                        file=sys.stderr,
                    )

    passed = sum(result.passed for result in results)
    report = {
        "passed": passed,
        "total": len(results),
        "languages": {
            language: {
                "passed": sum(
                    result.passed for result in results if result.language == language
                ),
                "total": sum(1 for result in results if result.language == language),
            }
            for language in languages
        },
        "results": [asdict(result) for result in results],
    }
    if args.json:
        destination = Path(args.json).resolve()
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"{passed}/{len(results)} cases passed")
    return 0 if passed == len(results) else 1


def source_path(language: str) -> Path:
    return {
        "oscan": ROOT / "oscan" / "main.osc",
        "rust": ROOT / "rust" / "src" / "main.rs",
        "typescript": ROOT / "typescript" / "src" / "main.ts",
        "csharp": ROOT / "csharp" / "Program.cs",
        "common-lisp": ROOT / "common-lisp" / "main.lisp",
    }[language]


def benchmark_action(args: argparse.Namespace) -> int:
    languages = selected(args.language)
    if not args.no_build:
        for language in languages:
            print(f"building {language}...", flush=True)
            build(language)

    work = BUILD / "benchmark"
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    lines = ["task0 | 1 |"] + [
        f"task{index} | 1 | task{index - 1}" for index in range(1, args.tasks)
    ]
    (work / "chain.bg").write_text(
        "\n".join(lines) + "\n", encoding="utf-8", newline="\n"
    )

    rows = []
    for language in languages:
        command = command_for(language) + ["analyze", "chain.bg"]
        startup_command = command_for(language) + ["--help"]
        for _ in range(args.warmup):
            for warmup_command in (startup_command, command):
                completed = subprocess.run(
                    warmup_command,
                    cwd=work,
                    env=environment(),
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.PIPE,
                    timeout=args.timeout,
                    check=False,
                )
                if completed.returncode != 0:
                    raise RuntimeError(
                        f"{language} warmup failed: "
                        f"{completed.stderr.decode(errors='replace')}"
                    )

        def measure(measured_command: list[str]) -> list[float]:
            samples = []
            for _ in range(args.iterations):
                started = time.perf_counter()
                completed = subprocess.run(
                    measured_command,
                    cwd=work,
                    env=environment(),
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.PIPE,
                    timeout=args.timeout,
                    check=False,
                )
                samples.append((time.perf_counter() - started) * 1000)
                if completed.returncode != 0:
                    raise RuntimeError(
                        f"{language} benchmark failed: "
                        f"{completed.stderr.decode(errors='replace')}"
                    )
            return samples

        startup_samples = measure(startup_command)
        process_samples = measure(command)
        startup_median = statistics.median(startup_samples)
        process_median = statistics.median(process_samples)

        source = source_path(language)
        artifact = artifact_path(language)
        rows.append(
            {
                "language": language,
                "tasks": args.tasks,
                "iterations": args.iterations,
                "process_median_ms": process_median,
                "process_min_ms": min(process_samples),
                "process_max_ms": max(process_samples),
                "process_samples_ms": process_samples,
                "startup_median_ms": startup_median,
                "startup_samples_ms": startup_samples,
                "estimated_work_median_ms": max(0.0, process_median - startup_median),
                "source_bytes": source.stat().st_size,
                "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                "artifact_bytes": artifact.stat().st_size,
                "artifact": str(artifact.relative_to(ROOT)),
            }
        )

    print(
        f"{'language':<12} {'process ms':>12} {'startup ms':>12} {'delta ms':>12} "
        f"{'source B':>12} {'artifact B':>12}"
    )
    for row in rows:
        print(
            f"{row['language']:<12} {row['process_median_ms']:>12.3f} "
            f"{row['startup_median_ms']:>12.3f} "
            f"{row['estimated_work_median_ms']:>12.3f} "
            f"{row['source_bytes']:>12} "
            f"{row['artifact_bytes']:>12}"
        )

    report = {
        "warning": (
            "Process time includes runtime startup. Delta is process median minus "
            "help-command startup median and is only an approximate workload cost. "
            "These are toolchain metrics, not evidence of LLM one-shot correctness."
        ),
        "rows": rows,
    }
    if args.json:
        destination = Path(args.json).resolve()
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return 0


def argument_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    for name in ("build", "test", "benchmark"):
        action = actions.add_parser(name)
        action.add_argument("--language", choices=("all",) + LANGUAGES, default="all")
        if name != "build":
            action.add_argument("--no-build", action="store_true")
            action.add_argument("--timeout", type=float, default=15.0)
        if name == "test":
            action.add_argument("--json")
        if name == "benchmark":
            action.add_argument("--tasks", type=int, default=1000)
            action.add_argument("--warmup", type=int, default=2)
            action.add_argument("--iterations", type=int, default=10)
            action.add_argument("--json")
    return parser


def main() -> int:
    args = argument_parser().parse_args()
    try:
        if args.action == "build":
            return build_action(args)
        if args.action == "test":
            return test_action(args)
        if args.action == "benchmark":
            return benchmark_action(args)
        raise AssertionError(args.action)
    except (RuntimeError, OSError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
