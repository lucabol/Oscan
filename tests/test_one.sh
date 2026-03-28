#!/bin/bash
# Test individual Oscan file with WSL gcc
set -e

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 <test-name>"
    exit 1
fi

NAME=$1
OSCAN="../target/release/oscan.exe"
OSC_FILE="positive/${NAME}.osc"
EXPECTED_FILE="expected/${NAME}.expected"

echo "Testing $NAME..."

# Transpile
$OSCAN "$OSC_FILE" -o "build/${NAME}.c" 2>"build/${NAME}.err"

# Compile
gcc "build/${NAME}.c" "../runtime/osc_runtime.c" -I../runtime -o "build/${NAME}" -std=c99 -lm

# Run
./build/${NAME} > build/${NAME}.out 2>&1

# Compare
if diff -q build/${NAME}.out $EXPECTED_FILE > /dev/null; then
    echo "✓ PASS"
else
    echo "✗ FAIL - output mismatch"
    echo "Expected:"
    cat $EXPECTED_FILE
    echo "Actual:"
    cat build/${NAME}.out
    exit 1
fi
