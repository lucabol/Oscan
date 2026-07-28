"""Variant-aware release smoke-test regressions.

The smoke scripts have two halves and both are covered here:

* what a package *is* — delegated by both scripts to
  ``release_tools.py verify-package-layout``, which is exercised directly
  against real staged fixture packages (and against packages deliberately
  broken in one way each); and
* how the scripts are *driven* — the mandatory backend, archives rather
  than directories, and the environment scrubbing that makes a passing
  smoke test mean "this works from the package alone".

Everything here is hermetic: fixture packages are staged from tiny fake
inputs and nothing in them is ever executed.
"""

from pathlib import Path
import json
import os
import re
import shutil
import subprocess
import tempfile
import unittest

import release_tools as rt
import test_release_packaging as packaging_fixtures


REPO_ROOT = Path(__file__).resolve().parent.parent
SMOKE_POWERSHELL = REPO_ROOT / "scripts" / "smoke-release.ps1"
SMOKE_SHELL = REPO_ROOT / "scripts" / "smoke-release.sh"
BUILD_MSI = REPO_ROOT / "scripts" / "build-msi.ps1"
WXS = REPO_ROOT / "packaging" / "windows" / "oscan.wxs"
STANDARD_USER_HELPER = REPO_ROOT / "scripts" / "windows-standard-user.ps1"

POWERSHELL = shutil.which("pwsh")
# `bash` on Windows is usually WSL's, which cannot open the Windows paths
# this test would hand it; the shell smoke test targets Linux/macOS anyway.
SHELL = None if os.name == "nt" else (shutil.which("sh") or shutil.which("bash"))


def stage_fixture_package(tmp: Path, target: str, backend: str) -> tuple[Path, Path, Path]:
    """Stage one real variant package from tiny offline inputs.

    Returns (bundle directory, archive path, contract path).
    """
    fixture = packaging_fixtures.PackagingFixture(tmp / f"{target}-{backend}-input", target)
    output_dir = tmp / f"{target}-{backend}-out"
    rt.stage_release(packaging_fixtures.stage_namespace(fixture, backend, output_dir))
    contract = rt.load_release_contract(fixture.contract_path)
    suffix = rt.ARCHIVE_SUFFIXES[contract["variants"][target]["archive_format"]]
    bundle = output_dir / "stage" / f"oscan-v9.9.9-{target}-{backend}"
    archive = output_dir / f"oscan-v9.9.9-{target}-{backend}{suffix}"
    return bundle, archive, fixture.contract_path


class PackageLayoutVerificationTests(unittest.TestCase):
    """The assertion both smoke scripts share."""

    def verify(self, bundle: Path, contract: Path, target: str, backend: str, **kwargs):
        return rt.verify_package_layout(
            bundle, contract, target, backend, version="9.9.9", **kwargs
        )

    def test_every_published_variant_verifies(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            for target, backend in packaging_fixtures.ALL_VARIANTS:
                with self.subTest(target=target, backend=backend):
                    bundle, archive, contract = stage_fixture_package(tmp, target, backend)
                    metadata = self.verify(
                        bundle,
                        contract,
                        target,
                        backend,
                        archive=archive,
                        expect_archive_root_name=True,
                    )
                    self.assertEqual(metadata["backend"], backend)
                    self.assertEqual(metadata["default_backend"], backend)
                    self.assertEqual(metadata["available_backends"], [backend])

    def test_the_command_line_entry_point_verifies_a_real_package(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            bundle, archive, contract = stage_fixture_package(tmp, "linux-x86_64", "llvm")
            result = subprocess.run(
                [
                    "python",
                    str(REPO_ROOT / "scripts" / "release_tools.py"),
                    "verify-package-layout",
                    "--target", "linux-x86_64",
                    "--backend", "llvm",
                    "--root", str(bundle),
                    "--stage", "extracted",
                    "--archive", str(archive),
                    "--version", "9.9.9",
                    "--contract", str(contract),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("linux-x86_64/llvm extracted package layout OK", result.stdout)

    def _reject(self, target: str, backend: str, break_package, **kwargs) -> str:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            bundle, archive, contract = stage_fixture_package(tmp, target, backend)
            break_package(bundle)
            with self.assertRaises(SystemExit) as caught:
                self.verify(bundle, contract, target, backend, archive=archive, **kwargs)
            return str(caught.exception)

    def test_a_missing_sidecar_is_refused(self) -> None:
        def remove_sidecar(bundle: Path) -> None:
            shutil.rmtree(bundle / "native-link")

        self.assertIn(
            "direct_link_sidecar",
            self._reject("windows-x86_64", "cranelift", remove_sidecar),
        )

    def test_a_tampered_sidecar_asset_is_refused(self) -> None:
        def tamper(bundle: Path) -> None:
            manifest = json.loads(
                (bundle / "native-link" / "native-link-assets.json").read_text(encoding="utf-8")
            )
            asset = bundle / "native-link" / manifest["linker"]["install_subpath"]
            asset.write_bytes(b"a different linker")

        self.assertIn(
            "digest mismatch", self._reject("linux-x86_64", "cranelift", tamper)
        )

    def test_a_missing_runtime_archive_is_refused(self) -> None:
        def remove_profile(bundle: Path) -> None:
            (
                bundle
                / "build"
                / "runtime-archives"
                / "linux-x86_64"
                / "libosc_runtime_freestanding_gfx.a"
            ).unlink()

        self.assertIn(
            "libosc_runtime_freestanding_gfx.a",
            self._reject("linux-x86_64", "llvm", remove_profile),
        )

    def test_an_undeclared_runtime_archive_is_refused(self) -> None:
        def add_hosted(bundle: Path) -> None:
            hosted = (
                bundle / "build" / "runtime-archives" / "linux-x86_64" / "libosc_runtime_hosted.a"
            )
            hosted.write_bytes(b"hosted archive")

        self.assertIn(
            "does not declare", self._reject("linux-x86_64", "cranelift", add_hosted)
        )

    def test_a_c_compiler_smuggled_into_an_object_package_is_refused(self) -> None:
        def plant_compiler(bundle: Path) -> None:
            (bundle / "native-link" / "linker").mkdir(parents=True, exist_ok=True)
            (bundle / "native-link" / "linker" / "gcc").write_bytes(b"not really gcc")

        self.assertIn(
            "C compiler executable",
            self._reject("linux-x86_64", "cranelift", plant_compiler),
        )

    def test_a_toolchain_directory_in_a_cranelift_package_is_refused(self) -> None:
        def plant_toolchain(bundle: Path) -> None:
            (bundle / "toolchain" / "bin").mkdir(parents=True)
            (bundle / "toolchain" / "bin" / "something").write_bytes(b"payload")

        self.assertIn(
            "declares no toolchain payload",
            self._reject("windows-x86_64", "cranelift", plant_toolchain),
        )

    def test_object_payload_in_a_c_package_is_refused(self) -> None:
        def plant_sidecar(bundle: Path) -> None:
            (bundle / "native-link").mkdir()
            (bundle / "native-link" / "native-link-assets.json").write_text("{}", encoding="utf-8")

        self.assertIn(
            "native-link", self._reject("linux-x86_64", "c", plant_sidecar)
        )

    def test_metadata_that_drifts_from_the_contract_is_refused(self) -> None:
        def rewrite_metadata(bundle: Path) -> None:
            path = bundle / rt.PACKAGE_METADATA_NAME
            metadata = json.loads(path.read_text(encoding="utf-8"))
            metadata["available_backends"] = ["llvm", "cranelift", "c"]
            path.write_text(json.dumps(metadata), encoding="utf-8")

        message = self._reject("windows-x86_64", "llvm", rewrite_metadata)
        self.assertIn("available_backends", message)

    def test_a_missing_compiler_binary_is_refused(self) -> None:
        def remove_binary(bundle: Path) -> None:
            (bundle / "oscan").unlink()

        self.assertIn(
            "missing its compiler binary",
            self._reject("macos-x86_64", "c", remove_binary),
        )

    def test_an_archive_named_off_contract_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            bundle, archive, contract = stage_fixture_package(tmp, "macos-x86_64", "c")
            renamed = archive.with_name("oscan-latest-macos.tar.gz")
            archive.rename(renamed)
            with self.assertRaises(SystemExit) as caught:
                self.verify(bundle, contract, "macos-x86_64", "c", archive=renamed)
            self.assertIn("is not the contract name", str(caught.exception))

    def test_the_extracted_stage_requires_the_contract_archive_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            bundle, archive, contract = stage_fixture_package(tmp, "linux-x86_64", "c")
            renamed = bundle.with_name("oscan")
            bundle.rename(renamed)
            with self.assertRaises(SystemExit) as caught:
                self.verify(
                    renamed,
                    contract,
                    "linux-x86_64",
                    "c",
                    archive=archive,
                    expect_archive_root_name=True,
                )
            self.assertIn("is not the contract archive root", str(caught.exception))
            # The same directory is a valid *installed* package: install
            # directories are named by the person installing them.
            self.verify(renamed, contract, "linux-x86_64", "c", archive=archive)


class PowerShellSmokeInterfaceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.script = SMOKE_POWERSHELL.read_text(encoding="utf-8")

    def test_the_backend_is_mandatory_and_canonical(self) -> None:
        self.assertIn('[ValidateSet("llvm", "cranelift", "c")]', self.script)
        self.assertRegex(
            self.script,
            r"\[Parameter\(Mandatory = \$true\)\]\s*\n\s*"
            r'\[ValidateSet\("llvm", "cranelift", "c"\)\]\s*\n\s*\[string\]\$Backend',
        )

    def test_it_takes_an_archive_and_refuses_a_directory(self) -> None:
        self.assertIn("-ArchivePath must name a packaged release archive file", self.script)
        self.assertIn("-PathType Leaf", self.script)

    def test_the_layout_is_verified_after_extraction_and_after_install(self) -> None:
        stages = re.findall(r'"--stage", "(\w+)"', self.script)
        self.assertEqual(stages, ["extracted", "installed"])
        self.assertEqual(self.script.count('"verify-package-layout",'), 2)

    def test_packaged_object_builds_get_no_runtime_archive_override(self) -> None:
        # The package ships its runtime archives at a fixed
        # executable-relative location; setting the override would hide a
        # regression in that lookup.
        self.assertNotIn("$env:OSCAN_RUNTIME_ARCHIVE_DIR =", self.script)
        self.assertIn('"OSCAN_RUNTIME_ARCHIVE_DIR",', self.script)

    def test_every_provider_and_toolchain_override_is_scrubbed(self) -> None:
        for name in (
            "OSCAN_LLVM_LIB",
            "OSCAN_LLVM_DIR",
            "OSCAN_TOOLCHAIN_DIR",
            "OSCAN_CC",
            "OSCAN_NATIVE_LINKER",
            "OSCAN_NATIVE_LINKER_FLAVOR",
            "OSCAN_NATIVE_ASSET_CACHE_DIR",
            "OSCAN_RUNTIME_ARCHIVE_DIR",
            "OSCAN_RUNTIME_BUILDER",
        ):
            with self.subTest(name=name):
                self.assertIn(f'"{name}",', self.script)

    def test_object_packages_are_smoked_under_the_strict_profile(self) -> None:
        self.assertIn('$env:OSCAN_NO_TOOLCHAIN = "1"', self.script)
        self.assertRegex(self.script, r"-NoToolchainProfile -BlockHostTools")

    def test_no_host_tool_bodies_go_through_the_isolating_helper(self) -> None:
        # The blocked-tools PATH is computed in one place, so the Windows
        # isolation cannot be bypassed by a second, ad-hoc "prepend" here.
        self.assertIn(
            "$env:PATH = Get-NoHostToolPath -BlockDir $BlockedHostToolDir -SavedPath $savedPath",
            self.script,
        )
        self.assertNotIn('$env:PATH = "$BlockedHostToolDir', self.script)

    def test_windows_compiles_run_the_installed_executable_not_the_shim(self) -> None:
        # An isolated PATH leaves no interpreter for a .cmd shim, and a
        # packaged compile must not need one; the shim is still covered by
        # the --version check, which runs with the ordinary PATH.
        self.assertIn("$OscanCommand = Join-Path $InstallDir $binaryName", self.script)
        self.assertIn('$OscanShim = Join-Path $BinDir "oscan.cmd"', self.script)
        self.assertIn("$versionText = (& $OscanShim --version", self.script)
        self.assertIn("& $script:OscanCommand @Arguments", self.script)

    def test_it_asserts_the_selected_backend_linker_and_provider(self) -> None:
        self.assertIn(r"^\[verbose\] $Backend backend target:", self.script)
        self.assertIn(r"^\[verbose\] LLVM code generator: ", self.script)
        # The native-link assertion checks *which* sidecar was used, so a
        # generic "the word sidecar appears" match must not creep back in.
        self.assertNotIn(r"^\[verbose\] native-link assets: sidecar \(", self.script)
        self.assertEqual(self.script.count("Assert-PackagedSidecarAssets -LogPath"), 2)
        self.assertIn("-SidecarRoot (Join-Path $InstallDir $sidecarDirName)", self.script)

    def test_it_asserts_the_version_metadata_block(self) -> None:
        for line in ("backends", "default-backend", "distribution", "toolchain-free"):
            with self.subTest(line=line):
                self.assertIn(f"(?m)^{line}: ", self.script)

    def test_the_deprecated_alias_is_tested_but_never_canonical(self) -> None:
        self.assertIn('"--backend", "native"', self.script)
        self.assertIn("'--backend native' is deprecated; use '--backend cranelift'", self.script)
        # The alias is never used as a package label.
        self.assertNotRegex(self.script, r"-Backend\s+['\"]?native")

    def test_success_clears_expected_negative_probe_exit_codes(self) -> None:
        self.assertTrue(
            self.script.rstrip().endswith("$global:LASTEXITCODE = 0"),
            "a successful smoke must not leak an expected refusal's exit code",
        )

    def test_object_packages_refuse_what_they_do_not_contain(self) -> None:
        for expectation in (
            "the c backend is not included in this compiler build",
            "refuses --libc",
            "refuses --extra-c",
            "archive name ends in '-c'",
        ):
            with self.subTest(expectation=expectation):
                self.assertIn(expectation, self.script)
        self.assertIn("fell back to a C compiler instead of refusing", self.script)

    def test_c_packages_assert_their_compiler_source(self) -> None:
        self.assertIn('Pattern "Compiling with .+ \\($expectedCompilerSource"', self.script)
        self.assertIn('$expectedCompilerSource = if ($requiresHostCompiler) { "host" } else { "bundled" }', self.script)

    def test_the_windows_elevated_opt_in_is_preserved(self) -> None:
        self.assertIn("WindowsBuiltInRole]::Administrator", self.script)
        self.assertIn('@("--allow-elevated-native-link")', self.script)

    def test_windows_object_packages_still_check_freestanding_imports(self) -> None:
        self.assertIn("KERNEL32\\.dll", self.script)
        self.assertIn("msvcrt|ucrt|vcruntime|api-ms-win-crt", self.script)

    @unittest.skipIf(POWERSHELL is None, "pwsh is not available in this environment")
    def test_the_script_parses(self) -> None:
        command = (
            "$errors = $null; $tokens = $null; "
            "[System.Management.Automation.Language.Parser]::ParseFile("
            f"'{SMOKE_POWERSHELL.as_posix()}', [ref]$tokens, [ref]$errors) | Out-Null; "
            "if ($errors) { $errors | ForEach-Object { $_.Message }; exit 1 }"
        )
        result = subprocess.run(
            [POWERSHELL, "-NoProfile", "-Command", command],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


@unittest.skipIf(POWERSHELL is None, "pwsh is not available in this environment")
class PowerShellSmokeHelperBehaviourTests(unittest.TestCase):
    """The two rules a review can't check by reading strings: what PATH a
    no-host-tool body runs with, and which sidecar directory counts."""

    EXTRACT_HELPERS = """
param([string]$Source, [string]$Destination)
$ErrorActionPreference = 'Stop'
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Source, [ref]$null, [ref]$null)
$functions = $ast.FindAll(
    { param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] },
    $false
)
(($functions | ForEach-Object { $_.Extent.Text }) -join "`n`n") |
    Set-Content -LiteralPath $Destination
"""

    NO_HOST_TOOL_PATH = """
param([string]$Helpers, [string]$BlockDir, [string]$SavedPath, [string]$OsValue)
$ErrorActionPreference = 'Stop'
. $Helpers
$env:OS = $OsValue
Write-Output (Get-NoHostToolPath -BlockDir $BlockDir -SavedPath $SavedPath)
"""

    SIDECAR_CASES = """
param([string]$Helpers, [string]$Root)
$ErrorActionPreference = 'Stop'
. $Helpers

$sidecar = Join-Path $Root 'install/native-link'
$elsewhere = Join-Path $Root 'elsewhere/native-link'
$bare = Join-Path $Root 'bare/native-link'
foreach ($dir in @($sidecar, $elsewhere)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $dir 'native-link-assets.json') -Value '{}'
}
New-Item -ItemType Directory -Path $bare -Force | Out-Null

function Invoke-CaseAgainst {
    param([string]$Name, [string]$Line, [string]$Expected)
    $log = Join-Path $Root "$Name.log"
    Set-Content -LiteralPath $log -Value $Line
    try {
        Assert-PackagedSidecarAssets -LogPath $log -SidecarRoot $Expected `
            -ManifestName 'native-link-assets.json' -What 'case'
        Write-Output "$Name PASS"
    } catch {
        Write-Output "$Name THROW"
    }
}

Invoke-CaseAgainst 'installed-sidecar' "[verbose] native-link assets: sidecar ($sidecar)" $sidecar
Invoke-CaseAgainst 'verbatim-path' "[verbose] native-link assets: sidecar (\\\\?\\$sidecar)" $sidecar
Invoke-CaseAgainst 'embedded-source' "[verbose] native-link assets: embedded ($sidecar)" $sidecar
Invoke-CaseAgainst 'relative-path' "[verbose] native-link assets: sidecar (native-link)" $sidecar
Invoke-CaseAgainst 'other-directory' "[verbose] native-link assets: sidecar ($elsewhere)" $sidecar
Invoke-CaseAgainst 'no-manifest' "[verbose] native-link assets: sidecar ($bare)" $bare
Invoke-CaseAgainst 'no-report' "[verbose] llvm backend target: x86_64" $sidecar
"""

    def run_powershell(self, tmp: Path, name: str, body: str, arguments: list[str]) -> str:
        script = tmp / f"{name}.ps1"
        script.write_text(body, encoding="utf-8")
        result = subprocess.run(
            [POWERSHELL, "-NoProfile", "-File", str(script)] + arguments,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout

    def extract_helpers(self, tmp: Path) -> Path:
        helpers = tmp / "helpers.ps1"
        self.run_powershell(
            tmp,
            "extract",
            self.EXTRACT_HELPERS,
            [str(SMOKE_POWERSHELL), str(helpers)],
        )
        self.assertIn("function Get-NoHostToolPath", helpers.read_text(encoding="utf-8"))
        return helpers

    def test_windows_no_host_tool_path_is_the_blocker_directory_alone(self) -> None:
        # A `.cmd` stub cannot shadow a host `gcc.exe`, so on Windows the
        # real PATH must not survive into a no-host-tool body at all.
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            helpers = self.extract_helpers(tmp)
            saved_path = "/opt/host-tools" + os.pathsep + "/usr/bin"
            resolved = self.run_powershell(
                tmp,
                "windows-path",
                self.NO_HOST_TOOL_PATH,
                [str(helpers), "/blocked-host-tools", saved_path, "Windows_NT"],
            ).strip()
            self.assertEqual(resolved, "/blocked-host-tools")
            self.assertNotIn("/opt/host-tools", resolved)
            self.assertNotIn("/usr/bin", resolved)

    def test_posix_no_host_tool_path_keeps_the_stub_directory_first(self) -> None:
        # POSIX stubs are real executables and do shadow the host tools, so
        # ordinary utilities stay reachable behind them.
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            helpers = self.extract_helpers(tmp)
            saved_path = "/opt/host-tools" + os.pathsep + "/usr/bin"
            resolved = self.run_powershell(
                tmp,
                "posix-path",
                self.NO_HOST_TOOL_PATH,
                [str(helpers), "/blocked-host-tools", saved_path, "Linux"],
            ).strip()
            self.assertTrue(
                resolved.startswith("/blocked-host-tools" + os.pathsep), resolved
            )
            self.assertIn(saved_path, resolved)

    def test_only_the_installed_packages_sidecar_satisfies_the_assertion(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            helpers = self.extract_helpers(tmp)
            output = self.run_powershell(
                tmp, "sidecar-cases", self.SIDECAR_CASES, [str(helpers), str(tmp)]
            )
            results = dict(
                line.split(" ", 1) for line in output.splitlines() if " " in line
            )
            expected = {
                # The installed package's own sidecar, however it is spelled.
                "installed-sidecar": "PASS",
                "verbatim-path": "PASS",
                # Anything else is a package that did not work from itself.
                "embedded-source": "THROW",
                "relative-path": "THROW",
                "other-directory": "THROW",
                "no-manifest": "THROW",
                "no-report": "THROW",
            }
            self.assertEqual(results, expected)


class ShellSmokeInterfaceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.script = SMOKE_SHELL.read_text(encoding="utf-8")

    def test_it_keeps_the_same_contract_driven_interface(self) -> None:
        for flag in ("--target", "--backend", "--archive", "--version", "--contract"):
            with self.subTest(flag=flag):
                self.assertIn(flag, self.script)
        stages = re.findall(r"--stage (\w+)", self.script)
        self.assertEqual(stages, ["extracted", "installed"])

    def test_it_scrubs_the_same_overrides(self) -> None:
        for name in (
            "OSCAN_NO_TOOLCHAIN",
            "OSCAN_CC",
            "OSCAN_TOOLCHAIN_DIR",
            "OSCAN_LLVM_LIB",
            "OSCAN_LLVM_DIR",
            "OSCAN_NATIVE_LINKER",
            "OSCAN_RUNTIME_ARCHIVE_DIR",
        ):
            with self.subTest(name=name):
                self.assertIn(f"-u {name}", self.script)
        self.assertNotIn("OSCAN_RUNTIME_ARCHIVE_DIR=", self.script)

    def test_it_asserts_the_same_backend_facts(self) -> None:
        self.assertIn("^backends: $BACKEND\\$", self.script)
        self.assertIn("^distribution: $BACKEND\\$", self.script)
        self.assertIn("^toolchain-free: $EXPECTED_TOOLCHAIN_FREE\\$", self.script)
        self.assertIn("native-link assets: sidecar", self.script)
        self.assertIn("the c backend is not included in this compiler build", self.script)

    @unittest.skipIf(SHELL is None, "no POSIX shell is available in this environment")
    def test_the_script_is_syntactically_valid(self) -> None:
        result = subprocess.run(
            [SHELL, "-n", str(SMOKE_SHELL)], capture_output=True, text=True
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    @unittest.skipIf(SHELL is None, "no POSIX shell is available in this environment")
    def test_it_refuses_to_run_without_a_backend(self) -> None:
        result = subprocess.run(
            [SHELL, str(SMOKE_SHELL), "--target", "linux-x86_64", "--archive", "x.tar.xz"],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing --backend", result.stderr)

    @unittest.skipIf(SHELL is None, "no POSIX shell is available in this environment")
    def test_it_refuses_the_deprecated_alias_as_a_package_label(self) -> None:
        result = subprocess.run(
            [
                SHELL, str(SMOKE_SHELL),
                "--target", "linux-x86_64",
                "--backend", "native",
                "--archive", "x.tar.xz",
            ],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("never a package label", result.stderr)

    @unittest.skipIf(SHELL is None, "no POSIX shell is available in this environment")
    def test_it_refuses_a_directory_instead_of_an_archive(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            result = subprocess.run(
                [
                    SHELL, str(SMOKE_SHELL),
                    "--target", "linux-x86_64",
                    "--backend", "llvm",
                    "--archive", tmp_name,
                ],
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("must name a packaged release archive file", result.stderr)


class MsiHarvestTests(unittest.TestCase):
    """The Windows installer is cut from an object package now, so the
    harvester may not assume the payload is a `toolchain/` directory."""

    def test_the_wix_source_and_harvester_agree_on_the_component_group(self) -> None:
        wxs = WXS.read_text(encoding="utf-8")
        harvester = BUILD_MSI.read_text(encoding="utf-8")
        self.assertIn('<ComponentGroupRef Id="BundlePayload" />', wxs)
        self.assertIn('ComponentGroup Id=`"BundlePayload`"', harvester)
        self.assertNotIn("ToolchainFiles", wxs)
        self.assertNotIn("ToolchainFiles", harvester)

    @unittest.skipIf(POWERSHELL is None, "pwsh is not available in this environment")
    def test_an_object_bundle_harvests_its_whole_payload(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            bundle, _, _ = stage_fixture_package(tmp, "windows-x86_64", "llvm")
            harvest = tmp / "harvest.wxs"
            result = subprocess.run(
                [
                    POWERSHELL, "-NoProfile", "-File", str(BUILD_MSI),
                    "-BundleDir", str(bundle),
                    "-HarvestOnly",
                    "-HarvestPath", str(harvest),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            fragment = harvest.read_text(encoding="utf-8")
            self.assertIn('Name="build"', fragment)
            self.assertIn('Name="runtime-archives"', fragment)
            self.assertIn('Name="native-link"', fragment)
            self.assertIn("native-link-assets.json", fragment)
            self.assertIn("libosc_runtime_freestanding.a", fragment)
            self.assertIn("oscan-package.json", fragment)
            self.assertIn('<ComponentGroup Id="BundlePayload">', fragment)
            # Files the .wxs declares itself, and the archive's own installer,
            # must not be harvested twice / at all.
            self.assertNotIn("README-install.txt", fragment)
            self.assertNotIn("install.ps1", fragment)
            self.assertNotRegex(fragment, r'Source="\$\(var\.BundleDir\)\\oscan\.exe"')


class StandardUserHelperTests(unittest.TestCase):
    def test_standard_user_launch_loads_new_account_profile(self) -> None:
        helper = STANDARD_USER_HELPER.read_text(encoding="utf-8")
        launch = re.search(
            r"\$process\s*=\s*Start-Process(?P<body>.*?)-PassThru",
            helper,
            flags=re.DOTALL,
        )
        self.assertIsNotNone(launch)
        self.assertRegex(
            launch.group("body"),
            r"-Credential\s+\$credential\s+`\s*"
            r"-LoadUserProfile\s+`\s*"
            r"-WorkingDirectory\s+\$working",
        )

    def test_standard_user_invocation_preserves_named_parameters(self) -> None:
        helper = STANDARD_USER_HELPER.read_text(encoding="utf-8")
        self.assertIn("[hashtable]$Parameters", helper)
        self.assertIn("parameters = $Parameters", helper)
        self.assertRegex(
            helper,
            r"foreach \(\$property in \$payload\.parameters\.PSObject\.Properties\)"
            r"(?s:.*?)"
            r"& \(\[string\]\$payload\.script_path\) @scriptParameters",
        )
        self.assertNotIn("@scriptArguments", helper)


if __name__ == "__main__":
    unittest.main()
