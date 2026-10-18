---
"id": "I-01"
"title": "equipment_zip not describing voltage response"
"status": "active"
"kind": "fix"
"description": "Summary. Every typed equipment reports as constant power (zp=0,
  ip=0, pp=1), yet varying HARES service voltage measurably changes consumption.
  Anything computed from the published coefficients returns zero sensitivity.\ 


  Reproduce:import ochre_next as on


  HPXML, SCHEDULE, WEATHER = \"<bldg0046363.xml>\", \"<schedule.csv>\",
  \"<G4804390 TMY3.epw>\"

  KW = dict(start_time=\"2018-01-01 00:00:00\", duration_s=365*86400,
  time_res_s=900,

  \          bldg_id=46363, master_seed=0, output_verbosity=6,

  \          write_output=True, output_to_parquet=True,

  \          output_path=\"./out\", defaults_path=\"<ABSOLUTE path to
  defaults>\")


  # --- Part A: what the published coefficients say
  -----------------------------

  dw = on.Dwelling.from_hpxml(HPXML, SCHEDULE, WEATHER, **KW)

  dw.initialize()

  for name in dw.equipment_names():                 # or however equipment is
  enumerated

  \    print(name, dw.equipment_zip(name))

  # Observed: every typed equipment returns zp=0, ip=0, pp=1 — constant power.

  # A constant-power premise has, by definition, zero voltage sensitivity.


  # --- Part B: what the engine actually does
  -----------------------------------

  def annual_kwh(pu):

  \    d = on.Dwelling.from_hpxml(HPXML, SCHEDULE, WEATHER, **KW)

  \    d.initialize()

  \    d.set_grid_voltage(pu)      # AFTER initialize (ZIP models resolved),
  BEFORE simulate

  \    df = d.simulate()

  \    return df[\"Total Electric Power (kW)\"].sum() * (900/3600)


  lo, nom, hi = annual_kwh(0.95), annual_kwh(1.00), annual_kwh(1.05)

  print(f\"0.95 pu: {100*(lo-nom)/nom:+.2f}%   1.05 pu:
  {100*(hi-nom)/nom:+.2f}%\")



  Expected from this script: 0.95 pu: −1.18%, 1.05 pu: +1.38% — i.e. 0.236 and
  0.276 %P per %V. Part A says zero. Same home, same seed, same build.


  The engine's ~0.25 %P/%V is sensible and matches published residential
  figures. It is the coefficients that don't describe it. The response evidently
  comes from load whose ZIP equipment_zip doesn't expose, plausibly the
  scheduled/untyped base load rather than typed equipment.


  What we need: is equipment_zip meant to describe whole-premise voltage
  response, or only the typed equipment it names? If the latter, what is the
  supported route to premise-level ZIP? The engine holds the information — it
  responds correctly when driven."
"created_at": "2026-09-10T20:40:10.832Z"
"created_by": "operator"
regression_tests:
  - tests/python/test_equipment_zip_voltage_response.py
  - crates/hares-core/src/dwelling/premise_zip.rs
  - crates/hares-equipment/src/scheduled_load.rs
  - crates/hares-io/src/schedule.rs
  - crates/hares-types/src/schedule.rs
  - crates/hares-equipment/src/event_load.rs
  - crates/hares-io/src/defaults.rs
  - crates/hares-equipment/src/schedule_helpers.rs
  - crates/hares-equipment/src/config.rs
  - crates/hares-types/src/zip.rs
---

Summary. Every typed equipment reports as constant power (zp=0, ip=0, pp=1), yet varying HARES service voltage measurably changes consumption. Anything computed from the published coefficients returns zero sensitivity. 

Reproduce:import ochre_next as on

HPXML, SCHEDULE, WEATHER = "<bldg0046363.xml>", "<schedule.csv>", "<G4804390 TMY3.epw>"
KW = dict(start_time="2018-01-01 00:00:00", duration_s=365*86400, time_res_s=900,
          bldg_id=46363, master_seed=0, output_verbosity=6,
          write_output=True, output_to_parquet=True,
          output_path="./out", defaults_path="<ABSOLUTE path to defaults>")

# --- Part A: what the published coefficients say -----------------------------
dw = on.Dwelling.from_hpxml(HPXML, SCHEDULE, WEATHER, **KW)
dw.initialize()
for name in dw.equipment_names():                 # or however equipment is enumerated
    print(name, dw.equipment_zip(name))
# Observed: every typed equipment returns zp=0, ip=0, pp=1 — constant power.
# A constant-power premise has, by definition, zero voltage sensitivity.

# --- Part B: what the engine actually does -----------------------------------
def annual_kwh(pu):
    d = on.Dwelling.from_hpxml(HPXML, SCHEDULE, WEATHER, **KW)
    d.initialize()
    d.set_grid_voltage(pu)      # AFTER initialize (ZIP models resolved), BEFORE simulate
    df = d.simulate()
    return df["Total Electric Power (kW)"].sum() * (900/3600)

lo, nom, hi = annual_kwh(0.95), annual_kwh(1.00), annual_kwh(1.05)
print(f"0.95 pu: {100*(lo-nom)/nom:+.2f}%   1.05 pu: {100*(hi-nom)/nom:+.2f}%")


Expected from this script: 0.95 pu: −1.18%, 1.05 pu: +1.38% — i.e. 0.236 and 0.276 %P per %V. Part A says zero. Same home, same seed, same build.

The engine's ~0.25 %P/%V is sensible and matches published residential figures. It is the coefficients that don't describe it. The response evidently comes from load whose ZIP equipment_zip doesn't expose, plausibly the scheduled/untyped base load rather than typed equipment.

What we need: is equipment_zip meant to describe whole-premise voltage response, or only the typed equipment it names? If the latter, what is the supported route to premise-level ZIP? The engine holds the information — it responds correctly when driven.

## Implementation Notes

Fix landed as two capabilities, both verified through the real Python bindings:

1. **Per-equipment regime flag.** `resolved_zip()` now returns a
   regime-carrying `ResolvedZip` (`crates/hares-types/src/zip.rs`): the
   coefficients plus `real_power_zip_applies`. The flag is set at the two
   canonical resolvers — `resolve_zip()` (governing: scheduled/event loads)
   and `resolve_reactive_zip()` (Rule R1 pin: HVAC, water heaters,
   ventilation), and at DER synthesis sites (battery, EV, PV — controller-
   driven real power) — so no equipment can publish a ZIP without declaring
   its regime. `equipment_zip()` dicts carry the flag; physics equipment
   report `False` with their pinned `(0, 0, 1)`, scheduled loads `True`.
2. **Premise-level ZIP.** `Dwelling.premise_zip()` aggregates the
   power-weighted mean ZIP over the ZIP-governed equipment (weights: schedule
   column mean over the loaded data, analytic source mean, or pre-extracted
   event-series energy) and names what it excludes
   (`constant_power_equipment`) and each governing entry's weight. `None`
   when no governing equipment has a computable positive expected draw —
   never a silent constant-power answer. Physics equipment's share of total
   draw is knowable only by simulation, so a whole-premise %P/%V combines
   this aggregate with those shares (documented on the API).

### Deviations

- **`SimulationEngine::run`/`run_dwelling` no longer misreport successful
  runs as "zero-step"** (exposed by the failed `cargo test --workspace`
  gate): the zero-step detection used `flushed_batches()`, which the
  output writer documents as *"always returns an empty slice"* when
  `retain_batches` is `false` (the default) — so every default-config run
  was flagged `"zero-step simulation: no metrics computed"` and silently
  returned `empty_metrics()` despite a complete output file on disk
  (regression tests: `run_without_retained_batches_reports_retention_not_zero_step`,
  `run_with_zero_duration_reports_zero_step` in
  `crates/hares-core/tests/engine.rs`). The signal is now the simulation's
  step count; a run with steps but no retained batches is flagged
  `"metrics not computed: no batches retained"` with a warning naming the
  remedy.
- **`PyControlSignal.power_setpoint` now accepts negative `kw`** (operator
  direction, "no broken windows"): the Python wrapper rejected negative
  active power while the core `ControlSignal` contract is signed
  (positive = charge, negative = discharge; V2G/V2H discharge strategies and
  the battery's own Rust tests command negative setpoints), breaking
  `test_py_equipment_mutation::test_min_soc_prevents_deep_discharge`,
  `test_gym_env::test_vec_gym_out_of_bounds_actions_no_nan`, and
  `test_helics_integration::test_control_signal_via_helics` (all failing at
  pristine HEAD before this change). The wrapper now validates finiteness
  only, matching the core; the wrong-contract tests were corrected to pin
  the signed semantics.
- **`alignment_oracles_regressions`**: the three `#[should_panic]`
  placeholders ("fix pending on fixture availability") had their premise
  satisfied by untracked reference-parquet fixtures appearing in the working
  tree, so they failed by not panicking. Converted to real alignment tests
  that skip (with a logged reason, following the parity-corpus convention)
  when a fixture's reference parquet is absent and assert for real when
  present.

### Known Limitations

- **What:** `premise_zip()`'s reactive aggregate (`zq`/`iq`/`pq`, `pf`) is a
  first-order power-weighted mean, not an exact reduction.
- **Constraint:** no exact reduction exists for a pf-heterogeneous mix —
  true premise Q is `Σ Pᵢ·tanφᵢ·rqᵢ(V)`, which cannot be factorized into a
  single `(pf, rq)` pair for all V; `tan(acos(pf))`-weighted `pf` is the
  standard first-order composite (Q is proportional to tanφ).
- **Tried:** none — the constraint is mathematical, not tooling; the real
  side (which the ticket is about) reduces exactly because coefficient rows
  sum to 1.
- **Resolvable when:** never — fundamental; callers needing exact premise Q
  at a specific voltage should simulate (the reactive side is not what this
  ticket asked for).
- **What:** `cargo test --workspace` retains two failures,
  `hares-core::parity parity_outputs_against_reference_corpus` and
  `hares-core::regression parity::parity_outputs_against_reference_corpus`.
- **Constraint:** untracked `tests/fixtures/parity/*/reference_output.parquet`
  files (another agent's in-flight work, present before this change, absent
  from `git ls-files`) turn those tests into active comparisons against
  60-row cold-start 1-hour reference windows with no committed provenance,
  and the engine genuinely deviates on those short windows:
  conditioned-zone temperature MAE 0.68-1.69 C (allowed 0.6), short-window
  HVAC energy 44-206% (allowed 25-48), while the single 24-hour fixture
  (`resstock_bldg0112631_24h`) passes temperature and energy -- the failures
  are transient-dominated short windows. Failure signatures are bit-identical
  at pristine HEAD (`c2e12206`) with the same parquets copied in, so the gaps
  predate this change. Closing them requires deciding whether the
  unprovenanced references or the engine physics are wrong -- the parity
  fixture owner's in-flight call (the repo already tracks this class of known
  physics gap as ticketed open work, e.g. BESTEST cases ignored with
  "T-0301" provenance). Loosening the committed tolerances or ignoring the
  test would silence a check reporting real deviations.
- **Tried:** verified pre-existence at HEAD in a pristine worktree; inspected
  the reference parquets (row counts, window shape) to rule out a harness
  alignment bug; fixed the adjacent engine defect the gate output exposed
  (the false "zero-step" flag, see Deviations) -- the metric deviations
  themselves remain, and touching the untracked fixtures or committed
  tolerances in a shared checkout would decide another agent's open work.
- **Resolvable when:** the parity-fixture owner commits the reference corpus
  with provenance and either closes the physics gaps or recalibrates the
  fixtures/tolerances; escalated in `decision-log.md`.

- **What:** `premise_zip()`'s reactive aggregate (`zq`/`iq`/`pq`, `pf`) is a
  first-order power-weighted mean, not an exact reduction.
- **Constraint:** no exact reduction exists for a pf-heterogeneous mix —
  true premise Q is `Σ Pᵢ·tanφᵢ·rqᵢ(V)`, which cannot be factorized into a
  single `(pf, rq)` pair for all V; `tan(acos(pf))`-weighted `pf` is the
  standard first-order composite (Q is proportional to tanφ).
- **Tried:** none — the constraint is mathematical, not tooling; the real
  side (which the ticket is about) reduces exactly because coefficient rows
  sum to 1.
- **Resolvable when:** never — fundamental; callers needing exact premise Q
  at a specific voltage should simulate (the reactive side is not what this
  ticket asked for).
- **What:** `cargo test --workspace` (nextest) retains two failures,
  `hares-core::parity parity_outputs_against_reference_corpus` and
  `hares-core::regression parity::parity_outputs_against_reference_corpus`.
- **Constraint:** untracked `tests/fixtures/parity/*/reference_output.parquet`
  files (another agent's in-flight work, present before this change and not
  committed) turn those tests into active comparisons, and the build's
  outputs deviate from the references by 25–156% on HVAC-energy/temperature
  metrics — verified bit-identical failure signatures at pristine HEAD
  (`c2e12206`) with the same parquets copied in, so the discrepancy
  predates this change and belongs to whoever owns the parity fixtures.
- **Tried:** none that would be honest — "fixing" it means either changing
  engine physics or regenerating the untracked references, both of which
  decide the other agent's in-flight parity work; the fixtures themselves
  must not be touched in a shared checkout.
- **Resolvable when:** the parity-fixture owner commits or regenerates the
  reference corpus against the current engine.

### Coverage-triage fix (second pass)

`schedule_column_mean_kw` (`crates/hares-core/src/dwelling/premise_zip.rs`)
clamped with `f64::max`, which drops a NaN operand — a schedule column
containing NaN (reachable: the CSV loader parses `str::parse::<f64>()`,
which accepts `"nan"`, `crates/hares-io/src/schedule.rs:389`) was averaged
as if the NaN timesteps drew zero power, silently skewing the published
premise weight. Any non-finite value in a column now makes the mean
`None` ("expected draw not computable"); the aggregation rosters such
equipment with `mean_power_kw: None`, distinguishable from a real weight.
Semantic decision (owned here, per the triage's escalation): degrade to
`None` at the mean rather than reject at the CSV boundary — the narrower
change; boundary rejection would hard-fail dwelling construction for any
NaN-bearing schedule and alter load-side semantics beyond this
initiative's scope. Pinned by
`nan_schedule_data_must_not_degrade_to_a_silently_skewed_mean`.

### Python test-interpreter init race (gate-blocking, pre-existing)

The `hares-python` lib test binary crashed with `Fatal Python error:
_PyImport_Init: global import state already initialized` — reproduced
identically at pristine HEAD (`c2e12206`), so pre-existing and previously
masked: fail-fast runs aborted at the earlier parity failure before
`hares-python` ran. Root cause: the `test_requires_python`
(`py_control.rs`) and `ensure_python` (`py_actor.rs`) test helpers called
raw `pyo3::ffi::Py_Initialize()` from parallel test threads — 20 call
sites; the helpers' own SAFETY comment claimed idempotency that CPython
3.13 does not provide on concurrent second-init. Fixed by delegating to
`Python::attach` (the crate's production pattern; with the
`auto-initialize` feature it initialises exactly once, thread-safely).
Verified 5/5 consecutive green runs of the binary and a fully green
`cargo test --workspace`.

### Coverage-triage fix (third pass): non-finite schedule data, class-closed

The class measurement found the same silent-absorption pattern at a second
site: `scheduled_load.rs` per-timestep consumption, where
`raw_schedule_kw > 0.0` is false for NaN, degrading corrupt input to a
silent 0.0 kW draw (red test:
`nan_schedule_value_must_not_degrade_to_silent_zero_draw`). Closed at three
levels:

1. **`ScheduleSource::value_at`** (`hares-types/src/schedule.rs`) — the
   shared funnel for every source kind and every consumer — now rejects
   non-finite values with an error naming the defect. This is what turns
   the red test green: `step()` propagates the error.
2. **CSV parse boundary** (`hares-io/src/schedule.rs`) — `parse::<f64>()`
   accepts "nan"/"inf"; the loader now rejects them naming row and column
   (the constitution's earliest stage; retires the input class for every
   real-world path at once). This supersedes the second-pass note that
   deferred boundary rejection — the class measurement showed the
   load-side "silently absorbs" behavior is a defect, not acceptable
   parity.
3. **`ScheduledLoad::expected_mean_power_kw`** — a non-finite
   `ScheduleSource::mean()` (a Constant NaN source is a producer the prior
   triage pass missed) degrades to `None` on the premise roster instead of
   a weight-looking `Some(NaN)`.

Regression pins: `non_finite_values_are_rejected_not_silently_absorbed`
(hares-types, NaN/±inf), and
`non_finite_schedule_values_are_rejected_at_the_parse_boundary` (hares-io,
"nan"/"inf"/"-inf"/"NaN"/"infinity", asserts row+column context), plus the
triage's red test passing unmodified.

### Defaults strict-mode env race (gate-blocking, pre-existing)

The gate's `cargo test --workspace` run failed once on
`defaults::tests::generator_dir_ignores_csv_with_battery_header_columns`
(`MissingFile(".../HVAC Multispeed Parameters.csv")` in a minimal temp dir)
— a scheduling race, not a code regression: the test passes in isolation
and passed in every prior gate round. Root cause:
`strict_missing_multispeed_csv_returns_missing_file_error` mutated the
process-global `HARES_STRICT_DEFAULTS` env var via `unsafe set_var`/
`remove_var` around its call, under a SAFETY comment asserting "nextest
runs each test in its own process" — false for the gate's `cargo test`
runner, where tests share one process and run in parallel. While the var
was held set, any concurrent `DefaultsStore::load` on a dir without the
multispeed CSV took the strict hard-error path. Fixed by adding an
internal `load_with_strict_defaults(dir, strict)` seam (public `load`
unchanged, still env-driven for CI opt-in); the strict test injects
`strict=true` directly — no env mutation — and the warning-path test pins
`strict=false` so an ambient CI setting cannot flip it. Verified: 6/6
consecutive green runs of the `hares-io` lib binary and a fully green
`cargo test --workspace`. Residual, named: a CI that exports
`HARES_STRICT_DEFAULTS` globally would still fail the remaining
minimal-dir tests that call plain `load` — an opt-in env feature with no
setter in this repo (the gate's single-failure signature proves the race,
not an ambient var); hardening every load-calling test against it would
be a broad change beyond the observed failure.

### Red-team pass: non-finite ZIP coefficients + month-multiplier expectations

Two defects, five red tests, all fixed at the layers the red team framed
(mirroring the schedule-data pattern):

**Finding 1 — non-finite ZIP coefficients admitted and silently
aggregated.** Three layers:
1. `load_zip_parameters` (`hares-io/src/defaults.rs`) — the earliest
   stage: rejects non-finite coefficients naming file, row, and field
   (TOML parses `nan`/`inf` natively).
2. `validate_zip_sums` (`hares-equipment/src/config.rs`) — the init-time
   backstop: rejects non-finite `zp/ip/pp/zq/iq/pq/pf/v0` before the
   sum checks (NaN defeats `(sum-1).abs() > tol`; infinite `pf` clamps
   to 1 zeroing Q; infinite `v0` rescales P to `pp`), and rejects
   non-positive `v0` (division operand in `apply`), per the red team's
   invitation. `debug_assert_zip_sums` delegates, so the step-time
   invariant gains the same guard.
3. `aggregate_premise_zip` (`hares-core`) — defense in depth: a
   governing load with non-finite coefficients is rostered but not
   weighted (the unknown-mean bucket), never published inside a
   structurally normal aggregate.
   The e2e root cause was one layer deeper than the loader:
   `DwellingBlueprint::from_parts` swallowed *every* defaults-load error
   (warn + empty store), silently swapping all equipment defaults for
   class-table fallbacks. Now `DefaultsError::MalformedToml` (corrupt
   data) propagates — `Dwelling.from_hpxml` raises naming the defect and
   row — while unavailable defaults (Io/MissingFile) keep the documented
   warn-and-degrade fallback.

**Finding 2 — `expected_mean_power_kw` ignored the month multipliers its
own step honors.** `ScheduledLoad` now scales the published mean by the
mean of the twelve multipliers (the `ScheduleSource::mean()` DailyProfile
convention; exact for constant sources). `expected_event_mean_power_kw`
takes the multipliers: uniform scales (incl. all-zero → 0, identity →
unscaled) scale exactly; *partial* seasonal scaling returns `None` —
which month each replay step falls in is not derivable from the series
(the red team's recorded design decision, resolved to the honest
"not computable without simulation").

Regression tests: the five red-team tests, unmodified, all green
(`validate_zip_sums_rejects_non_finite_coefficients`,
`non_finite_coefficients_are_never_published_as_an_aggregate`,
`test_non_finite_zip_sidecar_fails_loudly_with_cause`,
`expected_mean_power_accounts_for_month_multipliers`,
`expected_mean_power_is_zero_when_every_month_is_zeroed`).

### Coverage-triage fifth pass: ColumnRef arm + month-multiplier boundary

The red-team Finding-2 fix covered the constant arm of
`expected_mean_power_kw` but not the `ColumnRef` arm of the same method —
one method, two arms, one fixed (red test:
`column_sourced_load_with_zeroed_months_must_not_publish_unscaled_column_weight`).
Fixed with the semantics grounded in the channel's real users
(`defaults/Default Schedule Parameters.csv` + the resolve layer):

- **Production column sources carry no runtime month multipliers** — the
  resolve layer bakes monthly scaling into the generated column values
  (`schedule_resolve.rs:1088`, `hpxml_schedule.rs:117`), so the plain
  `ScheduleColumn` deferral is correct for every production load and is
  unchanged.
- With runtime `month_multiplier_N` present on a column source:
  all-zeroed → `Kw(0.0)` (unambiguous, matches step); identity → plain
  deferral; any other configuration → `None` ("not computable without
  simulation" — which month each column step falls in is unknowable from
  the column alone), never an unscaled deferral the premise aggregate
  would weight by a draw the equipment cannot take.
- **Boundary**: `parse_month_multipliers` now rejects non-finite
  multipliers (previously `val.max(0.0)` silently dropped NaN and zeroed
  the month — the same silent-absorption class closed everywhere else);
  the shared parse site covers both consumers (scheduled + event loads),
  erroring at init with the key named.

Design decision record: the triage offered scale-aware deferral or
explicit `None`; `None` was chosen because the scale-carrying case
(uniform non-identity runtime multipliers on a column source) has no
producer — production months are baked into columns, and the runtime
channel on a column source is a contradictory over-specification whose
honest answer is "not computable," not a scale to thread through the
shared enum.

### Red-team second pass: tiny-v0 overflow + roster audit honesty

Two defects in the prior round's own fixes, three red tests, fixed at the
boundary each channel actually has:

**Finding 1 — positive-but-tiny `v0`.** `v0 > 0` + finiteness is
necessary but not sufficient: a subnormal v0 (1e-320) makes `v / v0`
infinite — NaN real power even for constant-power rows (`0 * inf`) and a
mid-simulation panic on the pf=1 Q-invariant; a tiny v0 (1e-200) squares
past `f64::MAX` — infinite real power. Both pass every prior guard; the
red team executed the override channel end-to-end (builds clean, panics
at first step). Fixed at two boundaries:
- `validate_zip_sums` (hares-equipment, the init backstop for every
  ZIP-bearing equipment): a validated ZIP must produce finite real and
  reactive power at nominal voltage — the polynomials are evaluated at
  `v_norm = 1/v0` and non-finite results are rejected naming v0.
- `merge_zip_override` (hares-core conversions, the override channel's
  earliest stage — overrides merge after the loader, at
  `build_from_blueprint`, where non-critical equipment init failures are
  deliberately skip-and-warn, so an init-time rejection alone would
  silently drop the load): the merged v0 must keep `v / v0` in range;
  violation errors at build naming the field and equipment, so
  `Dwelling.from_hpxml` raises with its cause instead of deferring the
  typo to a mid-step crash.

**Finding 2 — rostered weight the aggregate silently ignored.** The
coefficient guard excluded a corrupt-coefficient load from the weighted
sums but rostered its real positive draw — `Some(2.0)` that lies, breaking
the module's own caller-side audit contract (recomputing the weighted
mean from rostered positive weights must reproduce the aggregate). Fixed:
a load whose ZIP coefficients are non-finite takes the established
unknown-mean semantics — rostered by name with an explicitly absent
weight (`None`), never a positive weight that was silently ignored.

Also corrected (red team's noticed-not-pinned): the ColumnRef arm comment
now states the true reason for its `None` — the exact value is computable
but not expressible in `ExpectedMeanPower`; honest over-degradation, not
"not computable without simulation."

Regression tests: the three red-team tests, unmodified, all green
(`validated_zip_produces_finite_power_at_nominal_voltage`,
`rostered_positive_weights_are_the_weights_the_aggregate_used`,
`test_tiny_v0_zip_override_fails_loudly_with_cause`).

### Red-team third pass: magnitude bounds replace point probes

Near-maximum cancellation coefficients (`zp = 1.79e308, ip = -1.79e308,
pp = 1.0`) passed every guard layer — fields finite, the sum cancels to
exactly 1.0, and the nominal-voltage probe cancels identically — yet
produce `inf - inf = NaN` real power at 1.05 pu, the service voltage this
initiative's own reproduction drives (red team executed end-to-end:
builds clean, publishes garbage, panics at first step). The class: a
single-voltage probe cannot bound band behavior.

Fix — magnitude bounds, not more sampled points, in
`hares_types::zip::validate_plausible_magnitudes` (the type's home,
single source of truth for both validation call sites):
- coefficients within `ZIP_COEFFICIENT_MAGNITUDE_MAX = 100`
  (literature table tops out near 28.6; an order of magnitude of
  headroom, six orders below overflow territory);
- `v0` within `[0.1, 10]` (per-unit reference; literature uses 1.0 —
  also closing the razor off-nominal tiny-v0 window at 7.5e-155 the red
  team noticed).

With those bounds the largest product across the ±5 % band — including
the `tan(acos(pf))` ceiling just above the `1e-9` sentinel — is ~1.2e21,
far below `f64::MAX`: the overflow class is unrepresentable, which
provably subsumes both prior point probes (nominal-voltage probe in
`validate_zip_sums`, v0 probe in `merge_zip_override`); both were
replaced, not accumulated. Wired into `validate_zip_sums` (init backstop)
and `merge_zip_override` (the override channel's earliest stage, where
non-critical init rejections are deliberately skip-and-warn — an
init-only rejection would silently drop the load). Verified end-to-end:
the cancellation override now fails `Dwelling.from_hpxml` naming the
equipment and field.

Regression tests: the red-team test unmodified
(`validated_zip_produces_finite_power_across_the_service_voltage_band`)
plus new hares-types pins (`magnitude_bounds_reject_overflow_class_rows`
— cancellation row and the tiny-v0 window;
`magnitude_bounds_accept_every_literature_row` — all 48 class rows and
sane non-default v0 stay accepted).
