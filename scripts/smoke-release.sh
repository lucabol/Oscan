#!/usr/bin/env sh
# Variant-aware release smoke test (POSIX shell port of smoke-release.ps1).
#
# Every published artifact is one (target, profile) pair, so --profile is
# mandatory here too. The package layout/metadata assertions are delegated to
# 'release_tools.py verify-package-layout', which the PowerShell smoke test
# also runs, so both check identical facts about the same contract; what
# stays here is the behaviour that needs the packaged compiler itself.
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

TARGET=""
PROFILE=""
ARCHIVE_PATH=""
VERSION=""
SCRATCH_DIR=""
CONTRACT_PATH="$REPO_ROOT/packaging/toolchains/release-contract.json"

usage() {
    echo "usage: $0 --target <linux-x86_64|macos-x86_64> --profile <full|llvm|cranelift|c> --archive <path> [--version <version>] [--scratch-dir <path>] [--contract <path>]" >&2
}

fail() {
    echo "$1" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --target)
            TARGET="$2"
            shift 2
            ;;
        --profile|--backend)
            PROFILE="$2"
            shift 2
            ;;
        --archive)
            ARCHIVE_PATH="$2"
            shift 2
            ;;
        --version)
            VERSION="$2"
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
            usage
            exit 1
            ;;
    esac
done

[ -n "$TARGET" ] || { echo "missing --target" >&2; usage; exit 1; }
[ -n "$PROFILE" ] || { echo "missing --profile (full|llvm|cranelift|c)" >&2; usage; exit 1; }
[ -n "$ARCHIVE_PATH" ] || { echo "missing --archive" >&2; usage; exit 1; }
case "$PROFILE" in
    full|llvm|cranelift|c) ;;
    native)
        fail "'native' is a deprecated CLI alias for the cranelift backend and is never a package label; pass --profile cranelift"
        ;;
    *)
        fail "unknown profile '$PROFILE' (expected full, llvm, cranelift or c)"
        ;;
esac
case "$TARGET" in
    windows-*)
        fail "$0 cannot install a Windows package; run scripts/smoke-release.ps1 for $TARGET"
        ;;
esac
[ -f "$ARCHIVE_PATH" ] || fail "--archive must name a packaged release archive file, not '$ARCHIVE_PATH'"
ARCHIVE_PATH="$(CDPATH= cd -- "$(dirname -- "$ARCHIVE_PATH")" && pwd)/$(basename -- "$ARCHIVE_PATH")"

if [ -z "$SCRATCH_DIR" ]; then
    SCRATCH_DIR="$REPO_ROOT/target/release-smoke/$TARGET-$PROFILE"
fi

PYTHON="$(command -v python3 || command -v python || true)"
[ -n "$PYTHON" ] || fail "no Python interpreter found on PATH (tried python3, python); the release smoke test needs one to verify the package layout"

# Resolve exactly the facts this variant promises, from the contract.
eval "$(
    "$PYTHON" - "$CONTRACT_PATH" "$TARGET" "$PROFILE" <<'PY'
import json
import shlex
import sys

contract_path, target, profile = sys.argv[1], sys.argv[2], sys.argv[3]
with open(contract_path, encoding="utf-8") as handle:
    contract = json.load(handle)

if contract.get("schema_version") != 3:
    raise SystemExit(
        f"release contract {contract_path} is not schema 3 "
        f"(got {contract.get('schema_version')!r})"
    )
target_spec = contract["variants"].get(target)
if target_spec is None:
    raise SystemExit(f"release contract does not define target '{target}'")
variant = target_spec["profiles"].get(profile)
if variant is None:
    known = ", ".join(sorted(target_spec["profiles"]))
    raise SystemExit(
        f"release contract does not define profile '{profile}' for target '{target}' "
        f"(known: {known})"
    )


def emit(name: str, value: str) -> None:
    print(f"{name}={shlex.quote(value)}")


emit("ARCHIVE_FORMAT", target_spec["archive_format"])
emit("BINARY_NAME", target_spec["binary_name"])
emit("BACKEND_KIND", "full" if profile == "full" else contract["backends"][profile]["kind"])
emit("AVAILABLE_BACKENDS", " ".join(contract["profiles"][profile]["backends"]))
emit("BACKEND_LIST", ", ".join(contract["profiles"][profile]["backends"]))
emit("DEFAULT_BACKEND", contract["profiles"][profile]["default_backend"])
emit("COMPONENTS", ",".join(variant["components"]))
emit("RUNTIME_PROFILES", ",".join(variant["runtime_profiles"]))
emit("REQUIRES_HOST_COMPILER", "1" if variant.get("requires_host_compiler") else "0")
emit("TOOLCHAIN_FREE", "yes" if variant.get("toolchain_free") else "no")
PY
)"

# Compatibility variable for the slim-profile checks below.
BACKEND="$PROFILE"

case "$ARCHIVE_FORMAT" in
    zip) EXPECTED_SUFFIX=".zip" ;;
    tar.gz) EXPECTED_SUFFIX=".tar.gz" ;;
    tar.xz) EXPECTED_SUFFIX=".tar.xz" ;;
    *) fail "unsupported archive format '$ARCHIVE_FORMAT' for $TARGET" ;;
esac
case "$ARCHIVE_PATH" in
    *"$EXPECTED_SUFFIX") ;;
    *) fail "archive '$ARCHIVE_PATH' does not match the contract format '$EXPECTED_SUFFIX' for $TARGET" ;;
esac

if [ "$BACKEND_KIND" = "object" ]; then
    IS_OBJECT_PACKAGE=1
else
    IS_OBJECT_PACKAGE=0
fi
if [ "$BACKEND_KIND" = "full" ]; then
    IS_FULL_PACKAGE=1
    IS_OBJECT_CAPABLE=1
else
    IS_FULL_PACKAGE=0
    IS_OBJECT_CAPABLE="$IS_OBJECT_PACKAGE"
fi

rm -rf "$SCRATCH_DIR"
mkdir -p "$SCRATCH_DIR/extract"
SCRATCH_DIR="$(CDPATH= cd -- "$SCRATCH_DIR" && pwd)"

# Every override that could make a packaged compiler behave like a
# development checkout. OSCAN_RUNTIME_ARCHIVE_DIR is deliberately included:
# an object package must find its runtime archives at the fixed
# executable-relative location it ships them in.
SCRUBBED="-u OSCAN_NO_TOOLCHAIN -u OSCAN_CC -u OSCAN_TOOLCHAIN_DIR -u OSCAN_LLVM_LIB \
-u OSCAN_LLVM_DIR -u OSCAN_NATIVE_LINKER -u OSCAN_NATIVE_LINKER_FLAVOR \
-u OSCAN_NATIVE_ASSET_CACHE_DIR -u OSCAN_RUNTIME_ARCHIVE_DIR -u OSCAN_ARCHIVE_CC \
-u OSCAN_ARCHIVE_AR -u CC -u CXX -u LD"

# Host tool names are shadowed with stubs that fail immediately: bundled
# toolchain discovery walks the package directory and never PATH, so a
# regression to a host compiler fails loudly here instead of passing because
# the runner happens to have build-essential/Xcode CLT installed. Object
# packages also shadow the host linkers, because their final link runs the
# verified linker inside their own native-link sidecar by absolute path.
BLOCK_DIR="$SCRATCH_DIR/blocked-host-tools"
mkdir -p "$BLOCK_DIR"
BLOCKED_TOOLS="cc gcc g++ clang clang++ x86_64-linux-musl-gcc"
if [ "$IS_OBJECT_CAPABLE" -eq 1 ]; then
    BLOCKED_TOOLS="$BLOCKED_TOOLS ld ld.lld lld x86_64-linux-musl-ld"
fi
for NAME in $BLOCKED_TOOLS; do
    printf '#!/bin/sh\nexit 127\n' > "$BLOCK_DIR/$NAME"
    chmod +x "$BLOCK_DIR/$NAME"
done

SAMPLE_SOURCE="$SCRATCH_DIR/hello.osc"
cat > "$SAMPLE_SOURCE" <<'EOF'
fn! main() {
    println("Hello, Release!");
}
EOF

STATUS=0

run_packaged() {
    # run_packaged <log> <strict:0|1> -- <oscan args...>
    _log="$1"
    _strict="$2"
    shift 3
    STATUS=0
    if [ "$_strict" -eq 1 ]; then
        PATH="$BLOCK_DIR:$SAVED_PATH" env $SCRUBBED OSCAN_NO_TOOLCHAIN=1 \
            "$OSCAN_COMMAND" "$@" >/dev/null 2>"$_log" || STATUS=$?
    else
        PATH="$BLOCK_DIR:$SAVED_PATH" env $SCRUBBED \
            "$OSCAN_COMMAND" "$@" >/dev/null 2>"$_log" || STATUS=$?
    fi
}

run_packaged_unblocked() {
    # Same, but with the real PATH: the macOS C package legitimately needs
    # the host Apple Command Line Tools.
    _log="$1"
    shift 2
    STATUS=0
    env $SCRUBBED "$OSCAN_COMMAND" "$@" >/dev/null 2>"$_log" || STATUS=$?
}

assert_matches() {
    # assert_matches <log> <extended-regex> <what>
    grep -Eq "$2" "$1" || {
        echo "$3 did not report /$2/:" >&2
        cat "$1" >&2
        exit 1
    }
}

assert_program_runs() {
    # assert_program_runs <exe> <what>
    [ -x "$1" ] || fail "$2 produced no executable at $1"
    _actual="$("$1")"
    [ "$_actual" = "Hello, Release!" ] || fail "$2 produced unexpected output: '$_actual'"
}

assert_refused() {
    # assert_refused <log> <output-path> <what>; reads $STATUS from run_packaged
    [ "$STATUS" -ne 0 ] || {
        echo "$3 was accepted (exit 0) but must be refused:" >&2
        cat "$1" >&2
        exit 1
    }
    if grep -q "Compiling with " "$1"; then
        echo "$3 fell back to a C compiler instead of refusing:" >&2
        cat "$1" >&2
        exit 1
    fi
    [ ! -e "$2" ] || fail "$3 was refused but still produced $2"
}

# --- extract -----------------------------------------------------------------

case "$ARCHIVE_FORMAT" in
    zip)
        "$PYTHON" - "$ARCHIVE_PATH" "$SCRATCH_DIR/extract" <<'PY'
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

BUNDLE_COUNT="$(find "$SCRATCH_DIR/extract" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
[ "$BUNDLE_COUNT" = "1" ] || fail "expected exactly one extracted bundle directory, found $BUNDLE_COUNT"
BUNDLE_DIR="$(find "$SCRATCH_DIR/extract" -mindepth 1 -maxdepth 1 -type d)"

set -- "$SCRIPT_DIR/release_tools.py" verify-package-layout \
    --target "$TARGET" --profile "$PROFILE" \
    --root "$BUNDLE_DIR" --stage extracted \
    --archive "$ARCHIVE_PATH" --contract "$CONTRACT_PATH"
if [ -n "$VERSION" ]; then
    set -- "$@" --version "$VERSION"
fi
"$PYTHON" "$@" || fail "extracted $TARGET/$PROFILE package does not match the release contract"

# --- install -----------------------------------------------------------------

INSTALL_ROOT="$SCRATCH_DIR/install"
BIN_DIR="$SCRATCH_DIR/bin"
PACKAGE_VERSION="$("$PYTHON" -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["version"])' "$BUNDLE_DIR/oscan-package.json")"
INSTALL_DIR="$INSTALL_ROOT/profiles/$PROFILE/$PACKAGE_VERSION"
sh "$BUNDLE_DIR/install.sh" --source-dir "$BUNDLE_DIR" --install-root "$INSTALL_ROOT" --bin-dir "$BIN_DIR"

OSCAN_COMMAND="$BIN_DIR/oscan-$PROFILE"
[ -x "$OSCAN_COMMAND" ] || OSCAN_COMMAND="$INSTALL_DIR/$BINARY_NAME"
[ -x "$OSCAN_COMMAND" ] || fail "installed oscan command was not found under $BIN_DIR or $INSTALL_DIR"
SAVED_PATH="$PATH"

set -- "$SCRIPT_DIR/release_tools.py" verify-package-layout \
    --target "$TARGET" --profile "$PROFILE" \
    --root "$INSTALL_DIR" --stage installed --contract "$CONTRACT_PATH"
if [ -n "$VERSION" ]; then
    set -- "$@" --version "$VERSION"
fi
"$PYTHON" "$@" || fail "installed $TARGET/$PROFILE package does not match the release contract"

# --- identity ----------------------------------------------------------------

VERSION_LOG="$SCRATCH_DIR/version.txt"
"$OSCAN_COMMAND" --version > "$VERSION_LOG" 2>&1 || {
    cat "$VERSION_LOG" >&2
    fail "packaged 'oscan --version' failed"
}
assert_matches "$VERSION_LOG" "^backends: $BACKEND_LIST\$" "packaged 'oscan --version'"
assert_matches "$VERSION_LOG" "^default-backend: $DEFAULT_BACKEND\$" "packaged 'oscan --version'"
assert_matches "$VERSION_LOG" "^distribution: $PROFILE\$" "packaged 'oscan --version'"
assert_matches "$VERSION_LOG" "^toolchain-free: $TOOLCHAIN_FREE\$" "packaged 'oscan --version'"
if [ -n "$VERSION" ]; then
    grep -qF "$VERSION" "$VERSION_LOG" || {
        cat "$VERSION_LOG" >&2
        fail "packaged 'oscan --version' does not carry release version '$VERSION'"
    }
fi

# --- behaviour ---------------------------------------------------------------

if [ "$IS_FULL_PACKAGE" -eq 1 ]; then
    DEFAULT_OUTPUT="$SCRATCH_DIR/hello-default"
    DEFAULT_LOG="$SCRATCH_DIR/default.stderr.txt"
    run_packaged "$DEFAULT_LOG" 0 -- --verbose "$SAMPLE_SOURCE" -o "$DEFAULT_OUTPUT"
    [ "$STATUS" -eq 0 ] || {
        cat "$DEFAULT_LOG" >&2
        fail "packaged full-profile default compile failed"
    }
    assert_matches "$DEFAULT_LOG" "^\[verbose\] $DEFAULT_BACKEND backend target:" \
        "packaged full-profile default compile"
    assert_matches "$DEFAULT_LOG" "^\[verbose\] native-link assets: sidecar \(" \
        "packaged full-profile default compile"
    assert_program_runs "$DEFAULT_OUTPUT" "packaged full-profile default compile"

    for SELECTED_BACKEND in $AVAILABLE_BACKENDS; do
        OUTPUT="$SCRATCH_DIR/hello-$SELECTED_BACKEND"
        LOG="$SCRATCH_DIR/$SELECTED_BACKEND.stderr.txt"
        run_packaged "$LOG" 0 -- --verbose --backend "$SELECTED_BACKEND" \
            "$SAMPLE_SOURCE" -o "$OUTPUT"
        [ "$STATUS" -eq 0 ] || {
            cat "$LOG" >&2
            fail "packaged full-profile '--backend $SELECTED_BACKEND' compile failed"
        }
        if [ "$SELECTED_BACKEND" = "c" ]; then
            assert_matches "$LOG" "Compiling with .+ \(bundled" \
                "packaged full-profile C compile"
        else
            assert_matches "$LOG" "^\[verbose\] $SELECTED_BACKEND backend target:" \
                "packaged full-profile $SELECTED_BACKEND compile"
            assert_matches "$LOG" "^\[verbose\] native-link assets: sidecar \(" \
                "packaged full-profile $SELECTED_BACKEND compile"
        fi
        assert_program_runs "$OUTPUT" "packaged full-profile $SELECTED_BACKEND compile"
    done

    HOSTED_OUTPUT="$SCRATCH_DIR/hello-hosted"
    HOSTED_LOG="$SCRATCH_DIR/hosted.stderr.txt"
    run_packaged "$HOSTED_LOG" 0 -- --verbose --libc "$SAMPLE_SOURCE" -o "$HOSTED_OUTPUT"
    [ "$STATUS" -eq 0 ] || {
        cat "$HOSTED_LOG" >&2
        fail "packaged full-profile '--libc' compile failed"
    }
    assert_matches "$HOSTED_LOG" "Linking hosted executable with .+ \(bundled\)" \
        "packaged full-profile hosted compile"
    assert_program_runs "$HOSTED_OUTPUT" "packaged full-profile hosted compile"

    EXTRA_C_SOURCE="$SCRATCH_DIR/extra.c"
    printf 'int oscan_smoke_extra(void) { return 0; }\n' > "$EXTRA_C_SOURCE"
    EXTRA_OUTPUT="$SCRATCH_DIR/hello-extra-c"
    EXTRA_LOG="$SCRATCH_DIR/extra-c.stderr.txt"
    run_packaged "$EXTRA_LOG" 0 -- --verbose --extra-c "$EXTRA_C_SOURCE" \
        "$SAMPLE_SOURCE" -o "$EXTRA_OUTPUT"
    [ "$STATUS" -eq 0 ] || {
        cat "$EXTRA_LOG" >&2
        fail "packaged full-profile '--extra-c' compile failed"
    }
    assert_matches "$EXTRA_LOG" "Linking freestanding executable with .+ \(bundled\)" \
        "packaged full-profile extra-C compile"
    assert_program_runs "$EXTRA_OUTPUT" "packaged full-profile extra-C compile"
elif [ "$IS_OBJECT_PACKAGE" -eq 1 ]; then
    # 1. The default backend: a distribution build defaults to the one
    #    backend it ships, deterministically and without probing.
    DEFAULT_OUTPUT="$SCRATCH_DIR/hello-default"
    DEFAULT_LOG="$SCRATCH_DIR/default.stderr.txt"
    run_packaged "$DEFAULT_LOG" 1 -- --verbose "$SAMPLE_SOURCE" -o "$DEFAULT_OUTPUT"
    [ "$STATUS" -eq 0 ] || {
        cat "$DEFAULT_LOG" >&2
        fail "packaged $BACKEND default-backend compile failed"
    }
    assert_matches "$DEFAULT_LOG" "^\[verbose\] $BACKEND backend target:" \
        "packaged $BACKEND default compile"
    assert_matches "$DEFAULT_LOG" "^\[verbose\] native-link assets: sidecar \(" \
        "packaged $BACKEND default compile"
    if [ "$BACKEND" = "llvm" ]; then
        assert_matches "$DEFAULT_LOG" \
            "^\[verbose\] LLVM code generator: .+ \(LLVM [0-9]+\.[0-9]+\.[0-9]+, targets: " \
            "packaged llvm default compile"
    fi
    assert_program_runs "$DEFAULT_OUTPUT" "packaged $BACKEND default compile"

    # 2. The same backend named explicitly.
    EXPLICIT_OUTPUT="$SCRATCH_DIR/hello-explicit"
    EXPLICIT_LOG="$SCRATCH_DIR/explicit.stderr.txt"
    run_packaged "$EXPLICIT_LOG" 1 -- --verbose --backend "$BACKEND" "$SAMPLE_SOURCE" -o "$EXPLICIT_OUTPUT"
    [ "$STATUS" -eq 0 ] || {
        cat "$EXPLICIT_LOG" >&2
        fail "packaged '--backend $BACKEND' compile failed"
    }
    assert_matches "$EXPLICIT_LOG" "^\[verbose\] $BACKEND backend target:" \
        "packaged '--backend $BACKEND' compile"
    assert_program_runs "$EXPLICIT_OUTPUT" "packaged '--backend $BACKEND' compile"

    # 3. Cranelift keeps accepting its deprecated spelling, with exactly one
    #    warning — the alias is a compatibility shim, never a package label.
    if [ "$BACKEND" = "cranelift" ]; then
        ALIAS_OUTPUT="$SCRATCH_DIR/hello-alias"
        ALIAS_LOG="$SCRATCH_DIR/alias.stderr.txt"
        run_packaged "$ALIAS_LOG" 1 -- --backend native "$SAMPLE_SOURCE" -o "$ALIAS_OUTPUT"
        [ "$STATUS" -eq 0 ] || {
            cat "$ALIAS_LOG" >&2
            fail "packaged '--backend native' alias compile failed"
        }
        assert_matches "$ALIAS_LOG" \
            "'--backend native' is deprecated; use '--backend cranelift'" \
            "packaged '--backend native' alias"
        assert_program_runs "$ALIAS_OUTPUT" "packaged '--backend native' alias compile"
    fi

    # 4. Everything this package does not contain is refused by name.
    if [ "$BACKEND" = "llvm" ]; then
        OTHER_OBJECT_BACKEND="cranelift"
    else
        OTHER_OBJECT_BACKEND="llvm"
    fi

    REFUSED_OUTPUT="$SCRATCH_DIR/refused-c"
    REFUSED_LOG="$SCRATCH_DIR/refused-backend-c.stderr.txt"
    run_packaged "$REFUSED_LOG" 1 -- --backend c "$SAMPLE_SOURCE" -o "$REFUSED_OUTPUT"
    assert_refused "$REFUSED_LOG" "$REFUSED_OUTPUT" "'--backend c' in the $TARGET/$BACKEND package"
    assert_matches "$REFUSED_LOG" "the c backend is not included in this compiler build" \
        "'--backend c' refusal"
    assert_matches "$REFUSED_LOG" "this build includes: $BACKEND" "'--backend c' refusal"
    assert_matches "$REFUSED_LOG" "archive name ends in '-full' or '-c'" "'--backend c' refusal"

    REFUSED_OUTPUT="$SCRATCH_DIR/refused-other"
    REFUSED_LOG="$SCRATCH_DIR/refused-other-backend.stderr.txt"
    run_packaged "$REFUSED_LOG" 1 -- --backend "$OTHER_OBJECT_BACKEND" "$SAMPLE_SOURCE" -o "$REFUSED_OUTPUT"
    assert_refused "$REFUSED_LOG" "$REFUSED_OUTPUT" \
        "'--backend $OTHER_OBJECT_BACKEND' in the $TARGET/$BACKEND package"
    assert_matches "$REFUSED_LOG" \
        "the $OTHER_OBJECT_BACKEND backend is not included in this compiler build" \
        "'--backend $OTHER_OBJECT_BACKEND' refusal"
    assert_matches "$REFUSED_LOG" "archive name ends in '-full' or '-$OTHER_OBJECT_BACKEND'" \
        "'--backend $OTHER_OBJECT_BACKEND' refusal"

    REFUSED_OUTPUT="$SCRATCH_DIR/refused-libc"
    REFUSED_LOG="$SCRATCH_DIR/refused-libc.stderr.txt"
    run_packaged "$REFUSED_LOG" 1 -- --libc "$SAMPLE_SOURCE" -o "$REFUSED_OUTPUT"
    assert_refused "$REFUSED_LOG" "$REFUSED_OUTPUT" "'--libc' in the $TARGET/$BACKEND package"
    assert_matches "$REFUSED_LOG" "does not include the C backend" "'--libc' refusal"
    assert_matches "$REFUSED_LOG" "refuses --libc" "'--libc' refusal"
    assert_matches "$REFUSED_LOG" "install a package that includes the C backend" "'--libc' refusal"

    EXTRA_C_SOURCE="$SCRATCH_DIR/extra.c"
    printf 'int oscan_smoke_extra(void) { return 0; }\n' > "$EXTRA_C_SOURCE"
    REFUSED_OUTPUT="$SCRATCH_DIR/refused-extra"
    REFUSED_LOG="$SCRATCH_DIR/refused-extra-c.stderr.txt"
    run_packaged "$REFUSED_LOG" 1 -- --extra-c "$EXTRA_C_SOURCE" "$SAMPLE_SOURCE" -o "$REFUSED_OUTPUT"
    assert_refused "$REFUSED_LOG" "$REFUSED_OUTPUT" "'--extra-c' in the $TARGET/$BACKEND package"
    assert_matches "$REFUSED_LOG" "does not include the C backend" "'--extra-c' refusal"
    assert_matches "$REFUSED_LOG" "refuses --extra-c" "'--extra-c' refusal"

    REFUSED_OUTPUT="$SCRATCH_DIR/refused-output.c"
    REFUSED_LOG="$SCRATCH_DIR/refused-c-output.stderr.txt"
    run_packaged "$REFUSED_LOG" 1 -- "$SAMPLE_SOURCE" -o "$REFUSED_OUTPUT"
    assert_refused "$REFUSED_LOG" "$REFUSED_OUTPUT" \
        "C source output in the $TARGET/$BACKEND package"
    assert_matches "$REFUSED_LOG" "the c backend is not included in this compiler build" \
        "C source output refusal"
else
    # A C package is the portability package: it emits C and needs a C
    # compiler for it. Linux bundles its own; macOS uses the host Apple
    # Command Line Tools.
    if [ "$REQUIRES_HOST_COMPILER" = "1" ]; then
        EXPECTED_COMPILER_SOURCE="host"
    else
        EXPECTED_COMPILER_SOURCE="bundled"
    fi

    DEFAULT_OUTPUT="$SCRATCH_DIR/hello-default"
    DEFAULT_LOG="$SCRATCH_DIR/default.stderr.txt"
    EXPLICIT_OUTPUT="$SCRATCH_DIR/hello-explicit"
    EXPLICIT_LOG="$SCRATCH_DIR/explicit.stderr.txt"
    if [ "$REQUIRES_HOST_COMPILER" = "1" ]; then
        run_packaged_unblocked "$DEFAULT_LOG" -- --verbose --libc "$SAMPLE_SOURCE" -o "$DEFAULT_OUTPUT"
    else
        run_packaged "$DEFAULT_LOG" 0 -- --verbose "$SAMPLE_SOURCE" -o "$DEFAULT_OUTPUT"
    fi
    [ "$STATUS" -eq 0 ] || {
        cat "$DEFAULT_LOG" >&2
        fail "packaged c default-backend compile failed"
    }
    assert_matches "$DEFAULT_LOG" "Compiling with .+ \($EXPECTED_COMPILER_SOURCE" \
        "packaged c default compile"
    assert_program_runs "$DEFAULT_OUTPUT" "packaged c default compile"

    if [ "$REQUIRES_HOST_COMPILER" = "1" ]; then
        run_packaged_unblocked "$EXPLICIT_LOG" -- --backend c --libc "$SAMPLE_SOURCE" -o "$EXPLICIT_OUTPUT"
    else
        run_packaged "$EXPLICIT_LOG" 0 -- --backend c "$SAMPLE_SOURCE" -o "$EXPLICIT_OUTPUT"
    fi
    [ "$STATUS" -eq 0 ] || {
        cat "$EXPLICIT_LOG" >&2
        fail "packaged '--backend c' compile failed"
    }
    assert_matches "$EXPLICIT_LOG" "Compiling with .+ \($EXPECTED_COMPILER_SOURCE" \
        "packaged '--backend c' compile"
    assert_program_runs "$EXPLICIT_OUTPUT" "packaged '--backend c' compile"

    for MISSING in llvm cranelift; do
        REFUSED_OUTPUT="$SCRATCH_DIR/refused-$MISSING"
        REFUSED_LOG="$SCRATCH_DIR/refused-$MISSING.stderr.txt"
        run_packaged "$REFUSED_LOG" 0 -- --backend "$MISSING" "$SAMPLE_SOURCE" -o "$REFUSED_OUTPUT"
        assert_refused "$REFUSED_LOG" "$REFUSED_OUTPUT" \
            "'--backend $MISSING' in the $TARGET/$BACKEND package"
        assert_matches "$REFUSED_LOG" \
            "the $MISSING backend is not included in this compiler build" \
            "'--backend $MISSING' refusal"
        assert_matches "$REFUSED_LOG" "this build includes: c" "'--backend $MISSING' refusal"
        assert_matches "$REFUSED_LOG" "archive name ends in '-full' or '-$MISSING'" \
            "'--backend $MISSING' refusal"
    done
fi

echo "Release smoke test passed for $TARGET/$PROFILE ($ARCHIVE_PATH)"
