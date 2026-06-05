//! Electrical domain solver for power flow aggregation.

use std::time::Duration;

use hares_physics::units::power_w_to_kw;
use hares_types::{DomainId, DomainSolver, DomainUpdate, ELECTRICAL, EnvironmentState, PortSlots};
use thiserror::Error;

const ZIP_SUM_TOLERANCE: f64 = 1e-6;

/// Lower bound for voltage clamping in per-unit.
///
/// The [0.9, 1.1] pu modeling range is wider than the ANSI C84.1-2020
/// Range A [0.95, 1.05] pu service-voltage band and the Range B
/// [0.917, 1.058] pu utilization-voltage band (ANSI C84.1-2020 §4.1).
/// The wider modeling range is chosen because:
/// — HARES simulation scenarios can include fault transients, motor starts,
///   and islanding events that drive voltage outside normal steady-state
///   limits, and clamping at tighter bounds would mask the physics of those
///   events.
/// — The ±10% envelope is the standard steady-state voltage tolerance for
///   electrical equipment nameplate ratings (ANSI C84.1-2020 §4.2.2,
///   "Range A — Service Voltage"). Within this band, the static ZIP
///   polynomial remains a physically meaningful load model.
/// — Below 0.9 pu, load compositions change qualitatively (motors stall,
///   electronics shut down) making the static ZIP model invalid; above
///   1.1 pu, equipment damage is likely and the ZIP model over-predicts
///   load.
const VOLTAGE_PU_MIN: f64 = 0.9;
const VOLTAGE_PU_MAX: f64 = 1.1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZipCoefficients {
    pub z: f64,
    pub i: f64,
    pub p: f64,
}

#[derive(Debug, Error)]
pub enum ElectricalSolverError {
    #[error("ZIP coefficients must be non-negative: z={z}, i={i}, p={p}")]
    NegativeZipCoefficient { z: f64, i: f64, p: f64 },
    #[error("ZIP coefficients must sum to 1.0 within 1e-6, got {sum}")]
    InvalidZipSum { sum: f64 },
    #[error("nominal voltage must be > 0, got {0}")]
    InvalidNominalVoltage(f64),
}

pub type Result<T> = std::result::Result<T, ElectricalSolverError>;

impl ZipCoefficients {
    pub fn new(z: f64, i: f64, p: f64) -> Result<Self> {
        if z < 0.0 || i < 0.0 || p < 0.0 {
            return Err(ElectricalSolverError::NegativeZipCoefficient { z, i, p });
        }
        let sum = z + i + p;
        if (sum - 1.0).abs() > ZIP_SUM_TOLERANCE {
            return Err(ElectricalSolverError::InvalidZipSum { sum });
        }
        Ok(Self { z, i, p })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElectricalSolverConfig {
    pub zip: ZipCoefficients,
    pub nominal_voltage_pu: f64,
}

impl Default for ElectricalSolverConfig {
    fn default() -> Self {
        Self {
            zip: ZipCoefficients {
                z: 0.0,
                i: 0.0,
                p: 1.0,
            },
            nominal_voltage_pu: 1.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ElectricalSolver {
    config: ElectricalSolverConfig,
    net_active_kw: f64,
    net_reactive_kvar: f64,
}

impl ElectricalSolver {
    pub fn new(config: ElectricalSolverConfig) -> Result<Self> {
        if config.nominal_voltage_pu <= 0.0 {
            return Err(ElectricalSolverError::InvalidNominalVoltage(
                config.nominal_voltage_pu,
            ));
        }
        Ok(Self {
            config,
            net_active_kw: 0.0,
            net_reactive_kvar: 0.0,
        })
    }

    /// Returns most recent net active power result.
    ///
    /// Value becomes stale after the next `resolve()` call and is not
    /// synchronization-safe with concurrent mutation.
    #[must_use]
    pub fn net_active_kw(&self) -> f64 {
        self.net_active_kw
    }

    /// Returns most recent net reactive power result.
    ///
    /// Value becomes stale after the next `resolve()` call and is not
    /// synchronization-safe with concurrent mutation.
    #[must_use]
    pub fn net_reactive_kvar(&self) -> f64 {
        self.net_reactive_kvar
    }

    /// Returns the ZIP load scale factor for the given per-unit voltage.
    ///
    /// Clamps `voltage_pu` to the modeling range [0.9, 1.1] pu (see
    /// `VOLTAGE_PU_MIN` / `VOLTAGE_PU_MAX` constants for rationale) before
    /// computing `Z·V² + I·V + P` where `V = voltage_pu / nominal_voltage_pu`.
    /// A `tracing::warn!` fires when the raw value is outside the modeling range.
    ///
    /// Used internally by `resolve()` and exposed so callers (e.g. the electrical
    /// balance invariant check) can apply the same scaling to port-side load
    /// accumulation for a like-for-like comparison with `net_active_kw()`.
    #[must_use]
    pub fn zip_load_scale(&self, voltage_pu: f64) -> f64 {
        let raw_voltage_pu = voltage_pu;
        let voltage_pu = raw_voltage_pu.clamp(VOLTAGE_PU_MIN, VOLTAGE_PU_MAX);
        if raw_voltage_pu != voltage_pu {
            tracing::warn!(
                raw_voltage_pu = raw_voltage_pu,
                clamped_voltage_pu = voltage_pu,
                "grid voltage_pu outside modeling range [{}, {}]; clamped",
                VOLTAGE_PU_MIN,
                VOLTAGE_PU_MAX,
            );
        }
        let v = voltage_pu / self.config.nominal_voltage_pu;
        #[cfg(debug_assertions)]
        {
            let nominal = self.config.nominal_voltage_pu;
            let v_lower = VOLTAGE_PU_MIN / nominal;
            let v_upper = VOLTAGE_PU_MAX / nominal;
            assert!(
                v >= v_lower - 2.0 * f64::EPSILON && v <= v_upper + 2.0 * f64::EPSILON,
                "clamped v = voltage_pu / nominal must be in [{}, {}], got {}",
                v_lower,
                v_upper,
                v,
            );
        }
        self.config.zip.z * v * v + self.config.zip.i * v + self.config.zip.p
    }
}

impl DomainSolver for ElectricalSolver {
    fn domain_id(&self) -> DomainId {
        ELECTRICAL
    }

    fn resolve(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
        _dt: Duration,
        out: &mut DomainUpdate,
    ) {
        let p_load = power_w_to_kw(ports.electrical.load_power_w);
        let p_gen = power_w_to_kw(ports.electrical.generation_power_w);
        let load_scale = self.zip_load_scale(env.grid.voltage_pu);

        let p_load_adj = p_load * load_scale;
        let p_gen_adj = p_gen;
        self.net_active_kw = p_load_adj + p_gen_adj;
        self.net_reactive_kvar = ports.electrical.reactive_power_kvar;

        out.domain_id = ELECTRICAL;
        out.zone_temperatures_c.clear();
        let payload = out
            .custom_payload
            .get_or_insert_with(|| Vec::with_capacity(2));
        payload.clear();
        payload.push(self.net_active_kw);
        payload.push(self.net_reactive_kvar);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{FixedOffset, TimeZone};
    use hares_types::{
        DomainSolver, EnvironmentState, GridState, PortContribution, PortSlots, WeatherState,
        ZoneState,
    };

    use crate::electrical_solver::{ElectricalSolver, ElectricalSolverConfig, ZipCoefficients};

    fn env_with_voltage(v: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: hares_types::ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                outdoor_wet_bulb_c: 7.0,
                outdoor_enthalpy_j_kg: 22_800.0,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: v,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: Default::default(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    #[test]
    fn sums_three_loads_without_zip_correction() {
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig::default()).unwrap();
        let env = env_with_voltage(1.0);
        let mut ports = PortSlots::default();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 1000.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 2000.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 3000.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        let update = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        assert_eq!(update.domain_id, hares_types::ELECTRICAL);
        assert_eq!(solver.net_active_kw(), 6.0);
    }

    #[test]
    fn zip_correction_matches_reference_formula() {
        let zip = ZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();
        let env = env_with_voltage(0.95);
        let mut ports = PortSlots::default();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 6000.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let expected = 6.0 * (0.5 * 0.95 * 0.95 + 0.3 * 0.95 + 0.2);
        assert!((solver.net_active_kw() - expected).abs() <= 1e-10);
    }

    #[test]
    fn generation_is_not_zip_scaled() {
        let zip = ZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();
        let mut ports = PortSlots::default();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: -5000.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 3000.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();

        let env_nominal = env_with_voltage(1.0);
        let _ = solver.resolve_new(&ports, &env_nominal, Duration::from_secs(60));
        assert!((solver.net_active_kw() - (-2.0)).abs() <= 1e-12);

        let env_low_v = env_with_voltage(0.95);
        let _ = solver.resolve_new(&ports, &env_low_v, Duration::from_secs(60));
        let expected = 3.0 * (0.5 * 0.95 * 0.95 + 0.3 * 0.95 + 0.2) - 5.0;
        assert!((solver.net_active_kw() - expected).abs() <= 1e-10);
    }

    #[test]
    fn electrical_balance_identity_holds() {
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig::default()).unwrap();
        let env = env_with_voltage(1.0);
        let mut ports = PortSlots::default();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 1500.0,
                reactive_power_kvar: 0.2,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: -500.0,
                reactive_power_kvar: -0.1,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let p_grid = solver.net_active_kw();
        let p_equipment_sum = ports.electrical.net_active_w();
        assert!((p_grid - p_equipment_sum / 1_000.0).abs() < 0.001);
    }

    #[test]
    fn electrical_balance_identity_holds_with_non_default_zip() {
        // IEEE residential ZIP: Z=0.2, I=0.2, P=0.6 at V=0.95 pu.
        // With the default (raw) comparison, this would produce a false-positive
        // residual of p_load * (scale - 1). The corrected comparison adjusts
        // port-side loads by the same scale factor.
        let zip = ZipCoefficients::new(0.2, 0.2, 0.6).unwrap();
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();
        let env = env_with_voltage(0.95);
        let mut ports = PortSlots::default();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 1500.0,
                reactive_power_kvar: 0.2,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: -500.0,
                reactive_power_kvar: -0.1,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let p_grid = solver.net_active_kw();
        // Raw comparison would fail: |p_grid - ports.electrical.net_active_w() / 1_000.0| > 0.001
        let scale = solver.zip_load_scale(env.grid.voltage_pu);
        let port_net_adj =
            ports.electrical.load_power_w * scale + ports.electrical.generation_power_w;
        assert!((p_grid - port_net_adj / 1_000.0).abs() < 0.001);
    }

    #[test]
    fn zip_adjusted_invariant_passes_non_default_config() {
        // Regression test: runs the solver with a non-default ZIP config at
        // non-nominal voltage, then verifies the corrected invariant check
        // (ZIP-adjusted port vs ZIP-adjusted grid) produces a near-zero residual.
        let zip = ZipCoefficients::new(0.2, 0.2, 0.6).unwrap();
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();
        let env = env_with_voltage(0.95);
        let mut ports = PortSlots::default();
        // 10 kW load, no generation
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 10000.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let p_grid = solver.net_active_kw();
        let scale = solver.zip_load_scale(env.grid.voltage_pu);
        let port_net = ports.electrical.load_power_w * scale + ports.electrical.generation_power_w;
        let residual = (p_grid - port_net / 1_000.0).abs();
        assert!(
            residual < 0.001,
            "ZIP-adjusted residual should be near-zero, got {residual}"
        );
    }

    #[test]
    fn empty_electrical_slots_returns_zero() {
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig::default()).unwrap();
        let env = env_with_voltage(1.0);
        let ports = PortSlots::default();
        let update = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        assert_eq!(update.custom_payload, Some(vec![0.0, 0.0]));
        assert_eq!(solver.net_active_kw(), 0.0);
        assert_eq!(solver.net_reactive_kvar(), 0.0);
    }

    #[test]
    fn ieee_zip_reference_residential_load() {
        // IEEE Task Force on Load Representation (1993),
        // IEEE T-PWRS 8(2):472-482
        // General residential ZIP: Z=0.2, I=0.2, P=0.6
        // At V=0.95 pu: scale = 0.2*0.9025 + 0.2*0.95 + 0.6 = 0.97050
        let zip = ZipCoefficients::new(0.2, 0.2, 0.6).unwrap();
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();
        let env = env_with_voltage(0.95);
        let mut ports = PortSlots::default();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 10000.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let expected = 10.0 * (0.2 * 0.9025 + 0.2 * 0.95 + 0.6);
        assert!(
            (solver.net_active_kw() - expected).abs() <= 1e-10,
            "net={}, expected={}",
            solver.net_active_kw(),
            expected
        );
    }

    #[test]
    fn zip_adjusted_power_balance_at_non_unity_voltage() {
        // Verify P_net = P_load * (Z*V² + I*V + P) + P_gen
        // with Z=0.5, I=0.3, P=0.2, V=0.95
        // scale = 0.5*0.9025 + 0.3*0.95 + 0.2 = 0.93625
        // 10kW load → 9.3625 kW adjusted, plus -3kW generation
        let zip = ZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();
        let env = env_with_voltage(0.95);
        let mut ports = PortSlots::default();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 10000.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: -3000.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let load_scale = 0.5 * 0.9025 + 0.3 * 0.95 + 0.2;
        let expected = 10.0 * load_scale + (-3.0);
        assert!(
            (solver.net_active_kw() - expected).abs() <= 1e-10,
            "net={}, expected={}",
            solver.net_active_kw(),
            expected
        );
    }

    #[test]
    fn zip_coefficients_validate_sum_and_sign() {
        let bad_sum = ZipCoefficients::new(0.5, 0.3, 0.3);
        assert!(bad_sum.is_err());
        let bad_sign = ZipCoefficients::new(-0.1, 0.6, 0.5);
        assert!(bad_sign.is_err());
    }

    #[test]
    fn zip_load_scale_matches_inline_formula() {
        // Verify the public method returns the same value as the inline
        // computation in resolve().
        let zip = ZipCoefficients::new(0.2, 0.2, 0.6).unwrap();
        let solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();
        // V=0.95 pu, Z=0.2, I=0.2, P=0.6 → scale = 0.2·0.9025 + 0.2·0.95 + 0.6 = 0.9705
        let scale = solver.zip_load_scale(0.95);
        let expected = 0.2 * 0.95_f64.powi(2) + 0.2 * 0.95 + 0.6;
        assert!((scale - expected).abs() <= 1e-10);
    }

    #[test]
    fn zip_load_scale_unity_for_default_config() {
        let solver = ElectricalSolver::new(ElectricalSolverConfig::default()).unwrap();
        assert!((solver.zip_load_scale(0.95) - 1.0).abs() <= 1e-10);
        assert!((solver.zip_load_scale(1.0) - 1.0).abs() <= 1e-10);
        assert!((solver.zip_load_scale(1.05) - 1.0).abs() <= 1e-10);
    }

    /// Helper: run solver with given voltage and ZIP config, return net_active_kw.
    fn run_with_voltage(zip: ZipCoefficients, voltage_pu: f64, load_w: f64) -> f64 {
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();
        let env = env_with_voltage(voltage_pu);
        let mut ports = PortSlots::default();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: load_w,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        solver.net_active_kw()
    }

    #[test]
    fn test_voltage_clamping() {
        // ZIP: Z=0.5, I=0.3, P=0.2 — voltage-sensitive so clamping is visible.
        let zip = ZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
        let load_w = 10000.0;

        // V=0.5 (below range) → clamped to 0.9.
        // load_scale at V=0.9: 0.5*0.81 + 0.3*0.9 + 0.2 = 0.875
        let result_low = run_with_voltage(zip, 0.5, load_w);
        let expected_low = load_w / 1000.0 * 0.875;
        assert!((result_low - expected_low).abs() <= 1e-10);

        // V=1.3 (above range) → clamped to 1.1.
        // load_scale at V=1.1: 0.5*1.21 + 0.3*1.1 + 0.2 = 1.135
        let result_high = run_with_voltage(zip, 1.3, load_w);
        let expected_high = load_w / 1000.0 * 1.135;
        assert!((result_high - expected_high).abs() <= 1e-10);

        // V=1.0 (in range) → passes through unchanged, scale = 1.0.
        let result_normal = run_with_voltage(zip, 1.0, load_w);
        let expected_normal = load_w / 1000.0;
        assert!((result_normal - expected_normal).abs() <= 1e-10);
    }

    #[test]
    fn test_zero_voltage_gives_reasonable_load() {
        // V=0.0 (extreme fault) is clamped to 0.9 pu. The solver produces
        // the load corresponding to the clamped lower bound, not the raw
        // V=0 result. With Z=0.5, I=0.3, P=0.2, V=0 raw gives scale=0.2
        // (2 kW for a 10 kW load), which dramatically understates demand.
        let zip = ZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
        let load_w = 10000.0;

        let result_zero = run_with_voltage(zip, 0.0, load_w);
        let result_bound = run_with_voltage(zip, 0.9, load_w);

        // Clamped result matches the V=0.9 lower-bound result.
        assert!((result_zero - result_bound).abs() <= 1e-10);

        // Clamped result is non-zero — there is still load at 0.9 pu.
        assert!(result_zero > 0.0);

        // Sanity check: raw V=0 result would be 2.0 kW (= 10 * 0.2).
        // The clamped result is substantially larger.
        assert!(result_zero > 2.1);
    }

    #[test]
    fn zip_load_scale_clamps_out_of_range_voltage() {
        // Verify that zip_load_scale clamps its input to [0.9, 1.1] pu.
        // Ensures the invariant checker and observer (which call
        // zip_load_scale directly) get the same scale as resolve()
        // when facing out-of-range voltages.
        let zip = ZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
        let solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();

        // V=0.5 (below range) → clamped to 0.9.
        // ZIP at V=0.9: 0.5*0.81 + 0.3*0.9 + 0.2 = 0.875
        let scale_low = solver.zip_load_scale(0.5);
        let expected_low = 0.5 * 0.9_f64.powi(2) + 0.3 * 0.9 + 0.2;
        assert!((scale_low - expected_low).abs() <= 1e-10);

        // V=1.3 (above range) → clamped to 1.1.
        // ZIP at V=1.1: 0.5*1.21 + 0.3*1.1 + 0.2 = 1.135
        let scale_high = solver.zip_load_scale(1.3);
        let expected_high = 0.5 * 1.1_f64.powi(2) + 0.3 * 1.1 + 0.2;
        assert!((scale_high - expected_high).abs() <= 1e-10);
    }
}
