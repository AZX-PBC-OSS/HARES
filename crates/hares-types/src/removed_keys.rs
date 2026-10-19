//! The removed-configuration-key table: every key a surface must reject,
//! naming its replacement.
//!
//! The table lives in `hares-types` - the lowest crate - so every layer
//! that owns a key surface can consult it: the TOML readers and the
//! input schema in `hares-io`, the actor registry and the observation
//! resolver in `hares-core`, the protocol bridge's JSON handler in
//! `hares-equipment`, and the Python keyword surfaces in `hares-python`.
//! `hares-io` re-exports it, so the `hares_io::` paths the older surfaces
//! use keep working.

use serde::{Deserialize, Serialize};

/// The table (or keyword surface) a removed configuration key belonged to.
///
/// A key is removed from one table, not everywhere: a key that is valid in
/// one table and removed in another is never rejected where it is valid.
/// Serialised into the input schema's `removed_keys` (the machine-readable
/// input schema work).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyScope {
    /// A `SimulationConfig` table (TOML, synthetic TOML `[simulation]`) or keyword.
    SimulationConfig,
    /// The top level of a synthetic TOML file; a `DwellingConfig` field.
    DwellingTable,
    /// A keyword only the Python constructors and functions ever accepted.
    PythonKeyword,
    /// An equipment override key, synthetic TOML equipment key or Python equipment keyword.
    EquipmentConfig,
    /// A parameter of a registry actor (`Dwelling.add_actor_by_name` params).
    ActorParameter,
    /// A `ControlSignal` field: Python constructor keyword, HELICS signal body
    /// key, gym action key.
    ControlSignal,
    /// A name `to_observation_vec` or the gym's observation layout resolves.
    Observation,
}

/// A configuration key that once parsed and now errors, naming its replacement.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RemovedKey {
    pub scope: KeyScope,
    pub key: &'static str,
    pub replacement: &'static str,
}

/// Every configuration key a surface must reject, in table order.
///
/// Each row's `replacement` is the error text a surface raises for the key:
/// it names where the capability lives now, or that nothing replaces it.
pub static REMOVED_KEYS: &[RemovedKey] = &[
    RemovedKey {
        scope: KeyScope::DwellingTable,
        key: "resample_overrides",
        replacement: "the engine has no per-field weather resampling overrides; \
                      for OCHRE comparisons hold the weather with the Python adapter \
                      (`hares.adapters.ochre.hold_weather`) and pass the held file \
                      as the weather input",
    },
    RemovedKey {
        scope: KeyScope::DwellingTable,
        key: "output",
        replacement: "the synthetic [output] table is gone: its output fields \
                      (output_chunk_size, master_seed) live in the [simulation] \
                      table, and per-report destinations (`SimulationConfig.reports`) \
                      replace the output booleans",
    },
    RemovedKey {
        scope: KeyScope::SimulationConfig,
        key: "initialization_duration_s",
        replacement: "the warm-up lives in SimulationConfig.warmup: \
                      warmup = \"disabled\" or a [warmup] table of the convergence criteria",
    },
    RemovedKey {
        scope: KeyScope::SimulationConfig,
        key: "duration_s",
        replacement: "pass duration (seconds)",
    },
    RemovedKey {
        scope: KeyScope::SimulationConfig,
        key: "time_res_s",
        replacement: "pass time_res (seconds)",
    },
    RemovedKey {
        scope: KeyScope::SimulationConfig,
        key: "write_output",
        replacement: "pass reports = [ReportSpec(name, selection, destination)]: a \
                      report with a Memory destination keeps every batch in memory, \
                      one with a Parquet or Csv destination writes the file, and a \
                      run with no reports writes nothing",
    },
    RemovedKey {
        scope: KeyScope::SimulationConfig,
        key: "output_format",
        replacement: "pass reports = [ReportSpec(name, selection, destination)]: the \
                      destination carries the format (Destination.parquet(path) or \
                      Destination.csv(path))",
    },
    RemovedKey {
        scope: KeyScope::SimulationConfig,
        key: "output_path",
        replacement: "pass reports = [ReportSpec(name, selection, destination)]: each \
                      file destination carries its own path; nothing defaults to \
                      the working directory",
    },
    RemovedKey {
        scope: KeyScope::SimulationConfig,
        key: "output_verbosity",
        replacement: "pass reports = [ReportSpec(name, selection, destination)]: the \
                      selection carries the tier (Selection.verbosity(n))",
    },
    RemovedKey {
        scope: KeyScope::SimulationConfig,
        key: "retain_batches",
        replacement: "pass reports = [ReportSpec(name, selection, destination)]: a \
                      Memory destination retains every batch in memory",
    },
    RemovedKey {
        scope: KeyScope::SimulationConfig,
        key: "rotation",
        replacement: "pass reports = [ReportSpec(name, selection, destination)]: files \
                      do not rotate; a report writes one file at its destination path",
    },
    RemovedKey {
        scope: KeyScope::DwellingTable,
        key: "initialization_duration",
        replacement: "the warm-up lives in SimulationConfig.warmup: \
                      warmup = \"disabled\" or a [warmup] table of the convergence criteria",
    },
    RemovedKey {
        scope: KeyScope::DwellingTable,
        key: "initialization_duration_s",
        replacement: "the warm-up lives in SimulationConfig.warmup: \
                      warmup = \"disabled\" or a [warmup] table of the convergence criteria",
    },
    RemovedKey {
        scope: KeyScope::PythonKeyword,
        key: "duration_s",
        replacement: "pass the simulation length as `SimulationConfig.duration` \
                      in the typed `config`",
    },
    RemovedKey {
        scope: KeyScope::PythonKeyword,
        key: "time_res_s",
        replacement: "pass the step length as `SimulationConfig.time_res` \
                      in the typed `config`",
    },
    RemovedKey {
        scope: KeyScope::PythonKeyword,
        key: "latitude",
        replacement: "pass site_location = SiteLocationOverride(latitude_deg=...)",
    },
    RemovedKey {
        scope: KeyScope::PythonKeyword,
        key: "longitude",
        replacement: "pass site_location = SiteLocationOverride(longitude_deg=...)",
    },
    RemovedKey {
        scope: KeyScope::PythonKeyword,
        key: "elevation_m",
        replacement: "pass site_location = SiteLocationOverride(elevation_m=...)",
    },
    RemovedKey {
        scope: KeyScope::PythonKeyword,
        key: "utc_offset_h",
        replacement: "pass site_location = SiteLocationOverride(utc_offset_h=...)",
    },
    RemovedKey {
        scope: KeyScope::PythonKeyword,
        key: "output_to_parquet",
        replacement: "pass reports = [ReportSpec(name, selection, destination)] with \
                      Destination.parquet(path): the report list is the output \
                      surface (SimulationConfig.reports)",
    },
    RemovedKey {
        scope: KeyScope::PythonKeyword,
        key: "defaults_path",
        replacement: "pass defaults (a path or a parsed ParsedDefaults handle)",
    },
    RemovedKey {
        scope: KeyScope::PythonKeyword,
        key: "hpxml_dir",
        replacement: "pass the unpacked buildings directory as the `buildings_dir` argument",
    },
    // ── Equipment config fields stored in SI ────────────────────────────
    // Every row names its config type (the class an override is keyed by
    // maps to it through the registry entry that builds the class), the
    // old key path and the replacement's unit.
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "PvConfig.capacity_kw",
        replacement: "PvConfig.capacity_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "PvConfig.inverter_capacity_kw",
        replacement: "PvConfig.inverter_capacity_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "PvConfig.arrays[].capacity_kw",
        replacement: "PvConfig.arrays[].capacity_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "BatteryConfig.capacity_kwh",
        replacement: "BatteryConfig.capacity_j (J)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "BatteryConfig.max_charge_kw",
        replacement: "BatteryConfig.max_charge_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "BatteryConfig.max_discharge_kw",
        replacement: "BatteryConfig.max_discharge_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "BatteryConfig.inverter_capacity_kva",
        replacement: "BatteryConfig.inverter_capacity_va (VA)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "BatteryConfig.bms_mode.SelfConsumption.surplus_deadband_kw",
        replacement: "BatteryConfig.bms_mode.SelfConsumption.surplus_deadband_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EvConfig.capacity_kwh",
        replacement: "EvConfig.capacity_j (J)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EvConfig.max_charging_kw",
        replacement: "EvConfig.max_charging_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EvConfig.max_charging_power_kw",
        replacement: "EvConfig.max_charging_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EvConfig.v2l_max_discharge_kw",
        replacement: "EvConfig.v2l_max_discharge_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EvConfig.v2g_max_discharge_kw",
        replacement: "EvConfig.v2g_max_discharge_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EvConfig.power_limit_kw",
        replacement: "EvConfig.power_limit_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EvConfig.fuel_economy_kwh_per_mi",
        replacement: "EvConfig.fuel_economy_j_per_m (J/m)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EvConfig.charger_capacity_kva",
        replacement: "EvConfig.charger_capacity_va (VA)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EvConfig.charging_strategy.SolarSurplus.min_charge_rate_kw",
        replacement: "EvConfig.charging_strategy.SolarSurplus.min_charge_rate_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EvConfig.charging_strategy.V2G.max_export_kw",
        replacement: "EvConfig.charging_strategy.V2G.max_export_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "GeneratorConfig.rated_power_kw",
        replacement: "GeneratorConfig.rated_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "GeneratorConfig.capacity_min_kw",
        replacement: "GeneratorConfig.capacity_min_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "GeneratorConfig.grid_import_limit_kw",
        replacement: "GeneratorConfig.grid_import_limit_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "GeneratorConfig.export_limit_kw",
        replacement: "GeneratorConfig.export_limit_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "GeneratorConfig.delta_kw_per_s",
        replacement: "GeneratorConfig.delta_w_per_s (W/s)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "CentralAirConditionerConfig.crankcase_heater_kw",
        replacement: "CentralAirConditionerConfig.crankcase_heater_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "RoomAcConfig.crankcase_heater_kw",
        replacement: "RoomAcConfig.crankcase_heater_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "HeatPumpCoolerConfig.crankcase_heater_kw",
        replacement: "HeatPumpCoolerConfig.crankcase_heater_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EventBasedLoad.active_power_kw",
        replacement: "EventBasedLoad.active_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EventBasedLoad.phase_<n>_power_kw",
        replacement: "EventBasedLoad.phase_<n>_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EventBasedLoad.cycle_phase_<n>_power_kw",
        replacement: "EventBasedLoad.cycle_phase_<n>_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "EventBasedLoad.event_power_kw_series",
        replacement: "EventBasedLoad.event_power_w_series (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "WetAppliance.active_power_kw",
        replacement: "WetAppliance.active_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "WetAppliance.phase_<n>_power_kw",
        replacement: "WetAppliance.phase_<n>_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "WetAppliance.cycle_phase_<n>_power_kw",
        replacement: "WetAppliance.cycle_phase_<n>_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "ScheduledLoad.power_profile_max_kw",
        replacement: "ScheduledLoad.power_profile_max_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "ScheduledLoad.power_constant_kw",
        replacement: "ScheduledLoad.power_constant_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "ScheduledLoad.gas_profile_max",
        replacement: "ScheduledLoad.gas_profile_max_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "ScheduledLoad.gas_constant",
        replacement: "ScheduledLoad.gas_constant_w (W)",
    },
    RemovedKey {
        scope: KeyScope::EquipmentConfig,
        key: "ScheduledLoad.gas_schedule_is_w",
        replacement: "ScheduledLoad.gas_constant_w (W): the gas schedule's values are watts",
    },
    // ── The control layer, actor parameters ────────────────────────────
    // The registry's parameter names, scope `ActorParameter`, keyed
    // `<actor_type>.<parameter>`; the replacement names the SI field and
    // its unit.
    RemovedKey {
        scope: KeyScope::ActorParameter,
        key: "EvDriver.daily_drive_miles_mean",
        replacement: "EvDriver.daily_drive_distance_mean_m (m)",
    },
    RemovedKey {
        scope: KeyScope::ActorParameter,
        key: "EvDriver.fuel_economy_kwh_per_mi",
        replacement: "EvDriver.fuel_economy_j_per_m (J/m)",
    },
    RemovedKey {
        scope: KeyScope::ActorParameter,
        key: "EvDriver.capacity_kwh",
        replacement: "EvDriver.capacity_j (J)",
    },
    RemovedKey {
        scope: KeyScope::ActorParameter,
        key: "EvDriver.average_speed_mph",
        replacement: "EvDriver.average_speed_m_per_s (m/s)",
    },
    RemovedKey {
        scope: KeyScope::ActorParameter,
        key: "EvDriver.max_charge_kw",
        replacement: "EvDriver.max_charge_w (W)",
    },
    RemovedKey {
        scope: KeyScope::ActorParameter,
        key: "EvDriver.range_anxiety_miles",
        replacement: "EvDriver.range_anxiety_distance_m (m)",
    },
    RemovedKey {
        scope: KeyScope::ActorParameter,
        key: "EvDriver.away_charge_power_kw",
        replacement: "EvDriver.away_charge_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::ActorParameter,
        key: "BatteryManagement.surplus_deadband_kw",
        replacement: "BatteryManagement.surplus_deadband_w (W)",
    },
    RemovedKey {
        scope: KeyScope::ActorParameter,
        key: "BatteryManagement.max_charge_kw",
        replacement: "BatteryManagement.max_charge_w (W)",
    },
    RemovedKey {
        scope: KeyScope::ActorParameter,
        key: "BatteryManagement.max_discharge_kw",
        replacement: "BatteryManagement.max_discharge_w (W)",
    },
    // ── The control layer, control signals ──────────────────────────────
    // Every signal surface's old keyword and dict key - the three Python
    // surfaces' positional-era names (`kw`, `reactive_kvar`, `max_kw`,
    // `kvar`) included, the HELICS signal body keys, the gym's
    // action-layout keys and the protocol bridge's JSON command keys. The
    // replacement names the SI field of the variant that carries it.
    RemovedKey {
        scope: KeyScope::ControlSignal,
        key: "active_power_kw",
        replacement: "PowerSetpoint.active_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::ControlSignal,
        key: "reactive_power_kvar",
        replacement: "PowerSetpoint.reactive_power_var (VAR)",
    },
    RemovedKey {
        scope: KeyScope::ControlSignal,
        key: "kw",
        replacement: "PowerSetpoint.active_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::ControlSignal,
        key: "reactive_kvar",
        replacement: "PowerSetpoint.reactive_power_var (VAR)",
    },
    RemovedKey {
        scope: KeyScope::ControlSignal,
        key: "max_power_kw",
        replacement: "PowerLimit.max_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::ControlSignal,
        key: "max_kw",
        replacement: "PowerLimit.max_power_w (W)",
    },
    RemovedKey {
        scope: KeyScope::ControlSignal,
        key: "ramp_rate_kw_per_s",
        replacement: "PowerLimit.ramp_rate_w_per_s (W/s)",
    },
    RemovedKey {
        scope: KeyScope::ControlSignal,
        key: "kvar",
        replacement: "ReactiveSetpoint.var (VAR)",
    },
    RemovedKey {
        scope: KeyScope::ControlSignal,
        key: "kwh",
        replacement: "EvDrive.energy_j (J)",
    },
    RemovedKey {
        scope: KeyScope::ControlSignal,
        key: "power_kw",
        replacement: "EvAwayCharge.power_w (W)",
    },
    // The gym's action-layout aliases, deleted: each names the field it
    // stood for.
    RemovedKey {
        scope: KeyScope::ControlSignal,
        key: "p_setpoint_kw",
        replacement: "PowerSetpoint.active_power_w (W)",
    },
];
/// The replacement text of the removed key matching `scope` and `key`, if
/// any. A pattern row writes `<n>` where the key carries a number (a
/// phase index): `<n>` matches a run of ASCII digits and every other
/// character matches exactly.
#[must_use]
pub fn removed_key(scope: KeyScope, key: &str) -> Option<&'static str> {
    REMOVED_KEYS
        .iter()
        .find(|removed| removed.scope == scope && key_matches(removed.key, key))
        .map(|removed| removed.replacement)
}

/// Whether the table's row key matches the lookup key: equal strings, or
/// a pattern row's `<n>` matching a run of ASCII digits at that spot.
fn key_matches(row_key: &str, key: &str) -> bool {
    match (row_key.split_once("<n>"), key) {
        (Some((prefix, suffix)), _) => {
            let Some(rest) = key.strip_prefix(prefix) else {
                return false;
            };
            let Some(digits_end) = rest.find(|c: char| !c.is_ascii_digit()) else {
                // All digits to the end.
                return rest.ends_with(suffix) && rest.len() >= suffix.len() && {
                    let digits = &rest[..rest.len() - suffix.len()];
                    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
                };
            };
            let (digits, tail) = rest.split_at(digits_end);
            !digits.is_empty() && tail == suffix
        }
        (None, _) => row_key == key,
    }
}

/// The removed-key error message a surface raises for `scope` and `key`:
/// one message per key, so the same removed key reads the same from the
/// `[simulation]` table a synthetic TOML file carries and from a Python
/// keyword surface.
pub fn removed_key_message(scope: KeyScope, key: &str) -> Option<String> {
    removed_key(scope, key)
        .map(|replacement| format!("configuration key '{key}' was removed: {replacement}"))
}
