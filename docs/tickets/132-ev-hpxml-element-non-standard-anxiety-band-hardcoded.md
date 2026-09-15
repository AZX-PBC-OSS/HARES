# EV HPXML Parser Reads a Non-Standard Element; Downstream Driving-Behaviour Parameters Are Hardcoded

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io, hares-core, hares-equipment

## Problem

Two related, compounding defects in the HPXML-declared EV path:

1. **`resolve_ev` reads an HPXML element that does not exist in the
   HPXML 4.x or 5.x specification.** `Systems/ElectricVehicles/
   ElectricVehicle` (with children `BatteryCapacity`, `MaxChargingPower`,
   `ChargingLevel`) is not defined by HPXML. The standard vehicle
   structure is `BuildingDetails/Vehicles/Vehicle/VehicleType/
   BatteryElectricVehicle`, and battery capacity lives at
   `.../BatteryElectricVehicle/Battery/{NominalCapacity,UsableCapacity}`
   (type `BatteryCapacityType`, wrapping a `Value` of type
   `BatteryCapacity` with a `Units` sibling). Maximum charging power is
   not a vehicle-level field at all — it lives on a separate
   `Systems/ElectricVehicleChargers/ElectricVehicleCharger/ChargingPower`
   element (`[W]`, "Maximum charging rate"), linked to the vehicle via
   `VehicleType/BatteryElectricVehicle/ConnectedCharger` (a
   `LocalReference`), not a single flat element.

   Checked this cycle whether `Systems/ElectricVehicles` is a documented
   OS-HPXML or ResStock extension HARES intentionally targets: the
   OS-HPXML sample files vendored under
   `vendors/OCHRE/test/OS-HPXML Sample Files` contain no `ElectricVehicle`
   element at all, and OCHRE's own HPXML reader sources EV load from the
   standard HPXML plug-load taxonomy
   (`PlugLoadType="electric vehicle charging"`,
   `vendors/OCHRE/ochre/utils/hpxml.py:42,1728-1730`), not from any
   vehicle element. The only definitions of `ElectricVehicles`/
   `ElectricVehicle` anywhere in this workspace are HARES's own
   (`crates/hares-io/src/hpxml/equipment.rs`,
   `crates/hares-io/src/hpxml/resolve_der.rs:176-212`, one regression
   test, and the Python injection test
   `tests/python/test_ev_tariff_integration.py`, which injects at
   `</Systems>`). This element has no documented external source; it
   appears to be a HARES invention.

2. **Downstream of (1), `daily_drive_miles`, `range_anxiety_miles`,
   `departure_time`, `trip_duration`, and `event_day_ratio` are hardcoded
   constants in `build_actors_from_seeds`'s `ActorSeed::Ev` arm**
   (`crates/hares-core/src/dwelling/mod.rs:6967-7034`: `30.0`, `20.0`,
   `480.0`, `600.0`, `0.8`), with no `EvConfig` field to override any of
   them for an HPXML-declared EV. HPXML's standard `Vehicle` element
   already carries the real data these constants stand in for:
   `MilesDrivenPerYear`, `HoursDrivenPerWeek`, and
   `FuelEconomyCombined{Units,Value}` (v4 schema
   `HPXMLBaseElements.xsd:2382-2384`). Even once (1) is fixed to parse
   the correct element, this second layer still needs to read those
   fields and thread them through `EvConfig` → `Ev::actor_seed()` →
   `ActorSeed::Ev` → `EvDriverActor::new`'s `daily_drive_miles`,
   `range_anxiety_miles`, `departure_time`, `trip_duration`,
   `event_day_ratio` parameters (`crates/hares-core/src/actors/
   ev_driver/mod.rs:313-330`) in place of the current literals.

## Consequence — I-05's completion bar is not fully reachable

I-05 ("Fix additional EV issues") fixes the actor/equipment silence
contract so a `ChargingStrategy`'s decision to idle is actually honoured
by the equipment (previously, silence let the BMS default to
charge-on-plug-in regardless of strategy). That fix is complete and
verified independent of this ticket. But for the fixture I-05's own
ticket names, `anxiety_soc` (computed from the hardcoded `30 mi` +
`20 mi` buffer, `needs_range_anxiety_override`,
`crates/hares-core/src/actors/ev_driver/mod.rs:715-742`) works out to
roughly `0.25-0.28` — *above* the ticket's own configured
`LowSoc { threshold: 0.15 }`. SOC cannot reach `0.15` without first
crossing the anxiety band, so for that specific configuration the
range-anxiety override — not the strategy's own gate — continues to
decide when the vehicle charges, even after I-05's silence-contract fix
lands. This is a distinct, secondary mechanism from the one I-05's RCA
diagnosed (documented there as "hardcoded driving parameters for
HPXML-declared EVs" and left out of that initiative's scope).

## Required Behavior

1. Determine whether `Systems/ElectricVehicles/ElectricVehicle` should be
   removed and replaced with parsing `BuildingDetails/Vehicles/Vehicle`
   (the standard path) — including deriving `capacity_kwh` from
   `Battery/{NominalCapacity,UsableCapacity}` and `max_charging_power_kw`
   from the linked `ElectricVehicleCharger/ChargingPower` via
   `ConnectedCharger` — or, if `Systems/ElectricVehicles` must be
   retained for an existing fixture-compatibility reason not evident from
   this review, document that reason and its source explicitly (this
   review found none).
2. Regenerate the missing `docs/hpxml/` reference
   (`docs/hpxml/fetch_hpxml_dd.py` per the constitution) before finalizing
   the element mapping, so the mapping is checked against the
   grep-friendly reference the constitution requires for HPXML parser
   work, not only the raw XSD.
3. Parse `MilesDrivenPerYear`, `HoursDrivenPerWeek`, and
   `FuelEconomyCombined` from the resolved `Vehicle` element (unit-guarded
   per `docs/hpxml/hpxml-units.md` once regenerated) and add the
   corresponding `Option<f64>` fields to `EvConfig`
   (`crates/hares-equipment/src/ev/config.rs`), following the existing
   `ready_soc`/`charging_strategy` optional-override pattern, with
   `EvConfig::validate` entries (finite, non-negative) matching the
   existing `soc_max`/`ready_soc` validation.
4. Thread the new fields through `Ev::actor_seed()` →
   `ActorSeed::Ev` (`crates/hares-equipment/src/lib.rs:65-71`) →
   `build_actors_from_seeds`'s `ActorSeed::Ev` arm
   (`crates/hares-core/src/dwelling/mod.rs:6967-7034`), replacing the
   hardcoded `30.0`/`20.0`/`480.0`/`600.0`/`0.8` literals with
   `.unwrap_or(<same value>)` defaults so every config that does not set
   the new fields is behaviourally unchanged.
5. Re-verify every existing HPXML EV fixture and test that currently
   constructs the non-standard `Systems/ElectricVehicles/ElectricVehicle`
   element (including `test_ev_tariff_integration.py`'s `</Systems>`
   injection point) against whichever mapping (1) settles on.

## Note on units

`EvDriverActor`'s existing constructor already takes several imperial
fields (`daily_drive_miles`, `range_anxiety_miles`,
`average_speed_mph`, `crates/hares-core/src/actors/ev_driver/mod.rs:
313-330`) — a pre-existing deviation from the constitution's "public
API surface outside `hares-io` is SI-only" rule, not introduced by this
ticket. Whether to fix that surface at the same time as adding the new
`EvConfig` fields, or to convert at the `hares-io` boundary only (leaving
the actor's existing imperial fields as a separately-scoped cleanup), is
a design decision for whoever picks this up — flagged here so it is not
rediscovered from scratch.
