# Decision Log

## dl-f8d2e388-db7b-4154-bfb2-c9cb9873a7f7

```yaml
event_id: dl-f8d2e388-db7b-4154-bfb2-c9cb9873a7f7
timestamp: 2026-09-24T16:40:09.285Z
actor: operator
action: flow_recipe_selected
target: Z:/home/rich/src/HARES/.shipwright/flows/fix.yaml
recipe_sha256: 857ab6a3ed3c0e283c8ef77f2f596cbde15393369c2a69347be7f0fb8b88b778
reason: workspace_override
```

---

## dl-i07-fix-01 (fix-phase decisions, appended by the fix agent)

```yaml
event_id: dl-i07-fix-01
timestamp: 2026-09-24T17:40:00.000Z
actor: fix_agent
action: model_alignment
target: crates/hares-equipment/src/battery/degradation.rs
reason: primary-source verification overruled an inherited divergence
```

**What was wrong:** HARES's degradation constants followed OCHRE's
Matlab-derived values (b0 = 1.0, b3_ref = −2.805e-2, θ = −0.135), which
OCHRE itself flags with TODOs ("Matlab value varies", "might be positive?").
**What it says now:** the constants match NREL's own implementation of
Smith 2017 (SSC `lib_battery_lifetime_nmc.{h,cpp}`, the lineage EnergyPlus
delegates to): α_b1 = −1 confirmed (the paper's printed "+1.0" line is an
extraction artifact — the PDF's inner-fraction minus glyphs are unmapped
in every text extractor, so the paper alone cannot settle the signs; the
reference implementation is decisive), b3_ref = +2.805e-2 (a break-in Li
LOSS, not a capacity boost), θ = +0.135, b0 = 1.07; the model gained the
negative-electrode site-loss branch (Eq. 8–11) and the d0 reference ratio,
with usable capacity = min(QLi, Qneg) per Eq. 1. **Other documents citing
the old version:** the fix plan's guard section assumed the provenance
claim was merely true; tickets/I-07-T3 (created then resolved in-session)
carries the full account.

## dl-i07-fix-02

```yaml
event_id: dl-i07-fix-02
timestamp: 2026-09-24T17:45:00.000Z
actor: fix_agent
action: architecture_decision
target: crates/hares-equipment/src/ev/mod.rs (refresh_usable_capacity)
reason: avoid double-counting the reversible temperature scaling
```

**Decision:** the reversible temperature-capacity scaling (a cold pack
holds less charge) lives at the equipment layer, not inside the shared
degradation fade. SSC evaluates both branches' temperature dependence
inside the daily capacity update; HARES's stationary Battery already
carries the identical NREL d0 Arrhenius as its `CapacityDerateModel`, so
baking it into the fade would double-count for the Battery and leave the
EV (which had no derate at all — another Battery/EV divergence in this
initiative's class) silently unscaled. The fade is now temperature-
independent (both branches' capacity levels at their T-reference ratios;
the damage paths temperature-scaled exactly as SSC scales them), and the
EV applies the same derate layer the Battery has. The dwelling invariant
`ev_capacity_degraded` was extended to the full contract
(capacity = rated·(1−fade)·derate(T)); its SOH-only form would have
rejected the derate as a violation.

## dl-i07-fix-03

```yaml
event_id: dl-i07-fix-03
timestamp: 2026-09-24T17:50:00.000Z
actor: fix_agent
action: control_contract_refinement
target: crates/hares-equipment/src/ev/mod.rs (run_charging_physics)
reason: two pinned contracts conflicted as written in the plan
```

**Decision:** a commanded `power_setpoint_kw` is a total-draw bound (the
heater's AC-equivalent carved out within it) EXCEPT while a
`DeadlineGuarantee` ready-by deadline has raised the charge demand above
the command — the setpoint is a soft floor under that priority (four
existing tests pin the raise). The plan's R17 form (the total never
exceeds the command) executed literally defeated the deadline contract;
the demand function now reports whether a deadline raise is active and
the allocator bounds accordingly. Both contracts verified green.

## dl-i07-fix-04

```yaml
event_id: dl-i07-fix-04
timestamp: 2026-09-24T22:35:00.000Z
actor: fix_agent
action: restore_contract_correction
target: crates/hares-equipment/src/ev/mod.rs (load_state), ev/checkpoint.rs (v6)
reason: the F1 fix's restore recomputed vars over the wrong power basis
```

**What was wrong:** the F1 fix's restore recomputed the reactive flow
over `active_power_kw` — the heater-inclusive port total — while the
step keys the var computation on the inverter leg (the charge/export
conversion only, because the DC-fed pack heater produces no vars and
consumes no kVA headroom). With a power-factor baseline configured, the
wrong basis published vars from a resistive load (2.69 kVAR in idle
preconditioning where the step publishes 0) and zeroed a commanded
6.5 kVAR at a bound-binding session — both guard-valid pairs, silent.

**What it says now:** the step-published `reactive_power_kvar` is
checkpointed (v6) and restored verbatim (the water heaters' lossless
restore pattern) — the inverter-leg basis is not derivable at restore
(the heater draw that splits the port total is transient step state).
**Other documents citing the old version:** red-team-note.md (round 2,
F4) records the defect; this entry records the fix.

## dl-i07-fix-05

```yaml
event_id: dl-i07-fix-05
timestamp: 2026-09-24T23:20:00.000Z
actor: fix_agent
action: restore_contract_completion
reason: the F4 fix left the same infidelity in the heater draw column
target: crates/hares-equipment/src/ev/checkpoint.rs (v7), ev/mod.rs load_state
```

**What was wrong:** the v6 restore still zeroed `heater_draw_w` while
restoring the heater-funded `active_power_kw`, the `Heating` mode, and
the verbatim reactive — publishing a state no live step produces
(`Heating` with a zero heater column, heater-funded port power
attributed to nothing).

**What it says now:** `heater_draw_w` is checkpointed (v7) and restored
verbatim — the same lossless pattern and the same rationale as the
reactive flow: the charge-leg/heater split of the port total is
transient step state, not derivable at restore. `heater_active` is
derived from the restored draw (draw > 0), the pairing the step's own
value-carrying telemetry contract makes canonical.
**Other documents citing the old version:** red-team-note.md (round 3,
F5) records the defect; this entry records the fix.

## dl-i07-fix-06

```yaml
event_id: dl-i07-fix-06
timestamp: 2026-09-24T23:50:00.000Z
actor: fix_agent
action: restore_contract_completion
reason: the v2l derivation keyed the dispatch on the mode; the step keys it on dispatch presence
target: crates/hares-equipment/src/ev/checkpoint.rs (v8), ev/mod.rs load_state
```

**What was wrong:** the v2l restore derived `v2l_active` from
`last_mode == Discharging`, but the step keys the field on dispatch
**presence** (`is_discharge`), not the computed export — so at the
reserve floor (and under an export DR) a dispatched step publishes
`v2l_active = true` with zero export and mode `Off`, and the derivation
flipped false exactly there, diverging from the saved state where
`island_source_available` reads it.

**What it says now:** `v2l_active`/`v2l_power_kw` are checkpointed (v8)
and restored verbatim — no checkpointed basis reproduces them in that
corner (the mode is `Off`; the port sign cannot distinguish a
dispatched zero export from an idle connection). Same lossless pattern
as the reactive flow and heater draw (v6/v7).
**Other documents citing the old version:** coverage-note-quality.md
(round 6) records the defect; this entry records the fix.

## dl-i07-fix-07

```yaml
event_id: dl-i07-fix-07
timestamp: 2026-09-25T00:20:00.000Z
actor: fix_agent
action: restore_contract_completion
reason: the away session's published intake was the last uncheckpointed member of the fidelity family
target: crates/hares-equipment/src/ev/checkpoint.rs (v9), ev/mod.rs load_state
```

**What was wrong:** the restore left `away_charge_actual_kw` at its
init zero for an active away session — a published transfer
observable (`AWAY_CHARGE_POWER_KW`, the documented way away
charging is observed) whose basis (the away charge leg) is not
derivable at restore, misreported as drawing nothing until the
next step. Earlier dispositions calling it a "documented lossy
placeholder" were wrong: it is a published observable, not an
internal diagnostic like `ohmic_loss_w`.

**What it says now:** `away_charge_actual_kw` is checkpointed (v9)
and restored verbatim — the charge-side counterpart of
`v2l_power_kw`, completing the verbatim family (reactive flow v6,
heater draw v7, V2L dispatch state v8, away intake v9).
**Other documents citing the old version:** red-team-note.md (round
4, F6) records the defect; this entry records the fix.



## Done-challenge produced changes — 2026-09-25T01:04:03.188Z

- **Phase:** fix
- **Run:** I-07-fix-44d537ed-c089-4e5b-9395-1f396632d16d

## Done-challenge produced changes — 2026-09-25T01:22:56.023Z

- **Phase:** round-out-coverage
- **Run:** I-07-round-out-coverage-44e64426-425b-42cd-837f-dc26b441bc04

## Done-challenge produced changes — 2026-09-25T02:04:20.608Z

- **Phase:** fix
- **Run:** I-07-fix-a0e92869-1c66-4516-8ce2-19b1201e1c6f

## Done-challenge produced changes — 2026-09-25T02:37:12.742Z

- **Phase:** red-team
- **Run:** I-07-red-team-495fee12-9f89-449b-9cf6-4d97295e53c2

## Done-challenge produced changes — 2026-09-25T03:08:22.798Z

- **Phase:** fix
- **Run:** I-07-fix-135192db-5ab6-49aa-b675-686f186aa56c

## Done-challenge produced changes — 2026-09-25T03:38:25.646Z

- **Phase:** coverage-triage
- **Run:** I-07-coverage-triage-66f3ec65-574d-4473-9c52-bb1480c9d7d5

## Done-challenge produced changes — 2026-09-25T05:05:51.342Z

- **Phase:** fix
- **Run:** I-07-fix-2494c869-f365-401e-adce-9fd9ad798c4e

## Done-challenge produced changes — 2026-09-25T06:36:24.581Z

- **Phase:** fix
- **Run:** I-07-fix-8a3892f4-50fd-454c-a7f5-4a62249fbced

## Done-challenge produced changes — 2026-09-25T06:55:37.985Z

- **Phase:** coverage-triage
- **Run:** I-07-coverage-triage-4cdca680-1af2-43e3-8236-0f9f580177d8

## Done-challenge produced changes — 2026-09-25T08:02:50.211Z

- **Phase:** red-team
- **Run:** I-07-red-team-0e7d51f4-bacb-495e-8095-1303be0858e6

## Done-challenge produced changes — 2026-09-25T16:11:07.997Z

- **Phase:** review
- **Run:** I-07-review-5ca93e42-617a-4391-8d7b-7adcd83d64a1

## Done-challenge produced changes — 2026-09-25T19:55:13.295Z

- **Phase:** red-team
- **Run:** I-07-red-team-24166c5f-35b5-4928-a51c-ccd36f6b1974

## Done-challenge produced changes — 2026-09-25T20:30:42.541Z

- **Phase:** red-team
- **Run:** I-07-red-team-133849b9-c422-4ce9-9c7b-83855b8edc3a

## Done-challenge produced changes — 2026-09-26T00:32:00.601Z

- **Phase:** review
- **Run:** I-07-review-ed3139d1-cf50-4aa9-b9d2-ff51364e6906

## Done-challenge produced changes — 2026-09-26T01:09:00.771Z

- **Phase:** fix
- **Run:** I-07-fix-540682d4-4e99-4411-945e-cb7c918df6fd

## Done-challenge produced changes — 2026-09-26T02:18:53.113Z

- **Phase:** coverage-triage
- **Run:** I-07-coverage-triage-b652061e-32e5-4e44-aef6-c9c7f0a7b0d7