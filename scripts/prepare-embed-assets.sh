#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

TARGET=""
TOOLCHAIN_DIR=""
TOOLCHAIN_MANIFEST=""
OUTPUT_DIR=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target)
            TARGET="$2"
            shift 2
            ;;
        --toolchain-dir)
            TOOLCHAIN_DIR="$2"
            shift 2
            ;;
        --toolchain-manifest)
            TOOLCHAIN_MANIFEST="$2"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        *)
            echo "usage: $0 --target <name> --toolchain-dir <path> [--toolchain-manifest <path>] [--output-dir <path>]" >&2
            exit 1
            ;;
    esac
done

[ -n "$TARGET" ] || { echo "missing --target" >&2; exit 1; }
[ -n "$TOOLCHAIN_DIR" ] || { echo "missing --toolchain-dir" >&2; exit 1; }

set -- "$SCRIPT_DIR/release_tools.py" prepare-embed-assets --target "$TARGET" --toolchain-dir "$TOOLCHAIN_DIR"
[ -n "$TOOLCHAIN_MANIFEST" ] && set -- "$@" --toolchain-manifest "$TOOLCHAIN_MANIFEST"
[ -n "$OUTPUT_DIR" ] && set -- "$@" --output-dir "$OUTPUT_DIR"

python3 "$@"
