#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

TARGET=""
VERSION=""
BINARY_PATH=""
OUTPUT_DIR="$REPO_ROOT/target/release-artifacts"
CONTRACT_PATH="$REPO_ROOT/packaging/toolchains/release-contract.json"

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
        *)
            echo "usage: $0 --target <windows-x86_64|linux-x86_64|macos-x86_64> --version <version> --binary <path> [--output-dir <path>] [--contract <path>]" >&2
            exit 1
            ;;
    esac
done

[ -n "$TARGET" ] || { echo "missing --target" >&2; exit 1; }
[ -n "$VERSION" ] || { echo "missing --version" >&2; exit 1; }
[ -n "$BINARY_PATH" ] || { echo "missing --binary" >&2; exit 1; }

python3 "$SCRIPT_DIR/release_tools.py" stage-release \
    --target "$TARGET" \
    --version "$VERSION" \
    --binary "$BINARY_PATH" \
    --output-dir "$OUTPUT_DIR" \
    --contract "$CONTRACT_PATH"
