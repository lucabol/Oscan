#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

MANIFEST_PATH=""
COMPONENT="toolchain"
DOWNLOAD_DIR="$REPO_ROOT/target/release-artifacts/downloads"
DOWNLOAD=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --manifest)
            MANIFEST_PATH="$2"
            shift 2
            ;;
        --component)
            COMPONENT="$2"
            shift 2
            ;;
        --download-dir)
            DOWNLOAD_DIR="$2"
            shift 2
            ;;
        --download)
            DOWNLOAD="1"
            shift
            ;;
        *)
            echo "usage: $0 --manifest <path> [--component <toolchain|llvm-provider|inprocess-llvm-sdk|inprocess-llvm-source>] [--download-dir <path>] [--download]" >&2
            exit 1
            ;;
    esac
done

[ -n "$MANIFEST_PATH" ] || { echo "missing --manifest" >&2; exit 1; }

set -- "$SCRIPT_DIR/release_tools.py" resolve-archive \
    --manifest "$MANIFEST_PATH" \
    --download-dir "$DOWNLOAD_DIR" \
    --component "$COMPONENT"
[ -n "$DOWNLOAD" ] && set -- "$@" --download
python3 "$@"
