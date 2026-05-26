# No HRV/ERV balanced mechanical ventilation physics
**Review ID**: types-physics-12
**Category**: types-physics
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/ventilation.rs`
- `crates/hares-envelope/src/thermal_solver/infiltration.rs`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/HeatRecovery.cc`

## Findings

### Finding 1: [Severity: critical]
**Description**: The ventilation model is a supply-only effectiveness model, not a balanced HRV/ERV with two air streams. Only the supply stream is modeled; the exhaust stream mass flow and its thermal impact are entirely absent. This means the model does not implement a true HRV/ERV.

**Code Location**: `crates/hares-equipment/src/ventilation.rs:452-454`, `crates/hares-envelope/src/thermal_solver/infiltration.rs:196-218`

**Root Cause**: The `Ventilation::step()` method computes only supply-side conditions (T_supply, W_supply) and supply mass flow. There is:
- No exhaust/secondary air stream inlet (indoor exhaust air entering the HX core)
- No exhaust/secondary air stream outlet temperature or humidity calculation
- No SecOutTemp, SecOutHumRat, or exhaust mass flow rate tracking
- No heat balance between supply and secondary streams
- No `CMin/CSup` ratio correction for unbalanced supply/exhaust flows

EnergyPlus in `CalcAirToAirGenericHeatExch()` (HeatRecovery.cc:2212-2229) models both streams:
```cpp
CSup = this->SupOutMassFlow * PsyCpAirFnW(this->SupInHumRat);
CSec = this->SecOutMassFlow * PsyCpAirFnW(this->SecInHumRat);
CMin = min(CSup, CSec);
this->SupOutTemp = this->SupInTemp + this->SensEffectiveness * CMin / CSup * (this->SecInTemp - this->SupInTemp);
QSensTrans = CSup * (this->SupInTemp - this->SupOutTemp);
this->SecOutTemp = this->SecInTemp + QSensTrans / CSec;
```

HARES uses only (ventilation.rs:453):
```rust
let t_supply_c = t_outdoor_c + eff_s * (t_indoor_c - t_outdoor_c);
```
This omits the `CMin/CSup` term, implicitly assuming equal mass flows (valid only when supply = exhaust perfectly).

**Impact**: 
1. The exhaust mass flow rate is never computed, so its effect on the zone air mass balance is ignored. A true balanced HRV removes indoor air at T_indoor, W_indoor from the zone at the same mass rate as supply. This exhaust removal constitutes a negative infiltration load (removing warm/moist indoor air) that is not accounted for.
2. The `balanced` flag in `infiltration.rs:209-218` only applies a load-reduction factor `(1 - sensible_recovery_efficiency)` to the forced ventilation flow, approximating the net effect but not enforcing mass conservation.
3. Because the exhaust stream's removal of conditioned indoor air is missing, the zone energy balance is incorrect: the model effectively treats the exhaust side as a sink that vanishes, violating the first law.

### Finding 2: [Severity: high]
**Description**: The effectiveness formula omits the `CMin/CSup` (capacity rate ratio) multiplier required by heat exchanger theory. This implicitly assumes perfectly balanced mass flows, which is not valid when infiltration, natural ventilation, or unbalanced fan flows are present.

**Code Location**: `crates/hares-equipment/src/ventilation.rs:453`

**Root Cause**: The effectiveness equation for a heat exchanger with two streams of different heat capacity rates includes the ratio of minimum to supply-side capacity: `T_supply_out = T_supply_in + ε × (CMin/CSup) × (T_sec_in - T_supply_in)`. HARES drops the `CMin/CSup` term. When the exhaust stream has different mass flow (e.g., due to unbalanced fan sizing or infiltration interacting with forced ventilation), this produces incorrect supply air temperatures.

EnergyPlus (HeatRecovery.cc:2216):
```cpp
this->SupOutTemp = this->SupInTemp + this->SensEffectiveness * CMin / CSup * (this->SecInTemp - this->SupInTemp);
```

HARES (ventilation.rs:453):
```rust
let t_supply_c = t_outdoor_c + eff_s * (t_indoor_c - t_outdoor_c);
```

**Impact**: Supply air temperature is over- or under-estimated when supply and exhaust mass flows differ (which is the common case when infiltration is present). For example, if exhaust flow is 50% of supply flow, CMin = C_sec = 0.5 × C_sup, so CMin/CSup = 0.5, meaning only half the effectiveness should apply. HARES applies full effectiveness regardless.

### Finding 3: [Severity: high]
**Description**: Frost/defrost control is overly simplified (fixed derating multiplier) compared to EnergyPlus's three-mode frost control with time-varying defrost fractions, supply bypass mixing, and exhaust recirculation.

**Code Location**: `crates/hares-equipment/src/ventilation.rs:254-268`, `crates/hares-equipment/src/ventilation.rs:52-53`, `crates/hares-equipment/src/ventilation.rs:119-120`

**Root Cause**: HARES uses a single `defrost_temp_c` threshold and a fixed `defrost_effectiveness_fraction` (default 0.5) to derate effectiveness when outdoor temperature is below the threshold:
```rust
if t_outdoor_c < self.defrost_temp_c {
    base * self.defrost_effectiveness_fraction
}
```

EnergyPlus (HeatRecovery.cc:2900-3063) implements three distinct frost control strategies:
1. **MinimumExhaustTemperature** (line 2901): Iteratively solves for the supply bypass fraction needed to keep exhaust temperature above threshold; recalculates effectiveness at reduced flow rates; mixes bypass and core streams.
2. **ExhaustAirRecirculation** (line 2993): Uses `InitialDefrostTime` + `RateofDefrostTimeIncrease × (ThresholdTemp - SupInTemp)` to compute a time-fraction DFFraction; blends exhaust inlet air with core outlet on the supply side; derates mass flows on both sides.
3. **ExhaustOnly** (line 3029): Time-fraction-based supply bypass; heat transfer derated by `(1 - DFFraction)`; no effectiveness derating (HX operates at full effectiveness when not bypassed).

HARES config fields are insufficient:
- `defrost_temp_c`: single temperature threshold (EnergyPlus uses `ThresholdTemperature` per frost type)
- `defrost_effectiveness_fraction`: single scalar multiplier (EnergyPlus uses InitialDefrostTime, RateofDefrostTimeIncrease, and iterative solution)
- No concept of supply bypass mass flow during defrost
- No concept of exhaust air recirculation
- No defrost time fraction or time-varying behavior
- No fan power increase during defrost (in EnergyPlus, defrost typically affects fan operation)

**Impact**: The simplified model cannot capture defrost dynamics (e.g., frost builds over time, defrost cycles have time-varying effectiveness). At very low outdoor temperatures, energy consumption and thermal performance diverge significantly from validated EnergyPlus results. The fixed `defrost_effectiveness_fraction` of 0.5 is arbitrary and not tied to equipment-specific defrost curves.

### Finding 4: [Severity: medium]
**Description**: The `balanced` configuration flag in `VentilationConfig` is parsed and stored as a field but is never used within the `Ventilation` equipment model itself (only in the envelope solver builder).

**Code Location**: `crates/hares-equipment/src/ventilation.rs:57` (config field), `crates/hares-equipment/src/ventilation.rs:139` (parsing logic never checks it as an enum variant), `crates/hares-core/src/dwelling/solver_builder.rs:1053` (only consumer)

**Root Cause**: The `balanced: Option<bool>` field in `VentilationConfig` is read by `solver_builder.rs:1053` to set `thermal_cfg.ventilation.balanced`. However, the `Ventilation` equipment itself makes no distinction between balanced and unbalanced operation. The `step()` method computes the same supply air conditions regardless of whether the system is balanced. The ventilation type parser (`parse_ventilation_type`) only checks the `ventilation_type` string, not whether the system is balanced.

EnergyPlus enforces balanced operation implicitly through its node-based architecture — both the supply (primary) and secondary (exhaust) streams are connected to nodes in the HVAC loop, and the mass flow on both sides is determined by the upstream components. A true balanced HX in EnergyPlus always has both streams flowing through it.

**Impact**: The `balanced` flag controls whether the envelope solver applies the `(1 - recovery_efficiency)` factor in the flow combination. However, since the equipment emits the same effectiveness values regardless, a user could configure `balanced: false` with nonzero recovery efficiencies and get inconsistent results — the equipment reports recovery in telemetry but the thermal solver ignores it. Conversely, `balanced: true` with `ExhaustFan` type produces meaningless results since exhaust fans have zero recovery effectiveness by definition.

### Finding 5: [Severity: medium]
**Description**: Fan power is a single constant wattage, not separated into supply fan + exhaust fan power, and does not vary with defrost or bypass modes.

**Code Location**: `crates/hares-equipment/src/ventilation.rs:474`, `crates/hares-equipment/src/ventilation.rs:47`

**Root Cause**: The `fan_power_w` field represents total fan power, which is appropriate for a supply-only system but not for a balanced HRV/ERV that typically has two fans (one for supply, one for exhaust). Additionally, fan power is constant regardless of operating conditions — during frost control bypass, the supply fan may continue to run but at a different operating point; during exhaust-only defrost, the exhaust fan may continue while the supply fan is modulated.

EnergyPlus does not model fan power within the HX object itself — fan power is handled by separate fan objects upstream/downstream. However, EnergyPlus's `NomElecPower` field represents the HX's own electrical consumption (e.g., rotary wheel motor) and is separate from fan power.

**Impact**: During defrost or bypass, the model assumes full fan power is consumed regardless of actual airflow. This overestimates fan energy during bypass (when the supply fan may see reduced pressure drop) and during defrost (when fan operating conditions change).

### Finding 6: [Severity: low]
**Description**: The `bypass` mode simultaneously sets both sensible and latent effectiveness to zero, which is physically correct for a full bypass, but the model does not account for the bypass air path pressure drop reduction or the continued operation of the exhaust fan.

**Code Location**: `crates/hares-equipment/src/ventilation.rs:258-260`, `crates/hares-equipment/src/ventilation.rs:276-278`

**Root Cause**: The bypass logic correctly zeros effectiveness within the comfort temperature range (lines 443-445 and 258-260). However, in a real plate HX with bypass dampers, the supply air is routed around the HX core, reducing pressure drop. The fan may operate at a different point on its curve, and the exhaust fan continues to pull indoor air through the HX core (exhaust side typically does not bypass in residential HRVs). The current model treats bypass as purely an effectiveness change with no fan power or flow rate adjustments.

EnergyPlus (HeatRecovery.cc:2022-2033) tracks bypass mass flows explicitly:
```cpp
this->SupBypassMassFlow = this->SupInMassFlow;
this->SupOutMassFlow = this->SupInMassFlow;
this->SecBypassMassFlow = this->SecInMassFlow;
this->SecOutMassFlow = this->SecInMassFlow;
```

**Impact**: Minor energy discrepancy during bypass operation. The bypass mode is typically active only during mild outdoor conditions when heating/cooling loads are small, so the absolute error is limited.

## Summary
- Total findings: 6
- Critical: 1
- High: 2
- Medium: 2
- Low: 1

## Recommendations
1. **Implement a two-stream HRV/ERV model**: Add exhaust (secondary) stream mass flow tracking with inlet conditions from the indoor zone and outlet exhaust conditions to the outdoors. Compute supply outlet using the proper CMin/CSup formulation from heat exchanger theory. Ensure mass conservation by enforcing `m_dot_supply = m_dot_exhaust` for balanced systems, with the exhaust removal constituting a negative infiltration term in the zone air mass balance.
2. **Add CMin/CSup ratio correction** to the effectiveness equation: `T_supply_out = T_supply_in + ε × min(CSup, CSec)/CSup × (T_sec_in - T_supply_in)` with equivalent treatment for humidity ratio, following EnergyPlus HeatRecovery.cc:2212-2217.
3. **Implement EnergyPlus-style frost control modes**: Add `FrostControlType` enum (None, MinimumExhaustTemperature, ExhaustAirRecirculation, ExhaustOnly), `InitialDefrostTime`, `RateofDefrostTimeIncrease` fields to the config. Implement the iterative bypass fraction solver for MinimumExhaustTemperature mode and the time-fraction-based derating for the other modes, with proper bypass/recirculation mass flow tracking.
4. **Separate supply and exhaust fan power**: Either model two distinct fan loads, or at minimum acknowledge that the single `fan_power_w` represents combined supply + exhaust fan consumption for balanced systems.
5. **Ensure `balanced` flag consistency**: In the `Ventilation` equipment model, validate that `balanced: true` implies HRV/ERV type and that balanced systems emit both supply and exhaust telemetry. Reject configurations where `balanced: true` is set with `exhaust_fan` type.

## References / Citations
- EnergyPlus Engineering Reference, "Heat Exchangers" chapter: HeatExchanger:AirToAir:SensibleAndLatent model with separate supply (primary) and exhaust (secondary) air streams.
- `vendors/EnergyPlus/src/EnergyPlus/HeatRecovery.cc:2212-2229` — Full two-stream supply/secondary outlet calculation with CMin/CSup ratio.
- `vendors/EnergyPlus/src/EnergyPlus/HeatRecovery.cc:2811-3063` — FrostControl method with three defrost strategies.
- `vendors/EnergyPlus/src/EnergyPlus/HeatRecovery.cc:2900-2991` — MinimumExhaustTemperature frost control with iterative bypass fraction solution.
- `vendors/EnergyPlus/src/EnergyPlus/HeatRecovery.cc:2993-3027` — ExhaustAirRecirculation frost control with time-fraction blending.
- `vendors/EnergyPlus/src/EnergyPlus/HeatRecovery.cc:3029-3058` — ExhaustOnly frost control with supply bypass derating.
- `crates/hares-equipment/src/ventilation.rs:452-454` — Supply-only effectiveness calculation (missing CMin/CSup, no exhaust stream).
- `crates/hares-envelope/src/thermal_solver/infiltration.rs:196-236` — "Balanced" flow reduction using recovery efficiency (single-stream approximation).
- `crates/hares-core/src/dwelling/mod.rs:2638-2643` — Per-timestep effectiveness propagation from equipment to thermal solver config.
