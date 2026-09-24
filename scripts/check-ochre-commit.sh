#!/usr/bin/env bash
set -euo pipefail

# Verify that the OCHRE submodule commit hash in vendors/OCHRE/ matches the
# commit hash documented in docs/validation.md. This ensures that the auditable
# reference version is kept in sync with the vendored submodule.
#
# The documented hash lives in docs/validation.md under the "OCHRE Reference"
# section as: `ffc8b56e99c61eb4e42af625bbbc310e198ec58d`

FAILED=0

# Extract the documented commit hash from docs/validation.md.
# The hash is a 40-character hex string in backticks following "Vendored commit:"
DOCUMENTED_HASH=$(grep -oE '`([0-9a-f]{40})`' docs/validation.md | head -1 | tr -d '`')

if [ -z "$DOCUMENTED_HASH" ]; then
    echo "FAIL: Could not find a 40-character commit hash in docs/validation.md"
    echo "Expected a line like: **Vendored commit:** \`ffc8b56e99c61eb4e42af625bbbc310e198ec58d\`"
    FAILED=1
    exit $FAILED
fi

# Get the actual submodule commit hash. git submodule status outputs:
#   <space><hash> <path> (branch-info)
# We extract the hash (second field of space-delimited output).
ACTUAL_HASH=$(git submodule status vendors/OCHRE | awk '{print $1}' | tr -d ' +-')

if [ -z "$ACTUAL_HASH" ]; then
    echo "FAIL: Could not determine the OCHRE submodule commit hash."
    echo "Is the vendors/OCHRE submodule initialised? Run: git submodule update --init"
    FAILED=1
    exit $FAILED
fi

if [ "$DOCUMENTED_HASH" != "$ACTUAL_HASH" ]; then
    echo "FAIL: OCHRE submodule commit hash mismatch."
    echo "  Documented (docs/validation.md): $DOCUMENTED_HASH"
    echo "  Actual (vendors/OCHRE):         $ACTUAL_HASH"
    echo ""
    echo "If the submodule was intentionally updated, update the hash in:"
    echo "  - docs/validation.md (OCHRE Reference section)"
    echo "  - docs/development.md (Key Reference Documentation table)"
    echo "  - docs/findings/hvac/ochre_comparison.md (OCHRE Reference header)"
    echo "  - scripts/check-ochre-commit.sh (this script's help text)"
    FAILED=1
fi

if [ $FAILED -eq 0 ]; then
    echo "OK: OCHRE commit hash matches docs/validation.md ($DOCUMENTED_HASH)"
fi

exit $FAILED
