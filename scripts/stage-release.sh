#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

TARGET=""
VERSION=""
BINARY_PATH=""
OUTPUT_DIR="$REPO_ROOT/target/release-artifacts"
CONTRACT_PATH="$REPO_ROOT/packaging/toolchains/release-contract.json"
RUNTIME_ARCHIVE_DIR=""
BACKEND=""
NATIVE_LINK_DIR=""
EMBEDDED_NOTICES_DIR=""
TOOLCHAIN_ARCHIVE=""
LLVM_PROVIDER_ARCHIVE=""

usage() {
    echo "usage: $0 --target <windows-x86_64|linux-x86_64|macos-x86_64> --backend <llvm|cranelift|c> --version <version> --binary <path> [--output-dir <path>] [--contract <path>] [--runtime-archive-dir <path>] [--native-link-dir <path>] [--embedded-notices-dir <path>] [--toolchain-archive <path>] [--llvm-provider-archive <path>]" >&2
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target)
            TARGET="$2"
            shift 2
            ;;
        --version)
            VERSION="$2"
            shift 2
            ;;
        --binary)
            BINARY_PATH="$2"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --contract)
            CONTRACT_PATH="$2"
            shift 2
            ;;
        --runtime-archive-dir)
            RUNTIME_ARCHIVE_DIR="$2"
            shift 2
            ;;
        --backend)
            BACKEND="$2"
            shift 2
            ;;
        --native-link-dir)
            NATIVE_LINK_DIR="$2"
            shift 2
            ;;
        --embedded-notices-dir)
            EMBEDDED_NOTICES_DIR="$2"
            shift 2
            ;;
        --toolchain-archive)
            TOOLCHAIN_ARCHIVE="$2"
            shift 2
            ;;
        --llvm-provider-archive)
            LLVM_PROVIDER_ARCHIVE="$2"
            shift 2
            ;;
        --toolchain-dir)
            echo "--toolchain-dir has been removed from release staging: a prepared toolchain directory cannot be checked against the digest the toolchain manifest pins. Pass --toolchain-archive with the pinned source archive instead." >&2
            exit 1
            ;;
        --llvm-provider-dir)
            echo "--llvm-provider-dir has been removed from release staging: its provenance record was self-asserted. Pass --llvm-provider-archive with the pinned source archive instead." >&2
            exit 1
            ;;
        *)
            usage
            exit 1
            ;;
    esac
done

[ -n "$TARGET" ] || { echo "missing --target" >&2; exit 1; }
[ -n "$BACKEND" ] || { echo "missing --backend (llvm|cranelift|c)" >&2; exit 1; }
[ -n "$VERSION" ] || { echo "missing --version" >&2; exit 1; }
[ -n "$BINARY_PATH" ] || { echo "missing --binary" >&2; exit 1; }

set -- "$SCRIPT_DIR/release_tools.py" stage-release \
    --target "$TARGET" \
    --backend "$BACKEND" \
    --version "$VERSION" \
    --binary "$BINARY_PATH" \
    --output-dir "$OUTPUT_DIR" \
    --contract "$CONTRACT_PATH"
[ -n "$RUNTIME_ARCHIVE_DIR" ] && set -- "$@" --runtime-archive-dir "$RUNTIME_ARCHIVE_DIR"
[ -n "$NATIVE_LINK_DIR" ] && set -- "$@" --native-link-dir "$NATIVE_LINK_DIR"
[ -n "$EMBEDDED_NOTICES_DIR" ] && set -- "$@" --embedded-notices-dir "$EMBEDDED_NOTICES_DIR"
[ -n "$TOOLCHAIN_ARCHIVE" ] && set -- "$@" --toolchain-archive "$TOOLCHAIN_ARCHIVE"
[ -n "$LLVM_PROVIDER_ARCHIVE" ] && set -- "$@" --llvm-provider-archive "$LLVM_PROVIDER_ARCHIVE"
python3 "$@"
