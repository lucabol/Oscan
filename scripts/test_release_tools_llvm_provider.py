#!/usr/bin/env python3
"""Tests for the packaged LLVM code generator's release contract.

`--backend llvm` loads Oscan's own LLVM shared library in-process; it
never invokes `clang`/`llvm-as`/`opt`/`llc` and never consults an
installed LLVM SDK. That makes the library a *release artifact*, so the
packaging pipeline has to guarantee it is actually in the bundle. These
tests cover the manifest declaration, the staged-tree verification, and
the interaction with the glob-driven prune step that would otherwise be
free to delete it.

Run with:
    python scripts/test_release_tools_llvm_provider.py
or:
    python -m unittest scripts.test_release_tools_llvm_provider -v
"""
from __future__ import annotations

import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_tools as rt  # noqa: E402

TOOLCHAINS = rt.REPO_ROOT / "packaging" / "toolchains"


class LlvmCodeGeneratorManifestTests(unittest.TestCase):
    def test_every_full_bundle_manifest_declares_its_llvm_status(self) -> None:
        for manifest_path in sorted(TOOLCHAINS.glob("*-*.json")):
            if manifest_path.name in ("release-contract.json", "runtime-archive-contract.json"):
                continue
            manifest = rt.load_manifest(manifest_path)
            spec = rt.llvm_code_generator_spec(manifest)
            self.assertIsNotNone(
                spec,
                f"{manifest_path.name} must declare toolchain.llvm_code_generator so the "
                "availability of --backend llvm for that target is data, not a surprise",
            )
            self.assertIn(spec["status"], ("present", "absent"), manifest_path.name)
            self.assertEqual(
                spec["required_major"],
                22,
                f"{manifest_path.name} must pin the LLVM C API major this compiler binds",
            )
            self.assertTrue(spec["search_names"], manifest_path.name)

    def test_windows_bundle_ships_the_code_generator(self) -> None:
        manifest = rt.load_manifest(TOOLCHAINS / "windows-x86_64.json")
        spec = rt.llvm_code_generator_spec(manifest)
        self.assertEqual(spec["status"], "present")
        self.assertEqual(spec["path"], "bin/libLLVM-22.dll")

    def test_windows_prune_keeps_the_code_generator(self) -> None:
        manifest = rt.load_manifest(TOOLCHAINS / "windows-x86_64.json")
        keep = manifest["toolchain"]["prune"]["keep_globs"]
        self.assertIn(
            "bin/libLLVM-*.dll",
            keep,
            "a prune rule must never be able to delete the LLVM code generator",
        )

    def test_linux_bundle_overlays_a_pinned_codegen_only_provider(self) -> None:
        manifest = rt.load_manifest(TOOLCHAINS / "linux-x86_64.json")
        spec = rt.llvm_code_generator_spec(manifest)
        self.assertEqual(spec["status"], "present")
        self.assertEqual(spec["path"], "lib/libLLVM.so.22.1")
        self.assertEqual(spec["archive"]["type"], "tar.xz")
        self.assertRegex(spec["archive"]["digest"]["value"], r"^[0-9a-f]{64}$")
        self.assertTrue(
            any(entry["path"] == spec["path"] for entry in spec["files"]),
            "the provider overlay must stage the exact library path Oscan searches",
        )
        self.assertNotIn("clang", spec["archive"]["url"].lower())
        self.assertEqual(spec["runtime_dependencies"][0], "glibc >= 2.34")
        self.assertIn("libz3-4", spec["debian_packages"])

    def test_release_contract_documents_the_lookup_rules(self) -> None:
        contract = json.loads((TOOLCHAINS / "release-contract.json").read_text(encoding="utf-8"))
        spec = contract["llvm_code_generator"]
        self.assertEqual(spec["required_major"], 22)
        # A code generator is executed code: it must never be loadable
        # from the working directory or PATH.
        self.assertIn("the current working directory", spec["never_searched"])
        self.assertIn("PATH", spec["never_searched"])
        self.assertIn("$OSCAN_LLVM_LIB (full path to the library)", spec["search_roots"])
        for name in ("libLLVM-22.dll", "LLVM-C.dll"):
            self.assertIn(name, spec["search_names"]["windows"])
        for name in ("libLLVM.so.22", "libLLVM.so"):
            self.assertIn(name, spec["search_names"]["linux"])


class VerifyLlvmCodeGeneratorTests(unittest.TestCase):
    def _manifest(self, spec: dict | None) -> dict:
        manifest: dict = {"toolchain": {}}
        if spec is not None:
            manifest["toolchain"]["llvm_code_generator"] = spec
        return manifest

    def test_absent_declaration_is_accepted_and_returns_none(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result = rt.verify_llvm_code_generator(
                Path(tmp),
                self._manifest({"status": "absent", "path": None, "required_major": 22}),
            )
            self.assertIsNone(result)

    def test_missing_declaration_is_accepted_and_returns_none(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self.assertIsNone(rt.verify_llvm_code_generator(Path(tmp), self._manifest(None)))

    def test_present_declaration_requires_the_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            spec = {"status": "present", "path": "bin/libLLVM-22.dll", "required_major": 22}
            with self.assertRaises(SystemExit):
                rt.verify_llvm_code_generator(root, self._manifest(spec))

            (root / "bin").mkdir()
            (root / "bin" / "libLLVM-22.dll").write_bytes(b"not really a dll, but non-empty")
            verified = rt.verify_llvm_code_generator(root, self._manifest(spec))
            self.assertEqual(verified, root / "bin" / "libLLVM-22.dll")

    def test_an_empty_staged_library_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "bin").mkdir()
            (root / "bin" / "libLLVM-22.dll").write_bytes(b"")
            spec = {"status": "present", "path": "bin/libLLVM-22.dll", "required_major": 22}
            with self.assertRaises(SystemExit):
                rt.verify_llvm_code_generator(root, self._manifest(spec))

    def test_a_path_escaping_the_toolchain_root_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            spec = {"status": "present", "path": "../evil.dll", "required_major": 22}
            with self.assertRaises(SystemExit):
                rt.verify_llvm_code_generator(Path(tmp), self._manifest(spec))

    def test_a_present_status_without_a_path_is_rejected(self) -> None:
        with self.assertRaises(SystemExit):
            rt.llvm_code_generator_spec(
                {"toolchain": {"llvm_code_generator": {"status": "present", "path": None}}}
            )

    def test_an_unknown_status_is_rejected(self) -> None:
        with self.assertRaises(SystemExit):
            rt.llvm_code_generator_spec(
                {"toolchain": {"llvm_code_generator": {"status": "maybe", "path": "x"}}}
            )


class PruneInteractionTests(unittest.TestCase):
    def test_a_keep_glob_protects_the_code_generator_from_a_remove_glob(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "bin").mkdir()
            (root / "bin" / "libLLVM-22.dll").write_bytes(b"code generator")
            (root / "bin" / "libclang-cpp.dll").write_bytes(b"not needed")
            rt.prune_toolchain(
                root,
                {
                    "remove_globs": ["bin/lib*.dll"],
                    "keep_globs": ["bin/libLLVM-*.dll"],
                },
            )
            self.assertTrue((root / "bin" / "libLLVM-22.dll").is_file())
            self.assertFalse((root / "bin" / "libclang-cpp.dll").exists())


class ProviderOverlayTests(unittest.TestCase):
    def test_cached_pinned_overlay_stages_only_declared_files(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            source_root = base / "source" / "llvm-provider"
            (source_root / "lib").mkdir(parents=True)
            (source_root / "LICENSES").mkdir()
            (source_root / "lib" / "libLLVM.so.22.1").write_bytes(b"llvm provider")
            (source_root / "LICENSES" / "copyright").write_text("license", encoding="utf-8")
            (source_root / "unlisted-tool").write_text("must not be staged", encoding="utf-8")

            archive_base = base / "downloads" / "provider"
            archive_base.parent.mkdir()
            archive_path = Path(
                shutil.make_archive(
                    str(archive_base),
                    "xztar",
                    root_dir=source_root.parent,
                    base_dir=source_root.name,
                )
            )
            digest = rt.compute_digest(archive_path, "sha256")
            manifest = {
                "target": "linux-x86_64",
                "toolchain": {
                    "llvm_code_generator": {
                        "status": "present",
                        "path": "lib/libLLVM.so.22.1",
                        "required_major": 22,
                        "archive": {
                            "url": f"https://example.invalid/{archive_path.name}",
                            "type": "tar.xz",
                            "digest": {"algorithm": "sha256", "value": digest},
                        },
                        "extract": {"strip_components": 1},
                        "files": [
                            {
                                "source": "lib/libLLVM.so.22.1",
                                "path": "lib/libLLVM.so.22.1",
                            },
                            {
                                "source": "LICENSES/copyright",
                                "path": "licenses/llvm/copyright",
                            },
                        ],
                    }
                },
            }
            destination = base / "toolchain"
            destination.mkdir()

            rt.stage_llvm_code_generator(destination, manifest, archive_path.parent)

            self.assertEqual(
                (destination / "lib" / "libLLVM.so.22.1").read_bytes(),
                b"llvm provider",
            )
            self.assertEqual(
                (destination / "licenses" / "llvm" / "copyright").read_text(encoding="utf-8"),
                "license",
            )
            self.assertFalse((destination / "unlisted-tool").exists())


if __name__ == "__main__":
    unittest.main()
