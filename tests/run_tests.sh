#!/bin/bash
# Oscan Test Runner — quiet by default, -v for verbose
# Usage: ./run_tests.sh <oscan-binary> [-v]
# Example: ./run_tests.sh ../target/release/oscan -v

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

if [ $# -lt 1 ]; then
    echo "Usage: $0 <oscan-binary> [-v]"
    exit 1
fi

OSCAN="$1"
VERBOSE=false
[ "${2:-}" = "-v" ] && VERBOSE=true

PASS=0; FAIL=0; NEG_PASS=0; NEG_FAIL=0
FAILURES=""

mkdir -p build

# ─── Positive Tests ───
for osc_file in positive/*.osc; do
    name=$(basename "$osc_file" .osc)
    expected_file="expected/${name}.expected"

    if [ ! -f "$expected_file" ]; then
        FAILURES="$FAILURES\n  $name — missing expected file"
        FAIL=$((FAIL + 1)); continue
    fi

    if echo "$name" | grep -q '^ffi'; then
      "$OSCAN" --libc "$osc_file" -o "build/${name}" 2>"build/${name}.err" || {
        FAILURES="$FAILURES\n  $name — compile error"; FAIL=$((FAIL + 1)); continue; }
    else
      "$OSCAN" "$osc_file" -o "build/${name}" 2>"build/${name}.err" || {
        FAILURES="$FAILURES\n  $name — compile error"; FAIL=$((FAIL + 1)); continue; }
    fi

    actual=$("./build/${name}" 2>&1) || true
    expected=$(cat "$expected_file")

    if [ "$actual" = "$expected" ]; then
        $VERBOSE && echo "  PASS: $name"
        PASS=$((PASS + 1))
    else
        FAILURES="$FAILURES\n  $name — output mismatch"
        FAIL=$((FAIL + 1))
    fi
done

# ─── Negative Tests ───
for osc_file in negative/*.osc; do
    name=$(basename "$osc_file" .osc)
    if "$OSCAN" "$osc_file" -o "build/${name}.c" 2>/dev/null; then
        FAILURES="$FAILURES\n  $name — should have been rejected"
        NEG_FAIL=$((NEG_FAIL + 1))
    else
        $VERBOSE && echo "  PASS: $name — correctly rejected"
        NEG_PASS=$((NEG_PASS + 1))
    fi
done

# ─── Freestanding Verification ───
V_PASS=0; V_FAIL=0
for exe in build/*; do
    [ -x "$exe" ] || continue
    name=$(basename "$exe")
    echo "$name" | grep -qE '\.(c|err)$' && continue
    echo "$name" | grep -q '^ffi' && continue
    file "$exe" | grep -q 'ELF' || continue

    file_info=$(file "$exe")
    ok=true
    if ! echo "$file_info" | grep -q 'statically linked'; then
        ldd_out=$(ldd "$exe" 2>&1 || true)
        echo "$ldd_out" | grep -q 'not a dynamic executable' || ok=false
    fi
    stdlib_count=$(strings "$exe" | grep -ciE 'libc\.so|libm\.so|glibc|__libc_start_main' || true)
    [ "$stdlib_count" -gt 0 ] && ok=false

    if $ok; then V_PASS=$((V_PASS + 1)); else V_FAIL=$((V_FAIL + 1)); fi
done

# ─── Summary ───
TOTAL_FAIL=$((FAIL + NEG_FAIL + V_FAIL))

echo ""
echo "━━━ Oscan Test Suite ━━━"
echo "  Positive:     $PASS passed, $FAIL failed"
echo "  Negative:     $NEG_PASS passed, $NEG_FAIL failed"
if [ $((V_PASS + V_FAIL)) -gt 0 ]; then
echo "  Freestanding: $V_PASS verified, $V_FAIL failed"
fi

if [ -n "$FAILURES" ]; then
    echo ""
    echo "  Failures:"
    echo -e "$FAILURES"
fi

echo ""
if [ $TOTAL_FAIL -gt 0 ]; then
    echo "  SOME TESTS FAILED"
    exit 1
else
    echo "  ALL TESTS PASSED ✓"
    exit 0
fi
