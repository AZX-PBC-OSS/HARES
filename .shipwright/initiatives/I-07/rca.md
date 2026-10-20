---
diagnosis_outcome: root_cause_found
complexity: moderate
failing_tests:
  - command: cargo test -p hares-core --test i07_diag i07_ev_pack_thermal_probe
    path: crates/hares-core/tests/i07_diag.rs
    name: i07_ev_pack_thermal_probe
  - command: cargo test -p hares-core --test i07_diag i07_ev_heater_preconditions_pack_below_min_charge_temp
    path: crates/hares-core/tests/i07_diag.rs
    name: i07_ev_heater_preconditions_pack_below_min_charge_temp
  - command: cargo test -p hares-core --test i07_diag i07_cold_constant_scenario
    path: crates/hares-core/tests/i07_diag.rs
    name: i07_cold_constant_scenario
passing_commands:
  - command: cargo nextest run --workspace
  - command: cargo clippy --workspace -- -D warnings
  - command: cargo fmt --check --all
---

# RCA: gh #32 — Nightly EV never charges in its window on the raw-injection
# repro (cold-weather EV pack thermal defects, not an identity residual)

## Symptom (restated)

On the I-06-fixed build the reporter re-ran the #26 recipe against
`tests/fixtures/resstock/2025.1/bldg0000007` with the EV added as raw HPXML
(`<SystemIdentifier id='ReproEV1'/>`, 75 kWh / 11.5 kW / Level 2, Nightly
22:00–06:00 target 0.90, 45 days @ 900 s): five consecutive nights show the
vehicle plugged in, `soc_gate_charging_allowed = 1`, `needed_charge_hours`
rising 0.75 → 9.0, SOC falling 0.83 → 0.07, and EV power exactly 0.00 kW all
night; charging then happens only via the range-anxiety backstop on days 6–7
(38, then 49 kWh); the tail is 2 charge-days in 45, `drive_cancelled = 30`,
end SOC 0.90. The same 45-day recipe on their Tarrant County calibration
premise charges 34 times. The reporter hypothesized either an
identity/binding residual in the raw-injection path ("driver unbound to
weather/occupancy/miles sampling") or a fixture-specific interaction.

## TL;DR root cause

**It is weather, not wiring — specifically the EV pack thermal sub-model's
cold path.** The reporter's run saw sub-freezing nights (proven from their
own telemetry, §3). In that regime:

1. The EV's cold-charge derate zeroes charge power at/below a 0 °C pack
   temperature (`min_charge_temp_c` default) — itself grounded physics
   (Li-plating cutoff);
2. but the pack's thermal mass default is **~24× too small** (20 kJ/K for a
   75 kWh pack whose real mass is ≈480 kJ/K), so the pack is thermally
   slaved to nighttime ambient within ~2 h (τ = 20 kJ/K ÷ 4 W/K ≈ 1.4 h)
   and every sub-freezing night is a full-window charging blackout;
3. and there is no working preconditioning: the heater defaults to 0 W, and
   even when configured its heat is discarded unless charging is active —
   dead exactly when needed (it still bills energy);
4. and a second, climate-independent defect compounds everything: the
   entire AC→DC conversion loss ((1−η)·P ≈ 1.15 kW at 11.5 kW L2) is dumped
   into the pack as heat, so routine L2 sessions drive the pack to
   165–281 °C (measured) and throw the Arrhenius degradation model out of
   its validity domain (measured capacity fade going *negative*; usable
   capacity whipsawing 75 → 208 → 75 kWh day over day) — corrupting the
   capacity telemetry that feeds the driver's trip-cancellation arithmetic,
   the likely mechanism of the ticket's post-rescue tail.

The stationary Battery equipment models every one of these aspects
correctly already (I²R-based cell heating, temperature-gated heater
independent of charge demand, 90 kJ/K per 13.5 kWh with citations and a
plausibility cap). The EV thermal model is a divergent, broken
reimplementation of the same architecture. The fix is to align the EV with
that architecture — one subsystem (`hares-equipment/src/ev/`), plus a
secondary driver-awareness question for plan-fix.

## What was executed (all on this tree, HEAD = 2ee83a86)

Diagnostic harness: `crates/hares-core/tests/i07_diag.rs` (new file; no
production code touched). Ten scenarios/probes; the three assertion tests
fail pre-fix and encode the post-fix contract (reproduce step should
formalize/rename per behavior-naming rules, per I-06 precedent):

| Test | Result | Meaning |
|---|---|---|
| `i07_ticket_scenario_repro_ev1` (literal recipe: fixture + raw HPXML + `ReproEV1` + Nightly override, fixture's own Hawaii weather) | passes (probe) | **39/45 driving-day charge-days, pack topped to 0.90 nightly, 0 cancellations — the ticket symptom does NOT reproduce with the fixture's own weather** |
| `i07_ticket_scenario_override_keyed_by_system_id` (override keyed `ReproEV1` instead of `EV`) | passes (probe) | override silently misses (matched by `spec.name` = "EV"), strategy falls back to `Immediate{1.0}` → charge-to-full every evening ~18:00 — the *opposite* signature; ruled out |
| `i07_ticket_scenario_texas_weather` (fixture home + repo's Texas weather `G4803030_2018.csv`) | passes (probe) | **reproduces the ticket's shape**: days 0–1 zero charging with `needed@22` rising 1.25→3.50 and SOC 0.81→0.60; a later cold stretch drives SOC to 0.04 (anxiety active), then 21 + 43.7 kWh backstop rescue sessions — the ticket's days 6–7 pattern |
| `i07_cold_constant_scenario` (constant −7 °C) | **FAILS** (0 charge-days, 34 cancelled trips) | the ticket signature, sustained: plugged in 100% of night steps, `needed@22` 1.25→10.00, SOC 1.00→0.01 in 4 days, first cancellation day 5, 34 by day 44 |
| `i07_cold_diurnal_scenario` / `i07_cold_diurnal_with_heater` | pass (probes) | marginal nights (22:00 ambient +4.8 °C) charge via the self-heating quirk; the configured heater bills 2.6 kWh/day in the evening hold window without warming the pack (see defect 3) |
| `i07_ev_pack_thermal_probe` (equipment-level: 0.5→0.9 at 11.5 kW, 20 °C ambient) | **FAILS** | pack temperature: 20 → 71.8 → … → **280.9 °C** in 12×900 s; `capacity_fade_pct` = **−15.8 %** after the day boundary |
| `i07_ev_degradation_45day_probe` (nightly 0.77→0.90 top-ups, 20 °C) | passes (probe) | every ordinary nightly top-up peaks the pack at **165.5 °C**; fade settles near **−13 %** (capacity grows 75→85 kWh) |
| `i07_ev_cold_rescue_degradation_probe` (ticket's rescue pattern: deep charges from 0.07 at −7 °C ambient) | passes (probe) | one rescue session: pack 279 °C, fade **−177 %** → capacity **75→207.8 kWh**, snapping back to 75.3 the next day — the capacity divisor whipsaws |
| `i07_ev_heater_preconditions_pack_below_min_charge_temp` (heater 1.5 kW, pack at −5 °C, SOCTarget active) | **FAILS** | heater bills **6.00 kWh over 4 h; pack warms 0.00 K** (−5.00 → −5.00 °C) |

`cargo clippy -p hares-core --tests` and `cargo fmt -p hares-core -- --check`
are clean with this file. The I-06 long-horizon regression
(`nightly_strategy_keeps_ev_charged_over_long_horizon`) still passes.

## 1. The identity/injection hypothesis is refuted by execution

The reporter's primary hypothesis — raw HPXML injection with a fresh
SystemIdentifier leaves the driver unbound — does not hold on this tree:

- `resolve_ev` (`hares-io/src/hpxml/resolve_der.rs:175-246`) names every
  HPXML EV spec `"EV"` regardless of SystemIdentifier (the id lands only in
  `spec.system_id`, which nothing on the EV path consumes); instance names
  are assigned only on collision (`assign_instance_names`). The equipment
  is named "EV" — matching the ticket's own `EV Electric Power (kW)`
  column.
- `apply_equipment_overrides` (`dwelling/conversions.rs:843`) matches
  overrides by `spec.name`, so `Equipment["EV"]` applies regardless of the
  SystemIdentifier. (Keying the override `"ReproEV1"` silently misses — a
  loud-error gap worth noting, but it produces Immediate-style evening
  charge-to-full, the opposite of the ticket; ruled out by execution.)
- The driver is seeded, targeted, resolved, and observed correctly:
  `Ev::actor_seed` → `build_actors_from_seeds` → `EvDriverActor` targeting
  "EV", `resolve_equipment_id` against the now-unique ids (I-06 fix),
  dispatches and observation both live — demonstrated end-to-end by the
  healthy Hawaii run and by the Texas run's mixed nights charging whenever
  the pack is above 0 °C.

`git status`: the only change is the new diagnostic test file.

## 2. What the ticket's own telemetry proves about the run's weather

`needed_charge_hours` is `(target − soc) · capacity / (max_kw · η) ·
temp_efficiency_multiplier(T_ambient)` (`preference.rs`,
`needed_charge_hours_to_target`), computed from the observed SOC. The
ticket's five nightly readings are internally consistent **only** at
ambient ≈ −6.5 °C:

- night 1: (0.90−0.83)·75/(11.5·0.9)·tm = 0.75 ⟹ tm = 1.48 ⟹ T ≈ −6.5 °C
- night 5: (0.90−0.07)·7.246·1.48 = 8.9 ≈ 9.0 ✓
- implied intermediate SOCs (0.64, 0.43, 0.27) fall ~0.19–0.21/day, which
  equals the cold-inflated daily drive: 30 mi · 0.325 kWh/mi · 1.48 / 75
  kWh = 0.193 SOC/day ✓ (the driver seed's hardcoded 30 mi/day,
  `dwelling/mod.rs:7626`: `ScheduleSource::Constant(30.0)`)

The fixture's own weather (`G1500030_2018.csv`, Hawaii — lat 20.9°N) has a
January minimum of 12.2 °C and can never produce tm = 1.48. The repo's
Texas fixture weather (`G4803030_2018.csv` — the FIPS-48 / Tarrant-County
class file their calibration pipeline would carry) opens January 2018 with
exactly this: nights of −15…−2 °C on days 0–7 with afternoons at 10–16 °C,
and 24 of the first 45 nights below freezing on average. The fixture swap
changed the run's effective climate; the injection path is inert (§1). The
reporter's intuition ("unbound to weather") was directionally right about
*weather* but wrong about the mechanism — it is the equipment's thermal
coupling to weather, not a driver binding.

(One caveat I cannot check: their build carries "three additive commits on
our branch" which are not visible here. The symptom is fully reproduced on
this tree from weather alone, so no engine delta is needed to explain it.)

## 3. The cold-night blackout mechanism, traced

- `Ev::step` → `run_charging_physics` → `charge_derate_factor()` =
  `linear_temp_derate(battery_temp_c, min_charge_temp_c = 0.0,
  full_power_temp_c = 10.0)` (`ev/mod.rs:1082-1088`,
  `ev/config.rs:58-59`), → 0 at/below 0 °C pack temperature.
  `compute_charging_power_kw` then yields exactly 0 kW regardless of the
  dispatched `SOCTarget` (`ev/mod.rs:676`: `derated_rated = curve · derate`
  = 0; the line is `let derated_rated = curve_limited_rated * charge_derate;`).
  The composer's in-window dispatch is made and honored — there is
  simply no power to honor it with. This is the ticket's "exactly 0.00 kW".
- `battery_temp_c` relaxes to `env.weather.outdoor_temp_c` with
  τ = `thermal_mass_j_per_k` / `ua_w_per_k` = 20 000 / 4.0 ≈ 1.4 h
  (`apply_soc_and_thermal`, `ev/mod.rs:1280`; defaults
  `ev/config.rs:64-65`). By 22:00 the pack sits at the cold-night ambient
  and stays there all night. Every sub-freezing night is a full-window
  blackout.
- The pack defaults to **no heater** (`DEFAULT_HEATER_POWER_W = 0.0`,
  `ev/config.rs:62`), so nothing reconditions it.
- The driver keeps commanding `SOCTarget(0.9)` each in-window step (its
  needed-hours estimate uses `temp_efficiency_multiplier`, capped at 2.0 —
  it models cold charging as *slower*, never as *impossible*), so
  `needed_charge_hours` climbs nightly while nothing happens — the
  ticket's exact telemetry.
- The cold-inflated drive consumption drains the pack 0.19 SOC/day until
  it can't cover the 14.4 kWh trip; the morning cancellation consumes
  `todays_event` (`ev_driver/mod.rs:1204-1220`), which removes the
  range-anxiety override's urgency gate
  (`range_anxiety_override_request`: with no known trip the band condition
  alone fires), latching `SOCTarget(band)` all day; when the afternoon
  ambient lifts the pack above 0 °C the equipment finally charges — the
  ticket's days 6–7 "backstop" (38/49 kWh). Reproduced on this tree with
  Texas weather: SOC collapse to 0.04, then 21 + 43.7 kWh rescues.

## 4. The compounding, climate-independent defect: charger losses heat the
## pack, and the degradation model leaves its domain

`apply_soc_and_thermal` (`ev/mod.rs:1295-1301`):

```rust
let ohmic_like_heat_w = power_kw_to_w(net_charge_kw - dc_stored_kw);  // = (1−η)·P  [ev/mod.rs:1296]
self.battery_temp_c += ((ohmic_like_heat_w + heater_w) * dt_s) / self.thermal_mass_j_per_k;
```

(the heat-application line is inside `} else if charger_kw > 0.0 {` at
`ev/mod.rs:1289` — see §5)

`(1−η)·P` — 1.15 kW at 11.5 kW L2 — is the **onboard charger's conversion
loss**, which physically heats the charger/power electronics, not the
cells. Pack self-heating at L2 is I²R: ~32 A into a ~355 V pack with
O(0.1 Ω) pack resistance ≈ 100 W. Into the tiny 20 kJ/K mass the
misattributed 1.15 kW produces **+42…+52 K per 900 s step** — measured:
20 → 280.9 °C over a single 0.5→0.9 session; 165.5 °C peak on every
ordinary nightly 0.77→0.90 top-up, in every climate, on this tree today.

The degradation model (`battery/degradation.rs`: Arrhenius + Tafel,
calibrated for cell temperatures in the physical range) is then evaluated
at 440–550 K: measured `capacity_fade_pct` = −15.8 % after one session,
−13 % after 45 nightly sessions, −177 % after one cold rescue session —
negative fade means the pack *grows* capacity (75 → 85 → 207.8 kWh), then
snaps back (75.3) at the next day boundary. Since `capacity_kwh` is the
SOC divisor and is exported as the `CAPACITY_KWH` telemetry the driver's
`observed_pack_kwh` multiplies by observed SOC
(`ev_driver/mod.rs:709-746`), this whipsaw directly corrupts trip
cancellation — the credible mechanism for the ticket's tail (trips
cancelled against a nominally full pack, "end SOC 0.90" relative to a
moving capacity). The exact tail is build- and weather-dependent; the
out-of-domain degradation is measured fact on this tree.

## 5. The preconditioning escape hatch is dead code when it matters

`run_charging_physics` engages the heater when the pack would accept charge
and is below the threshold (`ev/mod.rs:1258-1261`: `self.heater_active =
self.heater_power_w > 0.0 && would_charge_underated && self.battery_temp_c
<= self.heater_threshold_c`), and `step` bills it (`active_power_kw =
charger_kw + heater_kw`). But the heat is applied to the pack only inside
the `else if charger_kw > 0.0` branch of `apply_soc_and_thermal`
(`ev/mod.rs:1289`). Below `min_charge_temp_c` the
derate forces `charger_kw = 0`, so **a configured heater draws and bills
power without warming the pack at all** — measured: 6.00 kWh billed, pack
−5.00 → −5.00 °C over 4 h. Preconditioning below 0 °C is therefore
impossible by any configuration. (The stationary Battery does this right:
"heater activates based on cell temperature alone — regardless of
charge/discharge demand", `battery/mod.rs:1335-1352`, citing Tesla PW3
Heat Mode / FranklinWH practice. The EV also lacks the Battery's 700 W
heater plausibility cap.)

## 6. Physics grounding for the fix (researched provenance)

- **Pack thermal mass.** 75 kWh-class EV pack ≈ 479 kg (Tesla Model 3 LR
  pack spec listing, evshop.eu; consistent with the 250–600 kg industry
  range). Li-ion cell specific heat 800–1100 J/(kg·K) (literature range —
  batterydesign.net summary of calorimetry literature; NASA TFAWS-18
  characterization talk). ⟹ pack thermal mass ≈ **380–530 kJ/K**, i.e.
  ≈6.4 kJ/K per kWh. In-tree cross-check: the Battery equipment defaults
  to 90 kJ/K for a 13.5 kWh pack = 6.7 kJ/K/kWh (`battery/mod.rs:149`) —
  the same density. The EV default (20 kJ/K) is ~24× low and carries no
  citation (constitution violation independent of the bug). With a correct
  mass and UA 4 W/K, τ ≈ 33 h — day-scale carryover, so a pack does not
  flash-freeze at sundown; sub-freezing *nights* in a climate with
  above-freezing afternoons stop being automatic blackouts.
- **The 0 °C charge cutoff is real physics — keep it.** Charging Li-ion
  below 0 °C causes lithium plating; BMS low-temperature charge cutoffs
  are standard practice (multiple industry references). What real vehicles
  pair it with — and HARES lacks — is **preconditioning**: plugged-in pack
  heating before/during scheduled charging (Tesla scheduled-departure
  preconditioning; standard BMS behavior; see also the Battery model's
  cited Tesla PW3 / FranklinWH heater precedent).
- **Loss attribution.** Charger/inverter conversion losses dissipate in
  the charger; pack heating during charge is I²R from cell/pack
  resistance — exactly what the Battery model computes
  (`compute_electrical`: `current² × pack_resistance`,
  `battery/mod.rs:590`). The EV should use the same formulation (its OCV /
  pack-resistance tables already exist for degradation).
- **OCHRE floor.** OCHRE's EV model has no pack thermal model and no
  cold-charge block (`vendors/OCHRE/ochre/Equipment/EV.py` uses daily-mean
  ambient only for drive-event sampling). Today, in a cold snap, HARES
  simulates an EV that cannot charge for weeks — below the OCHRE floor for
  cold-climate residential EV loads. The fidelity target is not "remove
  the derate" (that would lose real physics OCHRE lacks) but "add the
  compensating real behavior": correct thermal inertia, correct loss
  attribution, and working plugged-in preconditioning.

## 6b. Strongest alternative explanation, and why it loses

The strongest remaining alternative is not in this repo: **the reporter's
build carries "three additive commits on our branch" that are not visible
here, and one of them could have changed engine behavior.** I cannot
inspect those commits, so this cannot be excluded by reading. It loses to
the weather/thermal explanation on evidence, not on preference:

1. The symptom reproduces on the *unmodified* I-06-fix tree (HEAD =
   2ee83a86) from cold weather input alone — no engine delta is required.
   `cargo test -p hares-core --test i07_diag i07_cold_constant_scenario`
   fails with: *"Nightly(22:00-06:00, target 0.90) in a sustained -7 C
   cold snap must still charge on most of 45 days … got 0 charge-days"*,
   and the run's day table shows the ticket's exact signature: plugged in
   100 % of night steps, zero night power, `needed@22` rising
   1.25 → 10.00, SOC 1.00 → 0.01 in four days, `range_anxiety` active from
   day 2, first cancellation day 5, 34 by day 44 (ticket: 30 by day 45).
   `cargo test -p hares-core --test i07_diag
   i07_ticket_scenario_texas_weather` (fixture home + the repo's Texas
   weather file) reproduces the rescue shape: SOC collapse to 0.04 with
   anxiety active, then 21 + 43.7 kWh backstop sessions (ticket: 38 + 49
   kWh on days 6–7).
2. The ticket's own numbers carry the cold signature independent of any
   code: the `needed_charge_hours` series pins
   `temp_efficiency_multiplier` ≈ 1.48 at every reading, and the daily SOC
   drain equals the cold-inflated drive consumption to within rounding
   (§2). A code change on their branch would have to coincidentally
   produce numbers that satisfy the cold-weather arithmetic at three
   independent checkpoints.
3. The same recipe with the fixture's own (warm) weather is healthy on
   this tree (39/45 charge-days, 0 cancellations) — so whatever their
   branch does, the *difference they observed between the two premises* is
   explained by the weather their harness supplied, not by the injection
   path.

If their branch's commits are later shown to matter, they would be a
second, independent cause layered on top of the defects proven here; the
three failing tests in §9 fail on the clean tree and need fixing
regardless.

## 7. Alternatives considered and ruled out

- **Identity/binding residual in the raw-injection path** (the reporter's
  primary hypothesis): refuted by execution — §1. The literal ticket
  recipe with the fixture's own weather is healthy (39/45 charge-days, 0
  cancellations).
- **Override keyed by SystemIdentifier silently missing**: real gap
  (silent no-op override — a loud-error violation worth its own look),
  but produces Immediate-style evening charge-to-full; opposite signature.
  Ruled out by execution.
- **Timezone/window shift**: would displace charging hours, not zero them;
  the ticket's power is exactly 0.00 all night. Ruled out.
- **Dispatch/silence contract regression (the I-05 class)**: the composer
  dispatches `SOCTarget(0.9)` every in-window step (traced), and the
  equipment honors it whenever the derate is nonzero (mixed nights in the
  Texas run charge: days 2–5, 7–13, …). Ruled out.
- **I-06 observation residual**: `reconcile_soc`/`actual_soc` work — the
  ticket's SOC/needed-hours track the physical pack (their telemetry is
  internally consistent only under working observation), and the I-06
  long-horizon regression passes on this tree. Ruled out.
- **"It's just physics, no bug"**: the derate is physics; a pack that
  flash-freezes in 2 h, cannot be preconditioned by any configuration,
  heats to 165–281 °C on routine charging, and whose degradation model
  responds by growing capacity is not. The behavior is also below the
  OCHRE floor in cold climates.

## 8. Root cause statement (for plan-fix)

Primary defects, all in `hares-equipment/src/ev/` — the EV pack thermal
sub-model diverged from the (correct) Battery architecture:

- **D1 — unphysical, uncited pack thermal mass** (20 kJ/K vs a grounded
  ≈480 kJ/K for 75 kWh; ~6.4–6.7 kJ/K/kWh, matching the Battery default
  density), slaving pack temperature to ambient with τ ≈ 1.4 h.
- **D2 — charger conversion losses misattributed to the pack as heat**
  (`(1−η)·P` instead of I²R), producing thermal-runaway-scale pack
  temperatures on every L2 session in every climate and throwing the
  degradation model out of domain (negative/oscillating fade; capacity
  whipsaw corrupting driver trip-cancellation arithmetic).
- **D3 — no working cold preconditioning**: heater defaults to 0 W, and
  heater heat is gated on `charger_kw > 0` so it can never warm a pack
  whose charging is derated to zero (while still billing energy).

Secondary (plan-fix to scope):
- **D4 — driver derate blindness**: needed-hours/urgency estimates model
  cold charging as ≤2× slower, never impossible; whether Nightly should
  opportunistically charge on warm plugged-in afternoons is a behavioral
  decision.
- **D5 — degradation domain guard**: with D1/D2 fixed the Arrhenius model
  stays in-domain; a guard against non-physical cell temperatures /
  negative fade is defense-in-depth.

Note D1–D3 are coupled: fixing D1 alone removes D2's self-heating quirk
that currently rescues marginal nights, so the cold-night behavior then
depends entirely on D3 (preconditioning) — the ticket's climate (daily
*mean* below freezing) cannot charge at night without it. All three must
land together. Constitution class note: the class is "EV thermal model vs
the Battery model's correct one" — align them; do not patch one constant.

## 9. Reproduction (verbatim) and the failing tests

Commands run, and what they printed (abridged to the decisive lines):

```
$ cargo test -p hares-core --test i07_diag i07_cold_constant_scenario -- --nocapture
day | charge_kWh (night) | soc min..max | actor soc eod | needed@22 | anxiety | cancelled | ...
  0 |    0.00 (   0.00) |  0.80.. 1.00 |  0.802 |    1.25 |  0.0 |     0 | ...
  1 |    0.00 (   0.00) |  0.60.. 0.80 |  0.603 |    3.50 |  0.0 |     0 | ...
  2 |    0.00 (   0.00) |  0.41.. 0.60 |  0.405 |    5.50 |  1.0 |     0 | ...
  3 |    0.00 (   0.00) |  0.21.. 0.41 |  0.207 |    7.75 |  1.0 |     0 | ...
  4 |    0.00 (   0.00) |  0.01.. 0.21 |  0.009 |   10.00 |  1.0 |     0 | ...
  5 |    0.00 (   0.00) |  0.01.. 0.01 |  0.009 |   10.00 |  1.0 |     1 | ...
 44 |    0.00 (   0.00) |  0.01.. 0.01 |  0.009 |   10.00 |  1.0 |    34 | ...
thread 'i07_cold_constant_scenario' panicked:
  Nightly(22:00-06:00, target 0.90) in a sustained -7 C cold snap must
  still charge on most of 45 days (preconditioning while plugged in);
  got 0 charge-days

$ cargo test -p hares-core --test i07_diag i07_ev_pack_thermal_probe -- --nocapture
step | soc | active_kW | batt_C | derate | fade_pct
   0 | 0.5345 |  11.500 |    71.75 |  1.00 | 0.000000
   5 | 0.7070 |  11.500 |   220.10 |  1.00 | 0.000000
  11 | 0.8999 |  11.500 |   280.93 |  1.00 | -15.795114
thread 'i07_ev_pack_thermal_probe' panicked:
  EV pack temperature during an L2 charge session must stay physical;
  peaked at 280.9 C from a 20 C start ...

$ cargo test -p hares-core --test i07_diag i07_ev_heater_preconditions_pack_below_min_charge_temp -- --nocapture
heater probe: start -5.00 C -> end -5.00 C, billed 6.00 kWh
thread 'i07_ev_heater_preconditions_pack_below_min_charge_temp' panicked:
  a running battery heater must warm the pack even while the cold-charge
  derate holds charge power at zero; pack went -5.00 C -> -5.00 C while
  billing 6.00 kWh of heater energy that warmed nothing
```

The three failing tests (the `failing_tests` frontmatter entries):

- `i07_ev_pack_thermal_probe` — asserts peak pack temperature ≤ 40 °C for
  an L2 session from a 20 °C start (already ~20× generous vs the
  physically expected low-single-digit-K rise).
- `i07_ev_heater_preconditions_pack_below_min_charge_temp` — asserts a
  running heater warms the pack while the derate holds charge power at
  zero.
- `i07_cold_constant_scenario` — asserts ≥30 charge-days and zero
  cancelled trips over 45 days at a sustained −7 °C: the ticket-level
  contract (a plugged-in EV in a cold snap must keep driving — real
  vehicles precondition; OCHRE has no cold block at all).

The harness also contains the non-asserting probes cited above
(Hawaii-healthy literal recipe, override-miss, Texas-weather reproduction,
diurnal ± heater, two degradation probes) as the evidence record.

Note for the coordinator: `cargo nextest run --workspace` is red until the
fix lands precisely because these three tests fail — that is the designed
must_fail/must_pass gate shape (same as I-06).

## 10. Scope of this phase (what was and was not done)

No production code was changed; no fix was written; no refactor was
started. `git status --porcelain` shows one new file from this phase:
`crates/hares-core/tests/i07_diag.rs`. (The `M`-marked files elsewhere in
the worktree — hares-envelope, hares-physics, hares-types, scripts — are
another process's in-progress work and were not touched.)

The test file is diagnostic evidence, within the role as this flow defines
it: the `rca.md` schema's `failing_tests` entries are consumed by the
reproduce step's `must_fail` gate and the fix step's `must_pass` gate, so
the diagnosis must ship executable reproduction — the I-06 diagnose phase
set the same precedent (`i06_ev_nightly_diag.rs`). The three assertion
tests encode the *post-fix contract*, not a fix: they fail today and say
nothing about how the code should change. The non-asserting probes exist
so a reviewer can rerun every quoted number. Whether any of the harness
survives permanently, and under what behavior-based names, is the
reproduce step's call.

## Post-fix addendum (fix agent, 2026-09-24)

The diagnosis held: D1–D3 were exactly as stated, and the fix followed the
stated class (alignment with the stationary Battery's architecture, not
one patched constant). One RCA §4 claim deserves a correction for the
record: the "likely mechanism of the ticket's post-rescue tail" — capacity
whipsaw corrupting the driver's trip-cancellation arithmetic through
`observed_pack_kwh` — was real but incomplete. The whipsaw's root was not
only the out-of-domain Arrhenius evaluation (D2's temperatures): the
degradation constants themselves had been inherited from OCHRE's
Matlab-derived values in divergence from the paper's own reference
implementation (NREL SSC). The fix aligned them (b3_ref/θ/b0 corrected,
α_b1 confirmed; the negative-electrode branch Eq. 8–11 and the d0 ratio
added; usable capacity = min(QLi, Qneg)), so the BOL capacity trajectory
now follows the reference (starting ≈ +0.9 % above nameplate, decaying
with the break-in loss) instead of OCHRE's opposite-sign capacity boost.
Also fixed beyond the RCA's scope, found during verification: the driver
estimate's derate blindness (D4) and the parity corpus's missing
charging-EV fixture. The fix's own final audit found one further
class-member the plan's class audit had enumerated only in the EV: the
stationary Battery's `clamp_power` multiplied a charging-curve LUT's
temperature axis on top of its capacity derate — the same
one-effect-applied-twice shape as the EV's LUT × ramp composition —
now fixed with its own regression test. Full record: initiative.md
Implementation Notes.
