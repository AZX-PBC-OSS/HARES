#!/usr/bin/env bash
# -*- mode: shell-script; -*-
#
# HARES Systematic Code Review Dispatcher
# ========================================
# Idempotent — re-running skips reviews whose output .md already exists.
# Review areas are defined in JSON manifest files under scripts/reviews/.
#
# Usage:
#   ./scripts/review-all.sh                           # run all pending reviews
#   ./scripts/review-all.sh --dry-run                 # list what would run
#   ./scripts/review-all.sh hpxml-01                  # run a single review by ID
#   ./scripts/review-all.sh --category hpxml          # run all in one category
#   ./scripts/review-all.sh --list-categories         # list available categories
#
# Output:
#   docs/reviews/{category}/{id}-{slug}.md
# ---------------------------------------------------------------------------
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REVIEWS_DIR="$REPO_ROOT/docs/reviews"
MANIFEST_DIR="$SCRIPT_DIR/reviews"

# ---------- opencode invocation ----------
OPENCODE_MODE="${OPENCODE_MODE:-prompt}"
OPENCODE_BIN="${OPENCODE_BIN:-opencode}"
OPENCODE_FLAGS="${OPENCODE_FLAGS:-}"

run_opencode() {
    local prompt="$1"
    local output_path="$2"
    mkdir -p "$(dirname "$output_path")"
    echo "---"
    echo "Starting review -> $output_path"
    echo "---"
    case "$OPENCODE_MODE" in
        prompt-file)
            local tmpfile; tmpfile="$(mktemp)"
            printf '%s' "$prompt" > "$tmpfile"
            trap 'rm -f "$tmpfile"' RETURN
            # shellcheck disable=SC2086
            $OPENCODE_BIN run $OPENCODE_FLAGS --prompt-file "$tmpfile"
            ;;
        *)
            # shellcheck disable=SC2086
            printf '%s' "$prompt" | $OPENCODE_BIN $OPENCODE_FLAGS
            ;;
    esac
    echo "--- Done: $output_path ---"
}

# ---------- load manifests ----------
load_all_reviews() {
    local json=""
    for mf in "$MANIFEST_DIR"/*.json; do
        [ -f "$mf" ] || continue
        local cat; cat=$(basename "$mf" .json)
        # Emit flattened: id|cat|slug|title|vendor_refs|files|prompt (one line per review)
        jq -r --arg cat "$cat" '
          .reviews[] |
          "\(.id)|\($cat)|\(.slug)|\(.title)|\(.vendor_refs // "")|\(.files // "")|\(.prompt)"
        ' "$mf"
    done
}

# ---------- helpers ----------
review_id()          { echo "$1" | cut -d'|' -f1; }
review_category()    { echo "$1" | cut -d'|' -f2; }
review_slug()        { echo "$1" | cut -d'|' -f3; }
review_title()       { echo "$1" | cut -d'|' -f4; }
review_vendor_refs() { echo "$1" | cut -d'|' -f5; }
review_files()       { echo "$1" | cut -d'|' -f6; }
review_prompt()      { echo "$1" | cut -d'|' -f7-; }

output_path_for() {
    local cat slug id
    cat=$(review_category "$1")
    id=$(review_id "$1")
    slug=$(review_slug "$1")
    echo "$REVIEWS_DIR/$cat/$id-$slug.md"
}

# ---------- optional: validate manifest schemas ----------
validate_manifests() {
    local ok=0
    for mf in "$MANIFEST_DIR"/*.json; do
        [ -f "$mf" ] || continue
        if ! jq -e '.category and (.reviews | type == "array")' "$mf" > /dev/null 2>&1; then
            echo "WARN: $mf does not match expected schema (needs .category and .reviews[])" >&2
            ok=1
        fi
    done
    return $ok
}

# ---------- main ----------
main() {
    local mode="${1:-all}"
    local filter="${2:-}"

    if [ "$mode" = "--list-categories" ]; then
        for mf in "$MANIFEST_DIR"/*.json; do
            [ -f "$mf" ] || continue
            local cat; cat=$(basename "$mf" .json)
            local count; count=$(jq '.reviews | length' "$mf")
            printf "%-25s %3d reviews\n" "$cat" "$count"
        done
        return 0
    fi

    validate_manifests || true  # warn but don't stop

    if [ "$mode" = "--dry-run" ]; then
        echo "=== DRY RUN: Would execute the following reviews ==="
        local entries
        mapfile -t entries < <(load_all_reviews)
        for entry in "${entries[@]}"; do
            local id cat title out
            id=$(review_id "$entry")
            cat=$(review_category "$entry")
            title=$(review_title "$entry")
            out=$(output_path_for "$entry")
            if [ -f "$out" ]; then
                echo "  SKIP ($out exists)  $id | $cat | $title"
            else
                echo "  RUN                 $id | $cat | $title  ->  $out"
            fi
        done
        echo "=== End dry run ==="
        return 0
    fi

    local filter_cat=""
    local filter_id=""
    if [ "$mode" = "--category" ]; then
        filter_cat="$filter"
    elif [ "$mode" != "all" ]; then
        filter_id="$mode"
    fi

    local count_skipped=0 count_ran=0 count_errors=0
    local entries
    mapfile -t entries < <(load_all_reviews)

    for entry in "${entries[@]}"; do
        local id cat
        id=$(review_id "$entry")
        cat=$(review_category "$entry")

        [ -n "$filter_cat" ] && [ "$cat" != "$filter_cat" ] && continue
        [ -n "$filter_id" ] && [ "$id" != "$filter_id" ] && continue

        local out; out=$(output_path_for "$entry")
        if [ -f "$out" ]; then
            echo "SKIP $id — $out already exists"
            ((count_skipped++)) || true
            continue
        fi

        local prompt title vendor files
        prompt=$(review_prompt "$entry")
        title=$(review_title "$entry")
        vendor=$(review_vendor_refs "$entry")
        files=$(review_files "$entry")

        local full_prompt
        full_prompt=$(cat <<PROMPT
You are performing a focused, single-concern code review of the HARES residential energy simulation codebase.

## Review: $title
## Review ID: $id
## Category: $cat

## HARES Source Files to Review
$files

## Vendor Reference Files (compare/contrast)
$vendor

## Review Instructions
$prompt

## Output Requirements
Write your findings to the file:
  $out

The output MUST be a markdown file with this structure:

# $title
**Review ID**: $id
**Category**: $cat
**Date**: $(date +%Y-%m-%d)

## Files Reviewed
$files

## Vendor/Reference Files Consulted
$vendor

## Findings
### Finding 1: [Severity: critical|high|medium|low]
**Description**: ...
**Code Location**: ...
**Root Cause**: ...
**Impact**: ...

## Summary
- Total findings: N
- Critical / High / Medium / Low: each count

## Recommendations
1. ...

## References / Citations
- ...
PROMPT
)

        echo ""
        echo "============================================================"
        echo "REVIEW $id: $title"
        echo "Output: $out"
        echo "============================================================"

        if run_opencode "$full_prompt" "$out"; then
            ((count_ran++)) || true
            echo "PASS $id"
        else
            ((count_errors++)) || true
            echo "FAIL $id (exit code $?)"
        fi
    done

    echo ""
    echo "============================================================"
    echo "SUMMARY: $count_ran run, $count_skipped skipped, $count_errors errors"
    echo "============================================================"
}

main "$@"
