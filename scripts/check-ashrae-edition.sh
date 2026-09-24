#!/usr/bin/env bash
set -euo pipefail

# Enforce ASHRAE HOF 2021 as the sole authoritative edition for all
# psychrometric constants, molecular weights, and film coefficient tables.
# Fails if any HOF-specific "2017" citation remains in crate source code.

TARGET_DIR="crates/"
FAILS=0

# Excluded:
#   - ASHRAE_Tau2017 (EnergyPlus solar model name)
#   - "2013/2017" (ASHRAETau model supporting both editions)
#   - Historical notes documenting the migration
#   - Combined "2017/2021" references
#   - stepping.rs Ch.14 design-day citations: The Ch.14 Table 1 diurnal
#     temperature profile and clear-sky model are edition-pinned model data
#     (not psychrometric properties). The 2017 data is what the code implements
#     and is unchanged between editions; these citations should not be migrated.

MATCHES=$(grep -rn -E 'ASHRAE.*20[0-9][0-9].*Handbook|ASHRAE.*20[0-9][0-9].*[Hh][Oo][Ff]|ASHRAE.*Handbook.*20[0-9][0-9]' "$TARGET_DIR" 2>/dev/null | \
    grep '2017' | \
    grep -v 'ASHRAE_Tau2017' | \
    grep -v '2013/2017' | \
    grep -v 'previously 28.9645 g/mol in the 2017' | \
    grep -v 'ASHRAE HoF 2017/2021' | \
    grep -v 'stepping\.rs' || true)

if [ -n "$MATCHES" ]; then
    echo "FAIL: Found ASHRAE HOF 2017 citations to migrate to 2021:"
    echo "$MATCHES"
    echo ""
    echo "ASHRAE HOF 2021 is the sole authoritative edition for psychrometric"
    echo "constants, molecular weights, and film coefficient tables in crates/."
    echo "Update all remaining 'ASHRAE 2017' citations to 'ASHRAE HOF 2021' and"
    echo "verify that any edition-sensitive values (molecular weights,"
    echo "humidity density correction) match the 2021 values."
    FAILS=1
fi

exit $FAILS
