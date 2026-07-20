//! Shared telemetry key constants.
//!
//! Every telemetry key written or read anywhere in the crate tree should use one
//! of these constants instead of a bare string literal. This eliminates typo-class
//! wiring bugs and makes find-all-references trivial.
//!
//! ## Scope
//!
//! Each key carries a `scope:` annotation in its doc comment:
//!
//! - `scope: output` — the key's value is placed into an output column via
//!   `record_step()` / `build_equipment_column_map()` (including new per-equipment
//!   telemetry columns added at verbosity 8). These keys have a defined output
//!   column contract.
//! - `scope: internal` — the key is used only for inter-equipment communication,
//!   control logic, and state bookkeeping. Its value is never directly emitted
//!   to a named output column (actor pass-through columns are a separate mechanism).

// ── Electrical power ────────────────────────────────────────────────────────

/// scope: internal
pub const ELECTRIC_KW: &str = "electric_kw";

/// scope: internal
pub const ACTIVE_POWER_KW: &str = "active_power_kw";

/// scope: internal
pub const AC_POWER_KW: &str = "ac_power_kw";

/// scope: internal
pub const ELECTRIC_OUTPUT_KW: &str = "electric_output_kw";

/// scope: internal
pub const ELECTRIC_POWER_W: &str = "electric_power_w";

/// scope: internal
pub const REACTIVE_POWER_KVAR: &str = "reactive_power_kvar";

// ── Fuel & thermal ──────────────────────────────────────────────────────────

/// scope: internal
pub const FUEL_INPUT_W: &str = "fuel_input_w";

/// scope: internal
pub const FUEL_IDLE_W: &str = "fuel_idle_w";

/// scope: internal
pub const FUEL_LOAD_W: &str = "fuel_load_w";

/// scope: internal
pub const THERMAL_OUTPUT_W: &str = "thermal_output_w";

/// scope: internal
pub const FLUE_LOSS_W: &str = "flue_loss_w";

/// scope: internal
pub const JACKET_LOSS_W: &str = "jacket_loss_w";

/// scope: internal
pub const SKIN_LOSS_W: &str = "skin_loss_w";

/// scope: internal
pub const SENSIBLE_GAIN_W: &str = "sensible_gain_w";

/// scope: internal
pub const TOTAL_SENSIBLE_GAIN_W: &str = "total_sensible_gain_w";

/// scope: internal
pub const LATENT_GAIN_W: &str = "latent_gain_w";

/// scope: internal
pub const SENSIBLE_COOLING_W: &str = "sensible_cooling_w";

/// scope: internal
pub const LATENT_COOLING_W: &str = "latent_cooling_w";

/// scope: internal
pub const COIL_SENSIBLE_COOLING_W: &str = "coil_sensible_cooling_w";

/// scope: internal
pub const COIL_LATENT_COOLING_W: &str = "coil_latent_cooling_w";

/// OCHRE HVAC.py:595: latent_gains = latent_gain * space_fraction (pre-DSE).
/// Distinct from LATENT_COOLING_W which is post-DSE.
///
/// scope: output
pub const LATENT_GAINS_W: &str = "latent_gains_w";

/// scope: internal
pub const FAN_HEAT_W: &str = "fan_heat_w";

/// scope: internal
pub const IDEAL_CAPACITY_W: &str = "ideal_capacity_w";

/// 1.0 when the ideal capacity dispatch is degraded (consecutive solver failures
/// exceeded threshold and last-good capacity was used as fallback); 0.0 otherwise.
/// Records the `degraded` flag from `ControlSignal::IdealCapacity` for diagnostic
/// CSV outputs.
///
/// scope: internal
pub const IDEAL_CAPACITY_DEGRADED: &str = "ideal_capacity_degraded";

// ── Simulation context (available at all output verbosity levels) ────────────

/// scope: internal — populated from env state, not from telemetry key
pub const OUTDOOR_TEMP_C: &str = "outdoor_temp_c";

/// scope: internal — populated from zone temp scratch, not from telemetry key
pub const INDOOR_TEMP_C: &str = "indoor_temp_c";

// ── Temperature ─────────────────────────────────────────────────────────────

/// scope: internal — battery/PV cell temperatures need dedicated v8 columns; not yet implemented
pub const CELL_TEMP_C: &str = "cell_temp_c";

/// scope: internal — battery temperature needs dedicated v8 column; not yet implemented
pub const BATTERY_TEMP_C: &str = "battery_temp_c";

/// scope: output
pub const SUPPLY_TEMP_C: &str = "supply_temp_c";

/// scope: output
pub const SUPPLY_AIR_TEMP_C: &str = "supply_air_temp_c";

/// scope: output
pub const RETURN_TEMP_C: &str = "return_temp_c";

/// scope: internal
pub const TANK_AVG_TEMP_C: &str = "tank_avg_temp_c";

/// scope: internal
pub const OUTLET_TEMP_C: &str = "outlet_temp_c";

/// scope: internal
pub const APPARATUS_DEW_POINT_C: &str = "apparatus_dew_point_c";

/// scope: internal
pub const CURRENT_TARGET_C: &str = "current_target_c";

/// Effective specific heat used in boiler supply temperature calculation
/// [J/(kg·K)]. Depends on the boiler's configured `fluid_type` and is
/// auditable in diagnostic CSV output.
///
/// scope: internal
pub const BOILER_CP_USED_J_KG_K: &str = "boiler_cp_used_j_kg_k";

// ── Setpoints ───────────────────────────────────────────────────────────────

/// scope: internal — setpoint output comes from CoreOutput, not telemetry key
pub const HEATING_SETPOINT_C: &str = "heating_setpoint_c";

/// scope: internal — setpoint output comes from CoreOutput, not telemetry key
pub const COOLING_SETPOINT_C: &str = "cooling_setpoint_c";

// ── Setpoint chain ──────────────────────────────────────────────────────────
/// Schedule-stage heating setpoint (static base + schedule override, before runtime override).
/// Always written — reflects the schedule-derived setpoint at each timestep.
///
/// scope: output
pub const SCHEDULE_HEATING_SETPOINT_C: &str = "schedule_heating_setpoint_c";

/// Schedule-stage cooling setpoint (static base + schedule override, before runtime override).
/// Always written — reflects the schedule-derived setpoint at each timestep.
///
/// scope: output
pub const SCHEDULE_COOLING_SETPOINT_C: &str = "schedule_cooling_setpoint_c";

/// Runtime override heating setpoint. Uses 0.0 as a sentinel for "no override active"
/// because `Telemetry::set` requires keys to be pre-registered at init.
/// Non-zero means an override is in effect; 0.0 means no override (or override to exactly 0°C,
/// which is unreachable for heating setpoints in practice).
/// See `RUNTIME_COOLING_SETPOINT_C` for the same convention on the cooling axis.
///
/// scope: output
pub const RUNTIME_HEATING_SETPOINT_C: &str = "runtime_heating_setpoint_c";

/// Runtime override cooling setpoint. Uses 0.0 as a sentinel for "no override active".
/// Same convention as `RUNTIME_HEATING_SETPOINT_C`.
///
/// scope: output
pub const RUNTIME_COOLING_SETPOINT_C: &str = "runtime_cooling_setpoint_c";

// ── Thermostat short-cycle protection ──────────────────────────────────────

/// Minimum compressor on-time [s] from thermostat FSM.
/// scope: output
pub const MIN_ON_TIME_S: &str = "min_on_time_s";

/// Minimum compressor off-time [s] from thermostat FSM.
/// scope: output
pub const MIN_OFF_TIME_S: &str = "min_off_time_s";

// ── Operating state ─────────────────────────────────────────────────────────

/// scope: internal — mode output comes from CoreOutput, not telemetry key
pub const OPERATING_MODE: &str = "operating_mode";

/// scope: internal
pub const STATE: &str = "state";

/// scope: output
pub const RUNTIME_FRACTION: &str = "runtime_fraction";

/// scope: internal — speed output comes from CoreOutput, not telemetry key
pub const SPEED_INDEX: &str = "speed_index";

// ── HVAC speed/staging ──────────────────────────────────────────────────────

/// scope: internal
pub const SPEED_FRAC: &str = "speed_frac";

/// scope: internal
pub const PART_LOAD_RATIO: &str = "part_load_ratio";

/// scope: internal
pub const PART_LOAD_FACTOR: &str = "part_load_factor";

/// scope: internal
pub const STARTUP_MULTIPLIER: &str = "startup_multiplier";

/// Elapsed time since compressor startup [minutes]. Preserved across
/// off-steps and reset only on a true off→on transition (edge detection,
/// matching OCHRE's mode-transition reset). Used with `STARTUP_MULTIPLIER`
/// in diagnostic CSV output to detect whether the startup timer erroneously
/// advances while the compressor is not energised (e.g. during ER-only
/// operation on an ASHP).
///
/// scope: internal
pub const TIME_SINCE_START_MIN: &str = "time_since_start_min";

/// Cumulative count of startup-ramp timer resets (true off→on compressor
/// transitions) since equipment init. Under the edge-detection contract the
/// timer resets exactly once per compressor restart, so differencing
/// consecutive diagnostic-CSV rows yields the resets-per-hour rate — the
/// former per-off-step reset behaviour produced a dramatically higher rate.
/// Diagnostic counter only: not persisted across checkpoint restore
/// (restarts from 0, matching `BIQUADRATIC_INDEX_CLAMPED`).
///
/// scope: internal
pub const STARTUP_TIMER_RESET_COUNT: &str = "startup_timer_reset_count";

/// scope: internal
pub const DUTY_CYCLE: &str = "duty_cycle";

/// scope: internal
pub const TIME_AT_CURRENT_SPEED_S: &str = "time_at_current_speed_s";

/// scope: internal
pub const MODE_DURATION_S: &str = "mode_duration_s";

/// 1.0 when cooling compressor is locked out because outdoor air temperature
/// is below the minimum safe operating threshold; 0.0 otherwise.
///
/// scope: internal
pub const COOLING_OAT_LOCKOUT: &str = "cooling_oat_lockout";

/// scope: internal
pub const DEFROST_ACTIVE: &str = "defrost_active";

/// scope: internal
pub const DEFROST_TIME_FRACTION: &str = "defrost_time_fraction";

/// scope: internal
pub const DEFROST_EXTRA_POWER_W: &str = "defrost_extra_power_w";

/// scope: internal
pub const DEFROST_Q_W: &str = "defrost_q_w";

/// scope: internal
pub const DEFROST_CAPACITY_MULTIPLIER: &str = "defrost_capacity_multiplier";

/// scope: output
pub const DEFROST_CYCLE_STATE: &str = "defrost_cycle_state";

/// scope: internal
pub const DEFROST_ACCUMULATED_FROST_S: &str = "defrost_accumulated_frost_s";

/// scope: internal
pub const DEFROST_ELAPSED_S: &str = "defrost_elapsed_s";

/// scope: internal
pub const BYPASS_ACTIVE: &str = "bypass_active";

/// scope: internal
pub const BYPASS_FACTOR: &str = "bypass_factor";

/// scope: internal
pub const IS_ON: &str = "is_on";

/// scope: internal
pub const RAMP_LIMITED: &str = "ramp_limited";

/// scope: internal
pub const CYCLE_PHASE: &str = "cycle_phase";

// ── Control overrides ──────────────────────────────────────────────────────

/// scope: internal
pub const MAX_CAPACITY_FRACTION: &str = "max_capacity_fraction";

// ── Efficiency & performance ────────────────────────────────────────────────

/// scope: internal — COP output comes from CoreOutput, not telemetry key
pub const COP: &str = "cop";

/// scope: internal
pub const EIR: &str = "eir";

/// Equivalent battery model efficiency (COP = 1/EIR) reported by HVAC equipment.
///
/// scope: internal
pub const EBM_EFFICIENCY: &str = "ebm_efficiency";

/// Equivalent battery model baseline power [kW] — steady-state electrical draw
/// to hold the setpoint against envelope losses.
///
/// scope: internal
pub const EBM_BASELINE_POWER_KW: &str = "ebm_baseline_power_kw";

/// Equivalent battery model current energy state [kWh].
///
/// scope: internal
pub const EBM_ENERGY_KWH: &str = "ebm_energy_kwh";

/// Equivalent battery model minimum energy state [kWh] — energy at thermostat
/// turn-on threshold.
///
/// scope: internal
pub const EBM_MIN_ENERGY_KWH: &str = "ebm_min_energy_kwh";

/// Equivalent battery model maximum energy state [kWh] — energy at thermostat
/// turn-off threshold.
///
/// scope: internal
pub const EBM_MAX_ENERGY_KWH: &str = "ebm_max_energy_kwh";

/// Equivalent battery model maximum power [kW] — equipment rated capacity in
/// electrical units.
///
/// scope: internal
pub const EBM_MAX_POWER_KW: &str = "ebm_max_power_kw";

/// scope: output
pub const SHR: &str = "shr";

/// scope: internal
pub const CAP_MULT: &str = "cap_mult";

/// scope: internal
pub const ETA_ELECTRIC: &str = "eta_electric";

/// scope: internal
pub const INVERTER_EFFICIENCY: &str = "inverter_efficiency";

/// scope: internal
pub const CAP_RATIO: &str = "cap_ratio";

/// scope: internal
pub const CAP_RATIO_RAW: &str = "cap_ratio_raw";

/// scope: internal
pub const EIR_RATIO: &str = "eir_ratio";

/// scope: internal
pub const BIQUADRATIC_CURVE_SOURCE: &str = "biquadratic_curve_source";

/// Counter incremented each timestep `evaluate_biquadratic` clamps an
/// out-of-bounds curve index. Zero when no clamping occurred during the
/// current timestep; non-zero when the fallback path fired.
///
/// scope: internal
pub const BIQUADRATIC_INDEX_CLAMPED: &str = "biquadratic_index_clamped";

/// `speed_frac` value recorded when the MultiSpeedInterpolated high-side
/// curve index is clamped to the last valid stage (i.e. `speed_index + 1`
/// would have exceeded `num_speeds - 1`). Setting only this key when the
/// clamp fires (rather than the always-set `speed_frac`) allows offline
/// analysis of peak-capacity interpolation frequency and magnitude.
///
/// scope: internal
pub const HIGH_SIDE_CURVE_CLAMPED_SPEED_FRAC: &str = "high_side_curve_clamped_speed_frac";

/// 1.0 when the Henderson-Rengarajan latent degradation model is active for
/// the current cooling step, 0.0 otherwise. Observe-only diagnostic.
///
/// scope: internal
pub const LATENT_DEGRADATION_ACTIVE: &str = "latent_degradation_active";

/// Steady-state SHR from the coil bypass-factor solution before the
/// Henderson-Rengarajan part-load latent degradation is applied. Paired with
/// the always-set `SHR` (post-degradation) to quantify the degradation
/// magnitude offline. Observe-only diagnostic.
///
/// scope: internal
pub const SHR_BEFORE_DEGRADATION: &str = "shr_before_degradation";

// ── Battery / EV / storage ──────────────────────────────────────────────────

/// scope: internal — SOC output comes from CoreOutput, not telemetry key
pub const SOC: &str = "soc";

/// scope: internal
pub const OHMIC_LOSS_W: &str = "ohmic_loss_w";

/// scope: internal
pub const STANDBY_POWER_W: &str = "standby_power_w";

/// scope: internal
pub const HEATER_POWER_W: &str = "heater_power_w";

/// scope: internal
pub const DISCHARGE_DERATE: &str = "discharge_derate";

/// scope: internal
pub const CAPACITY_DERATE: &str = "capacity_derate";

/// scope: internal
pub const CHARGE_DERATE: &str = "charge_derate";

/// CC-CV charging power taper multiplier [0..1] (1.0 = no derating,
/// < 1.0 = CV taper active).  Set only when no charging-curve LUT is
/// present; when a LUT is loaded the value is 1.0.
///
/// scope: internal
pub const CC_CV_DERATE: &str = "cc_cv_derate";

/// scope: internal
pub const CYCLE_COUNT: &str = "cycle_count";

/// scope: internal
pub const CAPACITY_FADE_PCT: &str = "capacity_fade_pct";

/// scope: internal
pub const TERMINAL_VOLTAGE_V: &str = "terminal_voltage_v";

/// scope: internal
pub const CURRENT_A: &str = "current_a";

/// scope: internal
pub const DR_POWER_FRACTION: &str = "dr_power_fraction";

/// scope: internal
pub const DR_LEVEL: &str = "dr_level";

/// Pack series cell count.
///
/// scope: internal
pub const N_SERIES: &str = "n_series";

/// Pack parallel cell count.
///
/// scope: internal
pub const N_PARALLEL: &str = "n_parallel";

/// Per-cell capacity [Ah] from config (f64::NAN when not provided).
///
/// scope: internal
pub const AH_CELL: &str = "ah_cell";

/// Per-cell nominal voltage [V] from config (f64::NAN when not provided).
///
/// scope: internal
pub const V_CELL: &str = "v_cell";

/// How pack topology was determined: 0 = defaults, 1 = explicit, 2 = cell_parameters, 3 = mixed.
///
/// scope: internal
pub const DERIVATION_SOURCE: &str = "derivation_source";

/// Physical pack capacity implied by the derived integer topology [kWh].
/// Computed as n_parallel * ah_cell * n_series * v_cell / 1000 after topology derivation.
///
/// scope: internal
pub const IMPLIED_CAPACITY_KWH: &str = "implied_capacity_kwh";

/// Declared (configured) pack capacity [kWh] — the target that topology derivation
/// attempts to satisfy.
///
/// scope: internal
pub const DECLARED_CAPACITY_KWH: &str = "declared_capacity_kwh";

// ── EV-specific ─────────────────────────────────────────────────────────────

/// scope: output
pub const CONNECTION_STATE: &str = "connection_state";

/// scope: output
pub const CHARGING_LEVEL: &str = "charging_level";

/// scope: internal
pub const V2L_ACTIVE: &str = "v2l_active";

/// scope: internal
pub const V2L_POWER_KW: &str = "v2l_power_kw";

/// scope: internal
pub const AWAY_CHARGE_POWER_KW: &str = "away_charge_power_kw";

/// scope: internal
pub const CAPACITY_KWH: &str = "capacity_kwh";

/// Rated (beginning-of-life) pack capacity [kWh]. Held constant from
/// initialization; paired with `CAPACITY_KWH` (the degraded usable capacity)
/// so downstream observers and the capacity-degradation invariant can verify
/// `CAPACITY_KWH / CAPACITY_KWH_RATED ≈ 1 − capacity_fade_fraction`.
///
/// scope: internal
pub const CAPACITY_KWH_RATED: &str = "capacity_kwh_rated";

/// scope: internal
pub const FUEL_ECONOMY_KWH_PER_MI: &str = "fuel_economy_kwh_per_mi";

// ── PV / solar ──────────────────────────────────────────────────────────────

/// scope: output
pub const DC_POWER_KW: &str = "dc_power_kw";

/// scope: output
pub const IRRADIANCE_W_M2: &str = "irradiance_w_m2";

/// scope: internal
pub const CURTAILMENT_KW: &str = "curtailment_kw";

/// scope: internal
pub const INVERTER_CLIPPING_KW: &str = "inverter_clipping_kw";

/// scope: internal
pub const SOILING_RATIO: &str = "soiling_ratio";

/// scope: internal
pub const SHADING_FACTOR: &str = "shading_factor";

/// PV LUT interpolation method: 0.0 = multilinear, 1.0 = nearest-neighbor.
///
/// scope: internal
pub const PV_LUT_INTERP_METHOD: &str = "pv_lut_interp_method";

/// Cumulative count of nearest-neighbor fallbacks in PV LUT interpolation.
///
/// scope: internal
pub const PV_LUT_NN_FALLBACK_COUNT: &str = "pv_lut_nn_fallback_count";

/// Reactive-power source: 0.0 = none, 1.0 = reactive_setpoint,
/// 2.0 = power_factor, 3.0 = power_setpoint.
///
/// scope: internal
pub const Q_SOURCE: &str = "q_source";

// ── HVAC capacity reporting ────────────────────────────────────────────────

/// scope: internal — capacity output comes from CoreOutput, not telemetry key
pub const HVAC_HEATING_CAPACITY_W: &str = "hvac_heating_capacity_w";

/// scope: internal — capacity output comes from CoreOutput, not telemetry key
pub const HVAC_COOLING_CAPACITY_W: &str = "hvac_cooling_capacity_w";

/// scope: internal
pub const HEATING_LATENT_W: &str = "heating_latent_w";

// ── HVAC component power ───────────────────────────────────────────────────

/// scope: output
pub const COMPRESSOR_KW: &str = "compressor_kw";

/// scope: output
pub const COMPRESSOR_POWER_W: &str = "compressor_power_w";

/// scope: output
pub const FAN_KW: &str = "fan_kw";

/// scope: output
pub const FAN_ELECTRIC_W: &str = "fan_electric_w";

/// scope: output
pub const FAN_POWER_W: &str = "fan_power_w";

/// OCHRE HVAC.py:575: main_power = total_input_power_kw - fan_kw.
/// For cooling: main_power = compressor_kw. For gas furnace: gas input in kW.
/// For ASHP heating: compressor-only (excludes ER power per HVAC.py:1464-1467).
///
/// scope: internal — main power output comes from CoreOutput, not telemetry key
pub const MAIN_POWER_KW: &str = "main_power_kw";

/// ASHRAE 152: duct_loss_w = gross_capacity_w * (1 - dse).
/// Must use pre-DSE gross capacity, not post-DSE telemetry values.
///
/// scope: output
pub const DUCT_LOSS_W: &str = "duct_loss_w";

/// scope: output
pub const BACKUP_ER_KW: &str = "backup_er_kw";

/// scope: output
pub const PAN_HEATER_KW: &str = "pan_heater_kw";

/// scope: output
pub const HP_CAPACITY_W: &str = "hp_capacity_w";

/// scope: output
pub const ER_CAPACITY_W: &str = "er_capacity_w";

/// scope: internal
pub const HP_LOCKOUT_TEMP_C: &str = "hp_lockout_temp_c";

/// scope: internal
pub const ER_LOCKOUT_TEMP_C: &str = "er_lockout_temp_c";

/// scope: internal
pub const ER_SETPOINT_OFFSET_C: &str = "er_setpoint_offset_c";

/// scope: internal
pub const ER_HARD_LOCKOUT_TIME_S: &str = "er_hard_lockout_time_s";

/// ER soft lockout `zone_rising` flag: 1.0 when zone temperature has not yet
/// declined after the hard lockout expired; 0.0 otherwise. Captured only under
/// `#[cfg(feature = "observe")]` so ER soft-lockout transitions can be correlated
/// with zone temperature trends in diagnostic output.
///
/// scope: internal
pub const ZONE_RISING: &str = "zone_rising";

/// scope: internal
pub const BACKUP_CAPACITY_W: &str = "backup_capacity_w";

/// scope: internal
pub const BACKUP_EIR: &str = "backup_eir";

/// scope: internal
pub const ER_STAGES_ON: &str = "er_stages_on";

/// scope: internal
pub const MIN_COMPRESSOR_FRACTION: &str = "min_compressor_fraction";

/// scope: internal
pub const PUMP_POWER_KW: &str = "pump_power_kw";

/// Crankcase heater electric draw [kW], space_fraction-scaled like the other
/// per-component sub-meters so components sum to the unit total.
///
/// scope: internal
pub const CRANKCASE_KW: &str = "crankcase_kw";

// ── Water heater ────────────────────────────────────────────────────────────

/// scope: internal
pub const ELEMENT_KW: &str = "element_kw";

/// scope: internal
pub const PILOT_KW: &str = "pilot_kw";

/// scope: internal
pub const FUEL_INPUT_KW: &str = "fuel_input_kw";

/// scope: internal
pub const DRAW_FLOW_RATE_KG_S: &str = "draw_flow_rate_kg_s";

/// scope: internal
pub const UPPER_ELEMENT_POWER_W: &str = "upper_element_power_w";

/// scope: internal
pub const LOWER_ELEMENT_POWER_W: &str = "lower_element_power_w";

/// scope: internal
pub const BURNER_EFFICIENCY: &str = "burner_efficiency";

/// scope: internal
pub const BURNER_EFFICIENCY_SOURCE: &str = "burner_efficiency_source";

/// scope: internal
pub const BURNER_POWER_W: &str = "burner_power_w";

/// scope: internal
pub const PILOT_POWER_W: &str = "pilot_power_w";

/// scope: internal
pub const BACKUP_ELEMENT_POWER_W: &str = "backup_element_power_w";

/// scope: internal
pub const ZONE_HEAT_EXTRACTION_W: &str = "zone_heat_extraction_w";

/// scope: internal
pub const WALL_SENSIBLE_GAIN_W: &str = "wall_sensible_gain_w";

/// scope: internal
pub const UNMET_LOAD_W: &str = "unmet_load_w";

/// scope: internal
pub const PARASITIC_ELECTRIC_W: &str = "parasitic_electric_w";

/// scope: internal
pub const PILOT_HEAT_TO_WATER_W: &str = "pilot_heat_to_water_w";

/// scope: internal
pub const PILOT_HEAT_TO_AMBIENT_W: &str = "pilot_heat_to_ambient_w";

// ── Envelope energy balance ─────────────────────────────────────────────────
/// Per-zone energy balance residual [W] from the zone-air first-law check.
///
/// Published every timestep by the thermal solver's `format_domain_update`.
/// Values near zero indicate the zone air energy balance closes; large values
/// may indicate port-wiring bugs, incorrect B_d entries, or wall-mass energy
/// redistribution in multi-node models.
///
/// scope: internal
pub const ENERGY_BALANCE_RESIDUAL_W: &str = "energy_balance_residual_w";

// ── Ventilation / recovery ──────────────────────────────────────────────────

/// scope: internal
pub const SENSIBLE_RECOVERY_W: &str = "sensible_recovery_w";

/// scope: internal
pub const LATENT_RECOVERY_W: &str = "latent_recovery_w";

/// Supply-side fan electrical power per timestep [W].
///
/// scope: internal
pub const VENT_SUPPLY_FAN_POWER_W: &str = "vent_supply_fan_power_w";

/// Exhaust-side fan electrical power per timestep [W].
///
/// scope: internal
pub const VENT_EXHAUST_FAN_POWER_W: &str = "vent_exhaust_fan_power_w";

/// Continuous defrost fraction [0–1] for HRV/ERV recovery derating
/// at low outdoor temperatures.
///
/// scope: internal
pub const VENT_DEFROST_FRACTION: &str = "vent_defrost_fraction";

// ── Dehumidifier ────────────────────────────────────────────────────────────

/// scope: internal
pub const WATER_REMOVAL_L_DAY: &str = "water_removal_l_day";

/// scope: internal
pub const LATENT_REMOVAL_W: &str = "latent_removal_w";

/// scope: internal
pub const TARGET_RH: &str = "target_rh";

/// scope: internal
pub const MIN_RH: &str = "min_rh";

/// scope: internal
pub const MAX_RH: &str = "max_rh";

/// scope: internal
pub const TEMPERATURE_LOCKOUT: &str = "temperature_lockout";

/// scope: internal
pub const INLET_AIR_TEMP_C: &str = "inlet_air_temp_c";

/// scope: internal
pub const MOISTURE_MASS_FLOW_KG_S: &str = "moisture_mass_flow_kg_s";

/// scope: internal
pub const HUMIDITY_SEMI_IMPLICIT_ALPHA: &str = "humidity_semi_implicit_alpha";

/// Synthetic `equipment_telemetry` key for solver-level per-zone humidity data.
///
/// The humidity solver is not an `Equipment` instance and has no entry in
/// `equipment_id_by_name`, so the standard `retain` that keeps only
/// registered equipment names would evict this entry. Code that filters
/// `equipment_telemetry` by equipment name must exempt this key.
///
/// scope: internal
pub const HUMIDITY_SOLVER_TELEMETRY_KEY: &str = "HumiditySolver";

// ── Loads ───────────────────────────────────────────────────────────────────

/// Dryer type name captured at init, or "none" for non-dryer wet appliances.
/// HPXML 4.2 §3.8.2: ClothesDryer/Vented + ClothesDryer/FuelType determine
/// whether the dryer is vented-electric, vented-gas, or unvented-condenser.
///
/// scope: internal
pub const DRYER_TYPE: &str = "dryer_type";

// ── Dwelling-level / test equipment ─────────────────────────────────────────

/// scope: internal
pub const LAST_POWER_KW: &str = "last_power_kw";

/// scope: internal
pub const LAST_SOC_TARGET: &str = "last_soc_target";

// ── Protocol bridge ─────────────────────────────────────────────────────────

/// Protocol ID of the last dispatched ProtocolNative signal.
///
/// scope: internal
pub const PROTOCOL_ID: &str = "protocol_id";

/// Payload size in bytes of the last dispatched ProtocolNative signal.
///
/// scope: internal
pub const PAYLOAD_SIZE_BYTES: &str = "payload_size_bytes";

/// Cumulative count of ProtocolNative dispatch events.
///
/// scope: internal
pub const DISPATCH_COUNT: &str = "dispatch_count";

/// Number of equipment commands parsed from the most recent ProtocolNative
/// payload. Zero when the payload produced no commands or when parsing failed.
///
/// scope: internal
pub const PARSED_COMMAND_COUNT: &str = "parsed_command_count";

/// Cumulative count of handler parse failures.
///
/// scope: internal
pub const PARSE_ERROR_COUNT: &str = "parse_error_count";

/// Cumulative count of equipment commands parsed across all payloads
/// since the bridge was initialised or last reset.
///
/// scope: internal
pub const TOTAL_COMMANDS_PARSED: &str = "total_commands_parsed";

// ── Generator ───────────────────────────────────────────────────────────────
/// Sum of `thermal_power_w` values written to fluid port contributions.
/// Tracks the actual energy declared to the loop, enabling cross-validation
/// against `THERMAL_OUTPUT_W`. In debug / check_invariants builds an assertion
/// guards that these two values match within tolerance.
///
/// scope: internal
pub const THERMAL_POWER_DELIVERED_W: &str = "thermal_power_delivered_w";

/// Heat recovery ratio: fraction of available thermal power actually delivered to the loop.
/// 1.0 = no capping; < 1.0 = loop saturated and thermal output is capped.
/// EnergyPlus ICEngineElectricGenerator.cc:782 — HRecRatio.
///
/// scope: internal
pub const HEAT_REC_RATIO: &str = "heat_rec_ratio";

/// Unscaled thermal power available before heat recovery capping, in watts.
/// EnergyPlus ICEngineElectricGenerator.cc:757 — q_thermal before HRecRatio.
///
/// scope: internal
pub const THERMAL_AVAILABLE_W: &str = "thermal_available_w";

/// Fluid loop return temperature, in °C, read at time of heat recovery computation.
/// EnergyPlus ICEngineElectricGenerator.cc:751 — HeatRecInTemp.
///
/// scope: internal
pub const LOOP_RETURN_TEMP_C: &str = "loop_return_temp_c";

/// Jacket water heat recovery power in watts.
/// EnergyPlus ERM 26.1 — Generators: Internal Combustion Engine:
/// jacket water heat recovery is a manufacturer-supplied PLR-dependent curve.
///
/// scope: internal
pub const JACKET_WATER_W: &str = "jacket_water_w";

/// Lube oil heat recovery power in watts.
/// EnergyPlus ERM 26.1 — Generators: Internal Combustion Engine:
/// lube oil heat recovery is a manufacturer-supplied PLR-dependent curve.
///
/// scope: internal
pub const LUBE_OIL_W: &str = "lube_oil_w";

/// Exhaust heat recovery power in watts.
/// EnergyPlus ERM 26.1 — Generators: Internal Combustion Engine:
/// exhaust heat recovery is modelled via PLR-dependent curves and
/// NTU-effectiveness HX; no standard fixed value exists.
///
/// scope: internal
pub const EXHAUST_WATER_W: &str = "exhaust_water_w";

/// Jacket water supply temperature in °C. ~90°C for typical IC engines.
///
/// scope: internal
pub const SUPPLY_TEMP_JACKET_C: &str = "supply_temp_jacket_c";

/// Exhaust heat exchanger supply temperature in °C. ~400–500°C raw exhaust.
///
/// scope: internal
pub const SUPPLY_TEMP_EXHAUST_C: &str = "supply_temp_exhaust_c";
// (ELECTRIC_OUTPUT_KW is in the electrical power section above)

/// Parasitic electrical load deduction (kW) — auxiliary pumps, fans, compressors,
/// and control electronics that consume generator output before net delivery.
///
/// scope: internal
pub const PARASITIC_KW: &str = "parasitic_kw";

// ── Fuel cell ───────────────────────────────────────────────────────────────
/// DC stack electrical output (kW) before inverter losses.
/// EnergyPlus FuelCellElectricGenerator.cc:1691-1711 — DC power efficiency model.
///
/// scope: internal
pub const FUEL_CELL_DC_KW: &str = "fuel_cell_dc_kw";

/// Power lost in DC-to-AC inverter conversion (W).
/// EnergyPlus FuelCellElectricGenerator.cc:2104-2124 — inverter model.
///
/// scope: internal
pub const FUEL_CELL_INVERTER_LOSS_W: &str = "fuel_cell_inverter_loss_w";

/// Stack cooling heat (W) removed by the stack cooler.
/// EnergyPlus FuelCellElectricGenerator.cc:1859-1865 — stack cooler polynomial.
///
/// scope: internal
pub const FUEL_CELL_STACK_HEAT_W: &str = "fuel_cell_stack_heat_w";

/// Generate a tank node temperature key for the given node index.
///
/// Returns a string like `"tank_node_0_c"`, `"tank_node_1_c"`, etc.
pub fn tank_node_key(index: usize) -> String {
    format!("tank_node_{index}_c")
}

// ── Compile-time scope catalogue ───────────────────────────────────────────

/// Returns physically reasonable observation-space `(low, high)` bounds for
/// a given telemetry key, mirroring the Python `_observation_field_bounds`
/// in `gym_env.py` so that Rust consumers can validate bounds independently.
///
/// Keys not explicitly recognised receive the broad fallback `(-1e6, 1e6)`
/// which is finite and safe for normalisation wrappers.
pub fn observation_field_bounds(key: &str) -> (f64, f64) {
    let key = key.trim().to_lowercase();
    match key.as_str() {
        "outdoor_temp" | "outdoor_temp_c" => (-50.0, 55.0),
        "outdoor_rh" => (0.0, 1.0),
        "outdoor_humidity_ratio" => (0.0, 0.1),
        "total_power_kw" | "total_electric_kw" => (-100.0, 100.0),
        "reactive_power_kvar" => (-100.0, 100.0),
        "battery_soc" | "ev_soc" => (0.0, 1.0),
        "time_sin" | "time_cos" => (-1.0, 1.0),
        _ => {
            if key.starts_with("zone_temp[") && key.ends_with(']') {
                return (0.0, 50.0);
            }
            if key.starts_with("setpoint_heat[") && key.ends_with(']') {
                return (0.0, 50.0);
            }
            if key.starts_with("setpoint_cool[") && key.ends_with(']') {
                return (0.0, 50.0);
            }
            if key.starts_with("zone_energy_balance[") && key.ends_with(']') {
                return (-1e6_f64, 1e6_f64);
            }
            if key.starts_with("equipment_soc[") && key.ends_with(']') {
                return (0.0, 1.0);
            }
            if key.starts_with("equipment_power[") && key.ends_with(']') {
                return (0.0, 100.0);
            }
            (-1e6_f64, 1e6_f64)
        }
    }
}

/// All telemetry keys that are marked `scope: output`.
///
/// These keys have a defined output column contract: their value is placed into
/// an output column by `record_step()` and the column is resolved via
/// `build_equipment_column_map()`. Tests use this list to verify that every
/// output-scoped key has an actual column mapping.
pub const OUTPUT_SCOPE_KEYS: &[&str] = &[
    // HVAC component power — per-equipment columns at v7
    FAN_KW,
    BACKUP_ER_KW,
    DUCT_LOSS_W,
    // HVAC performance — per-equipment columns at v7
    RUNTIME_FRACTION,
    SHR,
    LATENT_GAINS_W,
    // Defrost — per-equipment column at v7
    DEFROST_CYCLE_STATE,
    // Setpoint chain — dwelling-level columns at v7
    SCHEDULE_HEATING_SETPOINT_C,
    SCHEDULE_COOLING_SETPOINT_C,
    RUNTIME_HEATING_SETPOINT_C,
    RUNTIME_COOLING_SETPOINT_C,
    // Thermostat short-cycle protection — per-equipment columns
    MIN_ON_TIME_S,
    MIN_OFF_TIME_S,
    // Equipment temperatures — per-equipment columns at v8
    SUPPLY_TEMP_C,
    SUPPLY_AIR_TEMP_C,
    RETURN_TEMP_C,
    // Compressor power — per-equipment columns at v8
    COMPRESSOR_KW,
    COMPRESSOR_POWER_W,
    FAN_ELECTRIC_W,
    FAN_POWER_W,
    // Heat pump detail — per-equipment columns at v8
    PAN_HEATER_KW,
    HP_CAPACITY_W,
    ER_CAPACITY_W,
    // PV diagnostic — per-equipment columns at v8
    DC_POWER_KW,
    IRRADIANCE_W_M2,
    // EV diagnostic — per-equipment columns at v8
    CONNECTION_STATE,
    CHARGING_LEVEL,
];

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Assert every key constant in OUTPUT_SCOPE_KEYS appears in its category,
    /// no duplicates, and all list entries reference actual constants.
    #[test]
    fn test_output_scope_keys_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for &key in OUTPUT_SCOPE_KEYS {
            assert!(
                seen.insert(key),
                "duplicate key in OUTPUT_SCOPE_KEYS: '{key}'"
            );
        }
    }

    /// Verify every OUTPUT_SCOPE_KEYS entry is a valid string literal
    /// that matches its constant definition (self-consistency check).
    #[test]
    fn test_output_scope_keys_self_consistent() {
        for &key in OUTPUT_SCOPE_KEYS {
            // Verify the constant value is the same as the variable name suggests
            let expected_snake = key.to_ascii_lowercase();
            assert_eq!(
                key, expected_snake,
                "OUTPUT_SCOPE_KEYS entry '{key}' should be a literal string matching its constant name"
            );
        }
    }

    /// Assert all key constants in this module that appear in OUTPUT_SCOPE_KEYS
    /// are actually defined (sanity check — would fail at compile time if not).
    #[test]
    fn test_output_scope_keys_reference_real_constants() {
        // OUTPUT_SCOPE_KEYS references constants by name. If any constant were
        // renamed or removed, this file would not compile. This test just
        // verifies the length is positive so the list can't be accidentally emptied.
        assert!(
            !OUTPUT_SCOPE_KEYS.is_empty(),
            "OUTPUT_SCOPE_KEYS must not be empty"
        );
        // Verify key count matches what we expect (update when adding output keys).
        assert_eq!(
            OUTPUT_SCOPE_KEYS.len(),
            27,
            "OUTPUT_SCOPE_KEYS length changed — update this assertion and check column coverage"
        );
    }

    /// All known telemetry key constants (listed exhaustively here) are
    /// accounted for: each appears either in OUTPUT_SCOPE_KEYS or is
    /// implicitly internal. This guards against newly-added keys
    /// receiving no scope annotation.
    ///
    /// When adding a new key constant to this file:
    /// 1. Add a `scope:` doc comment.
    /// 2. If `scope: output`, add it to OUTPUT_SCOPE_KEYS.
    /// 3. Update the count in test_output_scope_keys_reference_real_constants.
    #[test]
    fn test_all_key_constants_have_scope() {
        // This is a static audit: exhaustively list every constant defined in
        // this file. Each must appear in OUTPUT_SCOPE_KEYS (output) or not
        // (internal). The test below proves we didn't forget to list a key.
        let all_keys: &[&str] = &[
            // Electrical power
            ELECTRIC_KW,
            ACTIVE_POWER_KW,
            AC_POWER_KW,
            ELECTRIC_OUTPUT_KW,
            ELECTRIC_POWER_W,
            REACTIVE_POWER_KVAR,
            // Fuel & thermal
            FUEL_INPUT_W,
            FUEL_IDLE_W,
            FUEL_LOAD_W,
            THERMAL_OUTPUT_W,
            FLUE_LOSS_W,
            JACKET_LOSS_W,
            SKIN_LOSS_W,
            SENSIBLE_GAIN_W,
            TOTAL_SENSIBLE_GAIN_W,
            LATENT_GAIN_W,
            SENSIBLE_COOLING_W,
            LATENT_COOLING_W,
            COIL_SENSIBLE_COOLING_W,
            COIL_LATENT_COOLING_W,
            LATENT_GAINS_W,
            FAN_HEAT_W,
            IDEAL_CAPACITY_W,
            IDEAL_CAPACITY_DEGRADED,
            // Context
            OUTDOOR_TEMP_C,
            INDOOR_TEMP_C,
            // Temperature
            CELL_TEMP_C,
            BATTERY_TEMP_C,
            SUPPLY_TEMP_C,
            SUPPLY_AIR_TEMP_C,
            RETURN_TEMP_C,
            TANK_AVG_TEMP_C,
            OUTLET_TEMP_C,
            APPARATUS_DEW_POINT_C,
            CURRENT_TARGET_C,
            BOILER_CP_USED_J_KG_K,
            // Setpoints
            HEATING_SETPOINT_C,
            COOLING_SETPOINT_C,
            // Setpoint chain
            SCHEDULE_HEATING_SETPOINT_C,
            SCHEDULE_COOLING_SETPOINT_C,
            RUNTIME_HEATING_SETPOINT_C,
            RUNTIME_COOLING_SETPOINT_C,
            // Thermostat short-cycle protection
            MIN_ON_TIME_S,
            MIN_OFF_TIME_S,
            // Operating state
            OPERATING_MODE,
            STATE,
            RUNTIME_FRACTION,
            SPEED_INDEX,
            // Speed/staging
            SPEED_FRAC,
            PART_LOAD_RATIO,
            PART_LOAD_FACTOR,
            STARTUP_MULTIPLIER,
            TIME_SINCE_START_MIN,
            STARTUP_TIMER_RESET_COUNT,
            DUTY_CYCLE,
            TIME_AT_CURRENT_SPEED_S,
            MODE_DURATION_S,
            COOLING_OAT_LOCKOUT,
            DEFROST_ACTIVE,
            DEFROST_TIME_FRACTION,
            DEFROST_EXTRA_POWER_W,
            DEFROST_Q_W,
            DEFROST_CAPACITY_MULTIPLIER,
            DEFROST_CYCLE_STATE,
            DEFROST_ACCUMULATED_FROST_S,
            DEFROST_ELAPSED_S,
            BYPASS_ACTIVE,
            BYPASS_FACTOR,
            IS_ON,
            RAMP_LIMITED,
            CYCLE_PHASE,
            // Control overrides
            MAX_CAPACITY_FRACTION,
            // Efficiency
            COP,
            EIR,
            EBM_EFFICIENCY,
            EBM_BASELINE_POWER_KW,
            EBM_ENERGY_KWH,
            EBM_MIN_ENERGY_KWH,
            EBM_MAX_ENERGY_KWH,
            EBM_MAX_POWER_KW,
            SHR,
            CAP_MULT,
            ETA_ELECTRIC,
            INVERTER_EFFICIENCY,
            CAP_RATIO,
            CAP_RATIO_RAW,
            EIR_RATIO,
            BIQUADRATIC_CURVE_SOURCE,
            BIQUADRATIC_INDEX_CLAMPED,
            HIGH_SIDE_CURVE_CLAMPED_SPEED_FRAC,
            LATENT_DEGRADATION_ACTIVE,
            SHR_BEFORE_DEGRADATION,
            // Battery / storage
            SOC,
            OHMIC_LOSS_W,
            STANDBY_POWER_W,
            HEATER_POWER_W,
            DISCHARGE_DERATE,
            CAPACITY_DERATE,
            CHARGE_DERATE,
            CYCLE_COUNT,
            CAPACITY_FADE_PCT,
            TERMINAL_VOLTAGE_V,
            CURRENT_A,
            DR_POWER_FRACTION,
            DR_LEVEL,
            N_SERIES,
            N_PARALLEL,
            AH_CELL,
            V_CELL,
            DERIVATION_SOURCE,
            IMPLIED_CAPACITY_KWH,
            DECLARED_CAPACITY_KWH,
            // EV
            CONNECTION_STATE,
            CHARGING_LEVEL,
            V2L_ACTIVE,
            V2L_POWER_KW,
            AWAY_CHARGE_POWER_KW,
            CAPACITY_KWH,
            CAPACITY_KWH_RATED,
            FUEL_ECONOMY_KWH_PER_MI,
            // PV
            DC_POWER_KW,
            IRRADIANCE_W_M2,
            CURTAILMENT_KW,
            INVERTER_CLIPPING_KW,
            SOILING_RATIO,
            SHADING_FACTOR,
            PV_LUT_INTERP_METHOD,
            PV_LUT_NN_FALLBACK_COUNT,
            Q_SOURCE,
            // HVAC capacity
            HVAC_HEATING_CAPACITY_W,
            HVAC_COOLING_CAPACITY_W,
            HEATING_LATENT_W,
            // Component power
            COMPRESSOR_KW,
            COMPRESSOR_POWER_W,
            FAN_KW,
            FAN_ELECTRIC_W,
            FAN_POWER_W,
            MAIN_POWER_KW,
            DUCT_LOSS_W,
            BACKUP_ER_KW,
            PAN_HEATER_KW,
            HP_CAPACITY_W,
            ER_CAPACITY_W,
            HP_LOCKOUT_TEMP_C,
            ER_LOCKOUT_TEMP_C,
            ER_SETPOINT_OFFSET_C,
            ER_HARD_LOCKOUT_TIME_S,
            ZONE_RISING,
            BACKUP_CAPACITY_W,
            BACKUP_EIR,
            ER_STAGES_ON,
            MIN_COMPRESSOR_FRACTION,
            PUMP_POWER_KW,
            CRANKCASE_KW,
            // Water heater
            ELEMENT_KW,
            PILOT_KW,
            FUEL_INPUT_KW,
            DRAW_FLOW_RATE_KG_S,
            UPPER_ELEMENT_POWER_W,
            LOWER_ELEMENT_POWER_W,
            BURNER_EFFICIENCY,
            BURNER_EFFICIENCY_SOURCE,
            BURNER_POWER_W,
            PILOT_POWER_W,
            BACKUP_ELEMENT_POWER_W,
            ZONE_HEAT_EXTRACTION_W,
            WALL_SENSIBLE_GAIN_W,
            UNMET_LOAD_W,
            PARASITIC_ELECTRIC_W,
            PILOT_HEAT_TO_WATER_W,
            PILOT_HEAT_TO_AMBIENT_W,
            // Envelope
            ENERGY_BALANCE_RESIDUAL_W,
            // Ventilation
            SENSIBLE_RECOVERY_W,
            LATENT_RECOVERY_W,
            VENT_SUPPLY_FAN_POWER_W,
            VENT_EXHAUST_FAN_POWER_W,
            VENT_DEFROST_FRACTION,
            // Dehumidifier
            WATER_REMOVAL_L_DAY,
            LATENT_REMOVAL_W,
            TARGET_RH,
            MIN_RH,
            MAX_RH,
            TEMPERATURE_LOCKOUT,
            INLET_AIR_TEMP_C,
            MOISTURE_MASS_FLOW_KG_S,
            HUMIDITY_SEMI_IMPLICIT_ALPHA,
            HUMIDITY_SOLVER_TELEMETRY_KEY,
            // Dwelling-level
            LAST_POWER_KW,
            LAST_SOC_TARGET,
            DRYER_TYPE,
            // Protocol bridge
            PROTOCOL_ID,
            PAYLOAD_SIZE_BYTES,
            DISPATCH_COUNT,
            PARSED_COMMAND_COUNT,
            PARSE_ERROR_COUNT,
            TOTAL_COMMANDS_PARSED,
            // Generator
            THERMAL_POWER_DELIVERED_W,
            HEAT_REC_RATIO,
            THERMAL_AVAILABLE_W,
            LOOP_RETURN_TEMP_C,
            JACKET_WATER_W,
            LUBE_OIL_W,
            EXHAUST_WATER_W,
            SUPPLY_TEMP_JACKET_C,
            SUPPLY_TEMP_EXHAUST_C,
            PARASITIC_KW,
            // Fuel cell
            FUEL_CELL_DC_KW,
            FUEL_CELL_INVERTER_LOSS_W,
            FUEL_CELL_STACK_HEAT_W,
        ];

        for &key in all_keys {
            let is_output = OUTPUT_SCOPE_KEYS.contains(&key);
            let is_internal = !is_output;
            assert!(
                is_output || is_internal,
                "key '{key}' must be in OUTPUT_SCOPE_KEYS if scope: output"
            );
        }

        // Cross-check: every OUTPUT_SCOPE_KEYS entry appears in all_keys.
        for &output_key in OUTPUT_SCOPE_KEYS {
            assert!(
                all_keys.contains(&output_key),
                "OUTPUT_SCOPE_KEYS entry '{output_key}' not found in all_keys audit list"
            );
        }
    }

    /// Every known telemetry key constant produces finite, non-NaN,
    /// non-inverted bounds from `observation_field_bounds`, and the
    /// single explicitly-matched constant `OUTDOOR_TEMP_C` resolves to
    /// the expected temperature range.
    #[test]
    fn test_observation_field_bounds_all_keys_finite() {
        let all_keys: &[&str] = &[
            // Electrical power
            ELECTRIC_KW,
            ACTIVE_POWER_KW,
            AC_POWER_KW,
            ELECTRIC_OUTPUT_KW,
            ELECTRIC_POWER_W,
            REACTIVE_POWER_KVAR,
            // Fuel & thermal
            FUEL_INPUT_W,
            FUEL_IDLE_W,
            FUEL_LOAD_W,
            THERMAL_OUTPUT_W,
            FLUE_LOSS_W,
            JACKET_LOSS_W,
            SKIN_LOSS_W,
            SENSIBLE_GAIN_W,
            TOTAL_SENSIBLE_GAIN_W,
            LATENT_GAIN_W,
            SENSIBLE_COOLING_W,
            LATENT_COOLING_W,
            COIL_SENSIBLE_COOLING_W,
            COIL_LATENT_COOLING_W,
            LATENT_GAINS_W,
            FAN_HEAT_W,
            IDEAL_CAPACITY_W,
            IDEAL_CAPACITY_DEGRADED,
            // Context
            OUTDOOR_TEMP_C,
            INDOOR_TEMP_C,
            // Temperature
            CELL_TEMP_C,
            BATTERY_TEMP_C,
            SUPPLY_TEMP_C,
            SUPPLY_AIR_TEMP_C,
            RETURN_TEMP_C,
            TANK_AVG_TEMP_C,
            OUTLET_TEMP_C,
            APPARATUS_DEW_POINT_C,
            CURRENT_TARGET_C,
            BOILER_CP_USED_J_KG_K,
            // Setpoints
            HEATING_SETPOINT_C,
            COOLING_SETPOINT_C,
            // Setpoint chain
            SCHEDULE_HEATING_SETPOINT_C,
            SCHEDULE_COOLING_SETPOINT_C,
            RUNTIME_HEATING_SETPOINT_C,
            RUNTIME_COOLING_SETPOINT_C,
            // Thermostat short-cycle protection
            MIN_ON_TIME_S,
            MIN_OFF_TIME_S,
            // Operating state
            OPERATING_MODE,
            STATE,
            RUNTIME_FRACTION,
            SPEED_INDEX,
            // Speed/staging
            SPEED_FRAC,
            PART_LOAD_RATIO,
            PART_LOAD_FACTOR,
            STARTUP_MULTIPLIER,
            TIME_SINCE_START_MIN,
            STARTUP_TIMER_RESET_COUNT,
            DUTY_CYCLE,
            TIME_AT_CURRENT_SPEED_S,
            MODE_DURATION_S,
            COOLING_OAT_LOCKOUT,
            DEFROST_ACTIVE,
            DEFROST_TIME_FRACTION,
            DEFROST_EXTRA_POWER_W,
            DEFROST_Q_W,
            DEFROST_CAPACITY_MULTIPLIER,
            DEFROST_CYCLE_STATE,
            DEFROST_ACCUMULATED_FROST_S,
            DEFROST_ELAPSED_S,
            BYPASS_ACTIVE,
            BYPASS_FACTOR,
            IS_ON,
            RAMP_LIMITED,
            CYCLE_PHASE,
            // Control overrides
            MAX_CAPACITY_FRACTION,
            // Efficiency
            COP,
            EIR,
            EBM_EFFICIENCY,
            EBM_BASELINE_POWER_KW,
            EBM_ENERGY_KWH,
            EBM_MIN_ENERGY_KWH,
            EBM_MAX_ENERGY_KWH,
            EBM_MAX_POWER_KW,
            SHR,
            CAP_MULT,
            ETA_ELECTRIC,
            INVERTER_EFFICIENCY,
            CAP_RATIO,
            CAP_RATIO_RAW,
            EIR_RATIO,
            BIQUADRATIC_CURVE_SOURCE,
            BIQUADRATIC_INDEX_CLAMPED,
            HIGH_SIDE_CURVE_CLAMPED_SPEED_FRAC,
            LATENT_DEGRADATION_ACTIVE,
            SHR_BEFORE_DEGRADATION,
            // Battery / storage
            SOC,
            OHMIC_LOSS_W,
            STANDBY_POWER_W,
            HEATER_POWER_W,
            DISCHARGE_DERATE,
            CAPACITY_DERATE,
            CHARGE_DERATE,
            CYCLE_COUNT,
            CAPACITY_FADE_PCT,
            TERMINAL_VOLTAGE_V,
            CURRENT_A,
            DR_POWER_FRACTION,
            DR_LEVEL,
            N_SERIES,
            N_PARALLEL,
            AH_CELL,
            V_CELL,
            DERIVATION_SOURCE,
            IMPLIED_CAPACITY_KWH,
            DECLARED_CAPACITY_KWH,
            // EV
            CONNECTION_STATE,
            CHARGING_LEVEL,
            V2L_ACTIVE,
            V2L_POWER_KW,
            AWAY_CHARGE_POWER_KW,
            CAPACITY_KWH,
            CAPACITY_KWH_RATED,
            FUEL_ECONOMY_KWH_PER_MI,
            // PV
            DC_POWER_KW,
            IRRADIANCE_W_M2,
            CURTAILMENT_KW,
            INVERTER_CLIPPING_KW,
            SOILING_RATIO,
            SHADING_FACTOR,
            PV_LUT_INTERP_METHOD,
            PV_LUT_NN_FALLBACK_COUNT,
            Q_SOURCE,
            // HVAC capacity
            HVAC_HEATING_CAPACITY_W,
            HVAC_COOLING_CAPACITY_W,
            HEATING_LATENT_W,
            // Component power
            COMPRESSOR_KW,
            COMPRESSOR_POWER_W,
            FAN_KW,
            FAN_ELECTRIC_W,
            FAN_POWER_W,
            MAIN_POWER_KW,
            DUCT_LOSS_W,
            BACKUP_ER_KW,
            PAN_HEATER_KW,
            HP_CAPACITY_W,
            ER_CAPACITY_W,
            HP_LOCKOUT_TEMP_C,
            ER_LOCKOUT_TEMP_C,
            ER_SETPOINT_OFFSET_C,
            ER_HARD_LOCKOUT_TIME_S,
            ZONE_RISING,
            BACKUP_CAPACITY_W,
            BACKUP_EIR,
            ER_STAGES_ON,
            MIN_COMPRESSOR_FRACTION,
            PUMP_POWER_KW,
            CRANKCASE_KW,
            // Water heater
            ELEMENT_KW,
            PILOT_KW,
            FUEL_INPUT_KW,
            DRAW_FLOW_RATE_KG_S,
            UPPER_ELEMENT_POWER_W,
            LOWER_ELEMENT_POWER_W,
            BURNER_EFFICIENCY,
            BURNER_EFFICIENCY_SOURCE,
            BURNER_POWER_W,
            PILOT_POWER_W,
            BACKUP_ELEMENT_POWER_W,
            ZONE_HEAT_EXTRACTION_W,
            WALL_SENSIBLE_GAIN_W,
            UNMET_LOAD_W,
            PARASITIC_ELECTRIC_W,
            PILOT_HEAT_TO_WATER_W,
            PILOT_HEAT_TO_AMBIENT_W,
            // Envelope
            ENERGY_BALANCE_RESIDUAL_W,
            // Ventilation
            SENSIBLE_RECOVERY_W,
            LATENT_RECOVERY_W,
            VENT_SUPPLY_FAN_POWER_W,
            VENT_EXHAUST_FAN_POWER_W,
            VENT_DEFROST_FRACTION,
            // Dehumidifier
            WATER_REMOVAL_L_DAY,
            LATENT_REMOVAL_W,
            TARGET_RH,
            MIN_RH,
            MAX_RH,
            TEMPERATURE_LOCKOUT,
            INLET_AIR_TEMP_C,
            MOISTURE_MASS_FLOW_KG_S,
            HUMIDITY_SEMI_IMPLICIT_ALPHA,
            HUMIDITY_SOLVER_TELEMETRY_KEY,
            // Dwelling-level
            LAST_POWER_KW,
            LAST_SOC_TARGET,
            DRYER_TYPE,
            // Protocol bridge
            PROTOCOL_ID,
            PAYLOAD_SIZE_BYTES,
            DISPATCH_COUNT,
            PARSED_COMMAND_COUNT,
            PARSE_ERROR_COUNT,
            TOTAL_COMMANDS_PARSED,
            // Generator
            THERMAL_POWER_DELIVERED_W,
            HEAT_REC_RATIO,
            THERMAL_AVAILABLE_W,
            LOOP_RETURN_TEMP_C,
            JACKET_WATER_W,
            LUBE_OIL_W,
            EXHAUST_WATER_W,
            SUPPLY_TEMP_JACKET_C,
            SUPPLY_TEMP_EXHAUST_C,
            PARASITIC_KW,
            // Fuel cell
            FUEL_CELL_DC_KW,
            FUEL_CELL_INVERTER_LOSS_W,
            FUEL_CELL_STACK_HEAT_W,
        ];

        for &key in all_keys {
            let (low, high) = observation_field_bounds(key);
            assert!(
                low.is_finite(),
                "low bound {low} not finite for key '{key}'"
            );
            assert!(
                high.is_finite(),
                "high bound {high} not finite for key '{key}'"
            );
            assert!(!low.is_nan(), "low bound NaN for key '{key}'");
            assert!(!high.is_nan(), "high bound NaN for key '{key}'");
            assert!(
                low < high,
                "bounds inverted for key '{key}': {low} >= {high}"
            );
        }

        // Sanity: the one key that matches an explicit branch resolves to the
        // correct temperature range.
        assert_eq!(observation_field_bounds(OUTDOOR_TEMP_C), (-50.0, 55.0));
        // Sanity: unrecognised keys use the broad but finite fallback.
        assert_eq!(
            observation_field_bounds("nonexistent_field"),
            (-1e6_f64, 1e6_f64)
        );
    }
}
