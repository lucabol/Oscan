#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

TARGET=""
ARCHIVE_PATH=""
SCRATCH_DIR=""
CONTRACT_PATH="$REPO_ROOT/packaging/toolchains/release-contract.json"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target)
            TARGET="$2"
            shift 2
            ;;
        --archive)
            ARCHIVE_PATH="$2"
            shift 2
            ;;
        --scratch-dir)
            SCRATCH_DIR="$2"
            shift 2
            ;;
        --contract)
            CONTRACT_PATH="$2"
            shift 2
            ;;
        *)
            echo "usage: $0 --target <linux-x86_64|macos-x86_64> --archive <path> [--scratch-dir <path>] [--contract <path>]" >&2
            exit 1
            ;;
    esac
done

[ -n "$TARGET" ] || { echo "missing --target" >&2; exit 1; }
[ -n "$ARCHIVE_PATH" ] || { echo "missing --archive" >&2; exit 1; }

if [ -z "$SCRATCH_DIR" ]; then
    SCRATCH_DIR="$REPO_ROOT/target/release-smoke/$TARGET"
fi

eval "$(
    python3 - "$CONTRACT_PATH" "$TARGET" <<'PY'
import json
import shlex
import sys

contract_path, target = sys.argv[1], sys.argv[2]
with open(contract_path, encoding="utf-8") as handle:
    contract = json.load(handle)

spec = contract.get("bundled_targets", {}).get(target)
if spec is None:
    spec = contract.get("binary_only_targets", {}).get(target)
if spec is None:
    raise SystemExit(f"release contract does not define target '{target}'")

def emit(name: str, value: str) -> None:
    print(f"{name}={shlex.quote(value)}")

emit("BUNDLE_KIND", spec["bundle_kind"])
emit("ARCHIVE_FORMAT", spec["archive_format"])
emit("NOTE_FILE", spec.get("note_file", ""))
emit("REQUIRES_HOST_COMPILER", "1" if spec.get("requires_host_compiler") else "0")
emit("NATIVE_RUNTIME_MODES", ",".join(spec["native_runtime_modes"]))
emit("NATIVE_SMOKE_MODE", spec.get("native_smoke_mode") or "")
PY
)"

case "$ARCHIVE_FORMAT" in
    zip) EXPECTED_SUFFIX=".zip" ;;
    tar.gz) EXPECTED_SUFFIX=".tar.gz" ;;
    tar.xz) EXPECTED_SUFFIX=".tar.xz" ;;
    *)
        echo "unsupported archive format '$ARCHIVE_FORMAT' for $TARGET" >&2
        exit 1
        ;;
esac

case "$ARCHIVE_PATH" in
    *"$EXPECTED_SUFFIX") ;;
    *)
        echo "archive '$ARCHIVE_PATH' does not match contract format '$EXPECTED_SUFFIX' for $TARGET" >&2
        exit 1
        ;;
esac

rm -rf "$SCRATCH_DIR"
mkdir -p "$SCRATCH_DIR/extract"

case "$ARCHIVE_FORMAT" in
    zip)
        python3 - "$ARCHIVE_PATH" "$SCRATCH_DIR/extract" <<'PY'
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1]) as archive:
    archive.extractall(sys.argv[2])
PY
        ;;
    *)
        tar -xf "$ARCHIVE_PATH" -C "$SCRATCH_DIR/extract"
        ;;
esac

BUNDLE_DIR="$(find "$SCRATCH_DIR/extract" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
[ -n "$BUNDLE_DIR" ] || { echo "expected an extracted bundle directory" >&2; exit 1; }

INSTALL_DIR="$SCRATCH_DIR/install"
BIN_DIR="$SCRATCH_DIR/bin"
sh "$BUNDLE_DIR/install.sh" --source-dir "$BUNDLE_DIR" --install-dir "$INSTALL_DIR" --bin-dir "$BIN_DIR"

[ -x "$INSTALL_DIR/oscan" ] || { echo "installed oscan binary not found" >&2; exit 1; }
if [ "$BUNDLE_KIND" = "full" ] && [ ! -d "$INSTALL_DIR/toolchain" ]; then
    echo "installed bundle is missing the sibling toolchain directory" >&2
    exit 1
fi
if [ -n "$NOTE_FILE" ] && [ ! -f "$INSTALL_DIR/$NOTE_FILE" ]; then
    echo "installed bundle is missing the contract note file '$NOTE_FILE'" >&2
    exit 1
fi
if [ -n "$NATIVE_RUNTIME_MODES" ]; then
    for SOURCE_NAME in osc_native_shim.c osc_runtime.h; do
        [ -f "$INSTALL_DIR/native-runtime/$SOURCE_NAME" ] || {
            echo "installed bundle is missing native runtime source '$SOURCE_NAME'" >&2
            exit 1
        }
    done
    RUNTIME_ARCHIVE_DIR="$INSTALL_DIR/build/runtime-archives/$TARGET"
    OLD_IFS=$IFS
    IFS=,
    for MODE in $NATIVE_RUNTIME_MODES; do
        for SUFFIX in .a .json; do
            [ -f "$RUNTIME_ARCHIVE_DIR/libosc_runtime_${MODE}${SUFFIX}" ] || {
                echo "installed bundle is missing native runtime asset libosc_runtime_${MODE}${SUFFIX}" >&2
                exit 1
            }
        done
    done
    IFS=$OLD_IFS
fi

cat > "$SCRATCH_DIR/hello.osc" <<'EOF'
fn! main() {
    println("Hello, Release!");
}
EOF

OSCAN_COMMAND="$BIN_DIR/oscan"
[ -x "$OSCAN_COMMAND" ] || OSCAN_COMMAND="$INSTALL_DIR/oscan"
COMPILE_LOG="$SCRATCH_DIR/compile.stderr.txt"
OUTPUT_EXE="$SCRATCH_DIR/hello"

if [ "$REQUIRES_HOST_COMPILER" = "1" ]; then
    if ! "$OSCAN_COMMAND" --libc "$SCRATCH_DIR/hello.osc" -o "$OUTPUT_EXE" 2>"$COMPILE_LOG"; then
        cat "$COMPILE_LOG" >&2
        exit 1
    fi
else
    if ! "$OSCAN_COMMAND" "$SCRATCH_DIR/hello.osc" -o "$OUTPUT_EXE" 2>"$COMPILE_LOG"; then
        cat "$COMPILE_LOG" >&2
        exit 1
    fi
fi

if [ "$BUNDLE_KIND" = "full" ]; then
    EXPECTED_COMPILER_SOURCE="bundled"
else
    EXPECTED_COMPILER_SOURCE="host"
fi

COMPILE_TEXT="$(cat "$COMPILE_LOG")"
printf '%s' "$COMPILE_TEXT" | grep -qi "$EXPECTED_COMPILER_SOURCE" || {
    echo "expected $EXPECTED_COMPILER_SOURCE compiler detection during release smoke test" >&2
    echo "$COMPILE_TEXT" >&2
    exit 1
}

ACTUAL="$("$OUTPUT_EXE")"
[ "$ACTUAL" = "Hello, Release!" ] || {
    echo "unexpected smoke test output: $ACTUAL" >&2
    exit 1
}

if [ -n "$NATIVE_RUNTIME_MODES" ]; then
    NATIVE_OUTPUT_EXE="$SCRATCH_DIR/hello-native"
    NATIVE_COMPILE_LOG="$SCRATCH_DIR/native.stderr.txt"
    if [ "$NATIVE_SMOKE_MODE" = "hosted" ]; then
        if ! OSCAN_RUNTIME_ARCHIVE_DIR="$RUNTIME_ARCHIVE_DIR" \
            "$OSCAN_COMMAND" --libc --backend native "$SCRATCH_DIR/hello.osc" \
            -o "$NATIVE_OUTPUT_EXE" 2>"$NATIVE_COMPILE_LOG"; then
            cat "$NATIVE_COMPILE_LOG" >&2
            exit 1
        fi
    else
        if ! OSCAN_RUNTIME_ARCHIVE_DIR="$RUNTIME_ARCHIVE_DIR" \
            "$OSCAN_COMMAND" --backend native "$SCRATCH_DIR/hello.osc" \
            -o "$NATIVE_OUTPUT_EXE" 2>"$NATIVE_COMPILE_LOG"; then
            cat "$NATIVE_COMPILE_LOG" >&2
            exit 1
        fi
    fi

    NATIVE_ACTUAL="$("$NATIVE_OUTPUT_EXE")"
    [ "$NATIVE_ACTUAL" = "Hello, Release!" ] || {
        echo "unexpected packaged native smoke output: $NATIVE_ACTUAL" >&2
        exit 1
    }
fi

echo "Release smoke test passed for $ARCHIVE_PATH"
