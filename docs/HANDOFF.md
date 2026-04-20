# HARES pre-public-release handoff

Scope: items remaining before the `initial-implementation` branch squash-merges
into `main`. The workspace currently builds clean (`cargo clippy --all-targets
-D warnings`), formats clean (`cargo fmt --check`), and all 3013 lib tests +
all integration suites pass with two documented xfails (BESTEST 900 / 900FF).

Commit state at handoff: `ba133ca fix: close dispatch ordering,
silent-default, and test-integrity blockers` on branch
`initial-implementation`.

---

## 1. BESTEST Case 900FF — min-temp +2.50 °C above band

**Priority: highest.** This is the only genuinely-wide physics gap; everything
else on the list is <1 % or cosmetic.

**Test**: `tests/bestest/mod.rs::bestest_case_900ff`, currently xfail via
`#[should_panic(expected = "metric=min_zone_temp_c")]`.

**Measured**: minimum free-float zone temperature 0.90 °C across an 8760-step
annual run, vs ASHRAE 140 Table B8-3a band `[-6.4, -1.6]` °C. We are
**+2.50 °C above** the upper bound. Peak temperature is comfortably inside
band (43.14 °C vs `[41.6, 44.8]` °C), so the defect is specifically in
night-time / cold-weather heat loss in heavyweight construction.

**Ruled out**:
- Not a lightweight-construction issue — sibling Case 600FF passes both
  metrics cleanly. The defect only shows up with heavy mass.
- Not a solar gain issue — peak temp is inside band.
- Not an actor / dispatch issue — no actors in a BESTEST case, and the
  recent dispatch-ordering fix closed Case 600 without moving 900FF.

**Candidate code paths** (in likely order):
1. `crates/hares-envelope/src/boundary_rc.rs:149-174` — heavyweight RC
   construction. Node count / mass placement in the wall assembly
   determines how much capacitance sees nighttime exterior temperatures
   vs stays coupled to the interior air node. A single-node wall (common
   simplification) under-drains at night.
2. `crates/hares-envelope/src/longwave_radiation.rs:169-210` — exterior
   longwave sky-loss. A clear night sky is ~-40 °C effective radiant
   temperature; if HARES is under-reporting the sky-view-factor-weighted
   radiant loss on opaque surfaces, the interior stays too warm.
3. `crates/hares-envelope/src/thermal_solver/infiltration.rs:62-120` —
   AIM-2 / Sherman-Grimsrud stack-and-wind superposition under the diurnal
   driving-pressure cycle. Heavy construction with under-estimated
   nighttime infiltration would retain heat into morning.

**Investigation suggestion**: enable the detailed observer feature and run
900FF for 48 hours starting in January. Dump interior/exterior surface
temperatures on each wall assembly and the sky-LWR term. Compare against a
hand calculation for a single representative wall. If surface temperatures
show the interior mass node staying coupled to room air rather than
sagging overnight, the fix is in `boundary_rc.rs`. If surface temps look
right but the infiltration rate at 2 AM is < 0.3 ACH, the fix is in
`infiltration.rs`.

---

## 2. BESTEST Case 900 — annual heating +0.021 % above band

**Priority: low.** Marginal. Very likely a side-effect of whatever
fixes 900FF.

**Test**: `tests/bestest/mod.rs::bestest_case_900`, xfail via
`#[should_panic(expected = "metric=annual_heating_load_kwh")]`.

**Measured**: annual heating load 2041.43 kWh vs ASHRAE 140 Table B8-2
upper band 2041.00 kWh. Overshoot is 0.43 kWh / year — a **0.021 %**
margin. Annual cooling is comfortably inside band (2806.34 vs
`[2132, 3415]`).

**Physical reading**: the heavyweight envelope loses slightly more heat
than the reference implementations predict. Same family of causes as 900FF
(heavy-wall mass placement, infiltration under diurnal drive, sky-LWR),
just manifesting in conditioned-mode annual energy rather than free-float
minimum temperature.

**Suggestion**: do not investigate this in isolation. Fix 900FF, then
re-run 900 and see if it closes. If 900FF's fix is in wall-assembly node
placement, 900 is extremely likely to close as well.

---

## 3. Physics constants that should match EnergyPlus / ASHRAE, not OCHRE

HARES inherited several constants from OCHRE that are known to differ from
the currently-published ASHRAE / EnergyPlus defaults. Our repo-level rule
(`feedback_ashrae_not_ochre.md`) is to target ASHRAE / EnergyPlus physics
and treat OCHRE only as a ballpark reference. These should be revisited
together as a "physics constants audit" ticket.

### 3a. Solar absorptance default — `SOLAR_ABSORPTANCE_DEFAULT = 0.60`

**Location**: `crates/hares-envelope/src/longwave_radiation.rs:67`, used in
`crates/hares-envelope/src/thermal_solver/mod.rs:734, 2980, 3237`.

**Issue**: EnergyPlus uses 0.70 as its default for opaque exterior surfaces
when no material is specified (EnergyPlus Engineering Reference §3.2.4,
"Exterior Convection and Absorption"). HARES currently uses 0.60, which
OCHRE inherited from a much older source.

**Impact**: 0.60 → 0.70 is a 16.7 % relative increase in absorbed shortwave
on opaque envelope surfaces with unspecified absorptance. This directly
reduces heating load and increases cooling load in any simulation where
HPXML doesn't pin the value. May help close BESTEST 600 cooling and /or
shift 900 heating — worth testing.

**Action**: change to 0.70; add a provenance comment citing
EP Eng. Ref §3.2.4; re-run BESTEST 600, 900, 600FF, 900FF and envelope
oracles; expect cooling loads up, heating loads down.

### 3b. Air density pinned constant — `AIR_DENSITY_KG_M3 = 1.2041`

**Location**: `crates/hares-envelope/src/boundary_rc.rs:17`, used at
`boundary_rc.rs:281, 1053, 1069, 1082` and elsewhere.

**Issue**: 1.2041 kg/m³ is dry-air density at 20 °C / 101 325 Pa / sea level.
For a simulator that operates across climate zones (cold dry to hot humid)
and can run at altitudes from sea level to ~2500 m (Denver at 1600 m is
already in the base fixture), using a single constant is a known bias.
ASHRAE HoF 2021 Ch.1 gives the full T, P, humidity correction; EnergyPlus
uses the Peng-Robinson correlation.

**Impact on what we're computing**:
- Zone thermal capacitance uses ρ·c_p·V (boundary_rc.rs:281) — a 5 %
  density error = 5 % capacitance error = changed thermal time constants.
- Infiltration mass flow uses the same ρ — 5 % error in mass flow = 5 %
  error in sensible infiltration load.

**Action**: replace the constant with a function of
`(temperature, pressure, humidity)` that is called at init time with the
site's design conditions, and at each step for the sensible-load calc.
Use the ASHRAE HoF §1.8 expression. Pressure correction from elevation is
the biggest piece (≈10 % lower density at 1600 m).

### 3c. Occupant heat gain — 66 W sensible / 51.2 W latent

**Location**: `crates/hares-physics/src/constants.rs:125` (`OCCUPANT_SENSIBLE_GAIN_W = 66.0`),
`:131` (`OCCUPANT_LATENT_GAIN_W = 51.2`).

**Issue**: HARES inherited these from OCHRE (comment says
"OCHRE Envelope.py:904-907: total gain = 400 BTU/h per person; sensible
fraction = 0.563"). 400 BTU/h total = 117.2 W total. ASHRAE 62.1-2022
Table 6.2.1.1 and HoF 2021 Ch.18 Table 1 give seated adult heat gains
as **75 W sensible / 55 W latent = 130 W total** for "moderately active
office work", which is the convention used for residential simulation.

**Impact**: +14 % on sensible occupant gain, +7 % on latent. Affects
indoor temperature during occupied hours, cooling peak, and latent
ventilation load. Larger dwellings with many occupants (family of 5)
will see a measurable cooling-energy shift.

**Action**: update to 75 W / 55 W with ASHRAE citation. Add a TOML /
config hook if user wants to override per-dwelling for specific activity
levels (ASHRAE HoF 2021 Ch.18 Table 1 has values for 11 activity
categories). Document the change as a physics fix, not a breaking-API
change.

### 3d. Zone mass multiplier — fallback `_ => 7.0`

**Location**: `crates/hares-core/src/dwelling/conversions.rs:24-31`.

```rust
pub fn mass_multiplier_for_zone(zone_type: &ZoneType) -> f64 {
    match zone_type {
        ZoneType::Conditioned => 7.0,
        ZoneType::Foundation => 1.0,
        ZoneType::Attic | ZoneType::Garage => 1.0,
        _ => 7.0,  // <-- this fallback
    }
}
```

**Issue**: the `_ => 7.0` branch silently assigns a full-furniture
conditioned-space thermal mass multiplier to any new zone type we might
add (basement, crawlspace, conditioned-attic, etc.). Per our
`feedback_no_silent_defaults.md` rule this should be an explicit match or
an error. 7.0 on a crawlspace with no furniture would overstate its mass
by ~7x.

**Action**: enumerate every `ZoneType` variant explicitly in the match and
remove the wildcard. If a new zone type is added in the future, the
compile error forces a deliberate mass-multiplier choice.

### 3e. Garage ACH heuristic

**Location**: garage ACH50/20 default handling in
`crates/hares-io/src/hpxml/` (audit flagged this; the exact line is in the
resolve_* files where garage-zone parameters are filled).

**Issue**: audit found a hard-coded ACH50 → ACH20 heuristic (dividing by
some factor) for garages when HPXML doesn't specify. Not obviously
anchored to a published source. Should be either an ASHRAE-cited default
or an explicit error per our no-silent-defaults rule.

**Action**: locate with
`grep -rn "ACH\|ach50\|ach20" crates/hares-io/src/hpxml/ | grep -i "garage"`,
cite or replace with typed error.

### 3f. Thermostat deadband asymmetry

**Location**: `crates/hares-core/src/actors/ideal_thermostat.rs` and
control dispatch.

**Issue**: audit reported a 0.2 °C asymmetric deadband (heating and cooling
using different tolerances). ANSI/ASHRAE 55-2020 and most stat manufacturer
specs use symmetric ±0.5-1.0 °C. Small difference, but makes hysteresis
behavior asymmetric in ways hard to reason about.

**Action**: verify current values match intent; if asymmetry is accidental,
make symmetric; if intentional, cite the source in a comment.

---

## 4. Stale `#[ignore]` documentation in `crates/hares-io/tests/hpxml_parity.rs`

**Test names**:
- `ac_has_startup_capacity_degradation_default` (line 702)
- `ashp_backup_lockout_temperature_extracted` (line 742)

**Issue**: the test-file comments above each test say
`// Marked #[ignore] because ...` but neither test actually has an
`#[ignore]` attribute. Both tests pass when run. This is stale
documentation from a prior state.

**Physics context (so the cleanup makes sense)**:
- `startup_capacity_degradation`: AHRI cyclic-degradation coefficient
  `C_d`. Captures the 5-7 % capacity loss during the first few minutes of
  an AC compressor cycle (evaporator wet-down, temperature gradient
  settling). For single-speed SEER-13: C_d ≈ 0.2. Needed for accurate
  part-load and short-cycle efficiency. HARES is already extracting
  `startup_cd` and the test passes.
- `backup_heating_lockout_temperature`: outdoor-air temperature below
  which the heat pump compressor shuts off and strip / gas backup carries
  full load. OCHRE field name `hp_min_temp`; HPXML field
  `BackupHeatingSwitchoverTemperature`; HARES field `hp_lockout_temp_c`
  (stored in Celsius). HARES is already doing the F→C conversion and the
  test passes.

**Action**: delete the two `// Marked #[ignore] because ...` comments.
Rename the documentation block above each to describe what the test
verifies (the physics above is a good starting point).

---

## 5. Review pass over the 228-file commit

The fix commit `ba133ca` changed 228 files (+3766 / -1696) across dispatch
ordering, silent-fallback removal, test integrity, AI-slop polish, and
fmt/clippy. No single human reviewed all of it end-to-end — the diff was
assembled from four parallel agent wave outputs plus my cleanup.

**Action**: run a structured review pass before the public PR is opened.
Practical options:
- `code-review` skill (delegates parallel reviewer agents by taxonomy).
- `review-triangulate` skill (2-3 reviewers with dedup).
- Or an in-person pair review of the diff split by crate, focused on:
  - `crates/hares-core/src/dwelling/mod.rs` — dispatch ordering semantics
    changed; stage ordering invariants worth a close read.
  - `crates/hares-io/src/hpxml/*.rs` — every silent unwrap was replaced
    with a typed error; verify callers propagate correctly and the error
    messages are user-actionable.
  - `tests/envelope_oracle.rs`, `tests/freefloat_oracle.rs` — tolerance
    bands were changed; verify every new band has a physics citation.

---

## 6. Python test suite run

`cargo test --workspace` passes, but `pytest` under `tests/python/` was
not run at handoff. The Python tests exercise the PyO3 bindings which the
Rust tests do not cover. Notable ones to verify:
- `test_py_dwelling_integration.py` — round-trip through the Python API.
- `test_py_fleet.py` — fleet binding that was touched by the AI-slop
  TODO(M-2) rewrite.
- `test_ochre_parity.py` — touches OCHRE-ballpark comparisons.
- `test_thermal_trace.py` — uses the observer feature, exercises the
  telemetry that was touched by the dispatch-ordering fix.

**Action**: `uv run maturin develop && uv run pytest`. Investigate any
failures; they will most likely be:
- Fixture files that need Site/Latitude added (we added it to most, but
  there may be Python-only fixtures we missed).
- Telemetry shape changes from the dispatch-ordering fix populating the
  `equipment_telemetry` map.

---

## 7. Two xfails should be "true fails" before stable release

The `#[should_panic]` pattern we used for 900 and 900FF is the Rust xfail
idiom, but it has a failure mode: if the test starts panicking for a
**different** reason than the expected substring, Rust's test harness
still reports PASS as long as _some_ panic matched the substring.

Our substrings are narrow (`metric=annual_heating_load_kwh`,
`metric=min_zone_temp_c`) and the test panic is a single-line format, so
this is unlikely to silently regress. But the correct long-term solution
is to fix the underlying physics so both tests become plain `#[test]`
without the `should_panic` attribute. The fix for 900FF (section 1) will
close both.

**Action**: when 900FF is green, remove both `#[should_panic]` attributes
and the associated xfail comments. Verify all four BESTEST tests
(600, 600FF, 640, 900, 900FF) pass as plain tests.

---

## Reference material

- Repo rules: `/home/rich/.claude/projects/-home-rich-src-HARES/memory/MEMORY.md`
  — in particular `feedback_ashrae_not_ochre.md`, `feedback_no_silent_defaults.md`,
  `feedback_best_physics.md`, `project_thermal_solver_parity.md`.
- OCHRE reference: `vendors/OCHRE/` — treat as ballpark, not oracle.
- ASHRAE 140 BESTEST bands: `tests/bestest/reference_bands.rs` and
  `tests/bestest/cases.rs`.
- Dispatch ordering regression tests:
  `crates/hares-core/tests/dispatch_ordering_regressions.rs`.
- Silent-default regression tests:
  `crates/hares-io/tests/silent_default_regressions.rs`.
