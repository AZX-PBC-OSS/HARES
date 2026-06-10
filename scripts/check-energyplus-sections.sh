#!/usr/bin/env bash
set -euo pipefail

# Enforce descriptive EnergyPlus section names — no §-style numeric references.
# EnergyPlus ERM 26.1 is the pinned reference version for HARES.
# §-style citations (e.g. "EnergyPlus Engineering Reference §15.4") are
# unverifiable against the web-hosted docs which use heading-based navigation.

TARGET_DIRS=("crates/" "tests/" "docs/")
EXCLUDE_DIRS="docs/reviews docs/tickets docs/findings docs/eplus vendors"
FAILED=0

build_exclude_args() {
    local args=()
    for d in $EXCLUDE_DIRS; do
        args+=(--exclude-dir="$d")
    done
    echo "${args[@]}"
}

mapfile -t MATCHES < <(
    grep -rn 'EnergyPlus.*§' "${TARGET_DIRS[@]}" $(build_exclude_args) 2>/dev/null || true
)

if [ ${#MATCHES[@]} -gt 0 ]; then
    echo "FAIL: Found ${#MATCHES[@]} §-style EnergyPlus references:"
    printf '%s\n' "${MATCHES[@]}"
    echo ""
    echo "Replace §-style section numbers with descriptive heading names from"
    echo "EnergyPlus ERM 26.1. Example:"
    echo '  Before: EnergyPlus Engineering Reference §15.4 (AIM-2)'
    echo '  After:  EnergyPlus ERM 26.1 — AirflowNetwork Model: AIM-2 Enhanced Model'
    FAILED=1
fi

exit $FAILED
