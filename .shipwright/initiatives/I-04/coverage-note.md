---
coverage_note_outcome: written
---

## Coverage rounding for I-04

Additional tests were warranted and written — this note records what was
added, what it pins, and what was deliberately left alone. The fix's own
tests (in the working tree of `crates/hares-core/src/actors/ev_driver/`)
already pin the three defect channels broadly: the reproduction test
(`telemetry_reflects_charging_while_equipment_soc_rises`, 4 strategies × 3
channels), an enum-exhaustive class guard
(`every_strategy_variant_telemetry_reflects_active_charging`, all 9
`ChargingStrategy` variants × both dispatch paths), the non-driving-day
refresh, exact-value/no-phantom tracking, rate-over-observation precedence,
ground-truth vs. fallback SOC, and away-charge telemetry with absence
degradation.

### Tests added by this pass

All in existing test modules; no new files, no production code changed.

- `crates/hares-core/src/actors/ev_driver/mod.rs`
  - `charge_kw_is_zero_while_driving` — pins the `DriverPhase::Driving` arm
    of `observed_charge_kw` (`mod.rs`, `DriverPhase::Driving { .. } => None`)
    and the `ControlSignal::EvDrive { .. } => {}` skip in
    `populate_telemetry`. With an `EvDrive` dispatch emitted and a planted
    observable equipment flow (3.6 kW), `charge_kw` must read 0.0.
  - `charge_kw_reports_negative_observed_export_flow` — pins the signed
    pass-through `.map(|p| p.signed_kw())` in `observed_charge_kw`: an
    observed −4.0 kW export with no rate dispatched is reported as −4.0,
    not clamped to 0.0 (truthful-face rule).
- `crates/hares-core/src/actors/ev_driver/soc_target.rs`
  - `needed_charge_hours_infinite_without_charge_rate` — pins the
    `max_charge_kw <= 0.0 || charging_efficiency <= 0.0 → INFINITY` guard
    in the shared helper `needed_charge_hours_to_target`
    (`preference.rs`). Uses inputs where deleting the guard changes the
    outcome (negative rate → negative hours → `debug_assert` panic; zero
    gap at zero rate → NaN), so the test cannot survive the guard's
    removal.
  - `needed_charge_hours_unrounded_when_timestep_zero` — pins the
    `time_res_minutes <= 0.0` unrounded branch of the same helper.

### Verification

`cargo test -p hares-core --lib ev_driver` → 171 passed, 0 failed (167
before this pass), each new test reported `ok`.
`cargo clippy -p hares-core --lib --tests -- -D warnings` → clean.
`cargo fmt -p hares-core -- --check` → clean.

### Deliberately not covered

The Away arm's present/absent `equipment_telemetry` cases and the
composer-fold/override dispatch paths per strategy variant are already
covered by the fix's own tests listed above. Concurrent access is not
applicable (single-threaded actor model). No remaining path near the
change lacks coverage that would catch a real regression.
