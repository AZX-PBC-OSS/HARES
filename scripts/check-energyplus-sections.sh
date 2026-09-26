#!/usr/bin/env bash
set -euo pipefail

# Enforce descriptive EnergyPlus section names — no §-style numeric references.
# EnergyPlus ERM 26.1 is the pinned reference version for HARES.
# §-style citations (e.g. "EnergyPlus Engineering Reference §15.4") are
# unverifiable against the web-hosted docs which use heading-based navigation.

TARGET_DIRS=("crates" "tests" "docs")

# Path-based pruning — not grep --exclude-dir: grep's --exclude-dir matches
# directory *basenames*, so an exempted docs directory name would also skip
# any production directory that happens to share it (the ASHRAE gate's
# "hpxml" exemption did exactly that to crates/hares-io/src/hpxml — the
# HPXML parser). find -prune matches full paths, so the exemptions stay
# pinned to the provenance docs they exist for and every production
# directory stays scanned.
EXEMPT_PRUNE=(
    -path 'docs/reviews'
    -o -path 'docs/tickets'
    -o -path 'docs/findings'
    -o -path 'docs/eplus'
    -o -name vendors
)
FAILED=0

scan_matches() {
    # Every non-exempt file under the target trees, grepped by pattern.
    find "${TARGET_DIRS[@]}" \( "${EXEMPT_PRUNE[@]}" \) -prune -o -type f -print0 2>/dev/null \
        | xargs -0 -r grep -n "$1" 2>/dev/null || true
}

mapfile -t MATCHES < <(scan_matches 'EnergyPlus.*§')

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
