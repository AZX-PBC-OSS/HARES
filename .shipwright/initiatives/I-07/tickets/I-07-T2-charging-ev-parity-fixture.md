---
id: I-07-T2
title: "Parity corpus has no charging-EV fixture"
status: done
created_at: 2026-09-24
created_by: i07-fix
resolved_at: 2026-09-24
resolution: >-
  Fixed in the same session (the operator's no-deferral directive). The
  corpus gained `cz5a_ev_charging`: the EV starts the window discharged
  (SOC 0.5) and both simulators charge it deterministically — OCHRE pinned
  by a single-day event list (`ev_events.csv`, one day_id so OCHRE's
  day-sampling is deterministic), HARES pinned by the per-fixture `[ev]`
  override (initial SOC, a warm pack, an Immediate strategy, heaterless —
  the reference has no thermal model to mirror). The metric it exists for
  — short-window total site energy, dominated by the EV's 11.5 kW charge —
  passes at the DEFAULT band (measured residual 1.36% vs 25% allowed):
  the EV charging power OCHRE models is now cross-checked. The two
  inherited-drift metrics (zone temp, HVAC energy) carry the same
  documented drift-lock overrides as the sibling cz5a_ev_only fixture
  (same building, same root causes: T-0075 envelope conformance and the
  step-0 ideal-capacity back-solve). Infrastructure: the reference
  generator gained the `[ev] ochre_event_file` pin
  (tests/python/generate_parity_reference.py) and the Rust harness gained
  the config-driven `[ev]` override merge (tests/parity/mod.rs,
  `ev_override`). The EV thermal sub-model remains permanently beyond
  parity's reach (OCHRE has no pack thermal state) — its coverage lives in
  crates/hares-core/tests/ev_thermal_regressions.rs.

# Charging-EV parity fixture

## Problem

No fixture in the OCHRE parity corpus exercises a *charging* EV: all three
EV-bearing fixtures (`cz2a_pv_ev`, `cz5a_ev_only`, `cz6b_pv_battery_ev`)
start at SOC 1.0 and draw 0.000 kW for their entire (one-hour) windows —
the pack is full, so no charging mechanism is ever exercised; and no EV
fixture carries `initialization_duration_seconds`, so none can reach a
discharged state. The only multi-hour fixture (`resstock_bldg0112631_24h`)
has no EV at all.

Consequently the EV's *charging power* — which OCHRE **does** model
(`EV.py` charging event logic) — has no cross-check against the reference
implementation.

## Boundary: what parity can never check

OCHRE's EV model has no pack thermal state (no temperature, no heater, no
derate), so the EV *thermal* sub-model is permanently beyond parity's
reach — it exceeds the floor, and the floor cannot check the excess. That
sub-model's coverage lives in `crates/hares-core/tests/ev_thermal_regressions.rs`.

## Work (superseded by the resolution above — done in-session)

The original scoping note deferred this to the OCHRE reference-generation
subsystem; the operator's no-deferral directive built it in the same
session (the generator extension is three lines of plumbing plus the
fixture). The heaterless-vehicle guard (`heater_power_w: 0` on every EV
fixture) remains in force for all EV fixtures — now via the config-driven
`ev_override` (tests/parity/mod.rs), which the charging fixture's `[ev]`
section builds on.
