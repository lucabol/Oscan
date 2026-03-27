#!/bin/bash
# Babel-C Test Runner
# Usage: ./run_tests.sh <babelc-binary> [cc-compiler]
# Example: ./run_tests.sh ../target/release/babelc gcc

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

if [ $# -lt 1 ]; then
    echo "Usage: $0 <babelc-binary> [cc-compiler]"
    echo "Example: $0 ../target/release/babelc gcc"
    exit 1
fi

BABELC="$1"
CC="${2:-gcc}"
PASS=0
FAIL=0
TOTAL=0

# Ensure build directory exists
mkdir -p build

echo "=== Babel-C Test Suite ==="
echo "Compiler: $BABELC"
echo "C Compiler: $CC"
echo ""

# ─── Positive Tests ───
echo "--- Positive Tests (must compile and produce correct output) ---"
echo ""

for bc_file in positive/*.bc; do
    name=$(basename "$bc_file" .bc)
    expected_file="expected/${name}.expected"
    TOTAL=$((TOTAL + 1))
    echo -n "  Testing $name... "

    # Check expected file exists
    if [ ! -f "$expected_file" ]; then
        echo "FAIL (missing expected file: $expected_file)"
        FAIL=$((FAIL + 1))
        continue
    fi

    # Step 1: Transpile .bc → .c
    if ! "$BABELC" "$bc_file" -o "build/${name}.c" 2>"build/${name}.err"; then
        echo "FAIL (transpile error)"
        cat "build/${name}.err" | sed 's/^/    /'
        FAIL=$((FAIL + 1))
        continue
    fi

    # Step 2: Compile .c → binary
    if ! $CC "build/${name}.c" ../runtime/bc_runtime.c -I../runtime -o "build/${name}" -std=c99 -lm 2>"build/${name}.err"; then
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

for bc_file in negative/*.bc; do
    name=$(basename "$bc_file" .bc)
    TOTAL=$((TOTAL + 1))
    echo -n "  Testing reject $name... "

    if "$BABELC" "$bc_file" -o "build/${name}.c" 2>"build/${name}.err"; then
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
