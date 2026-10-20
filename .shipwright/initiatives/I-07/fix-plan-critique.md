---
critique_outcome: clean
---

# Critique: `fix-plan.md` revision 17 (I-07 — EV pack thermal sub-model)

**Verdict: clean** — 0 critical, 0 warnings, 2 observations.

Revision 17 closes the last round's warning at both call sites it named, and
closes the last observation this review series was still carrying. The account
below is what earns the verdict; a clean result on the seventeenth revision is
exactly where a rubber stamp would be easiest to pass off, so the search is
enumerated rather than summarised.

---

## Why I found nothing — the attacks I ran

### 1. Was the previous round's warning resolved, or absorbed?

Resolved, at both instances and in both places — and I checked the derivation
rather than the summary, since accepting an index line at face value is the
shortcut that produced the Rule 4 finding I had to withdraw in round eleven.

W1 named two limiters that bound the AC input on the assumption that all
delivered DC reaches the pack: `taper_limit` (`ev/mod.rs:819-823`) and a
commanded `power_setpoint_kw` (`:678-682`). Revision 17 resolves each, with the
working shown (`:260-275`):

> (1) `taper_limit` … is the headroom-to-target AC bound,
> `(soc_limit − soc) · capacity / dt / η`; with the heater diverting DC, the
> bound that preserves its purpose — land exactly on the target, no overshoot —
> is `(headroom_dc + heater_dc) / η` … (2) A commanded `power_setpoint_kw` …
> is a **total-draw bound**: the heater is carved out within it by the priority
> rule and the charge leg receives the remainder — the vehicle never draws more
> than its dispatch commands

The first is arithmetically right: to land on target the AC input must deliver
`headroom_dc` to the cells *and* `heater_dc` to the heater, so the bound is
their sum over η. The second is the choice that keeps the dispatch-obedience
guarantee the plan's DR rationale rests on, rather than the one that would have
let the raise exceed a commanded setpoint. The passage also carries my own
magnitude correction from last round — that in the nominal regime a binding
step is "a delay the [window absorbs]" rather than a shortfall — so the
resolution is not over-claimed either.

### 2. The last carried observation

Closed. I had flagged for three rounds that
`heater_only_grid_draw_when_fully_cold_derated` asserts port draw equals heater
nameplate to 1e-9 (`ev/tests.rs:589-596`) while R14's billing makes it
`heater_dc / η`, and that the plan's test list omitted the magnitude change.
Revision 17 lists it: "`heater_only_grid_draw_when_fully_cold_derated`'s
identity updated to the heater's AC-equivalent (`heater_dc / η` — the existing
0.5 kW 1e-9 [assertion])" (`:988-989`).

### 3. A fresh enumeration, on the quantity whose semantics moved most

`heater_active` and `heater_power_w` changed meaning twice across this plan —
to post-cap *actual* draw in R10, and to a pack-side DC load in R14. If either
change leaked into a consumer that assumed the old meaning, this is where it
would show. I enumerated every reference in the workspace: 39 sites, and all of
them are one of —

- EV-internal state, config, checkpoint and catalog (`ev/mod.rs`,
  `ev/config.rs:31`/`:263`/`:388`, `ev/checkpoint.rs:20`, `ev/catalog.rs:167`);
- config-construction sites passing `None` (`hares-io/.../resolve_loads.rs:889`,
  `resolve_der.rs:153`/`:219`, `hares-core/.../ev_driver/mod.rs` fixtures,
  `dwelling/mod.rs:9157`);
- the telemetry key definition (`hares-types/src/telemetry_keys.rs:409`);
- Python bindings (`py_equipment.rs:236`–`:531`), already in the blast radius
  for the new EV config fields; `py_dwelling.rs:486` reads
  `battery.heater_power_w`, which is the *Battery's* field, not the EV's.

No cross-equipment consumer interprets the EV's heater state, and nothing
aggregates it. Both semantic changes are confined to the EV and its telemetry.

### 4. The structural bar, re-checked on this revision

All seven gate commands resolve to real functions — four in
`crates/hares-core/tests/ev_thermal_regressions.rs`, three in
`crates/hares-core/tests/i07_diag.rs`. `plan_outcome: ready` sits on a plan that
still targets the RCA's confirmed root cause (D1–D3 referenced throughout)
rather than the reproduction's symptom, with alternatives carrying reasons they
lose, a blast radius naming 15 files and call sites, verification using the
reproduction's own gate commands with a stated ordering, and behaviour described
rather than signatures invented.

### 5. Carried forward from prior rounds, not re-run

The enumerations that produced earlier findings and have since come back clean:
`active_power_kw`'s 16 consumers; `battery_temp_c`'s 14; the six SOC mutation
sites; `OperatingMode::Heating`'s 83 call sites across 26 files, including the
two dwelling-level zone classifiers; all four `mode_flow_guard` rules; the full
`CoreOutput` validation path; the V2L/V2G discharge budgets; and the limiter
chain in `compute_charging_power_kw`. The R16 DR correction — withdrawing
`min(rated, power_limit, dr·rated)` for the multiplier form the code actually
applies — I verified propagated to both the index (`:103`) and the body
(`:322-328`), with the remaining mentions being withdrawal notes rather than
live guidance.

---

## Observation

- **The `Heating` reuse depends on the EV having no zone.** The two
  dwelling-level classifiers that would sweep an EV into a zone's heating state
  are guarded by `eq.descriptor().zone` (`diagnostics.rs:931`,
  `dwelling/mod.rs:4213`), and the EV's is `None` (`ev/mod.rs:212`). Nothing in
  this plan proposes changing that, so it is not a finding against it — but the
  plan does discuss the pack being "outdoors/garage", and if the EV ever gains a
  zone, a preconditioning vehicle would begin setting `zone_has_heating` while
  emitting no zone thermal power. Worth knowing before that change is made, not
  before this one.

- **How to weight this critique history.** Seventeen revisions carry seventeen
  rounds of findings, and not all of them were right. The parity section exists
  because of a round-two finding of mine built on fixture names I had not
  opened, withdrawn in round six. The Rule 4 re-keying sat in the plan for two
  revisions on a defect I asserted in round ten and disproved in round eleven.
  The `min(rated, power_limit, dr·rated)` formula I quoted approvingly in three
  rounds and computed binding conditions from in two before finding it
  misdescribed the code. The plan's own reasoning caught at least two defects I
  missed — the heater double-count across the vehicle boundary, and the
  interaction between my own C1 and C2 in round seven. A fix-step reviewer
  should read the plan as the authority and this history as commentary.

---

## Which findings change the build, and which change the document

Neither list has anything in it: there are no findings. The two observations
below change nothing — the first is a dependency worth knowing before a future
change that nothing here proposes, the second is context for reading this
critique series. Recording the empty split explicitly rather than omitting the
section, so its absence is not mistaken for an oversight.

---

## What this verdict asserts

That I found no critical or warning defect in revision 17 after the searches
above, and that the plan is implementable as written.

Not that the constants are correct: five remain unsourced — pack nominal
voltage, cell resistance, UA, exact heater power, and whether the `deg_const`
activation energies are Smith 2017's — and the plan blocks merge on each. I
established in an earlier round that these are datasheet quantities rather than
literature quantities and cannot be closed from a critique chair; gating merge
on them is the correct disposition.

And not that nothing further exists to find. It asserts that the remaining
uncertainty now lives in code that has not been written and datasheets that
have not been opened, and that further critique rounds are the wrong instrument
for either.
