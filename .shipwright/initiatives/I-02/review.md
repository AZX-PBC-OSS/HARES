---
review_outcome: clean_with_nits_applied
---

# Review — I-02 (envelope_loads_kwh components don't balance)

## What the ticket asked for, and where it is answered

The ticket's symptom: `dw.metrics().envelope_loads_kwh` summed to +141 MWh/yr on
a home consuming 15,352 kWh/yr, driven almost entirely by
`opaque_solar_lwr_kwh` (+121,278 kWh) — a phantom gain incompatible with the
physically-sane indoor temperatures. The ticket's actual question was "confirm
the intended scope of the wall/roof/floor term, and whether the nine components
are meant to balance."

The answer landed in code is that the nine were *not* all meant to balance:
`opaque_solar_lwr_kwh` was the gross exterior-skin absorption (an outside-face
boundary condition), never a zone load. The zone-load contract now carries the
net interior-face convection, and the answer is enforced, not merely stated:

- **`opaque_conduction_kwh` (was `opaque_solar_lwr_kwh`)** — sourced from
  `wall_heat_gain_w + floor_heat_gain_w + roof_heat_gain_w`
  (`crates/hares-core/src/dwelling/mod.rs`, the `envelope_cols` "Opaque
  Surface Heat Gain - Indoor (W)" entry), the already-computed per-boundary
  interior-face convection (EnergyPlus ERM 26.1 — "Inside Heat Balance":
  Interior Convection). Accumulated from the corresponding CSV column in
  `crates/hares-io/src/output/metrics.rs` (`opaque_conduction_w_idx`).
- **Budget completed** with `window_conduction_kwh` and `internal_mass_kwh`
  (OCHRE's Window and Internal Mass component-load categories).
- **Gross term removed from "Net Sensible Heat Gain - Indoor (W)"** — the
  ~13.5 kW inflation on the gate fixture is gone.
- **Gate test** `opaque_surface_indoor_heat_gain_is_net_conduction`
  (`tests/envelope_opaque_loads.rs`) asserts the reported column equals the
  summed per-boundary net columns.

The resolution is physically right and destination-correct: the reported
budget is now the quantity F13 retrofit targeting needs, each field matching a
documented E+/OCHRE component-load semantics.

## Why I found nothing to block on

I attacked this from the angles where the defect class lives — physics,
wiring, and test truthfulness — and each attack either came back green or
resolved as a documented, owner-sanctioned decision.

### 1. Physics validity of the two new derivations

The session introduced two genuine first-principles changes. I did not take
either on faith; each is independently checkable and I checked both.

**Exterior skin `rad_res` (parallel Thévenin form).**
`crates/hares-envelope/src/boundary_rc.rs` (`SkinCoupling`,
`skin_rad_coupling`) documents and computes the eliminated-skin resistance as
`R_film·R_half/(R_film + R_half) / A`, with `rad_frac = R_film/(R_film +
R_half)`. The current-divider identity `rad_res == rad_frac · R_half / A` is
exactly the Thévenin equivalent of a capacitor-free node between two series
resistances, and EnergyPlus `CalcOutsideSurfTemp` solves the same balance with
every path carrying its own parallel conductance — so the change moves HARES
*toward* the E+ benchmark, which matches the direction the operator stated.
The independent pin is not re-derivation-by-the-author: it is a closed-form
Newton solve of the full nonlinear skin balance
(`thermal_solver::longwave::tests::iterative_skin_temperature_satisfies_exact_skin_balance`)
plus the `tests/exterior_skin_balance.rs` closure suite across resistance
ratios 0.02–100 and the fallback-R case. I ran the latter:

```
cargo test -p hares-envelope --test exterior_skin_balance
test result: ok. 5 passed; 0 failed  (incl. skin_balance_holds_across_film_to_layer_resistance_ratios,
                                       fallback_r_exterior_wall_steady_state_matches_exact_skin_balance)
```

**Interior beam distribution (cosine parallel-flood, P15).**
`solar.rs::beam_cosine_factor` and `compute_solar_distribution_into_generic`
replace the legacy `sin(altitude)` floor fraction with a per-face
`max(0, −cosα·sinτ·cos(φ−ψ) − sinα·cosτ)` weight. I hand-derived the four
limits against the code and they hold: floor (τ=180°) → `sin α`; ceiling
(τ=0°) → always 0 (ceilings never see direct beam); wall opposite the sun
(ψ = φ±180°) → `cos α`; window-side wall → 0. The azimuth convention
(outward normal, clockwise from north, interior face = opposite) is
self-consistent. Energy conservation is pinned by a 256-case randomized test
(`solar_distribution_conserves_energy_randomized`) plus the forced-fallback
pin (`beam_parallel_to_all_walls_executes_fallback_and_lands_on_surfaces`)
which distinguishes the fallback branch from the normalized branch — the
exact seam the red-team had earlier flagged as discounted.

### 2. Wiring completeness (forward direction)

Every new field, flag, and optional dependency has both a supplier and a
reader; I verified each by grep/read rather than assumption:

- `opaque_conduction_kwh` / `window_conduction_kwh` / `internal_mass_kwh` —
  produced by `MetricsCalculator::finish`, exposed in
  `crates/hares-python/src/py_metrics.rs` and `python/ochre_next/_hares.pyi`.
  `grep -rn opaque_solar_lwr_kwh` over the tree returns *only* the
  intentional `assert not hasattr(env, "opaque_solar_lwr_kwh")` pin — no
  stale reference to the renamed field survives.
- `StreamingRecorder::enable_metrics` / `take_metrics` / `total_rows` —
  suppliers wired in `dwelling/mod.rs` (construction and the
  equipment-change refresh path) and `writer.rs`; readers in
  `engine.rs::finalize_run_metrics` via `Dwelling::take_streamed_metrics` /
  `recorded_rows`.
- `renewable_energy_fraction` unclamped — only consumer is the Python getter;
  `grep` confirms no clamping consumer anywhere that would break on `> 1.0`.
- `ThermalSolverConfig::validate` + the construction-time wiring-range pass,
  ground-depth presence check, ground index/depth parallelism check, and the
  dedicated-column duplicate check — all invoked from `ThermalSolver::new`
  before any timestep, with each formerly-silent fallback (index-0
  attribution, 0 °C ground, 20 °C interior-LWR substitution, dropped outdoor
  column, ambient solar) now a named construction error.

### 3. Test truthfulness

The tests that matter assert observable quantities, not their own setup:

- The gate test compares the reported column against independently-computed
  per-boundary net columns (implementation-independent), not against a value
  the test itself injects.
- The skin closure tests solve the exact balance by independent Newton
  iteration.
- The energy-identity randomized test derives the reference from the documented
  semantics and checks all three accumulators, not just the identity.
- The streaming==batch equivalence is a real equivalence over the same
  deterministic run, and I confirmed the engine's `finalize_run_metrics`
  honors the honest zero-step / no-recorder / init-failure distinctions
  rather than the old false "zero-step" flag.

`cargo test -p hares-core --test envelope_opaque_loads` → 6 passed, 0 failed
(includes the gate, the budget closure, per-field CSV-integral equality, and
the residual-vs-loads bound). `cargo test -p hares-core --test
wiring_properties` → 4 passed, 0 failed.

### 4. Gate integrity

The parity corpus and BESTEST tolerances were re-shaped into drift-locks
("accumulated-drift overrides" in `tests/parity/mod.rs`, the "Prop-1 ratchet"
in `tests/bestest/mod.rs`). I looked hard at whether this is a silenced gate:
it is a redefinition of what those tests assert (drift-lock ±1 % rather than
ASHRAE/OCHRE conformance). I am treating it as a **deliberate, documented
owner decision, not a code defect** — the signed band distance still prints on
every run, the drifted metrics are tracked tickets, the ratcheted BESTEST
cases are strictly *more* signal than the `#[ignore]` they replaced, and the
OCHRE corpus is explicitly a ballpark comparison point rather than a
correctness oracle. It is a conformance-stance question, not a correctness bug
in the fix under review, and it was confirmed as intended during this review.

## Acknowledged limitations (real constraints, conceded)

- **Parity/BESTEST drift-lock bands** — as above; a conformance-policy
  decision with visible band-distance output and tracked follow-up, not a
  defect in the physics/reporting change.
- **PV generation on non-finite meter rows is dropped** (`metrics.rs`): the
  load-behind-meter triple is kept self-consistent by skipping a NaN-total
  row rather than counting its PV; `nan_step_count` reports the dropout. A
  defensible accounting choice with a pinning test; recorded so the
  data-dropping consequence is visible.
- **`interior_lwr_kwh` stays a non-summable gross metric** — carried over as
  the documented exception, correct.

## Fixes applied (trivial, by me)

- `crates/hares-io/src/output/metrics.rs` — struct doc now says the component
  fields *approximate* the zone air balance to within the semi-implicit/TARP
  split reported separately as the "Zone Air Heat Balance Residual (W)"
  column, instead of the overstated "close the zone air heat balance".
- `crates/hares-io/src/config.rs` — `time_res_secs_u32` doc corrected: it is
  a fallback for out-of-`u32`-range counts (including negative), not a
  saturation.
- `crates/hares-io/src/output/metrics.rs` — added
  `renewable_fraction_exceeds_one_for_net_exporter`, pinning the documented
  "net-positive home reports > 1.0" contract (5 kWh PV behind a −2 kW net →
  5/3). Air cover, not a defect: the test passes, confirming the accounting.

## Observations (non-blocking)

- `renewable_energy_fraction` naming: a "fraction" that legitimately exceeds
  1.0 surprises casual readers; a name like `pv_share_of_gross_load` would
  carry the semantics without the doc caveat. Cosmetic; not raised as a
  finding.
- `internal_mass_kwh` sits inside `EnvelopeComponentLoadsKwh` while its doc
  correctly notes it is a redistribution term, not an envelope load. This
  mirrors OCHRE's COMPONENT_LOAD_MAP grouping and is internally consistent.

## Verified

- `cargo test --no-fail-fast -p hares-envelope -p hares-io -p hares-core` → **78 `test result: ok` binaries, 0 failures, exit 0** (ran 2026-09-12 in this review).
- `cargo clippy --all-targets -p hares-envelope -p hares-io -p hares-core -- -D warnings` → **exit 0, zero warnings** (ran 2026-09-12).
- `cargo fmt --check` → **clean, exit 0** (ran 2026-09-12, after my edits).
- `uv run pytest tests/python/test_py_metrics.py -q` → **9 passed, exit 0** (ran 2026-09-12).

Fix resolution (root cause → change → green): the RCA traced
`opaque_solar_lwr_kwh` to `ThermalSolver::build_input_vector`, which injected
the gross exterior absorption into each surface's own exterior RC node; the
fix re-sources "Opaque Surface Heat Gain - Indoor (W)" from
`wall_heat_gain_w + floor_heat_gain_w + roof_heat_gain_w`
(`crates/hares-core/src/dwelling/mod.rs`) and `MetricsCalculator` reads that
same column into the renamed `opaque_conduction_kwh`. The gate test
`opaque_surface_indoor_heat_gain_is_net_conduction` is green now (my run).
The pre-fix red (13,581 W reported vs 104.6 W net) is recorded in
`reproduction.md` with observed output; I did not re-derive it by reverting
the fix, because reverting working-tree state is prohibited — the pre-
fix failure evidence is the reproduce phase's recorded run, the post-fix
green is my own run.

Regression surface: dependents of the changed symbols are the Python getters
(`py_metrics.rs`/`_hares.pyi`, exercised by the 9 pytest cases), the
freefloat oracle and parity/bestest/resstock gates (all green in the 78-
binary run), and fleet aggregation (passes `renewable_energy_fraction:
None` through, does not compute it — confirmed by grep). Edge cases traced
and pinned rather than listed: non-finite meter rows
(`energy_identity_holds_across_non_finite_total_rows`), net-exporting PV
generation (my added `renewable_fraction_exceeds_one_for_net_exporter`),
battery export divergence (documented caveat + the randomized identity
test's `−Σ min(total+pv_gen,0)` term), and streaming vs retained-batch
metrics equivalence (`tests/engine.rs`).

Similar-issue sweep: the class is "a reported quantity does not mean what
its name says." I searched `crates/`, `tests/`, and `python/` for the renamed
symbol (`opaque_solar_lwr_kwh` — only the intentional absence pin remains)
and for consumers of the unclamped `renewable_energy_fraction` (one getter,
no clamping consumer). The fix phase's own sweep (six instances of the
class, each with a pinning test) is recorded in
`coverage-note.md` / `initiative.md`; I did not find an additional instance.

- `cargo test -p hares-core --test envelope_opaque_loads` → **ok. 6 passed; 0 failed** (ran 2026-09-12).
- `cargo test -p hares-core --test wiring_properties` → **ok. 4 passed; 0 failed** (ran 2026-09-12).
- `cargo test -p hares-envelope --test exterior_skin_balance` → **ok. 5 passed; 0 failed** (ran 2026-09-12).
- `cargo test -p hares-io --lib renewable_fraction` → **ok. 3 passed; 0 failed**, including the new `renewable_fraction_exceeds_one_for_net_exporter` (ran 2026-09-12, after my edit).
- `grep -rn opaque_solar_lwr_kwh` across `crates/ tests/ python/` → only the intentional `assert not hasattr(env, "opaque_solar_lwr_kwh")` pin; no stale callers of the renamed field.
- `grep -rn renewable_energy_fraction` → single consumer (`py_metrics.rs` getter); no clamping consumer that the unclamped `> 1.0` path could break.
- Read `boundary_rc.rs` `skin_rad_coupling` and hand-checked `rad_res == rad_frac·R_half/A` (parallel Thévenin form); read `solar.rs` `beam_cosine_factor` and hand-derived floor/ceiling/opposite-wall/window-wall limits against the code — all four match.
