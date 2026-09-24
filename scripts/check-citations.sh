#!/usr/bin/env bash
set -euo pipefail

# Check for known erroneous citations in docs/.
# Ticket T-0309: Corrected Cutler et al. (2013) citation — wrong first author
# initial "B.", wrong title, wrong report number 57501 instead of 56354.
# Ticket T-0310: Corrected Winkler thesis year from 2011 to 2009;
# the Winkler doctoral dissertation is from 2009, not 2011.
# DRUM repository at https://drum.lib.umd.edu/handle/1903/9493 confirms 2009.
#
# Strikes-through text (~~...~~) preserves provenance and is exempt.
# docs/reviews/ is exempt — review documents may quote erroneous citations
# as part of describing findings.

FAILED=0

check_pattern() {
    local pattern="$1"
    local label="$2"
    local correct="$3"

    mapfile -t RAW_MATCHES < <(
        grep -rn "$pattern" docs/ \
            --exclude-dir=reviews \
            --exclude-dir=tickets \
            --exclude-dir=eplus \
            --exclude-dir=vendors \
            2>/dev/null \
        || true
    )

    if [ ${#RAW_MATCHES[@]} -eq 0 ]; then
        return
    fi

    # Filter out lines where the pattern appears inside strikethrough
    # (provenance-preserving markup). A line counts if the pattern appears
    # anywhere outside ~~ delimiters.
    local FILTERED=()
    for line in "${RAW_MATCHES[@]}"; do
        # Remove ~~...~~ spans; if pattern still appears, it's a real hit.
        local stripped
        stripped=$(echo "$line" | sed 's/~~[^~]*~~//g')
        if echo "$stripped" | grep -qF "$pattern"; then
            FILTERED+=("$line")
        fi
    done

    if [ ${#FILTERED[@]} -gt 0 ]; then
        echo "FAIL: Found ${#FILTERED[@]} occurrence(s) of erroneous citation '${label}':"
        printf '%s\n' "${FILTERED[@]}"
        echo ""
        echo "Correct citation is: ${correct}"
        FAILED=1
    fi
}

check_pattern "Cutler, B\\." "wrong first author (B. instead of D.)" \
    "Cutler, D., Winkler, J., Kruis, N., Christensen, C., Brandemuehl, M. (2013). Improved Modeling of Residential Air Conditioners and Heat Pumps for Energy Calculations. NREL/TP-5500-56354."

check_pattern "NREL/TP-5500-57501" "wrong report number (57501 instead of 56354)" \
    "Cutler, D., Winkler, J., Kruis, N., Christensen, C., Brandemuehl, M. (2013). Improved Modeling of Residential Air Conditioners and Heat Pumps for Energy Calculations. NREL/TP-5500-56354."

check_pattern "Winkler 2011" "Winkler thesis year 2011 instead of 2009" \
    "Winkler, J.M. (2009). \"Development of a Component Based Simulation Tool for the Steady State and Transient Analysis of Vapor Compression Systems.\" Ph.D. dissertation, University of Maryland. https://drum.lib.umd.edu/handle/1903/9493"

if [ $FAILED -eq 0 ]; then
    echo "OK: No unmarked erroneous citations found in docs/"
fi

exit $FAILED
