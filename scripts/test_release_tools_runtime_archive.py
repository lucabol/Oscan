#!/usr/bin/env python3
"""Focused tests for runtime archive building and release staging.

These cover compiler/archiver discovery, target-triple enforcement, clean
launch errors, atomic archive/manifest publication, canonical Make delegation,
and the native assets required in packaged releases.

Run with:
    python scripts/test_release_tools_runtime_archive.py
or:
    python -m unittest scripts.test_release_tools_runtime_archive -v
"""
from __future__ import annotations

import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_tools as rt  # noqa: E402


def _which_only(available: set[str]):
    return lambda name: (f"/fake/bin/{name}" if name in available else None)


def _pinned_windows_runtime_toolchain() -> dict:
    source = rt.load_manifest(
        rt.REPO_ROOT / "packaging" / "toolchains" / "windows-x86_64.json"
    )
    runtime = source["toolchain"]["runtime"]
    return {
        "source_manifest": "windows-x86_64.json",
        "vendor": source["toolchain"]["vendor"],
        "version": source["toolchain"]["version"],
        "archive_digest": source["toolchain"]["archive"]["digest"],
        "abi": runtime["abi"],
        "crt": runtime["crt"],
        "compiler": {
            "command": "C:/build/toolchain/bin/clang.exe",
            "family": runtime["compiler"]["family"],
            "version": f"clang version {runtime['compiler']['version']}",
            "target": runtime["compiler"]["target"],
            "size_flag": runtime["compiler"]["size_flag"],
        },
        "archiver": {
            "command": "C:/build/toolchain/bin/llvm-ar.exe",
            "family": runtime["archiver"]["family"],
            "version": f"LLVM version {runtime['archiver']['version']}",
        },
        "linker": {
            "command": "C:/build/toolchain/bin/ld.lld.exe",
            "family": runtime["linker"]["family"],
            "version": f"LLD {runtime['linker']['version']}",
            "driver_flags": runtime["linker"]["driver_flags"],
        },
    }


def _pinned_linux_runtime_toolchain() -> dict:
    source = rt.load_manifest(
        rt.REPO_ROOT / "packaging" / "toolchains" / "linux-x86_64.json"
    )
    runtime = source["toolchain"]["runtime"]
    return {
        "source_manifest": "linux-x86_64.json",
        "vendor": source["toolchain"]["vendor"],
        "version": source["toolchain"]["version"],
        "archive_digest": source["toolchain"]["archive"]["digest"],
        "abi": runtime["abi"],
        "crt": runtime["crt"],
        "compiler": {
            "command": "/build/toolchain/bin/x86_64-linux-musl-gcc",
            "family": runtime["compiler"]["family"],
            "version": f"x86_64-linux-musl-gcc (GCC) {runtime['compiler']['version']} 20211120",
            "target": runtime["compiler"]["target"],
            "size_flag": runtime["compiler"]["size_flag"],
        },
        "archiver": {
            "command": "/build/toolchain/bin/x86_64-linux-musl-ar",
            "family": runtime["archiver"]["family"],
            "version": f"GNU ar (GNU Binutils) {runtime['archiver']['version']}",
        },
        "linker": {
            "command": "/build/toolchain/bin/x86_64-linux-musl-ld",
            "family": runtime["linker"]["family"],
            "version": f"GNU ld (GNU Binutils) {runtime['linker']['version']}",
            "driver_flags": runtime["linker"]["driver_flags"],
        },
    }


class DefaultCcForTargetTests(unittest.TestCase):
    def test_env_override_wins_regardless_of_path(self):
        with mock.patch.dict(rt.os.environ, {"OSCAN_ARCHIVE_CC": "my-special-cc"}, clear=False):
            with mock.patch.object(rt.shutil, "which", _which_only(set())):
                self.assertEqual(rt.default_cc_for_target("windows-x86_64"), "my-special-cc")

    def test_windows_host_native_prefers_gcc_over_bare_cc(self):
        # Regression guard: must NOT fall back to a literal 'cc', which does
        # not exist on stock Windows/MinGW installs.
        with mock.patch.dict(rt.os.environ, {}, clear=True):
            with mock.patch.object(rt, "detect_host_target", return_value="windows-x86_64"):
                with mock.patch.object(rt.shutil, "which", _which_only({"gcc"})):
                    self.assertEqual(rt.default_cc_for_target("windows-x86_64"), "gcc")

    def test_linux_host_native_prefers_cc_then_gcc(self):
        with mock.patch.dict(rt.os.environ, {}, clear=True):
            with mock.patch.object(rt, "detect_host_target", return_value="linux-x86_64"):
                with mock.patch.object(rt.shutil, "which", _which_only({"cc", "gcc"})):
                    self.assertEqual(rt.default_cc_for_target("linux-x86_64"), "cc")

    def test_cross_target_uses_triple_prefixed_binary(self):
        with mock.patch.dict(rt.os.environ, {}, clear=True):
            with mock.patch.object(rt, "detect_host_target", return_value="linux-x86_64"):
                with mock.patch.object(rt.shutil, "which", _which_only({"x86_64-w64-mingw32-gcc"})):
                    self.assertEqual(rt.default_cc_for_target("windows-x86_64"), "x86_64-w64-mingw32-gcc")

    def test_fails_cleanly_with_actionable_message_when_nothing_found(self):
        with mock.patch.dict(rt.os.environ, {}, clear=True):
            with mock.patch.object(rt, "detect_host_target", return_value="windows-x86_64"):
                with mock.patch.object(rt.shutil, "which", _which_only(set())):
                    with self.assertRaises(SystemExit) as ctx:
                        rt.default_cc_for_target("windows-x86_64")
        message = str(ctx.exception)
        self.assertIn("no C compiler found", message)
        self.assertIn("--cc", message)
        self.assertIn("OSCAN_ARCHIVE_CC", message)


class DefaultArForTests(unittest.TestCase):
    def test_env_override_wins(self):
        with mock.patch.dict(rt.os.environ, {"OSCAN_ARCHIVE_AR": "my-ar"}, clear=False):
            with mock.patch.object(rt.shutil, "which", _which_only(set())):
                self.assertEqual(rt.default_ar_for("gcc"), "my-ar")

    def test_derives_prefixed_ar_from_prefixed_cc(self):
        with mock.patch.dict(rt.os.environ, {}, clear=True):
            with mock.patch.object(
                rt.shutil, "which", _which_only({"x86_64-linux-musl-ar"})
            ):
                self.assertEqual(rt.default_ar_for("x86_64-linux-musl-gcc"), "x86_64-linux-musl-ar")

    def test_clang_prefers_llvm_ar_when_plain_ar_missing(self):
        with mock.patch.dict(rt.os.environ, {}, clear=True):
            with mock.patch.object(rt.shutil, "which", _which_only({"llvm-ar"})):
                self.assertEqual(rt.default_ar_for("clang"), "llvm-ar")

    def test_falls_back_to_plain_ar(self):
        with mock.patch.dict(rt.os.environ, {}, clear=True):
            with mock.patch.object(rt.shutil, "which", _which_only({"ar"})):
                self.assertEqual(rt.default_ar_for("gcc"), "ar")

    def test_fails_cleanly_with_actionable_message_when_nothing_found(self):
        with mock.patch.dict(rt.os.environ, {}, clear=True):
            with mock.patch.object(rt.shutil, "which", _which_only(set())):
                with self.assertRaises(SystemExit) as ctx:
                    rt.default_ar_for("gcc")
        message = str(ctx.exception)
        self.assertIn("no archiver found", message)
        self.assertIn("--ar", message)


class CanonicalizeToolPathTests(unittest.TestCase):
    """Manifest provenance safety: _canonicalize_tool_path must never record
    a bare/relative compiler-tool reference verbatim. See the security
    review finding that motivated this — a manifest-recorded relative 'cc'
    resolved against the wrong CWD is how a malicious project directory
    could get an attacker-planted binary trusted as toolchain provenance."""

    def test_bare_name_is_resolved_via_which_not_raw_cwd(self):
        # A bare name must not be resolved against the current working
        # directory at all; it has to go through shutil.which() (PATH
        # lookup) first, only then get canonicalized.
        fake_absolute = str(Path.cwd() / "unrelated-dir" / "clang.exe")
        with mock.patch.object(rt.shutil, "which", return_value=fake_absolute):
            resolved = rt._canonicalize_tool_path("clang")
        self.assertEqual(resolved, str(Path(fake_absolute).resolve()))
        self.assertTrue(Path(resolved).is_absolute())

    def test_which_miss_still_resolves_best_effort(self):
        # If shutil.which() can't find it (e.g. a relative path that exists
        # but isn't marked executable in this environment), fall back to
        # resolving the raw input rather than raising, so a directly-
        # supplied --cc/--ar still ends up as an absolute path.
        with mock.patch.object(rt.shutil, "which", return_value=None):
            resolved = rt._canonicalize_tool_path("relative/cc")
        self.assertTrue(Path(resolved).is_absolute())
        self.assertEqual(resolved, str((Path.cwd() / "relative" / "cc").resolve()))

    def test_already_absolute_path_is_canonicalized_not_left_as_is(self):
        # ".." components must be normalized away even when the input is
        # already absolute, so two different-looking manifest strings that
        # refer to the same file always canonicalize identically.
        messy = str(Path.cwd() / "a" / ".." / "b" / "cc.exe")
        with mock.patch.object(rt.shutil, "which") as which_mock:
            resolved = rt._canonicalize_tool_path(messy)
        which_mock.assert_not_called()
        self.assertEqual(resolved, str((Path.cwd() / "b" / "cc.exe").resolve()))
        self.assertNotIn("..", resolved)


class RunToolMissingBinaryTests(unittest.TestCase):
    def test_run_tool_raises_clean_systemexit_not_traceback(self):
        with mock.patch.object(
            rt.subprocess, "run", side_effect=FileNotFoundError()
        ):
            with self.assertRaises(SystemExit) as ctx:
                rt.run_tool(["nonexistent-compiler-xyz", "-c", "foo.c"])
        message = str(ctx.exception)
        self.assertIn("nonexistent-compiler-xyz", message)
        self.assertIn("was not found on PATH", message)

    def test_extract_archive_members_raises_clean_systemexit_not_traceback(self):
        with mock.patch.object(rt, "ensure_clean_dir"):
            with mock.patch.object(
                rt.subprocess, "run", side_effect=FileNotFoundError()
            ):
                with self.assertRaises(SystemExit) as ctx:
                    rt.extract_archive_members(
                        "nonexistent-ar-xyz", Path("archive.a"), Path("dest")
                    )
        message = str(ctx.exception)
        self.assertIn("nonexistent-ar-xyz", message)
        self.assertIn("was not found on PATH", message)


class CompilerTargetValidationTests(unittest.TestCase):
    def test_target_triple_matching(self):
        self.assertTrue(
            rt._target_tag_matches_triple(
                "linux-x86_64", "x86_64-unknown-linux-musl"
            )
        )
        self.assertTrue(
            rt._target_tag_matches_triple(
                "windows-x86_64", "x86_64-w64-mingw32"
            )
        )
        self.assertFalse(
            rt._target_tag_matches_triple(
                "windows-x86_64", "x86_64-unknown-linux-gnu"
            )
        )
        self.assertFalse(
            rt._target_tag_matches_triple(
                "windows-x86_64", "x86_64-pc-windows-msvc"
            )
        )

    def test_cross_target_host_gcc_is_rejected(self):
        with mock.patch.object(rt, "detect_host_target", return_value="linux-x86_64"):
            with mock.patch.object(
                rt, "_probe_compiler_target", return_value="x86_64-linux-gnu"
            ):
                with self.assertRaises(SystemExit) as ctx:
                    rt.resolve_compiler_configuration(
                        "windows-x86_64", "gcc", None, None
                    )
        self.assertIn("Refusing to label host objects", str(ctx.exception))

    def test_unsuitable_bare_clang_has_actionable_cross_error(self):
        with mock.patch.object(rt, "detect_host_target", return_value="linux-x86_64"):
            with mock.patch.object(
                rt, "_probe_compiler_target", return_value="x86_64-linux-gnu"
            ):
                with self.assertRaises(SystemExit) as ctx:
                    rt.resolve_compiler_configuration(
                        "windows-x86_64", "clang", None, None
                    )
        message = str(ctx.exception)
        self.assertIn("bare clang targets", message)
        self.assertIn("--target-triple", message)
        self.assertIn("--sysroot", message)

    def test_bare_clang_retargeting_requires_sysroot(self):
        with mock.patch.object(rt, "detect_host_target", return_value="linux-x86_64"):
            with mock.patch.object(
                rt, "_probe_compiler_target", return_value="x86_64-linux-gnu"
            ):
                with self.assertRaises(SystemExit) as ctx:
                    rt.resolve_compiler_configuration(
                        "windows-x86_64",
                        "clang",
                        "x86_64-w64-windows-gnu",
                        None,
                    )
        self.assertIn("needs --sysroot", str(ctx.exception))

    def test_configured_clang_target_and_sysroot_are_passed_to_compiler(self):
        triples = iter(["x86_64-linux-gnu", "x86_64-w64-windows-gnu"])
        with mock.patch.object(rt, "detect_host_target", return_value="linux-x86_64"):
            with mock.patch.object(
                rt, "_probe_compiler_target", side_effect=lambda *_: next(triples)
            ) as probe:
                compiler_args, reported, sysroot = rt.resolve_compiler_configuration(
                    "windows-x86_64",
                    "clang",
                    "x86_64-w64-windows-gnu",
                    str(rt.REPO_ROOT),
                )
        self.assertEqual(reported, "x86_64-w64-windows-gnu")
        self.assertEqual(sysroot, str(rt.REPO_ROOT))
        self.assertEqual(
            compiler_args,
            [
                "--target=x86_64-w64-windows-gnu",
                f"--sysroot={rt.REPO_ROOT}",
            ],
        )
        self.assertEqual(probe.call_args_list[1].args[1], compiler_args)

    def test_missing_compiler_during_target_probe_is_clean(self):
        with mock.patch.object(rt.subprocess, "run", side_effect=FileNotFoundError()):
            with self.assertRaises(SystemExit) as ctx:
                rt._probe_compiler_target("missing-cc", [])
        self.assertIn("could not launch compiler target probe", str(ctx.exception))


class PinnedWindowsToolchainTests(unittest.TestCase):
    def test_release_and_runtime_contracts_pin_the_same_gnu_abi_toolchain(self):
        manifest = rt.load_manifest(
            rt.REPO_ROOT / "packaging" / "toolchains" / "windows-x86_64.json"
        )
        runtime_contract = rt.load_runtime_archive_contract(
            rt.RUNTIME_ARCHIVE_CONTRACT_PATH
        )
        source = manifest["toolchain"]
        runtime = source["runtime"]
        expected = runtime_contract["targets"]["windows-x86_64"][
            "release_toolchain"
        ]

        self.assertEqual(source["vendor"], expected["vendor"])
        self.assertEqual(source["version"], expected["version"])
        self.assertEqual(runtime["abi"], "gnu")
        self.assertEqual(runtime["compiler"]["target"], expected["compiler_target"])
        self.assertEqual(runtime["compiler"]["size_flag"], "-Oz")
        self.assertEqual(runtime["linker"]["family"], "lld")
        self.assertEqual(runtime["linker"]["driver_flags"], ["-fuse-ld=lld"])
        self.assertIsNotNone(source["archive"]["digest"])


class PinnedLinuxToolchainTests(unittest.TestCase):
    """Linux parity for PinnedWindowsToolchainTests: the bundled musl
    cross-compiler pinned in linux-x86_64.json must match the
    release_toolchain provenance runtime archives are validated against, the
    same way the Windows llvm-mingw pin already does. This is the contract
    that closes the archive/compiler target mismatch bug (native runtime
    archives silently built with host cc while the release packages a musl
    cross-compiler)."""

    def test_release_and_runtime_contracts_pin_the_same_musl_toolchain(self):
        manifest = rt.load_manifest(
            rt.REPO_ROOT / "packaging" / "toolchains" / "linux-x86_64.json"
        )
        runtime_contract = rt.load_runtime_archive_contract(
            rt.RUNTIME_ARCHIVE_CONTRACT_PATH
        )
        source = manifest["toolchain"]
        runtime = source["runtime"]
        expected = runtime_contract["targets"]["linux-x86_64"][
            "release_toolchain"
        ]

        self.assertEqual(source["vendor"], expected["vendor"])
        self.assertEqual(source["version"], expected["version"])
        self.assertEqual(runtime["abi"], "musl")
        self.assertEqual(runtime["crt"], "musl")
        self.assertEqual(runtime["compiler"]["family"], "gcc")
        self.assertEqual(runtime["compiler"]["target"], expected["compiler_target"])
        self.assertEqual(runtime["compiler"]["target"], "x86_64-linux-musl")
        self.assertEqual(runtime["compiler"]["size_flag"], "-Os")
        self.assertEqual(runtime["archiver"]["family"], "gnu-ar")
        self.assertEqual(runtime["linker"]["family"], "gnu-ld")
        # Unlike Windows/lld, GCC + GNU ld need no special linker-selection
        # driver flag (no -fuse-ld=... equivalent is required).
        self.assertEqual(runtime["linker"]["driver_flags"], [])
        self.assertIsNotNone(source["archive"]["digest"])
        self.assertEqual(source["archive"]["digest"]["algorithm"], "sha256")
        self.assertEqual(
            source["archive"]["digest"]["value"],
            "c5d410d9f82a4f24c549fe5d24f988f85b2679b452413a9f7e5f7b956f2fe7ea",
        )

    def test_unpinned_linux_clang_uses_the_default_gnu_linker_family(self):
        with mock.patch.object(
            rt,
            "_tool_identity_output",
            side_effect=[
                "Ubuntu clang version 18.1.3",
                "GNU ar (GNU Binutils) 2.42",
            ],
        ):
            provenance = rt._runtime_toolchain_provenance(
                target="linux-x86_64",
                cc="/usr/bin/clang",
                ar="/usr/bin/ar",
                cc_target="x86_64-unknown-linux-gnu",
                target_spec={},
                toolchain_manifest_path=None,
            )

        self.assertEqual(provenance["linker"]["family"], "gnu-ld")
        self.assertEqual(provenance["linker"]["driver_flags"], [])

    def test_windows_and_linux_release_toolchains_share_the_same_shape(self):
        """Both bundled targets must expose the exact same release_toolchain
        keys, so staging validates Linux archives exactly as strictly as it
        already validates Windows ones (see validate_runtime_archive_release_toolchain,
        which is generic over `target` and only activates when the key is present)."""
        runtime_contract = rt.load_runtime_archive_contract(
            rt.RUNTIME_ARCHIVE_CONTRACT_PATH
        )
        windows_keys = set(
            runtime_contract["targets"]["windows-x86_64"]["release_toolchain"].keys()
        )
        linux_keys = set(
            runtime_contract["targets"]["linux-x86_64"]["release_toolchain"].keys()
        )
        self.assertEqual(windows_keys, linux_keys)


class ConciseToolVersionTests(unittest.TestCase):
    """Regression coverage for the archive/compiler mismatch bug: GNU
    binutils' `--version` banners never say "version" on their own
    self-identifying first line, but their trailing GPL boilerplate does
    ("...GNU General Public License version 3..."). A naive whole-output
    search for a "version" line therefore picks the wrong line and makes a
    correctly pinned toolchain look unpinned. This is exercised directly
    (rather than only indirectly through PinnedLinuxToolchainTests) because
    it is easy to silently regress."""

    def test_gcc_style_banner_without_a_version_line_falls_back_to_first_line(self):
        output = (
            "x86_64-linux-musl-gcc (GCC) 11.2.1 20211120\n"
            "Copyright (C) 2021 Free Software Foundation, Inc.\n"
            "This is free software; see the source for copying conditions.  There is NO\n"
            "warranty; not even for MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.\n"
        )
        self.assertEqual(
            rt._concise_tool_version(output),
            "x86_64-linux-musl-gcc (GCC) 11.2.1 20211120",
        )

    def test_binutils_gpl_boilerplate_is_not_mistaken_for_the_version_line(self):
        output = (
            "GNU ar (GNU Binutils) 2.37\n"
            "Copyright (C) 2021 Free Software Foundation, Inc.\n"
            "This program is free software; you may redistribute it under the terms of\n"
            "the GNU General Public License version 3 or (at your option) any later version.\n"
            "This program has absolutely no warranty.\n"
        )
        self.assertEqual(rt._concise_tool_version(output), "GNU ar (GNU Binutils) 2.37")

    def test_llvm_style_banner_line_before_the_version_line_is_skipped(self):
        output = "LLVM (http://llvm.org/):\n  LLVM version 22.1.2\n"
        self.assertEqual(rt._concise_tool_version(output), "LLVM version 22.1.2")

    def test_clang_version_on_the_first_line_is_used_directly(self):
        output = "clang version 22.1.2 (https://github.com/llvm/llvm-project)\n"
        self.assertEqual(
            rt._concise_tool_version(output),
            "clang version 22.1.2 (https://github.com/llvm/llvm-project)",
        )

    def test_single_line_banner_without_the_word_version_falls_back_to_first_line(self):
        output = "LLD 22.1.2 (compatible with GNU linkers)\n"
        self.assertEqual(
            rt._concise_tool_version(output), "LLD 22.1.2 (compatible with GNU linkers)"
        )


class VersionCommandTests(unittest.TestCase):
    def test_missing_git_returns_unknown(self):
        with mock.patch.object(rt.subprocess, "run", side_effect=FileNotFoundError()):
            self.assertEqual(rt.git_describe_version(), "unknown")


class RepositoryScratchTests(unittest.TestCase):
    def setUp(self):
        self.scratch = (
            rt.REPO_ROOT
            / "target"
            / "release-tools-unit-tests"
            / self._testMethodName
        )
        rt.ensure_clean_dir(self.scratch)

    def tearDown(self):
        rt.remove_path(self.scratch)


class AtomicPublicationTests(RepositoryScratchTests):
    def _write_pair(self, directory: Path, archive_bytes: bytes, marker: str):
        directory.mkdir(parents=True, exist_ok=True)
        archive = directory / "libosc_runtime_hosted.a"
        manifest = directory / "libosc_runtime_hosted.json"
        archive.write_bytes(archive_bytes)
        manifest.write_text(marker, encoding="utf-8")
        return archive, manifest

    def test_archive_is_published_after_manifest(self):
        staged_archive, staged_manifest = self._write_pair(
            self.scratch / "staged", b"new archive", "new manifest"
        )
        final_archive = self.scratch / "final" / staged_archive.name
        final_manifest = self.scratch / "final" / staged_manifest.name
        calls: list[tuple[Path, Path]] = []
        real_replace = rt.os.replace

        def recording_replace(source, destination):
            calls.append((Path(source), Path(destination)))
            return real_replace(source, destination)

        with mock.patch.object(rt.os, "replace", side_effect=recording_replace):
            rt.publish_archive_pair(
                staged_archive, staged_manifest, final_archive, final_manifest
            )

        self.assertEqual(final_archive.read_bytes(), b"new archive")
        self.assertEqual(final_manifest.read_text(encoding="utf-8"), "new manifest")
        self.assertEqual(calls[-2][1], final_manifest)
        self.assertEqual(calls[-1][1], final_archive)

    def test_failed_archive_publication_restores_previous_pair(self):
        old_archive, old_manifest = self._write_pair(
            self.scratch / "final", b"old archive", "old manifest"
        )
        staged_archive, staged_manifest = self._write_pair(
            self.scratch / "staged", b"new archive", "new manifest"
        )
        real_replace = rt.os.replace

        def fail_new_archive(source, destination):
            if Path(source) == staged_archive:
                raise OSError("simulated archive rename failure")
            return real_replace(source, destination)

        with mock.patch.object(rt.os, "replace", side_effect=fail_new_archive):
            with self.assertRaises(SystemExit):
                rt.publish_archive_pair(
                    staged_archive, staged_manifest, old_archive, old_manifest
                )

        self.assertEqual(old_archive.read_bytes(), b"old archive")
        self.assertEqual(old_manifest.read_text(encoding="utf-8"), "old manifest")

    def test_failed_first_publication_leaves_no_usable_archive(self):
        staged_archive, staged_manifest = self._write_pair(
            self.scratch / "staged", b"new archive", "new manifest"
        )
        final_archive = self.scratch / "final" / staged_archive.name
        final_manifest = self.scratch / "final" / staged_manifest.name
        real_replace = rt.os.replace

        def fail_new_archive(source, destination):
            if Path(source) == staged_archive:
                raise OSError("simulated archive rename failure")
            return real_replace(source, destination)

        with mock.patch.object(rt.os, "replace", side_effect=fail_new_archive):
            with self.assertRaises(SystemExit):
                rt.publish_archive_pair(
                    staged_archive, staged_manifest, final_archive, final_manifest
                )

        self.assertFalse(final_archive.exists())
        self.assertFalse(final_manifest.exists())


class ReleaseStagingTests(RepositoryScratchTests):
    def test_native_runtime_pairs_and_shim_sources_are_staged(self):
        contract = rt.load_release_contract(rt.CONTRACT_PATH)
        target_spec = rt.resolve_release_target(
            contract, rt.CONTRACT_PATH, "windows-x86_64"
        )
        runtime_archive_dir = self.scratch / "archives"
        runtime_archive_dir.mkdir(parents=True)
        runtime_contract = rt.load_runtime_archive_contract(
            rt.RUNTIME_ARCHIVE_CONTRACT_PATH
        )
        for mode in target_spec["native_runtime_modes"]:
            mode_spec = runtime_contract["modes"][mode]
            archive = runtime_archive_dir / mode_spec["archive_name"]
            archive.write_bytes(f"{mode} archive".encode())
            manifest = {
                "schema_version": 1,
                "target": "windows-x86_64",
                "mode": mode,
                "toolchain": _pinned_windows_runtime_toolchain(),
                "sha256": rt.compute_digest(archive, "sha256"),
            }
            (runtime_archive_dir / mode_spec["manifest_name"]).write_text(
                rt.json.dumps(manifest), encoding="utf-8"
            )

        bundle_dir = self.scratch / "bundle"
        rt.stage_native_runtime_assets(
            contract, target_spec, bundle_dir, runtime_archive_dir
        )

        self.assertTrue(
            (bundle_dir / "native-runtime" / "osc_native_shim.c").is_file()
        )
        self.assertTrue((bundle_dir / "native-runtime" / "osc_runtime.h").is_file())
        staged_archives = (
            bundle_dir / "build" / "runtime-archives" / "windows-x86_64"
        )
        for mode in target_spec["native_runtime_modes"]:
            self.assertTrue(
                (staged_archives / f"libosc_runtime_{mode}.a").is_file()
            )
            self.assertTrue(
                (staged_archives / f"libosc_runtime_{mode}.json").is_file()
            )

    def test_native_runtime_pairs_and_shim_sources_are_staged_linux(self):
        """Linux parity for test_native_runtime_pairs_and_shim_sources_are_staged:
        staging must accept a Linux native runtime archive pair whose manifest
        carries the pinned musl toolchain provenance, exactly as it already
        does for Windows."""
        contract = rt.load_release_contract(rt.CONTRACT_PATH)
        target_spec = rt.resolve_release_target(
            contract, rt.CONTRACT_PATH, "linux-x86_64"
        )
        runtime_archive_dir = self.scratch / "archives"
        runtime_archive_dir.mkdir(parents=True)
        runtime_contract = rt.load_runtime_archive_contract(
            rt.RUNTIME_ARCHIVE_CONTRACT_PATH
        )
        for mode in target_spec["native_runtime_modes"]:
            mode_spec = runtime_contract["modes"][mode]
            archive = runtime_archive_dir / mode_spec["archive_name"]
            archive.write_bytes(f"{mode} archive".encode())
            manifest = {
                "schema_version": 1,
                "target": "linux-x86_64",
                "mode": mode,
                "toolchain": _pinned_linux_runtime_toolchain(),
                "embedded_bearssl": mode
                in {"freestanding", "freestanding_core", "hosted"},
                "sha256": rt.compute_digest(archive, "sha256"),
            }
            (runtime_archive_dir / mode_spec["manifest_name"]).write_text(
                rt.json.dumps(manifest), encoding="utf-8"
            )

        bundle_dir = self.scratch / "bundle"
        rt.stage_native_runtime_assets(
            contract, target_spec, bundle_dir, runtime_archive_dir
        )

        self.assertTrue(
            (bundle_dir / "native-runtime" / "osc_native_shim.c").is_file()
        )
        self.assertTrue((bundle_dir / "native-runtime" / "osc_runtime.h").is_file())
        staged_archives = (
            bundle_dir / "build" / "runtime-archives" / "linux-x86_64"
        )
        for mode in target_spec["native_runtime_modes"]:
            self.assertTrue(
                (staged_archives / f"libosc_runtime_{mode}.a").is_file()
            )
            self.assertTrue(
                (staged_archives / f"libosc_runtime_{mode}.json").is_file()
            )

    def test_make_archive_targets_delegate_to_canonical_builder(self):
        makefile = (rt.REPO_ROOT / "runtime" / "Makefile").read_text(
            encoding="utf-8"
        )
        self.assertIn("scripts/release_tools.py build-runtime-archive", makefile)
        self.assertNotIn("TARGET       ?= host", makefile)
        self.assertNotIn("$(AR) rcs $(HOSTED_ARCHIVE)", makefile)
        self.assertNotIn("$(AR) rcs $(FREESTANDING_ARCHIVE)", makefile)

    def test_release_assembly_uses_pinned_tools_and_an_isolated_archive_dir(self):
        assembly = (rt.REPO_ROOT / "scripts" / "assemble-release.ps1").read_text(
            encoding="utf-8"
        )
        self.assertIn('ToolchainManifest"] = $manifestPath', assembly)
        self.assertIn('OutDir = $runtimeArchiveDir', assembly)
        self.assertIn('RuntimeArchiveDir"] = $runtimeArchiveDir', assembly)

    def test_staging_rejects_a_non_pinned_windows_runtime_toolchain(self):
        contract = rt.load_release_contract(rt.CONTRACT_PATH)
        target_spec = rt.resolve_release_target(
            contract, rt.CONTRACT_PATH, "windows-x86_64"
        )
        runtime_archive_dir = self.scratch / "archives"
        runtime_archive_dir.mkdir(parents=True)
        runtime_contract = rt.load_runtime_archive_contract(
            rt.RUNTIME_ARCHIVE_CONTRACT_PATH
        )
        for mode in target_spec["native_runtime_modes"]:
            mode_spec = runtime_contract["modes"][mode]
            archive = runtime_archive_dir / mode_spec["archive_name"]
            archive.write_bytes(f"{mode} archive".encode())
            bad_toolchain = _pinned_windows_runtime_toolchain()
            bad_toolchain["compiler"]["family"] = "gcc"
            manifest = {
                "schema_version": 1,
                "target": "windows-x86_64",
                "mode": mode,
                "toolchain": bad_toolchain,
                "sha256": rt.compute_digest(archive, "sha256"),
            }
            (runtime_archive_dir / mode_spec["manifest_name"]).write_text(
                rt.json.dumps(manifest), encoding="utf-8"
            )

        with self.assertRaises(SystemExit) as ctx:
            rt.stage_native_runtime_assets(
                contract,
                target_spec,
                self.scratch / "bundle",
                runtime_archive_dir,
            )
        self.assertIn("compiler family mismatch", str(ctx.exception))

    def test_staging_rejects_a_non_pinned_linux_runtime_toolchain(self):
        """Linux parity for test_staging_rejects_a_non_pinned_windows_runtime_toolchain:
        this is the exact enforcement that was previously missing for Linux
        (validate_runtime_archive_release_toolchain silently no-ops when
        targets.linux-x86_64.release_toolchain is absent from the runtime
        archive contract), which is how a host-gcc-built archive could ship
        next to the packaged musl cross-compiler without staging ever
        noticing the mismatch."""
        contract = rt.load_release_contract(rt.CONTRACT_PATH)
        target_spec = rt.resolve_release_target(
            contract, rt.CONTRACT_PATH, "linux-x86_64"
        )
        runtime_archive_dir = self.scratch / "archives"
        runtime_archive_dir.mkdir(parents=True)
        runtime_contract = rt.load_runtime_archive_contract(
            rt.RUNTIME_ARCHIVE_CONTRACT_PATH
        )
        for mode in target_spec["native_runtime_modes"]:
            mode_spec = runtime_contract["modes"][mode]
            archive = runtime_archive_dir / mode_spec["archive_name"]
            archive.write_bytes(f"{mode} archive".encode())
            bad_toolchain = _pinned_linux_runtime_toolchain()
            bad_toolchain["compiler"]["target"] = "x86_64-linux-gnu"
            manifest = {
                "schema_version": 1,
                "target": "linux-x86_64",
                "mode": mode,
                "toolchain": bad_toolchain,
                "sha256": rt.compute_digest(archive, "sha256"),
            }
            (runtime_archive_dir / mode_spec["manifest_name"]).write_text(
                rt.json.dumps(manifest), encoding="utf-8"
            )

        with self.assertRaises(SystemExit) as ctx:
            rt.stage_native_runtime_assets(
                contract,
                target_spec,
                self.scratch / "bundle",
                runtime_archive_dir,
            )
        self.assertIn("compiler target mismatch", str(ctx.exception))


class RuntimeArchiveContractShimSchemaTests(unittest.TestCase):
    """docs/design/native-link-embedding.md §3.1: schema_version bumped to 2,
    with osc_native_shim.c precompiled into every mode's archive."""

    def test_schema_version_is_bumped_to_2_with_shim_in_every_mode(self):
        contract = rt.load_runtime_archive_contract(rt.RUNTIME_ARCHIVE_CONTRACT_PATH)
        self.assertEqual(contract["schema_version"], 2)
        for mode_name, mode_spec in contract["modes"].items():
            self.assertIn(
                "osc_native_shim.c",
                mode_spec["sources"],
                f"mode '{mode_name}' is missing osc_native_shim.c in its sources",
            )
            self.assertTrue(
                mode_spec.get("contains_native_shim"),
                f"mode '{mode_name}' does not set contains_native_shim: true",
            )


class NativeShimManifestValidationTests(unittest.TestCase):
    """docs/design/native-link-embedding.md §3.2: validate_runtime_archive_
    release_toolchain must assert the shim member is present for schema_version
    2 archive manifests, without disturbing schema_version 1 (pre-shim)
    manifests."""

    def _runtime_contract(self):
        return rt.load_runtime_archive_contract(rt.RUNTIME_ARCHIVE_CONTRACT_PATH)

    def test_schema_2_manifest_with_shim_passes(self):
        manifest = {
            "schema_version": 2,
            "contains_native_shim": True,
            "native_shim_member": "osc_native_shim.o",
            "toolchain": _pinned_windows_runtime_toolchain(),
        }
        rt.validate_runtime_archive_release_toolchain(
            self._runtime_contract(), "windows-x86_64", manifest, Path("fake.json")
        )

    def test_schema_2_manifest_missing_shim_flag_fails(self):
        manifest = {
            "schema_version": 2,
            "contains_native_shim": False,
            "native_shim_member": None,
            "toolchain": _pinned_windows_runtime_toolchain(),
        }
        with self.assertRaises(SystemExit) as ctx:
            rt.validate_runtime_archive_release_toolchain(
                self._runtime_contract(), "windows-x86_64", manifest, Path("fake.json")
            )
        self.assertIn("contains_native_shim is not true", str(ctx.exception))

    def test_schema_2_manifest_with_wrong_shim_member_fails(self):
        manifest = {
            "schema_version": 2,
            "contains_native_shim": True,
            "native_shim_member": "wrong.o",
            "toolchain": _pinned_windows_runtime_toolchain(),
        }
        with self.assertRaises(SystemExit) as ctx:
            rt.validate_runtime_archive_release_toolchain(
                self._runtime_contract(), "windows-x86_64", manifest, Path("fake.json")
            )
        self.assertIn("native_shim_member is", str(ctx.exception))

    def test_schema_1_manifest_without_shim_fields_is_unaffected(self):
        manifest = {
            "schema_version": 1,
            "toolchain": _pinned_windows_runtime_toolchain(),
        }
        # Legacy pre-shim manifests must not be newly rejected.
        rt.validate_runtime_archive_release_toolchain(
            self._runtime_contract(), "windows-x86_64", manifest, Path("fake.json")
        )


class LinuxReleaseBearSslValidationTests(unittest.TestCase):
    def _runtime_contract(self):
        return rt.load_runtime_archive_contract(rt.RUNTIME_ARCHIVE_CONTRACT_PATH)

    def _manifest(self, mode: str, embedded_bearssl: bool):
        return {
            "schema_version": 2,
            "mode": mode,
            "contains_native_shim": True,
            "native_shim_member": "osc_native_shim.o",
            "embedded_bearssl": embedded_bearssl,
            "toolchain": _pinned_linux_runtime_toolchain(),
        }

    def test_linux_freestanding_release_rejects_tls_less_archive(self):
        with self.assertRaises(SystemExit) as ctx:
            rt.validate_runtime_archive_release_toolchain(
                self._runtime_contract(),
                "linux-x86_64",
                self._manifest("freestanding", False),
                Path("fake.json"),
            )
        self.assertIn("does not embed BearSSL", str(ctx.exception))

    def test_linux_freestanding_release_accepts_embedded_bearssl(self):
        rt.validate_runtime_archive_release_toolchain(
            self._runtime_contract(),
            "linux-x86_64",
            self._manifest("freestanding_core", True),
            Path("fake.json"),
        )

    def test_linux_hosted_release_rejects_tls_less_archive(self):
        with self.assertRaises(SystemExit) as ctx:
            rt.validate_runtime_archive_release_toolchain(
                self._runtime_contract(),
                "linux-x86_64",
                self._manifest("hosted", False),
                Path("fake.json"),
            )
        self.assertIn("does not embed BearSSL", str(ctx.exception))

    def test_linux_hosted_release_accepts_embedded_bearssl(self):
        rt.validate_runtime_archive_release_toolchain(
            self._runtime_contract(),
            "linux-x86_64",
            self._manifest("hosted", True),
            Path("fake.json"),
        )


class PrepareEmbedAssetsTests(RepositoryScratchTests):
    """The prepare-embed-assets subcommand stages each target's exact linker
    payload and writes the manifest consumed by the Rust asset reader."""

    def _write_fake_toolchain_manifest(self, path: Path) -> None:
        manifest = {
            "schema_version": 1,
            "target": "windows-x86_64",
            "bundle_kind": "full",
            "toolchain": {
                "vendor": "llvm-mingw",
                "version": "20260324",
                "archive": {
                    "url": "https://example.invalid/llvm-mingw.zip",
                    "type": "zip",
                    "digest": {"algorithm": "sha256", "value": "fake-digest-value"},
                },
            },
            "stage": {"root": "toolchain"},
        }
        path.write_text(rt.json.dumps(manifest), encoding="utf-8")

    def _write_fake_toolchain_dir(self, toolchain_dir: Path) -> None:
        files = {
            "bin/ld.lld.exe": b"fake-linker-bytes",
            "bin/libLLVM-22.dll": b"fake-libllvm",
            "bin/libwinpthread-1.dll": b"fake-libwinpthread",
            "bin/libunwind.dll": b"fake-libunwind",
            "bin/libffi-8.dll": b"fake-libffi",
            "bin/libc++.dll": b"fake-libcxx",
            "x86_64-w64-mingw32/lib/libkernel32.a": b"fake-kernel32",
            "x86_64-w64-mingw32/lib/libws2_32.a": b"fake-ws2_32",
            "x86_64-w64-mingw32/lib/libuser32.a": b"fake-user32",
            "x86_64-w64-mingw32/lib/libgdi32.a": b"fake-gdi32",
            "x86_64-w64-mingw32/lib/libsecur32.a": b"fake-secur32",
            "x86_64-w64-mingw32/lib/libcrypt32.a": b"fake-crypt32",
            "lib/clang/22/lib/windows/libclang_rt.builtins-x86_64.a": b"fake-builtins",
        }
        for rel, content in files.items():
            dest = toolchain_dir / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(content)

    def test_stages_exactly_thirteen_files_with_matching_manifest_digests(self):
        toolchain_dir = self.scratch / "toolchain"
        self._write_fake_toolchain_dir(toolchain_dir)
        manifest_path = self.scratch / "windows-x86_64.json"
        self._write_fake_toolchain_manifest(manifest_path)
        output_dir = self.scratch / "prebuilt"

        manifest = rt.prepare_embed_assets(
            "windows-x86_64", toolchain_dir, manifest_path, output_dir
        )

        self.assertEqual(manifest["schema_version"], 1)
        self.assertEqual(manifest["target"], "windows-x86_64")
        self.assertEqual(manifest["toolchain"]["vendor"], "llvm-mingw")
        self.assertEqual(manifest["toolchain"]["version"], "20260324")
        self.assertEqual(
            manifest["toolchain"]["archive_digest"],
            {"algorithm": "sha256", "value": "fake-digest-value"},
        )
        self.assertEqual(manifest["linker"]["role"], "linker")
        self.assertEqual(manifest["linker"]["install_subpath"], "bin/ld.lld.exe")
        self.assertEqual(manifest["linker"]["flavor"], "mingw")
        self.assertEqual(manifest["linker"]["emulation"], "i386pep")
        self.assertEqual(len(manifest["assets"]), 12)
        roles = [asset["role"] for asset in manifest["assets"]]
        self.assertEqual(roles.count("linker_runtime"), 5)
        self.assertEqual(roles.count("import_lib"), 6)
        self.assertEqual(roles.count("compiler_builtins"), 1)
        linker_runtime_names = {
            asset["name"] for asset in manifest["assets"] if asset["role"] == "linker_runtime"
        }
        self.assertEqual(
            linker_runtime_names,
            {
                "libLLVM-22.dll",
                "libwinpthread-1.dll",
                "libunwind.dll",
                "libffi-8.dll",
                "libc++.dll",
            },
        )
        libs = {
            asset["lib"] for asset in manifest["assets"] if asset["role"] == "import_lib"
        }
        self.assertEqual(
            libs, {"kernel32", "ws2_32", "user32", "gdi32", "secur32", "crypt32"}
        )

        staged_files = sorted(
            p.relative_to(output_dir).as_posix()
            for p in output_dir.rglob("*")
            if p.is_file()
        )
        self.assertEqual(
            staged_files,
            sorted(
                [
                    "native-link-assets.json",
                    "bin/ld.lld.exe",
                    "bin/libLLVM-22.dll",
                    "bin/libwinpthread-1.dll",
                    "bin/libunwind.dll",
                    "bin/libffi-8.dll",
                    "bin/libc++.dll",
                    "lib/libkernel32.a",
                    "lib/libws2_32.a",
                    "lib/libuser32.a",
                    "lib/libgdi32.a",
                    "lib/libsecur32.a",
                    "lib/libcrypt32.a",
                    "lib/clang/libclang_rt.builtins-x86_64.a",
                ]
            ),
        )

    def test_linux_stages_only_the_static_linker(self):
        toolchain_dir = self.scratch / "toolchain-linux"
        linker = toolchain_dir / "bin" / "x86_64-linux-musl-ld"
        linker.parent.mkdir(parents=True)
        linker.write_bytes(b"fake-static-linux-linker")

        manifest_path = self.scratch / "linux-x86_64.json"
        manifest_path.write_text(
            rt.json.dumps(
                {
                    "schema_version": 1,
                    "target": "linux-x86_64",
                    "bundle_kind": "full",
                    "toolchain": {
                        "vendor": "musl.cc",
                        "version": "gcc-11.2.1-binutils-2.37",
                        "archive": {
                            "url": "https://example.invalid/musl-cross.tgz",
                            "type": "tar.gz",
                            "digest": {
                                "algorithm": "sha256",
                                "value": "fake-linux-digest",
                            },
                        },
                    },
                    "stage": {"root": "toolchain"},
                }
            ),
            encoding="utf-8",
        )
        output_dir = self.scratch / "prebuilt-linux"

        manifest = rt.prepare_embed_assets(
            "linux-x86_64", toolchain_dir, manifest_path, output_dir
        )

        self.assertEqual(manifest["target"], "linux-x86_64")
        self.assertEqual(manifest["linker"]["flavor"], "elf")
        self.assertEqual(manifest["linker"]["emulation"], "elf_x86_64")
        self.assertEqual(
            manifest["linker"]["install_subpath"],
            "linker/x86_64-linux-musl-ld",
        )
        self.assertEqual(manifest["assets"], [])
        self.assertEqual(
            sorted(
                p.relative_to(output_dir).as_posix()
                for p in output_dir.rglob("*")
                if p.is_file()
            ),
            [
                "linker/x86_64-linux-musl-ld",
                "native-link-assets.json",
            ],
        )

        for entry in [manifest["linker"], *manifest["assets"]]:
            staged_path = output_dir / entry["install_subpath"]
            self.assertEqual(staged_path.stat().st_size, entry["size"])
            self.assertEqual(rt.compute_digest(staged_path, "sha256"), entry["sha256"])

        written_manifest = rt.json.loads(
            (output_dir / "native-link-assets.json").read_text(encoding="utf-8")
        )
        self.assertEqual(written_manifest, manifest)

    def test_fails_actionably_when_toolchain_dir_missing(self):
        with self.assertRaises(SystemExit) as ctx:
            rt.prepare_embed_assets(
                "windows-x86_64",
                self.scratch / "does-not-exist",
                self.scratch / "windows-x86_64.json",
                self.scratch / "prebuilt",
            )
        self.assertIn("run fetch-toolchain first", str(ctx.exception))

    def test_fails_for_an_unsupported_target(self):
        toolchain_dir = self.scratch / "toolchain"
        toolchain_dir.mkdir(parents=True)
        with self.assertRaises(SystemExit) as ctx:
            rt.prepare_embed_assets(
                "macos-x86_64",
                toolchain_dir,
                self.scratch / "macos-x86_64.json",
                self.scratch / "prebuilt",
            )
        self.assertIn(
            "does not know the embedded native-link asset", str(ctx.exception)
        )

    def test_ambiguous_compiler_builtins_glob_fails_actionably(self):
        toolchain_dir = self.scratch / "toolchain"
        self._write_fake_toolchain_dir(toolchain_dir)
        # A second clang version directory makes the builtins glob ambiguous.
        extra = (
            toolchain_dir
            / "lib"
            / "clang"
            / "23"
            / "lib"
            / "windows"
            / "libclang_rt.builtins-x86_64.a"
        )
        extra.parent.mkdir(parents=True, exist_ok=True)
        extra.write_bytes(b"another-fake-builtins")
        manifest_path = self.scratch / "windows-x86_64.json"
        self._write_fake_toolchain_manifest(manifest_path)

        with self.assertRaises(SystemExit) as ctx:
            rt.prepare_embed_assets(
                "windows-x86_64",
                toolchain_dir,
                manifest_path,
                self.scratch / "prebuilt",
            )
        self.assertIn("ambiguous embedded asset source", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
