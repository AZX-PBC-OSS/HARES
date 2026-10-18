---
id: I-07-T1
title: "EV driver actor is blind to the cold-charge derate (D4)"
status: done
created_at: 2026-09-24
created_by: i07-fix
resolved_at: 2026-09-24
resolution: >-
  Fixed in the same session (the operator's no-deferral directive). The
  needed-hours estimate now keys on the equipment's own published
  `CHARGE_DERATE` telemetry when observable — one source of truth with the
  model that actually moves the power — with zero derate reading as +∞
  (impossible, never "slower") for the control consumers (the anxiety gate
  and the departure deadline treat ∞ as maximum urgency), and the ambient
  driving-range curve remaining only as the no-observation fallback. The
  telemetry channel publishes the ambient-curve fallback while charging is
  blocked (the telemetry layer's finiteness contract cannot carry +∞); the
  blocked state itself is carried exactly by the equipment's CHARGE_DERATE
  and power columns. Coverage:
  `needed_charge_hours_keys_on_equipment_charge_derate`,
  `needed_charge_hours_reports_impossible_when_charge_is_physically_blocked`
  (crates/hares-core/src/actors/ev_driver/mod.rs).

# D4 — driver derate blindness

## Problem

The EV driver actor's charging-time and urgency estimates
(`needed_charge_hours_to_target`, `preference.rs`) model cold charging as
*slower* — `temp_efficiency_multiplier(ambient_c)`, capped at 2.0 — never as
*impossible*. The equipment's BMS cutoff at `min_charge_temp_c` (0 °C) makes
charging physically zero below the plating boundary until the pack is
preconditioned, so the actor's `needed_charge_hours` telemetry climbs
nightly while nothing happens — the exact telemetry signature reported in
gh #32 (0.75 → 9.0 over five nights with zero charging).

## Scope (superseded by the resolution above — fixed in-session)

The original scoping note deferred this as a behavioral decision with its
own contract; the operator's no-deferral directive resolved it in the same
session. Kept for the record:

- `hares-core/src/actors/ev_driver/preference.rs` (needed-hours estimate)
- `hares-core/src/actors/ev_driver/efficiency.rs` (temperature multiplier)
- The composer's urgency gating that consumes the estimate

## Verification note from the I-07 fix

The I-07 cold-snap telemetry confirms no *wrong* driver behavior in the
exercised scenarios: zero cancelled trips and ≥ 30 charge-days through the
sustained −7 °C snap, with the actor's estimates producing only cosmetic
`needed_charge_hours` inflation while preconditioning ran.
