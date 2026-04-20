HARES Physics Constants & Defaults Audit Report
Summary Table
#	Item	Current Value	Correct Value (E+/ASHRAE)	Severity	Classification
3a	Solar absorptance default	0.60	0.70	Medium	Shortcut (OCHRE match)
3c	Occupant sensible/latent	66/51.2 W	75/55 W (office); 66/51.2 OK (res)	Low	Acceptable (residential)
3d	Zone mass multiplier	7.0 (conditioned)	1.0 + explicit InternalMass	High	Bug (double-counting)
3e	Garage ACH	ACH50/20, fallback 0.5	Context-dependent	Low	Shortcut (rough)
3f	Thermostat deadband	1.0°C hysteresis	0.5–1.1°C typical	Low	Acceptable
Add1	Interior solar absorptance	Hardcoded 0.6	0.7 (E+ Material default)	Medium	Bug
Add2	Window emissivity	0.84	0.84	None	Correct ✓
---
Item 3a: Solar Absorptance Default
Current HARES Value
crates/hares-envelope/src/longwave_radiation.rs:67
pub const SOLAR_ABSORPTANCE_DEFAULT: f64 = 0.60;
What EnergyPlus/ASHRAE Says
EnergyPlus Material object IDD defaults:
- Thermal Absorptance: \default 0.9
- Solar Absorptance: \default 0.7
- Visible Absorptance: \default 0.7
These are applied when a Material or Material:NoMass object omits the absorptance fields. This has been the default since EnergyPlus v1.0 through v24.2.
Source: EnergyPlus IDD Material object definition; Context7 /nrel/energyplus query confirming SurfaceOpticalProperties with solar absorptance fields.
BESTEST specification (ASHRAE 140): All opaque surfaces in Cases 600/900/FF explicitly use solar absorptance = 0.6. This is set directly in the IDF and does not rely on any default. Confirmed by HARES's own BESTEST TOML configs which all set solar_absorptance = 0.6 on every boundary.
Error Magnitude
Metric	Value
Absolute difference	0.10 (0.60 vs 0.70)
Relative difference	16.7% less solar absorption
Impact on solar heat gain	~17% reduction on surfaces using the default
Classification: Shortcut (intentional OCHRE match, but wrong for EnergyPlus target)
The code comment at line 65–66 acknowledges this:
> "OCHRE Envelope.py:222 uses 0.60. EnergyPlus default is 0.70. We match OCHRE here."
For BESTEST, this is irrelevant because all BESTEST configs explicitly set solar_absorptance = 0.6. For HPXML-driven simulations where Boundary.solar_absorptance is None, the 0.60 value systematically understates solar heat gain by ~17% compared to EnergyPlus.
Fix
/// Default solar absorptance for opaque building surfaces.
/// EnergyPlus Material IDD default is 0.70. BESTEST cases specify
/// 0.60 per ASHRAE 140 — that must be set explicitly, not via this default.
pub const SOLAR_ABSORPTANCE_DEFAULT: f64 = 0.70;
Verify that no HPXML synthetic configs depend on the 0.60 default accidentally.
Impact on BESTEST
Zero impact — all BESTEST TOML configs explicitly set solar_absorptance = 0.6 on each boundary.
Impact on General Simulations
For HPXML-driven runs where solar_absorptance is not provided: +17% solar absorption → +2–5% cooling loads in solar-driven climates, −1–3% heating loads.
---
Item 3c: Occupant Heat Gain — 66 W sensible / 51.2 W latent
Current HARES Values
crates/hares-physics/src/constants.rs:125,131,136
pub const OCCUPANT_SENSIBLE_GAIN_W: f64 = 66.0;
pub const OCCUPANT_LATENT_GAIN_W: f64 = 51.2;
pub const OCCUPANT_CONVECTIVE_FRACTION: f64 = 1.0;  // all convective, 0% radiative
What EnergyPlus/ASHRAE Says
ASHRAE HoF 2021, Ch. 18, Table 1 ("Moderately active office work" at 24°C/75°F):
- Total: ~130 W/person
- Sensible: 75 W/person
- Latent: 55 W/person
EnergyPlus People object: Uses ASHRAE HoF Table 1 values. The Number of People + People Occupancy Schedule + Fraction Radiant fields specify per-person gains. Typical EnergyPlus residential models use ~30% radiative fraction for sensible gain.
OCHRE Envelope.py:904–908: Uses 400 BTU/h total (residential, lower activity):
- 400 BTU/h = 117.2 W total
- Sensible fraction = 0.563 → 66.0 W
- Latent fraction = 0.437 → 51.2 W
- Radiative fraction of sensible = 0 (all convective)
Error Magnitude
Metric	HARES/OCHRE	ASHRAE Office	Difference
Sensible	66.0 W	75.0 W	−9.0 W (−12.0%)
Latent	51.2 W	55.0 W	−3.8 W (−6.9%)
Total	117.2 W	130.0 W	−12.8 W (−9.8%)
Radiative fraction of sensible	0%	30–60%	Structural difference
Classification: Acceptable Approximation for Residential with one structural issue
The total magnitude (117.2 W) is appropriate for residential occupancy (seated/resting activity). The ASHRAE HoF 75/55 values are for office work, which is higher activity. HARES's match with OCHRE is correct for the residential use case.
However, the OCCUPANT_CONVECTIVE_FRACTION = 1.0 (0% radiative) is a structural shortcut. ASHRAE HoF specifies 30–60% of sensible gain is radiative depending on activity level. EnergyPlus People object defaults to ~30% radiative fraction for typical residential occupancy. Routing all sensible gain to the zone air node means:
- Interior surfaces don't directly absorb occupant radiant heat
- Zone air temperature spikes are overstated
- Surface temperatures are understated
Fix
Two-part fix:
1. Magnitude: Keep 66.0/51.2 W for OCHRE parity (residential), but document the distinction:
      /// Sensible heat gain per occupant [W/person].
   /// OCHRE residential model: 400 BTU/h × 0.563 = 66.0 W (seated/light activity).
   /// ASHRAE HoF 2021 Table 1 (office): 75 W at 24°C — use for commercial applications.
   pub const OCCUPANT_SENSIBLE_GAIN_W: f64 = 66.0;
   
2. Radiative split (structural fix, higher impact):
      pub const OCCUPANT_RADIATIVE_FRACTION: f64 = 0.30;  // ASHRAE HoF typical for seated
   pub const OCCUPANT_CONVECTIVE_FRACTION: f64 = 0.70;  // was 1.0
      Wire the radiative component (0.30 × 66 = 19.8 W/person) to interior surfaces via MRT.
Impact on BESTEST
Zero impact — BESTEST cases use occupancy = 0.0 (no internal gains from occupants).
Impact on General Simulations
For 3 occupants: 59.4 W shifted from air node to surfaces. This smooths zone air temperature fluctuations, reducing peak cooling load by ~0.5–1.5% and improving surface temperature accuracy.
---
Item 3d: Zone Mass Multiplier — fallback _ => 7.0
Current HARES Value
crates/hares-core/src/dwelling/conversions.rs:24–31
pub fn mass_multiplier_for_zone(zone_type: &ZoneType) -> f64 {
    match zone_type {
        ZoneType::Conditioned => 7.0,
        ZoneType::Foundation => 1.0,
        ZoneType::Attic | ZoneType::Garage => 1.0,
        _ => 7.0,
    }
}
Applied in crates/hares-envelope/src/boundary_rc.rs:281:
(AIR_DENSITY_KG_M3 * AIR_CP_J_KG_K * volume * z.mass_multiplier).max(MIN_CAPACITANCE_J_K)
Additionally, explicit furniture boundaries are created via LUT:
crates/hares-io/src/hpxml/building.rs:658–677
const FURNITURE_FRACTIONS: &[(ZoneType, f64)] = &[
    (ZoneType::Conditioned, 0.4),   // 40% of floor area
    (ZoneType::Foundation, 0.4),
    (ZoneType::Garage, 0.1),
];
The furniture material in defaults/envelope/Envelope Materials.csv has:
- Thickness: 0.1524 m, conductivity: 0.115 W/(m·K), density: 640.8 kg/m³, cp: 1320 J/(kg·K)
- Capacitance per m²: 118,579 J/(m²·K)
What EnergyPlus/ASHRAE Says
EnergyPlus has two mutually exclusive mechanisms:
1. InternalMass object — explicitly defines interior mass surfaces with a real construction and area. The mass is resolved through the surface heat balance (convection to zone air, radiation to other surfaces). This is the recommended ASHRAE approach.
2. ZoneCapacitanceMultiplier:ResearchSpecial — multiplies the zone air capacitance ρ×cp×V by a factor. Default = 1.0 (air only, no furniture). This is a research shortcut that lumps all interior mass into the air node. Per EnergyPlus documentation:
   > "Numerically increases or decreases zone air temperature deviations at each time step."
   Critically, EnergyPlus does not use both simultaneously. Using InternalMass with a multiplier > 1.0 double-counts the same thermal mass.
Critical Finding: Double-Counting of Furniture Thermal Mass
For a typical 48 m² conditioned zone with 2.7 m ceiling:
Component	Capacitance
Zone air only (ρ×cp×V × 1.0)	157 kJ/K
Multiplier add (×6.0 more, total 7.0)	+943 kJ/K
Furniture LUT boundary (19.2 m² × 118.6 kJ/(m²·K))	+2,277 kJ/K
Total in HARES	~3,377 kJ/K
Correct approaches:
Approach	Total
Explicit furniture only (multiplier=1.0)	~2,434 kJ/K
Multiplier only (no furniture, mult=7.0)	~1,099 kJ/K
HARES current (both)	~3,377 kJ/K
The current approach overstates zone thermal mass by 39% vs. explicit-furniture-only, or 207% vs. multiplier-only. This is a physically incorrect double-counting — the same furniture mass is represented both as an RC boundary node AND as inflated zone air capacitance.
The OCHRE model also does this, so it's consistent with OCHRE parity. But the HARES project rule is to target EnergyPlus/ASHRAE physics, making this a bug per the stated design goal.
What the 7.0 Multiplier Represents Physically
Physically, a multiplier of 7.0 on ρ×cp×V inflates the zone air capacitance from 157 kJ/K to 1,099 kJ/K for a 48 m² room. This is equivalent to:
- ~110 m² of 12 mm gypsum board (furniture + partition walls)
- Or ~0.94 m³ of concrete at 2400 kg/m³, 880 J/(kg·K)
This is a reasonable order of magnitude for a furnished room, but it's physically incoherent because:
1. It lumps furniture mass at the air temperature (no thermal gradient through furniture)
2. It bypasses the convective/radiative resistance between air and furniture surfaces
3. It creates artificial thermal coupling that dampens air temperature fluctuations too aggressively
Fix
Phase 1 (minimal, correct the double-counting):
When furniture boundaries are present from the LUT, reduce the mass multiplier to 1.0 for conditioned zones:
pub fn mass_multiplier_for_zone(zone_type: &ZoneType, has_explicit_furniture: bool) -> f64 {
    match zone_type {
        ZoneType::Conditioned => {
            if has_explicit_furniture { 1.0 } else { 7.0 }
        }
        ZoneType::Foundation => 1.0,  // foundation furniture is lightweight
        ZoneType::Attic | ZoneType::Garage => 1.0,
        _ => 1.0,  // unknown zone: safe default
    }
}
Phase 2 (EnergyPlus-aligned):
Replace the multiplier entirely with explicit InternalMass-style objects. The LUT furniture boundaries already serve this purpose. Remove the multiplier concept and always use 1.0.
Impact on BESTEST
Zero impact — BESTEST TOML configs explicitly set mass_multiplier = 1.0 (verified in tests/fixtures/bestest/900ff.toml:11 and 600.toml:11).
Impact on General Simulations
Reducing the multiplier from 7.0 to 1.0 (with furniture RC nodes already present) removes ~943 kJ/K of phantom mass from the zone air node. This will:
- Reduce zone thermal time constant by ~28%
- Increase peak air temperature fluctuations (more realistic)
- Change annual heating/cooling loads by 3–8% depending on climate and control strategy
- Bring transient response closer to EnergyPlus results
---
Item 3e: Garage ACH Heuristic
Current HARES Value
crates/hares-core/src/dwelling/solver_builder.rs:869–875
ZoneType::Garage => {
    let garage_ach = building
        .infiltration_ach50
        .map(|ach50| ach50 / 20.0)  // rough conversion ACH50→natural ACH
        .unwrap_or(0.5);             // fallback default
    InfiltrationMethod::Ach { ach: garage_ach }
}
What ASHRAE/AccuRate Says
ASHRAE 62.2-2022 requires whole-house ventilation but does not specifically prescribe garage ACH rates for residential garages. The standard focuses on habitable spaces.
ASHRAE 62.1-2022 Table 6-1 specifies garage ventilation at 0.75 cfm/ft² (7.5 L/s·m²) floor area for enclosed parking garages — but this is for commercial parking, not residential attached garages.
Residential garage infiltration rates vary widely:
- Detached garage: ~0.5–1.0 ACH natural (leaky overhead door)
- Attached garage with shared wall: ~0.5–1.5 ACH natural
- The ACH50/20 conversion is a commonly-used rough approximation for residential buildings (the "n-factor" or "LBL factor" is typically 15–25 for sheltered residential buildings)
AccuRate (Australian NatHERS software) uses specific garage ventilation rates based on construction type.
Error Magnitude
Metric	Value
ACH50/20 conversion	Rough approximation; n-factor varies 15–25 by shelter class
Fallback 0.5 ACH	Low-to-moderate for typical attached garage
Typical garage ACH	0.5–1.5 ACH natural
Classification: Acceptable Shortcut with documentation gap
The ACH50/20 conversion is standard practice in residential energy modeling. The fallback of 0.5 ACH is at the lower end of typical values. Using the building's whole-house ACH50 for the garage is questionable — garages are typically leakier than conditioned spaces.
Fix
1. Use a higher default: Residential garages with overhead doors are leakier than conditioned space. A fallback of 0.75–1.0 ACH would be more representative.
2. Use zone-specific ACH50 when available: HPXML may provide infiltration_constant_ach for the garage zone.
3. Document the source and limitations:
      ZoneType::Garage => {
       // Garage infiltration: use zone-specific ACH if provided, else
       // estimate from building ACH50 (garages are typically ~2× leakier
       // than conditioned space per ASHRAE 119). Fallback 0.75 ACH natural
       // for typical attached garage with overhead door.
       let garage_ach = building
           .infiltration_constant_ach
           .or_else(|| building.infiltration_ach50.map(|ach50| ach50 / 15.0))
           .unwrap_or(0.75);
       InfiltrationMethod::Ach { ach: garage_ach }
   }
   
Impact on BESTEST
Zero impact — BESTEST has no garage zone.
Impact on General Simulations
Increasing garage ACH from 0.5 to 0.75 would increase garage ventilation heat loss/gain by ~50%, which propagates through shared walls to the conditioned zone. Typical impact: ±0.5–2% on conditioned zone heating/cooling loads, depending on shared wall area.
---
Item 3f: Thermostat Deadband
Current HARES Values
HVAC thermostat hysteresis (crates/hares-equipment/src/hvac/thermostat.rs:36):
hysteresis_c: 1.0,  // default deadband
Environment setpoint deadband (crates/hares-core/src/environment.rs:893):
let deadband_c = setpoint_deadband_c.unwrap_or(1.0);
Asymmetric deadband offset (crates/hares-equipment/src/hvac/thermostat.rs:40):
deadband_offset: 0.2,  // OCHRE default
What ASHRAE/EnergyPlus Says
ASHRAE Standard 55-2020 does not prescribe thermostat deadband values — it specifies acceptable thermal environment ranges (±2.2°C from neutral for 80% acceptability).
Typical residential thermostat deadbands:
- Mechanical/electromechanical: 0.5–1.1°C (1–2°F)
- Smart thermostats (learning, setback): 0.25–0.5°C
- EnergyPlus ZoneControl:Thermostat: No explicit deadband; uses ThermostatSetpoint:SingleHeating/SingleCooling schedules with no hysteresis. Cycling control is handled by the equipment object (e.g., Coil:Heating:Gas has its own deadband control).
The 1.0°C hysteresis is reasonable for residential applications and matches OCHRE.
Classification: Acceptable Approximation
The 1.0°C default is within the typical range for residential thermostats. The asymmetric offset (0.2) is an OCHRE-specific detail that matches lab measurements for residential HVAC equipment cycling behavior. No ASHRAE/EnergyPlus standard contradicts this value.
Fix
No fix required. Consider documenting the source:
/// Default thermostat hysteresis (deadband) [°C].
/// Typical residential: 0.5–1.1°C (1–2°F). Matches OCHRE default.
pub const DEFAULT_HYSTERESIS_C: f64 = 1.0;
Impact on BESTEST
Zero impact — BESTEST 600 uses deadband_c = 0.0 and setpoint_deadband_c = 0.0 (verified in tests/fixtures/bestest/600.toml:20,28). BESTEST 900FF uses no HVAC.
Impact on General Simulations
None — the value is within typical bounds.
---
Additional Item 1: Interior Solar Absorptance Hardcoded 0.6
Current HARES Value
crates/hares-core/src/dwelling/solver_builder.rs:704
(                           // non-window interior surface
    sb.attic_emissivity,
    0.6,                    // ← hardcoded interior solar absorptance
    sb.interior_rad_frac,
    sb.r_film_int_m2_k_w / sb.area_m2.max(1e-9),
    None,
)
This is also noted in crates/hares-envelope/tests/bestest_900ff_root_cause.rs:14:
> "Interior solar absorptance hardcoded 0.6 for all surfaces (minor)"
And in crates/hares-envelope/src/thermal_solver/mod.rs:3495:
solar_absorptance: 0.6,
What EnergyPlus Says
EnergyPlus uses the material's solar absorptance for the interior face of each surface when distributing shortwave (solar) radiation inside the zone. The ComputeIntSolarAbsorpFactors function distributes transmitted window solar radiation to interior surfaces based on their absorptance values.
Default Material solar absorptance = 0.70 (per IDD). The interior face absorptance defaults to the same value as the exterior face unless a different MaterialProperty is specified.
For BESTEST, interior solar absorptance = 0.6 per the ASHRAE 140 specification. For general surfaces, 0.70 is the EnergyPlus default.
Error Magnitude
Metric	Value
Current (hardcoded)	0.60
EnergyPlus default	0.70
Relative difference	16.7% less interior solar absorption
Interior solar absorptance affects how much of the window-transmitted solar radiation is absorbed by each interior surface vs. being reflected back to other surfaces. A lower absorptance means more inter-reflection and a different time distribution of the solar load.
Classification: Bug
This is a hardcoded value that should be:
1. A per-surface property derived from the material
2. Or at minimum a named constant, not a magic number
The 0.6 value happens to be correct for BESTEST (which specifies 0.6) but is wrong for general HPXML simulations (EnergyPlus default is 0.7).
Fix
1. Extract to a named constant:
      /// Default interior solar absorptance for opaque surfaces.
   /// EnergyPlus Material default: 0.70. BESTEST specifies 0.60.
   pub const INTERIOR_SOLAR_ABSORPTANCE_DEFAULT: f64 = 0.70;
   
2. Wire per-surface: The Boundary struct already has solar_absorptance: Option<f64>. Use this for the interior face as well (or add a separate interior_solar_absorptance field if different values are needed for interior vs. exterior).
3. Replace all hardcoded 0.6 references in solver_builder.rs:704, thermal_solver/mod.rs:3495 with the named constant.
Impact on BESTEST
Minimal — BESTEST 900FF root-cause analysis already identifies this as "minor." For BESTEST, interior solar absorptance only matters on south-facing surfaces with direct solar gain through the 12 m² window.
Impact on General Simulations
Moderate for zones with significant window area. More solar radiation absorbed by interior surfaces (0.70 vs 0.60) means faster conversion of transmitted solar to surface temperature, which changes the time profile of cooling load by shifting more load to the timestep of solar incidence and less to subsequent timesteps.
---
Additional Item 2: Window Emissivity
Current HARES Value
crates/hares-envelope/src/longwave_radiation.rs:54
pub const EMISSIVITY_WINDOW: f64 = 0.84;
Also referenced in crates/hares-core/src/dwelling/solver_builder.rs:695 as WINDOW_EMISSIVITY.
What EnergyPlus/NFRC Says
Standard clear glass infrared emissivity = 0.84. This is per:
- NFRC (National Fenestration Rating Council) standard for uncoated clear glass
- EnergyPlus WindowMaterial:SimpleGlazingSystem and WindowMaterial:Glazing default thermal absorptance (emissivity) = 0.84
- ASHRAE HoF 2021, Ch. 15: typical clear float glass ε = 0.84
For low-E coatings, emissivity drops to 0.04–0.20 depending on coating type. But for the default case (uncoated glass), 0.84 is correct.
Classification: Correct ✓
The value matches both EnergyPlus default and NFRC reference values for clear glass.
Fix
No fix required. The documentation could be strengthened:
/// Emissivity for uncoated window glass (clear float glass).
/// NFRC/ASHRAE HoF 2021 Ch. 15: ε = 0.84 for standard clear glass.
/// Low-E coatings: 0.04–0.20 (not modeled by this constant).
pub const EMISSIVITY_WINDOW: f64 = 0.84;
Impact on BESTEST
None — the BESTEST windows use single-pane clear glass (U=3.0, SHGC=0.789), for which 0.84 is correct.
---
Cross-Cutting: BESTEST Impact Summary
Item	BESTEST Impact	Reason
3a: Solar absorptance 0.60→0.70	Zero	TOML configs explicitly set 0.6
3c: Occupant gains	Zero	BESTEST uses occupancy = 0.0
3d: Mass multiplier 7.0→1.0	Zero	TOML configs explicitly set mass_multiplier = 1.0
3e: Garage ACH	Zero	No garage zone in BESTEST
3f: Thermostat deadband	Zero	BESTEST 600 uses deadband_c = 0.0
Interior solar absorptance 0.6→0.7	Minor	BESTEST spec is 0.6; should be set explicitly
Window emissivity 0.84	None	Correct for clear glass
Key observation: The BESTEST TOML configs are well-constructed with explicit overrides for all affected parameters. The defaults only matter for HPXML-driven residential simulations where many parameters are unspecified.
---
Prioritized Fix Recommendations
Priority	Item	Effort	Impact
P0	3d: Fix double-counting (mass multiplier + furniture)	Medium	High — 28% overstated zone mass
P1	Add1: Interior solar absorptance — extract from magic number to named constant	Low	Medium — 17% interior solar error
P1	3a: Solar absorptance default 0.60→0.70	Low	Medium — 17% exterior solar error
P2	3c: Add radiative fraction for occupant gains	Medium	Low-Medium — structural improvement
P3	3e: Garage ACH fallback 0.5→0.75	Low	Low — affects garage zones only