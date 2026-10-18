# 30: the risk register, first pass

The operator's mandate: find the high-impact, high-risk surfaces and work
them systematically: red teaming, breaking, finding miswirings, bad physics,
brittle architecture, poor design. This register ranks the top 15 surfaces by
expected information per red-team effort, where expected information weighs
impact on correctness, probability of hidden defects and blast radius. The
defect-class evidence is the I-08 ledger (its found-defect entries, counted
in the heat map below); the surface evidence is the I-09 specs (10, 11, 12)
and the two external sweeps (20, 21). Tree state: `9c9d9751` on
`feat/output-results`, 2026-10-17.

Ranking rubric. A surface ranks high when three things hold together: the
grid-ops or money mission rides its numbers, the I-08 ledger already found
its defect class there (so the class reproduces under new construction), and
the pins that exist today prove the happy path only. New construction
(surface 14, 15) ranks last not because it is safe but because attacking it
buys information only after it exists.

## The ranked surfaces

| # | surface | home | one-line risk |
|---|---------|------|---------------|
| 1 | the sub-0.9 pu clamp and the ZIP/reactive aggregate | `hares-envelope/src/electrical_solver.rs` | the channel admits [0.5, 1.5] pu; the load model clamps at [0.9, 1.1] and answers a sag with 0.9 pu forever |
| 2 | islanding's binary state and the interchange publication | `hares-core/src/dwelling/mod.rs` | during an island the published `total_power_kw` is the internal imbalance, not zero |
| 3 | the RNG stream algebra and the scenario dimension | `hares-types/src/rng.rs`, `hares-core/src/rng.rs` | the shared-keystream class produced correlated fiction with zero failures; the scenario nonce that must prevent it does not exist yet |
| 4 | the HPXML/OS-HPXML resolve pipeline | `hares-io/src/hpxml/`, the defaults store | the I-08 ledger's dominant class concentrated here; even the ledger's own quotes drift (EV-8) |
| 5 | the checkpoint/restore contract | the snapshot family, `CHECKPOINT_VERSION` 18 | restore-loss found twice by red team; every new capability adds state that can miss the snapshot |
| 6 | the HELICS boundary and the grid-signal channel | `python/hares/helics/` | two federates, two different boundary behaviors; `set_grid_voltage` overwrites frequency with 60 Hz |
| 7 | the equipment curve and LUT machinery | `hares-equipment` (`staging.rs`, `coil_physics.rs`, `default_curves.rs`), the envelope LUT | curves evaluated at corners and beyond tables, with an undocumented extrapolation policy |
| 8 | the tariff/billing engine | `hares-tariff/src/billing.rs` | the demand basis is net, silently; the ratchet spans 13 months of state |
| 9 | the fleet fold's aggregates and the memory math | `hares-fleet`, the output fold | weighted folds over nulls and codes, just re-plumbed; the 10k-home math forces chunking |
| 10 | the weather interpolation and calendar | `hares-io` weather (B-1a/b/c) | freshly rewritten, maximum blast radius, and a phase-shift class already slipped through once |
| 11 | the Python boundary (GIL, capsule, exceptions) | `hares-python`, `python/hares/` | deadlock and pickle-loss classes both found; C3 puts reward machinery on this seam |
| 12 | the gym's bounds and spaces | `py_gym.rs:288-316`, `gym_env.py:191-262` | name-keyed fallback bounds make a 5 kW and a 20 kW heater identical to the agent |
| 13 | warm-up convergence | the B-2 loop | a forged convergence poisons every downstream number silently; strict policy is opt-in |
| 14 | the scenario/event scripting engine (A2) | new construction | ordering semantics on the outage/price surface, built once and trusted everywhere |
| 15 | the density-based premise path (D1) | new construction | a second premise loader doubles the substitution surface beside the bedroom-sized one |

---

## 1. The sub-0.9 pu clamp and the ZIP/reactive aggregate

**What it is.** `hares-envelope/src/electrical_solver.rs`: `zip_load_scale`
computes `Z*V^2 + I*V + P` after clamping `voltage_pu` to [0.9, 1.1] with a
per-step `tracing::warn`; the module default is pure P (z 0.0, i 0.0, p 1.0).
The reactive side lives per equipment (`hares-equipment/src/hvac/reactive.rs`
and the battery/EV reactive setpoints); the aggregate's Q(V) response is the
sum of whatever the equipment models do, with no aggregate contract. Every
premise in every run passes through this on every step; the grid-ops mission
(sags, brownouts, DER trips with loads behind the meter) reads nothing else.

**Risk thesis.** The clamp is a documented silent substitution: a 0.7 pu sag
behaves as 0.9 pu every step forever, motor loads never stall, and a
brownout run emits one warning line per dwelling per step (A5's problem
statement, verified at `electrical_solver.rs:13-32`, `:135-162`). The
channel admits [0.5, 1.5] pu, so the admitted input space is wider than the
honored input space by exactly the band where the physics is most
interesting. The default ZIP is pure P, so a fleet deployed with defaults
has zero voltage sensitivity while looking like a ZIP model. The
de-energized-bus branch holds the scale at 1.0 by design so leaked loads
stay visible: a deliberate masking choice whose only guard is the
electrical-balance invariant. The I-08 classes here: substitution (OD-10,
OD-57: input replaced by a policy), the uncounted-clamp class (OD-18:
clamps that no counter sees), and unit confusion (pu vs V, IN-8's class).

**Attack plan.** Brownout vectors at 0.5/0.7/0.88 pu against pure-Z, pure-I
and pure-P compositions (assert V-squared, V and constant scaling
respectively); a pure-I corner (the composition no test exercises); reactive
under sag (does Q scale with the same V weighting as P, and does any test
know); motor-family behavior enumerated family by family (stall, lockout or
non-model, each recorded in `docs/PHYSICS_DECISIONS.md` per A5); the
warn-to-event census mutation (turn the per-step warn into a counted event
and prove a sag scenario's count is finite and per-dwelling); the aggregate
against the per-equipment sum under a fixed V profile (a reconciliation
property). Row coverage: **A5** (row 30) owns the sub-0.9 contract family by
family and **A7** (row 34) the S-squared-equals-P-squared-plus-Q-squared
invariant. Gaps: neither row names the pure-I corner, the Q-under-sag
vectors, the aggregate reconciliation property, or the census mutation on
the clamp itself; A5 must precede A7 (same file) so the round should run
between them.

**Coverage.** Pins today: `tests/python/test_equipment_zip_voltage_response.py`,
`tests/python/test_reactive_power.py`,
`crates/hares-core/tests/grid_outage_islanding.rs`,
`thermal_invariant_wiring.rs`. Pin gap: no sub-0.9 pu vector exists
anywhere, no I-component test, no Q(V) aggregate pin, no clamp-count pin.

## 2. Islanding's binary state and the interchange publication

**What it is.** `hares-core/src/dwelling/mod.rs` `GridState`:
`island_bus_voltage_pu` (an Option holding 1.0 when a grid-forming source
regulates), `island_unserved_kw`, `island_excess_kw`, the one-shot
`island_unserved_warned` latch, and the publication of `total_power_kw`
during an island as the internal imbalance (`dwelling/mod.rs:5252`,
`:7683-7685`, A3's citation). The binary islanded/not branch decides
publication, invariants and warning suppression.

**Risk thesis.** A feeder federate watching `total_power_kw` sees a phantom
load exactly during the disturbances a grid study exists to examine
(A3's problem statement): the number is real, physical and wrong for the
consumer. The binary state machine has the failure mode where one
mis-ordered transition (an outage landing on the same step as a DR event, a
restoration racing the control queue) reclassifies publication, conservation
and warnings at once. The one-shot warning latch is a stateful branch whose
reset timing is untested across checkpoint restore. The I-08 classes here:
ordering/lifecycle (OR-19's roster transitions, OD-16's failure
reconciliation) and substitution (a plausible value published where a
contract was wanted).

**Attack plan.** An outage-then-island-then-restoration scenario asserted
bitwise: the meter-side interchange exactly 0.0 from island formation while
the premise-balance topic continues; island formation while PV is exporting
(the excess path); an island whose sources fail mid-island (the unserved
path); restoration on the same step as a queued control signal; the
unserved warning latch across a checkpoint save/restore. Row coverage:
**A3** (row 14) builds the grid-facing publications and the temporal
contract, **A4** (row 29) the per-DER trip observability. Gaps: neither row
names the restoration-race probes, the latch-across-checkpoint probe, or a
fleet-fold island aggregation (the fold has no island column).

**Coverage.** Pins today: `crates/hares-core/tests/grid_outage_islanding.rs`
(bus dead without backup, island keeps loads, unserved warnings counted).
Pin gap: no federation-level interchange pin, no restoration pin, no
trip-during-island interaction, no latch-across-resume pin.

## 3. The RNG stream algebra and the scenario dimension

**What it is.** `hares-types/src/rng.rs` (`RngStream`: the generator key and
stream nonce, `event_load`'s top-bit nonce partition, `dwelling_seed(master,
bldg_id)`) and `hares-core/src/rng.rs` (the re-derivation on reseed). B3
(row 20) extends this with a scenario nonce so `(master_seed, scenario_id,
bldg_id, stream)` partitions the stream space.

**Risk thesis.** The shared-keystream class is the ledger's sneakiest: OD-29
found every event load and the dwelling drawing stream 0, two probes
producing identical patterns, and no test failing, because correlated
fiction is indistinguishable from data without a disjointness proof. The
fan-out plan's Tasks 2.8 and 2.9 closed the shared-keystream and seed-0
defects for the run dimension; the scenario dimension (B2, B3, B5) is about
to multiply the stream space and nothing yet proves two scenarios
independent. Every Monte Carlo study's statistical validity rides this, and
its defects are invisible at N=1. The I-08 classes here: RNG (OD-29, OD-32)
and ordering (OR-19's EV driver stream cursor, a checkpointed stream
position).

**Attack plan.** Disjointness proofs by construction (scenario A and B
streams disjoint under the nonce algebra; hash-distance probes over
1000 scenario ids for collisions or near-collisions in the stream space);
interleaved re-runs (run scenario A, run B, re-run A: bitwise unchanged);
the gym's per-episode seed composed with the scenario seed; the warmup-day
invariance extended to every new source (B1's acceptance); a correlation
scan over two scenarios' event-load draws (a shared keystream shows up as
identical draw sequences); the cursor checkpointed mid-stream. Row
coverage: **B3** (row 20) is the algebra row and **B1** (row 22) the
diversity layer. Gaps: no row names the collision-distance census, the
correlation scan, or the cursor-mid-stream checkpoint probe; B3 is S-sized
while carrying the whole reproducibility story for axis B.

**Coverage.** Pins today: `crates/hares-core/tests/event_load_rng_streams.rs`,
`reseed_streams.rs`, the replay path's seed-derivation proof (row 69's
record). Pin gap: no scenario dimension, no disjointness property, no
cross-scenario independence pin, no correlation scan.

## 4. The HPXML/OS-HPXML resolve pipeline

**What it is.** `hares-io/src/hpxml/` (the resolvers: `resolve_loads.rs`,
`resolve_hvac.rs`, the envelope and infiltration resolution), the defaults
store, and `hares-io/src/hpxml_schedule.rs` (the generated-schedule family,
B1's rewrite target). The parse-to-typed-config seam every premise passes
through exactly once per construction.

**Risk thesis.** The ledger's dominant class concentrated here: OD-10
(undocumented infiltration defaults), OD-19, OD-20, OD-21, OD-31, OD-36,
OD-48, OD-53, OD-57, OD-58, OD-60, OD-63, OD-64: thirteen entries in one
pipeline, all the same shape (a missing or malformed input answered with an
undocumented default instead of a named error or a cited default). The
external sweep adds a meta-defect: the ledger's own quote of the OS-HPXML
flue threshold was stale (EV-8: 0.59 quoted, 0.63 in v1.12.0), and two
citations had drifted (EV-7, EV-9), so the provenance chain that fixes this
class can itself mislead an implementer. I-09 adds four new loaders beside
it (B1's stochastic generator, B2's sampler, D1's density loader, A2's
script engine), each inheriting the substitution temptation.

**Attack plan.** The mutated-fixture corpus: take `base.xml` and the 67-fixture
corpus, delete or corrupt one field at a time, and assert each site either
fails with a named error or applies a default whose citation names the
OS-HPXML file and line (any silent default is a finding); re-fetch every
cited default against v1.12.0 (the EV-8 class); E1's property 5 (parsers
return errors, never panic, on generated arbitrary bytes) at 10k cases; the
B1 rewrite probed with the same corpus before it lands. Row coverage: **E1**
(row 10) lands the parser property; the OD-48 and OD-57 fixes (I-08 rows
21x, 21zh) cover the named sites. Gaps: no row owns the mutated-fixture
corpus, no row re-verifies the citation corpus per default, and E1's five
properties are only five.

**Coverage.** Pins today: `crates/hares-core/tests/od57_red_probes.rs`,
`assembly_errors.rs`, `window_u_factor_shgc_silent_defaults.rs`,
`hpxml_required_storeys.rs`, `occupant_count_silent_zero.rs`,
`defaults_store_policy.rs`, `schedule_columns.rs`. Pin gap: no property
fuzz, no one-field-at-a-time corpus sweep, no per-default citation pin.

## 5. The checkpoint/restore contract

**What it is.** The dwelling snapshot family (`CHECKPOINT_VERSION` 18 after
B-6's nullable-schema bump; the tariff snapshot at schema v3; the thermal
snapshot at version 9 folded at 10), the version gate and the
`PINNED_SCHEMA_SHA256` tripwire, the fleet's chunk checkpoints to come
(B5).

**Risk thesis.** Restore-loss is a proven class: RT-9's red team found the
thermal solver's coupling state, the HRV recovery effectiveness and fallback
state all lost on resume, plus a non-atomic `load_checkpoint`; OR-10
existed because checkpoints carried no billing, metrics, reports, health or
warnings at all; OD-34 found the tariff evaluator unrestored. The contract's
claim is bitwise "resumed equals continuous", and the version gate catches
schema moves, not missing fields: a field absent from both save and restore
passes every gate while the resumed run quietly diverges. I-09 adds actor
state (C2's three actors carry state the acceptance says must round-trip),
event-engine state (A2), chunk-level fleet state (B5) and ratchet history
(D6's 13-month lookback), each a new restore-loss candidate. The I-08
classes here: restore-loss (RT-9, OR-10, OD-34) and ordering (OR-19's
stream cursor).

**Attack plan.** The resume-at-every-phase matrix: kill and restore
mid-island, mid-DR-event, mid-defrost event, mid-billing-period, mid-ratchet
lookback, mid-warmup, mid-roster-transition, mid-stream (the cursor at a
non-zero word position); save under one feature set and restore under
another; a truncated file (the non-atomic load); a version downgrade
refusal; the mutation battery's pattern (two masked mutations were found by
OD-34's round: keep that standard). Row coverage: **E3** (row 4) adds the
checkpoint bench (performance only), **B5** (row 27) the kill-and-resume at
study level. Gaps: no row owns the per-capability resume matrix; E3's bench
says nothing about semantics.

**Coverage.** Pins today: `crates/hares-core/tests/checkpoint_resume.rs`,
`checkpoint_regressions.rs`, `checkpoint_file_resume.rs`,
`tests/python/test_checkpoint_resume.py`, the version-gate tripwire, OD-34's
ratchet/window/TOU resume pins. Pin gap: the island/DR/defrost/roster phase
matrix, cross-feature-set restore, downgrade pins.

## 6. The HELICS boundary and the grid-signal channel

**What it is.** `python/hares/helics/` (`federate.py`, `dwelling.py`,
`fleet.py`) and the Rust-side `set_grid_voltage`
(`dwelling/mod.rs:4991-4998`). The single surface where an outside world
(federated or scripted) writes into the engine's grid state.

**Risk thesis.** Two federates implement two different boundary behaviors:
the dwelling refuses out-of-range voltage (`dwelling.py:725-741`), the fleet
warns and applies anyway (`fleet.py:494-522`), a negative price warns then
applies (A1's citations): the same violation, two outcomes, neither counted
uniformly. `set_grid_voltage` overwrites `frequency_hz` with a 60 Hz
constant on every call, so a frequency study cannot exist; `export_price`
and `ghg_intensity` are never applied; the fleet has no price subscription.
The wheel workaround rides the wrapper's private `_finalizer`
(`federate.py:259-276`): a wheel update silently degrades to a leak path
(A8). The I-08 classes here: substitution (a 60 Hz constant written over
state), silent-fallback (apply-anyway), ordering (intermediate HELICS
grants publishing without stepping, `dwelling.py:583-606`).

**Attack plan.** The out-of-band matrix (voltage, frequency, price, each
below/at/above every bound) into both federates, asserting count-once,
never-applied, and identical outcomes between federates; the
frequency-overwrite probe (set frequency, then set voltage, then read
frequency); per-dwelling price changing the dwelling-level bill; stale-value
semantics at an intermediate grant; the watchdog probe (a simulated wrapper
without `_finalizer` produces exactly one warning). Row coverage: **A1**
(row 13) owns the unified boundary contract and the new channels, **A8**
(row 5) the watchdog. Gaps: A1 depends on I-08 Task 1.6 (the guard built
once); nothing covers the intermediate-grant staleness pin except A3's
multi-rate extension; no row pins federation equivalence of the boundary
contract itself.

**Coverage.** Pins today: about 5,300 pytest helics lines, OD-32's
federation-event synchronization fix (23-commit chain), the port/core
determinism work. Pin gap: boundary uniformity between federates, the
frequency channel (does not exist), price topics, the staleness pin.

## 7. The equipment curve and LUT machinery

**What it is.** `hares-equipment`'s performance surfaces: the multispeed
stage tables and biquadratic curves (`staging.rs:29-46`,
`default_curves.rs`), the coil physics and its degradation
(`coil_physics.rs`, `latent_degradation.rs`), the heat-pump constants and
lockouts, battery degradation (`battery/degradation.rs`), PV
(`pv/mod.rs`, `pv/soiling.rs`), and the envelope LUT
(`EnvelopeLookup` and its convention films).

**Risk thesis.** The class: a curve fit evaluated at a corner or beyond its
table, with extrapolation behavior nobody declared. Found already: OD-54
(an ASHP's COP below 1 at -17 C), the coarse-dt duty quantization
(Hardening 3's table: the realized duty 2.66x the documented fraction at
900 s and 5.32x at 3600 s, measured after Hardening 1's fix), EV-6 (the
SAM-NOCT wind and cover factors dropped), EV-3 (the Henderson-Rengarajan
companion-heating form diverging), B-13 (the LUT R-scaling's film
conventions needed a whole correction round). Each is a wrong number with a
green test suite: the defect hides at the domain edges, exactly where
validation scenarios (cold snaps, sag events, part load) go. The I-08
classes here: physics substitution at edges and unit/scale (the 7.2222 vs
7.3 K depression, the 0.09 percent B1_REF nuance in EV-3 and item 8).

**Attack plan.** The corner census: evaluate every performance curve
exactly at each bin edge and one step beyond, asserting the declared policy
(clamp, extrapolate, error) and that every clamp lands in the Task 2.11
census; the dt sweep across {10, 60, 300, 900, 3600} s asserting the
realized duty against Hardening 3's table; monotonicity properties (capacity
vs OAT, COP vs OAT within the physical envelope, so PLF and staging cannot
invert); the defrost floor at 15.555 C both sides; the multispeed stage
selection at exact boundaries (a capacity equidistant between stages). Row
coverage: **A5** touches the motor families, and the sweep's V9 (the IEEE
1547 settings pack) the trip curves. Gaps: no I-09 row owns a curve-corner
audit, no row pins an extrapolation policy, no dt-sweep gate exists.

**Coverage.** Pins today: `crates/hares-core/tests/envelope_assembly_r.rs`
(B-13's pins), `tests/python/test_py_lut_injection.py`, the BESTEST bands,
the OCHRE parity records, Hardening 3's duty table in
`02-measurement.md`. Pin gap: HVAC stage-boundary pins, extrapolation
policy pins, the dt-sweep gate.

## 8. The tariff/billing engine

**What it is.** `hares-tariff/src/billing.rs` (the demand windows,
per-period peaks, ratchet history, `billing.rs:142-154`; the demand basis
fed `net_power_kw.max(0.0)` at `billing.rs:323`), `types.rs`
(`DemandRate` with lookback months and minimum fraction), `evaluator.rs`,
the mid-run attach and its snapshot (OD-34's schema v3).

**Risk thesis.** Money defects are silent by construction: the demand basis
is net, applied via `max(0.0)` with no documentation and no configuration
(the Axis D assessment's finding), so a PV home's demand charge is
understated with no warning anywhere; D6 (row 7) will fork the basis
(total-import for commercial tariffs, net for residential back-compat),
which is itself a new defect factory (two bases in one evaluator, selected
by a tariff field). The ratchet spans 13 months of state: a lookback or
minimum-fraction bug shows up once a year per home. The I-08 classes here:
ordering (OD-34's mid-run attach period, B-4's boundary-step booking),
restore (the ratchet history in the snapshot), substitution (a basis chosen
silently).

**Attack plan.** The hand-computed bill: a two-day fixture with midday PV
export reconciled line by line under both bases (D6's acceptance, run
before D6 lands as the baseline); a 13-month run walking the ratchet's
lookback and minimum fraction month by month; TOU demand windows at their
edges; a negative price; a DST spring-forward month boundary; a checkpoint
resume mid-ratchet-accumulation; the demand basis fork probed with a tariff
that declares commercial rates on a residential premise. Row coverage:
**D6** (row 7) owns the basis; OD-34's pins own the attach and resume.
Gaps: no negative-price row, no DST-duplicate-hour billing pin, no
resume-mid-ratchet pin beyond OD-34's battery.

**Coverage.** Pins today:
`tests/python/test_tariff_evaluator.py`, `test_tariff_integration.py`,
`test_tariff_builder.py`, `test_tariff_telemetry.py`, the
`month_boundary_billing_3600s` golden, `crates/hares-core/tests/dwelling_billing_boundary.rs`,
`crates/hares-tariff/tests/json_float_roundtrip.rs`, OD-34's mutation
battery. Pin gap: the basis duality, the ratchet walk, negative price, DST
billing.

## 9. The fleet fold's aggregates and the memory math

**What it is.** `hares-fleet` (`fleet.rs`, `Fleet::simulate`'s streaming
fold), the fold's three tables (`aggregate`, `covered_weight`, `homes`),
the declared-kind fold rules (FL-5, B-11), the weights (FL-4's fleet
manifests, PY-12's weight lookups), and B5's arithmetic (the schedule
resample about 50 MB per dwelling until Task 2.14: 10k homes about 500 GB,
so chunking is mandatory).

**Risk thesis.** The fold's rules were just re-plumbed (fold by declared
kind, mode by weighted vote, nulls do not vote, degraded homes out of the
fold), and each rule is a silent-aggregation hazard: a weighted mean over a
nullable column, a mode tie, a union fold that used to empty the aggregate
(FL-2's change), a chunk boundary that reorders the sum. The memory math is
itself a correctness surface: a chunked fold must equal an unchunked fold
bitwise, and nothing pins that at scale. The I-08 classes here:
off-by-one/index (B-11's mean-vs-mode cells, B-5's name-based
classification), ordering (FL-1's shared-checkpoint-file race found at the
fold), substitution (a defaulted weight, PY-12's class).

**Attack plan.** The fold properties: weight permutation invariance, a
single-home fold equal to the home's own frame, null-vote counting against
hand totals, mixed-offset folds equal to single-offset folds run separately
and recombined by hand; the chunk-equivalence probe (the same 200-home
study at chunk sizes 1, 7, 200: bitwise); the kill-mid-chunk resume; the
high-water probe with synthetic 10k homes (a build-failure storm included:
one home's failure must not lose the study). Row coverage: **B5** (row 27)
owns kill-and-resume, **B6** (row 28) the analysis surface. Gaps: no row
owns the fold properties or the chunk-equivalence probe; B5's bitwise claim
is its acceptance but not a standing property.

**Coverage.** Pins today:
`crates/hares-fleet/tests/fleet_all_failed_causes.rs`,
`resstock_retained_output.rs`, the three fleet goldens,
`crates/hares-core/tests/thread_bounds.rs` (bitwise concurrency proofs).
Pin gap: fold properties, chunk equivalence at scale, the 10k probe.

## 10. The weather interpolation and the calendar

**What it is.** The B-1a/b/c surface: the EnergyPlus last-hour point
interpolation and midpoint solar interpolation over hourly files, sub-hourly
files, the first-hour rule, `WeatherYear::Typical/Actual`, the wrap, the
Feb 29 skip, the interval-mismatch error.

**Risk thesis.** This surface was rewritten late in I-08 (rows 79 to 81,
landed 2026-10-16/17): fresh code with the maximum blast radius (every run
of everything reads weather) and a proven near-miss: the first
implementation's 1-minute read lagged 30 minutes and wrapped to the file's
end, and the phase shift passed every test until a bitwise differential
caught it. The calendar edges (wrap, leap skip, DST) are exactly the
off-by-one habitat; the I-08 classes here: off-by-one/index (the hour
offsets) and substitution (the first-hour rule is a declared choice that
could silently flip).

**Attack plan.** The integral property (step-value sums reconstruct the
hourly records bitwise under both interpolation modes, for N in 1 to 60
steps per hour); circle continuity at midnight and the year wrap (wind
direction); the DST spring-forward and fall-back over weather (the leap
golden's neighbors); the typical-year wrap crossing pinned at the fleet
level (B-9's pin, extended); an actual-year file with a missing day (the
construction error). Row coverage: none in I-09's schedule names weather:
that absence is the finding; the natural home is E1's property list. Gaps:
no I-09 row covers weather at all.

**Coverage.** Pins today: `crates/hares-core/tests/weather_interpolation.rs`,
`weather_year.rs`, `resample_weather.rs`,
`timezone_weather_regressions.rs`, `tests/python/test_py_weather.py`,
`test_resample_weather.py`, the leap and wrap goldens. Pin gap: the
integral property, the N-sweep, no standing property at all.

## 11. The Python boundary (GIL, capsule, exceptions)

**What it is.** `hares-python` (the pyo3 bindings, `py_gym.rs`,
`py_result.rs`, the arrow exports through `pyo3-arrow`) and
`python/hares/`. Every value crossing the seam, every exception coming
back, every GIL release/re-acquire.

**Risk thesis.** Both failure modes were found live: OD-1 (engine exceptions
not surviving pickling: a spawn worker's error arriving as the wrong class)
and row 40's deadlock (a callback re-entering the dwelling while holding
the mutex and the GIL, caught only by a subprocess probe). The capsule
surface (PY-4, PY-1, row 59's `PyRecordBatch` delivery) is zero-copy: an
aliasing or lifetime defect there corrupts data without failing. C3 will
put reward components Rust-side but keeps the Python callable escape hatch
on the hot path, and C2's Python actors declare output variables on this
seam. The I-08 classes here: ordering/concurrency (the GIL sections) and
exception mapping (the typed-error churn in rows 42, 43a, 28).

**Attack plan.** The exception matrix: every exported `HaresError` variant
raised inside a spawn worker and inside a callback, asserting class, args
and `__cause__` in the parent; the re-entrancy probes (a callback that
steps the dwelling, attaches a callback, or mutates the roster); the capsule
consumed by polars, by pandas, and by a raw reader in one process, then
across spawn; a Rust sink panic under the GIL during a batch step; the stub
truth sweep (every documented keyword actually accepted, pyright-enforced).
Row coverage: **C6** (row 16) the conformance checker, E5's version
contract the surface freeze. Gaps: no row owns the exception matrix or the
cross-consumer capsule pins; the re-entrancy regression is row 40's
one-off, not a standing property.

**Coverage.** Pins today: `tests/python/test_engine_exceptions_pickle.py`,
`test_callback.py`, `test_py_metrics.py`, `test_py_stubs.py`, the pyright
gate, the pickle round-trip enumeration. Pin gap: the exception matrix,
cross-consumer capsule pins, re-entrancy as a property.

## 12. The gym's bounds and spaces

**What it is.** `py_gym.rs:288-316` and `gym_env.py:191-262`: name-keyed,
type-wide action bound tables with a broad fallback, hardcoded observation
ranges, the `clip_action` path (OD-18's counting), the observation plan
(RT-7's frozen plans), `copy=False` shared buffers.

**Risk thesis.** Every `min_soc` is (0,1) and every power-ish field gets the
broad box whether the heater is rated 5 or 20 kW (C1's finding): the agent's
action semantics silently depend on the roster, and the clamp counter
(OD-18) is the only witness that a study's actions were reshaped. Wrong
observation ranges can hide NaNs and out-of-distribution states.
`copy=False` returns a shared buffer: an aliasing defect is a silent data
corruption. The I-08 classes here: substitution (fallback bounds),
off-by-one/index (name-keyed lookups).

**Attack plan.** The capability cross-check: every built-in roster's derived
bounds equal the rated telemetry (a rated-5 kW heater's bound is 5); the
unknown-field error at construction; the `copy=False` aliasing probe (write
through one env's buffer, read another's); a masked auto-reset under a
failed step; the observation range checks against injected NaN and extreme
telemetry. Row coverage: **C1** (row 17) builds the capability readback,
**C6** the conformance. Gaps: no row owns the aliasing probe or observation
range derivation; C1's acceptance names the bounds but not the buffer.

**Coverage.** Pins today: `tests/python/test_rl_warning_surfacing.py`, the
OD-18 clamp pins, the `rl_step` and `rl_episode` benches. Pin gap:
bounds-vs-capability (C1 does not exist yet), buffer aliasing, observation
range derivation.

## 13. Warm-up convergence

**What it is.** The B-2 loop: the EnergyPlus criteria (0.4 delta C, 0.04
loads fraction, 1 to 25 days, the 100 W floor and 1e-4 W pass-through),
`WarmupResiduals`, the health record, CN-2's strict policy opt-in.

**Risk thesis.** A forged or truncated convergence poisons every downstream
number with no failure: the run completes, the health field records a
converged flag nobody reads unless the strict policy is armed (CN-2 made
the calibration system arm it; nothing else does). The per-zone delivery
read comes from port accumulators (a new-ish wiring), and the max-days
truncation is a silent-by-default substitution. The I-08 classes here:
substitution (a converged flag), ordering (warm-up end versus the first
stepped step).

**Attack plan.** The forged-convergence probes: a premise that oscillates at
the tolerance boundary, a max-days 1 fixture (truncation recorded as
health), warmup None versus disabled distinction, Python/Rust default
equality (B-3's pin, re-probed after the config unifications), a non-finite
residual injection naming the zone. Row coverage: none specific in I-09;
E1's property 4 (the checkpoint roundtrip) wants warm-up landed first, so
the surface is load-bearing for row 10. Gaps: no row probes the convergence
logic itself.

**Coverage.** Pins today: `tests/warmup_regression.rs`,
`tests/python/test_warmup.py`, the diagnostics pin, the B-2 acceptance
fixtures. Pin gap: the forged-convergence class, the strict-policy-not-armed
default.

## 14. The scenario/event scripting engine (A2, new construction)

**What it is.** The declarative event script (A2, row 15): time-windowed
entries of voltage, frequency, price, DR events and named signals, one
evaluation engine driving either the direct setters or a HELICS source
federate, with the standalone-equals-federation bitwise claim as its
acceptance.

**Risk thesis.** New construction on the exact surface where ordering
semantics bite: overlapping windows, non-monotone times, restoration
ordering, and one vocabulary shared by two drive paths. The failure class:
the script engine and the federation publish path disagree by one step (a
staged signal applied at a different step), which no test notices until a
study's bitwise replay claim breaks. The I-08 classes here: ordering
(OD-34's boundary-attach class, B-4's boundary-step class) and
substitution (an unknown signal silently ignored would be the OD-2 class
reborn in a new loader).

**Attack plan.** Run after A2's first commit: the malformed-script matrix
(overlapping events, unknown signals, non-monotone times, each failing with
the field and line named); the same script through both paths asserted
bitwise per step; events exactly on step boundaries and exactly on each
other's boundaries; a restoration script racing a queued control signal.
Row coverage: **A2** is the construction row; the standing review-and-red-team
rule is the only planned adversarial pass. Gap: no dedicated round.

**Coverage.** Pins today: none (the surface does not exist). Pin gap: the
entire surface; the replay-equality claim needs a standing test, not a
one-time acceptance.

## 15. The density-based premise path (D1, new construction)

**What it is.** The config-first premise loader (D1, row 9) beside the HPXML
bedroom path: density-based lighting and plug loads over the existing
`Building` model, skipping the residential appliance set; D2's RTU assembly
and D5's schedule library stack on it.

**Risk thesis.** A second premise path doubles the substitution surface:
`resolve_loads.rs:80-118, :444` sizes every appliance and water-heater draw
per ANSI/RESNET 301 from `NumberofBedrooms`, and the density path must
never reference it (D1's acceptance says so, but acceptance is a moment,
not a property). The shared `Building` model's residential assumptions
(occupant behavior, water-heater draws) are the leak paths. The I-08 class
here: substitution, the ledger's dominant class, at a brand-new loader.

**Attack plan.** Run after D1's first commit: the cross-contamination probes
(a density premise with a stray bedrooms key, an HPXML premise with density
keys, a zero-bedroom residential premise through each path); the
end-use-table-equals-scheduled-integrals check as a standing property; the
energy-closure residual on the office fixture. Row coverage: **D1** (row 9)
and **D7** (row 35) own the construction and the acceptance posture. Gap:
the contamination pins as standing tests.

**Coverage.** Pins today: none (the surface does not exist). Pin gap: the
path-isolation property, the closure property.

---

## The defect-class heat map

Counted from the I-08 ledger by each entry's primary class (57 entries
accounted; embedded sites noted in the text). This is the predictor of
where the next defects hide: the class concentrates where the mechanism
concentrates.

| class | count | where it concentrated (the I-08 evidence) | the I-09 predictor |
|---|---|---|---|
| substitution / silent default / silent skip | 29 | the HPXML resolve pipeline and the defaults store (OD-2, 3, 6, 7, 8, 9, 10, 17, 19, 20, 21, 22, 24, 25, 30, 31, 36, 42, 46, 48, 49, 53, 57, 58, 60, 63, 64, B-6, B-10) | every new loader: B1's generator, B2's sampler, D1's premise path, A2's script engine, A1's grid channel (surfaces 3, 4, 6, 14, 15) |
| ordering / concurrency / lifecycle | 8 | roster and tariff state machines, the fold, HELICS sync (OR-19, OD-16, OD-34, B-4, OD-62, OD-37, OD-32, FL-1's fold race) | island transitions, event scripts, actor state, chunked folds (surfaces 2, 5, 9, 11, 14) |
| off-by-one / index / misclassification | 6 | end-use classification and the fold (OD-43, OD-47, B-5, B-11, OD-40, OD-26) | the catalogue-derived spaces (C1) and per-equipment attribution (C4), the positional equipment topics A6 names |
| unit | 5 | config fields crossing the Python/Rust and HPXML/US-unit seams (B-14's three commits, IN-8, OD-41, OD-14, OD-26) | the grid channel (pu/Hz), the tariff basis (kW), C3's reward components, A7's per-leg amps |
| silent-fallback (execution path) | 4 dedicated, plus embedded sites | env/batch paths, defaults store, geometry (OD-52, B-8, OD-12, OD-64; the `ZoneId(1)` fallback inlined at 18 sites per OR-18's audit; RT-9's fallback state) | the A1 apply-anyway fleet paths, A6's heuristic PV identification, C1's broad fallback bounds |
| RNG / reproducibility | 2 dedicated, plus the plan-named class | event loads and the HELICS test (OD-29's stream 0, OD-32) | the scenario algebra (B3): the class reproduces wherever a new stream dimension is added |
| checkpoint/restore loss | 3 | the snapshot family's per-domain state (RT-9, OR-10, OD-34) | every surface 14 and C2 state added without a resume-matrix row (surface 5) |

Reading: substitution is an order of magnitude ahead and lives wherever
inputs meet defaults, so the four new loaders of I-09 are the
highest-density habitat for the next 30 entries. Ordering is second and
lives wherever state machines meet, which is precisely what A2, A3, C2 and
B5 build. RNG is small by count and enormous by blast radius: one shared
keystream in B3 invalidates every study built on it silently.

## The attack calendar: the next six red-team rounds

Ordered so each round's findings land before the rows that would repeat the
class, and each round is one charter line the attacking agents can hold.

1. **The brownout round** (surfaces 1 and 2, before A5 and alongside A3's
   design). Charter: "Drive the channel's full admitted band and the island
   boundary until the premise answers with physics instead of clamps: every
   clamped step that was not counted, every phantom interchange at the
   meter, every Z/I/P composition that scales wrong is a finding." Expected
   findings' class: substitution (the clamp), uncounted-event (OD-18's
   class), ordering at island transitions.

2. **The correlation round** (surface 3, before B2/B3/B5 land the scenario
   dimension). Charter: "Two scenarios that must be independent are guilty
   until proven innocent: prove every stream disjoint by construction,
   every interleaved re-run bitwise stable, and name every keystream two
   draws share." Expected findings' class: RNG (the OD-29 class), plus
   checkpointed stream cursors.

3. **The substitution census round** (surface 4, with surface 13's
   forged-convergence probes riding). Charter: "One field at a time, make
   the corpus incomplete: every default that fires without naming its
   OS-HPXML citation, every parse that survives malformed bytes, and every
   convergence that arrives too easily is a finding." Expected findings'
   class: substitution (the dominant class), parser panics.

4. **The resume round** (surface 5, with surface 8's ratchet state riding).
   Charter: "Kill everything mid-anything: island, DR event, defrost,
   ratchet accumulation, roster transition, stream position; a restored run
   that does not bitwise the uninterrupted run, and any state that failed
   to enter the snapshot, is a finding." Expected findings' class:
   restore-loss (RT-9's class), ordering.

5. **The corner round** (surfaces 7 and 8). Charter: "Rate every curve at
   its bin edges and one step beyond, sweep the timestep across the duty
   table, and reconcile every bill by hand under both demand bases: corners,
   extrapolations and bases that disagree with the hand computation are
   findings." Expected findings' class: off-by-one/index at boundaries,
   unit (pu, kW, basis), physics substitution at domain edges.

6. **The boundary round** (surfaces 6, 11 and 9's scale probes). Charter:
   "Cross every boundary hostile: the out-of-band matrix into both
   federates and the fleet, exceptions across spawn and callbacks, the
   capsule into three consumers, a killed 10k-home study resumed mid-chunk;
   any inconsistency between the two federates or two consumers is a
   finding." Expected findings' class: ordering/concurrency (GIL, grants,
   races), silent-fallback (apply-anyway), exception-mapping.

Each round files its findings as ledger entries against the surface numbers
above; a finding inside a planned row's scope names the row and narrows the
row's acceptance; a finding outside every row's scope proposes its own row
per the schedule's rules.
