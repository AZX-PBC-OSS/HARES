---
review_outcome: clean
---

# Review — I-04: EV driver telemetry reflects charging (re-review)

This round re-reviews the tree after the prior review's Warning was addressed.
The prior finding — `needed_charge_hours` frozen at the driver's belief during
a home charging session — is **resolved**: the fold now runs on the *observed*
equipment SOC via `telemetry_estimate_context` (mod.rs:836), while dispatch
keeps the driver's belief via `decision_context` (mod.rs:810-833). The failing
test that proved the original defect now passes, and the implementer added
three tests pinning the split at both ends plus an away-charging case. Full
verification below is green.

## Why I found nothing

I re-ran each attack rather than trusting the prior round.

- **The original defect (belief-frozen channel).** Re-ran
  `needed_charge_hours_decreases_while_equipment_charges_at_home` — the test I
  wrote last round — and it now **passes**; the estimate falls as observed SOC
  climbs 0.5 → 0.619. Mechanism confirmed by reading `decide`: the step-start
  fold uses `telemetry_estimate_context` (mod.rs:940), which substitutes
  `actual_soc(env)` into `current_soc` (mod.rs:836-848). `evaluate` was made
  dispatch-only so its perceived-SOC context cannot clobber the observed
  estimate (composer.rs:83-90).
- **Dispatch/telemetry split leak (the fix going too far).** If the fix had
  pushed observed SOC into the *control* context, the driver's documented
  belief-based behavior would change. It did not: `decision_context` still
  sets `current_soc: self.perceived_soc()` (mod.rs:816), and the new test
  `evaluate_charging_uses_perceived_soc_for_decision_context` (mod.rs:5081)
  pins that a low-belief/high-truth actor still dispatches a charge signal.
  Read it; consistent.
- **Away phase regression.** The Away arm was historically the one most likely
  to hold a stale value. The new test
  `needed_charge_hours_decreases_during_away_charging` (mod.rs:2496) drives a
  60-step workplace charge and asserts both that the driver's belief is static
  (precondition — so the fall must come from the observation) and that the
  estimate falls. Passes.
- **Anxiety-override estimate now consistent with `soc`/`charge_kw`.**
  `record_anxiety_plan_hours` (mod.rs:852-857) now computes charge-to-full from
  the observed SOC; `needed_charge_hours_under_anxiety_reports_observed_gap_not_belief`
  (mod.rs:2453) pins the divergent case (belief 0.2, truth 0.6 → ≈3.72 h, not
  ≈7.42 h). The rolled-over day-rollover test
  `needed_charge_hours_on_day_rollover_reflects_observed_charged_soc` was
  correctly updated to assert the observed at-target gap (≈0) pre- and
  post-rollover, fixing the stale-belief precondition the prior review flagged.
- **Sentinel-collapse class after the fold moved out of `evaluate`.** With
  `evaluate` no longer refreshing, the only writers of
  `last_needed_charge_hours` are the step-start refresh and
  `set_needed_charge_hours`. `grep` confirms the sole production caller of
  `evaluate` is `evaluate_charging` (mod.rs:877), which always runs after the
  step-start refresh — no path reads the estimate without a prior refresh.
  Every strategy still carries a real estimate (`SocTarget`/`SocGate`/
  `DepartureDeadline`), so the INFINITY→0.0 sentinel cannot reappear.
- **Regression sweep.** `cargo test --workspace` → every test binary reports
  **0 failed** (including `hares-core --lib` at 933 passed, the
  `actor_telemetry_regressions` / `checkpoint_regressions` / `regression`
  integration targets, and the `hares-python` / `hares-fleet` crates).
  `cargo clippy --workspace --all-targets -- -D warnings` → clean.
  `cargo fmt --all -- --check` → clean. The change is confined to
  `crates/hares-core/src/actors/ev_driver/`; the direct public-API caller
  (`tests/regression/checkpoint_restart.rs` constructs `EvDriverActor::new` and
  `add_actor`) and the telemetry regression tests all still pass.

## Acknowledged limitations (conceded — unchanged from prior round)

- **One-step `charge_kw` lag under goal control.** Actors decide against
  `latest_env`, the previous step's committed equipment output; the
  end-of-step snapshot (`dwelling/mod.rs:5780-5835`) feeds the next step. The
  current step's power does not exist when the driver populates telemetry.
- **Unresolved `equipment_id` degrades `soc` to the estimate and `charge_kw`
  to dispatched-only.** `Telemetry` is `HashMap<String, f64>` with no absent
  channel; warned at startup in `resolve_equipment_id`. `telemetry_estimate_context`
  shares the same fallback as the `soc` channel, so the degradation is uniform.
- **`steps_left == 0` early return skips `populate_telemetry`.** Pre-existing,
  unreachable in the normal flow (the Driving phase transitions to `Away` in
  the same step the trip completes).

## Observations (non-blocking, pre-existing)

- `charge_kw` still reports the *dispatched* rate ahead of the observed flow
  (mod.rs ~470), so a `PreDeparture`/`TouAware` urgency `PowerSetpoint(max)`
  reads as a flat max while the BMS tapers near full. Pre-existing
  ("rate-wins" was the pre-fix behavior) and defensible as "the driver's own
  command"; not introduced or worsened by this change.
- The RCA (`rca.md`, mechanism 2) and the ticket's Implementation Notes still
  list `SolarSurplus` among the strategies whose `needed_charge_hours`
  collapsed to 0.0. That was already false pre-fix (its stack contained
  `DepartureDeadline`). Harmless to the code; worth a one-line correction in
  the initiative doc so the chain stays accurate.

## Verified

- `CARGO_TARGET_DIR=/tmp/hares-review-target cargo test --workspace` → green:
  grepping the run for `test result:` / `FAILED` / `panicked at` shows **0 failed**
  across every target (hares-core `--lib` 933 passed; integration targets
  `actor_telemetry_regressions`, `checkpoint_regressions`, `regression`, `parity`,
  `bestest`; hares-python; hares-fleet; all doc-tests).
- `CARGO_TARGET_DIR=/tmp/hares-review-target cargo clippy --workspace --all-targets -- -D warnings`
  → exit 0.
- `cargo fmt --all -- --check` → clean.
- Reproduction test `telemetry_reflects_charging_while_equipment_soc_rises` is in
  the passing set (part of the hares-core 933). I did **not** re-run it against
  the pre-fix tree: the fix is the working-tree state and reverting the ev_driver
  files to HEAD is a forbidden working-tree restore in this repo. The
  before-fix failure (all twelve strategy×channel gaps) is documented with
  captured output in `reproduction.md`, not re-derived here.
- `crates/hares-core/src/actors/ev_driver/mod.rs:836-848` — `telemetry_estimate_context`
  substitutes `actual_soc(env)` into the fold context; `:940` — `decide`
  refreshes the fold through it; `:816` — `decision_context` still uses
  `perceived_soc()` for dispatch.
- `crates/hares-core/src/actors/ev_driver/composer.rs:83-90` — `evaluate` is
  dispatch-only and no longer touches `last_needed_charge_hours`.
- `grep -rn "composer.evaluate" crates/hares-core/src/actors/ev_driver/` →
  sole production call site is `mod.rs:877` (`evaluate_charging`); all others
  are tests.
- `grep -rn 'telemetry.set("soc"' crates/hares-core/src/actors/` → only
  `bms.rs:1040` besides the driver; BMS already reads `read_soc(env)` ground
  truth, so the class does not recur elsewhere.
