//! Standalone dehumidifier model.

use std::borrow::Cow;
use std::time::Duration;

use hares_physics::biquadratic::BiquadraticCurve;
use hares_physics::biquadratic::cubic;
use hares_physics::constants::LATENT_HEAT_VAPORISATION_0C_J_KG;
use hares_physics::units::power_w_to_kw;
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, Telemetry, TelemetryField, ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

use super::ac_config::DehumidifierConfig;
use super::dehumidifier_defaults::{
    DEFAULT_ENERGY_FACTOR_CURVE, DEFAULT_WATER_REMOVAL_CURVE, RATED_DB_C, RATED_RH,
};
use super::helpers::{
    equipment_id_from_config, zone_id_from_config, zone_id_from_config_or_default,
};

// Water density assumed as 1.0 kg/L (constant approximation).
// EnergyPlus ZoneDehumidifier.cc:723 uses RhoH2O(max(InletAirTemp - 11.0, 1.0))
// — temperature-dependent water density. At rated conditions (26.7°C) this
// is ~0.9965 kg/L. The constant 1.0 kg/L introduces ±0.35% error at rated
// conditions, which is within engineering tolerance for residential simulation.
const KG_PER_LITER_WATER: f64 = 1.0;
const HOURS_PER_DAY: f64 = 24.0;
const MINUTES_PER_HOUR: f64 = 60.0;
const SECONDS_PER_MINUTE: f64 = 60.0;
const SECONDS_PER_HOUR: f64 = MINUTES_PER_HOUR * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: f64 = HOURS_PER_DAY * SECONDS_PER_HOUR;
const DEFAULT_RATED_WATER_REMOVAL_L_DAY: f64 = 30.0;
const DEFAULT_TARGET_RH_FRACTION: f64 = 0.50;
// Engineering choice: 2.5% total deadband (±1.25% half-width) is a common
// residential dehumidifier hysteresis specification. No primary standard (ASHRAE
// HoF, AHAM DH-1, or EnergyPlus) prescribes a specific default RH deadband value
// for standalone dehumidifiers. This value is set below the typical 3–5%
// factory-set range to prioritise tighter humidity control over cycle duration.
// If a standards-mandated default is identified, that value should replace this.
const DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION: f64 = 0.0125;
const RH_MIN_FRACTION: f64 = 0.0;
const RH_MAX_FRACTION: f64 = 1.0;
// Default dry-bulb operating bounds. HPXML does not tag the AHAM DH-1 edition;
// the rated conditions are:
//   DH-1-2008 (legacy, pre-2019): 26.7°C DB (80°F) / 60% RH
//   DH-1-2017 / DH-1-2022 (current, per 10 CFR Part 430 Appendix X1):
//     18.3°C DB (65°F) / 60% RH for portable; 22.8°C DB (73°F) / 60% RH for whole-home
const DEFAULT_DB_BOUNDS_C: (f64, f64) = (10.0, 40.0);
// Default inlet air temperature operating limits for the dehumidifier compressor.
// EnergyPlus ZoneDehumidifier.cc:687–688 gates the unit when inlet air temperature
// is outside [MinInletAirTemp, MaxInletAirTemp].
// EnergyPlus IDD schema v4.2 (`Energy+.idd.in:36987`): fields N4 "Minimum Dry-Bulb
// Temperature for Dehumidifier Operation" default 10.0°C and N5 "Maximum Dry-Bulb
// Temperature for Dehumidifier Operation" default 35.0°C. These are the
// `MinInletAirTemp` / `MaxInletAirTemp` operating-lockout fields on the
// `ZoneHVAC:Dehumidifier:DX` object, distinct from the `Curve:Biquadratic`
// calibration domain (21.0°C / 32.22°C, see T-1703).
// EnergyPlus `vendors/EnergyPlus/idd/Energy+.idd.in` (v4.2).
const DEFAULT_MIN_OPERATING_TEMP_C: f64 = 10.0;
const DEFAULT_MAX_OPERATING_TEMP_C: f64 = 35.0;
const DEFAULT_RH_BOUNDS: (f64, f64) = (RH_MIN_FRACTION, RH_MAX_FRACTION);

const WATTS_PER_KILOWATT_HOUR: f64 = 3_600_000.0;
const DEFAULT_NORMALIZED_CURVE: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];

// Default PLF identity curve: PLF = 1.0 for all PLR (no cycling loss).
// EnergyPlus defaults to PLF = 1.0 when no PartLoadCurve is configured
// on the ZoneHVAC:Dehumidifier:DX object (ZoneDehumidifier.cc lines 763–767).
// Users can supply custom cubic coefficients via `DehumidifierConfig::
// part_load_curve_coeffs` to model cycling degradation.
const DEFAULT_PLF_CURVE_COEFFS: [f64; 4] = [1.0, 0.0, 0.0, 0.0];
// PLF lower clamp per EnergyPlus ZoneDehumidifier.cc lines 769–808.
const DEFAULT_PLF_MIN: f64 = 0.7;
const DEFAULT_PLF_MAX: f64 = 1.0;

const KEY_MIN_RH: &str = "min_rh";
const KEY_MAX_RH: &str = "max_rh";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DehumidifierState {
    is_on: bool,
    accumulated_water_removal_l: f64,
    target_rh: f64,
    min_rh: f64,
    max_rh: f64,
    mode_override: Option<OperatingMode>,
}

#[derive(Clone, Copy, Debug)]
struct PerformanceSnapshot {
    water_removal_l_day: f64,
    electric_power_w: f64,
    latent_removal_w: f64,
    sensible_gain_w: f64,
    plr: f64,
    plf: f64,
    rtf: f64,
    /// Actual off-cycle parasitic electric draw [W] this step, already
    /// scaled by the off-cycle fraction. EnergyPlus ZoneDehumidifier.cc:901:
    /// OffCycleParasiticElecPower = (1 - RunTimeFraction) * OffCycleParasiticLoad.
    /// Carried on the snapshot (not read from config) so the grid-outage gate
    /// in `step()` zeroes it alongside `electric_power_w` — telemetry and port
    /// contribution must never diverge.
    parasitic_electric_w: f64,
}

pub struct Dehumidifier {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    zone_id: ZoneId,
    operating_mode: OperatingMode,
    is_on: bool,
    mode_override: Option<OperatingMode>,
    rated_water_removal_l_day: f64,
    rated_energy_factor_l_kwh: f64,
    fraction_load_served: f64,
    target_rh: f64,
    min_rh: f64,
    max_rh: f64,
    water_removal_curve: BiquadraticCurve,
    energy_factor_curve: BiquadraticCurve,
    water_removal_curve_rated_value: f64,
    energy_factor_curve_rated_value: f64,
    accumulated_water_removal_l: f64,
    /// Cubic part-load curve coefficients [C0, C1, C2, C3] mapping PLR → PLF.
    part_load_curve_coeffs: [f64; 4],
    /// Lower clamp for PLF (part-load factor).
    plf_min: f64,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
    /// Off-cycle parasitic electric load [W].
    ///
    /// When the unit is off, this constant load (standby electronics, controls,
    /// crankcase heater) is drawn continuously. EnergyPlus
    /// `ZoneDehumidifier.hh:93` accepts `OffCycleParasiticLoad` as user input
    /// and applies it to the off-cycle portion of each timestep
    /// (`ZoneDehumidifier.cc:852, 880–886`).
    ///
    /// When `is_on == true`, the on-cycle rated power (via energy factor) already
    /// includes the parasitic implicitly; the parasitic is only applied to the
    /// off-cycle fraction `(1 - RTF)` to avoid double-counting.
    off_cycle_parasitic_load_w: Option<f64>,
    /// Minimum inlet air dry-bulb temperature for compressor operation [°C].
    ///
    /// Below this temperature the unit is locked out to prevent evaporator
    /// freeze-up. `None` disables the low-temperature lockout.
    min_operating_temp_c: Option<f64>,
    /// Maximum inlet air dry-bulb temperature for compressor operation [°C].
    ///
    /// Above this temperature the unit is locked out for compressor thermal
    /// protection. `None` disables the high-temperature lockout.
    max_operating_temp_c: Option<f64>,
    /// Rule R1 reactive-only ZIP (resolved via `crate::config::resolve_reactive_zip`):
    /// compressor pf 0.96 on the total electric draw. Real power stays
    /// bit-identical; Q comes from `ZipLoad::reactive_kvar`.
    zip: hares_types::zip::ZipLoad,
}

impl Dehumidifier {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
                name: config.name,
                end_use: EndUse::DEHUMIDIFIER,
                equipment_type: Cow::Borrowed("Dehumidifier"),
                zone: Some(zone),
                fuel: FuelType::Electric,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::HUMIDITY_SETPOINT
                    | ControlCapabilities::MODE_OVERRIDE,
                core_capabilities: CoreCapabilities::ELECTRIC
                    | CoreCapabilities::HAS_MODE
                    | CoreCapabilities::REACTIVE,
                telemetry_fields: telemetry_fields(),
                zone_type: None,
            },
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
                PortDeclaration::humidity(zone),
            ],
            telemetry: default_telemetry(),
            core_output: CoreOutput::default(),
            zone_id: zone,
            operating_mode: OperatingMode::Off,
            is_on: false,
            mode_override: None,
            rated_water_removal_l_day: DEFAULT_RATED_WATER_REMOVAL_L_DAY,
            rated_energy_factor_l_kwh: 1.8,
            fraction_load_served: 1.0,
            target_rh: DEFAULT_TARGET_RH_FRACTION,
            min_rh: DEFAULT_TARGET_RH_FRACTION - DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION,
            max_rh: DEFAULT_TARGET_RH_FRACTION + DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION,
            water_removal_curve: BiquadraticCurve {
                coeffs: DEFAULT_NORMALIZED_CURVE,
                x1_bounds: DEFAULT_DB_BOUNDS_C,
                x2_bounds: DEFAULT_RH_BOUNDS,
                warn_on_clamp: false,
                output_min: None,
                output_max: None,
            },
            energy_factor_curve: BiquadraticCurve {
                coeffs: DEFAULT_NORMALIZED_CURVE,
                x1_bounds: DEFAULT_DB_BOUNDS_C,
                x2_bounds: DEFAULT_RH_BOUNDS,
                warn_on_clamp: false,
                output_min: None,
                output_max: None,
            },
            water_removal_curve_rated_value: 1.0,
            energy_factor_curve_rated_value: 1.0,
            accumulated_water_removal_l: 0.0,
            part_load_curve_coeffs: DEFAULT_PLF_CURVE_COEFFS,
            plf_min: DEFAULT_PLF_MIN,
            zone_id_explicit,
            off_cycle_parasitic_load_w: None,
            min_operating_temp_c: Some(DEFAULT_MIN_OPERATING_TEMP_C),
            max_operating_temp_c: Some(DEFAULT_MAX_OPERATING_TEMP_C),
            zip: hares_types::zip::ZipLoad::constant_power(),
        }
    }

    fn update_is_on(&mut self, current_rh: f64) {
        let maybe_override = self.mode_override;
        self.is_on = match maybe_override {
            Some(OperatingMode::Off) | Some(OperatingMode::Standby) => false,
            Some(OperatingMode::Cooling) => true,
            Some(_) | None => {
                if self.is_on {
                    current_rh >= self.min_rh
                } else {
                    current_rh > self.max_rh
                }
            }
        };
        self.operating_mode = if self.is_on {
            OperatingMode::Cooling
        } else {
            OperatingMode::Off
        };
    }

    fn performance_snapshot(&self, zone_temp_c: f64, zone_rh: f64) -> PerformanceSnapshot {
        if !self.is_on {
            let parasitic = self.off_cycle_parasitic_load_w.unwrap_or(0.0);
            return PerformanceSnapshot {
                water_removal_l_day: 0.0,
                electric_power_w: parasitic,
                latent_removal_w: 0.0,
                sensible_gain_w: parasitic,
                plr: 0.0,
                plf: 1.0,
                rtf: 0.0,
                parasitic_electric_w: parasitic,
            };
        }

        // ---- PLR: part-load ratio ---------------------------------------
        // EnergyPlus CalcZoneDehumidifier lines 726–731:
        //   PLR = max(0.0, min(1.0, -QZnDehumidReq / WaterRemovalMassRate))
        //
        // Under normal operation (no mode override) HARES approximates the
        // zone moisture load as proportional to the excess relative humidity
        // above the switch-off threshold, normalised by the deadband width.
        // When mode_override forces Cooling, the unit runs at full capacity
        // (PLR = 1.0) regardless of the current zone humidity.
        let plr = if matches!(self.mode_override, Some(OperatingMode::Cooling)) {
            1.0
        } else {
            let deadband_width = (self.max_rh - self.min_rh).max(1e-12);
            ((zone_rh - self.min_rh) / deadband_width).clamp(0.0, 1.0)
        };

        // ---- PLF: part-load factor (cycling efficiency) ------------------
        // EnergyPlus CalcZoneDehumidifier lines 763–767:
        //   PLF = PartLoadCurve->value(PLR)   if curve present
        //   PLF = 1.0                         otherwise (no degradation)
        //
        // HARES defaults to the identity cubic [1,0,0,0] giving PLF = 1.0 at
        // all PLR, matching the EnergyPlus default. Users can supply custom
        // coefficients via DehumidifierConfig::part_load_curve_coeffs.
        let plf_raw = cubic(&self.part_load_curve_coeffs, plr);

        // EnergyPlus CalcZoneDehumidifier lines 769–808: clamps PLF to
        // [0.7, 1.0] and then separately handles PLF < PLR by resetting
        // RTF to 1.0.  HARES deviates from that sequential strategy by
        // instead clamping PLF to [plf_min.max(plr), 1.0], guaranteeing
        // PLF ≥ PLR structurally so RTF ≤ 1.0 without a downstream branch.
        let plf = plf_raw.clamp(self.plf_min.max(plr), DEFAULT_PLF_MAX);

        // ---- RTF: runtime fraction --------------------------------------
        // EnergyPlus CalcZoneDehumidifier lines 810–831:
        //   if (PLF > 0.0 && PLF >= PLR) RunTimeFraction = PLR / PLF
        //   else                         RunTimeFraction = 1.0
        let rtf = if plf > 0.0 && plf >= plr {
            (plr / plf).clamp(0.0, 1.0)
        } else {
            1.0
        };

        let wr_multiplier = evaluate_normalized_curve(
            &self.water_removal_curve,
            self.water_removal_curve_rated_value,
            zone_temp_c,
            zone_rh,
        );
        let ef_multiplier = evaluate_normalized_curve(
            &self.energy_factor_curve,
            self.energy_factor_curve_rated_value,
            zone_temp_c,
            zone_rh,
        );

        let water_removal_l_day =
            (self.rated_water_removal_l_day * self.fraction_load_served * wr_multiplier).max(0.0);
        let energy_factor_l_kwh = (self.rated_energy_factor_l_kwh * ef_multiplier).max(0.0);

        // EnergyPlus CalcZoneDehumidifier lines 850, 852: average electric power
        // is on-cycle power (ElectricPowerOnCycle, line 850) scaled by runtime
        // fraction (RTF) as part of the blend at line 852.
        // EnergyPlus CalcZoneDehumidifier line 855: latent (moisture) output
        // is scaled by PLR, not RTF — the two scalars are independent.
        let water_removal_kg_s = water_removal_l_day * KG_PER_LITER_WATER / SECONDS_PER_DAY;
        let electric_power_w_on = if energy_factor_l_kwh > 0.0 {
            water_removal_kg_s * WATTS_PER_KILOWATT_HOUR / energy_factor_l_kwh
        } else {
            0.0
        };
        // EnergyPlus CalcZoneDehumidifier lines 852, 880–886: off-cycle
        // parasitic load is applied to the off-cycle fraction (1 - RTF).
        // The on-cycle rated power (via energy factor) already includes
        // parasitic implicitly, so parasitic is only added to the off-cycle
        // portion to avoid double-counting.
        let parasitic = self.off_cycle_parasitic_load_w.unwrap_or(0.0);
        let electric_power_w = electric_power_w_on * rtf + parasitic * (1.0 - rtf);
        let water_removal_kg_s_avg = water_removal_kg_s * plr;
        let latent_removal_w = water_removal_kg_s_avg * LATENT_HEAT_VAPORISATION_0C_J_KG;
        let sensible_gain_w = latent_removal_w + electric_power_w;

        PerformanceSnapshot {
            water_removal_l_day: water_removal_l_day * plr,
            electric_power_w,
            latent_removal_w,
            sensible_gain_w,
            plr,
            plf,
            rtf,
            // EnergyPlus ZoneDehumidifier.cc:901: the reported parasitic power
            // is the off-cycle fraction of the configured load, matching the
            // parasitic term already blended into electric_power_w above.
            parasitic_electric_w: parasitic * (1.0 - rtf),
        }
    }

    fn write_step_telemetry(&mut self, snapshot: PerformanceSnapshot) {
        self.telemetry
            .set(tk::WATER_REMOVAL_L_DAY, snapshot.water_removal_l_day);
        self.telemetry
            .set(tk::ELECTRIC_POWER_W, snapshot.electric_power_w);
        self.telemetry
            .set(tk::ELECTRIC_KW, power_w_to_kw(snapshot.electric_power_w));
        self.telemetry
            .set(tk::LATENT_REMOVAL_W, snapshot.latent_removal_w);
        self.telemetry
            .set(tk::SENSIBLE_GAIN_W, snapshot.sensible_gain_w);
        let water_removal_kg_s =
            snapshot.water_removal_l_day * KG_PER_LITER_WATER / SECONDS_PER_DAY;
        self.telemetry
            .set(tk::MOISTURE_MASS_FLOW_KG_S, -water_removal_kg_s);
        self.telemetry.set(tk::TARGET_RH, self.target_rh);
        self.telemetry.set(tk::MIN_RH, self.min_rh);
        self.telemetry.set(tk::MAX_RH, self.max_rh);
        self.telemetry
            .set(tk::IS_ON, if self.is_on { 1.0 } else { 0.0 });
        self.telemetry.set(tk::PART_LOAD_RATIO, snapshot.plr);
        self.telemetry.set(tk::PART_LOAD_FACTOR, snapshot.plf);
        self.telemetry.set(tk::RUNTIME_FRACTION, snapshot.rtf);
        self.telemetry
            .set(tk::PARASITIC_ELECTRIC_W, snapshot.parasitic_electric_w);
    }
}

impl Dehumidifier {
    fn init_from_typed(&mut self, config: &EquipmentConfig) -> crate::Result<()> {
        let cfg = config.require_typed::<DehumidifierConfig>("Dehumidifier")?;
        cfg.validate()?;

        self.rated_water_removal_l_day = cfg
            .capacity_liters_per_day
            .unwrap_or(DEFAULT_RATED_WATER_REMOVAL_L_DAY);

        if cfg.integrated_energy_factor.is_some() {
            tracing::warn!(
                equipment = %self.descriptor.name,
                "integrated_energy_factor (IEF, composite rating per 10 CFR Part 430 \
                 Appendix X1) is not supported; only the single-condition Energy Factor \
                 (EF, AHAM DH-1-2008) model is implemented. IEF value will be ignored."
            );
        }
        self.rated_energy_factor_l_kwh = cfg.energy_factor.unwrap_or(1.8);

        self.fraction_load_served = cfg.fraction_served.unwrap_or(1.0).clamp(0.0, 1.0);

        self.part_load_curve_coeffs = cfg
            .part_load_curve_coeffs
            .unwrap_or(DEFAULT_PLF_CURVE_COEFFS);
        self.plf_min = cfg
            .plf_min
            .map(|v| v.clamp(0.0, 1.0))
            .unwrap_or(DEFAULT_PLF_MIN);

        self.off_cycle_parasitic_load_w = cfg.off_cycle_parasitic_load_w;

        if cfg.min_operating_temp_c.is_some() {
            self.min_operating_temp_c = cfg.min_operating_temp_c;
        }
        if cfg.max_operating_temp_c.is_some() {
            self.max_operating_temp_c = cfg.max_operating_temp_c;
        }

        let target_rh_raw = cfg.target_rh.unwrap_or(DEFAULT_TARGET_RH_FRACTION);
        self.target_rh = parse_rh_fraction(target_rh_raw, "target_rh")?;
        self.min_rh = (self.target_rh - DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION)
            .clamp(RH_MIN_FRACTION, RH_MAX_FRACTION);
        self.max_rh = (self.target_rh + DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION)
            .clamp(RH_MIN_FRACTION, RH_MAX_FRACTION);

        // Use default dehumidifier biquadratic curves sourced from EnergyPlus
        // ZoneHVAC:Dehumidifier:DX (WindACRHControl.idf), adapted from RH-% to
        // RH-fraction domain.  Identity [1,0,0,0,0,0] is available as a
        // last-resort fallback in new() for the pre-init state.
        self.water_removal_curve = BiquadraticCurve {
            coeffs: DEFAULT_WATER_REMOVAL_CURVE,
            x1_bounds: DEFAULT_DB_BOUNDS_C,
            x2_bounds: DEFAULT_RH_BOUNDS,
            warn_on_clamp: false,
            output_min: None,
            output_max: None,
        };
        self.energy_factor_curve = BiquadraticCurve {
            coeffs: DEFAULT_ENERGY_FACTOR_CURVE,
            x1_bounds: DEFAULT_DB_BOUNDS_C,
            x2_bounds: DEFAULT_RH_BOUNDS,
            warn_on_clamp: false,
            output_min: None,
            output_max: None,
        };
        // Compute normalisation divisors at the EnergyPlus rated condition
        // (26.7°C / 60 %RH) so that rated_capacity_liters_per_day passes
        // through unchanged at the design point.  Both curves are normalised
        // to ≈1.0 at the rating point; the evaluated value self-corrects for
        // any minor deviation.
        self.water_removal_curve_rated_value =
            self.water_removal_curve.evaluate(RATED_DB_C, RATED_RH);
        self.energy_factor_curve_rated_value =
            self.energy_factor_curve.evaluate(RATED_DB_C, RATED_RH);

        #[cfg(feature = "observe")]
        {
            tracing::debug!(
                equipment = %self.descriptor.name,
                rating_type = "EF",
                normalisation_temperature_c = RATED_DB_C,
                normalisation_rh = RATED_RH,
                "dehumidifier using single-condition Energy Factor (EF) model; \
                 normalised at AHAM DH-1-2008 rated condition (26.7°C, 60% RH)"
            );
        }

        Ok(())
    }

    /// Check PLF/RTF invariant bounds.
    ///
    /// EnergyPlus CalcZoneDehumidifier lines 769–808 require
    /// `0.7 ≤ PLF ≤ 1.0` and `0 ≤ RTF ≤ 1`.  Gated behind
    /// `debug_assertions` or `feature = "check_invariants"` so the
    /// check compiles to nothing in production release builds.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn check_invariants(&self, plf: f64, rtf: f64) -> crate::Result<()> {
        use hares_types::HaresError;
        if plf < self.plf_min || plf > 1.0 {
            return Err(HaresError::InvariantViolation {
                check_name: "dehumidifier_plf_bounds".to_string(),
                value: plf,
                tolerance: 0.0,
            });
        }
        if !(0.0..=1.0).contains(&rtf) {
            return Err(HaresError::InvariantViolation {
                check_name: "dehumidifier_rtf_bounds".to_string(),
                value: rtf,
                tolerance: 0.0,
            });
        }
        Ok(())
    }

    /// Stub for unchecked builds — the body is eliminated by the compiler.
    #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
    fn check_invariants(&self, _plf: f64, _rtf: f64) -> crate::Result<()> {
        Ok(())
    }
}

impl Equipment for Dehumidifier {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn zone_id_explicit(&self) -> bool {
        self.zone_id_explicit
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, _env: &EnvironmentState) -> crate::Result<()> {
        let equipment_id = equipment_id_from_config(config)?;
        self.descriptor.id = EquipmentId(equipment_id);
        let new_zone = zone_id_from_config(config);
        let explicit = new_zone.is_some();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        if !explicit {
            tracing::warn!(
                equipment = %self.descriptor.name,
                key = crate::config::KEY_ZONE_ID,
                "zone_id not present in equipment config during init; preserving existing zone_id"
            );
        }
        self.zone_id_explicit = explicit;
        self.zone_id = new_zone.unwrap_or(self.zone_id);
        self.descriptor.zone = Some(self.zone_id);
        self.ports = vec![
            PortDeclaration::electrical(),
            PortDeclaration::thermal(self.zone_id),
            PortDeclaration::humidity(self.zone_id),
        ];

        self.init_from_typed(config)?;

        self.operating_mode = OperatingMode::Off;
        self.is_on = false;
        self.mode_override = None;
        self.accumulated_water_removal_l = 0.0;
        self.zip = crate::config::resolve_reactive_zip(config)?;
        self.telemetry = default_telemetry();
        self.core_output = CoreOutput::default();
        self.write_step_telemetry(PerformanceSnapshot {
            water_removal_l_day: 0.0,
            electric_power_w: 0.0,
            latent_removal_w: 0.0,
            sensible_gain_w: 0.0,
            plr: 0.0,
            plf: 1.0,
            rtf: 0.0,
            parasitic_electric_w: 0.0,
        });
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        // Grid outage: a de-energized bus removes the compressor/fan supply —
        // force off before humidistat control (WH precedent). Humidistat
        // hysteresis resumes once power returns. Islanded homes keep an
        // energized bus and are not affected. See docs/outage-behavior.md.
        if !env.grid.bus_energized() {
            self.is_on = false;
            self.operating_mode = OperatingMode::Off;
            return OperatingMode::Off;
        }
        let pressure_pa = env.weather.pressure_pa();
        let zone = env.zones.iter().find(|z| z.id == self.zone_id);
        if let Some(zone_state) = zone {
            // Inlet air temperature operating limits per EnergyPlus
            // ZoneDehumidifier.cc:687–688: lock out the compressor when
            // the inlet air temperature is outside [MinInletAirTemp, MaxInletAirTemp].
            // Low-temperature lockout prevents evaporator freeze-up;
            // high-temperature lockout provides compressor thermal protection.
            let temp_locked_out = match (self.min_operating_temp_c, self.max_operating_temp_c) {
                (Some(min), _) if zone_state.temperature_c < min => {
                    #[cfg(feature = "observe")]
                    tracing::debug!(
                        equipment = %self.descriptor.name,
                        inlet_air_temp_c = zone_state.temperature_c,
                        min_operating_temp_c = min,
                        "dehumidifier locked out: inlet air temperature {:.1}°C below minimum {:.1}°C",
                        zone_state.temperature_c,
                        min,
                    );
                    true
                }
                (_, Some(max)) if zone_state.temperature_c > max => {
                    #[cfg(feature = "observe")]
                    tracing::debug!(
                        equipment = %self.descriptor.name,
                        inlet_air_temp_c = zone_state.temperature_c,
                        max_operating_temp_c = max,
                        "dehumidifier locked out: inlet air temperature {:.1}°C above maximum {:.1}°C",
                        zone_state.temperature_c,
                        max,
                    );
                    true
                }
                _ => false,
            };
            if temp_locked_out {
                self.is_on = false;
                self.operating_mode = OperatingMode::Off;
                return OperatingMode::Off;
            }
            let rh = hares_physics::psychrometrics::zone_relative_humidity(zone_state, pressure_pa);
            self.update_is_on(rh.clamp(RH_MIN_FRACTION, RH_MAX_FRACTION));
        } else {
            self.is_on = false;
            self.operating_mode = OperatingMode::Off;
        }
        self.operating_mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let zone = env
            .zones
            .iter()
            .find(|z| z.id == self.zone_id)
            .ok_or_else(|| HaresError::Equipment(format!("zone {} not found", self.zone_id.0)))?;

        let pressure_pa = env.weather.pressure_pa();
        let rh = hares_physics::psychrometrics::zone_relative_humidity(zone, pressure_pa);
        let raw = self.performance_snapshot(
            zone.temperature_c,
            rh.clamp(RH_MIN_FRACTION, RH_MAX_FRACTION),
        );
        // Grid outage guard: off-cycle parasitic load (standby electronics,
        // controls, crankcase heater) must not be drawn from a de-energized
        // bus — matches the tankless.rs (lines 455-459), resistance.rs
        // (lines 488-491), and heat_pump_wh.rs (lines 784-790) precedent.
        // update_control already forces is_on=false when the bus is
        // de-energized; this guard zeroes the parasitic contribution that
        // performance_snapshot would otherwise return for the off-cycle case.
        let snapshot = if !env.grid.bus_energized() {
            PerformanceSnapshot {
                electric_power_w: 0.0,
                sensible_gain_w: 0.0,
                parasitic_electric_w: 0.0,
                ..raw
            }
        } else {
            raw
        };
        self.check_invariants(snapshot.plf, snapshot.rtf)?;
        #[cfg(feature = "observe")]
        {
            tracing::debug!(
                dehumidifier = %self.descriptor.name,
                plr = snapshot.plr,
                plf = snapshot.plf,
                rtf = snapshot.rtf,
                "dehumidifier part-load cycle metrics"
            );
            if snapshot.rtf > snapshot.plr {
                tracing::debug!(
                    dehumidifier = %self.descriptor.name,
                    plf = snapshot.plf,
                    plr = snapshot.plr,
                    rtf = snapshot.rtf,
                    "rtf > plr: cycling losses applied"
                );
            }
            if let Some(parasitic) = self.off_cycle_parasitic_load_w {
                if !self.is_on {
                    let parasitic_energy_j = parasitic * dt.as_secs_f64();
                    tracing::debug!(
                        dehumidifier = %self.descriptor.name,
                        off_cycle_parasitic_w = parasitic,
                        parasitic_energy_j,
                        "dehumidifier off-cycle parasitic load active: {parasitic} W, accumulated {parasitic_energy_j} J this step"
                    );
                }
            }
        }
        // Rule R1: Q from the already-computed real power (whole-unit pf
        // 0.96). The dehumidifier is a sealed unit whose energy-factor model
        // never splits the small internal fan from the compressor, so the
        // single-component whole-unit pf is deliberate (unlike ducted HVAC,
        // which computes Q per component — see `hvac::reactive`).
        let reactive_power_kvar = self.zip.reactive_kvar(
            power_w_to_kw(snapshot.electric_power_w),
            env.grid.bus_voltage_pu(),
        );
        if snapshot.electric_power_w > 0.0 || reactive_power_kvar != 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: snapshot.electric_power_w,
                reactive_power_kvar,
            })?;
        }
        if snapshot.sensible_gain_w != 0.0 || snapshot.latent_removal_w != 0.0 {
            ports.accumulate(&PortContribution::Thermal {
                zone: self.zone_id,
                sensible_gain_w: snapshot.sensible_gain_w,
                radiant_gain_w: 0.0,
                latent_gain_w: -snapshot.latent_removal_w,
                category: ThermalCategory::HvacDehumidification,
            })?;
        }

        let water_removal_kg_s =
            snapshot.water_removal_l_day * KG_PER_LITER_WATER / SECONDS_PER_DAY;
        if water_removal_kg_s.abs() > 0.0 {
            ports.accumulate(&PortContribution::Humidity {
                zone: self.zone_id,
                moisture_mass_flow_kg_s: -water_removal_kg_s,
            })?;
        }

        let water_removed_l = snapshot.water_removal_l_day * dt.as_secs_f64() / SECONDS_PER_DAY;
        self.accumulated_water_removal_l += water_removed_l.max(0.0);
        let electric_kw = power_w_to_kw(snapshot.electric_power_w).max(0.0);
        self.write_step_telemetry(snapshot);
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, reactive_power_kvar);
        let temp_locked_out = match (self.min_operating_temp_c, self.max_operating_temp_c) {
            (Some(min), _) if zone.temperature_c < min => true,
            (_, Some(max)) if zone.temperature_c > max => true,
            _ => false,
        };
        self.telemetry.set(
            tk::TEMPERATURE_LOCKOUT,
            if temp_locked_out { 1.0 } else { 0.0 },
        );
        self.telemetry.set(tk::INLET_AIR_TEMP_C, zone.temperature_c);
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(electric_kw)),
                reactive_power_kvar: Some(reactive_power_kvar),
                fuel_w: None,
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(self.operating_mode),
                soc: None,
                speed_index: None,
                setpoint_c: None,
            },
            performance: CorePerformance::default(),
        };
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn resolved_zip(&self) -> Option<hares_types::zip::ZipLoad> {
        Some(self.zip)
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &DehumidifierState {
                is_on: self.is_on,
                accumulated_water_removal_l: self.accumulated_water_removal_l,
                target_rh: self.target_rh,
                min_rh: self.min_rh,
                max_rh: self.max_rh,
                mode_override: self.mode_override,
            },
            Self::checkpoint_version(),
            "Dehumidifier",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: DehumidifierState = load_versioned(
            state,
            Self::checkpoint_version(),
            "Dehumidifier",
            self.descriptor().id,
        )?;
        self.is_on = decoded.is_on;
        self.accumulated_water_removal_l = decoded.accumulated_water_removal_l;
        self.target_rh = decoded.target_rh;
        self.min_rh = decoded.min_rh;
        self.max_rh = decoded.max_rh;
        self.mode_override = decoded.mode_override;
        self.operating_mode = if self.is_on {
            OperatingMode::Cooling
        } else {
            OperatingMode::Off
        };
        self.write_step_telemetry(PerformanceSnapshot {
            water_removal_l_day: self.telemetry.get(tk::WATER_REMOVAL_L_DAY).unwrap_or(0.0),
            electric_power_w: self.telemetry.get(tk::ELECTRIC_POWER_W).unwrap_or(0.0),
            latent_removal_w: self.telemetry.get(tk::LATENT_REMOVAL_W).unwrap_or(0.0),
            sensible_gain_w: self.telemetry.get(tk::SENSIBLE_GAIN_W).unwrap_or(0.0),
            plr: 0.0,
            plf: 1.0,
            rtf: 0.0,
            parasitic_electric_w: self.telemetry.get(tk::PARASITIC_ELECTRIC_W).unwrap_or(0.0),
        });
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn apply_signal(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::HumiditySetpoint {
                target_rh,
                min_rh,
                max_rh,
            } => {
                let target = parse_rh_fraction(*target_rh, "target_rh")?;
                let min = (*min_rh)
                    .map(|v| parse_rh_fraction(v, KEY_MIN_RH))
                    .transpose()?;
                let max = (*max_rh)
                    .map(|v| parse_rh_fraction(v, KEY_MAX_RH))
                    .transpose()?;
                let (min, max) = resolve_rh_band(target, min, max)?;
                self.target_rh = target;
                self.min_rh = min;
                self.max_rh = max;
                self.telemetry.set(tk::TARGET_RH, self.target_rh);
                self.telemetry.set(tk::MIN_RH, self.min_rh);
                self.telemetry.set(tk::MAX_RH, self.max_rh);
            }
            ControlSignal::ModeOverride { mode } => {
                self.mode_override = Some(*mode);
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Dehumidifier",
        Box::new(|config| Box::new(Dehumidifier::new(config))),
    );
}

fn parse_rh_fraction(value: f64, field: &str) -> crate::Result<f64> {
    if !value.is_finite() {
        return Err(HaresError::Equipment(format!("{field} must be finite")));
    }
    let normalized = if value > RH_MAX_FRACTION && value <= 100.0 {
        value / 100.0
    } else {
        value
    };
    if !(RH_MIN_FRACTION..=RH_MAX_FRACTION).contains(&normalized) {
        return Err(HaresError::Equipment(format!(
            "{field} must be in [0,1] fraction (or [0,100] percent), got {value}"
        )));
    }
    Ok(normalized)
}

fn resolve_rh_band(
    target_rh: f64,
    min_rh: Option<f64>,
    max_rh: Option<f64>,
) -> crate::Result<(f64, f64)> {
    let min = min_rh.unwrap_or(target_rh - DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION);
    let max = max_rh.unwrap_or(target_rh + DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION);
    if !min.is_finite() || !max.is_finite() {
        return Err(HaresError::Equipment(
            "min_rh and max_rh must be finite".to_string(),
        ));
    }
    if min >= max {
        return Err(HaresError::Equipment(format!(
            "invalid RH deadband: min_rh ({min}) must be < max_rh ({max})"
        )));
    }
    Ok((
        min.clamp(RH_MIN_FRACTION, RH_MAX_FRACTION),
        max.clamp(RH_MIN_FRACTION, RH_MAX_FRACTION),
    ))
}

fn evaluate_normalized_curve(
    curve: &BiquadraticCurve,
    rated_value: f64,
    dry_bulb_c: f64,
    rh_fraction: f64,
) -> f64 {
    let value = curve.evaluate(dry_bulb_c, rh_fraction);
    if !value.is_finite() || rated_value <= 0.0 || !rated_value.is_finite() {
        return 0.0;
    }
    (value / rated_value).max(0.0)
}

fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(17);
    telemetry.insert(tk::WATER_REMOVAL_L_DAY, 0.0);
    telemetry.insert(tk::ELECTRIC_POWER_W, 0.0);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    telemetry.insert(tk::LATENT_REMOVAL_W, 0.0);
    telemetry.insert(tk::SENSIBLE_GAIN_W, 0.0);
    telemetry.insert(tk::MOISTURE_MASS_FLOW_KG_S, 0.0);
    telemetry.insert(tk::TARGET_RH, DEFAULT_TARGET_RH_FRACTION);
    telemetry.insert(
        tk::MIN_RH,
        DEFAULT_TARGET_RH_FRACTION - DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION,
    );
    telemetry.insert(
        tk::MAX_RH,
        DEFAULT_TARGET_RH_FRACTION + DEFAULT_DEADBAND_HALF_WIDTH_RH_FRACTION,
    );
    telemetry.insert(tk::IS_ON, 0.0);
    telemetry.insert(tk::PART_LOAD_RATIO, 0.0);
    telemetry.insert(tk::PART_LOAD_FACTOR, 1.0);
    telemetry.insert(tk::RUNTIME_FRACTION, 0.0);
    telemetry.insert(tk::PARASITIC_ELECTRIC_W, 0.0);
    telemetry.insert(tk::TEMPERATURE_LOCKOUT, 0.0);
    telemetry.insert(tk::INLET_AIR_TEMP_C, 0.0);
    telemetry
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::WATER_REMOVAL_L_DAY.to_string(),
            unit: "L/day".to_string(),
            description: "Water removed from air at current conditions".to_string(),
        },
        TelemetryField {
            name: tk::ELECTRIC_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Compressor + fan electric power draw".to_string(),
        },
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Total electric draw (kW, for dwelling power aggregation)".to_string(),
        },
        TelemetryField {
            name: tk::REACTIVE_POWER_KVAR.to_string(),
            unit: "kVAR".to_string(),
            description: "Reactive power (positive = inductive/lagging), compressor pf 0.96"
                .to_string(),
        },
        TelemetryField {
            name: tk::LATENT_REMOVAL_W.to_string(),
            unit: "W".to_string(),
            description: "Latent cooling removed from zone air".to_string(),
        },
        TelemetryField {
            name: tk::SENSIBLE_GAIN_W.to_string(),
            unit: "W".to_string(),
            description: "Sensible heat gain dumped back to zone".to_string(),
        },
        TelemetryField {
            name: tk::MOISTURE_MASS_FLOW_KG_S.to_string(),
            unit: "kg/s".to_string(),
            description: "Moisture mass removal rate (negative = dehumidification)".to_string(),
        },
        TelemetryField {
            name: tk::TARGET_RH.to_string(),
            unit: "fraction".to_string(),
            description: "Active RH setpoint target".to_string(),
        },
        TelemetryField {
            name: tk::MIN_RH.to_string(),
            unit: "fraction".to_string(),
            description: "Deadband lower bound".to_string(),
        },
        TelemetryField {
            name: tk::MAX_RH.to_string(),
            unit: "fraction".to_string(),
            description: "Deadband upper bound".to_string(),
        },
        TelemetryField {
            name: tk::IS_ON.to_string(),
            unit: "bool".to_string(),
            description: "1 when compressor/fan are on, else 0".to_string(),
        },
        TelemetryField {
            name: tk::PART_LOAD_RATIO.to_string(),
            unit: "fraction".to_string(),
            description: "Part-load ratio: fraction of rated capacity needed".to_string(),
        },
        TelemetryField {
            name: tk::PART_LOAD_FACTOR.to_string(),
            unit: "fraction".to_string(),
            description: "Part-load factor: cycling efficiency correction (PLF_MIN ≤ PLF ≤ 1.0)"
                .to_string(),
        },
        TelemetryField {
            name: tk::RUNTIME_FRACTION.to_string(),
            unit: "fraction".to_string(),
            description: "Runtime fraction: PLR / PLF".to_string(),
        },
        TelemetryField {
            name: tk::PARASITIC_ELECTRIC_W.to_string(),
            unit: "W".to_string(),
            description: "Actual off-cycle parasitic electric draw (standby electronics, controls, crankcase heater); scaled by the off-cycle fraction and zero during grid outage".to_string(),
        },
        TelemetryField {
            name: tk::TEMPERATURE_LOCKOUT.to_string(),
            unit: "bool".to_string(),
            description: "1.0 when the dehumidifier is locked out by inlet air temperature limits; 0.0 otherwise".to_string(),
        },
        TelemetryField {
            name: tk::INLET_AIR_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Inlet air dry-bulb temperature at the dehumidifier inlet".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_physics::constants::LATENT_HEAT_VAPORISATION_0C_J_KG;
    use hares_types::{
        ControlSignal, EnvironmentState, ExecutionStage, GridState, HumidityAccumulator,
        OperatingMode, PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
        telemetry_keys as tk,
    };

    use super::{Dehumidifier, KG_PER_LITER_WATER, SECONDS_PER_DAY};
    use crate::{Equipment, EquipmentConfig, EquipmentRegistry};

    const TOLERANCE_REL: f64 = 1e-9;

    fn approx_eq(a: f64, b: f64) {
        let denom = b.abs().max(1e-12);
        let rel_err = (a - b).abs() / denom;
        assert!(
            rel_err <= TOLERANCE_REL,
            "values differ: left={a}, right={b}, rel_err={rel_err}"
        );
    }

    fn env(relative_humidity: f64) -> EnvironmentState {
        env_with_temp(26.666_666_666_7, relative_humidity)
    }

    fn env_with_temp(temperature_c: f64, relative_humidity: f64) -> EnvironmentState {
        // Compute humidity_ratio from desired RH so the computed accessor
        // zone_relative_humidity() returns the intended value.
        let pressure_pa = 101_325.0;
        let p_sat = hares_physics::psychrometrics::saturation_pressure_pa(temperature_c);
        let p_v = (relative_humidity * p_sat).clamp(0.0, p_sat * 0.9999);
        let humidity_ratio = if p_v > 0.0 {
            hares_physics::psychrometrics::EPSILON * p_v / (pressure_pa - p_v)
        } else {
            0.0
        };
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c,
                humidity_ratio,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 30.0,
                outdoor_humidity_ratio: 0.012,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 20.0,
                sky_temp_c: 15.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                ..Default::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .unwrap(),
            time_res: ChronoDuration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn config() -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "Test Dehumidifier".to_string(),
            "Dehumidifier".to_string(),
            crate::DehumidifierConfig {
                equipment_id: Some(9),
                zone_id: Some(1),
                capacity_liters_per_day: Some(70.0 * 0.473_176_5),
                energy_factor: Some(2.0),
                integrated_energy_factor: None,
                fraction_served: None,
                target_rh: Some(50.0),
                part_load_curve_coeffs: None,
                plf_min: None,
                off_cycle_parasitic_load_w: None,
                min_operating_temp_c: None,
                max_operating_temp_c: None,
            },
        )
        .unwrap()
    }

    fn ports() -> PortSlots {
        PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        }
    }

    #[test]
    fn off_outputs_are_zero() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.40)).unwrap();

        let mut slots = ports();
        eq.step(&env(0.40), Duration::from_secs(60), &mut slots)
            .unwrap();
        assert_eq!(eq.update_control(&env(0.40)), OperatingMode::Off);
        assert_eq!(eq.telemetry().get(tk::WATER_REMOVAL_L_DAY), Some(0.0));
        assert_eq!(eq.telemetry().get(tk::ELECTRIC_POWER_W), Some(0.0));
        assert_eq!(eq.telemetry().get(tk::LATENT_REMOVAL_W), Some(0.0));
        assert_eq!(eq.telemetry().get(tk::SENSIBLE_GAIN_W), Some(0.0));
        assert_eq!(slots.electrical.load_power_w, 0.0);
        assert_eq!(slots.thermal[0].sensible_gain_w, 0.0);
        assert_eq!(slots.thermal[0].latent_gain_w, 0.0);
    }

    /// Grid outage (de-energized bus): compressor/fan have no supply —
    /// forced off at the control level even at high RH. Islanded homes keep
    /// dehumidifying; humidistat control resumes on restoration.
    #[test]
    fn grid_outage_forces_dehumidifier_off_and_islanded_home_keeps_running() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.60)).unwrap();

        // Baseline: high RH → runs.
        assert_eq!(eq.update_control(&env(0.60)), OperatingMode::Cooling);

        // Utility outage: forced off, zero outputs.
        let mut env_outage = env(0.60);
        env_outage.grid.voltage_pu = 0.0;
        assert_eq!(eq.update_control(&env_outage), OperatingMode::Off);
        let mut slots = ports();
        eq.step(&env_outage, Duration::from_secs(60), &mut slots)
            .unwrap();
        assert_eq!(slots.electrical.load_power_w, 0.0);
        assert_eq!(slots.thermal[0].sensible_gain_w, 0.0);
        assert_eq!(slots.thermal[0].latent_gain_w, 0.0);

        // Islanded: bus energized by a backup source → runs again.
        let mut env_islanded = env(0.60);
        env_islanded.grid.voltage_pu = 0.0;
        env_islanded.grid.island_bus_voltage_pu = Some(1.0);
        assert_eq!(eq.update_control(&env_islanded), OperatingMode::Cooling);

        // Restoration: humidistat control resumes.
        assert_eq!(eq.update_control(&env(0.60)), OperatingMode::Cooling);
    }

    /// Grid outage with off-cycle parasitic load configured: the parasitic
    /// standby draw must be zeroed when the bus is de-energized, matching
    /// the tankless.rs/resistance.rs/heat_pump_wh.rs precedent.
    /// Off-cycle parasitic loads rely on grid electricity; they have no
    /// source during an unpowered outage.
    #[test]
    fn parasitic_load_blocked_during_grid_outage() {
        let cfg = config_with_parasitic(Some(5.0));
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.40)).unwrap();

        // Baseline: off at low RH, parasitic draws 5.0 W.
        let mut slots_on = ports();
        eq.step(&env(0.40), Duration::from_secs(60), &mut slots_on)
            .unwrap();
        assert_eq!(slots_on.electrical.load_power_w, 5.0);
        assert_eq!(slots_on.thermal[0].sensible_gain_w, 5.0);
        assert_eq!(eq.telemetry().get(tk::PARASITIC_ELECTRIC_W), Some(5.0));

        // Utility outage: zero parasitic; no electrical or sensible output.
        let mut env_outage = env(0.40);
        env_outage.grid.voltage_pu = 0.0;
        assert_eq!(eq.update_control(&env_outage), OperatingMode::Off);
        let mut slots_outage = ports();
        eq.step(&env_outage, Duration::from_secs(60), &mut slots_outage)
            .unwrap();
        assert_eq!(slots_outage.electrical.load_power_w, 0.0);
        assert_eq!(slots_outage.thermal[0].sensible_gain_w, 0.0);
        assert_eq!(slots_outage.thermal[0].latent_gain_w, 0.0);
        // Telemetry must agree with the port contribution: no phantom
        // parasitic draw reported while the bus is de-energized.
        assert_eq!(eq.telemetry().get(tk::PARASITIC_ELECTRIC_W), Some(0.0));

        // Islanded: bus energized by backup → parasitic resumes.
        let mut env_islanded = env(0.40);
        env_islanded.grid.voltage_pu = 0.0;
        env_islanded.grid.island_bus_voltage_pu = Some(1.0);
        assert_eq!(eq.update_control(&env_islanded), OperatingMode::Off);
        let mut slots_islanded = ports();
        eq.step(&env_islanded, Duration::from_secs(60), &mut slots_islanded)
            .unwrap();
        assert_eq!(slots_islanded.electrical.load_power_w, 5.0);
        assert_eq!(slots_islanded.thermal[0].sensible_gain_w, 5.0);
        assert_eq!(eq.telemetry().get(tk::PARASITIC_ELECTRIC_W), Some(5.0));
    }

    #[test]
    fn deadband_turns_on_and_off_with_hold_behavior() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.40)).unwrap();

        // target=0.50 -> min=0.4875, max=0.5125
        assert_eq!(eq.update_control(&env(0.53)), OperatingMode::Cooling);
        assert_eq!(eq.update_control(&env(0.50)), OperatingMode::Cooling);
        assert_eq!(eq.update_control(&env(0.48)), OperatingMode::Off);
        assert_eq!(eq.update_control(&env(0.47)), OperatingMode::Off);
        assert_eq!(eq.update_control(&env(0.50)), OperatingMode::Off);
    }

    #[test]
    fn rated_condition_reproduces_rated_outputs() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.60)).unwrap();

        eq.update_control(&env(0.60));
        let mut slots = ports();
        eq.step(&env(0.60), Duration::from_secs(60), &mut slots)
            .unwrap();

        let water_l_day = eq.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap();
        let electric_power_w = eq.telemetry().get(tk::ELECTRIC_POWER_W).unwrap();
        let latent_removal_w = eq.telemetry().get(tk::LATENT_REMOVAL_W).unwrap();

        // 70 pints/day → liters/day (raw path converts from pints; uom uses 4.731765E-4 m³/pint = 0.4731765 L/pint)
        let rated_l_day = 70.0 * 0.473_176_5;
        approx_eq(water_l_day, rated_l_day);

        let expected_electric_w = (rated_l_day / SECONDS_PER_DAY) * 3_600_000.0 / 2.0;
        approx_eq(electric_power_w, expected_electric_w);
        let expected_latent_w = (rated_l_day / SECONDS_PER_DAY) * LATENT_HEAT_VAPORISATION_0C_J_KG;
        approx_eq(latent_removal_w, expected_latent_w);
    }

    #[test]
    fn energy_balance_and_ports_are_consistent() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.62)).unwrap();

        eq.update_control(&env(0.62));
        let mut slots = ports();
        eq.step(&env(0.62), Duration::from_secs(60), &mut slots)
            .unwrap();

        let electric_power_w = eq.telemetry().get(tk::ELECTRIC_POWER_W).unwrap();
        let latent_removal_w = eq.telemetry().get(tk::LATENT_REMOVAL_W).unwrap();
        let sensible_gain_w = eq.telemetry().get(tk::SENSIBLE_GAIN_W).unwrap();

        approx_eq(sensible_gain_w, latent_removal_w + electric_power_w);
        approx_eq(slots.electrical.load_power_w, electric_power_w);
        approx_eq(slots.thermal[0].sensible_gain_w, sensible_gain_w);
        approx_eq(slots.thermal[0].latent_gain_w, -latent_removal_w);
    }

    #[test]
    fn save_load_round_trip_preserves_state_and_subsequent_output() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.62)).unwrap();

        eq.update_control(&env(0.62));
        let mut slots_a = ports();
        eq.step(&env(0.62), Duration::from_secs(60), &mut slots_a)
            .unwrap();
        let state = eq.save_state().unwrap();

        let mut restored = Dehumidifier::new(cfg.clone());
        restored.init(&cfg, &env(0.62)).unwrap();
        restored.load_state(&state).unwrap();
        restored.update_control(&env(0.62));
        let mut slots_b = ports();
        restored
            .step(&env(0.62), Duration::from_secs(60), &mut slots_b)
            .unwrap();

        approx_eq(
            restored.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap(),
            eq.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap(),
        );
        approx_eq(
            restored.telemetry().get(tk::ELECTRIC_POWER_W).unwrap(),
            eq.telemetry().get(tk::ELECTRIC_POWER_W).unwrap(),
        );
        approx_eq(
            restored.telemetry().get(tk::LATENT_REMOVAL_W).unwrap(),
            eq.telemetry().get(tk::LATENT_REMOVAL_W).unwrap(),
        );
        approx_eq(
            restored.telemetry().get(tk::SENSIBLE_GAIN_W).unwrap(),
            eq.telemetry().get(tk::SENSIBLE_GAIN_W).unwrap(),
        );
    }

    #[test]
    fn control_signal_applies_setpoint_and_mode_override() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.45)).unwrap();
        eq.apply_control(&ControlSignal::HumiditySetpoint {
            target_rh: 0.55,
            min_rh: None,
            max_rh: None,
        })
        .unwrap();
        approx_eq(eq.telemetry().get(tk::TARGET_RH).unwrap(), 0.55);
        approx_eq(eq.telemetry().get(tk::MIN_RH).unwrap(), 0.5375);
        approx_eq(eq.telemetry().get(tk::MAX_RH).unwrap(), 0.5625);
        assert_eq!(eq.update_control(&env(0.58)), OperatingMode::Cooling);

        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();
        assert_eq!(eq.update_control(&env(0.90)), OperatingMode::Off);
    }

    #[test]
    fn registry_includes_dehumidifier_and_thermal_stage() {
        let registry = EquipmentRegistry::new();
        let eq = registry.create("Dehumidifier", config()).unwrap();
        assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
    }

    #[test]
    fn standby_mode_override_results_in_zero_power() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.90)).unwrap();

        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Standby,
        })
        .unwrap();

        let mode = eq.update_control(&env(0.90));
        assert_eq!(mode, OperatingMode::Off);
        assert_eq!(eq.telemetry().get(tk::IS_ON), Some(0.0));

        let mut slots = ports();
        eq.step(&env(0.90), Duration::from_secs(60), &mut slots)
            .unwrap();
        assert_eq!(eq.telemetry().get(tk::ELECTRIC_POWER_W), Some(0.0));
        assert_eq!(slots.electrical.load_power_w, 0.0);
    }

    #[test]
    fn cooling_mode_override_results_in_active_operation() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.40)).unwrap();

        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Cooling,
        })
        .unwrap();

        let mode = eq.update_control(&env(0.40));
        assert_eq!(mode, OperatingMode::Cooling);

        let mut slots = ports();
        eq.step(&env(0.40), Duration::from_secs(60), &mut slots)
            .unwrap();
        assert_eq!(eq.telemetry().get(tk::IS_ON), Some(1.0));
        assert!(eq.telemetry().get(tk::ELECTRIC_POWER_W).unwrap() > 0.0);
        assert!(slots.electrical.load_power_w > 0.0);
    }

    #[test]
    fn off_mode_override_results_in_no_operation() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.90)).unwrap();

        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();

        let mode = eq.update_control(&env(0.90));
        assert_eq!(mode, OperatingMode::Off);
        assert_eq!(eq.telemetry().get(tk::IS_ON), Some(0.0));

        let mut slots = ports();
        eq.step(&env(0.90), Duration::from_secs(60), &mut slots)
            .unwrap();
        assert_eq!(eq.telemetry().get(tk::ELECTRIC_POWER_W), Some(0.0));
        assert_eq!(slots.electrical.load_power_w, 0.0);
    }

    /// Regression test for ticket 001: the dehumidifier must use the same h_fg constant
    /// as the humidity solver so that moisture mass round-trips without systematic error.
    ///
    /// The dehumidifier writes `latent_gain_w = -water_removal_kg_s * h_fg_dehumidifier`
    /// to the thermal port. The humidity solver converts back via:
    ///   delta_w = latent_gain_w * dt / (h_fg_solver * rho * V)
    /// For the moisture mass to round-trip (delta_w * rho * V == water_removal_kg_s * dt)
    /// both h_fg values must be identical. With the current bug, h_fg_dehumidifier = 2_454_000
    /// but h_fg_solver = 2_501_000, creating a ~1.9% systematic error.
    ///
    /// This test FAILS until the fix in ticket 001 (Phase 1) is applied.
    #[test]
    fn dehumidifier_h_fg_matches_physics_constant() {
        use hares_physics::constants::LATENT_HEAT_VAPORISATION_0C_J_KG;

        // The h_fg used inside the dehumidifier's performance_snapshot is exposed
        // indirectly: for a known water_removal_kg_s, latent_removal_w / water_removal_kg_s
        // must equal LATENT_HEAT_VAPORISATION_0C_J_KG (2_501_000 J/kg).
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.60)).unwrap();

        eq.update_control(&env(0.60));
        let mut slots = ports();
        eq.step(&env(0.60), Duration::from_secs(60), &mut slots)
            .unwrap();

        let water_l_day = eq.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap();
        let latent_removal_w = eq.telemetry().get(tk::LATENT_REMOVAL_W).unwrap();

        // Recover the h_fg the dehumidifier actually used: h_fg = W / (kg/s)
        let water_removal_kg_s = water_l_day * KG_PER_LITER_WATER / SECONDS_PER_DAY;
        let implied_h_fg = latent_removal_w / water_removal_kg_s;

        assert!(
            (implied_h_fg - LATENT_HEAT_VAPORISATION_0C_J_KG).abs() < 1.0,
            "dehumidifier uses h_fg = {implied_h_fg:.0} J/kg but humidity solver uses \
             LATENT_HEAT_VAPORISATION_0C_J_KG = {LATENT_HEAT_VAPORISATION_0C_J_KG:.0} J/kg; \
             this creates a {:.2}% moisture mass balance error (ticket 001)",
            ((implied_h_fg - LATENT_HEAT_VAPORISATION_0C_J_KG).abs()
                / LATENT_HEAT_VAPORISATION_0C_J_KG)
                * 100.0
        );
    }

    /// Regression test: the dehumidifier writes its thermal contribution under
    /// `ThermalCategory::HvacDehumidification`, not `ThermalCategory::InternalGain`.
    ///
    /// A standalone dehumidifier is intentional mechanical conditioning equipment;
    /// attributing its sensible heat to InternalGain conflates mechanical output
    /// with passive gains (lighting, plug loads, occupants). EnergyPlus classifies
    /// the ZoneDehumidifier as zone HVAC equipment (Eng. Ref., Zone Equipment and
    /// Zone Forced Air Units).
    #[test]
    fn dehumidifier_thermal_category_is_hvac_not_internal_gain() {
        use hares_types::ThermalCategory;

        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.60)).unwrap();
        eq.update_control(&env(0.60));

        let mut slots = ports();
        eq.step(&env(0.60), Duration::from_secs(60), &mut slots)
            .unwrap();

        let sensible = eq.telemetry().get(tk::SENSIBLE_GAIN_W).unwrap();
        assert!(
            sensible > 0.0,
            "expected non-zero sensible gain but got {sensible}"
        );

        // InternalGain bucket must be zero — the dehumidifier is not a passive gain.
        let internal_gain_bucket =
            slots.thermal[0].sensible_for_category(ThermalCategory::InternalGain);
        assert_eq!(
            internal_gain_bucket, 0.0,
            "dehumidifier wrote {internal_gain_bucket} W to InternalGain bucket; \
             dehumidifier contributions must be under HvacDehumidification"
        );

        // The dehumidifier's sensible gain must appear under HvacDehumidification.
        let dehumidification_bucket =
            slots.thermal[0].sensible_for_category(ThermalCategory::HvacDehumidification);
        approx_eq(dehumidification_bucket, sensible);
    }

    /// A physically correct biquadratic curve must produce strictly less water
    /// removal at 10°C than at the EnergyPlus rated condition (26.7°C / 60% RH).
    #[test]
    fn water_removal_decreases_below_rated_temperature() {
        // Rated condition: 26.7°C / 60% RH — should give full capacity.
        let cfg = config();
        let mut eq_rated = Dehumidifier::new(cfg.clone());
        eq_rated.init(&cfg, &env(0.60)).unwrap();
        eq_rated.update_control(&env_with_temp(26.666_666_666_7, 0.60));
        let mut slots_rated = ports();
        eq_rated
            .step(
                &env_with_temp(26.666_666_666_7, 0.60),
                Duration::from_secs(60),
                &mut slots_rated,
            )
            .unwrap();
        let wr_rated = eq_rated.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap();

        // Cold condition: 10°C / 60% RH — refrigerant cycle is less effective,
        // so water removal must be strictly less than at rated conditions.
        // Disable temperature lockout for this test — it validates curve shape
        // at low temperature, not the operating limit gate.
        let mut eq_cold = Dehumidifier::new(cfg.clone());
        eq_cold.init(&cfg, &env(0.60)).unwrap();
        eq_cold.min_operating_temp_c = None;
        eq_cold.max_operating_temp_c = None;
        eq_cold
            .apply_control(&ControlSignal::ModeOverride {
                mode: OperatingMode::Cooling,
            })
            .unwrap();
        eq_cold.update_control(&env_with_temp(10.0, 0.60));
        let mut slots_cold = ports();
        eq_cold
            .step(
                &env_with_temp(10.0, 0.60),
                Duration::from_secs(60),
                &mut slots_cold,
            )
            .unwrap();
        let wr_cold = eq_cold.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap();

        assert!(
            wr_cold < wr_rated,
            "at 10°C water removal ({wr_cold:.4} L/day) must be less than at \
             26.7°C ({wr_rated:.4} L/day); identity curves mask all temperature dependence"
        );
    }

    /// At the EnergyPlus rated condition (26.7°C / 60% RH), the curve output
    /// normalised by `rated_value` must equal exactly 1.0 so that
    /// `rated_capacity_liters_per_day` passes through unchanged.
    #[test]
    fn water_removal_at_rated_condition_equals_rated_capacity() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.60)).unwrap();
        eq.update_control(&env_with_temp(26.666_666_666_7, 0.60));
        let mut slots = ports();
        eq.step(
            &env_with_temp(26.666_666_666_7, 0.60),
            Duration::from_secs(60),
            &mut slots,
        )
        .unwrap();

        let wr = eq.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap();
        let rated = 70.0 * 0.473_176_5;
        let rel_err = (wr - rated).abs() / rated;
        assert!(
            rel_err < 1e-9,
            "water removal at rated condition must equal rated capacity \
             ({rated:.4} L/day) but got {wr:.4} L/day (rel_err={rel_err:.2e})"
        );
    }

    /// Default deadband is 2.5% total (±1.25%) at the default 50% RH target.
    /// With a half-width of 0.0125, min_rh = 0.4875 and max_rh = 0.5125.
    #[test]
    fn default_deadband_produces_correct_min_max_in_telemetry() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.40)).unwrap();

        let min = eq.telemetry().get(tk::MIN_RH).unwrap();
        let max = eq.telemetry().get(tk::MAX_RH).unwrap();
        approx_eq(min, 0.4875);
        approx_eq(max, 0.5125);
    }

    // ── PLF cycling model tests ──────────────────────────────────────────

    /// At continuous full load (PLR = 1.0), PLF must equal 1.0 and RTF must
    /// equal 1.0. The unit must produce the same water removal, electric power,
    /// latent removal, and sensible gain as it would under the pre-PLF binary-on
    /// model (i.e. no cycling degradation at full load).
    #[test]
    fn plf_at_full_load_is_identity() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.60)).unwrap();
        eq.update_control(&env(0.60));
        let mut slots = ports();
        eq.step(&env(0.60), Duration::from_secs(60), &mut slots)
            .unwrap();

        let plr = eq.telemetry().get(tk::PART_LOAD_RATIO).unwrap();
        let plf = eq.telemetry().get(tk::PART_LOAD_FACTOR).unwrap();
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap();

        approx_eq(plr, 1.0);
        approx_eq(plf, 1.0);
        approx_eq(rtf, 1.0);

        // Verify full-load output matches the rated capacity:
        // At PLR=1.0, PLF=1.0, RTF=1.0 the output must equal the pre-PLF
        // binary-on output.  The rated_condition_reproduces_rated_outputs test
        // already validates this path; here we assert that the PLF model
        // introduces identity scaling at full load.
        let water_l_day = eq.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap();
        let rated_l_day = 70.0 * 0.473_176_5;
        approx_eq(water_l_day, rated_l_day);
    }

    /// At part load (PLR ≈ 0.5), the default identity PLF curve gives 1.0.
    /// The runtime fraction is PLR / PLF = 0.5 / 1.0 = 0.5.
    #[test]
    fn plf_clamping_at_half_load() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.53)).unwrap();
        // Turn on at 0.53 (above max_rh=0.5125).
        eq.update_control(&env(0.53));
        // Now run at 0.50 (mid-band, unit stays on from hysteresis).
        eq.update_control(&env(0.50));
        let mut slots = ports();
        eq.step(&env(0.50), Duration::from_secs(60), &mut slots)
            .unwrap();

        let plr = eq.telemetry().get(tk::PART_LOAD_RATIO).unwrap();
        let plf = eq.telemetry().get(tk::PART_LOAD_FACTOR).unwrap();
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap();

        // PLR ≈ 0.5 because RH is at the midpoint of the deadband:
        // plr = (0.50 - 0.4875) / (0.5125 - 0.4875) = 0.0125 / 0.025 = 0.5
        approx_eq(plr, 0.5);

        // PLF at PLR=0.5 via identity cubic [1.0, 0.0, 0.0, 0.0]:
        // PLF = 1.0 for all PLR; clamped to [0.7, 1.0] → 1.0
        approx_eq(plf, 1.0);

        // RTF = PLR / PLF = 0.5 / 1.0 = 0.5
        approx_eq(rtf, 0.5);

        // Water removal at RTF=0.5: rated * 0.5 (plus curve factors at 0.50 RH)
        let wr_rated = 70.0 * 0.473_176_5;
        let wr_observed = eq.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap();
        // At 0.50 RH (vs rated 0.60 RH) the curve decreases output.
        // Water removal is scaled by PLR (0.5) per EnergyPlus line 858;
        // since PLF=1.0 and RTF=0.5 at this point the scalars coincide.
        assert!(
            wr_observed < wr_rated * 0.6,
            "water removal at half load ({wr_observed:.4}) should be well below \
             rated capacity ({wr_rated:.4})"
        );
        assert!(wr_observed > 0.0, "unit should still be removing moisture");
    }

    /// Verify PLF clamping at the PLR boundaries using a non-identity curve.
    ///
    /// - At PLR = 0, the cubic [0.7, 1.0, -0.7, 0.0] gives 0.7, clamped to [0.7, 1.0] → 0.7.
    /// - At PLR = 1.0, the same cubic gives 1.0, clamped to [1.0, 1.0] → 1.0.
    #[test]
    fn plf_clamping_at_boundaries() {
        // Analytical check of the default cubic curve.
        use hares_physics::biquadratic::cubic;
        let coeffs = [0.7, 1.0, -0.7, 0.0];

        // PLR = 0: cubic → 0.7, clamped to [0.7, 1.0] → 0.7
        let plf_at_0 = cubic(&coeffs, 0.0).clamp(0.7, 1.0);
        approx_eq(plf_at_0, 0.7);

        // PLR = 1.0: cubic → 0.7 + 1.0 - 0.7 = 1.0, clamped to [1.0, 1.0] → 1.0
        let plf_at_1 = cubic(&coeffs, 1.0).clamp(0.7, 1.0);
        approx_eq(plf_at_1, 1.0);

        // Verify through the dehumidifier at PLR=1.0 (rated conditions).
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.60)).unwrap();
        eq.update_control(&env(0.60));
        let mut slots = ports();
        eq.step(&env(0.60), Duration::from_secs(60), &mut slots)
            .unwrap();

        let plf = eq.telemetry().get(tk::PART_LOAD_FACTOR).unwrap();
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap();

        // At full load, PLF = 1.0 and RTF = 1.0.
        assert!(
            (plf - 1.0).abs() < 1e-9,
            "PLF at full load must be 1.0, got {plf}"
        );
        assert!(
            (rtf - 1.0).abs() < 1e-9,
            "RTF at full load must be 1.0, got {rtf}"
        );
    }

    /// PLR, PLF, and RTF telemetry fields must be present and populated
    /// after every step, even when the unit is off.
    #[test]
    fn plf_telemetry_fields_always_present() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.40)).unwrap();

        // Off state: PLR=0, PLF=1.0, RTF=0
        let mut slots = ports();
        eq.step(&env(0.40), Duration::from_secs(60), &mut slots)
            .unwrap();

        assert!(eq.telemetry().get(tk::PART_LOAD_RATIO).is_some());
        assert!(eq.telemetry().get(tk::PART_LOAD_FACTOR).is_some());
        assert!(eq.telemetry().get(tk::RUNTIME_FRACTION).is_some());

        approx_eq(eq.telemetry().get(tk::PART_LOAD_RATIO).unwrap(), 0.0);
        approx_eq(eq.telemetry().get(tk::PART_LOAD_FACTOR).unwrap(), 1.0);
        approx_eq(eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap(), 0.0);

        // On state: PLR > 0
        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Cooling,
        })
        .unwrap();
        eq.update_control(&env(0.60));
        eq.step(&env(0.60), Duration::from_secs(60), &mut slots)
            .unwrap();

        assert!(
            eq.telemetry().get(tk::PART_LOAD_RATIO).unwrap() > 0.0,
            "PLR should be > 0 when unit is on at high humidity"
        );
    }

    /// Regression test: compare HARES dehumidifier output against EnergyPlus
    /// `CalcZoneDehumidifier` for matched configurations. EnergyPlus uses
    /// independent scalars: PLR for moisture output (line 858) and RTF for
    /// electric power (line 855). This test verifies HARES follows the same
    /// pattern by using a custom PLF curve that creates PLR ≠ RTF divergence
    /// at part load, and asserts the correct scalar is applied to each output.
    ///
    /// Test points:
    /// 1. Full load (PLR=1.0): PLR=RTF=1.0, outputs match rated capacity.
    /// 2. Part load (PLR=0.5) with PLF=0.7: water removal scales by PLR (0.5),
    ///    electric power scales by RTF (≈0.7143). The ratio water_removal / PLR
    ///    must equal the full-load water removal, proving the PLR scalar is
    ///    applied to moisture and NOT the electric scalar (RTF).
    #[test]
    fn energyplus_regression_independent_plr_rtf_scalars() {
        // Why: Clippy fires `items_after_test_module` on inner function definitions
        // inside test functions because rustc treats them as module-level items even
        // when nested inside a function body. There is no way to move this helper
        // without duplicating the test or pulling it out of the test module.
        #[allow(clippy::items_after_test_module)]
        fn custom_eplus_dehumidifier_config(coeffs: Option<[f64; 4]>) -> EquipmentConfig {
            EquipmentConfig::from_typed(
                "E+ Regression".to_string(),
                "Dehumidifier".to_string(),
                crate::DehumidifierConfig {
                    equipment_id: Some(9),
                    zone_id: Some(1),
                    capacity_liters_per_day: Some(70.0 * 0.473_176_5),
                    energy_factor: Some(2.0),
                    integrated_energy_factor: None,
                    fraction_served: None,
                    target_rh: Some(50.0),
                    part_load_curve_coeffs: coeffs,
                    plf_min: None,
                    off_cycle_parasitic_load_w: None,
                    min_operating_temp_c: None,
                    max_operating_temp_c: None,
                },
            )
            .unwrap()
        }

        // ── Test 1: Full load (default curve, PLR=1.0) ──────────────────
        // EnergyPlus: PLR=1.0 → PLF=1.0 → RTF=1.0. All scalars unity.
        let cfg = custom_eplus_dehumidifier_config(None);
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.60)).unwrap();
        eq.update_control(&env(0.60));
        let mut slots = ports();
        eq.step(&env(0.60), Duration::from_secs(60), &mut slots)
            .unwrap();

        let plr = eq.telemetry().get(tk::PART_LOAD_RATIO).unwrap();
        let plf = eq.telemetry().get(tk::PART_LOAD_FACTOR).unwrap();
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap();
        approx_eq(plr, 1.0);
        approx_eq(plf, 1.0);
        approx_eq(rtf, 1.0);

        let wr_full_load = eq.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap();
        let rated_l_day = 70.0 * 0.473_176_5;
        approx_eq(wr_full_load, rated_l_day);

        // ── Test 2: Part load, PLF=0.7 constant curve ────────────────────
        // PLF = 0.7 + 0·PLR + 0·PLR² + 0·PLR³ = 0.7 for all PLR.
        // At PLR=0.5: RTF = 0.5/0.7 ≈ 0.7143.
        // Water removal MUST scale by PLR (0.5), NOT RTF (0.7143).
        // Electric power MUST scale by RTF (0.7143).
        let cfg2 = custom_eplus_dehumidifier_config(Some([0.7, 0.0, 0.0, 0.0]));
        let mut eq2 = Dehumidifier::new(cfg2.clone());
        eq2.init(&cfg2, &env(0.53)).unwrap();

        // Turn on above max_rh, then step at mid-band (hysteresis keeps it on).
        eq2.update_control(&env(0.53));
        eq2.update_control(&env(0.50));
        let mut slots2 = ports();
        eq2.step(&env(0.50), Duration::from_secs(60), &mut slots2)
            .unwrap();

        let plr2 = eq2.telemetry().get(tk::PART_LOAD_RATIO).unwrap();
        let plf2 = eq2.telemetry().get(tk::PART_LOAD_FACTOR).unwrap();
        let rtf2 = eq2.telemetry().get(tk::RUNTIME_FRACTION).unwrap();
        let wr_part = eq2.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap();
        let ep_part = eq2.telemetry().get(tk::ELECTRIC_POWER_W).unwrap();

        approx_eq(plr2, 0.5);
        approx_eq(plf2, 0.7);
        approx_eq(rtf2, 0.5 / 0.7);

        // Now force full-load (PLR=1.0) at the same ambient conditions
        // to get the base water removal and electric power at this RH.
        // At PLR=1.0, PLF is clamped to max(0.7, 1.0)=1.0, RTF=1.0.
        eq2.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Cooling,
        })
        .unwrap();
        eq2.update_control(&env(0.50));
        let mut slots_full = ports();
        eq2.step(&env(0.50), Duration::from_secs(60), &mut slots_full)
            .unwrap();
        let wr_base = eq2.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap();
        let ep_base = eq2.telemetry().get(tk::ELECTRIC_POWER_W).unwrap();

        // EnergyPlus line 858: LatentOutput = WaterRemovalMassRate * PLR
        // → water removal at part load = water removal at full load × PLR.
        approx_eq(wr_part, wr_base * plr2);
        // EnergyPlus line 855: ElectricPowerAvg = ElectricPowerOnCycle * RTF
        // → electric power at part load = electric power at full load × RTF.
        approx_eq(ep_part, ep_base * rtf2);

        // Negative control: prove the scalars differ
        // (this assertion would fail if both used the same scalar).
        assert!(
            (plr2 - rtf2).abs() > 1e-9,
            "PLR ({plr2}) and RTF ({rtf2}) must differ for the custom curve to exercise independent scalars"
        );
    }

    #[test]
    fn init_updates_zone_id_explicit_when_config_lacks_zone_id() {
        let cfg_with_zone = config();
        let mut dehu = Dehumidifier::new(cfg_with_zone);
        assert!(
            dehu.zone_id_explicit(),
            "new() with explicit zone_id must set zone_id_explicit = true"
        );

        let cfg_without_zone = EquipmentConfig::from_typed(
            "Test Dehumidifier".to_string(),
            "Dehumidifier".to_string(),
            crate::DehumidifierConfig {
                equipment_id: Some(9),
                zone_id: None,
                capacity_liters_per_day: Some(70.0 * 0.473_176_5),
                energy_factor: Some(2.0),
                integrated_energy_factor: None,
                fraction_served: None,
                target_rh: Some(50.0),
                part_load_curve_coeffs: None,
                plf_min: None,
                off_cycle_parasitic_load_w: None,
                min_operating_temp_c: None,
                max_operating_temp_c: None,
            },
        )
        .unwrap();
        dehu.init(&cfg_without_zone, &env(50.0))
            .expect("init must succeed");
        assert!(
            !dehu.zone_id_explicit(),
            "init() with absent zone_id must set zone_id_explicit = false"
        );
    }

    /// Reactive-power contract for the dehumidifier: compressor pf 0.96, Q/P =
    /// tan(acos(0.96)) at nominal voltage, REACTIVE declared, and
    /// port/CoreOutput/telemetry agree bit-for-bit. Off ⇒ Q == 0.
    #[test]
    fn dehumidifier_reactive_power_pf_and_channels_agree() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        let environment = env(0.60);
        eq.init(&cfg, &environment).unwrap();
        assert!(
            eq.descriptor()
                .core_capabilities
                .contains(hares_types::CoreCapabilities::REACTIVE),
            "dehumidifier must declare REACTIVE"
        );
        assert_eq!(eq.zip.pf, 0.96, "class default pf");
        eq.update_control(&environment);
        let mut slots = ports();
        eq.step(&environment, Duration::from_secs(60), &mut slots)
            .unwrap();

        let p_kw = slots.electrical.load_power_w / 1000.0;
        assert!(p_kw > 0.0, "dehumidifier must draw real power when running");
        let q = slots.electrical.reactive_power_kvar;
        let expected = p_kw * 0.96_f64.acos().tan();
        assert!(
            (q - expected).abs() < 1e-9,
            "Q/P must equal tan(acos(0.96)): q={q}, expected={expected}"
        );
        assert_eq!(
            eq.core_output()
                .flows
                .reactive_power_kvar
                .expect("Some")
                .to_bits(),
            q.to_bits(),
            "CoreOutput Q must equal port Q"
        );
        assert_eq!(
            eq.telemetry()
                .get(tk::REACTIVE_POWER_KVAR)
                .expect("telemetry Q")
                .to_bits(),
            q.to_bits(),
            "telemetry Q must equal port Q"
        );
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output())
            .expect("core contract must hold with REACTIVE declared");

        // Off case: RH below target ⇒ dehumidifier off ⇒ Q == 0.
        let off_env = env(0.40);
        eq.update_control(&off_env);
        let mut off_ports = ports();
        eq.step(&off_env, Duration::from_secs(60), &mut off_ports)
            .unwrap();
        assert_eq!(
            off_ports.electrical.reactive_power_kvar, 0.0,
            "off ⇒ Q == 0"
        );
        assert_eq!(
            eq.core_output().flows.reactive_power_kvar,
            Some(0.0),
            "off ⇒ CoreOutput Q == Some(0.0)"
        );
    }

    /// Rule R1 regression: the power factor affects only Q. Twin instances —
    /// one with the class pf 0.96, one with a constant-power sidecar override
    /// (pf 0 sentinel) — must produce bit-identical real power at every step
    /// and voltage.
    #[test]
    fn dehumidifier_real_power_bit_identical_with_and_without_reactive_zip() {
        let config_pf = config();
        let mut config_nopf = config_pf.clone();
        config_nopf.zip = Some(hares_types::zip::ZipLoad::constant_power());

        let mut eq_pf = Dehumidifier::new(config_pf.clone());
        let mut eq_nopf = Dehumidifier::new(config_nopf.clone());
        let mut environment = env(0.60);
        eq_pf.init(&config_pf, &environment).unwrap();
        eq_nopf.init(&config_nopf, &environment).unwrap();

        let mut any_reactive = false;
        for (i, v) in [1.0, 0.95, 1.05, 1.0, 0.9, 1.1].iter().enumerate() {
            environment.grid.voltage_pu = *v;
            eq_pf.update_control(&environment);
            eq_nopf.update_control(&environment);
            let mut ports_pf = ports();
            let mut ports_nopf = ports();
            eq_pf
                .step(&environment, Duration::from_secs(60), &mut ports_pf)
                .unwrap();
            eq_nopf
                .step(&environment, Duration::from_secs(60), &mut ports_nopf)
                .unwrap();
            assert_eq!(
                ports_pf.electrical.load_power_w.to_bits(),
                ports_nopf.electrical.load_power_w.to_bits(),
                "step {i} (v={v}): real power diverged between pf and no-pf twins"
            );
            assert_eq!(
                ports_nopf.electrical.reactive_power_kvar, 0.0,
                "pf-0 twin must produce zero reactive power"
            );
            if ports_pf.electrical.reactive_power_kvar != 0.0 {
                any_reactive = true;
            }
            environment.current_time += ChronoDuration::minutes(1);
        }
        assert!(
            any_reactive,
            "the pf 0.96 twin must produce reactive power while running"
        );
    }

    /// Evaluate PLF curve with known coefficients [0.5, 0.5, 0.0, 0.0]
    /// (PLF = 0.5 + 0.5·PLR) at PLR = 0.25, 0.50, 0.75, 1.0. Verify
    /// RTF = PLR / PLF and derated electric power = rated_power_on · RTF,
    /// and water removal scales by PLR independently.
    ///
    /// Uses `plf_min = 0.0` so the lower clamp does not interfere with
    /// PLR < 0.7 test points.
    #[test]
    fn plf_curve_known_coeffs_at_plr_points() {
        fn custom_config(coeffs: Option<[f64; 4]>, plf_min: Option<f64>) -> EquipmentConfig {
            EquipmentConfig::from_typed(
                "PLF Test".to_string(),
                "Dehumidifier".to_string(),
                crate::DehumidifierConfig {
                    equipment_id: Some(9),
                    zone_id: Some(1),
                    capacity_liters_per_day: Some(70.0 * 0.473_176_5),
                    energy_factor: Some(2.0),
                    integrated_energy_factor: None,
                    fraction_served: None,
                    target_rh: Some(50.0),
                    part_load_curve_coeffs: coeffs,
                    plf_min,
                    off_cycle_parasitic_load_w: None,
                    min_operating_temp_c: None,
                    max_operating_temp_c: None,
                },
            )
            .unwrap()
        }

        // PLF = 0.5 + 0.5·PLR (quadratic via cubic with C2=C3=0).
        let coeffs = [0.5, 0.5, 0.0, 0.0];

        // target_rh=0.50 → min_rh=0.4875, max_rh=0.5125, deadband=0.025.
        // PLR = (zone_rh - 0.4875) / 0.025

        let test_points: [(f64, f64, f64); 4] = [
            // (zone_rh, expected_plr, expected_plf)
            // PLR=0.25: rh = 0.4875 + 0.25*0.025 = 0.49375
            (0.49375, 0.25, 0.5 + 0.5 * 0.25),
            // PLR=0.50: rh = 0.4875 + 0.50*0.025 = 0.50000
            (0.50, 0.50, 0.5 + 0.5 * 0.50),
            // PLR=0.75: rh = 0.4875 + 0.75*0.025 = 0.50625
            (0.50625, 0.75, 0.5 + 0.5 * 0.75),
            // PLR=1.00: rh = 0.4875 + 1.00*0.025 = 0.51250
            // PLF = 0.5+0.5=1.0, clamped to [max(0.0, 1.0), 1.0] = [1.0, 1.0] → 1.0
            (0.5125, 1.0, 1.0),
        ];

        for (i, &(zone_rh, expected_plr, expected_plf)) in test_points.iter().enumerate() {
            let cfg = custom_config(Some(coeffs), Some(0.0));
            let mut eq = Dehumidifier::new(cfg.clone());
            eq.init(&cfg, &env(zone_rh)).unwrap();

            // Turn on by stepping above max_rh, then run at target.
            eq.update_control(&env(0.53));
            eq.update_control(&env(zone_rh));
            let mut slots = ports();
            eq.step(&env(zone_rh), Duration::from_secs(60), &mut slots)
                .unwrap();

            let plr = eq.telemetry().get(tk::PART_LOAD_RATIO).unwrap();
            let plf = eq.telemetry().get(tk::PART_LOAD_FACTOR).unwrap();
            let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap();
            let ep_part = eq.telemetry().get(tk::ELECTRIC_POWER_W).unwrap();
            let wr_part = eq.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap();

            approx_eq(plr, expected_plr);
            approx_eq(plf, expected_plf);

            let expected_rtf = (plr / plf).clamp(0.0, 1.0);
            approx_eq(rtf, expected_rtf);

            // Force full-load at same ambient to get base power / water removal.
            eq.apply_control(&ControlSignal::ModeOverride {
                mode: OperatingMode::Cooling,
            })
            .unwrap();
            eq.update_control(&env(zone_rh));
            let mut slots_full = ports();
            eq.step(&env(zone_rh), Duration::from_secs(60), &mut slots_full)
                .unwrap();
            let ep_base = eq.telemetry().get(tk::ELECTRIC_POWER_W).unwrap();
            let wr_base = eq.telemetry().get(tk::WATER_REMOVAL_L_DAY).unwrap();

            // Electric power scales by RTF (EnergyPlus line 855).
            approx_eq(ep_part, ep_base * rtf);
            // Water removal scales by PLR (EnergyPlus line 858).
            approx_eq(wr_part, wr_base * plr);

            assert!(
                ep_part > 0.0,
                "point {i} (PLR={expected_plr}): electric power must be > 0"
            );
            assert!(
                wr_part > 0.0,
                "point {i} (PLR={expected_plr}): water removal must be > 0"
            );
        }
    }

    /// PLF curve defaults to constant 1.0 when not provided by the user,
    /// producing RTF = PLR (no cycling loss).
    ///
    /// EnergyPlus ZoneDehumidifier.cc lines 763–767: when no PartLoadCurve
    /// is configured, PLF = 1.0.
    #[test]
    fn plf_curve_default_identity_gives_rtf_equals_plr() {
        let cfg = config(); // part_load_curve_coeffs: None → identity default
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.53)).unwrap();

        // At PLR ≈ 0.75 (rh = 0.50625), identity curve gives PLF = 1.0,
        // so RTF = 0.75 = PLR (no cycling loss).
        eq.update_control(&env(0.53));
        eq.update_control(&env(0.50625));
        let mut slots = ports();
        eq.step(&env(0.50625), Duration::from_secs(60), &mut slots)
            .unwrap();

        let plr = eq.telemetry().get(tk::PART_LOAD_RATIO).unwrap();
        let plf = eq.telemetry().get(tk::PART_LOAD_FACTOR).unwrap();
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap();

        approx_eq(plf, 1.0);
        approx_eq(rtf, plr);
        // RTF = PLR means no cycling penalty — the unit is derated only by PLR,
        // not by an additional PLF factor.
    }

    /// Providing `integrated_energy_factor` without `energy_factor` must
    /// fall back to the default EF (1.8 L/kWh) because IEF is not supported.
    ///
    /// At rated conditions (26.7°C / 60% RH, PLR=1.0, RTF=1.0), the
    /// electric power directly reflects the energy factor used. If IEF=3.0
    /// were accepted, power would be ~460 W; with the correct default EF=1.8,
    /// power is ~767 W — matching a control dehumidifier with no EF set.
    #[test]
    fn ief_only_falls_back_to_default_ef() {
        let cfg_ief_only = EquipmentConfig::from_typed(
            "IEF-only Test".to_string(),
            "Dehumidifier".to_string(),
            crate::DehumidifierConfig {
                equipment_id: Some(9),
                zone_id: Some(1),
                capacity_liters_per_day: Some(70.0 * 0.473_176_5),
                energy_factor: None,
                integrated_energy_factor: Some(3.0),
                fraction_served: None,
                target_rh: Some(50.0),
                part_load_curve_coeffs: None,
                plf_min: None,
                off_cycle_parasitic_load_w: None,
                min_operating_temp_c: None,
                max_operating_temp_c: None,
            },
        )
        .unwrap();
        let cfg_no_ef = EquipmentConfig::from_typed(
            "No-EF Control".to_string(),
            "Dehumidifier".to_string(),
            crate::DehumidifierConfig {
                equipment_id: Some(9),
                zone_id: Some(1),
                capacity_liters_per_day: Some(70.0 * 0.473_176_5),
                energy_factor: None,
                integrated_energy_factor: None,
                fraction_served: None,
                target_rh: Some(50.0),
                part_load_curve_coeffs: None,
                plf_min: None,
                off_cycle_parasitic_load_w: None,
                min_operating_temp_c: None,
                max_operating_temp_c: None,
            },
        )
        .unwrap();

        let mut eq_ief = Dehumidifier::new(cfg_ief_only.clone());
        eq_ief.init(&cfg_ief_only, &env(0.60)).unwrap();
        eq_ief.update_control(&env(0.60));
        let mut slots_ief = ports();
        eq_ief
            .step(&env(0.60), Duration::from_secs(60), &mut slots_ief)
            .unwrap();
        let power_ief = eq_ief.telemetry().get(tk::ELECTRIC_POWER_W).unwrap();

        let mut eq_none = Dehumidifier::new(cfg_no_ef.clone());
        eq_none.init(&cfg_no_ef, &env(0.60)).unwrap();
        eq_none.update_control(&env(0.60));
        let mut slots_none = ports();
        eq_none
            .step(&env(0.60), Duration::from_secs(60), &mut slots_none)
            .unwrap();
        let power_none = eq_none.telemetry().get(tk::ELECTRIC_POWER_W).unwrap();

        approx_eq(power_ief, power_none);
    }

    /// Providing both `energy_factor` and `integrated_energy_factor`
    /// must use `energy_factor` and ignore `integrated_energy_factor`.
    ///
    /// If IEF=3.0 were erroneously used, power would be ~460 W at rated
    /// conditions. With EF=2.0 correctly selected, power is ~690 W — matching
    /// a control dehumidifier with EF=2.0 and no IEF.
    #[test]
    fn both_ef_and_ief_uses_ef() {
        let cfg_both = EquipmentConfig::from_typed(
            "Both EF+IEF Test".to_string(),
            "Dehumidifier".to_string(),
            crate::DehumidifierConfig {
                equipment_id: Some(9),
                zone_id: Some(1),
                capacity_liters_per_day: Some(70.0 * 0.473_176_5),
                energy_factor: Some(2.0),
                integrated_energy_factor: Some(3.0),
                fraction_served: None,
                target_rh: Some(50.0),
                part_load_curve_coeffs: None,
                plf_min: None,
                off_cycle_parasitic_load_w: None,
                min_operating_temp_c: None,
                max_operating_temp_c: None,
            },
        )
        .unwrap();
        let cfg_ef_only = EquipmentConfig::from_typed(
            "EF-only Control".to_string(),
            "Dehumidifier".to_string(),
            crate::DehumidifierConfig {
                equipment_id: Some(9),
                zone_id: Some(1),
                capacity_liters_per_day: Some(70.0 * 0.473_176_5),
                energy_factor: Some(2.0),
                integrated_energy_factor: None,
                fraction_served: None,
                target_rh: Some(50.0),
                part_load_curve_coeffs: None,
                plf_min: None,
                off_cycle_parasitic_load_w: None,
                min_operating_temp_c: None,
                max_operating_temp_c: None,
            },
        )
        .unwrap();

        let mut eq_both = Dehumidifier::new(cfg_both.clone());
        eq_both.init(&cfg_both, &env(0.60)).unwrap();
        eq_both.update_control(&env(0.60));
        let mut slots_both = ports();
        eq_both
            .step(&env(0.60), Duration::from_secs(60), &mut slots_both)
            .unwrap();
        let power_both = eq_both.telemetry().get(tk::ELECTRIC_POWER_W).unwrap();

        let mut eq_ef = Dehumidifier::new(cfg_ef_only.clone());
        eq_ef.init(&cfg_ef_only, &env(0.60)).unwrap();
        eq_ef.update_control(&env(0.60));
        let mut slots_ef = ports();
        eq_ef
            .step(&env(0.60), Duration::from_secs(60), &mut slots_ef)
            .unwrap();
        let power_ef = eq_ef.telemetry().get(tk::ELECTRIC_POWER_W).unwrap();

        approx_eq(power_both, power_ef);
    }

    // ── Off-cycle parasitic load tests ────────────────────────────────────

    fn config_with_parasitic(parasitic: Option<f64>) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "Parasitic Test".to_string(),
            "Dehumidifier".to_string(),
            crate::DehumidifierConfig {
                equipment_id: Some(9),
                zone_id: Some(1),
                capacity_liters_per_day: Some(70.0 * 0.473_176_5),
                energy_factor: Some(2.0),
                integrated_energy_factor: None,
                fraction_served: None,
                target_rh: Some(50.0),
                part_load_curve_coeffs: None,
                plf_min: None,
                off_cycle_parasitic_load_w: parasitic,
                min_operating_temp_c: None,
                max_operating_temp_c: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn off_with_parasitic_yields_nonzero_power() {
        let cfg = config_with_parasitic(Some(5.0));
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.40)).unwrap();

        let mut slots = ports();
        eq.step(&env(0.40), Duration::from_secs(60), &mut slots)
            .unwrap();

        assert_eq!(eq.update_control(&env(0.40)), OperatingMode::Off);
        assert_eq!(eq.telemetry().get(tk::ELECTRIC_POWER_W), Some(5.0));
        assert_eq!(eq.telemetry().get(tk::WATER_REMOVAL_L_DAY), Some(0.0));
        assert_eq!(eq.telemetry().get(tk::LATENT_REMOVAL_W), Some(0.0));
        // Parasitic load becomes sensible heat in the zone.
        assert_eq!(eq.telemetry().get(tk::SENSIBLE_GAIN_W), Some(5.0));
        assert_eq!(slots.electrical.load_power_w, 5.0);
        // Parasitic power is reflected in the thermal port as sensible gain.
        assert_eq!(slots.thermal[0].sensible_gain_w, 5.0);
        assert_eq!(slots.thermal[0].latent_gain_w, 0.0);
    }

    #[test]
    fn on_with_parasitic_no_double_count() {
        // At full load (PLR=1.0, RTF=1.0), the parasitic must not be
        // double-counted: on-cycle power already includes parasitic
        // implicitly through the energy factor.
        let cfg_parasitic = config_with_parasitic(Some(5.0));
        let mut eq_parasitic = Dehumidifier::new(cfg_parasitic.clone());
        eq_parasitic.init(&cfg_parasitic, &env(0.60)).unwrap();
        eq_parasitic.update_control(&env(0.60));
        let mut slots_p = ports();
        eq_parasitic
            .step(&env(0.60), Duration::from_secs(60), &mut slots_p)
            .unwrap();

        let cfg_none = config_with_parasitic(None);
        let mut eq_none = Dehumidifier::new(cfg_none.clone());
        eq_none.init(&cfg_none, &env(0.60)).unwrap();
        eq_none.update_control(&env(0.60));
        let mut slots_n = ports();
        eq_none
            .step(&env(0.60), Duration::from_secs(60), &mut slots_n)
            .unwrap();

        // At full load (RTF=1.0): electric_power_w = on_power * 1.0 + parasitic * 0.0.
        // Both instances must produce the same electric power.
        let power_parasitic = eq_parasitic.telemetry().get(tk::ELECTRIC_POWER_W).unwrap();
        let power_none = eq_none.telemetry().get(tk::ELECTRIC_POWER_W).unwrap();
        approx_eq(power_parasitic, power_none);
        // EnergyPlus ZoneDehumidifier.cc:901: reported parasitic power is
        // (1 - RTF) * configured load — zero at full load (RTF = 1.0), not
        // the configured nameplate value.
        assert_eq!(
            eq_parasitic.telemetry().get(tk::PARASITIC_ELECTRIC_W),
            Some(0.0)
        );
    }

    #[test]
    fn off_without_parasitic_returns_zero_power() {
        let cfg = config_with_parasitic(None);
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.40)).unwrap();

        let mut slots = ports();
        eq.step(&env(0.40), Duration::from_secs(60), &mut slots)
            .unwrap();

        assert_eq!(eq.update_control(&env(0.40)), OperatingMode::Off);
        assert_eq!(eq.telemetry().get(tk::ELECTRIC_POWER_W), Some(0.0));
        assert_eq!(eq.telemetry().get(tk::SENSIBLE_GAIN_W), Some(0.0));
        assert_eq!(slots.electrical.load_power_w, 0.0);
    }

    #[test]
    fn parasitic_telemetry_field_is_present() {
        let cfg = config_with_parasitic(Some(5.0));
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.40)).unwrap();

        let mut slots = ports();
        eq.step(&env(0.40), Duration::from_secs(60), &mut slots)
            .unwrap();

        assert_eq!(eq.telemetry().get(tk::PARASITIC_ELECTRIC_W), Some(5.0));
    }

    // ── Temperature lockout tests ──────────────────────────────────────────

    /// Inlet air temperature below the minimum operating temperature forces
    /// `is_on = false` even when RH is above the deadband upper threshold.
    /// EnergyPlus ZoneDehumidifier.cc:687–688: `InletAirTemp < MinInletAirTemp`
    /// disables the unit.
    #[test]
    fn temp_below_min_operating_forces_is_on_false() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env_with_temp(5.0, 0.60)).unwrap();

        // 5°C is below the default min_operating_temp_c of 10.0°C.
        // RH=0.60 is above max_rh=0.5125, so normally the unit would run.
        let mode = eq.update_control(&env_with_temp(5.0, 0.60));
        assert_eq!(mode, OperatingMode::Off, "unit must be off below min temp");
        assert_eq!(eq.telemetry().get(tk::IS_ON), Some(0.0));

        let mut slots = ports();
        eq.step(
            &env_with_temp(5.0, 0.60),
            Duration::from_secs(60),
            &mut slots,
        )
        .unwrap();
        assert_eq!(eq.telemetry().get(tk::ELECTRIC_POWER_W), Some(0.0));
        assert_eq!(eq.telemetry().get(tk::TEMPERATURE_LOCKOUT), Some(1.0));
        assert!(eq.telemetry().get(tk::INLET_AIR_TEMP_C).unwrap() > 0.0);
    }

    /// Inlet air temperature above the maximum operating temperature forces
    /// `is_on = false`.
    #[test]
    fn temp_above_max_operating_forces_is_on_false() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env_with_temp(40.0, 0.60)).unwrap();

        // 40°C is above the default max_operating_temp_c of 35.0°C.
        let mode = eq.update_control(&env_with_temp(40.0, 0.60));
        assert_eq!(mode, OperatingMode::Off, "unit must be off above max temp");
        assert_eq!(eq.telemetry().get(tk::IS_ON), Some(0.0));

        let mut slots = ports();
        eq.step(
            &env_with_temp(40.0, 0.60),
            Duration::from_secs(60),
            &mut slots,
        )
        .unwrap();
        assert_eq!(eq.telemetry().get(tk::ELECTRIC_POWER_W), Some(0.0));
        assert_eq!(eq.telemetry().get(tk::TEMPERATURE_LOCKOUT), Some(1.0));
    }

    /// Inlet air temperature within the operating range allows normal
    /// dehumidifier operation when RH is above the deadband.
    #[test]
    fn temp_within_operating_range_allows_normal_operation() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env_with_temp(25.0, 0.60)).unwrap();

        // 25°C is within the default [10.0, 35.0]°C range.
        let mode = eq.update_control(&env_with_temp(25.0, 0.60));
        assert_eq!(
            mode,
            OperatingMode::Cooling,
            "unit must run within operating temp range"
        );

        let mut slots = ports();
        eq.step(
            &env_with_temp(25.0, 0.60),
            Duration::from_secs(60),
            &mut slots,
        )
        .unwrap();
        assert!(eq.telemetry().get(tk::ELECTRIC_POWER_W).unwrap() > 0.0);
        assert_eq!(eq.telemetry().get(tk::IS_ON), Some(1.0));
        assert_eq!(eq.telemetry().get(tk::TEMPERATURE_LOCKOUT), Some(0.0));
    }

    /// When both `min_operating_temp_c` and `max_operating_temp_c` are `None`,
    /// the temperature lockout is disabled and the unit operates as it did
    /// before the feature was added (backward compatible).
    #[test]
    fn temp_limits_none_disables_lockout_backward_compatible() {
        let cfg = config();
        let mut eq = Dehumidifier::new(cfg.clone());
        eq.init(&cfg, &env(0.60)).unwrap();

        // Override internal limits to None — lockout disabled.
        eq.min_operating_temp_c = None;
        eq.max_operating_temp_c = None;

        // At 5°C (well below typical lockout thresholds), the unit must still
        // run when RH is high because the lockout is disabled.
        let mode = eq.update_control(&env_with_temp(5.0, 0.60));
        assert_eq!(
            mode,
            OperatingMode::Cooling,
            "unit must run at 5°C when lockout is disabled"
        );

        let mut slots = ports();
        eq.step(
            &env_with_temp(5.0, 0.60),
            Duration::from_secs(60),
            &mut slots,
        )
        .unwrap();
        assert!(eq.telemetry().get(tk::ELECTRIC_POWER_W).unwrap() > 0.0);
        assert_eq!(eq.telemetry().get(tk::IS_ON), Some(1.0));
        assert_eq!(eq.telemetry().get(tk::TEMPERATURE_LOCKOUT), Some(0.0));
    }
}
