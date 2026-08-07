"""Release/CI workflow regressions for release contract schema 3.

Two kinds of check live here:

* behaviour — the package matrix the release workflow consumes is produced
  by ``release_tools.ci_target_matrix``, so it is tested directly against
  the real contract (and against mutated contracts) rather than by reading
  YAML; and
* wiring — the workflow really does feed that matrix into per-target jobs
  that build the exact Cargo features each variant declares, prepare the
  strict Windows LLVM inputs, and smoke every archive it uploads.
"""

from pathlib import Path
import json
import re
import unittest

import release_tools as rt

try:  # Structural YAML checks run wherever PyYAML happens to be available.
    import yaml
except ImportError:  # pragma: no cover - depends on the environment
    yaml = None


REPO_ROOT = Path(__file__).resolve().parent.parent
RELEASE_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "release.yml"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
INPROCESS_BUILD_SCRIPT = REPO_ROOT / "scripts" / "build-inprocess-toolchain.ps1"
RELEASE_POWERSHELL_SCRIPTS = (
    REPO_ROOT / "scripts" / "assemble-release.ps1",
    REPO_ROOT / "scripts" / "build-runtime-archive.ps1",
    REPO_ROOT / "scripts" / "smoke-release.ps1",
    REPO_ROOT / "scripts" / "stage-release.ps1",
)
WINDOWS_STYLE_RELEASE_PATH = re.compile(
    r"(?:\./)?(?:scripts|packaging|build|runtime-archives|runtime-toolchains)"
    r"\\[^\s\"']*|\$sidecarBase\\[^\s\"']*"
)


def release_workflow_text() -> str:
    return RELEASE_WORKFLOW.read_text(encoding="utf-8")


def ci_workflow_text() -> str:
    return CI_WORKFLOW.read_text(encoding="utf-8")


class PackageMatrixTests(unittest.TestCase):
    """The matrix the release workflow fans out over."""

    def setUp(self) -> None:
        self.contract = rt.load_release_contract(rt.CONTRACT_PATH)
        self.matrix = rt.ci_target_matrix(self.contract, "1.2.3")

    def test_one_job_per_target_not_per_target_and_profile(self) -> None:
        # Expensive per-target work must happen once, so the matrix is keyed
        # on the target and carries the profile list.
        self.assertEqual(
            [entry["target"] for entry in self.matrix],
            ["linux-x86_64", "macos-x86_64", "windows-x86_64"],
        )
        variants = rt.release_variant_matrix(self.contract)
        self.assertEqual(
            sum(len(entry["profiles"].split(",")) for entry in self.matrix),
            len(variants),
        )

    def test_profiles_are_listed_in_canonical_order(self) -> None:
        for entry in self.matrix:
            with self.subTest(target=entry["target"]):
                profiles = entry["profiles"].split(",")
                self.assertEqual(
                    profiles,
                    [name for name in rt.CANONICAL_PROFILES if name in profiles],
                )
                self.assertNotIn("native", profiles)

    def test_each_entry_carries_its_runner_and_binary(self) -> None:
        expected = {
            "linux-x86_64": ("ubuntu-latest", "oscan", "target/release/oscan"),
            "macos-x86_64": ("macos-15-intel", "oscan", "target/release/oscan"),
            "windows-x86_64": ("windows-latest", "oscan.exe", "target/release/oscan.exe"),
        }
        for entry in self.matrix:
            with self.subTest(target=entry["target"]):
                runner, binary_name, binary_path = expected[entry["target"]]
                self.assertEqual(entry["os"], runner)
                self.assertEqual(entry["binary_name"], binary_name)
                self.assertEqual(entry["binary_path"], binary_path)

    def test_prepared_inputs_follow_the_declared_components(self) -> None:
        by_target = {entry["target"]: entry for entry in self.matrix}
        linux = by_target["linux-x86_64"]
        windows = by_target["windows-x86_64"]
        macos = by_target["macos-x86_64"]

        for entry in (linux, windows):
            with self.subTest(target=entry["target"]):
                self.assertEqual(entry["needs_base_toolchain"], "true")
                self.assertEqual(entry["needs_native_link"], "true")
                self.assertEqual(
                    entry["runtime_profiles"].split(","),
                    list(rt.RUNTIME_PROFILES),
                )

        # Only Linux overlays a separately pinned provider archive; Windows
        # statically links LLVM into its strict compiler.
        self.assertEqual(linux["needs_provider_archive"], "true")
        self.assertEqual(windows["needs_provider_archive"], "false")
        self.assertEqual(linux["needs_inprocess_toolchain"], "false")
        self.assertEqual(windows["needs_inprocess_toolchain"], "true")

        # macOS is C-only and relies on the host Apple CLT: nothing to fetch.
        self.assertEqual(macos["profiles"], "c")
        self.assertEqual(macos["needs_base_toolchain"], "false")
        self.assertEqual(macos["needs_native_link"], "false")
        self.assertEqual(macos["needs_provider_archive"], "false")
        self.assertEqual(macos["needs_inprocess_toolchain"], "false")
        self.assertEqual(macos["runtime_profiles"], "")

    def test_archive_names_are_rendered_and_globally_unique(self) -> None:
        names = [name for entry in self.matrix for name in entry["archives"].split(",")]
        self.assertEqual(len(names), len(set(names)))
        self.assertIn("oscan-v1.2.3-windows-x86_64-llvm.zip", names)
        self.assertIn("oscan-v1.2.3-windows-x86_64-full.zip", names)
        self.assertIn("oscan-v1.2.3-linux-x86_64-full.tar.xz", names)
        self.assertIn("oscan-v1.2.3-linux-x86_64-cranelift.tar.xz", names)
        self.assertIn("oscan-v1.2.3-macos-x86_64-c.tar.gz", names)
        for name in names:
            self.assertNotIn("{version}", name)

    def test_exactly_one_recommended_msi_is_requested(self) -> None:
        msi = [entry for entry in self.matrix if entry["msi_profile"]]
        self.assertEqual([entry["target"] for entry in msi], ["windows-x86_64"])
        self.assertEqual(msi[0]["msi_profile"], "llvm")

    def test_a_contract_with_a_non_canonical_profile_is_refused(self) -> None:
        contract = json.loads(rt.CONTRACT_PATH.read_text(encoding="utf-8"))
        target = contract["variants"]["linux-x86_64"]["profiles"]
        target["native"] = target.pop("cranelift")
        with self.assertRaises(SystemExit) as caught:
            rt.ci_target_matrix(contract, "1.2.3")
        self.assertIn("non-canonical profile", str(caught.exception))

    def test_a_target_with_no_profiles_is_refused(self) -> None:
        contract = json.loads(rt.CONTRACT_PATH.read_text(encoding="utf-8"))
        contract["variants"]["macos-x86_64"]["profiles"] = {}
        with self.assertRaises(SystemExit) as caught:
            rt.ci_target_matrix(contract, "1.2.3")
        self.assertIn("declares no profiles", str(caught.exception))

    def test_colliding_archive_names_are_refused(self) -> None:
        contract = json.loads(rt.CONTRACT_PATH.read_text(encoding="utf-8"))
        windows = contract["variants"]["windows-x86_64"]["profiles"]
        windows["cranelift"]["archive_name_template"] = windows["llvm"][
            "archive_name_template"
        ]
        with self.assertRaises(SystemExit) as caught:
            rt.ci_target_matrix(contract, "1.2.3")
        self.assertIn("is produced by both", str(caught.exception))


class ReleaseWorkflowWiringTests(unittest.TestCase):
    def setUp(self) -> None:
        self.workflow = release_workflow_text()

    def test_the_matrix_comes_from_the_contract(self) -> None:
        self.assertIn("release_tools.py ci-matrix --version", self.workflow)
        self.assertIn("release_tools.py validate-contract", self.workflow)
        self.assertIn("fromJson(needs.prepare.outputs.matrix)", self.workflow)

    def test_every_profile_is_built_with_its_contract_features(self) -> None:
        self.assertIn("$features = @($variant['cargo_features'])", self.workflow)
        self.assertIn("$featureList = $features -join ','", self.workflow)
        self.assertIn(
            "cargo build --release --no-default-features --features $featureList",
            self.workflow,
        )
        self.assertIn("$env:OSCAN_DISTRIBUTION_PROFILE = $profile", self.workflow)
        self.assertIn(
            "$env:OSCAN_DEFAULT_BACKEND = [string]$profileSpec['default_backend']",
            self.workflow,
        )
        self.assertIn(
            "Remove-Item Env:OSCAN_DISTRIBUTION_BACKEND",
            self.workflow,
        )
        self.assertIn("$env:OSCAN_VERSION = $env:RELEASE_TAG", self.workflow)
        # Each build overwrites target/release, so the binary has to be moved
        # aside before the next backend is built.
        self.assertRegex(
            self.workflow,
            r"Copy-Item -LiteralPath \$env:RELEASE_BINARY_PATH\s+`?\s*"
            r"-Destination \(Join-Path \$destinationDir \$env:RELEASE_BINARY_NAME\)",
        )

    def test_sidecar_and_strict_build_inputs_are_kept_separate(self) -> None:
        self.assertNotRegex(self.workflow, r"OSCAN_EMBED_ASSETS_DIR:\s")
        self.assertNotRegex(self.workflow, r"OSCAN_REQUIRE_EMBEDDED_ASSETS:\s")
        self.assertIn(
            "Remove-Item Env:OSCAN_EMBED_ASSETS_DIR, Env:OSCAN_REQUIRE_EMBEDDED_ASSETS",
            self.workflow,
        )
        self.assertIn("$env:OSCAN_INPROCESS_TOOLCHAIN_DIR =", self.workflow)
        self.assertIn("$env:OSCAN_INPROCESS_ASSETS_DIR =", self.workflow)
        self.assertIn("$env:RUSTFLAGS = [string]$variant['rustflags']", self.workflow)
        self.assertIn(
            "Remove-Item Env:OSCAN_INPROCESS_TOOLCHAIN_DIR, Env:OSCAN_INPROCESS_ASSETS_DIR, Env:RUSTFLAGS",
            self.workflow,
        )

    def test_expensive_inputs_are_prepared_once_per_target(self) -> None:
        for command, expected in (
            ("resolve-archive.ps1", 4),  # base + provider + strict SDK/source
            ("fetch-toolchain.ps1", 1),
            ("prepare-embed-assets.ps1", 1),
            ("prepare-inprocess-link-assets", 1),
            ("prepare-inprocess-notices", 1),
            ("build-inprocess-toolchain.ps1", 2),  # cache key + invocation
            ("build-runtime-archive.ps1", 1),  # in a loop over the profiles
        ):
            with self.subTest(command=command):
                self.assertEqual(self.workflow.count(command), expected)
        self.assertIn("$env:RELEASE_RUNTIME_PROFILES.Split(',',", self.workflow)

    def test_inprocess_patch_is_scoped_to_nested_extracted_sources(self) -> None:
        script = INPROCESS_BUILD_SCRIPT.read_text(encoding="utf-8")
        self.assertIn("[IO.Path]::GetRelativePath($sourceGitRoot, $source)", script)
        self.assertIn('"--directory=$patchDirectory"', script)

    def test_no_cross_target_toolchains_or_sidecars_are_packaged(self) -> None:
        for absent in (
            "linux-aarch64",
            "linux-riscv64",
            "cross-linker-sidecars",
            "CrossLinkerSidecarDir",
        ):
            with self.subTest(absent=absent):
                self.assertNotIn(absent, self.workflow)

    def test_staging_consumes_verified_archives_not_prepared_directories(self) -> None:
        self.assertIn("$assembleArgs['ToolchainArchive'] = $toolchainArchive", self.workflow)
        self.assertIn("$assembleArgs['LlvmProviderArchive'] = $providerArchive", self.workflow)
        self.assertNotIn("ToolchainDir =", self.workflow)
        self.assertNotIn("LlvmProviderDir", self.workflow)

    def test_every_archive_is_smoked_with_its_profile_before_upload(self) -> None:
        self.assertIn("./scripts/smoke-release.ps1", self.workflow)
        self.assertRegex(self.workflow, r"-Profile \$profile")
        self.assertRegex(self.workflow, r"-ArchivePath \$archivePath")
        self.assertRegex(self.workflow, r"-Version \$version")
        smoke_index = self.workflow.index("./scripts/smoke-release.ps1")
        upload_index = self.workflow.index("actions/upload-artifact@v4")
        self.assertLess(smoke_index, upload_index)

    def test_successful_package_step_clears_smoke_probe_exit_codes(self) -> None:
        success_index = self.workflow.index('Write-Host "Packaged and smoked:')
        reset_index = self.workflow.index("$LASTEXITCODE = 0", success_index)
        upload_index = self.workflow.index("actions/upload-artifact@v4")
        self.assertLess(success_index, reset_index)
        self.assertLess(reset_index, upload_index)

    def test_the_package_set_is_checked_against_the_contract(self) -> None:
        for message in (
            "Two $target packages claim the same archive name",
            "No packages were produced for $target.",
            "The release contract expects packages this job did not produce",
            "This job produced packages the release contract does not declare",
        ):
            with self.subTest(message=message):
                self.assertIn(message, self.workflow)

    def test_only_the_recommended_llvm_msi_is_built(self) -> None:
        self.assertIn("if: matrix.msi_profile != ''", self.workflow)
        self.assertIn("$profile = $env:RELEASE_MSI_PROFILE", self.workflow)
        self.assertIn("$msiPath = Join-Path $uploadDir \"$bundleName.msi\"", self.workflow)
        # Built from the bundle the assemble step staged and smoked.
        self.assertIn("The staged $target/$profile bundle is missing at $bundleDir", self.workflow)

    def test_the_installed_msi_payload_is_smoked_before_upload(self) -> None:
        self.assertIn("Start-Process -FilePath msiexec.exe", self.workflow)
        self.assertIn("'/a'", self.workflow)
        self.assertIn("'PFiles64\\Oscan'", self.workflow)
        self.assertIn("-InstalledPackageDir $installedRoot", self.workflow)
        build_index = self.workflow.index("& ./scripts/build-msi.ps1")
        extract_index = self.workflow.index("Start-Process -FilePath msiexec.exe")
        installed_smoke_index = self.workflow.index("-InstalledPackageDir $installedRoot")
        upload_index = self.workflow.index("actions/upload-artifact@v4")
        self.assertLess(build_index, extract_index)
        self.assertLess(extract_index, installed_smoke_index)
        self.assertLess(installed_smoke_index, upload_index)

    def test_publishing_still_collects_every_asset_kind(self) -> None:
        self.assertIn("'\\.(zip|msi|tar\\.gz|tar\\.xz)$'", self.workflow)
        self.assertIn("No packaged release assets were downloaded.", self.workflow)
        self.assertIn("needs.prepare.outputs.should_publish == 'true'", self.workflow)
        self.assertIn("tag_name: ${{ needs.prepare.outputs.tag }}", self.workflow)

    def test_workflow_paths_use_cross_platform_separators(self) -> None:
        self.assertEqual([], WINDOWS_STYLE_RELEASE_PATH.findall(self.workflow))

    def test_cross_platform_release_scripts_use_portable_paths(self) -> None:
        for script_path in RELEASE_POWERSHELL_SCRIPTS:
            with self.subTest(script=script_path.name):
                script = script_path.read_text(encoding="utf-8")
                self.assertEqual([], WINDOWS_STYLE_RELEASE_PATH.findall(script))


class CiWorkflowTests(unittest.TestCase):
    def setUp(self) -> None:
        self.workflow = ci_workflow_text()

    def test_single_backend_distribution_builds_are_first_class(self) -> None:
        self.assertIn("backend-distributions:", self.workflow)
        matrix = re.search(r"backend: \[(?P<backends>[^\]]+)\]", self.workflow)
        self.assertIsNotNone(matrix)
        self.assertEqual(
            [name.strip() for name in matrix.group("backends").split(",")],
            list(rt.CANONICAL_BACKENDS),
        )
        self.assertIn(
            "cargo test --no-default-features --features backend-${{ matrix.backend }}",
            self.workflow,
        )
        self.assertIn("OSCAN_DISTRIBUTION_BACKEND: ${{ matrix.backend }}", self.workflow)

    def test_the_all_backend_jobs_are_kept(self) -> None:
        for job in ("linux:", "windows:", "macos:", "release-tooling-tests:"):
            with self.subTest(job=job):
                self.assertIn(job, self.workflow)
        self.assertIn('python -m unittest discover -s scripts -p "test_*.py"', self.workflow)

    def test_the_strict_full_profile_is_tested_before_release(self) -> None:
        self.assertIn("full-distribution:", self.workflow)
        self.assertIn("OSCAN_DISTRIBUTION_PROFILE: full", self.workflow)
        self.assertIn("OSCAN_DEFAULT_BACKEND: llvm", self.workflow)
        self.assertIn('grep -qx "distribution: full"', self.workflow)

    def test_windows_runs_archive_installer_coexistence_tests(self) -> None:
        self.assertIn(
            "python -m unittest scripts.test_installers.WindowsInstallerCoexistenceTests",
            self.workflow,
        )


@unittest.skipIf(yaml is None, "PyYAML is not available in this environment")
class WorkflowStructureTests(unittest.TestCase):
    """Parse checks, so a workflow can never be merged unparseable."""

    def test_release_workflow_is_structurally_valid(self) -> None:
        workflow = yaml.safe_load(release_workflow_text())
        self.assertEqual(
            sorted(workflow["jobs"]), ["checksums", "package", "prepare", "publish"]
        )
        package = workflow["jobs"]["package"]
        self.assertEqual(package["strategy"]["matrix"], "${{ fromJson(needs.prepare.outputs.matrix) }}")
        self.assertFalse(package["strategy"]["fail-fast"])
        self.assertEqual(package["runs-on"], "${{ matrix.os }}")
        for name in ("RELEASE_TARGET", "RELEASE_PROFILES", "RELEASE_VERSION", "RELEASE_TAG"):
            self.assertIn(name, package["env"])
        self.assertEqual(
            workflow["jobs"]["publish"]["needs"], ["prepare", "package", "checksums"]
        )

    def test_ci_workflow_is_structurally_valid(self) -> None:
        workflow = yaml.safe_load(ci_workflow_text())
        job = workflow["jobs"]["backend-distributions"]
        self.assertEqual(job["strategy"]["matrix"]["backend"], list(rt.CANONICAL_BACKENDS))
        self.assertFalse(job["strategy"]["fail-fast"])
        full = workflow["jobs"]["full-distribution"]
        self.assertEqual(full["runs-on"], "ubuntu-latest")


if __name__ == "__main__":
    unittest.main()
