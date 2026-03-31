//! Shared telemetry key constants.
//!
//! Every telemetry key written or read anywhere in the crate tree should use one
//! of these constants instead of a bare string literal. This eliminates typo-class
//! wiring bugs and makes find-all-references trivial.

// ── Electrical power ────────────────────────────────────────────────────────
pub const ELECTRIC_KW: &str = "electric_kw";
pub const ACTIVE_POWER_KW: &str = "active_power_kw";
pub const AC_POWER_KW: &str = "ac_power_kw";
pub const ELECTRIC_OUTPUT_KW: &str = "electric_output_kw";
pub const ELECTRIC_POWER_W: &str = "electric_power_w";
pub const REACTIVE_POWER_KVAR: &str = "reactive_power_kvar";

// ── Fuel & thermal ──────────────────────────────────────────────────────────
pub const FUEL_INPUT_W: &str = "fuel_input_w";
pub const THERMAL_OUTPUT_W: &str = "thermal_output_w";
pub const FLUE_LOSS_W: &str = "flue_loss_w";
pub const JACKET_LOSS_W: &str = "jacket_loss_w";
pub const SKIN_LOSS_W: &str = "skin_loss_w";
pub const SENSIBLE_GAIN_W: &str = "sensible_gain_w";
pub const LATENT_GAIN_W: &str = "latent_gain_w";
pub const SENSIBLE_COOLING_W: &str = "sensible_cooling_w";
pub const LATENT_COOLING_W: &str = "latent_cooling_w";
pub const IDEAL_CAPACITY_W: &str = "ideal_capacity_w";

// ── Temperature ─────────────────────────────────────────────────────────────
pub const CELL_TEMP_C: &str = "cell_temp_c";
pub const BATTERY_TEMP_C: &str = "battery_temp_c";
pub const SUPPLY_TEMP_C: &str = "supply_temp_c";
pub const SUPPLY_AIR_TEMP_C: &str = "supply_air_temp_c";
pub const RETURN_TEMP_C: &str = "return_temp_c";
pub const TANK_AVG_TEMP_C: &str = "tank_avg_temp_c";
pub const OUTLET_TEMP_C: &str = "outlet_temp_c";
pub const APPARATUS_DEW_POINT_C: &str = "apparatus_dew_point_c";
pub const CURRENT_TARGET_C: &str = "current_target_c";

// ── Setpoints ───────────────────────────────────────────────────────────────
pub const HEATING_SETPOINT_C: &str = "heating_setpoint_c";
pub const COOLING_SETPOINT_C: &str = "cooling_setpoint_c";

// ── Operating state ─────────────────────────────────────────────────────────
pub const OPERATING_MODE: &str = "operating_mode";
pub const STATE: &str = "state";
pub const RUNTIME_FRACTION: &str = "runtime_fraction";
pub const SPEED_INDEX: &str = "speed_index";
pub const DEFROST_ACTIVE: &str = "defrost_active";
pub const DEFROST_TIME_FRACTION: &str = "defrost_time_fraction";
pub const BYPASS_ACTIVE: &str = "bypass_active";
pub const BYPASS_FACTOR: &str = "bypass_factor";
pub const IS_ON: &str = "is_on";
pub const RAMP_LIMITED: &str = "ramp_limited";
pub const CYCLE_PHASE: &str = "cycle_phase";

// ── Efficiency & performance ────────────────────────────────────────────────
pub const COP: &str = "cop";
pub const EIR: &str = "eir";
pub const SHR: &str = "shr";
pub const CAP_MULT: &str = "cap_mult";
pub const ETA_ELECTRIC: &str = "eta_electric";
pub const INVERTER_EFFICIENCY: &str = "inverter_efficiency";

// ── Battery / EV / storage ──────────────────────────────────────────────────
pub const SOC: &str = "soc";
pub const OHMIC_LOSS_W: &str = "ohmic_loss_w";
pub const STANDBY_POWER_W: &str = "standby_power_w";
pub const HEATER_POWER_W: &str = "heater_power_w";
pub const DISCHARGE_DERATE: &str = "discharge_derate";
pub const CAPACITY_DERATE: &str = "capacity_derate";
pub const CHARGE_DERATE: &str = "charge_derate";
pub const CYCLE_COUNT: &str = "cycle_count";
pub const CAPACITY_FADE_PCT: &str = "capacity_fade_pct";
pub const TERMINAL_VOLTAGE_V: &str = "terminal_voltage_v";
pub const CURRENT_A: &str = "current_a";

// ── EV-specific ─────────────────────────────────────────────────────────────
pub const CONNECTION_STATE: &str = "connection_state";
pub const CHARGING_LEVEL: &str = "charging_level";
pub const V2L_ACTIVE: &str = "v2l_active";
pub const V2L_POWER_KW: &str = "v2l_power_kw";
pub const AWAY_CHARGE_POWER_KW: &str = "away_charge_power_kw";
pub const CAPACITY_KWH: &str = "capacity_kwh";
pub const FUEL_ECONOMY_KWH_PER_MI: &str = "fuel_economy_kwh_per_mi";

// ── PV / solar ──────────────────────────────────────────────────────────────
pub const DC_POWER_KW: &str = "dc_power_kw";
pub const IRRADIANCE_W_M2: &str = "irradiance_w_m2";
pub const CURTAILMENT_KW: &str = "curtailment_kw";
pub const INVERTER_CLIPPING_KW: &str = "inverter_clipping_kw";
pub const SOILING_RATIO: &str = "soiling_ratio";
pub const SHADING_FACTOR: &str = "shading_factor";

// ── HVAC component power ───────────────────────────────────────────────────
pub const COMPRESSOR_KW: &str = "compressor_kw";
pub const COMPRESSOR_POWER_W: &str = "compressor_power_w";
pub const FAN_KW: &str = "fan_kw";
pub const FAN_ELECTRIC_W: &str = "fan_electric_w";
pub const FAN_POWER_W: &str = "fan_power_w";

// ── Water heater ────────────────────────────────────────────────────────────
pub const DRAW_FLOW_RATE_KG_S: &str = "draw_flow_rate_kg_s";
pub const UPPER_ELEMENT_POWER_W: &str = "upper_element_power_w";
pub const LOWER_ELEMENT_POWER_W: &str = "lower_element_power_w";
pub const BURNER_POWER_W: &str = "burner_power_w";
pub const PILOT_POWER_W: &str = "pilot_power_w";
pub const BACKUP_ELEMENT_POWER_W: &str = "backup_element_power_w";
pub const ZONE_HEAT_EXTRACTION_W: &str = "zone_heat_extraction_w";
pub const WALL_SENSIBLE_GAIN_W: &str = "wall_sensible_gain_w";
pub const UNMET_LOAD_W: &str = "unmet_load_w";
pub const PARASITIC_ELECTRIC_W: &str = "parasitic_electric_w";

// ── Ventilation / recovery ──────────────────────────────────────────────────
pub const SENSIBLE_RECOVERY_W: &str = "sensible_recovery_w";
pub const LATENT_RECOVERY_W: &str = "latent_recovery_w";

// ── Dehumidifier ────────────────────────────────────────────────────────────
pub const WATER_REMOVAL_L_DAY: &str = "water_removal_l_day";
pub const LATENT_REMOVAL_W: &str = "latent_removal_w";
pub const TARGET_RH: &str = "target_rh";
pub const MIN_RH: &str = "min_rh";
pub const MAX_RH: &str = "max_rh";

// ── Loads ───────────────────────────────────────────────────────────────────

// ── Dwelling-level / test equipment ─────────────────────────────────────────
pub const LAST_POWER_KW: &str = "last_power_kw";

// ── Generator ───────────────────────────────────────────────────────────────
// (ELECTRIC_OUTPUT_KW is in the electrical power section above)

/// Generate a tank node temperature key for the given node index.
///
/// Returns a string like `"tank_node_0_c"`, `"tank_node_1_c"`, etc.
pub fn tank_node_key(index: usize) -> String {
    format!("tank_node_{index}_c")
}
