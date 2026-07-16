#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import stat
import subprocess
import sys
import tarfile
import textwrap
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path, PurePosixPath


REPO_ROOT = Path(__file__).resolve().parent.parent
CONTRACT_PATH = REPO_ROOT / "packaging" / "toolchains" / "release-contract.json"
RUNTIME_ARCHIVE_CONTRACT_PATH = REPO_ROOT / "packaging" / "toolchains" / "runtime-archive-contract.json"
ZIP_EPOCH = 315532800  # 1980-01-01 UTC
ARCHIVE_SUFFIXES = {
    "zip": ".zip",
    "tar.gz": ".tar.gz",
    "tar.xz": ".tar.xz",
}
DOWNLOAD_RETRIES = 5
DOWNLOAD_RETRY_BASE_DELAY_SECONDS = 2


def fail(message: str) -> "NoReturn":
    raise SystemExit(message)


def safe_relative_path(value: str) -> Path:
    normalized = value.replace("\\", "/")
    pure = PurePosixPath(normalized)
    if pure.is_absolute():
        fail(f"expected a relative path, got '{value}'")
    if any(part in ("", ".", "..") for part in pure.parts):
        fail(f"unsafe relative path '{value}'")
    return Path(*pure.parts)


def ensure_clean_dir(path: Path) -> None:
    if path.exists():
        remove_path(path)
    path.mkdir(parents=True, exist_ok=True)


def archive_epoch() -> int:
    raw = os.environ.get("SOURCE_DATE_EPOCH", "").strip()
    if not raw:
        return ZIP_EPOCH
    try:
        return max(int(raw), ZIP_EPOCH)
    except ValueError as exc:
        fail(f"invalid SOURCE_DATE_EPOCH '{raw}': {exc}")


def compute_digest(path: Path, algorithm: str) -> str:
    hasher = hashlib.new(algorithm)
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                break
            hasher.update(chunk)
    return hasher.hexdigest()


def load_manifest(path: Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("schema_version") != 1:
        fail(f"unsupported manifest schema in {path}")
    for key in ("target", "bundle_kind", "toolchain", "stage"):
        if key not in data:
            fail(f"manifest {path} is missing '{key}'")
    toolchain = data["toolchain"]
    archive = toolchain.get("archive", {})
    digest = archive.get("digest")
    for key in ("url", "type"):
        if key not in archive:
            fail(f"manifest {path} is missing toolchain.archive.{key}")
    if digest is not None and isinstance(digest, dict):
        for key in ("algorithm", "value"):
            if key not in digest:
                fail(f"manifest {path} is missing toolchain.archive.digest.{key}")
    stage = data["stage"]
    stage.setdefault("root", "toolchain")
    stage.setdefault("license_globs", [])
    stage.setdefault("wrappers", [])

    runtime = toolchain.get("runtime")
    if runtime is not None:
        for key in ("abi", "crt", "compiler", "archiver", "linker"):
            if key not in runtime:
                fail(f"manifest {path} is missing toolchain.runtime.{key}")
        for tool_name in ("compiler", "archiver", "linker"):
            tool = runtime[tool_name]
            for key in ("path", "family", "version"):
                if key not in tool:
                    fail(
                        f"manifest {path} is missing "
                        f"toolchain.runtime.{tool_name}.{key}"
                    )
            safe_relative_path(tool["path"])
        for key in ("target", "size_flag"):
            if key not in runtime["compiler"]:
                fail(f"manifest {path} is missing toolchain.runtime.compiler.{key}")
        runtime["linker"].setdefault("driver_flags", [])
    return data


def load_release_contract(path: Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("schema_version") != 1:
        fail(f"unsupported release contract schema in {path}")
    for key in (
        "phase",
        "install_surface",
        "toolchains_committed_to_git",
        "release_layout",
        "native_backend",
        "lookup_contract",
        "bundled_targets",
        "binary_only_targets",
    ):
        if key not in data:
            fail(f"release contract {path} is missing '{key}'")

    if data["phase"] != "phase1":
        fail(f"unsupported release contract phase '{data['phase']}'")
    if data["install_surface"] != "github-releases":
        fail(f"unsupported release install surface '{data['install_surface']}'")
    if data["toolchains_committed_to_git"]:
        fail("phase 1 release contract must keep toolchains out of git")

    release_layout = data["release_layout"]
    for key in ("binary_position", "toolchain_position", "binary_and_toolchain_are_siblings"):
        if key not in release_layout:
            fail(f"release contract {path} is missing release_layout.{key}")
    if release_layout["binary_position"] != "archive-root":
        fail(
            "unsupported release layout: expected release_layout.binary_position to be "
            "'archive-root'"
        )
    if release_layout["toolchain_position"] != "archive-root/toolchain":
        fail(
            "unsupported release layout: expected release_layout.toolchain_position to be "
            "'archive-root/toolchain'"
        )
    if not release_layout["binary_and_toolchain_are_siblings"]:
        fail("unsupported release layout: oscan binary and toolchain must remain siblings")

    native_backend = data["native_backend"]
    for key in (
        "runtime_source_position",
        "runtime_archive_position_template",
        "source_files",
    ):
        if key not in native_backend:
            fail(f"release contract {path} is missing native_backend.{key}")
    if native_backend["runtime_source_position"] != "archive-root/native-runtime":
        fail("native runtime sources must be staged at archive-root/native-runtime")
    if (
        native_backend["runtime_archive_position_template"]
        != "archive-root/build/runtime-archives/{target}"
    ):
        fail(
            "native runtime archives must be staged at "
            "archive-root/build/runtime-archives/{target}"
        )
    if native_backend["source_files"] != ["osc_native_shim.c", "osc_runtime.h"]:
        fail(
            "native_backend.source_files must contain osc_native_shim.c and "
            "osc_runtime.h in that order"
        )

    lookup_contract = data["lookup_contract"]
    for platform in ("windows", "linux"):
        if platform not in lookup_contract:
            fail(f"release contract {path} is missing lookup_contract.{platform}")
        lookup_entry = lookup_contract[platform]
        for key in ("search_roots", "bin_directories", "compiler_names"):
            if key not in lookup_entry:
                fail(f"release contract {path} is missing lookup_contract.{platform}.{key}")

    return data


def render_release_template(template: str, version: str, field_name: str) -> str:
    try:
        return template.format(version=version)
    except KeyError as exc:
        fail(f"release template '{field_name}' is missing placeholder data: {exc}")


def resolve_release_target(contract: dict, contract_path: Path, target: str) -> dict:
    if target in contract["bundled_targets"]:
        target_spec = dict(contract["bundled_targets"][target])
        target_spec["target_class"] = "bundled"
    elif target in contract["binary_only_targets"]:
        target_spec = dict(contract["binary_only_targets"][target])
        target_spec["target_class"] = "binary-only"
    else:
        fail(f"release contract does not define target '{target}'")

    for key in (
        "binary_name",
        "bundle_kind",
        "archive_format",
        "archive_name_template",
        "archive_root_template",
    ):
        if key not in target_spec:
            fail(f"release target '{target}' is missing '{key}'")

    archive_format = target_spec["archive_format"]
    if archive_format not in ARCHIVE_SUFFIXES:
        fail(f"unsupported archive format '{archive_format}' for target '{target}'")

    archive_name_template = target_spec["archive_name_template"]
    archive_root_template = target_spec["archive_root_template"]
    if "{version}" not in archive_name_template:
        fail(f"release target '{target}' archive_name_template must include '{{version}}'")
    if "{version}" not in archive_root_template:
        fail(f"release target '{target}' archive_root_template must include '{{version}}'")
    if not archive_name_template.endswith(ARCHIVE_SUFFIXES[archive_format]):
        fail(
            f"release target '{target}' archive_name_template must end with "
            f"{ARCHIVE_SUFFIXES[archive_format]}"
        )
    if target_spec["target_class"] == "bundled" and target_spec["bundle_kind"] != "full":
        fail(f"bundled release target '{target}' must use bundle_kind 'full'")
    if target_spec["target_class"] == "binary-only" and target_spec["bundle_kind"] != "binary-only":
        fail(f"binary-only release target '{target}' must use bundle_kind 'binary-only'")

    if target_spec["target_class"] == "bundled":
        manifest_name = target_spec.get("toolchain_manifest")
        if not manifest_name:
            fail(f"release target '{target}' is missing 'toolchain_manifest'")
        manifest_path = contract_path.parent / manifest_name
        if not manifest_path.is_file():
            fail(f"toolchain manifest not found for target '{target}': {manifest_path}")
        target_spec["toolchain_manifest_path"] = manifest_path
    else:
        for key in ("requires_host_compiler", "required_host_toolchain", "note_file"):
            if key not in target_spec:
                fail(f"release target '{target}' is missing '{key}'")
        note_path = contract_path.parent / target_spec["note_file"]
        if not note_path.is_file():
            fail(f"note file not found for target '{target}': {note_path}")
        target_spec["note_file_path"] = note_path

    native_runtime_modes = target_spec.get("native_runtime_modes")
    if not isinstance(native_runtime_modes, list):
        fail(f"release target '{target}' must define native_runtime_modes as a list")
    invalid_modes = [
        mode
        for mode in native_runtime_modes
        if mode not in ("hosted", "freestanding", "freestanding_core")
    ]
    if invalid_modes:
        fail(
            f"release target '{target}' has invalid native runtime mode(s): "
            f"{', '.join(invalid_modes)}"
        )
    native_smoke_mode = target_spec.get("native_smoke_mode")
    if native_runtime_modes and native_smoke_mode not in native_runtime_modes:
        fail(
            f"release target '{target}' native_smoke_mode must name one of its "
            "native_runtime_modes"
        )
    if not native_runtime_modes and native_smoke_mode is not None:
        fail(
            f"release target '{target}' has no native_runtime_modes, so "
            "native_smoke_mode must be null"
        )

    target_spec["target"] = target
    return target_spec


def python_unpack_format(archive_type: str) -> str:
    mapping = {
        "zip": "zip",
        "tar.gz": "gztar",
        "tgz": "gztar",
        "tar.xz": "xztar",
        "tar.bz2": "bztar",
    }
    try:
        return mapping[archive_type]
    except KeyError:
        fail(f"unsupported archive type '{archive_type}'")


def _download_with_curl(url: str, destination: Path) -> bool:
    """Try downloading with curl (preferred on CI). Returns True on success."""
    curl = shutil.which("curl")
    if not curl:
        return False
    result = subprocess.run(
        [
            curl,
            "--proto", "=https",
            "--tlsv1.2",
            "--retry", str(DOWNLOAD_RETRIES),
            "--retry-connrefused",
            "--retry-delay", "2",
            "--location",
            "--silent",
            "--show-error",
            "--fail",
            "--output", str(destination),
            url,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if result.returncode == 0 and destination.exists():
        return True
    if destination.exists():
        destination.unlink()
    print(
        f"warning: curl download failed for {url}: {result.stderr.strip()}",
        file=sys.stderr,
    )
    return False


def download_file(url: str, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    if _download_with_curl(url, destination):
        return
    request = urllib.request.Request(
        url,
        headers={
            "User-Agent": "Oscan release packaging",
            "Accept": "*/*",
        },
    )
    last_error: BaseException | None = None
    for attempt in range(1, DOWNLOAD_RETRIES + 1):
        try:
            with urllib.request.urlopen(request) as response, destination.open("wb") as output:
                shutil.copyfileobj(response, output)
            return
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            last_error = exc
            if destination.exists():
                destination.unlink()
            if attempt == DOWNLOAD_RETRIES:
                break
            delay_seconds = DOWNLOAD_RETRY_BASE_DELAY_SECONDS * (2 ** (attempt - 1))
            print(
                f"warning: download attempt {attempt}/{DOWNLOAD_RETRIES} failed for {url}: {exc}; "
                f"retrying in {delay_seconds}s",
                file=sys.stderr,
            )
            time.sleep(delay_seconds)
    fail(f"failed to download {url} after {DOWNLOAD_RETRIES} attempts: {last_error}")


def copy_path(source: Path, destination: Path) -> None:
    if source.is_symlink():
        target = source.resolve()
        if target.is_dir():
            destination.mkdir(parents=True, exist_ok=True)
            for child in sorted(target.iterdir(), key=lambda item: item.name):
                copy_path(child, destination / child.name)
            return
        destination.parent.mkdir(parents=True, exist_ok=True)
        try:
            shutil.copy2(target, destination)
        except FileNotFoundError as exc:
            fail(f"failed to copy symlink target '{target}' to '{destination}': {exc}")
        return
    if source.is_dir():
        destination.mkdir(parents=True, exist_ok=True)
        for child in sorted(source.iterdir(), key=lambda item: item.name):
            copy_path(child, destination / child.name)
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        shutil.copy2(source, destination)
    except FileNotFoundError as exc:
        fail(f"failed to copy '{source}' to '{destination}': {exc}")


def copy_tree_contents(source_root: Path, destination_root: Path) -> None:
    ensure_clean_dir(destination_root)
    for entry in sorted(source_root.iterdir(), key=lambda item: item.name):
        copy_path(entry, destination_root / entry.name)


def handle_remove_readonly(function, path, exc_info) -> None:
    _, error, _ = exc_info
    if isinstance(error, PermissionError):
        os.chmod(path, stat.S_IWRITE)
        function(path)
        return
    if getattr(error, "winerror", None) == 145 and Path(path).is_dir():
        for child in Path(path).iterdir():
            remove_path(child)
        function(path)
        return
    raise error


def remove_path(path: Path) -> None:
    if not path.exists() and not path.is_symlink():
        return
    if path.is_symlink() or path.is_file():
        path.unlink()
        return
    if os.name == "nt":
        result = subprocess.run(
            ["cmd", "/c", "rmdir", "/s", "/q", str(path)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        if result.returncode == 0 or not path.exists():
            return
    shutil.rmtree(path, onerror=handle_remove_readonly)


def _extract_with_system_tar(archive_path: Path, dest: Path) -> bool:
    """Extract using system tar. Handles absolute symlinks that Python 3.14+ rejects."""
    tar_bin = shutil.which("tar")
    if not tar_bin:
        return False
    result = subprocess.run(
        [tar_bin, "-xf", str(archive_path), "-C", str(dest)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    return result.returncode == 0


def extract_archive(archive_path: Path, archive_type: str, destination: Path, strip_components: int) -> None:
    temp_root = archive_path.parent / f".extract-{archive_path.stem}"
    ensure_clean_dir(temp_root)
    try:
        if _extract_with_system_tar(archive_path, temp_root):
            pass  # system tar succeeded
        else:
            shutil.unpack_archive(
                str(archive_path),
                str(temp_root),
                format=python_unpack_format(archive_type),
            )
        source_root = temp_root
        for _ in range(strip_components):
            children = [child for child in source_root.iterdir()]
            if len(children) != 1 or not children[0].is_dir():
                fail(
                    f"cannot strip {strip_components} path component(s) from {archive_path.name}; "
                    "expected a single top-level directory"
                )
            source_root = children[0]
        destination.parent.mkdir(parents=True, exist_ok=True)
        remove_path(destination)
        shutil.move(str(source_root), str(destination))
    finally:
        if temp_root.exists():
            remove_path(temp_root)


def create_wrapper(destination_root: Path, wrapper_spec: dict) -> None:
    wrapper_path = destination_root / safe_relative_path(wrapper_spec["path"])
    target = wrapper_spec["target"]
    kind = wrapper_spec["kind"]
    wrapper_path.parent.mkdir(parents=True, exist_ok=True)
    if kind == "posix-exec":
        wrapper_path.write_text(
            textwrap.dedent(
                f"""\
                #!/usr/bin/env sh
                set -eu
                SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
                exec "$SCRIPT_DIR/{target}" "$@"
                """
            ),
            encoding="utf-8",
            newline="\n",
        )
        wrapper_path.chmod(0o755)
        return
    fail(f"unsupported wrapper kind '{kind}'")


def fix_absolute_symlinks(root: Path) -> None:
    """Convert absolute symlinks to relative ones when the target exists in the tree.

    Sysroot-based cross toolchains (e.g. musl-cross-make's
    x86_64-linux-musl/lib/ld-musl-x86_64.so.1 -> /lib/libc.so) ship symlinks
    whose absolute target is only meaningful once resolved against the
    toolchain's own embedded sysroot directory, not the extraction root: "/"
    there really means "the sysroot", not "the tree root". Trying only
    `root / relative_target` therefore misses these — the real file lives at
    `root/<sysroot-dir>/<relative_target>` — and leaves a dangling absolute
    symlink pointing at the *host's* filesystem once the toolchain is moved
    (this is the actual, fixable cause of past "not relocatable" reports;
    every toolchain tool here is itself a statically linked executable, so
    none of them depend on this symlink to run). Every ancestor directory of
    the symlink itself is tried as a stand-in root, innermost first, falling
    back to the toolchain's outer extraction root last: the first ancestor
    whose combination with the absolute path's own components exists on disk
    is used to compute the new, relative target.
    """
    if os.name == "nt":
        return
    for path in root.rglob("*"):
        if not path.is_symlink():
            continue
        target = os.readlink(path)
        if not os.path.isabs(target):
            continue
        relative_target = Path(target).relative_to("/")
        ancestors = []
        current = path.parent
        while True:
            ancestors.append(current)
            if current == root or current == current.parent:
                break
            current = current.parent
        for ancestor in ancestors:
            candidate = ancestor / relative_target
            if candidate.exists() or candidate.is_symlink():
                new_target = os.path.relpath(candidate, path.parent)
                path.unlink()
                os.symlink(new_target, path)
                break


def prune_toolchain(root: Path, prune_config: dict) -> None:
    """Remove unnecessary files from extracted toolchain to reduce bundle size."""
    remove_globs = prune_config.get("remove_globs", [])
    strip_debug = prune_config.get("strip_debug", False)
    keep_globs = prune_config.get("keep_globs", [])

    if not remove_globs and not strip_debug:
        return

    # Build keep set first (files that must not be deleted even if matched by remove)
    keep_paths: set[Path] = set()
    for pattern in keep_globs:
        for match in root.rglob(pattern):
            keep_paths.add(match.resolve())

    # Remove files matching remove_globs (dirs are removed if emptied)
    removed_count = 0
    for pattern in remove_globs:
        for match in sorted(root.rglob(pattern), key=lambda p: p.as_posix(), reverse=True):
            if match.resolve() in keep_paths:
                continue
            if match.is_symlink() or match.is_file():
                match.unlink()
                removed_count += 1
            elif match.is_dir():
                remove_path(match)
                removed_count += 1

    # Clean up empty directories left behind
    for dirpath in sorted(root.rglob("*"), key=lambda p: len(p.parts), reverse=True):
        if dirpath.is_dir() and not any(dirpath.iterdir()):
            dirpath.rmdir()

    # Strip debug symbols from binaries and archives
    if strip_debug and os.name != "nt":
        strip_bin = shutil.which("strip")
        if strip_bin:
            for path in root.rglob("*"):
                if not path.is_file() or path.is_symlink():
                    continue
                suffix = path.suffix.lower()
                if suffix in (".a", ".o"):
                    subprocess.run(
                        [strip_bin, "--strip-debug", str(path)],
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL,
                        check=False,
                    )
                elif suffix in ("", ".so") and os.access(path, os.X_OK):
                    subprocess.run(
                        [strip_bin, "--strip-unneeded", str(path)],
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL,
                        check=False,
                    )

    print(f"Pruned toolchain: removed {removed_count} entries", file=sys.stderr)


def fetch_toolchain(manifest_path: Path, download_dir: Path, destination: Path) -> tuple[dict, Path]:
    manifest = load_manifest(manifest_path)
    archive = manifest["toolchain"]["archive"]
    digest = archive.get("digest")
    url = archive["url"]
    file_name = Path(urllib.parse.urlparse(url).path).name
    if not file_name:
        fail(f"cannot derive archive file name from {url}")
    download_path = download_dir / file_name

    if digest is not None and isinstance(digest, dict):
        expected = digest["value"].lower()
        algorithm = digest["algorithm"].lower()
        if not download_path.exists() or compute_digest(download_path, algorithm) != expected:
            if download_path.exists():
                download_path.unlink()
            download_file(url, download_path)
        actual = compute_digest(download_path, algorithm)
        if actual.lower() != expected:
            fail(
                f"digest mismatch for {download_path.name}: expected {expected}, got {actual}"
            )
    else:
        # No digest validation — download if not cached
        if not download_path.exists():
            download_file(url, download_path)

    strip_components = int(manifest["toolchain"].get("extract", {}).get("strip_components", 0))
    extract_archive(download_path, archive["type"], destination, strip_components)
    fix_absolute_symlinks(destination)

    # Ensure all files are writable (zip archives may preserve read-only attributes)
    if os.name == "nt":
        for path in destination.rglob("*"):
            if path.is_file() and not path.is_symlink():
                try:
                    path.chmod(stat.S_IREAD | stat.S_IWRITE)
                except OSError:
                    pass

    prune_config = manifest["toolchain"].get("prune", {})
    if prune_config:
        prune_toolchain(destination, prune_config)

    for wrapper in manifest["stage"].get("wrappers", []):
        create_wrapper(destination, wrapper)

    return manifest, destination


def write_install_readme(
    path: Path,
    target_spec: dict,
    asset_name: str,
    cross_linker_targets: list[str] | None = None,
) -> None:
    platform, arch = target_spec["target"].split("-", 1)
    bundle_kind = target_spec["bundle_kind"]
    if platform == "windows":
        install_hint = "Run install.ps1 from this directory, or keep this directory on PATH."
        extra = "This bundle keeps oscan.exe next to toolchain/ so bundled compiler discovery works after installation."
    elif platform == "linux":
        install_hint = "Run install.sh from this directory, or keep this directory on PATH."
        extra = "This full bundle keeps oscan next to toolchain/ so bundled compiler discovery works after installation."
    else:
        install_hint = "Run install.sh from this directory, or copy oscan somewhere on PATH."
        extra = (
            f"macOS phase 1 archives ship the {target_spec['binary_name']} binary only; "
            f"{target_spec['required_host_toolchain']} remains required."
        )
        if target_spec.get("note_file"):
            extra = f"{extra} See {target_spec['note_file']}."

    cross_linker_note = ""
    if cross_linker_targets:
        targets_list = "\n".join(f"  - cross-linkers/{t}/" for t in cross_linker_targets)
        cross_linker_note = (
            "\n"
            "This bundle also includes static cross-linker sidecars for\n"
            "cross-compiling `--backend native` freestanding executables to\n"
            "other Linux targets from this host, without needing an external\n"
            "toolchain. Each target's directory ships both the linker binary\n"
            "and its matching freestanding runtime archive:\n"
            f"{targets_list}\n"
            "To use one, point oscan at both explicitly:\n"
            "  OSCAN_NATIVE_LINKER=./cross-linkers/<target>/<triple>-ld \\\n"
            "  OSCAN_NATIVE_LINKER_FLAVOR=elf \\\n"
            "  OSCAN_RUNTIME_ARCHIVE_DIR=./cross-linkers/<target> \\\n"
            "  oscan prog.osc --backend native --native-target <target> -o prog\n"
            f"This binary's own embedded linker only targets {target_spec['target']};\n"
            "the sidecars are separate opt-in binaries for the other targets listed above.\n"
        )

    text = textwrap.dedent(
        f"""\
        Oscan release asset: {asset_name}
        Platform: {platform} {arch}
        Bundle type: {bundle_kind}

        {install_hint}
        {extra}
        """
    ) + cross_linker_note + textwrap.dedent(
        """
        GitHub Releases are the canonical install surface for phase 1 bundles.
        """
    )
    path.write_text(text, encoding="utf-8", newline="\n")


def copy_license_files(source_root: Path, destination_root: Path, globs: list[str]) -> list[str]:
    copied: list[str] = []
    seen: set[str] = set()
    for pattern in globs:
        for candidate in sorted(source_root.rglob(pattern), key=lambda item: item.as_posix()):
            if not candidate.is_file():
                continue
            relative = candidate.relative_to(source_root).as_posix()
            if relative in seen:
                continue
            seen.add(relative)
            copy_path(candidate, destination_root / relative)
            copied.append(relative)
    return copied


def write_provenance_file(path: Path, manifest: dict, copied_licenses: list[str]) -> None:
    digest = manifest["toolchain"]["archive"].get("digest")
    digest_line = (
        f"Archive digest ({digest['algorithm']}): {digest['value']}"
        if digest and isinstance(digest, dict)
        else "Archive digest: not verified"
    )
    licenses = "\n".join(f"- {entry}" for entry in copied_licenses) or "- none matched configured globs"
    text = textwrap.dedent(
        f"""\
        Toolchain vendor: {manifest["toolchain"]["vendor"]}
        Toolchain version: {manifest["toolchain"]["version"]}
        Target: {manifest["target"]}
        Archive URL: {manifest["toolchain"]["archive"]["url"]}
        {digest_line}

        Copied license files:
        {licenses}
        """
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")


def create_zip_archive(bundle_dir: Path, archive_path: Path) -> None:
    if archive_path.exists():
        archive_path.unlink()
    result = subprocess.run(
        ["tar", "-a", "-cf", str(archive_path), "-C", str(bundle_dir.parent), bundle_dir.name],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        fail(f"failed to create {archive_path.name}: {result.stderr.strip()}")


def normalize_tarinfo(info: tarfile.TarInfo) -> tarfile.TarInfo:
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = archive_epoch()
    return info


def create_tar_archive(bundle_dir: Path, archive_path: Path, archive_format: str) -> None:
    mode = {
        "tar.gz": "w:gz",
        "tar.xz": "w:xz",
    }.get(archive_format)
    if mode is None:
        fail(f"unsupported tar archive format '{archive_format}'")
    if archive_path.exists():
        archive_path.unlink()
    entries = sorted(
        [bundle_dir] + list(bundle_dir.rglob("*")),
        key=lambda item: item.relative_to(bundle_dir.parent).as_posix(),
    )
    tar_kwargs: dict[str, object] = {"format": tarfile.GNU_FORMAT}
    if archive_format == "tar.gz":
        tar_kwargs["compresslevel"] = 9
    with tarfile.open(archive_path, mode, **tar_kwargs) as archive:
        for entry in entries:
            arcname = entry.relative_to(bundle_dir.parent).as_posix()
            if entry.is_symlink():
                info = tarfile.TarInfo(arcname)
                info.type = tarfile.SYMTYPE
                info.linkname = os.readlink(entry)
                info.mode = entry.lstat().st_mode & 0o777
                archive.addfile(normalize_tarinfo(info))
                continue
            if entry.is_dir():
                info = archive.gettarinfo(str(entry), arcname)
                archive.addfile(normalize_tarinfo(info))
                continue
            with entry.open("rb") as handle:
                info = archive.gettarinfo(str(entry), arcname)
                archive.addfile(normalize_tarinfo(info), handle)


def _release_bundle_relative_path(position: str, field_name: str, target: str) -> Path:
    try:
        rendered = position.format(target=target)
    except KeyError as exc:
        fail(f"release contract field {field_name} uses an unknown placeholder: {exc}")
    prefix = "archive-root/"
    if not rendered.startswith(prefix):
        fail(f"release contract field {field_name} must start with '{prefix}'")
    return safe_relative_path(rendered[len(prefix) :])


def validate_runtime_archive_release_toolchain(
    runtime_contract: dict,
    target: str,
    manifest: dict,
    manifest_path: Path,
) -> None:
    expected = runtime_contract.get("targets", {}).get(target, {}).get(
        "release_toolchain"
    )
    if expected is None:
        return
    provenance = manifest.get("toolchain")
    if not isinstance(provenance, dict):
        fail(
            f"native runtime manifest {manifest_path} has no exact release "
            "toolchain provenance"
        )
    compiler = provenance.get("compiler", {})
    archiver = provenance.get("archiver", {})
    linker = provenance.get("linker", {})
    checks = (
        ("source manifest", expected["manifest"], provenance.get("source_manifest")),
        ("vendor", expected["vendor"], provenance.get("vendor")),
        ("version", expected["version"], provenance.get("version")),
        ("ABI", expected["abi"], provenance.get("abi")),
        ("CRT", expected["crt"], provenance.get("crt")),
        (
            "compiler family",
            expected["compiler_family"],
            compiler.get("family"),
        ),
        (
            "compiler target",
            expected["compiler_target"],
            compiler.get("target"),
        ),
        (
            "compiler size flag",
            expected["compile_size_flag"],
            compiler.get("size_flag"),
        ),
        (
            "archiver family",
            expected["archiver_family"],
            archiver.get("family"),
        ),
        ("linker family", expected["linker_family"], linker.get("family")),
        (
            "linker driver flags",
            expected["linker_driver_flags"],
            linker.get("driver_flags"),
        ),
    )
    for label, wanted, actual in checks:
        if wanted != actual:
            fail(
                f"native runtime manifest {manifest_path} {label} mismatch: "
                f"expected {wanted!r}, got {actual!r}"
            )
    for label, wanted, actual in (
        ("compiler version", expected["compiler_version"], compiler.get("version", "")),
        ("archiver version", expected["archiver_version"], archiver.get("version", "")),
        ("linker version", expected["linker_version"], linker.get("version", "")),
    ):
        if wanted not in actual:
            fail(
                f"native runtime manifest {manifest_path} {label} mismatch: "
                f"expected {wanted!r} in {actual!r}"
            )

    source_manifest = load_manifest(
        RUNTIME_ARCHIVE_CONTRACT_PATH.parent / expected["manifest"]
    )
    expected_digest = source_manifest["toolchain"]["archive"].get("digest")
    if provenance.get("archive_digest") != expected_digest:
        fail(
            f"native runtime manifest {manifest_path} toolchain archive digest "
            "does not match the pinned release manifest"
        )

    # Schema 2 archives must carry the precompiled native shim (§3.2 of
    # docs/design/native-link-embedding.md); the freestanding native backend
    # no longer compiles osc_native_shim.c locally, so a schema-2 archive
    # that is missing the shim member would silently break that contract.
    if manifest.get("schema_version") == 2:
        if manifest.get("contains_native_shim") is not True:
            fail(
                f"native runtime manifest {manifest_path} is schema_version 2 but "
                "contains_native_shim is not true; rebuild it with "
                "'scripts/build-runtime-archive.ps1|.sh' from the current "
                "runtime-archive-contract.json"
            )
        if manifest.get("native_shim_member") != "osc_native_shim.o":
            fail(
                f"native runtime manifest {manifest_path} is schema_version 2 but "
                f"native_shim_member is {manifest.get('native_shim_member')!r}, "
                "expected 'osc_native_shim.o'"
            )

    if (
        target in ("linux-x86_64", "linux-aarch64", "linux-riscv64")
        and manifest.get("mode") in {"freestanding", "freestanding_core"}
        and manifest.get("embedded_bearssl") is not True
    ):
        fail(
            f"native runtime manifest {manifest_path} does not embed BearSSL; "
            f"Linux release freestanding archives for {target} must be built with "
            f"packaging/prebuilt/{target}/libbearssl.a present"
        )


def stage_native_runtime_assets(
    contract: dict,
    target_spec: dict,
    bundle_dir: Path,
    runtime_archive_dir: Path,
) -> None:
    target = target_spec["target"]
    native_spec = contract["native_backend"]
    runtime_contract = load_runtime_archive_contract(RUNTIME_ARCHIVE_CONTRACT_PATH)

    source_destination = bundle_dir / _release_bundle_relative_path(
        native_spec["runtime_source_position"],
        "native_backend.runtime_source_position",
        target,
    )
    for source_name in native_spec["source_files"]:
        source_path = REPO_ROOT / "runtime" / safe_relative_path(source_name)
        if not source_path.is_file():
            fail(f"native runtime source not found: {source_path}")
        copy_path(source_path, source_destination / source_name)

    archive_destination = bundle_dir / _release_bundle_relative_path(
        native_spec["runtime_archive_position_template"],
        "native_backend.runtime_archive_position_template",
        target,
    )
    for mode in target_spec["native_runtime_modes"]:
        mode_spec = runtime_contract["modes"][mode]
        archive_path = runtime_archive_dir / mode_spec["archive_name"]
        manifest_path = runtime_archive_dir / mode_spec["manifest_name"]
        if not archive_path.is_file() or not manifest_path.is_file():
            fail(
                f"native runtime {mode} archive pair for '{target}' is missing from "
                f"{runtime_archive_dir}; run scripts/build-runtime-archive.ps1|.sh "
                f"--target {target} --mode {mode} before staging the release"
            )
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            fail(f"cannot read native runtime manifest {manifest_path}: {exc}")
        if manifest.get("target") != target or manifest.get("mode") != mode:
            fail(
                f"native runtime manifest {manifest_path} identifies "
                f"{manifest.get('target')}/{manifest.get('mode')}, expected {target}/{mode}"
            )
        validate_runtime_archive_release_toolchain(
            runtime_contract, target, manifest, manifest_path
        )
        expected_digest = manifest.get("sha256")
        actual_digest = compute_digest(archive_path, "sha256")
        if expected_digest != actual_digest:
            fail(
                f"native runtime archive digest mismatch for {archive_path}: "
                f"manifest has {expected_digest!r}, actual is {actual_digest}"
            )
        copy_path(archive_path, archive_destination / archive_path.name)
        copy_path(manifest_path, archive_destination / manifest_path.name)


def stage_release(args: argparse.Namespace) -> int:
    contract_path = Path(args.contract).resolve()
    contract = load_release_contract(contract_path)
    target_spec = resolve_release_target(contract, contract_path, args.target)
    target = target_spec["target"]
    platform = target.split("-", 1)[0]
    version = args.version
    bundle_name = render_release_template(
        target_spec["archive_root_template"], version, "archive_root_template"
    )
    archive_name = render_release_template(
        target_spec["archive_name_template"], version, "archive_name_template"
    )
    expected_toolchain_root = contract["release_layout"]["toolchain_position"].removeprefix(
        "archive-root/"
    )
    output_dir = Path(args.output_dir).resolve()
    bundle_dir = output_dir / "stage" / bundle_name
    ensure_clean_dir(bundle_dir)

    binary_source = Path(args.binary).resolve()
    if not binary_source.is_file():
        fail(f"binary not found: {binary_source}")
    binary_name = target_spec["binary_name"]
    binary_destination = bundle_dir / binary_name
    copy_path(binary_source, binary_destination)
    if platform != "windows":
        binary_destination.chmod(0o755)

    install_source = REPO_ROOT / "scripts" / (
        "install-oscan.ps1" if platform == "windows" else "install-oscan.sh"
    )
    install_destination = bundle_dir / (
        "install.ps1" if platform == "windows" else "install.sh"
    )
    copy_path(install_source, install_destination)
    if platform != "windows":
        install_destination.chmod(0o755)

    cross_linker_targets: list[str] = []
    if args.cross_linker_sidecar_dir:
        sidecar_source = Path(args.cross_linker_sidecar_dir).resolve()
        if sidecar_source.is_dir():
            cross_linkers_dest = bundle_dir / "cross-linkers"
            for child in sorted(sidecar_source.iterdir(), key=lambda item: item.name):
                if not child.is_dir():
                    continue
                copy_path(child, cross_linkers_dest / child.name)
                if platform != "windows":
                    for linker_bin in (cross_linkers_dest / child.name).iterdir():
                        if linker_bin.is_file():
                            linker_bin.chmod(0o755)
                cross_linker_targets.append(child.name)

    write_install_readme(
        bundle_dir / "README-install.txt",
        target_spec,
        archive_name,
        cross_linker_targets=cross_linker_targets,
    )

    runtime_archive_dir = (
        Path(args.runtime_archive_dir).resolve()
        if args.runtime_archive_dir
        else REPO_ROOT / "build" / "runtime-archives" / target
    )
    if target_spec["native_runtime_modes"]:
        stage_native_runtime_assets(
            contract,
            target_spec,
            bundle_dir,
            runtime_archive_dir,
        )

    if target_spec["target_class"] == "bundled":
        manifest_path = Path(target_spec["toolchain_manifest_path"])
        manifest = load_manifest(manifest_path)
        if manifest["target"] != target:
            fail(f"manifest target {manifest['target']} does not match requested {target}")
        if manifest["bundle_kind"] != target_spec["bundle_kind"]:
            fail(
                f"manifest bundle kind {manifest['bundle_kind']} does not match release "
                f"contract bundle kind {target_spec['bundle_kind']} for {target}"
            )
        if manifest["stage"]["root"] != expected_toolchain_root:
            fail(
                f"manifest stage root '{manifest['stage']['root']}' does not match the "
                "release contract sibling toolchain layout"
            )
        toolchain_root = bundle_dir / safe_relative_path(manifest["stage"]["root"])
        fetched_manifest, fetched_root = fetch_toolchain(
            manifest_path,
            output_dir / "downloads",
            toolchain_root,
        )
        copy_path(manifest_path, bundle_dir / Path(target_spec["toolchain_manifest"]).name)
        copied_licenses = copy_license_files(
            fetched_root,
            bundle_dir / "LICENSES" / "toolchain",
            fetched_manifest["stage"].get("license_globs", []),
        )
        write_provenance_file(
            bundle_dir / "LICENSES" / "toolchain-source.txt",
            fetched_manifest,
            copied_licenses,
        )
    else:
        note_path = Path(target_spec["note_file_path"])
        copy_path(note_path, bundle_dir / Path(target_spec["note_file"]).name)

    archive_path = output_dir / archive_name
    archive_path.parent.mkdir(parents=True, exist_ok=True)
    if target_spec["archive_format"] == "zip":
        create_zip_archive(bundle_dir, archive_path)
    else:
        create_tar_archive(bundle_dir, archive_path, target_spec["archive_format"])

    print(str(archive_path))
    return 0


def fetch_toolchain_command(args: argparse.Namespace) -> int:
    _, destination = fetch_toolchain(
        Path(args.manifest).resolve(),
        Path(args.download_dir).resolve(),
        Path(args.destination).resolve(),
    )
    print(str(destination))
    return 0


def detect_host_target_command(_args: argparse.Namespace) -> int:
    print(detect_host_target())
    return 0


def write_checksums(args: argparse.Namespace) -> int:
    files = [Path(item).resolve() for item in args.files]
    missing = [str(item) for item in files if not item.is_file()]
    if missing:
        fail(f"cannot checksum missing file(s): {', '.join(missing)}")
    lines = []
    for file_path in sorted(files, key=lambda item: item.name):
        digest = compute_digest(file_path, "sha256")
        lines.append(f"{digest}  {file_path.name}")
    output = Path(args.output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(str(output))
    return 0


def load_runtime_archive_contract(path: Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    # Schema 1: no precompiled native shim member. Schema 2 (current): every
    # mode's "sources" includes "osc_native_shim.c" and sets
    # "contains_native_shim": true (see docs/design/native-link-embedding.md
    # §3.1). Both are accepted here; build_runtime_archive derives
    # contains_native_shim per mode from "sources" rather than trusting a
    # stale top-level version number.
    if data.get("schema_version") not in (1, 2):
        fail(f"unsupported runtime archive contract schema in {path}")
    for key in ("modes", "targets"):
        if key not in data:
            fail(f"runtime archive contract {path} is missing '{key}'")
    for target, target_spec in data["targets"].items():
        release_toolchain = target_spec.get("release_toolchain")
        if release_toolchain is None:
            continue
        for key in (
            "manifest",
            "vendor",
            "version",
            "abi",
            "crt",
            "compiler_family",
            "compiler_version",
            "compiler_target",
            "compile_size_flag",
            "archiver_family",
            "archiver_version",
            "linker_family",
            "linker_version",
            "linker_driver_flags",
        ):
            if key not in release_toolchain:
                fail(
                    f"runtime archive contract {path} is missing "
                    f"targets.{target}.release_toolchain.{key}"
                )
    return data


def detect_host_target() -> str:
    system = platform.system()
    machine = platform.machine().lower()
    arch = "x86_64" if machine in ("x86_64", "amd64") else machine
    if system == "Windows":
        return f"windows-{arch}"
    if system == "Linux":
        return f"linux-{arch}"
    if system == "Darwin":
        return f"macos-{arch}"
    fail(f"cannot auto-detect a runtime archive target for host platform '{system}'; pass --target explicitly")


def _dedupe_preserve_order(items: list[str]) -> list[str]:
    seen: set[str] = set()
    ordered: list[str] = []
    for item in items:
        if item not in seen:
            seen.add(item)
            ordered.append(item)
    return ordered


def _first_on_path(candidates: list[str]) -> str | None:
    return next((candidate for candidate in candidates if shutil.which(candidate)), None)


def _toolchain_fetch_hint(target: str) -> str:
    return (
        f"fetch it first, e.g.:\n"
        f"  scripts/fetch-toolchain.ps1|.sh --manifest packaging/toolchains/{target}.json "
        f"--destination build/toolchain-{target}"
    )


def _cc_candidates_for_target(target: str, host_target: str | None) -> list[str]:
    """Ordered list of C compiler executable names worth probing with
    shutil.which() for the given archive target.

    Two sources of candidates are combined:
      - Triple-prefixed cross-compiler names produced by the bundled
        toolchains that scripts/fetch-toolchain.ps1|.sh mirror (see
        packaging/toolchains/<target>.json).
      - Plain host-compiler names, but ONLY when `target` matches the
        detected host platform: a bare `gcc`/`clang`/`cc` on PATH targets
        whatever platform it was built for, so it must not be assumed to
        produce binaries for a *different* target.
    """
    candidates: list[str] = []
    if target == "linux-x86_64":
        candidates.append("x86_64-linux-musl-gcc")
    elif target == "linux-aarch64":
        candidates.append("aarch64-linux-musl-gcc")
    elif target == "linux-riscv64":
        candidates.append("riscv64-linux-musl-gcc")
    elif target == "windows-x86_64":
        # llvm-mingw (the bundled Windows toolchain) ships a bare clang.exe
        # driven by its own default target triple; some standalone MinGW-w64
        # installs additionally expose a triple-prefixed gcc.
        candidates += ["x86_64-w64-mingw32-gcc"]

    if target == host_target:
        if target.startswith("windows"):
            # MinGW-w64's gcc is the common Windows-native compiler; clang is
            # also viable when the llvm-mingw toolchain is on PATH.
            candidates += ["gcc", "clang"]
        elif target.startswith("macos"):
            candidates += ["clang", "cc"]
        else:
            candidates += ["cc", "gcc", "clang"]
    elif target.startswith("windows"):
        candidates.append("clang")

    return _dedupe_preserve_order(candidates)


def default_cc_for_target(target: str) -> str:
    env_cc = os.environ.get("OSCAN_ARCHIVE_CC")
    if env_cc:
        return env_cc

    try:
        host_target = detect_host_target()
    except SystemExit:
        host_target = None

    candidates = _cc_candidates_for_target(target, host_target)
    found = _first_on_path(candidates)
    if found:
        return found

    tried = ", ".join(candidates) if candidates else "(no known candidates for this target)"
    fail(
        f"no C compiler found on PATH for target '{target}' (tried: {tried}).\n"
        f"Pass --cc explicitly, set $OSCAN_ARCHIVE_CC, or {_toolchain_fetch_hint(target)}"
    )


def default_ar_for(cc: str) -> str:
    env_ar = os.environ.get("OSCAN_ARCHIVE_AR")
    if env_ar:
        return env_ar

    candidates: list[str] = []
    lowered = cc.lower()
    # (cc suffix, matching archiver suffix) — triple-prefixed toolchains like
    # x86_64-linux-musl-gcc keep their separating dash (-> ...-musl-ar), while
    # bare gcc/clang/cc do not (-> ar).
    for cc_suffix, ar_suffix in (
        ("-gcc", "-ar"),
        ("-clang", "-ar"),
        ("gcc", "ar"),
        ("clang", "ar"),
        ("cc", "ar"),
    ):
        if lowered.endswith(cc_suffix):
            prefix = cc[: len(cc) - len(cc_suffix)]
            candidates.append(prefix + ar_suffix)
            break
    if "clang" in lowered:
        # llvm-mingw and other clang-based toolchains ship llvm-ar rather
        # than (or in addition to) a plain binutils 'ar'.
        candidates.append("llvm-ar")
    candidates.append("ar")
    candidates = _dedupe_preserve_order(candidates)

    found = _first_on_path(candidates)
    if found:
        return found

    fail(
        f"no archiver found on PATH for compiler '{cc}' (tried: {', '.join(candidates)}).\n"
        f"Pass --ar explicitly, set $OSCAN_ARCHIVE_AR, or fetch a matching toolchain "
        f"via scripts/fetch-toolchain.ps1|.sh."
    )


def _target_tag_matches_triple(target: str, triple: str) -> bool:
    try:
        target_platform, target_arch = target.split("-", 1)
    except ValueError:
        return False

    normalized = triple.strip().lower()
    triple_arch = normalized.split("-", 1)[0]
    arch_matches = {
        "x86_64": {"x86_64", "amd64"},
        "aarch64": {"aarch64", "arm64"},
    }.get(target_arch, {target_arch})
    if triple_arch not in arch_matches:
        return False

    if target_platform == "windows":
        return "mingw" in normalized or (
            "windows" in normalized and "gnu" in normalized
        )
    if target_platform == "linux":
        return "linux" in normalized
    if target_platform == "macos":
        return "darwin" in normalized or "apple" in normalized
    return False


def _probe_compiler_target(cc: str, compiler_args: list[str]) -> str:
    command = [cc, *compiler_args, "-dumpmachine"]
    try:
        result = subprocess.run(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
        )
    except OSError as exc:
        fail(f"could not launch compiler target probe '{cc} -dumpmachine': {exc}")
    reported = result.stdout.strip()
    if result.returncode != 0 or not reported:
        detail = result.stderr.strip() or result.stdout.strip() or "no target triple reported"
        fail(f"compiler target probe failed for '{cc}': {detail}")
    return reported


def _tool_identity_output(command: list[str], description: str) -> str:
    try:
        result = subprocess.run(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            check=False,
        )
    except OSError as exc:
        fail(f"could not launch {description} probe '{command[0]}': {exc}")
    output = result.stdout.strip()
    if result.returncode != 0 or not output:
        detail = output or f"exit status {result.returncode}"
        fail(f"{description} probe failed for '{command[0]}': {detail}")
    return output


def _concise_tool_version(output: str) -> str:
    """Pick the single line of `--version` output that actually names a version.

    LLVM tools sometimes lead with a banner line before the version line
    (e.g. older llvm-ar: "LLVM (http://llvm.org/):" then "  LLVM version
    X.Y.Z"), so the first couple of lines are searched for one that mentions
    "version". That search is deliberately bounded to those first two
    lines rather than the whole output: GNU tools (gcc/ar/ld) never say
    "version" on their own self-identifying first line (e.g. "GNU ar (GNU
    Binutils) 2.37"), but their trailing GPL boilerplate does ("...GNU
    General Public License version 3..."), which would otherwise be
    mistaken for the tool's own version line. Falling back to the first
    line covers both that GNU case and single-line banners (e.g. lld's
    "LLD X.Y.Z (compatible with GNU linkers)", which never says "version"
    at all) — every format seen here already carries its version number on
    line one whenever "version" isn't found near the top.
    """
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    for line in lines[:2]:
        if "version" in line.lower():
            return line
    return lines[0] if lines else "unknown"


def _compiler_family(cc: str) -> str:
    name = Path(cc).name.lower().removesuffix(".exe")
    return "clang" if "clang" in name else "gcc"


def _archiver_family(ar: str) -> str:
    name = Path(ar).name.lower().removesuffix(".exe")
    return "llvm-ar" if "llvm-ar" in name else "gnu-ar"


def _canonicalize_tool_path(tool: str) -> str:
    """Resolve a compiler/archiver/linker reference to a canonical absolute path.

    `tool` may be a bare command name discovered on PATH (e.g. "clang"), a
    relative path, or an already-absolute path. Recording any of the first
    two forms verbatim in a runtime archive manifest is ambiguous at best and
    spoofable at worst (a relative path canonicalized against the wrong CWD
    could silently point at an attacker-planted binary in some other
    directory). To avoid that: bare/relative inputs are first located via
    shutil.which(), which searches the real PATH, and the result — or the
    original input if it was already absolute — is then canonicalized with
    Path.resolve(), which follows symlinks and normalizes '.'/'..'
    components. This makes the manifest's recorded path unambiguous
    provenance; it does not by itself make that path *trusted* for
    execution — the Rust-side reader independently validates that against a
    known trusted toolchain root before ever running it.
    """
    path = Path(tool)
    if not path.is_absolute():
        found = shutil.which(tool)
        if found:
            path = Path(found)
    return str(path.resolve())


def _toolchain_root_from_tool(tool: str, relative_path: str) -> Path:
    tool_path = Path(tool).resolve()
    relative = safe_relative_path(relative_path)
    root = tool_path
    for _ in relative.parts:
        root = root.parent
    expected = (root / relative).resolve()
    if expected != tool_path:
        fail(
            f"tool '{tool_path}' does not match manifest-relative path "
            f"'{relative_path}'"
        )
    return root


def _runtime_toolchain_provenance(
    *,
    target: str,
    cc: str,
    ar: str,
    cc_target: str,
    target_spec: dict,
    toolchain_manifest_path: str | None,
) -> dict:
    cc_output = _tool_identity_output([cc, "--version"], "compiler version")
    ar_output = _tool_identity_output([ar, "--version"], "archiver version")
    compiler = {
        "command": cc,
        "family": _compiler_family(cc),
        "version": _concise_tool_version(cc_output),
        "target": cc_target,
        "size_flag": "-Oz" if _compiler_family(cc) == "clang" else "-Os",
    }
    archiver = {
        "command": ar,
        "family": _archiver_family(ar),
        "version": _concise_tool_version(ar_output),
    }
    provenance = {
        "source_manifest": None,
        "vendor": compiler["family"],
        "version": compiler["version"],
        "archive_digest": None,
        "abi": "gnu" if target.startswith("windows-") else None,
        "crt": None,
        "compiler": compiler,
        "archiver": archiver,
        "linker": {
            "command": None,
            # Clang only implies LLD for the pinned Windows llvm-mingw
            # toolchain. On Linux, ordinary host Clang defaults to GNU ld
            # unless explicitly configured otherwise.
            "family": (
                "lld"
                if target.startswith("windows-") and compiler["family"] == "clang"
                else "gnu-ld"
            ),
            "version": "unknown",
            "driver_flags": (
                ["-fuse-ld=lld"]
                if target.startswith("windows-") and compiler["family"] == "clang"
                else []
            ),
        },
    }

    if toolchain_manifest_path:
        manifest_path = Path(toolchain_manifest_path).resolve()
        manifest = load_manifest(manifest_path)
        if manifest["target"] != target:
            fail(
                f"runtime toolchain manifest {manifest_path} identifies target "
                f"'{manifest['target']}', expected '{target}'"
            )
        source = manifest["toolchain"]
        runtime = source.get("runtime")
        if runtime is None:
            fail(
                f"runtime toolchain manifest {manifest_path} has no "
                "toolchain.runtime contract"
            )

        root = _toolchain_root_from_tool(cc, runtime["compiler"]["path"])
        expected_ar = (root / safe_relative_path(runtime["archiver"]["path"])).resolve()
        if Path(ar).resolve() != expected_ar:
            fail(
                f"archiver '{Path(ar).resolve()}' does not match runtime toolchain "
                f"manifest path '{expected_ar}'"
            )
        linker_path = (root / safe_relative_path(runtime["linker"]["path"])).resolve()
        if not linker_path.is_file():
            fail(f"runtime toolchain linker is missing: {linker_path}")
        linker_output = _tool_identity_output(
            [str(linker_path), "--version"], "linker version"
        )

        expected_checks = (
            ("compiler family", runtime["compiler"]["family"], compiler["family"]),
            ("compiler target", runtime["compiler"]["target"], compiler["target"]),
            ("archiver family", runtime["archiver"]["family"], archiver["family"]),
        )
        for label, expected, actual in expected_checks:
            if actual != expected:
                fail(
                    f"runtime toolchain {label} mismatch: manifest requires "
                    f"'{expected}', selected tool reports '{actual}'"
                )
        for label, expected, output in (
            ("compiler version", runtime["compiler"]["version"], cc_output),
            ("archiver version", runtime["archiver"]["version"], ar_output),
            ("linker version", runtime["linker"]["version"], linker_output),
        ):
            if expected not in output:
                fail(
                    f"runtime toolchain {label} mismatch: manifest requires "
                    f"'{expected}', probe output was '{_concise_tool_version(output)}'"
                )

        digest = source["archive"].get("digest")
        provenance.update(
            {
                "source_manifest": manifest_path.name,
                "vendor": source["vendor"],
                "version": source["version"],
                "archive_digest": digest,
                "abi": runtime["abi"],
                "crt": runtime["crt"],
                "linker": {
                    "command": str(linker_path),
                    "family": runtime["linker"]["family"],
                    "version": _concise_tool_version(linker_output),
                    "driver_flags": runtime["linker"]["driver_flags"],
                },
            }
        )

    expected_release = target_spec.get("release_toolchain")
    if toolchain_manifest_path and expected_release:
        checks = (
            ("manifest", expected_release["manifest"], provenance["source_manifest"]),
            ("vendor", expected_release["vendor"], provenance["vendor"]),
            ("version", expected_release["version"], provenance["version"]),
            ("ABI", expected_release["abi"], provenance["abi"]),
            ("CRT", expected_release["crt"], provenance["crt"]),
            (
                "compiler family",
                expected_release["compiler_family"],
                compiler["family"],
            ),
            (
                "compiler target",
                expected_release["compiler_target"],
                compiler["target"],
            ),
            (
                "archiver family",
                expected_release["archiver_family"],
                archiver["family"],
            ),
            (
                "linker family",
                expected_release["linker_family"],
                provenance["linker"]["family"],
            ),
        )
        for label, expected, actual in checks:
            if expected != actual:
                fail(
                    f"runtime archive contract {label} mismatch: expected "
                    f"'{expected}', got '{actual}'"
                )
        for label, expected, actual in (
            (
                "compiler version",
                expected_release["compiler_version"],
                compiler["version"],
            ),
            (
                "archiver version",
                expected_release["archiver_version"],
                archiver["version"],
            ),
            (
                "linker version",
                expected_release["linker_version"],
                provenance["linker"]["version"],
            ),
        ):
            if expected not in actual:
                fail(
                    f"runtime archive contract {label} mismatch: expected "
                    f"'{expected}' in '{actual}'"
                )
        if expected_release["compile_size_flag"] != compiler["size_flag"]:
            fail(
                "runtime archive contract size optimization mismatch: expected "
                f"'{expected_release['compile_size_flag']}', got "
                f"'{compiler['size_flag']}'"
            )
        if (
            expected_release["linker_driver_flags"]
            != provenance["linker"]["driver_flags"]
        ):
            fail("runtime archive contract linker driver flags do not match manifest")

    return provenance


def resolve_compiler_configuration(
    target: str,
    cc: str,
    target_triple: str | None,
    sysroot: str | None,
) -> tuple[list[str], str, str | None]:
    requested_triple = target_triple.strip() if target_triple else None
    if requested_triple and not _target_tag_matches_triple(target, requested_triple):
        fail(
            f"--target-triple '{requested_triple}' does not describe requested "
            f"archive target '{target}'"
        )

    sysroot_path: Path | None = None
    if sysroot:
        sysroot_path = Path(sysroot).resolve()
        if not sysroot_path.is_dir():
            fail(f"--sysroot directory does not exist: {sysroot_path}")

    default_triple = _probe_compiler_target(cc, [])
    try:
        host_target = detect_host_target()
    except SystemExit:
        host_target = None
    is_cross = target != host_target
    cc_name = Path(cc).name.lower()
    clang_stem = cc_name.removesuffix(".exe")
    bare_clang = clang_stem == "clang" or (
        clang_stem.startswith("clang-") and clang_stem[6:].isdigit()
    )

    if (
        is_cross
        and bare_clang
        and not _target_tag_matches_triple(target, default_triple)
    ):
        if not requested_triple:
            fail(
                f"bare clang targets '{default_triple}', not requested cross target "
                f"'{target}'. Use a target-specific compiler, or pass both "
                "--target-triple and --sysroot for the target toolchain"
            )
        if sysroot_path is None:
            fail(
                f"bare clang needs --sysroot when retargeting from '{default_triple}' "
                f"to cross target '{requested_triple}'; a target triple alone can "
                "silently use unsuitable host headers and libraries"
            )

    compiler_args: list[str] = []
    if requested_triple:
        if "clang" in cc_name:
            compiler_args.append(f"--target={requested_triple}")
        elif default_triple.lower() != requested_triple.lower():
            fail(
                f"compiler '{cc}' does not accept clang-style --target selection and "
                f"reports '{default_triple}', not requested triple '{requested_triple}'; "
                "use the matching triple-prefixed compiler"
            )
    if sysroot_path is not None:
        compiler_args.append(f"--sysroot={sysroot_path}")

    configured_triple = _probe_compiler_target(cc, compiler_args)
    if not _target_tag_matches_triple(target, configured_triple):
        cross_hint = (
            " Bare clang must be paired with --target-triple and --sysroot, or "
            "replaced by a target-specific compiler."
            if bare_clang
            else ""
        )
        fail(
            f"compiler '{cc}' reports target triple '{configured_triple}', which "
            f"does not match requested archive target '{target}'. Refusing to label "
            f"host objects as {target}.{cross_hint}"
        )

    return (
        compiler_args,
        configured_triple,
        str(sysroot_path) if sysroot_path is not None else None,
    )


def git_describe_version() -> str:
    try:
        result = subprocess.run(
            ["git", "-C", str(REPO_ROOT), "describe", "--tags", "--always", "--dirty"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            check=False,
        )
    except OSError:
        return "unknown"
    if result.returncode == 0 and result.stdout.strip():
        return result.stdout.strip()
    return "unknown"


def run_tool(command: list[str]) -> None:
    verbose = bool(os.environ.get("OSCAN_ARCHIVE_VERBOSE"))
    if verbose:
        print("+ " + " ".join(command), file=sys.stderr)
    try:
        result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, check=False)
    except FileNotFoundError:
        fail(
            f"'{command[0]}' was not found on PATH.\n"
            f"Pass an explicit --cc/--ar, set $OSCAN_ARCHIVE_CC/$OSCAN_ARCHIVE_AR, or "
            f"fetch a matching toolchain via scripts/fetch-toolchain.ps1|.sh."
        )
    if result.returncode != 0:
        fail(f"command failed ({command[0]}):\n{result.stdout}")


def hosted_compile_args(
    cc: str,
    compiler_args: list[str],
    src: Path,
    obj: Path,
    include_dirs: list[Path],
) -> list[str]:
    args = [
        cc,
        *compiler_args,
        "-std=c99",
        "-O2",
        "-w",
        "-ffunction-sections",
        "-fdata-sections",
    ]
    for inc in include_dirs:
        args.append(f"-I{inc}")
    args += ["-c", str(src), "-o", str(obj)]
    return args


def freestanding_compile_args(
    cc: str,
    compiler_args: list[str],
    target: str,
    src: Path,
    obj: Path,
    include_dirs: list[Path],
) -> list[str]:
    size_opt = "-Oz" if "clang" in cc.lower() else "-Os"
    args = [
        cc,
        *compiler_args,
    ]
    if target.startswith("linux-"):
        # laststanding deliberately redirects libc-style identifiers (memcpy,
        # realpath, ...) after its initial header block. Some glibc headers are
        # first reached later through l_img/stb and would then have their own
        # declarations macro-renamed into conflicting l_* declarations.
        # Pre-including the declaration-only headers establishes their guards
        # before those redirects; it does not link libc into the archive.
        args += ["-include", "stdlib.h", "-include", "string.h"]
    args += [
        "-std=gnu11",
        "-ffreestanding",
        "-w",
        size_opt,
        "-fno-builtin",
        "-fno-asynchronous-unwind-tables",
        "-fomit-frame-pointer",
        "-ffunction-sections",
        "-fdata-sections",
        # A switch's jump table can otherwise land in a shared, non-
        # function-scoped section that keeps unrelated dead code (and its
        # platform imports, e.g. unused Win32 DLL calls) alive even under
        # --gc-sections; see src/backend/link.rs's module docs ("Windows
        # import-library minimization") for the full explanation. Must
        # match src/backend/link.rs's compile_shim_object flags.
        "-fno-jump-tables",
    ]
    if "clang" in cc.lower():
        args.append("-Wno-error=implicit-function-declaration")
    for inc in include_dirs:
        args.append(f"-I{inc}")
    args += ["-c", str(src), "-o", str(obj)]
    return args


def extract_archive_members(ar: str, archive: Path, dest_dir: Path) -> list[Path]:
    ensure_clean_dir(dest_dir)
    try:
        result = subprocess.run(
            [ar, "x", str(archive.resolve())],
            cwd=dest_dir,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            check=False,
        )
    except FileNotFoundError:
        fail(
            f"'{ar}' was not found on PATH; cannot extract {archive}.\n"
            f"Pass an explicit --ar, set $OSCAN_ARCHIVE_AR, or fetch a matching "
            f"toolchain via scripts/fetch-toolchain.ps1|.sh."
        )
    if result.returncode != 0:
        fail(f"failed to extract {archive} with {ar}:\n{result.stdout}")
    return sorted(dest_dir.glob("*.o"))


def publish_archive_pair(
    staged_archive: Path,
    staged_manifest: Path,
    archive_path: Path,
    manifest_path: Path,
) -> None:
    """Publish a matching manifest/archive pair without exposing a partial archive.

    The final archive is renamed last: until that atomic rename succeeds, the
    destination contains no usable-looking archive. Existing pairs are moved
    aside first and restored if any publication operation fails.
    """

    archive_path.parent.mkdir(parents=True, exist_ok=True)
    backup_archive = archive_path.parent / f".{archive_path.name}.previous"
    backup_manifest = manifest_path.parent / f".{manifest_path.name}.previous"
    for backup in (backup_archive, backup_manifest):
        remove_path(backup)

    archive_backed_up = False
    manifest_backed_up = False
    try:
        if archive_path.exists():
            os.replace(archive_path, backup_archive)
            archive_backed_up = True
        if manifest_path.exists():
            os.replace(manifest_path, backup_manifest)
            manifest_backed_up = True

        # A manifest without its archive is intentionally harmless. Publishing
        # the archive last is the single transition that makes the new pair
        # visible to consumers.
        os.replace(staged_manifest, manifest_path)
        os.replace(staged_archive, archive_path)
    except OSError as exc:
        rollback_errors: list[str] = []
        for final_path in (archive_path, manifest_path):
            try:
                remove_path(final_path)
            except OSError as rollback_exc:
                rollback_errors.append(f"remove {final_path}: {rollback_exc}")
        for was_backed_up, backup, final_path in (
            (manifest_backed_up, backup_manifest, manifest_path),
            (archive_backed_up, backup_archive, archive_path),
        ):
            if not was_backed_up:
                continue
            try:
                os.replace(backup, final_path)
            except OSError as rollback_exc:
                rollback_errors.append(f"restore {final_path}: {rollback_exc}")
        detail = (
            f"; rollback also failed: {'; '.join(rollback_errors)}"
            if rollback_errors
            else ""
        )
        fail(f"failed to publish runtime archive pair for {archive_path.name}: {exc}{detail}")
    else:
        remove_path(backup_archive)
        remove_path(backup_manifest)


def build_runtime_archive(args: argparse.Namespace) -> int:
    contract_path = Path(args.contract).resolve()
    contract = load_runtime_archive_contract(contract_path)

    target = args.target or detect_host_target()
    modes = list(contract["modes"].keys()) if args.mode == "all" else [args.mode]

    cc = args.cc or default_cc_for_target(target)
    ar = args.ar or default_ar_for(cc)
    # Canonicalize before anything gets recorded in the manifest: cc/ar may
    # still be bare PATH-discovered names or relative paths at this point
    # (see _canonicalize_tool_path), and only their resolved absolute form
    # is safe, unambiguous provenance to write down.
    cc = _canonicalize_tool_path(cc)
    ar = _canonicalize_tool_path(ar)
    compiler_args, cc_target, configured_sysroot = resolve_compiler_configuration(
        target,
        cc,
        getattr(args, "target_triple", None),
        getattr(args, "sysroot", None),
    )
    target_contract = contract.get("targets", {}).get(target, {})
    toolchain_provenance = _runtime_toolchain_provenance(
        target=target,
        cc=cc,
        ar=ar,
        cc_target=cc_target,
        target_spec=target_contract,
        toolchain_manifest_path=getattr(args, "toolchain_manifest", None),
    )

    out_root = Path(args.out_dir).resolve() if args.out_dir else (
        REPO_ROOT / safe_relative_path(contract["output_root_template"].format(target=target))
    )
    out_root.mkdir(parents=True, exist_ok=True)

    runtime_dir = REPO_ROOT / "runtime"
    deps_dir = REPO_ROOT / "deps" / "laststanding"
    include_dirs = [runtime_dir, deps_dir]

    archive_paths: list[str] = []

    for mode in modes:
        mode_spec = contract["modes"].get(mode)
        if mode_spec is None:
            fail(f"unknown runtime archive mode '{mode}' (available: {', '.join(contract['modes'])})")

        supported_targets = mode_spec.get("supported_targets")
        if supported_targets is not None and target not in supported_targets:
            print(
                f"note: skipping '{mode}' runtime archive for target '{target}' "
                f"(supported targets: {', '.join(supported_targets)})",
                file=sys.stderr,
            )
            continue

        target_spec = target_contract.get(mode, {})

        work_dir = out_root / f"_obj-{mode}"
        ensure_clean_dir(work_dir)

        object_paths: list[Path] = []
        for src_name in mode_spec["sources"]:
            src_path = runtime_dir / src_name
            if not src_path.is_file():
                fail(f"runtime source not found: {src_path}")
            obj_path = work_dir / (Path(src_name).stem + ".o")
            if mode == "hosted":
                compile_args = hosted_compile_args(
                    cc, compiler_args, src_path, obj_path, include_dirs
                )
            else:
                compile_args = freestanding_compile_args(
                    cc, compiler_args, target, src_path, obj_path, include_dirs
                )
            run_tool(compile_args)
            object_paths.append(obj_path)

        embedded_bearssl = False
        embed_from = target_spec.get("embed_bearssl_from")
        if embed_from:
            bearssl_path = REPO_ROOT / safe_relative_path(embed_from)
            if bearssl_path.is_file():
                object_paths.extend(
                    extract_archive_members(ar, bearssl_path, work_dir / "bearssl-objs")
                )
                embedded_bearssl = True
            else:
                print(
                    f"note: {bearssl_path} not found; the freestanding archive will not embed TLS "
                    f"objects (link {embed_from} manually, or run the 'Build BearSSL' workflow first)",
                    file=sys.stderr,
                )

        archive_name = mode_spec["archive_name"]
        archive_path = out_root / archive_name
        staged_archive = work_dir / archive_name
        remove_path(staged_archive)
        run_tool([ar, "rcs", str(staged_archive)] + [str(p) for p in object_paths])

        # Native shim (§3.2 of docs/design/native-link-embedding.md): derived
        # from "sources" rather than trusted blindly, so a manifest never
        # claims contains_native_shim when the shim wasn't actually one of
        # the compiled translation units. The ar member name is the source
        # stem + ".o", matching the compile loop above exactly.
        native_shim_source = "osc_native_shim.c"
        contains_native_shim = native_shim_source in mode_spec["sources"]
        native_shim_member = (
            Path(native_shim_source).stem + ".o" if contains_native_shim else None
        )

        manifest = {
            "schema_version": 2,
            "target": target,
            "mode": mode,
            "requires_libc": mode_spec["requires_libc"],
            "sources": mode_spec["sources"],
            "contains_native_shim": contains_native_shim,
            "native_shim_member": native_shim_member,
            "cc": cc,
            "cc_args": compiler_args,
            "cc_target": cc_target,
            "sysroot": configured_sysroot,
            "ar": ar,
            "compile_optimization": (
                "-O2" if mode == "hosted" else toolchain_provenance["compiler"]["size_flag"]
            ),
            "toolchain": toolchain_provenance,
            "link_flags": target_spec.get("link_flags", []),
            "embedded_bearssl": embedded_bearssl,
            "oscan_version": git_describe_version(),
            "sha256": compute_digest(staged_archive, "sha256"),
        }
        manifest_path = out_root / mode_spec["manifest_name"]
        staged_manifest = work_dir / mode_spec["manifest_name"]
        staged_manifest.write_text(
            json.dumps(manifest, indent=2) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        publish_archive_pair(
            staged_archive,
            staged_manifest,
            archive_path,
            manifest_path,
        )

        if not args.keep_objects:
            remove_path(work_dir)

        archive_paths.append(str(archive_path))
        print(str(archive_path))

    if not archive_paths:
        fail(f"no runtime archives were built for target '{target}' (mode '{args.mode}')")
    return 0


# ---------------------------------------------------------------------------
# Embedded native-link asset staging (docs/design/native-link-embedding.md §5.4)
#
# Copies exactly the ~85.4 MB minimal linker/linker-runtime/import-lib/
# compiler-builtins set out of an already-fetched, pinned toolchain directory
# into packaging/prebuilt/<target>/, and writes the native-link-assets.json
# manifest that build.rs (OSCAN_EMBED_ASSETS_DIR) reads at compiler build
# time. Field names in the emitted manifest are a strict ABI shared with
# Bishop's Rust reader (native_assets.rs) — see design §4.2/§8.3; do not
# rename without updating both sides.
# ---------------------------------------------------------------------------

# Windows x86-64 asset set (design §4.1): the linker plus the six optional
# Win32 import libraries LLD must see while resolving undefined imports in
# dead runtime-archive sections, plus compiler-builtins for the intrinsics
# clang's freestanding codegen may emit. Only these files are ever copied —
# never a whole directory.
_WINDOWS_X86_64_EMBED_LINKER = {
    "role": "linker",
    "name": "ld.lld.exe",
    "source": "bin/ld.lld.exe",
    "install_subpath": "bin/ld.lld.exe",
    "flavor": "mingw",
    "emulation": "i386pep",
}

# ld.lld.exe is NOT statically linked: it needs these 5 sibling DLLs present
# in the exact same install directory (Windows resolves a loaded EXE's DLL
# imports by searching the directory containing the EXE first — that's the
# whole fix, no PATH manipulation). Confirmed by a real manual link of
# hello.osc reproducing the exact working executable with this set.
# libclang-cpp.dll is deliberately NOT included: it's only needed by
# clang.exe/clang++.exe, not by ld.lld.exe, and is not part of this embed set.
_WINDOWS_X86_64_EMBED_LINKER_RUNTIME = [
    {
        "name": "libLLVM-22.dll",
        "source": "bin/libLLVM-22.dll",
        "install_subpath": "bin/libLLVM-22.dll",
    },
    {
        "name": "libwinpthread-1.dll",
        "source": "bin/libwinpthread-1.dll",
        "install_subpath": "bin/libwinpthread-1.dll",
    },
    {
        "name": "libunwind.dll",
        "source": "bin/libunwind.dll",
        "install_subpath": "bin/libunwind.dll",
    },
    {
        "name": "libffi-8.dll",
        "source": "bin/libffi-8.dll",
        "install_subpath": "bin/libffi-8.dll",
    },
    {
        "name": "libc++.dll",
        "source": "bin/libc++.dll",
        "install_subpath": "bin/libc++.dll",
    },
]

_WINDOWS_X86_64_EMBED_IMPORT_LIBS = [
    {
        "lib": "kernel32",
        "name": "libkernel32.a",
        "source": "x86_64-w64-mingw32/lib/libkernel32.a",
        "install_subpath": "lib/libkernel32.a",
    },
    {
        "lib": "ws2_32",
        "name": "libws2_32.a",
        "source": "x86_64-w64-mingw32/lib/libws2_32.a",
        "install_subpath": "lib/libws2_32.a",
    },
    {
        "lib": "user32",
        "name": "libuser32.a",
        "source": "x86_64-w64-mingw32/lib/libuser32.a",
        "install_subpath": "lib/libuser32.a",
    },
    {
        "lib": "gdi32",
        "name": "libgdi32.a",
        "source": "x86_64-w64-mingw32/lib/libgdi32.a",
        "install_subpath": "lib/libgdi32.a",
    },
    {
        "lib": "secur32",
        "name": "libsecur32.a",
        "source": "x86_64-w64-mingw32/lib/libsecur32.a",
        "install_subpath": "lib/libsecur32.a",
    },
    {
        "lib": "crypt32",
        "name": "libcrypt32.a",
        "source": "x86_64-w64-mingw32/lib/libcrypt32.a",
        "install_subpath": "lib/libcrypt32.a",
    },
]

# NOT lib/clang/*/lib/linux/... (wrong target). The clang resource-dir
# version component (e.g. "22") tracks the pinned toolchain's clang major
# version, so it is resolved with a glob against the toolchain dir rather
# than hardcoded, and the tool fails loudly if that resolves to anything
# other than exactly one file.
_WINDOWS_X86_64_EMBED_BUILTINS = {
    "role": "compiler_builtins",
    "name": "libclang_rt.builtins-x86_64.a",
    "source_glob": "lib/clang/*/lib/windows/libclang_rt.builtins-x86_64.a",
    "install_subpath": "lib/clang/libclang_rt.builtins-x86_64.a",
}

EMBED_ASSET_SPECS = {
    "windows-x86_64": {
        "linker": _WINDOWS_X86_64_EMBED_LINKER,
        "linker_runtime": _WINDOWS_X86_64_EMBED_LINKER_RUNTIME,
        "import_libs": _WINDOWS_X86_64_EMBED_IMPORT_LIBS,
        "compiler_builtins": _WINDOWS_X86_64_EMBED_BUILTINS,
    },
    "linux-x86_64": {
        "linker": {
            "role": "linker",
            "name": "x86_64-linux-musl-ld",
            "source": "bin/x86_64-linux-musl-ld",
            "install_subpath": "linker/x86_64-linux-musl-ld",
            "flavor": "elf",
            "emulation": "elf_x86_64",
        },
        "linker_runtime": [],
        "import_libs": [],
    },
    "linux-aarch64": {
        "linker": {
            "role": "linker",
            "name": "aarch64-linux-musl-ld",
            "source": "bin/aarch64-linux-musl-ld",
            "install_subpath": "linker/aarch64-linux-musl-ld",
            "flavor": "elf",
            "emulation": "aarch64linux",
        },
        "linker_runtime": [],
        "import_libs": [],
    },
    "linux-riscv64": {
        "linker": {
            "role": "linker",
            "name": "riscv64-linux-musl-ld",
            "source": "bin/riscv64-linux-musl-ld",
            "install_subpath": "linker/riscv64-linux-musl-ld",
            "flavor": "elf",
            "emulation": "elf64lriscv",
        },
        "linker_runtime": [],
        "import_libs": [],
    },
}


def _resolve_embed_asset_source(toolchain_dir: Path, spec: dict) -> Path:
    if "source" in spec:
        path = toolchain_dir / safe_relative_path(spec["source"])
        if not path.is_file():
            fail(
                f"embedded asset source not found in toolchain dir {toolchain_dir}: "
                f"{spec['source']} (needed for '{spec['name']}')"
            )
        return path

    glob_pattern = spec["source_glob"]
    matches = sorted(p for p in toolchain_dir.glob(glob_pattern) if p.is_file())
    if not matches:
        fail(
            f"no file under toolchain dir {toolchain_dir} matches '{glob_pattern}' "
            f"(needed for '{spec['name']}')"
        )
    if len(matches) > 1:
        fail(
            f"ambiguous embedded asset source: multiple files under {toolchain_dir} "
            f"match '{glob_pattern}': {', '.join(str(m) for m in matches)}"
        )
    return matches[0]


def _stage_embed_asset(toolchain_dir: Path, output_dir: Path, spec: dict) -> dict:
    source_path = _resolve_embed_asset_source(toolchain_dir, spec)
    install_subpath = safe_relative_path(spec["install_subpath"])
    dest_path = output_dir / install_subpath
    dest_path.parent.mkdir(parents=True, exist_ok=True)
    remove_path(dest_path)
    shutil.copy2(source_path, dest_path)
    return {
        "path": dest_path,
        "size": dest_path.stat().st_size,
        "sha256": compute_digest(dest_path, "sha256"),
    }


def prepare_embed_assets(
    target: str,
    toolchain_dir: Path,
    toolchain_manifest_path: Path,
    output_dir: Path,
) -> dict:
    """Stage the embedded native-link asset set + native-link-assets.json.

    No network access: the toolchain must already be fetched (fetch_toolchain)
    to `toolchain_dir`. Returns the manifest dict that was written.
    """
    asset_spec = EMBED_ASSET_SPECS.get(target)
    if asset_spec is None:
        fail(
            f"prepare-embed-assets does not know the embedded native-link asset "
            f"set for target '{target}' (supported: "
            f"{', '.join(sorted(EMBED_ASSET_SPECS))}); see "
            "docs/design/native-link-embedding.md §1.1 for current scope"
        )
    if not toolchain_dir.is_dir():
        fail(
            f"toolchain directory not found: {toolchain_dir}\n"
            f"run fetch-toolchain first, e.g.:\n"
            f"  scripts/fetch-toolchain.ps1|.sh --manifest {toolchain_manifest_path} "
            f"--destination {toolchain_dir}"
        )
    if not toolchain_manifest_path.is_file():
        fail(f"toolchain manifest not found: {toolchain_manifest_path}")

    manifest = load_manifest(toolchain_manifest_path)
    if manifest["target"] != target:
        fail(
            f"toolchain manifest {toolchain_manifest_path} identifies target "
            f"'{manifest['target']}', expected '{target}'"
        )
    toolchain_info = manifest["toolchain"]
    archive_digest = toolchain_info.get("archive", {}).get("digest")
    if not isinstance(archive_digest, dict):
        fail(
            f"toolchain manifest {toolchain_manifest_path} has no "
            "toolchain.archive.digest"
        )

    output_dir.mkdir(parents=True, exist_ok=True)

    linker_staged = _stage_embed_asset(toolchain_dir, output_dir, asset_spec["linker"])
    linker_entry = {
        "role": "linker",
        "name": asset_spec["linker"]["name"],
        "install_subpath": asset_spec["linker"]["install_subpath"],
        "flavor": asset_spec["linker"]["flavor"],
        "emulation": asset_spec["linker"]["emulation"],
        "size": linker_staged["size"],
        "sha256": linker_staged["sha256"],
    }

    assets_entries = []
    for runtime_spec in asset_spec["linker_runtime"]:
        staged = _stage_embed_asset(toolchain_dir, output_dir, runtime_spec)
        assets_entries.append(
            {
                "role": "linker_runtime",
                "name": runtime_spec["name"],
                "install_subpath": runtime_spec["install_subpath"],
                "size": staged["size"],
                "sha256": staged["sha256"],
            }
        )

    for lib_spec in asset_spec["import_libs"]:
        staged = _stage_embed_asset(toolchain_dir, output_dir, lib_spec)
        assets_entries.append(
            {
                "role": "import_lib",
                "name": lib_spec["name"],
                "lib": lib_spec["lib"],
                "install_subpath": lib_spec["install_subpath"],
                "size": staged["size"],
                "sha256": staged["sha256"],
            }
        )

    # compiler_builtins is optional — Linux freestanding has none (the musl
    # toolchain supplies what intrinsics are needed via static linking), while
    # Windows needs explicit clang_rt.builtins-x86_64.a.
    builtins_spec = asset_spec.get("compiler_builtins")
    if builtins_spec is not None:
        builtins_staged = _stage_embed_asset(toolchain_dir, output_dir, builtins_spec)
        assets_entries.append(
            {
                "role": "compiler_builtins",
                "name": builtins_spec["name"],
                "install_subpath": builtins_spec["install_subpath"],
                "size": builtins_staged["size"],
                "sha256": builtins_staged["sha256"],
            }
        )

    out_manifest = {
        "schema_version": 1,
        "target": target,
        "toolchain": {
            "vendor": toolchain_info["vendor"],
            "version": toolchain_info["version"],
            "archive_digest": {
                "algorithm": archive_digest["algorithm"],
                "value": archive_digest["value"],
            },
        },
        "linker": linker_entry,
        "assets": assets_entries,
    }

    manifest_path = output_dir / "native-link-assets.json"
    manifest_path.write_text(
        json.dumps(out_manifest, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return out_manifest


def prepare_embed_assets_command(args: argparse.Namespace) -> int:
    target = args.target
    toolchain_dir = Path(args.toolchain_dir).resolve()
    toolchain_manifest_path = (
        Path(args.toolchain_manifest).resolve()
        if args.toolchain_manifest
        else REPO_ROOT / "packaging" / "toolchains" / f"{target}.json"
    )
    output_dir = (
        Path(args.output_dir).resolve()
        if args.output_dir
        else REPO_ROOT / "packaging" / "prebuilt" / target
    )
    prepare_embed_assets(target, toolchain_dir, toolchain_manifest_path, output_dir)
    print(str(output_dir / "native-link-assets.json"))
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Oscan release asset helpers")
    subparsers = parser.add_subparsers(dest="command", required=True)

    fetch = subparsers.add_parser("fetch-toolchain")
    fetch.add_argument("--manifest", required=True)
    fetch.add_argument("--download-dir", required=True)
    fetch.add_argument("--destination", required=True)
    fetch.set_defaults(func=fetch_toolchain_command)

    stage = subparsers.add_parser("stage-release")
    stage.add_argument("--target", required=True)
    stage.add_argument("--contract", default=str(CONTRACT_PATH))
    stage.add_argument("--version", required=True)
    stage.add_argument("--binary", required=True)
    stage.add_argument("--output-dir", required=True)
    stage.add_argument(
        "--runtime-archive-dir",
        default=None,
        help="directory containing the target's prebuilt runtime archive/manifest pairs",
    )
    stage.add_argument(
        "--cross-linker-sidecar-dir",
        default=None,
        help="directory of per-target cross-linker sidecar subdirs (e.g. build/cross-linker-sidecars) to bundle as cross-linkers/<target>/ in the release archive",
    )
    stage.set_defaults(func=stage_release)

    checksums = subparsers.add_parser("write-checksums")
    checksums.add_argument("--output", required=True)
    checksums.add_argument("files", nargs="+")
    checksums.set_defaults(func=write_checksums)

    detect_target = subparsers.add_parser("detect-host-target")
    detect_target.set_defaults(func=detect_host_target_command)

    runtime_archive = subparsers.add_parser("build-runtime-archive")
    runtime_archive.add_argument("--target", default=None, help="e.g. linux-x86_64, windows-x86_64; defaults to the host platform")
    runtime_archive.add_argument("--mode", choices=["hosted", "freestanding", "freestanding_core", "all"], default="all")
    runtime_archive.add_argument("--cc", default=None, help="C compiler to use (defaults to $OSCAN_ARCHIVE_CC, else an auto-detected host/cross compiler on PATH for --target)")
    runtime_archive.add_argument("--ar", default=None, help="archiver to use (defaults to $OSCAN_ARCHIVE_AR, else one auto-detected from --cc)")
    runtime_archive.add_argument(
        "--target-triple",
        default=None,
        help="clang target triple for an explicitly configured cross compiler",
    )
    runtime_archive.add_argument(
        "--sysroot",
        default=None,
        help="target sysroot used with --target-triple for bare-clang cross builds",
    )
    runtime_archive.add_argument(
        "--toolchain-manifest",
        default=None,
        help=(
            "pinned release toolchain manifest used to validate and record exact "
            "compiler/archiver/linker provenance"
        ),
    )
    runtime_archive.add_argument("--out-dir", default=None, help="output directory (defaults to build/runtime-archives/<target>)")
    runtime_archive.add_argument("--contract", default=str(RUNTIME_ARCHIVE_CONTRACT_PATH))
    runtime_archive.add_argument("--keep-objects", action="store_true", help="keep intermediate .o files for inspection")
    runtime_archive.set_defaults(func=build_runtime_archive)

    prepare_embed = subparsers.add_parser(
        "prepare-embed-assets",
        help=(
            "stage the embedded native-link asset set (linker/linker-runtime "
            "DLLs/import libs/compiler-builtins) + native-link-assets.json for "
            "OSCAN_EMBED_ASSETS_DIR"
        ),
    )
    prepare_embed.add_argument("--target", required=True, help="e.g. windows-x86_64")
    prepare_embed.add_argument(
        "--toolchain-dir",
        required=True,
        help="already-fetched pinned toolchain directory (see fetch-toolchain)",
    )
    prepare_embed.add_argument(
        "--toolchain-manifest",
        default=None,
        help="toolchain manifest (defaults to packaging/toolchains/<target>.json)",
    )
    prepare_embed.add_argument(
        "--output-dir",
        default=None,
        help="staging output directory (defaults to packaging/prebuilt/<target>)",
    )
    prepare_embed.set_defaults(func=prepare_embed_assets_command)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
