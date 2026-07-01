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
const DEFAULT_RH_BOUNDS: (f64, f64) = (RH_MIN_FRACTION, RH_MAX_FRACTION);

const WATTS_PER_KILOWATT_HOUR: f64 = 3_600_000.0;
const DEFAULT_NORMALIZED_CURVE: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];

// Default cubic PLF curve coefficients derived from the ticket directive:
// PLF = C0 + C1·PLR + C2·PLR² + C3·PLR³
// Default values give PLF ≈ 0.7 at PLR=0 and PLF = 1.0 at PLR=1.0.
// These match the shape described in H-1153 problem statement §2 but do
// not correspond to any EnergyPlus-shipped default — EnergyPlus defaults
// to PLF = 1.0 (no degradation) when no PartLoadCurve is configured
// (ZoneDehumidifier.cc lines 763–767).
const DEFAULT_PLF_CURVE_COEFFS: [f64; 4] = [0.7, 1.0, -0.7, 0.0];
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
                core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::HAS_MODE,
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
            return PerformanceSnapshot {
                water_removal_l_day: 0.0,
                electric_power_w: 0.0,
                latent_removal_w: 0.0,
                sensible_gain_w: 0.0,
                plr: 0.0,
                plf: 1.0,
                rtf: 0.0,
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
        // HARES uses a cubic curve by default; the default coefficients are an
        // engineering choice matching the shape described in the ticket directive
        // (not an EnergyPlus-shipped default).
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

        // EnergyPlus CalcZoneDehumidifier lines 829–831, 852–855: average
        // electric power is on-cycle power scaled by runtime fraction (RTF).
        // EnergyPlus CalcZoneDehumidifier line 858: latent (moisture) output
        // is scaled by PLR, not RTF — the two scalars are independent.
        let water_removal_kg_s = water_removal_l_day * KG_PER_LITER_WATER / SECONDS_PER_DAY;
        let electric_power_w_on = if energy_factor_l_kwh > 0.0 {
            water_removal_kg_s * WATTS_PER_KILOWATT_HOUR / energy_factor_l_kwh
        } else {
            0.0
        };
        let electric_power_w = electric_power_w_on * rtf;
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
    }
}

impl Dehumidifier {
    fn init_from_typed(&mut self, config: &EquipmentConfig) -> crate::Result<()> {
        let cfg = config.require_typed::<DehumidifierConfig>("Dehumidifier")?;
        cfg.validate()?;

        self.rated_water_removal_l_day = cfg
            .capacity_liters_per_day
            .unwrap_or(DEFAULT_RATED_WATER_REMOVAL_L_DAY);

        self.rated_energy_factor_l_kwh = cfg
            .integrated_energy_factor
            .or(cfg.energy_factor)
            .unwrap_or(1.8);

        self.fraction_load_served = cfg.fraction_served.unwrap_or(1.0).clamp(0.0, 1.0);

        self.part_load_curve_coeffs = cfg
            .part_load_curve_coeffs
            .unwrap_or(DEFAULT_PLF_CURVE_COEFFS);
        self.plf_min = cfg
            .plf_min
            .map(|v| v.clamp(0.0, 1.0))
            .unwrap_or(DEFAULT_PLF_MIN);

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
        });
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        let pressure_pa = env.weather.pressure_pa();
        let zone = env.zones.iter().find(|z| z.id == self.zone_id);
        if let Some(zone_state) = zone {
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
        let snapshot = self.performance_snapshot(
            zone.temperature_c,
            rh.clamp(RH_MIN_FRACTION, RH_MAX_FRACTION),
        );
        self.check_invariants(snapshot.plf, snapshot.rtf)?;
        if snapshot.electric_power_w > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: snapshot.electric_power_w,
                reactive_power_kvar: 0.0,
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
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(electric_kw)),
                reactive_power_kvar: None,
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
        });
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
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
    let mut telemetry = Telemetry::with_capacity(13);
    telemetry.insert(tk::WATER_REMOVAL_L_DAY, 0.0);
    telemetry.insert(tk::ELECTRIC_POWER_W, 0.0);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
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
            target_rh: 55.0,
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
        let mut eq_cold = Dehumidifier::new(cfg.clone());
        eq_cold.init(&cfg, &env(0.60)).unwrap();
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

    /// At part load (PLR ≈ 0.5), the default cubic PLF curve evaluates to
    /// 1.025 at PLR=0.5 (> 1.0) and clamps to 1.0 — this test verifies the
    /// clamping behavior when the cubic exceeds the upper bound. The runtime
    /// fraction should be PLR / PLF = 0.5 / 1.0 = 0.5.
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

        // PLF at PLR=0.5 via cubic [0.7, 1.0, -0.7, 0.0]:
        // PLF = 0.7 + 1.0*0.5 - 0.7*0.25 = 0.7 + 0.5 - 0.175 = 1.025
        // Clamped to [0.7, 1.0] → 1.0
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

    /// Verify PLF clamping at the PLR boundaries.
    ///
    /// - At PLR = 0, PLF = 0.7 (the PLF_MIN floor, since the cubic gives 0.7).
    /// - At PLR = 1.0, PLF = 1.0 (the PLF_MAX ceiling).
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
}
