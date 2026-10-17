# Axis D: small-commercial readiness; Axis E: the contributor platform (the rows' full specs)

Source: the I-09 planning round, 2026-10-17, verified against the tree at
`0911b711`. "Task N.N" refers to the I-08 hardening fan-out plan.

## Axis D assessment: how far the residential core stretches

**What stretches.** The staged DX model is genuinely shape-compatible with
light-commercial RTUs (speed staging covers single, two-speed,
multi-speed interpolated and variable-ideal, `staging.rs:29-46`; free-form
capacity lists with biquadratic curves, PLF and startup Cd). The RC
network is per-zone with node IDs from `LAYER_NODE_BASE = 1_000`
(`boundary_rc.rs:50`): 1-10 zones are far from the >= 1000 collision
(already rowed as Task 2.3). The tariff engine already bills commercial
demand (`DemandRate` with per-kW rate, TOU period, season filter and a
ratchet with lookback months and a minimum fraction, `tariff/types.rs:49-73`;
a separate demand TOU schedule; a 5-60 min demand window; per-period peaks
and ratchet history, `billing.rs:142-154`). Scheduled loads are first-class
with independent power and gas sources. Water heating scales (tank volume
free-range, 1-12 nodes).

**What breaks.** The premise model is residential at the root:
`NumberofBedrooms` is required and sizes every appliance and water-heater
draw per ANSI/RESNET 301 (`resolve_loads.rs:80-118`, `:444`); a small
office has no bedrooms (the `Building` model itself is generic:
`building.rs:264-284`). No economizer exists anywhere (the code comments
assume one, `cooler.rs:193-197`). No demand-controlled ventilation (flow
is rated flow times a schedule fraction, `ventilation.rs:703-704`; the
"bypass" is an HRV core bypass). The occupant model is
behavioral-residential; commercial premises run on schedules, but no
commercial profiles ship and there is no holiday calendar. The demand
measurement basis is net, silently (`billing.rs:323` feeds
`net_power_kw.max(0.0)`): undocumented and not configurable.

## Axis D rows (the EPIC: the single-zone small-commercial premise)

### D1: a density-based premise path beside the HPXML bedroom path (M)

**Change.** A config-first premise loader (TOML/JSON) constructing the
existing `Building` model plus density-based lighting and plug loads
(W/m2 with schedules) reusing the `ScheduledLoad` machinery, skipping the
residential appliance set; the HPXML path untouched and still requiring
bedrooms.

**Acceptance.** A 500 m2 single-zone office premise builds and runs a
year; on the density path `NumberofBedrooms` is never referenced; on the
HPXML path the required-field error still fires.

**Measurement.** The office run's end-use table equals the scheduled
loads' integrals within 0.1 percent; the run-level energy closure
(Task 5.1's report) within its bound.

### D2: the packaged single-zone RTU assembly (M)

**Change.** A registered assembly instantiating the existing DX cooler,
gas furnace and supply fan in one zone with one fan power, plus an
OA-damper object (minimum position fraction). No new physics; composition
and shared fan accounting only.

**Acceptance.** The assembly runs a year in the D1 office; heating and
cooling never run simultaneously above the deadband; fan energy metered
exactly once; cooling energy matches the equivalent separate AC within 1
percent at minimum damper.

**Measurement.** The assembly vs separate-equipment delta in kWh by end
use.

**Dependencies.** D1.

### D3: the differential-dry-bulb outdoor-air economizer (M)

**Change.** OA damper fraction state from minimum position to full;
changeover when OAT is below the zone's changeover setpoint (the 10 C
guidance already cited); the free-cooling credit reduces the DX stage
load; the damper fan-power penalty; damper-limit clamps counted in the
Task 2.11 census.

**Acceptance.** A shoulder-season fixture: the compressor stays off and
the zone holds setpoint with OAT below changeover; above changeover the
damper returns to minimum; the mixed-air energy balance closes against
the envelope's ventilation path.

**Measurement.** The economizer's free-cooling kWh per run; the
compressor-hours delta with and without.

**Dependencies.** D2.

### D4: schedule-driven DCV, occupancy-modulated OA, no CO2 feedback (S)

**Change.** An optional occupied-fraction schedule source on
`VentilationConfig` (min OA position, occupied fraction) reusing
`ScheduleSource`; the envelope's ventilation application receives the
modulated flow unchanged.

**Acceptance.** An office schedule modulates OA between min and occupied
rates, visible in telemetry and the envelope load; with no schedule
configured the behavior is bit-identical to today (goldens clean).

**Dependencies.** None technically; the acceptance scenario needs D1.

### D5: the small-commercial schedule library and a holiday list (M)

**Change.** Shipped example profiles (small office, retail strip, small
school) as CSV custom-domain fixtures; an optional holiday list forcing
the weekend profile on listed dates, validated at load (unknown dates are
errors).

**Acceptance.** A July-4 fixture run reads the weekend profile; the three
example premises run a year.

**Dependencies.** None; pairs with D1.

### D6: the demand-charge basis pinned, documented, demonstrated (S/M)

**Change.** A documented demand basis (net vs total-import), defaulting to
total-import on tariffs declaring commercial demand rates and net for the
residential back-compat path; a shipped small-commercial example tariff
with TOU demand and a ratchet.

**Acceptance.** A two-day fixture with midday PV export produces different
peaks under the two bases and the difference reconciles to the exported
kW; a 13-month run honors the ratchet's lookback and minimum fraction.

**Measurement.** A bill reconciliation table (energy + demand + fixed)
against a hand-computed reference.

**Dependencies.** None (tariff-only).

### D7: the small-commercial acceptance scenario and its validation posture (M)

**Change.** Three committed example premises (single-zone office, retail
strip, small school at 1-3 zones) run in CI's fast profile; a `docs/` note
stating the validation posture honestly: energy closure, conservation
invariants and schedule plausibility per ASHRAE 90.1 light-commercial
schedule sources; explicitly no measured-building claim.

**Acceptance.** All three run in CI; the closure residual within the Task
5.1 bound; the demand bill reconciles per D6; each example's config
resolves with zero silent fallbacks.

**Measurement.** The closure residual distribution across the three
premises in the measurement log.

**Dependencies.** D1-D6.

## Axis D: deliberately not doing (I-09)

No multi-zone VAV/AHU systems, reheat loops, central plants, cooling
towers or air-loop/plant-loop networks (EnergyPlus-scale scope; the
charter's non-goals say so). No CO2-feedback DCV (no CO2 state variable;
the schedule-driven row is the offer). No enthalpy economizer
(differential dry bulb only). No commercial occupant-behavior model (the
Occupant actor stays residential; commercial premises run on schedules).
No raising the 999-zone ceiling for its own sake. No commercial compliance
reporting (90.1 PRM and the like).

## Axis E rows: the contributor platform

### E1: adopt the property harness, five highest-value properties (M; ranked first)

**Problem.** Charter criterion 3 demands parsers fuzzed, but `proptest` is
a declared dependency used nowhere (workspace `Cargo.toml:82`,
`hares-equipment/Cargo.toml:29`, zero hits); there is no fuzz directory.

**Decision.** ADOPT, not drop: the properties below pin contracts the
fan-out plan is actively fixing; dropping the dep leaves charter criterion
3 unmet with no path.

**Shape.** Five properties in order: (1) the tariff invoice identity
(energy charge equals the summed per-step energy times the resolved rate;
the demand charge equals the max window average times the rate; monotone
in demand); (2) island conservation: `dE_pack <= E_sources` every step
over random battery/EV/PV islands (generalizes Task 1.4's property); (3)
the schedule resample: a `ColumnRef` value at any step equals the source
row under the declared boundary policy; (4) the checkpoint roundtrip:
`restore(save(state))` bit-identical for random states; (5) the HPXML/EPW/
ResStock parsers return errors, never panic, on generated arbitrary bytes.

**Acceptance.** All five run with 256 cases each in the default nextest
profile; the parser property additionally runs a 10k-case configuration in
the slow partition (E7).

**Dependencies.** Property (4) wants Task 1.5 landed first.

### E2: the whole-step allocation assertion (S)

**Change.** One integration test: a fully-loaded dwelling (HVAC both
sides, battery, EV, PV, HRV, scheduled loads) steps N steps between two
allocation-counter reads; the delta must be zero with the observe/profiling
features off.

**Acceptance.** Fails on a deliberately introduced per-step allocation
(the mutation check); passes on main; runs in the default PR profile.

### E3: the checkpoint bench and the complete CI bench gate set (S)

**Problem.** Benches exist (single_building, metrics_request, fleet,
rl_step, rl_episode, output_row_plan, diagnostics_report, rc_solver,
thermal_step) but the CI gate runs only four (`ci.yml:379, 385`);
`thermal_step` and `rl_episode` are ungated; no checkpoint bench exists
despite checkpoints being a pinned contract.

**Change.** A checkpoint bench (save and restore at two roster sizes);
add `thermal_step`, `rl_episode` and the checkpoint bench to the CI bench
job's list.

**Acceptance.** The CI bench job runs the full set green; the checkpoint
bench reports save/restore times at both sizes.

### E4: the phase-share re-record over the step pipeline (M)

**Change.** A per-phase timing report (profiling-feature timers or a
criterion harness over the StepPipeline once Task 4.2 lands), recorded in
the measurement log and re-run after each Wave 4 structural change.

**Acceptance.** The report names every phase with its percent of the step;
three consecutive runs agree within noise.

**Dependencies.** Task 4.2.

### E5: CHANGELOG and the version contract (S)

**Change.** CHANGELOG.md seeded from python.md's release notes; a short
VERSIONING contract (the semver scope for the .pyi, gym and checkpoint
surfaces; who bumps what); a CI check that workspace and pyproject
versions agree.

**Acceptance.** The version-sync check fails a deliberately mismatched
fixture and passes on main; python.md links out instead of embedding.

### E6: the submodule and reproducibility posture (S)

**Problem.** `vendors/EnergyPlus` is fetched over SSH (`git@github.com:...`
in `.gitmodules`): a fresh clone without the org's SSH keys fails
`--recursive`; the OCHRE submodule carries a branch hint; the
cross-platform golden gate triggers only on PRs to `initial-implementation`
and manual dispatch.

**Change.** The EnergyPlus URL to https; both submodules pinned to recorded
commits with a documented update procedure; the reproducibility workflow
widened to main PRs (or a weekly schedule plus release tags at minimum).

**Acceptance.** A fresh container clone with no SSH keys runs
`git submodule update --init` recursively and the test job passes; a PR to
main runs the golden-agreement job.

### E7: the nextest slow partition (M)

**Problem.** `.config/nextest.toml` defines only the thread count and a ci
profile; CI's single test job runs seven sequential full-workspace nextest
invocations with no partition of the year-long simulation suites: the
slowest tests gate every PR in every combination.

**Change.** A slow partition excluded from the default PR run; a scheduled
nightly job running the slow partition across the feature matrix; each
retained PR-matrix leg justified in the row.

**Acceptance.** The PR CI wall time drops by a recorded amount with the PR
suite green; the slow partition runs to green on the nightly.

### E8: the docs-truth sweep and its guard, one home for sample configs (M)

**Change.** Generate the per-equipment config tables into `docs/equipment/`
from `sample_config.rs`/`DescribeFields` (a check-time script; CI fails on
drift); extend the docs-check script family with the contradiction classes
Task 2.12 names.

**Acceptance.** Injecting a false field name into `docs/equipment/hvac.md`
fails CI; the five named contradictions are fixed (Task 2.12) and guarded
(this row).

**Dependencies.** Coordinates with Task 2.12 (it fixes; this row guards).

### E9: the contributor chapters (L)

**Change.** Three chapters written against the post-Wave-4 structure: the
Dwelling lifecycle (blueprint, init, warm-up, step, checkpoint, report),
the equipment roster and registry contract, the output pipeline (writer,
catalogue, reduce, metrics); every cited symbol checked by a
symbol-exists script in the docs-check family.

**Acceptance.** The three chapters exist, the symbol check is green, and a
walkthrough (register a trivial equipment through the registry) is
validated by a fresh contributor or a recorded dry run.

**Dependencies.** Tasks 4.1 and 4.2 (writing them first guarantees
immediate staleness); last.

## Axis E: deliberately not doing (I-09)

No cargo-fuzz/LLVM fuzzer infrastructure (proptest covers the property and
parser-robustness surface with zero CI infrastructure); no docs website;
no benchmark snapshot storage beyond the existing baseline cache; no
`hares-control` merge, telemetry-keys redesign or SoA equipment rewrite
(the arch review excluded them deliberately); no hares-io split or
recording-layer extraction here (Tasks 4.1 and 4.2 own them).
