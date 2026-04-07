#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

MANIFEST_PATH=""
DESTINATION=""
DOWNLOAD_DIR="$REPO_ROOT/target/release-artifacts/downloads"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --manifest)
            MANIFEST_PATH="$2"
            shift 2
            ;;
        --destination)
            DESTINATION="$2"
            shift 2
            ;;
        --download-dir)
            DOWNLOAD_DIR="$2"
            shift 2
            ;;
        *)
            echo "usage: $0 --manifest <path> --destination <path> [--download-dir <path>]" >&2
            exit 1
            ;;
    esac
done

[ -n "$MANIFEST_PATH" ] || { echo "missing --manifest" >&2; exit 1; }
[ -n "$DESTINATION" ] || { echo "missing --destination" >&2; exit 1; }

python3 "$SCRIPT_DIR/release_tools.py" fetch-toolchain \
    --manifest "$MANIFEST_PATH" \
    --download-dir "$DOWNLOAD_DIR" \
    --destination "$DESTINATION"
