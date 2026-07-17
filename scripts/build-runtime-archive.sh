#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

TARGET=""
MODE="all"
CC_BIN=""
AR_BIN=""
TARGET_TRIPLE=""
SYSROOT=""
TOOLCHAIN_MANIFEST=""
OUT_DIR=""
CONTRACT_PATH="$REPO_ROOT/packaging/toolchains/runtime-archive-contract.json"
KEEP_OBJECTS=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target)
            TARGET="$2"
            shift 2
            ;;
        --mode)
            MODE="$2"
            shift 2
            ;;
        --cc)
            CC_BIN="$2"
            shift 2
            ;;
        --ar)
            AR_BIN="$2"
            shift 2
            ;;
        --target-triple)
            TARGET_TRIPLE="$2"
            shift 2
            ;;
        --sysroot)
            SYSROOT="$2"
            shift 2
            ;;
        --toolchain-manifest)
            TOOLCHAIN_MANIFEST="$2"
            shift 2
            ;;
        --out-dir)
            OUT_DIR="$2"
            shift 2
            ;;
        --contract)
            CONTRACT_PATH="$2"
            shift 2
            ;;
        --keep-objects)
            KEEP_OBJECTS="--keep-objects"
            shift 1
            ;;
        *)
            echo "usage: $0 [--target <name>] [--mode hosted|freestanding|all] [--cc <cc>] [--ar <ar>] [--target-triple <triple> --sysroot <path>] [--toolchain-manifest <path>] [--out-dir <path>] [--contract <path>] [--keep-objects]" >&2
            exit 1
            ;;
    esac
done

set -- "$SCRIPT_DIR/release_tools.py" build-runtime-archive --mode "$MODE" --contract "$CONTRACT_PATH"
[ -n "$TARGET" ] && set -- "$@" --target "$TARGET"
[ -n "$CC_BIN" ] && set -- "$@" --cc "$CC_BIN"
[ -n "$AR_BIN" ] && set -- "$@" --ar "$AR_BIN"
[ -n "$TARGET_TRIPLE" ] && set -- "$@" --target-triple "$TARGET_TRIPLE"
[ -n "$SYSROOT" ] && set -- "$@" --sysroot "$SYSROOT"
[ -n "$TOOLCHAIN_MANIFEST" ] && set -- "$@" --toolchain-manifest "$TOOLCHAIN_MANIFEST"
[ -n "$OUT_DIR" ] && set -- "$@" --out-dir "$OUT_DIR"
[ -n "$KEEP_OBJECTS" ] && set -- "$@" "$KEEP_OBJECTS"

python3 "$@"
