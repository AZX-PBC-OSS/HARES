//! Named constants for heat-pump models.

pub const DEFAULT_ZONE_ID: u16 = 1;
pub const DEFAULT_EQUIPMENT_ID: u32 = 0;

pub const DEFROST_ENABLE_TEMP_C: f64 = 4.4445;
pub const DEFROST_COIL_TEMP_SLOPE: f64 = 0.82;
pub const DEFROST_COIL_TEMP_OFFSET_C: f64 = -8.589;
pub const DEFROST_MIN_DELTA_HUMIDITY_RATIO: f64 = 0.000_001;
pub const DEFROST_TIME_FRACTION_NUMERATOR: f64 = 0.01446;
pub const DEFROST_CAPACITY_MULTIPLIER_BASE: f64 = 0.875;
pub const DEFROST_POWER_MULTIPLIER_NUMERATOR: f64 = 0.954;
pub const DEFROST_Q_MULTIPLIER: f64 = 0.01;
pub const DEFROST_REFERENCE_TEMP_C: f64 = 7.222;
pub const DEFROST_CAPACITY_UNIT_FACTOR: f64 = 1.01667;
/// Dimensionless defrost EIR modifier applied to (capacity_w / DEFROST_CAPACITY_UNIT_FACTOR).
/// Sourced from EnergyPlus OnDemand defrost formula. Despite the legacy OCHRE comment
/// "# in kW", dimensional analysis of OCHRE's update_eir (line 1172 of HVAC.py) confirms
/// the result is in watts: `(eir * capacity_W * mult + power_defrost) / capacity_W`.
pub const DEFROST_EIR_TEMP_MODIFIER: f64 = 0.1528;

// Timed defrost mode constants (EnergyPlus / DOE-2)
/// Default timed defrost fraction: ~3.5 min/hr (EnergyPlus default).
pub const DEFAULT_DEFROST_TIME_FRACTION: f64 = 0.058;
/// Timed mode capacity multiplier: C0 - C1 * outdoor_coil_dw
pub const TIMED_DEFROST_CAP_MULT_BASE: f64 = 0.909;
pub const TIMED_DEFROST_CAP_MULT_SLOPE: f64 = 107.33;
/// Timed mode power multiplier: C0 - C1 * outdoor_coil_dw
pub const TIMED_DEFROST_PWR_MULT_BASE: f64 = 0.90;
pub const TIMED_DEFROST_PWR_MULT_SLOPE: f64 = 36.45;
/// Minimum wet-bulb / outdoor DB for defrost EIR curve evaluation [°C].
pub const DEFROST_EIR_CURVE_TEMP_MIN_C: f64 = 15.555;

/// Default compressor lockout temperature for air-source heat pumps: -17.78°C (0°F).
///
/// This is the outdoor air temperature below which the heat pump compressor is disabled,
/// falling through to backup electric resistance heat when the HPXML file omits
/// `CompressorLockoutTemperature`.
///
/// Source chain:
/// - **Direct source**: OCHRE `vendors/OCHRE/ochre/Equipment/HVAC.py:1208` defaults to
///   `-17.78` with the annotation `# 0F default`. OCHRE's HPXML parser
///   (`hpxml.py:947-951`) also uses 0°F (-17.78°C) when both `CompressorLockoutTemperature`
///   and `BackupHeatingSwitchoverTemperature` are absent.
/// - **Corroborating evidence**: OpenStudio-HPXML defaults `CompressorLockoutTemperature` to
///   0°F for standard single-speed air-to-air heat pumps (per HPXML Working Group PR #309
///   and openstudio-hpxml.readthedocs.io).
/// - **Intentional divergence from EnergyPlus**: the EnergyPlus `Coil:Heating:DX:SingleSpeed`
///   object's "Minimum Outdoor Dry-Bulb Temperature for Compressor Operation" defaults to
///   -8°C (≈ 17.6°F). HARES intentionally follows the OCHRE / OpenStudio-HPXML 0°F value,
///   which reflects the practical minimum operating temperature for typical residential
///   non-cold-climate ASHPs as manufactured and installed.
///
/// Consumed by `HeatPumpHeater` in `heater.rs` as the default for `hp_lockout_temp_c` when
/// the HPXML `CompressorLockoutTemperature` or `BackupHeatingSwitchoverTemperature` field is
/// absent.
pub const DEFAULT_HP_LOCKOUT_TEMP_C: f64 = -17.78;
/// Compressor lockout hysteresis band width [°C].
/// Prevents rapid HP enable/disable chatter near the lockout threshold.
pub const DEFAULT_HP_LOCKOUT_HYSTERESIS_C: f64 = 0.5;
pub const DEFAULT_ER_LOCKOUT_TEMP_C: f64 = 4.44;
/// OCHRE default has no minimum backup-strip off-time unless explicitly set.
pub const DEFAULT_MIN_ER_CYCLE_TIME_S: f64 = 0.0;
/// OCHRE-parity default for backup-strip hard lockout after setpoint raise.
/// Vendored OCHRE HVAC.py uses 0 minutes when not configured.
pub const DEFAULT_ER_HARD_LOCKOUT_TIME_S: f64 = 0.0;
/// EnergyPlus supplemental-ER upper OAT bound. Above this temperature the heat
/// pump alone is sufficient; ER must not fire. Hard maximum matches EnergyPlus
/// default (21°C / 69.8°F). User values > 21°C are clamped down to this limit.
pub const MAX_OAT_SUPPLEMENTAL_C: f64 = 21.0;
/// Multiplier applied to the HP deadband to compute the default ER setpoint offset.
/// From OCHRE / Winkler: offset = deadband * (MULTIPLIER - DEADBAND_OFFSET).
pub const DEFAULT_ER_SETPOINT_OFFSET_MULTIPLIER: f64 = 1.8;
/// Deadband subtraction term in the ER setpoint offset formula.
/// With default deadband=1.0: offset = 1.0 * (1.8 - 0.2) = 1.6°C.
pub const DEFAULT_ER_SETPOINT_DEADBAND_OFFSET: f64 = 0.2;

pub const DEFAULT_HEATING_CAPACITY_W: f64 = 10_000.0;
pub const DEFAULT_HEATING_EIR: f64 = 0.35;
pub const DEFAULT_BACKUP_CAPACITY_W: f64 = 5_000.0;
pub const DEFAULT_BACKUP_EIR: f64 = 1.0;
pub const MSHP_PAN_HEATER_DEFAULT_TEMP_C: f64 = 0.0;
/// Pan heater rated power for MSHP condensate management [kW].
/// Per OCHRE MinisplitAHSPHeater class attribute (HVAC.py:1482).
pub const MSHP_PAN_HEATER_DEFAULT_KW: f64 = 0.150;

pub const HEATER_TELEMETRY_CAPACITY: usize = 55;

// Discrete defrost cycle parameters (HARES-specific enhancement, not from EnergyPlus)
/// Default defrost cycle duration [s]. 3.5 min ≈ typical residential ASHP reverse-cycle
/// defrost period. EnergyPlus ERM 26.1 — Coils: Single-Speed Electric DX Air Heating Coil — Defrost Operation uses a continuous model; HARES adds discrete
/// ON/OFF cycling for subhourly dispatch fidelity.
pub const DEFAULT_DEFROST_CYCLE_DURATION_S: f64 = 210.0;
/// Maximum defrost cycle duration [s]. Hard cap prevents runaway defrost; 10 min
/// is the practical upper bound for residential equipment safety.
pub const MAX_DEFROST_CYCLE_DURATION_S: f64 = 600.0;

// Frost decay during extended off-periods.
// Engineering estimate — no primary source exists for temperature-indexed frost
// sublimation time constants on outdoor coils. OCHRE's defrost model is
// stateless (no accumulator, no decay concept) and EnergyPlus's DOE-2.1E-derived
// defrost formulas are steady-state humidity-driven multipliers, not a
// temperature-indexed exponential decay τ. The qualitative direction is
// physically sound: warmer air holds more energy for sublimation, so decay
// accelerates as OAT rises. Linear interpolation between the two anchor points
// at 0°C (~1 h τ) and 10°C (~10 min τ). Exact values vary with wind speed and
// coil geometry.
/// Decay time constant at 0°C OAT [s]. Below 0°C, frost does not decay.
pub const FROST_DECAY_TAU_AT_0C_S: f64 = 3600.0;
/// Decay time constant at 10°C OAT [s].
pub const FROST_DECAY_TAU_AT_10C_S: f64 = 600.0;
/// OAT below which frost decay is disabled [°C].
pub const FROST_DECAY_CLEAR_TEMP_C: f64 = 0.0;
