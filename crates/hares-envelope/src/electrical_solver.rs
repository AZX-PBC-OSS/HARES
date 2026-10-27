//! Electrical domain solver for power flow aggregation.
//!
//! The voltage channel's admitted modeling band, per unit: the outage
//! setter (`Dwelling::set_grid_voltage`) and the HELICS voltage contract
//! admit exactly `0.0` (a utility outage) or a finite per-unit voltage in
//! [0.5, 1.5] pu. The ZIP load model's voltage sensitivity operates over
//! that full admitted band: the polynomial terms are evaluated at the
//! ACTUAL per-unit voltage with no clamp. Clamping the evaluation to a
//! narrower band (the [0.9, 1.1] modeling range this solver once used)
//! substituted the band edge for the sag: a 0.7 pu sag read as 0.9 pu
//! every step forever, which is the substitution class the input-honesty
//! rules forbid (the value given was replaced by a policy). The static
//! ZIP polynomial evaluated across the admitted band is the
//! voltage-dependence physics the model name carries; the per-equipment
//! ZIP (`hares-types::zip::ZipLoad::apply`) has never clamped. A value
//! outside the admitted band cannot reach this solver through the setters
//! (they reject it naming the band); the dead-bus branch of
//! [`ElectricalSolver::effective_load_scale`] holds the scale at 1.0 for
//! the de-energized case, and a non-finite utility voltage de-energizes
//! the bus by that branch's NaN-robust comparison, so the polynomial is
//! evaluated only at finite per-unit voltages in practice.

use std::time::Duration;

use hares_types::{
    DomainId, DomainSolver, DomainUpdate, ELECTRICAL, EnvironmentState, HaresError, PortSlots,
};
use thiserror::Error;

const ZIP_SUM_TOLERANCE: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolverZipCoefficients {
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

impl SolverZipCoefficients {
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
    pub zip: SolverZipCoefficients,
    pub nominal_voltage_pu: f64,
}

impl Default for ElectricalSolverConfig {
    fn default() -> Self {
        Self {
            zip: SolverZipCoefficients {
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
    net_active_w: f64,
    net_reactive_var: f64,
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
            net_active_w: 0.0,
            net_reactive_var: 0.0,
        })
    }

    /// Returns most recent net active power result.
    ///
    /// Value becomes stale after the next `resolve()` call and is not
    /// synchronization-safe with concurrent mutation.
    #[must_use]
    pub fn net_active_w(&self) -> f64 {
        self.net_active_w
    }

    /// Returns most recent net reactive power result.
    ///
    /// Value becomes stale after the next `resolve()` call and is not
    /// synchronization-safe with concurrent mutation.
    #[must_use]
    pub fn net_reactive_var(&self) -> f64 {
        self.net_reactive_var
    }

    /// Returns the ZIP load scale factor for the given per-unit voltage.
    ///
    /// Evaluates `Z·V² + I·V + P` where `V = voltage_pu / nominal_voltage_pu`
    /// at the ACTUAL per-unit voltage: no clamp. The load model's voltage
    /// sensitivity operates over the voltage channel's full admitted band
    /// ([0.5, 1.5] pu; see the module's band note above); the polynomial is
    /// defined and evaluated at any finite voltage, so a value that
    /// bypassed the boundary validation still evaluates rather than being
    /// substituted.
    ///
    /// Used internally by `resolve()` and exposed so callers (e.g. the electrical
    /// balance invariant check) can apply the same scaling to port-side load
    /// accumulation for a like-for-like comparison with `net_active_w()`.
    #[must_use]
    pub fn zip_load_scale(&self, voltage_pu: f64) -> f64 {
        let v = voltage_pu / self.config.nominal_voltage_pu;
        self.config.zip.z * v * v + self.config.zip.i * v + self.config.zip.p
    }

    /// ZIP load scale for the current grid state, evaluated at the **bus**
    /// voltage ([`hares_types::GridState::bus_voltage_pu`]) so islanded
    /// operation (utility out, backup source holding the bus at nominal)
    /// scales loads correctly.
    ///
    /// When the bus is de-energized (utility outage with no island source)
    /// the scale is held at 1.0: every load is force-gated off at the
    /// control level in that state, so port-side load is expected to be
    /// zero, and passing any residual through at full weight keeps a leaked
    /// (un-gated) load visible at the meter and in the electrical-balance
    /// invariant instead of masking it with the clamped-ZIP attenuation.
    ///
    /// Callers comparing port-side accumulation against `net_active_w()`
    /// (invariant checker, observer) must use this method so both sides see
    /// the same scale.
    #[must_use]
    pub fn effective_load_scale(&self, grid: &hares_types::GridState) -> f64 {
        if grid.bus_energized() {
            self.zip_load_scale(grid.bus_voltage_pu())
        } else {
            1.0
        }
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
    ) -> std::result::Result<(), HaresError> {
        let p_load = ports.electrical.load_power_w;
        let p_gen = ports.electrical.generation_power_w;
        let load_scale = self.effective_load_scale(&env.grid);

        let p_load_adj = p_load * load_scale;
        let p_gen_adj = p_gen;
        self.net_active_w = p_load_adj + p_gen_adj;
        self.net_reactive_var = ports.electrical.reactive_power_var;

        out.domain_id = ELECTRICAL;
        out.zone_temperatures_c.clear();
        let payload = out
            .custom_payload
            .get_or_insert_with(|| Vec::with_capacity(2));
        payload.clear();
        payload.push(self.net_active_w);
        payload.push(self.net_reactive_var);
        Ok(())
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

    use crate::electrical_solver::{
        ElectricalSolver, ElectricalSolverConfig, SolverZipCoefficients,
    };

    fn env_with_voltage(v: f64) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            unavailable_periods: hares_types::UnavailablePeriodState::default(),
            domains: hares_types::DomainSlots::default(),
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
                reactive_power_var: 0.0,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 2000.0,
                reactive_power_var: 0.0,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 3000.0,
                reactive_power_var: 0.0,
            })
            .unwrap();
        let update = solver
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap();
        assert_eq!(update.domain_id, hares_types::ELECTRICAL);
        assert_eq!(solver.net_active_w(), 6000.0);
    }

    #[test]
    fn zip_correction_matches_reference_formula() {
        let zip = SolverZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
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
                reactive_power_var: 0.0,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let expected = 6000.0 * (0.5 * 0.95 * 0.95 + 0.3 * 0.95 + 0.2);
        assert!((solver.net_active_w() - expected).abs() <= 1e-10);
    }

    #[test]
    fn generation_is_not_zip_scaled() {
        let zip = SolverZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();
        let mut ports = PortSlots::default();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: -5000.0,
                reactive_power_var: 0.0,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 3000.0,
                reactive_power_var: 0.0,
            })
            .unwrap();

        let env_nominal = env_with_voltage(1.0);
        let _ = solver.resolve_new(&ports, &env_nominal, Duration::from_secs(60));
        assert!((solver.net_active_w() - (-2_000.0)).abs() <= 1e-9);

        let env_low_v = env_with_voltage(0.95);
        let _ = solver.resolve_new(&ports, &env_low_v, Duration::from_secs(60));
        let expected = 3000.0 * (0.5 * 0.95 * 0.95 + 0.3 * 0.95 + 0.2) - 5000.0;
        assert!((solver.net_active_w() - expected).abs() <= 1e-10);
    }

    #[test]
    fn electrical_balance_identity_holds() {
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig::default()).unwrap();
        let env = env_with_voltage(1.0);
        let mut ports = PortSlots::default();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 1500.0,
                reactive_power_var: 0.2,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: -500.0,
                reactive_power_var: -0.1,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let p_grid = solver.net_active_w();
        let p_equipment_sum = ports.electrical.net_active_w();
        assert!((p_grid - p_equipment_sum).abs() < 1.0);
    }

    #[test]
    fn electrical_balance_identity_holds_with_non_default_zip() {
        // IEEE residential ZIP: Z=0.2, I=0.2, P=0.6 at V=0.95 pu.
        // With the default (raw) comparison, this would produce a false-positive
        // residual of p_load * (scale - 1). The corrected comparison adjusts
        // port-side loads by the same scale factor.
        let zip = SolverZipCoefficients::new(0.2, 0.2, 0.6).unwrap();
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
                reactive_power_var: 0.2,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: -500.0,
                reactive_power_var: -0.1,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let p_grid = solver.net_active_w();
        // Raw comparison would fail: |p_grid - ports.electrical.net_active_w()| > 1.0
        let scale = solver.zip_load_scale(env.grid.voltage_pu);
        let port_net_adj =
            ports.electrical.load_power_w * scale + ports.electrical.generation_power_w;
        assert!((p_grid - port_net_adj).abs() < 1.0);
    }

    #[test]
    fn zip_adjusted_invariant_passes_non_default_config() {
        // Regression test: runs the solver with a non-default ZIP config at
        // non-nominal voltage, then verifies the corrected invariant check
        // (ZIP-adjusted port vs ZIP-adjusted grid) produces a near-zero residual.
        let zip = SolverZipCoefficients::new(0.2, 0.2, 0.6).unwrap();
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
                reactive_power_var: 0.0,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let p_grid = solver.net_active_w();
        let scale = solver.zip_load_scale(env.grid.voltage_pu);
        let port_net = ports.electrical.load_power_w * scale + ports.electrical.generation_power_w;
        let residual = (p_grid - port_net).abs();
        assert!(
            residual < 1.0,
            "ZIP-adjusted residual should be near-zero, got {residual}"
        );
    }

    #[test]
    fn empty_electrical_slots_returns_zero() {
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig::default()).unwrap();
        let env = env_with_voltage(1.0);
        let ports = PortSlots::default();
        let update = solver
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap();
        assert_eq!(update.custom_payload, Some(vec![0.0, 0.0]));
        assert_eq!(solver.net_active_w(), 0.0);
        assert_eq!(solver.net_reactive_var(), 0.0);
    }

    #[test]
    fn ieee_zip_reference_residential_load() {
        // IEEE Task Force on Load Representation (1993),
        // IEEE T-PWRS 8(2):472-482
        // General residential ZIP: Z=0.2, I=0.2, P=0.6
        // At V=0.95 pu: scale = 0.2*0.9025 + 0.2*0.95 + 0.6 = 0.97050
        let zip = SolverZipCoefficients::new(0.2, 0.2, 0.6).unwrap();
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
                reactive_power_var: 0.0,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let expected = 10_000.0 * (0.2 * 0.9025 + 0.2 * 0.95 + 0.6);
        assert!(
            (solver.net_active_w() - expected).abs() <= 1e-10,
            "net={}, expected={}",
            solver.net_active_w(),
            expected
        );
    }

    #[test]
    fn zip_adjusted_power_balance_at_non_unity_voltage() {
        // Verify P_net = P_load * (Z*V² + I*V + P) + P_gen
        // with Z=0.5, I=0.3, P=0.2, V=0.95
        // scale = 0.5*0.9025 + 0.3*0.95 + 0.2 = 0.93625
        // 10kW load → 9.3625 kW adjusted, plus -3kW generation
        let zip = SolverZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
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
                reactive_power_var: 0.0,
            })
            .unwrap();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: -3000.0,
                reactive_power_var: 0.0,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let load_scale = 0.5 * 0.9025 + 0.3 * 0.95 + 0.2;
        let expected = 10_000.0 * load_scale + (-3_000.0);
        assert!(
            (solver.net_active_w() - expected).abs() <= 1e-10,
            "net={}, expected={}",
            solver.net_active_w(),
            expected
        );
    }

    #[test]
    fn zip_coefficients_validate_sum_and_sign() {
        let bad_sum = SolverZipCoefficients::new(0.5, 0.3, 0.3);
        assert!(bad_sum.is_err());
        let bad_sign = SolverZipCoefficients::new(-0.1, 0.6, 0.5);
        assert!(bad_sign.is_err());
    }

    #[test]
    fn zip_load_scale_matches_inline_formula() {
        // Verify the public method returns the same value as the inline
        // computation in resolve().
        let zip = SolverZipCoefficients::new(0.2, 0.2, 0.6).unwrap();
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

    /// Helper: run solver with given voltage and ZIP config, return net_active_w.
    fn run_with_voltage(zip: SolverZipCoefficients, voltage_pu: f64, load_w: f64) -> f64 {
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
                reactive_power_var: 0.0,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        solver.net_active_w()
    }

    #[test]
    fn test_voltage_clamping() {
        // Renamed contract (axis A5): no clamping. The ZIP scale
        // evaluates at the ACTUAL pu voltage across the whole admitted
        // band and beyond it; these vectors pin the deep-sag and
        // overvoltage values the old clamp substituted away.
        // ZIP: Z=0.5, I=0.3, P=0.2, voltage-sensitive.
        let zip = SolverZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
        let load_w = 10000.0;

        // V=0.5: no clamp; scale = 0.5*0.25 + 0.3*0.5 + 0.2 = 0.575.
        let result_low = run_with_voltage(zip, 0.5, load_w);
        let expected_low = load_w * (0.5 * 0.25 + 0.3 * 0.5 + 0.2);
        assert!((result_low - expected_low).abs() <= 1e-10);

        // V=1.3: no clamp; scale = 0.5*1.69 + 0.3*1.3 + 0.2 = 1.535.
        let result_high = run_with_voltage(zip, 1.3, load_w);
        let expected_high = load_w * (0.5 * 1.69 + 0.3 * 1.3 + 0.2);
        assert!((result_high - expected_high).abs() <= 1e-10);

        // V=1.0 (in range) → passes through unchanged, scale = 1.0.
        let result_normal = run_with_voltage(zip, 1.0, load_w);
        let expected_normal = load_w;
        assert!((result_normal - expected_normal).abs() <= 1e-10);
    }

    #[test]
    fn test_zero_voltage_passes_load_through_unscaled() {
        // V=0.0 with no island source is a dead bus. Every load is
        // force-gated off at the control level in that state, so the port
        // load is expected to be zero; the solver deliberately holds the
        // ZIP scale at 1.0 so any un-gated (leaked) load stays fully
        // visible at the meter instead of being attenuated by the
        // ZIP polynomial (see `effective_load_scale`).
        let zip = SolverZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
        let load_w = 10000.0;

        let result_zero = run_with_voltage(zip, 0.0, load_w);
        assert!(
            (result_zero - 10_000.0).abs() <= 1e-7,
            "dead-bus scale must be 1.0 (leak-visible), got {result_zero} W"
        );

        // A merely sagged (nonzero) voltage evaluates the ZIP polynomial
        // at the actual pu: V=0.5 and V=0.9 now answer DIFFERENTLY (the
        // deep-sag band carries information), and both stay positive.
        let result_sag = run_with_voltage(zip, 0.5, load_w);
        let result_bound = run_with_voltage(zip, 0.9, load_w);
        assert!((result_sag - result_bound).abs() > 1e-6);
        assert!(result_sag > 0.0);
    }

    // The brownout witness (axis A5): a loaded dwelling's electrical
    // domain stepped through a sag must answer with the ZIP polynomial
    // evaluated at the ACTUAL per-unit voltage, not the clamped modeling
    // band's flat line. The voltage channel admits [0.5, 1.5] pu (the
    // outage setter's band); the deep-sag band is exactly where the
    // study's information lives, so the load model's response must cover
    // it.
    #[test]
    fn sag_step_responds_at_the_true_pu_across_the_admitted_band() {
        // IEEE Task Force residential ZIP: Z=0.2, I=0.2, P=0.6.
        let zip = SolverZipCoefficients::new(0.2, 0.2, 0.6).unwrap();
        let load_w = 10_000.0;
        for (voltage_pu, expected_scale) in [
            (0.5, 0.2 * 0.25 + 0.2 * 0.5 + 0.6),
            (0.7, 0.2 * 0.49 + 0.2 * 0.7 + 0.6),
            (0.88, 0.2 * 0.88_f64.powi(2) + 0.2 * 0.88 + 0.6),
            (1.3, 0.2 * 1.69 + 0.2 * 1.3 + 0.6),
            (1.5, 0.2 * 2.25 + 0.2 * 1.5 + 0.6),
        ] {
            let net = run_with_voltage(zip, voltage_pu, load_w);
            let expected = load_w * expected_scale;
            assert!(
                (net - expected).abs() <= 1e-10,
                "a {voltage_pu} pu sag must scale the load by the ZIP terms \
                 evaluated at {voltage_pu} pu ({expected} W), got {net} W \
                 (the clamped flat line)"
            );
        }
    }

    // The attack plan's composition vectors at 0.7 pu: a pure-Z load
    // draws V-squared scaled, a pure-I load V scaled (the corner no
    // other test exercises), a pure-P load holds.
    #[test]
    fn brownout_vectors_scale_by_composition_at_the_true_pu() {
        let load_w = 10_000.0;
        let v = 0.7;
        let pure_z = SolverZipCoefficients::new(1.0, 0.0, 0.0).unwrap();
        assert!((run_with_voltage(pure_z, v, load_w) - load_w * v * v).abs() <= 1e-10);
        let pure_i = SolverZipCoefficients::new(0.0, 1.0, 0.0).unwrap();
        assert!((run_with_voltage(pure_i, v, load_w) - load_w * v).abs() <= 1e-10);
        let pure_p = SolverZipCoefficients::new(0.0, 0.0, 1.0).unwrap();
        assert!((run_with_voltage(pure_p, v, load_w) - load_w).abs() <= 1e-10);
    }

    #[test]
    fn zip_load_scale_evaluates_the_true_pu_across_the_admitted_band() {
        // The scale at the admitted band's edges is the polynomial
        // evaluated at that voltage: no clamp. Ensures the invariant
        // checker and observer (which call zip_load_scale directly) get
        // the same scale as resolve() across the full admitted band.
        let zip = SolverZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
        let solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();

        // V=0.5 (the admitted band's floor): scale = 0.5*0.5² + 0.3*0.5 + 0.2.
        let scale_low = solver.zip_load_scale(0.5);
        let expected_low = 0.5 * 0.5_f64.powi(2) + 0.3 * 0.5 + 0.2;
        assert!((scale_low - expected_low).abs() <= 1e-10);

        // V=1.5 (the admitted band's ceiling): scale = 0.5*2.25 + 0.3*1.5 + 0.2.
        let scale_high = solver.zip_load_scale(1.5);
        let expected_high = 0.5 * 1.5_f64.powi(2) + 0.3 * 1.5 + 0.2;
        assert!((scale_high - expected_high).abs() <= 1e-10);
    }

    #[test]
    fn effective_load_scale_uses_bus_voltage_and_passes_dead_bus_through() {
        use hares_types::GridState;
        let zip = SolverZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
        let solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();

        // Utility up: identical to zip_load_scale at the utility voltage.
        let up = GridState {
            voltage_pu: 0.95,
            frequency_hz: 60.0,
            island_bus_voltage_pu: None,
        };
        assert!((solver.effective_load_scale(&up) - solver.zip_load_scale(0.95)).abs() <= 1e-12);

        // Islanded: the backup source holds the bus at nominal → scale is
        // evaluated at the island bus voltage, not the (zero) utility voltage.
        let islanded = GridState {
            voltage_pu: 0.0,
            frequency_hz: 60.0,
            island_bus_voltage_pu: Some(1.0),
        };
        assert!((solver.effective_load_scale(&islanded) - 1.0).abs() <= 1e-12);

        // Dead bus: scale held at 1.0 so any un-gated (leaked) load stays
        // fully visible at the meter instead of being ZIP-attenuated.
        let dead = GridState {
            voltage_pu: 0.0,
            frequency_hz: 60.0,
            island_bus_voltage_pu: None,
        };
        assert_eq!(solver.effective_load_scale(&dead), 1.0);
    }

    #[test]
    fn resolve_scales_loads_at_island_bus_voltage_during_outage() {
        // 10 kW of (gated-off-in-practice, but here synthetic) load during
        // islanded operation must be scaled at the island bus voltage (1.0)
        // — not clamped-to-0.9 utility voltage.
        let zip = SolverZipCoefficients::new(0.5, 0.3, 0.2).unwrap();
        let mut solver = ElectricalSolver::new(ElectricalSolverConfig {
            zip,
            nominal_voltage_pu: 1.0,
        })
        .unwrap();
        let mut env = env_with_voltage(0.0);
        env.grid.island_bus_voltage_pu = Some(1.0);
        let mut ports = PortSlots::default();
        ports
            .accumulate(&PortContribution::Electrical {
                active_power_w: 10_000.0,
                reactive_power_var: 0.0,
            })
            .unwrap();
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        assert!((solver.net_active_w() - 10_000.0).abs() <= 1e-7);
    }
}
