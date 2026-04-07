#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import os
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
    digest = archive.get("digest", {})
    for key in ("url", "type", "digest"):
        if key not in archive:
            fail(f"manifest {path} is missing toolchain.archive.{key}")
    for key in ("algorithm", "value"):
        if key not in digest:
            fail(f"manifest {path} is missing toolchain.archive.digest.{key}")
    stage = data["stage"]
    stage.setdefault("root", "toolchain")
    stage.setdefault("license_globs", [])
    stage.setdefault("wrappers", [])
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


def download_file(url: str, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
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


def extract_archive(archive_path: Path, archive_type: str, destination: Path, strip_components: int) -> None:
    temp_root = archive_path.parent / f".extract-{archive_path.stem}"
    ensure_clean_dir(temp_root)
    try:
        if os.name == "nt" and archive_type == "zip":
            result = subprocess.run(
                ["tar", "-xf", str(archive_path), "-C", str(temp_root)],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                text=True,
                check=False,
            )
            if result.returncode != 0:
                fail(f"failed to extract {archive_path.name}: {result.stderr.strip()}")
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


def fetch_toolchain(manifest_path: Path, download_dir: Path, destination: Path) -> tuple[dict, Path]:
    manifest = load_manifest(manifest_path)
    archive = manifest["toolchain"]["archive"]
    digest = archive["digest"]
    url = archive["url"]
    file_name = Path(urllib.parse.urlparse(url).path).name
    if not file_name:
        fail(f"cannot derive archive file name from {url}")
    download_path = download_dir / file_name
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

    strip_components = int(manifest["toolchain"].get("extract", {}).get("strip_components", 0))
    extract_archive(download_path, archive["type"], destination, strip_components)

    for wrapper in manifest["stage"].get("wrappers", []):
        create_wrapper(destination, wrapper)

    return manifest, destination


def write_install_readme(path: Path, target_spec: dict, asset_name: str) -> None:
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

    text = textwrap.dedent(
        f"""\
        Oscan release asset: {asset_name}
        Platform: {platform} {arch}
        Bundle type: {bundle_kind}

        {install_hint}
        {extra}

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
    digest = manifest["toolchain"]["archive"]["digest"]
    licenses = "\n".join(f"- {entry}" for entry in copied_licenses) or "- none matched configured globs"
    text = textwrap.dedent(
        f"""\
        Toolchain vendor: {manifest["toolchain"]["vendor"]}
        Toolchain version: {manifest["toolchain"]["version"]}
        Target: {manifest["target"]}
        Archive URL: {manifest["toolchain"]["archive"]["url"]}
        Archive digest ({digest["algorithm"]}): {digest["value"]}

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

    write_install_readme(
        bundle_dir / "README-install.txt",
        target_spec,
        archive_name,
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
    stage.set_defaults(func=stage_release)

    checksums = subparsers.add_parser("write-checksums")
    checksums.add_argument("--output", required=True)
    checksums.add_argument("files", nargs="+")
    checksums.set_defaults(func=write_checksums)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
