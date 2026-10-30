#!/usr/bin/env bash
# The benchmark regression gate's statistic, its interleaved pairing and
# its confirmation logic.
#
# The gate compares criterion's MEDIAN point estimate between the base
# tree's and the head tree's benchmark run. The median is robust to a
# runner stall: one stalled sample drags a mean far past any 20 percent
# gate (run 40633875876: 911.3 +/- 956.47us on
# rl_episode/batch_step/step_and_observe/16 while the identical
# per-element path at /64 and /256 measured 1.00 exactly) but barely
# moves a median.
#
# The comparison is PAIRED: base and head are separate binaries (each
# tree builds its own), and the rounds alternate base/head per benchmark
# so both sides sample the same environment. A sitting's environment is
# not constant (CPU steal, page-fault behaviour, allocator state), and a
# sequential base phase followed by a head phase charges every drift to
# the head: rl_step/dwelling_step_single measured 36.6us on the base and
# 3.5ms on the head of one job (a stable +9455 percent with a 3.4
# percent relative stddev) while the same two trees measure
# indistinguishable locally. The confirmation re-run could not catch it,
# because it re-ran in the same shifted sitting. Under the interleaved
# design both samples of a benchmark come from one round, so a drift
# lands on both sides of the round it starts in instead of being charged
# to whichever side runs second.
#
# The job screens with ONE interleaved round: the whole suite takes tens
# of minutes on the runner, and a run per side per round is the most the
# job's wall time affords. A flagged bench is real only if it
# reproduces: the confirmation re-runs exactly the flagged benchmarks
# (criterion's single positional FILTER argument, an anchored
# alternation of the flagged ids) in THREE fresh interleaved rounds,
# order flipped each round, and the job fails only when the median of the
# confirmation rounds' ratios also exceeds the threshold. The screen is
# one round wide, so its flag is a question; the confirmation's median is
# the answer.
#
# Subcommands:
#   build-binaries <out-dir>
#       Build every benchmark target and copy each executable into
#       <out-dir> under its target name. Run from the tree whose
#       binaries are wanted; a target the tree cannot build is a warning
#       (the base's own fixtures may predate it) and its benchmarks are
#       reported unpaired, never silently compared.
#   rounds <first> <count> [filter]
#       Run <count> interleaved rounds, starting at round number <first>,
#       of every benchmark in target/base-bench and target/head-bench,
#       alternating the two sides per benchmark and flipping their order
#       each round. Each round saves the criterion baselines base-r<k>
#       and head-r<k> and exports them to
#       target/bench-rounds/{base,head}-r<k>.json. The optional filter is
#       criterion's positional FILTER (the confirmation re-runs only the
#       flagged ids). A side whose binary is missing or fails to run is a
#       warning and its benchmarks are unpaired this round; an export
#       that records no benchmark data is an error, never a completed
#       round. Run from the workspace root: critcmp reads
#       ./target/criterion, where the binaries write.
#   paired <base-1.json> <head-1.json> [<base-2.json> <head-2.json> ...]
#       Emit {"statistic", "threshold_ratio", "rounds", "regressions",
#       "unpaired"}: for every benchmark present in every round, the
#       median of its per-round head/base ratios and the median of each
#       side's round medians; a regression is a bench whose ratio median
#       exceeds the threshold. rel stddev and rel MAD are the head
#       side's round-to-round spread (the sitting's noise, measured in
#       the same rounds the ratio comes from; null on a one-round
#       screen). A bench without a sample in every round is listed under
#       "unpaired" and never gated.
#   flag <baseline.json> <current.json> [median|mean]
#       The single-pair comparison (one base export against one current
#       export): the same ratio and threshold without the pairing. Kept
#       for a local one-off comparison; the gate uses paired.
#   filter <regressions.json>
#       Emit the single anchored alternation regex for criterion's
#       positional FILTER argument (empty when nothing is flagged).
#   table <regressions.json>
#       Emit one markdown row per flagged benchmark for the step summary.
#   test
#       Run the fixture matrix and assert the behavior end to end: a
#       clean pair, a poisoned mean with a fine median, a real
#       regression that reproduces on the confirmation rounds, a
#       single-round drift that must not flag, the recorded within-job
#       drift (a sequential base-then-head phase shift) that the paired
#       design absorbs, and an unpaired new benchmark. jq and bash only:
#       deciding the gate's verdict must never need a Rust build.
#
# critcmp --export shape (BurntSushi/critcmp src/data.rs):
#   {"name": ..., "benchmarks": {"<full_id>": {
#      "criterion_estimates_v1": {"mean"|"median"|"median_abs_dev"|"std_dev":
#        {"point_estimate": <f64 ns>, ...}, "slope": null|...}}}}

set -euo pipefail

THRESHOLD_RATIO="1.20"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# The benchmark targets the gate runs, in the order cargo benches them.
# One list, owned here, so the workflow names none of them.
BENCH_TARGETS=(
    single_building fleet rl_step rc_solver metrics_request
    rl_episode output_row_plan diagnostics_report thermal_step checkpoint
)

# The one ratio program both comparisons share. $stat selects the
# estimate (median in the gate, mean only in the test matrix to show the
# old false positive).
JQ_RATIOS='
def median:
  sort as $s
  | ($s | length) as $n
  | if $n == 0 then null
    elif ($n % 2) == 1 then $s[($n - 1) / 2]
    else (($s[$n / 2 - 1] + $s[$n / 2]) / 2)
    end;

def mean: if length == 0 then null else add / length end;

def re_escape: gsub("(?<c>[\\[\\]{}().*+?|^$\\\\])"; "\\\(.c)");

# One entry per benchmark both documents carry, with the current over
# baseline ratio.
def ratios($base; $curr; $stat):
  [ $base.benchmarks | to_entries[] |
    . as $entry |
    ($entry.value.criterion_estimates_v1[$stat].point_estimate) as $bns |
    ($curr.benchmarks[$entry.key].criterion_estimates_v1[$stat].point_estimate) as $cns |
    select($bns != null and $cns != null and $bns > 0) |
    { name: $entry.key, baseline_ns: $bns, current_ns: $cns, ratio: ($cns / $bns) } ];
'

# Single pair: every benchmark over the threshold, with the current run's
# own uncertainty (criterion's std_dev and median_abs_dev over the
# current estimate, from the same JSON, so a verdict attributes itself).
run_flag() {
    local base="$1" curr="$2" stat="$3"
    jq -n \
        --arg stat "$stat" \
        --argjson ratio "$THRESHOLD_RATIO" \
        --slurpfile base "$base" \
        --slurpfile curr "$curr" \
        "$JQ_RATIOS"'
        {
          statistic: $stat,
          threshold_ratio: $ratio,
          regressions:
            [ ratios($base[0]; $curr[0]; $stat)[] |
              select(.ratio > $ratio) |
              . as $r |
              {
                name: $r.name,
                baseline_ns: $r.baseline_ns,
                current_ns: $r.current_ns,
                pct_slower: (((($r.ratio - 1) * 1000) | round) / 10),
                re: ($r.name | re_escape),
                rel_stddev_pct: ((((($curr[0].benchmarks[$r.name].criterion_estimates_v1.std_dev.point_estimate // 0) / $r.current_ns) * 1000) | round) / 10),
                rel_mad_pct: ((((($curr[0].benchmarks[$r.name].criterion_estimates_v1.median_abs_dev.point_estimate // 0) / $r.current_ns) * 1000) | round) / 10)
              } ]
        }'
}

# The paired comparison: the documents arrive alternating base/head, one
# pair per round. The statistic is the median of the per-round ratios;
# a benchmark missing from any round is reported, not gated.
run_paired() {
    local stat="median"
    jq -s \
        --arg stat "$stat" \
        --argjson ratio "$THRESHOLD_RATIO" \
        "$JQ_RATIOS"'
        def rel_stddev_pct:
          length as $n
          | mean as $m
          | if $n < 2 or $m <= 0 then 0
            else (((((map((. - $m) * (. - $m)) | add / $n) | sqrt) / $m) * 1000) | round) / 10
            end;

        def rel_mad_pct:
          median as $md
          | if $md <= 0 then 0
            else (((map((. - $md) | fabs) | median) / $md) * 1000 | round) / 10
            end;

        . as $docs
        | if ($docs | length) % 2 != 0 or ($docs | length) == 0 then
            error("paired needs an even number of export documents, alternating base/head")
          else . end
        | [ range(0; $docs | length; 2) | ratios($docs[.]; $docs[. + 1]; $stat) ] as $rounds
        | ($rounds | length) as $n_rounds
        | [ ($rounds | map(map(.name)) | add | unique)[] as $name
            | [ $rounds[] | .[] | select(.name == $name) ] as $hits
            | ($hits | map(.current_ns)) as $head_ns
            | { name: $name,
                samples: ($hits | length),
                ratio: ($hits | map(.ratio) | median),
                baseline_ns: ($hits | map(.baseline_ns) | median),
                current_ns: ($head_ns | median),
                per_round_pct: ($hits | map(((.ratio - 1) * 1000 | round) / 10)),
                rel_stddev_pct: ($head_ns | if length < 2 then null else rel_stddev_pct end),
                rel_mad_pct: ($head_ns | if length < 2 then null else rel_mad_pct end) } ] as $scored
        | [ $docs[] | .benchmarks | keys[] ] | unique as $all_names
        | [ $scored[] | select(.samples == $n_rounds) | .name ] as $gated
        | { statistic: $stat,
            threshold_ratio: $ratio,
            rounds: $n_rounds,
            regressions:
              [ $scored[] |
                select(.samples == $n_rounds and .ratio > $ratio) |
                { name: .name,
                  baseline_ns: .baseline_ns,
                  current_ns: .current_ns,
                  pct_slower: ((((.ratio - 1) * 1000) | round) / 10),
                  per_round_pct: .per_round_pct,
                  re: (.name | re_escape),
                  rel_stddev_pct: .rel_stddev_pct,
                  rel_mad_pct: .rel_mad_pct } ],
            unpaired: [ $all_names[] | select(. as $n | ($gated | index($n)) == null) ] }' "$@"
}

cmd_flag() {
    local base="$1" curr="$2" stat="${3:-median}"
    case "$stat" in
        median|mean) ;;
        *) echo "statistic must be median or mean, got: $stat" >&2; exit 2 ;;
    esac
    run_flag "$base" "$curr" "$stat"
}

cmd_paired() {
    if [ "$#" -lt 2 ] || [ $(( $# % 2 )) -ne 0 ]; then
        echo "paired needs base/head export pairs: paired <base-1> <head-1> [<base-2> <head-2> ...]" >&2
        exit 2
    fi
    run_paired "$@"
}

cmd_build_binaries() {
    local out="${1:?build-binaries needs an output directory}"
    mkdir -p "$out"
    local target manifest copied=0
    for target in "${BENCH_TARGETS[@]}"; do
        # One cargo invocation per target: a target this tree does not
        # have must not take the other targets' binaries with it.
        if ! manifest="$(cargo bench --no-run --message-format=json --bench "$target")"; then
            echo "::warning::bench target '$target' does not build here: its benchmarks are unpaired" >&2
            continue
        fi
        local exe
        exe="$(jq -r --arg t "$target" '
            select(.reason == "compiler-artifact"
                   and .executable != null
                   and .target.name == $t
                   and (.target.kind | index("bench")))
            | .executable' <<<"$manifest" | tail -1)"
        if [ -z "$exe" ] || [ "$exe" = "null" ]; then
            echo "::warning::bench target '$target' produced no executable: its benchmarks are unpaired" >&2
            continue
        fi
        cp -f "$exe" "$out/$target"
        copied=$((copied + 1))
    done
    if [ "$copied" -eq 0 ]; then
        echo "no benchmark binaries built into $out" >&2
        return 1
    fi
    echo "built $copied benchmark binaries into $out"
}

cmd_rounds() {
    local first="${1:?rounds needs a first round number}"
    local count="${2:?rounds needs a round count}"
    local filter="${3:-}"
    local bin_dir_base="target/base-bench"
    local bin_dir_head="target/head-bench"
    local out="target/bench-rounds"
    [ -d "$bin_dir_head" ] || { echo "no head bench binaries at $bin_dir_head" >&2; return 2; }
    [ -d "$bin_dir_base" ] || { echo "no base bench binaries at $bin_dir_base" >&2; return 2; }
    mkdir -p "$out"

    local k r side target
    for k in $(seq "$first" $((first + count - 1))); do
        for target in "${BENCH_TARGETS[@]}"; do
            # Odd rounds run base then head, even rounds the reverse: a
            # positional effect (the sitting's own warm-up, a co-tenant
            # arriving) lands on the second side of its own round instead
            # of accumulating on one side across the job. An odd round
            # count cannot balance the two orders exactly (the
            # confirmation's three rounds run head-first twice and
            # base-first once); what survives is one round's positional
            # effect, and the spread columns report exactly that
            # round-to-round movement.
            if [ $((k % 2)) -eq 1 ]; then
                for side in base head; do
                    run_round "$side" "$target" "$k" "$filter"
                done
            else
                for side in head base; do
                    run_round "$side" "$target" "$k" "$filter"
                done
            fi
        done
    done

    for r in $(seq "$first" $((first + count - 1))); do
        for side in base head; do
            local f="$out/$side-r$r.json"
            # A critcmp that exits 0 having recorded nothing (a truncated
            # write) or a document whose shape it cannot read leaves no
            # count below: both are missing data, never a clean round.
            local recorded=0
            if critcmp --export "$side-r$r" > "$f" 2>/dev/null; then
                recorded="$(jq -r '(.benchmarks // {}) | length' "$f" 2>/dev/null)" \
                    || recorded=0
            fi
            if [ -z "$recorded" ] || [ "$recorded" -eq 0 ]; then
                echo "::error::no $side benchmark data was recorded for round $r" >&2
                return 1
            fi
        done
    done
    echo "wrote the base and head exports for rounds $(seq -s, "$first" $((first + count - 1))) to $out"
}

run_round() {
    local side="$1" target="$2" round="$3" filter="${4:-}"
    local bin="target/$side-bench/$target"
    if [ ! -x "$bin" ]; then
        # A side without a binary (a base tree that predates the target)
        # still runs its opposite: the half pair reports as unpaired
        # instead of the target vanishing from the gate.
        echo "::warning::$side/$target has no binary: its benchmarks are unpaired this round" >&2
        return 0
    fi
    local args=(--bench --save-baseline "$side-r$round")
    if [ -n "$filter" ]; then
        args+=("$filter")
    fi
    echo "== round $round $side/$target"
    if ! "$bin" "${args[@]}"; then
        echo "::warning::$side/$target failed in round $round: its benchmarks are unpaired this round" >&2
    fi
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
    echo '| benchmark | baseline median (ns) | current median (ns) | delta | per-round (head/base) | rel stddev | rel MAD |'
    echo '|---|---:|---:|---:|---|---:|---:|'
    jq -r '.regressions[] |
      "| `\(.name)` | \(.baseline_ns) | \(.current_ns) | +\(.pct_slower)% | \((.per_round_pct // []) as $r | if ($r | length) == 0 then "n/a" else ([$r[] | select(. != null) | "+\(.)%"] | join(", ")) end) | \(if .rel_stddev_pct == null then "n/a" else "\(.rel_stddev_pct)%" end) | \(if .rel_mad_pct == null then "n/a" else "\(.rel_mad_pct)%" end) |"' <<<"$payload"
}

cmd_test() {
    local fx="$SCRIPT_DIR/fixtures"
    local failures=0 pass=0
    local scratch
    scratch="$(mktemp -d)"
    # shellcheck disable=SC2064
    trap "rm -rf '$scratch'" RETURN

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

    # One round's export: every criterion estimate of the source scaled by
    # the factor. The knob models whatever moved the sitting: a code
    # change (both sides of a pair) or an environment shift (both sides
    # from the round it starts in).
    scale() {
        jq --argjson s "$2" '
          .benchmarks |= with_entries(
            .value.criterion_estimates_v1 |= with_entries(
              if .value == null then . else .value.point_estimate *= $s end
            )
          )' "$1" > "$3"
    }

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

    # 3. The recorded within-job drift: a sitting whose environment
    #    shifts after round 1 (the shift holds for the rest of the job).
    #    The sequential design charged the whole shift to the head: base
    #    round 1 at 1x, head round 3 at 96x is +9500 percent, the shape
    #    of the rl_step/dwelling_step_single flag this gate's pairing
    #    exists to stop. The paired design sees the same 96/95 ratio in
    #    the shifted rounds it shares with the base.
    scale "$fx/clean-baseline.json" 1 "$scratch/base-r1.json"
    scale "$fx/clean-baseline.json" 1 "$scratch/head-r1.json"
    scale "$fx/clean-baseline.json" 95 "$scratch/base-r2.json"
    scale "$fx/clean-baseline.json" 96 "$scratch/head-r2.json"
    scale "$fx/clean-baseline.json" 95 "$scratch/base-r3.json"
    scale "$fx/clean-baseline.json" 96 "$scratch/head-r3.json"

    local drift_seq drift_paired
    drift_seq="$(run_flag "$scratch/base-r1.json" "$scratch/head-r3.json" median)"
    drift_paired="$(run_paired "$scratch/base-r1.json" "$scratch/head-r1.json" \
        "$scratch/base-r2.json" "$scratch/head-r2.json" \
        "$scratch/base-r3.json" "$scratch/head-r3.json")"
    check "sequential phases reproduce the recorded drift as a flag" 3 \
        "$(count <<<"$drift_seq")"
    check "sequential phases record the drift's magnitude" "+9500" \
        "$(jq -r '.regressions[0].pct_slower | "+\(.)"' <<<"$drift_seq")"
    check "interleaved pairing absorbs the within-job drift: 0 flags" 0 \
        "$(count <<<"$drift_paired")"
    check "interleaved pairing reports the rounds it compared" 3 \
        "$(jq -r '.rounds' <<<"$drift_paired")"

    # The screen is one round, and it pairs its own samples: the same
    # round's base and head agree, and the spread columns report n/a
    # rather than a fabricated 0.0 (one round has no round-to-round
    # spread).
    local screen
    screen="$(run_paired "$scratch/base-r1.json" "$scratch/head-r1.json")"
    check "one-round screen pairs its own samples: 0 flags" 0 "$(count <<<"$screen")"
    check "one-round screen reports one round" 1 "$(jq -r '.rounds' <<<"$screen")"

    # 4. A real regression: the committed pair's two slower benches
    #    (+27.7 percent by the fixture's own medians), repeated in every
    #    round. All rounds agree, so the paired median
    #    flags; the confirmation rounds (4-6) repeat it, and the job fails.
    local r
    for r in 1 2 3 4 5 6; do
        cp "$fx/real-baseline.json" "$scratch/rbase-r$r.json"
        cp "$fx/real-confirmed.json" "$scratch/rhead-r$r.json"
    done
    local real_paired real_confirmed
    real_paired="$(run_paired "$scratch/rbase-r1.json" "$scratch/rhead-r1.json" \
        "$scratch/rbase-r2.json" "$scratch/rhead-r2.json" \
        "$scratch/rbase-r3.json" "$scratch/rhead-r3.json")"
    real_confirmed="$(run_paired "$scratch/rbase-r4.json" "$scratch/rhead-r4.json" \
        "$scratch/rbase-r5.json" "$scratch/rhead-r5.json" \
        "$scratch/rbase-r6.json" "$scratch/rhead-r6.json")"
    check "real regression: every paired round flags" 2 "$(count <<<"$real_paired")"
    check "real regression: the ratio median records the fixture's delta" "27.7" \
        "$(jq -r '.regressions[0].pct_slower' <<<"$real_paired")"
    check "real regression: the per-round ratios agree" "27.7,27.7,27.7" \
        "$(jq -r '.regressions[0].per_round_pct | map(tostring) | join(",")' <<<"$real_paired")"
    check "real regression: confirmation rounds still flag (job fails)" 2 \
        "$(count <<<"$real_confirmed")"
    check "real regression: confirmation flags the same benches" \
        "$(names <<<"$real_paired")" "$(names <<<"$real_confirmed")"

    # 5. A shifted round is not a regression: it moved both sides of its
    #    own round, so the ratio it carries is still 1.00. The round rides
    #    on the drift scenario's files (round 3 still carries the 95/96
    #    shift), and the median of the three per-round ratios is still
    #    1.00; the sequential view of the same data (round 1's base
    #    against round 2's head) is +50 percent.
    scale "$fx/clean-baseline.json" 1.5 "$scratch/base-r2.json"
    scale "$fx/clean-baseline.json" 1.5 "$scratch/head-r2.json"
    local one_round one_round_seq
    one_round="$(run_paired "$scratch/base-r1.json" "$scratch/head-r1.json" \
        "$scratch/base-r2.json" "$scratch/head-r2.json" \
        "$scratch/base-r3.json" "$scratch/head-r3.json")"
    one_round_seq="$(run_flag "$scratch/base-r1.json" "$scratch/head-r2.json" median)"
    check "one shifted round does not flag" 0 "$(count <<<"$one_round")"
    check "one shifted round leaves every bench paired" 0 \
        "$(jq -r '.unpaired | length' <<<"$one_round")"
    check "the same shift through sequential phases reads +50%" "+50" \
        "$(jq -r '.regressions[0].pct_slower | "+\(.)"' <<<"$one_round_seq")"

    # 6. The job's whole flow: a screen round whose pair caught a drift
    #    mid-pair flags (+50 percent above), and the confirmation rounds
    #    pair the same trees three more times and clear it, so the job
    #    passes.
    local screen_flag
    screen_flag="$(run_paired "$scratch/base-r1.json" "$scratch/head-r2.json")"
    check "screen flags a drift that split its own pair" 3 \
        "$(count <<<"$screen_flag")"
    check "one-round table reports no round spread" 3 \
        "$(cmd_table <<<"$screen_flag" | grep -c '| n/a | n/a |')"
    for r in 4 5 6; do
        cp "$fx/clean-baseline.json" "$scratch/cbase-r$r.json"
        cp "$fx/clean-current.json" "$scratch/chead-r$r.json"
    done
    local cleared
    cleared="$(run_paired "$scratch/cbase-r4.json" "$scratch/chead-r4.json" \
        "$scratch/cbase-r5.json" "$scratch/chead-r5.json" \
        "$scratch/cbase-r6.json" "$scratch/chead-r6.json")"
    check "first-pass flag cleared on confirmation: 0 flags (job passes)" 0 \
        "$(count <<<"$cleared")"

    # 7. An unpaired benchmark (new on the head side, so no base sample)
    #    is reported and never gated: it has no ratio to judge.
    jq '.benchmarks["rl_step/new_bench"] = .benchmarks["single_building/simulate/16"]' \
        "$scratch/head-r1.json" > "$scratch/head-r1-new.json"
    local unpaired
    unpaired="$(run_paired "$scratch/base-r1.json" "$scratch/head-r1-new.json" \
        "$scratch/base-r3.json" "$scratch/head-r3.json")"
    check "unpaired benchmark is listed" "rl_step/new_bench" \
        "$(jq -r '.unpaired | join(",")' <<<"$unpaired")"
    check "unpaired benchmark is not gated" 0 \
        "$(jq -r '[.regressions[] | select(.name == "rl_step/new_bench")] | length' <<<"$unpaired")"

    # 8. The confirmation filter re-runs ONLY the flagged benchmarks: it
    #    matches every flagged id (including metacharacters and an
    #    apostrophe in the id) and no clean id.
    local bench_filter
    bench_filter="$(cmd_filter <<<"$real_paired")"
    local flagged_match clean_match
    flagged_match="$(jq -r --arg f "$bench_filter" '
        [ .regressions[].name | select(test($f)) ] | length' <<<"$real_paired")"
    check "confirmation filter matches every flagged id" "2" "$flagged_match"
    clean_match="$(jq -n --arg f "$bench_filter" --slurpfile base "$fx/real-baseline.json" \
        '[ $base[0].benchmarks | keys[] | select(test($f)) ] | length')"
    check "confirmation filter matches exactly the flagged baseline keys" "2" "$clean_match"
    check "confirmation filter does not match the unflagged bench" 0 \
        "$(jq -n --arg f "$bench_filter" 'if ("single_building/simulate/16" | test($f)) then 1 else 0 end')"
    check "confirmation filter is anchored and alternated" 1 \
        "$(case "$bench_filter" in '^(?:'*) case "$bench_filter" in *')$') echo 1;; *) echo 0;; esac;; *) echo 0;; esac)"

    # 9. The table renders both sides' medians, the delta, the per-round
    #    ratios and the head side's round-to-round spread.
    local table_out
    table_out="$(cmd_table <<<"$real_confirmed")"
    check "table renders the flagged bench" 1 \
        "$(grep -c 'rl_episode/batch_step/step_and_observe' <<<"$table_out")"
    check "table renders the baseline median" 1 \
        "$(grep -c '| 698900' <<<"$table_out")"
    check "table renders the per-round column" 1 \
        "$(grep -c 'per-round (head/base)' <<<"$table_out")"
    check "table renders the rel stddev column" 1 \
        "$(grep -c 'rel stddev' <<<"$table_out")"

    # 10. The round's export guard: a critcmp that exits 0 having recorded
    #     nothing (a truncated write) or a document whose shape it cannot
    #     read leaves no count, and the round fails. Stub binaries in a
    #     scratch workspace write no criterion data, so the export is the
    #     script's own read.
    local stub="$scratch/stub"
    mkdir -p "$stub/target/base-bench" "$stub/target/head-bench" "$scratch/bin"
    local target
    for target in "${BENCH_TARGETS[@]}"; do
        printf '#!/bin/sh\nexit 0\n' > "$stub/target/base-bench/$target"
        printf '#!/bin/sh\nexit 0\n' > "$stub/target/head-bench/$target"
        chmod +x "$stub/target/base-bench/$target" "$stub/target/head-bench/$target"
    done
    local stub_rc
    stub_rc=0
    ( cd "$stub" && bash "$SCRIPT_DIR/bench-gate.sh" rounds 1 1 ) \
        >/dev/null 2>"$scratch/stub-err" || stub_rc=$?
    check "a round with no export data is an error" 1 \
        "$(grep -c 'no base benchmark data was recorded for round 1' <<<"$(cat "$scratch/stub-err")")"
    check "a round with no export data fails the gate" 1 "$stub_rc"

    # A critcmp that succeeds but writes nothing is missing data too: the
    # guard reads no count from the file and the round fails, rather than
    # reporting a completed round with no benchmarks.
    printf '#!/bin/sh\nexit 0\n' > "$scratch/bin/critcmp"
    chmod +x "$scratch/bin/critcmp"
    stub_rc=0
    ( cd "$stub" && PATH="$scratch/bin:$PATH" bash "$SCRIPT_DIR/bench-gate.sh" rounds 1 1 ) \
        >/dev/null 2>"$scratch/empty-err" || stub_rc=$?
    check "an export with no benchmark data is an error" 1 \
        "$(grep -c 'no base benchmark data was recorded for round 1' \
            <<<"$(cat "$scratch/empty-err")")"
    check "an export with no benchmark data fails the gate" 1 "$stub_rc"

    echo ""
    if [ "$failures" -gt 0 ]; then
        echo "FAILED: $failures assertion(s), $pass passed"
        return 1
    fi
    echo "OK: all $pass assertions passed"
}

case "${1:-}" in
    build-binaries) shift; cmd_build_binaries "$@" ;;
    rounds)         shift; cmd_rounds "$@" ;;
    paired)         shift; cmd_paired "$@" ;;
    flag)           shift; cmd_flag "$@" ;;
    filter)         shift; cmd_filter "$@" ;;
    table)          shift; cmd_table "$@" ;;
    test)           shift; cmd_test "$@" ;;
    *) echo "usage: bench-gate.sh {build-binaries <out> | rounds <first> <count> [filter] | paired <base-1> <head-1> [...] | flag <base> <curr> [median|mean] | filter <regressions.json> | table <regressions.json> | test}" >&2; exit 2 ;;
esac
