# Appendix: Physics Improvements — Verified Equations & Data Interfaces

Verified against ASHRAE Handbook of Fundamentals, EnergyPlus Engineering Reference,
HPXML 4.0 schema, and ResStock 2024.2 documentation.

## 1. Altitude-Corrected Air Density

**Problem**: OCHRE hardcodes `outside_air_density = 0.0765 lb/ft³` (sea level, 15°C ISA).
This produces 18% error at Denver/Albuquerque, propagating to 10-12% overestimate in
infiltration coefficients.

**Equations** (ASHRAE HOF / EnergyPlus `PsyRhoAirFnPbTdbW`):

```rust
/// ASHRAE/ISA standard barometric pressure at altitude.
/// z_m: altitude in metres above sea level.
/// Returns pressure in Pa.
pub fn standard_pressure_pa(z_m: f64) -> f64 {
    101_325.0 * (1.0 - 2.25577e-5 * z_m).powf(5.2559)
}

/// Moist air density (EnergyPlus PsyRhoAirFnPbTdbW).
/// p_pa: pressure in Pa, t_db_c: dry-bulb °C, w: humidity ratio kg/kg.
pub fn moist_air_density_kg_m3(p_pa: f64, t_db_c: f64, w: f64) -> f64 {
    let w_eff = w.max(1e-5);
    p_pa / (287.0 * (t_db_c + 273.15) * (1.0 + 1.6077687 * w_eff))
}
```

**Error magnitude**:

| City | Elevation (m) | Error vs 0.0765 lb/ft³ |
|------|--------------|------------------------|
| Sea level | 0 | −1.7% |
| Salt Lake City | 1288 | −14.9% |
| Denver | 1609 | −18.0% |
| Albuquerque | 1619 | −18.1% |

**Data interface**: Elevation from HPXML `Building/Site/Elevation` (optional) → EPW
header field 10 → hard error (no silent zero default).

**Sources**: ASHRAE HOF Ch.1, EnergyPlus Engineering Reference §Climate Calculations,
EnergyPlus Psychrometrics.hh

---

## 2. Thermostat Deadband Model

**Problem**: OCHRE's deadband logic is broken — inconsistent data sources (HPXML vs
schedule CSV vs runtime control), asymmetric hysteresis with no physical basis
(`deadband_offset=0.2`).

**Clean model**:

```rust
pub struct ThermostatConfig {
    pub hysteresis_c: f64,      // default 1.0°C (symmetric band)
    pub cutout_ratio: f64,      // default 0.0 (symmetric), range 0.0-1.0
    pub min_cycle_time_s: f64,  // minimum on/off duration to prevent short-cycling
}

pub enum ThermostatMode { Heating, Cooling, Deadband }
```

**Priority stack** (later overrides earlier):
1. HPXML `HVACControl` — static heating/cooling setpoints
2. ResStock schedule CSV — `heating_setpoint` / `cooling_setpoint` columns (time-varying)
3. Runtime `ControlSignal::ThermalSetpoint` — external controller override

**Hysteresis FSM**:
- Heat ON when `T_zone < T_heat_sp - hysteresis_c`
- Heat OFF when `T_zone > T_heat_sp + hysteresis_c * cutout_ratio`
- Cool ON when `T_zone > T_cool_sp + hysteresis_c`
- Cool OFF when `T_zone < T_cool_sp - hysteresis_c * cutout_ratio`
- Invariant: `T_cool_sp - T_heat_sp >= 2 * hysteresis_c`

**Deadband is a simulation config parameter**, not per-building from HPXML (HPXML
does not include deadband). Each setpoint value carries source annotation for debugging.

**Note**: ResStock `No Space Heating` / `No Space Cooling` schedule columns should set
setpoint to sentinel value (±999°C) causing equipment to stay off.

**ResStock `No Space Heating`/`No Space Cooling` columns**: Currently mapped to `Ignore`
and discarded by OCHRE (`schedule.py:56-68`). These should set setpoint to sentinel
values (e.g., -999°C / +999°C) causing equipment to stay off during those periods.

**Sources**: EnergyPlus ZoneControl:Thermostat (I/O Reference §Group Zone Controls),
ASHRAE Standard 55-2023, OpenStudio-HPXML Workflow Inputs (OnOffThermostatDeadbandTemperature)

---

## 3. HVAC Supply Air Temperature

**Problem**: OCHRE hardcodes 105°F (40.6°C) heating, 54-58°F (12.2-14.4°C) cooling,
312 CFM/ton for all equipment types. Too low for gas furnaces, too high for heat pump
HP-only mode.

**Recommended defaults by equipment type**:

| System | Heating Supply (°C) | Cooling CFM/ton | Notes |
|--------|--------------------:|----------------:|-------|
| Gas furnace | 54.4 (130°F) | N/A | Fixed supply temp |
| Electric furnace | 48.9 (120°F) | N/A | Fixed |
| ASHP (HP-only) | 32.2 (90°F) | 375 | Varies with OAT |
| ASHP (HP+aux) | 40.6 (105°F) | 375 | When auxiliary kicks in |
| Mini-split (heat) | 43.3 (110°F) | 375 | |
| Baseboard | N/A | N/A | No forced air |

**Variable-speed ASHP supply temp** (HP-only mode, approximate):
```
T_supply_c = 32.2 + 0.15 * (T_outdoor_c - 8.3)   // linear regression
// ~32.2°C at 8.3°C OAT, ~27°C at -17.8°C OAT
```

**Data interface**: HPXML does not include supply air temperature directly. Dispatch
on equipment type at initialization. `AirflowDefectRatio` from HPXML should scale
derived airflow rate (OCHRE ignores this). All values configurable via equipment kwargs.

**Caveat**: OCHRE's 312 CFM/ton may be a ResStock calibration value. Changing it
affects energy totals — document the change in PHYSICS_DECISIONS.md.

**Sources**: ASHRAE Handbook of Fundamentals Ch.1, ACCA Manual S, EnergyPlus Coil:*
objects

---

## 4. Terrain & Wind Exposure Coefficients

**Problem**: OCHRE hardcodes suburban terrain (`alpha=0.22`, `delta=370m`) and shelter
coefficient `1/6 ≈ 0.167` regardless of HPXML `SiteType`. Shelter value 0.5 is mapped
incorrectly (corresponds to rural, not suburban — AIM-2 class 3 vs 4).

**ASHRAE wind power law** (weather station → site):
```
U_site(h) = U_met × (δ_met / h_met)^α_met × (h / δ_site)^α_site
```
Weather station: `α_met = 0.14`, `δ_met = 270m`, `h_met = 10m` (airport/open terrain).

**Terrain coefficient table** (ASHRAE HOF Ch.16):

| SiteType | α_site | δ_site (m) |
|----------|-------:|----------:|
| rural | 0.14 | 270 |
| suburban | 0.22 | 370 |
| urban | 0.33 | 460 |

**AIM-2 shelter class** (from HPXML SiteType + ShieldingofHome):
```
base_class = 4   // default suburban
if SiteType == "urban":  class += 1
if SiteType == "rural":  class -= 1
if ShieldingofHome == "well-shielded": class += 1
if ShieldingofHome == "exposed": class -= 1
class = clamp(class, 1, 5)
```

| Class | s coefficient | Description |
|------:|:-------------|:------------|
| 1 | 0.90 | No obstructions |
| 2 | 0.70 | Light — few trees/outbuildings |
| 3 | 0.50 | Moderate — rural, some shelter |
| 4 | 0.30 | Heavy — typical suburban |
| 5 | 0.10 | Very heavy — dense urban |

**Data interface**: HPXML `Site/SiteType` (rural/suburban/urban), `Site/ShieldingofHome`
(exposed/normal/well-shielded), `AirInfiltration/InfiltrationHeight`.

**OCHRE shelter coefficient bug**: OCHRE maps HPXML `ShieldingofHome` "normal" to
`s = 0.5`. This corresponds to AIM-2 class 3 (moderate rural), not class 4 (typical
suburban). The correct value for suburban is `s = 0.30`. This systematically
overestimates wind-driven infiltration for suburban homes by ~40%.

**Sources**: ASHRAE HOF Ch.16 Table 1, AIM-2 (Walker & Wilson 1998), EnergyPlus
Engineering Reference §Infiltration, NREL/CP-550-33698

---

## 5. Dehumidifier Equipment Model

**Problem**: OCHRE acknowledges this gap (never implemented). HPXML 4.0 has first-class
`Dehumidifier` element; ResStock 2024 does not include dehumidifier scenarios.

**Model** (EnergyPlus `ZoneHVAC:Dehumidifier:DX` simplified):
```
WaterRemoval(T,RH) = Rated_L_day × f_wr(T_db, RH)   // biquadratic
EnergyFactor(T,RH) = Rated_IEF × f_ef(T_db, RH)     // biquadratic
ElectricPower = WaterRemoval / EnergyFactor
SensibleGain = LatentRemoval + ElectricPower          // waste heat to zone
LatentRemoval = WaterRemoval_kg_s × 2_454_000 J/kg
```

Where `f_wr` and `f_ef` are biquadratic performance curves (same kernel as HVAC):
```
f(T, RH) = a + b×T + c×T² + d×RH + e×RH² + f×T×RH
```

Generic coefficients from NREL/TP-550-49899 (Winkler et al. 2010, 6 ENERGY STAR units).
PLF curve coefficients from NREL/TP-5500-61076 (Winkler et al. 2014, cyclic testing).

**Control**: RH setpoint with 5% deadband hysteresis. On when RH > setpoint + 2.5%,
off when RH < setpoint − 2.5%.

**Data interface**: HPXML `Dehumidifier/Capacity` (pints/day), `IntegratedEnergyFactor`
or `EnergyFactor` (L/kWh), `DehumidistatSetpoint` (% RH), `FractionDehumidificationLoadServed`.

**v1 approach**: Rated-capacity + two biquadratic curves. On/off only (no part-load
cycling in v1). Reuses existing biquadratic evaluation kernel.

**Caveat**: Pre-2019 units rated at 80°F/60% RH; post-2019 at 65°F/60% RH. Curve
anchoring depends on test condition.

**EF vs IEF test conditions**: Pre-2019 units rated at 80°F/60% RH (EF); post-2019
rated at 65°F/60% RH (IEF). NREL curves calibrated at 80°F anchor. IEF inputs need
careful anchoring or a documented accuracy note.

**Sources**: EnergyPlus ZoneHVAC:Dehumidifier:DX, NREL/TP-550-49899 (Winkler 2010),
NREL/TP-5500-61076 (Winkler 2014), HPXML 4.0 schema, ENERGY STAR V5.0 specification

---

## 6. OpenADR 3.0 Control Signal Mapping

**Status**: OpenADR 3.0 finalized mid-2023. OpenADR 3.1 shipped Sept 2025 (not
backwards-compatible). Design against 3.0, plan 3.1 migration.

**Signal type → ControlSignal mapping**:

| OpenADR Signal | ControlSignal Variant | Notes |
|----------------|----------------------|-------|
| `SIMPLE` (levels 0-3) | `DemandResponse { level }` | Direct mapping |
| `LOAD_DISPATCH` (kW) | `PowerSetpoint` / `PowerLimit` | Requires disaggregation |
| `CHARGE_STATE_SETPOINT` | `SOCTarget` | Battery/EV |
| `GRID_EMERGENCY` | `DemandResponse { GridEmergency }` | |
| `ELECTRICITY_PRICE` | *Not a ControlSignal* | Feeds controller layer |
| `EXPORT_PRICE` | *Not a ControlSignal* | Feeds controller layer |
| `GHG` | *Not a ControlSignal* | Feeds controller layer |

**Architecture gap**: Price signals need a carrier type on the Dwelling API (not a
`ControlSignal` — they're inputs to a controller that produces `ControlSignal` values).

**Rust crate**: `openleadr-wire` for typed data model, `openleadr-client` for VEN
transport. Protocol adapter layer: VEN Transport → Strategy Mapper → `ControlSignal`.

**OpenADR 3.1**: Shipped Sept 2025, not backwards-compatible with 3.0. Key changes:
`/resources` promoted to top-level, refined target assignment, simplified program
object. `openleadr-rs` has a `openadr3_1` branch in active development. Design against
3.0 now, plan migration.

**Implementation phasing**: Phase 1 (type system via `openleadr-wire`, no network) →
Phase 2 (simulated VTN in tests via `openleadr-vtn`) → Phase 3 (live VEN transport
via `openleadr-client` + OAuth 2) → Phase 4 (3.1 migration).

**Sources**: OpenADR 3.0 specification, OpenADR Alliance 3.1 announcement (Sept 2025),
openleadr-rs (LF Energy, FOSDEM 2026 presentation), LBNL mdns-openadr3

---

## 7. CHP / Waste Heat Recovery

**Status**: OCHRE computes `power_chp` but never uses it (dead code with a latent bug —
not declared in `__init__`). HPXML 4.0 has no CHP fields. US residential CHP market
is <50,000 units.

**Approach**: Wire thermal port only. Full Annex 42 physics deferred.

- Generator computes: `Q_thermal = P_fuel × η_thermal` (where `η_thermal` configurable,
  default 0 for backwards compatibility)
- Writes `PortContribution::Thermal` to the zone containing the generator
- For DHW pre-heat: `PortContribution::Fluid` to water heater loop (requires Fluid port)

**Typical residential micro-CHP parameters**:

| Type | Electrical η | Thermal η | Heat-to-Power Ratio |
|------|:-----------:|:---------:|:------------------:|
| SOFC fuel cell | 0.45-0.60 | 0.25-0.35 | 0.6:1 |
| Stirling engine | 0.12-0.25 | 0.60-0.75 | 3-5:1 |
| Reciprocating engine | 0.25-0.35 | 0.45-0.55 | 1.5-2:1 |

**v1 approach**: Add `efficiency_thermal` parameter to Generator config (default 0.0).
When > 0, write thermal port contribution. No Annex 42 ODE or operating mode FSM.

**Sources**: EnergyPlus Generator:MicroCHP (Annex 42), IEA Annex 42 FCT Model
Specification
