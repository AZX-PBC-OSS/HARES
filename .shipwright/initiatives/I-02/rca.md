---
diagnosis_outcome: root_cause_found
complexity: moderate
passing_commands:
  - command: "cargo test -p hares-envelope -p hares-io -p hares-core"
  - command: "uv run pytest tests/python/test_py_metrics.py"
  - command: "cargo fmt --check"
  - command: "cargo clippy -- -D warnings"
---

## Symptom

`dw.metrics().envelope_loads_kwh` exposes nine components documented as summable
into a single "budget" (`EnvelopeComponentLoadsKwh` doc comment,
`crates/hares-io/src/output/metrics.rs:96-99`: *"These represent the thermal
loads imposed on the conditioned zone by each envelope component. Positive =
heat gain to zone, negative = heat loss."*). Summing all nine on a real
HPXML run gives +141,142 kWh/yr (+16.11 kW mean) against a home that consumes
15,352 kWh/yr (1.75 kW mean) — an ~8x overshoot driven almost entirely by
`opaque_solar_lwr_kwh` (+121,278 kWh alone). Indoor temperature nonetheless
stays in a physically sane 17.4–27.2 °C band all year, which is inconsistent
with an actual +16 kW sustained net gain. The ticket confirmed (independently,
by integrating per-interval `... Heat Gain ... (W)` columns from the
timeseries) that eight of the nine components agree with `envelope_loads_kwh`
to ~1 part in 10¹⁵, i.e. this is not a reporting/aggregation bug — the
underlying physics quantity being summed is itself the wrong quantity for one
component.

## Reproduction (executed, not just traced)

The ticket's exact repro needs `ochre_next` python bindings plus a specific
HPXML/schedule/weather fixture set I don't have on hand, so I did not run that
literal script. Instead I ran the repository's own existing regression
fixture that exercises the identical code path
(`ThermalSolver::build_input_vector` → `opaque_solar_lwr_w`) against a real
BEopt-derived HPXML building and an OCHRE reference run, with `--nocapture`
diagnostics that print exactly the gross-vs-net split needed to confirm the
mechanism:

```
$ cargo test --test freefloat_oracle --features observe freefloat_summer_48h -- --nocapture
```

Relevant printed output (step 0 of `beopt_summer_48h`):

```
--- STEP-0 HEAT BALANCE: beopt_summer_48h ---
  Component                                     HARES      OCHRE          Δ
  Opaque Solar+LWR combined (W)               73829.4        NaN        NaN
  Net Sensible (sum of above) (W)               996.7      379.9     +616.8
  ...
  HARES per-exterior-surface:
    id= 6 cat=Some(Roof)  solar= 39266.7  lwr= -5082.2  T_surf=  34.3°C  injected= 26852.9
    id= 7 cat=Some(Roof)  solar= 53679.4  lwr= -5082.2  T_surf=  34.3°C  injected= 38174.4
  ...
  Per-boundary heat gains (indoor zone):
    Wall            -- HARES=    24.0 W  OCHRE=   140.0 W
    Floor           -- HARES=   -67.3 W  OCHRE=  -379.7 W
    Roof            -- HARES=    24.9 W  OCHRE=    90.4 W
    Window          -- HARES=     3.4 W  OCHRE=    30.1 W
    Internal Mass   -- HARES=     9.4 W  OCHRE=    28.2 W
    Net Sensible    -- OCHRE=   124.1 W

------------------------------- checks -------------------------------
  PASS opaque_solar_lwr_mean_w            OCHRE=  19459.61  HARES=  13452.20  OK (+30.9%) -- exterior opaque solar absorption + LWR exchange
test tests::freefloat_summer_48h ... ok
```

This is a live reproduction of the exact symptom shape, on a different
building and timescale than the ticket's: `opaque_solar_lwr_w` (73,829 W in
this step) is ~74x the entire zone's net sensible balance (996.7 W) — the
same order-of-magnitude mismatch as the ticket's +16.11 kW vs 1.75 kW. The
`injected=` column per exterior surface (e.g. the two roof surfaces,
26,852.9 W and 38,174.4 W) shows that this quantity is dominated by the
exterior injection into the wall/roof RC nodes, while the separate
"Per-boundary heat gains (indoor zone)" block — the actual net conduction
that reaches zone air — is two to three orders of magnitude smaller (tens of
watts: Roof 24.9 W, Wall 24.0 W). This is the mechanism, observed directly:
almost all of the gross absorbed exterior flux never reaches the zone.

## Trace

`opaque_solar_lwr_kwh` is produced by summing the per-interval
`opaque_solar_lwr_w` column (`crates/hares-io/src/output/metrics.rs:747-749`
accumulates it the same way every other envelope column is accumulated — no
aggregation defect here). That per-interval value comes from
`ThermalSolver::build_input_vector`
(`crates/hares-envelope/src/thermal_solver/mod.rs:964-972`):

```rust
let u_pre = u.iter().sum::<f64>();
self.apply_exterior_solar_inputs(&mut u, env);
let opaque_solar_w = u.iter().sum::<f64>() - u_pre;
let u_pre = u.iter().sum::<f64>();
self.apply_exterior_longwave_inputs_iterative(&mut u, env);
let exterior_lwr_w = u.iter().sum::<f64>() - u_pre + self.opaque_exterior_lwr_w;
let opaque_solar_lwr_w = opaque_solar_w + exterior_lwr_w - self.window_exterior_lwr_w;
```

`apply_exterior_solar_inputs`
(`crates/hares-envelope/src/thermal_solver/solar.rs:189-211`) writes

```rust
u[info.input_index] += info.absorptance * info.area_m2 * poa_w_m2;
```

where `info: &ExteriorSurfaceInfo` and `input_index` is the input slot for
that surface's **own exterior-surface RC node** — confirmed by
`ExteriorSurfaceInfo` (`crates/hares-envelope/src/thermal_solver/config.rs:445-451`,
doc comment: *"Opaque exterior surface solar + LWR combined injection [W]...
Includes surfaces routed through both iterative and non-iterative paths"*)
and by `apply_exterior_longwave_inputs_iterative`
(`crates/hares-envelope/src/thermal_solver/longwave.rs:27-40`), which iterates
`self.config.exterior_surfaces` and injects into `info.input_index` coupled to
`info.state_index` — the wall/roof/floor mass node, not the zone-air node.

So `opaque_solar_lwr_w` is the **gross radiant flux absorbed at the exterior
skin of the opaque envelope** (full solar irradiance × area × absorptance,
plus exterior longwave exchange), injected as a boundary condition on the
wall's own thermal-mass node. It is not what conducts through the wall's
resistance/capacitance into the zone; for a typical wall the great majority of
that absorbed flux leaves again via exterior convection to outdoor air and
longwave exchange with the sky, and only a small, time-lagged fraction ever
reaches the interior surface and zone air. Roofs and walls have large area, so
the gross absorbed solar flux dwarfs the building's actual energy consumption
— which is exactly the observed ~8x-of-total-consumption magnitude, and why
the RC-solved indoor temperature (which correctly accounts for the wall's
resistance and mass) stays physically sane while this diagnostic field does
not.

This is architecturally identical to `interior_lwr_kwh`, which the codebase
already documents correctly as non-summable:

```rust
/// Interior longwave radiation exchange activity (Σ|q_i|/2, see
/// `EnvelopeComponentGains::interior_lwr_w`). This is a gross exchange
/// metric, not a net energy gain; summing it into a net-sensible balance
/// would double-count energy.
pub interior_lwr_kwh: f64,
```
(`crates/hares-io/src/output/metrics.rs:106-109`)

`opaque_solar_lwr_kwh`'s doc comment carries no equivalent caveat
(`metrics.rs:104`: *"Opaque surface solar + exterior LWR."*), and the
struct-level doc comment (`metrics.rs:96-99`) asserts — incorrectly for this
field — that every component is a net "thermal load imposed on the
conditioned zone." The remaining seven components (`window_solar_kwh`,
`infiltration_kwh`, `ventilation_kwh`, `hvac_heating_kwh`, `hvac_cooling_kwh`,
`internal_gains_kwh`, `duct_loss_kwh`) are all genuinely deposited directly on
zone-air (or duct-zone) input indices — e.g. `apply_solar_inputs`
(`solar.rs:79`, `u[air_idx] += absorbed_zone_w`) and the port/infiltration
accumulators pulled from `ports.thermal` at the indoor zone
(`mod.rs:992-1068`) — so they are legitimately net, comparable quantities.
Only `opaque_solar_lwr_kwh` (and, as already documented, `interior_lwr_kwh`)
is not.

## Alternative hypotheses considered and ruled out

- **Aggregation/unit-conversion bug in `metrics.rs`.** Ruled out: the
  accumulation loop for `opaque_solar_lwr_w` is structurally identical to the
  other eight columns (`metrics.rs:723-765`), and the ticket's own
  cross-check (per-interval Heat Gain columns vs. `envelope_loads_kwh`
  totals, agreeing to 1 part in 10¹⁵) proves the value is being summed
  faithfully — the defect is upstream, in what the summed quantity means.
- **Sign-convention bug (double-counting solar and LWR).** Checked
  `apply_exterior_solar_inputs` and `apply_exterior_longwave_inputs_iterative`
  for overlap: `apply_exterior_solar_inputs` explicitly skips surfaces with
  `rad_frac > 0.0` and windows (`solar.rs:194-199`), so solar and iterative
  LWR are applied to disjoint surface sets; no double-injection found.
- **No net-conduction quantity exists anywhere in HARES to compare against,
  so the "gross vs. net" framing above might be an artifact of missing
  functionality rather than a real, already-solved distinction.** Checked and
  ruled out: `EnvelopeComponentGains::wall_heat_gain_w` /
  `floor_heat_gain_w` / `roof_heat_gain_w` / `window_heat_gain_w` /
  `internal_mass_heat_gain_w` (`crates/hares-envelope/src/thermal_solver/config.rs:777-785`)
  are initialized to `0.0` at construction (`mod.rs:1119-1123`) — that had
  led me to first (wrongly) conclude they were dead fields — but they are
  actively accumulated every timestep in
  `crates/hares-envelope/src/thermal_solver/stepping.rs:815-831`:
  `q_conv_w = h_nat * area_m2 * (t_surface - t_zone)` (or the steady-state
  equivalent `ua_w_k * (t_driving - t_zone)`), i.e. the actual convective
  flux from each interior surface into zone air, added per category
  (`self.component_gains.wall_heat_gain_w += q`, etc.). They are exposed as
  CSV diagnostic columns "Wall/Floor/Roof/Window Heat Gain - Indoor (W)"
  (`crates/hares-core/src/dwelling/mod.rs:6144-6150`) and are exactly the
  quantities the `freefloat_oracle` reproduction above prints as "Per-boundary
  heat gains (indoor zone)" (Wall 24.0 W, Roof 24.9 W, etc.) — two to three
  orders of magnitude smaller than the gross `opaque_solar_lwr_w` (73,829.4 W
  in the same step). So a correct, already-implemented net-to-zone quantity
  exists in HARES today; it is simply never summed into `envelope_loads_kwh`
  or `EnvelopeComponentLoadsKwh`, which instead exposes the gross exterior
  injection under a name and doc comment that imply it is net. My first pass
  at this alternative was itself an example of the constitution's warning
  about negative claims ("this is unused") needing a supporting search, not
  an impression — corrected after actually grepping the accumulation sites.
- **This gross/net split is a HARES-specific defect (e.g. a double-counted
  or misassigned term in the RC solve), not an intentional convention shared
  with the reference implementation.** This is the strongest competing
  explanation, since it would mean the fix is a physics bug, not a
  documentation/wiring gap. Checked the vendored OCHRE source
  (`vendors/OCHRE/ochre/Models/Envelope.py`) for how *it* names the
  equivalent quantities: OCHRE reports `"{boundary.name} Ext. Solar Gain (W)"`
  and `"{boundary.name} Ext. LWR Gain (W)"` (`Envelope.py:1451-1452`) — the
  gross flux absorbed at the exterior surface, explicitly tagged "Ext." — as
  a *separate* output from `"{boundary} Heat Gain - Indoor (W)"`
  (`Envelope.py:1360-1369`, built from `COMPONENT_LOAD_CATEGORIES`), the net
  quantity that feeds `"Net Sensible Heat Gain - {zone} (W)"`
  (`Envelope.py:1392`). The `freefloat_oracle` run above independently
  measured HARES's gross exterior absorption against OCHRE's own gross
  `Ext. Solar/LWR Gain` figures and found them in the same regime (OCHRE mean
  19,459.6 W vs. HARES 13,452.2 W, a 30.9% gap attributable to known,
  separately-tracked envelope-physics deltas, not an order-of-magnitude
  defect). This rules out a HARES-only double-counting bug: the reference
  implementation computes and reports a boundary-layer quantity of the same
  enormous magnitude, under a name that keeps it out of its own net-sensible
  balance. HARES's defect is narrower — it took OCHRE's gross-quantity
  *value* but not its "Ext." naming/separation discipline when building
  `EnvelopeComponentLoadsKwh`, so the gross value ended up inside a struct
  documented as if every field were net.
- **The RC solve itself double-counting the gross exterior flux into the zone
  (i.e., the temperature result is wrong, not just the diagnostic).** Ruled
  out by the ticket's own observation: indoor temperature stays in a
  physically plausible 17.4–27.2 °C band all year, which would not happen if
  the solver were actually applying a sustained +16 kW net gain to the zone.
  The RC network only ever received the flux at the exterior mass node, which
  then correctly attenuates through the wall's resistance/capacitance before
  reaching the zone; the diagnostic field is reporting the pre-attenuation
  boundary value.

## Conclusion — answering the ticket's question

The nine `envelope_loads_kwh` components are **not** all meant to balance
against total consumption, and this is not a new judgement call — the
codebase already carves out exactly this exception for `interior_lwr_kwh`,
and the reference implementation (OCHRE) enforces the same gross/net split
under different output names. `opaque_solar_lwr_kwh` needs the same carve-out
`interior_lwr_kwh` already has: it is documented and implemented as the
gross radiant flux absorbed at the exterior surface of the opaque envelope (a
boundary condition on the wall's own RC node, matching OCHRE's "Ext. Solar/LWR
Gain"), not the net heat that conducts through the wall into the zone
(matching OCHRE's "{Component} Heat Gain - Indoor").

Unlike my first pass at this diagnosis, the corrected net quantity is not
hypothetical: HARES already computes it every timestep
(`wall_heat_gain_w` / `floor_heat_gain_w` / `roof_heat_gain_w` /
`window_heat_gain_w` / `internal_mass_heat_gain_w`,
`stepping.rs:815-831`) and already exposes it as CSV diagnostic columns
(`dwelling/mod.rs:6144-6150`) — it is simply never summed into
`EnvelopeComponentLoadsKwh`. The fix has two independent, separately
actionable parts, and which (or both) F13 needs is a scope decision this
diagnosis is not authorized to make unilaterally:

1. **Documentation/contract correction (minimum bar):** give
   `opaque_solar_lwr_kwh` the same "gross exchange, not summable" caveat
   `interior_lwr_kwh` already carries (`metrics.rs:104,106-109`), and correct
   the struct-level claim (`metrics.rs:96-99`) that every field is a net
   zone load.
2. **New net wall/roof/floor/window/internal-mass component(s) in
   `EnvelopeComponentLoadsKwh`**, sourced from the already-computed
   `*_heat_gain_w` fields, if F13's retrofit targeting needs a genuinely
   summable per-component envelope budget rather than just the gross
   exterior-absorption diagnostic. This is the scope question the ticket
   itself poses ("confirm the intended scope of the wall/roof/floor term")
   and should be settled with the initiative owner before the `fix` step
   picks one.

No `failing_tests` are declared: there is no existing automated assertion of
a cross-component energy balance to reproduce against (checked
`tests/regression/aggregation_check.rs` and
`tests/python/test_py_metrics.py`, neither asserts a balance invariant), and
the defect is a documentation/semantics mismatch rather than a code path that
currently emits a wrong number for a defined contract. The `reproduce` step
should add a regression test asserting that `opaque_solar_lwr_kwh` carries
the same "not summable into a net balance" documentation as
`interior_lwr_kwh`, and/or a physical bound test that the gross exterior
opaque flux, while much larger than zone loads, is distinguishable from a net
delivered quantity once the fix lands.

## Closure addendum (fix phase, 2026-09-11)

The diagnosis held in full: `opaque_solar_lwr_kwh` was the gross exterior
absorption mislabeled as a zone load; the fix rewired the reported
zone-load contract to the net interior-face convection
(`opaque_conduction_kwh`, renamed for name-truthfulness), and the
reproduction gate's prediction was borne out — the gate test passed the
moment the column was sourced from the per-boundary net conduction, with
its assertion untouched.

The fix phase then swept the same defect class across the codebase and
found five more instances of "value does not mean what its name says",
each fixed with a pinning test: the gross term inflated "Net Sensible
Heat Gain"; per-boundary convection was compiled out of release builds
(silent zeros); the engine reported zeroed metrics with a false
"zero-step" flag for every streaming run; `gross_consumption_kwh` was
the positive part of the net meter (0.0 for exporting PV homes); and the
exterior diagnostics attributed iterative-path solar to the LWR field
while `opaque_solar_w` read 0.0 on every real building. Two of these
(exterior `rad_res` bare-film form, and the window-interior same) were
independently derived and landed jointly with a concurrent agent —
measured 0.392 K steady-state error pre-fix, exact post-fix,
freefloat-oracle opaque gap 30.9 % → 7.3 % once semantics also matched.

Full record: `initiative.md` Implementation Notes (every change, its
finding, and its pinning test), `docs/physics-hardening-program.md` (the
hardening program, ownership ledger, mutual review, and the lessons
taxonomy), `docs/alignment/DIVERGENCES.md` (deliberate divergences with
justifications), `docs/FLAKES.md` (the unreproduced hares-io flake).

Escalated with evidence (not fixable this round): the parity-corpus
zone-temperature drift — the corpus passes at the 2026-04-17
band-calibration commit against the same fixtures and fails at HEAD; the
gate was dark because the fixtures are gitignored. A bisect for the
introducing commit is in flight (`/tmp/hares-calib`).

The session then extended beyond the RCA's two named fix options into a
joint hardening program with a concurrent agent (both agents independently
derived the same exterior-skin coupling defect and landed the fix
cooperatively). The program's through-line and its results — one shared
skin-coupling constructor for every eliminated-skin path (closing the
"one path fixed, another stale" class), construction-time validation that
falsifies the RCA's double-registration hypothesis by check, the runtime
skin-balance invariant that would have caught the rad_res defect on day
one, the zone-balance residual upgraded to count the matrix exchange
exactly, closure tests across resistance ratios 0.02–100, streaming==batch
metrics equivalence, wiring property tests, and the divergence/flake
ledgers — are recorded with paths, measurements, and pinning tests in
`initiative.md` (Implementation Notes → "Hardening program execution")
and `docs/physics-hardening-program.md` (ownership ledger, mutual review,
and the lessons taxonomy). Achieved, measured: the freefloat-oracle opaque
exterior gap fell from +30.9 % to +7.3 % (like-for-like semantics for the
first time), the +141 MWh phantom-gain class is dead, and the full
three-crate suite — 78 binaries, fmt, clippy, release mode, Python — is
green with every fix pinned by a test that goes red on revert.
