# RV-016: OCHRE Parity Gaps Design

## 1. Islanded / Resilience Mode

### OCHRE Behavior

OCHRE implements resilience mode in `Dwelling.py:248-272`. The logic has two phases:

**Phase 1 — Input override (`update_inputs`, line 214-220):**
When `Voltage (-) == 0` is received from the external schedule/co-sim, OCHRE overrides it to `1.0` before passing to equipment. This lets all equipment run their normal models for the first pass.

**Phase 2 — Post-model check (`update_model`, line 248-272):**
After all equipment have been stepped, OCHRE checks:
- If `voltage > 0` OR `abs(total_p_kw) < 0.001`: grid-connected or self-sufficient islanded operation. No action needed.
- Otherwise (grid disconnected and load not met): OCHRE resets all dwelling power to zero, zeros all zone internal/HVAC gains and surface gains, then sets `Voltage (-) = 0` on every electric equipment's schedule and **re-runs the entire model** via `super().update_model()`.

Key detail: `Equipment.is_electric` defaults to `True` for ALL equipment (`Equipment.py:13`), including gas furnaces and gas boilers — these have electric blower fans and therefore require grid power. Gas furnaces inherit `is_electric = True` from the `Heater` base class and ARE shed in islanded mode. The only equipment that sets `is_electric = False` is tank-type gas water heaters with EF < 0.7 (`WaterHeater.py:713`), which have no electric ignition or controls. All other equipment — including all gas furnaces, gas boilers, and heat pumps — is treated as electric and gets shed when the grid is disconnected.

The re-step recalculates thermal gains with only non-electric equipment (gas water heaters with EF < 0.7) contributing. All electric loads are forced off.

### Proposed HARES Design

HARES already has `Dwelling::set_grid_voltage(voltage_pu)` and a `GridState` struct with `voltage_pu` in `hares-types/src/environment.rs:173`. The infrastructure exists but no shed/re-step logic fires when `voltage_pu == 0`.

**Implementation approach:**

1. Add an `is_electric: bool` flag on `EquipmentDescriptor` (default `true`), mirroring OCHRE's `Equipment.is_electric`. This is NOT a fuel-type check — gas furnaces and gas boilers are electric (blower fans) and must be shed. Only gas water heaters with EF < 0.7 set `is_electric = false`. A `FuelType`-based predicate would incorrectly spare gas furnaces.

2. In `Dwelling::step()`, after the normal equipment step loop, check `self.environment.grid().voltage_pu == 0.0`. If total electric draw > epsilon:
   - Zero all zone internal/HVAC/surface gains (mirrors OCHRE line 260-266).
   - For each equipment with `is_electric == true`, inject a `voltage_pu = 0.0` override into its environment snapshot.
   - Reset electric equipment's `PortContribution` entries in the `PortSlots` system so the re-step does not double-count prior contributions.
   - Skip the actor/dispatch phase on re-step — only re-run equipment `step()` with the mutated environment. Actors have already made their decisions for this timestep.
   - Re-run the equipment step loop. Non-electric equipment (gas water heaters with EF < 0.7) runs normally; electric equipment sees zero voltage and produces zero output.

   Note: The PortSlots reset and selective re-step is a significant implementation concern. The follow-up ticket must include careful design of the reset mechanism to avoid corrupting accumulator state.

3. Expose `Dwelling::is_islanded() -> bool` for Python/RL observability.

4. The re-step must not double-count gas equipment output. Either: (a) re-step only electric equipment, or (b) reset all accumulators before the full re-step (OCHRE does (b) — zeroes `total_p_kw`, `total_q_kvar`, `total_gas_therms_per_hour` before re-running all equipment).

**Prefer approach (b)** to match OCHRE semantics exactly. The full re-step is cleaner and avoids partial-state bugs.

### Test Plan

1. **Grid-connected baseline:** `voltage_pu = 1.0`, electric HVAC runs normally, power draw > 0.
2. **Islanded with electric load:** `voltage_pu = 0.0`, electric furnace + AC both shed, `total_electric_kw == 0`, zone gains zeroed.
3. **Islanded with gas furnace:** `voltage_pu = 0.0`, gas furnace IS shed (blower fan is electric, `is_electric = true`). Heating output = 0 during islanded mode. Gas consumption == 0, electric consumption == 0.
4. **Islanded self-sufficient:** `voltage_pu = 0.0` but battery/PV covers load (total_p_kw near zero). No shed occurs — equipment continues normally.
5. **Transition:** Step with `voltage_pu = 1.0`, then `voltage_pu = 0.0`, then `voltage_pu = 1.0`. Verify equipment resumes normally after grid reconnection.
6. **Telemetry:** `is_islanded()` returns correct value. Shed events appear in telemetry output.

### Follow-up Ticket

**RV-016a: Implement islanded/resilience mode**

Scope:
- Add `is_electric: bool` field on `EquipmentDescriptor` (default `true`). Gas water heaters with EF < 0.7 set `is_electric = false`.
- Implement post-step voltage check and re-step logic in `Dwelling::step()`, including `PortSlots` contribution reset for electric equipment.
- Design the selective re-step mechanism: reset `PortContribution` entries, skip actor/dispatch, re-run equipment `step()` only.
- Add `Dwelling::is_islanded()` accessor.
- Expose islanded state in Python `DwellingWrapper`.

Acceptance criteria:
- All 6 test scenarios above pass.
- No allocation in the re-step path (reuse existing equipment step infrastructure).
- Non-electric equipment (gas water heaters with EF < 0.7) output is preserved exactly during islanded mode. Gas furnaces and boilers are correctly shed.

## 2. Generic Heater / Cooler

### OCHRE Behavior

`Equipment/__init__.py:36-101` defines `EQUIPMENT_BY_NAME`. The catch-all types are:

- `Heater` (line 41) — `name = "Generic Heater"`, `end_use = "HVAC Heating"`. Defined at `HVAC.py:644-651`. Single-speed, static capacity, constant EIR. No biquadratic performance curves. No airflow modeling (not a `DynamicHVAC` subclass). Accepts optional inputs for deadband, capacity, and max capacity fraction.
- `Cooler` (line 53) — `name = "Generic Cooler"`, `end_use = "HVAC Cooling"`. Defined at `HVAC.py:654-661`. Same structure as `Heater` but with cooling end-use.

These are pure resistance-model equivalents: constant COP (via EIR), no weather-dependent performance, no wet-bulb corrections. They serve as fallbacks when HPXML specifies a system type that does not map to a specific equipment class (e.g., portable heaters, radiant panels, or unspecified fuel types).

Specific types that inherit from `Heater`: `ElectricFurnace`, `ElectricBoiler`, `ElectricBaseboard`, `GasFurnace`, `GasBoiler`. From `Cooler`: `AirConditioner` (via `DynamicHVAC`).

### Proposed HARES Design

HARES already has `HvacEquipmentType::Other` in `hvac_core.rs:57` but it is not wired to any concrete equipment implementation.

**Implementation approach:**

1. Add a `GenericHvac` equipment struct in `hares-equipment/src/hvac/`. It wraps `HvacEquipment` with `HvacEquipmentType::Other` (or split into `GenericHeater` / `GenericCooler` variants).

2. Model: constant capacity (W) and constant EIR. No biquadratic curves. No airflow/duct losses. No SHR adjustment. Power = `capacity * eir`. Heat delivered = `capacity * space_fraction`. This matches the base `HVAC` class in OCHRE when not subclassed by `DynamicHVAC`.

3. Support both electric and gas fuel types via a `fuel_type` config field, matching the `Furnace` pattern already in HARES.

4. Wire into HPXML resolver (`hares-io/src/hpxml/equipment.rs`): when system type does not match any specific equipment, fall back to `GenericHvac` instead of returning an error.

5. Thermostat integration: reuse existing `HvacEquipment` thermostat/deadband/setpoint logic. No new control surface needed.

6. Fan power: OCHRE's base `HVAC` class includes fan power consumption (`HVAC.py` `fan_power` attribute) even for the generic heater/cooler. HARES's `GenericHvac` should include a configurable `fan_power_w: f64` parameter (default 0.0) so that users can model blower fan draw. If omitted, this is a known divergence from OCHRE that should be documented.

**Keep it minimal.** This is a catch-all, not a new physics model. The entire implementation should be under 150 lines.

### Test Plan

1. **Constant heating:** Generic heater with 10 kW capacity, EIR 1.0, electric. Verify power draw == 10 kW, heat delivered == 10 kW when on.
2. **Constant cooling:** Generic cooler with 5 kW capacity, EIR 0.33 (COP 3). Verify power draw == 1.65 kW, heat removed == 5 kW.
3. **Gas fuel type:** Generic heater with `FuelType::Gas`, AFUE 0.80. Verify gas consumption, no electric draw.
4. **Thermostat cycling:** Generic heater with deadband. Verify on/off cycling around setpoint matches existing HVAC thermostat behavior.
5. **HPXML fallback:** HPXML config with unrecognized system type resolves to generic equipment without error.

### Follow-up Ticket

**RV-016b: Implement Generic Heater/Cooler equipment**

Scope:
- New `GenericHvac` struct in `hares-equipment/src/hvac/generic.rs`.
- HPXML fallback mapping in `hares-io/src/hpxml/equipment.rs`.
- Reuse `HvacEquipment` thermostat logic; no new control signals.

Acceptance criteria:
- All 5 test scenarios above pass.
- ResStock HPXML configs with "Other" or unrecognized heating/cooling types parse without error.
- No biquadratic curves or airflow modeling — constant capacity/EIR only.

## 3. EVI-Pro Stochastic EV Schedules

### OCHRE Behavior

`EV.py:115-200` implements `generate_events()`. The stochastic schedule generation works as follows:

1. **PDF data files:** Pre-computed joint probability tables from EVI-Pro, stored as CSV files named `pdf_Veh{N}_{Level}.csv`. Each file contains rows with `day_id`, `start_time` (minutes from midnight), `duration` (minutes), `start_soc` (0-100), grouped by `weekday` (bool) and `temperature` (integer, rounded to 5C bins, clipped to [-20, 40]).

2. **Vehicle classification:** Vehicle number (1-4) selected from `vehicle_type` (PHEV/BEV) and range: PHEV20, PHEV50, BEV100, BEV250 (`EV.py:65-70`).

3. **Event day ratio:** Probability that any given day has a charging event. Varies by charging level and battery capacity (`EV.py:126-137`): Level 1 = 0.9, large BEV = 0.2, medium BEV = 0.33, small/PHEV = 0.5.

4. **Day matching:** For each simulation day, compute daily average ambient temperature (rounded to nearest 5C) and weekday/weekend. Use these as keys to look up matching `day_id` groups from the PDF table. Randomly sample one `day_id` per simulation day (`EV.py:164-168`).

5. **Event assignment:** For each day, with probability `event_day_ratio`, pull all events for the sampled `day_id`. Convert `start_time` from minutes-from-midnight to absolute timestamps (`EV.py:172-176`).

6. **Overlap resolution:** OCHRE only checks cross-`day_id` overlaps — events within the same sampled `day_id` are assumed non-overlapping. If two events from different `day_id`s overlap (gap < 1 hour), the first event is truncated. Events shorter than 1 hour after truncation are removed. The fix is non-transitive: truncating event A may create a new overlap with event B, but OCHRE does not re-check (`EV.py:189-197`).

7. **SOC tracking:** `start_soc` from the PDF is used as arrival SOC. `end_soc` is computed from `start_soc + max_power * efficiency * hours / capacity`, clipped to 1.0 (`EV.py:206-208`). Unmet load from delayed charging carries forward to reduce the next event's start SOC (`EV.py:218-234`).

OCHRE also has `ScheduledEV` (`EV.py:364`) — a non-stochastic variant using a fixed load profile, not controllable.

### Proposed HARES Design

**This belongs in the Python adapter layer**, not in Rust core. Rationale:

- The stochastic generation runs once at init, not in the hot loop. No performance benefit from Rust.
- It requires `numpy.random` for sampling and `pandas` for time-series manipulation. Reimplementing in Rust would mean porting the PDF lookup and overlap resolution logic with no physics benefit.
- HARES's Rust EV model (`hares-core/src/actors/ev_driver/`) already accepts a schedule of charging events. The stochastic generator just needs to produce that schedule.
- Users who want deterministic/custom schedules can skip the generator entirely.

**Implementation approach:**

1. Add `ochre_next.schedules.ev_schedule_generator` Python module. It reads EVI-Pro PDF CSV files and generates a list of `ChargingEvent(start_time, duration, start_soc)` dicts.

2. The generator accepts: `vehicle_type` (PHEV/BEV), `capacity_kwh`, `charging_level` (1/2), `ambient_temps` (daily series), `start_date`, `end_date`, `seed` (for reproducibility).

3. Output format: list of dicts compatible with HARES EV driver `ChargingEvent` struct. The Python adapter converts these to the Rust-expected format before passing to `Dwelling::add_ev_schedule()`.

4. Ship the EVI-Pro PDF CSV files as package data (they are small, ~100KB total for 9 files: 4 vehicle types x 2 charging levels, plus a Level0 file for Vehicle 1).

5. Expose a `seed` parameter for deterministic testing. OCHRE uses bare `np.random.rand()` / `np.random.choice()` which is not reproducible — we use `numpy.random.Generator` with explicit seeding.

**Do not add `rand` or stochastic logic to the Rust crates.** The Rust EV driver consumes a deterministic event schedule; randomness lives in Python.

### Test Plan

1. **Deterministic seeding:** Same seed produces identical event schedules across runs.
2. **Event count distribution:** Over 365 days, event count matches expected `days * event_day_ratio` within statistical tolerance (chi-squared test with generous bounds).
3. **Temperature stratification:** Cold-day events differ from warm-day events (different day_id distributions).
4. **Weekday/weekend split:** Weekday events differ from weekend events.
5. **Overlap resolution:** Synthetic overlapping events are correctly truncated; events shorter than 1 hour after truncation are removed.
6. **SOC bounds:** All generated `start_soc` values in [0, 1]. All `end_soc` values in [0, 1].
7. **Round-trip integration:** Generated schedule feeds into HARES `Dwelling` EV driver, simulation runs without error, EV charges during scheduled windows.

### Follow-up Ticket

**RV-016c: Implement EVI-Pro stochastic EV schedule generator**

Scope:
- New Python module `ochre_next/schedules/ev_schedule_generator.py`.
- Ship EVI-Pro PDF CSV data files from `vendors/OCHRE/ochre/defaults/EV/`.
- Integration with `DwellingWrapper` to accept generated schedules.
- Explicit `seed` parameter on all RNG paths.

**Scope note:** The `ChargingEvent` struct and `Dwelling::add_ev_schedule()` API referenced in the design above do not exist yet. This ticket must design and implement both the Rust-side `ChargingEvent` type (in `hares-core` or `hares-types`) and the `add_ev_schedule` method on `Dwelling` / `DwellingWrapper` before the Python generator can integrate with them.

Acceptance criteria:
- All 7 test scenarios above pass.
- Generator runs in < 1 second for a 1-year schedule.
- No `rand` crate additions to any Rust crate.
- `ChargingEvent` struct and `add_ev_schedule()` API are implemented and documented.
- Generated schedules are compatible with the new HARES EV driver `ChargingEvent` format.
