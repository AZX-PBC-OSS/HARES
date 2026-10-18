---
"id": "I-07"
"title": "Fix Issue #32"
"status": "active"
"kind": "fix"
"description": "https://github.com/AZX-PBC-OSS/HARES/issues/32


  Verifying the fix from 2ee83a8 (cross-wired EV observations) on a 45-day
  horizon turned up a residual that the calibration suite's own premise does not
  show, so this may be fixture- or injection-path-specific — recording both
  here.


  What works on the fixed build

  The pack no longer dies: nightly top-ups recover to target (45-day repro, end
  SOC 0.90; pre-fix it ended at 0.07 with the driver dead after ~a week).

  On our study premise (Tarrant County home, EV injected through our
  HPXML-rewrite path), charging is near-nightly: 34 sessions in 45 days, and the
  sessions now TAPER (per-session mean-power/rating: median 0.87, min 0.69 —
  CC-CV, which the pre-fix build did not model; its sessions were flat
  rectangles).

  What looks wrong

  Same repro recipe as #26, but on fixture
  tests/fixtures/resstock/2025.1/bldg0000007 (EV added as raw HPXML text,
  <SystemIdentifier id='ReproEV1'/>, 75 kWh / 11.5 kW / Level 2, Nightly
  override, 45 days, 900 s):


  Five consecutive nights show: plugged in, soc_gate_charging_allowed = 1,
  needed_charge_hours rising 0.75 -> 2.75 -> 5.0 -> 6.75 -> 9.0, SOC falling
  0.83 -> 0.07 — and zero charging in the 22:00-06:00 window. (EV Electric Power
  (kW) is exactly 0.00 on those nights.)

  Charging only starts at ~7% SOC, days 6-7 (38 kWh, then 49 kWh) — the
  range-anxiety backstop, not the window.

  Then it settles: end SOC 0.90, but only 2 charge-days in 45 and
  drive_cancelled = 30 by day 45, i.e. after the backstop rescue the vehicle
  mostly stops driving (trips cancelled) rather than cycling normally.

  The same 45-day recipe on our study premise charges 34 times. The difference
  between the two setups is the home and how the EV reaches the engine (raw
  HPXML text with a fresh SystemIdentifier vs our rewritten HPXML) — so either
  the identity assignment leaves a raw-injected EV driver unbound to something
  (weather/occupancy/miles sampling?) until the backstop bypasses it, or fixture
  0000007 trips an interaction worth knowing about.


  Engine build 129e676c76886682 from 2ee83a86 (+ three additive commits on our
  branch), Linux x86_64, Python 3.13. The day-by-day telemetry table is ready if
  useful; the repro script is the one in #26 with the fixture path swapped."
"created_at": "2026-09-24T16:40:09.263Z"
"created_by": "operator"
regression_tests:
  - crates/hares-core/tests/ev_thermal_regressions.rs
  - crates/hares-equipment/src/ev/tests.rs
  - crates/hares-equipment/src/generator.rs
  - crates/hares-equipment/src/ventilation.rs
  - crates/hares-equipment/src/water_heater/resistance.rs
  - crates/hares-core/tests/equipment_id_regressions.rs
  - tests/python/test_py_config.py
  - tests/python/test_actor_param_boundary.py
  - tests/python/test_solar_override_boundary.py
  - tests/python/test_non_finite_input_boundaries.py
---

https://github.com/AZX-PBC-OSS/HARES/issues/32

Verifying the fix from 2ee83a8 (cross-wired EV observations) on a 45-day horizon turned up a residual that the calibration suite's own premise does not show, so this may be fixture- or injection-path-specific — recording both here.

What works on the fixed build
The pack no longer dies: nightly top-ups recover to target (45-day repro, end SOC 0.90; pre-fix it ended at 0.07 with the driver dead after ~a week).
On our study premise (Tarrant County home, EV injected through our HPXML-rewrite path), charging is near-nightly: 34 sessions in 45 days, and the sessions now TAPER (per-session mean-power/rating: median 0.87, min 0.69 — CC-CV, which the pre-fix build did not model; its sessions were flat rectangles).
What looks wrong
Same repro recipe as #26, but on fixture tests/fixtures/resstock/2025.1/bldg0000007 (EV added as raw HPXML text, <SystemIdentifier id='ReproEV1'/>, 75 kWh / 11.5 kW / Level 2, Nightly override, 45 days, 900 s):

Five consecutive nights show: plugged in, soc_gate_charging_allowed = 1, needed_charge_hours rising 0.75 -> 2.75 -> 5.0 -> 6.75 -> 9.0, SOC falling 0.83 -> 0.07 — and zero charging in the 22:00-06:00 window. (EV Electric Power (kW) is exactly 0.00 on those nights.)
Charging only starts at ~7% SOC, days 6-7 (38 kWh, then 49 kWh) — the range-anxiety backstop, not the window.
Then it settles: end SOC 0.90, but only 2 charge-days in 45 and drive_cancelled = 30 by day 45, i.e. after the backstop rescue the vehicle mostly stops driving (trips cancelled) rather than cycling normally.
The same 45-day recipe on our study premise charges 34 times. The difference between the two setups is the home and how the EV reaches the engine (raw HPXML text with a fresh SystemIdentifier vs our rewritten HPXML) — so either the identity assignment leaves a raw-injected EV driver unbound to something (weather/occupancy/miles sampling?) until the backstop bypasses it, or fixture 0000007 trips an interaction worth knowing about.

Engine build 129e676c76886682 from 2ee83a86 (+ three additive commits on our branch), Linux x86_64, Python 3.13. The day-by-day telemetry table is ready if useful; the repro script is the one in #26 with the fixture path swapped.

## Implementation Notes

Fix landed on the working tree (uncommitted; HEAD 2ee83a86 + this change).
The RCA's D1–D3 and the same-class divergences are fixed at the root: the
EV pack thermal sub-model now shares the stationary Battery's architecture
through one pack-electrical home, with every new constant carrying a named
source.

### What landed

- **D2 (loss attribution)** — pack heating is I²R only, through the new
  shared home `crates/hares-equipment/src/pack_electrical.rs` (quadratic
  terminal-voltage solve, matched-impedance clamp, I²R; OCHRE Battery.py:295
  citation in one place). `Battery::compute_electrical` delegates to it
  (public surface and behavior unchanged, tests green across the move).
  The EV's drive path heats through the same solve (drive energy dispatched
  per step → equivalent discharge current → `pending_drive_kwh` consumed by
  the next step; checkpoint v5).
- **D1 (thermal mass / UA)** — capacity-derived defaults: mass
  `capacity × 6.4 kg/kWh × 1000 J/(kg·K)` (Tesla Model 3 LR pack; Li-ion
  specific-heat band midpoint) → 480 kJ/K at 75 kWh; UA area-scaled from the
  Battery's 5 W/K (`5 × (capacity/13.5)^⅔` → 15.7 W/K at 75 kWh). The
  adopted pair meets the parked-pack two-sided constraint with 3.5 K margin
  (τ ≈ 8.5 h) — no joint re-derivation was needed.
- **D3 (preconditioning)** — 5 kW pack-duty heater (Webasto HVH entry
  class, datasheet-sourced; the plan's merge condition resolved), 5 °C
  threshold (datasheet charge-limit band 0–10 °C midpoint), gated on pack
  temperature alone whenever connected with an energized supply. The heater
  is a pack-side DC load: the charger's raised import covers it while
  connected (the pack nets zero in the heater-only state), the port carries
  the charger's AC input alone in every state, and during V2L/V2G the
  discharge budget covers the total pack-side draw with both stopping at
  the effective floor.
- **Allocation** — supply bound `min(rating, power_limit)` with the
  priority swap exactly at `min_charge_temp_c` (heater first below —
  charging is physically zero there; charging first above), DR as a
  multiplier on the allocated AC total (never a ceiling term; a commanded
  zero zeroes the port), the taper bound landing the pack exactly on
  target with the heater running.
- **Mode classification** — from the pack-side net charge rate with
  `Heating` (preconditioning) and `On` (standby var support) states; guard
  Rule 1 gained the reactive term Rule 2 already counted (an inverter
  supplying vars at zero real power is genuinely active); Rule 4's
  `Bidirectional` exemption verified and untouched (the plan's R12
  withdrawal confirmed).
- **Degradation guard** — `DegradationState::accumulate` → `Result`, error
  bounds [−100, +130] °C, the 0–55 °C validated domain verified against the
  primary source with the sub-zero extrapolation owned and documented
  inline.
- **Ambient-aware init** — the Battery's cascade (config → ambient; the EV
  has no zone); `DEFAULT_BATTERY_TEMP_C` is now only the pre-init
  construction placeholder, making its doc comment true.
- **LUT composition** — a configured LUT's temperature axis is the
  temperature-dependent capability (the linear ramp is not multiplied on
  top); the plating cutoff survives unconditionally.
- **Override loud-error** — `validate_equipment_override_keys` at the
  dwelling assembly boundary: unknown keys (e.g. SystemIdentifiers) and
  keys naming the handled-outside-registry `Occupancy` spec fail the build
  with the key, the matching rule, and the valid names.
- **Python surface** — `PyEv` gains `battery_temp_c`, `min_charge_temp_c`,
  `full_power_temp_c`, `heater_power_w`, `heater_threshold_c`,
  `thermal_mass_j_per_k`, `ua_w_per_k`, `n_series`, `n_parallel`,
  `cell_resistance_ohm` (keyword args, `None` defaults), threaded through
  `ev_config_from_py` and the archetype path.
- **Parity** — EV fixtures carry `heater_power_w: 0` via self-maintaining
  per-fixture plumbing (a config-driven `[ev]` section; detects the EV
  plug load in the building XML), modeling exactly the heaterless vehicle
  the OCHRE reference models. The corpus also gained the **charging-EV
  fixture** (`cz5a_ev_charging`): the EV starts the window discharged and
  both simulators charge it deterministically — OCHRE pinned by a
  single-day event list, HARES pinned by the `[ev]` override (initial
  SOC, warm pack, Immediate strategy) — cross-checking the EV charging
  power OCHRE does model at the default site-energy band.
- **Driver estimate (D4, fixed in-session)** — the needed-hours estimate
  keys on the equipment's own published `CHARGE_DERATE` telemetry when
  observable: zero derate reads as +∞ for the control consumers (the
  anxiety gate and the departure deadline treat ∞ as maximum urgency —
  impossible, never "slower"), a positive derate gives the
  instantaneous-capability bound, and the ambient driving-range curve
  remains only as the no-observation fallback. The telemetry channel
  publishes the ambient fallback while charging is blocked (the telemetry
  layer's finiteness contract cannot carry +∞), with the blocked state
  carried exactly by the equipment's `CHARGE_DERATE` and power columns.

### Measured evidence (post-fix)

- Cold-snap scenario (the ticket's recipe, sustained −7 °C, 45 days):
  **39 of 45 charge-days, 0 cancelled trips** — identical to the healthy
  warm-weather run of the same recipe (pre-fix: 0 charge-days, 34
  cancellations).
- L2 session (0.5 → 0.9 at 11.5 kW, 20 °C start): peak pack temperature
  **20.15 °C** — a 0.15 K I²R rise (pre-fix: 280.9 °C).
- Charging-EV parity fixture (`cz5a_ev_charging`): the EV's 11.5 kW charge
  dominates the 1-hour site-energy integral; measured residual vs the
  OCHRE event-driven reference **1.36 %** against the 25 % default band.
- Fade trajectory (10 days of charge/drive cycling at physical
  temperatures, reference-aligned model): usable capacity starts ≈ +0.9 %
  above nameplate (the min(QLi, Qneg) BOL state — the b0 = 1.07 Li
  intercept capped by the negative-electrode branch), decays smoothly
  with the break-in loss, and stays within the physical band with
  bounded day-over-day change (pre-fix: 75 → 208 → 75 kWh whipsaw). The
  permanent regression asserts the band and the bound.
- Drive-path I²R, measured while re-deriving a test: a 10 kWh burst in
  one 1-minute step is a ~600 kW discharge (~1700 A into the pack) whose
  I²R legitimately warms the pack ~4 K (28.6e6 A²·s × the 11.2 mΩ pack
  resistance ≈ 1.9 MJ into the 480 kJ/K mass) — the drive-path instance of
  the loss-attribution rule, previously invisible because drives touched
  no temperature at all. (The earlier "~6 K" figure here was an arithmetic
  slip, corrected in the review round.)

### Deviations (from the fix plan)

1. **Per-model electrical parameters were not added to the EV catalog.**
   The plan's blast radius listed `ev/catalog.rs` for per-model topology
   values; no sourceable per-model data exists (the physical quantities the
   literature reports are cell-level, shared through the chemistry tables),
   and fabricating datasheet values would violate the citation bar. The
   capacity-derived default (96S, `n_parallel` from the 21700 5 Ah cell
   class) with per-instance config override covers the physics; documented
   in the catalog.
2. **The commanded-setpoint total bound carries a documented exception.**
   Executing the plan's R17 form (the total never exceeds the command)
   defeated the pinned `DeadlineGuarantee` contract (an urgent ready-by
   deadline legitimately raises charge power above a soft setpoint — four
   existing tests pin it). Resolution: the demand function reports whether
   a deadline raise is active; the allocator bounds the total by the
   command except when the raise is active, where the supply-bound priority
   rule governs. Both contracts verified green.
3. **The away arm reports `OperatingMode::Off`.** The plan classified all
   three sites from the pack-side net; executing that for away states
   tripped the mode/flow guard (active mode, zero flows) because the away
   arm's CoreOutput flows are the *dwelling-side* quantity (zero — an
   off-site load must not enter the dwelling electrical summary that feeds
   BMS dispatch). Found by the Python integration gate; resolved with the
   dwelling-side mode for away and a dedicated regression
   (`away_activity_reports_off_mode_consistent_with_zero_dwelling_flows`).
   Away activity remains observable through `AWAY_CHARGE_POWER_KW` and
   `HEATER_POWER_W`.
4. **The diagnostic harness was folded to a tombstone, not deleted.** The
   session's permission layer denied every file-deletion route; the
   `i07_diag.rs` tombstone documents where each contract went (the I-06
   flow recorded the same constraint, and its harness was later removed by
   the operator — the same disposition applies here).
5. **The plan's 13-test exposure list mostly proved inert.** Ambient-aware
   initialization moved the default pack temperature from 20 °C to the
   10 °C test ambient, but the seven LUT tests (Clamp strategy on a
   25 °C-only axis) and the six degradation tests (structural assertions)
   passed unchanged. The six actual reds were the heater-default exposure
   and the battery-temp default, resolved by premise pins and the two
   defective-pin rewrites per the triage rule — no tolerance was loosened.
6. **The degradation model was aligned to the paper's reference
   implementation, beyond the plan's guard-only scope.** The plan's merge
   condition (verify `deg_const` against Smith 2017) was executed against
   both the open-access NREL preprint and NREL's own implementation of the
   paper (SSC `lib_battery_lifetime_nmc.{h,cpp}` — the lineage EnergyPlus
   itself delegates to). The PDF's inner-fraction minus glyphs are
   unmapped in every text extractor, so the paper alone could not settle
   four sign conventions; the reference implementation is decisive. It
   **confirms** α_b1 = −1 (HARES was right; the paper's printed "+1.0"
   line is an artifact) and **corrects** three: `b3_ref` is a positive
   break-in loss (not OCHRE's negated capacity boost), θ = +0.135, and
   b0 = 1.07. The model also gained what HARES (like OCHRE) lacked
   entirely: the negative-electrode site-loss branch (Eq. 8–11) and the
   d0 reference ratio, with usable capacity = min(QLi, Qneg) per Eq. 1.
   Reversing OCHRE's BOL capacity *boost* to the paper's BOL loss also
   reverses the modeled BOL capacity trajectory — every
   degradation-pinned test was re-derived against the reference behavior
   (see the tickets' resolution notes for the full account).
7. **The reversible temperature-capacity scaling is factored to the
   equipment layer, not the fade.** SSC evaluates both branches'
   temperature dependence inside the daily capacity update; HARES's
   stationary Battery already carries the identical NREL d0 Arrhenius as
   its `CapacityDerateModel`, so the shared degradation fade is
   deliberately temperature-independent (both branches' capacity levels
   at their T-reference ratios; the damage paths — c2's Arrhenius, βc2
   weighting — temperature-scaled exactly as SSC scales them), and the
   EV now applies the same derate layer the Battery always had
   (`refresh_usable_capacity`: rated × SOH × derate, per step, published
   self-consistently with the pack temperature). The divergence is
   bounded and reversible: at 15 °C SSC's negative-electrode level scale
   is 0.963 vs the d0 Arrhenius 0.943. The dwelling invariant
   `ev_capacity_degraded` was extended to the full contract
   (capacity = rated·(1−fade)·derate(T)) — its original SOH-only form
   would have rejected the new derate layer as a violation.

### Verification

- `cargo nextest run --workspace` — **6970 passed, 0 failed**, 12 skipped
  (pre-existing documented skips; +1 vs the first pass — the audit's
  Battery LUT composition regression).
- All four reproduction gate commands green:
  `l2_charge_session_keeps_pack_temperature_physical`,
  `battery_heater_warms_pack_while_cold_derate_blocks_charging`,
  `parked_pack_stays_above_charge_cutoff_after_a_cold_night`,
  `nightly_strategy_keeps_ev_charged_through_a_sustained_cold_snap`.
- `cargo clippy --workspace --tests -- -D warnings` — 0 errors.
- `cargo fmt --check --all` — clean.
- OCHRE parity layer: the constitution's literal filter
  (`cargo nextest run -p hares-core ochre_parity`) matches no test by name
  — the layer is the `parity` tests (`parity_outputs_against_reference_corpus`
  + matchers), run green: 8/8.
- BESTEST 26/26; envelope conservation 9/9.
- Python suite (this change touches `hares-python`, so the I-06 lesson's
  gate applies): `uv run maturin develop` + `uv run pytest tests/python/` —
  **1172 passed, 12 skipped, 0 failed** (at `-n 4`). Under the default
  `-n auto` (28 workers) the wall-clock-sensitive HELICS federate tests
  flake non-deterministically — different tests per run, all passing
  serially and at `-n 4`; those fixtures contain no EV and no overrides,
  so the flakiness is machine-load sensitivity, not this change (recorded
  for the flow owner; reproducing it on the pre-change build was attempted
  but the scratch worktree's environment lacks the optional `helics`
  package).

### Known Limitations

The three items the fix plan scoped as tracked deferrals (the driver
estimate's cold-charge blindness, the charging-EV parity fixture, and the
degradation model's divergence from Smith 2017's printed values) were all
**resolved in-session** under the operator's no-deferral directive — their
tickets carry `status: done` with full resolution notes, and the deviations
section records how. What remains below are genuine modeling boundaries of
working code.

- **What:** The Bernardi reversible-entropic term (`I·T·dU/dT`) is omitted
  from the cell heat balance.
  **Constraint:** the in-tree OCV tables carry no entropy (dU/dT) data;
  the term is ~1.7 W against ~10 W ohmic at Level 2 and ~0.15 W at the
  cold trickle, both dwarfed by ~110 W of UA loss and the kilowatt-class
  heater in the regime this model matters for (arithmetic documented at
  `apply_soc_and_thermal`).
  **Tried:** none — the omission is a documented modeling boundary with
  its magnitude stated, not a silently-held approximation.
  **Resolvable when:** per-chemistry entropy tables are added to the OCV
  tables.
- **What:** Road-speed convective cooling during the drive is not modeled
  (the pack relaxes to ambient at the parked-regime UA while driving).
  **Constraint:** a drive-thermal airflow model is a separate sub-model;
  the omission is conservative for the cold-charge contract (it can only
  overstate how cold the pack arrives at the plug-in).
  **Tried:** none — the boundary is deliberate (plan scope note).
  **Resolvable when:** a drive thermal model with speed-dependent film
  coefficient is wanted.
- **What:** The heater is modeled as constant-power at its rating; real
  PTC elements self-regulate (~12 % lower average draw).
  **Constraint:** PTC power curves are element-specific and not generically
  sourceable; the error direction is known and conservative for the
  grid-load studies the billed draw feeds.
  **Tried:** none — a declared simplification with the error direction
  stated (plan W1), documented in `DEFAULT_HEATER_POWER_W`.
  **Resolvable when:** element-specific PTC curves become sourceable.
- **What:** The Smith 2017 fit is extrapolated below its 0 °C validated
  floor during parked/driving/unheated cold hours.
  **Constraint:** HARES must simulate cold climates — erroring there would
  refuse the exact regime this initiative models; no sub-zero-calibrated
  in-tree data source exists (a degradation-research dependency).
  **Tried:** preconditioning moves plugged-in packs into the validated
  domain for the charging hours that dominate the aging arithmetic; the
  extrapolation factors (≈8× cycle rate at −7 °C) are documented at the
  guard.
  **Resolvable when:** a sub-zero-calibrated fit exists.
- **What:** The EV's site-energy parity is pinned to a heaterless,
  warm-packed vehicle; OCHRE cannot cross-check the EV *thermal* sub-model
  at all.
  **Constraint:** OCHRE's EV model has no pack thermal state (no
  temperature, no heater, no derate) — the sub-model exceeds the floor,
  and the floor cannot check the excess. Regenerating any reference with
  thermal behavior is impossible (there is nothing to generate it from).
  **Tried:** the thermal sub-model's coverage lives in
  `ev_thermal_regressions.rs` (four reproduction contracts plus the
  mechanism coverage); the charging *power* cross-check the floor can
  make now exists (`cz5a_ev_charging`, 1.36 % residual at the default
  band).
  **Resolvable when:** never — fundamental (the reference lacks the
  physics); the in-repo regression suite is the standing substitute.
- ~~**What:** `crates/hares-core/tests/i07_diag.rs` remains as a tombstone
  rather than being deleted.~~ **Resolved** (red-team round 4): the
  operator removed the file; the contracts it pointed to live in
  `crates/hares-core/tests/ev_thermal_regressions.rs`. The constraint
  record (two `rm` denials by the session permission layer, the tombstone
  fold per the I-06 precedent) is kept in the round notes below.

### Tickets (all resolved in-session — full resolution notes in each)

- `tickets/I-07-T1-driver-derate-blindness.md` — done: the estimate keys on
  the equipment's published charge derate; +∞ for control, ambient
  fallback for the telemetry channel.
- `tickets/I-07-T2-charging-ev-parity-fixture.md` — done: the
  `cz5a_ev_charging` fixture with its OCHRE event-list pin and HARES
  `[ev]` override; site-energy residual 1.36 % at the default band.
- `tickets/I-07-T3-smith-2017-sign-conventions.md` — done: constants
  aligned to the SSC reference implementation (α_b1 confirmed, b3_ref/θ/b0
  corrected, negative-electrode branch and d0 added, min(QLi, Qneg)).

### Fix's own final audit (pre-done interrogation)

**Overreach.** Walked the diff hunk by hunk; every production hunk names
its finding: the pack-electrical home and `compute_electrical`
delegation answer D2 (the plan's DRY commitment forced the move); the
ev/config defaults answer D1; the allocator/heater/mode/vars hunks answer
D3 + the plan's class-audit rows; the ev_driver hunks answer D4;
`invariants.rs` was extended (not loosened — it now checks the derate
too, which the SOH-only form would have rejected as a violation); the
override validation answers the plan's silent-no-op scope item; the
Battery `clamp_power` composition answers the class rule "fix the class,
not the instance" — a class-member the plan's audit row had enumerated
only in the EV, found by this audit pass, fixed with its own regression
(`lut_temperature_axis_is_the_capability_not_multiplied_by_derate`).
Four hunks answer operator directives rather than plan findings (the
Smith alignment, the D4 estimate, the charging-EV fixture — the
no-deferral directive — and the `evaluate` visibility widening forced by
the invariant checker consuming the same derate model the equipment
applies); each is documented above (Deviations 6–7, the Tickets section,
dl-i07-fix-01..03), so the next reviewer can tell them from the RCA
findings. Adjacent artifacts updated for the same claims:
`docs/equipment/ev.md`, `docs/equipment/battery.md` (its degradation
section still described the pre-alignment constants — corrected),
`reproduction.md` post-fix addendum, `rca.md` addendum, `context.md`.

**Shortfall.** Defect origins and change sites coincide for every
finding (D1: `ev/config.rs` constants; D2: `apply_soc_and_thermal`'s
heat term; D3: `run_charging_physics`' gate; D4:
`needed_charge_hours_to_target`; D5: `accumulate`). Class searches run
anew (not recalled): conversion-loss-as-heat greps across
`hares-equipment` outside `ev/` — the only efficiency-adjacent hits are
pump-power draws and the shared-solve `ohmic_loss_w` (correct); heater
gates — the Battery's is quoted temperature-only + bus supply (the EV
was the divergence); LUT × derate — the Battery sibling found and fixed
(this pass); override no-ops — the validation covers all keys at the
population boundary. One loosened comparison in the whole diff, named:
the whipsaw day-over-day bound 5 % → 11 %, a bound authored in this same
session, re-derived because the aligned model added the reversible
derate span (≈8 % across 0–25 °C) plus the break-in first-day step
(≤2.8 %); the guarded pathology (±170 % day whipsaw) remains ~15×
outside the bound; the arithmetic is inline in the test. No `#[allow]`,
no `#[ignore]`, no catch-and-swallow, no widened types, no
commented-out code. Every existing-test edit is a premise pin (the
plan's triage rule: "pinning battery_temp_c explicitly when the test's
subject is not temperature") or an exact re-derivation — tolerances
preserved at 1e-9/1e-6 throughout. Red-if-reverted coverage, by defect:
D1 `parked_pack_stays_above_charge_cutoff_after_a_cold_night`; D2
`l2_charge_session_keeps_pack_temperature_physical`; D3
`battery_heater_warms_pack_while_cold_derate_blocks_charging`;
integrated `nightly_strategy_keeps_ev_charged_through_a_sustained_cold_snap`;
D4 `needed_charge_hours_keys_on_equipment_charge_derate` +
`needed_charge_hours_reports_impossible_when_charge_is_physically_blocked`;
D5 `accumulate_rejects_cell_temperatures_outside_the_physical_envelope`;
Smith alignment `degradation_accumulate_multi_day` +
`break_in_loss_q_li3_reaches_equilibrium` +
`degradation_break_in_loss_and_bol_capacity_follow_reference_model`; EV
LUT composition `lut_temperature_axis_is_the_capability_not_the_linear_ramp`;
Battery LUT composition
`lut_temperature_axis_is_the_capability_not_multiplied_by_derate`;
override `override_keyed_by_unknown_equipment_name_errors_loudly`; mode
`preconditioning_pack_reports_heating_mode` +
`commanded_vars_at_zero_real_power_report_on_mode` +
`away_activity_reports_off_mode_consistent_with_zero_dwelling_flows`;
allocation `cold_charge_session_allocates_supply_bound_with_charge_priority`
+ `dr_scales_the_allocated_total_as_a_multiplier` +
`charging_with_heater_lands_exactly_on_target` +
`commanded_power_setpoint_bounds_total_draw_with_heater_running`; V2L
`cold_v2l_export_conserves_pack_energy_and_stops_at_floor`; whipsaw
`usable_capacity_stays_physical_through_daily_cycling`; charging
parity `parity_outputs_against_reference_corpus` (the new fixture's
site-energy check at the default band).

**Abdication.** No finding's trace is prose only — every item above has
a code change and a test. The remaining Known Limitations each name a
concrete constraint and what was tried: the Bernardi entropic term
(in-tree OCV tables carry no dU/dT data — nothing to implement from);
road-speed convective cooling (conservative omission for the
contract); constant-power heater (element-specific PTC curves not
generically sourceable; error direction known); sub-zero degradation
extrapolation (no sub-zero-calibrated data exists; HARES must simulate
cold climates); OCHRE thermal parity (OCHRE's EV model has no pack
thermal state — the floor cannot check physics it does not have);
the i07_diag tombstone (`rm` denied by the session's permission layer;
the write tool cannot delete files — the disposition follows the I-06
precedent, whose harness the operator later removed).

### Coverage-triage routing (fixed on return)

The coverage phase's note flagged in prose that `EvConfig::validate`
left the four temperature fields (`battery_temp_c`, `min_charge_temp_c`,
`full_power_temp_c`, `heater_threshold_c`) without finiteness rules and
accepted an inverted `min_charge_temp_c > full_power_temp_c` pair;
triage escalated it to a proven defect with a failing gate test
(`ev_config_validate_rejects_non_finite_temperature_fields`, kept
verbatim — the fix answers the test, never edits it). **Fixed at the
cause:** `ev/config.rs` `validate()` now rejects non-finite values for
all four fields (a NaN temperature falls through `linear_temp_derate`'s
comparison branches into the interpolation, silently poisoning charging
power, SOC, and every downstream energy total) and rejects the inverted
ramp (the `temp >= temp_max` branch would win everywhere, silently
charging at full power across the derate band); finite extremes and
equal (step-cutoff) pairs remain legal, with the reasoning inline.

**Class extension:** the stationary Battery's `validate()`
(`battery/config.rs`) carried the identical gap — five temperature
fields (`initial_cell_temp_c`, `min_discharge_temp_c`,
`full_power_temp_c`, `min_charge_temp_c`, `heater_threshold_c`)
unguarded, and both derate ramps (charge and discharge) without
ordering checks; a NaN `min_charge_temp_c` additionally disables
`charge_allowed`'s plating-cutoff guard forever (NaN `>=` compares
false). Fixed in the same style with its own regression gate
(`battery_config_validate_rejects_non_finite_or_inverted_temperature_fields`).
This completes the validation class the fix had half-covered: the six
fields the fix added (mass, UA, heater power, topology, resistance) were
checked while the temperature fields — the same class, in the same
function — were not.

Triage's two dismissed items (discharge-leg DR × heater placement;
degradation-guard bound edges) stand as dismissed — both re-verified
against the code during the fix pass, both pinned by existing tests the
dismissals name.

**Post-fix audit of the routing pass (pre-done interrogation):** the
routed finding's defect originates and was changed in the same function
(`EvConfig::validate`, `ev/config.rs`) — no displacement. The
next-door sweep for the same mistake, run by enumerating **every
`validate()` in `hares-equipment` carrying temperature-typed fields**
(12 files inspected; the water-heater configs guard theirs via
`check_finite`, the dehumidifier guards `min/max_operating_temp_c`),
found **two further unguarded instances**, both fixed in each file's
own style with regression gates:

- `ventilation.rs` `defrost_temp_c` — a NaN makes
  `compute_defrost_fraction`'s `deficit <= 0.0` guard compare false
  forever, flowing the NaN into the defrost-time interpolation and
  silently poisoning ventilation power/energy. Gate:
  `ventilation_config_validate_rejects_non_finite_defrost_temp`.
- `heat_pump_config.rs` `enter_water_temp_c` (both the heater and cooler
  validates) — a NaN becomes `SourceTemperature::Constant(NaN)` and
  poisons the WSHP COP. Gate:
  `heat_pump_configs_reject_non_finite_enter_water_temp`.

The ordering half of the class was swept too: `linear_temp_derate` has
exactly two production callers (Battery, EV — both now guarded), so
the class is closed. The triage's gate test passed **unedited** — the
fix answers it, never edits it. No suppression, skipped test, broadened
catch, widened type, weakened assertion, or loosened comparison exists
anywhere in the routing-pass diff: every change is an added check or an
added test. Red-if-reverted map for this pass: EV — the triage's
`ev_config_validate_rejects_non_finite_temperature_fields`; Battery —
`battery_config_validate_rejects_non_finite_or_inverted_temperature_fields`;
ventilation — `ventilation_config_validate_rejects_non_finite_defrost_temp`;
heat pump — `heat_pump_configs_reject_non_finite_enter_water_temp`.
Overreach check: the Battery/ventilation/heat-pump extensions are the
routed finding's own class (the triage itself framed the EV gap as "the
unguarded remainder of the same class" — "the class was half-fixed"),
mandated by the constitution's class rule, not adjacent improvements.
Final verification: workspace **6982 passed / 0 failed**, clippy 0
errors, fmt clean, parity 8/8, BESTEST 26/26, conservation 9/9, the
four reproduction gates green, Python suite 1172 passed / 0 failed.

### Red-team round (all three findings fixed at the cause)

- **F1 — var-support checkpoint restore violated the mode/flow guard.**
  Fixed in `ev/mod.rs` `load_state`: the reactive flow is a pure function
  of the restored latched control state (`q_setpoint_kvar` /
  `power_factor`) and the restored port power, so the restore now
  recomputes it (`compute_reactive_kvar`) instead of zeroing it — the
  restored output re-pairs the checkpointed `On` mode with its commanded
  flow, and the rebuilt `CoreOutput` publishes that flow. The red team's
  gate test
  (`var_support_checkpoint_restores_an_output_the_mode_flow_guard_accepts`)
  passes **unedited**. **Class sibling found and fixed:** the Battery's
  `load_state` restores the checkpointed mode while publishing zero
  electric flow — a mid-charge checkpoint restored `Charging` with
  `Bidirectional(0.0)`, the same Rule 1 violation (proven red by a new
  probe-turned-gate test before fixing). The Battery cannot use the EV's
  face (its checkpoint carries no power — self-consumption follows the
  live net load the restore has not yet seen), so the fix is the
  zero-flow truth: an active power mode at restore becomes `Standby`
  (its own idle state); the next step re-derives the mode from actual
  power. Gate:
  `charging_checkpoint_restores_an_output_the_mode_flow_guard_accepts`
  (battery/mod.rs).
- **F2 — non-object `overrides` payload bypassed the loud-error
  validator.** Fixed in `dwelling/conversions.rs`
  `validate_equipment_override_keys`: any non-object payload (string,
  array, number — reachable from the Python surface, which converts any
  Python object into the field) now fails the build with a typed error
  naming the payload — the same rule the `zip` override channel already
  enforces. `overrides: None` remains the canonical no-overrides (upstream
  wraps it into an empty object); `Some(Null)` is rejected with the rest
  (a misconfiguration of the same class). Gate:
  `non_object_overrides_payload_fails_the_build_loudly`, passes unedited.
  **Class swept:** `patches` is a typed struct (serde rejects malformed
  shapes at its own boundary); `overrides` was the only free-form
  `Value` channel into the dwelling build.
- **F3 — the LUT c-rate applied the degradation SOH twice.** Fixed in
  `ev/mod.rs` `compute_charging_power_kw`'s LUT branch:
  `battery_capacity_kwh` is already the SOH- and temperature-scaled usable
  capacity (`refresh_usable_capacity`), so the c-rate now divides by it
  directly — the extra `· soh` computed the c-rate over the SOH-squared
  capacity (0.148 vs 0.133 at 10 % fade), skewing every configured LUT's
  c-rate axis. The Battery's LUT branch verified clean in the same sweep
  (`capacity_kwh_nominal` is SOH-scaled once, never double-multiplied);
  no other `· soh` consumer exists (grep-verified). The red team's gate
  test passes with one **precondition arithmetic correction, documented
  openly**: the precondition hardcodes the 25 °C step-start temperature
  for a post-step capacity reading, which only held pre-fix because the
  defect zeroed the charge power (no current → no I²R heat → the
  temperature never moved). Under the correct fix the session's own I²R
  heat warms the pack ~0.012 K, and the post-step refresh — required for
  the published (capacity, temperature) pair to satisfy the dwelling
  invariant — legitimately reports `derate(25.012)`. The corrected
  precondition derives the expected capacity from the pack's actual
  post-step temperature (published in the same telemetry) at the same
  1e-9 tolerance, preserving its stated purpose (the injected fade must
  be in effect); the main contract assertion is untouched and passes.

Red-team items assessed and dismissed by the red team itself (allocator
algebra, discharge floor, guard reachability, cast saturation,
classification epsilons, the D4 fallback, the other EV restore faces)
were independently re-verified during this fix pass and stand as
dismissed with the red team's stated reasons. Final verification for
this round: workspace **6987 passed / 0 failed** (the three red-team
gates + the Battery class gate all green), clippy 0 errors, fmt clean,
parity 8/8, BESTEST 26/26, conservation 9/9, reproduction gates green,
Python suite **1173** passed / 0 failed.

**Post-fix audit of the red-team round (pre-done interrogation):** the
audit itself caught a regression the F2 fix had introduced — the Python
surface converts `overrides=None` (the idiomatic "not provided") into
`Some(Value::Null)` (`py_dwelling.rs` `python_to_json_value`), which the
new non-object rejection refused — **proven by execution** (a live
`Dwelling.from_hpxml(..., overrides=None)` failed the build) before any
fix. Fixed at the boundary that owns the translation: `py_dwelling.rs`
now maps a converted `null` to `Option::None` (the Rust contract's
canonical absent), so Python `None` builds cleanly while a bare
string/array/number payload still fails loudly, and a `null` nested
inside a provided dict keeps its per-key meaning. Guarded by a permanent
Python test
(`test_overrides_none_builds_and_non_object_overrides_raise`), which
pins both faces of the contract. The raw Rust `Some(Value::Null)`
remains rejected (its canonical absent is `None`; `Some(Null)` is a
misconfiguration). The F3 gate test's precondition correction (documented
in the F3 entry above) was re-examined and stands: the precondition
hardcoded the 25 °C step-start temperature for a post-step reading,
which only held pre-fix because the defect zeroed the charge power (no
current → no I²R heat); under the correct fix the session's ~4.7 W of
I²R heat warms the pack ~0.012 K over the 900 s step (hand-verified:
4.7 W × 900 s ÷ 384 kJ/K), and the published (capacity, temperature)
pair must stay same-instant for the dwelling invariant — the corrected
precondition derives its expectation from the actual post-step
temperature at the same 1e-9 tolerance, with the main contract assertion
untouched. The transient `hares-envelope` workspace failure observed
in one run is the other process's staged in-progress file
(`radiant_gain_weight_distribution_zero_allocations` passes in
isolation and on re-run; not this change's code).

### Coverage-triage round 3: the checkpoint-restore contract class (15 equipment, fixed)

The round-3 triage found the F1 restore contract broken across **15
equipment** — far wider than its note's count of 3 — in two shapes:

- **Shape A (persisted mode contradicts restored flows):** the
  **Generator** (the note's finding — a checkpoint saved in the
  zeroing-setpoint window restores `Off` alongside `Generation(5 kW)` +
  fuel; Rule 2) and **Ventilation** (an `On` checkpoint restored with
  zeroed flows; Rule 1 — missed by the note's sweep).
- **Shape B (defaulted output against declared capabilities):** 13
  equipment whose `load_state` published `CoreOutput::default()` — every
  presence-required field `None` — enumerated by the triage as
  pv, heat_pump_wh, tankless, gas, indirect_tank, resistance, event_load,
  ideal_hvac, boiler, furnace, baseboard, dehumidifier, air_conditioner
  (plus the GSHP/WSHP cooler wrappers and the heat-pump heater, whose
  restores delegate or rebuild).

**Fixed per shape at the restore, which is where the violated contract
lives:**

- **Generator** (`generator.rs` `load_state`): the restored mode is
  re-derived from the restored output power by `update_control`'s own
  rule (`> IDLE_KW_THRESHOLD → Standby else Off`) — the checkpointed
  mode can be a provisional value derived from a pending setpoint while
  power was still flowing, and re-deriving keeps the published (mode,
  flows) pair guard-valid for every checkpoint window.
- **Ventilation** (`ventilation.rs` `load_state`): the step's own
  not-running branch maps `mode = mode.resolve_idle(false, None)`
  (active → `Standby`); the restore applies the same mapping, so
  `update_control` (which reports `self.mode`) and the published output
  agree, and the next step re-derives from ambient.
- **The 13 Shape-B restorers:** each now rebuilds its output at restore
  mirroring its own step's literal — from the checkpointed faces where
  they exist (electric/thermal/fuel/speed/mode/setpoint, per file),
  with the mode re-derived by the step's own rule from the restored
  flow where the mode itself is not checkpointed (water heaters:
  dispatch booleans + `resolve_idle`; ideal HVAC: capacity sign;
  dehumidifier: zero-flow truth `Off`, its checkpoint carrying no
  power), reactive ZIP publishing as 0 where not checkpointed, and COP
  publishing as the guard-allowed not-yet-recomputed 0 where not
  checkpointed. PV and event_load publish the zero-flow truth (their
  power depends on live irradiance/schedule/RNG the checkpoint does not
  carry). The GSHP/WSHP coolers additionally rebuild their own outer
  `core_output` field after the inner restore — their `load_state`
  delegated to the inner AirConditioner while the unit publishes its
  own field (the pump-folded one).

**Gates:** the triage's three failing tests pass **unedited**
(`zeroing_setpoint_window…` Generator/Rule 2,
`running_ventilation…` Ventilation/Rule 1,
`restored_checkpoint_output_satisfies_the_core_contract` resistance
WH/presence), and **sibling gates were added per equipment as each fix
landed** (the F1 precedent): gas, tankless, heat_pump_wh,
event_load, baseboard, dehumidifier, ideal_hvac, pv, boiler (both
variants), furnace (both variants), air_conditioner, the GSHP cooler's
outer-field rebuild, the WSHP cooler (its own gate, through the WSHP
config-conversion construction path), and the ASHP heat-pump heater —
each a permanent
`restored_checkpoint_output_satisfies_the_core_contract` round-trip
through the real save/load path asserting
`validate_core_contract` on the restored output. The Battery's sibling
gate from the red-team round already stands. **Post-fix audit of this
round caught and fixed three face-consistency defects in the initial
fix, all found by reading the diffs against the gates before the
signal:** (1) the dehumidifier's restore published a hardcoded `Off`
while `self.operating_mode` (which `update_control` reports) held the
`is_on`-derived `Cooling` — the two-faces inconsistency the red team's
F1 itself flagged; fixed by applying the same `resolve_idle(false,
None)` mapping the ventilation restore uses, so both faces hold the
zero-flow truth; (2) the ideal HVAC's restore derived its mode from
the capacity sign alone (`0 → Off`) while the step's own mapping
resolves an active thermostat mode with zero capacity to `Standby`
(and `update_control` reports the FSM mode) — fixed by mirroring the
step's mode×capacity match arms; (3) the air conditioner's restored
`sensible/latent_cooling_w` published the checkpointed magnitudes with
the wrong sign (the step negates them; the guard's sign-convention
rule caught it via the sibling gate going red) — fixed by mirroring
the step's negation.

Final verification for this round: workspace **7005 passed / 0
failed** (the three triage gates + 14 sibling gates),
clippy 0 errors, fmt clean, parity 8/8, BESTEST 26/26, conservation
9/9, reproduction gates green, Python suite 1173 passed / 0 failed.

### Red-team round 2 (F4 fixed at the cause)

The second red-team pass found one defect **in the F1 fix itself**:
the restore's reactive recompute used `active_power_kw` — the
heater-inclusive **port total** — as the power basis, while the step
keys the var computation on the **inverter leg** (the charge/export
conversion only; the DC-fed pack heater produces no vars and consumes
no kVA headroom). With a power-factor baseline configured, the
restore's wrong basis published **2.69 kVAR of vars from a resistive
load** in idle preconditioning (where the step publishes 0), and
**clamped a commanded 6.5 kVAR to zero** at a bound-binding session
(where the step served it in full) — both guard-valid pairs, so
nothing caught the silent infidelity. **Fixed with the water heaters'
lossless pattern the red team named as the correct mechanism:** the
step-published `reactive_power_kvar` is now checkpointed (v6) and
restored verbatim, because the inverter-leg basis is not derivable at
restore — the heater draw that splits the port total into leg +
heater AC-equivalent is transient step state, zeroed by the restore.
Both red-team gates pass **unedited**
(`heater_active_checkpoint_restores_the_reactive_flow_the_step_published`,
`commanded_vars_served_at_a_bound_binding_checkpoint_survive_restore`),
and the F1 gate
(`var_support_checkpoint_restores_an_output_the_mode_flow_guard_accepts`)
plus the checkpoint round-trip regressions stay green. The red team's
named unification candidate (the furnace/battery restores' lossy
`q = 0` placeholder — guard-valid, documented, no fidelity claim) was
assessed and stands as documented, not a defect.

Final verification for this round: workspace **7008 passed / 0
failed** (the two F4 gates + the full suite), clippy 0 errors, fmt
clean, parity 8/8, BESTEST 26/26, conservation 9/9, reproduction
gates green, Python suite 1173 passed / 0 failed.

### Red-team round 3 (F5 fixed with the same lossless pattern)

The third red-team pass found the F4 fix **correct for the reactive
column but incomplete**: the same restore block still zeroed
`heater_draw_w` while restoring the heater-funded `active_power_kw`,
the `Heating` mode, and the verbatim reactive — publishing a state no
live step can produce (`classify_mode` yields `Heating` only with a
nonzero draw; the billing rule decomposes every live step's port into
charge leg + heater AC-equivalent, but the restored triple attributed
the heater's 5.56 kW AC-equivalent to nothing). The v6 fix's own
rationale — the step-published value's basis is transient step state,
not derivable at restore — applies identically to the heater draw.
**Fixed with the same lossless pattern the red team named:**
`heater_draw_w` is checkpointed (v7) and restored verbatim, with
`heater_active` derived from the restored draw (draw > 0) — the
pairing the step's own value-carrying telemetry contract makes
canonical. The red team's gate passes **unedited**
(`heater_active_checkpoint_restores_the_heater_draw_the_step_published`),
and all four prior gates (F1 var-support, F3 LUT, F4's two reactive
faces) stay green. The red team's named sibling columns
(`v2l_power_kw`, `away_charge_actual_kw` — reconcilable through the
canonical electric flow and the away arm's documented zero-port
semantics) were assessed and stand as documented unification
candidates, not defects.

Final verification for this round: workspace **7009 passed / 0
failed** (the F5 gate + the full suite), clippy 0 errors, fmt clean,
parity 8/8, BESTEST 26/26, conservation 9/9, reproduction gates
green, Python suite 1173 passed / 0 failed.

**Post-fix audit of the F5 round (pre-done interrogation):** the
red team's two named sibling columns were re-examined against the
same fidelity family rather than inherited as held. **`v2l_active` /
`v2l_power_kw` — fixed:** the red team's own qualifier
("reconcilable at restore through the canonical electric flow") concedes
the information is recoverable, and the transient was real:
`island_source_available` reads `v2l_active`, so a checkpoint-restore
mid-discharge reported no island source for the restore-to-first-step
window — the restored-state-disagrees-with-saved-state family F5
itself pinned. The fix is lossless derivation from
already-checkpointed state (no schema change):
`v2l_active ⟺ last_mode == Discharging` (classify_mode produces
`Discharging` only from a negative pack net rate, and the export is
the only negative path) and `v2l_power_kw = −active_power_kw` in
that state (the port carries the export alone). **
`away_charge_actual_kw` — stands as held:** it is a genuine step
outcome (recomputed by the away arm's allocation each away step
from `power.charge_leg_ac_kw`), not derivable from checkpointed state
without re-running the allocation; its zeroed restore is the
documented lossy placeholder of the same standing as the
furnace/battery `q = 0` — self-healing on the next away step, no
published-flow infidelity.

Final verification for this round: workspace **7009 passed / 0
failed**, clippy 0 errors, fmt clean, parity 8/8, BESTEST 26/26,
conservation 9/9, reproduction gates green, Python suite 1173
passed / 0 failed.

### Coverage-triage round 6 (F6: floor-held discharge — v2l verbatim, v8)

The coverage triage's reassessment probe found the v2l restore
**derivation unfaithful in one corner both the red team and the fix's
own audit missed**: the step keys `v2l_active` on dispatch **presence**
(`is_discharge` — a negative setpoint latched with V2L/V2G enabled),
not on the computed export, so at the reserve floor (and under an
export DR that zeroes the export) a dispatched step publishes
`v2l_active = true` with zero export and mode `Off` (zero pack net
rate). The v7-era `last_mode == Discharging` derivation keyed on the
mode and flipped false exactly there — diverging from the saved state
where `island_source_available` (the dwelling's islanding reads it)
needed the dispatch-active answer. **Fixed with the established
verbatim pattern:** `v2l_active`/`v2l_power_kw` are checkpointed (v8)
and restored verbatim — no checkpointed basis reproduces them in that
corner (the mode is `Off`, the port sign cannot distinguish a
dispatched zero export from an idle connection). The triage's gate
passes **unedited**
(`floor_held_discharge_checkpoint_restores_the_published_v2l_state`),
and all five prior gates stay green. The triage's dismissed items
(`away_charge_actual_kw` lossy placeholder; furnace/battery `q = 0`;
the carried dismissals) hold with their reasons on record.

Final verification for this round: workspace **7011 passed / 0
failed**, clippy 0 errors, fmt clean, parity 8/8, BESTEST 26/26,
conservation 9/9, reproduction gates green, Python suite 1173
passed / 0 failed.

### Red-team round 4 (F6: away intake — the family's last member, v9)

The fourth red-team pass found the v7/v8 verbatim sweep **one
member short**: `away_charge_actual_kw` — published as
`AWAY_CHARGE_POWER_KW`, the documented way away charging is
observed ("the actual intake after taper, never the rating") —
meets the same underivable-basis criterion (the away state
publishes zero port power, so no checkpointed column reconstructs
the away charge leg) yet was left at its init zero by the restore,
misreporting an active away session as drawing nothing. The
asymmetry was sharp: `v2l_power_kw` (the discharge-side actual)
was checkpointed; the charge-side away actual was not. Previous
dispositions of this column as a "documented lossy placeholder"
are superseded by the red team's proof that it is a **published
transfer observable**, not an internal diagnostic. **Fixed with
the established verbatim pattern:** the field is checkpointed
(v9) and restored verbatim. The red team's gate passes
**unedited**
(`away_charging_checkpoint_restores_the_intake_the_step_published`),
and the red team's held items stand: the presence-keyed
`v2l_active` semantics, the restored pairs' guard-validity in the
new corners, the `heater_active` derivation, and the v8 loud
version-mismatch behavior (v9 follows it). The zeroed diagnostics
(`ohmic_loss_w`, `pack_net_charge_kw`) stand on the red team's
own line — internal per-step physical diagnostics, recomputed
next step — which the red team explicitly affirmed as
"defensible"; `away_charge_actual_kw` was on the wrong side of
that line, which is what made it the finding.

Final verification for this round: workspace **7012 passed / 0
failed**, clippy 0 errors, fmt clean, parity 8/8, BESTEST 26/26,
conservation 9/9, reproduction gates green, Python suite 1173
passed / 0 failed.

### Review round (three warnings in the new machinery — fixed at the cause)

The review (`review.md`, 2026-09-25) confirmed the ticket result
(D1–D3 solved, gates green) and raised three warnings in the machinery
this initiative introduced. The review's line numbers pointed at a
partially-applied edit state — the review-round fix edits had been
started but never verified (the session died mid-work) — so this round
re-derived each warning against the actual tree, completed the fixes,
repaired the in-flight compile breaks, and pinned every contract:

- **W1 (heater current counted as cell heating while netted out of
  SOC)** — completed as found: `apply_soc_and_thermal` feeds the shared
  I²R solve the cells' net DC power alone on the charging arm
  (`net_kw`, exactly what moves SOC); the heater's draw reaches the
  pack only through the thermal equation. The discharge arm keeps the
  heater in the ohmic basis — correct, the pack is the source there
  (export + heater both cross the terminals). The stale test that had
  pinned the contradiction as intent
  (`heater_only_grid_draw_when_fully_cold_derated`, "the heater current's
  own I²R … it is a terminal current too") was rewritten to the
  corrected contract: heater-only state ⇒ SOC unchanged, port =
  heater_dc/η, **`ohmic_loss_w` exactly 0** — a nonzero value there is
  the D2 class returning. Class sweep: the stationary Battery's heater
  is an AC standby load that never enters its ohmic solve or its SOC —
  no sibling instance; `OHMIC_LOSS_W`'s description now matches the
  code (cell-resistance I²R only).
- **W2 (LUT c-rate divisor folded the reversible temperature derate
  into a dimension the LUT's temperature axis already encodes, and
  diverged from the Battery sibling)** — completed as found: one shared
  home (`pack_electrical::charging_lut_c_rate`), both siblings passing
  their degradation-adjusted, temperature-independent rating (the EV
  via the new `degradation_adjusted_capacity_kwh()` = rated × SOH; the
  Battery via `capacity_kwh_nominal`, the same semantics OCHRE
  `Battery.py:438` gives its nominal — degradation-adjusted,
  temperature-independent — so the convention is OCHRE-anchored, not
  invented). Two repairs were needed to make the inherited edit
  compile/work: the unresolved bare path (`pack_electrical::` →
  `crate::pack_electrical::`) and a borrow conflict (the divisor
  hoisted above the LUT's `as_mut`). Pinned twice over:
  `lut_c_rate_divisor_is_temperature_independent` (EV: 5 °C pack,
  c-rate 0.12 mid-band vs the bug's ≈0.145 zero-band — discriminating
  bands, no grid boundary), and
  `lut_c_rate_divisor_is_the_soh_adjusted_temperature_independent_nominal`
  (Battery sibling: pins the divisor rule from the other side so a
  future "alignment" to the live usable capacity cannot reintroduce
  the divergence). The SOH-once companion
  (`lut_c_rate_axis_receives_the_soh_scaled_capacity_once`) had been
  added by the interrupted run and stands. Documented in
  `docs/equipment/ev.md` and `docs/equipment/battery.md` (both docs
  previously described the LUT composition but not the divisor rule).
- **W3 (actor seed froze the init-time usable capacity into the
  driver's fixed belief model)** — completed as found:
  `actor_seed` passes `degradation_adjusted_capacity_kwh()` (the
  temperature-independent rating) — the value the driver's own docs
  describe. The review's failing test
  (`actor_seed_capacity_is_independent_of_init_weather`) passes. The
  stale sibling test (`actor_seed_nightly_returns_ev_seed`) that
  asserted the old temperature-derated seed was updated to the
  corrected contract (fresh pack ⇒ exactly the 60 kWh rating, at any
  init temperature).
- **Review observation (the dwelling `ev_capacity_degraded` gate
  silently skipped when any of its four static telemetry keys was
  absent)** — fixed at the cause rather than remembered: a monitor must
  detect the absence of expected input, not merely anomalous values
  (the I-06 lesson). The gate now fails the step loudly with a
  `HaresError::Dwelling` naming the check, the equipment, and the
  missing key; false-positive sweep found no reachable path (the keys
  are published by `Ev::new` at construction, before any init/step),
  and the regression
  `ev_capacity_check_fails_loudly_when_ev_telemetry_keys_are_absent`
  (equipment_id_regressions — the I-06 monitor-blindness home) pins it
  through the real dwelling path.
- **Review observation (drive-I²R prose figure)** — the ~6 K figure
  above was an arithmetic slip; corrected to ~4 K with the arithmetic
  inline (28.6e6 A²·s × 11.2 mΩ ≈ 1.9 MJ into 480 kJ/K).

Verification for this round (bounded runs, `CARGO_BUILD_JOBS=2`,
`--test-threads=1`): **hares-equipment 2553/2553** (lib +
integration, includes the six contract tests above and the
`pack_electrical` tests), **hares-core 1271/1271** (lib + all
integration, includes the four reproduction gates, the override
loud-error gates, and the new monitor regression), parity + BESTEST
targets 17 passed (4 pre-existing skips), conservation 9/9, clippy 0
warnings (hares-equipment, hares-core, tests included), fmt clean.

### Coverage-triage round 9 (routed defect: `check_soc` silent skip — fixed at the cause)

The coverage triage (`coverage-note-quality.md`, round 9) verified the
review round's fixes, added four tests, and routed one proven production
defect: `check_soc`'s `if let Some(soc)` silently skipped the SOC-bounds
check whenever a BATTERY/EV equipment's core-output SOC was absent —
the same monitor-blindness class as the `ev_capacity_degraded` gate this
initiative had just made loud, ten lines above it in the same invariant
loop, with a comment naming the very anti-pattern ("a monitor must
detect the absence of expected input, not merely anomalous values").

- **The fix** (`dwelling/mod.rs`, the storage loop in
  `check_invariants`): the gate is now loud — an absent core-output SOC
  for the BATTERY/EV population fails the step with a
  `HaresError::Dwelling` naming the check (`soc_bounds`), the equipment,
  and the contract (every real Battery and EV publishes SOC at every
  step; the publish sites' `Soc::try_from(..).ok()` nulls the channel
  only on a non-finite/out-of-band internal SOC — itself a defect).
  The routed gate
  (`ev_soc_check_fails_loudly_when_core_output_soc_is_absent`)
  went green **unedited**.
- **Class question 1 — publish site vs monitor:** the monitor-side fix
  is the chosen remediation. It closes the observable defect for every
  cause of absence (a wiring defect and a `.ok()`-nulled channel both
  now fail the step loudly instead of silently skipping); the internal
  SOC is clamped to [0,1] at every mutation site in both models, so an
  equipment-side second error path would duplicate the failure for one
  underlying issue (the triage itself held the two as one design
  question, not two findings).
- **Class question 2 — other silent skips in the invariant section:**
  swept. The tank-node `None => break` is node-count discovery (a tank
  with fewer nodes legitimately publishes fewer keys — load-bearing,
  not a blind skip); the thermal check's `!q_gains.is_empty()` guard is
  a documented no-system case (an RC-less fixture cannot be
  energy-balanced); the remaining `if let Some` shapes in the step path
  are domain-update application (absence = nothing to apply — a valid
  state, not a monitor gated on an expected input). No other instance.
- **Caller updates forced by the fix (probe realism; assertions
  untouched):** `TelemetryRecordSpy` (multi_instance_naming) and
  `StepOrderSpy` (equipment_ordering_tests) now declare `HAS_SOC` and
  publish a core-output SOC **for storage end uses only** — the real
  Battery/EV they stand in for publishes SOC at every step, and the
  soc_bounds monitor now enforces it (2 stepped tests per file were
  affected; their dispatch/ordering assertions are unchanged).
  `IdentityProbeEquipment` gains a `with_published_soc` builder so the
  capacity-keys regression's probe isolates its own gate — the new
  soc gate sits earlier in the loop and its probe must stay green
  there (the test's assertion is unchanged; only the isolation
  premise was restored).
- **The CoreOutput contract caught the first probe attempt** — a spy
  publishing `state.soc` without declaring `HAS_SOC` fails the step
  with "undeclared fields populated: state.soc" — so the probe
  realism fix had to declare the capability exactly as the real
  storage equipment does (`ELECTRIC | HAS_SOC`, the same composition
  `hares-types/src/equipment.rs:1944` documents).

Verification for this round (bounded, `CARGO_BUILD_JOBS=2`,
`--test-threads=1`): **hares-core 1272/1272** (lib + all integration —
the routed gate green unedited, the two spy binaries green, the older
capacity-keys gate green with restored isolation), clippy 0 warnings
(hares-core, tests included), fmt clean. No `hares-equipment`
production file changed this round (the fix is core-only); its suite
stands verified from the review round (2553/2553).

### Red-team round (GridEmergency discharge corner: `Discharging` with zero flows — fixed at the classifier)

The red team found a corner the fix's own discharge-side preconditioning
enablement created: a commanded V2L/V2G export curtailed to zero by a
GridEmergency DR event (a commanded zero — the port is legitimately zero)
while the discharge-side heater draws 5 kW from the pack (deliberately
DR-fraction-unbounded, bounded by the floor invariant). The pack nets
−(export + heater) = −5 kW, so `classify_mode` reported `Discharging` —
but every published flow was zero (electric `Bidirectional(−0.0)`, unity
pf on a zero inverter leg, no thermal, no fuel), so the mode-flow guard's
Rule 1 rejected the pair and the dwelling's post-step
`validate_core_contract` killed the simulation — in the flagship
cold-outage V2L scenario itself.

- **Root cause:** `classify_mode` conflated "the pack nets negative" with
  "the vehicle is exporting". The pack-fed heater draw is internal
  preconditioning, not a grid-facing discharge — `Discharging` keyed on
  the pack net alone produced an active mode whose only flow (the
  pack-side DC draw) is port-invisible by design (a pack-side DC load,
  never a port flow), so no honest flow could ever satisfy Rule 1.
- **The fix** (`ev/mod.rs`, `classify_mode` + its one live call site):
  `Discharging` now keys on the export leg (`export_ac_kw < −1e-9`) —
  the grid-facing discharge; the pack-fed preconditioning state (heater
  drawing, zero export) reports `Standby` — energized and connected,
  zero port exchange, the same label the stationary Battery reports for
  the identical commanded-zero discharge corner, and guard-valid because
  the enum excludes Standby from active modes. The charger-fed
  preconditioning state (pack netting zero, port billing the heater's
  AC-equivalent) still reports `Heating`, unchanged. The guard was not
  touched (a check that fires was doing its job); the heater gate was
  not touched (the routed tests pin the heater running through the
  event — the R16 discharge-side contract stands).
- **Behavioral change confined to the corner:** the only (state, mode)
  pair that moved is net < 0 ∧ export == 0 ∧ heater > 0
  (`Discharging` → `Standby`); every other classification is bit-
  identical (Charging, export-leg Discharging, charger-fed Heating, On,
  Off, away-Off) — verified by the full suites below, including the
  mode-cycle parity corpus (5/5) and the mode/checkpoint regressions.
- **Doc surfaces updated:** `classify_mode`'s contract comment,
  `checkpoint.rs`'s `last_mode` field doc, and `ev.md`'s operating-mode
  line (which now states the export-leg keying and the `Standby` corner).

Verification for this round (bounded, `CARGO_BUILD_JOBS=2`,
`--test-threads=1`): the two routed gates green **unedited** (within the
15 grid-emergency gates, all green), **hares-equipment 2558/2558** (lib +
integration), **hares-core 1272/1272** (lib + all integration), parity
5/5 (incl. the `cz5a_ev_charging` corpus check), clippy 0 warnings
(hares-equipment, hares-core, tests included), fmt clean.

### Red-team round 2 (the classifier's 1e-9 threshold vs the guard's exact zero — lowered to the guard's exactness)

The second red-team run found the window the re-keyed classifier itself
opened: `classify_mode`'s charging branch required `net_charge_kw >
1e-9`, but the mode-flow guard's zero test (`is_zero()`, an exact float
match on 0.0) sees the published flow — so a sub-threshold charge leg
(any SOC residue small enough: a hand-set `initial_soc` 1e-12 under the
target, or an ordinary taper landing rounding **one ULP** short of the
target — both measured) classified `Off` while publishing
`Bidirectional(≈2.6e-10)`, and Rule 2 (Off forbids non-zero flows) failed
the dwelling step. The measured boundary (residue ≈4.2e-12 SOC) matched
the analytic prediction, sharpened by an η-factor mismatch (the
threshold keyed the post-η net while the guard sees the pre-η port
flow).

- **Root cause:** the classifier's "treat-as-nothing" tolerance disagreed
  with the guard's exact zero — the two sides of one classification
  contract, one thresholded, one exact.
- **The fix** (`ev/mod.rs`, `classify_mode` only): every comparison
  lowered to the guard's exactness — `net > 0.0`, `export < 0.0`, the
  heater/Standby boundary at `net >= 0.0`. A sub-threshold charge leg
  now classifies `Charging` with its honest nonzero flow (Rule 1
  satisfied); a landed target nets exactly zero and reports `Off` with
  all-zero flows (Rule 2 satisfied). The guard was not touched (a check
  that fires is doing its job); no flow was clamped (the charge leg
  genuinely delivers the tiny power — the routed test's precondition
  pins it nonzero); the routed tests went green **unedited**.
- **Class sweep:** the only other sign thresholds on classification
  paths — the EV's `heater_draw_w > 0.0` and `reactive_kvar != 0.0`
  (already exact) and the stationary Battery's
  `IDLE_POWER_THRESHOLD_KW = 1e-6` (its classifier yields `Standby` in
  the sub-threshold window — Rule-1-exempt — and its port carries the
  10 W standby draw plus heater watts, so its pair is guard-valid on
  both sides of the window). No other instance.

Verification for this round (bounded, `CARGO_BUILD_JOBS=2`,
`--test-threads=1`): both routed gates green unedited (within the 18
gates matching the mode-classification families — the 15 grid-emergency
gates, the Standby restore gate, and the two routed sub-threshold/taper
sweep gates), **hares-equipment 2561/2561** (lib + integration),
**hares-core 1272/1272** (lib + all integration), parity 5/5 (incl. the
`cz5a_ev_charging` corpus), clippy 0 warnings (hares-equipment), fmt
clean.

### Self-directed red team (the EV's full (mode, flows) publication surface, enumerated and pinned)

After the two routed red-team rounds both landed in the mode classifier,
a self-directed pass enumerated every reachable (mode, flows) pair the
EV publishes — Home charge path (net > 0 → Charging + nonzero port; net
= 0 with heater → Heating + nonzero port; vars-only → On + nonzero
reactive; all-zero → Off + zero), Home discharge path (export < 0 →
Discharging + nonzero port; export = 0 with heater → Standby + zero;
export = 0, heater 0 → On/Off), away (Off + zero by construction),
disconnected (Off + zero), dead bus (Off + zero), both dust windows
(exact-sign Charging/Discharging), and the restore faces (verbatim
mode/active/reactive) — and verified each against the guard's four
rules by construction.

The enumeration found every pair guard-valid and **two unpinned
corners** — both one edit away from `Off` + nonzero reactive → Rule 2 →
a dead dwelling, both in this initiative's own surface:

1. The dead-bus var suppression ("a commanded q-setpoint cannot be
   served by a dead EVSE", the gate at the reactive computation) —
   unpinned: a leak would publish the vars-only `On` mode on a
   physically dead bus. Pinned:
   `dead_bus_suppresses_a_latched_var_setpoint_flows_all_zero`.
2. The away-arm var zeroing — unpinned: a leak would pair the away
   `Off` mode with a nonzero reactive flow and kill every dwelling
   step while the vehicle is off-site. Pinned:
   `away_arm_zeroes_a_latched_var_setpoint_off_mode_flows_all_zero`.

Both pins assert the value (reactive = 0), the mode (`Off`), and the
behavior-level contract (`validate_core_contract`) — they go red if
either gate regresses. The remaining faces were already pinned by the
routed rounds and the checkpoint family (var-support restore, Standby
restore, the sub-threshold charge/export pair, the taper sweep). Sibling
sweep re-confirmed: the stationary Battery's 1e-6 idle threshold yields
`Standby` (guard-exempt) with a port carrying standby + heater watts —
no window on either side; the Battery's restore-contract coverage lives
at `battery/mod.rs:4144`.

Verification (bounded, `CARGO_BUILD_JOBS=2`, `--test-threads=1`): both
pins green, **hares-equipment 2564/2564** (lib + integration), clippy 0
warnings, fmt clean; hares-core unchanged since its last full green run
(1272/1272 — no core file touched in this pass).

### Red-team round 3 (the Python boundary's silent non-finite → default substitution — fixed at the `from_typed` choke point)

The third red-team run attacked the Python boundary of the fix's new
pack-thermal fields and found the silent-substitution channel: a
non-finite float (NaN/±inf — a bad sensor value, an unset variable, a
placeholder through a pandas pipeline) crossed the pyo3 extraction
verbatim, then `EquipmentConfig::from_typed` serialized the typed config
to JSON — **serde_json writes non-finite floats as `null`** — so a
`Some(NaN)` in an `Option<f64>` field became `None` in the stored
payload, `EvConfig::validate()` saw a clean absent field and passed,
and the field's documented default applied with no error anywhere.
Measured end to end by the red team: a NaN `battery_temp_c` silently
attached a differently-configured vehicle (ambient-cascade temperature,
default 5 kW heater preconditioning, 5.5556 kW constant on the port); a
NaN `initial_soc` silently became "full pack". Ten hostile cases red
(one per non-finite value on each new `Option<f64>` field).

- **Root cause:** the information loss lives in `from_typed`'s
  serialization, not at the Python layer — the same channel is
  reachable for **every** typed config field of **every** equipment
  from **any** boundary that can produce a non-finite float (the
  stationary Battery's `heater_power_w`/`heater_threshold_c` cross the
  identical channel through `add_battery`). Fixing `ev_config_from_py`
  would have fixed the instance and left the class open.
- **The fix** (`config/finite_walk.rs` + the guard call in
  `from_typed`): a full `serde::Serializer` walk that visits the same
  structure `serde_json::to_value` would, rejecting non-finite
  `f32`/`f64` leaves **with their dotted field path** (e.g.
  `battery_temp_c`, `inner.list[1]`), before the serialization that
  would silently null them — the one choke point every typed config
  crosses. A nested `Serialize` failure falls through to serde_json's
  own canonical error (the existing `FailSerialize` test stays green
  unedited). The rejected message names the equipment type, the field
  path, and the mechanism (JSON cannot represent non-finite floats).
- **Sibling proof, end to end:** a new boundary file for the stationary
  Battery (`tests/python/test_battery_pack_thermal_boundary.py`, 6
  hostile cases on the heater/cutoff/resistance fields through
  `add_battery`) pins the class fix at the sibling surface — the guard
  cannot regress for one equipment only.
- **No legitimate non-finite exists in any typed config** (swept: the
  `f64::INFINITY` uses in the workspace are `ControlSignal` test code,
  not configs), so the guard cannot break a real configuration path —
  proven by the full suites below, every one of which crosses
  `from_typed` for every equipment it constructs. The raw-config path
  (`EquipmentConfig::raw`, the Python adapter layer's custom equipment)
  is outside the class by construction: it crosses no typed-config
  serialization, and a non-finite value there is the adapter author's
  own data consumed by their own equipment — no HARES-documented default
  is ever silently substituted for it.

Verification for this round (bounded throughout: `CARGO_BUILD_JOBS=2`,
pytest `-n 1`/`-n 2` instead of the `-n auto` addopts default, the
maturin rebuild bounded — the unbounded variants are the OOM hazard
this session's history forbids): the routed EV boundary file green
**unedited** (26 tests: the 10 previously-red non-finite cases, the 10
finite air-cover cases, the usable-dwelling case, the 5 topology
cases), the new Battery sibling file 6/6, the walker's 5 unit tests
and 3 `from_typed` regression tests (NaN with path, ±inf nested in a
container with the full path, finite/absent pass) green, **hares-
equipment 2572/2572** (lib + integration), **hares-core 1272/1272**,
**hares-io 1334/1334**, **hares-fleet 59/59**, the **full Python suite
1199 passed / 12 skipped** (the skips are the pre-existing observer /
Helics feature-gated markers, named and none caused by this change),
clippy 0 warnings (hares-equipment), fmt clean.

### Re-review round (one Critical, three Warnings, two observations — all addressed)

The re-review confirmed every prior round's fix and raised one Critical
boundary defect, three process findings, and two observations. All
addressed:

- **Critical (non-finite floats silently become defaults at the
  `overrides` and tariff Python boundaries):** the round-3 fix's sweep
  missed two channels because the Python→JSON converter was duplicated
  — `python_to_json_value` (py_dwelling) and `py_to_json_value`
  (py_tariff), plus a third copy `python_to_json` (py_config, already
  loud by construction via `Number::from_f64` → fall-through). **Fixed
  by unification:** one shared converter (`utils.rs`,
  `python_to_json_value`) whose f64 arm rejects non-finite floats with
  `PyValueError` naming the mechanism; all three per-module copies
  deleted; every free-form Value channel (equipment `overrides` — the
  raw and typed-payload paths — the `zip` sub-channel, both tariff
  `from_dict` surfaces, config overrides) crosses the one reviewed
  converter. **The sweep went further than the review:** a fourth
  channel it did not list — the `EquipmentSpec` parameters path
  (`insert_opt`/`json!` in `py_hvac`/`py_water_heater`, where `json!`
  nulls a non-finite before the spec reaches
  `equipment_config_from_spec`) — is closed at the `make_spec` choke
  point: any `null` anywhere in the spec parameters tree is rejected
  naming the key path (`reject_nulled_value`), sound because no
  legitimate null can appear (`insert_opt` skips `None`s, the explicit
  `json!` sites insert computed finite values — verified by grep). The
  custom-actor raw path (`py_to_config_value` → `add_actor_by_name`)
  stays outside the class: it crosses no typed-config serialization and
  no HARES-documented default is substituted (the adapter author's own
  values, consumed by their own actor). The two red gates went green
  **unedited**, and the spec-params channel has its own permanent pin
  (`test_non_finite_spec_parameter_fails_loudly` in
  `tests/python/test_blueprint_errors.py` — `oversizing_factor` is
  unvalidated at construction and crosses to `make_spec` exactly when
  `autosize=True`, so the three non-finite cases go red if the guard
  regresses).
- **Warning (the ASHRAE citation gate's coverage silently narrowed):**
  `EXCLUDE_DIRS` basename matching exempted `crates/hares-io/src/hpxml`
  — production parser code carrying this gate's exact citation class.
  **Fixed with path-based pruning:** both citation scripts
  (`check-ashrae-chapters.sh`, `check-energyplus-sections.sh`) now
  `find`-prune by full path (`docs/reviews`, `docs/eplus`, …,
  `tests/fixtures/hpxml`, `-name vendors`), so every production
  directory stays scanned. Coverage restoration proven by execution:
  the gate's own output now lists `crates/hares-io/src/hpxml/building.rs`
  citations (13 parser files, 2 carrying ASHRAE HoF 2021 citations,
  back under the gate; the three fatal patterns re-run against the
  directory: zero matches — nothing was ever hidden, but now nothing
  can be).
- **Warning (the `i07_diag.rs` tombstone deferral):** deletion re-attempted
  at the re-review's explicit direction — **blocked again** by the
  session permission layer (verbatim: "NEEDS APPROVAL: deletes files";
  the approval cannot be granted in this non-interactive session, and
  searching for an unblocked deletion route is forbidden). The Known
  Limitations entry is updated with the exact one-command operator
  action and the quoted denial; no tool this fix loop holds can remove
  the file.
- **Warning (the recorded HELICS flake, no ticket):** root-caused and
  **fixed in the tests**: the flake was two absolute wall-clock
  assertions (`elapsed < 5.0` at a 0.2 s timeout, `elapsed < 15.0` at a
  2 s connect timeout) whose "promptly" budgets break under the
  suite's own default `-n auto` (28 xdist workers) machine load. The
  assertions' real discrimination — fires vs hangs — was already proven
  by their `pytest.raises(TimeoutError)`; the bounds are now
  load-insensitive (30 s / 90 s, well inside each file's thread-method
  pytest timeout, which catches true hangs) with the constraint
  documented at each site. Verified by execution: helics is installed
  in this environment — both files run green (33/33).
- **Observations (both doc notes added):** `heater_threshold_c`'s field
  doc now states the deliberate no-validation-against-`min_charge_temp_c`
  contract (a threshold below the cutoff is a coherent protect-only
  policy — anti-freeze heating without cold-charge preconditioning —
  observable in telemetry, never silent); `deg_const`'s comment now
  names the SSC lineage (current trunk, `lib_battery_lifetime_nmc.h`,
  carrying the rainflow cycle model) and warns that the copy vendored
  under `vendors/EnergyPlus/third_party/ssc` is an older revision whose
  constants differ (c0,ref 75.64, c2,ref 0.0039193).

Verification for this round (bounded throughout: `CARGO_BUILD_JOBS=2`,
pytest `-n 1`/`-n 2` — never the `-n auto` default, the maturin rebuild
bounded): the two red gates green **unedited** (6 tests with controls),
the HELICS files 33/33, **the full Python suite 1201 passed / 12
skipped** (the +2 vs the review's 1199 = the two now-green gates),
**workspace nextest 7037/7037** (sequential), workspace clippy 0
warnings, fmt clean, both citation gates OK with production coverage
restored.

### Red-team round 4 (the actor-param boundary — the class member my own scoping got wrong)

The red team proved, by execution, that my earlier class scoping was
wrong: I had classified the `add_actor_by_name` raw params path as
"outside the class — the adapter author's own values, consumed by
their own actor." Wrong on the consumed-by half: the params feed
**built-in** actor seeds (`actor_registry.rs` reads `heating_c`/
`cooling_c`/`deadband_c`/`hysteresis_c` for the IdealThermostat,
`daily_drive_miles_mean`/`event_day_ratio`/`seed` for the EvDriver)
with no finite guard, so a NaN parameter armed a real actor with a
poisoned value and the failure surfaced only mid-simulation — a
telemetry panic in debug builds, a silently never-acting controller
behind a `tracing::error!` line in release builds (the panic branch
is cfg-gated to debug/`check_invariants`). Measured end to end: the
finite `heating_c = 21.0` control heats (0.7665 kWh); the NaN variant
attaches identically and then dies or silently never heats.

- **The fix** (`py_dwelling.rs`, `py_to_config_value` — the one
  Python entrance for actor params, one caller: `add_actor_by_name`):
  the f64 arm rejects non-finite floats with `PyValueError` naming the
  value and the poisoning mechanism, and the `Vec<f64>` arm rejects
  any non-finite **element** with its index (the same class one deeper
  — `ConfigValue::FloatArray` consumers are poisoned the same way).
  The guard covers every built-in seed's params by construction,
  including the EvDriver set the red team flagged. The eight routed
  cases went green **unedited**; the finite control (attach, simulate,
  heat) stays green.
- **The deeper-layer question (guard at `ActorConfig::get_f64` or in
  each seed) rejected:** the boundary is where the user's value enters
  — the same architectural choice every prior round made (the
  converter, `from_typed`, `make_spec`); Rust-side `with_param` callers
  are finite test literals (grep-verified), so no Rust-side route
  exists to guard.

Verification for this round (bounded: `CARGO_BUILD_JOBS=2`, pytest
`-n 1`/`-n 2`, the maturin rebuild bounded): the routed file **9/9**
(the eight previously-red cases unedited), **the full Python suite
1213 passed / 12 skipped** (the same named pre-existing skips),
**workspace nextest 7037/7037** (sequential), clippy 0 warnings
(hares-python), fmt clean. The operator also removed the `i07_diag.rs`
tombstone this round (the one blocked disposition — resolved).

### Coverage-triage round (the solar-override boundary — the sixth non-finite channel, found by the triage's own class sweep)

The coverage triage's challenge-pass re-examination of its own
solar-override dismissal found the dismissal's load-bearing leg
collapsed: the downstream finiteness screens I had cited are
`#[cfg(any(debug_assertions, feature = "check_invariants"))]` gated —
**silent in release builds**, the exact shape this initiative fixed at
five other boundaries. `Dwelling.set_solar_override` accepted
non-finite irradiance with no rejection at any layer: the parse arms'
extraction sites carry no `is_finite` guard,
`EnvironmentState::set_solar_override` stores verbatim, and the channel
is the documented PySAM/pvlib injection pipeline
(`generate_pvlib_solar_override.py`) where a NaN from missing satellite
data is the normal failure mode.

- **The fix** (`py_dwelling.rs`, `PyDwelling::set_solar_override`): a
  **post-parse walk** over the parsed `Vec<Vec<SurfaceIrradiance>>`
  table, rejecting any non-finite field with the field name, timestep,
  and surface index. The walk runs on the parsed table, not on any
  arm's extraction internals, so it covers **every parse arm (path,
  list, polars/pandas DataFrame, ndarray, dict) by construction** —
  the triage's class sweep ("if any arm has its own extraction, the
  fix must cover it") is closed structurally rather than arm by arm.
  The three routed gates went green **unedited**; the finite control
  attaches and `has_solar_override()` stays true.
- **The layer choice** (the triage offered the extraction helper or
  `set_solar_override`): the post-parse walk at the boundary entry —
  the boundary that received the user's value, the same architectural
  choice as every prior round (the converter, `from_typed`,
  `make_spec`, `py_to_config_value`), and arm-agnostic where a
  per-extraction guard would be arm-coupled.
- **The triage's own honesty noted:** its first-draft dismissal was
  corrected by its own challenge pass — the dismissal's downstream-leg
  was cfg-gated; the correction is the finding. The record here mirrors
  that: my earlier sweep had also dismissed this family as
  "downstream-screened" data propagation — the same wrong leg.

Verification for this round (bounded: `CARGO_BUILD_JOBS=2`, pytest
`-n 1`/`-n 2`, the maturin rebuild bounded): the routed file **4/4**
(the three once-red cases unedited, the finite control), **the full
Python suite 1221 passed / 12 skipped** (the same named pre-existing
skips; the pvlib/parity feeds that use this channel pass unchanged),
**workspace nextest 7037/7037** (sequential), clippy 0 warnings
(hares-python), fmt clean.

### T4 settlement + the directive sweep (no tickets, no `should_panic`, sccache + nextest)

- **I-07-T4 settled by execution, not deferral:** the f64-arm comment
  at the unified converter (`utils.rs`) now states the verified
  mechanism — genuine non-finite floats are rejected by the arm's
  `is_finite` check, while huge Python ints (10**400) fail pyo3's f64
  extraction in practice and fall to the unsupported-type catch-all,
  which rejects them loudly there. Both mechanisms re-verified by
  execution through the real `overrides` channel (the huge int:
  `cannot convert Python object of type 'int' to JSON`; the NaN:
  `non-finite float value (NaN) cannot cross the Python boundary`).
  The ticket is resolved with the resolution note in-file; **no open
  tickets remain** (T1–T3 were already resolved).
- **`should_panic` sweep:** the initiative's entire uncommitted diff
  adds **zero** `#[should_panic]` (`git diff HEAD | grep -c
  '^+.*#\[should_panic'` → 0); the eight pre-existing ones
  (water_heater_tests, pv/soiling, equivalent_battery ×4, registry,
  schedule) predate HEAD in files the initiative never touched and
  carry `expected = "…"` strings — outside the initiative's diff and
  outside every routed finding.
- **Toolchain directives adopted:** sccache for every subsequent Rust
  build (18 Rust cache hits on the final builds) and nextest for every
  Rust test run (the session's convention throughout).

Final verification (sccache + nextest, bounded): the six boundary
surfaces **44/44** (EV pack-thermal 20, Battery sibling 6, actor-param
13, solar-override 4, the huge-int gate), **workspace nextest
7037/7037** (sequential, `--test-threads=1`), workspace clippy 0
warnings, fmt clean, the full Python suite green twice this session
(bounded `-n 2`: 1221/12; and an accidentally-unbounded default-worker
run my malformed probe command triggered: 1225 passed / 12 skipped /
8 xfailed — green, but not the bounded form I hold myself to; noted
honestly, no recurrence).

### Coverage-triage round (the non-finite boundary class's remaining members — price, grid voltage, OCV/U-neg tables, ControlSignal)

The triage's round-16 class sweep routed one consolidated finding:
four more members of the non-finite boundary class silently accept
NaN/±inf. All four fixed at the boundary that received the value, plus
the class sweep's own extensions beyond the gated faces:

- **Shared home first (DRY):** `validate_finite`/
  `validate_non_negative`/`validate_range` + `dict_required`/
  `dict_optional` unified into `utils.rs` (pub(crate)) with the
  rationale section comment (NaN comparisons always false; downstream
  screens cfg-gated debug-only), plus the boundary-guarded f64
  spellings `finite_f64_required`/`finite_f64_optional`. The two
  former `dict_optional` copies (py_dwelling, py_control) and
  py_control's local validators deleted — one reviewed home. The
  validators' order fixed by construction: finiteness first, never
  the one-sided comparison a NaN fails-and-passes (the exact shape
  the OCV positivity check exhibited).
- **`set_price_signal`:** the three price fields cross via
  `finite_f64_optional` — a NaN price can no longer silently arm the
  price actors.
- **`set_grid_voltage`:** `validate_finite(voltage_pu)` — finiteness
  only, the nominal-range contract left to the grid model.
- **OCV/U-neg tables (`battery/ocv.rs`):** finiteness legs in
  `OcvTable::new` (both vectors — a NaN slipped every one-sided
  comparison: strictly-increasing, positivity) and `UNegTable::new`
  (soc_points; potential_v was already guarded — its dead
  `|| v.is_nan()` clause dropped, message normalized to the indexed
  sibling convention). Mirrors `RegularGridInterpolator::new`'s
  existing axis/value finiteness legs — the sibling constructor's own
  convention, and the discovery that the charging-LUT channel
  (npz + dict) was **already guarded** there, so member 3's fix
  needed only the two table constructors. Covers all six Python
  arms + the Battery kwargs arm by construction. One self-caught
  slip during the edit: my UNeg rewrite initially dropped the
  soc strictly-increasing leg — restored immediately (finite checks
  are additive, never replacements).
- **ControlSignal (`py_control.rs`):** every raw-crossing f64 field
  guarded — the 15 gated faces (typed + from_dict) plus my sweep's
  23 ungated same-class faces (typed `thermal_setpoint_delta`,
  `humidity_setpoint`, `power_limit` ramp, `duty_cycle` period,
  `demand_response`/`_str` duration, `reactive_setpoint`,
  `power_factor_setpoint`, `ev_set_ready_by` departure_hour,
  `event_delay`; from_dict's `kvar`, `power_factor`, the deltas,
  `delay_s`, `departure_hour`, `duration_s`). Nine constructors moved
  `Self` → `PyResult<Self>` (the raising contract the boundary
  implies); from_dict's f64 sites now use the `finite_f64_*`
  spellings. The sharpest gap the triage named — `PowerSetpoint`'s
  min_soc/max_soc raw next to SOCTarget's validated same-named
  fields — closed with the same mechanism.

**The strict-xfail lifecycle honored:** the four routed gate commands
run with `--runxfail` all pass for real (price 4/4, grid voltage 4/4,
OCV 4/4, ControlSignal 16/16 — every face raises); the markers then
removed per the self-destruct contract (the gate file's own docstring
instructs this), the file re-run plainly: **28/28 green** — the gates
are now the permanent regression tests. No gate was weakened: the
faces are byte-identical, only the markers and the routed-state prose
updated to the landed state.

Verification (sccache + nextest, bounded): the routed file 28/28,
hares-equipment + hares-python nextest **2746/2746**, full Python
suite **1249 passed / 12 skipped** (the 23 former xfails now pass),
**workspace nextest 7037/7037** (sequential), conservation 9/9,
workspace clippy 0, fmt clean.
