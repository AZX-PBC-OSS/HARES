# Generator CHP thermal routing energy balance
**Review ID**: equip-der-07
**Category**: equipment-der
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/generator.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/Generator.py

## Findings
### Finding 1: [Severity: critical]
**Description**: Fluid port energy capacity is decoupled from the generator's computed thermal output. The generator computes `q_thermal_w = fuel_w * self.eta_thermal` internally (line 757), but writes fixed user-configured flow rate and temperatures to the fluid port (lines 808-815) that are independent of the generator's actual thermal power. The `PortContribution::Fluid` variant carries no energy field — only `flow_rate_kg_s`, `supply_temp_c`, and `return_temp_c` (hares-types `ports.rs` lines 76-82). The downstream fluid solver independently computes `cp * flow * delta_T` from these values, which has no guarantee of matching the generator's `q_thermal_w`. The default values (flow_rate=0.1 kg/s, ΔT=10°C, lines 208-214) imply ~4.2 kW thermal regardless of actual generator output, creating an energy balance closure failure between the generator's internal accounting and what the fluid solver sees.

**Code Location**: `crates/hares-equipment/src/generator.rs:757-815` (CHP computation and fluid port write), `hares-types/src/ports.rs:76-82` (Fluid port definition lacking energy field), `hares-types/src/ports.rs:322-369` (FluidAccumulator performing flow-weighted averaging only, no energy tracking)

**Root Cause**: The `PortContribution::Fluid` type was designed as a mass-flow/temperature interface rather than an energy/power interface. The generator model treats `q_thermal_w` as a tracking variable but does not couple it to the fluid port parameters. There is no code that computes `flow_rate * cp * (T_supply - T_return)` against `q_thermal_w`, and the fluid accumulator performs only flow-weighted temperature averaging with no energy summation or verification.

**Impact**: (1) At low generator output, the fluid port may over-report thermal energy to the hydronic loop. (2) At high generator output, significant CHP heat may be silently lost because the fixed flow/temperatures cannot transport it. (3) Tests confirm the decoupling: the CHP fluid port test at line 1973-1977 explicitly states "the fluid port carries flow, not watts directly -- just confirm it is active." (4) Compare with the thermal (zone) port which carries `sensible_gain_w` directly in watts and has verified energy balance in tests (lines 1954-1958, 1961-1965).

### Finding 2: [Severity: high]
**Description**: No configuration-time or runtime validation that the fluid port parameters (flow_rate_kg_s, supply_temp_c, return_temp_c) are physically consistent with the expected CHP thermal output. The generator's `GeneratorConfig::validate()` method (lines 81-144) checks the efficiency sum constraint (`eta_electric + eta_thermal <= 1.0`) and validates per-field numerical ranges, but performs no cross-check between CHP thermal output and fluid port parameters. For example, a 10 kW generator with eta_thermal=0.4 produces 4 kW of CHP heat at rated load, requiring a flow of ~0.095 kg/s at ΔT=10°C — so the defaults happen to work for rated output, but at part load (e.g. 2.5 kW electric, ~3.3 kW thermal at eta_thermal=0.4), the fluid port would still carry ~4.2 kW, a 27% overestimate.

**Code Location**: `generator.rs:81-144` (validate method), `generator.rs:208-214` (default fluid port parameters)

**Root Cause**: The fluid port parameters are designed as static configuration rather than runtime-computed quantities. There is no coupling function that computes flow from `q_thermal_w` or validates consistency.

**Impact**: Users can configure impossible or energy-violating scenarios without error. The fluid loop solver will see thermal energy that does not match the generator's internal energy balance. Building energy simulation results would be incorrect for CHP scenarios run at part load.

### Finding 3: [Severity: high]
**Description**: No parasitic electrical load accounting for CHP auxiliary equipment. In real CHP systems, circulating pumps, cooling fans, fuel compressors (for fuel cells), and control electronics consume a significant portion of the generator's electrical output (typically 2-10% of rated power for pumps and fans alone). The HARES generator model applies all computed `output_kw` directly to the electrical port (line 779) with no deduction for self-consumption or auxiliary loads. The `self_consumption_enabled` flag controls whether the generator follows net load, not whether it deducts parasitic loads. OCHRE's `Generator.py` has the same limitation — its `self.power_chp` field is declared but commented as "not implemented" (line 39). EnergyPlus `Generator:FuelCell` explicitly models parasitic loads including air compressor, fuel compressor, water pump, and ancillary power with separate sub-models.

**Code Location**: `generator.rs:779-782` (electrical port write with no parasitic deduction), `generator.rs:39` comment line: "self.power_chp = 0  # usable output heat... # not implemented" (OCHRE reference)

**Root Cause**: The model follows OCHRE's simplified approach where the generator's electrical output at the port is net-available power with parasitic losses implicitly bundled into the efficiency (P_fuel = P_electric / eta means losses appear as increased fuel consumption rather than explicit electrical deductions). However, for CHP systems, this approach conflates inefficiency with auxiliary loads that have distinct thermal consequences (pump motors add heat to the building; fan power does not contribute to useful CHP heat).

**Impact**: (1) Electrical output is overestimated by 2-10% relative to real CHP systems. (2) The generator appears to meet electrical loads that would actually be consumed by its own auxiliaries. (3) The thermal balance is distorted because pump/fan motor heat should be accounted separately from combustion losses.

### Finding 4: [Severity: medium]
**Description**: No differentiation between fuel cell and internal combustion engine CHP thermal characteristics. Both `GasGenerator` and `FuelCell` use the identical `Generator` struct with the same CHP physics (lines 445-479). In reality:
- ICE CHP: jacket water operates at 80-95°C (near engine coolant boiling point of ~105-120°C at pressure), exhaust gas heat recovery at 400-600°C, higher-grade heat suitable for space heating and DHW.
- Fuel cell CHP: stack cooling operates at 60-80°C, lower-grade heat, requires separate low-temperature hydronic loop or heat pump upgrade for space heating.

The default `supply_temp_c` of 70°C (line 211) is at the boundary between these regimes and is used identically for both equipment types. EnergyPlus `Generator:FuelCell` has separate sub-models for stack cooling water, reformer water, and exhaust gas heat recovery, each with distinct temperature ranges and heat exchanger UA values.

**Code Location**: `generator.rs:445-479` (shared Generator struct for both kinds), `generator.rs:210-214` (temperature defaults), `generator.rs:959-968` (newtype wrappers with no thermal differentiation)

**Root Cause**: The code comment at line 3-5 explicitly states "Both types share identical physics." This is valid for the electrical efficiency model but not for the CHP thermal model, where fuel cells and reciprocating engines have fundamentally different heat rejection architectures and temperature regimes.

**Impact**: (1) A fuel cell model using default temperatures of 70°C supply / 60°C return may overstate the usable heat grade — PEM fuel cells typically operate at 60-80°C stack temperature, making a 70°C supply temperature marginally achievable. (2) An ICE CHP model using 70°C supply understates the available temperature — engine jacket water at 90°C could serve higher-temperature hydronic loads. (3) There is no validation to prevent configuring an ICE CHP with supply_temp_c above the engine coolant boiling point (~105-120°C), which would be physically impossible.

### Finding 5: [Severity: medium]
**Description**: CHP thermal output is not load-dependent in temperature — the supply and return temperatures are constants regardless of generator output. In real CHP systems, part-load operation reduces the exhaust gas temperature and coolant temperature, reducing the grade of recoverable heat. The model applies constant temperatures (lines 811-812) even at low generator output where actual CHP supply temperature would drop. The test at line 2091-2098 confirms CHP telemetry fields are present but no test validates temperature behavior at part load. OCHRE has the same limitation — its `power_chp` is a simple scalar product with no temperature model. EnergyPlus models part-load temperature degradation through heat exchanger UA effectiveness relationships that depend on mass flow and source temperature.

**Code Location**: `generator.rs:808-815` (constant temperature fluid port write), `generator.rs:757` (thermal power scales with fuel but temperature does not)

**Root Cause**: The fluid port is a simple pass-through of user-configured constants with no physics-based temperature model. The thermal power `q_thermal_w` is computed correctly from `fuel_w * eta_thermal` and scales with load, but the temperature/supply grade does not.

**Impact**: At part load, the model may represent the CHP system as delivering the same high-grade heat as at full load, overstating the usefulness of the recovered heat for applications with minimum supply temperature requirements (e.g., absorption chillers, DHW heating).

### Finding 6: [Severity: medium]
**Description**: The default return temperature of 60°C (line 214) implies a very high-temperature return from the hydronic loop heat sink. For typical residential hydronic space heating with radiators or radiant floors, return temperatures are typically 30-50°C. A 60°C return temperature would only be seen in high-temperature radiator systems or DHW preheating. Together with the 70°C supply, the 10°C delta-T at 0.1 kg/s implies only ~4.2 kW thermal capacity, which limits the CHP model's ability to represent larger thermal recovery systems. If a user configures a larger generator with higher thermal output but does not adjust flow_rate_kg_s, the fluid port silently carries insufficient flow.

**Code Location**: `generator.rs:208-214` (default fluid port parameters), `generator.rs:651-653` (flow and temperature config reading with no consistency check)

**Root Cause**: The defaults are generic "reasonable" values with no derivation from generator size or CHP fraction. Compare with thermal zone ports where the energy is computed directly from the energy balance — the zone heat always matches the energy accounting regardless of user config.

**Impact**: Users must manually tune flow_rate_kg_s if the thermal output differs from the 4.2 kW implied by defaults. The config validation does not warn if power and flow parameters are inconsistent.

### Finding 7: [Severity: medium]
**Description**: The energy balance `P_fuel = P_electric + Q_thermal + Q_flue` is computationally verified (tests at lines 1800-1829 and 1925-1978) and the constraint `eta_electric + eta_thermal <= 1.0` is enforced at config validation (line 99-104). However, there is no config validation that `eta_thermal` is physically reasonable for the generator type. For example, a residential gas generator might have max eta_thermal of 0.35-0.45 (jacket water recovery only), while a fuel cell might achieve 0.40-0.55 (stack + reformer heat). The code allows any eta_thermal up to `1.0 - eta_electric - epsilon`, which could produce unrealistic thermal recovery claims. OCHRE has the same lack of type-specific thermal bounds (its `efficiency_chp` defaults to 0 with no upper bound constraint, lines 51-53).

**Code Location**: `generator.rs:99-104` (sum constraint only, no per-type bounds), `generator.rs:87-98` (per-field range check is only [0,1] for both types)

**Root Cause**: The model intentionally shares physics between generator types. Adding type-specific thermal bounds would require acknowledging fuel cell vs. ICE differences in the CHP model — which is the same issue noted in Finding 4.

**Impact**: A user could configure a gas generator with eta_thermal=0.60 and eta_electric=0.35 (total 0.95, which passes validation) — physically implausible for an air-cooled residential generator but accepted by the model. The same eta_thermal for a fuel cell could be plausible.

### Finding 8: [Severity: low]
**Description**: The flue loss Q_flue is computed as a residual `fuel_w - electrical_w - q_thermal_w` (line 758) rather than being modeled from exhaust gas properties. All non-recovered energy is lumped into a single term. While this satisfies mathematical closure of the energy balance, it prevents modeling:
- Exhaust gas temperature and composition for emissions calculations
- Latent heat recovery from condensing heat exchangers (different from sensible heat recovery)
- Stack effect on building ventilation

OCHRE (Generator.py lines 200-202) has the same simplification: `sensible_gain = (electric_kw - power_input) * 1000` — a single lumped waste heat term. EnergyPlus Generator:FuelCell tracks exhaust gas temperature, composition, and water vapor content, enabling latent heat recovery modeling.

**Code Location**: `generator.rs:758` (Q_flue as residual), `OCHRE Generator.py:200-202` (same approach)

**Root Cause**: This is a deliberate simplification for a whole-building energy model where exhaust gas properties are not needed. Both HARES and OCHRE make this choice.

**Impact**: (1) Condensing CHP heat recovery (exhaust gas water vapor condensation providing additional latent heat) cannot be modeled — the model can only capture sensible heat via eta_thermal. (2) Emissions calculations require exhaust temperature and composition data not available from this model.

### Finding 9: [Severity: low]
**Description**: The `load_state()` method (lines 875-906) restores derived CHP telemetry values (q_thermal_w, q_flue_w) when `eta_thermal > 0`, but `core_output` is reset to default without CHP thermal data (line 903: `self.core_output = CoreOutput::default()`). The `core_output.flows.thermal_output_w` field is always `None` in `CoreOutput` (line 842), even when CHP is active. This means the `core_output()` API — which is the primary inter-equipment communication channel — never reports thermal output, forcing all CHP-aware consumers to read raw telemetry instead.

**Code Location**: `generator.rs:842` (thermal_output_w always None), `generator.rs:903` (load_state resets core_output to default)

**Root Cause**: The `CoreFlows` type lacks a CHP-specific thermal field, and the generator does not populate `thermal_output_w` because that field may be intended for a different purpose (possibly cooling output).

**Impact**: Downstream controllers or reporting logic that reads `core_output()` for thermal balance calculations will see zero thermal output from the CHP system, causing energy balance errors at the system level even though the fluid port and telemetry correctly report the values.

## Summary
- Total findings: 9
- Critical: 1 (Finding 1 — fluid port energy decoupled from q_thermal_w)
- High: 2 (Findings 2, 3 — no consistency validation, missing parasitic loads)
- Medium: 4 (Findings 4, 5, 6, 7 — no FC/ICE differentiation, constant temperatures, return temp defaults, no type-specific eta_thermal bounds)
- Low: 2 (Findings 8, 9 — lumped flue loss, thermal_output_w always None in core_output)

## Recommendations
1. **Couple fluid port to q_thermal_w**: Either (a) add an `energy_w` field to `PortContribution::Fluid` and have the fluid solver verify it against `cp * flow * delta_T`, or (b) compute flow_rate_kg_s dynamically from `q_thermal_w`, supply_temp_c, and return_temp_c at each timestep so the fluid port always carries the correct thermal power. Option (b) is simpler and avoids changing the port type.

2. **Add config consistency validation**: In `GeneratorConfig::validate()`, if `eta_thermal > 0` and `loop_id` is configured, verify that `flow_rate_kg_s * 4186.0 * (supply_temp_c - return_temp_c) >= rated_power_kw * 1000.0 * eta_thermal / eta_electric_rated` for the rated output case, and emit a warning if the fluid port cannot transport the peak thermal output.

3. **Add parasitic load fraction**: Add an optional `parasitic_fraction` or `aux_power_kw` config field deducted from electrical output before port write. Default to 0 for backward compatibility. This matches EnergyPlus Generator:FuelCell's approach.

4. **Differentiate fuel cell vs. ICE CHP defaults**: Add `GeneratorKind`-aware temperature defaults (e.g., fuel cell: supply=65°C/return=55°C; ICE: supply=85°C/return=70°C) and validate supply_temp_c against type-specific maxima (ICE: <105°C coolant boiling point; fuel cell: <85°C stack limit).

5. **Scale fluid port parameters with load**: At minimum, scale flow_rate_kg_s proportionally with generator output to maintain constant delta-T, or make the flow rate proportional to `q_thermal_w / (cp * delta_T)`. This ensures the fluid port carries the correct energy at all operating points.

6. **Populate thermal_output_w in core_output**: Set `core_output.flows.thermal_output_w = Some(q_thermal_w)` when CHP is active so the inter-equipment API correctly reports thermal output. Also propagate this in `load_state()`.

7. **Add eta_thermal type-specific bounds**: For gas generators, cap eta_thermal at ~0.50; for fuel cells, cap at ~0.60. Document these limits based on representative equipment specifications (e.g., Marathon Ecopower micro-CHP, Panasonic Ene-Farm fuel cell).

## References / Citations
- OCHRE Generator.py: `calculate_power_and_heat()` (lines 176-202) — lumped waste heat model, `power_chp` declared but not implemented (line 39)
- EnergyPlus Generator:FuelCell — `FuelCellData.cc` tracks separate thermal streams (stack cooling, reformer, exhaust), models parasitic loads through `AuxiliaryPower` inputs, uses temperature-dependent heat exchanger UA models for part-load degradation
- Vishwanathan et al. (2018) — quadratic efficiency curve used by both HARES and OCHRE: https://doi.org/10.1016/j.apenergy.2018.06.013
- Typical ICE CHP jacket water: 80-95°C (ASME PTC 22-2014, Gas Turbine Performance Test Codes)
- Typical fuel cell CHP stack temperature: PEMFC 60-80°C (DOE Hydrogen Program Record #16015)
- Residential micro-CHP parasitic loads: 5-15% of rated electrical output for pumps, blowers, controls (IEA Annex 42, FC+Micro-CHP Test Protocol)
