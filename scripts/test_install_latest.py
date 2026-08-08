"""Offline regressions for ``scripts/install-latest.ps1``.

The Windows quick installer exposes full and slim profiles as both archives and
independent MSI families. Two kinds of check live here:

* static — the script's parameters, defaults and asset-name templates are
  read out of its text, so the contract's canonical names stay the ones the
  installer asks for even where PowerShell is unavailable; and
* behaviour — where ``pwsh`` exists, the script is dot-sourced (which
  defines its functions without installing anything) and its selection
  function is driven against synthetic release asset lists.

Nothing here touches the network.
"""

from pathlib import Path
import json
import shutil
import subprocess
import tempfile
import unittest

import release_tools as rt


REPO_ROOT = Path(__file__).resolve().parent.parent
INSTALLER = REPO_ROOT / "scripts" / "install-latest.ps1"
POWERSHELL = shutil.which("pwsh")

WINDOWS_TARGET = "windows-x86_64"
TAG = "v1.2.3"


def installer_text() -> str:
    return INSTALLER.read_text(encoding="utf-8")


def contract_windows_archives() -> dict[str, str]:
    """The exact Windows archive name the contract renders per profile."""
    contract = rt.load_release_contract(rt.CONTRACT_PATH)
    profiles = contract["variants"][WINDOWS_TARGET]["profiles"]
    return {
        profile: spec["archive_name_template"].replace("{version}", TAG.lstrip("v"))
        for profile, spec in profiles.items()
    }


def run_selection(assets: list[str], profile: str, mode: str, tag: str = TAG) -> dict:
    """Dot-source the installer and run one asset selection offline."""
    asset_json = json.dumps([{"name": name} for name in assets])
    script = f"""
$ErrorActionPreference = 'Stop'
. '{INSTALLER.as_posix()}'
$assets = @('{asset_json.replace("'", "''")}' | ConvertFrom-Json)
try {{
    $selection = Select-OscanReleaseAsset -Assets $assets -Tag '{tag}' -Profile '{profile}' -Mode '{mode}'
    @{{ ok = $true; name = $selection.Asset.name; kind = $selection.Kind }} | ConvertTo-Json -Compress
}} catch {{
    @{{ ok = $false; error = $_.Exception.Message }} | ConvertTo-Json -Compress
}}
"""
    completed = subprocess.run(
        [POWERSHELL, "-NoProfile", "-NonInteractive", "-Command", script],
        capture_output=True,
        text=True,
        check=True,
        cwd=REPO_ROOT,
    )
    payload = [line for line in completed.stdout.splitlines() if line.strip().startswith("{")]
    if not payload:
        raise AssertionError(f"no JSON result from the installer: {completed.stdout!r}")
    return json.loads(payload[-1])


class InstallerContractTests(unittest.TestCase):
    """What the installer asks the release for, read from its own text."""

    def setUp(self) -> None:
        self.text = installer_text()

    def test_slim_llvm_remains_the_transition_default(self) -> None:
        self.assertRegex(self.text, r"\[string\]\$Profile\s*=\s*'slim'")
        self.assertRegex(self.text, r"\[string\]\$Backend\s*=\s*'llvm'")
        self.assertRegex(self.text, r"ValidateSet\('llvm',\s*'cranelift',\s*'c',\s*'native'\)")

    def test_the_zip_flow_stays_the_default_install_mode(self) -> None:
        self.assertRegex(self.text, r"\[string\]\$Mode\s*=\s*'zip'")

    def test_asset_names_are_rendered_exactly_like_the_release_contract(self) -> None:
        # One template, no globbing: 'oscan-<tag>-windows-x86_64-<profile>.<ext>'.
        self.assertIn('return "oscan-$Tag-$InstallerTarget-$Profile.$Kind"', self.text)
        self.assertIn("$InstallerTarget = 'windows-x86_64'", self.text)
        self.assertNotIn("'*windows-x86_64", self.text)

    def test_no_suffix_or_version_drift_matching_survives(self) -> None:
        # Exact-name comparison only: no EndsWith/StartsWith/-like/-match
        # fallback may creep back into asset selection.
        for smell in ("EndsWith(", "StartsWith('oscan-')", "-like", "$suffix"):
            with self.subTest(smell=smell):
                self.assertNotIn(smell, self.text)
        self.assertIn("$_.name -ieq $candidate.Name", self.text)

    def test_full_is_a_first_class_package_choice(self) -> None:
        self.assertRegex(self.text, r"ValidateSet\('slim',\s*'full'\)")
        self.assertIn("return 'full'", self.text)
        self.assertIn("Select LLVM, Cranelift, or C with --backend.", self.text)

    def test_every_profile_can_be_installed_from_an_msi(self) -> None:
        self.assertNotIn("$MsiProfile", self.text)
        self.assertIn("MSI mode is", self.text)
        self.assertIn("available for every profile", self.text)

    def test_archive_commands_and_legacy_msi_collisions_are_detected(self) -> None:
        self.assertRegex(self.text, r"\[switch\]\$SetDefault")
        self.assertIn("$installArgs['SetDefault']", self.text)
        self.assertIn("A per-user archive command is installed", self.text)
        self.assertIn('"oscan-$packageProfile.cmd"', self.text)
        self.assertIn('"Programs\\Oscan\\archive-profile-$packageProfile"', self.text)
        self.assertIn("The legacy flat Oscan LLVM MSI is still installed", self.text)
        self.assertIn("OSCAN_ALLOW_ARCHIVE_CONFLICT=1", self.text)

    def test_checksum_verification_and_tag_selection_are_preserved(self) -> None:
        self.assertIn("SHA256SUMS", self.text)
        self.assertIn("Checksum mismatch for", self.text)
        self.assertIn("Get-FileHash", self.text)
        self.assertIn("$ApiBase/tags/$tag", self.text)
        self.assertIn("$ApiBase/latest", self.text)
        self.assertRegex(self.text, r"\[switch\]\$SkipChecksum")

    def test_github_token_uses_a_supported_authorization_header(self) -> None:
        self.assertIn(
            '$headers[\'Authorization\'] = "Bearer $env:GITHUB_TOKEN"',
            self.text,
        )
        self.assertNotIn('"******"', self.text)

    def test_a_missing_checksum_file_fails_closed(self) -> None:
        self.assertIn("publishes no SHA256SUMS", self.text)
        # The old behaviour — warn and install anyway — must not return.
        self.assertNotIn("skipping verification", self.text)
        self.assertRegex(
            self.text, r"if \(-not \$sumsAsset\) \{\s*\n\s*throw "
        )

    def test_the_deprecated_native_spelling_is_only_a_compatibility_alias(self) -> None:
        self.assertIn("'-Backend native' is deprecated; use '-Backend cranelift'", self.text)
        # It is normalised before it can ever reach an asset name.
        self.assertIn("return 'cranelift'", self.text)

    def test_dot_sourcing_defines_the_functions_without_installing(self) -> None:
        self.assertIn("if ($MyInvocation.InvocationName -ne '.') {", self.text)

    def test_every_windows_contract_archive_is_reachable_by_profile(self) -> None:
        archives = contract_windows_archives()
        self.assertEqual(sorted(archives), ["c", "cranelift", "full", "llvm"])
        for profile, archive in archives.items():
            self.assertEqual(archive, f"oscan-{TAG}-{WINDOWS_TARGET}-{profile}.zip")


@unittest.skipUnless(POWERSHELL, "pwsh is required for installer behaviour tests")
class InstallerSelectionBehaviourTests(unittest.TestCase):
    """The selection function, driven against synthetic release assets."""

    def setUp(self) -> None:
        self.assets = [
            f"oscan-{TAG}-windows-x86_64-full.zip",
            f"oscan-{TAG}-windows-x86_64-full.msi",
            f"oscan-{TAG}-windows-x86_64-llvm.zip",
            f"oscan-{TAG}-windows-x86_64-llvm.msi",
            f"oscan-{TAG}-windows-x86_64-cranelift.zip",
            f"oscan-{TAG}-windows-x86_64-cranelift.msi",
            f"oscan-{TAG}-windows-x86_64-c.zip",
            f"oscan-{TAG}-windows-x86_64-c.msi",
            f"oscan-{TAG}-linux-x86_64-llvm.tar.xz",
            f"oscan-{TAG}-linux-x86_64-cranelift.tar.xz",
            f"oscan-{TAG}-macos-x86_64-c.tar.gz",
            "Source code (zip)",
            "SHA256SUMS",
        ]

    def test_the_default_backend_resolves_to_the_llvm_zip(self) -> None:
        result = run_selection(self.assets, "llvm", "zip")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["name"], f"oscan-{TAG}-windows-x86_64-llvm.zip")
        self.assertEqual(result["kind"], "zip")

    def test_each_profile_selects_its_own_exact_archive(self) -> None:
        for profile in ("full", "llvm", "cranelift", "c"):
            with self.subTest(profile=profile):
                result = run_selection(self.assets, profile, "zip")
                self.assertTrue(result["ok"], result)
                self.assertEqual(
                    result["name"], f"oscan-{TAG}-windows-x86_64-{profile}.zip"
                )

    def test_msi_mode_selects_each_profiles_exact_installer(self) -> None:
        for profile in ("full", "llvm", "cranelift", "c"):
            with self.subTest(profile=profile):
                result = run_selection(self.assets, profile, "msi")
                self.assertTrue(result["ok"], result)
                self.assertEqual(
                    result["name"],
                    f"oscan-{TAG}-windows-x86_64-{profile}.msi",
                )
                self.assertEqual(result["kind"], "msi")

    def test_msi_mode_falls_back_to_the_exact_same_profile_zip(self) -> None:
        assets = [name for name in self.assets if not name.endswith(".msi")]
        for profile in ("full", "llvm", "cranelift", "c"):
            with self.subTest(profile=profile):
                result = run_selection(assets, profile, "msi")
                self.assertTrue(result["ok"], result)
                self.assertEqual(
                    result["name"],
                    f"oscan-{TAG}-windows-x86_64-{profile}.zip",
                )
                self.assertEqual(result["kind"], "zip")

    def test_a_missing_backend_package_is_an_actionable_error(self) -> None:
        assets = [name for name in self.assets if "windows-x86_64-cranelift" not in name]
        result = run_selection(assets, "cranelift", "zip")
        self.assertFalse(result["ok"], result)
        self.assertIn(f"oscan-{TAG}-windows-x86_64-cranelift.zip", result["error"])
        self.assertIn(
            "full, llvm, cranelift, and c profiles as both ZIP archives and MSIs",
            result["error"],
        )

    def test_a_legacy_unsuffixed_archive_never_satisfies_a_profile_request(self) -> None:
        result = run_selection(
            [
                f"oscan-{TAG}-windows-x86_64.zip",
                f"oscan-{TAG}-windows-x86_64.msi",
                "SHA256SUMS",
            ],
            "llvm",
            "zip",
        )
        self.assertFalse(result["ok"], result)
        self.assertIn(f"oscan-{TAG}-windows-x86_64-llvm.zip", result["error"])

    def test_a_backend_request_never_matches_another_backend(self) -> None:
        # Only the C archive is published: llvm and cranelift must fail
        # rather than install the package that happens to be there.
        assets = [f"oscan-{TAG}-windows-x86_64-c.zip", "SHA256SUMS"]
        for backend in ("llvm", "cranelift"):
            with self.subTest(backend=backend):
                result = run_selection(assets, backend, "zip")
                self.assertFalse(result["ok"], result)

    def test_a_version_drifted_asset_name_is_rejected_not_guessed(self) -> None:
        # The asset carries a different version spelling than the release
        # tag. That is a broken release, not something to resolve by
        # suffix: the installer must refuse it.
        result = run_selection(
            ["oscan-v1.2.3-rc1-windows-x86_64-cranelift.zip", "SHA256SUMS"],
            "cranelift",
            "zip",
        )
        self.assertFalse(result["ok"], result)
        self.assertIn(f"oscan-{TAG}-windows-x86_64-cranelift.zip", result["error"])
        self.assertIn("derived from the release tag", result["error"])

    def test_msi_mode_never_drifts_to_a_differently_versioned_installer(self) -> None:
        # Only a drifted MSI and a drifted zip exist: neither is the exact
        # tag-derived name, so the request fails instead of falling back.
        result = run_selection(
            [
                "oscan-v1.2.3-rc1-windows-x86_64-llvm.msi",
                "oscan-v1.2.3-rc1-windows-x86_64-llvm.zip",
                "SHA256SUMS",
            ],
            "llvm",
            "msi",
        )
        self.assertFalse(result["ok"], result)
        self.assertIn(f"oscan-{TAG}-windows-x86_64-llvm.msi", result["error"])
        self.assertIn(f"oscan-{TAG}-windows-x86_64-llvm.zip", result["error"])

    def test_msi_mode_falls_back_only_to_the_exact_same_tag_zip(self) -> None:
        # The exact zip is present, a drifted MSI is not usable: the exact
        # zip is the only acceptable fallback.
        result = run_selection(
            [
                "oscan-v1.2.3-rc1-windows-x86_64-llvm.msi",
                f"oscan-{TAG}-windows-x86_64-llvm.zip",
                "SHA256SUMS",
            ],
            "llvm",
            "msi",
        )
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["name"], f"oscan-{TAG}-windows-x86_64-llvm.zip")
        self.assertEqual(result["kind"], "zip")

    def test_an_ambiguous_asset_list_is_refused_rather_than_guessed(self) -> None:
        result = run_selection(
            [
                f"oscan-{TAG}-windows-x86_64-llvm.zip",
                f"oscan-{TAG}-windows-x86_64-llvm.zip",
            ],
            "llvm",
            "zip",
        )
        self.assertFalse(result["ok"], result)
        self.assertIn("more than one", result["error"])

    def test_the_native_alias_resolves_to_the_cranelift_package(self) -> None:
        script = (
            f"$ErrorActionPreference='Stop'; . '{INSTALLER.as_posix()}'; "
            "(Resolve-OscanBackend 'native') 3>&1 | ForEach-Object { \"$_\" }"
        )
        completed = subprocess.run(
            [POWERSHELL, "-NoProfile", "-NonInteractive", "-Command", script],
            capture_output=True,
            text=True,
            check=True,
            cwd=REPO_ROOT,
        )
        output = completed.stdout + completed.stderr
        self.assertIn("deprecated", output)
        self.assertIn("cranelift", output)

    def test_full_profile_resolves_without_a_slim_backend(self) -> None:
        script = (
            f"$ErrorActionPreference='Stop'; . '{INSTALLER.as_posix()}'; "
            "Resolve-OscanPackageProfile -Profile full -Backend llvm"
        )
        completed = subprocess.run(
            [POWERSHELL, "-NoProfile", "-NonInteractive", "-Command", script],
            capture_output=True,
            text=True,
            check=True,
            cwd=REPO_ROOT,
        )
        self.assertEqual(completed.stdout.strip(), "full")

    def test_an_unknown_backend_is_rejected(self) -> None:
        script = (
            f"$ErrorActionPreference='Stop'; . '{INSTALLER.as_posix()}'; "
            "try { Resolve-OscanBackend 'gcc' } catch { $_.Exception.Message }"
        )
        completed = subprocess.run(
            [POWERSHELL, "-NoProfile", "-NonInteractive", "-Command", script],
            capture_output=True,
            text=True,
            check=True,
            cwd=REPO_ROOT,
        )
        self.assertIn("Unknown backend 'gcc'", completed.stdout)

    def test_checksum_lookup_matches_the_selected_asset_only(self) -> None:
        script = f"""
$ErrorActionPreference = 'Stop'
. '{INSTALLER.as_posix()}'
$lines = @(
    'aaaa  oscan-{TAG}-windows-x86_64-cranelift.zip',
    'bbbb *oscan-{TAG}-windows-x86_64-llvm.zip'
)
Get-OscanExpectedChecksum -SumsLines $lines -AssetName 'oscan-{TAG}-windows-x86_64-llvm.zip'
Get-OscanExpectedChecksum -SumsLines $lines -AssetName 'oscan-{TAG}-windows-x86_64-c.zip'
"""
        completed = subprocess.run(
            [POWERSHELL, "-NoProfile", "-NonInteractive", "-Command", script],
            capture_output=True,
            text=True,
            check=True,
            cwd=REPO_ROOT,
        )
        self.assertEqual(completed.stdout.split(), ["bbbb"])

    def test_profile_aware_bundle_preflight_accepts_matching_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bundle = Path(tmp)
            metadata = {
                "schema_version": 2,
                "package_id": "oscan-full",
                "target": WINDOWS_TARGET,
                "profile": "full",
                "is_distribution": True,
                "available_backends": ["llvm", "cranelift", "c"],
                "default_backend": "llvm",
            }
            (bundle / "oscan-package.json").write_text(
                json.dumps(metadata), encoding="utf-8"
            )
            script = (
                f"$ErrorActionPreference='Stop'; . '{INSTALLER.as_posix()}'; "
                f"Assert-OscanArchiveBundle -BundleDir '{bundle.as_posix()}' "
                "-ExpectedProfile full | Out-Null; 'ok'"
            )
            completed = subprocess.run(
                [POWERSHELL, "-NoProfile", "-NonInteractive", "-Command", script],
                capture_output=True,
                text=True,
                cwd=REPO_ROOT,
            )
            self.assertEqual(
                completed.returncode, 0, completed.stdout + completed.stderr
            )
            self.assertEqual(completed.stdout.strip(), "ok")

    def test_legacy_bundle_preflight_is_refused_before_its_installer_runs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            script = (
                f"$ErrorActionPreference='Stop'; . '{INSTALLER.as_posix()}'; "
                "try { "
                f"Assert-OscanArchiveBundle -BundleDir '{Path(tmp).as_posix()}' "
                "-ExpectedProfile llvm "
                "} catch { $_.Exception.Message }"
            )
            completed = subprocess.run(
                [POWERSHELL, "-NoProfile", "-NonInteractive", "-Command", script],
                capture_output=True,
                text=True,
                check=True,
                cwd=REPO_ROOT,
            )
            self.assertIn("destructive flat installer", completed.stdout)


@unittest.skipUnless(POWERSHELL, "pwsh is required to parse the installer")
class InstallerSyntaxTests(unittest.TestCase):
    def test_the_installer_parses(self) -> None:
        script = (
            "$errors = $null; "
            "[void][System.Management.Automation.Language.Parser]::ParseFile("
            f"'{INSTALLER.as_posix()}', [ref]$null, [ref]$errors); "
            "if ($errors) { $errors | ForEach-Object { $_.Message }; exit 1 }"
        )
        completed = subprocess.run(
            [POWERSHELL, "-NoProfile", "-NonInteractive", "-Command", script],
            capture_output=True,
            text=True,
            cwd=REPO_ROOT,
        )
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)


class InstallerDocumentationTests(unittest.TestCase):
    """The user-facing docs must match what the tooling actually does."""

    def setUp(self) -> None:
        self.readme = (REPO_ROOT / "README.md").read_text(encoding="utf-8")
        self.guide = (REPO_ROOT / "docs" / "guide.md").read_text(encoding="utf-8")
        self.technical = (
            REPO_ROOT / "docs" / "technical-details.md"
        ).read_text(encoding="utf-8")
        self.spec = (REPO_ROOT / "docs" / "spec" / "oscan-spec.md").read_text(
            encoding="utf-8"
        )

    def test_the_readme_shows_the_backend_flag_for_every_backend(self) -> None:
        for backend in ("llvm", "cranelift", "c"):
            self.assertIn(f"install-latest.ps1 -Backend {backend}", self.readme)

    def test_technical_details_list_the_exact_release_archive_names(self) -> None:
        contract = rt.load_release_contract(rt.CONTRACT_PATH)
        for target, spec in contract["variants"].items():
            for profile in spec["profiles"]:
                name = f"oscan-vX.Y.Z-{target}-{profile}"
                self.assertIn(
                    name, self.technical, f"{target}/{profile} is undocumented"
                )

    def test_the_package_docs_describe_full_and_coexisting_profiles(self) -> None:
        self.assertIn("oscan-vX.Y.Z-windows-x86_64-full.zip", self.technical)
        self.assertIn("oscan-vX.Y.Z-linux-x86_64-full.tar.xz", self.technical)
        self.assertIn("oscan-full", self.technical)
        self.assertIn("oscan-llvm", self.technical)

    def test_the_package_docs_name_every_profile_msi(self) -> None:
        for profile in rt.CANONICAL_PROFILES:
            with self.subTest(profile=profile):
                self.assertIn(
                    f"oscan-vX.Y.Z-windows-x86_64-{profile}.msi",
                    self.technical,
                )
        self.assertNotIn("oscan-vX.Y.Z-windows-x86_64.msi", self.technical)

    def test_checksum_commands_verify_exactly_the_downloaded_asset(self) -> None:
        # A bare `sha256sum -c SHA256SUMS` checks every listed asset,
        # including ones the reader never downloaded, so the docs must
        # filter to the canonical file name first.
        for name, text in (
            ("docs/guide.md", self.guide),
            ("docs/spec/oscan-spec.md", self.spec),
        ):
            with self.subTest(doc=name):
                self.assertNotRegex(text, r"sha256sum -c SHA256SUMS")
                self.assertNotRegex(text, r"shasum -a 256 -c SHA256SUMS")
        for text in (self.guide, self.spec):
            self.assertRegex(text, r"grep -E .*SHA256SUMS.*\| sha256sum -c -")
        # The archive must keep its published name so the entry matches.
        self.assertIn("keeping its original file name", self.guide)

    def test_macos_checksum_command_is_filtered_too(self) -> None:
        for text in (self.guide, self.spec):
            self.assertRegex(text, r"grep -E .*SHA256SUMS.*\| shasum -a 256 -c -")

    def test_msi_and_archive_uninstall_paths_are_documented_separately(self) -> None:
        for name, text in (
            ("docs/guide.md", self.guide),
            ("docs/spec/oscan-spec.md", self.spec),
        ):
            with self.subTest(doc=name):
                self.assertIn("msiexec /x", text)
                self.assertIn("Apps", text)
        # Deleting the directory is only correct for archive installs.
        self.assertIn("Uninstall (archive install)", self.guide)
        self.assertIn("Uninstall (Windows MSI)", self.guide)

    def test_default_selection_and_profile_isolation_are_documented(self) -> None:
        for text in (self.guide, self.technical, self.spec):
            self.assertIn("SetDefault", text)
            self.assertIn("oscan-full", text)
            self.assertIn("oscan-llvm", text)

    def test_the_provider_search_roots_include_the_sidecar(self) -> None:
        for name, text in (
            ("docs/technical-details.md", self.technical),
            ("docs/guide.md", self.guide),
            ("docs/spec/oscan-spec.md", self.spec),
        ):
            with self.subTest(doc=name):
                self.assertIn("<exe-dir>/native-link", text)
                self.assertIn("<exe-dir>/toolchain", text)

    def test_sidecar_and_embedded_modes_are_distinguished(self) -> None:
        for name, text in (
            ("docs/technical-details.md", self.technical),
            ("docs/guide.md", self.guide),
            ("docs/spec/oscan-spec.md", self.spec),
        ):
            with self.subTest(doc=name):
                self.assertIn("OSCAN_EMBED_ASSETS_DIR", text)
                self.assertIn("used in place", text)
        self.assertIn("The binary embeds nothing", self.guide)

    def test_readme_keeps_implementation_details_out_of_the_quick_start(self) -> None:
        self.assertIn("docs/technical-details.md", self.readme)
        for detail in (
            "OSCAN_LLVM_LIB",
            "OSCAN_EMBED_ASSETS_DIR",
            "<exe-dir>/native-link",
            "sample-backend-matrix.ps1",
        ):
            self.assertNotIn(detail, self.readme)


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
