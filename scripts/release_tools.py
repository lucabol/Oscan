#!/usr/bin/env python3
from __future__ import annotations

import argparse
import contextlib
import gzip
import hashlib
import json
import os
import platform
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import textwrap
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile
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

# Canonical backend names. `native` is only a deprecated CLI alias for
# `cranelift` and never appears in an artifact name, package label, or
# contract entry.
CANONICAL_BACKENDS = ("llvm", "cranelift", "c")
REQUIRED_COMPONENTS = (
    "compiler",
    "direct_link_sidecar",
    "runtime_archives",
    "llvm_provider",
    "c_toolchain",
)
# Object packages ship freestanding runtime archives only: a hosted
# archive would need the host CRT, which is exactly the C-toolchain
# dependency these packages exist to remove.
FREESTANDING_PROFILES = ("freestanding", "freestanding_gfx", "freestanding_core")
# The published matrix. macOS is C-only: there is no Darwin object target
# for either object backend.
SUPPORTED_RELEASE_TARGETS = {
    "windows-x86_64": ("llvm", "cranelift", "c"),
    "linux-x86_64": ("llvm", "cranelift", "c"),
    "macos-x86_64": ("c",),
}
PACKAGE_METADATA_NAME = "oscan-package.json"
PROVIDER_PROVENANCE_NAME = "llvm-provider-provenance.json"
NATIVE_LINK_DIR_NAME = "native-link"
NATIVE_LINK_MANIFEST_NAME = "native-link-assets.json"
INPROCESS_LINK_MANIFEST_NAME = "inprocess-link-assets.json"


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
    """Load and fully validate the release contract (schema 2).

    Schema 2 replaces schema 1's single "full bundle per target" model with
    an explicit target x backend variant matrix: every artifact this
    repository publishes is one (target, backend) pair with its own archive
    names, Cargo feature, distribution stamp, capability flag, and component
    list. There is no `-full` bundle any more, and `native` is never an
    artifact label (it survives only as a deprecated CLI alias).
    """
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"cannot read release contract {path}: {exc}")
    if data.get("schema_version") != 2:
        fail(
            f"unsupported release contract schema in {path}: expected schema_version 2 "
            f"(target x backend variants), got {data.get('schema_version')!r}"
        )
    for key in (
        "install_surface",
        "toolchains_committed_to_git",
        "backends",
        "components",
        "lookup_contract",
        "variants",
    ):
        if key not in data:
            fail(f"release contract {path} is missing '{key}'")
    if data["install_surface"] != "github-releases":
        fail(f"unsupported release install surface '{data['install_surface']}'")
    if data["toolchains_committed_to_git"]:
        fail("the release contract must keep toolchains out of git")

    _validate_contract_backends(path, data["backends"])
    _validate_contract_components(path, data["components"])

    lookup_contract = data["lookup_contract"]
    for platform in ("windows", "linux"):
        if platform not in lookup_contract:
            fail(f"release contract {path} is missing lookup_contract.{platform}")
        for key in ("search_roots", "bin_directories", "compiler_names"):
            if key not in lookup_contract[platform]:
                fail(f"release contract {path} is missing lookup_contract.{platform}.{key}")

    _validate_contract_variants(path, data)
    return data


def _validate_contract_backends(path: Path, backends: dict) -> None:
    if sorted(backends) != sorted(CANONICAL_BACKENDS):
        fail(
            f"release contract {path} must declare exactly the canonical backends "
            f"{', '.join(CANONICAL_BACKENDS)}, got {', '.join(sorted(backends))}"
        )
    for name, spec in backends.items():
        if not isinstance(spec, dict):
            fail(f"release contract backend '{name}' must be an object")
        for key in ("cargo_feature", "artifact_suffix", "kind"):
            if key not in spec:
                fail(f"release contract backend '{name}' is missing '{key}'")
        if spec["cargo_feature"] != f"backend-{name}":
            fail(
                f"release contract backend '{name}' must use cargo feature 'backend-{name}', "
                f"got '{spec['cargo_feature']}'"
            )
        if spec["artifact_suffix"] != name:
            fail(
                f"release contract backend '{name}' must use artifact suffix '{name}', got "
                f"'{spec['artifact_suffix']}'"
            )
        if spec["kind"] not in ("object", "c-source"):
            fail(f"release contract backend '{name}' has unknown kind '{spec['kind']}'")


def _validate_contract_components(path: Path, components: dict) -> None:
    missing = [name for name in REQUIRED_COMPONENTS if name not in components]
    if missing:
        fail(f"release contract {path} is missing component definition(s): {', '.join(missing)}")
    if components["direct_link_sidecar"].get("position") != "archive-root/native-link":
        fail("direct-link sidecar assets must be staged at archive-root/native-link")
    if (
        components["runtime_archives"].get("position_template")
        != "archive-root/build/runtime-archives/{target}"
    ):
        fail(
            "runtime archives must be staged at archive-root/build/runtime-archives/{target}"
        )
    allowed_profiles = components["runtime_archives"].get("allowed_profiles")
    if allowed_profiles != list(FREESTANDING_PROFILES):
        fail(
            "release contract runtime_archives.allowed_profiles must be "
            f"{list(FREESTANDING_PROFILES)} (freestanding only: object packages ship no hosted "
            "archive)"
        )
    for name in ("llvm_provider", "c_toolchain"):
        if components[name].get("position") != "archive-root/toolchain":
            fail(f"component '{name}' must be staged at archive-root/toolchain")


def _validate_contract_variants(path: Path, contract: dict) -> None:
    variants = contract["variants"]
    if not variants:
        fail(f"release contract {path} declares no variants")
    seen_archive_names: dict[str, str] = {}
    seen_roots: dict[str, str] = {}
    for target, target_spec in variants.items():
        if target not in SUPPORTED_RELEASE_TARGETS:
            fail(f"release contract declares unsupported target '{target}'")
        for key in ("binary_name", "archive_format", "backends"):
            if key not in target_spec:
                fail(f"release contract target '{target}' is missing '{key}'")
        if target_spec["archive_format"] not in ARCHIVE_SUFFIXES:
            fail(
                f"unsupported archive format '{target_spec['archive_format']}' for target "
                f"'{target}'"
            )
        backends = target_spec["backends"]
        if not backends:
            fail(f"release contract target '{target}' declares no backends")
        expected_backends = SUPPORTED_RELEASE_TARGETS[target]
        if sorted(backends) != sorted(expected_backends):
            fail(
                f"release contract target '{target}' must declare backends "
                f"{', '.join(expected_backends)}, got {', '.join(sorted(backends))}"
            )
        for backend, variant in backends.items():
            _validate_contract_variant(path, contract, target, backend, variant)
            name = variant["archive_name_template"]
            root = variant["archive_root_template"]
            if name in seen_archive_names:
                fail(
                    f"release contract archive name '{name}' is used by both "
                    f"{seen_archive_names[name]} and {target}/{backend}"
                )
            if root in seen_roots:
                fail(
                    f"release contract archive root '{root}' is used by both "
                    f"{seen_roots[root]} and {target}/{backend}"
                )
            seen_archive_names[name] = f"{target}/{backend}"
            seen_roots[root] = f"{target}/{backend}"


def _validate_contract_variant(
    path: Path, contract: dict, target: str, backend: str, variant: dict
) -> None:
    where = f"release contract variant {target}/{backend}"
    if backend not in contract["backends"]:
        fail(f"{where} names a backend the contract does not declare")
    for key in (
        "archive_name_template",
        "archive_root_template",
        "cargo_feature",
        "distribution_backend",
        "toolchain_free",
        "components",
        "runtime_profiles",
    ):
        if key not in variant:
            fail(f"{where} is missing '{key}'")

    backend_spec = contract["backends"][backend]
    if variant["cargo_feature"] != backend_spec["cargo_feature"]:
        fail(
            f"{where} declares cargo feature '{variant['cargo_feature']}', but backend "
            f"'{backend}' is built with '{backend_spec['cargo_feature']}'"
        )
    if variant["distribution_backend"] != backend:
        fail(
            f"{where} declares distribution backend '{variant['distribution_backend']}', which "
            f"does not match the variant's own backend '{backend}'"
        )

    suffix = ARCHIVE_SUFFIXES[contract["variants"][target]["archive_format"]]
    for field in ("archive_name_template", "archive_root_template"):
        template = variant[field]
        if "{version}" not in template:
            fail(f"{where} {field} must include '{{version}}'")
        if "native" in template:
            fail(
                f"{where} {field} uses 'native', which is only a deprecated CLI alias; the "
                "canonical artifact label is 'cranelift'"
            )
    if not variant["archive_name_template"].endswith(f"-{backend}{suffix}"):
        fail(f"{where} archive_name_template must end with '-{backend}{suffix}'")
    if not variant["archive_root_template"].endswith(f"-{backend}"):
        fail(f"{where} archive_root_template must end with '-{backend}'")

    components = variant["components"]
    unknown = [name for name in components if name not in contract["components"]]
    if unknown:
        fail(f"{where} lists undefined component(s): {', '.join(unknown)}")
    if "compiler" not in components:
        fail(f"{where} must include the 'compiler' component")

    has_c_toolchain = "c_toolchain" in components
    if variant["toolchain_free"] and has_c_toolchain:
        fail(f"{where} claims toolchain_free but ships the c_toolchain component")
    if backend == "c" and variant["toolchain_free"]:
        fail(f"{where} is a C-backend package and can never be toolchain-free")

    profiles = variant["runtime_profiles"]
    invalid = [profile for profile in profiles if profile not in FREESTANDING_PROFILES]
    if invalid:
        fail(f"{where} lists non-freestanding runtime profile(s): {', '.join(invalid)}")

    if contract["backends"][backend]["kind"] == "object":
        if not variant["toolchain_free"]:
            fail(f"{where} is an object package and must be toolchain-free")
        for required in ("direct_link_sidecar", "runtime_archives"):
            if required not in components:
                fail(f"{where} is an object package and must include the '{required}' component")
        if not profiles:
            fail(f"{where} must declare at least one freestanding runtime profile")
        if backend == "llvm":
            if "llvm_provider" not in components:
                fail(f"{where} must include the 'llvm_provider' component")
            source = variant.get("llvm_provider_source")
            if source not in contract["components"]["llvm_provider"]["sources"]:
                fail(f"{where} declares unknown llvm_provider_source {source!r}")
            if source == "direct-link-sidecar" and not variant.get("llvm_provider_asset"):
                fail(
                    f"{where} shares its provider with the direct-link sidecar, so it must name "
                    "the sidecar asset in 'llvm_provider_asset'"
                )
        elif "llvm_provider" in components:
            fail(f"{where} is a Cranelift package and must not ship an LLVM provider")
    else:
        for forbidden in ("direct_link_sidecar", "runtime_archives", "llvm_provider"):
            if forbidden in components:
                fail(
                    f"{where} is a C package and must not ship the '{forbidden}' component"
                )
        if profiles:
            fail(f"{where} is a C package and must not ship native runtime archives")

    if target.startswith("macos") and has_c_toolchain:
        fail(f"{where} must not bundle a C toolchain: macOS relies on the Apple CLT")


def render_release_template(template: str, version: str, field_name: str) -> str:
    try:
        return template.format(version=version)
    except KeyError as exc:
        fail(f"release template '{field_name}' is missing placeholder data: {exc}")


def resolve_release_variant(
    contract: dict, contract_path: Path, target: str, backend: str
) -> dict:
    """The fully resolved (target, backend) variant spec, or a hard error."""
    variants = contract["variants"]
    if target not in variants:
        fail(
            f"release contract does not define target '{target}' "
            f"(known: {', '.join(sorted(variants))})"
        )
    target_spec = variants[target]
    if backend not in target_spec["backends"]:
        fail(
            f"release contract does not define backend '{backend}' for target '{target}' "
            f"(known: {', '.join(sorted(target_spec['backends']))})"
        )

    spec = dict(target_spec["backends"][backend])
    spec["target"] = target
    spec["backend"] = backend
    spec["binary_name"] = target_spec["binary_name"]
    spec["archive_format"] = target_spec["archive_format"]
    spec["platform"] = target.split("-", 1)[0]
    spec["backend_kind"] = contract["backends"][backend]["kind"]

    manifest_name = target_spec.get("toolchain_manifest")
    if manifest_name:
        manifest_path = contract_path.parent / manifest_name
        if not manifest_path.is_file():
            fail(f"toolchain manifest not found for target '{target}': {manifest_path}")
        spec["toolchain_manifest"] = manifest_name
        spec["toolchain_manifest_path"] = manifest_path
    note_file = target_spec.get("note_file")
    if note_file:
        note_path = contract_path.parent / note_file
        if not note_path.is_file():
            fail(f"note file not found for target '{target}': {note_path}")
        spec["note_file"] = note_file
        spec["note_file_path"] = note_path
    return spec


def release_variant_matrix(contract: dict) -> list[dict]:
    """Every (target, backend) pair the contract publishes, in a stable order."""
    matrix: list[dict] = []
    for target in sorted(contract["variants"]):
        target_spec = contract["variants"][target]
        for backend in sorted(target_spec["backends"]):
            variant = target_spec["backends"][backend]
            matrix.append(
                {
                    "target": target,
                    "backend": backend,
                    "cargo_feature": variant["cargo_feature"],
                    "distribution_backend": variant["distribution_backend"],
                    "toolchain_free": variant["toolchain_free"],
                    "archive_format": target_spec["archive_format"],
                    "archive_name_template": variant["archive_name_template"],
                    "archive_root_template": variant["archive_root_template"],
                    "components": list(variant["components"]),
                    "runtime_profiles": list(variant["runtime_profiles"]),
                }
            )
    return matrix


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


def archive_member_parts(name: str) -> list[str] | None:
    """The safe, relative components of an archive member name, or None.

    Absolute paths, drive letters, UNC names, and any `..` component are
    rejected outright: a release archive may only ever describe paths
    *below* the directory it is extracted into.
    """
    normalized = name.replace("\\", "/")
    if normalized.startswith("/"):
        return None
    if len(normalized) >= 2 and normalized[1] == ":":
        return None
    parts: list[str] = []
    for part in normalized.split("/"):
        if part in ("", "."):
            continue
        if part == "..":
            return None
        parts.append(part)
    return parts


def _member_destination(root: Path, parts: list[str], archive_path: Path, name: str) -> Path:
    """The on-disk path for a validated member, with a containment re-check.

    The component walk is the second half of the guarantee: a member name
    can be perfectly relative and still escape if one of its parent
    directories is a symlink planted by an earlier member. Links are
    materialized only after every file is written *and* are themselves
    constrained to the root, so this cannot happen; the check stays because
    it is what makes that argument true rather than assumed.
    """
    candidate = root
    for part in parts:
        candidate = candidate / part
        if candidate.is_symlink():
            fail(
                f"archive '{archive_path.name}' member '{name}' resolves through the symlink "
                f"'{candidate}'; refusing to extract"
            )
    try:
        candidate.relative_to(root)
    except ValueError:
        fail(f"archive '{archive_path.name}' member '{name}' escapes the extraction root")
    return candidate


def _strip_parts(
    parts: list[str],
    strip_components: int,
    is_directory: bool,
    archive_path: Path,
    name: str,
) -> list[str] | None:
    if strip_components <= 0:
        return parts
    if len(parts) <= strip_components:
        if is_directory or len(parts) == strip_components:
            return None
        fail(
            f"cannot strip {strip_components} path component(s) from member '{name}' of "
            f"{archive_path.name}"
        )
    return parts[strip_components:]


def _write_member_stream(source, destination: Path, mode: int) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("wb") as handle:
        shutil.copyfileobj(source, handle, 1024 * 1024)
    # Owner read/write always: pruning and packaging have to be able to
    # read and delete what was just extracted, whatever the archive says.
    os.chmod(destination, (mode & 0o777) | stat.S_IRUSR | stat.S_IWUSR)


def _resolve_absolute_link_in_root(root: Path, link_dir: Path, target: str) -> Path | None:
    """Resolve an absolute symlink target against the extracted tree.

    Sysroot-based cross toolchains ship links like
    `x86_64-linux-musl/lib/ld-musl-x86_64.so.1 -> /lib/libc.so`, where "/"
    means "this toolchain's own sysroot", not the host root. Every ancestor
    of the link is tried as a stand-in root, innermost first; the host
    filesystem is never consulted.
    """
    relative = PurePosixPath(target.replace("\\", "/")).parts[1:]
    if not relative or (len(target) >= 2 and target[1] == ":"):
        # A drive-letter target is a host path by construction, never a
        # sysroot-relative one.
        return None
    ancestors: list[Path] = []
    current = link_dir
    while True:
        ancestors.append(current)
        if current == root or current.parent == current:
            break
        current = current.parent
    for ancestor in ancestors:
        candidate = ancestor.joinpath(*relative)
        try:
            candidate.relative_to(root)
        except ValueError:
            continue
        if candidate.exists() or candidate.is_symlink():
            return candidate
    return None


def _materialize_link(
    root: Path,
    destination: Path,
    kind: str,
    target: str,
    archive_path: Path,
    name: str,
    allow_absolute_symlinks: bool,
) -> None:
    if kind == "hardlink":
        resolved = Path(os.path.normpath(destination.parent / target))
        try:
            resolved.relative_to(root)
        except ValueError:
            fail(
                f"archive '{archive_path.name}' member '{name}' hard-links outside the "
                "extraction root"
            )
        if not resolved.is_file():
            fail(
                f"archive '{archive_path.name}' member '{name}' hard-links to "
                f"'{target}', which the archive never provides"
            )
        destination.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.link(resolved, destination)
        except OSError:
            shutil.copy2(resolved, destination)
        return

    absolute = target.startswith("/") or target.startswith("\\") or (
        len(target) >= 2 and target[1] == ":"
    )
    if absolute:
        if not allow_absolute_symlinks:
            fail(
                f"archive '{archive_path.name}' member '{name}' is a symlink to the absolute "
                f"path '{target}'; refusing to extract"
            )
        resolved = _resolve_absolute_link_in_root(root, destination.parent, target)
        if resolved is None:
            fail(
                f"archive '{archive_path.name}' member '{name}' is a symlink to '{target}', "
                "which does not resolve inside the archive's own tree; it would point at the "
                "build host's filesystem"
            )
        link_target = os.path.relpath(resolved, destination.parent)
    else:
        resolved = Path(os.path.normpath(destination.parent / target))
        try:
            resolved.relative_to(root)
        except ValueError:
            fail(
                f"archive '{archive_path.name}' member '{name}' is a symlink to '{target}', "
                "which escapes the extraction root"
            )
        link_target = target.replace("\\", "/")

    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists() or destination.is_symlink():
        remove_path(destination)
    try:
        os.symlink(link_target, destination)
        return
    except OSError as exc:
        # Windows hosts without the symlink privilege: copy the in-tree
        # target instead. Nothing outside the root can be reached either
        # way, and the substitution is reported rather than hidden.
        if resolved.is_file():
            shutil.copy2(resolved, destination)
            return
        if resolved.is_dir():
            shutil.copytree(resolved, destination, symlinks=False, dirs_exist_ok=True)
            return
        print(
            f"warning: archive '{archive_path.name}' member '{name}' could not be created as "
            f"a link to '{target}', and that target is not available in the extracted tree; "
            f"skipping it ({exc})",
            file=sys.stderr,
        )


def _extract_tar_safely(
    archive_path: Path,
    archive_type: str,
    root: Path,
    strip_components: int,
    allow_absolute_symlinks: bool,
) -> list[str]:
    mode = {"gztar": "r:gz", "xztar": "r:xz", "bztar": "r:bz2"}[
        python_unpack_format(archive_type)
    ]
    extracted: list[str] = []
    deferred: list[tuple[list[str], str, str, str]] = []
    top_level: set[str] = set()
    with tarfile.open(archive_path, mode) as archive:
        for member in archive:
            parts = archive_member_parts(member.name)
            if parts is None:
                fail(
                    f"archive '{archive_path.name}' member '{member.name}' is not a safe "
                    "relative path; refusing to extract"
                )
            if not parts:
                continue
            top_level.add(parts[0])
            if member.mode & (stat.S_ISUID | stat.S_ISGID):
                fail(
                    f"archive '{archive_path.name}' member '{member.name}' is setuid/setgid; "
                    "refusing to extract"
                )
            if not (member.isreg() or member.isdir() or member.issym() or member.islnk()):
                fail(
                    f"archive '{archive_path.name}' member '{member.name}' is neither a file, "
                    "directory, nor link; refusing to extract"
                )
            stripped = _strip_parts(
                parts, strip_components, member.isdir(), archive_path, member.name
            )
            if stripped is None:
                continue
            destination = _member_destination(root, stripped, archive_path, member.name)
            relative = PurePosixPath(*stripped).as_posix()
            if member.isdir():
                destination.mkdir(parents=True, exist_ok=True)
                extracted.append(relative + "/")
                continue
            if member.issym():
                deferred.append((stripped, "symlink", member.linkname, member.name))
                extracted.append(relative)
                continue
            if member.islnk():
                link_parts = archive_member_parts(member.linkname)
                if link_parts is None:
                    fail(
                        f"archive '{archive_path.name}' member '{member.name}' hard-links to "
                        f"the unsafe path '{member.linkname}'"
                    )
                link_stripped = _strip_parts(
                    link_parts, strip_components, False, archive_path, member.linkname
                )
                if link_stripped is None:
                    fail(
                        f"archive '{archive_path.name}' member '{member.name}' hard-links "
                        f"outside the archive's stripped root ('{member.linkname}')"
                    )
                link_destination = _member_destination(
                    root, link_stripped, archive_path, member.linkname
                )
                deferred.append(
                    (
                        stripped,
                        "hardlink",
                        os.path.relpath(link_destination, destination.parent),
                        member.name,
                    )
                )
                extracted.append(relative)
                continue
            source = archive.extractfile(member)
            if source is None:
                fail(
                    f"archive '{archive_path.name}' member '{member.name}' has no readable "
                    "content"
                )
            with source:
                _write_member_stream(source, destination, member.mode)
            extracted.append(relative)
    if strip_components > 0 and len(top_level) > 1:
        fail(
            f"cannot strip {strip_components} path component(s) from {archive_path.name}; "
            f"expected a single top-level directory, found {', '.join(sorted(top_level))}"
        )
    for stripped, kind, target, name in deferred:
        destination = _member_destination(root, stripped, archive_path, name)
        _materialize_link(
            root, destination, kind, target, archive_path, name, allow_absolute_symlinks
        )
    return extracted


def _extract_zip_safely(
    archive_path: Path,
    root: Path,
    strip_components: int,
    allow_absolute_symlinks: bool,
) -> list[str]:
    extracted: list[str] = []
    deferred: list[tuple[list[str], str, str, str]] = []
    top_level: set[str] = set()
    with zipfile.ZipFile(archive_path) as archive:
        for info in archive.infolist():
            if info.flag_bits & 0x1:
                fail(
                    f"archive '{archive_path.name}' member '{info.filename}' is encrypted; "
                    "refusing to extract"
                )
            parts = archive_member_parts(info.filename)
            if parts is None:
                fail(
                    f"archive '{archive_path.name}' member '{info.filename}' is not a safe "
                    "relative path; refusing to extract"
                )
            if not parts:
                continue
            top_level.add(parts[0])
            unix_mode = (info.external_attr >> 16) & 0xFFFF
            if unix_mode & (stat.S_ISUID | stat.S_ISGID):
                fail(
                    f"archive '{archive_path.name}' member '{info.filename}' is "
                    "setuid/setgid; refusing to extract"
                )
            is_symlink = stat.S_ISLNK(unix_mode)
            is_directory = info.is_dir() or (
                stat.S_ISDIR(unix_mode) and not info.file_size
            )
            if not is_symlink and unix_mode and not (
                stat.S_ISREG(unix_mode) or is_directory
            ):
                fail(
                    f"archive '{archive_path.name}' member '{info.filename}' is neither a "
                    "file, directory, nor symlink; refusing to extract"
                )
            stripped = _strip_parts(
                parts, strip_components, is_directory, archive_path, info.filename
            )
            if stripped is None:
                continue
            destination = _member_destination(root, stripped, archive_path, info.filename)
            relative = PurePosixPath(*stripped).as_posix()
            if is_directory:
                destination.mkdir(parents=True, exist_ok=True)
                extracted.append(relative + "/")
                continue
            if is_symlink:
                target = archive.read(info).decode("utf-8", errors="strict")
                deferred.append((stripped, "symlink", target, info.filename))
                extracted.append(relative)
                continue
            permissions = unix_mode & 0o777 if unix_mode else 0o644
            with archive.open(info) as source:
                _write_member_stream(source, destination, permissions)
            extracted.append(relative)
    if strip_components > 0 and len(top_level) > 1:
        fail(
            f"cannot strip {strip_components} path component(s) from {archive_path.name}; "
            f"expected a single top-level directory, found {', '.join(sorted(top_level))}"
        )
    for stripped, kind, target, name in deferred:
        destination = _member_destination(root, stripped, archive_path, name)
        _materialize_link(
            root, destination, kind, target, archive_path, name, allow_absolute_symlinks
        )
    return extracted


def extract_archive_safely(
    archive_path: Path,
    archive_type: str,
    destination: Path,
    strip_components: int,
    *,
    allow_absolute_symlinks: bool = False,
) -> list[str]:
    """Extract a *verified* archive member by member into a fresh directory.

    Nothing is ever handed to an external extractor and no member is
    written before it has been checked: absolute names, drive letters,
    `..` traversal, device/fifo members, setuid bits, encrypted entries,
    hard links leaving the tree, and symlinks resolving outside the
    extraction root are all hard errors. Links are materialized only after
    every regular member exists, so no member can be written *through* a
    link planted by an earlier one.

    `allow_absolute_symlinks` accepts sysroot-style absolute symlinks
    (`/lib/libc.so`) by rewriting them against the archive's own tree; a
    target that does not resolve inside that tree is still rejected, so a
    staged toolchain can never reference the build host's filesystem. With
    it off — the rule for payload archives such as the pinned LLVM provider
    — only relative links contained by the extraction root survive, which
    is exactly what an archive's own alias links (`libLLVM.so.22 ->
    libLLVM.so.22.1`) are.
    """
    if strip_components < 0:
        fail(f"invalid strip_components {strip_components} for {archive_path.name}")
    if not archive_path.is_file():
        fail(f"archive not found: {archive_path}")
    if archive_path.stat().st_size == 0:
        fail(f"archive '{archive_path}' is empty")
    ensure_clean_dir(destination)
    root = destination.resolve()
    unpack_format = python_unpack_format(archive_type)
    try:
        if unpack_format == "zip":
            if not zipfile.is_zipfile(archive_path):
                fail(f"archive '{archive_path}' is not a valid zip archive")
            return _extract_zip_safely(
                archive_path, root, strip_components, allow_absolute_symlinks
            )
        return _extract_tar_safely(
            archive_path,
            archive_type,
            root,
            strip_components,
            allow_absolute_symlinks,
        )
    except (tarfile.TarError, zipfile.BadZipFile, EOFError, OSError) as exc:
        fail(f"cannot read archive '{archive_path}': {exc}")


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


def llvm_code_generator_spec(manifest: dict) -> dict | None:
    """The manifest's declared LLVM code generator, or None when the
    manifest predates the declaration.

    `--backend llvm` loads this shared library in-process to parse,
    verify, optimize, and emit object code. It is a *release artifact*,
    not a host dependency: nothing about the backend consults an
    installed LLVM SDK, `clang`, `llvm-as`, `opt`, or `llc`.
    """
    spec = manifest.get("toolchain", {}).get("llvm_code_generator")
    if spec is None:
        return None
    if not isinstance(spec, dict):
        fail("toolchain.llvm_code_generator must be an object when present")
    status = spec.get("status")
    if status not in ("present", "absent"):
        fail(
            "toolchain.llvm_code_generator.status must be 'present' or 'absent', "
            f"got {status!r}"
        )
    if status == "present" and not spec.get("path"):
        fail("toolchain.llvm_code_generator.status is 'present' but no path is declared")
    return spec


def fetch_declared_archive(archive: dict, download_dir: Path) -> Path:
    """Download and digest-check one manifest-declared archive."""
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
            fail(f"digest mismatch for {download_path.name}: expected {expected}, got {actual}")
    elif not download_path.exists():
        download_file(url, download_path)

    return download_path


def llvm_provider_declared_files(spec: dict) -> list[dict]:
    """Exactly the archive members the manifest authorizes for the overlay.

    An LLVM provider archive is a general-purpose upstream tarball; only the
    members named here — the code generator itself plus the notices and
    metadata the manifest lists — may ever reach a release package. Anything
    else in the archive is ignored by construction rather than by filtering.
    """
    files = spec.get("files")
    if not isinstance(files, list) or not files:
        fail(
            "a present LLVM code generator with a separate archive must declare a non-empty "
            "toolchain.llvm_code_generator.files list"
        )
    notices = spec.get("notice_files") or []
    if not isinstance(notices, list):
        fail("toolchain.llvm_code_generator.notice_files must be a list when present")
    declared: list[dict] = []
    seen: set[str] = set()
    for file_spec in list(files) + list(notices):
        if not isinstance(file_spec, dict) or "source" not in file_spec or "path" not in file_spec:
            fail("each LLVM provider file must declare 'source' and 'path'")
        source = safe_relative_path(file_spec["source"]).as_posix()
        path = safe_relative_path(file_spec["path"]).as_posix()
        if path in seen:
            fail(f"the LLVM provider declares '{path}' twice")
        seen.add(path)
        declared.append({"source": source, "path": path})
    return declared


def extract_llvm_provider_archive(
    spec: dict, archive_path: Path, destination: Path, description: str
) -> dict:
    """Authenticate and safely extract a pinned LLVM provider archive.

    The archive carries alias links beside the real library, so links are
    extracted rather than refused — but only relative ones that stay inside
    the archive's own tree. An absolute link, or one resolving outside the
    root, is still a hard error, and no link is ever staged unless the
    repository manifest declares the file it names.
    """
    archive = spec.get("archive")
    if not isinstance(archive, dict):
        fail(f"{description}: this target declares no separately pinned provider archive")
    verified = verify_supplied_archive(archive_path, archive, description)
    strip_components = int(spec.get("extract", {}).get("strip_components", 0))
    extract_archive_safely(
        archive_path,
        archive["type"],
        destination,
        strip_components,
        allow_absolute_symlinks=False,
    )
    assert_no_escaping_symlinks(destination, description)
    return verified


def resolve_contained_payload(source: Path, root: Path, description: str, label: str) -> Path:
    """The regular file a declared archive member ultimately names.

    A pinned provider archive legitimately ships alias links next to the
    real library (`lib/libLLVM.so.22`, `lib/libLLVM-22.so` ->
    `libLLVM.so.22.1`), so a declared source is allowed to be a link. What
    it may never be is a link out of the extraction root, a dangling link,
    or anything other than a regular file once fully resolved — `realpath`
    collapses the whole chain, so a multi-hop alias cannot smuggle in a
    target the individual member checks would have caught.
    """
    if not source.exists() and not source.is_symlink():
        fail(f"{description} is missing declared file '{label}'")
    resolved = Path(os.path.realpath(source))
    root_real = Path(os.path.realpath(root))
    try:
        resolved.relative_to(root_real)
    except ValueError:
        fail(
            f"{description} declares '{label}', which resolves to '{resolved}' outside the "
            "extracted archive"
        )
    if not resolved.is_file():
        fail(
            f"{description} declares '{label}', which does not resolve to a regular file "
            "inside the archive"
        )
    return resolved


def copy_declared_provider_files(
    extracted: Path, declared: list[dict], destination_root: Path, description: str
) -> list[dict]:
    """Copy only the declared members, each checked to be a real payload.

    An alias link is resolved and staged as a deterministic regular copy of
    the file it names; undeclared members — including the archive's own
    alias links — are never staged, because only this list is walked.
    """
    staged: list[dict] = []
    for file_spec in declared:
        source = extracted / safe_relative_path(file_spec["source"])
        resolved = resolve_contained_payload(
            source, extracted, description, file_spec["source"]
        )
        if resolved.stat().st_size == 0:
            fail(f"{description} declares '{file_spec['source']}', which is empty")
        relative = safe_relative_path(file_spec["path"])
        target = destination_root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(resolved, target)
        entry = {
            "path": relative.as_posix(),
            "sha256": compute_digest(target, "sha256"),
            "size": target.stat().st_size,
        }
        archive_relative = resolved.relative_to(Path(os.path.realpath(extracted))).as_posix()
        if archive_relative != safe_relative_path(file_spec["source"]).as_posix():
            # The declared source was an alias: record what was actually
            # copied so the evidence names the real payload.
            entry["archive_source"] = archive_relative
        staged.append(entry)
    return staged


def stage_llvm_code_generator(root: Path, manifest: dict, download_dir: Path) -> None:
    """Overlay a separately pinned LLVM provider into a fetched toolchain.

    Windows' llvm-mingw archive already contains libLLVM, so its manifest
    declares no overlay. Linux keeps the compact musl cross-toolchain archive
    stable and layers a code-generator-only archive on top: no Clang, SDK,
    headers, or command-line LLVM tools enter the release bundle.
    """
    spec = llvm_code_generator_spec(manifest)
    if spec is None or spec.get("status") != "present":
        return
    archive = spec.get("archive")
    if archive is None:
        return
    declared = llvm_provider_declared_files(spec)

    archive_path = fetch_declared_archive(archive, download_dir)
    description = (
        f"pinned LLVM provider archive for {manifest.get('target', 'unknown')} "
        f"('{archive_path.name}')"
    )
    with temporary_staging_dir(
        download_dir, f".llvm-provider-{manifest.get('target', 'unknown')}-"
    ) as workspace:
        extracted = workspace / "provider"
        extract_llvm_provider_archive(spec, archive_path, extracted, description)
        copy_declared_provider_files(extracted, declared, root, description)


def verify_llvm_code_generator(root: Path, manifest: dict) -> Path | None:
    """Fail if the manifest promises a packaged LLVM code generator that
    the staged toolchain does not actually contain.

    Pruning is glob-driven, so this is the mechanical guard against a
    future prune rule silently deleting the one artifact that makes
    `--backend llvm` work at all. Returns the verified path, or None when
    the manifest declares no code generator (that target simply has no
    LLVM backend, which is a supported configuration).
    """
    spec = llvm_code_generator_spec(manifest)
    if spec is None or spec.get("status") != "present":
        return None
    relative = safe_relative_path(spec["path"])
    candidate = root / relative
    if not candidate.is_file():
        fail(
            f"toolchain manifest declares an LLVM code generator at '{spec['path']}', but "
            f"'{candidate}' does not exist after staging/pruning. --backend llvm loads this "
            "library in-process and cannot fall back to a compiler; add a keep_glob for it or "
            "set toolchain.llvm_code_generator.status to 'absent'."
        )
    size = candidate.stat().st_size
    if size == 0:
        fail(f"staged LLVM code generator '{candidate}' is empty")
    print(
        f"Verified LLVM code generator: {relative.as_posix()} ({size} bytes, "
        f"LLVM {spec.get('required_major', '?')} C API)",
        file=sys.stderr,
    )
    return candidate


def declared_archive_digest(archive: dict, description: str) -> dict:
    """The pinned digest of a manifest-declared archive, or a hard error.

    A release input that carries no pinned digest cannot be authenticated,
    so there is no "unverified but accepted" path here.
    """
    digest = archive.get("digest")
    if not isinstance(digest, dict):
        fail(f"{description} declares no pinned digest; it cannot be trusted as a release input")
    algorithm = str(digest.get("algorithm", "")).lower()
    value = str(digest.get("value", "")).lower()
    if algorithm not in hashlib.algorithms_guaranteed:
        fail(f"{description} pins an unsupported digest algorithm {digest.get('algorithm')!r}")
    if not value:
        fail(f"{description} pins an empty digest value")
    return {"algorithm": algorithm, "value": value}


def verify_supplied_archive(archive_path: Path, archive: dict, description: str) -> dict:
    """Authenticate a caller-supplied archive against its pinned digest.

    This is the trust boundary for release staging: everything downstream —
    extraction, pruning, copying, the recorded provenance — is only as good
    as this check, so it happens before a single member is read.
    """
    digest = declared_archive_digest(archive, description)
    if not archive_path.is_file():
        fail(f"{description}: archive not found at {archive_path}")
    size = archive_path.stat().st_size
    if size == 0:
        fail(f"{description}: archive {archive_path} is empty")
    actual = compute_digest(archive_path, digest["algorithm"]).lower()
    if actual != digest["value"]:
        fail(
            f"{description}: archive {archive_path} does not match the pinned "
            f"{digest['algorithm']} digest (expected {digest['value']}, got {actual})"
        )
    return {"algorithm": digest["algorithm"], "value": actual, "size": size}


@contextlib.contextmanager
def temporary_staging_dir(parent: Path, prefix: str):
    """A scratch directory beside the release output that always goes away.

    Extraction happens here, never into the bundle: a rejected archive must
    not be able to leave a partial payload where packaging could pick it up.
    """
    parent.mkdir(parents=True, exist_ok=True)
    path = Path(tempfile.mkdtemp(prefix=prefix, dir=str(parent)))
    try:
        yield path
    finally:
        remove_path(path)


def assert_no_escaping_symlinks(root: Path, description: str) -> None:
    """Fail if any staged symlink can reach outside its own tree.

    Copying follows symlinks, so a link left pointing at the build host
    would quietly pull host content into a release package. Links that
    dangle *inside* the tree are fine — pruning legitimately removes their
    targets — because they can never name anything the archive did not.
    """
    for path in sorted(root.rglob("*"), key=lambda item: item.as_posix()):
        if not path.is_symlink():
            continue
        target = os.readlink(path)
        if os.path.isabs(target) or (len(target) >= 2 and target[1] == ":"):
            fail(
                f"{description}: '{path.relative_to(root).as_posix()}' is an absolute symlink "
                f"to '{target}'; a packaged toolchain must not reference the build host"
            )
        resolved = Path(os.path.normpath(path.parent / target))
        try:
            resolved.relative_to(root)
        except ValueError:
            fail(
                f"{description}: '{path.relative_to(root).as_posix()}' is a symlink to "
                f"'{target}', which escapes the staged tree"
            )


def prepare_toolchain_from_archive(
    manifest: dict, archive_path: Path, destination: Path, description: str
) -> dict:
    """Verify, safely extract, and prune one pinned toolchain archive.

    The single implementation shared by `fetch-toolchain` (which downloads
    the archive first) and release staging (which is handed an already
    downloaded, cached archive and never touches the network). Both get the
    same digest check, the same member-validated extraction, and the same
    manifest-driven strip/symlink-fix/prune/wrapper rules.
    """
    archive = manifest["toolchain"]["archive"]
    verified = verify_supplied_archive(archive_path, archive, description)
    strip_components = int(manifest["toolchain"].get("extract", {}).get("strip_components", 0))
    extract_archive_safely(
        archive_path,
        archive["type"],
        destination,
        strip_components,
        allow_absolute_symlinks=True,
    )
    fix_absolute_symlinks(destination)
    assert_no_escaping_symlinks(destination, description)

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

    return verified


def fetch_toolchain(manifest_path: Path, download_dir: Path, destination: Path) -> tuple[dict, Path]:
    manifest = load_manifest(manifest_path)
    archive = manifest["toolchain"]["archive"]
    download_path = fetch_declared_archive(archive, download_dir)

    prepare_toolchain_from_archive(
        manifest, download_path, destination, f"pinned toolchain archive for {manifest['target']}"
    )

    stage_llvm_code_generator(destination, manifest, download_dir)

    # Pruning happens above, so this is the last chance to notice that a
    # glob removed the packaged LLVM code generator `--backend llvm`
    # depends on.
    verify_llvm_code_generator(destination, manifest)

    return manifest, destination


def write_install_readme(path: Path, variant: dict, asset_name: str) -> None:
    """The package README: what this artifact is, and precisely what it can
    and cannot do. Every claim here is enforced by the compiler itself (see
    `backend::select` and `backend::no_toolchain`), so it must not overstate
    a package's capabilities."""
    target = variant["target"]
    platform, arch = target.split("-", 1)
    backend = variant["backend"]
    components = variant["components"]

    if platform == "windows":
        install_hint = "Run install.ps1 from this directory, or keep this directory on PATH."
    else:
        install_hint = "Run install.sh from this directory, or copy oscan somewhere on PATH."

    lines: list[str] = [
        f"Oscan release asset: {asset_name}",
        f"Platform: {platform} {arch}",
        f"Backend: {backend} (this package's only backend, and its default)",
        "",
        install_hint,
        "",
        "What this package contains",
        "--------------------------",
    ]
    if variant["backend_kind"] == "object":
        lines.append(
            f"  oscan compiled with --features {variant['cargo_feature']} only: it emits object "
            "code directly and never writes or compiles C."
        )
        lines.append(
            f"  {NATIVE_LINK_DIR_NAME}/  the linker and its runtime libraries, verified against "
            f"{NATIVE_LINK_DIR_NAME}/{NATIVE_LINK_MANIFEST_NAME} (SHA-256) before every use."
        )
        lines.append(
            f"  build/runtime-archives/{target}/  precompiled freestanding runtime archives "
            f"({', '.join(variant['runtime_profiles'])})."
        )
        if "llvm_provider" in components:
            if variant.get("llvm_provider_source") == "direct-link-sidecar":
                lines.append(
                    f"  the LLVM code generator is the single verified "
                    f"{variant.get('llvm_provider_asset', 'libLLVM')} in {NATIVE_LINK_DIR_NAME}/ "
                    "— it is shared with the linker rather than duplicated."
                )
            else:
                lines.append(
                    "  toolchain/  the packaged LLVM code generator only (no clang, no GCC, no "
                    "headers, no sysroot, no LLVM command-line tools)."
                )
        lines += [
            "",
            "What this package does NOT do",
            "-----------------------------",
            "  --backend c, --emit-c and -o *.c are refused: this build has no C backend.",
            "  --libc is refused: the hosted runtime needs this machine's CRT/libm.",
            "  --extra-c/--extra-cflags are refused: there is no C compilation step.",
            "  extern functions with `str` parameters/returns need a generated C shim and are "
            "refused for the same reason.",
            "  No C compiler is bundled, and none is searched for on PATH.",
        ]
        if backend == "cranelift" and platform == "windows":
            lines.append(
                "  libLLVM in this package is only LLD's runtime dependency; this build has no "
                "LLVM backend (--backend llvm is not included)."
            )
        lines.append(
            f"  Other backends are not included: --backend {'llvm' if backend == 'cranelift' else 'cranelift'} "
            "and --backend c report which package to install instead."
        )
    else:
        lines.append(
            f"  oscan compiled with --features {variant['cargo_feature']} only: it emits C and "
            "needs a C toolchain to compile it."
        )
        if "c_toolchain" in components:
            lines.append(
                "  toolchain/  the pinned C toolchain this package uses; oscan finds it beside "
                "its own executable, never on PATH."
            )
        else:
            lines.append(
                f"  No toolchain is bundled: {variant.get('required_host_toolchain', 'a host C toolchain')} "
                "must be installed on this machine."
            )
        lines += [
            "",
            "What this package does NOT do",
            "-----------------------------",
            "  --backend llvm and --backend cranelift are not included; each reports which "
            "package to install instead.",
            "  No native-link sidecar, LLVM provider, or freestanding runtime archives are "
            "shipped.",
        ]
        if variant.get("note_file"):
            lines.append(f"  See {variant['note_file']} for the macOS phase 1 note.")

    lines += [
        "",
        f"Package metadata: {PACKAGE_METADATA_NAME}",
        "GitHub Releases are the canonical install surface.",
        "",
    ]
    path.write_text("\n".join(lines), encoding="utf-8", newline="\n")


def write_package_metadata(
    path: Path,
    variant: dict,
    version: str,
    archive_name: str,
    bundle_name: str,
    component_digests: dict,
) -> None:
    """Machine-readable record of what this package is, mirroring what the
    packaged compiler reports through `oscan --version`."""
    metadata = {
        "schema_version": 1,
        "version": version,
        "target": variant["target"],
        "backend": variant["backend"],
        "available_backends": [variant["backend"]],
        "default_backend": variant["distribution_backend"],
        "cargo_feature": variant["cargo_feature"],
        "toolchain_free": variant["toolchain_free"],
        "components": list(variant["components"]),
        "runtime_profiles": list(variant["runtime_profiles"]),
        "archive_name": archive_name,
        "archive_root": bundle_name,
        "requirements": {
            "host_c_toolchain": variant.get("required_host_toolchain")
            if variant.get("requires_host_compiler")
            else None,
            "bundled_c_toolchain": "c_toolchain" in variant["components"],
        },
        "component_digests": component_digests,
    }
    path.write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n"
    )


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


def write_provenance_file(
    path: Path,
    manifest: dict,
    copied_licenses: list[str],
    verified_archive: dict,
    source_manifest: str,
) -> None:
    """Record which authenticated source archive this toolchain came from.

    The digest written here is the one that was actually computed over the
    staged archive and matched against the manifest, not a value copied out
    of the manifest and asserted.
    """
    licenses = "\n".join(f"- {entry}" for entry in copied_licenses) or "- none matched configured globs"
    text = (
        textwrap.dedent(
            f"""\
            Toolchain vendor: {manifest["toolchain"]["vendor"]}
            Toolchain version: {manifest["toolchain"]["version"]}
            Target: {manifest["target"]}
            Source manifest: {source_manifest}
            Archive URL: {manifest["toolchain"]["archive"]["url"]}
            Archive digest ({verified_archive["algorithm"]}): {verified_archive["value"]}
            Archive size (bytes): {verified_archive["size"]}

            Copied license files:
            """
        )
        + licenses
        + "\n"
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")


def _is_executable_entry(relative: PurePosixPath, source: Path | None = None) -> bool:
    """Which staged files carry the executable bit in a package.

    Canonicalized rather than copied from the build machine: a Windows
    checkout has no Unix permission bits to preserve, and a Linux one may
    carry whatever umask the runner had. The compiler binary, the install
    script, and the staged linker/runtime binaries must be executable; data
    files must not be.
    """
    name = relative.name
    # Preserve a packaged toolchain's own executability: any source file
    # with an executable bit set stays executable (nested bin/ and libexec/
    # tools included). Windows-hosted assembly has no such bits, which is
    # why the known package executables below are listed explicitly.
    if source is not None and not source.is_dir():
        try:
            if source.stat().st_mode & 0o111:
                return True
        except OSError:
            pass
    if name in ("oscan", "install.sh"):
        return True
    if relative.parts and relative.parts[0] == NATIVE_LINK_DIR_NAME:
        return relative.suffix in ("", ".exe", ".dll", ".so") or ".so." in name
    if relative.suffix in (".sh", ".exe"):
        return True
    return False


def _archive_entries(bundle_dir: Path) -> list[tuple[PurePosixPath, Path]]:
    """Every staged entry, in one deterministic (sorted, POSIX) order."""
    entries = [
        (PurePosixPath(item.relative_to(bundle_dir).as_posix()), item)
        for item in bundle_dir.rglob("*")
    ]
    return sorted(entries, key=lambda pair: pair[0].as_posix())


def create_zip_archive(bundle_dir: Path, archive_path: Path) -> None:
    """Byte-for-byte reproducible ZIP: sorted entries, one fixed timestamp,
    canonical modes, and no host-dependent metadata (no external tool, no
    source mtimes, no creator-system quirks)."""
    if archive_path.exists():
        archive_path.unlink()
    stamp = time.gmtime(archive_epoch())
    date_time = (
        stamp.tm_year,
        stamp.tm_mon,
        stamp.tm_mday,
        stamp.tm_hour,
        stamp.tm_min,
        stamp.tm_sec,
    )
    root = PurePosixPath(bundle_dir.name)
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for relative, source in _archive_entries(bundle_dir):
            arcname = (root / relative).as_posix()
            if source.is_dir():
                info = zipfile.ZipInfo(arcname + "/", date_time=date_time)
                info.external_attr = ((0o40000 | 0o755) << 16) | 0x10
                info.create_system = 3
                archive.writestr(info, b"")
                continue
            mode = 0o755 if _is_executable_entry(relative, source) else 0o644
            info = zipfile.ZipInfo(arcname, date_time=date_time)
            info.external_attr = (0o100000 | mode) << 16
            info.create_system = 3
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, source.read_bytes())


def normalize_tarinfo(info: tarfile.TarInfo, mode: int | None = None) -> tarfile.TarInfo:
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = archive_epoch()
    if mode is not None:
        info.mode = mode
    info.pax_headers = {}
    return info


def create_tar_archive(bundle_dir: Path, archive_path: Path, archive_format: str) -> None:
    """Byte-for-byte reproducible tar.gz/tar.xz.

    Sorted entries, normalized ownership/timestamps/modes, and — for gzip —
    a fixed header mtime with no embedded file name, which is the part
    `tarfile`'s own gzip mode would otherwise take from the clock and the
    output path.
    """
    if archive_format not in ("tar.gz", "tar.xz"):
        fail(f"unsupported tar archive format '{archive_format}'")
    if archive_path.exists():
        archive_path.unlink()

    def write_members(archive: tarfile.TarFile) -> None:
        root = PurePosixPath(bundle_dir.name)
        info = archive.gettarinfo(str(bundle_dir), root.as_posix())
        archive.addfile(normalize_tarinfo(info, 0o755))
        for relative, source in _archive_entries(bundle_dir):
            arcname = (root / relative).as_posix()
            if source.is_symlink():
                link = tarfile.TarInfo(arcname)
                link.type = tarfile.SYMTYPE
                link.linkname = os.readlink(source)
                archive.addfile(normalize_tarinfo(link, 0o777))
                continue
            if source.is_dir():
                info = archive.gettarinfo(str(source), arcname)
                archive.addfile(normalize_tarinfo(info, 0o755))
                continue
            mode = 0o755 if _is_executable_entry(relative, source) else 0o644
            info = archive.gettarinfo(str(source), arcname)
            with source.open("rb") as handle:
                archive.addfile(normalize_tarinfo(info, mode), handle)

    if archive_format == "tar.xz":
        with tarfile.open(archive_path, "w:xz", format=tarfile.GNU_FORMAT) as archive:
            write_members(archive)
        return

    with archive_path.open("wb") as raw:
        with gzip.GzipFile(
            filename="", mode="wb", fileobj=raw, compresslevel=9, mtime=archive_epoch()
        ) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.GNU_FORMAT) as archive:
                write_members(archive)


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
    # docs/design/native-link-embedding.md); the freestanding object backends
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
        and manifest.get("mode")
        in {"hosted", "freestanding", "freestanding_gfx", "freestanding_core"}
        and manifest.get("embedded_bearssl") is not True
    ):
        fail(
            f"native runtime manifest {manifest_path} does not embed BearSSL; "
            f"Linux release runtime archives for {target} must be built with "
            f"packaging/prebuilt/{target}/libbearssl.a present"
        )


def stage_direct_link_sidecar(
    bundle_dir: Path, variant: dict, native_link_dir: Path
) -> dict:
    """Stage the prepared native-link asset set by explicit allowlist.

    Exactly the manifest plus the files that manifest declares are copied,
    each verified against its declared SHA-256 first. Nothing else in the
    prepared directory is staged, so a stray file there can never reach a
    package.
    """
    manifest_path = native_link_dir / NATIVE_LINK_MANIFEST_NAME
    if not manifest_path.is_file():
        fail(
            f"native-link asset manifest not found: {manifest_path} (run "
            "scripts/prepare-embed-assets.ps1|.sh for this target first)"
        )
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"cannot read native-link asset manifest {manifest_path}: {exc}")
    if manifest.get("schema_version") != 1:
        fail(
            f"unsupported native-link asset manifest schema in {manifest_path}: "
            f"{manifest.get('schema_version')!r}"
        )
    if manifest.get("target") != variant["target"]:
        fail(
            f"native-link asset manifest {manifest_path} is staged for target "
            f"{manifest.get('target')!r}, expected {variant['target']!r}"
        )

    entries = [manifest.get("linker")] + list(manifest.get("assets", []))
    destination_root = bundle_dir / NATIVE_LINK_DIR_NAME
    staged: list[str] = []
    for entry in entries:
        if not isinstance(entry, dict):
            fail(f"native-link asset manifest {manifest_path} has a malformed entry")
        for key in ("role", "name", "install_subpath", "sha256"):
            if not entry.get(key):
                fail(f"native-link asset manifest {manifest_path} entry is missing '{key}'")
        relative = safe_relative_path(entry["install_subpath"])
        source = native_link_dir / relative
        if not source.is_file():
            fail(
                f"native-link asset '{entry['name']}' is missing from {native_link_dir} "
                f"(expected {source})"
            )
        actual = compute_digest(source, "sha256")
        if actual.lower() != str(entry["sha256"]).lower():
            fail(
                f"native-link asset '{entry['name']}' digest mismatch in {native_link_dir}: "
                f"manifest has {entry['sha256']}, actual is {actual}"
            )
        copy_path(source, destination_root / relative)
        staged.append(relative.as_posix())
    copy_path(manifest_path, destination_root / NATIVE_LINK_MANIFEST_NAME)

    linker = manifest.get("linker") or {}
    linker_dir = safe_relative_path(linker["install_subpath"]).parent
    for entry in manifest.get("assets", []):
        if entry.get("role") != "linker_runtime":
            continue
        if safe_relative_path(entry["install_subpath"]).parent != linker_dir:
            fail(
                f"native-link runtime library '{entry['name']}' is staged outside the linker's "
                "own directory; the compiler refuses a package whose linker cannot find its "
                "sibling runtime libraries"
            )
    return {
        "manifest": manifest,
        "staged": staged,
        "digest": compute_digest(manifest_path, "sha256"),
    }


def stage_freestanding_runtime_archives(
    bundle_dir: Path, variant: dict, runtime_archive_dir: Path
) -> dict:
    """Stage exactly the freestanding archive/manifest pairs this variant
    declares — no hosted archive, no runtime sources, no runtime builder."""
    target = variant["target"]
    runtime_contract = load_runtime_archive_contract(RUNTIME_ARCHIVE_CONTRACT_PATH)
    destination = bundle_dir / "build" / "runtime-archives" / target
    digests: dict = {}
    for profile in variant["runtime_profiles"]:
        if profile not in FREESTANDING_PROFILES:
            fail(f"runtime profile '{profile}' is not a freestanding profile")
        mode_spec = runtime_contract["modes"][profile]
        archive_path = runtime_archive_dir / mode_spec["archive_name"]
        manifest_path = runtime_archive_dir / mode_spec["manifest_name"]
        if not archive_path.is_file() or not manifest_path.is_file():
            fail(
                f"freestanding runtime {profile} archive pair for '{target}' is missing from "
                f"{runtime_archive_dir}; build it before staging the release"
            )
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            fail(f"cannot read native runtime manifest {manifest_path}: {exc}")
        if manifest.get("target") != target or manifest.get("mode") != profile:
            fail(
                f"native runtime manifest {manifest_path} identifies "
                f"{manifest.get('target')}/{manifest.get('mode')}, expected {target}/{profile}"
            )
        validate_runtime_archive_release_toolchain(
            runtime_contract, target, manifest, manifest_path
        )
        actual_digest = compute_digest(archive_path, "sha256")
        if manifest.get("sha256") != actual_digest:
            fail(
                f"native runtime archive digest mismatch for {archive_path}: manifest has "
                f"{manifest.get('sha256')!r}, actual is {actual_digest}"
            )
        copy_path(archive_path, destination / archive_path.name)
        copy_path(manifest_path, destination / manifest_path.name)
        digests[archive_path.name] = actual_digest
    return digests


def stage_llvm_provider_component(
    bundle_dir: Path, variant: dict, sidecar: dict | None, provider_archive: Path | None
) -> dict:
    """Make the packaged LLVM code generator available to this package.

    Windows shares the single verified `libLLVM` already staged in
    `native-link/` — the provider search reaches into that directory and the
    compiler verifies the file against the sidecar manifest before loading
    it, so staging a second copy would only duplicate ~80 MB.

    Linux authenticates the pinned provider *archive* against the digest in
    this repository's toolchain manifest, extracts only the members that
    manifest declares, and stages them into the executable-relative
    `toolchain/` layout the provider already searches: no clang, no GCC, no
    headers, no sysroot, no LLVM command-line tools. The archive is the only
    input; nothing about the payload is taken on the word of a file that
    travelled with it.
    """
    source = variant.get("llvm_provider_source")
    if source == "direct-link-sidecar":
        asset_name = variant["llvm_provider_asset"]
        if sidecar is None:
            fail("the LLVM provider is shared with the native-link sidecar, which was not staged")
        declared = [
            entry
            for entry in sidecar["manifest"].get("assets", [])
            if entry.get("role") == "linker_runtime" and entry.get("name") == asset_name
        ]
        if not declared:
            fail(
                f"this package shares its LLVM code generator with the native-link sidecar, but "
                f"'{asset_name}' is not declared there as a linker_runtime asset"
            )
        return {
            "source": "direct-link-sidecar",
            "path": f"{NATIVE_LINK_DIR_NAME}/{declared[0]['install_subpath']}",
            "sha256": declared[0]["sha256"],
        }

    manifest_path = variant.get("toolchain_manifest_path")
    if manifest_path is None:
        fail("a manifest-sourced LLVM provider needs the target's toolchain manifest")
    manifest_path = Path(manifest_path)
    manifest = load_manifest(manifest_path)
    if manifest.get("target") != variant["target"]:
        fail(
            f"toolchain manifest {manifest_path} describes target {manifest.get('target')!r}, "
            f"but this package targets {variant['target']!r}"
        )
    spec = llvm_code_generator_spec(manifest)
    if spec is None or spec.get("status") != "present":
        fail(
            f"target '{variant['target']}' declares no packaged LLVM code generator, so an "
            "llvm variant cannot be staged for it"
        )
    if not isinstance(spec.get("archive"), dict):
        fail(
            f"target '{variant['target']}' embeds its LLVM code generator in the toolchain "
            "archive; it has no separately pinned provider archive to stage"
        )
    if provider_archive is None:
        fail(
            "staging an LLVM package for this target needs --llvm-provider-archive (the "
            "already downloaded, digest-pinned provider archive); release staging never "
            "downloads"
        )
    provider_archive = Path(provider_archive)
    declared = llvm_provider_declared_files(spec)
    description = (
        f"pinned LLVM provider archive for {variant['target']} "
        f"({manifest_path.name} -> '{provider_archive.name}')"
    )
    toolchain_root = bundle_dir / "toolchain"
    with temporary_staging_dir(
        bundle_dir.parent, f".llvm-provider-{variant['target']}-"
    ) as workspace:
        extracted = workspace / "provider"
        verified = extract_llvm_provider_archive(
            spec, provider_archive, extracted, description
        )
        staged = copy_declared_provider_files(
            extracted, declared, toolchain_root, description
        )

    generator = verify_llvm_code_generator(toolchain_root, manifest)
    provenance = {
        "schema_version": 1,
        "target": variant["target"],
        "source_manifest": manifest_path.name,
        "source_archive": {
            "url": spec["archive"]["url"],
            "type": spec["archive"]["type"],
            "digest": {"algorithm": verified["algorithm"], "value": verified["value"]},
            "size": verified["size"],
        },
        "staged_root": "toolchain",
        "files": sorted(staged, key=lambda entry: entry["path"]),
    }
    write_provider_provenance_evidence(
        bundle_dir / "LICENSES" / "llvm-provider" / PROVIDER_PROVENANCE_NAME, provenance
    )
    return {
        "source": "toolchain-manifest",
        "path": f"toolchain/{safe_relative_path(spec['path']).as_posix()}",
        "sha256": compute_digest(generator, "sha256") if generator else None,
        "staged": [entry["path"] for entry in staged],
        "source_manifest": manifest_path.name,
        "source_archive_digest": {
            "algorithm": verified["algorithm"],
            "value": verified["value"],
        },
    }


def write_provider_provenance_evidence(path: Path, provenance: dict) -> None:
    """Write the staged provider's provenance record.

    This file is *output*, never input: it states which pinned archive was
    verified and what was actually staged from it. Nothing in the packaging
    pipeline reads it back to decide whether a payload can be trusted.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(provenance, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n"
    )


def llvm_provider_staged_paths(manifest: dict) -> set[str]:
    """Every toolchain-relative path the separately pinned LLVM provider
    overlay contributes, derived from the manifest rather than guessed from
    file names.

    A C package must ship the complete C toolchain and *none* of this: the
    provider is only there for `--backend llvm`, which a C package does not
    contain, and it is by far the largest single payload in the bundle.
    """
    spec = llvm_code_generator_spec(manifest)
    if spec is None:
        return set()
    # Only a *separately overlaid* provider is excludable. Windows' llvm-mingw
    # archive integrates libLLVM into the toolchain itself: clang.exe links
    # against it, so removing it from a Windows C package would break the very
    # compiler that package exists to ship. Such a manifest declares no
    # provider archive of its own, and nothing is excluded here.
    if not spec.get("archive"):
        return set()
    paths: set[str] = set()
    declared = spec.get("path")
    if declared:
        paths.add(safe_relative_path(declared).as_posix())
    for file_spec in spec.get("files", []) or []:
        if isinstance(file_spec, dict) and file_spec.get("path"):
            paths.add(safe_relative_path(file_spec["path"]).as_posix())
    for file_spec in spec.get("notice_files", []) or []:
        if isinstance(file_spec, dict) and file_spec.get("path"):
            paths.add(safe_relative_path(file_spec["path"]).as_posix())
    for extra in spec.get("metadata_files", []) or []:
        paths.add(safe_relative_path(extra).as_posix())
    return paths


def resolve_declared_archive(
    archive: dict, download_dir: Path, description: str, allow_download: bool
) -> tuple[Path, dict]:
    """The verified local path of a pinned archive, downloading only if asked.

    Staging is handed archives by CI and must never reach the network, so
    the download is opt-in; the digest check is not.
    """
    url = archive["url"]
    file_name = Path(urllib.parse.urlparse(url).path).name
    if not file_name:
        fail(f"cannot derive archive file name from {url}")
    archive_path = download_dir / file_name
    if not archive_path.is_file():
        if not allow_download:
            fail(
                f"{description}: {archive_path} is not present in the download cache and "
                "downloading was not requested"
            )
        download_dir.mkdir(parents=True, exist_ok=True)
        download_file(url, archive_path)
    verified = verify_supplied_archive(archive_path, archive, description)
    return archive_path, verified


def manifest_component_archive(manifest: dict, component: str) -> tuple[dict, str]:
    """The pinned archive spec for one manifest component."""
    target = manifest.get("target", "unknown")
    if component == "toolchain":
        return manifest["toolchain"]["archive"], f"pinned toolchain archive for {target}"
    if component == "llvm-provider":
        spec = llvm_code_generator_spec(manifest)
        if spec is None or spec.get("status") != "present":
            fail(f"target '{target}' declares no packaged LLVM code generator")
        archive = spec.get("archive")
        if not isinstance(archive, dict):
            fail(
                f"target '{target}' embeds its LLVM code generator in the toolchain archive; "
                "it has no separately pinned provider archive"
            )
        return archive, f"pinned LLVM provider archive for {target}"
    fail(f"unknown archive component '{component}'")


def resolve_archive_command(args: argparse.Namespace) -> int:
    """Print the verified local path of a manifest-pinned archive.

    This is how CI turns "the manifest pins X" into the concrete
    `--toolchain-archive` / `--llvm-provider-archive` input for staging:
    download once (`--download`), then resolve offline as often as needed.
    """
    manifest = load_manifest(Path(args.manifest).resolve())
    archive, description = manifest_component_archive(manifest, args.component)
    archive_path, _ = resolve_declared_archive(
        archive, Path(args.download_dir).resolve(), description, args.download
    )
    print(str(archive_path))
    return 0


def prepare_llvm_provider(args: argparse.Namespace) -> int:
    """Download (once) and verify the pinned provider archive, then print it.

    CI runs this per target and feeds the printed path straight to
    `stage-release --llvm-provider-archive`. The archive itself is the
    release input; `--extract-to` only unpacks the manifest-declared members
    for human inspection and is never consulted by packaging.
    """
    manifest_path = Path(args.manifest).resolve()
    manifest = load_manifest(manifest_path)
    target = manifest["target"]
    spec = llvm_code_generator_spec(manifest)
    if spec is None or spec.get("status") != "present":
        fail(f"target '{target}' declares no packaged LLVM code generator to prepare")
    if not isinstance(spec.get("archive"), dict):
        fail(
            f"target '{target}' embeds its LLVM code generator in the toolchain archive; it has "
            "no separately pinned provider archive to prepare"
        )
    declared = llvm_provider_declared_files(spec)

    download_dir = Path(args.download_dir).resolve()
    description = f"pinned LLVM provider archive for {target} ({manifest_path.name})"
    if args.archive:
        archive_path = Path(args.archive).resolve()
        verify_supplied_archive(archive_path, spec["archive"], description)
    else:
        archive_path, _ = resolve_declared_archive(
            spec["archive"], download_dir, description, not args.no_download
        )

    if args.extract_to:
        inspection = Path(args.extract_to).resolve()
        with temporary_staging_dir(inspection.parent, f".provider-inspect-{target}-") as workspace:
            extracted = workspace / "provider"
            extract_llvm_provider_archive(spec, archive_path, extracted, description)
            ensure_clean_dir(inspection)
            copy_declared_provider_files(extracted, declared, inspection, description)
        print(
            f"Extracted {len(declared)} declared provider file(s) to {inspection} for "
            "inspection only; staging consumes the archive itself",
            file=sys.stderr,
        )

    print(str(archive_path))
    return 0


def assert_toolchain_matches_manifest(
    root: Path, manifest: dict, variant: dict, description: str
) -> None:
    """The staged tree really is this target's declared toolchain.

    File names alone prove nothing, so every check here is driven by the
    manifest: the declared runtime compiler and archiver must exist as real,
    non-empty, executable files at their declared paths, and the target the
    manifest describes must be the target being packaged.
    """
    if manifest.get("target") != variant["target"]:
        fail(
            f"{description}: manifest describes target {manifest.get('target')!r}, but this "
            f"package targets {variant['target']!r}"
        )
    runtime = manifest["toolchain"].get("runtime")
    if not isinstance(runtime, dict):
        fail(f"{description}: manifest declares no runtime toolchain to verify")
    for role in ("compiler", "archiver"):
        relative = safe_relative_path(runtime[role]["path"])
        candidate = root / relative
        if not candidate.is_file():
            fail(
                f"{description}: the archive does not provide the manifest-declared {role} "
                f"'{relative.as_posix()}'"
            )
        if candidate.stat().st_size == 0:
            fail(f"{description}: the manifest-declared {role} '{relative.as_posix()}' is empty")
        # PE binaries carry no Unix mode inside a zip, so the executable bit
        # is only meaningful for the archives (and hosts) that record it.
        if os.name != "nt" and relative.suffix.lower() != ".exe":
            if not os.access(candidate, os.X_OK):
                fail(
                    f"{description}: the manifest-declared {role} '{relative.as_posix()}' is "
                    "not executable"
                )


def assert_c_toolchain_provider_layout(root: Path, manifest: dict, description: str) -> None:
    """A C package's toolchain must be exactly the base archive.

    Windows' llvm-mingw archive integrates libLLVM — `clang.exe` links
    against it — so it has to survive pruning. Linux overlays its provider
    from a *separate* pinned archive, so the base toolchain archive must not
    already contain it; if it does, the supplied archive is not the base
    archive this manifest pins.
    """
    overlaid = llvm_provider_staged_paths(manifest)
    if overlaid:
        present = sorted(
            relative for relative in overlaid if (root / safe_relative_path(relative)).exists()
        )
        if present:
            fail(
                f"{description}: the base toolchain archive already contains the separately "
                f"overlaid LLVM provider ({', '.join(present)}); a C package stages the base "
                "archive only"
            )
        return
    spec = llvm_code_generator_spec(manifest)
    if spec is not None and spec.get("status") == "present":
        # Integrated provider: the C compiler in this package links against
        # it, so its absence is a broken package, not a smaller one.
        verify_llvm_code_generator(root, manifest)


def copy_trusted_toolchain_tree(
    source_root: Path, destination_root: Path, excluded: set[str], description: str
) -> list[str]:
    """Copy an extracted toolchain into the bundle, entry by entry.

    Explicit rather than `copytree`: every entry is classified before it is
    copied, the manifest-declared LLVM provider paths are skipped, and a
    symlink that resolves outside the extracted tree is an error instead of
    a silent copy of whatever the build host happens to have there.
    """
    skipped: list[str] = []
    root = source_root.resolve()
    for item in sorted(source_root.rglob("*"), key=lambda path: path.as_posix()):
        relative = item.relative_to(source_root).as_posix()
        if relative in excluded:
            skipped.append(relative)
            continue
        if item.is_symlink():
            immediate = Path(os.path.normpath(item.parent / os.readlink(item)))
            resolved = Path(os.path.realpath(item))
            for candidate in (immediate, resolved):
                try:
                    candidate.relative_to(root)
                except ValueError:
                    fail(
                        f"{description}: staged entry '{relative}' is a symlink leaving the "
                        "extracted toolchain"
                    )
            if resolved.is_dir():
                # Directory links are not walked: the tree is copied as
                # files, and following one would either duplicate the whole
                # toolchain (sysroots ship `usr -> .`) or loop.
                print(
                    f"note: {description}: skipping directory symlink '{relative}'",
                    file=sys.stderr,
                )
                continue
            if not resolved.is_file():
                # A link whose in-tree target pruning removed: it can only
                # ever have named something from this archive, so dropping
                # it cannot leak host content.
                continue
            shutil.copy2(resolved, _prepared_destination(destination_root / Path(relative)))
            continue
        if item.is_dir():
            continue
        if not item.is_file():
            fail(f"{description}: staged entry '{relative}' is not a regular file")
        shutil.copy2(item, _prepared_destination(destination_root / Path(relative)))
    return skipped


def _prepared_destination(path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def stage_c_toolchain_component(
    bundle_dir: Path, variant: dict, toolchain_archive: Path
) -> dict:
    """Stage the pinned C toolchain from its authenticated source archive.

    The archive — not a directory somebody prepared earlier — is the
    authority: it is checked against the digest this repository's manifest
    pins, extracted member by member into a scratch directory, pruned by the
    manifest's own rules, checked to actually be this target's toolchain,
    and only then copied into the sibling `toolchain/` directory the
    compiler's secure lookup expects.
    """
    toolchain_archive = Path(toolchain_archive)
    manifest_path = variant.get("toolchain_manifest_path")
    if manifest_path is None:
        fail(f"target '{variant['target']}' declares no toolchain manifest to stage")
    manifest_path = Path(manifest_path)
    manifest = load_manifest(manifest_path)
    if manifest["stage"]["root"] != "toolchain":
        fail(
            f"manifest stage root '{manifest['stage']['root']}' does not match the sibling "
            "toolchain layout the release contract requires"
        )
    description = (
        f"pinned C toolchain archive for {variant['target']} "
        f"({manifest_path.name} -> '{toolchain_archive.name}')"
    )
    destination = bundle_dir / "toolchain"
    # A C package ships the complete C toolchain and none of the separately
    # overlaid LLVM provider: the excluded set comes from the manifest's own
    # declaration, never from a name guess.
    excluded = llvm_provider_staged_paths(manifest)
    with temporary_staging_dir(
        bundle_dir.parent, f".c-toolchain-{variant['target']}-"
    ) as workspace:
        extracted = workspace / "toolchain"
        verified = prepare_toolchain_from_archive(
            manifest, toolchain_archive, extracted, description
        )
        assert_toolchain_matches_manifest(extracted, manifest, variant, description)
        assert_c_toolchain_provider_layout(extracted, manifest, description)
        copy_trusted_toolchain_tree(extracted, destination, excluded, description)
        copied_licenses = copy_license_files(
            extracted,
            bundle_dir / "LICENSES" / "toolchain",
            manifest["stage"].get("license_globs", []),
        )
    copy_path(manifest_path, bundle_dir / Path(variant["toolchain_manifest"]).name)
    write_provenance_file(
        bundle_dir / "LICENSES" / "toolchain-source.txt",
        manifest,
        copied_licenses,
        verified,
        manifest_path.name,
    )
    return {
        "vendor": manifest["toolchain"]["vendor"],
        "version": manifest["toolchain"]["version"],
        "source_manifest": manifest_path.name,
        "source_archive": {
            "url": manifest["toolchain"]["archive"]["url"],
            "type": manifest["toolchain"]["archive"]["type"],
            "digest": {"algorithm": verified["algorithm"], "value": verified["value"]},
            "size": verified["size"],
        },
    }


C_COMPILER_EXECUTABLE_NAMES = (
    "clang",
    "clang.exe",
    "clang++",
    "clang++.exe",
    "cc",
    "cc.exe",
    "gcc",
    "gcc.exe",
    "g++",
    "g++.exe",
    "cl.exe",
    "cpp",
    "cpp.exe",
)


def assert_object_package_is_toolchain_free(bundle_dir: Path, variant: dict) -> None:
    """Belt-and-braces denylist: an object package may not contain a C
    compiler, C headers, or a sysroot, whatever the allowlists staged."""
    for path in sorted(bundle_dir.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(bundle_dir).as_posix()
        name = path.name
        if name in C_COMPILER_EXECUTABLE_NAMES:
            fail(
                f"{variant['target']}/{variant['backend']} package must be toolchain-free, but "
                f"it contains a C compiler executable: {relative}"
            )
        if name.endswith(".h") and not relative.startswith("native-link/"):
            fail(
                f"{variant['target']}/{variant['backend']} package must be toolchain-free, but "
                f"it contains a C header: {relative}"
            )
        if "/sysroot/" in f"/{relative}" or "/include/" in f"/{relative}":
            fail(
                f"{variant['target']}/{variant['backend']} package must be toolchain-free, but "
                f"it contains toolchain headers/sysroot: {relative}"
            )
    for forbidden in ("native-runtime", "cross-linkers"):
        if (bundle_dir / forbidden).exists():
            fail(
                f"{variant['target']}/{variant['backend']} package must not ship '{forbidden}/'"
            )
    hosted = bundle_dir / "build" / "runtime-archives" / variant["target"] / "libosc_runtime_hosted.a"
    if hosted.exists():
        fail("an object package must not ship the hosted runtime archive")


def assert_c_package_has_no_object_payload(bundle_dir: Path, variant: dict) -> None:
    for forbidden in (NATIVE_LINK_DIR_NAME, "build", "native-runtime", "cross-linkers"):
        if (bundle_dir / forbidden).exists():
            fail(
                f"{variant['target']}/{variant['backend']} is a C package and must not ship "
                f"'{forbidden}/'"
            )


REMOVED_STAGING_INPUTS = {
    "toolchain_dir": (
        "--toolchain-dir",
        "--toolchain-archive",
        "a prepared toolchain directory is not an authenticated release input: its contents "
        "cannot be checked against the digest the toolchain manifest pins",
    ),
    "llvm_provider_dir": (
        "--llvm-provider-dir",
        "--llvm-provider-archive",
        "a prepared provider directory carried its own provenance record, which is "
        "self-asserted: only the pinned source archive can be authenticated",
    ),
}


class RemovedOption(argparse.Action):
    """Refuse a removed input instead of quietly accepting it."""

    def __init__(self, option_strings, dest, replacement: str = "", reason: str = "", **kwargs):
        super().__init__(option_strings, dest, **kwargs)
        self.replacement = replacement
        self.reason = reason

    def __call__(self, parser, namespace, values, option_string=None) -> None:
        fail(
            f"{option_string} has been removed: {self.reason}. Pass {self.replacement} "
            "instead (see 'release_tools.py resolve-archive')."
        )


def reject_removed_staging_inputs(args: argparse.Namespace) -> None:
    """Also refuse the removed inputs when the namespace is built directly."""
    for dest, (option, replacement, reason) in REMOVED_STAGING_INPUTS.items():
        if getattr(args, dest, None):
            fail(
                f"{option} has been removed from release staging: {reason}. "
                f"Pass {replacement} with the pinned source archive instead."
            )


def stage_release(args: argparse.Namespace) -> int:
    contract_path = Path(args.contract).resolve()
    contract = load_release_contract(contract_path)
    variant = resolve_release_variant(contract, contract_path, args.target, args.backend)
    target = variant["target"]
    platform = variant["platform"]
    version = args.version
    reject_removed_staging_inputs(args)

    bundle_name = render_release_template(
        variant["archive_root_template"], version, "archive_root_template"
    )
    archive_name = render_release_template(
        variant["archive_name_template"], version, "archive_name_template"
    )
    output_dir = Path(args.output_dir).resolve()
    bundle_dir = output_dir / "stage" / bundle_name
    ensure_clean_dir(bundle_dir)

    binary_source = Path(args.binary).resolve()
    if not binary_source.is_file():
        fail(f"binary not found: {binary_source}")
    binary_destination = bundle_dir / variant["binary_name"]
    copy_path(binary_source, binary_destination)
    if platform != "windows":
        binary_destination.chmod(0o755)
    component_digests = {variant["binary_name"]: compute_digest(binary_destination, "sha256")}

    install_source = REPO_ROOT / "scripts" / (
        "install-oscan.ps1" if platform == "windows" else "install-oscan.sh"
    )
    install_destination = bundle_dir / ("install.ps1" if platform == "windows" else "install.sh")
    copy_path(install_source, install_destination)
    if platform != "windows":
        install_destination.chmod(0o755)

    sidecar = None
    if "direct_link_sidecar" in variant["components"]:
        if not args.native_link_dir:
            fail(
                f"staging {target}/{variant['backend']} needs --native-link-dir (the prepared "
                "native-link asset set for this target)"
            )
        sidecar = stage_direct_link_sidecar(
            bundle_dir, variant, Path(args.native_link_dir).resolve()
        )
        component_digests[f"{NATIVE_LINK_DIR_NAME}/{NATIVE_LINK_MANIFEST_NAME}"] = sidecar[
            "digest"
        ]

    if "runtime_archives" in variant["components"]:
        runtime_archive_dir = (
            Path(args.runtime_archive_dir).resolve()
            if args.runtime_archive_dir
            else REPO_ROOT / "build" / "runtime-archives" / target
        )
        component_digests.update(
            stage_freestanding_runtime_archives(bundle_dir, variant, runtime_archive_dir)
        )

    provider_info = None
    if "llvm_provider" in variant["components"]:
        provider_info = stage_llvm_provider_component(
            bundle_dir,
            variant,
            sidecar,
            Path(args.llvm_provider_archive).resolve()
            if args.llvm_provider_archive
            else None,
        )

    toolchain_info = None
    if "c_toolchain" in variant["components"]:
        if not args.toolchain_archive:
            fail(
                f"staging {target}/{variant['backend']} needs --toolchain-archive (the "
                "already downloaded, digest-pinned C toolchain source archive for this target)"
            )
        toolchain_info = stage_c_toolchain_component(
            bundle_dir, variant, Path(args.toolchain_archive).resolve()
        )

    if variant.get("note_file_path"):
        copy_path(Path(variant["note_file_path"]), bundle_dir / Path(variant["note_file"]).name)

    write_install_readme(bundle_dir / "README-install.txt", variant, archive_name)
    if provider_info is not None:
        component_digests["llvm_provider"] = provider_info
    if toolchain_info is not None:
        component_digests["c_toolchain"] = toolchain_info
    write_package_metadata(
        bundle_dir / PACKAGE_METADATA_NAME,
        variant,
        version,
        archive_name,
        bundle_name,
        component_digests,
    )

    if variant["backend_kind"] == "object":
        assert_object_package_is_toolchain_free(bundle_dir, variant)
    else:
        assert_c_package_has_no_object_payload(bundle_dir, variant)

    archive_path = output_dir / archive_name
    archive_path.parent.mkdir(parents=True, exist_ok=True)
    if variant["archive_format"] == "zip":
        create_zip_archive(bundle_dir, archive_path)
    else:
        create_tar_archive(bundle_dir, archive_path, variant["archive_format"])

    print(str(archive_path))
    return 0


CI_RUNNERS = {
    "windows": "windows-latest",
    "linux": "ubuntu-latest",
    "macos": "macos-15-intel",
}
CI_PLATFORM_LABELS = {"windows": "Windows", "linux": "Linux", "macos": "macOS"}


def ci_target_matrix(contract: dict, version: str) -> list[dict]:
    """The release workflow's package matrix: one entry per *target*.

    Deliberately not one entry per (target, backend): every backend variant
    of a target reuses the same pinned toolchain download, the same
    freestanding runtime archives, and the same native-link sidecar, so
    fanning out per backend would repeat the most expensive work in the
    release three times per platform. The per-target entry therefore carries
    the canonical backend list, and the job builds/assembles/smokes them
    sequentially.
    """
    entries: list[dict] = []
    rendered_archives: dict[str, str] = {}
    for target in sorted(contract["variants"]):
        target_spec = contract["variants"][target]
        platform = target.split("-", 1)[0]
        if platform not in CI_RUNNERS:
            fail(f"release contract target '{target}' has no CI runner mapping")
        declared = target_spec["backends"]
        unknown = sorted(set(declared) - set(CANONICAL_BACKENDS))
        if unknown:
            fail(
                f"release contract target '{target}' declares non-canonical backend(s): "
                f"{', '.join(unknown)}"
            )
        backends = [backend for backend in CANONICAL_BACKENDS if backend in declared]
        if not backends:
            fail(f"release contract target '{target}' declares no backends")

        archives: list[str] = []
        runtime_profiles: list[str] = []
        needs_toolchain_archive = False
        needs_provider_archive = False
        needs_native_link = False
        for backend in backends:
            variant = declared[backend]
            archive_name = render_release_template(
                variant["archive_name_template"], version, "archive_name_template"
            )
            if archive_name in rendered_archives:
                fail(
                    f"release archive name '{archive_name}' is produced by both "
                    f"{rendered_archives[archive_name]} and {target}/{backend}"
                )
            rendered_archives[archive_name] = f"{target}/{backend}"
            archives.append(archive_name)
            components = variant["components"]
            if "c_toolchain" in components:
                needs_toolchain_archive = True
            if "direct_link_sidecar" in components:
                needs_native_link = True
            if (
                "llvm_provider" in components
                and variant.get("llvm_provider_source") == "toolchain-manifest"
            ):
                needs_provider_archive = True
            for profile in variant["runtime_profiles"]:
                if profile not in runtime_profiles:
                    runtime_profiles.append(profile)

        # The base toolchain archive is also what the runtime archives are
        # compiled with and what the native-link sidecar is cut from, so a
        # target needs it whenever it needs either of those.
        needs_base_toolchain = (
            needs_toolchain_archive or needs_native_link or bool(runtime_profiles)
        )
        entries.append(
            {
                "target": target,
                "label": f"{CI_PLATFORM_LABELS[platform]} {target.split('-', 1)[1]}",
                "os": CI_RUNNERS[platform],
                "binary_name": target_spec["binary_name"],
                "binary_path": f"target/release/{target_spec['binary_name']}",
                "backends": ",".join(backends),
                "archives": ",".join(archives),
                "runtime_profiles": ",".join(runtime_profiles),
                "needs_base_toolchain": "true" if needs_base_toolchain else "false",
                "needs_native_link": "true" if needs_native_link else "false",
                "needs_provider_archive": "true" if needs_provider_archive else "false",
                "msi_backend": "llvm" if platform == "windows" and "llvm" in backends else "",
            }
        )
    if not entries:
        fail("the release contract publishes no packages")
    return entries


def ci_matrix_command(args: argparse.Namespace) -> int:
    contract = load_release_contract(Path(args.contract).resolve())
    entries = ci_target_matrix(contract, args.version)
    print(json.dumps({"include": entries}, separators=(",", ":"), sort_keys=True))
    return 0


def expected_package_metadata(variant: dict, version: str) -> dict:
    """What `oscan-package.json` must say for this variant at this version.

    Derived from the contract rather than restated, so a package whose
    metadata drifts from the contract that produced it fails instead of
    being described by whatever it happens to contain.
    """
    return {
        "schema_version": 1,
        "version": version,
        "target": variant["target"],
        "backend": variant["backend"],
        "available_backends": [variant["backend"]],
        "default_backend": variant["distribution_backend"],
        "cargo_feature": variant["cargo_feature"],
        "toolchain_free": variant["toolchain_free"],
        "components": list(variant["components"]),
        "runtime_profiles": list(variant["runtime_profiles"]),
        "archive_name": render_release_template(
            variant["archive_name_template"], version, "archive_name_template"
        ),
        "archive_root": render_release_template(
            variant["archive_root_template"], version, "archive_root_template"
        ),
    }


def _verify_package_metadata(root: Path, variant: dict, version: str | None) -> dict:
    """Check the package's own machine-readable record against the contract."""
    metadata_path = root / PACKAGE_METADATA_NAME
    if not metadata_path.is_file():
        fail(f"packaged bundle {root} is missing {PACKAGE_METADATA_NAME}")
    try:
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"cannot read {metadata_path}: {exc}")
    if version is None:
        version = metadata.get("version")
        if not isinstance(version, str) or not version:
            fail(f"{metadata_path} has no usable 'version' field")
    for key, expected in expected_package_metadata(variant, version).items():
        actual = metadata.get(key)
        if actual != expected:
            fail(
                f"{metadata_path} field '{key}' is {actual!r}, expected {expected!r} for "
                f"{variant['target']}/{variant['backend']}"
            )
    requirements = metadata.get("requirements")
    if not isinstance(requirements, dict):
        fail(f"{metadata_path} has no 'requirements' block")
    expected_bundled = "c_toolchain" in variant["components"]
    if requirements.get("bundled_c_toolchain") is not expected_bundled:
        fail(
            f"{metadata_path} claims bundled_c_toolchain="
            f"{requirements.get('bundled_c_toolchain')!r}, expected {expected_bundled!r}"
        )
    expected_host = (
        variant.get("required_host_toolchain") if variant.get("requires_host_compiler") else None
    )
    if requirements.get("host_c_toolchain") != expected_host:
        fail(
            f"{metadata_path} claims host_c_toolchain="
            f"{requirements.get('host_c_toolchain')!r}, expected {expected_host!r}"
        )
    return metadata


def _verify_sidecar_component(root: Path, variant: dict) -> dict:
    """Every native-link asset the packaged manifest declares must be
    present, at its declared subpath, with its declared content."""
    manifest_path = root / NATIVE_LINK_DIR_NAME / NATIVE_LINK_MANIFEST_NAME
    if not manifest_path.is_file():
        fail(
            f"{variant['target']}/{variant['backend']} declares the direct_link_sidecar "
            f"component, but {manifest_path} is missing"
        )
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"cannot read packaged native-link manifest {manifest_path}: {exc}")
    if manifest.get("target") != variant["target"]:
        fail(
            f"packaged native-link manifest {manifest_path} is staged for "
            f"{manifest.get('target')!r}, expected {variant['target']!r}"
        )
    linker = manifest.get("linker")
    if not isinstance(linker, dict) or not linker.get("install_subpath"):
        fail(f"packaged native-link manifest {manifest_path} declares no linker")
    for entry in [linker] + list(manifest.get("assets", [])):
        if not isinstance(entry, dict):
            fail(f"packaged native-link manifest {manifest_path} has a malformed entry")
        staged = root / NATIVE_LINK_DIR_NAME / safe_relative_path(entry["install_subpath"])
        if not staged.is_file():
            fail(
                f"packaged native-link asset '{entry.get('name')}' is missing from the installed "
                f"package (expected {staged})"
            )
        actual = compute_digest(staged, "sha256")
        if actual.lower() != str(entry.get("sha256", "")).lower():
            fail(
                f"packaged native-link asset '{entry.get('name')}' digest mismatch: manifest has "
                f"{entry.get('sha256')!r}, actual is {actual}"
            )
    return manifest


def _verify_runtime_archive_component(root: Path, variant: dict) -> None:
    """Exactly the declared freestanding archive/manifest pairs, at the one
    fixed executable-relative location a packaged compiler looks in."""
    target = variant["target"]
    runtime_contract = load_runtime_archive_contract(RUNTIME_ARCHIVE_CONTRACT_PATH)
    archive_root = root / "build" / "runtime-archives" / target
    if not archive_root.is_dir():
        fail(
            f"{target}/{variant['backend']} declares runtime_archives, but "
            f"{archive_root} does not exist; the compiler resolves its runtime archives "
            "relative to its own executable and never auto-builds them"
        )
    expected_files: set[str] = set()
    for profile in variant["runtime_profiles"]:
        mode_spec = runtime_contract["modes"][profile]
        archive_path = archive_root / mode_spec["archive_name"]
        manifest_path = archive_root / mode_spec["manifest_name"]
        for path in (archive_path, manifest_path):
            if not path.is_file():
                fail(f"packaged bundle is missing runtime asset {path}")
        expected_files.add(archive_path.name)
        expected_files.add(manifest_path.name)
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            fail(f"cannot read packaged runtime manifest {manifest_path}: {exc}")
        if manifest.get("target") != target or manifest.get("mode") != profile:
            fail(
                f"packaged runtime manifest {manifest_path} identifies "
                f"{manifest.get('target')}/{manifest.get('mode')}, expected {target}/{profile}"
            )
        actual = compute_digest(archive_path, "sha256")
        if manifest.get("sha256") != actual:
            fail(
                f"packaged runtime archive digest mismatch for {archive_path}: manifest has "
                f"{manifest.get('sha256')!r}, actual is {actual}"
            )
    unexpected = sorted(
        entry.name for entry in archive_root.iterdir() if entry.name not in expected_files
    )
    if unexpected:
        fail(
            f"packaged runtime archive directory {archive_root} contains files this variant "
            f"does not declare: {', '.join(unexpected)}"
        )


def _verify_llvm_provider_component(root: Path, variant: dict, sidecar: dict | None) -> None:
    source = variant.get("llvm_provider_source")
    if source == "direct-link-sidecar":
        asset_name = variant["llvm_provider_asset"]
        if sidecar is None:
            fail(
                f"{variant['target']}/{variant['backend']} shares its LLVM provider with the "
                "native-link sidecar, but no sidecar was packaged"
            )
        declared = [
            entry
            for entry in sidecar.get("assets", [])
            if entry.get("role") == "linker_runtime" and entry.get("name") == asset_name
        ]
        if not declared:
            fail(
                f"packaged native-link manifest does not declare '{asset_name}' as a "
                "linker_runtime asset, so this package has no LLVM code generator to load"
            )
        staged = root / NATIVE_LINK_DIR_NAME / safe_relative_path(declared[0]["install_subpath"])
        if not staged.is_file():
            fail(f"packaged LLVM code generator '{asset_name}' is missing (expected {staged})")
        if (root / "toolchain").exists():
            fail(
                f"{variant['target']}/{variant['backend']} shares the sidecar's LLVM provider "
                "and must not ship a separate toolchain/ directory"
            )
        return
    manifest_path = variant.get("toolchain_manifest_path")
    if manifest_path is None:
        fail("a manifest-sourced LLVM provider needs the target's toolchain manifest")
    manifest = load_manifest(Path(manifest_path))
    verify_llvm_code_generator(root / "toolchain", manifest)
    provenance = root / "LICENSES" / "llvm-provider" / PROVIDER_PROVENANCE_NAME
    if not provenance.is_file():
        fail(f"packaged LLVM provider is missing its provenance evidence ({provenance})")


def _verify_c_toolchain_component(root: Path, variant: dict, contract: dict) -> None:
    toolchain_root = root / "toolchain"
    if not toolchain_root.is_dir():
        fail(
            f"{variant['target']}/{variant['backend']} declares the c_toolchain component, but "
            f"{toolchain_root} does not exist"
        )
    manifest_path = variant.get("toolchain_manifest_path")
    if manifest_path is None:
        fail(f"target '{variant['target']}' declares no toolchain manifest")
    manifest = load_manifest(Path(manifest_path))
    runtime = manifest["toolchain"]["runtime"]
    for role in ("compiler", "archiver", "linker"):
        tool = toolchain_root / safe_relative_path(runtime[role]["path"])
        if not tool.is_file():
            fail(
                f"packaged C toolchain is missing its {role}: {tool} (declared by "
                f"{Path(manifest_path).name})"
            )
    lookup = contract["lookup_contract"][variant["platform"]]
    found = [
        str(candidate.relative_to(root).as_posix())
        for bin_dir in lookup["bin_directories"]
        for name in lookup["compiler_names"]
        for candidate in [root / PurePosixPath(bin_dir.replace("\\", "/")) / name]
        if candidate.is_file()
    ]
    if not found:
        fail(
            f"packaged C toolchain has no compiler where the contract's lookup finds one "
            f"(searched {', '.join(lookup['bin_directories'])} for "
            f"{', '.join(lookup['compiler_names'])})"
        )
    excluded = sorted(
        relative
        for relative in llvm_provider_staged_paths(manifest)
        if (toolchain_root / safe_relative_path(relative)).exists()
    )
    if excluded:
        fail(
            f"{variant['target']}/{variant['backend']} is a C package and must not ship the "
            f"separately overlaid LLVM provider payload: {', '.join(excluded)}"
        )


def _verify_absent_components(root: Path, variant: dict) -> None:
    """The other half of the contract: what a variant must *not* contain."""
    components = variant["components"]
    if "direct_link_sidecar" not in components and (root / NATIVE_LINK_DIR_NAME).exists():
        fail(
            f"{variant['target']}/{variant['backend']} does not declare a native-link sidecar, "
            f"but the package contains '{NATIVE_LINK_DIR_NAME}/'"
        )
    if not variant["runtime_profiles"] and (root / "build").exists():
        fail(
            f"{variant['target']}/{variant['backend']} ships no runtime archives, but the "
            "package contains 'build/'"
        )
    if "c_toolchain" not in components and variant.get("llvm_provider_source") != (
        "toolchain-manifest"
    ):
        if (root / "toolchain").exists():
            fail(
                f"{variant['target']}/{variant['backend']} declares no toolchain payload, but "
                "the package contains 'toolchain/'"
            )
    for forbidden in ("native-runtime", "cross-linkers", "runtime"):
        if (root / forbidden).exists():
            fail(f"{variant['target']}/{variant['backend']} must not ship '{forbidden}/'")


def _verify_object_package_payload(root: Path, variant: dict) -> None:
    """An object package carries no C-toolchain payload of any kind: no
    compiler, no headers, no sysroot, no C sources, no hosted runtime."""
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(root).as_posix()
        if path.name in C_COMPILER_EXECUTABLE_NAMES:
            fail(
                f"{variant['target']}/{variant['backend']} must be toolchain-free, but the "
                f"installed package contains a C compiler executable: {relative}"
            )
        if path.name.endswith(".h") and not relative.startswith(f"{NATIVE_LINK_DIR_NAME}/"):
            fail(
                f"{variant['target']}/{variant['backend']} must be toolchain-free, but the "
                f"installed package contains a C header: {relative}"
            )
        if path.name.endswith(".c"):
            fail(
                f"{variant['target']}/{variant['backend']} must ship no runtime C source, but "
                f"the installed package contains: {relative}"
            )
        if "/sysroot/" in f"/{relative}" or "/include/" in f"/{relative}":
            fail(
                f"{variant['target']}/{variant['backend']} must be toolchain-free, but the "
                f"installed package contains toolchain headers/sysroot: {relative}"
            )
    hosted = (
        root / "build" / "runtime-archives" / variant["target"] / "libosc_runtime_hosted.a"
    )
    if hosted.exists():
        fail("an object package must not ship the hosted runtime archive")


def _verify_c_package_payload(root: Path, variant: dict) -> None:
    for forbidden in (NATIVE_LINK_DIR_NAME, "build"):
        if (root / forbidden).exists():
            fail(
                f"{variant['target']}/{variant['backend']} is a C package and must not ship "
                f"'{forbidden}/'"
            )
    if variant["platform"] == "macos" and (root / "toolchain").exists():
        fail(
            "the macOS C package relies on the host Apple Command Line Tools and must not "
            "bundle a C toolchain"
        )


def verify_package_layout(
    root: Path,
    contract_path: Path,
    target: str,
    backend: str,
    version: str | None = None,
    archive: Path | None = None,
    expect_archive_root_name: bool = False,
) -> dict:
    """Assert that an extracted or installed package is exactly the variant
    the contract describes — nothing missing, nothing extra.

    This is the assertion half of the release smoke test, kept here so the
    PowerShell and shell smoke scripts check identical facts and so it can
    be tested hermetically against staged fixture packages.
    """
    contract = load_release_contract(contract_path)
    variant = resolve_release_variant(contract, contract_path, target, backend)
    if not root.is_dir():
        fail(f"package root '{root}' is not a directory")

    if archive is not None:
        suffix = ARCHIVE_SUFFIXES[variant["archive_format"]]
        if not archive.name.endswith(suffix):
            fail(
                f"archive '{archive}' does not carry the '{suffix}' format the contract declares "
                f"for {target}"
            )
        if version is not None:
            expected_name = render_release_template(
                variant["archive_name_template"], version, "archive_name_template"
            )
            if archive.name != expected_name:
                fail(f"archive '{archive.name}' is not the contract name '{expected_name}'")

    metadata = _verify_package_metadata(root, variant, version)
    version = metadata["version"]
    if expect_archive_root_name and root.name != metadata["archive_root"]:
        fail(
            f"extracted bundle directory '{root.name}' is not the contract archive root "
            f"'{metadata['archive_root']}'"
        )

    binary = root / variant["binary_name"]
    if not binary.is_file():
        fail(f"package is missing its compiler binary: {binary}")
    if binary.stat().st_size == 0:
        fail(f"packaged compiler binary is empty: {binary}")
    if variant["platform"] != "windows" and not os.access(binary, os.X_OK):
        fail(f"packaged compiler binary is not executable: {binary}")

    sidecar = None
    if "direct_link_sidecar" in variant["components"]:
        sidecar = _verify_sidecar_component(root, variant)
    if "runtime_archives" in variant["components"]:
        _verify_runtime_archive_component(root, variant)
    if "llvm_provider" in variant["components"]:
        _verify_llvm_provider_component(root, variant, sidecar)
    if "c_toolchain" in variant["components"]:
        _verify_c_toolchain_component(root, variant, contract)
    _verify_absent_components(root, variant)

    if variant["backend_kind"] == "object":
        _verify_object_package_payload(root, variant)
    else:
        _verify_c_package_payload(root, variant)

    if variant.get("note_file") and not (root / Path(variant["note_file"]).name).is_file():
        fail(f"package is missing the contract note file '{variant['note_file']}'")
    return metadata


def verify_package_layout_command(args: argparse.Namespace) -> int:
    metadata = verify_package_layout(
        Path(args.root).resolve(),
        Path(args.contract).resolve(),
        args.target,
        args.backend,
        version=args.version,
        archive=Path(args.archive).resolve() if args.archive else None,
        expect_archive_root_name=args.stage == "extracted",
    )
    print(
        f"{args.target}/{args.backend} {args.stage} package layout OK "
        f"(version {metadata['version']}, components: {', '.join(metadata['components'])})"
    )
    return 0


def list_variants_command(args: argparse.Namespace) -> int:
    contract = load_release_contract(Path(args.contract).resolve())
    matrix = release_variant_matrix(contract)
    if args.target:
        matrix = [entry for entry in matrix if entry["target"] == args.target]
    if args.backend:
        matrix = [entry for entry in matrix if entry["backend"] == args.backend]
    if not matrix:
        fail("no release variant matches the requested target/backend")
    print(json.dumps(matrix, indent=2, sort_keys=True))
    return 0


def validate_contract_command(args: argparse.Namespace) -> int:
    contract_path = Path(args.contract).resolve()
    contract = load_release_contract(contract_path)
    count = len(release_variant_matrix(contract))
    print(f"{contract_path}: schema 2 OK, {count} variants")
    return 0


def fetch_toolchain_command(args: argparse.Namespace) -> int:
    _, destination = fetch_toolchain(
        Path(args.manifest).resolve(),
        Path(args.download_dir).resolve(),
        Path(args.destination).resolve(),
    )
    print(str(destination))
    return 0


def verify_llvm_code_generator_command(args: argparse.Namespace) -> int:
    manifest = load_manifest(Path(args.manifest).resolve())
    verified = verify_llvm_code_generator(Path(args.toolchain_dir).resolve(), manifest)
    if verified is None:
        print(
            "this target's toolchain manifest declares no packaged LLVM code generator "
            "(--backend llvm is unavailable for it; --backend cranelift/c are unaffected)",
            file=sys.stderr,
        )
    else:
        print(str(verified))
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


def prepare_inprocess_link_assets(
    target: str,
    toolchain_dir: Path,
    toolchain_manifest_path: Path,
    runtime_archive_dir: Path,
    output_dir: Path,
) -> dict:
    """Stage only the bytes consumed by the in-process linker."""
    if target != "windows-x86_64":
        fail(
            "prepare-inprocess-link-assets currently supports only "
            f"windows-x86_64, not '{target}'"
        )
    if not toolchain_dir.is_dir():
        fail(f"toolchain directory not found: {toolchain_dir}")
    if not runtime_archive_dir.is_dir():
        fail(f"runtime archive directory not found: {runtime_archive_dir}")

    toolchain_manifest = load_manifest(toolchain_manifest_path)
    if toolchain_manifest["target"] != target:
        fail(
            f"toolchain manifest {toolchain_manifest_path} identifies target "
            f"'{toolchain_manifest['target']}', expected '{target}'"
        )

    ensure_clean_dir(output_dir)
    entries = []
    for profile in FREESTANDING_PROFILES:
        archive_name = f"libosc_runtime_{profile}.a"
        source = runtime_archive_dir / archive_name
        if not source.is_file():
            fail(
                f"runtime archive '{profile}' not found: {source}; build all "
                "freestanding profiles before staging the strict compiler"
            )
        sidecar = source.with_suffix(".json")
        if not sidecar.is_file():
            fail(f"runtime archive provenance manifest not found: {sidecar}")
        runtime_manifest = json.loads(sidecar.read_text(encoding="utf-8"))
        expected_sha256 = compute_digest(source, "sha256")
        runtime_toolchain = runtime_manifest.get("toolchain", {})
        if (
            runtime_manifest.get("schema_version") != 2
            or runtime_manifest.get("target") != target
            or runtime_manifest.get("mode") != profile
            or runtime_manifest.get("requires_libc") is not False
            or runtime_manifest.get("sha256") != expected_sha256
            or runtime_toolchain.get("vendor")
            != toolchain_manifest["toolchain"]["vendor"]
            or runtime_toolchain.get("version")
            != toolchain_manifest["toolchain"]["version"]
        ):
            fail(
                f"runtime archive provenance in {sidecar} does not match the "
                f"strict {target}/{profile} toolchain"
            )
        destination = output_dir / "runtime" / archive_name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        entries.append(
            {
                "role": "runtime_archive",
                "profile": profile,
                "name": archive_name,
                "install_subpath": destination.relative_to(output_dir).as_posix(),
                "size": destination.stat().st_size,
                "sha256": expected_sha256,
            }
        )

    asset_spec = EMBED_ASSET_SPECS[target]
    for spec in asset_spec["import_libs"]:
        staged = _stage_embed_asset(toolchain_dir, output_dir, spec)
        entries.append(
            {
                "role": "import_lib",
                "name": spec["name"],
                "lib": spec["lib"],
                "install_subpath": Path(spec["install_subpath"]).as_posix(),
                "size": staged["size"],
                "sha256": staged["sha256"],
            }
        )
    builtins_spec = asset_spec["compiler_builtins"]
    staged = _stage_embed_asset(toolchain_dir, output_dir, builtins_spec)
    entries.append(
        {
            "role": "compiler_builtins",
            "name": builtins_spec["name"],
            "install_subpath": Path(builtins_spec["install_subpath"]).as_posix(),
            "size": staged["size"],
            "sha256": staged["sha256"],
        }
    )

    out_manifest = {
        "schema_version": 1,
        "target": target,
        "toolchain": {
            "vendor": toolchain_manifest["toolchain"]["vendor"],
            "version": toolchain_manifest["toolchain"]["version"],
        },
        "assets": entries,
    }
    manifest_path = output_dir / INPROCESS_LINK_MANIFEST_NAME
    manifest_path.write_text(
        json.dumps(out_manifest, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return out_manifest


def prepare_inprocess_link_assets_command(args: argparse.Namespace) -> int:
    target = args.target
    toolchain_dir = Path(args.toolchain_dir).resolve()
    toolchain_manifest_path = (
        Path(args.toolchain_manifest).resolve()
        if args.toolchain_manifest
        else REPO_ROOT / "packaging" / "toolchains" / f"{target}.json"
    )
    runtime_archive_dir = Path(args.runtime_archive_dir).resolve()
    output_dir = Path(args.output_dir).resolve()
    prepare_inprocess_link_assets(
        target,
        toolchain_dir,
        toolchain_manifest_path,
        runtime_archive_dir,
        output_dir,
    )
    print(str(output_dir / INPROCESS_LINK_MANIFEST_NAME))
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
    stage.add_argument(
        "--backend",
        required=True,
        choices=list(CANONICAL_BACKENDS),
        help="which backend variant to package ('native' is only a deprecated CLI alias for cranelift and is never an artifact label)",
    )
    stage.add_argument("--contract", default=str(CONTRACT_PATH))
    stage.add_argument("--version", required=True)
    stage.add_argument("--binary", required=True)
    stage.add_argument("--output-dir", required=True)
    stage.add_argument(
        "--runtime-archive-dir",
        default=None,
        help="directory containing the target's prebuilt freestanding runtime archive/manifest pairs",
    )
    stage.add_argument(
        "--native-link-dir",
        default=None,
        help="prepared native-link asset directory (native-link-assets.json plus its assets at their install_subpaths) for object variants",
    )
    stage.add_argument(
        "--toolchain-archive",
        default=None,
        help="the pinned C toolchain source archive for the c variant; verified against the digest in this repository's toolchain manifest before anything is extracted (see 'resolve-archive')",
    )
    stage.add_argument(
        "--llvm-provider-archive",
        default=None,
        help="the pinned LLVM provider source archive for targets whose provider comes from the toolchain manifest rather than the native-link sidecar (see 'resolve-archive')",
    )
    stage.add_argument(
        "--toolchain-dir",
        nargs=1,
        action=RemovedOption,
        default=None,
        replacement="--toolchain-archive",
        reason=REMOVED_STAGING_INPUTS["toolchain_dir"][2],
        help=argparse.SUPPRESS,
    )
    stage.add_argument(
        "--llvm-provider-dir",
        nargs=1,
        action=RemovedOption,
        default=None,
        replacement="--llvm-provider-archive",
        reason=REMOVED_STAGING_INPUTS["llvm_provider_dir"][2],
        help=argparse.SUPPRESS,
    )
    stage.set_defaults(func=stage_release)

    prepare_provider = subparsers.add_parser(
        "prepare-llvm-provider",
        help="download (once) and digest-verify the pinned LLVM provider archive, printing the verified archive path for 'stage-release --llvm-provider-archive'",
    )
    prepare_provider.add_argument("--manifest", required=True)
    prepare_provider.add_argument("--download-dir", required=True)
    prepare_provider.add_argument(
        "--archive",
        default=None,
        help="verify an already downloaded archive instead of consulting the download cache",
    )
    prepare_provider.add_argument(
        "--no-download",
        action="store_true",
        help="fail instead of downloading when the archive is not already cached",
    )
    prepare_provider.add_argument(
        "--extract-to",
        default=None,
        help="also extract the manifest-declared provider files here for inspection; packaging always consumes the archive itself",
    )
    prepare_provider.add_argument(
        "--destination",
        nargs=1,
        action=RemovedOption,
        default=None,
        replacement="--extract-to (for inspection) plus --llvm-provider-archive when staging",
        reason=(
            "a prepared provider directory is no longer a staging input; the pinned archive is"
        ),
        help=argparse.SUPPRESS,
    )
    prepare_provider.set_defaults(func=prepare_llvm_provider)

    resolve = subparsers.add_parser(
        "resolve-archive",
        help="print the verified local path of a manifest-pinned archive (offline unless --download is given)",
    )
    resolve.add_argument("--manifest", required=True)
    resolve.add_argument("--download-dir", required=True)
    resolve.add_argument(
        "--component",
        choices=("toolchain", "llvm-provider"),
        default="toolchain",
    )
    resolve.add_argument(
        "--download",
        action="store_true",
        help="download the archive when it is not already cached (staging itself never downloads)",
    )
    resolve.set_defaults(func=resolve_archive_command)

    variants = subparsers.add_parser(
        "list-variants",
        help="print the target x backend release matrix this contract publishes",
    )
    variants.add_argument("--contract", default=str(CONTRACT_PATH))
    variants.add_argument("--target", default=None)
    variants.add_argument("--backend", default=None, choices=list(CANONICAL_BACKENDS))
    variants.set_defaults(func=list_variants_command)

    verify_layout = subparsers.add_parser(
        "verify-package-layout",
        help=(
            "assert an extracted or installed package is exactly the (target, backend) variant "
            "the contract describes: metadata fields, component presence, and component absence"
        ),
    )
    verify_layout.add_argument("--target", required=True)
    verify_layout.add_argument("--backend", required=True, choices=list(CANONICAL_BACKENDS))
    verify_layout.add_argument(
        "--root",
        required=True,
        help="the extracted archive-root directory, or the directory the package installed into",
    )
    verify_layout.add_argument(
        "--stage",
        choices=("extracted", "installed"),
        default="extracted",
        help="'extracted' additionally requires the directory to be named after the archive root",
    )
    verify_layout.add_argument(
        "--version",
        default=None,
        help="the release version; defaults to the version the package itself records",
    )
    verify_layout.add_argument(
        "--archive",
        default=None,
        help="also check this archive's name/suffix against the contract",
    )
    verify_layout.add_argument("--contract", default=str(CONTRACT_PATH))
    verify_layout.set_defaults(func=verify_package_layout_command)

    validate_contract = subparsers.add_parser(
        "validate-contract",
        help="validate the release contract without staging anything",
    )
    validate_contract.add_argument("--contract", default=str(CONTRACT_PATH))
    validate_contract.set_defaults(func=validate_contract_command)

    ci_matrix = subparsers.add_parser(
        "ci-matrix",
        help=(
            "print the release workflow's package matrix (one entry per target, each carrying "
            "that target's canonical backend list and the prepared inputs it needs)"
        ),
    )
    ci_matrix.add_argument("--contract", default=str(CONTRACT_PATH))
    ci_matrix.add_argument(
        "--version",
        required=True,
        help="the release version, used to render each variant's archive name",
    )
    ci_matrix.set_defaults(func=ci_matrix_command)

    verify_llvm = subparsers.add_parser(
        "verify-llvm-code-generator",
        help="assert a staged toolchain contains the LLVM code generator its manifest declares",
    )
    verify_llvm.add_argument("--manifest", required=True)
    verify_llvm.add_argument("--toolchain-dir", required=True)
    verify_llvm.set_defaults(func=verify_llvm_code_generator_command)

    checksums = subparsers.add_parser("write-checksums")
    checksums.add_argument("--output", required=True)
    checksums.add_argument("files", nargs="+")
    checksums.set_defaults(func=write_checksums)

    detect_target = subparsers.add_parser("detect-host-target")
    detect_target.set_defaults(func=detect_host_target_command)

    runtime_archive = subparsers.add_parser("build-runtime-archive")
    runtime_archive.add_argument("--target", default=None, help="e.g. linux-x86_64, windows-x86_64; defaults to the host platform")
    runtime_archive.add_argument(
        "--mode",
        choices=[
            "hosted",
            "freestanding",
            "freestanding_gfx",
            "freestanding_core",
            "all",
        ],
        default="all",
    )
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

    prepare_inprocess = subparsers.add_parser(
        "prepare-inprocess-link-assets",
        help=(
            "stage runtime archives/import libraries/compiler-builtins for a "
            "strict no-extraction in-process linker build"
        ),
    )
    prepare_inprocess.add_argument("--target", required=True)
    prepare_inprocess.add_argument("--toolchain-dir", required=True)
    prepare_inprocess.add_argument("--toolchain-manifest", default=None)
    prepare_inprocess.add_argument("--runtime-archive-dir", required=True)
    prepare_inprocess.add_argument("--output-dir", required=True)
    prepare_inprocess.set_defaults(func=prepare_inprocess_link_assets_command)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
