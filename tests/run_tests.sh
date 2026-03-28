#!/bin/bash
# Oscan Test Runner
# Usage: ./run_tests.sh <oscan-binary> [cc-compiler]
# Example: ./run_tests.sh ../target/release/oscan gcc

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

if [ $# -lt 1 ]; then
    echo "Usage: $0 <oscan-binary> [cc-compiler]"
    echo "Example: $0 ../target/release/oscan gcc"
    exit 1
fi

OSCAN="$1"
CC="${2:-gcc}"
PASS=0
FAIL=0
TOTAL=0

# Ensure build directory exists
mkdir -p build

echo "=== Oscan Test Suite ==="
echo "Compiler: $OSCAN"
echo "C Compiler: $CC"
echo ""

# ─── Positive Tests ───
echo "--- Positive Tests (must compile and produce correct output) ---"
echo ""

for osc_file in positive/*.osc; do
    name=$(basename "$osc_file" .osc)
    expected_file="expected/${name}.expected"
    TOTAL=$((TOTAL + 1))
    echo -n "  Testing $name... "

    # Check expected file exists
    if [ ! -f "$expected_file" ]; then
        echo "FAIL (missing expected file: $expected_file)"
        FAIL=$((FAIL + 1))
        continue
    fi

    # Step 1: Transpile .osc → .c
    if ! "$OSCAN" "$osc_file" -o "build/${name}.c" 2>"build/${name}.err"; then
        echo "FAIL (transpile error)"
        cat "build/${name}.err" | sed 's/^/    /'
        FAIL=$((FAIL + 1))
        continue
    fi

    # Step 2: Compile .c → binary
    if ! $CC "build/${name}.c" ../runtime/osc_runtime.c -I../runtime -I../deps/laststanding -o "build/${name}" -std=c99 -lm 2>"build/${name}.err"; then
        echo "FAIL (C compile error)"
        cat "build/${name}.err" | sed 's/^/    /'
        FAIL=$((FAIL + 1))
        continue
    fi

    # Step 3: Run and compare output
    actual=$("./build/${name}" 2>&1) || true
    expected=$(cat "$expected_file")

    if [ "$actual" = "$expected" ]; then
        echo "PASS"
        PASS=$((PASS + 1))
    else
        echo "FAIL (output mismatch)"
        echo "    Expected:"
        echo "$expected" | sed 's/^/      /'
        echo "    Actual:"
        echo "$actual" | sed 's/^/      /'
        FAIL=$((FAIL + 1))
    fi
done

echo ""

# ─── Negative Tests ───
echo "--- Negative Tests (must be rejected by the compiler) ---"
echo ""

for osc_file in negative/*.osc; do
    name=$(basename "$osc_file" .osc)
    TOTAL=$((TOTAL + 1))
    echo -n "  Testing reject $name... "

    if "$OSCAN" "$osc_file" -o "build/${name}.c" 2>"build/${name}.err"; then
        echo "FAIL (should have been rejected)"
        FAIL=$((FAIL + 1))
    else
        echo "PASS (correctly rejected)"
        PASS=$((PASS + 1))
    fi
done

echo ""
echo "========================================="
echo "Results: $PASS passed, $FAIL failed out of $TOTAL tests"
echo "========================================="

if [ $FAIL -gt 0 ]; then
    exit 1
else
    exit 0
fi
