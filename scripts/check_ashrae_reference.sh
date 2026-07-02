#!/usr/bin/env bash
set -euo pipefail

# check_ashrae_reference.sh — Verify that the committed ASHRAE RC reference
# fixture matches the output of gen_ashrae_reference.py.
#
# Usage: ./scripts/check_ashrae_reference.sh
#   Fails with exit code 1 if running gen_ashrae_reference.py produces output
#   that differs from the committed fixture, printing a unified diff.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

GEN_SCRIPT="$REPO_ROOT/scripts/gen_ashrae_reference.py"
FIXTURE="$REPO_ROOT/tests/fixtures/parity/ashrae_rc_reference.json"

if [ ! -f "$GEN_SCRIPT" ]; then
    echo "ERROR: gen_ashrae_reference.py not found at $GEN_SCRIPT"
    exit 1
fi

if [ ! -f "$FIXTURE" ]; then
    echo "ERROR: ashrae_rc_reference.json not found at $FIXTURE"
    exit 1
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "Running gen_ashrae_reference.py..."
cd "$REPO_ROOT"

GENERATED_FIXTURE="$TMPDIR/generated_ashrae_rc_reference.json"

uv run python "$GEN_SCRIPT" --output "$GENERATED_FIXTURE" > "$TMPDIR/gen_output.txt" 2>&1 || {
    echo "ERROR: gen_ashrae_reference.py failed"
    cat "$TMPDIR/gen_output.txt"
    if grep -q "ModuleNotFoundError.*xmltodict" "$TMPDIR/gen_output.txt"; then
        echo ""
        echo "xmltodict is not installed. Install it for gen_ashrae_reference.py CI checks:"
        echo "  uv sync --extra ochre"
    fi
    exit 1
}

echo "Comparing generated fixture against committed file..."

if ! diff -q "$GENERATED_FIXTURE" "$FIXTURE" > /dev/null 2>&1; then
    echo ""
    echo "FAIL: ashrae_rc_reference.json is out of sync with gen_ashrae_reference.py."
    echo ""
    diff -u "$FIXTURE" "$GENERATED_FIXTURE" || true
    echo ""
    echo "To fix: re-run the script and commit the updated fixture:"
    echo "  uv run python scripts/gen_ashrae_reference.py"
    echo "  git add tests/fixtures/parity/ashrae_rc_reference.json"
    echo "  git commit -m 'Regenerate ASHRAE RC reference fixture'"
    exit 1
fi

echo ""
echo "PASS: ashrae_rc_reference.json matches gen_ashrae_reference.py output."
exit 0
