---
"id": "I-03"
"title": "HPXML-declared EV ignores its charging srategy"
"status": "active"
"kind": "fix"
"description": "Summary. The charging_strategy override is accepted without
  error and its content is discarded; only its presence matters. An EV given
  Immediate or no strategy never charges at all while still appearing in the
  equipment list and emitting a column.\ 


  Evidence. bldg0168875/G4804390, 30 days, varying only the override: Nightly
  22:00–06:00 → 332.1 kWh; Nightly 02:00–06:00 → 332.1 kWh identical; LowSoc
  0.15 → 332.1 kWh identical; Immediate → 0.0 kWh; no override → 0.0 kWh. What
  runs instead is charge-on-plug-in from ~18:00. ev/mod.rs:305-308 parses with
  .and_then(|s| from_str(s).ok()).unwrap_or(Immediate) — a mistyped field
  silently becomes Immediate, i.e. no charging, with no diagnostic. A Result
  would surface it at construction.\ 


  What we need. Is the equipment-override path supported for this field? If not
  it should reject rather than accept-and-ignore. And should a strategy-less EV
  really never charge?\ 


  Note. add_ev_with_driver(VehicleId, EvArchetypeId, seed) works and is our
  workaround."
"created_at": "2026-09-12T06:58:10.008Z"
"created_by": "operator"
regression_tests:
  - crates/hares-equipment/src/ev/tests.rs
  - crates/hares-core/src/dwelling/mod.rs
  - tests/python/test_ev_tariff_integration.py
  - crates/hares-types/src/equipment.rs
  - crates/hares-types/src/control_signal.rs
  - crates/hares-equipment/src/config.rs
  - crates/hares-equipment/tests/control_gate_guard.rs
---

Summary. The charging_strategy override is accepted without error and its content is discarded; only its presence matters. An EV given Immediate or no strategy never charges at all while still appearing in the equipment list and emitting a column. 

Evidence. bldg0168875/G4804390, 30 days, varying only the override: Nightly 22:00–06:00 → 332.1 kWh; Nightly 02:00–06:00 → 332.1 kWh identical; LowSoc 0.15 → 332.1 kWh identical; Immediate → 0.0 kWh; no override → 0.0 kWh. What runs instead is charge-on-plug-in from ~18:00. ev/mod.rs:305-308 parses with .and_then(|s| from_str(s).ok()).unwrap_or(Immediate) — a mistyped field silently becomes Immediate, i.e. no charging, with no diagnostic. A Result would surface it at construction. 

What we need. Is the equipment-override path supported for this field? If not it should reject rather than accept-and-ignore. And should a strategy-less EV really never charge? 

Note. add_ev_with_driver(VehicleId, EvArchetypeId, seed) works and is our workaround.

## Implementation Notes

Fixed: `Ev::actor_seed()` no longer suppresses the driver actor for
`ChargingStrategy::Immediate` (crates/hares-equipment/src/ev/mod.rs). Every
strategy now provisions the `EvDriverActor`, which is the vehicle-use
simulation (trips, SOC depletion), not just charging-schedule logic. An
HPXML-declared EV with no override — or an explicit `Immediate` override —
now charges once its SOC drops. `add_ev_with_driver` now names its manually
attached actor with the canonical `EvDriver:<equipment>` convention so a
later `set_tariff` re-registration cannot attach a second, competing driver.
The python `add_ev` path re-runs actor registration when the equipment is
added: equipment added after construction never sees the construction-time
registration, and registration otherwise only re-runs on `set_tariff`, so
without this an `add_ev` EV sat inert unless a tariff happened to be set
later (verified: 0.0 kWh without, 148.5 kWh with, over 3 days). Manual
ControlSignal control of an `add_ev` EV coexists with the attached driver
(verified: `TestEvControlSignals`, including the exact-SOC drive assertion,
40/40 passing with registration enabled).

Red-team round: `Ev::init_typed()` — the boundary every construction path
funnels override strings through — now rejects domain-invalid values instead
of silently accepting them: `ChargingStrategy::validate()` (which existed
with zero callers) is invoked after the JSON parse, `initial_connection_state`
propagates its `FromStr` error instead of degrading to `HomePluggedIn`, and
`PlugInPolicy` gained the `validate()` it never had (hares-types, reusing
`validate_fraction`). Each of those silently left an EV never charging —
the symptom class this initiative fixed — reachable through well-formed
nonsense that previously parsed clean. Class completed in reassessment: the
same silent-swallow shape on the `chemistry` override — present-but-unparseable
string silently substituting NMC, whose OCV/u_neg tables then drive every
charge-voltage decision — was fixed at both load-bearing sites (EV
`init_typed` and Battery `init_typed`), each with a regression test.

Class hardening round (the same defect class, measured across every config
boundary in the equipment crate — present-but-invalid values silently
substituted, degraded, or dropped):

- Shared `normalize_config_name` (hares-equipment config.rs): one documented
  case/whitespace/underscore-insensitivity contract for every strict name
  parser, so accepted-spelling rules cannot drift between sites.
- EV `charging_level` now parses strictly (`parse_charging_level`: L1,
  Level 1, 1, L2, Level 2, 2) — an unknown string previously became L2 and
  silently clamped the configured charge power; the clamp itself now warns
  when it alters a supplied value (level hardware bounds are correct physics,
  but the adjustment must be observable).
- `ChargingStrategy`, `PlugInPolicy`, `BmsMode`, `GridExportRule` gained
  `#[serde(deny_unknown_fields)]`: a mistyped key inside an override payload
  (e.g. `off_peak_statr_hour`) was silently dropped by serde's default.
- `Ev::new`'s raw-config parses are now strict through PV's deferred-error
  channel: an invalid raw value keeps a placeholder (the registry factory is
  infallible) and the first `init()` surfaces the error — the raw/typed parity
  tests keep passing while the silent fallbacks are gone. The parse logic
  itself is shared with `init_typed` (`parse_charging_strategy`,
  `parse_plug_in_policy`, `parse_charging_priority`), one source of truth for
  both surfaces.
- PV `ModuleType::from_str` is strict like its sibling `ArrayType::from_str`
  (an unknown module type previously became Standard, silently skewing the
  temperature coefficient and every PV output).
- PV `parse_shading_config` rejects unknown model names (previously silently
  unshaded), deferred through PV's `init_error` channel.
- Gas water heater `ignition_type` parses strictly (a typo of "electronic"
  previously ran a 150 W standing pilot for the whole simulation); explicit
  StandingPilot spellings are now accepted rather than falling through.
- Heat-pump water heater `element_hp_control_mode` and resistance water
  heater `element_priority_mode` parse strictly (typos previously became the
  default element-interlock behaviour).
- PV SAM-LUT parquet key-value metadata parses strictly
  (`harvest_kv_f64`/`harvest_kv_u8`): a corrupt or hand-edited LUT previously
  defaulted latitude to 0 and inverter efficiency to 0 with no trace.

Class survey (what was searched and found already strict): battery
`bms_mode`/`grid_export_rule`, generator `efficiency_type`, ventilation
`ventilation_type`, PV `array_type`, event-load `fuel_type`; water-heater
`zone_type` is label-based default selection where the fallback is correct
for every real HPXML location label — not an instance. `vehicle_type`
(EV `vehicle_number_from_config`) is raw-only and unreachable from typed
payloads (`EvConfig` denies the key; raw payloads fail at init) — documented
at the function.

Known gap flagged for its own work item: raw simple-load overrides
(event_load, 49 KEY_ constants) have no key/shape validation at the merge
boundary — an unknown override key or a stringly numeric on that surface is
silently ignored, while the typed path denies unknown fields and
`merge_zip_override` hard-errors them. The remedy needs a per-class known-key
schema that does not exist yet (design sketch: `known_keys()` per equipment
class, checked in `merged_equipment_config`'s raw branch, mirroring the
`merge_zip_override` ethos). HPXML parameters arrive shape-correct (parsed
numerics), so the hazard is user-override-only.

Red-team pass-6 round (five findings, plus the same classes hunted):

- `DepartureConstraint` (nested inside `ChargingStrategy` schedules) gained
  `deny_unknown_fields` — the sweep had covered the four override enums but
  not the struct nested inside them.
- The `EvDrive` arm's overdraw guard was one-sided: negative/NaN/-inf kWh
  passed it and created SOC from nowhere (or poisoned every downstream
  clamp). The arm now rejects non-finite or negative energy outright.
- Same class, same fix in the sibling arms: `EvAwayCharge` (a non-finite or
  negative power silently no-oped against the `> 0.0` gate), `EvSetReadyBy`
  (out-of-domain hour/SOC feed the charging target and deadline pacing),
  and Battery `PowerSetpoint` (the arm already re-checked its reactive
  component for finiteness — the active component now gets the same guard).
  All four arms are self-safe on `apply_control_unchecked`, matching the
  battery's own precedent for arm-level defense-in-depth beneath the central
  `validate_numeric_bounds`.
- Actor lifecycle: the `Actor` trait gained `dispatch_target_name()`;
  `remove_equipment`, `remove_equipment_by_end_use`, and `clear_equipment`
  evict actors targeting removed equipment (a controller without its
  equipment is an orphan), and `add_actor` now rejects duplicate names,
  mirroring `add_equipment`. Together these close the
  add→remove→re-add double-driver (re-added vehicle drew 2.8x the energy of
  an identical single-add control).
- `deny_unknown_fields` sweep of nested config structs found and closed two
  more gaps: `SoilingConfig` (PV) and `HvacSetpointConfig` (heating). One
  named limitation documented at the type:
  `ScheduleSourceConfig` is an internally tagged enum, and serde's
  internally-tagged derive does not implement `deny_unknown_fields`
  (serde_derive 1.0.229 `de/enum_internally.rs` never reads the attribute) —
  unknown fields inside its variants are silently dropped and no type-level
  guard is available.

Red-team pass-7 round (three findings — the pass-6 arm guards under-counted
the EV's own surface):

- EV `PowerSetpoint` arm now rejects a non-finite `active_power_kw` (NaN
  silently disarmed charging forever — `f64::max` swallows NaN; −∞ with
  v2g/v2l passed the negative-power gate and saturated to the hardware
  maximum) and a non-finite SOC window, matching the battery/PV/
  scheduled-load arm-level precedent.
- EV `SOCTarget` arm now rejects non-finite target/window values before
  the clamp (`f64::clamp` propagates NaN, making `soc >= soc_limit` false
  forever — full-power charging toward an unreachable target), matching
  the battery arm's post-clamp rejection.
- The central `validate_numeric_bounds` `PowerSetpoint` arm previously
  ignored `min_soc`/`max_soc` (`..`), so a non-finite window reached
  equipment arms on the production `apply_control` path and was silently
  substituted with defaults downstream (`max(NaN)` returns the reserve,
  `min(NaN)` no-ops). It now validates the window with the same rules its
  own `SOCTarget` arm already enforced (finite, [0, 1], min < max) —
  closing the hole for every equipment, not just the EV. With this, the
  central validator covers every f64 field of every `ControlSignal`
  variant (the remaining `..` arms hide only bool/enum fields); PV and
  scheduled-load ignore the window because they have no SOC concept, and
  the battery rejects the fields outright — verified no legitimate sender
  is affected (actor V2G/V2H floors come from strategy-validated configs).

Red-team pass-8 round (the class, measured across every
`apply_control_unchecked` arm — 14 findings, one shared root):

- Root cause: `apply_control_unchecked` was the *required* trait method
  (the arm) with no validation, so each equipment decided for itself
  whether a value the central validator rejects was errored, clamped,
  no-oped, or raw-stored — sibling arms disagreed about the identical
  garbage. Fixed at the trait, not per-arm: the arm is now the required
  `apply_signal`; `apply_control_unchecked` is a provided method that
  enforces `validate_numeric_bounds` then delegates. Whatever path
  delivers a signal, the arm can never receive a value the central
  validator rejects — the rule cannot be forgotten by the next equipment.
  `apply_control` delegates to `apply_signal` after its capability gate
  (single validation on the checked path). Arms keep their
  state-dependent checks (EV connection state, battery field
  rejections, the water-heater zero-delay case the central [0, ∞) rule
  deliberately allows); the pass-6/7 arm-level finiteness guards remain
  as defense-in-depth beneath the trait, matching the battery-arm
  precedent.
- Mechanical fallout, all verified: ~45 implementor renames across
  equipment, dwelling test stubs, and integration-test stubs; the
  `delegate_equipment!` macro forwards `apply_signal` (one validation,
  not two); one pre-existing test's message-wording assertion updated
  (`event_delay_negative_returns_error` — the rejection contract is
  unchanged; the error now comes from the central validator, the single
  source of truth, while the arm's zero-case check remains live).
- Performance: none in production — the dwelling dispatches every signal
  through the validated `apply_control` (dwelling/mod.rs:1091, 1186,
  2829); `apply_control_unchecked` has no production callers (verified by
  sweeping every call site), so the added validation on that path costs
  nothing outside tests, and the checked path still validates exactly
  once (`apply_control` delegates straight to `apply_signal`).

### Known Limitations

- **Resolved in the class-hardening round:** `Ev::new()` no longer silently
  falls back to `Immediate` on an unparseable `charging_strategy` string. The
  factory contract stays infallible, but the parse error is now deferred via
  `defer_strict` into `Ev.init_error` and surfaced as `Err` by the
  immediately-following `Ev::init()` (crates/hares-equipment/src/ev/mod.rs,
  `defer_strict` call sites and `fn init`). No silent-fallback path remains.
- **What:** HPXML v3 `Systems/Vehicles/Vehicle/VehicleType/BatteryElectricVehicle`
  declarations are silently ignored — `resolve_ev`
  (crates/hares-io/src/hpxml/resolve_der.rs) only parses
  `Systems/ElectricVehicles/ElectricVehicle`, so the two resstock 2025.1
  fixtures declaring EVs (bldg0442669, bldg0176775) build dwellings with no
  EV equipment and no diagnostic.
  **Constraint:** supporting the v3 `Vehicles` dialect (with its
  `ElectricVehicleChargers` companion block) is new parser surface, and
  erroring on the unhandled shape would fail the existing smoke fixtures —
  a behavior decision the operator owns.
  **Tried:** none — discovered while seeking an e2e fixture for this fix;
  flagged rather than fixed because both remedies change HPXML acceptance
  behavior beyond this ticket.
  **Resolvable when:** resolve_ev gains a v3 `Vehicles` parser or a
  hard-error policy on unhandled vehicle declarations.
