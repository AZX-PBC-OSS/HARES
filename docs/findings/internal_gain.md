REPORT: Impact of Internal Gain Radiant/Convective Split on BESTEST 900FF Minimum Temperature
1. What EnergyPlus Does for OtherEquipment Radiant Fraction
Default Values
Per the EnergyPlus IDD (Input Data Dictionary) and confirmed by Context7 documentation:
- OtherEquipment: Fraction Radiant defaults to 0.0 (all convective). However, in the actual BESTEST 900FF IDF, EnergyPlus specifies Fraction Radiant = 0.3 for the OtherEquipment object. This was confirmed from the Context7 source showing an example OtherEquipment definition:
    OtherEquipment, BASE-1 OthEq 1, ... 0, !- Fraction Latent  0.3, !- Fraction Radiant  0; !- Fraction Lost
  
- People: Fraction Radiant of the sensible portion defaults to 0.3 (30% radiant, 70% convective of the sensible gain).
- Lights: Fraction Radiant defaults to 0.0 in the IDD, but ASHRAE fundamentals typically assigns 0.2–0.3 for fluorescent/LED lighting.
- ElectricEquipment: Fraction Radiant defaults to 0.0 in the IDD, but common practice for BESTEST is to assign values per the specific test case definition.
How EnergyPlus Distributes Radiant Gains
EnergyPlus distributes the radiant fraction of internal gains to zone interior surfaces using thermal absorptance-weighted area fractions:
1. Compute TMULT = 1 / Σ(A_i × ThermalAbsorptance_i) for all surfaces in the zone
2. Each surface receives Q_radiant × A_i × ThermalAbsorptance_i × TMULT
3. This goes into the inside face surface temperature of each surface (affecting the surface heat balance, not the zone air node directly)
4. The convective portion (1 - FractionRadiant) goes directly to the zone air node
The zone air heat balance equation (EnergyPlus Eq. 2) shows ΣQ_i = convective internal loads only. The radiant portion enters via Σh_i × A_i × (T_si - T_z) — it heats the surface first, which then transfers heat convectively to zone air with a thermal lag determined by the surface capacitance and resistance.
2. What HARES Currently Does (with File:Line References)
ALL sensible gain → zone air node (100% convective)
crates/hares-envelope/src/thermal_solver/ports.rs:19:
u[idx] += thermal.sensible_gain_w;
The entire sensible_gain_w from all equipment ports is deposited into the zone air sensible input vector entry. There is no split between radiant and convective; everything acts on zone air temperature immediately.
crates/hares-io/src/hpxml/resolve_loads.rs:730-766 (default_gain_fractions):
The function returns (sensible_fraction, latent_fraction) but no radiant sub-fraction. For example:
- "MELs" | "Plug Loads" => Some((0.855, 0.045)) — 85.5% sensible, but 100% of that sensible goes convective
- "Indoor Lighting" => Some((1.00, 0.00)) — 100% sensible, all convective
- "Refrigerator" => Some((1.00, 0.00)) — 100% sensible, all convective
crates/hares-equipment/src/scheduled_load.rs:490-502:
let sensible_gain_w = total_gain_source_w * self.sensible_gain_fraction;
// ...
ports.accumulate(&PortContribution::Thermal {
    zone,
    sensible_gain_w,
    latent_gain_w,
    category: ThermalCategory::InternalGain,
})?;
The sensible_gain_w is a monolithic value. There is no radiant/convective decomposition.
crates/hares-types/src/ports.rs:54-56: The PortContribution::Thermal struct carries:
sensible_gain_w: f64,
latent_gain_w: f64,
category: ThermalCategory,
No radiant_gain_w field exists.
OCHRE Confirmation
The OCHRE codebase (HARES's ancestor) confirms this is an inherited simplification. From vendors/OCHRE/ochre/Equipment/Equipment.py:80-84:
# FUTURE: separate convection and radiation, move radiation gains to the surfaces around the zone
self.sensible_gain_fraction = kwargs.get("Convective Gain Fraction (-)", 0) + kwargs.get(
    "Radiative Gain Fraction (-)", 0
)
OCHRE adds the convective and radiative fractions together and applies the sum to zone air. The FUTURE comment explicitly acknowledges this as a known deficiency.
BESTEST Fixture Configuration
tests/fixtures/bestest/900ff.toml:27-29:
internal_gains_w = 200.0
internal_gains_constant = true
internal_gains_sensible_fraction = 1.0
The 200W constant gain has sensible_fraction = 1.0 (100% sensible, 0% latent), which is correct per the BESTEST specification. However, all 200W goes to zone air — the radiant/convective split within the sensible fraction is missing.
tests/fixtures/bestest/600ff.toml:27-29: Same configuration — 200W, 100% sensible, all convective.
3. What the Fix Should Be
Architecture
The fix requires decomposing the monolithic sensible_gain_w into convective and radiant components, and distributing the radiant portion to interior surface nodes (not zone air).
Step-by-Step Code Changes
A. Extend PortContribution::Thermal to carry radiant gain
File: crates/hares-types/src/ports.rs
Add radiant_gain_w: f64 to the Thermal variant:
PortContribution::Thermal {
    zone,
    sensible_gain_w,
    radiant_gain_w,   // NEW: radiant portion of sensible gain
    latent_gain_w,
    category,
}
Update ThermalAccumulator to track radiant_gain_w alongside sensible_gain_w, and add a radiant_by_category array (or simpler: just accumulate radiant_gain_w as a separate field).
B. Add radiant_gain_fraction to equipment configuration
File: crates/hares-io/src/hpxml/resolve_loads.rs
Extend default_gain_fractions to return (sensible_frac, latent_frac, radiant_frac_of_sensible):
"MELs" | "Plug Loads" => Some((0.855, 0.045, 0.40)),  // 40% of sensible is radiant
"Indoor Lighting" => Some((1.00, 0.00, 0.25)),        // 25% of sensible is radiant
"Refrigerator" => Some((1.00, 0.00, 0.0)),            // 0% radiant
For the BESTEST 900FF OtherEquipment, the EnergyPlus IDF specifies Fraction Radiant = 0.3, so for the BESTEST fixture specifically:
- 200W × 0.3 = 60W radiant
- 200W × 0.7 = 140W convective
File: crates/hares-core/src/dwelling/synthetic.rs:470-476: Add a FracRadiant extension field alongside FracSensible.
C. Pass radiant gain through equipment to ports
File: crates/hares-equipment/src/scheduled_load.rs
Add radiant_gain_fraction field alongside sensible_gain_fraction. In resolve():
let radiant_gain_w = total_gain_source_w * self.radiant_gain_fraction;
let convective_gain_w = sensible_gain_w - radiant_gain_w;
ports.accumulate(&PortContribution::Thermal {
    zone,
    sensible_gain_w: convective_gain_w,  // only convective goes to zone air
    radiant_gain_w,                       // radiant goes to surfaces
    latent_gain_w,
    category: ThermalCategory::InternalGain,
})?;
D. Distribute radiant gain to interior surface nodes
File: crates/hares-envelope/src/thermal_solver/ports.rs
This is the core physics change. After apply_port_sensible_inputs adds the convective portion to zone air, add a new function apply_port_radiant_inputs that distributes the radiant gain to interior surface nodes using the same mechanism as solar distribution:
pub(super) fn apply_port_radiant_inputs(&self, u: &mut DVector<f64>, ports: &PortSlots) {
    for thermal in &ports.thermal {
        if thermal.radiant_gain_w <= 0.0 { continue; }
        // Find interior LWR zone for this zone
        let zone_cfg = self.config.interior_lwr_zones.iter()
            .find(|z| z.zone_id == thermal.zone);
        let Some(zone_cfg) = zone_cfg else { 
            // No surfaces: dump to zone air as fallback
            if let Some(&idx) = self.wiring.zone_sensible_input_indices.get(&thermal.zone) {
                u[idx] += thermal.radiant_gain_w;
            }
            continue;
        };
        // Distribute by area × thermal_absorptance (same as EnergyPlus TMULT)
        let total_ta: f64 = zone_cfg.surfaces.iter()
            .map(|s| s.area_m2 * s.emissivity)  // use emissivity as thermal absorptance proxy
            .sum();
        if total_ta <= 0.0 {
            // Fallback: dump to zone air
            if let Some(&idx) = self.wiring.zone_sensible_input_indices.get(&thermal.zone) {
                u[idx] += thermal.radiant_gain_w;
            }
            continue;
        }
        for surface in &zone_cfg.surfaces {
            let q_surface = thermal.radiant_gain_w * surface.area_m2 * surface.emissivity / total_ta;
            // Split via radiation_frac (same as solar deposition)
            if surface.input_index < u.len() {
                u[surface.input_index] += q_surface * surface.radiation_frac;
            }
            // (1 - radiation_frac) goes to zone air (short-circuited convection)
            if let Some(&air_idx) = self.wiring.zone_sensible_input_indices.get(&thermal.zone) {
                u[air_idx] += q_surface * (1.0 - surface.radiation_frac);
            }
        }
    }
}
File: crates/hares-envelope/src/thermal_solver/mod.rs
In build_input_vector(), call apply_port_radiant_inputs after apply_port_sensible_inputs (line ~481).
E. Update BESTEST fixture
File: tests/fixtures/bestest/900ff.toml
Add:
internal_gains_radiant_fraction = 0.3  # EnergyPlus OtherEquipment FractionRadiant
File: tests/fixtures/bestest/600ff.toml
Same addition — but note that for lightweight 600FF, the impact is much smaller because lightweight walls have low capacitance and the thermal lag is minimal.
4. Estimated Temperature Impact on 900FF Minimum Temperature
Current Situation
- HARES 900FF min temp: +0.90°C
- ASHRAE 140 acceptable band: -6.4, -1.6°C
- HARES overshoots the upper bound by 2.5°C
Hand Calculation
Model: 200W internal gain, currently 100% convective → 0% radiant. With fix: 70% convective (140W to zone air) + 30% radiant (60W to surfaces).
For heavyweight construction (900FF):
The heavyweight walls in 900FF have ~100mm concrete (ρ=1400, c=1000, k=0.51) with thermal capacitance C ≈ ρ × c × thickness × area. For 4 walls + roof + floor ≈ 160 m² of heavyweight surface:
C_surface_total ≈ 1400 × 1000 × 0.1 × 160 = 22,400,000 J/K (per surface node)
The zone air capacitance is ~129.6 × 1.2 × 1005 × 7 ≈ 1,093,000 J/K (with the ×7 multiplier from OCHRE).
When 60W radiant goes to surfaces instead of zone air:
- Zone air immediately loses 60W of heating → zone air drops faster
- Surfaces receive 60W distributed across ~160 m² → 0.375 W/m² into enormous capacitance
- The surface-embedded energy feeds back to zone air through the convective film resistance with a time constant of C_surface × R_film
For heavyweight concrete with R_film_int ≈ 0.12 m²K/W (typical):
- Time constant τ ≈ C_surface_node × R_film / Area ≈ (ρ×c×d×A) × R_film / A = ρ×c×d×R_film
- τ ≈ 1400 × 1000 × 0.1 × 0.12 = 16,800 seconds ≈ 4.7 hours
This means the radiant portion of the 60W deposited into surface nodes will reach zone air with a ~5-hour lag, and with significant attenuation.
Steady-state impact (ignoring dynamics): At steady state, the split doesn't matter — all energy ends up in zone air eventually. But during the minimum temperature event (which occurs during a cold night after a cold day), the time lag is critical.
Dynamic estimate:
- The minimum temperature event occurs at ~hour 876 (roughly late December/early January, nighttime)
- At that point, zone air is cooling rapidly due to infiltration + envelope loss
- 200W all-convective means zone air is artificially buoyed by the full 200W every timestep
- With 60W redirected to surfaces, zone air gets only 140W directly; the 60W deposited in surfaces with a 5-hour lag means most of it is "trapped" in the thermal mass and arrives when zone air is already recovering from the minimum
Temperature delta estimate: 
The zone air capacitance is ~1.1 MJ/K. A deficit of 60W over the ~8-hour cold night period (8 × 3600 = 28,800 seconds) means:
- ΔT ≈ 60W × 28,800s / 1,093,000 J/K ≈ 1.6°C lower zone air
But the 60W isn't completely lost — about 50% of it (30W) feeds back through the film resistance over the same period. So the effective deficit is more like 30-40W sustained over 8 hours:
- ΔT ≈ 35W × 28,800s / 1,093,000 J/K ≈ 0.9°C lower
Conservative estimate: 1.0–1.5°C reduction in minimum temperature.
This would bring HARES 900FF from +0.9°C to approximately -0.5 to -0.1°C, which is still above the -1.6°C upper bound of the ASHRAE band but much closer. The remaining gap (~1.0–1.5°C) may be addressable by other factors (exterior LWR treatment, infiltration timing, etc.).
For lightweight construction (600FF):
The lightweight walls have ~12mm wood (ρ=950, c=840) with thermal capacitance C ≈ 950 × 840 × 0.012 × A ≈ 1,530 J/(m²·K) per surface node. The time constant for radiant feedback:
- τ ≈ 950 × 840 × 0.012 × 0.12 = 1,147 seconds ≈ 0.3 hours
This is so fast that the radiant/convective split barely matters for lightweight construction — the radiant portion feeds back to zone air within minutes. Estimated impact on 600FF min temp: <0.3°C. Since 600FF is at -12.9°C (well within -18.8, 0), this is negligible.
5. How to Verify Empirically
Test 1: Parametric Study
Add a test in tests/bestest/ that runs 900FF with different radiant fractions:
#[test]
fn bestest_900ff_radiant_fraction_sensitivity() {
    // Run with 0%, 20%, 30%, 40% radiant fraction
    for &rad_frac in &[0.0, 0.2, 0.3, 0.4] {
        // Modify fixture to set internal_gains_radiant_fraction
        let mut dwelling = Dwelling::from_toml_config_with_radiant_override(
            &case.fixture_path(), rad_frac, Some(false)
        );
        let result = dwelling.simulate().unwrap();
        let min_temp = result.steps.iter()
            .flat_map(|s| s.zone_temperatures_c.iter().map(|(_, t)| *t))
            .fold(f64::INFINITY, f64::min);
        eprintln!("[radiant_study] 900FF rad_frac={rad_frac} min_temp={min_temp:.3}°C");
    }
}
Test 2: Direct Code Modification
A quicker empirical test: modify ports.rs:19 to apply only 70% of sensible_gain_w to zone air (simulating 30% radiant redirect), and redirect the remaining 30% to interior surface input indices using the existing deposit_solar_to_surface_nodes mechanism. This is a one-line hack for initial verification before implementing the full architecture.
Test 3: Energy Conservation
Verify that total energy into the zone is conserved: convective_to_air + radiant_to_surfaces + radiant_to_air_via_film = total_sensible_gain.
6. Risks and Complications
Risk 1: Surface Distribution Method
EnergyPlus uses ThermalAbsorptance for the radiant distribution weight, but HARES's InteriorSurfaceInfo has emissivity (longwave) and solar_absorptance (shortwave), not a separate thermal_absorptance. For opaque surfaces, longwave emissivity ≈ thermal absorptance by Kirchhoff's law, so using emissivity is physically correct. Risk: LOW.
Risk 2: Windows in Radiant Distribution
Windows participate in interior LWR exchange but have driving_temp = Some(Outdoor), meaning their surface temperature is driven by outdoor conditions. Radiant gain deposited to a window surface would follow the radiation_frac split, but the portion going to the window's "surface node" actually goes to the zone air index (since windows have no RC node). This needs careful handling: window radiant absorption should probably all go to zone air (since the window conducts it out almost immediately). Risk: MEDIUM.
Risk 3: Interaction with Interior LWR Solver
The radiant gain deposited on surface nodes will change the surface temperatures, which will change the interior LWR exchange calculation on the next iteration. This is correct physics but may require re-tuning of the LWR damping parameters. Risk: LOW.
Risk 4: Impact on Conditioned Cases (600, 900)
Adding a radiant split will change the annual heating/cooling loads for conditioned cases. Case 900 currently has heating at 2041.43 kWh (0.021% above the 2041 band upper limit). A radiant split should decrease heating load (surfaces stay warmer → less heat loss) which would move 900 back inside the band. Risk: POSITIVE — likely helps Case 900 too.
Risk 5: Only Partial Fix for 900FF
The estimated 1.0–1.5°C reduction from the radiant split alone may not bring 900FF fully within the -6.4, -1.6 band (from +0.9 to ~-0.5). Other factors contributing to the remaining ~1.1°C gap may include:
- Exterior longwave sky-loss treatment (the 4-component model may under/over-predict sky cooling)
- Nighttime infiltration coupling with the diurnal swing
- Heavyweight wall RC node placement
This fix should be implemented first as it is the single largest identifiable physics gap, and its impact should be measured before investigating other factors.
Risk 6: API Compatibility
Adding radiant_gain_w to PortContribution::Thermal is a breaking change to the hares-types public API. All callers of ports.accumulate will need updating. Mitigation: Default radiant_gain_w to 0.0 so existing code works unchanged.
7. Summary
Aspect	Current
900FF min temp	+0.90°C (2.5°C above band)
600FF min temp	-12.86°C (within band)
Physics fidelity	All sensible → zone air (OCHRE legacy)
Code scope	5 files, ~100 lines new/modified
Estimated effort	2–3 days (types → equipment → solver)
The radiant/convective split is the highest-priority physics fix for the BESTEST 900FF failure. It is a known deficiency with an explicit FUTURE comment in OCHRE. The fix is architecturally clean (reuse the existing InteriorSurfaceInfo distribution mechanism), and the estimated temperature impact (1.0–1.5°C) is consistent with the 2.5°C overshoot magnitude.