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


if __name__ == "__main__":
    unittest.main()
