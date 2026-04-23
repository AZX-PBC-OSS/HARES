HARES HVAC Telemetry, Tracing & Debuggability Assessment
Executive Summary
Current debuggability rating: Adequate for production parity, Needs Improvement for diagnostic depth
HARES provides a structured telemetry system with 16+ channels per cooling equipment, a tiered output verbosity system (0–8), and a feature-gated observe mode for deep step-level inspection. However, significant gaps exist between what the internal HVAC state machines compute and what surfaces to developers. The system matches OCHRE's output columns at equivalent verbosity levels but does not exceed them, and several critical decision-traceability and energy-attribution gaps remain.
---
1. Telemetry / Observability Architecture
1.1 Internal Telemetry System (hares-types::Telemetry)
Location: crates/hares-types/src/telemetry.rs
The Telemetry struct is a HashMap<String, f64> with a panic-on-unknown-key set() method. Keys are centralized in telemetry_keys.rs (~164 constants covering electrical, thermal, operating state, HVAC, battery, PV, EV, and water heating).
Per-equipment HVAC telemetry channels (AirConditioner example):
Channel	Key Constant
electric_kw	ELECTRIC_KW
sensible_cooling_w	SENSIBLE_COOLING_W
latent_cooling_w	LATENT_COOLING_W
shr	SHR
operating_mode	OPERATING_MODE
speed_index	SPEED_INDEX
cop	COP
runtime_fraction	RUNTIME_FRACTION
compressor_kw	COMPRESSOR_KW
fan_kw	FAN_KW
supply_temp_c	SUPPLY_TEMP_C
apparatus_dew_point_c	APPARATUS_DEW_POINT_C
bypass_factor	BYPASS_FACTOR
max_capacity_fraction	MAX_CAPACITY_FRACTION
heating_setpoint_c	HEATING_SETPOINT_C
cooling_setpoint_c	COOLING_SETPOINT_C
Furnace telemetry is sparser — only 7–9 channels: FAN_KW, ELECTRIC_KW, FUEL_INPUT_W (gas only), THERMAL_OUTPUT_W, OPERATING_MODE, SUPPLY_AIR_TEMP_C, HEATING_SETPOINT_C, COOLING_SETPOINT_C, and SPEED_INDEX (gas only).
1.2 Output Verbosity Tiers
Location: crates/hares-io/src/output/columns.rs
Verbosity	HVAC-Specific Columns
0	Total Electric/Gas Power only
1	Per-equipment Electric/Gas Power
2	Zone temperatures, Unmet Load, Ground temp
3	Mode, Setpoint, SOC per equipment
4	Energy (kWh), HVAC Heating Delivered (W), HVAC Cooling Delivered (W)
5	Reactive power, power factor, Net Sensible Heat Gain
6	Envelope component loads per boundary
7	Schedule inputs, detailed equipment modes, Capacity (W), COP
8	Additional diagnostics
1.3 tracing Usage in HVAC
Only 5 tracing calls exist across the entire HVAC crate:
File	Level
hvac_core.rs:530	warn
staging.rs:287	debug
staging.rs:301	warn
ac_config.rs:185	warn
duct_distribution.rs:9	warn
Finding: Thermostat mode transitions, speed changes, startup degradation, and ideal-capacity decisions produce zero tracing output. A developer cannot diagnose why the thermostat chose Heating vs Deadband from logs alone.
---
2. State Inspection: Observable vs Hidden
2.1 HvacEquipment (hvac_core.rs)
The HvacEquipment struct has 49 fields. Here's the observability breakdown:
State	Observable via Telemetry?	Observable via CoreOutput?
mode (ThermostatMode)	✅ OPERATING_MODE	✅ state.operating_mode
duty_cycle	❌ Not written	❌
last_speed_index	✅ SPEED_INDEX	❌
last_speed_frac	❌ Not written	❌
time_at_current_speed_s	❌	❌
plf_state	❌	❌
startup.time_since_start_min	❌	❌
startup.c_d	❌	❌
effective_setpoints() result	✅ HEATING/COOLING_SETPOINT_C	❌
schedule_setpoints	❌ Not written separately	❌
runtime_setpoints	❌ Not written separately	❌
mode_start_at / last_mode_switch_at	❌	❌
disabled_speeds	❌	❌
prev_zone_temp_c	❌	❌
duct_dse	✅ (via THERMAL_OUTPUT_W = gross * dse)	❌
zone_heat_fractions	❌	❌
max_capacity_fraction	✅ MAX_CAPACITY_FRACTION	❌
2.2 Thermostat (thermostat.rs)
ThermostatConfig is observable at init but not at runtime:
- hysteresis_c, cutout_ratio, deadband_offset — set once, never logged in telemetry
- min_cycle_time_s — enforced silently, no tracing when it blocks a transition
- The FSM transition logic (update_mode) returns the mode but never explains which threshold triggered the transition
2.3 Speed Control (speed_control.rs / staging.rs)
- SpeedSelection (speed_index, speed_frac, part_load_ratio) — speed_index is written to telemetry, but speed_frac and part_load_ratio are not
- part_load_factor result — written to internal plf_state but not to telemetry
- Startup capacity degradation multiplier — computed and applied but never exposed
---
3. Type Definitions (hares-types)
Location: crates/hares-types/src/
Key types for HVAC output:
Type	Purpose
Telemetry	Per-equipment scalar channels
TelemetryField	Schema descriptor (name, unit, description)
CoreOutput	Cross-cutting output (electric, fuel, mode, SOC)
CoreFlows	Energy/power flows
CoreState	Operating mode + SOC only
DwellingTelemetry	RL/control observation vector
StepResult	Per-step dwelling output
Critical gap: CoreOutput carries operating_mode and electric_kw but no thermal output, no COP, no capacity, no speed, no setpoint. The record_step method in dwelling/mod.rs accesses telemetry directly (with comments like "allowed: setpoint remains telemetry-only until CoreOutput gains setpoint fields") confirming this is a known debt.
---
4. Python Bindings
Location: python/ochre_next/_hares.pyi
The Python surface provides:
API
StepResult
Telemetry.equipment()
Equipment.telemetry()
Equipment.core_output
EquipmentDescriptor.telemetry_fields
Assessment: Python users can access the full telemetry map via Equipment.telemetry(), which returns all 16 AC channels. However, StepResult (the primary per-step return type) only carries aggregate hvac_heating_w / hvac_cooling_w — no per-equipment breakdown, no COP, no mode. A Python developer must iterate Dwelling.equipment() to get HVAC detail.
---
5. Test Infrastructure
HVAC unit tests exist in:
- hvac_core.rs — thermostat FSM, cutout ratio, min cycle time, setpoint overrides
- thermostat.rs — (no separate test module; tested through hvac_core)
- speed_control.rs — StartupConfig ramp behavior
- staging.rs — PLF, speed selection for all SpeedControlMode variants
- air_conditioner.rs — not shown but implied (extensive CoolingCore tests)
- furnace.rs — power balance, DSE, state round-trip, typed config
Observability test gaps:
- No test verifies that telemetry keys are populated correctly after a step
- No test checks that output CSV schema matches telemetry channels
- The observe feature (ObserverBuffer) is not tested in the HVAC crate
---
6. Dwelling/Simulator Level
6.1 Output Recording (record_step)
Dwelling::record_step() writes to the StreamingRecorder based on pre-resolved column indices. For HVAC equipment:
- Electric power → from CoreOutput::flows.electric_kw
- Gas power → from CoreOutput::flows.fuel_w
- Mode → from CoreOutput::state.operating_mode
- Setpoint → from Telemetry (heating or cooling, whichever is non-zero)
- Capacity → from Telemetry (THERMAL_OUTPUT_W or SENSIBLE_COOLING_W)
- COP → from Telemetry
Key finding: The CSV output columns at verbosity 7 include Capacity (W) and COP (-) but NOT:
- SHR
- Speed index (speed_frac)
- Runtime fraction (PLR/RTF)
- Compressor power vs fan power breakdown
- Supply air temperature
- Duct losses (separate from delivered)
- Startup degradation multiplier
- Latent gains (cooling only at OCHRE verbosity 7)
6.2 Observer System
The #[cfg(feature = "observe")] system provides:
- StepSnapshot with full phase-by-phase captures
- EquipmentObservation with telemetry + port contributions + contribution deltas
- DispatchCapture tracking all control signals and their resolution
Assessment: This is the richest debugging tool in the system, but it's feature-gated off by default and only available via the observe feature flag. It captures telemetry snapshots at each phase boundary, which would allow reconstructing thermostat decisions if the feature is enabled.
6.3 Diagnostics Module
crates/hares-core/src/diagnostics.rs provides a simple CSV diagnostic writer with zone temps, thermal gains, latent gains, and electrical net power. Equipment diagnostics are captured but the EquipmentDiag struct only has name, mode, electric_kw, sensible_gain_w — far less than the telemetry map.
---
7. Comparison with OCHRE
OCHRE's HVAC.generate_results() at verbosity 7 outputs:
OCHRE Column	HARES Equivalent
{end_use} Delivered (W)	HVAC Heating/Cooling Delivered (W) (v4)
{end_use} Setpoint (C)	Setpoint (C) (v3)
{end_use} COP (-)	COP (-) (v7)
{end_use} Duct Losses (W)	❌ Not in HARES output schema
{end_use} Main Power (kW)	❌ Not in HARES output (compressor_kw in telemetry only)
{end_use} Fan Power (kW)	❌ Not in HARES output (fan_kw in telemetry only)
{end_use} Latent Gains (W)	❌ Not in HARES output
{end_use} SHR (-)	❌ Not in HARES output (in telemetry only)
{end_use} Speed (-)	❌ Not in HARES output schema (in telemetry only)
{end_use} Capacity (W)	✅ Capacity (W) (v7)
{end_use} Max Capacity (W)	❌ Not in HARES output
{end_use} ER Power (kW)	❌ Not in HARES output (HP heater has in telemetry)
Summary: HARES matches OCHRE at v4 (delivered + setpoint + COP) but falls behind at v7 where OCHRE exposes component power, SHR, speed, latent, and duct losses. HARES has this data in telemetry but does not route it to the output schema.
---
8. Gap Analysis by Diagnostic Question
Q1: Can a developer diagnose unexpected HVAC behavior?
Partially. They can see:
- What mode the equipment is in (via output v3 or telemetry)
- What setpoint it's targeting (via output v3 or telemetry)
- How much power it's drawing (via output v1)
- What the zone temperature is (via output v2)
They cannot see:
- Why the thermostat chose this mode (which threshold triggered the transition)
- Whether a mode transition was blocked by min_cycle_time_s or min_on_time_s / min_off_time_s
- The effective vs static vs schedule vs runtime setpoint chain
- How long the unit has been in the current mode
- Whether startup degradation is still active
- The PLF/PLR that determined actual capacity
Rating: Needs Improvement
Q2: Can you trace exactly how much energy the HVAC consumed and why?
Partially. Total electric_kw is available. For cooling, compressor_kw and fan_kw are in telemetry. For furnaces, fan_kw and fuel_input_w are in telemetry. But:
- The component breakdown (compressor vs fan vs crankcase) is NOT in the output CSV schema
- The reason for the power level (PLR, PLF, startup multiplier, speed selection) is not exposed
- Gas furnace fuel_input_w is in telemetry but not routed to Capacity (W) in output correctly (output uses THERMAL_OUTPUT_W which is post-DSE, not fuel)
Rating: Needs Improvement
Q3: Can you trace how much heat the HVAC added/removed from each zone?
Yes at the aggregate level, No at the per-zone attribution level:
- HVAC Heating/Cooling Delivered (W) at v4 gives total delivered to the indoor zone
- The ThermalAccumulator port system correctly routes heat to multiple zones via zone_heat_fractions
- But the per-zone breakdown (conditioned zone vs duct zone vs basement zone) is not in any output column
- The ObserverBuffer captures port contributions per equipment, but only when the observe feature is enabled
Rating: Adequate for single-zone, Needs Improvement for multi-zone
Q4: Can you see why the thermostat made each decision?
No. The thermostat FSM in update_mode() computes next_mode based on zone temperature, setpoints, hysteresis, deadband_offset, and cutout_ratio. None of the intermediate calculations are logged or exposed. A developer seeing mode = Deadband when they expected Heating cannot determine whether:
- The zone temperature was above the turn-on threshold
- is_cycle_change_allowed() blocked the transition
- can_transition_mode() blocked it due to min on/off time
- The setpoint was overridden by schedule or control signal
Rating: Poor
Q5: Is the output rich enough to reconstruct HVAC behavior from logs alone?
No. At the maximum output verbosity (v8), you get mode, setpoint, power, capacity, and COP. But you cannot reconstruct:
- The actual PLR/PLF applied
- The startup degradation state
- The speed control decision (which stage, what interpolation fraction)
- The biquadratic curve inputs/outputs (indoor wet-bulb, outdoor temp corrections)
- The duct DSE effect (only delivered is shown, not gross vs loss)
- The SHR and latent/sensible split (cooling)
OCHRE's reference CSV includes all of these at v7+. The observe feature captures telemetry snapshots that would help, but it's not on by default.
Rating: Needs Improvement
---
9. Recommendations (Priority-Ordered)
P0 — Thermostat Decision Logging
Add tracing::debug! calls in update_mode() and can_transition_mode() that log:
- Current zone temp, effective setpoints (heating/cooling), hysteresis
- Which threshold triggered the mode transition (or why it was blocked)
- The min-cycle-time and min-on/off-time enforcement outcomes
Impact: Directly addresses the "why did it do that?" question. Zero performance cost when debug logging is off.
P1 — Missing Output Columns at High Verbosity
Add the following columns to the output schema at v7 (matching OCHRE parity):
- {name} SHR (-) — for cooling equipment
- {name} Speed (-) — speed index (already in telemetry as SPEED_INDEX)
- {name} Fan Power (kW) — already in telemetry as FAN_KW
- {name} Main Power (kW) — compressor-only for cooling, element-only for furnace
- {name} Duct Losses (W) — gross * (1 - dse)
- {name} Runtime Fraction (-) — PLR/PLF
These are already computed and stored in Telemetry; they just need routing to the output schema.
P2 — CoreOutput Extension
Promote key HVAC fields from telemetry to CoreOutput:
- thermal_output_w: Option<f64> (heating) or sensible_cooling_w / latent_cooling_w (cooling)
- cop: Option<f64>
- speed_index: Option<u8>
This eliminates the "allowed: telemetry-only" hack in record_step() and makes the structured output self-describing.
P3 — Speed & Startup Telemetry Gaps
Add telemetry channels for:
- speed_frac (interpolation weight for multi-speed)
- part_load_ratio (the PLR before PLF)
- part_load_factor (the PLF applied)
- startup_multiplier (the Winkler degradation multiplier)
- duty_cycle (the thermostat's raw on/off fraction)
These are already computed internally; they just need telemetry.insert() at init and telemetry.set() in step().
P4 — Setpoint Chain Visibility
Add telemetry channels that distinguish the setpoint override chain:
- schedule_heating_c / schedule_cooling_c (from schedule)
- runtime_heating_c / runtime_cooling_c (from control signal)
Currently only the final effective_setpoints() result is exposed, making it impossible to tell whether a setpoint change came from the schedule, a control signal, or the static config.
P5 — Zone Heat Fraction Attribution
For multi-zone buildings, add per-zone thermal attribution in the output schema at v6+:
- HVAC Heating Delivered - {zone} (W) per zone in zone_heat_fractions
This is critical for duct-zone and basement-zone debugging.
P6 — Observe Feature Default for Debug Builds
Consider making observe a debug_assertions feature (on in debug, off in release) rather than an explicit feature flag. This would give developers step-by-step telemetry snapshots in development without any opt-in.
---
10. Summary Ratings
Diagnostic Capability	Rating
Energy consumption visibility	Adequate
Thermal contribution visibility (single zone)	Adequate
Thermal contribution visibility (multi-zone)	Needs Improvement
Control decision visibility (thermostat)	Poor
Control decision visibility (speed/PLF)	Needs Improvement
Time-series output reconstructability	Needs Improvement
OCHRE parity at equivalent verbosity	Needs Improvement
Structured API (Python/Rust)	Adequate
Deep debugging (observe feature)	Good
Test coverage for telemetry correctness	Needs Improvement
Overall: Adequate for production parity, Needs Improvement for diagnostic depth. The telemetry infrastructure is well-designed (centralized keys, typed fields, panic-on-unknown-key), but the HVAC state machines are "data furnaces" — they consume inputs and produce outputs without logging their internal reasoning. The biggest single improvement would be P0: thermostat decision logging (near-zero cost, maximal diagnostic value).