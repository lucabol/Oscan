"""Offline coexistence tests for the archive installers."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time
import unittest


REPO_ROOT = Path(__file__).resolve().parent.parent
WINDOWS_INSTALLER = REPO_ROOT / "scripts" / "install-oscan.ps1"
UNIX_INSTALLER = REPO_ROOT / "scripts" / "install-oscan.sh"
POWERSHELL = shutil.which("pwsh") or shutil.which("powershell")
SH = shutil.which("sh")


def make_bundle(root: Path, profile: str, version: str, windows: bool) -> Path:
    bundle = root / f"{profile}-{version}"
    bundle.mkdir(parents=True)
    binary = bundle / ("oscan.exe" if windows else "oscan")
    binary.write_bytes(
        b"fake windows executable"
        if windows
        else b"#!/bin/sh\nprintf '%s\\n' fake-oscan\n"
    )
    if not windows:
        binary.chmod(0o755)
    metadata = {
        "schema_version": 2,
        "package_id": f"oscan-{profile}",
        "profile": profile,
        "version": version,
        "target": "windows-x86_64" if windows else "linux-x86_64",
        "is_distribution": True,
        "available_backends": (
            ["llvm", "cranelift", "c"] if profile == "full" else [profile]
        ),
        "default_backend": "llvm" if profile == "full" else profile,
        "component_digests": {
            binary.name: hashlib.sha256(binary.read_bytes()).hexdigest(),
            "c_toolchain": {"version": "nested-toolchain-version"},
        },
    }
    (bundle / "oscan-package.json").write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return bundle


@unittest.skipUnless(
    POWERSHELL and os.name == "nt", "Windows PowerShell is required"
)
class WindowsInstallerCoexistenceTests(unittest.TestCase):
    def invoke(
        self,
        bundle: Path,
        install_root: Path | str,
        bin_dir: Path | str,
        *extra: str,
        succeeds: bool = True,
    ) -> subprocess.CompletedProcess[str]:
        command = [
            POWERSHELL,
            "-NoProfile",
            "-NonInteractive",
            "-File",
            str(WINDOWS_INSTALLER),
            "-SourceDir",
            str(bundle),
            "-InstallRoot",
            str(install_root),
            "-BinDir",
            str(bin_dir),
            "-NoPathUpdate",
            "-AllowMsiCommandConflict",
            *extra,
        ]
        env = os.environ.copy()
        install_path = Path(str(install_root).rstrip("\\/"))
        env["ProgramW6432"] = str(install_path.parent / "no-msi")
        env["LOCALAPPDATA"] = str(install_path.parent / "local-app-data")
        completed = subprocess.run(
            command, capture_output=True, text=True, env=env
        )
        if succeeds:
            self.assertEqual(
                completed.returncode, 0, completed.stdout + completed.stderr
            )
        else:
            self.assertNotEqual(completed.returncode, 0)
        return completed

    def test_profiles_coexist_upgrade_independently_and_preserve_default(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            llvm_v1 = make_bundle(root, "llvm", "1.0.0", True)
            c_v1 = make_bundle(root, "c", "1.0.0", True)
            llvm_v2 = make_bundle(root, "llvm", "2.0.0", True)
            archive_marker = (
                root / "local-app-data" / "Programs" / "Oscan" / "archive-default"
            )
            llvm_marker = (
                root / "local-app-data" / "Programs" / "Oscan" / "archive-profile-llvm"
            )
            c_marker = (
                root / "local-app-data" / "Programs" / "Oscan" / "archive-profile-c"
            )

            self.invoke(llvm_v1, install_root, bin_dir)
            self.assertTrue(archive_marker.is_file())
            self.assertTrue(llvm_marker.is_file())
            self.invoke(c_v1, install_root, bin_dir)
            self.assertTrue(c_marker.is_file())
            self.assertTrue((bin_dir / "oscan-llvm.cmd").is_file())
            self.assertTrue((bin_dir / "oscan-c.cmd").is_file())
            self.assertEqual(
                (install_root / "default-profile").read_text().strip(), "llvm"
            )

            self.invoke(c_v1, install_root, bin_dir, "-SetDefault")
            self.invoke(llvm_v2, install_root, bin_dir)
            self.assertTrue(
                (install_root / "profiles" / "llvm" / "2.0.0" / "oscan.exe").is_file()
            )
            self.assertFalse((install_root / "profiles" / "llvm" / "1.0.0").exists())
            self.assertTrue(
                (install_root / "profiles" / "c" / "1.0.0" / "oscan.exe").is_file()
            )
            self.assertEqual(
                (install_root / "default-profile").read_text().strip(), "c"
            )

            self.invoke(c_v1, install_root, bin_dir, "-Uninstall")
            self.assertFalse((install_root / "profiles" / "c").exists())
            self.assertTrue((install_root / "profiles" / "llvm").is_dir())
            self.assertFalse((bin_dir / "oscan.cmd").exists())
            self.assertFalse((install_root / "default-profile").exists())
            self.assertFalse(archive_marker.exists())
            self.assertFalse(c_marker.exists())
            self.assertTrue(llvm_marker.is_file())

    def test_invalid_repair_does_not_remove_working_profile(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            llvm = make_bundle(root, "llvm", "1.0.0", True)
            self.invoke(llvm, install_root, bin_dir)

            broken = make_bundle(root, "llvm", "2.0.0", True)
            (broken / "oscan.exe").unlink()
            self.invoke(broken, install_root, bin_dir, succeeds=False)
            self.assertTrue(
                (install_root / "profiles" / "llvm" / "1.0.0" / "oscan.exe").is_file()
            )

    def test_explicit_profile_can_uninstall_a_damaged_bundle(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            bundle = make_bundle(root, "llvm", "1.0.0", True)
            self.invoke(bundle, install_root, bin_dir)
            shutil.rmtree(bundle)
            empty_source = root / "empty"
            empty_source.mkdir()
            self.invoke(
                empty_source,
                install_root,
                bin_dir,
                "-Profile",
                "llvm",
                "-Uninstall",
            )
            self.assertFalse((install_root / "profiles" / "llvm").exists())

    def test_activation_failure_rolls_back_a_same_version_repair(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            bundle = make_bundle(root, "llvm", "1.0.0", True)
            self.invoke(bundle, install_root, bin_dir)
            invalid_bin_dir = root / "not-a-directory"
            invalid_bin_dir.write_text("occupied", encoding="utf-8")
            self.invoke(
                bundle,
                install_root,
                invalid_bin_dir,
                succeeds=False,
            )
            self.assertTrue(
                (install_root / "profiles" / "llvm" / "1.0.0" / "oscan.exe").is_file()
            )
            self.assertTrue((bin_dir / "oscan-llvm.cmd").is_file())

    def test_foreign_install_root_cannot_replace_or_remove_shared_selectors(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            first_root = root / "first"
            second_root = root / "second"
            bin_dir = root / "bin"
            llvm = make_bundle(root, "llvm", "1.0.0", True)
            c_bundle = make_bundle(root, "c", "1.0.0", True)

            self.invoke(llvm, first_root, bin_dir)
            qualified_before = (bin_dir / "oscan-llvm.cmd").read_bytes()
            default_before = (bin_dir / "oscan.cmd").read_bytes()

            self.invoke(llvm, second_root, bin_dir, succeeds=False)
            self.invoke(c_bundle, second_root, bin_dir, "-SetDefault", succeeds=False)
            self.invoke(
                root / "missing-source",
                second_root,
                bin_dir,
                "-Profile",
                "llvm",
                "-Uninstall",
            )

            self.assertEqual((bin_dir / "oscan-llvm.cmd").read_bytes(), qualified_before)
            self.assertEqual((bin_dir / "oscan.cmd").read_bytes(), default_before)
            self.assertTrue(
                (first_root / "profiles" / "llvm" / "1.0.0" / "oscan.exe").is_file()
            )

    def test_selector_failure_rolls_back_a_staged_uninstall(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            bundle = make_bundle(root, "llvm", "1.0.0", True)
            self.invoke(bundle, install_root, bin_dir)
            qualified = bin_dir / "oscan-llvm.cmd"

            with qualified.open("rb"):
                self.invoke(
                    bundle,
                    install_root,
                    bin_dir,
                    "-Uninstall",
                    succeeds=False,
                )

            self.assertTrue(
                (install_root / "profiles" / "llvm" / "1.0.0" / "oscan.exe").is_file()
            )
            self.assertTrue(qualified.is_file())
            self.assertTrue((bin_dir / "oscan.cmd").is_file())
            self.assertEqual(
                (install_root / "default-profile").read_text().strip(), "llvm"
            )

    def test_unicode_install_root_uses_an_ascii_relative_shim(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "unicod\u00e9"
            install_root = root / "install"
            bin_dir = root / "commands"
            bundle = make_bundle(root, "llvm", "1.0.0", True)
            self.invoke(bundle, install_root, bin_dir)
            shim = (bin_dir / "oscan-llvm.cmd").read_bytes()
            self.assertTrue(all(byte < 128 for byte in shim), shim)
            self.assertIn(b"%~dp0.oscan-profiles", shim)

    def test_uninstall_keeps_the_msi_marker_for_another_install_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            first_root = root / "first"
            second_root = root / "second"
            bundle = make_bundle(root, "llvm", "1.0.0", True)
            self.invoke(bundle, first_root, first_root / "bin")
            self.invoke(bundle, second_root, second_root / "bin")
            marker = (
                root / "local-app-data" / "Programs" / "Oscan" / "archive-default"
            )
            profile_marker = (
                root
                / "local-app-data"
                / "Programs"
                / "Oscan"
                / "archive-profile-llvm"
            )
            self.assertEqual(len(profile_marker.read_text().splitlines()), 2)
            self.invoke(
                bundle,
                first_root,
                first_root / "bin",
                "-Uninstall",
            )
            self.assertTrue(marker.is_file())
            self.assertEqual(len(profile_marker.read_text().splitlines()), 1)

    def test_failed_uninstall_preserves_commands_and_selector(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            bundle = make_bundle(root, "llvm", "1.0.0", True)
            self.invoke(bundle, install_root, bin_dir)
            executable = install_root / "profiles" / "llvm" / "1.0.0" / "oscan.exe"
            with executable.open("rb"):
                self.invoke(
                    bundle,
                    install_root,
                    bin_dir,
                    "-Uninstall",
                    succeeds=False,
                )
            self.assertTrue(executable.is_file())
            self.assertTrue((bin_dir / "oscan-llvm.cmd").is_file())
            self.assertTrue((bin_dir / "oscan.cmd").is_file())
            self.assertEqual(
                (install_root / "default-profile").read_text().strip(), "llvm"
            )

    def test_equivalent_root_spellings_share_marker_ownership(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            bundle = make_bundle(root, "llvm", "1.0.0", True)
            self.invoke(
                bundle,
                f"{install_root}{os.sep}",
                f"{bin_dir}{os.sep}",
            )
            marker = (
                root / "local-app-data" / "Programs" / "Oscan" / "archive-default"
            )
            self.invoke(bundle, install_root, bin_dir, "-Uninstall")
            self.assertFalse(marker.exists())

    def test_install_waits_for_an_in_progress_operation(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            bundle = make_bundle(root, "llvm", "1.0.0", True)
            lock_path = install_root / ".install.lock"
            ready = root / "lock-ready"
            escaped_root = str(install_root).replace("'", "''")
            escaped_ready = str(ready).replace("'", "''")
            escaped_lock = str(lock_path).replace("'", "''")
            holder = subprocess.Popen(
                [
                    POWERSHELL,
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    (
                        f"$root = '{escaped_root}'; $ready = '{escaped_ready}'; "
                        "New-Item -ItemType Directory -Path $root -Force | Out-Null; "
                        "$stream = [IO.File]::Open("
                        f"'{escaped_lock}', "
                        "[IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, "
                        "[IO.FileShare]::None); "
                        "Set-Content -LiteralPath $ready -Value ready; "
                        "Start-Sleep -Seconds 2; $stream.Dispose()"
                    ),
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            deadline = time.monotonic() + 5
            while not ready.exists() and time.monotonic() < deadline:
                time.sleep(0.05)
            self.assertTrue(ready.exists(), "lock holder did not start")
            started = time.monotonic()
            self.invoke(bundle, install_root, bin_dir)
            self.assertGreaterEqual(time.monotonic() - started, 1.0)
            stdout, stderr = holder.communicate(timeout=5)
            self.assertEqual(holder.returncode, 0, stdout + stderr)

    def test_archive_install_waits_for_windows_installer_execution(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            bundle = make_bundle(root, "llvm", "1.0.0", True)
            ready = root / "mutex-ready"
            escaped_ready = str(ready).replace("'", "''")
            holder = subprocess.Popen(
                [
                    POWERSHELL,
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    (
                        "$created = $false; "
                        "$mutex = [Threading.Mutex]::new("
                        "$true, 'Global\\_MSIExecute', [ref]$created); "
                        "if (-not $created) { $mutex.WaitOne() | Out-Null }; "
                        f"Set-Content -LiteralPath '{escaped_ready}' -Value ready; "
                        "Start-Sleep -Seconds 2; "
                        "$mutex.ReleaseMutex(); $mutex.Dispose()"
                    ),
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            deadline = time.monotonic() + 5
            while not ready.exists() and time.monotonic() < deadline:
                time.sleep(0.05)
            self.assertTrue(ready.exists(), "MSI mutex holder did not start")
            started = time.monotonic()
            self.invoke(bundle, install_root, bin_dir)
            self.assertGreaterEqual(time.monotonic() - started, 1.0)
            stdout, stderr = holder.communicate(timeout=5)
            self.assertEqual(holder.returncode, 0, stdout + stderr)


@unittest.skipUnless(SH and os.name != "nt", "a native POSIX shell is required")
class UnixInstallerCoexistenceTests(unittest.TestCase):
    def invoke(
        self,
        bundle: Path,
        install_root: Path,
        bin_dir: Path,
        *extra: str,
        succeeds: bool = True,
        env: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        command = [
            SH,
            str(UNIX_INSTALLER),
            "--source-dir",
            str(bundle),
            "--install-root",
            str(install_root),
            "--bin-dir",
            str(bin_dir),
            *extra,
        ]
        completed = subprocess.run(command, capture_output=True, text=True, env=env)
        if succeeds:
            self.assertEqual(
                completed.returncode, 0, completed.stdout + completed.stderr
            )
        else:
            self.assertNotEqual(completed.returncode, 0)
        return completed

    def test_profiles_coexist_upgrade_independently_and_preserve_default(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            full_v1 = make_bundle(root, "full", "1.0.0", False)
            llvm_v1 = make_bundle(root, "llvm", "1.0.0", False)
            full_v2 = make_bundle(root, "full", "2.0.0", False)

            self.invoke(full_v1, install_root, bin_dir)
            self.invoke(llvm_v1, install_root, bin_dir)
            self.assertTrue((bin_dir / "oscan-full").is_symlink())
            self.assertTrue((bin_dir / "oscan-llvm").is_symlink())
            self.assertEqual((install_root / "default-profile").read_text().strip(), "full")

            self.invoke(llvm_v1, install_root, bin_dir, "--set-default")
            self.invoke(full_v2, install_root, bin_dir)
            self.assertTrue(
                (install_root / "profiles" / "full" / "2.0.0" / "oscan").is_file()
            )
            self.assertFalse((install_root / "profiles" / "full" / "1.0.0").exists())
            self.assertTrue(
                (install_root / "profiles" / "llvm" / "1.0.0" / "oscan").is_file()
            )
            self.assertEqual((install_root / "default-profile").read_text().strip(), "llvm")
            self.assertEqual((bin_dir / "oscan").readlink(), Path("oscan-llvm"))

            self.invoke(llvm_v1, install_root, bin_dir, "--uninstall")
            self.assertFalse((install_root / "profiles" / "llvm").exists())
            self.assertTrue((install_root / "profiles" / "full").is_dir())
            self.assertFalse((bin_dir / "oscan").exists())

    def test_invalid_repair_does_not_remove_working_profile(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            c_bundle = make_bundle(root, "c", "1.0.0", False)
            self.invoke(c_bundle, install_root, bin_dir)
            broken = make_bundle(root, "c", "2.0.0", False)
            (broken / "oscan").unlink()
            self.invoke(broken, install_root, bin_dir, succeeds=False)
            self.assertTrue(
                (install_root / "profiles" / "c" / "1.0.0" / "oscan").is_file()
            )

    def test_explicit_profile_can_uninstall_a_damaged_bundle(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            bundle = make_bundle(root, "c", "1.0.0", False)
            self.invoke(bundle, install_root, bin_dir)
            shutil.rmtree(bundle)
            empty_source = root / "empty"
            empty_source.mkdir()
            self.invoke(
                empty_source,
                install_root,
                bin_dir,
                "--profile",
                "c",
                "--uninstall",
            )
            self.assertFalse((install_root / "profiles" / "c").exists())

    def test_activation_failure_rolls_back_a_same_version_repair(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            bundle = make_bundle(root, "c", "1.0.0", False)
            self.invoke(bundle, install_root, bin_dir)
            invalid_bin_dir = root / "not-a-directory"
            invalid_bin_dir.write_text("occupied", encoding="utf-8")
            self.invoke(
                bundle,
                install_root,
                invalid_bin_dir,
                succeeds=False,
            )
            self.assertTrue(
                (install_root / "profiles" / "c" / "1.0.0" / "oscan").is_file()
            )
            self.assertTrue((bin_dir / "oscan-c").is_symlink())

    def test_shared_bin_preserves_selectors_owned_by_other_install_roots(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            first_root = root / "first"
            second_root = root / "second"
            bin_dir = root / "bin"
            llvm = make_bundle(root, "llvm", "1.0.0", False)
            c_bundle = make_bundle(root, "c", "1.0.0", False)

            self.invoke(llvm, first_root, bin_dir)
            self.invoke(c_bundle, second_root, bin_dir)
            self.assertEqual((bin_dir / "oscan").readlink(), Path("oscan-llvm"))

            self.invoke(c_bundle, second_root, bin_dir, "--set-default", succeeds=False)
            self.invoke(c_bundle, second_root, bin_dir, "--uninstall")
            self.invoke(
                root / "missing-source",
                second_root,
                bin_dir,
                "--profile",
                "llvm",
                "--uninstall",
            )

            self.assertTrue((bin_dir / "oscan-llvm").is_symlink())
            self.assertEqual((bin_dir / "oscan").readlink(), Path("oscan-llvm"))
            self.assertTrue(
                (first_root / "profiles" / "llvm" / "1.0.0" / "oscan").is_file()
            )

    def test_selector_failure_rolls_back_a_staged_uninstall(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            bundle = make_bundle(root, "c", "1.0.0", False)
            self.invoke(bundle, install_root, bin_dir)

            fake_bin = root / "fake-bin"
            fake_bin.mkdir()
            fake_rm = fake_bin / "rm"
            real_rm = shutil.which("rm")
            self.assertIsNotNone(real_rm)
            fake_rm.write_text(
                "#!/bin/sh\n"
                'if [ "$#" -eq 2 ] && [ "$1" = "-f" ] && '
                '[ "$2" = "$FAIL_RM_PATH" ]; then exit 1; fi\n'
                f"exec {real_rm} \"$@\"\n",
                encoding="utf-8",
            )
            fake_rm.chmod(0o755)
            env = os.environ.copy()
            env["PATH"] = f"{fake_bin}{os.pathsep}{env['PATH']}"
            env["FAIL_RM_PATH"] = str(bin_dir / "oscan-c")

            self.invoke(
                bundle,
                install_root,
                bin_dir,
                "--uninstall",
                succeeds=False,
                env=env,
            )

            self.assertTrue(
                (install_root / "profiles" / "c" / "1.0.0" / "oscan").is_file()
            )
            self.assertTrue((bin_dir / "oscan-c").is_symlink())
            self.assertTrue((bin_dir / "oscan").is_symlink())
            self.assertEqual(
                (install_root / "default-profile").read_text().strip(), "c"
            )

    def test_no_bin_link_preserves_payloads_referenced_by_existing_links(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            v1 = make_bundle(root, "c", "1.0.0", False)
            v2 = make_bundle(root, "c", "2.0.0", False)
            self.invoke(v1, install_root, bin_dir)
            old_target = (bin_dir / "oscan-c").readlink()
            self.invoke(v2, install_root, bin_dir, "--no-bin-link")
            self.assertEqual((bin_dir / "oscan-c").readlink(), old_target)
            self.assertTrue(
                (install_root / "profiles" / "c" / "1.0.0" / "oscan").is_file()
            )
            self.assertTrue(
                (install_root / "profiles" / "c" / "2.0.0" / "oscan").is_file()
            )

    def test_rejects_a_non_x86_64_host(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bundle = make_bundle(root, "c", "1.0.0", False)
            fake_bin = root / "fake-bin"
            fake_bin.mkdir()
            fake_uname = fake_bin / "uname"
            fake_uname.write_text(
                "#!/bin/sh\n"
                'case "$1" in\n'
                "  -s) printf '%s\\n' Linux ;;\n"
                "  -m) printf '%s\\n' aarch64 ;;\n"
                "esac\n",
                encoding="utf-8",
            )
            fake_uname.chmod(0o755)
            env = os.environ.copy()
            env["PATH"] = f"{fake_bin}{os.pathsep}{env['PATH']}"
            completed = subprocess.run(
                [
                    SH,
                    str(UNIX_INSTALLER),
                    "--source-dir",
                    str(bundle),
                    "--install-root",
                    str(root / "install"),
                    "--bin-dir",
                    str(root / "bin"),
                ],
                capture_output=True,
                text=True,
                env=env,
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("requires an x86_64 host", completed.stderr)

    def test_install_waits_for_an_in_progress_operation(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            install_root = root / "install"
            bin_dir = root / "bin"
            bundle = make_bundle(root, "c", "1.0.0", False)
            lock = install_root / ".install.lock"
            lock.mkdir(parents=True)
            (lock / "pid").write_text(f"{os.getpid()}\n", encoding="ascii")
            release = threading.Timer(1.2, shutil.rmtree, args=(lock,))
            release.start()
            try:
                started = time.monotonic()
                self.invoke(bundle, install_root, bin_dir)
                self.assertGreaterEqual(time.monotonic() - started, 1.0)
            finally:
                release.cancel()


if __name__ == "__main__":
    unittest.main()
