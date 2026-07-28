"""Release contract v2 + variant-aware packaging tests.

Everything here is offline and hermetic: each test builds a complete fake
package input set (tiny binaries, a tiny native-link asset set, tiny
runtime archives, and tiny *source archives* for the pinned C toolchain and
LLVM provider) in a temporary directory and stages real archives from it.
No network, no fetched toolchain, and no dependency on any path outside the
repository.

The toolchain and provider fixtures deliberately mirror the real trust
model: staging is handed an archive, and a temporary copy of the packaging
manifests pins that archive's digest. A test that wants to prove a
rejection therefore tampers with something an attacker could actually
control — the archive bytes, an archive member, or the payload inside it.
"""

from pathlib import Path
import copy
import gzip
import hashlib
import io
import json
import os
import shutil
import tarfile
import tempfile
import time
import unittest
import zipfile

import release_tools as rt


TOOLCHAINS = rt.REPO_ROOT / "packaging" / "toolchains"
ALL_VARIANTS = (
    ("windows-x86_64", "llvm"),
    ("windows-x86_64", "cranelift"),
    ("windows-x86_64", "c"),
    ("linux-x86_64", "llvm"),
    ("linux-x86_64", "cranelift"),
    ("linux-x86_64", "c"),
    ("macos-x86_64", "c"),
)


def sha256_of(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_entry(name: str, data: bytes, mode: int = 0o644) -> dict:
    return {"name": name, "kind": "file", "data": data, "mode": mode}


def dir_entry(name: str) -> dict:
    return {"name": name, "kind": "dir", "mode": 0o755}


def symlink_entry(name: str, target: str) -> dict:
    return {"name": name, "kind": "symlink", "target": target, "mode": 0o777}


def hardlink_entry(name: str, target: str) -> dict:
    return {"name": name, "kind": "hardlink", "target": target, "mode": 0o644}


def build_tar_archive(path: Path, entries: list[dict], compression: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)

    def write(archive: tarfile.TarFile) -> None:
        for entry in entries:
            info = tarfile.TarInfo(entry["name"])
            info.mode = entry.get("mode", 0o644)
            kind = entry.get("kind", "file")
            if kind == "dir":
                info.type = tarfile.DIRTYPE
                archive.addfile(info)
            elif kind == "symlink":
                info.type = tarfile.SYMTYPE
                info.linkname = entry["target"]
                archive.addfile(info)
            elif kind == "hardlink":
                info.type = tarfile.LNKTYPE
                info.linkname = entry["target"]
                archive.addfile(info)
            else:
                data = entry["data"]
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))

    if compression == "gz":
        # A gzip header records the clock, which would make the fixture's
        # own pinned digest change between runs and defeat the
        # reproducibility tests.
        with path.open("wb") as raw:
            with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w") as archive:
                    write(archive)
        return path
    with tarfile.open(path, f"w:{compression}") as archive:
        write(archive)
    return path


def build_zip_archive(path: Path, entries: list[dict]) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        for entry in entries:
            kind = entry.get("kind", "file")
            mode = entry.get("mode", 0o644)
            name = entry["name"] + ("/" if kind == "dir" else "")
            info = zipfile.ZipInfo(name)
            info.create_system = 3
            if kind == "dir":
                info.external_attr = ((0o40000 | mode) << 16) | 0x10
                archive.writestr(info, b"")
            elif kind == "symlink":
                info.external_attr = (0o120000 | mode) << 16
                archive.writestr(info, entry["target"])
            else:
                info.external_attr = (0o100000 | mode) << 16
                archive.writestr(info, entry["data"])
    return path


class PackagingFixture:
    """A complete, tiny, offline package input set.

    The C toolchain and the LLVM provider are supplied the way release
    staging really consumes them — as digest-pinned source archives — and
    the packaging manifests are copied into the fixture so their pinned
    digests can name those archives instead of the multi-hundred-megabyte
    upstream ones.
    """

    def __init__(self, root: Path, target: str) -> None:
        self.root = root
        self.target = target
        self.platform = target.split("-", 1)[0]
        self.binary = root / "bin" / ("oscan.exe" if self.platform == "windows" else "oscan")
        self.binary.parent.mkdir(parents=True, exist_ok=True)
        self.binary.write_bytes(b"fake oscan binary")

        self.native_link_dir = root / "native-link-input"
        self.runtime_dir = root / "runtime-archives"
        self.archive_dir = root / "archives"
        self.packaging_dir = root / "packaging"
        self.repo_manifest_path = TOOLCHAINS / f"{target}.json"
        self.toolchain_archive: Path | None = None
        self.provider_archive: Path | None = None

        self._copy_packaging()
        self._write_toolchain_archive()
        self._write_provider_archive()
        self._write_native_link()
        self._write_runtime_archives()

    # -- packaging manifests -------------------------------------------------

    def _copy_packaging(self) -> None:
        self.packaging_dir.mkdir(parents=True, exist_ok=True)
        for path in sorted(TOOLCHAINS.iterdir()):
            if path.is_file():
                shutil.copy2(path, self.packaging_dir / path.name)
        self.contract_path = self.packaging_dir / "release-contract.json"
        candidate = self.packaging_dir / f"{self.target}.json"
        self.manifest_path = candidate if candidate.is_file() else None

    def manifest(self) -> dict:
        assert self.manifest_path is not None
        return json.loads(self.manifest_path.read_text(encoding="utf-8"))

    def write_manifest(self, manifest: dict) -> None:
        assert self.manifest_path is not None
        self.manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")

    def _pin(self, section: dict, archive_path: Path) -> None:
        section["url"] = f"https://example.invalid/{archive_path.name}"
        section["digest"] = {
            "algorithm": "sha256",
            "value": rt.compute_digest(archive_path, "sha256"),
        }

    # -- pinned C toolchain source archive -----------------------------------

    def toolchain_entries(self) -> list[dict]:
        return copy.deepcopy(self._toolchain_entries)

    def _default_toolchain_entries(self, manifest: dict) -> list[dict]:
        toolchain = manifest["toolchain"]
        runtime = toolchain["runtime"]
        root = self.toolchain_root
        entries = [dir_entry(root)]
        for role in ("compiler", "archiver", "linker"):
            entries.append(
                file_entry(f"{root}/{runtime[role]['path']}", f"fake {role}".encode(), 0o755)
            )
        spec = toolchain.get("llvm_code_generator") or {}
        if spec.get("status") == "present" and not spec.get("archive"):
            # Windows' llvm-mingw integrates libLLVM into the toolchain
            # archive itself; clang.exe links against it.
            entries.append(file_entry(f"{root}/{spec['path']}", b"integrated libLLVM"))
        entries.append(file_entry(f"{root}/LICENSE.txt", b"toolchain license\n"))
        if self.platform == "windows":
            entries.extend(
                [
                    file_entry(f"{root}/COPYING", b"copying\n"),
                    file_entry(f"{root}/x86_64-w64-mingw32/include/stdio.h", b"/* fake */\n"),
                    # Pruned by the manifest's remove_globs.
                    file_entry(f"{root}/bin/llvm-objdump.exe", b"pruned", 0o755),
                    file_entry(f"{root}/share/doc/readme.txt", b"pruned"),
                ]
            )
        else:
            entries.extend(
                [
                    file_entry(f"{root}/COPYING", b"copying\n"),
                    file_entry(f"{root}/x86_64-linux-musl/include/stdio.h", b"/* fake */\n"),
                    file_entry(f"{root}/x86_64-linux-musl/lib/libc.so", b"fake libc"),
                    # A sysroot-relative absolute symlink, exactly as the
                    # musl cross toolchain ships one.
                    symlink_entry(
                        f"{root}/x86_64-linux-musl/lib/ld-musl-x86_64.so.1", "/lib/libc.so"
                    ),
                    symlink_entry(f"{root}/bin/cc", "x86_64-linux-musl-gcc"),
                    # Pruned by the manifest's remove_globs.
                    file_entry(f"{root}/lib/libstdc++.a", b"pruned"),
                    file_entry(f"{root}/share/man/man1/gcc.1", b"pruned"),
                ]
            )
        return entries

    def _write_toolchain_archive(self) -> None:
        if self.manifest_path is None:
            return
        manifest = self.manifest()
        archive_type = manifest["toolchain"]["archive"]["type"]
        self.toolchain_root = f"{self.target}-toolchain"
        self._toolchain_entries = self._default_toolchain_entries(manifest)
        self.toolchain_archive = self.rebuild_toolchain_archive(self._toolchain_entries)

    def rebuild_toolchain_archive(self, entries: list[dict]) -> Path:
        manifest = self.manifest()
        archive_type = manifest["toolchain"]["archive"]["type"]
        suffix = {"zip": ".zip", "tgz": ".tar.gz", "tar.gz": ".tar.gz", "tar.xz": ".tar.xz"}[
            archive_type
        ]
        path = self.archive_dir / f"{self.target}-toolchain{suffix}"
        if archive_type == "zip":
            build_zip_archive(path, entries)
        else:
            build_tar_archive(path, entries, "gz" if suffix == ".tar.gz" else "xz")
        self._pin(manifest["toolchain"]["archive"], path)
        self.write_manifest(manifest)
        self.toolchain_archive = path
        return path

    # -- pinned LLVM provider source archive ---------------------------------

    def provider_entries(self) -> list[dict]:
        return copy.deepcopy(self._provider_entries)

    def _write_provider_archive(self) -> None:
        if self.manifest_path is None:
            return
        manifest = self.manifest()
        spec = manifest["toolchain"].get("llvm_code_generator") or {}
        if spec.get("status") != "present" or not spec.get("archive"):
            return
        root = f"{self.target}-llvm-provider"
        self.provider_root = root
        entries = [dir_entry(root)]
        for file_spec in spec["files"]:
            entries.append(
                file_entry(
                    f"{root}/{file_spec['source']}",
                    f"provider payload for {file_spec['source']}".encode(),
                )
            )
        # The real pinned provider archive ships SONAME alias links beside
        # the library; they are contained, relative, and undeclared.
        entries.append(symlink_entry(f"{root}/lib/libLLVM.so.22", "libLLVM.so.22.1"))
        entries.append(symlink_entry(f"{root}/lib/libLLVM-22.so", "libLLVM.so.22.1"))
        # Members the manifest does not declare must never reach a package.
        entries.append(file_entry(f"{root}/unlisted-tool", b"must not be staged", 0o755))
        entries.append(file_entry(f"{root}/bin/llvm-config", b"must not be staged", 0o755))
        self._provider_entries = entries
        self.provider_archive = self.rebuild_provider_archive(entries)

    def rebuild_provider_archive(self, entries: list[dict]) -> Path:
        manifest = self.manifest()
        spec = manifest["toolchain"]["llvm_code_generator"]
        archive_type = spec["archive"]["type"]
        suffix = {"zip": ".zip", "tgz": ".tar.gz", "tar.gz": ".tar.gz", "tar.xz": ".tar.xz"}[
            archive_type
        ]
        path = self.archive_dir / f"{self.target}-llvm-provider{suffix}"
        if archive_type == "zip":
            build_zip_archive(path, entries)
        else:
            build_tar_archive(path, entries, "gz" if suffix == ".tar.gz" else "xz")
        self._pin(spec["archive"], path)
        self.write_manifest(manifest)
        self.provider_archive = path
        return path

    # -- prepared (digest-manifested) inputs ---------------------------------

    def _stage_asset(self, subpath: str, payload: bytes) -> str:
        path = self.native_link_dir / subpath
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)
        return sha256_of(payload)

    def _write_native_link(self) -> None:
        self.native_link_dir.mkdir(parents=True, exist_ok=True)
        if self.platform == "windows":
            linker_sub = "bin/ld.lld.exe"
            linker_name = "ld.lld.exe"
            assets = []
            for dll in (
                "libLLVM-22.dll",
                "libc++.dll",
                "libunwind.dll",
                "libwinpthread-1.dll",
                "libffi-8.dll",
            ):
                digest = self._stage_asset(f"bin/{dll}", dll.encode())
                assets.append(
                    {
                        "role": "linker_runtime",
                        "name": dll,
                        "install_subpath": f"bin/{dll}",
                        "sha256": digest,
                    }
                )
            for lib in ("kernel32", "ws2_32", "user32", "gdi32", "secur32", "crypt32"):
                digest = self._stage_asset(f"lib/lib{lib}.a", lib.encode())
                assets.append(
                    {
                        "role": "import_lib",
                        "name": f"lib{lib}.a",
                        "lib": lib,
                        "install_subpath": f"lib/lib{lib}.a",
                        "sha256": digest,
                    }
                )
            digest = self._stage_asset(
                "lib/clang/libclang_rt.builtins-x86_64.a", b"builtins"
            )
            assets.append(
                {
                    "role": "compiler_builtins",
                    "name": "libclang_rt.builtins-x86_64.a",
                    "install_subpath": "lib/clang/libclang_rt.builtins-x86_64.a",
                    "sha256": digest,
                }
            )
        else:
            linker_sub = "linker/x86_64-linux-musl-ld"
            linker_name = "x86_64-linux-musl-ld"
            assets = []
        linker_digest = self._stage_asset(linker_sub, b"fake linker")
        manifest = {
            "schema_version": 1,
            "target": self.target,
            "toolchain": {"vendor": "test", "version": "0"},
            "linker": {
                "role": "linker",
                "name": linker_name,
                "install_subpath": linker_sub,
                "sha256": linker_digest,
            },
            "assets": assets,
        }
        (self.native_link_dir / "native-link-assets.json").write_text(
            json.dumps(manifest, indent=2), encoding="utf-8"
        )

    def _write_runtime_archives(self) -> None:
        if not self.repo_manifest_path.is_file():
            return
        self.runtime_dir.mkdir(parents=True, exist_ok=True)
        contract = rt.load_runtime_archive_contract(rt.RUNTIME_ARCHIVE_CONTRACT_PATH)
        toolchain = self._runtime_toolchain()
        for profile in rt.FREESTANDING_PROFILES:
            mode_spec = contract["modes"][profile]
            archive = self.runtime_dir / mode_spec["archive_name"]
            archive.write_bytes(f"{profile} archive".encode())
            manifest = {
                "schema_version": 1,
                "target": self.target,
                "mode": profile,
                "toolchain": toolchain,
                "sha256": rt.compute_digest(archive, "sha256"),
                # Linux release archives must embed BearSSL; the fixture
                # records the same provenance a real build would.
                "embedded_bearssl": not self.target.startswith("windows"),
            }
            (self.runtime_dir / mode_spec["manifest_name"]).write_text(
                json.dumps(manifest), encoding="utf-8"
            )
        # A hosted pair exists in the prepared directory and must never be
        # staged into an object package.
        hosted = self.runtime_dir / "libosc_runtime_hosted.a"
        hosted.write_bytes(b"hosted archive")

    def _runtime_toolchain(self) -> dict:
        """The pinned provenance block the runtime-archive contract requires.

        Runtime archives are validated against the *repository's* pinned
        manifest, not the fixture's rewritten copy, so this deliberately
        reads the real one.
        """
        source = rt.load_manifest(self.repo_manifest_path)
        runtime = source["toolchain"]["runtime"]
        return {
            "source_manifest": self.repo_manifest_path.name,
            "vendor": source["toolchain"]["vendor"],
            "version": source["toolchain"]["version"],
            "archive_digest": source["toolchain"]["archive"]["digest"],
            "abi": runtime["abi"],
            "crt": runtime["crt"],
            "compiler": {
                "command": "/build/toolchain/" + runtime["compiler"]["path"],
                "family": runtime["compiler"]["family"],
                "version": f"version {runtime['compiler']['version']}",
                "target": runtime["compiler"]["target"],
                "size_flag": runtime["compiler"]["size_flag"],
            },
            "archiver": {
                "command": "/build/toolchain/" + runtime["archiver"]["path"],
                "family": runtime["archiver"]["family"],
                "version": f"version {runtime['archiver']['version']}",
            },
            "linker": runtime.get("linker"),
        }


def stage_namespace(fixture: PackagingFixture, backend: str, output_dir: Path, **overrides):
    kwargs = {
        "target": fixture.target,
        "backend": backend,
        "version": "9.9.9",
        "binary": str(fixture.binary),
        "output_dir": str(output_dir),
        "contract": str(fixture.contract_path),
        "runtime_archive_dir": str(fixture.runtime_dir),
        "native_link_dir": str(fixture.native_link_dir),
        "toolchain_archive": str(fixture.toolchain_archive)
        if fixture.toolchain_archive
        else None,
        "llvm_provider_archive": str(fixture.provider_archive)
        if fixture.provider_archive
        else None,
    }
    kwargs.update(overrides)
    return rt.argparse.Namespace(**kwargs)



class ContractMatrixTests(unittest.TestCase):
    def setUp(self) -> None:
        self.contract = rt.load_release_contract(rt.CONTRACT_PATH)

    def test_the_published_matrix_is_exactly_seven_variants(self) -> None:
        matrix = rt.release_variant_matrix(self.contract)
        self.assertEqual(
            sorted((entry["target"], entry["backend"]) for entry in matrix),
            sorted(ALL_VARIANTS),
        )

    def test_archive_names_are_canonical_and_unique(self) -> None:
        names = set()
        for target, backend in ALL_VARIANTS:
            variant = rt.resolve_release_variant(
                self.contract, rt.CONTRACT_PATH, target, backend
            )
            name = rt.render_release_template(
                variant["archive_name_template"], "1.2.3", "archive_name_template"
            )
            root = rt.render_release_template(
                variant["archive_root_template"], "1.2.3", "archive_root_template"
            )
            suffix = rt.ARCHIVE_SUFFIXES[variant["archive_format"]]
            self.assertEqual(name, f"oscan-v1.2.3-{target}-{backend}{suffix}")
            self.assertEqual(root, f"oscan-v1.2.3-{target}-{backend}")
            self.assertNotIn("full", name)
            self.assertNotIn("native", name)
            self.assertNotIn(name, names)
            names.add(name)

    def test_object_variants_are_toolchain_free_and_c_variants_are_not(self) -> None:
        for target, backend in ALL_VARIANTS:
            variant = rt.resolve_release_variant(
                self.contract, rt.CONTRACT_PATH, target, backend
            )
            with self.subTest(target=target, backend=backend):
                if backend == "c":
                    self.assertFalse(variant["toolchain_free"])
                    self.assertNotIn("direct_link_sidecar", variant["components"])
                    self.assertNotIn("runtime_archives", variant["components"])
                    self.assertNotIn("llvm_provider", variant["components"])
                    self.assertEqual(variant["runtime_profiles"], [])
                else:
                    self.assertTrue(variant["toolchain_free"])
                    self.assertIn("direct_link_sidecar", variant["components"])
                    self.assertIn("runtime_archives", variant["components"])
                    self.assertNotIn("c_toolchain", variant["components"])
                    self.assertEqual(
                        variant["runtime_profiles"],
                        ["freestanding", "freestanding_gfx", "freestanding_core"],
                    )
                    self.assertNotIn("hosted", variant["runtime_profiles"])

    def test_only_llvm_variants_carry_a_provider_and_windows_shares_the_sidecar_copy(self) -> None:
        windows_llvm = rt.resolve_release_variant(
            self.contract, rt.CONTRACT_PATH, "windows-x86_64", "llvm"
        )
        self.assertEqual(windows_llvm["llvm_provider_source"], "direct-link-sidecar")
        self.assertEqual(windows_llvm["llvm_provider_asset"], "libLLVM-22.dll")
        linux_llvm = rt.resolve_release_variant(
            self.contract, rt.CONTRACT_PATH, "linux-x86_64", "llvm"
        )
        self.assertEqual(linux_llvm["llvm_provider_source"], "toolchain-manifest")
        for target in ("windows-x86_64", "linux-x86_64"):
            cranelift = rt.resolve_release_variant(
                self.contract, rt.CONTRACT_PATH, target, "cranelift"
            )
            self.assertNotIn("llvm_provider", cranelift["components"])

    def test_macos_is_c_only(self) -> None:
        self.assertEqual(
            sorted(self.contract["variants"]["macos-x86_64"]["backends"]), ["c"]
        )
        with self.assertRaises(SystemExit):
            rt.resolve_release_variant(
                self.contract, rt.CONTRACT_PATH, "macos-x86_64", "llvm"
            )

    def test_unknown_target_or_backend_is_refused(self) -> None:
        with self.assertRaises(SystemExit):
            rt.resolve_release_variant(
                self.contract, rt.CONTRACT_PATH, "freebsd-x86_64", "c"
            )
        with self.assertRaises(SystemExit):
            rt.resolve_release_variant(
                self.contract, rt.CONTRACT_PATH, "linux-x86_64", "native"
            )


class ContractValidationTests(unittest.TestCase):
    def _contract(self) -> dict:
        return json.loads(rt.CONTRACT_PATH.read_text(encoding="utf-8"))

    def _reject(self, mutate) -> str:
        data = self._contract()
        mutate(data)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "release-contract.json"
            path.write_text(json.dumps(data), encoding="utf-8")
            with self.assertRaises(SystemExit) as caught:
                rt.load_release_contract(path)
            return str(caught.exception)

    def test_the_repository_contract_validates(self) -> None:
        rt.load_release_contract(rt.CONTRACT_PATH)

    def test_schema_1_is_refused(self) -> None:
        message = self._reject(lambda data: data.update(schema_version=1))
        self.assertIn("schema_version 2", message)

    def test_an_invalid_backend_name_is_refused(self) -> None:
        def mutate(data: dict) -> None:
            data["backends"]["native"] = data["backends"].pop("cranelift")

        self.assertIn("canonical backends", self._reject(mutate))

    def test_a_mismatched_cargo_feature_is_refused(self) -> None:
        def mutate(data: dict) -> None:
            data["variants"]["linux-x86_64"]["backends"]["llvm"]["cargo_feature"] = "backend-c"

        self.assertIn("cargo feature", self._reject(mutate))

    def test_a_mismatched_distribution_default_is_refused(self) -> None:
        def mutate(data: dict) -> None:
            data["variants"]["linux-x86_64"]["backends"]["llvm"]["distribution_backend"] = "c"

        self.assertIn("distribution backend", self._reject(mutate))

    def test_a_toolchain_free_variant_with_a_c_toolchain_is_refused(self) -> None:
        def mutate(data: dict) -> None:
            data["variants"]["linux-x86_64"]["backends"]["cranelift"]["components"].append(
                "c_toolchain"
            )

        self.assertIn("toolchain_free", self._reject(mutate))

    def test_an_object_variant_without_its_sidecar_is_refused(self) -> None:
        def mutate(data: dict) -> None:
            variant = data["variants"]["windows-x86_64"]["backends"]["cranelift"]
            variant["components"] = [
                name for name in variant["components"] if name != "direct_link_sidecar"
            ]

        self.assertIn("direct_link_sidecar", self._reject(mutate))

    def test_an_object_variant_without_runtime_archives_is_refused(self) -> None:
        def mutate(data: dict) -> None:
            variant = data["variants"]["linux-x86_64"]["backends"]["llvm"]
            variant["components"] = [
                name for name in variant["components"] if name != "runtime_archives"
            ]

        self.assertIn("runtime_archives", self._reject(mutate))

    def test_a_c_variant_carrying_object_payload_is_refused(self) -> None:
        for extra in ("direct_link_sidecar", "llvm_provider", "runtime_archives"):
            def mutate(data: dict, extra=extra) -> None:
                data["variants"]["windows-x86_64"]["backends"]["c"]["components"].append(extra)

            with self.subTest(component=extra):
                self.assertIn("C package", self._reject(mutate))

    def test_a_cranelift_variant_with_a_provider_is_refused(self) -> None:
        def mutate(data: dict) -> None:
            data["variants"]["windows-x86_64"]["backends"]["cranelift"]["components"].append(
                "llvm_provider"
            )

        self.assertIn("must not ship an LLVM provider", self._reject(mutate))

    def test_a_hosted_runtime_profile_is_refused(self) -> None:
        def mutate(data: dict) -> None:
            data["variants"]["linux-x86_64"]["backends"]["cranelift"]["runtime_profiles"].append(
                "hosted"
            )

        self.assertIn("non-freestanding", self._reject(mutate))

    def test_duplicate_archive_names_cannot_pass_validation(self) -> None:
        def mutate(data: dict) -> None:
            data["variants"]["linux-x86_64"]["backends"]["llvm"]["archive_name_template"] = data[
                "variants"
            ]["linux-x86_64"]["backends"]["cranelift"]["archive_name_template"]

        # The per-variant suffix rule catches the attempt first; the\n        # cross-variant uniqueness check behind it is the backstop.\n        self.assertIn("must end with '-llvm.tar.xz'", self._reject(mutate))

    def test_a_native_label_is_refused(self) -> None:
        def mutate(data: dict) -> None:
            variant = data["variants"]["windows-x86_64"]["backends"]["cranelift"]
            variant["archive_name_template"] = "oscan-v{version}-windows-x86_64-native.zip"

        self.assertIn("deprecated CLI alias", self._reject(mutate))

    def test_an_unsupported_target_is_refused(self) -> None:
        def mutate(data: dict) -> None:
            data["variants"]["freebsd-x86_64"] = data["variants"]["linux-x86_64"]

        self.assertIn("unsupported target", self._reject(mutate))

    def test_a_target_missing_a_declared_backend_is_refused(self) -> None:
        def mutate(data: dict) -> None:
            del data["variants"]["linux-x86_64"]["backends"]["cranelift"]

        self.assertIn("must declare backends", self._reject(mutate))


class VariantStagingTests(unittest.TestCase):
    def _stage(self, tmp: Path, target: str, backend: str) -> tuple[Path, Path]:
        fixture = PackagingFixture(tmp / f"{target}-{backend}-input", target)
        output_dir = tmp / f"{target}-{backend}-out"
        rt.stage_release(stage_namespace(fixture, backend, output_dir))
        bundle = output_dir / "stage" / f"oscan-v9.9.9-{target}-{backend}"
        suffix = rt.ARCHIVE_SUFFIXES[
            rt.load_release_contract(rt.CONTRACT_PATH)["variants"][target]["archive_format"]
        ]
        archive = output_dir / f"oscan-v9.9.9-{target}-{backend}{suffix}"
        self.assertTrue(bundle.is_dir())
        self.assertTrue(archive.is_file(), f"missing archive {archive}")
        return bundle, archive

    def test_every_variant_stages_with_the_expected_contents(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            for target, backend in ALL_VARIANTS:
                with self.subTest(target=target, backend=backend):
                    bundle, _ = self._stage(tmp, target, backend)
                    binary_name = "oscan.exe" if target.startswith("windows") else "oscan"
                    self.assertTrue((bundle / binary_name).is_file())
                    self.assertTrue((bundle / "README-install.txt").is_file())
                    metadata = json.loads(
                        (bundle / rt.PACKAGE_METADATA_NAME).read_text(encoding="utf-8")
                    )
                    self.assertEqual(metadata["target"], target)
                    self.assertEqual(metadata["backend"], backend)
                    self.assertEqual(metadata["default_backend"], backend)
                    self.assertEqual(metadata["available_backends"], [backend])
                    self.assertEqual(metadata["cargo_feature"], f"backend-{backend}")
                    self.assertEqual(metadata["version"], "9.9.9")
                    self.assertIn(binary_name, metadata["component_digests"])

                    if backend == "c":
                        self.assertFalse((bundle / "native-link").exists())
                        self.assertFalse((bundle / "build").exists())
                        self.assertFalse(metadata["toolchain_free"])
                        if target.startswith("macos"):
                            self.assertFalse((bundle / "toolchain").exists())
                            self.assertEqual(
                                metadata["requirements"]["host_c_toolchain"], "apple-clt"
                            )
                        else:
                            self.assertTrue((bundle / "toolchain").is_dir())
                            self.assertTrue(metadata["requirements"]["bundled_c_toolchain"])
                    else:
                        self.assertTrue(metadata["toolchain_free"])
                        self.assertTrue(
                            (bundle / "native-link" / "native-link-assets.json").is_file()
                        )
                        archives = bundle / "build" / "runtime-archives" / target
                        for profile in rt.FREESTANDING_PROFILES:
                            self.assertTrue(
                                (archives / f"libosc_runtime_{profile}.a").is_file()
                            )
                            self.assertTrue(
                                (archives / f"libosc_runtime_{profile}.json").is_file()
                            )
                        self.assertFalse((archives / "libosc_runtime_hosted.a").exists())
                        self.assertFalse((bundle / "native-runtime").exists())
                        self.assertFalse((bundle / "cross-linkers").exists())

    def test_object_packages_contain_no_c_toolchain_payload(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            for target, backend in (
                ("windows-x86_64", "llvm"),
                ("windows-x86_64", "cranelift"),
                ("linux-x86_64", "llvm"),
                ("linux-x86_64", "cranelift"),
            ):
                with self.subTest(target=target, backend=backend):
                    bundle, _ = self._stage(tmp, target, backend)
                    staged = [
                        path.relative_to(bundle).as_posix()
                        for path in bundle.rglob("*")
                        if path.is_file()
                    ]
                    for relative in staged:
                        name = Path(relative).name
                        self.assertNotIn(name, rt.C_COMPILER_EXECUTABLE_NAMES, relative)
                        if name.endswith(".h"):
                            self.assertTrue(relative.startswith("native-link/"), relative)
                        self.assertNotIn("/include/", f"/{relative}")
                        self.assertNotIn("/sysroot/", f"/{relative}")

    def test_windows_llvm_shares_the_single_sidecar_provider_copy(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            bundle, _ = self._stage(Path(tmp_name), "windows-x86_64", "llvm")
            copies = [
                path.relative_to(bundle).as_posix()
                for path in bundle.rglob("libLLVM-22.dll")
            ]
            self.assertEqual(copies, ["native-link/bin/libLLVM-22.dll"])
            self.assertFalse((bundle / "toolchain").exists())
            metadata = json.loads(
                (bundle / rt.PACKAGE_METADATA_NAME).read_text(encoding="utf-8")
            )
            provider = metadata["component_digests"]["llvm_provider"]
            self.assertEqual(provider["source"], "direct-link-sidecar")
            self.assertEqual(provider["path"], "native-link/bin/libLLVM-22.dll")

    def test_linux_llvm_stages_provider_payload_only(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            bundle, _ = self._stage(Path(tmp_name), "linux-x86_64", "llvm")
            toolchain_files = [
                path.relative_to(bundle / "toolchain").as_posix()
                for path in (bundle / "toolchain").rglob("*")
                if path.is_file()
            ]
            self.assertTrue(toolchain_files)
            # Provider payload only: the library plus its own copyright
            # notice, and nothing resembling a C toolchain.
            for relative in toolchain_files:
                name = Path(relative).name.lower()
                self.assertTrue(
                    "libllvm" in name
                    or "copyright" in name
                    or "license" in relative.lower()
                    or "provider" in name
                    or "readme" in name,
                    f"unexpected toolchain payload: {relative}",
                )
                self.assertNotIn(name, rt.C_COMPILER_EXECUTABLE_NAMES)
                self.assertFalse(name.endswith(".h"), relative)
            metadata = json.loads(
                (bundle / rt.PACKAGE_METADATA_NAME).read_text(encoding="utf-8")
            )
            self.assertEqual(
                metadata["component_digests"]["llvm_provider"]["source"], "toolchain-manifest"
            )

    def test_linux_cranelift_has_neither_provider_nor_toolchain(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            bundle, _ = self._stage(Path(tmp_name), "linux-x86_64", "cranelift")
            self.assertFalse((bundle / "toolchain").exists())
            self.assertEqual(list(bundle.rglob("libLLVM*")), [])

    def test_package_readme_states_what_each_variant_can_and_cannot_do(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            bundle, _ = self._stage(tmp, "windows-x86_64", "cranelift")
            readme = (bundle / "README-install.txt").read_text(encoding="utf-8")
            self.assertIn("Backend: cranelift", readme)
            self.assertIn("--libc is refused", readme)
            self.assertIn("--extra-c", readme)
            self.assertIn("--backend c, --emit-c and -o *.c are refused", readme)
            self.assertIn("only LLD's runtime dependency", readme)

            bundle, _ = self._stage(tmp, "linux-x86_64", "c")
            readme = (bundle / "README-install.txt").read_text(encoding="utf-8")
            self.assertIn("Backend: c", readme)
            self.assertIn("pinned C toolchain", readme)
            self.assertNotIn("--libc is refused", readme)

            bundle, _ = self._stage(tmp, "macos-x86_64", "c")
            readme = (bundle / "README-install.txt").read_text(encoding="utf-8")
            self.assertIn("apple-clt", readme)
            self.assertIn("No toolchain is bundled", readme)

    def test_a_sidecar_digest_mismatch_fails_staging(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "windows-x86_64")
            (fixture.native_link_dir / "bin" / "libunwind.dll").write_bytes(b"tampered")
            with self.assertRaises(SystemExit) as caught:
                rt.stage_release(stage_namespace(fixture, "cranelift", tmp / "out"))
            self.assertIn("digest mismatch", str(caught.exception))

    def test_a_foreign_sidecar_target_fails_staging(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "windows-x86_64")
            manifest_path = fixture.native_link_dir / "native-link-assets.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest["target"] = "linux-x86_64"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaises(SystemExit) as caught:
                rt.stage_release(stage_namespace(fixture, "cranelift", tmp / "out"))
            self.assertIn("staged for target", str(caught.exception))

    def test_an_object_variant_without_prepared_inputs_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            with self.assertRaises(SystemExit) as caught:
                rt.stage_release(
                    stage_namespace(fixture, "cranelift", tmp / "out", native_link_dir=None)
                )
            self.assertIn("--native-link-dir", str(caught.exception))


class RemovedStagingInputTests(unittest.TestCase):
    """The prepared directories are gone as *authoritative* inputs: they
    were never authenticated, so accepting them silently would defeat the
    digest check the archives now get."""

    def test_the_removed_directory_options_are_refused_by_the_cli(self) -> None:
        for option, replacement in (
            ("--toolchain-dir", "--toolchain-archive"),
            ("--llvm-provider-dir", "--llvm-provider-archive"),
        ):
            with self.subTest(option=option):
                with self.assertRaises(SystemExit) as caught:
                    rt.main(
                        [
                            "stage-release",
                            "--target", "linux-x86_64",
                            "--backend", "c",
                            "--version", "9.9.9",
                            "--binary", "unused",
                            "--output-dir", "unused",
                            option, "some/dir",
                        ]
                    )
                message = str(caught.exception)
                self.assertIn(f"{option} has been removed", message)
                self.assertIn(replacement, message)

    def test_the_removed_options_are_refused_programmatically(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            with self.assertRaises(SystemExit) as caught:
                rt.stage_release(
                    stage_namespace(fixture, "c", tmp / "out", toolchain_dir=str(tmp))
                )
            self.assertIn("--toolchain-dir has been removed", str(caught.exception))

    def test_staging_a_c_package_without_an_archive_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            with self.assertRaises(SystemExit) as caught:
                rt.stage_release(
                    stage_namespace(fixture, "c", tmp / "out", toolchain_archive=None)
                )
            self.assertIn("--toolchain-archive", str(caught.exception))

    def test_staging_an_llvm_package_without_a_provider_archive_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            with self.assertRaises(SystemExit) as caught:
                rt.stage_release(
                    stage_namespace(fixture, "llvm", tmp / "out", llvm_provider_archive=None)
                )
            self.assertIn("--llvm-provider-archive", str(caught.exception))


class TrustedToolchainArchiveTests(unittest.TestCase):
    """The C toolchain is authenticated against the digest the repository's
    manifest pins, and every archive member is validated before it is
    written anywhere."""

    def _stage(self, tmp: Path, fixture: PackagingFixture, backend: str = "c") -> Path:
        output_dir = tmp / f"{fixture.target}-{backend}-out"
        rt.stage_release(stage_namespace(fixture, backend, output_dir))
        return output_dir / "stage" / f"oscan-v9.9.9-{fixture.target}-{backend}"

    def _reject(self, target: str, mutate, backend: str = "c") -> str:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", target)
            overrides = mutate(fixture) or {}
            with self.assertRaises(SystemExit) as caught:
                rt.stage_release(stage_namespace(fixture, backend, tmp / "out", **overrides))
            output = tmp / "out"
            for suffix in rt.ARCHIVE_SUFFIXES.values():
                self.assertEqual(
                    [], list(output.glob(f"*{suffix}")), "no archive may be produced"
                )
            return str(caught.exception)

    def test_the_declared_toolchain_archive_stages_completely(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            bundle = self._stage(tmp, fixture)
            manifest = fixture.manifest()
            runtime = manifest["toolchain"]["runtime"]
            staged = {
                path.relative_to(bundle / "toolchain").as_posix()
                for path in (bundle / "toolchain").rglob("*")
                if path.is_file()
            }
            for role in ("compiler", "archiver", "linker"):
                self.assertIn(
                    rt.safe_relative_path(runtime[role]["path"]).as_posix(), staged
                )
            # The manifest's own wrapper, prune rules, and sysroot survive.
            self.assertIn("bin/gcc", staged)
            self.assertIn("x86_64-linux-musl/include/stdio.h", staged)
            self.assertNotIn("lib/libstdc++.a", staged)
            self.assertNotIn("share/man/man1/gcc.1", staged)
            # A sysroot-relative absolute symlink is resolved inside the
            # tree instead of pointing at the build host.
            self.assertIn("x86_64-linux-musl/lib/ld-musl-x86_64.so.1", staged)
            self.assertEqual(
                (bundle / "toolchain" / "x86_64-linux-musl/lib/ld-musl-x86_64.so.1").read_bytes(),
                b"fake libc",
            )
            # A contained relative alias ships as a regular copy of what it
            # names, never as a link into the installed tree.
            self.assertIn("bin/cc", staged)
            alias = bundle / "toolchain" / "bin" / "cc"
            self.assertFalse(alias.is_symlink())
            self.assertEqual(alias.read_bytes(), b"fake compiler")
            self.assertTrue((bundle / (manifest["target"] + ".json")).is_file())

    def test_a_tampered_archive_is_refused(self) -> None:
        def tamper(fixture: PackagingFixture) -> None:
            data = bytearray(fixture.toolchain_archive.read_bytes())
            data[-1] ^= 0xFF
            fixture.toolchain_archive.write_bytes(bytes(data))

        message = self._reject("linux-x86_64", tamper)
        self.assertIn("does not match the pinned sha256 digest", message)

    def test_a_wrong_archive_is_refused(self) -> None:
        def swap(fixture: PackagingFixture) -> dict:
            return {"toolchain_archive": str(fixture.provider_archive)}

        message = self._reject("linux-x86_64", swap)
        self.assertIn("does not match the pinned sha256 digest", message)

    def test_an_empty_archive_is_refused(self) -> None:
        def empty(fixture: PackagingFixture) -> None:
            fixture.toolchain_archive.write_bytes(b"")

        self.assertIn("is empty", self._reject("linux-x86_64", empty))

    def test_a_missing_archive_is_refused(self) -> None:
        def missing(fixture: PackagingFixture) -> None:
            fixture.toolchain_archive.unlink()

        self.assertIn("archive not found", self._reject("linux-x86_64", missing))

    def test_an_absolute_member_is_refused(self) -> None:
        def absolute(fixture: PackagingFixture) -> None:
            entries = fixture.toolchain_entries()
            entries.append(file_entry("/etc/evil", b"owned"))
            fixture.rebuild_toolchain_archive(entries)

        self.assertIn("not a safe relative path", self._reject("linux-x86_64", absolute))

    def test_a_windows_drive_letter_member_is_refused(self) -> None:
        def absolute(fixture: PackagingFixture) -> None:
            entries = fixture.toolchain_entries()
            entries.append(file_entry("C:/Windows/evil.dll", b"owned"))
            fixture.rebuild_toolchain_archive(entries)

        self.assertIn("not a safe relative path", self._reject("windows-x86_64", absolute))

    def test_a_traversal_member_is_refused(self) -> None:
        def traversal(fixture: PackagingFixture) -> None:
            entries = fixture.toolchain_entries()
            entries.append(file_entry(f"{fixture.toolchain_root}/../../evil", b"owned"))
            fixture.rebuild_toolchain_archive(entries)

        self.assertIn("not a safe relative path", self._reject("linux-x86_64", traversal))

    def test_a_symlink_escaping_the_tree_is_refused(self) -> None:
        def escape(fixture: PackagingFixture) -> None:
            entries = fixture.toolchain_entries()
            entries.append(
                symlink_entry(f"{fixture.toolchain_root}/bin/escape", "../../../../etc/passwd")
            )
            fixture.rebuild_toolchain_archive(entries)

        self.assertIn("escapes the extraction root", self._reject("linux-x86_64", escape))

    def test_an_absolute_symlink_with_no_in_tree_target_is_refused(self) -> None:
        def host_link(fixture: PackagingFixture) -> None:
            entries = fixture.toolchain_entries()
            entries.append(
                symlink_entry(f"{fixture.toolchain_root}/bin/hostlink", "/etc/shadow")
            )
            fixture.rebuild_toolchain_archive(entries)

        message = self._reject("linux-x86_64", host_link)
        self.assertIn("does not resolve inside the archive's own tree", message)

    def test_a_hardlink_leaving_the_tree_is_refused(self) -> None:
        def hardlink(fixture: PackagingFixture) -> None:
            entries = fixture.toolchain_entries()
            entries.append(
                hardlink_entry(f"{fixture.toolchain_root}/bin/stolen", "/etc/shadow")
            )
            fixture.rebuild_toolchain_archive(entries)

        self.assertIn("hard-links to the unsafe path", self._reject("linux-x86_64", hardlink))

    def test_a_traversing_hardlink_is_refused(self) -> None:
        def hardlink(fixture: PackagingFixture) -> None:
            entries = fixture.toolchain_entries()
            entries.append(
                hardlink_entry(f"{fixture.toolchain_root}/bin/stolen", "../../../etc/shadow")
            )
            fixture.rebuild_toolchain_archive(entries)

        self.assertIn("hard-links to the unsafe path", self._reject("linux-x86_64", hardlink))

    def test_a_missing_compiler_is_refused(self) -> None:
        def drop_compiler(fixture: PackagingFixture) -> None:
            manifest = fixture.manifest()
            compiler = manifest["toolchain"]["runtime"]["compiler"]["path"]
            entries = [
                entry
                for entry in fixture.toolchain_entries()
                if entry["name"] != f"{fixture.toolchain_root}/{compiler}"
            ]
            fixture.rebuild_toolchain_archive(entries)

        message = self._reject("linux-x86_64", drop_compiler)
        self.assertIn("manifest-declared compiler", message)

    def test_a_missing_archiver_is_refused(self) -> None:
        def drop_archiver(fixture: PackagingFixture) -> None:
            manifest = fixture.manifest()
            archiver = manifest["toolchain"]["runtime"]["archiver"]["path"]
            entries = [
                entry
                for entry in fixture.toolchain_entries()
                if entry["name"] != f"{fixture.toolchain_root}/{archiver}"
            ]
            fixture.rebuild_toolchain_archive(entries)

        message = self._reject("windows-x86_64", drop_archiver)
        self.assertIn("manifest-declared archiver", message)

    def test_an_empty_compiler_is_refused(self) -> None:
        def empty_compiler(fixture: PackagingFixture) -> None:
            manifest = fixture.manifest()
            compiler = manifest["toolchain"]["runtime"]["compiler"]["path"]
            entries = fixture.toolchain_entries()
            for entry in entries:
                if entry["name"] == f"{fixture.toolchain_root}/{compiler}":
                    entry["data"] = b""
            fixture.rebuild_toolchain_archive(entries)

        self.assertIn("is empty", self._reject("linux-x86_64", empty_compiler))

    def test_a_foreign_target_manifest_is_refused(self) -> None:
        def foreign(fixture: PackagingFixture) -> None:
            manifest = fixture.manifest()
            manifest["target"] = "linux-aarch64"
            fixture.write_manifest(manifest)

        message = self._reject("linux-x86_64", foreign)
        self.assertIn("describes target 'linux-aarch64'", message)

    def test_the_linux_base_archive_may_not_carry_the_overlaid_provider(self) -> None:
        def plant_provider(fixture: PackagingFixture) -> None:
            manifest = fixture.manifest()
            spec = manifest["toolchain"]["llvm_code_generator"]
            entries = fixture.toolchain_entries()
            entries.append(
                file_entry(f"{fixture.toolchain_root}/{spec['path']}", b"overlaid libLLVM")
            )
            fixture.rebuild_toolchain_archive(entries)

        message = self._reject("linux-x86_64", plant_provider)
        self.assertIn("already contains the separately overlaid LLVM provider", message)

    def test_windows_c_retains_the_integrated_llvm_the_compiler_needs(self) -> None:
        """Windows' llvm-mingw toolchain integrates libLLVM: clang.exe links
        against it, so a Windows C package must keep it. Exclusion only
        applies to a *separately overlaid* provider archive."""
        manifest = rt.load_manifest(TOOLCHAINS / "windows-x86_64.json")
        spec = manifest["toolchain"].get("llvm_code_generator") or {}
        self.assertNotIn("archive", spec, "the Windows provider is integrated, not overlaid")
        self.assertEqual(set(), rt.llvm_provider_staged_paths(manifest))

        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "windows-x86_64")
            bundle = self._stage(tmp, fixture)
            staged = bundle / "toolchain" / rt.safe_relative_path(spec["path"])
            self.assertTrue(staged.is_file(), "clang's own libLLVM must remain")
            self.assertEqual(staged.read_bytes(), b"integrated libLLVM")

    def test_a_pruned_away_integrated_llvm_is_refused(self) -> None:
        def drop_integrated(fixture: PackagingFixture) -> None:
            manifest = fixture.manifest()
            spec = manifest["toolchain"]["llvm_code_generator"]
            entries = [
                entry
                for entry in fixture.toolchain_entries()
                if entry["name"] != f"{fixture.toolchain_root}/{spec['path']}"
            ]
            fixture.rebuild_toolchain_archive(entries)

        message = self._reject("windows-x86_64", drop_integrated)
        self.assertIn("does not exist after staging/pruning", message)

    def test_package_metadata_records_the_trusted_archive_digest(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            bundle = self._stage(tmp, fixture)
            expected = rt.compute_digest(fixture.toolchain_archive, "sha256")
            pinned = fixture.manifest()["toolchain"]["archive"]["digest"]["value"]
            self.assertEqual(pinned, expected)
            metadata = json.loads(
                (bundle / rt.PACKAGE_METADATA_NAME).read_text(encoding="utf-8")
            )
            recorded = metadata["component_digests"]["c_toolchain"]
            self.assertEqual(recorded["source_manifest"], "linux-x86_64.json")
            self.assertEqual(recorded["source_archive"]["digest"]["algorithm"], "sha256")
            self.assertEqual(recorded["source_archive"]["digest"]["value"], expected)
            provenance = (bundle / "LICENSES" / "toolchain-source.txt").read_text(
                encoding="utf-8"
            )
            self.assertIn(expected, provenance)
            self.assertIn("Source manifest: linux-x86_64.json", provenance)


class TrustedProviderArchiveTests(unittest.TestCase):
    """The Linux LLVM provider comes from its own pinned archive, and only
    the members the manifest declares are ever staged."""

    def _stage(self, tmp: Path, fixture: PackagingFixture) -> Path:
        output_dir = tmp / "llvm-out"
        rt.stage_release(stage_namespace(fixture, "llvm", output_dir))
        return output_dir / "stage" / f"oscan-v9.9.9-{fixture.target}-llvm"

    def _reject(self, mutate) -> str:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            overrides = mutate(fixture) or {}
            with self.assertRaises(SystemExit) as caught:
                rt.stage_release(stage_namespace(fixture, "llvm", tmp / "out", **overrides))
            return str(caught.exception)

    def test_every_declared_provider_file_is_staged(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            bundle = self._stage(tmp, fixture)
            spec = fixture.manifest()["toolchain"]["llvm_code_generator"]
            for file_spec in spec["files"]:
                staged = bundle / "toolchain" / rt.safe_relative_path(file_spec["path"])
                self.assertTrue(staged.is_file(), file_spec["path"])
                self.assertEqual(
                    staged.read_bytes(),
                    f"provider payload for {file_spec['source']}".encode(),
                )

    def test_undeclared_archive_members_are_never_staged(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            bundle = self._stage(tmp, fixture)
            staged = [path.name for path in bundle.rglob("*") if path.is_file()]
            self.assertNotIn("unlisted-tool", staged)
            self.assertNotIn("llvm-config", staged)
            self.assertEqual([], list(bundle.rglob("unlisted-tool")))

    def test_the_archives_own_alias_links_are_not_staged(self) -> None:
        """The pinned archive ships `libLLVM.so.22` / `libLLVM-22.so` next to
        the library. They are safe (relative, contained) so extraction keeps
        them, but the manifest does not declare them, so nothing stages
        them: the compiler loads the declared `libLLVM.so.22.1` path."""
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            aliases = {
                entry["name"].split("/", 1)[1]
                for entry in fixture.provider_entries()
                if entry.get("kind") == "symlink"
            }
            self.assertEqual(
                aliases, {"lib/libLLVM.so.22", "lib/libLLVM-22.so"}, "fixture must ship aliases"
            )
            bundle = self._stage(tmp, fixture)
            self.assertEqual([], list(bundle.rglob("libLLVM.so.22")))
            self.assertEqual([], list(bundle.rglob("libLLVM-22.so")))
            self.assertTrue((bundle / "toolchain" / "lib" / "libLLVM.so.22.1").is_file())
            staged = [path for path in bundle.rglob("*") if path.is_symlink()]
            self.assertEqual([], staged, "a package ships regular files, not links")

    def test_the_provenance_evidence_matches_the_staged_files(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            bundle = self._stage(tmp, fixture)
            record = json.loads(
                (bundle / "LICENSES" / "llvm-provider" / rt.PROVIDER_PROVENANCE_NAME).read_text(
                    encoding="utf-8"
                )
            )
            expected = rt.compute_digest(fixture.provider_archive, "sha256")
            self.assertEqual(record["target"], "linux-x86_64")
            self.assertEqual(record["source_manifest"], "linux-x86_64.json")
            self.assertEqual(record["source_archive"]["digest"]["value"], expected)
            self.assertEqual(record["staged_root"], "toolchain")
            self.assertTrue(record["files"])
            for entry in record["files"]:
                staged = bundle / "toolchain" / rt.safe_relative_path(entry["path"])
                self.assertTrue(staged.is_file(), entry["path"])
                self.assertEqual(rt.compute_digest(staged, "sha256"), entry["sha256"])
            metadata = json.loads(
                (bundle / rt.PACKAGE_METADATA_NAME).read_text(encoding="utf-8")
            )
            provider = metadata["component_digests"]["llvm_provider"]
            self.assertEqual(provider["source_archive_digest"]["value"], expected)
            self.assertEqual(provider["source_manifest"], "linux-x86_64.json")

    def test_a_provenance_file_planted_next_to_the_archive_is_ignored(self) -> None:
        """The old model trusted a provenance record that travelled with the
        payload; nothing reads one as input any more."""
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            planted = fixture.provider_archive.parent / rt.PROVIDER_PROVENANCE_NAME
            planted.write_text(json.dumps({"schema_version": 99}), encoding="utf-8")
            bundle = self._stage(tmp, fixture)
            record = json.loads(
                (bundle / "LICENSES" / "llvm-provider" / rt.PROVIDER_PROVENANCE_NAME).read_text(
                    encoding="utf-8"
                )
            )
            self.assertEqual(record["schema_version"], 1)
            self.assertFalse(hasattr(rt, "load_provider_provenance"))

    def test_a_tampered_provider_archive_is_refused(self) -> None:
        def tamper(fixture: PackagingFixture) -> None:
            data = bytearray(fixture.provider_archive.read_bytes())
            data[-1] ^= 0xFF
            fixture.provider_archive.write_bytes(bytes(data))

        self.assertIn("does not match the pinned sha256 digest", self._reject(tamper))

    def test_a_wrong_provider_archive_is_refused(self) -> None:
        def swap(fixture: PackagingFixture) -> dict:
            return {"llvm_provider_archive": str(fixture.toolchain_archive)}

        self.assertIn("does not match the pinned sha256 digest", self._reject(swap))

    def test_an_empty_provider_archive_is_refused(self) -> None:
        def empty(fixture: PackagingFixture) -> None:
            fixture.provider_archive.write_bytes(b"")

        self.assertIn("is empty", self._reject(empty))

    def test_a_provider_archive_missing_a_declared_file_is_refused(self) -> None:
        def drop(fixture: PackagingFixture) -> None:
            spec = fixture.manifest()["toolchain"]["llvm_code_generator"]
            missing = f"{fixture.provider_root}/{spec['files'][0]['source']}"
            entries = [
                entry for entry in fixture.provider_entries() if entry["name"] != missing
            ]
            fixture.rebuild_provider_archive(entries)

        self.assertIn("is missing declared file", self._reject(drop))

    def test_a_provider_archive_with_an_empty_payload_is_refused(self) -> None:
        def empty_payload(fixture: PackagingFixture) -> None:
            spec = fixture.manifest()["toolchain"]["llvm_code_generator"]
            target = f"{fixture.provider_root}/{spec['files'][0]['source']}"
            entries = fixture.provider_entries()
            for entry in entries:
                if entry["name"] == target:
                    entry["data"] = b""
            fixture.rebuild_provider_archive(entries)

        self.assertIn("which is empty", self._reject(empty_payload))

    def test_a_traversing_provider_member_is_refused(self) -> None:
        def traversal(fixture: PackagingFixture) -> None:
            entries = fixture.provider_entries()
            entries.append(file_entry(f"{fixture.provider_root}/../evil.so", b"owned"))
            fixture.rebuild_provider_archive(entries)

        self.assertIn("not a safe relative path", self._reject(traversal))

    def test_an_absolute_link_in_the_provider_archive_is_refused(self) -> None:
        def absolute_link(fixture: PackagingFixture) -> None:
            entries = fixture.provider_entries()
            entries.append(
                symlink_entry(f"{fixture.provider_root}/lib/host.so", "/usr/lib/libLLVM.so")
            )
            fixture.rebuild_provider_archive(entries)

        self.assertIn("symlink to the absolute path", self._reject(absolute_link))

    def test_a_link_escaping_the_provider_archive_is_refused(self) -> None:
        def escape(fixture: PackagingFixture) -> None:
            entries = fixture.provider_entries()
            entries.append(
                symlink_entry(
                    f"{fixture.provider_root}/lib/escape.so", "../../../../etc/shadow"
                )
            )
            fixture.rebuild_provider_archive(entries)

        self.assertIn("escapes the extraction root", self._reject(escape))

    def test_a_declared_file_supplied_as_a_contained_alias_is_materialized(self) -> None:
        """A declared source may be one of the archive's own alias links, as
        long as it resolves to a regular file inside the archive: what ships
        is a deterministic regular copy of that file."""
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            manifest = fixture.manifest()
            spec = manifest["toolchain"]["llvm_code_generator"]
            declared = spec["files"][0]["source"]
            entries = [
                entry
                for entry in fixture.provider_entries()
                if entry["name"] != f"{fixture.provider_root}/{declared}"
            ]
            entries.append(
                file_entry(f"{fixture.provider_root}/lib/libLLVM.so.22.1.real", b"real payload")
            )
            entries.append(
                symlink_entry(
                    f"{fixture.provider_root}/{declared}", "libLLVM.so.22.1.real"
                )
            )
            fixture.rebuild_provider_archive(entries)
            bundle = self._stage(tmp, fixture)
            staged = bundle / "toolchain" / rt.safe_relative_path(spec["files"][0]["path"])
            self.assertTrue(staged.is_file())
            self.assertFalse(staged.is_symlink())
            self.assertEqual(staged.read_bytes(), b"real payload")
            record = json.loads(
                (bundle / "LICENSES" / "llvm-provider" / rt.PROVIDER_PROVENANCE_NAME).read_text(
                    encoding="utf-8"
                )
            )
            entry = next(
                item
                for item in record["files"]
                if item["path"] == rt.safe_relative_path(spec["files"][0]["path"]).as_posix()
            )
            self.assertEqual(entry["sha256"], sha256_of(b"real payload"))
            # A host that can create symlinks records which member the alias
            # actually resolved to; one that cannot has already materialized
            # the copy during extraction, so the declared source *is* the
            # payload. Either way the staged bytes are the same.
            self.assertIn(
                entry.get("archive_source", declared),
                ("lib/libLLVM.so.22.1.real", declared),
            )

    def test_a_declared_source_that_is_not_a_regular_file_is_refused(self) -> None:
        def directory_alias(fixture: PackagingFixture) -> None:
            spec = fixture.manifest()["toolchain"]["llvm_code_generator"]
            declared = spec["files"][0]["source"]
            entries = [
                entry
                for entry in fixture.provider_entries()
                if entry["name"] != f"{fixture.provider_root}/{declared}"
            ]
            entries.append(dir_entry(f"{fixture.provider_root}/lib/payload-dir"))
            entries.append(
                file_entry(f"{fixture.provider_root}/lib/payload-dir/inner", b"inner")
            )
            entries.append(
                symlink_entry(f"{fixture.provider_root}/{declared}", "payload-dir")
            )
            fixture.rebuild_provider_archive(entries)

        message = self._reject(directory_alias)
        self.assertIn("does not resolve to a regular file inside the archive", message)

    def test_a_declared_alias_with_no_payload_is_refused(self) -> None:
        def dangling(fixture: PackagingFixture) -> None:
            spec = fixture.manifest()["toolchain"]["llvm_code_generator"]
            declared = spec["files"][0]["source"]
            entries = [
                entry
                for entry in fixture.provider_entries()
                if entry["name"] != f"{fixture.provider_root}/{declared}"
            ]
            entries.append(
                symlink_entry(f"{fixture.provider_root}/{declared}", "never-shipped.so")
            )
            fixture.rebuild_provider_archive(entries)

        message = self._reject(dangling)
        # A host that cannot create symlinks drops the dangling link
        # entirely; either way the declared payload is absent and staging
        # stops before a package exists.
        self.assertTrue(
            "does not resolve to a regular file inside the archive" in message
            or "is missing declared file" in message,
            message,
        )


class ProviderExclusionTests(unittest.TestCase):
    """A C package ships the C toolchain and none of the separately
    overlaid LLVM provider."""

    def test_the_linux_c_package_excludes_the_overlaid_llvm_provider(self) -> None:
        manifest = rt.load_manifest(TOOLCHAINS / "linux-x86_64.json")
        excluded = rt.llvm_provider_staged_paths(manifest)
        self.assertTrue(excluded, "the Linux manifest must declare provider paths")
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            output_dir = tmp / "out"
            rt.stage_release(stage_namespace(fixture, "c", output_dir))
            bundle = output_dir / "stage" / "oscan-v9.9.9-linux-x86_64-c"
            staged = {
                path.relative_to(bundle / "toolchain").as_posix()
                for path in (bundle / "toolchain").rglob("*")
                if path.is_file()
            }
            for relative in excluded:
                self.assertNotIn(relative, staged)
            self.assertEqual([], list(bundle.rglob("libLLVM*")))
            self.assertFalse((bundle / "LICENSES" / "llvm-provider").exists())
            self.assertEqual([], list(bundle.rglob(rt.PROVIDER_PROVENANCE_NAME)))
            # ...while the complete C toolchain is still there.
            runtime = manifest["toolchain"]["runtime"]
            for tool in (runtime["compiler"]["path"], runtime["archiver"]["path"]):
                self.assertIn(rt.safe_relative_path(tool).as_posix(), staged)
            metadata = json.loads(
                (bundle / rt.PACKAGE_METADATA_NAME).read_text(encoding="utf-8")
            )
            self.assertNotIn("llvm_provider", metadata["component_digests"])

    def test_executable_modes_follow_the_source_archive(self) -> None:
        """Nested bin/ and libexec/ tools keep their executability; data
        files stay 0644; directories are 0755."""
        nested = {
            "libexec/gcc/collect2": True,
            "bin/nested-tool": True,
            "extra/data.txt": False,
        }
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            entries = fixture.toolchain_entries()
            for relative, executable in nested.items():
                entries.append(
                    file_entry(
                        f"{fixture.toolchain_root}/{relative}",
                        b"tool",
                        0o755 if executable else 0o644,
                    )
                )
            fixture.rebuild_toolchain_archive(entries)
            output_dir = tmp / "out"
            rt.stage_release(stage_namespace(fixture, "c", output_dir))
            archive = output_dir / "oscan-v9.9.9-linux-x86_64-c.tar.xz"
            with tarfile.open(archive) as handle:
                modes = {
                    member.name.split("/", 1)[1]: member
                    for member in handle.getmembers()
                    if "/" in member.name
                }
            # A Windows host cannot express Unix executable bits, so the
            # source-mode rule is only assertable on a POSIX host; the
            # deterministic 0644/0755 split applies everywhere.
            posix_host = os.name == "posix"
            for relative, executable in nested.items():
                member = modes[f"toolchain/{relative}"]
                expected = 0o755 if (executable and posix_host) else 0o644
                self.assertEqual(member.mode, expected, relative)
            runtime = fixture.manifest()["toolchain"]["runtime"]
            for tool in (runtime["compiler"]["path"], runtime["archiver"]["path"]):
                member = modes[f"toolchain/{rt.safe_relative_path(tool).as_posix()}"]
                self.assertEqual(member.mode, 0o755 if posix_host else 0o644, tool)
            self.assertTrue(
                all(member.mode in (0o644, 0o755) for member in modes.values()),
                "every member must carry one of the two canonical modes",
            )
            self.assertEqual(modes["toolchain"].mode, 0o755)
            self.assertEqual(modes[rt.PACKAGE_METADATA_NAME].mode, 0o644)


class OfflineStagingTests(unittest.TestCase):
    """Staging is handed cached archives and must never reach the network."""

    def test_no_variant_downloads_anything(self) -> None:
        def refuse(*args, **kwargs):
            raise AssertionError("release staging must not access the network")

        originals = (rt.download_file, rt._download_with_curl, rt.fetch_declared_archive)
        rt.download_file = refuse
        rt._download_with_curl = refuse
        rt.fetch_declared_archive = refuse
        try:
            with tempfile.TemporaryDirectory() as tmp_name:
                tmp = Path(tmp_name)
                for target, backend in ALL_VARIANTS:
                    with self.subTest(target=target, backend=backend):
                        fixture = PackagingFixture(tmp / f"{target}-{backend}-in", target)
                        rt.stage_release(
                            stage_namespace(fixture, backend, tmp / f"{target}-{backend}-out")
                        )
        finally:
            rt.download_file, rt._download_with_curl, rt.fetch_declared_archive = originals

    def test_resolving_an_archive_is_offline_unless_asked(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            downloads = tmp / "downloads"
            downloads.mkdir()
            with self.assertRaises(SystemExit) as caught:
                rt.resolve_archive_command(
                    rt.argparse.Namespace(
                        manifest=str(fixture.manifest_path),
                        download_dir=str(downloads),
                        component="toolchain",
                        download=False,
                    )
                )
            self.assertIn("downloading was not requested", str(caught.exception))

            # With the cached archive in place it resolves and verifies it.
            cached = downloads / Path(
                fixture.manifest()["toolchain"]["archive"]["url"]
            ).name
            shutil.copy2(fixture.toolchain_archive, cached)
            rt.resolve_archive_command(
                rt.argparse.Namespace(
                    manifest=str(fixture.manifest_path),
                    download_dir=str(downloads),
                    component="toolchain",
                    download=False,
                )
            )

            cached.write_bytes(b"tampered")
            with self.assertRaises(SystemExit) as caught:
                rt.resolve_archive_command(
                    rt.argparse.Namespace(
                        manifest=str(fixture.manifest_path),
                        download_dir=str(downloads),
                        component="toolchain",
                        download=False,
                    )
                )
            self.assertIn("does not match the pinned sha256 digest", str(caught.exception))

    def test_preparing_the_provider_verifies_a_supplied_archive(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            fixture = PackagingFixture(tmp / "input", "linux-x86_64")
            inspection = tmp / "inspect"
            rt.prepare_llvm_provider(
                rt.argparse.Namespace(
                    manifest=str(fixture.manifest_path),
                    download_dir=str(tmp / "downloads"),
                    archive=str(fixture.provider_archive),
                    no_download=True,
                    extract_to=str(inspection),
                )
            )
            spec = fixture.manifest()["toolchain"]["llvm_code_generator"]
            for file_spec in spec["files"]:
                self.assertTrue(
                    (inspection / rt.safe_relative_path(file_spec["path"])).is_file()
                )
            self.assertFalse((inspection / "unlisted-tool").exists())

            fixture.provider_archive.write_bytes(b"tampered")
            with self.assertRaises(SystemExit) as caught:
                rt.prepare_llvm_provider(
                    rt.argparse.Namespace(
                        manifest=str(fixture.manifest_path),
                        download_dir=str(tmp / "downloads"),
                        archive=str(fixture.provider_archive),
                        no_download=True,
                        extract_to=None,
                    )
                )
            self.assertIn("does not match the pinned sha256 digest", str(caught.exception))


class ReproducibleArchiveTests(unittest.TestCase):
    """Finding 3: identical inputs must produce byte-identical archives, and
    perturbed source mtimes/modes must not change a single byte."""

    def _build(self, tmp: Path, target: str, backend: str, tag: str, perturb: bool) -> Path:
        fixture = PackagingFixture(tmp / f"{tag}-input", target)
        if perturb:
            stamp = time.time() - 86_400
            for path in sorted(fixture.root.rglob("*")):
                if path.is_file():
                    os.utime(path, (stamp, stamp))
        output_dir = tmp / f"{tag}-out"
        rt.stage_release(stage_namespace(fixture, backend, output_dir))
        suffix = rt.ARCHIVE_SUFFIXES[
            rt.load_release_contract(rt.CONTRACT_PATH)["variants"][target]["archive_format"]
        ]
        return output_dir / f"oscan-v9.9.9-{target}-{backend}{suffix}"

    def test_repeat_assembly_is_byte_for_byte_identical(self) -> None:
        cases = (
            ("windows-x86_64", "cranelift", "zip"),
            ("macos-x86_64", "c", "tar.gz"),
            ("linux-x86_64", "cranelift", "tar.xz"),
        )
        for target, backend, fmt in cases:
            with self.subTest(format=fmt):
                with tempfile.TemporaryDirectory() as tmp_name:
                    tmp = Path(tmp_name)
                    first = self._build(tmp, target, backend, "first", perturb=False)
                    # A short gap plus perturbed source mtimes: neither may
                    # reach the archive bytes.
                    time.sleep(1.1)
                    second = self._build(tmp, target, backend, "second", perturb=True)
                    self.assertEqual(
                        first.read_bytes(),
                        second.read_bytes(),
                        f"{fmt} archives must be reproducible",
                    )

    def test_a_staged_c_package_is_reproducible_from_its_archive(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            first = self._build(tmp, "linux-x86_64", "c", "first", perturb=False)
            time.sleep(1.1)
            second = self._build(tmp, "linux-x86_64", "c", "second", perturb=True)
            self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_zip_entries_are_sorted_with_canonical_modes_and_timestamps(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            archive = self._build(Path(tmp_name), "windows-x86_64", "cranelift", "zip", False)
            with zipfile.ZipFile(archive) as handle:
                names = [info.filename for info in handle.infolist()]
                self.assertEqual(names, sorted(names))
                epoch = time.gmtime(rt.archive_epoch())
                for info in handle.infolist():
                    self.assertEqual(info.date_time[0], epoch.tm_year)
                    self.assertEqual(info.create_system, 3)
                    mode = (info.external_attr >> 16) & 0o777
                    self.assertIn(mode, (0o644, 0o755))
                    if info.filename.endswith("oscan.exe"):
                        self.assertEqual(mode, 0o755)
                    if info.filename.endswith("oscan-package.json"):
                        self.assertEqual(mode, 0o644)

    def test_tar_members_are_normalized_and_executable_bits_are_canonical(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            archive = self._build(Path(tmp_name), "linux-x86_64", "cranelift", "tar", False)
            with tarfile.open(archive) as handle:
                members = handle.getmembers()
                names = [member.name for member in members]
                self.assertEqual(names, sorted(names))
                for member in members:
                    self.assertEqual(member.uid, 0)
                    self.assertEqual(member.gid, 0)
                    self.assertEqual(member.uname, "")
                    self.assertEqual(member.gname, "")
                    self.assertEqual(member.mtime, rt.archive_epoch())
                    if member.isdir():
                        self.assertEqual(member.mode, 0o755)
                    elif member.name.endswith("/oscan") or member.name.endswith("install.sh"):
                        self.assertEqual(member.mode, 0o755)
                    elif member.name.endswith(".json") or member.name.endswith(".txt"):
                        self.assertEqual(member.mode, 0o644)

    def test_gzip_header_carries_no_clock_or_file_name(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_name:
            archive = self._build(Path(tmp_name), "macos-x86_64", "c", "gz", False)
            header = archive.read_bytes()[:10]
            mtime = int.from_bytes(header[4:8], "little")
            self.assertEqual(mtime, rt.archive_epoch())
            # FNAME (bit 3) must be clear: no host path in the header.
            self.assertEqual(header[3] & 0x08, 0)


class WrapperInterfaceTests(unittest.TestCase):
    def test_powershell_wrappers_require_a_backend(self) -> None:
        for name in ("stage-release.ps1", "assemble-release.ps1"):
            script = (rt.REPO_ROOT / "scripts" / name).read_text(encoding="utf-8")
            with self.subTest(script=name):
                self.assertIn('[ValidateSet("llvm", "cranelift", "c")]', script)
                self.assertIn("[string]$Backend", script)
                self.assertNotIn("CrossLinkerSidecarDir", script)

    def test_powershell_wrappers_take_archives_and_refuse_directories(self) -> None:
        for name in ("stage-release.ps1", "assemble-release.ps1"):
            script = (rt.REPO_ROOT / "scripts" / name).read_text(encoding="utf-8")
            with self.subTest(script=name):
                self.assertIn("[string]$ToolchainArchive", script)
                self.assertIn("[string]$LlvmProviderArchive", script)
                self.assertIn("-ToolchainDir has been removed", script)
                self.assertIn("-LlvmProviderDir has been removed", script)

    def test_shell_wrapper_requires_a_backend(self) -> None:
        script = (rt.REPO_ROOT / "scripts" / "stage-release.sh").read_text(encoding="utf-8")
        self.assertIn("--backend", script)
        self.assertIn('missing --backend', script)
        self.assertNotIn("--cross-linker-sidecar-dir", script)

    def test_shell_wrapper_takes_archives_and_refuses_directories(self) -> None:
        script = (rt.REPO_ROOT / "scripts" / "stage-release.sh").read_text(encoding="utf-8")
        self.assertIn("--toolchain-archive", script)
        self.assertIn("--llvm-provider-archive", script)
        self.assertIn("--toolchain-dir has been removed", script)
        self.assertIn("--llvm-provider-dir has been removed", script)

    def test_an_archive_resolution_wrapper_exists_for_ci(self) -> None:
        for name in ("resolve-archive.ps1", "resolve-archive.sh"):
            script = (rt.REPO_ROOT / "scripts" / name).read_text(encoding="utf-8")
            with self.subTest(script=name):
                self.assertIn("resolve-archive", script)
                self.assertIn("llvm-provider", script)


if __name__ == "__main__":
    unittest.main()
