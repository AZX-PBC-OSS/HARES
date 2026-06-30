#!/usr/bin/env bash
set -euo pipefail

# Validate ASHRAE HOF 2021 chapter citations against known topic-to-chapter
# mappings. Flags citations where the chapter number is known to be wrong for
# its claimed topic or edition.
#
# Ticket T-0311: ASHRAE HOF chapter numbers in docs/code were incorrect for
# the claimed 2021 edition. This CI check prevents recurrence.
#
# The chapter index is maintained at docs/references/ashrae-hof-2021-chapters.md.
#
# docs/reviews/ is exempt — review documents may quote known-erroneous
# citations as part of describing findings. Ticket docs are also exempt:
# verification-audit sections within tickets may quote the original erroneous
# text for provenance.

FAILED=0
TARGET_DIRS=("crates/" "docs/" "tests/")
EXCLUDE_DIRS="docs/reviews docs/eplus docs/findings docs/hpxml docs/references docs/tickets vendors"

build_exclude_args() {
    local args=()
    for d in $EXCLUDE_DIRS; do
        args+=(--exclude-dir="$d")
    done
    echo "${args[@]}"
}

# ---------------------------------------------------------------------------
# Pattern 1: "ASHRAE HoF 2021 Ch. 25" referencing TARP / natural convection
# Ch. 25 in 2021 is "Heat, Air, and Moisture Control in Building Assemblies",
# not heat transfer. TARP natural convection belongs in Ch. 4.
# ---------------------------------------------------------------------------
check_pattern() {
    local pattern="$1"
    local label="$2"
    local correct="$3"

    mapfile -t MATCHES < <(
        grep -rn "$pattern" "${TARGET_DIRS[@]}" $(build_exclude_args) 2>/dev/null || true
    )

    if [ ${#MATCHES[@]} -gt 0 ]; then
        echo "FAIL: Found ${#MATCHES[@]} occurrence(s) of '${label}':"
        printf '%s\n' "${MATCHES[@]}"
        echo ""
        echo "Correct: ${correct}"
        echo "See: docs/references/ashrae-hof-2021-chapters.md"
        echo ""
        FAILED=1
    fi
}

# TARP / natural convection should not reference Ch. 25.
# Ch. 25 (2021) = HAM Control in Building Assemblies
# TARP / natural convection → Ch. 4 (Heat Transfer)
check_pattern \
    'ASHRAE.*Ch\.[ ]*25.*TARP\|TARP.*ASHRAE.*Ch\.[ ]*25\|ASHRAE.*Ch\.[ ]*25.*natural.convection\|natural.convection.*ASHRAE.*Ch\.[ ]*25' \
    "ASHRAE HOF Ch. 25 cited for TARP / natural convection (should be Ch. 4)" \
    "ASHRAE HOF 2021 Ch. 4 (Heat Transfer)"

# F-factor perimeter method / below-grade residential should not reference
# Ch. 18.31. Residential below-grade → Ch. 17.
check_pattern \
    'ASHRAE.*2021.*Ch\.\s*18\.31' \
    "ASHRAE HOF 2021 Ch. 18.31 cited for below-grade residential (should be Ch. 17)" \
    "ASHRAE HOF 2021 Ch. 17 (Residential Cooling and Heating Load Calculations)"

# Ch. 18 without an edition year but with residential topic keywords
check_pattern \
    'ASHRAE.*Ch\.\s*18[^\.].*residential\|ASHRAE.*Ch\.\s*18[^\.].*below.grade\|ASHRAE.*Ch\.\s*18[^\.].*slab\|ASHRAE.*Ch\.\s*18[^\.].*F.factor' \
    "ASHRAE Ch. 18 (no edition year) cited with residential / below-grade keywords (should be Ch. 17 for 2021)" \
    "ASHRAE HOF 2021 Ch. 17 (Residential Cooling and Heating Load Calculations)"

# ---------------------------------------------------------------------------
# Pattern 4: Ambiguous citations without edition year
# "ASHRAE HOF Ch. N" (no edition year) is ambiguous because chapters shift
# across editions. Require the edition year.
# ---------------------------------------------------------------------------
# This check is informational only (non-fatal) because there are legitimate
# cases (e.g., in review documents). It fires on code and primary docs where
# the edition year should always be present.

mapfile -t AMBIG_MATCHES < <(
    grep -rn 'ASHRAE.*HOF\|ASHRAE.*HoF\|ASHRAE.*Handbook.of.Fundamentals' "${TARGET_DIRS[@]}" $(build_exclude_args) 2>/dev/null | \
        grep -v -E '20[0-9][0-9]' | \
        grep -v -E 'ASHRAE[[:space:]]+[0-9]+' || true
)

if [ ${#AMBIG_MATCHES[@]} -gt 0 ]; then
    echo "WARN: Found ${#AMBIG_MATCHES[@]} ASHRAE HOF citation(s) without edition year:"
    printf '%s\n' "${AMBIG_MATCHES[@]}"
    echo ""
    echo "Add the edition year (e.g., 'ASHRAE HOF 2021 Ch. 4') to make the"
    echo "citation verifiable. Chapters shift across HOF editions."
    echo ""
    # Non-fatal: too many existing legacy citations without edition years.
    # This warning drives awareness; it does not block CI.
fi

if [ $FAILED -eq 0 ]; then
    echo "OK: No known-wrong ASHRAE HOF chapter citations found"
fi

exit $FAILED
