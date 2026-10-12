//! Water heater resolution from HPXML into canonical equipment specs.

use serde::Serialize;

use serde_json::{Map, Value, json};

use hares_equipment::{
    ElectricResistanceWaterHeaterConfig, EquipmentConfig, GasWaterHeaterConfig,
    HeatPumpWaterHeaterConfig, IndirectTankConfig, TanklessWaterHeaterConfig,
};
use hares_types::{FuelType, Warning, parse_trimmed_f64};

use super::building::{Building, XmlNode};
use super::data_patches::HpxmlDataPatches;
use super::equipment::EquipmentSpec;
use super::water_heater_ua::{UaInputs, WhCategory, ua_from_energy_factor};
use super::xml_helpers::{
    child_bool, child_f64, child_temperature_c, child_text, descendants_named, element_id,
};
use hares_physics::units as conv;

use super::elements::*;
use crate::defaults::DefaultsStore;
use crate::draw_profile::{DistributionSystem, FixtureEfficiency, combined_daily_hot_water_l};

/// ANSI/RESNET 301 on-time fractions for gas tankless parasitic power, indexed by 1–5 bedrooms.
const TANKLESS_ON_TIME_FRACS: [f64; 5] = [0.0269, 0.0333, 0.0397, 0.0462, 0.0529];

/// Compute gas tankless parasitic power (W) from bedroom count.
///
/// Formula: `5 + 60 * on_time_frac` where `on_time_frac` is from the RESNET 301 table.
/// Bedroom count rounded to nearest integer, clamped to [1, 5].
fn tankless_parasitic_power_w(n_bedrooms: f64) -> f64 {
    let idx = (n_bedrooms.round() as usize).clamp(1, 5) - 1;
    5.0 + 60.0 * TANKLESS_ON_TIME_FRACS[idx]
}

/// Resolve a bedroom count, trying multiple sources in priority order:
/// 1. The provided `n_bedrooms` (from HPXML)
/// 2. The `data_patches.number_of_bedrooms` field
///
/// The field is required input: OCHRE reads it with a direct index and
/// fails on absence (hpxml.py:263, `construction["NumberofBedrooms"]`; the
/// OS-HPXML defaults apply no default either, defaults.rb `apply_water_heaters`
/// reads it directly, :3303), so an unavailable count is a typed error naming
/// the field, never a substituted count (the same rule
/// [`extract_bedroom_count`] applies).
fn resolve_bedroom_count(
    n_bedrooms: Option<f64>,
    data_patches: Option<&HpxmlDataPatches>,
) -> std::result::Result<f64, super::HpxmlError> {
    n_bedrooms
        .or_else(|| data_patches.and_then(|p| p.number_of_bedrooms))
        .ok_or_else(|| super::HpxmlError::MissingField {
            path: "BuildingSummary/BuildingConstruction/NumberofBedrooms",
            system_kind: "BuildingConstruction",
            system_id: "Building".to_string(),
            reason: "the water heater tank volume, first-hour rating and hot-water \
                     draw are sized by the bedroom count; OCHRE reads the field \
                     with a direct index and fails on absence (hpxml.py:263), so \
                     no count may be substituted for it",
        })
}

/// Resolve the numeric `zone_id` (1-indexed) corresponding to an HPXML
/// `<Location>` string (e.g. "garage", "conditioned space"), by matching
/// against the zone types in the parsed `Building`.
///
/// Returns `None` when the location string does not match any zone in the
/// building (should not happen for well-formed HPXML but is not a hard error).
fn zone_id_for_location(building: &Building, location_text: &str) -> Option<u16> {
    let zone_type = super::building::parse_zone_label(location_text);
    let key = super::building::zone_key(&zone_type);
    building
        .zones
        .iter()
        .position(|z| super::building::zone_key(&z.zone_type) == key)
        .map(|idx| (idx as u16) + 1)
}

/// Share of a heat pump water heater's sensible gain that lands on interior
/// partition walls instead of the zone air, by HPXML `Location`. OCHRE
/// splits 0.5 to the "Interior Wall" surface it builds only in the living
/// zone (WaterHeater.py "HPWH Wall Interaction Factor", hpxml.py
/// "Interior Wall"); every other zone has no such wall and OS-HPXML adds all
/// HPWH gains convectively to the zone air (waterheater.rb,
/// apply_hpwh_zone_heat_gain_program, frac_radiant 0), so the share there is
/// 0.0, including a conditioned basement or crawlspace.
fn hpwh_wall_heat_fraction(location: &str) -> f64 {
    if super::building::is_living_space_label(location) {
        0.5
    } else {
        0.0
    }
}

/// The same surfaces as declared elements (the descendants_named reads),
/// the set OS-HPXML's `HPXML::Building#surfaces` searches in
/// `has_location` (hpxml.rb:1790-1797).
const ENCLOSURE_SURFACE_ELEMENTS: [&super::elements::HpxmlElement; 7] = [
    &ROOF,
    &RIM_JOIST,
    &WALL,
    &FOUNDATION_WALL,
    &FLOOR,
    &FRAME_FLOOR,
    &SLAB,
];

/// Whether any enclosure surface is adjacent to `location` (OS-HPXML
/// `has_location`, hpxml.rb:1790-1797).
fn has_surface_adjacent_to(building: &Building, location: &str) -> bool {
    let Some(enclosure) = building.details_xml.child(&ENCLOSURE) else {
        return false;
    };
    ENCLOSURE_SURFACE_ELEMENTS.iter().any(|surface| {
        descendants_named(enclosure, surface)
            .into_iter()
            .any(|node| {
                [&INTERIOR_ADJACENT_TO, &EXTERIOR_ADJACENT_TO]
                    .iter()
                    .any(|side| child_text(node, side).is_some_and(|text| text.trim() == location))
            })
    })
}

/// The location OS-HPXML gives a water heater with no `<Location>`:
/// defaults.rb:6104-6132 (`get_water_heater_location`, v1.12.0, after
/// ANSI/RESNET/ICC 301-2022C), called from defaults.rb:3401-3406 with the
/// building's first IECC climate zone: the HPXML's, else the one OS-HPXML
/// derives from the weather station (the blueprint's effective zone
/// resolution). The first location of the zone's hierarchy the building
/// has surfaces in wins. Conditioned space, last in every hierarchy, is
/// present in every HPXML building (the schema requires conditioned floor
/// area; HARES always builds the conditioned zone), and older files spell
/// it "living space".
fn default_water_heater_location(
    building: &Building,
    climate_zone_iecc: Option<&str>,
) -> std::result::Result<&'static str, super::HpxmlError> {
    let hierarchy: &[&'static str] = match climate_zone_iecc {
        Some("1A" | "1B" | "1C" | "2A" | "2B" | "2C" | "3A" | "3B" | "3C") => {
            &["garage", "conditioned space"]
        }
        Some("4A" | "4B" | "4C" | "5A" | "5B" | "5C" | "6A" | "6B" | "6C" | "7" | "8") => &[
            "basement - unconditioned",
            "basement - conditioned",
            "conditioned space",
        ],
        None => &[
            "basement - conditioned",
            "basement - unconditioned",
            "conditioned space",
        ],
        Some(other) => {
            return Err(super::HpxmlError::Parse(
                format!(
                    "unexpected IECC climate zone '{other}' for the water heater location default"
                )
                .into(),
            ));
        }
    };
    hierarchy
        .iter()
        .copied()
        .find(|&location| {
            has_surface_adjacent_to(building, location)
                || (location == "conditioned space"
                    && building.conditioned_zone_index().ok().flatten().is_some())
        })
        .ok_or_else(|| {
            super::HpxmlError::Parse(
                "the building has none of the water heater default locations"
                    .to_string()
                    .into(),
            )
        })
}

pub(super) fn resolve_water_heaters(
    building: &Building,
    climate_zone_iecc: Option<&str>,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
    data_patches: Option<&HpxmlDataPatches>,
    warnings: &mut Vec<Warning>,
) -> std::result::Result<(), super::HpxmlError> {
    let details = &building.details_xml;
    let (avg_water_draw_l_per_day, n_bedrooms) =
        parse_avg_water_draw_and_bedrooms(details, data_patches)?;
    // The systems that declare a FractionDHWLoadServed, in file order, for
    // the schematron's sum rule after the loop.
    let mut declared_fractions: Vec<(String, f64)> = Vec::new();

    for wh in descendants_named(details, &WATER_HEATING_SYSTEM) {
        let wh_type = child_text(wh, &WATER_HEATER_TYPE).unwrap_or_default();
        let fuel = parse_water_heater_fuel(wh, &wh_type)?;
        let name = canonical_water_heater_name(&wh_type, fuel)?;
        let setpoint_c = child_temperature_c(wh);
        let performance_adjustment = child_f64(wh, &PERFORMANCE_ADJUSTMENT);
        let location = match child_text(wh, &LOCATION) {
            Some(location) => location,
            None => {
                let location = default_water_heater_location(building, climate_zone_iecc)?;
                warnings.push(Warning::new(
                    "hpxml",
                    format!(
                        "{name} ({}) has no Location; defaulted to '{location}' as OS-HPXML does",
                        element_id(wh).unwrap_or_default()
                    ),
                ));
                location.to_string()
            }
        };
        // The raw HPXML-normalized Location text goes into the typed configs'
        // `zone_type` verbatim, so the placements that name space with no
        // modeled zone ("other heated space", "other housing unit", ...) stay
        // distinguishable downstream instead of collapsing into one
        // "adjacent" bucket. The zone-id lookup keeps using the collapsed
        // zone key below.
        let zone_id = zone_id_for_location(building, &location);
        let zone_name = Some(location.clone());

        let energy_factor = child_f64(wh, &ENERGY_FACTOR);
        let uniform_energy_factor = child_f64(wh, &UNIFORM_ENERGY_FACTOR);
        let tank_volume_rated_gal = child_f64(wh, &TANK_VOLUME);
        let first_hour_rating_gal = child_f64(wh, &FIRST_HOUR_RATING);
        let heating_capacity_input = child_f64(wh, &HEATING_CAPACITY);
        let recovery_efficiency = child_f64(wh, &RECOVERY_EFFICIENCY);

        let volume_correction = if fuel == FuelType::Electric {
            0.9
        } else {
            0.95
        };
        let tank_volume_m3 =
            tank_volume_rated_gal.map(|gal| conv::volume_gal_to_m3(gal * volume_correction));
        // Tank height is optional in HPXML; propagate None so the equipment
        // model falls back to its documented stratified-tank geometry defaults
        // rather than having the IO layer silently substitute 4 ft here.
        let tank_height_m = child_f64(wh, &TANK_HEIGHT).map(conv::length_ft_to_m);
        let first_hour_rating_m3 = first_hour_rating_gal.map(conv::volume_gal_to_m3);
        let heating_capacity_btu_hr = heating_capacity_input;
        let heating_capacity_w = heating_capacity_input.map(conv::power_btu_h_to_w);

        // HPXML JacketRValue is in hr·ft²·°F/Btu; convert to SI m²·K/W.
        let jacket_r_value_m2_k_w = wh
            .path(&[&WATER_HEATER_INSULATION, &JACKET])
            .and_then(|j| child_f64(j, &JACKET_R_VALUE))
            .map(conv::r_value_ip_to_si);

        // RelatedHVACSystem cross-reference for combi boiler / indirect tank.
        let related_hvac_idref = wh
            .child(&RELATED_HVAC_SYSTEM)
            .and_then(|n| n.attrs.get("idref").cloned());

        let category = wh_category(&wh_type, fuel);
        let ua_inputs = UaInputs {
            category,
            energy_factor,
            uniform_energy_factor,
            tank_volume_rated_gal,
            recovery_efficiency,
            heating_capacity_btu_hr,
            first_hour_rating_gal,
        };
        // A UA calculation failure is a construction error naming the water
        // heater: the tank's UA is a physical input, and the default the
        // failure used to fall back to would silently re-rate the tank. An
        // indirect tank is exempt: its water-side efficiency is the boiler's
        // (the loop wiring carries it), so no EF→UA derivation applies and
        // the tank model's own UA default stands.
        let ua_result = if name == "Indirect Tank" {
            None
        } else {
            ua_from_energy_factor(&ua_inputs).map_err(|err| {
                super::HpxmlError::Parse(
                    format!(
                        "water heater '{}': UA calculation failed: {err}; the tank's UA \
                         is a physical input and no default may stand in for it",
                        element_id(wh).unwrap_or_else(|| "unknown".to_string())
                    )
                    .into(),
                )
            })?
        };
        let ua_w_per_k = ua_result.map(|r| r.ua_w_per_k);

        let mut spec = match name.as_str() {
            "Gas Water Heater" => {
                let conversion_efficiency = ua_result.map(|r| r.conversion_efficiency);
                let gas_flue_loss_fraction = child_f64(wh, &FLUE_LOSS_FRACTION)
                    .or_else(|| conversion_efficiency.map(|_| 0.0));
                // Draft-inducer / power-vent blower wattage. The HPXML schema
                // has no standard WaterHeatingSystem element for water-heater
                // fan power, so HARES reads the `extension/FanPowerWatts`
                // convention already used for HVAC systems (see
                // resolve_hvac.rs and OCHRE's HPXML parser,
                // vendors/OCHRE/ochre/utils/hpxml.py:900). Absent → None →
                // 0 W (atmospheric vent), leaving existing simulations
                // unchanged.
                let fan_power_w = wh
                    .path(&[&EXTENSION])
                    .and_then(|ext| child_power_w(ext, &FAN_POWER_WATTS));
                let cfg = GasWaterHeaterConfig {
                    equipment_id: None,
                    zone_id,
                    loop_id: None,
                    fuel_type: fuel,
                    tank_volume_m3,
                    tank_height_m,
                    energy_factor,
                    uniform_energy_factor,
                    heating_capacity_w,
                    ua_w_per_k,
                    setpoint_c,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    avg_water_draw_l_per_day,
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    appliance_draw_flow_rate_source: None,
                    pilot_power_w: child_pilot_power_w(wh),
                    flue_loss_fraction: gas_flue_loss_fraction,
                    skin_loss_fraction: None,
                    ignition_type: child_text(wh, &IGNITION_TYPE),
                    performance_adjustment,
                    zone_type: zone_name.clone(),
                    first_hour_rating_m3,
                    jacket_r_value_m2_k_w,
                    conversion_efficiency,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                    pilot_fraction_to_tank: None,
                    fan_power_w,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)?
            }
            "Electric Resistance Water Heater" => {
                let cfg = ElectricResistanceWaterHeaterConfig {
                    equipment_id: None,
                    zone_id,
                    loop_id: None,
                    tank_volume_m3,
                    tank_height_m,
                    energy_factor,
                    uniform_energy_factor,
                    heating_capacity_w,
                    ua_w_per_k,
                    setpoint_c,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    avg_water_draw_l_per_day,
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    appliance_draw_flow_rate_source: None,
                    performance_adjustment,
                    zone_type: zone_name.clone(),
                    first_hour_rating_m3,
                    element_power_w: heating_capacity_w,
                    element_priority_mode: None,
                    max_setpoint_ramp_rate_c_per_min: None,
                    max_combined_power_w: None,
                    jacket_r_value_m2_k_w,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)?
            }
            "Tankless Water Heater" | "Gas Tankless Water Heater" => {
                // UEF-only inputs (no EF) use 0.94 default per RESNET 301;
                // EF-sourced inputs use the legacy 0.92 default.
                let default_perf_adj = if energy_factor.is_none() { 0.94 } else { 0.92 };
                let perf_adj = child_f64(wh, &PERFORMANCE_ADJUSTMENT).unwrap_or(default_perf_adj);
                let cfg = TanklessWaterHeaterConfig {
                    equipment_id: None,
                    zone_id,
                    loop_id: None,
                    fuel_type: fuel,
                    energy_factor,
                    uniform_energy_factor,
                    heating_capacity_w,
                    setpoint_c,
                    parasitic_power_w: (fuel == FuelType::Gas)
                        .then(|| tankless_parasitic_power_w(n_bedrooms.expect("n_bedrooms always Some; parse_avg_water_draw_and_bedrooms resolves the count from the HPXML or errors"))),
                    performance_adjustment: Some(perf_adj),
                    inlet_temp_c: None,
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    appliance_draw_flow_rate_source: None,
                    avg_water_draw_l_per_day,
                    zone_type: zone_name.clone(),
                    min_flow_kg_s: None,
                    min_flow_gpm: None,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)?
            }
            "Heat Pump Water Heater" => {
                // UEF→EF conversion coefficients for HPWH.
                // Source: ResStock waterheater.rb; NREL ResStock calibration (Maguire & Roberts 2020).
                const HPWH_UEF_TO_EF_SLOPE: f64 = 0.60522;
                const HPWH_UEF_TO_EF_DENOM: f64 = 1.2101;
                // Scale factor mapping UEF to rated COP for HPWH.
                // Source: OCHRE WaterHeater.py; derived from GE GeoSpring calibration.
                const HPWH_UEF_TO_COP: f64 = 1.174_536_058;
                let uef = uniform_energy_factor.or_else(|| {
                    energy_factor.map(|ef| (HPWH_UEF_TO_EF_SLOPE + ef) / HPWH_UEF_TO_EF_DENOM)
                });
                let low_power = uef.is_some_and(|u| u >= 4.9);
                // HARES divergence from OCHRE: OCHRE's HPXML import layer
                // (ochre/utils/hpxml.py:1174-1181) uses exact-equality
                // UEF == 4.9 as a sentinel for one specific 120V ResStock
                // product ("temporary flag for designating 120V HPWHs in
                // [the] panels branch of ResStock"). HARES generalizes to
                // a physically-motivated continuous UEF threshold: cop is
                // always derived from UEF via HPWH_UEF_TO_COP, and the
                // low-power coefficient set and ambient bounds are handled
                // via blending in HeatPumpWH::init_typed — the parser does
                // not substitute manufacturer-specific preset values.
                let cop = uef.map(|uef_val| HPWH_UEF_TO_COP * uef_val);
                let tempering_valve_setpoint_c = setpoint_c;
                let cfg = HeatPumpWaterHeaterConfig {
                    equipment_id: None,
                    zone_id,
                    loop_id: None,
                    tank_volume_m3,
                    tank_height_m,
                    cop,
                    backup_element_power_w: heating_capacity_w,
                    ua_w_per_k,
                    setpoint_c,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    tempering_valve_setpoint_c,
                    avg_water_draw_l_per_day,
                    draw_flow_rate_kg_s: None,
                    appliance_draw_flow_rate_source: None,
                    compressor_power_w: None,
                    backup_enable_offset_c: None,
                    min_ambient_temp_c: None,
                    max_ambient_temp_c: None,
                    min_on_time_s: None,
                    min_off_time_s: None,
                    hp_only_mode: None,
                    element_hp_control_mode: None,
                    fan_power_w: None,
                    parasitic_power_w: None,
                    backup_efficiency: None,
                    shr: None,
                    lost_heat_fraction: None,
                    wall_heat_fraction: Some(hpwh_wall_heat_fraction(&location)),
                    capacity_biquadratic_coeffs: None,
                    cop_biquadratic_coeffs: None,
                    cop_curve_is_normalized: None,
                    performance_adjustment,
                    zone_type: zone_name.clone(),
                    first_hour_rating_m3,
                    jacket_r_value_m2_k_w,
                    fixture_delivery_temp_c: None,
                    low_power_hpwh: Some(low_power),
                    uniform_energy_factor: uef,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)?
            }
            "Indirect Tank" => {
                let cfg = IndirectTankConfig {
                    equipment_id: None,
                    zone_id,
                    boiler_loop_id: None,
                    tank_volume_m3,
                    tank_height_m,
                    ua_w_per_k,
                    hx_ua_w_per_k: None,
                    setpoint_c,
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    draw_flow_rate_kg_s: None,
                    avg_water_draw_l_per_day,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    appliance_draw_flow_rate_source: None,
                    performance_adjustment,
                    zone_type: zone_name.clone(),
                    first_hour_rating_m3,
                    jacket_r_value_m2_k_w,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                    boiler_loop_flow_rate_kg_s: None,
                };
                typed_spec(name.clone(), fuel, cfg, defaults)?
            }
            // Unreachable: canonical_water_heater_name (called at the top of
            // this loop) generates exactly the six names matched above and
            // rejects all others with Err, so execution never reaches this arm.
            _ => unreachable!(
                "canonical_water_heater_name validated '{}' but match did not cover it",
                name,
            ),
        };

        if name == "Indirect Tank" {
            spec.related_hvac_idref = related_hvac_idref;
        }
        spec.system_id = element_id(wh);

        // FractionDHWLoadServed apportionment (OpenStudio-HPXML
        // hotwater_appliances.rb:383 and :474-487): each water heating
        // system's share of the home's hot-water load scales its fixture
        // draw and wet-appliance volumes. The schematron requires either
        // the element on every system summing to 1 or on none at all; a
        // file without it has a single system serving everything, so the
        // absent value reads as 1.0. A share of zero draws nothing, the
        // same skip `if gpd_frac > 0` runs.
        let fraction_dhw_load_served = match child_f64(wh, &FRACTION_DHW_LOAD_SERVED) {
            // A negative share is a validation error, not a value the
            // clamp silently turns into a zero-draw system: the schematron
            // bounds the element and a mistyped negative would otherwise
            // hide the mistype behind a max(0.0).
            Some(raw) if raw < 0.0 || !raw.is_finite() => {
                return Err(super::HpxmlError::InvalidField {
                    path: "WaterHeatingSystem/FractionDHWLoadServed",
                    system_kind: "Water Heating System",
                    system_id: element_id(wh).unwrap_or_else(|| "unknown".to_string()),
                    value_received: raw.to_string(),
                    reason: "the share of the home's hot-water load the system serves \
                             cannot be negative",
                });
            }
            Some(raw) => {
                declared_fractions
                    .push((element_id(wh).unwrap_or_else(|| "unknown".to_string()), raw));
                raw
            }
            None => 1.0,
        };
        spec.parameters.insert(
            "fraction_dhw_load_served".to_string(),
            json!(fraction_dhw_load_served),
        );

        // Set autosize_water_heater flag when capacity or tank volume is
        // absent for storage-type water heaters. Tankless water heaters
        // are sized by flow rate, not FHR/tank volume, and are excluded.
        let is_storage_type = matches!(
            name.as_str(),
            "Electric Resistance Water Heater"
                | "Gas Water Heater"
                | "Heat Pump Water Heater"
                | "Indirect Tank"
        );
        if is_storage_type {
            let needs_capacity = heating_capacity_w.is_none();
            let needs_volume = tank_volume_m3.is_none();
            if needs_capacity || needs_volume {
                spec.parameters
                    .insert("autosize_water_heater".to_string(), json!(true));
                // Parse factor from extension if present.
                if let Some(factor) = wh
                    .path(&[&EXTENSION])
                    .and_then(|ext| child_f64(ext, &WATER_HEATER_AUTOSIZING_FACTOR))
                {
                    spec.parameters
                        .insert("autosize_water_heater_factor".to_string(), json!(factor));
                }
            }
        }

        specs.push(spec);
    }
    // Invariant: when multiple WaterHeatingSystem elements are present, each
    // must carry a unique SystemIdentifier/@id so downstream code can
    // cross-reference individual water heaters without ambiguity.
    let wh_count = descendants_named(details, &WATER_HEATING_SYSTEM).len();
    // The schematron's sum rule (EPvalidator.sch:290) on the multi-system
    // path: the declared FractionDHWLoadServed values sum to 1 (within
    // [0.99, 1.01]) when any system declares one, or no system declares
    // one. Shares outside the band apportion the hot-water load wrongly,
    // so the resolve fails naming the systems and the sum.
    if wh_count > 1 && !declared_fractions.is_empty() {
        let sum: f64 = declared_fractions.iter().map(|(_, f)| *f).sum();
        if !(0.99..=1.01).contains(&sum) {
            let systems: Vec<String> = declared_fractions
                .iter()
                .map(|(id, f)| format!("{id}: {f}"))
                .collect();
            return Err(super::HpxmlError::Parse(
                format!(
                    "sum(FractionDHWLoadServed) across {wh_count} WaterHeatingSystem elements \
                     is {sum} (systems: {}): the schematron expects the shares to sum to 1 \
                     (within [0.99, 1.01]) or the element on no system at all",
                    systems.join(", "),
                )
                .into(),
            ));
        }
    }
    if wh_count > 1 {
        let wh_specs_start = specs.len().saturating_sub(wh_count);
        let ids: Vec<_> = specs[wh_specs_start..]
            .iter()
            .filter_map(|s| s.system_id.as_deref())
            .collect();
        if ids.len() != wh_count {
            return Err(super::HpxmlError::Parse(format!(
                "expected {wh_count} unique system identifiers for {wh_count} WaterHeatingSystem \
                 elements but only {found} have an id attribute",
                found = ids.len(),
            ).into()));
        }
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        if sorted.len() != ids.len() {
            return Err(super::HpxmlError::Parse(
                format!(
                    "duplicate SystemIdentifier/@id found among {wh_count} WaterHeatingSystem \
                 elements: {ids:?}"
                )
                .into(),
            ));
        }
    }

    Ok(())
}

/// Parse the combined average daily hot water draw [L/day] and bedroom count from `<BuildingDetails>`.
///
/// Reads `<BuildingSummary>/<BuildingConstruction>/<NumberofBedrooms>`,
/// `<WaterHeating>/<WaterFixture>/<LowFlow>`, the `<WaterFixturesUsageMultiplier>` extension,
/// and `<HotWaterDistribution>` to compute the OCHRE/ANSI-RESNET 301 draw estimate.
///
/// A missing bedroom count is [`super::HpxmlError::MissingField`] naming the
/// field (via [`resolve_bedroom_count`], the same rule
/// [`extract_bedroom_count`] applies): the draw and the water-heater sizing
/// inputs are required, not defaulted.
fn parse_avg_water_draw_and_bedrooms(
    details: &XmlNode,
    data_patches: Option<&HpxmlDataPatches>,
) -> Result<(Option<f64>, Option<f64>), super::HpxmlError> {
    let n_bedrooms_raw = match details
        .path(&[
            &BUILDING_SUMMARY,
            &BUILDING_CONSTRUCTION,
            &NUMBEROF_BEDROOMS,
        ])
        .and_then(|n| parse_trimmed_f64(&n.text))
    {
        Some(v) => v,
        None => resolve_bedroom_count(None, data_patches)?,
    };

    // Adjust bedroom count by occupancy and house type (OCHRE hpxml.py:789-797),
    // the shared adjustment the appliance energies also apply.
    let n_occupants = details
        .path(&[&BUILDING_SUMMARY, &BUILDING_OCCUPANCY, &NUMBEROF_RESIDENTS])
        .and_then(|n| parse_trimmed_f64(&n.text));
    let house_type = details
        .path(&[
            &BUILDING_SUMMARY,
            &BUILDING_CONSTRUCTION,
            &RESIDENTIAL_FACILITY_TYPE,
        ])
        .map(|n| n.text.trim().to_ascii_lowercase());
    let n_bedrooms = super::resolve_loads::adjusted_bedroom_count(
        n_occupants,
        house_type.as_deref(),
        n_bedrooms_raw,
    );

    // Fixture efficiency: low-flow if any WaterFixture has <LowFlow>true</LowFlow>.
    let mut any_low_flow = false;
    if let Some(wh_section) = details.path(&[&WATER_HEATING]) {
        for fixture in descendants_named(wh_section, &WATER_FIXTURE) {
            any_low_flow |=
                child_bool(fixture, &LOW_FLOW, "WaterFixture/LowFlow", "Water Fixture")?
                    .unwrap_or(false);
        }
    }
    let fixture_efficiency = if any_low_flow {
        FixtureEfficiency::LowFlow
    } else {
        FixtureEfficiency::Standard
    };

    // Usage multiplier from the WaterHeating extension (OCHRE: WaterFixturesUsageMultiplier).
    let usage_multiplier = details
        .path(&[&WATER_HEATING, &EXTENSION])
        .and_then(|ext| child_f64(ext, &WATER_FIXTURES_USAGE_MULTIPLIER))
        .unwrap_or(1.0);

    // Distribution system.
    let distribution = parse_distribution_system(details, n_bedrooms);

    let draw_l_per_day = combined_daily_hot_water_l(
        n_bedrooms,
        fixture_efficiency,
        usage_multiplier,
        &distribution,
    );

    Ok((Some(draw_l_per_day), Some(n_bedrooms)))
}

/// Public entry point: extract the bedroom count from a parsed [`Building`].
///
/// Reads `<BuildingSummary>/<BuildingConstruction>/<NumberofBedrooms>` from
/// the building's details XML. Falls back to data patches when the HPXML
/// field is absent. The field is required input: OCHRE reads it with a
/// direct index and fails on absence (hpxml.py:263,
/// `construction["NumberofBedrooms"]`; the OS-HPXML defaults apply no
/// default either, defaults.rb `apply_water_heaters` reads it directly,
/// :3303), so an unavailable count is an error naming the field, never a
/// substituted count.
pub fn extract_bedroom_count(
    building: &Building,
    data_patches: Option<&HpxmlDataPatches>,
) -> Result<f64, super::HpxmlError> {
    let details = &building.details_xml;
    let raw = details
        .path(&[
            &BUILDING_SUMMARY,
            &BUILDING_CONSTRUCTION,
            &NUMBEROF_BEDROOMS,
        ])
        .and_then(|n| parse_trimmed_f64(&n.text));
    raw.or_else(|| data_patches.and_then(|p| p.number_of_bedrooms))
        .ok_or_else(|| super::HpxmlError::MissingField {
            path: "BuildingSummary/BuildingConstruction/NumberofBedrooms",
            system_kind: "BuildingConstruction",
            system_id: "Building".to_string(),
            reason: "the water heater tank volume and first-hour rating are sized by \
                     the bedroom count; OCHRE reads the field with a direct index \
                     and fails on absence (hpxml.py:263), so no count may be \
                     substituted for it",
        })
}

fn typed_spec<T>(
    name: String,
    fuel_type: FuelType,
    cfg: T,
    defaults: &DefaultsStore,
) -> Result<EquipmentSpec, super::HpxmlError>
where
    T: hares_equipment::EquipmentTypedConfig + Serialize,
{
    let parameters = serde_json::to_value(&cfg)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let typed_config =
        EquipmentConfig::from_typed(name.clone(), T::equipment_type_name().to_string(), cfg)?;
    Ok(EquipmentSpec {
        name: name.clone(),
        instance_name: None,
        fuel_type,
        parameters,
        zip_params: defaults.zip_params(&name).cloned(),
        typed_overrides: serde_json::Map::new(),
        typed_config: Some(typed_config),
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
        heating_setpoint_source: None,
        cooling_setpoint_source: None,
    })
}

/// Parse a FuelType string value from HPXML into a [`FuelType`].
///
/// Accepts the HPXML fuel type vocabulary and maps it to the canonical
/// [`FuelType`] enum. Returns `Err` for unrecognised fuel type strings.
fn parse_water_heater_fuel_from_str(raw: &str) -> Result<FuelType, super::HpxmlError> {
    super::xml_helpers::parse_fuel(Some(raw))
}

/// Resolve the fuel type for a `WaterHeatingSystem` element.
///
/// For space-heating boiler types (indirect tanks), `FuelType` is optional in
/// HPXML — the fuel comes from the linked heating system via `RelatedHVACSystem`.
/// When absent and the type is a boiler-linked system, we default to `Gas` since
/// the linked boiler's fuel will be resolved independently.
/// For all other water heater types, `FuelType` is required.
fn parse_water_heater_fuel(
    wh: &XmlNode,
    wh_type: &str,
) -> std::result::Result<FuelType, super::HpxmlError> {
    match child_text(wh, &FUEL_TYPE) {
        Some(raw) => parse_water_heater_fuel_from_str(&raw),
        None => {
            let ty = wh_type.trim();
            if ty == "space-heating boiler with storage tank"
                || ty == "space-heating boiler with tankless coil"
            {
                Ok(FuelType::Gas)
            } else {
                Err(super::HpxmlError::Parse(
                    format!("WaterHeatingSystem type '{ty}' is missing required FuelType").into(),
                ))
            }
        }
    }
}

/// Parse `<HotWaterDistribution>` from the `<WaterHeating>` section.
///
/// Falls back to `DistributionSystem::Unknown` when the section is absent or the type
/// is unrecognised.  Default piping length for Standard distribution is derived from
/// conditioned floor area and number of floors following the OCHRE formula.
fn parse_distribution_system(details: &XmlNode, n_bedrooms: f64) -> DistributionSystem {
    let Some(dist_node) = details.path(&[&WATER_HEATING, &HOT_WATER_DISTRIBUTION]) else {
        return DistributionSystem::Unknown;
    };

    // HPXML PipeRValue is in hr·ft²·°F/Btu (IP R-value). Convert to SI [m²·K/W] at parse time.
    let pipe_r_value = dist_node
        .path(&[&PIPE_INSULATION])
        .and_then(|n| child_f64(n, &PIPE_R_VALUE))
        .map(conv::r_value_ip_to_si)
        .unwrap_or(0.0);

    let system_type = dist_node.child(&SYSTEM_TYPE);

    if let Some(standard) = system_type.and_then(|n| n.child(&STANDARD)) {
        // Default piping length: OCHRE formula using floor area and floors.
        // 2 * sqrt(floor_area_ft2 / floors) + 10 * floors + 5 * has_unfinished_basement
        // We use a bedroom-count proxy when floor area data is not available.
        let default_piping_length_m = derive_default_piping_length_m(details, n_bedrooms);
        let piping_length_m = child_f64(standard, &PIPING_LENGTH).map(conv::length_ft_to_m); // HPXML PipingLength is in feet
        DistributionSystem::Standard {
            pipe_r_value,
            piping_length_m,
            default_piping_length_m,
        }
    } else if let Some(recirc) = system_type.and_then(|n| n.child(&RECIRCULATION)) {
        // BranchPipingLoopLength is in feet in HPXML.
        let branch_loop_length_m =
            child_f64(recirc, &BRANCH_PIPING_LOOP_LENGTH).map(conv::length_ft_to_m);
        DistributionSystem::Recirculation {
            pipe_r_value,
            branch_loop_length_m,
        }
    } else {
        DistributionSystem::Unknown
    }
}

/// Derive the OCHRE default piping length [m] from building geometry.
///
/// OCHRE formula (converted to SI):
/// `default_length_ft = 2 * sqrt(floor_area_ft2 / floors) + 10 * floors + 5 * has_unfinished_basement`
///
/// Falls back to a bedroom-count proxy (25 + 5 * n_bedrooms ft ≈ 7.6 + 1.5 * n_bedrooms m)
/// when floor area data is unavailable.
fn derive_default_piping_length_m(details: &XmlNode, n_bedrooms: f64) -> f64 {
    let floor_area_m2 = details
        .path(&[
            &BUILDING_SUMMARY,
            &BUILDING_CONSTRUCTION,
            &CONDITIONED_FLOOR_AREA,
        ])
        .and_then(|n| {
            let val = parse_trimmed_f64(&n.text)?;
            // HPXML ConditionedFloorArea may carry a `units` attribute; default is ft2.
            let units = n.attrs.get("units").map(String::as_str).unwrap_or("ft2");
            if units.eq_ignore_ascii_case("m2") {
                Some(val)
            } else {
                Some(conv::area_ft2_to_m2(val))
            }
        });

    let n_floors = details
        .path(&[
            &BUILDING_SUMMARY,
            &BUILDING_CONSTRUCTION,
            &NUMBEROF_CONDITIONED_FLOORS,
        ])
        .or_else(|| {
            details.path(&[
                &BUILDING_SUMMARY,
                &BUILDING_CONSTRUCTION,
                &NUMBEROF_CONDITIONED_FLOORS_ABOVE_GRADE,
            ])
        })
        .and_then(|n| parse_trimmed_f64(&n.text))
        .unwrap_or(1.0)
        .max(1.0);

    let has_unfinished_bsmt = details
        .path(&[&BUILDING_SUMMARY, &BUILDING_CONSTRUCTION, &FOUNDATION_TYPE])
        .map(|n| n.text.trim().eq_ignore_ascii_case("UnfinishedBasement"))
        .unwrap_or(false);

    if let Some(area_m2) = floor_area_m2 {
        let area_ft2 = conv::area_m2_to_ft2(area_m2);
        let ft_per_floor = area_ft2 / n_floors;
        let default_ft = 2.0 * ft_per_floor.sqrt()
            + 10.0 * n_floors
            + if has_unfinished_bsmt { 5.0 } else { 0.0 };
        conv::length_ft_to_m(default_ft)
    } else {
        // Bedroom-count proxy when floor area is absent.
        conv::length_ft_to_m(25.0 + 5.0 * n_bedrooms)
    }
}

fn wh_category(wh_type: &str, fuel: FuelType) -> WhCategory {
    match (wh_type.trim(), fuel) {
        ("heat pump water heater", FuelType::Electric) => WhCategory::HeatPump,
        ("instantaneous water heater", _) => WhCategory::Instantaneous,
        (_, FuelType::Electric) => WhCategory::StorageElectric,
        // The remaining classes canonical_water_heater_name maps to a
        // canonical name are storage-tank UA categories, not unrecognized
        // combinations: a non-electric storage tank (the Gas Water Heater
        // class) and the two indirect-tank combi types (the Indirect Tank
        // class). Naming them before the catch-all keeps the catch-all's
        // warning off every file those canonical names resolve; only an
        // input no canonical name matches warns.
        ("storage water heater", _) => WhCategory::StorageGas,
        (
            "space-heating boiler with storage tank" | "space-heating boiler with tankless coil",
            _,
        ) => WhCategory::StorageGas,
        (other_type, other_fuel) => {
            tracing::warn!(
                wh_type = other_type,
                fuel = ?other_fuel,
                "Unrecognized water heater type/fuel combination; \
                 defaulting UA category to StorageGas"
            );
            WhCategory::StorageGas
        }
    }
}

fn canonical_water_heater_name(
    wh_type: &str,
    fuel: FuelType,
) -> std::result::Result<String, super::HpxmlError> {
    let ty = wh_type.trim();
    let name = match (ty, fuel) {
        ("storage water heater", FuelType::Electric) => "Electric Resistance Water Heater",
        ("instantaneous water heater", FuelType::Electric) => "Tankless Water Heater",
        ("heat pump water heater", FuelType::Electric) => "Heat Pump Water Heater",
        (
            "storage water heater",
            FuelType::Gas
            | FuelType::Propane
            | FuelType::Oil
            | FuelType::Wood
            | FuelType::Coal
            | FuelType::WoodPellet,
        ) => "Gas Water Heater",
        (
            "instantaneous water heater",
            FuelType::Gas
            | FuelType::Propane
            | FuelType::Oil
            | FuelType::Wood
            | FuelType::Coal
            | FuelType::WoodPellet,
        ) => "Gas Tankless Water Heater",
        ("space-heating boiler with storage tank", _) => "Indirect Tank",
        ("space-heating boiler with tankless coil", _) => "Indirect Tank",
        _ => {
            return Err(super::HpxmlError::Parse(
                format!(
                    "unsupported HPXML water heater type/fuel combination: \
                 WaterHeaterType='{ty}', fuel='{fuel:?}'"
                )
                .into(),
            ));
        }
    };
    Ok(name.to_string())
}

/// Parse an electrical-power child element into SI watts, honouring an
/// optional HPXML `units` attribute (SI-at-the-boundary policy: never read an
/// HPXML numeric without checking its declared units). Elements named
/// `*Watts` carry watts by definition, so `W` is the default when no `units`
/// attribute is present; an explicit `kW` is converted. Unrecognized units
/// are rejected (with a warning) rather than guessed.
fn child_power_w(node: &XmlNode, element: &'static super::elements::HpxmlElement) -> Option<f64> {
    let el = node.child(element)?;
    let value = hares_types::parse_trimmed_f64(&el.text)?;
    let units = el.attrs.get("units").map(String::as_str).unwrap_or("W");
    match units.to_ascii_lowercase().as_str() {
        "w" | "watts" => Some(value),
        "kw" => Some(conv::power_kw_to_w(value)),
        other => {
            tracing::warn!(
                units = other,
                value,
                field = element.name,
                "unrecognized power unit; accepted: W, kW — ignoring value"
            );
            None
        }
    }
}

/// Parse the `PilotPower` child element into SI watts, honouring an optional
/// HPXML `units` attribute (SI-at-the-boundary policy: never read an HPXML
/// numeric without checking its declared units). HPXML expresses pilot-light
/// power in Btu/h — the sibling HVAC convention is literally named
/// `PilotLightBtuh` (see vendors/OCHRE/test/OS-HPXML Sample Files/
/// base-hvac-furnace-gas-only-pilot.xml and OCHRE's parser,
/// vendors/OCHRE/ochre/utils/hpxml.py:845) — so `Btuh` is the default when no
/// `units` attribute is present. An explicit `W` is accepted as-is.
/// Unrecognized units are rejected (with a warning) rather than guessed.
fn child_pilot_power_w(node: &XmlNode) -> Option<f64> {
    let el = node.child(&PILOT_POWER)?;
    let value = hares_types::parse_trimmed_f64(&el.text)?;
    let units = el.attrs.get("units").map(String::as_str).unwrap_or("Btuh");
    match units.to_ascii_lowercase().as_str() {
        "btuh" | "btu/h" | "btu/hr" => Some(conv::power_btu_h_to_w(value)),
        "w" | "watts" => Some(value),
        other => {
            tracing::warn!(
                units = other,
                value,
                field = "PilotPower",
                "unrecognized power unit; accepted: Btuh, W — ignoring value"
            );
            None
        }
    }
}

fn param_f64(params: &Map<String, Value>, key: &str) -> Option<f64> {
    params.get(key).and_then(Value::as_f64)
}

fn param_u16(params: &Map<String, Value>, key: &str) -> Option<u16> {
    params.get(key).and_then(Value::as_u64).map(|v| v as u16)
}

fn param_str(params: &Map<String, Value>, key: &str) -> Option<String> {
    params.get(key).and_then(Value::as_str).map(str::to_string)
}

fn param_fuel(params: &Map<String, Value>) -> FuelType {
    param_str(params, "fuel_type")
        .and_then(|s| match s.to_lowercase().as_str() {
            "gas" | "natural gas" => Some(FuelType::Gas),
            "propane" => Some(FuelType::Propane),
            "oil" | "fuel oil" => Some(FuelType::Oil),
            "wood" => Some(FuelType::Wood),
            "coal" => Some(FuelType::Coal),
            "woodpellet" | "wood pellet" => Some(FuelType::WoodPellet),
            _ => None,
        })
        .unwrap_or(FuelType::Gas)
}

/// Build a `GasWaterHeaterConfig` typed config.
///
/// # Errors
///
/// A `from_typed` failure naming the equipment and the config type.
fn try_build_gas_wh_config(
    name: &str,
    params: &Map<String, Value>,
) -> Result<EquipmentConfig, super::HpxmlError> {
    let cfg = GasWaterHeaterConfig {
        equipment_id: None,
        zone_id: param_u16(params, "zone_id"),
        loop_id: param_u16(params, "loop_id"),
        fuel_type: param_fuel(params),
        tank_volume_m3: param_f64(params, "tank_volume_m3"),
        tank_height_m: param_f64(params, "tank_height_m"),
        energy_factor: param_f64(params, "energy_factor"),
        uniform_energy_factor: param_f64(params, "uniform_energy_factor"),
        heating_capacity_w: param_f64(params, "heating_capacity_w"),
        ua_w_per_k: param_f64(params, "ua_w_per_k"),
        setpoint_c: param_f64(params, "setpoint_c"),
        deadband_c: param_f64(params, "deadband_c"),
        max_tank_temp_c: param_f64(params, "max_tank_temp_c"),
        initial_tank_temp_c: param_f64(params, "initial_tank_temp_c"),
        tank_nodes: None,
        avg_water_draw_l_per_day: param_f64(params, "avg_water_draw_l_per_day"),
        draw_flow_rate_kg_s: param_f64(params, "draw_flow_rate_kg_s"),
        draw_flow_rate_source: None,
        mains_temp_c_source: None,
        appliance_draw_flow_rate_source: None,
        pilot_power_w: param_f64(params, "pilot_power_w"),
        flue_loss_fraction: param_f64(params, "flue_loss_fraction"),
        skin_loss_fraction: param_f64(params, "skin_loss_fraction"),
        ignition_type: param_str(params, "ignition_type"),
        performance_adjustment: param_f64(params, "performance_adjustment"),
        zone_type: param_str(params, "zone_type"),
        first_hour_rating_m3: param_f64(params, "first_hour_rating_m3"),
        jacket_r_value_m2_k_w: param_f64(params, "jacket_r_value_m2_k_w"),
        conversion_efficiency: param_f64(params, "conversion_efficiency"),
        fixture_delivery_temp_c: param_f64(params, "fixture_delivery_temp_c"),
        hot_draw_temp_c: param_f64(params, "hot_draw_temp_c"),
        pilot_fraction_to_tank: param_f64(params, "pilot_fraction_to_tank"),
        fan_power_w: param_f64(params, "fan_power_w"),
    };
    Ok(EquipmentConfig::from_typed(
        name.to_string(),
        "Gas Water Heater".to_string(),
        cfg,
    )?)
}

/// Build an `ElectricResistanceWaterHeaterConfig` typed config.
///
/// # Errors
///
/// A `from_typed` failure naming the equipment and the config type.
fn try_build_elec_res_wh_config(
    name: &str,
    params: &Map<String, Value>,
) -> Result<EquipmentConfig, super::HpxmlError> {
    let cfg = ElectricResistanceWaterHeaterConfig {
        equipment_id: None,
        zone_id: param_u16(params, "zone_id"),
        loop_id: param_u16(params, "loop_id"),
        tank_volume_m3: param_f64(params, "tank_volume_m3"),
        tank_height_m: param_f64(params, "tank_height_m"),
        energy_factor: param_f64(params, "energy_factor"),
        uniform_energy_factor: param_f64(params, "uniform_energy_factor"),
        heating_capacity_w: param_f64(params, "heating_capacity_w"),
        ua_w_per_k: param_f64(params, "ua_w_per_k"),
        setpoint_c: param_f64(params, "setpoint_c"),
        deadband_c: param_f64(params, "deadband_c"),
        max_tank_temp_c: param_f64(params, "max_tank_temp_c"),
        initial_tank_temp_c: param_f64(params, "initial_tank_temp_c"),
        tank_nodes: None,
        avg_water_draw_l_per_day: param_f64(params, "avg_water_draw_l_per_day"),
        draw_flow_rate_kg_s: param_f64(params, "draw_flow_rate_kg_s"),
        draw_flow_rate_source: None,
        mains_temp_c_source: None,
        appliance_draw_flow_rate_source: None,
        performance_adjustment: param_f64(params, "performance_adjustment"),
        zone_type: param_str(params, "zone_type"),
        first_hour_rating_m3: param_f64(params, "first_hour_rating_m3"),
        element_power_w: param_f64(params, "element_power_w")
            .or_else(|| param_f64(params, "heating_capacity_w")),
        max_setpoint_ramp_rate_c_per_min: param_f64(params, "max_setpoint_ramp_rate_c_per_min"),
        element_priority_mode: param_str(params, "element_priority_mode"),
        jacket_r_value_m2_k_w: param_f64(params, "jacket_r_value_m2_k_w"),
        max_combined_power_w: param_f64(params, "max_combined_power_w"),
        fixture_delivery_temp_c: param_f64(params, "fixture_delivery_temp_c"),
        hot_draw_temp_c: param_f64(params, "hot_draw_temp_c"),
    };
    Ok(EquipmentConfig::from_typed(
        name.to_string(),
        "Electric Resistance Water Heater".to_string(),
        cfg,
    )?)
}

/// Build a `HeatPumpWaterHeaterConfig` typed config.
///
/// # Errors
///
/// A `from_typed` failure naming the equipment and the config type.
fn try_build_hpwh_config(
    name: &str,
    params: &Map<String, Value>,
) -> Result<EquipmentConfig, super::HpxmlError> {
    let cfg = HeatPumpWaterHeaterConfig {
        equipment_id: None,
        zone_id: param_u16(params, "zone_id"),
        loop_id: param_u16(params, "loop_id"),
        tank_volume_m3: param_f64(params, "tank_volume_m3"),
        tank_height_m: param_f64(params, "tank_height_m"),
        cop: param_f64(params, "cop"),
        backup_element_power_w: param_f64(params, "backup_element_power_w")
            .or_else(|| param_f64(params, "heating_capacity_w")),
        ua_w_per_k: param_f64(params, "ua_w_per_k"),
        setpoint_c: param_f64(params, "setpoint_c"),
        deadband_c: param_f64(params, "deadband_c"),
        max_tank_temp_c: param_f64(params, "max_tank_temp_c"),
        initial_tank_temp_c: param_f64(params, "initial_tank_temp_c"),
        tank_nodes: None,
        tempering_valve_setpoint_c: param_f64(params, "tempering_valve_setpoint_c"),
        avg_water_draw_l_per_day: param_f64(params, "avg_water_draw_l_per_day"),
        draw_flow_rate_kg_s: param_f64(params, "draw_flow_rate_kg_s"),
        compressor_power_w: param_f64(params, "compressor_power_w"),
        appliance_draw_flow_rate_source: None,
        backup_enable_offset_c: param_f64(params, "backup_enable_offset_c"),
        min_ambient_temp_c: param_f64(params, "min_ambient_temp_c"),
        max_ambient_temp_c: param_f64(params, "max_ambient_temp_c"),
        min_on_time_s: param_f64(params, "min_on_time_s"),
        min_off_time_s: param_f64(params, "min_off_time_s"),
        hp_only_mode: params.get("hp_only_mode").and_then(Value::as_bool),
        element_hp_control_mode: param_str(params, "element_hp_control_mode"),
        fan_power_w: param_f64(params, "fan_power_w"),
        parasitic_power_w: param_f64(params, "parasitic_power_w"),
        backup_efficiency: param_f64(params, "backup_efficiency"),
        shr: param_f64(params, "shr"),
        lost_heat_fraction: param_f64(params, "lost_heat_fraction"),
        wall_heat_fraction: param_f64(params, "wall_heat_fraction"),
        capacity_biquadratic_coeffs: None,
        cop_biquadratic_coeffs: None,
        cop_curve_is_normalized: None,
        performance_adjustment: param_f64(params, "performance_adjustment"),
        zone_type: param_str(params, "zone_type"),
        first_hour_rating_m3: param_f64(params, "first_hour_rating_m3"),
        jacket_r_value_m2_k_w: param_f64(params, "jacket_r_value_m2_k_w"),
        fixture_delivery_temp_c: param_f64(params, "fixture_delivery_temp_c"),
        low_power_hpwh: params.get("low_power_hpwh").and_then(Value::as_bool),
        uniform_energy_factor: param_f64(params, "uniform_energy_factor"),
    };
    Ok(EquipmentConfig::from_typed(
        name.to_string(),
        "Heat Pump Water Heater".to_string(),
        cfg,
    )?)
}

/// Build a `TanklessWaterHeaterConfig` typed config.
///
/// # Errors
///
/// A `from_typed` failure naming the equipment and the config type.
fn try_build_tankless_wh_config(
    name: &str,
    params: &Map<String, Value>,
) -> Result<EquipmentConfig, super::HpxmlError> {
    let cfg = TanklessWaterHeaterConfig {
        equipment_id: None,
        zone_id: param_u16(params, "zone_id"),
        loop_id: param_u16(params, "loop_id"),
        fuel_type: param_fuel(params),
        energy_factor: param_f64(params, "energy_factor"),
        uniform_energy_factor: param_f64(params, "uniform_energy_factor"),
        heating_capacity_w: param_f64(params, "heating_capacity_w"),
        setpoint_c: param_f64(params, "setpoint_c"),
        parasitic_power_w: param_f64(params, "parasitic_power_w"),
        performance_adjustment: param_f64(params, "performance_adjustment"),
        inlet_temp_c: param_f64(params, "inlet_temp_c"),
        draw_flow_rate_kg_s: param_f64(params, "draw_flow_rate_kg_s"),
        draw_flow_rate_source: None,
        mains_temp_c_source: None,
        appliance_draw_flow_rate_source: None,
        avg_water_draw_l_per_day: param_f64(params, "avg_water_draw_l_per_day"),
        zone_type: param_str(params, "zone_type"),
        min_flow_kg_s: None,
        min_flow_gpm: None,
    };
    Ok(EquipmentConfig::from_typed(
        name.to_string(),
        "Tankless Water Heater".to_string(),
        cfg,
    )?)
}

/// Build an `IndirectTankConfig` typed config.
///
/// # Errors
///
/// A `from_typed` failure naming the equipment and the config type.
fn try_build_indirect_tank_config(
    name: &str,
    params: &Map<String, Value>,
) -> Result<EquipmentConfig, super::HpxmlError> {
    let cfg = IndirectTankConfig {
        equipment_id: None,
        zone_id: param_u16(params, "zone_id"),
        boiler_loop_id: param_u16(params, "boiler_loop_id"),
        tank_volume_m3: param_f64(params, "tank_volume_m3"),
        tank_height_m: param_f64(params, "tank_height_m"),
        ua_w_per_k: param_f64(params, "ua_w_per_k"),
        hx_ua_w_per_k: param_f64(params, "hx_ua_w_per_k"),
        setpoint_c: param_f64(params, "setpoint_c"),
        deadband_c: param_f64(params, "deadband_c"),
        max_tank_temp_c: param_f64(params, "max_tank_temp_c"),
        initial_tank_temp_c: param_f64(params, "initial_tank_temp_c"),
        tank_nodes: None,
        draw_flow_rate_kg_s: param_f64(params, "draw_flow_rate_kg_s"),
        avg_water_draw_l_per_day: param_f64(params, "avg_water_draw_l_per_day"),
        draw_flow_rate_source: None,
        mains_temp_c_source: None,
        appliance_draw_flow_rate_source: None,
        performance_adjustment: param_f64(params, "performance_adjustment"),
        zone_type: param_str(params, "zone_type"),
        first_hour_rating_m3: param_f64(params, "first_hour_rating_m3"),
        jacket_r_value_m2_k_w: param_f64(params, "jacket_r_value_m2_k_w"),
        fixture_delivery_temp_c: param_f64(params, "fixture_delivery_temp_c"),
        hot_draw_temp_c: param_f64(params, "hot_draw_temp_c"),
        boiler_loop_flow_rate_kg_s: param_f64(params, "boiler_loop_flow_rate_kg_s"),
    };
    Ok(EquipmentConfig::from_typed(
        name.to_string(),
        "Indirect Tank".to_string(),
        cfg,
    )?)
}

/// Rebuild a water heater equipment typed config using updated parameters.
///
/// Called after autosizing to replace placeholder capacities and volumes
/// with computed values. Matches on the canonical equipment name to select
/// the correct config builder. A builder failure is an error naming the
/// water heater and the failure: a rebuild that silently left the raw spec
/// (placeholder capacities and volumes) in place would run autosized
/// numbers through un-autosized equipment.
///
/// # Errors
///
/// The matched builder's own error, or the unrecognized-name error.
pub fn rebuild_wh_typed_config(
    name: &str,
    params: &Map<String, Value>,
) -> Result<EquipmentConfig, super::HpxmlError> {
    match name {
        "Gas Water Heater" => try_build_gas_wh_config(name, params),
        "Electric Resistance Water Heater" => try_build_elec_res_wh_config(name, params),
        "Heat Pump Water Heater" => try_build_hpwh_config(name, params),
        "Tankless Water Heater" => try_build_tankless_wh_config(name, params),
        "Indirect Tank" => try_build_indirect_tank_config(name, params),
        other => Err(super::HpxmlError::Parse(
            format!("rebuild_wh_typed_config called with unrecognized water heater name '{other}'")
                .into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hpxml::parse_xml_document;
    use serde_json::Value;

    use hares_physics::units as conv;
    use hares_types::FuelType;

    use crate::defaults::DefaultsStore;

    use super::super::building::{Building, Site, SiteType, Zone, ZoneType};

    use std::io::Write as IoWrite;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt::MakeWriter;

    /// A tracing writer capturing formatted events into a shared buffer,
    /// so a pin can assert on the warnings the resolver emits.
    #[derive(Clone, Default)]
    struct LogBuf(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for LogBuf {
        type Writer = LogBufGuard;

        fn make_writer(&'a self) -> Self::Writer {
            LogBufGuard(self.0.clone())
        }
    }

    struct LogBufGuard(Arc<Mutex<Vec<u8>>>);

    impl IoWrite for LogBufGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log buffer mutex").write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.0.lock().expect("log buffer mutex").flush()
        }
    }

    /// Run `f` under a WARN-level capturing subscriber and return the
    /// captured log text.
    fn captured_warnings(f: impl FnOnce()) -> String {
        let buf = LogBuf::default();
        let subscriber = tracing_subscriber::fmt::Subscriber::builder()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .with_writer(buf.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let captured = buf.0.lock().expect("log buffer mutex").clone();
        String::from_utf8(captured).expect("log buffer is utf8")
    }

    /// Build a minimal [`Building`] wrapping the details node from a parsed HPXML.
    /// Used by tests that only exercise water-heater parsing and do not provide
    /// a complete building envelope. Zones are empty so `zone_id_for_location`
    /// returns `None`, preserving the pre-fix behaviour for tests that don't set
    /// zone topology.
    fn building_for_test(root: &XmlNode) -> Building {
        let details = root
            .path(&[&BUILDING, &BUILDING_DETAILS])
            .cloned()
            .unwrap_or_else(|| root.clone());
        Building {
            site: Site {
                elevation_m: None,
                site_type: Some(SiteType::Suburban),
                shielding_of_home: None,
                latitude_deg: None,
                longitude_deg: None,
                utc_offset_h: None,
            },
            // Every HPXML building has its conditioned zone.
            zones: vec![Zone {
                zone_type: ZoneType::Conditioned,
                floor_area_m2: None,
                volume_m3: None,
                attached_wall_ids: vec![],
                duct_systems: vec![],
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
                height_m: None,
                hpxml_location: None,
            }],
            boundaries: vec![],
            windows: vec![],
            skylights: vec![],
            infiltration_ach50: None,
            infiltration_cfm50: None,
            infiltration_ach_natural: None,
            infiltration_cfm_natural: None,
            infiltration_ela_cm2: None,
            infiltration_constant_ach: Some(0.0),
            hvac_capacity_w: None,
            seer2: None,
            hspf2: None,
            water_heater_setpoint_c: None,
            heating_weekday_setpoints_c: None,
            heating_weekend_setpoints_c: None,
            cooling_weekday_setpoints_c: None,
            cooling_weekend_setpoints_c: None,
            battery_round_trip_efficiency: None,
            pv_tilt_deg: None,
            conditioned_volume_m3: 400.0,
            ceiling_height_m: 2.5,
            infiltration_height_m: None,
            floors_above_grade: 1.0,
            has_flue_or_chimney: None,
            foundation_name: None,
            conditioned_foundation_merged: false,
            residential_facility_type: None,
            temperature_capacitance_multiplier: 7.0,
            hvac_deadband_c: None,
            climate_zone_iecc: None,
            unavailable_periods: Vec::new(),
            details_xml: details,
            parse_warnings: Vec::new(),
            eri_calculation_version: None,
        }
    }

    #[test]
    fn propane_storage_water_heater_maps_to_gas_class() {
        let name = canonical_water_heater_name("storage water heater", FuelType::Propane)
            .expect("propane storage WH should map to gas class");
        assert_eq!(name, "Gas Water Heater");
    }

    /// The post-autosize typed-config rebuild keeps the loop id each
    /// water-heater class was wired or allocated: every builder reads it
    /// from the raw parameters under the key the wiring pass and the loop
    /// allocator mirror it into (`loop_id` for the water-heater classes,
    /// `boiler_loop_id` for the indirect tank).
    #[test]
    fn rebuild_keeps_the_params_loop_id_for_every_water_heater_class() {
        for (name, key) in [
            ("Gas Water Heater", "loop_id"),
            ("Electric Resistance Water Heater", "loop_id"),
            ("Tankless Water Heater", "loop_id"),
            ("Heat Pump Water Heater", "loop_id"),
            ("Indirect Tank", "boiler_loop_id"),
        ] {
            let mut params = serde_json::Map::new();
            params.insert(
                key.to_string(),
                serde_json::Value::Number(serde_json::Number::from(3_u16)),
            );
            let cfg = rebuild_wh_typed_config(name, &params)
                .unwrap_or_else(|e| panic!("{name} rebuilds from params: {e}"));
            let loop_id = match name {
                "Indirect Tank" => {
                    cfg.typed::<IndirectTankConfig>()
                        .expect("indirect tank config")
                        .boiler_loop_id
                }
                "Gas Water Heater" => {
                    cfg.typed::<GasWaterHeaterConfig>()
                        .expect("gas wh config")
                        .loop_id
                }
                "Electric Resistance Water Heater" => {
                    cfg.typed::<ElectricResistanceWaterHeaterConfig>()
                        .expect("electric resistance wh config")
                        .loop_id
                }
                "Tankless Water Heater" => {
                    cfg.typed::<TanklessWaterHeaterConfig>()
                        .expect("tankless wh config")
                        .loop_id
                }
                "Heat Pump Water Heater" => {
                    cfg.typed::<HeatPumpWaterHeaterConfig>()
                        .expect("heat pump wh config")
                        .loop_id
                }
                other => panic!("unmatched water heater class: {other}"),
            };
            assert_eq!(
                loop_id,
                Some(3),
                "{name} rebuild keeps the params {key} it was wired or allocated"
            );
        }
    }

    /// Without a loop id in the parameters the rebuild leaves the field
    /// unset, exactly as before the id ever travelled there.
    #[test]
    fn rebuild_without_a_params_loop_id_leaves_it_none() {
        for (name, key) in [
            ("Gas Water Heater", "loop_id"),
            ("Electric Resistance Water Heater", "loop_id"),
            ("Tankless Water Heater", "loop_id"),
            ("Heat Pump Water Heater", "loop_id"),
            ("Indirect Tank", "boiler_loop_id"),
        ] {
            let params = serde_json::Map::new();
            let cfg = rebuild_wh_typed_config(name, &params)
                .unwrap_or_else(|e| panic!("{name} rebuilds from params: {e}"));
            let loop_id = match name {
                "Indirect Tank" => {
                    cfg.typed::<IndirectTankConfig>()
                        .expect("indirect tank config")
                        .boiler_loop_id
                }
                "Gas Water Heater" => {
                    cfg.typed::<GasWaterHeaterConfig>()
                        .expect("gas wh config")
                        .loop_id
                }
                "Electric Resistance Water Heater" => {
                    cfg.typed::<ElectricResistanceWaterHeaterConfig>()
                        .expect("electric resistance wh config")
                        .loop_id
                }
                "Tankless Water Heater" => {
                    cfg.typed::<TanklessWaterHeaterConfig>()
                        .expect("tankless wh config")
                        .loop_id
                }
                "Heat Pump Water Heater" => {
                    cfg.typed::<HeatPumpWaterHeaterConfig>()
                        .expect("heat pump wh config")
                        .loop_id
                }
                other => panic!("unmatched water heater class: {other}"),
            };
            assert_eq!(
                loop_id, None,
                "{name} rebuild with no {key} param leaves the loop id unset"
            );
        }
    }

    /// A rebuild error is the builder's own named error or the dispatch's
    /// unrecognized-name error: the dispatch's former `None` (which
    /// silently downgraded the spec to its raw parameters) is gone, and a
    /// rebuild that fails construction names the water heater.
    #[test]
    fn wh_typed_config_rebuild_error_fails_construction() {
        // The unknown-name error names the name (the dispatch's own arm).
        let err = rebuild_wh_typed_config("Not A Water Heater", &Map::new())
            .expect_err("an unrecognized name is an error naming it");
        assert!(
            err.to_string().contains("Not A Water Heater"),
            "the error names the water heater, got: {err}"
        );
    }

    /// Every type/fuel combination `canonical_water_heater_name` accepts
    /// resolves to its UA category without the unrecognized-combination
    /// warning: the named arms cover the known classes, so the catch-all's
    /// warning only fires for genuinely unrecognized input. The indirect
    /// tank (both combi types) is the class that previously warned on
    /// every file that resolved it.
    #[test]
    fn wh_category_warns_only_on_genuinely_unrecognized_combinations() {
        let recognized: &[(&str, FuelType, WhCategory)] = &[
            (
                "heat pump water heater",
                FuelType::Electric,
                WhCategory::HeatPump,
            ),
            (
                "instantaneous water heater",
                FuelType::Electric,
                WhCategory::Instantaneous,
            ),
            (
                "instantaneous water heater",
                FuelType::Propane,
                WhCategory::Instantaneous,
            ),
            (
                "storage water heater",
                FuelType::Electric,
                WhCategory::StorageElectric,
            ),
            (
                "storage water heater",
                FuelType::Gas,
                WhCategory::StorageGas,
            ),
            (
                "space-heating boiler with storage tank",
                FuelType::Gas,
                WhCategory::StorageGas,
            ),
            (
                "space-heating boiler with tankless coil",
                FuelType::Gas,
                WhCategory::StorageGas,
            ),
        ];
        for (wh_type, fuel, expected) in recognized {
            let category = captured_warnings(|| {
                let resolved = wh_category(wh_type, *fuel);
                assert_eq!(
                    resolved, *expected,
                    "{wh_type}/{fuel:?} resolves to its named category"
                );
            });
            assert!(
                !category.contains("Unrecognized water heater type/fuel"),
                "{wh_type}/{fuel:?} is a recognized combination and must not warn, got: {category}"
            );
        }

        let unknown = captured_warnings(|| {
            let category = wh_category("solar thermal", FuelType::Gas);
            assert_eq!(
                category,
                WhCategory::StorageGas,
                "genuinely unrecognized combinations keep the StorageGas default"
            );
        });
        assert!(
            unknown.contains("Unrecognized water heater type/fuel"),
            "a genuinely unrecognized combination still warns, got: {unknown}"
        );
    }

    /// Resolving an indirect-tank file warns nothing about its type/fuel
    /// combination: the class is a recognized combination, and the
    /// catch-all's warning never reaches an indirect-tank file.
    #[test]
    fn indirect_tank_file_emits_no_unrecognized_combination_warning() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>space-heating boiler with storage tank</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <TankVolume>40</TankVolume>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        let log = captured_warnings(|| {
            resolve_water_heaters(
                &building,
                building.climate_zone_iecc.as_deref(),
                &DefaultsStore::empty(),
                &mut specs,
                None,
                &mut Vec::new(),
            )
            .expect("combi boiler with storage tank must resolve successfully");
        });
        assert!(
            !log.contains("Unrecognized water heater type/fuel"),
            "an indirect-tank file must not warn about its type/fuel combination, got: {log}"
        );
        assert!(
            specs.iter().any(|s| s.name == "Indirect Tank"),
            "the file still resolves to the Indirect Tank spec"
        );
    }

    #[test]
    fn typed_spec_serializes_canonical_fields() {
        let cfg = GasWaterHeaterConfig {
            fan_power_w: None,
            equipment_id: None,
            zone_id: Some(2),
            loop_id: Some(3),
            fuel_type: FuelType::Propane,
            tank_volume_m3: Some(0.151),
            tank_height_m: Some(1.2),
            energy_factor: Some(0.78),
            uniform_energy_factor: Some(0.81),
            heating_capacity_w: Some(11_000.0),
            ua_w_per_k: Some(2.5),
            setpoint_c: Some(51.67),
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: Some(227.0),
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            appliance_draw_flow_rate_source: None,
            pilot_power_w: None,
            flue_loss_fraction: None,
            skin_loss_fraction: None,
            ignition_type: None,
            performance_adjustment: Some(0.92),
            zone_type: Some("conditioned".to_string()),
            first_hour_rating_m3: Some(0.2),
            jacket_r_value_m2_k_w: None,
            conversion_efficiency: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            pilot_fraction_to_tank: None,
        };
        let spec = typed_spec(
            "Gas Water Heater".to_string(),
            FuelType::Propane,
            cfg,
            &DefaultsStore::empty(),
        )
        .unwrap();
        assert!(
            spec.typed_config
                .as_ref()
                .expect("typed payload")
                .is_typed()
        );
        assert_eq!(
            spec.parameters.get("fuel_type").and_then(Value::as_str),
            Some("Propane")
        );
        assert_eq!(
            spec.parameters
                .get("avg_water_draw_l_per_day")
                .and_then(Value::as_f64),
            Some(227.0)
        );
    }

    #[test]
    fn typed_spec_preserves_explicit_none_schedule_sources() {
        let cfg = ElectricResistanceWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            setpoint_c: Some(51.67),
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            appliance_draw_flow_rate_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            element_power_w: None,
            max_setpoint_ramp_rate_c_per_min: None,
            element_priority_mode: None,
            jacket_r_value_m2_k_w: None,
            max_combined_power_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        };
        let spec = typed_spec(
            "Electric Resistance Water Heater".to_string(),
            FuelType::Electric,
            cfg,
            &DefaultsStore::empty(),
        )
        .unwrap();
        assert_eq!(
            spec.parameters.get("draw_flow_rate_source"),
            Some(&Value::Null)
        );
        assert_eq!(
            spec.parameters.get("mains_temp_c_source"),
            Some(&Value::Null)
        );
        let typed_cfg: ElectricResistanceWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed resistance config")
            .typed()
            .expect("typed resistance decode");
        assert!(typed_cfg.draw_flow_rate_source.is_none());
        assert!(typed_cfg.mains_temp_c_source.is_none());
    }

    #[test]
    fn resolve_water_heaters_parses_realistic_tankless_xml() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <extension>
                      <WaterFixturesUsageMultiplier>1.05</WaterFixturesUsageMultiplier>
                    </extension>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>electricity</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                      <PerformanceAdjustment>0.93</PerformanceAdjustment>
                      <Location>attic</Location>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");

        let spec = specs
            .iter()
            .find(|s| s.name == "Tankless Water Heater")
            .expect("tankless spec must be emitted");
        let cfg: TanklessWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed tankless config");

        assert_eq!(cfg.setpoint_c, Some(conv::temperature_f_to_c(120.0)));
        assert_eq!(cfg.performance_adjustment, Some(0.93));
        assert!(
            cfg.avg_water_draw_l_per_day
                .expect("avg draw should be derived")
                > 0.0
        );
        assert!(cfg.draw_flow_rate_source.is_none());
        assert!(cfg.mains_temp_c_source.is_none());
    }

    #[test]
    fn gas_tankless_parasitic_power_computed_from_hpxml_bedrooms() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>4</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");

        let cfg: TanklessWaterHeaterConfig = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("tankless spec must be emitted")
            .typed_config
            .as_ref()
            .expect("typed config")
            .typed()
            .expect("tankless config");

        let expected_w = 5.0 + 60.0 * 0.0462_f64;
        assert_eq!(
            cfg.parasitic_power_w,
            Some(expected_w),
            "gas tankless parasitic for 4 bedrooms must be {expected_w} W"
        );
    }

    #[test]
    fn gas_tankless_parasitic_power_falls_back_to_data_patches() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let patches = HpxmlDataPatches {
            number_of_bedrooms: Some(5.0),
        };

        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            Some(&patches),
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");

        let cfg: TanklessWaterHeaterConfig = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("tankless spec must be emitted")
            .typed_config
            .as_ref()
            .expect("typed config")
            .typed()
            .expect("tankless config");

        let expected_w = 5.0 + 60.0 * 0.0529_f64;
        assert_eq!(
            cfg.parasitic_power_w,
            Some(expected_w),
            "gas tankless parasitic for 5 bedrooms from patches must be {expected_w} W"
        );
    }

    #[test]
    fn gas_tankless_parasitic_power_uses_the_stated_bedroom_count() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>2</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");

        let cfg: TanklessWaterHeaterConfig = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("tankless spec must be emitted")
            .typed_config
            .as_ref()
            .expect("typed config")
            .typed()
            .expect("tankless config");

        let expected_w = 5.0 + 60.0 * 0.0333_f64;
        assert_eq!(
            cfg.parasitic_power_w,
            Some(expected_w),
            "gas tankless parasitic at the stated 2 bedrooms must be {expected_w} W"
        );
    }

    /// A tankless water heater with no stated bedroom count fails the
    /// resolve naming the field: the RESNET 301 on-time table is indexed
    /// by the bedroom count and the field is required input (OCHRE reads
    /// it with a direct index, hpxml.py:263); no count is substituted for
    /// it.
    #[test]
    fn gas_tankless_without_a_bedroom_count_errors_naming_the_field() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        let err = resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect_err("an absent NumberofBedrooms must fail the water-heater resolve");
        assert!(
            err.to_string().contains("NumberofBedrooms"),
            "the error names the required field: {err}"
        );
    }

    #[test]
    fn electric_tankless_parasitic_power_is_none() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>electricity</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");

        let cfg: TanklessWaterHeaterConfig = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("tankless spec must be emitted")
            .typed_config
            .as_ref()
            .expect("typed config")
            .typed()
            .expect("tankless config");

        assert!(
            cfg.parasitic_power_w.is_none(),
            "electric tankless must not have parasitic_power_w"
        );
    }

    /// An HPXML whose construction names no NumberofBedrooms fails the
    /// extract naming the field: the tank volume and the first-hour rating
    /// are sized by it and OCHRE reads it with a direct index (hpxml.py:263).
    #[test]
    fn a_bedroom_count_without_numberofbedrooms_errors_naming_the_field() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let err = extract_bedroom_count(&building, None)
            .expect_err("an absent NumberofBedrooms must fail the extract");
        assert!(
            err.to_string().contains("NumberofBedrooms"),
            "the error names the required field: {err}"
        );

        // A data patch satisfies the requirement when the HPXML field is
        // absent.
        let patches = HpxmlDataPatches {
            number_of_bedrooms: Some(4.0),
        };
        assert_eq!(
            extract_bedroom_count(&building, Some(&patches)).expect("the patch supplies the count"),
            4.0
        );
    }

    #[test]
    fn hpxml_bedrooms_take_priority_over_data_patches() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>4</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let patches = HpxmlDataPatches {
            number_of_bedrooms: Some(1.0),
        };

        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            Some(&patches),
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");

        let cfg: TanklessWaterHeaterConfig = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("tankless spec must be emitted")
            .typed_config
            .as_ref()
            .expect("typed config")
            .typed()
            .expect("tankless config");

        let expected_w = 5.0 + 60.0 * 0.0462_f64;
        assert_eq!(
            cfg.parasitic_power_w,
            Some(expected_w),
            "HPXML bedrooms=4 must beat patches bedrooms=1; expected {expected_w} W for 4 beds"
        );
    }

    #[test]
    fn fractional_bedrooms_rounded_to_nearest_integer() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3.7</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.91</EnergyFactor>
                      <HeatingCapacity>45000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");

        let cfg: TanklessWaterHeaterConfig = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("tankless spec must be emitted")
            .typed_config
            .as_ref()
            .expect("typed config")
            .typed()
            .expect("tankless config");

        let expected_w = 5.0 + 60.0 * 0.0462_f64;
        assert_eq!(
            cfg.parasitic_power_w,
            Some(expected_w),
            "fractional 3.7 bedrooms must round to 4; expected {expected_w} W"
        );
    }

    #[test]
    fn invalid_water_heater_fuel_is_rejected() {
        let wh = XmlNode {
            name: "WaterHeatingSystem".to_string(),
            attrs: Default::default(),
            text: String::new(),
            children: vec![
                XmlNode {
                    name: "FuelType".to_string(),
                    attrs: Default::default(),
                    text: "mystery-fuel".to_string(),
                    children: vec![],
                },
                XmlNode {
                    name: "WaterHeaterType".to_string(),
                    attrs: Default::default(),
                    text: "storage water heater".to_string(),
                    children: vec![],
                },
            ],
        };

        let err = parse_water_heater_fuel(&wh, "storage water heater")
            .expect_err("invalid fuel must be rejected");
        assert!(err.to_string().contains("unsupported FuelType"));
    }

    #[test]
    fn parse_water_heater_fuel_none_maps_to_none() {
        let result = parse_water_heater_fuel_from_str("none");
        assert!(
            result.is_ok(),
            "\"none\" is a valid HPXML fuel type and must map to FuelType::None"
        );
        assert_eq!(result.unwrap(), FuelType::None);
    }

    #[test]
    fn jacket_r_value_parsed_and_converted_to_si() {
        let r_ip = 5.0_f64;
        let expected_si = conv::r_value_ip_to_si(r_ip);

        let xml = format!(
            r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>electricity</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <TankVolume>50</TankVolume>
                      <EnergyFactor>0.92</EnergyFactor>
                      <WaterHeaterInsulation>
                        <Jacket>
                          <JacketRValue>{r_ip}</JacketRValue>
                        </Jacket>
                      </WaterHeaterInsulation>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#
        );
        let root = parse_xml_document(&xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");

        let spec = specs
            .iter()
            .find(|s| s.name == "Electric Resistance Water Heater")
            .expect("ERWH spec must be emitted");
        let cfg: ElectricResistanceWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed ERWH config");

        let jacket_si = cfg
            .jacket_r_value_m2_k_w
            .expect("jacket_r_value_m2_k_w must be populated");
        assert!(
            (jacket_si - expected_si).abs() < 1e-9,
            "expected {expected_si} m²·K/W, got {jacket_si}"
        );
    }

    #[test]
    fn jacket_r_value_absent_yields_none() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>electricity</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <TankVolume>50</TankVolume>
                      <EnergyFactor>0.92</EnergyFactor>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");

        let spec = specs
            .iter()
            .find(|s| s.name == "Electric Resistance Water Heater")
            .expect("ERWH spec must be emitted");
        let cfg: ElectricResistanceWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed ERWH config");

        assert!(
            cfg.jacket_r_value_m2_k_w.is_none(),
            "jacket_r_value_m2_k_w must be None when HPXML element is absent"
        );
    }

    fn gas_wh_xml_with_ignition(ignition_type: Option<&str>) -> String {
        let ignition_elem = ignition_type
            .map(|v| format!("                      <IgnitionType>{v}</IgnitionType>\n"))
            .unwrap_or_default();
        format!(
            r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <TankVolume>40</TankVolume>
                      <EnergyFactor>0.59</EnergyFactor>
                      <HeatingCapacity>36000</HeatingCapacity>
{ignition_elem}
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#
        )
    }

    fn parse_gas_wh_config(xml: &str) -> GasWaterHeaterConfig {
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);
        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");
        let spec = specs
            .iter()
            .find(|s| s.name == "Gas Water Heater")
            .expect("Gas Water Heater spec must be emitted");
        spec.typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed GasWaterHeaterConfig")
    }

    #[test]
    fn hpxml_electronic_ignition_type_is_parsed() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_ignition(Some("electronic ignition")));
        assert_eq!(
            cfg.ignition_type.as_deref(),
            Some("electronic ignition"),
            "IgnitionType 'electronic ignition' must be forwarded to config"
        );
    }

    #[test]
    fn hpxml_standing_pilot_ignition_type_is_parsed() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_ignition(Some("standing pilot")));
        assert_eq!(
            cfg.ignition_type.as_deref(),
            Some("standing pilot"),
            "IgnitionType 'standing pilot' must be forwarded to config"
        );
    }

    #[test]
    fn hpxml_absent_ignition_type_yields_none() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_ignition(None));
        assert!(
            cfg.ignition_type.is_none(),
            "absent IgnitionType must yield None in config"
        );
    }

    fn gas_wh_xml_with_pilot_power(pilot_power_elem: &str) -> String {
        format!(
            r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <TankVolume>40</TankVolume>
                      <EnergyFactor>0.59</EnergyFactor>
                      <HeatingCapacity>36000</HeatingCapacity>
                      {pilot_power_elem}
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#
        )
    }

    #[test]
    fn pilot_power_without_units_defaults_to_btu_h_and_converts_to_watts() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_pilot_power("<PilotPower>600</PilotPower>"));
        let expected_w = 600.0 * 0.293_071_07;
        let pw = cfg
            .pilot_power_w
            .expect("pilot_power_w must be set when PilotPower is in HPXML");
        assert!(
            (pw - expected_w).abs() < 0.01,
            "PilotPower=600 BTU/h must convert to ~{expected_w:.2} W, got {pw:.2}"
        );
    }

    #[test]
    fn pilot_power_with_explicit_btuh_units_converts_to_watts() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_pilot_power(
            r#"<PilotPower units="Btuh">500</PilotPower>"#,
        ));
        let expected_w = 500.0 * 0.293_071_07;
        let pw = cfg
            .pilot_power_w
            .expect("pilot_power_w must be set for units=\"Btuh\"");
        assert!(
            (pw - expected_w).abs() < 0.01,
            "PilotPower=500 Btuh must convert to ~{expected_w:.2} W, got {pw:.2}"
        );
    }

    #[test]
    fn pilot_power_with_explicit_watt_units_passes_through_unconverted() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_pilot_power(
            r#"<PilotPower units="W">150</PilotPower>"#,
        ));
        assert_eq!(
            cfg.pilot_power_w,
            Some(150.0),
            "PilotPower with units=\"W\" must be taken as SI watts without conversion"
        );
    }

    #[test]
    fn pilot_power_with_unknown_units_is_rejected() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_pilot_power(
            r#"<PilotPower units="therms">600</PilotPower>"#,
        ));
        assert!(
            cfg.pilot_power_w.is_none(),
            "PilotPower with unrecognized units must be rejected (None), not guessed"
        );
    }

    #[test]
    fn pilot_power_absent_yields_none() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_ignition(None));
        assert!(
            cfg.pilot_power_w.is_none(),
            "absent PilotPower must yield None in config"
        );
    }

    fn gas_wh_xml_with_extension(extension: &str) -> String {
        format!(
            r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <TankVolume>40</TankVolume>
                      <EnergyFactor>0.59</EnergyFactor>
                      <HeatingCapacity>36000</HeatingCapacity>
{extension}
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#
        )
    }

    #[test]
    fn gas_wh_fan_power_watts_extension_is_mapped() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_extension(
            "                      <extension><FanPowerWatts>65</FanPowerWatts></extension>",
        ));
        assert_eq!(
            cfg.fan_power_w,
            Some(65.0),
            "extension/FanPowerWatts must map to fan_power_w in watts"
        );
    }

    #[test]
    fn gas_wh_fan_power_units_attribute_is_honoured() {
        // SI-at-the-boundary: an explicit units attribute must be checked and
        // converted, never assumed.
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_extension(
            "                      <extension><FanPowerWatts units=\"kW\">0.065</FanPowerWatts></extension>",
        ));
        assert_eq!(
            cfg.fan_power_w,
            Some(65.0),
            "kW units must convert to watts"
        );
    }

    #[test]
    fn gas_wh_fan_power_unrecognized_units_rejected() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_extension(
            "                      <extension><FanPowerWatts units=\"hp\">0.1</FanPowerWatts></extension>",
        ));
        assert!(
            cfg.fan_power_w.is_none(),
            "unrecognized power units must be rejected, not guessed"
        );
    }

    #[test]
    fn gas_wh_fan_power_absent_yields_none() {
        let cfg = parse_gas_wh_config(&gas_wh_xml_with_ignition(None));
        assert!(
            cfg.fan_power_w.is_none(),
            "absent extension/FanPowerWatts must yield None (0 W, atmospheric vent)"
        );
    }

    #[test]
    fn hpwh_heating_capacity_converted_from_btu_h_to_watts() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>electricity</FuelType>
                      <WaterHeaterType>heat pump water heater</WaterHeaterType>
                      <TankVolume>50</TankVolume>
                      <HotWaterTemperature>125</HotWaterTemperature>
                      <UniformEnergyFactor>3.75</UniformEnergyFactor>
                      <HeatingCapacity>4500</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);
        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");
        let spec = specs
            .iter()
            .find(|s| s.name == "Heat Pump Water Heater")
            .expect("HPWH spec must be emitted");
        let cfg: HeatPumpWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed HeatPumpWaterHeaterConfig");

        let expected_w = conv::power_btu_h_to_w(4500.0);
        let actual_w = cfg
            .backup_element_power_w
            .expect("backup_element_power_w must be populated");
        assert!(
            (actual_w - expected_w).abs() < 1e-6,
            "expected {expected_w} W, got {actual_w} W (HeatingCapacity must be converted from Btu/h)"
        );
    }

    /// Gas WH resolver must populate conversion_efficiency (eta_c) from the UA derivation
    /// and set flue_loss_fraction=0 when deriving from EF/RE/capacity.
    /// EF=0.59, RE=0.76, 36 kBtu/hr => eta_c ~ 0.782.
    #[test]
    fn gas_wh_resolver_emits_eta_c_as_conversion_efficiency() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <TankVolume>40</TankVolume>
                      <EnergyFactor>0.59</EnergyFactor>
                      <RecoveryEfficiency>0.76</RecoveryEfficiency>
                      <HeatingCapacity>36000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);
        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");
        let spec = specs
            .iter()
            .find(|s| s.name == "Gas Water Heater")
            .expect("Gas Water Heater spec must be emitted");
        let cfg: GasWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed GasWaterHeaterConfig");

        let eta_c = cfg
            .conversion_efficiency
            .expect("conversion_efficiency must be populated");
        assert!(
            (eta_c - 0.782).abs() < 0.01,
            "expected eta_c ~ 0.782, got {eta_c:.4}"
        );
        assert!(
            eta_c > 0.76,
            "eta_c must be greater than RE (0.76), got {eta_c:.4}"
        );
        assert_eq!(
            cfg.energy_factor,
            Some(0.59),
            "energy_factor must remain the original EF, not eta_c"
        );
        assert_eq!(
            cfg.flue_loss_fraction,
            Some(0.0),
            "flue_loss_fraction must be 0.0 when eta_c is derived from UA calc"
        );
    }

    /// Tankless WH with UEF-only (no EF) should default perf_adj to 0.94.
    #[test]
    fn tankless_uef_only_default_performance_adjustment() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <UniformEnergyFactor>0.87</UniformEnergyFactor>
                      <HeatingCapacity>199000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);
        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");
        let spec = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("Tankless spec must be emitted");
        let cfg: TanklessWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed TanklessWaterHeaterConfig");

        assert_eq!(
            cfg.performance_adjustment,
            Some(0.94),
            "UEF-only tankless must default perf_adj to 0.94, got {:?}",
            cfg.performance_adjustment
        );
    }

    /// Tankless WH with EF (not UEF-only) should default perf_adj to 0.92.
    #[test]
    fn tankless_ef_default_performance_adjustment() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>instantaneous water heater</WaterHeaterType>
                      <EnergyFactor>0.82</EnergyFactor>
                      <HeatingCapacity>199000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);
        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");
        let spec = specs
            .iter()
            .find(|s| s.name.contains("Tankless"))
            .expect("Tankless spec must be emitted");
        let cfg: TanklessWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed TanklessWaterHeaterConfig");

        assert_eq!(
            cfg.performance_adjustment,
            Some(0.92),
            "EF-sourced tankless must default perf_adj to 0.92, got {:?}",
            cfg.performance_adjustment
        );
    }

    // Combi boiler types (HPXML v4 §8.5) resolve to the Indirect Tank
    // equipment model for combined boiler + indirect-tank DHW configurations.
    // Full IndirectTank equipment model is implemented in T-0134.

    #[test]
    fn combi_boiler_with_storage_tank_type_resolves_to_indirect_tank() {
        let name =
            canonical_water_heater_name("space-heating boiler with storage tank", FuelType::Gas)
                .expect("combi boiler with storage tank must resolve to Indirect Tank");
        assert_eq!(name, "Indirect Tank");
    }

    #[test]
    fn combi_boiler_with_tankless_coil_type_resolves_to_indirect_tank() {
        let name =
            canonical_water_heater_name("space-heating boiler with tankless coil", FuelType::Gas)
                .expect("combi boiler with tankless coil must resolve to Indirect Tank");
        assert_eq!(name, "Indirect Tank");
    }

    #[test]
    fn combi_boiler_full_xml_round_trip_resolves_to_indirect_tank() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>space-heating boiler with storage tank</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <TankVolume>40</TankVolume>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);

        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("combi boiler with storage tank must resolve successfully");

        let spec = specs
            .iter()
            .find(|s| s.name == "Indirect Tank")
            .expect("Indirect Tank spec must be emitted");
        let cfg: IndirectTankConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed IndirectTankConfig");
        assert_eq!(cfg.setpoint_c, Some(conv::temperature_f_to_c(120.0)));
        let expected_vol_m3 = conv::volume_gal_to_m3(40.0 * 0.95);
        assert!(
            (cfg.tank_volume_m3.expect("tank volume must be set") - expected_vol_m3).abs() < 1e-6
        );
        assert!(
            cfg.avg_water_draw_l_per_day
                .expect("avg draw should be derived")
                > 0.0
        );
    }

    #[test]
    fn zone_id_for_location_maps_each_zone_type() {
        let building = Building {
            site: Site {
                elevation_m: None,
                site_type: Some(SiteType::Suburban),
                shielding_of_home: None,
                latitude_deg: None,
                longitude_deg: None,
                utc_offset_h: None,
            },
            zones: vec![
                Zone {
                    zone_type: ZoneType::Conditioned,
                    floor_area_m2: None,
                    volume_m3: None,
                    attached_wall_ids: vec![],
                    duct_systems: vec![],
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                    height_m: None,
                    hpxml_location: None,
                },
                Zone {
                    zone_type: ZoneType::Garage,
                    floor_area_m2: None,
                    volume_m3: None,
                    attached_wall_ids: vec![],
                    duct_systems: vec![],
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                    height_m: None,
                    hpxml_location: None,
                },
                Zone {
                    zone_type: ZoneType::Attic,
                    floor_area_m2: None,
                    volume_m3: None,
                    attached_wall_ids: vec![],
                    duct_systems: vec![],
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                    height_m: None,
                    hpxml_location: None,
                },
            ],
            boundaries: vec![],
            windows: vec![],
            skylights: vec![],
            infiltration_ach50: None,
            infiltration_cfm50: None,
            infiltration_ach_natural: None,
            infiltration_cfm_natural: None,
            infiltration_ela_cm2: None,
            infiltration_constant_ach: Some(0.0),
            hvac_capacity_w: None,
            seer2: None,
            hspf2: None,
            water_heater_setpoint_c: None,
            heating_weekday_setpoints_c: None,
            heating_weekend_setpoints_c: None,
            cooling_weekday_setpoints_c: None,
            cooling_weekend_setpoints_c: None,
            battery_round_trip_efficiency: None,
            pv_tilt_deg: None,
            conditioned_volume_m3: 400.0,
            ceiling_height_m: 2.5,
            infiltration_height_m: None,
            floors_above_grade: 1.0,
            has_flue_or_chimney: None,
            foundation_name: None,
            conditioned_foundation_merged: false,
            residential_facility_type: None,
            temperature_capacitance_multiplier: 7.0,
            hvac_deadband_c: None,
            climate_zone_iecc: None,
            unavailable_periods: Vec::new(),
            details_xml: XmlNode {
                name: String::new(),
                attrs: std::collections::HashMap::new(),
                text: String::new(),
                children: vec![],
            },
            parse_warnings: Vec::new(),
            eri_calculation_version: None,
        };

        // Conditioned zone is at index 0 → zone_id 1
        assert_eq!(
            zone_id_for_location(&building, "conditioned space"),
            Some(1)
        );
        // Garage zone is at index 1 → zone_id 2
        assert_eq!(zone_id_for_location(&building, "garage"), Some(2));
        // Attic zone is at index 2 → zone_id 3
        assert_eq!(zone_id_for_location(&building, "attic vented"), Some(3));
        // Foundation not in building → None
        assert_eq!(zone_id_for_location(&building, "unfinished basement"), None);
        // Unknown location → None
        assert_eq!(zone_id_for_location(&building, "rooftop"), None);
    }

    #[test]
    fn water_heater_in_garage_receives_correct_zone_id() {
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <Site>
                      <SiteType>suburban</SiteType>
                    </Site>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                      <ConditionedFloorArea units="ft2">1800</ConditionedFloorArea>
                      <ConditionedBuildingVolume units="ft3">14400</ConditionedBuildingVolume>
                      <NumberofConditionedFloorsAboveGrade>1</NumberofConditionedFloorsAboveGrade>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <Enclosure>
                    <Walls />
                    <Slabs>
                      <Slab>
                        <SystemIdentifier id="garage-slab"/>
                        <InteriorAdjacentTo>garage</InteriorAdjacentTo>
                        <Area units="ft2">400</Area>
                      </Slab>
                    </Slabs>
                    <Garages>
                      <Garage>
                        <SystemIdentifier id="g1"/>
                      </Garage>
                    </Garages>
                  </Enclosure>
                  <WaterHeating>
                    <HotWaterDistribution>
                      <SystemType>
                        <Standard>
                          <PipingLength units="ft">30</PipingLength>
                        </Standard>
                      </SystemType>
                    </HotWaterDistribution>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh1"/>
                      <FuelType>electricity</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <HotWaterTemperature units="F">120</HotWaterTemperature>
                      <EnergyFactor>0.92</EnergyFactor>
                      <TankVolume>50</TankVolume>
                      <PerformanceAdjustment>0.93</PerformanceAdjustment>
                      <Location>garage</Location>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let building = crate::hpxml::building::parse_building(xml).expect("building must parse");
        let mut specs = Vec::new();
        resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect("water heaters must resolve");

        let spec = specs
            .iter()
            .find(|s| s.name == "Electric Resistance Water Heater")
            .expect("WH spec must be emitted");
        let cfg: ElectricResistanceWaterHeaterConfig = spec
            .typed_config
            .as_ref()
            .expect("typed config expected")
            .typed()
            .expect("typed resistance config");

        // Garage is zone 2 (conditioned is zone 1).
        assert_eq!(
            cfg.zone_id,
            Some(2),
            "water heater in garage must have zone_id=2"
        );
        assert_eq!(cfg.zone_type.as_deref(), Some("garage"));
    }

    #[test]
    fn fixture_low_flow_is_an_xs_boolean() {
        crate::hpxml::xml_helpers::assert_reads_xs_boolean("WaterFixture/LowFlow", |v| {
            let details = parse_xml_document(&format!(
                "<BuildingDetails><BuildingSummary><BuildingConstruction>\
                 <NumberofBedrooms>3</NumberofBedrooms></BuildingConstruction></BuildingSummary>\
                 <WaterHeating><WaterFixture><WaterFixtureType>shower head</WaterFixtureType>\
                 <LowFlow>{v}</LowFlow></WaterFixture></WaterHeating></BuildingDetails>"
            ))
            .expect("xml");
            parse_avg_water_draw_and_bedrooms(&details, None).map(|(draw, _)| draw)
        });
    }

    /// A UA calculation failure fails the water heater's resolution naming
    /// the water heater: a default UA would silently re-rate the tank.
    #[test]
    fn water_heater_ua_failure_is_an_error() {
        // A gas storage WH whose UA derivation needs an efficiency and is
        // given none: the derivation's own error names the cause.
        let xml = r#"
            <HPXML schemaVersion="4.0" xmlns="http://hpxmlonline.com/2019/10">
              <Building>
                <BuildingDetails>
                  <BuildingSummary>
                    <BuildingConstruction>
                      <NumberofBedrooms>3</NumberofBedrooms>
                    </BuildingConstruction>
                  </BuildingSummary>
                  <WaterHeating>
                    <WaterHeatingSystem>
                      <SystemIdentifier id="wh_noua"/>
                      <FuelType>natural gas</FuelType>
                      <WaterHeaterType>storage water heater</WaterHeaterType>
                      <TankVolume>40</TankVolume>
                      <HeatingCapacity>36000</HeatingCapacity>
                    </WaterHeatingSystem>
                  </WaterHeating>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("xml must parse");
        let building = building_for_test(&root);
        let mut specs = Vec::new();
        let err = resolve_water_heaters(
            &building,
            building.climate_zone_iecc.as_deref(),
            &DefaultsStore::empty(),
            &mut specs,
            None,
            &mut Vec::new(),
        )
        .expect_err("a UA calculation failure fails the resolution");
        assert!(
            err.to_string().contains("wh_noua"),
            "the error names the water heater, got: {err}"
        );
    }
}
