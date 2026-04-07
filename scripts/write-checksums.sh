#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

OUTPUT_PATH=""

if [ "$#" -lt 3 ]; then
    echo "usage: $0 --output <path> <file> [file...]" >&2
    exit 1
fi

if [ "$1" != "--output" ]; then
    echo "usage: $0 --output <path> <file> [file...]" >&2
    exit 1
fi

OUTPUT_PATH="$2"
shift 2

python3 "$SCRIPT_DIR/release_tools.py" write-checksums --output "$OUTPUT_PATH" "$@"
