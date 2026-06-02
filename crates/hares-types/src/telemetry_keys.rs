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
pub const TOTAL_SENSIBLE_GAIN_W: &str = "total_sensible_gain_w";
pub const LATENT_GAIN_W: &str = "latent_gain_w";
pub const SENSIBLE_COOLING_W: &str = "sensible_cooling_w";
pub const LATENT_COOLING_W: &str = "latent_cooling_w";
pub const COIL_SENSIBLE_COOLING_W: &str = "coil_sensible_cooling_w";
pub const COIL_LATENT_COOLING_W: &str = "coil_latent_cooling_w";
// OCHRE HVAC.py:595: latent_gains = latent_gain * space_fraction (pre-DSE).
// Distinct from LATENT_COOLING_W which is post-DSE.
pub const LATENT_GAINS_W: &str = "latent_gains_w";
pub const FAN_HEAT_W: &str = "fan_heat_w";
pub const IDEAL_CAPACITY_W: &str = "ideal_capacity_w";

// ── Simulation context (available at all output verbosity levels) ────────────
pub const OUTDOOR_TEMP_C: &str = "outdoor_temp_c";
pub const INDOOR_TEMP_C: &str = "indoor_temp_c";

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
/// Effective specific heat used in boiler supply temperature calculation
/// [J/(kg·K)]. Depends on the boiler's configured `fluid_type` and is
/// auditable in diagnostic CSV output.
pub const BOILER_CP_USED_J_KG_K: &str = "boiler_cp_used_j_kg_k";

// ── Setpoints ───────────────────────────────────────────────────────────────
pub const HEATING_SETPOINT_C: &str = "heating_setpoint_c";
pub const COOLING_SETPOINT_C: &str = "cooling_setpoint_c";

// ── Setpoint chain ──────────────────────────────────────────────────────────
/// Schedule-stage heating setpoint (static base + schedule override, before runtime override).
/// Always written — reflects the schedule-derived setpoint at each timestep.
pub const SCHEDULE_HEATING_SETPOINT_C: &str = "schedule_heating_setpoint_c";
/// Schedule-stage cooling setpoint (static base + schedule override, before runtime override).
/// Always written — reflects the schedule-derived setpoint at each timestep.
pub const SCHEDULE_COOLING_SETPOINT_C: &str = "schedule_cooling_setpoint_c";
/// Runtime override heating setpoint. Uses 0.0 as a sentinel for "no override active"
/// because `Telemetry::set` requires keys to be pre-registered at init.
/// Non-zero means an override is in effect; 0.0 means no override (or override to exactly 0°C,
/// which is unreachable for heating setpoints in practice).
/// See `RUNTIME_COOLING_SETPOINT_C` for the same convention on the cooling axis.
pub const RUNTIME_HEATING_SETPOINT_C: &str = "runtime_heating_setpoint_c";
/// Runtime override cooling setpoint. Uses 0.0 as a sentinel for "no override active".
/// Same convention as `RUNTIME_HEATING_SETPOINT_C`.
pub const RUNTIME_COOLING_SETPOINT_C: &str = "runtime_cooling_setpoint_c";

// ── Operating state ─────────────────────────────────────────────────────────
pub const OPERATING_MODE: &str = "operating_mode";
pub const STATE: &str = "state";
pub const RUNTIME_FRACTION: &str = "runtime_fraction";
pub const SPEED_INDEX: &str = "speed_index";

// ── HVAC speed/staging ──────────────────────────────────────────────────────
pub const SPEED_FRAC: &str = "speed_frac";
pub const PART_LOAD_RATIO: &str = "part_load_ratio";
pub const PART_LOAD_FACTOR: &str = "part_load_factor";
pub const STARTUP_MULTIPLIER: &str = "startup_multiplier";
pub const DUTY_CYCLE: &str = "duty_cycle";
pub const TIME_AT_CURRENT_SPEED_S: &str = "time_at_current_speed_s";
pub const MODE_DURATION_S: &str = "mode_duration_s";
/// 1.0 when cooling compressor is locked out because outdoor air temperature
/// is below the minimum safe operating threshold; 0.0 otherwise.
pub const COOLING_OAT_LOCKOUT: &str = "cooling_oat_lockout";
pub const DEFROST_ACTIVE: &str = "defrost_active";
pub const DEFROST_TIME_FRACTION: &str = "defrost_time_fraction";
pub const DEFROST_EXTRA_POWER_W: &str = "defrost_extra_power_w";
pub const DEFROST_Q_W: &str = "defrost_q_w";
pub const DEFROST_CAPACITY_MULTIPLIER: &str = "defrost_capacity_multiplier";
pub const DEFROST_CYCLE_STATE: &str = "defrost_cycle_state";
pub const DEFROST_ACCUMULATED_FROST_S: &str = "defrost_accumulated_frost_s";
pub const DEFROST_ELAPSED_S: &str = "defrost_elapsed_s";
pub const BYPASS_ACTIVE: &str = "bypass_active";
pub const BYPASS_FACTOR: &str = "bypass_factor";
pub const IS_ON: &str = "is_on";
pub const RAMP_LIMITED: &str = "ramp_limited";
pub const CYCLE_PHASE: &str = "cycle_phase";

// ── Control overrides ──────────────────────────────────────────────────────
pub const MAX_CAPACITY_FRACTION: &str = "max_capacity_fraction";

// ── Efficiency & performance ────────────────────────────────────────────────
pub const COP: &str = "cop";
pub const EIR: &str = "eir";
pub const SHR: &str = "shr";
pub const CAP_MULT: &str = "cap_mult";
pub const ETA_ELECTRIC: &str = "eta_electric";
pub const INVERTER_EFFICIENCY: &str = "inverter_efficiency";
pub const CAP_RATIO: &str = "cap_ratio";
pub const CAP_RATIO_RAW: &str = "cap_ratio_raw";
pub const EIR_RATIO: &str = "eir_ratio";
pub const BIQUADRATIC_CURVE_SOURCE: &str = "biquadratic_curve_source";

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
pub const DR_POWER_FRACTION: &str = "dr_power_fraction";
pub const DR_LEVEL: &str = "dr_level";
/// Pack series cell count.
pub const N_SERIES: &str = "n_series";
/// Pack parallel cell count.
pub const N_PARALLEL: &str = "n_parallel";
/// Per-cell capacity [Ah] from config (f64::NAN when not provided).
pub const AH_CELL: &str = "ah_cell";
/// Per-cell nominal voltage [V] from config (f64::NAN when not provided).
pub const V_CELL: &str = "v_cell";
/// How pack topology was determined: 0 = defaults, 1 = explicit, 2 = cell_parameters, 3 = mixed.
pub const DERIVATION_SOURCE: &str = "derivation_source";
/// Physical pack capacity implied by the derived integer topology [kWh].
/// Computed as n_parallel * ah_cell * n_series * v_cell / 1000 after topology derivation.
pub const IMPLIED_CAPACITY_KWH: &str = "implied_capacity_kwh";
/// Declared (configured) pack capacity [kWh] — the target that topology derivation
/// attempts to satisfy.
pub const DECLARED_CAPACITY_KWH: &str = "declared_capacity_kwh";

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
/// PV LUT interpolation method: 0.0 = multilinear, 1.0 = nearest-neighbor.
pub const PV_LUT_INTERP_METHOD: &str = "pv_lut_interp_method";
/// Cumulative count of nearest-neighbor fallbacks in PV LUT interpolation.
pub const PV_LUT_NN_FALLBACK_COUNT: &str = "pv_lut_nn_fallback_count";

// ── HVAC capacity reporting ────────────────────────────────────────────────
pub const HVAC_HEATING_CAPACITY_W: &str = "hvac_heating_capacity_w";
pub const HVAC_COOLING_CAPACITY_W: &str = "hvac_cooling_capacity_w";
pub const HEATING_LATENT_W: &str = "heating_latent_w";

// ── HVAC component power ───────────────────────────────────────────────────
pub const COMPRESSOR_KW: &str = "compressor_kw";
pub const COMPRESSOR_POWER_W: &str = "compressor_power_w";
pub const FAN_KW: &str = "fan_kw";
pub const FAN_ELECTRIC_W: &str = "fan_electric_w";
pub const FAN_POWER_W: &str = "fan_power_w";
// OCHRE HVAC.py:575: main_power = total_input_power_kw - fan_kw.
// For cooling: main_power = compressor_kw. For gas furnace: gas input in kW.
// For ASHP heating: compressor-only (excludes ER power per HVAC.py:1464-1467).
pub const MAIN_POWER_KW: &str = "main_power_kw";
// ASHRAE 152: duct_loss_w = gross_capacity_w * (1 - dse).
// Must use pre-DSE gross capacity, not post-DSE telemetry values.
pub const DUCT_LOSS_W: &str = "duct_loss_w";
pub const BACKUP_ER_KW: &str = "backup_er_kw";
pub const PAN_HEATER_KW: &str = "pan_heater_kw";
pub const HP_CAPACITY_W: &str = "hp_capacity_w";
pub const ER_CAPACITY_W: &str = "er_capacity_w";
pub const HP_LOCKOUT_TEMP_C: &str = "hp_lockout_temp_c";
pub const ER_LOCKOUT_TEMP_C: &str = "er_lockout_temp_c";
pub const ER_SETPOINT_OFFSET_C: &str = "er_setpoint_offset_c";
pub const ER_HARD_LOCKOUT_TIME_S: &str = "er_hard_lockout_time_s";
pub const BACKUP_CAPACITY_W: &str = "backup_capacity_w";
pub const BACKUP_EIR: &str = "backup_eir";
pub const ER_STAGES_ON: &str = "er_stages_on";
pub const MIN_COMPRESSOR_FRACTION: &str = "min_compressor_fraction";
pub const PUMP_POWER_KW: &str = "pump_power_kw";

// ── Water heater ────────────────────────────────────────────────────────────
pub const ELEMENT_KW: &str = "element_kw";
pub const PILOT_KW: &str = "pilot_kw";
pub const FUEL_INPUT_KW: &str = "fuel_input_kw";
pub const DRAW_FLOW_RATE_KG_S: &str = "draw_flow_rate_kg_s";
pub const UPPER_ELEMENT_POWER_W: &str = "upper_element_power_w";
pub const LOWER_ELEMENT_POWER_W: &str = "lower_element_power_w";
pub const BURNER_EFFICIENCY: &str = "burner_efficiency";
pub const BURNER_EFFICIENCY_SOURCE: &str = "burner_efficiency_source";
pub const BURNER_POWER_W: &str = "burner_power_w";
pub const PILOT_POWER_W: &str = "pilot_power_w";
pub const BACKUP_ELEMENT_POWER_W: &str = "backup_element_power_w";
pub const ZONE_HEAT_EXTRACTION_W: &str = "zone_heat_extraction_w";
pub const WALL_SENSIBLE_GAIN_W: &str = "wall_sensible_gain_w";
pub const UNMET_LOAD_W: &str = "unmet_load_w";
pub const PARASITIC_ELECTRIC_W: &str = "parasitic_electric_w";
pub const PILOT_HEAT_TO_WATER_W: &str = "pilot_heat_to_water_w";
pub const PILOT_HEAT_TO_AMBIENT_W: &str = "pilot_heat_to_ambient_w";

// ── Envelope energy balance ─────────────────────────────────────────────────
/// Per-zone energy balance residual [W] from the zone-air first-law check.
///
/// Published every timestep by the thermal solver's `format_domain_update`.
/// Values near zero indicate the zone air energy balance closes; large values
/// may indicate port-wiring bugs, incorrect B_d entries, or wall-mass energy
/// redistribution in multi-node models.
pub const ENERGY_BALANCE_RESIDUAL_W: &str = "energy_balance_residual_w";

// ── Ventilation / recovery ──────────────────────────────────────────────────
pub const SENSIBLE_RECOVERY_W: &str = "sensible_recovery_w";
pub const LATENT_RECOVERY_W: &str = "latent_recovery_w";
/// Supply-side fan electrical power per timestep [W].
pub const VENT_SUPPLY_FAN_POWER_W: &str = "vent_supply_fan_power_w";
/// Exhaust-side fan electrical power per timestep [W].
pub const VENT_EXHAUST_FAN_POWER_W: &str = "vent_exhaust_fan_power_w";

// ── Dehumidifier ────────────────────────────────────────────────────────────
pub const WATER_REMOVAL_L_DAY: &str = "water_removal_l_day";
pub const LATENT_REMOVAL_W: &str = "latent_removal_w";
pub const TARGET_RH: &str = "target_rh";
pub const MIN_RH: &str = "min_rh";
pub const MAX_RH: &str = "max_rh";
pub const MOISTURE_MASS_FLOW_KG_S: &str = "moisture_mass_flow_kg_s";
pub const HUMIDITY_SEMI_IMPLICIT_ALPHA: &str = "humidity_semi_implicit_alpha";

/// Synthetic `equipment_telemetry` key for solver-level per-zone humidity data.
///
/// The humidity solver is not an `Equipment` instance and has no entry in
/// `equipment_id_by_name`, so the standard `retain` that keeps only
/// registered equipment names would evict this entry. Code that filters
/// `equipment_telemetry` by equipment name must exempt this key.
pub const HUMIDITY_SOLVER_TELEMETRY_KEY: &str = "HumiditySolver";

// ── Loads ───────────────────────────────────────────────────────────────────

// ── Dwelling-level / test equipment ─────────────────────────────────────────
pub const LAST_POWER_KW: &str = "last_power_kw";
pub const LAST_SOC_TARGET: &str = "last_soc_target";

// ── Protocol bridge ─────────────────────────────────────────────────────────
/// Protocol ID of the last dispatched ProtocolNative signal.
pub const PROTOCOL_ID: &str = "protocol_id";
/// Payload size in bytes of the last dispatched ProtocolNative signal.
pub const PAYLOAD_SIZE_BYTES: &str = "payload_size_bytes";
/// Cumulative count of ProtocolNative dispatch events.
pub const DISPATCH_COUNT: &str = "dispatch_count";
/// Number of equipment commands parsed from the most recent ProtocolNative
/// payload. Zero when the payload produced no commands or when parsing failed.
pub const PARSED_COMMAND_COUNT: &str = "parsed_command_count";
/// Cumulative count of handler parse failures.
pub const PARSE_ERROR_COUNT: &str = "parse_error_count";
/// Cumulative count of equipment commands parsed across all payloads
/// since the bridge was initialised or last reset.
pub const TOTAL_COMMANDS_PARSED: &str = "total_commands_parsed";

// ── Generator ───────────────────────────────────────────────────────────────
/// Sum of `thermal_power_w` values written to fluid port contributions.
/// Tracks the actual energy declared to the loop, enabling cross-validation
/// against `THERMAL_OUTPUT_W`. In debug / check_invariants builds an assertion
/// guards that these two values match within tolerance.
pub const THERMAL_POWER_DELIVERED_W: &str = "thermal_power_delivered_w";
/// Heat recovery ratio: fraction of available thermal power actually delivered to the loop.
/// 1.0 = no capping; < 1.0 = loop saturated and thermal output is capped.
/// EnergyPlus ICEngineElectricGenerator.cc:782 — HRecRatio.
pub const HEAT_REC_RATIO: &str = "heat_rec_ratio";
/// Unscaled thermal power available before heat recovery capping, in watts.
/// EnergyPlus ICEngineElectricGenerator.cc:757 — q_thermal before HRecRatio.
pub const THERMAL_AVAILABLE_W: &str = "thermal_available_w";
/// Fluid loop return temperature, in °C, read at time of heat recovery computation.
/// EnergyPlus ICEngineElectricGenerator.cc:751 — HeatRecInTemp.
pub const LOOP_RETURN_TEMP_C: &str = "loop_return_temp_c";
/// Jacket water heat recovery power in watts.
/// EnergyPlus ERM 26.1 §Generators §Internal Combustion Engine:
/// jacket water heat recovery is a manufacturer-supplied PLR-dependent curve.
pub const JACKET_WATER_W: &str = "jacket_water_w";
/// Lube oil heat recovery power in watts.
/// EnergyPlus ERM 26.1 §Generators §Internal Combustion Engine:
/// lube oil heat recovery is a manufacturer-supplied PLR-dependent curve.
pub const LUBE_OIL_W: &str = "lube_oil_w";
/// Exhaust heat recovery power in watts.
/// EnergyPlus ERM 26.1 §Generators §Internal Combustion Engine:
/// exhaust heat recovery is modelled via PLR-dependent curves and
/// NTU-effectiveness HX; no standard fixed value exists.
pub const EXHAUST_WATER_W: &str = "exhaust_water_w";
/// Jacket water supply temperature in °C. ~90°C for typical IC engines.
pub const SUPPLY_TEMP_JACKET_C: &str = "supply_temp_jacket_c";
/// Exhaust heat exchanger supply temperature in °C. ~400–500°C raw exhaust.
pub const SUPPLY_TEMP_EXHAUST_C: &str = "supply_temp_exhaust_c";
// (ELECTRIC_OUTPUT_KW is in the electrical power section above)

// ── Fuel cell ───────────────────────────────────────────────────────────────
/// DC stack electrical output (kW) before inverter losses.
/// EnergyPlus FuelCellElectricGenerator.cc:1691-1711 — DC power efficiency model.
pub const FUEL_CELL_DC_KW: &str = "fuel_cell_dc_kw";
/// Power lost in DC-to-AC inverter conversion (W).
/// EnergyPlus FuelCellElectricGenerator.cc:2104-2124 — inverter model.
pub const FUEL_CELL_INVERTER_LOSS_W: &str = "fuel_cell_inverter_loss_w";
/// Stack cooling heat (W) removed by the stack cooler.
/// EnergyPlus FuelCellElectricGenerator.cc:1859-1865 — stack cooler polynomial.
pub const FUEL_CELL_STACK_HEAT_W: &str = "fuel_cell_stack_heat_w";

/// Generate a tank node temperature key for the given node index.
///
/// Returns a string like `"tank_node_0_c"`, `"tank_node_1_c"`, etc.
pub fn tank_node_key(index: usize) -> String {
    format!("tank_node_{index}_c")
}
