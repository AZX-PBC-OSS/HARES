---
reproduction_outcome: confirmed
reason: >-
  All four regression tests were run on the pre-fix tree (HEAD 2ee83a86,
  no production code changed) and fail with the expected messages,
  matching the RCA's measured signatures exactly: the L2 session probe
  peaks the pack at 280.9 C (RCA measured 280.9), the heater probe bills
  6.00 kWh while the pack sits at -5.00 C unchanged (RCA measured the
  same), the parked-pack probe watches a 20 C pack fall to -6.95 C in 8 h
  at -7 C ambient (the ~1.4 h flash-freeze time constant), and the
  ticket-level 45-day cold-snap scenario charges on 0 of 45 days (the
  sustained form of the ticket's zero-power nights).
failing_tests:
  - command: cargo test -p hares-core --test ev_thermal_regressions l2_charge_session_keeps_pack_temperature_physical
    path: crates/hares-core/tests/ev_thermal_regressions.rs
    name: l2_charge_session_keeps_pack_temperature_physical
  - command: cargo test -p hares-core --test ev_thermal_regressions battery_heater_warms_pack_while_cold_derate_blocks_charging
    path: crates/hares-core/tests/ev_thermal_regressions.rs
    name: battery_heater_warms_pack_while_cold_derate_blocks_charging
  - command: cargo test -p hares-core --test ev_thermal_regressions parked_pack_stays_above_charge_cutoff_after_a_cold_night
    path: crates/hares-core/tests/ev_thermal_regressions.rs
    name: parked_pack_stays_above_charge_cutoff_after_a_cold_night
  - command: cargo test -p hares-core --test ev_thermal_regressions nightly_strategy_keeps_ev_charged_through_a_sustained_cold_snap
    path: crates/hares-core/tests/ev_thermal_regressions.rs
    name: nightly_strategy_keeps_ev_charged_through_a_sustained_cold_snap
---

# Reproduction: EV pack thermal model — cold-night charging blackout,
# runaway pack temperatures, dead preconditioning (gh #32)

## Root cause being reproduced (from `rca.md`)

Not an identity/binding residual in the raw-injection path (refuted by
execution in RCA §1). The EV equipment's pack thermal sub-model diverged
from the stationary Battery equipment's correct architecture in three
coupled ways, and cold weather triggers all three at once:

- **D1** — pack thermal mass defaults to 20 kJ/K for a 75 kWh pack
  (~24× below the grounded ≈480 kJ/K; ≈6.4–6.7 kJ/K per kWh is the
  density the Battery model already carries), so with UA = 4 W/K the pack
  relaxes to ambient with τ ≈ 1.4 h. Every sub-freezing night freezes the
  pack to ambient, and the (physically correct) 0 °C cold-charge cutoff
  blacks out the whole charge window — the ticket's "exactly 0.00 kW all
  night with `needed_charge_hours` climbing".
- **D2** — the entire AC→DC charger conversion loss ((1−η)·P ≈ 1.15 kW at
  11.5 kW L2) is injected into the pack as heat instead of I²R cell
  heating (~O(100 W)), producing 165–281 °C pack temperatures on routine
  sessions in every climate and driving the Arrhenius degradation model
  out of its validity domain (negative capacity fade, capacity whipsaw).
- **D3** — heater heat is applied only inside the charge-power-positive
  branch of the thermal update, so below `min_charge_temp_c` a configured
  heater bills energy without warming the pack: preconditioning is
  impossible by any configuration, exactly when it is needed.

## The reproduction tests

New file: `crates/hares-core/tests/ev_thermal_regressions.rs` — four
behavior-named tests, one per defect plus the integrated ticket contract,
formalizing the RCA §9 diagnostic assertion tests under permanent
behavior-based names (the constitution's naming rule; the I-06 reproduce
step set the same precedent with `equipment_id_regressions.rs`). The
equipment-level probes exercise the public `Ev` equipment directly
(config → init → signal → step → telemetry); the scenario test builds the
ticket dwelling through the real assembly path
(`SimulationEngine::run` on ResStock bldg0000007 with the EV injected as
raw HPXML carrying the ticket's fresh `ReproEV1` SystemIdentifier).

### 1. `l2_charge_session_keeps_pack_temperature_physical` — pins D2

Charges 0.5 → 0.9 at 11.5 kW from a 20 °C start and asserts the pack
never exceeds 40 °C (a ~20× generous bound on the physical low
single-digit kelvin I²R rise). A guard asserts the session actually ran
(`soc ≥ 0.85`) so the temperature bound cannot hold vacuously. This is
the loss-attribution rule, not one session's path: any future
reintroduction of charger-loss-into-pack heating fails here regardless of
scenario.

### 2. `battery_heater_warms_pack_while_cold_derate_blocks_charging` — pins D3

A 1.5 kW heater with a 5 °C threshold, pack at −5 °C, `SOCTarget(0.9)`
active, plugged in. Asserts the heater draws power (billed > 0.5 kWh) and
the pack warms by more than 1 K over 4 h. The rule: a running battery
heater must warm the pack *precisely when* the cold-charge derate holds
charge power at zero — gating heater heat on charge power is the bug
shape, in any gating arrangement.

### 3. `parked_pack_stays_above_charge_cutoff_after_a_cold_night` — pins D1 directly

A new test beyond the RCA's three (rationale below): a pack at 20 °C,
parked and unplugged (pure ambient relaxation — no charging, no heater),
8 h at a constant −7 °C ambient. Asserts the pack cools measurably toward
ambient (the relaxation must exist — UA > 0, and this also rejects a
degenerate infinite-mass "fix") but stays above the 0 °C minimum charge
temperature. The rule is day-scale thermal inertia, not hour-scale; the
assertion is on the time constant's order of magnitude, not on any
specific constant, so any grounded mass/UA pair passes and the 20 kJ/K
default fails.

### 4. `nightly_strategy_keeps_ev_charged_through_a_sustained_cold_snap` — the ticket contract, D1+D2+D3 integrated

The ticket recipe through the real assembly path: fixture home, EV
injected as raw HPXML (`ReproEV1`), `Nightly(22:00–06:00, target 0.90)`
override, 45 days at 900 s, constant −7 °C weather (a flat profile, so no
diurnal daytime ambient can rescue the nights — the sustained form of the
ticket's cold snap). Asserts ≥ 30 charge-days and zero cancelled trips.
Keeping the ticket's literal entry path (raw injection with a fresh
SystemIdentifier) also guards the RCA's refuted-but-plausible identity
hypothesis: if plan-fix touches identity/assembly code, this test
watches the raw-injection end-to-end path.

## Why the fourth test was added

The RCA's three failing tests pin D2 (test 1), D3 (test 2), and the
integrated contract (test 4), but D1 is pinned only *indirectly*: the
cold-snap scenario could stay green after a future D1 regression (e.g. a
reverted thermal-mass default) as long as the heater keeps the pack warm
overnight, while the hour-scale flash-freeze physics stays wrong. The
invariant actually broken by D1 — pack thermal inertia is day-scale — is
directly observable in isolation on the parked, unplugged equipment, so
it gets its own test. Sibling coverage now maps one-to-one onto the
three primary defects, plus the integrated observable.

## Observed failures (verbatim, pre-fix tree, HEAD = 2ee83a86)

```
$ cargo test -p hares-core --test ev_thermal_regressions   # exit code 101
test l2_charge_session_keeps_pack_temperature_physical ... FAILED
test battery_heater_warms_pack_while_cold_derate_blocks_charging ... FAILED
test parked_pack_stays_above_charge_cutoff_after_a_cold_night ... FAILED
test nightly_strategy_keeps_ev_charged_through_a_sustained_cold_snap ... FAILED
test result: FAILED. 0 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out
```

Each frontmatter gate command was re-run verbatim (filtered, one test per
invocation) and every one exits 101 with its test panicking at the
assertion lines quoted below (`ev_thermal_regressions.rs:232`, `:286`,
`:331`, `:407`):

```
l2_charge_session_keeps_pack_temperature_physical:
  EV pack temperature during a Level 2 charge session must stay physical;
  peaked at 280.9 C from a 20 C start — charger conversion losses are
  being misattributed to the pack as heat and/or the pack thermal mass is
  orders of magnitude too small

battery_heater_warms_pack_while_cold_derate_blocks_charging:
  a running battery heater must warm the pack even while the cold-charge
  derate holds charge power at zero; pack went -5.00 C -> -5.00 C while
  billing 6.00 kWh of heater energy that warmed nothing

parked_pack_stays_above_charge_cutoff_after_a_cold_night:
  a warm (20 C) parked pack must stay above the 0 C minimum charge
  temperature after one -7 C night — day-scale thermal inertia, not
  hour-scale; went 20.00 C -> -6.95 C in 8 h

nightly_strategy_keeps_ev_charged_through_a_sustained_cold_snap:
  Nightly(22:00-06:00, target 0.90) in a sustained -7 C cold snap must
  still charge on most of 45 days (preconditioning while plugged in);
  got 0 charge-days
```

Each number matches the RCA's independent measurements (§9: 280.9 °C
peak; −5.00 → −5.00 °C with 6.00 kWh billed; 0 charge-days with 34
cancelled trips on the same scenario). `cargo clippy -p hares-core
--tests` and `cargo fmt --check` are clean with the new file.

## Deliberate scope decisions

- **No degradation-domain assertion** (e.g. `capacity_fade_pct ≥ 0`).
  Negative fade is a *consequence* of D2 — the Arrhenius model evaluated
  at 165–281 °C cell temperatures — and test 1's 40 °C bound keeps the
  model inside its calibrated domain, which is the root-cause-level fix.
  The RCA scopes an explicit domain guard as D5, secondary
  defense-in-depth for plan-fix; asserting non-negative fade here would
  over-constrain that step's degradation recalibration choices.
- **The diagnostic harness stays.** `crates/hares-core/tests/i07_diag.rs`
  (the RCA's evidence record, including the non-asserting probes behind
  every quoted number) is left untouched; its three assertion tests fail
  with the same signatures and keep the workspace red until the fix
  lands, exactly as the RCA notes. The fix step should fold or remove it
  once these permanent regressions are green, as the I-06 fix did with
  its diagnostic harness.
- **No production code was changed.** `git status --porcelain` shows the
  new test file as the only addition from this phase; the `M`-marked
  files elsewhere in the worktree (hares-envelope, hares-physics,
  hares-types, scripts, fixtures) are another process's in-progress work
  and were not touched.

## Post-fix addendum (fix agent, 2026-09-24)

All four gate tests now pass on the fixed tree (`cargo test -p
hares-core --test ev_thermal_regressions`: 6 passed / 0 failed). The
`l2_charge_session...` probe's start is pinned explicitly at 20 °C in the
fixed tree (the pre-fix coincidence with the removed 20 °C default no
longer holds under ambient-aware initialization). The `i07_diag.rs`
harness was folded to a self-documenting tombstone naming where each
contract went (deletion was blocked by the session's permission layer;
the disposition follows the I-06 precedent). The charge-day metric the
gates consume was corrected to the charging-isolating form (a day the
pack's stored energy actually rose) and moved to one shared helper
(`crates/hares-core/tests/common/mod.rs`) replacing the three drifting
copies. The fix's own final audit found and fixed one further same-class
sibling beyond this document's four defects: the stationary Battery's
`clamp_power` applied a charging-curve LUT's temperature axis on top of
its capacity derate — the identical double-application shape as the
EV's LUT × ramp composition — now fixed with its own regression
(`lut_temperature_axis_is_the_capability_not_multiplied_by_derate`,
`battery/mod.rs`). Full record: `initiative.md` Implementation Notes.
