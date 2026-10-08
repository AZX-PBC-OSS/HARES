#!/usr/bin/env bash
set -euo pipefail

# check_os_hpxml_tables.sh: regenerate the OpenStudio-HPXML station tables
# under crates/hares-io/data from the pinned upstream release and fail if
# the committed copies differ.
#
# Usage: ./scripts/check_os_hpxml_tables.sh          (check)
#        ./scripts/check_os_hpxml_tables.sh --write  (regenerate in place)
#
# Source: OpenStudio-HPXML v1.12.0, HPXMLtoOpenStudio/resources/data/
# (BSD-3-Clause; notice in crates/hares-io/data/OS-HPXML-LICENSE.md).

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DATA_DIR="$REPO_ROOT/crates/hares-io/data"
BASE_URL="https://raw.githubusercontent.com/NREL/OpenStudio-HPXML/v1.12.0/HPXMLtoOpenStudio/resources/data"
ZIPCODE_SHA256="d453ec428e9ddf21375dc1a3c606cb7ec628eed280cd49ca26d3e2c5ff094ecd"
WSF_SHA256="94e64f002695bd24a4ee1a10ad0425726fe35c7b822aabbc62f6d1a2bba9edbb"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

fetch() { # $1: upstream file name, $2: expected sha256
    curl -sSfL -o "$TMPDIR/$1" "$BASE_URL/$1"
    local actual
    actual="$(sha256sum "$TMPDIR/$1" | cut -d' ' -f1)"
    if [ "$actual" != "$2" ]; then
        echo "ERROR: $1 sha256 $actual, pinned $2"
        exit 1
    fi
}

fetch zipcode_weather_stations.csv "$ZIPCODE_SHA256"
# The ASHRAE 62.2 weather and shielding factor per station is used verbatim.
fetch ashrae622_wsf.csv "$WSF_SHA256"

# The IECC zone of each station is the zone of the first row naming it,
# the row lookup_weather_data_from_wmo (defaults.rb:5471-5500) returns.
{
    echo "station_wmo,iecc_zone"
    awk -F, 'NR > 1 && NF > 0 && !($9 in seen) { seen[$9] = 1; print $9 "," $7 }' \
        "$TMPDIR/zipcode_weather_stations.csv"
} > "$TMPDIR/wmo_iecc_zones.csv"

status=0
for table in wmo_iecc_zones.csv ashrae622_wsf.csv; do
    if [ "${1:-}" = "--write" ]; then
        cp "$TMPDIR/$table" "$DATA_DIR/$table"
        echo "wrote $DATA_DIR/$table"
    elif ! diff -u "$DATA_DIR/$table" "$TMPDIR/$table"; then
        echo "ERROR: $table differs from its regeneration"
        status=1
    else
        echo "PASS: $table matches its regeneration"
    fi
done
exit "$status"
