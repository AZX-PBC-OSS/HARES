#!/usr/bin/env bash
# The benchmark regression gate's statistic and confirmation logic.
#
# The gate compares criterion's MEDIAN point estimate between two
# same-runner critcmp exports. The paired base-then-head structure lives
# in the workflow; this script owns only the comparison. The median is
# robust to a runner stall: one stalled sample drags a mean far past any
# 20 percent gate (run 40633875876: 911.3 +/- 956.47us on
# rl_episode/batch_step/step_and_observe/16 while the identical
# per-element path at /64 and /256 measured 1.00 exactly) but barely
# moves a median.
#
# A flagged bench is real only if the flag reproduces. The workflow asks
# this script for the criterion FILTER regex that re-runs exactly the
# flagged benchmarks (criterion takes a single positional filter, a
# regex matched against the benchmark id), re-exports, and re-compares
# against the same baseline with the same statistic. The job fails only
# when the flag reproduces on the confirmation re-run.
#
# Subcommands:
#   flag <baseline.json> <current.json> [median|mean]
#       Emit {"statistic", "threshold_ratio", "regressions": [...]} for
#       every benchmark whose current point estimate is more than 20
#       percent above the baseline's. Each regression carries both
#       medians, the percent delta, the current run's relative
#       uncertainty (std_dev and median_abs_dev over the current
#       estimate, same JSON, so a verdict attributes itself), and the
#       regex-escaped id for the confirmation filter.
#   filter <regressions.json>
#       Emit the single anchored alternation regex for criterion's
#       positional FILTER argument (empty when nothing is flagged).
#   table <regressions.json>
#       Emit one markdown row per flagged benchmark for the step summary.
#   test
#       Run the fixture matrix under fixtures/ and assert the behavior
#       end to end: a clean pair, a poisoned mean with a fine median,
#       a real regression that reproduces on the confirmation input, and
#       a first-pass flag that clears on confirmation. jq and bash only:
#       deciding the gate's verdict must never need a Rust build.
#
# critcmp --export shape (BurntSushi/critcmp src/data.rs):
#   {"name": ..., "benchmarks": {"<full_id>": {
#      "criterion_estimates_v1": {"mean"|"median"|"median_abs_dev"|"std_dev":
#        {"point_estimate": <f64 ns>, ...}, "slope": null|...}}}}

set -euo pipefail

THRESHOLD_RATIO="1.20"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# The single comparison program. $stat selects the estimate (median in
# the gate, mean only in the test matrix to show the old false positive).
run_flag() {
    local base="$1" curr="$2" stat="$3"
    jq -n \
        --arg stat "$stat" \
        --argjson ratio "$THRESHOLD_RATIO" \
        --slurpfile base "$base" \
        --slurpfile curr "$curr" \
        '
        def re_escape: gsub("(?<c>[\\[\\]{}().*+?|^$\\\\])"; "\\\(.c)");
        {
          statistic: $stat,
          threshold_ratio: $ratio,
          regressions:
            [ $base[0].benchmarks | to_entries[] |
              . as $entry |
              ($curr[0].benchmarks[$entry.key].criterion_estimates_v1[$stat].point_estimate // empty) as $cns |
              ($entry.value.criterion_estimates_v1[$stat].point_estimate) as $bns |
              select($cns / $bns > $ratio) |
              {
                name: $entry.key,
                baseline_ns: $bns,
                current_ns: $cns,
                pct_slower: (((($cns / $bns - 1) * 1000) | round) / 10),
                re: ($entry.key | re_escape),
                rel_stddev_pct: ((((($curr[0].benchmarks[$entry.key].criterion_estimates_v1.std_dev.point_estimate // 0) / $cns) * 1000) | round) / 10),
                rel_mad_pct: ((((($curr[0].benchmarks[$entry.key].criterion_estimates_v1.median_abs_dev.point_estimate // 0) / $cns) * 1000) | round) / 10)
              } ]
        }'
}

cmd_flag() {
    local base="$1" curr="$2" stat="${3:-median}"
    case "$stat" in
        median|mean) ;;
        *) echo "statistic must be median or mean, got: $stat" >&2; exit 2 ;;
    esac
    run_flag "$base" "$curr" "$stat"
}

cmd_filter() {
    local payload="${1:-$(cat)}"
    jq -r '
      if (.regressions | length) == 0 then ""
      else "^(?:" + ([.regressions[].re] | join("|")) + ")$"
      end' <<<"$payload"
}

cmd_table() {
    local payload="${1:-$(cat)}"
    echo '| benchmark | baseline median (ns) | current median (ns) | delta | rel stddev | rel MAD |'
    echo '|---|---:|---:|---:|---:|---:|'
    jq -r '.regressions[] |
      "| `\(.name)` | \(.baseline_ns) | \(.current_ns) | +\(.pct_slower)% | \(.rel_stddev_pct)% | \(.rel_mad_pct)% |"' <<<"$payload"
}

cmd_test() {
    local fx="$SCRIPT_DIR/fixtures"
    local failures=0 pass=0

    check() {
        local label="$1" expect="$2" got="$3"
        if [ "$expect" = "$got" ]; then
            echo "PASS  $label (got $got)"
            pass=$((pass + 1))
        else
            echo "FAIL  $label (expected $expect, got $got)"
            failures=$((failures + 1))
        fi
    }

    count() { jq '.regressions | length'; }
    names() { jq -r '[.regressions[].name] | sort | join(",")'; }

    echo "== fixture matrix =="

    # 1. Clean pair: nothing flagged under either statistic.
    check "clean pair, median" 0 \
        "$(run_flag "$fx/clean-baseline.json" "$fx/clean-current.json" median | count)"
    check "clean pair, mean" 0 \
        "$(run_flag "$fx/clean-baseline.json" "$fx/clean-current.json" mean | count)"

    # 2. The recorded false positive: the current mean is poisoned by a
    #    runner stall, the median is fine, /64 and /256 are at 1.00
    #    exactly. The old mean gate flags +30.3%; the median gate must
    #    pass.
    check "poisoned mean fixture reproduces the old mean-gate false positive" 1 \
        "$(run_flag "$fx/poisoned-baseline.json" "$fx/poisoned-current.json" mean | count)"
    check "poisoned mean fixture: old gate flags the recorded bench" \
        "rl_episode/batch_step/step_and_observe/16" \
        "$(run_flag "$fx/poisoned-baseline.json" "$fx/poisoned-current.json" mean | names)"
    check "poisoned mean fixture: old gate records +30.3%" "30.3" \
        "$(run_flag "$fx/poisoned-baseline.json" "$fx/poisoned-current.json" mean | jq -r '.regressions[0].pct_slower')"
    check "poisoned mean fixture, median" 0 \
        "$(run_flag "$fx/poisoned-baseline.json" "$fx/poisoned-current.json" median | count)"

    # 3. A real regression: the median is +30% and it reproduces on the
    #    confirmation input. First pass flags, confirmation flags, job
    #    fails.
    local real_first real_confirm real_cleared
    real_first="$(run_flag "$fx/real-baseline.json" "$fx/real-current.json" median)"
    real_confirm="$(run_flag "$fx/real-baseline.json" "$fx/real-confirmed.json" median)"
    real_cleared="$(run_flag "$fx/real-baseline.json" "$fx/real-cleared-confirmed.json" median)"
    check "real regression: first pass flags" 2 "$(count <<<"$real_first")"
    check "real regression: confirmation still flags (job fails)" 2 "$(count <<<"$real_confirm")"
    check "real regression: confirmation flags the same benches" \
        "$(names <<<"$real_first")" "$(names <<<"$real_confirm")"

    # 4. A first-pass flag that clears on confirmation passes the job.
    check "first-pass flag cleared on confirmation: 0 flags (job passes)" 0 \
        "$(count <<<"$real_cleared")"

    # 5. The confirmation filter re-runs ONLY the flagged benchmarks: it
    #    matches every flagged id (including metacharacters and an
    #    apostrophe in the id) and no clean id.
    local bench_filter
    bench_filter="$(cmd_filter <<<"$real_confirm")"
    local flagged_match clean_match
    flagged_match="$(jq -r --arg f "$bench_filter" '
        [ .regressions[].name | select(test($f)) ] | length' <<<"$real_confirm")"
    check "confirmation filter matches every flagged id" "2" "$flagged_match"
    clean_match="$(jq -n --arg f "$bench_filter" --slurpfile base "$fx/real-baseline.json" \
        '[ $base[0].benchmarks | keys[] | select(test($f)) ] | length')"
    check "confirmation filter matches exactly the flagged baseline keys" "2" "$clean_match"
    check "confirmation filter does not match the unflagged bench" 0 \
        "$(jq -n --arg f "$bench_filter" 'if ("single_building/simulate/16" | test($f)) then 1 else 0 end')"
    check "confirmation filter is anchored and alternated" 1 \
        "$(case "$bench_filter" in '^(?:'*) case "$bench_filter" in *')$') echo 1;; *) echo 0;; esac;; *) echo 0;; esac)"

    # 6. The table renders both medians, the delta, and the current
    #    run's relative uncertainty columns.
    local table_out
    table_out="$(cmd_table <<<"$real_confirm")"
    check "table renders the flagged bench" 1 \
        "$(grep -c 'rl_episode/batch_step/step_and_observe/16' <<<"$table_out")"
    check "table renders the baseline median" 1 \
        "$(grep -c '| 698900' <<<"$table_out")"
    check "table renders the rel stddev column" 1 \
        "$(grep -c 'rel stddev' <<<"$table_out")"

    echo ""
    if [ "$failures" -gt 0 ]; then
        echo "FAILED: $failures assertion(s), $pass passed"
        return 1
    fi
    echo "OK: all $pass assertions passed"
}

case "${1:-}" in
    flag)   shift; cmd_flag "$@" ;;
    filter) shift; cmd_filter "$@" ;;
    table)  shift; cmd_table "$@" ;;
    test)   shift; cmd_test "$@" ;;
    *) echo "usage: bench-gate.sh {flag <base> <curr> [median|mean] | filter <regressions.json> | table <regressions.json> | test}" >&2; exit 2 ;;
esac
