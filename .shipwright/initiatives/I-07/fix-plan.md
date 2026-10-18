---
plan_outcome: ready
---

# Fix plan: align the EV pack thermal sub-model with the Battery equipment's
# architecture (gh #32 / I-07) — revision 17

This plan commits to one approach: align the EV pack thermal sub-model
with the stationary Battery equipment's architecture — behavioral
alignment inside `hares-equipment/src/ev/`, not a structural merge —
fixing the RCA's three coupled defects (D1 unphysical thermal mass, D2
misattributed charger losses, D3 dead preconditioning) plus the
same-class divergences the fix's own files expose, with every value
sourced or a merge condition. The binding constraints are collected
below; the revision log records how the plan converged under adversarial
critique.

**Revision log:** R2 — pack-electrical model owned; V2L heater billing;
shared charging-isolating metric; override loud-error in scope. R3 —
heater sized per level; guard bounds stated. R4 — τ recomputed from the
adopted UA; the UA ceiling stated; the priority rule's boundary
disambiguated; `away_charge_actual_kw` kept at its documented meaning.
R5 — heater telemetry value-carrying; the degradation shift stated and
kept observable past the fold. R6 — heater re-classed to the pack duty
(the cabin figure withdrawn); the supply cap reframed as the normal
path; the guard rebuilt on the 0–55 °C validated domain; thermal mass
via mass × specific heat. R7 — parity withdrawn as thermal
cross-validation on measurement; ambient-aware initialization in scope;
the C1×C2 interaction found in reassessment, forcing the
`heater_power_w: 0` fixture policy. R8 — the ambient-init test exposure
named (13 temperature-sensitive tests); the parity-harness no-op
condition; the binding-constraints index. R9 — the LUT × ramp
composition decided. R10 — the billing rule's consumers traced into
production code: mode classification to the charge leg with a `Heating`
preconditioning state, the mode-flow guard's direction rule fixed with
it, the var model keyed to the inverter leg; the header restructured into
the current-state form. **R11 (this revision)** — guard Rule 2 analysed
(the vars-at-zero-power hard error) and fixed with the classifier's var
leg (`On`) and Rule 1's reactive symmetry; the `Heating` reuse grounded
on the zone-guarded classifiers and `zone: None`, with its boundaries
recorded; the blast-radius list gains the `hares-types` entry its text
claimed. **R12 (this revision)** — the Rule 4 direction-rule re-keying
**withdrawn**: read at its match arms, the rule exempts `Bidirectional` —
which the EV emits at both CoreOutput sites — so the claimed
V2L-with-heater runtime error never existed; the claim had rested on the
rule's doc-comment summary adopted without reading the arms. `hares-types`
keeps only Rule 1's reactive term (required by the `On` classifier state,
its workspace-wide reach stated) and the Rule 2 fix. **R13 (this
revision)** — the discharge leg: the heater's pack-side draw is bounded
by the same `effective_floor` as the export (the discharge budget covers
the total pack-side draw, and at the floor the heater stops with the
export), restoring the mobility guarantee the removed gate implicitly
carried; the debit's efficiency is named — `charging_efficiency`, the
EV's single field, not the Battery's two-field split. **R14 (this
revision)** — found in reassessment by a conservation check: the billing
rule (port = charger + heater, "both signs") and the discharge debit
(pack = export + heater) cannot both hold — they double-count the heater
across the vehicle boundary. Resolved by the electrical placement the
plan's own var section already claimed ("resistive, not
inverter-coupled"): the heater is a **pack-side DC load** (HV PTC, as in
real EVs), not an AC-side pad heater as in the stationary Battery — the
port carries the charger's AC input alone in every state, the pack's
netting includes the heater draw in all states, and the old code's
`(charger − heater)` deduction is recorded as the right shape insight
(pack-side heater) with wrong η placement and wrong port accounting.
**R15 (this revision)** — the re-placement split what one name covered:
the charger's **AC input** (port, supply cap, var model; raised to cover
the heater) and the **pack-side net charge rate** (mode classification,
SOC credit; `charger_ac · η − heater_dc`) are now named and distinguished,
because keying mode on the AC input's sign would reinstate the
revision-10 defect in the heater-only state; the cap's binding condition
is corrected for the η inflation (`heater_dc / η > 0.5 · rated`), and the
`heater_only_grid_draw` identity's magnitude change is recorded.
**R16 (this revision)** — the supply-cap formula
`min(rated, power_limit, dr·rated)`, invented in R3 and carried eleven
revisions, **misdescribed the code and is withdrawn**: DR is a multiplier
on the computed power (`ev/mod.rs:829`), not a term in a `min` — the two
agree only at full demand, and the ceiling form would have silently
weakened dispatch obedience in exactly the derated regime this fix
creates (a `High` event with no effect on a half-rating vehicle;
`Critical` delivering twice the allowed power). The composition is now
semantics-preserving: bound `min(rated, power_limit)`, then the DR
fraction scales the allocated total; the DR figures in the justification
are recomputed under the multiplier; and the two discharge legs' DR
composition is stated as unchanged. **R17 (this revision)** — the raised
charge command is bounded, not free: `taper_limit`
(`ev/mod.rs:819-823`) is corrected to cover the heater's DC diversion
(the bound's purpose — land exactly on the target — is preserved by
construction), and a commanded `power_setpoint_kw` (`:678-682`) is a
total-draw bound with the heater carved out by the priority rule, so the
vehicle never draws more than its dispatch commands; the nominal-regime
effect is a delay the window absorbs, and the marginal-band shortfall is
stated with the critique's arithmetic.

## Binding constraints (collected)

One index of the load-bearing constraints, so the fix step does not have
to reassemble them from ~700 lines of prose:

- **UA ceiling** (parked-pack regression): ≈24 W/K at ~500 kJ/K mass; the
  (mass, UA) pair is re-derived jointly if a cited derivation exceeds it —
  the test is never widened.
- **Thermal mass:** rated capacity via pack mass × 800–1100 J/(kg·K)
  cell specific heat (≈480–500 kJ/K for 75 kWh); OCHRE is the
  cross-check, not the source.
- **Heater default:** the pack-preheating duty class (trade indication
  3–8 kW; the exact value is a cited merge condition); constant-power, a
  documented divergence with overstating error direction.
- **Threshold default:** 5 °C class; clears the ≳1.7 °C break-even lower
  bound (CC-CV taper raises the true value).
- **Supply cap:** the AC-input bound is `min(rated, power_limit)` at
  home; the DR fraction is a multiplier on the allocated total (as the
  code applies it — never a term in the `min`); priority
  swaps exactly at `min_charge_temp_c` — the heater never takes budget
  charging could use; the bound is the normal path for cold sessions.
- **Raised-command bounds:** `taper_limit` gains the heater's
  AC-equivalent when the heater runs (lands exactly on target, no
  overshoot); a commanded `power_setpoint_kw` is a total-draw bound —
  the heater carved out by the priority rule, the charge leg taking the
  remainder.
- **LUT × ramp composition:** a configured charging-curve LUT's temperature
  axis *is* the temperature-dependent capability — the linear ramp's
  derate is not multiplied on top; the `min_charge_temp_c` safety cutoff
  applies unconditionally, under every extrapolation strategy.
- **Mode classification:** from the charge leg — `Charging`/`Discharging`
  by the pack-side net charge rate's sign — never the AC input's, which
  the heater-only state raises positive — heater-only as `Heating`,
  vars-only as `On`,
  all-zero (including vars) as `Off`; the guard's flow rules count
  reactive power symmetrically (Rule 1 gains the term Rule 2 has), and
  Rule 4's `Bidirectional` exemption — which permits heater-during-V2L
  by design — is verified and left as-is.
- **Vars:** both the power-factor baseline and the kVA headroom clamp key
  on the inverter leg — the charger's AC input — never the
  heater-inclusive port
  total.
- **Discharge-side floor invariant:** during V2L/V2G the discharge budget
  covers the total pack-side draw (export at `charging_efficiency` plus
  the heater's unconverted DC draw); the heater runs only
  above `effective_floor` and stops with the export at it; the heater is
  a pack-side DC load in all states — the port carries the charger's AC
  input alone.
- **Guard:** two layers — physical-envelope error bounds, and the 0–55 °C
  validated domain (Smith 2017) with owned, documented sub-zero
  extrapolation; `deg_const` verified against the paper as a merge
  condition.
- **Ambient init:** explicit config → zone → outdoor ambient (the
  Battery's cascade); 20 °C only the no-ambient fallback.
- **Parity:** withdrawn as cross-validation for pack *thermal* behavior
  (no OCHRE reference can exist); the EV fixtures' vehicles configured
  `heater_power_w: 0` (forced); a charging-EV fixture routed to a
  tracking ticket.
- **Metric:** one shared, charging-isolating charge-day helper (the SOC
  column needs verbosity ≥ 3; all current call sites run 5).
- **Ordering:** `i07_diag` assertions green → whipsaw regression green →
  fold.

## Target

The RCA's confirmed root cause is D1–D3 in `hares-equipment/src/ev/`: the EV
pack thermal sub-model is a divergent reimplementation of the architecture
the stationary Battery equipment already gets right — unphysical thermal
mass (D1), charger conversion losses misattributed to the pack as heat (D2),
and heater heat gated on charge power so preconditioning is dead exactly when
needed (D3). The ticket's identity hypothesis was refuted by execution and is
**not** the target; the reproduction's scenario test keeps the raw-injection
path exercised end-to-end as a guard. The reproduction's four failing tests
map one-to-one onto the defects.

## Chosen approach

Align the EV with the Battery model's equations and gating philosophy —
behavioral alignment inside the EV subsystem, not a structural merge — plus
the class fixes the audit found. The 0 °C cold-charge cutoff is **kept**
(Li-ion plating below 0 °C; BMS cutoffs standard practice; the fidelity
target is the compensating real behavior). `DEFAULT_MIN_CHARGE_TEMP_C = 0.0`
enters the fixed code with a named primary source — now identified
(observation): Battery University BU-410 and industry guidance (metallic
lithium plates on the anode below 0 °C instead of intercalating; the damage
is permanent; BMS low-temperature charge cutoffs at 0 °C are standard
practice) — with the rest of the defaults block.

### The pack electrical model — with its alternatives

D2's I²R formulation needs pack OCV and pack resistance
(`battery/mod.rs:548-552`): `pack_ocv = cell_ocv · n_series`,
`pack_resistance = cell_resistance · n_series / n_parallel`. The EV has
**one** of the four inputs; `OcvTable::voltage_at_soc` is *cell* OCV
(2.3–3.75 V), and cell-level and pack-level voltage are never
interchangeable in this code.

**Alternatives considered:** lumped pack-level fields (`pack_voltage` +
`pack_resistance_ohm`) — rejected: pack OCV must track SOC through the
shared per-chemistry cell table, requiring a series count anyway; cell
resistance is the quantity the electrochemistry literature reports, so it
is the citable one; dropping the decomposition loses the consistency check
(series × per-cell capacity ↔ `capacity_kwh`). An empirical I²R
coefficient per kW — rejected: an uncited magic number hiding exactly the
physics this fix exists to expose. **Chosen: the Battery's parameterization**
— new EV config fields for series/parallel configuration and cell resistance
with cited defaults, pack OCV and resistance derived from the existing
`OcvTable`, current by the same quadratic terminal-voltage method.

**DRY committed:** the quadratic solve, the `P_max = Voc²/4R` clamp, and the
I²R computation move to **one shared home in `hares-equipment`**, called by
both models; the Battery's `compute_electrical` body becomes a call to it
with its public surface and behavior unchanged (tests green across the
move); the OCHRE `Battery.py:295` citation lands in exactly one place.

### D2 — loss attribution

Replace the `(1−η)·P` pack heating in `apply_soc_and_thermal` with I²R cell
heating from the pack electrical model. SOC accounting on the charge leg is
untouched (`dc_stored = charger · η`). **Inherited approximation,
documented:** the Battery applies the flat η *and* adds I²R as heat without
deducting it from stored DC; alignment inherits this small, pre-existing
approximation, recorded in the alignment comment. **Bernardi entropic term
— checked and recorded as a modeling boundary (observation):** the full
cell heat balance is `q = I(V − U) + I·T·(dU/dT)`; HARES models the
irreversible ohmic half only. The reversible entropic term's *share* grows
as C-rate falls — this fix's cold-charge regime is ~0.013 C — but its
magnitude does not matter here: ~1.7 W against ~102 W ohmic at full L2,
~0.15 W against ~0.8 W at the cold trickle, both dwarfed by ~110 W of UA
loss and the kilowatt-class heater that dominate this regime's pack energy
balance. The omission is stated as a known divergence with its arithmetic,
not silently held.

### D3 — preconditioning: gate, billing, supply cap, and limits

- **Gate:** the heater activates on pack temperature whenever the
  vehicle is connected with an energized supply — charging, idle, or
  discharging (Battery precedent:
  `heater_activates_when_discharge_requested_and_cells_cold`,
  `battery/mod.rs:3307`) — with **one discharge-side SOC bound (C1-rev12):
  during V2L/V2G the heater runs only while headroom above
  `effective_floor` exists, and at the floor it stops with the export.**
  The old gate's two terms are not restored — `!is_v2l_discharge`
  (heater never during discharge) is the D3 defect being fixed, and the
  old `soc < effective_soc_limit()` term conflated "heater only when
  charging is on the table"; the floor term is narrower and exists for a
  different reason: preconditioning protects future mobility, and
  draining the pack below the departure floor — `effective_floor` is the
  max of the reserve, any commanded `min_soc`, and `ready_by_soc`
  (`ev/mod.rs:918-931`) — to warm it destroys the very mobility it
  protects. Below the floor, the reserve wins. `Disconnected` stays
  drift-only; the outage gate continues to zero the heater.
- **Billing — the heater's electrical placement, corrected in
  reassessment (conservation):** the heater is a **pack-side DC load**
  (HV PTC, as in real EVs — the same "resistive, not inverter-coupled"
  physics the var section below already claims, cited at fix time), not
  an AC-side pad heater as in the stationary Battery. The Battery's
  alignment was right for the *gate* and the *thermal equation*; its
  electrical placement was never the right analogue. Consequently: **the
  port carries the charger's AC input alone in every state** — import
  when charging or covering the heater, export when discharging — and
  **the pack's netting includes the heater draw in all states**. While
  connected with the heater running, the charger's commanded AC input
  covers it: the BMS raises the charge command so the pack still reaches
  its target despite the heater's DC draw — **and the raise is bounded,
  not free (W1-rev16): the two limiters built on the assumption that
  every delivered joule reaches the cells are resolved, not overridden.**
  (1) `taper_limit` (`ev/mod.rs:819-823`) is the headroom-to-target AC
  bound, `(soc_limit − soc) · capacity / dt / η`; with the heater
  diverting DC, the bound that preserves its purpose — land exactly on
  the target, no overshoot — is `(headroom_dc + heater_dc) / η`: the
  taper term gains the heater's AC-equivalent when the heater runs, so
  the corrected bound still lands the pack on target **by construction**,
  and the plan's "still reaches its target" is true rather than
  aspirational. (2) A commanded `power_setpoint_kw` (`:678-682`) is a
  **total-draw bound**: the heater is carved out within it by the
  priority rule and the charge leg receives the remainder — the vehicle
  never draws more than its dispatch commands, the obedience guarantee
  this plan's DR rationale exists to keep. The magnitude is stated
  honestly: in the nominal regime (pack at the 5 °C class, derate 0.5, a
  ≈2.8 h session in an 8 h window) a binding step is a **delay the
  window absorbs**; only in the marginal band near the ≈1.7 °C
  break-even, where the heater runs at high duty and the window is fully
  consumed, does it cost 0.010–0.027 SOC per binding step, non-cumulative.
  (The plan's one taper mention, in the break-even derivation, is
  `cc_cv_mult` — the electrochemical roll-off, a different quantity from
  this headroom bound.) A heater-only plugged-in state bills the heater's
  AC-equivalent (`heater_dc / charging_efficiency`) at the port, which
  is the honest version of the old `heater_only_grid_draw` identity. The old code's
  `net_charge_kw = (charger_kw − heater_kw)` deduction is thereby
  **recorded as half-right**: the pack-side placement was the correct
  shape insight; the η placement (converting the heater through the
  charger efficiency in the wrong direction) and the port accounting
  (billing `charger + heater` on top of the deduction) were the defects.
  `AwayPluggedIn` → **zero residential port contribution, unchanged**
  (an office car park must not appear on the home meter). **
  `away_charge_actual_kw` keeps its documented meaning (W1)** — "the
  actual intake after CC-CV taper, not the [rating]" (`ev/tests.rs:3096`,
  asserted at `:3068`/`:3099`/`:3120`) — it is *not* overloaded with
  heater draw. Away heater draw is observed through `HEATER_POWER_W`,
  **which this fix makes value-carrying (W1)**: today the key emits the
  configured nameplate gated on the `heater_active` bool
  (`ev/mod.rs:1153-1160`); the fix computes the **post-cap actual heater
  draw as a value** used by the pack netting, the thermal equation, and
  the telemetry alike — the Battery's computed-value pattern
  (`battery/mod.rs:1342`, `:1569`) — with `heater_active` true exactly
  when actual draw > 0, so "active but drawing nothing" is unrepresentable
  rather than reportable.
- **Two quantities, named (C1-rev14):** the re-placement splits what one
  name covered — the **charger's AC input** (port-side: what the port
  bills, what the supply cap bounds, what the var model keys on; raised
  to the heater's AC-equivalent when the heater runs while connected) and
  the **pack-side net charge rate** (cell-side: `charger_ac · η −
  heater_dc` — the rate the pack is actually gaining through its
  terminals; what the mode classifier and the SOC credit key on, and what
  the derate zeroes below the cutoff). The implementer names them in
  code; the plan's constraints each key on the one they mean — supply cap
  and var model on the AC input, mode rule and SOC credit on the
  pack-side net — and this is the corrected completion of the old
  deduction's shape insight: `charger_ac · η − heater_dc`, not
  `(charger_ac − heater_ac) · η` — the η on the charger's conversion
  only. Keying mode on the AC input's sign would classify the heater-only
  state (input raised to `heater_dc / η`, cells netting exactly zero) as
  `Charging` — reinstating the revision-10 defect; the distinction is
  what keeps that fix.
- **Away energy boundary, stated explicitly (observation):** away heater
  energy is consumed off-site and appears in no dwelling energy balance —
  correct, since the load genuinely is off-site, but the boundary is
  documented in the alignment comment so a reader comparing pack heat gain
  against site energy finds an explained source, not an unexplained one.
- **Discharge-leg SOC debit and budget (C1-rev12, W1):** during V2L/V2G
  the port carries the export alone (the heater is pack-side, not at the
  port), and the pack supplies two loads: the export converted at
  `charging_efficiency` — the EV's single efficiency field, already used
  for both directions in the existing discharge budgets (`:996`,
  `:1078`); the Battery's `charge_efficiency`/`discharge_efficiency`
  split is a deliberate divergence not adopted (new config surface the
  root cause does not require) — and the heater's DC draw, unconverted
  (pack-side load). A cold V2L step conserves energy: pack debit =
  export/η + heater; port = −export; heater = heater — balanced, where
  the earlier `charger + heater` port rule and `export + heater` debit
  together would have double-counted the heater across the boundary (the
  inconsistency that forced the placement correction). **And the floor
  invariant holds (C1-rev12):**
  the discharge budgets (`:995-997`, `:1077-1078`) cap the *export* to
  `(soc − effective_floor) · capacity · η / dt` — a cap whose own comment
  states its purpose: no floor landing overshoots. With a second
  pack-side load, that cap alone would overshoot the floor by the
  heater's draw each step, and the pre-discharge gate (`:984`) stops only
  the export — so the fix makes the budget cover the **total pack-side
  draw**: the export cap is computed over the available headroom *minus*
  the heater's draw, with the heater taking headroom priority below the
  chargeable temperature (the discharge-side mirror of the charge-side
  rule — preconditioning is the precondition for any future recharge)
  and the export otherwise; and at the floor both stop (the gate bullet
  above). The pack never lands below `effective_floor` — not past
  `v2l_soc_reserve`, not past `ready_by_soc`, not toward zero. The
  flagship V2L case — a cold-weather outage with the vehicle backing up
  the home — is exactly the collision this bounds.
- **Supply cap and priority — boundary disambiguated (C3), composition
  corrected (C1-rev15):** the **bound** on the charger's AC input is
  `min(rated_power_kw, power_limit_kw)` at home — which covers charging
  plus the heater's AC-equivalent
  (`heater_dc / charging_efficiency`) behind the conversion; the away
  branch uses the away charger's rating — and **the DR fraction is a
  multiplier on the allocated total, exactly as the code applies it
  today (`power · dr_power_fraction()`, `ev/mod.rs:829`), never a term
  in the `min`.** The earlier `min(rated, power_limit, dr·rated)` formula
  is withdrawn: it agrees with the multiplier only at full demand and
  would have silently weakened dispatch obedience in exactly the
  derated regime this fix creates — a `High` event with no effect on a
  vehicle already below half its rating, `Critical` delivering twice the
  allowed power. Both guarantees survive under the corrected
  composition: a commanded zero (`GridEmergency` 0.0, or
  `power_limit_kw = 0`) zeroes the port, and moderate curtailment scales
  the whole AC draw proportionally. The priority rule swaps **exactly at
  `min_charge_temp_c`**: the heater has budget priority only while
  `battery_temp_c ≤ min_charge_temp_c` — the regime where charging power is
  *physically zero* (derate = 0) — and charging has budget priority in the
  derate band above it, with the heater taking the remainder; above
  `heater_threshold_c` the heater is off. **The invariant this encodes: the
  heater never consumes dispatch budget that charging could have used.** A
  commanded `power_limit_kw = 1.0 kW` on an 11.5 kW vehicle therefore never
  produces a heater-monopolized blackout: below the cutoff, charging is zero
  with or without the heater (spending the budget on warming is the only
  path to ever charging); in the derate band, charging draws first. The
  ticket's blackout is not reintroduced by either route — the rating-scaled
  default (C1-rev2) or the dispatched limit (C3). The cap-binding test
  asserts the invariant. The cap's binding condition is stated so it is
  tested, not discovered: with a threshold below `full_power_temp_c` the
  derate at threshold is 0.5, so the cap binds when
  `heater_dc / η > 0.5 · rated` — `heater_dc > 0.45 · rated` at the
  default η = 0.9 (5.18 kW on an 11.5 kW EVSE; the pre-R14 figure
  `heater > 0.5 · rated` omitted the η inflation); with
  `heater_threshold_c ≥ full_power_temp_c` (config-reachable) any
  heater binds.
- **Dispatched limits:** the heater participates in `power_limit_kw` and the
  DR fraction through the shared supply bound — a commanded 0 kW holds on
  the port. A deliberate divergence from the Battery (which has no DR or
  limit machinery), documented with rationale: dispatch obedience is the
  EV's control contract. **Justification with the corrected τ (C1):** a pack
  *can* fall below the cutoff under full curtailment — `GridEmergency ⇒
  0.0` (`ev/mod.rs:1090-1097`) is a representable commanded zero, and from
  5 °C at −7 °C ambient the cutoff is reached in ~4.8 h at τ ≈ 8.9 h — but
  that is the *commanded* behavior (a 0 kW dispatch, obeyed), not the
  ticket's defect (unable to charge while allowed); recovery after the
  event is bounded, and faster with the pack-duty heater class: a 3–5 kW
  heater rewarms the 5 K climb from the cutoff in ~0.15–0.5 h (~2.5 MJ ÷
  3–5 kW) and the ~12 K climb after a longer event in ~0.3–1 h before UA
  losses; and ordinary curtailment levels still leave a workable thermal
  budget — recomputed under the multiplier semantics (C1-rev15): routine
  DR **scales** the heater's draw proportionally — `High` (0.5) halves a
  3–5 kW heater's AC-equivalent to 1.67–2.78 kW, `Critical` (0.25)
  quarters it to 0.83–1.39 kW — and the steady-state hold (a pack at 5 °C
  against −7 °C at UA ≈ 15.7 W/K) needs ≈0.19 kW DC ≈ 0.21 kW AC,
  comfortably inside even `Critical`'s scaled allocation — so the heater
  is throttled, not starved, the pack is held or slowly recovering
  through routine DR, and only a commanded zero (`GridEmergency`, an
  explicit low `power_limit_kw`) suspends thermal management entirely.
  **The two discharge legs' DR composition is unchanged and stated:**
  V2G and V2L apply the fraction as a multiplier on the exported power
  (`ev/mod.rs:593`, `:595`) today, and this plan does not modify that —
  the heater's pack-side draw during discharge is bounded by the floor
  invariant, not by the DR fraction.

### Charging-curve LUT × linear derate composition (W1) — decided

`compute_charging_power_kw` (`ev/mod.rs:664-676`) applies two independent
temperature-dependent factors multiplicatively when a charging-curve LUT
is configured: the LUT's own temperature axis
(`rated × lut.interpolate(&[soc, battery_temp_c, c_rate, soh])` — a
genuine N-D interpolation axis) and then the linear BMS ramp
(`derated_rated = curve_limited_rated × charge_derate`). A pre-existing
broken window in a function this fix already opens, acknowledged nowhere
before this round. **The decision:** the LUT field is injectable from
config, the `Equipment` trait, and Python precisely so a sophisticated
user can supply a *measured* vehicle charging curve — and a measured
curve already contains the manufacturer's low-temperature derate — so
when a LUT is present, its temperature axis **is** the
temperature-dependent charge capability and the linear ramp's derate is
**not applied on top of it** (one physical effect, applied once).
**The safety floor survives unconditionally:** the `min_charge_temp_c`
cutoff still zeroes charge power below the plating boundary regardless of
the LUT — a curve fitted only above the cutoff (the normal measurement
regime) cannot grant permission to charge below it, and the
interpolator's out-of-bounds strategies include clamping to the axis
bound (`ndinterp.rs`), which must not open that window under any
configured strategy. The composition and its rationale are documented in
the code where the two factors meet, per the constitution's
divergence-documentation rule; a unit test pins it — a cold pack with a
temperature-axis LUT charges at the LUT's value, not the product, and
still zeroes below `min_charge_temp_c`. The seven LUT tests are already
in the named 13-test exposure list and are re-derived under the triage
rule either way. The path is opt-in (no catalog entry ships a LUT;
`hares-io` never sets one; the constructor defaults to `None`), so no
gate depends on it — a broken window closed while the function is open,
not new contract scope.

### Mode classification and vars: the billing rule's traced consumers (C1, W1)

Putting heater draw into `active_power_kw` changes the meaning of a
quantity with sixteen readers in `ev/mod.rs`. The intended ones — the
port contribution (`:1408`), the `ACTIVE_POWER_KW` telemetry (`:1141`),
the checkpoint fields (`:1516`, `:1559`), and `CoreFlows::electric_kw`
(`:1456`, `:1610` — a flows structure should carry total port power) —
see exactly what they should. Two consumers were never traced and are
fixed with the rule rather than after it:

- **Operating mode (C1):** three sites classify mode from the sign of
  `active_power_kw` (`ev/mod.rs:1342-1345`, `:1447-1451`, `:1601-1605`),
  so a preconditioning pack — charge power exactly zero, heater drawing —
  reports `OperatingMode::Charging` to the output column (`Mode (-)`,
  `hares-io/src/output/columns.rs:102`) and the mode-cycle parity metric,
  in the default configuration, in the exact scenario under repair: the
  same conflation this plan already fixed in the test metric, in
  production code, unfixed. **The classification moves to the charge
  leg — the pack-side net charge rate (C1-rev14), never the AC input's
  sign:** net > 0 → `Charging`; net < 0 (exporting) → `Discharging`;
  net zero with the heater drawing — the heater-only state, where the
  AC input is raised to `heater_dc / η` but the cells net exactly zero —
  → `OperatingMode::Heating` — the
  shared enum's thermal-production mode
  (`hares-types/src/equipment.rs:218-233`), literally true of a pack
  heater, satisfying the guard's active-mode flow rule through the
  nonzero electric draw and not tripping its thermal-sign rule (the EV
  emits no `thermal_output_w`); all zero **including vars** → `Off`;
  charge zero, heater zero, but commanded vars nonzero → `On` — the
  inverter is actively exchanging reactive power (standby var support, a
  normal smart-inverter mode), an active state and the honest label once
  the guard's Rule 1 counts reactive flow (below). All three sites,
  consistently.
- **The mode-flow guard — analyzed rule by rule, one claim withdrawn
  (C1):** `check_mode_flow_consistency`
  (`hares-types/src/mode_flow_guard.rs`, invoked unconditionally from
  CoreOutput validation at `hares-types/src/equipment.rs:1601` as a hard
  error). **Rule 4's re-keying is withdrawn — the defect never existed.**
  Revisions 10–11 claimed the direction rule (Charging must have net
  consumption, Discharging net generation) breaks in the
  V2L-with-heater corner; read at its match arms, Rule 4
  (`mode_flow_guard.rs:146-159`) applies **only to the unidirectional
  `Consumption`/`Generation` variants**, with a deliberate exemption —
  the comment names "battery with standby load", the same shape — for
  `Bidirectional`, which is exactly what the EV emits at both CoreOutput
  sites (`ev/mod.rs:1456`, `:1610`, as the Battery does at
  `battery/mod.rs:1600`/`:1760`). The corner is permitted by design; the
  earlier claim rested on the rule's doc-comment summary adopted without
  reading the arms — the constitution's "confirm the bug exists as
  described; if it never existed, stop and document that", and the same
  failure this plan corrected in others. No direction-rule change; no
  mechanism fork; `hares-types` keeps only what the fix requires. **Rule
  2 stands (W1-rev10):** the Off-mode zero-flow guard
  (`mode_flow_guard.rs:85-111`) counts `reactive_power_kvar` as flow
  with no `Bidirectional` exemption — `Bidirectional(0.0)` is zero, so
  an EV commanded to vars at zero real power (plugged in, at target,
  heater off; `q_setpoint_kvar` from `ReactiveSetpoint` or
  `PowerSetpoint.reactive_power_kvar`) classifies `Off` with nonzero Q
  and **fails the simulation**: pre-existing, reachable by control, in
  the opened guard — fixed here with the classifier's var leg above.
  **Rule 1's `has_flow` gains the reactive term — required by that
  classifier change** (`On` backed only by vars would otherwise fail
  Rule 1's flow test) and correcting a real internal asymmetry (Rule 2
  counts reactive; Rule 1 does not). Its reach is stated honestly: the
  edit is one line, but it **relaxes a workspace-wide invariant** for
  every reactive-emitting equipment (PV, Battery, Generator, EV) —
  correct physics for all of them (an inverter supplying vars at zero
  real power is genuinely active), with the guard's tests updated. The
  blast radius keeps `mode_flow_guard.rs` and the `equipment.rs`
  validation call site for these two changes and the doc-comment update.
- **Vars (W1):** `compute_reactive_kvar` (`ev/mod.rs:1121-1136`) uses
  `active_power_kw` twice — the power-factor baseline (`P · tan φ`) and
  the kVA headroom clamp (`√(S² − P²)` against `charger_capacity_kva`).
  A PTC pack heater is resistive, unity-power-factor, not
  inverter-coupled: it produces no vars and consumes no inverter kVA
  headroom, yet the billing rule feeds it into both — overstating the
  baseline 25–40 % for a 3–8 kW heater against an 11.5 kW charger while
  understating var capability, two errors in opposite directions, the
  signature of a quantity changed without its consumers traced. **Both
  uses key on the inverter leg — the charger's AC input** — which under the
  pack-side heater placement is exactly right in every state: the DC-fed
  heater never touches the inverter, so it contributes no vars and
  consumes no kVA headroom whether the vehicle is charging, idle, or
  exporting. The default path is
  inert (`power_factor` defaults to 1.0; HPXML resolution passes `None`),
  but `power_factor` and `q_setpoint_kvar` are exactly the
  grid-services controls this plan's dispatch-obedience rationale exists
  to protect — an explicit var command must not have its headroom eaten
  by heater watts.
- **Coverage:** a regression asserts the preconditioning state — cold
  pack, heater running, charger AC input raised to the heater's
  AC-equivalent, cells netting exactly zero — reports `Heating`, not
  `Charging`
  (through the output column), and that the V2L-with-heater corner
  stays exempt — pinning the `Bidirectional` design that permits it, so
  a future guard edit cannot silently re-key it; a second asserts a commanded
  `q_setpoint_kvar` on a satisfied, warm, plugged-in EV reports a
  non-`Off` mode and produces no guard violation.

`island_source_available` keys on `v2l_active` and connection state, not
power magnitude — unaffected.

**`Heating` reuse — grounded, with its boundaries recorded:** the variant
is the HVAC space-heating mode, and two dwelling-level classifiers sweep
heating variants into per-zone state (`hares-core/src/diagnostics.rs:938-942`
sets `zone_has_heating`; `dwelling/mod.rs:4217-4223` writes
`setpoint_heat_c`) — but both are zone-guarded (`diagnostics.rs:931`,
`dwelling/mod.rs:4213`) and the EV's descriptor carries `zone: None`
(`ev/mod.rs:212`), so the EV never reaches them; `dwelling/mod.rs`
further requires a setpoint the EV does not emit; `is_heating_variant`'s
only consumer is the guard's Rule 3, inert for the EV (no
`thermal_output_w`); and `MODE_ORDINALS` needs no new entry for a reused
variant. Three boundaries, recorded rather than silently held:
`is_heating_variant`'s doc comment ("delivers positive into-zone thermal
power") becomes imprecise for the EV and is updated with the boundary
(thermal production at the equipment level; into-zone delivery only for
zoned equipment); the reuse **acquires the no-zone invariant** — giving
the EV a zone (a garage, say) would put a preconditioning pack into the
dwellings' zone-heating classifiers while emitting no zone thermal
power, so that invariant is documented in the alignment comment; and the
`Mode (-)` code 1 is shared between space heating and pack
preconditioning — per-equipment columns disambiguate, and a consumer
reading mode codes across equipment types should know. **`On`'s consumers
are enumerated too:** ventilation, event loads, HVAC fan power, and the
guard's own Rule 3 allowances — no dwelling-level classifier keys on it,
so nothing is misled by an EV reporting it during var support.

### Drive-event I²R heating — in scope

The `EvDrive` handler (`ev/mod.rs:1820-1850`) debits `kwh` from SOC and
touches no temperature — negligible at τ ≈ 1.4 h, and **still material
after D1 with the corrected τ (C1): at τ ≈ 8.9 h, ~63 % of an 18:00
commute's heat remains at the 22:00 window start** (weakened from the ~89 %
the discarded-UA figure implied, but single-digit kelvin on the 0–10 °C
derate ramp is still the difference between a chargeable window and a
blackout). Same class as D2; fixed with the same machinery: the drive
energy's equivalent discharge current through the pack electrical model
feeds the unified thermal equation.

### Working preconditioning by default — pack-duty class, with a working
verifier

The cold-snap contract cannot pass with a 0 W default heater. **The default
is sized to the *pack preheating* duty, not the cabin duty (C2):** industry
practice separates them explicitly — PTC pack preheating on large-format
packs is kilowatt-class (trade material indicates 3–8 kW; cabin heating is
the separate 1.5–3 kW duty) — so the earlier "several hundred W to ~2 kW"
class was the cabin figure and is withdrawn. The exact value is a merge
condition sourced properly at fix time (the trade figure is indicative, not
authoritative), but the *class* is fixed: kilowatt-class pack preheating.
**Consequence, accepted and reframed: with a pack-duty heater the supply
cap is the normal control path for cold charging sessions, not an edge
case** — on 11.5 kW L2 the bound's binding condition
(`heater_dc / η > 0.5 · rated`, i.e. `heater_dc > 0.45 · rated` at the
default η = 0.9) is met by a ≥5.18 kW heater — a mid-range 5 kW pack-duty
heater binds, strengthening this claim — and
in the derate band the heater and charger share the bound routinely; the
cap-binding coverage below is therefore *primary* coverage, and every
slack-cap argument in this plan is scoped to the warm-pack case. The
earlier fraction-of-rating default is **dropped**: the cap protects every
circuit rating better than a shrunken default does — on L1, a realistic
heater throttled to the 1.4 kW circuit warms the pack quickly and then,
above the threshold, *gets out of the way*, leaving the whole circuit to
charging, whereas a fraction-scaled heater paces its warming across the
whole window and starves charging throughout; and an L1 circuit in
sustained deep cold is structurally energy-short regardless (1.4 kW × 8 h
= 11.2 kWh against a 9.75–14.4 kWh daily need), which is real physics the
L1 regression records rather than fixes. Explicitly configured heater
power exceeding the circuit is throttled by the cap, not rejected; the
reproduction's 1.5 kW probe remains accepted (explicit config is honored
within the cap).

**Constant-power heater — declared simplification (W1):** the model treats
the heater as constant-power at its configured rating. Real EV pack
heaters are predominantly PTC elements that self-regulate — power falls as
the element warms, with trade figures around 12 % lower average consumption
than a constant-power element of the same rating. Constant-power is chosen
deliberately: PTC power curves are element-specific and not generically
sourceable, the error direction is known and bounded (overstatement of
draw — conservative for the grid-load studies the billed heater draw
feeds), and the divergence is documented with this rationale in the
alignment comment. An unexamined default it must not be.

**Threshold:** default in the 5 °C class. **Citation (W4, line corrected):**
the Battery catalog's `heater_threshold_c` values are exactly **0.0 and
5.0**; the "5–10 °C" range is the comment at **`battery/mod.rs:132-133`**
("Tesla Heat Mode targets 0 C minimum cell temp; Franklin activates around
5-10 C"), directly above `DEFAULT_HEATER_THRESHOLD_C` at `:134` — cited as
in-tree secondary material, and the independent support is stronger than
the in-tree argument: industry guidance recommends keeping charge
temperatures above ~5 °C for cell health, not merely above the 0 °C safety
floor, which the fix step cites as the primary reference (observation). The
stationary catalog is admissible for the *threshold* but not the heater
*power* because the threshold is a chemistry-shared cell-temperature
phenomenon while heater power is application-specific.

**Quantitative constraint:** ~14.4 kWh/day through an 8 h window at
11.5 kW·η ⟹ ≳2 kW average ⟹ derate ≳ 0.17 ⟹ sustained pack temperature
≳ 1.7 °C *as a lower bound* (the CC-CV taper limits power further in the
band the recipe finishes in; recorded so the number is not quoted as
exact). The 5 °C-class threshold clears it with margin.

**The verifier:** the charge-day metric is corrected to **isolate charging**
(a charge-day is a day the pack's stored energy actually rose; the exact
observable is the fix step's, the isolation requirement is not), scooped
into **one shared test helper** replacing the byte-identical copies in
`ev_thermal_regressions.rs` and `equipment_id_regressions.rs` and the
variant in `i07_diag.rs`. **The helper's verbosity floor is documented
(observation):** the SOC column is emitted only at `output_verbosity ≥ 3`
(`dwelling/mod.rs:448`, `columns.rs:1105-1112`), all four current call
sites run verbosity 5, and the helper's doc comment names the floor so a
future below-3 call site fails legibly on the requirement rather than on a
mystery missing-column panic. The new mechanisms get their own coverage:

- **An L1 cold regression with a discriminating contract (W2):** at a
  *marginal* cold night (−2 °C class — where an L1 circuit can physically
  work), the vehicle must accept charge energy above a floor derived from
  the physics class (window-average delivered power at or above a stated
  fraction of the 1.4 kW rating) — a bound a degenerate choice fails: a
  heater monopolizing the budget, or a mass/UA pair that flash-freezes the
  pack, both go red. At *sustained* extreme cold (−7 °C), the invariants
  are asserted — the cap holds (port draw never exceeds the rating), the
  heater runs within its budget, charging is never silently zero while
  allowed — and the resulting behavior is recorded as a probe, because an
  L1 vehicle in a sustained −7 °C snap genuinely cannot meet a daily drive
  in reality, and the test does not pretend otherwise. The test's
  assertions constrain; the probe records.
- **Cap-binding coverage — primary, not corner (C2):** with a pack-duty
  heater the cap binds in normal cold operation, so its tests assert the
  clamped total draw, the priority invariant (the heater never takes budget
  charging could have used), the hand-off — the heater warms the pack
  through the cutoff, charging takes the bound in the derate band with the
  heater throttled to the remainder, and the heater stops above the
  threshold — the cold charging session's normal path, exercised as such —
  **and the DR multiplier semantics are pinned (C1-rev15): a `High` event
  halves the allocated draw of a demand already below half the rating —
  exactly the derated-regime case where the withdrawn ceiling form would
  have had no effect — so the semantics cannot silently change again.**
- **Raised-command bound regressions:** the corrected taper bound lands
  the pack exactly on its commanded target with the heater running (no
  overshoot, no shortfall), and a commanded `power_setpoint_kw` is never
  exceeded by the raise — the heater is carved out within it.
- **The rewritten `heater_draw_slows_soc_gain` invariant carries its
  precondition:** the additional-load identity ("at the same pack-side
  charge rate, SOC gain equals the no-heater case and port power is
  higher by the heater's AC-equivalent") holds **while the cap is
  slack**; at the binding cap, SOC gain is
  deliberately not equal — the clamp is asserted separately.
- **A V2L cold-export conservation test:** a discharging cold pack's SOC
  debit equals export at `charging_efficiency` plus the heater's DC draw
  (unconverted — pack-side), with the port carrying the export alone;
  and the floor regressions — a cold V2L export at the cap with the
  heater running never lands below `effective_floor`, and at the floor
  both the export and the heater stop.

### D1 — thermal mass and UA, with the constraint arithmetic stated (C1, C2)

- **Thermal mass** derives from **`battery_capacity_kwh_rated` via the
  physics route (W2): pack mass × cell specific heat** — the RCA's cited
  479 kg pack mass × the literature's 800–1100 J/(kg·K) Li-ion specific
  heat → ≈383–527 kJ/K (≈480–500 kJ/K for 75 kWh at the band's middle).
  The Battery's 90 kJ/K per 13.5 kWh (OCHRE `Battery.py`) is demoted to
  what the constitution says OCHRE is — a cross-check, not a specification
  — and the per-kWh density is a derived convenience, not the source. The
  number is unchanged; the citation chain now names the primary.
- **UA is re-derived, and the pair's constraint is stated before it is
  hit (C2):** at the plan's mass density (~500 kJ/K), the parked-pack
  regression — 20 °C start, −7 °C ambient, 8 h, must stay above 0 °C —
  implies a **UA ceiling of ≈24 W/K** (at 25 W/K the pack lands at −0.6 °C
  and the test fails); at the plan's own area-scaling figure
  (`5.0 × (75/13.5)^(2/3)` ≈ 15.7 W/K) the margin is 3.9 K. The test is
  therefore a **known two-sided constraint on the (mass, UA) pair**, not a
  loose order-of-magnitude check — stated here so the fix step meets it as
  a constraint rather than discovering it as a red test with two tempting
  outs. If a proper derivation lands above the ceiling, the **pair** is
  re-derived jointly (mass and UA together set the trajectory); widening
  the test is not an available move.
- **Which condition UA is derived for (C2):** the parked/plugged-in regime
  — the charge window is a parked vehicle in still air, where the film
  coefficient sits at the low end (consistent with the ceiling), and it is
  the regime every charging-physics assertion in this contract exercises.
  Road-speed convective boost during the drive is deliberately **not**
  modeled — a documented modeling boundary, conservative for the
  cold-charge problem (it can only overstate how cold the pack arrives).
- **τ, recomputed from the adopted UA (C1):** τ = C/UA ≈ 500 kJ/K ÷ 15.7
  W/K ≈ **8.9 h** — a planning figure from the scaling rule, restated as a
  range pending the cited value — *not* the ≈33 h figure the RCA computed
  with the 4.0 W/K UA this plan discards. Every τ-dependent argument above
  uses the corrected figure, and the five τ-derived numbers in this plan
  (the UA ceiling, the parked-pack margin, the drive-heat retention, the
  initialization window, the DR fall time) are **readings of this one
  (mass, UA) pair** — they move together when the cited value lands and are
  re-checked together.
- **Initialization transient:** superseded by ambient-aware initialization
  (below, C2) — with the pack starting at ambient (or explicit config),
  the 20 °C construction transient applies only to the no-ambient fallback
  path, and the scenario initialization-window check narrows to that path.
- **Thermostat overshoot (observation):** the heater plausibility bound's
  second job (explicit-Euler overshoot — at a 3–5 kW pack-duty heater into
  ~500 kJ/K at 900 s, ~5.4–9 K/step if ungated) moves with the final
  (mass, UA) pair and the sourced heater value; with a kilowatt-class
  default the overshoot question is live, and the gate needs hysteresis or
  sub-step clamping if the fix step finds overshoot matters — one of the
  bound's stated reasons.

### Ambient-aware initialization (C2) — in scope, same class as D1–D3

The EV's `init` binds ambient to `_env` and discards it
(`ev/mod.rs:1313`), setting pack temperature unconditionally from
`DEFAULT_BATTERY_TEMP_C` at both construction paths (`ev/mod.rs:357-359`,
`:519`) — while the Battery threads `env` and initializes cell temperature
by cascade (`battery/mod.rs:1081-1090`): explicit config wins, else the
zone temperature when the equipment sits in a zone, else outdoor ambient.
That is another row of the "EV thermal sub-model diverged from the
Battery's correct architecture" class, in files this fix already opens,
and **D1 makes it material**: at today's τ ≈ 1.4 h a wrong initial
temperature is forgotten within hours; at the post-fix τ ≈ 8.9 h it
persists past a day. The fix adopts the Battery's cascade: explicit
`battery_temp_c` config wins, else zone temperature, else outdoor ambient,
with `DEFAULT_BATTERY_TEMP_C` as the true fallback only where ambient is
genuinely unavailable — which also makes its doc comment
(`ev/config.rs:65-68`, "when ambient conditions are unavailable at
construction time") true for the first time; today it describes a path
that does not exist (the observation's broken window). The equipment-level
regressions that pin specific start temperatures set them explicitly via
config — the parked test's `probe_ev(Some(20.0), …)` and the heater test's
`probe_ev(Some(-5.0), …)` — so their premises are unchanged. The cold-snap
scenario becomes *stronger*: the pack starts at the −7 °C ambient with no
20 °C head start, and the contract (≥30 charge-days) must be met by
preconditioning alone, which the pack-duty heater class delivers (~12 K
in ~0.3–0.6 h at 3–5 kW). **One interaction this plan owns, found in
reassessment:** ambient-aware initialization arms the heater in the cold
parity fixtures — their EVs are real `Ev` equipment built from
`PlugLoadType: electric vehicle charging`
(`hares-io/src/hpxml/resolve_loads.rs:868+`, matching OCHRE's `parse_ev`),
defaulting to `HomePluggedIn` with no configured temperature, so a
cold-fixture pack starts at ambient, below the threshold, and the default
heater fires — the forced fixture policy in Verification handles it.
Two design facts verified during critique and recorded so the fix step
does not re-derive them: `init` receives a populated environment — the
dwelling calls it with `initial_env` from `environment.update(&clock,
&[])` (`dwelling/mod.rs:2331`, `:2005`), so ambient-aware initialization
reads real weather, never a zeroed state; and checkpoint ordering is safe
— equipment `init` runs before `load_state` (`dwelling/mod.rs:4683`,
documented at `:4806-4808`), and restore overwrites `battery_temp_c`
from the checkpoint (`ev/mod.rs:1549+`), so a restart keeps the saved
thermal state rather than resetting it to ambient. One premise tightened
(observation): the L2 probe passes `probe_ev(None, …)` and survives
ambient-aware initialization only because its `env_at(20.0, …)` ambient
equals the very default being removed — the fix step pins its start
explicitly (`probe_ev(Some(20.0), …)`) so the test's documented "from a
20 °C start" premise no longer rests on that coincidence.

### Class fix — degradation domain guard

`DegradationState::accumulate` (`battery/degradation.rs:252`) gains the
guard with a **signature change to `Result`**; both call sites
(`ev/mod.rs:1239-1241`, `battery/mod.rs:1556-1559`) propagate it.
**Rationale rebuilt on the sourced validated domain (C1):** the model's own
provenance (`degradation.rs:1`) is Smith et al. 2017 (IEEE 7963578), whose
aging tests span **0 °C to 55 °C** — that is the model's accuracy boundary,
and the earlier "non-physical vs out-of-calibration" distinction is
withdrawn: for a model whose parameters were regressed over a stated range,
out-of-calibration *is* the concern. The guard therefore has two layers,
stated separately: (1) **error bounds** — the physically implausible
envelope outside which no simulated pack can exist (the D2-class 440–550 K
excursion, and a symmetric cryogenic floor), which error loudly; (2) the
**validated domain 0–55 °C**, below which HARES operates deliberately and
with owned inaccuracy — a negative `EA_B2` makes the fit extrapolate badly
downward (cycle aging ≈8× the 25 °C rate at this contract's −7 °C, ≈32× at
the Battery tests' −25 °C, worse below), and erroring there would refuse
to simulate the exact cold-climate regime this initiative exists to model.
The disposition is documented inaccuracy, not silence: the domain, the
extrapolation factors, and the rationale (HARES must simulate cold
climates; this fix moves plugged-in packs *into* the domain for the hours
that matter — a preconditioned pack at the 5 °C threshold is in-domain;
the extrapolation applies to parked/driving/unheated hours) are recorded in
the alignment comment. **Fidelity claim upgraded (observation):** the model
is the NREL lineage EnergyPlus itself reaches —
`ElectricLoadCenter:Storage:LiIonNMCBattery` delegates to NREL's SSC
battery library (`vendors/EnergyPlus/src/EnergyPlus/ElectricPowerServiceManager.cc:3673-3677`)
— so the degradation model family is the constitution's fidelity target's
own family, stated here rather than justified by in-tree precedent alone.
**Merge condition added (C1's named outstanding check):** the fix step
verifies the `deg_const` activation energies against Smith 2017's tables
(the critique could not — paywalled); the domain number rests on the code's
provenance claim being true.

### Deliberate consequence — preconditioning shifts modeled degradation (W2)

Holding a cold-climate pack at the 5 °C-class threshold instead of letting
it track −7 °C ambient moves the degradation model's dominant driver —
cell temperature — deliberately. The three Arrhenius terms do not move
together (`EA_B2` is negative — "negative ⟹ faster at lower T",
`battery/degradation.rs:144`): at +5 °C vs −7 °C, `b1` (calendar) runs
≈1.99× faster, `b3` ≈2.30× faster, and `b2` ≈0.43× slower — for every
plugged-in EV in a cold climate, for the heating season. **This shift is
accepted as correct physics, stated rather than unnoticed:** a real
preconditioned vehicle ages differently than a frozen one, and the model
is now exercised in the regime real cold-climate vehicles operate in.
What the RCA named as the credible tail mechanism (§4) was capacity
*whipsaw* — 75 → 208 → 75 kWh day over day, out-of-domain — not a smooth
shift, and the distinction is the contract: **a permanent regression
asserts the whipsaw pathology stays gone** — `CAPACITY_KWH` telemetry
stays within a physical band of rated and day-over-day changes stay
bounded, a bound any calibrated degradation model passes and only the
out-of-domain pathology fails. This preserves the reproduction's
deliberate decision not to assert fade *values* (calibration is not
over-constrained) while keeping the tail mechanism observed after
`i07_diag.rs` is folded — it is currently the suite's only reader of
`capacity_fade_pct` (`i07_diag.rs:413`, `:610`, `:634`, `:724`). The
cold-snap scenario's measured fade trajectory is recorded in the fix
evidence — a probe, not an assertion. The guard and the heater are
related, and stated as related: both move the degradation model's input;
while preconditioning, the heater moves cell temperature *into* the
validated 0–55 °C domain, and the extrapolation below it is the owned
inaccuracy the guard section records.

### Override silent no-op — in scope, probe disposition deliberate

At the assembly boundary where the equipment population is known
(`dwelling/conversions.rs`, the `apply_equipment_overrides` callers at
`:786`/`:816`), an overrides key that is neither the `"all"`/`"*"` wildcard
nor an existing equipment name is rejected with a typed error naming the
key and the available names. Regression test through the real assembly
path: an override keyed by a fresh SystemIdentifier-style name errors
loudly. **Deliberate disposition of the RCA's override-miss probe**
(`i07_diag.rs:757`, `i07_ticket_scenario_override_keyed_by_system_id` —
verified to exist): the permanent loud-error regression subsumes it; the
probe dies with the fold, and the RCA §1/§7 elimination of the identity
hypothesis thereafter rests on the historical pre-fix record — recorded so
a future reader finds the probe gone *with* the explanation. Existing
override consumers are audited for silent no-ops they relied on.

### Class audit

| Class shape | Instances | Disposition |
|---|---|---|
| Conversion loss injected as heat into a thermal mass | EV only (PV/HPWH/Battery verified clean) | Fixed by D2 |
| Billed load gated on a sibling power-positive branch | EV heater only | Fixed by D3's gate |
| Power through pack resistance without I²R heating | EV charge leg **and** drive path | Fixed by D2 + drive heating |
| Sub-load billed in one state-machine arm but not another | EV heater on the non-discharge leg only | Fixed by the billing rule; all three arms + outage gate swept |
| The EV discards ambient at init while the Battery threads it (`ev/mod.rs:1313` vs `battery/mod.rs:1081-1090`) | same D1–D3 divergence class, in files the fix opens | Fixed by ambient-aware initialization (C2) |
| A pack-side sub-load outside the discharge floor-energy budget | the heater's draw during V2L/V2G, once the gate opens it (C1-rev12); the Battery is checked and clean — its `clamp_power` is a hardware power clamp and its floor is enforced elsewhere, no per-step energy-budget shape | The discharge budget covers the total pack-side draw; the heater stops at `effective_floor` with the export |
| One physical effect applied twice (LUT temperature axis × linear BMS ramp) | `compute_charging_power_kw` (`ev/mod.rs:664-676`), opt-in path, unacknowledged | LUT axis replaces the ramp's derate; the safety cutoff survives unconditionally (W1) |
| Degradation evaluated with unguarded cell temperature | EV and Battery call sites | Fixed by the shared guard |
| An observable or classifier that an accounting change makes non-discriminating | test: `summarize_ev_run` ×3; production: the three `OperatingMode` sites and the var model's two uses of `active_power_kw` | Test: one shared charging-isolating helper. Production: mode from the charge leg with `Heating`/`On` states, Rule 1's reactive symmetry and Rule 2's fix, vars keyed to the inverter leg; the Rule 4 re-key withdrawn — the defect never existed (C1) |
| A dispatched limit applied to part of a load but not the rest | EV heater outside `power_limit_kw`/`dr_power_fraction` | Fixed by the shared supply bound with the swap-at-`min_charge_temp_c` priority invariant |
| An existing observable silently redefined | `away_charge_actual_kw` (documented pack-intake meaning, three asserting tests) — caught in planning, not in review | **Not redefined**; `HEATER_POWER_W`'s *emission* becomes value-carrying actual draw (post-cap) — aligned with its Battery twin and its own meaning; nameplate-gated-on-a-bool is the defect once throttling exists (W1) |
| Unit tests pinning defective physics | `ev/tests.rs` ~3619, `heater_draw_slows_soc_gain` ~519 | Rewritten with preconditioned invariants; `battery_thermal_exponential_decay` stays |
| Uncited defaults in touched files | the 17-constant `ev/config.rs:47-72` block | Every constant sourced; `DEFAULT_MIN_CHARGE_TEMP_C` gets a primary source |

## Deliberately out of scope, with routing

- **D4 — driver derate blindness** (`hares-core/src/actors/ev_driver/`): a
  behavioral decision with no verifier in this contract; the fix step
  confirms from the cold-snap telemetry that the driver's estimates produce
  no wrong behavior in the exercised scenarios; a tracking ticket is
  created in `.shipwright/initiatives/I-07/tickets/` before the fix is
  marked done.
- **Degradation recalibration beyond the guard:** the model stays in its
  calibrated domain once temps are physical; the reproduction deliberately
  asserts the 40 °C bound instead of non-negative fade.
- **Zone coupling — considered, deliberately divergent:** the Battery emits
  ohmic loss to its zone; the EV's pack is outdoors/garage — no zone gain,
  documented in the alignment comment.
- **The Battery's η-plus-I²R approximation:** pre-existing, small, inherited
  by alignment, documented — not re-derived.
- **Sub-zero degradation extrapolation:** operating the Smith 2017 fit
  below its 0 °C validated floor is pre-existing (HARES runs it at −7 °C
  and −25 °C today), inherited, and *reduced* by this fix — preconditioning
  moves plugged-in packs into the domain for the charging hours that drive
  the ticket's arithmetic — with the extrapolation factors and rationale
  recorded in the guard section and the alignment comment. Recalibrating
  the fit for sub-zero operation has no in-tree data source; it is a
  degradation-research dependency documented here, not fixable work
  silently deferred.
- **A charging-EV parity fixture:** no corpus fixture exercises a charging
  EV (measured — see Verification), and no OCHRE reference can exist for
  pack *thermal* behavior because OCHRE has no pack thermal state — the
  thermal sub-model is declared beyond parity's reach, stated here rather
  than silently assumed. A fixture cross-checking EV *charging* power
  against OCHRE requires OCHRE reference-generation infrastructure — a
  different subsystem; a tracking ticket is created in
  `.shipwright/initiatives/I-07/tickets/` before the fix is marked done.
- **Road-speed convective cooling during drive (C2):** deliberately not
  modeled — a documented boundary; conservative for the cold-charge
  contract.

## Alternatives considered and rejected (top level)

1. **Remove or lower the 0 °C cold-charge cutoff** — discards grounded
   physics OCHRE lacks; the RCA names it the wrong fidelity target (§6).
2. **Patch the constants only** — D1–D3 are coupled; D1 alone kills the
   self-heating rescue while preconditioning stays dead.
3. **Unify the EV and Battery equipment into one shared model** — a refactor
   no failing test demands; zero additional contract coverage.
4. **Fix it driver-side (D4)** — the equipment cannot charge regardless of
   what the driver knows; the equipment-level failing tests prove it.

Plus the pack-electrical parameterization alternatives (lumped fields,
empirical coefficient), both rejected with reasons above.

## Blast radius

**Touched:**

- `crates/hares-equipment/src/ev/mod.rs` — `run_charging_physics` (gate,
  limits-aware supply bound), `apply_soc_and_thermal` (unified equation,
  I²R, discharge debit including heater), all three `step` arms (billing
  rule, port destinations, mode classification from the charge leg at
  `:1342-1345`/`:1447-1451`/`:1601-1605`), `compute_reactive_kvar`'s
  inverter-leg input, the `EvDrive` handler (drive I²R),
  pack-electrical plumbing. The submodule extraction lands as a
  behavior-preserving, separately reviewable step (tests green across the
  move).
  move).
- `crates/hares-equipment/src/ev/config.rs` — new pack-electrical fields
  with validation; defaults re-derived/cited (thermal mass from rated
  capacity via mass × specific heat; UA from the parked-regime derivation
  inside the ≈24 W/K ceiling; heater power in the sourced pack-preheating
  class; threshold 5 °C class); all 17 constants sourced.
- `crates/hares-equipment/src/ev/catalog.rs` — per-model electrical
  parameters.
- `crates/hares-equipment/src/ev/telemetry.rs` — one new ohmic/I²R key
  (additive; no output-schema regression pins the EV column set per the
  critique's search — the fix step confirms this when adding the key,
  rather than treating the search as a guarantee), existing keys' meanings
  unchanged, and **`HEATER_POWER_W`'s emission becomes value-carrying**
  (W1): the actual post-cap heater draw instead of nameplate-gated-on-a-bool
  (`ev/mod.rs:1153-1160` today), mirroring the Battery's computed-`heater_w`
  emission — so the throttled draw is reported truthfully, including the
  commanded-zero case; `away_charge_actual_kw` unchanged.
- `crates/hares-equipment/src/ev/tests.rs` — the two defective-pin tests
  rewritten with preconditioned invariants, and
  `heater_only_grid_draw_when_fully_cold_derated`'s identity updated to
  the heater's AC-equivalent (`heater_dc / η` — the existing 0.5 kW 1e-9
  assertion fails on magnitude under the pack-side billing, its companion
  no-SOC-change assertion surviving); cap-binding and conservation
  tests as the harness allows. **Known exposure, named before it is met
  (W1):** ambient-aware initialization moves every test that does not set
  `battery_temp_c` explicitly — ~168 of the file's 188 — from the 20 °C
  default to `sample_env()`'s 10 °C outdoor ambient (`ev/tests.rs:19-31`).
  Inert for most (derate 1.0 at both; threshold below both), but **13
  touch temperature-computational paths and are expected to move**: the
  seven charging-curve LUT tests, where `battery_temp_c` is an
  interpolation axis (`ev/mod.rs:672` —
  `charging_curve_lut_limits_power`,
  `lut_tapers_charge_power_at_high_soc`,
  `no_lut_charges_at_full_rated_power`, `lut_with_soh_adjusted_c_rate`,
  `cc_cv_taper_omitted_when_lut_present`,
  `set_charging_curve_lut_via_equipment_trait`,
  `clear_charging_curve_lut_via_equipment_trait`) — these seven also
  exercise the LUT path the composition decision redefines, so they carry
  both changes at once — and the six
  degradation tests, where `cell_temp_k` feeds the Arrhenius terms
  (`ev/mod.rs:1237` — `degradation_starts_at_zero`,
  `capacity_fade_fraction_in_telemetry`,
  `degradation_state_survives_checkpoint`,
  `daily_degradation_rescales_usable_capacity`,
  `fixed_drive_soc_swing_scales_with_degraded_capacity`,
  `load_state_recomputes_degraded_capacity_from_rated_and_soh`); at
  10 °C vs 20 °C the calendar term runs 0.60×, `b3` 0.54×, and the
  negative-`EA_B2` cycle term 1.86×. **Triage rule:** a red test after
  this change is answered by re-deriving the expected value at the new
  input, or by pinning `battery_temp_c` explicitly when the test's
  subject is not temperature — never by loosening a tolerance.
- `crates/hares-equipment/src/battery/degradation.rs` — the guard; `Result`
  signature.
- `crates/hares-equipment/src/battery/mod.rs` — the `accumulate` call site's
  error propagation, and `compute_electrical`'s body moving to the shared
  home (public surface and behavior unchanged, tests green across the
  move).
- `crates/hares-types/src/mode_flow_guard.rs` and the `equipment.rs`
  CoreOutput validation call site — **no direction-rule change** (the
  Rule 4 re-keying is withdrawn; the defect never existed — see the
  guard section); Rule 1's `has_flow` gaining the reactive term (a
  workspace-wide invariant change with its rationale stated), the
  guard's tests updated, and `is_heating_variant`'s doc comment updated
  for the `Heating` reuse boundary.
- `crates/hares-equipment/src/` (shared home) — the one home for the
  quadratic solve / `P_max` clamp / I²R.
- `crates/hares-core/src/dwelling/conversions.rs` — the override loud-error
  fix at the `:786`/`:816` callers.
- `crates/hares-python/src/py_equipment.rs` — **`PyEv`'s `#[pyo3]`
  constructor signature (`:654`) gains the new EV config fields** (pack
  electrical parameters, heater power/threshold, thermal overrides),
  following the existing enumerated-parameter pattern (W3): the Python
  surface is a first-class consumer of the EV's configuration, and a
  default unreachable from every public surface is the placeholder-default
  shape the constitution forbids. `py_dwelling.rs` only if its
  `from_typed` plumbing requires it.
- `crates/hares-core/tests/ev_thermal_regressions.rs` — corrected shared
  metric; the L1 cold regression and the override loud-error regression
  land here (behavior-named).
- `crates/hares-core/tests/equipment_id_regressions.rs` — its metric copy
  replaced by the shared helper.
- `crates/hares-core/tests/i07_diag.rs` — assertion tests green and
  recorded, then the file folded; the override-miss probe dies with the
  fold per the deliberate disposition. **Because it is the suite's only
  reader of `capacity_fade_pct`, the fold is ordered after degradation
  observation survives the deletion (W2): assertion tests green → the
  permanent whipsaw-bound regression added and green → fold.**
- `tests/parity/mod.rs` — per-fixture EV override plumbing
  (`heater_power_w: 0` for the EV fixtures' vehicles), so the fixtures
  model the heaterless vehicle the OCHRE reference models — the forced
  outcome of the corrected parity analysis (see Verification). **The same
  condition the `compute_electrical` extraction carries (W2):** the
  plumbing lands as a behavior-preserving no-op for every fixture that
  defines no overrides — the 8 non-EV fixtures' built configs unchanged,
  the full parity suite green across the move (their bands are sized to
  observed residual plus a 1 % margin with no headroom, so any
  unintended config drift goes red by design) — and only then are the EV
  fixtures' overrides applied.

**Deliberately untouched:** `hares-core/src/actors/ev_driver/**` and the
composer; `hares-io` HPXML resolution; the Battery model beyond the two
named sites; PV and water heaters (audited clean); `EvCheckpoint`
(parameter-free); the public `Ev` API apart from the new config fields;
`away_charge_actual_kw` and every other existing telemetry observable's
meaning.

**Scope ledger (observation):** each blast-radius element maps to the
defect or finding that requires it, so the fix reviewer audits scope
against defects, not against critique momentum:

| Element | Required by |
|---|---|
| `ev/mod.rs` thermal/electrical rewrite + ambient-aware init + LUT composition | D1–D3 — the root cause; C2 (same class); W1-rev8 (broken window in an opened function) |
| `ev/config.rs` defaults + new fields | D1/D3 + the uncited-defaults class (17 constants) |
| `ev/catalog.rs` | the pack electrical model (C2-rev1) |
| `ev/telemetry.rs` | inspectability of the new I²R quantity (W7-rev1); truthful heater draw under throttling (W1-rev4) |
| `ev/tests.rs` rewrites | the two defective-pin tests (W5-rev1, W11-rev1) |
| `battery/degradation.rs` guard | the unguarded-domain class (two call sites) |
| `battery/mod.rs` | DRY (W2-rev2) + the guard call site |
| shared pack-electrical home | DRY (W2-rev2) |
| `dwelling/conversions.rs` | the silent-override class (W8-rev1) |
| `py_equipment.rs` | the unreachable-default class (W3-rev3) |
| `tests/parity/mod.rs` fixture EV overrides | the heater/OCHRE one-sided divergence under ambient-aware init (C1-rev6, corrected in reassessment) |
| `hares-types` `mode_flow_guard.rs` + `equipment.rs` validation | Rule 1/Rule 2 (W1-rev10, C1-rev11) — the Rule 4 re-key withdrawn |
| the four test files | the non-discriminating metric class (C1/CS1-rev1) + new-mechanism coverage (C1–C3 rev2/3, W2-rev4) |

**Callers that could notice:** `BATTERY_TEMP_C`/`CAPACITY_KWH`/
`CAPACITY_FADE_PCT`/`HEATER_POWER_W` consumers (physical values, no
whipsaw); `ACTIVE_POWER_KW`/net site load (cold plugged-in EVs gain a real
heater load — the point of the fix). The OCHRE parity corpus *can* notice
this fix — through the heater mechanism once ambient-aware initialization
arms it in the cold fixtures (see Verification) — which is why the forced
fixture policy exists; the *charging* mechanisms it cannot notice
(measured: the fixture EVs start at SOC 1.0 and never charge), which is
the tracking ticket's purpose.

## Verification

**Ordering:** the three `i07_diag` assertion tests go green on the fixed
tree — recorded — then the permanent whipsaw-bound regression is added to
`ev_thermal_regressions.rs` and green (W2), and only then is the harness
folded. The workspace gate: green at the end, everything outside the I-07
tests never having regressed.

Failing tests that must go green (the reproduction's gate commands,
verbatim), with the cold-snap assertion resting on the corrected,
charging-isolating metric:

```
cargo test -p hares-core --test ev_thermal_regressions l2_charge_session_keeps_pack_temperature_physical
cargo test -p hares-core --test ev_thermal_regressions battery_heater_warms_pack_while_cold_derate_blocks_charging
cargo test -p hares-core --test ev_thermal_regressions parked_pack_stays_above_charge_cutoff_after_a_cold_night
cargo test -p hares-core --test ev_thermal_regressions nightly_strategy_keeps_ev_charged_through_a_sustained_cold_snap
```

Passing commands that must stay green (the RCA's `passing_commands`):

```
cargo nextest run --workspace        # red today only on the I-07 tests; everything else must hold
cargo clippy --workspace -- -D warnings
cargo fmt --check --all
```

Named inside the workspace run: the I-06 regressions
(`nightly_strategy_keeps_ev_charged_over_long_horizon`,
`equipment_id_regressions.rs` on the shared metric) — named gates in their
own right — and the OCHRE parity layer, which must stay green and will,
trivially: **it is withdrawn as cross-validation for the EV *thermal*
sub-model (C1, W1) — on the ground that holds, not the one that does
not.** The critique measured the reference corpus rather than its
description: in all three EV-bearing fixtures (`cz2a_pv_ev`,
`cz5a_ev_only`, `cz6b_pv_battery_ev`) the EV draws **0.000 kW max and
mean** for the entire window with SOC pinned at 1.000 (`DEFAULT_SOC`) —
the pack is full, HVAC heating is 89–92 % of site load, and the round-two
baseline this plan carried (4.71 %/0.47 %, "the exposed case", "the EV
is most of the site load") was false — the critique's own claim, adopted
here in good faith and now corrected. **The *charging* mechanisms cannot
move those residuals** — charge power is zero because the pack is full
(nothing to derate, zero current, no I²R heat). **But the heater
mechanism can, and this plan's own C2 change is what arms it — the
interaction the critique's two findings did not connect, found in
reassessment:** its immobility argument (a pack initialising at 20 °C
cannot cool to the threshold within the hour) used the pre-C2
initialization that C2 replaces. The fixtures' EVs are real `Ev`
equipment (`resolve_loads.rs:868+`), defaulting to `HomePluggedIn` with
no configured temperature — so with ambient-aware initialization a
cold-fixture pack starts *at* ambient (−18 °C at cz5a's first hour),
below the 5 °C threshold, and the default-on pack-duty heater fires for
most or all of the window: against a ~4.7 kW mean site load, a 3–8 kW
heater over a one-hour window blows the 25 % band by design, as a
one-sided divergence from a reference whose EV model has no heater at
all. **The fixture policy is therefore restored in corrected form — and
it is forced, not one of two options:** regenerating the reference with
a heater is impossible (OCHRE has none), and widening the band is
forbidden, so the only coherent outcome is configuring the EV fixtures'
vehicles `heater_power_w: 0` — modeling exactly the vehicle the
heaterless OCHRE reference models — via per-fixture override plumbing in
the Rust parity harness (`tests/parity/mod.rs`, added to the blast
radius). No band is widened; the divergence HARES *does* carry (real
preconditioning OCHRE lacks) is recorded as the rationale comment the
constitution requires. The structural gap is recorded and routed: no
fixture in the corpus exercises a charging EV at all (the three EV
fixtures start at SOC 1.0 and never charge; the only multi-hour fixture,
`resstock_bldg0112631_24h`, has no EV; no EV fixture carries
`initialization_duration_seconds`), and no OCHRE reference can ever
cross-check *pack thermal* behavior because OCHRE's EV model has no pack
thermal state — this sub-model exceeds the floor, and the floor cannot
check the excess. A tracking ticket for a
charging-EV parity fixture (cross-checking the *charging* power OCHRE
does model) is created in `.shipwright/initiatives/I-07/tickets/` before
the fix is marked done.

**Post-fix invariants for the rewritten unit tests (preconditioned):** the
~3619 replacement asserts an I²R-magnitude single-digit-kelvin rise;
`heater_draw_slows_soc_gain`'s replacement asserts the additional-load
identity **while the cap is slack**, with the binding-cap clamp asserted
separately; `heater_only_grid_draw_when_fully_cold_derated`'s grid-draw
identity becomes the heater's AC-equivalent — `heater_dc / η`, 0.556 kW
for the configured 500 W at the default η, where the existing test
asserts 0.5 kW to 1e-9 and fails on magnitude (C1-rev14's observation) —
its companion no-SOC-change assertion survives unchanged (the raised
input delivers exactly the heater's DC), and it gains the warming
assertion. No assertion is weakened to pass.
