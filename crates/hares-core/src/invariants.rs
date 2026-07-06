//! Per-timestep numerical invariant checks for the dwelling simulation loop.
//!
//! Checks are compiled and executed when either:
//! - The `check_invariants` Cargo feature is enabled, or
//! - The build has `debug_assertions` enabled (i.e., `cargo build` / `cargo test`
//!   without `--release`).
//!
//! In production release builds without the feature flag all public functions
//! compile to nothing -- the compiler eliminates the bodies entirely.

#[cfg(any(debug_assertions, test, feature = "check_invariants"))]
use std::collections::HashMap;

use hares_types::ports::FuelAccumulator;
#[cfg(any(debug_assertions, test, feature = "check_invariants"))]
use hares_types::ports::{ALL_FUEL_TYPES, FUEL_TYPE_COUNT, fuel_index_reverse};
use hares_types::{ControlCapabilities, HaresError, ZoneId};

/// Entrypoint for per-timestep numerical invariant validation.
///
/// Construct once per dwelling; call the `check_*` methods each timestep
/// inside a `cfg(any(debug_assertions, feature = "check_invariants"))` block.
/// All methods return `Ok(())` in unchecked builds.
pub struct InvariantChecker {
    /// Per-zone accumulated heating energy [Wh]. Used by
    /// [`check_heating_accumulator`] to detect sign errors that produce
    /// persistently negative cumulative totals.
    #[cfg(any(test, debug_assertions, feature = "check_invariants"))]
    total_heating_wh: HashMap<ZoneId, f64>,
    /// Per-zone accumulated cooling energy [Wh]. See [`total_heating_wh`].
    #[cfg(any(test, debug_assertions, feature = "check_invariants"))]
    total_cooling_wh: HashMap<ZoneId, f64>,
}

impl InvariantChecker {
    pub fn new() -> Self {
        Self {
            #[cfg(any(test, debug_assertions, feature = "check_invariants"))]
            total_heating_wh: HashMap::new(),
            #[cfg(any(test, debug_assertions, feature = "check_invariants"))]
            total_cooling_wh: HashMap::new(),
        }
    }
}

impl Default for InvariantChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, debug_assertions, feature = "check_invariants"))]
impl InvariantChecker {
    /// Verifies energy conservation across the thermal domain for one zone.
    ///
    /// The check asserts:
    /// `|Σ(Q_gain) − ΔE_storage − Q_loss_envelope| < max(1.0, 1e-6 · Σ|Q_gain_i|)`
    ///
    /// All values in watts [W].
    pub fn check_thermal(
        &self,
        q_gains: &[f64],
        delta_e_storage: f64,
        q_loss: f64,
    ) -> Result<(), HaresError> {
        // Guard against NaN/Inf propagating through the comparison below:
        // `NaN >= tolerance` evaluates to `false`, silently passing the check.
        // `check_temperatures` uses the same `is_finite()` guard pattern.
        for &q in q_gains {
            if !q.is_finite() {
                return Err(HaresError::InvariantViolation {
                    check_name: "thermal_balance".to_string(),
                    value: q,
                    tolerance: 0.0,
                });
            }
        }
        if !delta_e_storage.is_finite() {
            return Err(HaresError::InvariantViolation {
                check_name: "thermal_balance".to_string(),
                value: delta_e_storage,
                tolerance: 0.0,
            });
        }
        if !q_loss.is_finite() {
            return Err(HaresError::InvariantViolation {
                check_name: "thermal_balance".to_string(),
                value: q_loss,
                tolerance: 0.0,
            });
        }
        let q_sum: f64 = q_gains.iter().sum();
        let residual = (q_sum - delta_e_storage - q_loss).abs();
        // Use gross flux (sum of absolute values) for relative tolerance,
        // not net sum -- a balanced system with large opposed fluxes still has
        // floating-point accumulation error proportional to gross magnitude.
        let gross_flux: f64 = q_gains.iter().map(|q| q.abs()).sum();
        let tolerance = f64::max(1.0, 1e-6 * gross_flux);
        if residual >= tolerance {
            return Err(HaresError::InvariantViolation {
                check_name: "thermal_balance".to_string(),
                value: residual,
                tolerance,
            });
        }
        Ok(())
    }

    /// Verifies reactive power balance across the bus.
    ///
    /// The check asserts:
    /// `|Q_solver − Q_ports| < max(0.001, 1e-6 · gross_reactive_flux)` [kvar]
    ///
    /// Unlike active power, reactive power is accumulated as one signed sum
    /// (no load/gen split).  The gross reactive flux is therefore
    /// `max(|q_solver|, |q_ports|)` — the larger of the two magnitudes
    /// dominates floating-point error and avoids an abs-sum that would
    /// double-count the same physical quantity.
    pub fn check_reactive(&self, q_solver: f64, q_ports: f64) -> Result<(), HaresError> {
        if !q_solver.is_finite() {
            return Err(HaresError::InvariantViolation {
                check_name: "reactive_balance".to_string(),
                value: q_solver,
                tolerance: 0.0,
            });
        }
        if !q_ports.is_finite() {
            return Err(HaresError::InvariantViolation {
                check_name: "reactive_balance".to_string(),
                value: q_ports,
                tolerance: 0.0,
            });
        }
        let residual = (q_solver - q_ports).abs();
        // Gross reactive flux: max(|solver|, |ports|).  The two sides
        // represent the same signed bus quantity, so an abs-sum would be
        // ~2× the true gross flux.  max() is the defensible single-sided
        // floor that still scales with system size.
        let gross_flux = f64::max(q_solver.abs(), q_ports.abs());
        let tolerance = f64::max(0.001, 1e-6 * gross_flux);
        if residual >= tolerance {
            return Err(HaresError::InvariantViolation {
                check_name: "reactive_balance".to_string(),
                value: residual,
                tolerance,
            });
        }
        Ok(())
    }

    /// Verifies electrical power balance across the bus.
    ///
    /// The check asserts:
    /// `|P_grid + Σ P_equipment_ports| < max(0.001, 1e-6 · Σ|P_i|)` [kW]
    pub fn check_electrical(
        &self,
        p_grid: f64,
        p_equipment_ports: &[f64],
    ) -> Result<(), HaresError> {
        // Guard against NaN/Inf propagating through the comparison below:
        // `NaN >= tolerance` evaluates to `false`, silently passing the check.
        // `check_temperatures` uses the same `is_finite()` guard pattern.
        if !p_grid.is_finite() {
            return Err(HaresError::InvariantViolation {
                check_name: "electrical_balance".to_string(),
                value: p_grid,
                tolerance: 0.0,
            });
        }
        for &p in p_equipment_ports {
            if !p.is_finite() {
                return Err(HaresError::InvariantViolation {
                    check_name: "electrical_balance".to_string(),
                    value: p,
                    tolerance: 0.0,
                });
            }
        }
        let p_sum: f64 = p_equipment_ports.iter().sum();
        let residual = (p_grid + p_sum).abs();
        // Use gross flux (sum of absolute values of all terms on the bus)
        // for relative tolerance — a balanced system with large opposed fluxes
        // still has floating-point accumulation error proportional to gross
        // magnitude. Floor of 0.001 kW (1 W) for near-zero systems.
        let gross_flux: f64 = p_grid.abs() + p_equipment_ports.iter().map(|p| p.abs()).sum::<f64>();
        let tolerance = f64::max(0.001, 1e-6 * gross_flux);
        if residual >= tolerance {
            return Err(HaresError::InvariantViolation {
                check_name: "electrical_balance".to_string(),
                value: residual,
                tolerance,
            });
        }
        Ok(())
    }

    /// Verifies moisture mass conservation with independent source/sink tracking.
    ///
    /// The check compares independently tracked moisture sources and sinks against
    /// the humidity solver's actual output. Unlike the previous implementation which
    /// was a mathematical tautology (both sides derived from the same `Q_latent` and
    /// `moisture_buffering_multiplier`), this version:
    ///
    /// 1. Independently enumerates physical moisture sources (equipment latent gains)
    ///    and sinks (dehumidification, condensation, infiltration exchange).
    /// 2. Computes the expected air moisture change using the expected buffering
    ///    multiplier — catching regressions where the solver uses a wrong multiplier.
    /// 3. Monitors the net sorption residual (physical − actual) as a physically
    ///    meaningful quantity representing moisture absorbed/desorbed by building
    ///    materials.
    ///
    /// Two invariant checks are performed:
    ///
    /// **Moisture balance**: `|expected_balance_kg − actual_delta_kg| < max(5e-4, 1e-4 · gross_kg)`
    /// Catches missing source contributions, extra sources, or incorrect buffering
    /// multiplier when the independently-tracked accounting disagrees with the solver.
    ///
    /// **Sorption bound**: `|independent_physical_kg − actual_delta_kg| < max(1.0, 5.0 · gross_kg)`
    /// Catches absurdly large buffering multipliers by bounding the material sorption
    /// residual to multiple times the zone moisture inventory. Fires when sorption
    /// exceeds either 1 kg absolute or 5× the zone air moisture content — conditions
    /// that indicate a runaway multiplier, a simulation input error, or a solver
    /// logic bug.
    pub fn check_moisture(
        &self,
        independent_physical_kg: f64,
        expected_balance_kg: f64,
        actual_delta_kg: f64,
        gross_moisture_mass_kg: f64,
    ) -> Result<(), HaresError> {
        // Guard against NaN/Inf propagating through the comparison below:
        // `NaN >= tolerance` evaluates to `false`, silently passing the check.
        // `check_temperatures` uses the same `is_finite()` guard pattern.
        if !independent_physical_kg.is_finite() {
            return Err(HaresError::InvariantViolation {
                check_name: "moisture_balance".to_string(),
                value: independent_physical_kg,
                tolerance: 0.0,
            });
        }
        if !expected_balance_kg.is_finite() {
            return Err(HaresError::InvariantViolation {
                check_name: "moisture_balance".to_string(),
                value: expected_balance_kg,
                tolerance: 0.0,
            });
        }
        if !actual_delta_kg.is_finite() {
            return Err(HaresError::InvariantViolation {
                check_name: "moisture_balance".to_string(),
                value: actual_delta_kg,
                tolerance: 0.0,
            });
        }
        if !gross_moisture_mass_kg.is_finite() {
            return Err(HaresError::InvariantViolation {
                check_name: "moisture_balance".to_string(),
                value: gross_moisture_mass_kg,
                tolerance: 0.0,
            });
        }
        // Check 1: moisture balance — independent accounting vs solver output.
        let balance_residual = (expected_balance_kg - actual_delta_kg).abs();
        // Floor of 5e-4 kg (0.5 g) absorbs floating-point noise from the
        // solver's round-trip multiplication (dW = Q·dt/(hfg·ρ·V·M); actual = dW·ρ·V)
        // vs the invariant's direct computation (Q·dt/hfg/M). The relative factor
        // scales with zone moisture mass so large zones aren't over-tolerated.
        let balance_tolerance = f64::max(5e-4, 1e-4 * gross_moisture_mass_kg);
        if balance_residual >= balance_tolerance {
            return Err(HaresError::InvariantViolation {
                check_name: "moisture_balance".to_string(),
                value: balance_residual,
                tolerance: balance_tolerance,
            });
        }

        // Check 2: sorption bound — physical net vs apparent air moisture change.
        let sorption_residual = (independent_physical_kg - actual_delta_kg).abs();
        let sorption_tolerance = f64::max(1e0, 5.0 * gross_moisture_mass_kg);
        if sorption_residual >= sorption_tolerance {
            return Err(HaresError::InvariantViolation {
                check_name: "moisture_sorption".to_string(),
                value: sorption_residual,
                tolerance: sorption_tolerance,
            });
        }
        Ok(())
    }

    /// Validates state-of-charge is within `[0.0, 1.0]` and warns if accumulated
    /// integration error exceeds threshold.
    ///
    /// Returns `Ok(())` always -- SoC out-of-bounds is clamped (not a fatal error).
    /// Emits `tracing::warn!` when `accumulated_error.abs() > 0.001`.
    pub fn check_soc(&self, soc: f64, accumulated_error: f64) -> Result<(), HaresError> {
        let clamped = soc.clamp(0.0, 1.0);
        if (clamped - soc).abs() > f64::EPSILON {
            tracing::warn!(
                soc = soc,
                clamped = clamped,
                "SoC out of [0, 1] range; clamped"
            );
        }
        if accumulated_error.abs() > 0.001 {
            tracing::warn!(
                accumulated_error = accumulated_error,
                "SoC accumulated integration error exceeds 0.001"
            );
        }
        Ok(())
    }

    /// Validates that zone and tank temperatures are within physically plausible bounds.
    ///
    /// - Conditioned zone temperatures: `[-50, 80]` °C
    /// - Unconditioned zone temperatures: `[-50, 120]` °C (attics under solar load
    ///   routinely exceed 80 °C in hot climates)
    /// - Tank temperatures: `[0, 100]` °C
    ///
    /// Returns the first violation found, or `Ok(())`.
    pub fn check_temperatures(
        &self,
        conditioned_zone_temps_c: &[f64],
        unconditioned_zone_temps_c: &[f64],
        tank_temps_c: &[f64],
    ) -> Result<(), HaresError> {
        const ZONE_MIN_C: f64 = -50.0;
        const CONDITIONED_MAX_C: f64 = 80.0;
        const UNCONDITIONED_MAX_C: f64 = 120.0;
        const TANK_MIN_C: f64 = 0.0;
        const TANK_MAX_C: f64 = 100.0;

        for &t in conditioned_zone_temps_c {
            if !t.is_finite() || !(ZONE_MIN_C..=CONDITIONED_MAX_C).contains(&t) {
                return Err(HaresError::InvariantViolation {
                    check_name: "zone_temperature_bounds".to_string(),
                    value: t,
                    tolerance: 0.0,
                });
            }
        }
        for &t in unconditioned_zone_temps_c {
            if !t.is_finite() || !(ZONE_MIN_C..=UNCONDITIONED_MAX_C).contains(&t) {
                return Err(HaresError::InvariantViolation {
                    check_name: "unconditioned_zone_temperature_bounds".to_string(),
                    value: t,
                    tolerance: 0.0,
                });
            }
        }
        for &t in tank_temps_c {
            if !t.is_finite() || !(TANK_MIN_C..=TANK_MAX_C).contains(&t) {
                return Err(HaresError::InvariantViolation {
                    check_name: "tank_temperature_bounds".to_string(),
                    value: t,
                    tolerance: 0.0,
                });
            }
        }
        Ok(())
    }

    /// Verifies that at least one equipment in the dwelling declares the
    /// `PROTOCOL_NATIVE` capability, so a `ProtocolNative` dispatch has a
    /// consumer. Without a registered consumer the dispatch is silently
    /// rejected by the capability gate and the signal never reaches any
    /// equipment.
    ///
    /// Called before dispatching `ProtocolNative` signals in the dwelling's
    /// Step 2b derived-signal cascade. An empty `capability_sets` — a dwelling
    /// with no equipment at all — always fails this check.
    pub fn check_protocol_native_registration(
        &self,
        capability_sets: &[ControlCapabilities],
    ) -> Result<(), HaresError> {
        let has_consumer = capability_sets
            .iter()
            .any(|caps| caps.contains(ControlCapabilities::PROTOCOL_NATIVE));
        if !has_consumer {
            return Err(HaresError::InvariantViolation {
                check_name: "protocol_native_registration".to_string(),
                value: capability_sets.len() as f64,
                tolerance: 0.0,
            });
        }
        Ok(())
    }

    /// Verifies that every non-zero slot in the fuel accumulator maps to a fuel
    /// type present in `ALL_FUEL_TYPES`, the single source of truth used by the
    /// observer capture layer to enumerate captured fuel types.
    ///
    /// If `ALL_FUEL_TYPES` or `FUEL_TYPE_COUNT` is updated without the other,
    /// a non-zero accumulator slot may have no corresponding observer entry,
    /// silently dropping the fuel contribution from diagnostic output.
    ///
    /// Takes a `FuelAccumulator` reference and iterates `ALL_FUEL_TYPES` to
    /// verify each accumulator index is covered; if a non-zero slot exists at
    /// an index that maps to a fuel type absent from `ALL_FUEL_TYPES`, the
    /// observer would not capture it — this check catches that desynchronisation.
    pub fn check_fuel_coverage(&self, fuel: &FuelAccumulator) -> Result<(), HaresError> {
        use hares_types::ports::fuel_index;
        if ALL_FUEL_TYPES.len() != FUEL_TYPE_COUNT {
            return Err(HaresError::InvariantViolation {
                check_name: "fuel_observer_coverage".to_string(),
                value: ALL_FUEL_TYPES.len() as f64,
                tolerance: FUEL_TYPE_COUNT as f64,
            });
        }
        for (i, &ft) in ALL_FUEL_TYPES.iter().enumerate() {
            match fuel_index(ft) {
                Some(idx) if idx == i => {}
                Some(idx) => {
                    return Err(HaresError::InvariantViolation {
                        check_name: "fuel_observer_coverage".to_string(),
                        value: idx as f64,
                        tolerance: i as f64,
                    });
                }
                None => {
                    return Err(HaresError::InvariantViolation {
                        check_name: "fuel_observer_coverage".to_string(),
                        value: (ft as i32) as f64,
                        tolerance: 0.0,
                    });
                }
            }
        }
        for i in 0..FUEL_TYPE_COUNT {
            let ft = match fuel_index_reverse(i) {
                Some(ft) => ft,
                None => {
                    return Err(HaresError::InvariantViolation {
                        check_name: "fuel_observer_coverage".to_string(),
                        value: i as f64,
                        tolerance: 0.0,
                    });
                }
            };
            // ALL_FUEL_TYPES is [0..FUEL_TYPE_COUNT), so ft must be in range.
            let acc_value = fuel.get(ft);
            if acc_value > 0.0 && !ALL_FUEL_TYPES.contains(&ft) {
                tracing::warn!(
                    fuel_type = ?ft,
                    fuel_type_idx = i,
                    accumulator_value_w = acc_value,
                    "non-zero fuel accumulator slot has no observer coverage"
                );
            }
        }
        Ok(())
    }

    /// Verifies that the fuel accumulator contains no electric contributions.
    ///
    /// Electric power must route through `ElectricalAccumulator` — never through
    /// the fuel accumulator. A non-zero electric slot in the fuel accumulator
    /// indicates equipment is mistakenly treating electricity as a fuel.
    pub fn check_fuel_electric_absent(&self, electric_fuel_w: f64) -> Result<(), HaresError> {
        if electric_fuel_w > 0.0 {
            return Err(HaresError::InvariantViolation {
                check_name: "electric_fuel_in_fuel_accumulator".to_string(),
                value: electric_fuel_w,
                tolerance: 0.0,
            });
        }
        Ok(())
    }

    /// Verifies that equipment step execution order respects `stage_rank` ordering.
    ///
    /// The `stage_ranks` slice contains the `stage_rank()` values for each
    /// equipment in the order they were stepped — collected at runtime from the
    /// actual step loops. These must be non-decreasing (Independent < Electrical
    /// < Thermal). A violation indicates equipment was stepped out of the
    /// documented stage order by the dwelling's step loop.
    pub fn check_equipment_step_order(&self, stage_ranks: &[u8]) -> Result<(), HaresError> {
        for window in stage_ranks.windows(2) {
            if window[0] > window[1] {
                return Err(HaresError::InvariantViolation {
                    check_name: "equipment_step_order".to_string(),
                    value: window[0] as f64,
                    tolerance: window[1] as f64,
                });
            }
        }
        Ok(())
    }

    /// Screens key float values for NaN before any residual computation.
    ///
    /// NaN propagation is a silent correctness hazard: `NaN >= tolerance`
    /// evaluates to `false`, so a NaN-bearer passes every comparison-based
    /// invariant check without detection.  This method provides a dedicated,
    /// context-rich NaN-screening gate that fires at the earliest possible
    /// phase boundary — before any residual is computed.
    ///
    /// Called from the dwelling's [`check_invariants`] at the top of the
    /// per-step invariant pass, and from diagnostic / telemetry construction
    /// sites as a backstop.
    ///
    /// Returns `Err(HaresError::NanDetected { .. })` on the first non-finite
    /// value found.
    pub fn check_nan_screen(
        &self,
        step_index: u64,
        values: &[(&str, Option<ZoneId>, f64)],
    ) -> Result<(), HaresError> {
        for &(name, zone_id, value) in values {
            if !value.is_finite() {
                return Err(HaresError::NanDetected {
                    step_index,
                    zone_id,
                    value_name: name.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Checks that per-step HVAC delivered power follows the physical sign
    /// convention.
    ///
    /// By the `EnvelopeComponentGains` convention, positive = heat flowing
    /// **into** the indoor zone:
    /// - `hvac_heating_w >= 0.0` — heating adds heat; negative would indicate
    ///   a sign error in equipment port contributions or a COP reciprocity bug.
    /// - `hvac_cooling_w <= 0.0` — cooling removes heat; positive would
    ///   indicate a sign inversion (e.g. a cooling coil producing sensible
    ///   gain instead of removal).
    ///
    /// This check fires on the raw component gains before the historical
    /// clamping `max(0.0)` / `abs()` converted the signed convention to
    /// delivered-energy magnitudes.
    pub fn check_hvac_power_non_negative(
        &self,
        step_index: u64,
        zone_id: ZoneId,
        hvac_heating_w: f64,
        hvac_cooling_w: f64,
    ) -> Result<(), HaresError> {
        if hvac_heating_w < 0.0 {
            return Err(HaresError::NegativeDeliveredEnergy {
                zone_id: Some(zone_id),
                field: "hvac_heating_w".to_string(),
                value: hvac_heating_w,
                step_index,
            });
        }
        if hvac_cooling_w > 0.0 {
            return Err(HaresError::NegativeDeliveredEnergy {
                zone_id: Some(zone_id),
                field: "hvac_cooling_w".to_string(),
                value: hvac_cooling_w,
                step_index,
            });
        }
        Ok(())
    }

    /// Accumulates per-zone heating/cooling energy and checks cumulative
    /// sign-convention consistency.
    ///
    /// Each call adds this step's energy (`power_w × dt_h`) to the per-zone
    /// running total.  Heating should accumulate non-negative; cooling should
    /// accumulate non-positive (cooling removes heat from the zone by
    /// convention).  The check tolerates floating-point drift proportional
    /// to step count while catching genuine sign errors that produce a
    /// persistent upward/downward trend in the wrong direction.
    pub fn check_heating_accumulator(
        &mut self,
        step_index: u64,
        zone_id: ZoneId,
        hvac_heating_w: f64,
        hvac_cooling_w: f64,
        dt_h: f64,
    ) -> Result<(), HaresError> {
        let heating_wh = hvac_heating_w * dt_h;
        let cooling_wh = hvac_cooling_w * dt_h;
        let total_heating = self.total_heating_wh.entry(zone_id).or_insert(0.0);
        *total_heating += heating_wh;
        let total_cooling = self.total_cooling_wh.entry(zone_id).or_insert(0.0);
        *total_cooling += cooling_wh;
        // -1e-6 Wh ≈ -3.6 mJ absorbency: at f64 precision (≈ 15 decimal digits)
        // a single Wh-order accumulation carries ~1e-15 relative error, far below
        // this floor.  The floor at -1e-6 Wh prevents noise-triggered violations
        // at step 0 (where the proportional term would be zero) while the
        // -1e-6 × step_index term grows linearly to accommodate accumulated
        // round-trip drift across many arithmetic operations over long runs.
        let drift_neg = f64::min(-1e-6, -1e-6 * step_index as f64);
        // Heating cumulative must not go negative (beyond drift).
        if *total_heating < drift_neg {
            return Err(HaresError::NegativeDeliveredEnergy {
                zone_id: Some(zone_id),
                field: "total_heating_wh".to_string(),
                value: *total_heating,
                step_index,
            });
        }
        // Cooling cumulative must not go positive (beyond drift).
        // Cooling stores negative values (heat removed from zone);
        // a positive total indicates a sign error on the cooling path.
        let drift_pos = -drift_neg;
        if *total_cooling > drift_pos {
            return Err(HaresError::NegativeDeliveredEnergy {
                zone_id: Some(zone_id),
                field: "total_cooling_wh".to_string(),
                value: *total_cooling,
                step_index,
            });
        }
        Ok(())
    }
}

#[cfg(not(any(test, debug_assertions, feature = "check_invariants")))]
impl InvariantChecker {
    pub fn check_thermal(&self, _: &[f64], _: f64, _: f64) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_reactive(&self, _: f64, _: f64) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_electrical(&self, _: f64, _: &[f64]) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_moisture(&self, _: f64, _: f64, _: f64, _: f64) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_soc(&self, _: f64, _: f64) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_temperatures(&self, _: &[f64], _: &[f64], _: &[f64]) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_protocol_native_registration(
        &self,
        _: &[ControlCapabilities],
    ) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_fuel_coverage(&self, _: &FuelAccumulator) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_fuel_electric_absent(&self, _: f64) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_equipment_step_order(&self, _: &[u8]) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_nan_screen(
        &self,
        _: u64,
        _: &[(&str, Option<ZoneId>, f64)],
    ) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_hvac_power_non_negative(
        &self,
        _: u64,
        _: ZoneId,
        _: f64,
        _: f64,
    ) -> Result<(), HaresError> {
        Ok(())
    }

    pub fn check_heating_accumulator(
        &mut self,
        _: u64,
        _: ZoneId,
        _: f64,
        _: f64,
        _: f64,
    ) -> Result<(), HaresError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hares_types::FuelType;

    fn checker() -> InvariantChecker {
        InvariantChecker::new()
    }

    // ── thermal_balance ───────────────────────────────────────────────────────

    #[test]
    fn thermal_balance_passes_when_balanced() {
        // 1000 W gain, 200 W stored, 800 W lost → residual = 0
        let result = checker().check_thermal(&[1000.0], 200.0, 800.0);
        assert!(result.is_ok());
    }

    #[test]
    fn thermal_balance_fails_on_sign_flipped_gain() {
        // Deliberately broken: gain reported as -1000 W instead of +1000 W.
        // Σ = -1000, storage = 200, loss = 800 → residual = |-1000 - 200 - 800| = 2000.
        // tolerance = max(1.0, 1e-6 * 1000) = 1.0 → should fail.
        let result = checker().check_thermal(&[-1000.0], 200.0, 800.0);
        assert!(
            result.is_err(),
            "sign-flipped gain must trigger thermal invariant"
        );
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "thermal_balance"
        ));
    }

    #[test]
    fn thermal_balance_uses_relative_tolerance_for_large_gains() {
        // With Q_sum = 1e8 W, tolerance = 1e-6 * 1e8 = 100 W.
        // Residual = 50 W → passes.
        let result = checker().check_thermal(&[1e8], 0.0, 1e8 - 50.0);
        assert!(result.is_ok());

        // Residual = 200 W → fails (> 100 W tolerance).
        let result = checker().check_thermal(&[1e8], 0.0, 1e8 - 200.0);
        assert!(result.is_err());
    }

    // ── electrical_balance ────────────────────────────────────────────────────

    #[test]
    fn electrical_balance_passes_when_balanced() {
        // Grid supplies 3 kW, equipment draws 3 kW → net = 0.
        let result = checker().check_electrical(3.0, &[-3.0]);
        assert!(result.is_ok());
    }

    #[test]
    fn electrical_balance_fails_when_imbalanced() {
        // Grid 3 kW, equipment 2 kW → residual = 1 kW > 0.001 kW.
        let result = checker().check_electrical(3.0, &[-2.0]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "electrical_balance"
        ));
    }

    #[test]
    fn electrical_balance_passes_with_zip_adjusted_port_load() {
        // IEEE residential ZIP: Z=0.2, I=0.2, P=0.6 at V=0.95 pu.
        // scale = 0.2·0.95² + 0.2·0.95 + 0.6 = 0.2·0.9025 + 0.19 + 0.6 = 0.9705
        // p_load = 10 kW, p_gen = 0 kW.
        let scale = 0.2_f64 * 0.95_f64.powi(2) + 0.2 * 0.95 + 0.6;
        let p_load = 10.0_f64;
        let p_gen = 0.0_f64;
        let p_grid = p_load * scale + p_gen; // ZIP-adjusted solver output
        let port_net = p_load * scale + p_gen; // ZIP-adjusted port accumulation
        // When both sides use the same scaling, the residual is near-zero.
        let result = checker().check_electrical(p_grid, &[-port_net]);
        assert!(
            result.is_ok(),
            "ZIP-adjusted comparison should pass; residual should be < 0.001 kW"
        );
    }

    #[test]
    fn electrical_balance_fails_with_mismatched_zip_scaling() {
        // Demonstrates the bug: comparing ZIP-adjusted grid against raw ports
        // produces a false positive when scale ≠ 1.0.
        let scale = 0.2_f64 * 0.95_f64.powi(2) + 0.2 * 0.95 + 0.6;
        let p_load = 10.0_f64;
        let p_gen = 0.0_f64;
        let p_grid = p_load * scale + p_gen; // ZIP-adjusted solver output
        let port_net_raw = p_load + p_gen; // raw (unadjusted) port
        let result = checker().check_electrical(p_grid, &[-port_net_raw]);
        assert!(
            result.is_err(),
            "Comparing ZIP-adjusted grid vs raw ports with scale={scale} should produce false positive"
        );
    }

    #[test]
    fn electrical_balance_tolerance_scales_with_system_size() {
        // 10,000 kW system: gross_flux ≈ 20,000 kW → tolerance ≈ max(0.001, 1e-6*20000) = 0.02 kW.
        // A 0.010 kW residual is under tolerance and should pass.
        let p_grid = 10000.0;
        let p_equipment = -9999.99; // residual = |10000 - 9999.99| = 0.01
        let result = checker().check_electrical(p_grid, &[p_equipment]);
        assert!(
            result.is_ok(),
            "0.01 kW residual should pass for 10,000 kW system (tolerance ≈ 0.02 kW)"
        );
    }

    #[test]
    fn electrical_balance_fails_when_residual_exceeds_scaled_tolerance() {
        // Same 10,000 kW system: tolerance ≈ 0.02 kW. 0.03 kW residual exceeds it.
        let p_grid = 10000.0;
        let p_equipment = -9999.97; // residual = |10000 - 9999.97| = 0.03
        let result = checker().check_electrical(p_grid, &[p_equipment]);
        assert!(
            result.is_err(),
            "0.03 kW residual should fail for 10,000 kW system (tolerance ≈ 0.02 kW)"
        );
    }

    #[test]
    fn electrical_balance_catches_small_residual_on_tiny_system() {
        // 0.05 kW system: gross_flux ≈ 0.1 kW → tolerance = floor = 0.001 kW.
        // 0.002 kW residual > 0.001 kW floor → must fail.
        let p_grid = 0.052;
        let p_equipment = -0.05; // residual = |0.052 - 0.05| = 0.002
        let result = checker().check_electrical(p_grid, &[p_equipment]);
        assert!(
            result.is_err(),
            "0.002 kW residual should fail for 0.05 kW system (tolerance floor = 0.001 kW)"
        );
    }

    #[test]
    fn electrical_balance_floor_tolerance_works_near_zero() {
        // Near-zero system: gross_flux small → tolerance = floor = 0.001 kW.
        // Residual of 0.0005 kW is under floor and should pass.
        let result = checker().check_electrical(0.0, &[0.0005]);
        assert!(result.is_ok());
    }

    // ── reactive_balance ──────────────────────────────────────────────────────

    #[test]
    fn reactive_balance_passes_when_balanced() {
        let result = checker().check_reactive(1.5, 1.5);
        assert!(result.is_ok());
    }

    #[test]
    fn reactive_balance_fails_when_imbalanced() {
        let result = checker().check_reactive(2.0, 1.0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "reactive_balance"
        ));
    }

    #[test]
    fn reactive_balance_with_negative_q_values() {
        // Signed Q: negative = supplying vars (PV/battery).  Both sides agree.
        let result = checker().check_reactive(-3.0, -3.0);
        assert!(result.is_ok());
    }

    #[test]
    fn reactive_balance_fails_with_solver_negative_ports_positive() {
        // Divergent signs: solver says -3 kvar (supplying), ports say +3 kvar
        // (absorbing).  Residual = 6 kvar > tolerance.
        let result = checker().check_reactive(-3.0, 3.0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "reactive_balance"
        ));
    }

    #[test]
    fn reactive_balance_tolerance_scales_with_system_size() {
        // 10,000 kvar system: gross_flux = 10000 → tol ≈ max(0.001, 1e-6*10000) = 0.01 kvar.
        // 0.005 kvar residual < 0.01 → passes.
        let result = checker().check_reactive(10000.0, 9999.995);
        assert!(
            result.is_ok(),
            "0.005 kvar residual should pass for 10,000 kvar system (tolerance ≈ 0.01 kvar)"
        );
    }

    #[test]
    fn reactive_balance_fails_when_residual_exceeds_scaled_tolerance() {
        // Same 10,000 kvar system → tol ≈ 0.01 kvar. 0.02 kvar residual exceeds it.
        let result = checker().check_reactive(10000.0, 9999.98);
        assert!(
            result.is_err(),
            "0.02 kvar residual should fail for 10,000 kvar system (tolerance ≈ 0.01 kvar)"
        );
    }

    #[test]
    fn reactive_balance_catches_small_residual_on_tiny_system() {
        // 0.05 kvar system: tolerance floor = 0.001 kvar. 0.002 > floor → fail.
        let result = checker().check_reactive(0.052, 0.05);
        assert!(
            result.is_err(),
            "0.002 kvar residual should fail for 0.05 kvar system (tolerance floor = 0.001 kvar)"
        );
    }

    #[test]
    fn reactive_balance_floor_tolerance_works_near_zero() {
        let result = checker().check_reactive(0.0, 0.0005);
        assert!(result.is_ok());
    }

    #[test]
    fn reactive_balance_fails_on_nan_solver() {
        let result = checker().check_reactive(f64::NAN, 1.5);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "reactive_balance"
        ));
    }

    #[test]
    fn reactive_balance_fails_on_nan_ports() {
        let result = checker().check_reactive(1.5, f64::NAN);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "reactive_balance"
        ));
    }

    // ── moisture_balance ──────────────────────────────────────────────────────

    /// When independently-tracked sources match the solver output (same multiplier, all
    /// sources accounted), both the balance and sorption checks pass.
    #[test]
    fn moisture_balance_passes_with_independent_tracking() {
        // Zone: 200 m³ at 25°C, w=0.010 → ~2.33 kg water vapor.
        // 100 W latent for 600 s → physical source = 100*600/2_501_000 ≈ 0.024 kg.
        // With M=15: actual delta = 0.024/15 ≈ 0.0016 kg.
        let h_fg = 2_501_000.0;
        let dt_s = 600.0;
        let q_latent = 100.0;
        let moisture_mult: f64 = 15.0;
        let gross_kg = 2.33;
        let physical_source = q_latent * dt_s / h_fg; // ~0.024 kg
        let expected_balance = physical_source / moisture_mult; // ~0.0016 kg
        let actual_delta = physical_source / moisture_mult; // same formula, all sources tracked
        let result =
            checker().check_moisture(physical_source, expected_balance, actual_delta, gross_kg);
        assert!(result.is_ok(), "balanced independent check should pass");
    }

    /// Missing source (occupant latent): the independent accounting sees less moisture
    /// than the solver used → balance check must fail.
    #[test]
    fn moisture_balance_fails_on_missing_source() {
        let h_fg = 2_501_000.0;
        let dt_s = 600.0;
        let moisture_mult: f64 = 15.0;
        let gross_kg = 2.33;
        // Solver sees 300 W total (200 equipment + 100 occupant).
        // Independent tracking only accounts for 200 W equipment (occupant output not wired).
        let q_total = 300.0; // solver's bundled latent
        let q_independent = 200.0; // independent port tally
        let physical_independent = q_independent * dt_s / h_fg;
        let expected_balance = physical_independent / moisture_mult; // ~0.0032 kg
        let actual_delta = q_total * dt_s / h_fg / moisture_mult; // ~0.0048 kg
        // Residual = |0.0032 − 0.0048| = 0.0016 kg > max(5e-4, 1e-4*2.33=2.33e-4) = 5e-4
        let result = checker().check_moisture(
            physical_independent,
            expected_balance,
            actual_delta,
            gross_kg,
        );
        assert!(
            result.is_err(),
            "missing occupant source must fail balance check"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(&err, HaresError::InvariantViolation { check_name, .. } if check_name == "moisture_balance"),
            "first violation should be moisture_balance, got {err:?}"
        );
    }

    /// Manipulated multiplier (30× instead of 15×): the solver uses a different M
    /// than the independent check expects. The old tautological check would pass;
    /// this check fails.
    #[test]
    fn moisture_balance_fails_on_wrong_multiplier() {
        let h_fg = 2_501_000.0;
        let dt_s = 600.0;
        let q_latent = 100.0;
        let expected_mult: f64 = 15.0; // what the invariant expects
        let actual_mult: f64 = 30.0; // what solver was manipulated to use
        let gross_kg = 2.33;
        let physical_source = q_latent * dt_s / h_fg;
        let expected_balance = physical_source / expected_mult; // 0.0016 kg
        let actual_delta = physical_source / actual_mult; // 0.0008 kg
        // Residual = |0.0016 − 0.0008| = 0.0008 kg > max(5e-4, 1e-4*2.33=2.33e-4) = 5e-4
        let result =
            checker().check_moisture(physical_source, expected_balance, actual_delta, gross_kg);
        assert!(
            result.is_err(),
            "manipulated multiplier must fail balance check"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(&err, HaresError::InvariantViolation { check_name, .. } if check_name == "moisture_balance")
        );
    }

    /// Regression: known moisture sources over a 1-hour step — the invariant residual
    /// (sorption) equals the expected material buffering contribution within tolerance.
    #[test]
    fn moisture_sorption_residual_matches_buffering() {
        let h_fg = 2_501_000.0;
        let dt_s = 3600.0; // 1 hour
        let q_latent = 100.0;
        let moisture_mult: f64 = 15.0;
        let gross_kg = 2.33;
        // Physical source mass for the step:
        let physical_source = q_latent * dt_s / h_fg; // ~0.1439 kg
        // Actual air moisture change with M=15:
        let actual_delta = physical_source / moisture_mult; // ~0.0096 kg
        // Sorption (material buffering) = physical − actual:
        let expected_sorption = physical_source - actual_delta; // ~0.1343 kg
        // The sorption check bounds the residual: |physical − actual| < max(1e-3, 0.1*gross)
        // |0.1439 − 0.0096| = 0.1343 < max(1e-3, 0.1*2.33=0.233) → passes
        // The balance check: |physical/M − actual| = 0 → passes (same M)
        let expected_balance = physical_source / moisture_mult;
        let result =
            checker().check_moisture(physical_source, expected_balance, actual_delta, gross_kg);
        assert!(
            result.is_ok(),
            "regression step should pass; expected sorption {expected_sorption:.4} < tolerance"
        );
    }

    /// The sorption check must fail when the buffering multiplier is absurdly large
    /// (e.g., 100×) causing the sorption residual to approach the full source mass.
    #[test]
    fn moisture_sorption_fails_with_runaway_multiplier() {
        let h_fg = 2_501_000.0;
        let dt_s = 600.0;
        let q_latent = 100.0;
        let expected_mult: f64 = 15.0; // expected
        let actual_mult: f64 = 100.0; // absurdly large
        let gross_kg = 0.01; // very dry zone — forces loose check to use absolute floor
        let physical_source = q_latent * dt_s / h_fg; // ~0.024 kg
        let expected_balance = physical_source / expected_mult; // ~0.0016 kg
        let actual_delta = physical_source / actual_mult; // ~0.00024 kg
        // Sorption = |0.024 − 0.00024| = 0.0238 kg > max(1.0, 5*0.01=0.05) = 1.0
        // → sorption check would also fail, but balance fires first:
        // |0.0016 − 0.00024| = 0.00136 > max(5e-4, 1e-4*0.01=1e-6) = 5e-4
        let result =
            checker().check_moisture(physical_source, expected_balance, actual_delta, gross_kg);
        assert!(result.is_err(), "100× multiplier must fail invariant check");
        let err = result.unwrap_err();
        assert!(
            matches!(&err, HaresError::InvariantViolation { check_name, .. } if check_name == "moisture_balance")
        );
    }

    /// The sorption check guards against sorption exceeding the absolute floor
    /// of 1 kg or 5× the zone moisture inventory. When the buffering produces a
    /// sorption residual that exceeds the sorption bound (but NOT the balance
    /// bound because M matches), the sorption check catches it.
    ///
    /// Scenario: an absurdly large moisture source (~10 kW latent) in a very dry
    /// zone — the per-step sorption exceeds 1 kg while the zone barely holds any
    /// moisture, indicating either a runaway multiplier or a simulation input error.
    #[test]
    fn sorption_bound_catches_excessive_buffering() {
        let h_fg = 2_501_000.0;
        let dt_s = 600.0;
        let q_latent = 10_000.0; // absurdly large latent
        let moisture_mult: f64 = 15.0;
        let gross_kg = 0.01; // very small zone moisture inventory
        let physical_source = q_latent * dt_s / h_fg; // ~2.398 kg
        let expected_balance = physical_source / moisture_mult; // ~0.160 kg
        let actual_delta = physical_source / moisture_mult; // same (balance passes)
        // Balance: |0.160 − 0.160| = 0 < 5e-4 → passes
        // Sorption: |2.398 − 0.160| = 2.238 kg > max(1.0, 5*0.01=0.05) = 1.0
        // → sorption bound fires
        let result =
            checker().check_moisture(physical_source, expected_balance, actual_delta, gross_kg);
        assert!(
            result.is_err(),
            "excessive sorption must fail sorption bound"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(&err, HaresError::InvariantViolation { check_name, .. } if check_name == "moisture_sorption"),
            "sorption check should fire when sorption exceeds bound, got {err:?}"
        );
    }

    /// Regression: dehumidifier + occupant scenario must not cancel dehumidifier
    /// contribution in the independent physical mass.
    ///
    /// In the prior implementation, `independent_physical_kg` subtracted
    /// `dehum_kg` (from the humidity port's moisture_mass_flow_kg_s) from the
    /// equipment contribution (thermal port's latent_gain_w / h_fg). Equipment
    /// that writes both ports (dehumidifiers, ACs with latent cooling, ideal
    /// HVAC) had its contribution cancel to zero. This test verifies that the
    /// invariant correctly includes the dehumidifier's moisture removal in the
    /// independent physical mass.
    #[test]
    fn moisture_balance_includes_dehumidifier_not_cancelled() {
        let h_fg = 2_501_000.0;
        let dt_s = 3600.0; // 1 hour step
        let moisture_mult: f64 = 15.0;
        let gross_kg = 2.33;

        // Occupant generates 75 W latent → 75*3600/2_501_000 ≈ 0.108 kg moisture.
        let occupant_w = 75.0;
        // Dehumidifier removes 100 W latent → -100*3600/2_501_000 ≈ -0.144 kg moisture.
        let dehum_w = -100.0;

        // Correct independent physical mass (thermal port only, no cancellation):
        // equipment_kg = (75 + (-100)) * 3600 / 2_501_000 ≈ -0.036 kg.
        let independent_physical_kg = (occupant_w + dehum_w) * dt_s / h_fg;
        // Expected balance with M=15: -0.036 / 15 ≈ -0.0024 kg.
        let expected_balance_kg = independent_physical_kg / moisture_mult;
        // Solver uses the same net equipment latent and same M → same actual delta.
        let actual_delta_kg = independent_physical_kg / moisture_mult;

        // Balance check: residual = 0 → passes.
        // Sorption check: |physical - actual| = |(-0.036) - (-0.0024)| ≈ 0.0336 kg
        //   < max(1.0, 5*2.33=11.65) = 1.0 → passes.
        let result = checker().check_moisture(
            independent_physical_kg,
            expected_balance_kg,
            actual_delta_kg,
            gross_kg,
        );
        assert!(
            result.is_ok(),
            "dehumidifier + occupant with correct independent accounting must pass; got {result:?}"
        );

        // Verify that the old (buggy) value WOULD fail.
        // Old code: independent_physical_kg = equipment_kg - dehum_kg
        //   = (occupant*dt/h_fg + dehum*dt/h_fg) - dehum*dt/h_fg
        //   = occupant*dt/h_fg (dehumidifier cancelled)
        let old_independent = occupant_w * dt_s / h_fg; // ≈ 0.108 kg
        let old_expected = old_independent / moisture_mult; // ≈ 0.0072 kg
        // actual_delta_kg is still the correct solver output (-0.0024 kg)
        // Residual = |0.0072 - (-0.0024)| = 0.0096 kg > max(5e-4, 1e-4*2.33) = 5e-4
        let old_result =
            checker().check_moisture(old_independent, old_expected, actual_delta_kg, gross_kg);
        assert!(
            old_result.is_err(),
            "old cancelled independent mass must produce false-positive invariant failure; got {old_result:?}"
        );
        let err = old_result.unwrap_err();
        assert!(
            matches!(&err, HaresError::InvariantViolation { check_name, .. } if check_name == "moisture_balance"),
            "old cancelling code should fail balance check, got {err:?}"
        );
    }

    /// Condensation mass is included in the moisture balance: when the solver
    /// clamps w_new at w_sat, the portion of moisture that condensed is added
    /// back so that `|expected − (actual + condensation)|` stays within tolerance.
    #[test]
    fn moisture_balance_passes_with_condensation_sink() {
        let h_fg = 2_501_000.0;
        let dt_s = 600.0;
        let moisture_mult: f64 = 15.0;
        let gross_kg = 2.33;
        // 300 W latent drives w_raw above w_sat, then condensation removes
        // ~0.0048 kg from the air. The solver sees only the post-clamp dW.
        let q_latent = 300.0;
        let physical_source = q_latent * dt_s / h_fg; // ~0.072 kg
        let expected_balance = physical_source / moisture_mult; // ~0.0048 kg
        // Simulate condensation removing 0.002 kg (air moisture change smaller
        // than expected because clamp truncated the peak).
        let condensation_kg = 0.002;
        let actual_delta = expected_balance - condensation_kg; // 0.0028 kg
        let adjusted_actual = actual_delta + condensation_kg; // 0.0048 kg = expected
        let result =
            checker().check_moisture(physical_source, expected_balance, adjusted_actual, gross_kg);
        assert!(
            result.is_ok(),
            "balance with condensation sink must pass; got {result:?}"
        );
    }

    /// Without accounting for condensation mass, a clamped step that would
    /// otherwise balance fails the invariant — this is the bug T-0180 fixes.
    #[test]
    fn moisture_balance_fails_when_condensation_not_accounted() {
        let h_fg = 2_501_000.0;
        let dt_s = 600.0;
        let moisture_mult: f64 = 15.0;
        let gross_kg = 2.33;
        let q_latent = 300.0;
        let physical_source = q_latent * dt_s / h_fg;
        let expected_balance = physical_source / moisture_mult;
        let condensation_kg = 0.002;
        let actual_delta = expected_balance - condensation_kg;
        // Passing actual_delta without adding condensation back — the old (buggy) call.
        // Residual = |expected − actual| = |0.0048 − 0.0028| = 0.002 kg
        // > max(5e-4, 1e-4*2.33=2.33e-4) = 5e-4 → must fail.
        let result =
            checker().check_moisture(physical_source, expected_balance, actual_delta, gross_kg);
        assert!(result.is_err(), "condensation-unaware check must fail");
    }

    // ── temperature bounds ────────────────────────────────────────────────────

    #[test]
    fn conditioned_zone_temperature_within_bounds_passes() {
        let result = checker().check_temperatures(&[20.0, -10.0], &[], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn unconditioned_zone_temperature_within_bounds_passes() {
        // 95°C is valid for an attic under solar load.
        let result = checker().check_temperatures(&[], &[95.0, 119.0], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn conditioned_zone_temperature_below_minimum_fails() {
        let result = checker().check_temperatures(&[-51.0], &[], &[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "zone_temperature_bounds"
        ));
    }

    #[test]
    fn conditioned_zone_temperature_above_maximum_fails() {
        // 80.1°C should fail for a conditioned zone.
        let result = checker().check_temperatures(&[80.1], &[], &[]);
        assert!(result.is_err());
    }

    #[test]
    fn unconditioned_zone_temperature_above_maximum_fails() {
        // 120.1°C should fail even for an unconditioned zone.
        let result = checker().check_temperatures(&[], &[120.1], &[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "unconditioned_zone_temperature_bounds"
        ));
    }

    #[test]
    fn zone_temperature_nan_fails() {
        let result = checker().check_temperatures(&[f64::NAN], &[], &[]);
        assert!(result.is_err());
    }

    #[test]
    fn tank_temperature_within_bounds_passes() {
        let result = checker().check_temperatures(&[], &[], &[50.0, 99.9]);
        assert!(result.is_ok());
    }

    #[test]
    fn tank_temperature_above_maximum_fails() {
        let result = checker().check_temperatures(&[], &[], &[100.1]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "tank_temperature_bounds"
        ));
    }

    #[test]
    fn tank_temperature_below_minimum_fails() {
        let result = checker().check_temperatures(&[], &[], &[-0.1]);
        assert!(result.is_err());
    }

    // ── soc ──────────────────────────────────────────────────────────────────

    #[test]
    fn soc_within_range_returns_ok() {
        let result = checker().check_soc(0.5, 0.0);
        assert!(result.is_ok());
    }

    #[test]
    fn soc_out_of_range_returns_ok_with_warning() {
        // SoC violations warn but do not return Err.
        let result = checker().check_soc(1.5, 0.002);
        assert!(result.is_ok());
    }

    // ── protocol_native_registration ────────────────────────────────────────

    #[test]
    fn protocol_native_registration_passes_with_consumer() {
        let caps = [
            ControlCapabilities::POWER_SETPOINT,
            ControlCapabilities::PROTOCOL_NATIVE,
        ];
        let result = checker().check_protocol_native_registration(&caps);
        assert!(result.is_ok());
    }

    #[test]
    fn protocol_native_registration_fails_without_consumer() {
        let caps = [
            ControlCapabilities::POWER_SETPOINT,
            ControlCapabilities::THERMAL_SETPOINT,
        ];
        let result = checker().check_protocol_native_registration(&caps);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "protocol_native_registration"
        ));
    }

    #[test]
    fn protocol_native_registration_fails_empty() {
        let caps: [ControlCapabilities; 0] = [];
        let result = checker().check_protocol_native_registration(&caps);
        assert!(result.is_err());
    }

    // ── check_fuel_electric_absent ───────────────────────────────────────────

    #[test]
    fn fuel_electric_absent_passes_when_zero() {
        let result = checker().check_fuel_electric_absent(0.0);
        assert!(result.is_ok());
    }

    #[test]
    fn fuel_electric_absent_fails_when_positive() {
        let result = checker().check_fuel_electric_absent(100.0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "electric_fuel_in_fuel_accumulator"
        ));
    }

    #[test]
    fn fuel_electric_absent_passes_when_negative() {
        // Negative contributions shouldn't appear for fuel but the check
        // only flags positive values to avoid false alarms.
        let result = checker().check_fuel_electric_absent(-50.0);
        assert!(result.is_ok());
    }

    // ── check_fuel_coverage ────────────────────────────────────────────────

    #[test]
    fn fuel_coverage_passes_with_all_types_covered() {
        let mut fuel = hares_types::ports::FuelAccumulator::default();
        fuel.add(FuelType::Wood, 1000.0).unwrap();
        fuel.add(FuelType::Coal, 500.0).unwrap();
        fuel.add(FuelType::WoodPellet, 750.0).unwrap();
        let result = checker().check_fuel_coverage(&fuel);
        assert!(
            result.is_ok(),
            "ALL_FUEL_TYPES covers all fuel types; got {result:?}"
        );
    }

    #[test]
    fn fuel_coverage_passes_on_empty_accumulator() {
        let fuel = hares_types::ports::FuelAccumulator::default();
        let result = checker().check_fuel_coverage(&fuel);
        assert!(
            result.is_ok(),
            "empty accumulator must pass fuel coverage check"
        );
    }

    #[test]
    fn fuel_coverage_validates_index_consistency() {
        // This test validates that the invariant checker's fuel_index / ALL_FUEL_TYPES
        // consistency check passes when the data structures are properly aligned.
        // A failure here indicates a static data corruption in ALL_FUEL_TYPES,
        // FUEL_TYPE_COUNT, or fuel_index.
        use hares_types::ports::fuel_index;
        for (i, &ft) in ALL_FUEL_TYPES.iter().enumerate() {
            let idx = fuel_index(ft).unwrap_or_else(|| panic!("fuel_index missing for {ft:?}"));
            assert_eq!(
                idx, i,
                "ALL_FUEL_TYPES index mismatch: expected {i} for {ft:?}, got {idx}"
            );
        }
        assert_eq!(
            ALL_FUEL_TYPES.len(),
            FUEL_TYPE_COUNT,
            "ALL_FUEL_TYPES length {} != FUEL_TYPE_COUNT {}",
            ALL_FUEL_TYPES.len(),
            FUEL_TYPE_COUNT
        );
    }

    // ── NaN bypass guards ──────────────────────────────────────────────────

    #[test]
    fn thermal_balance_fails_on_nan_gain() {
        // NaN in q_gains previously bypassed the check (NaN >= tolerance == false).
        // After fix: the is_finite() guard fires first.
        let result = checker().check_thermal(&[f64::NAN], 200.0, 800.0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "thermal_balance"
        ));
    }

    #[test]
    fn thermal_balance_fails_on_nan_delta_e() {
        let result = checker().check_thermal(&[1000.0], f64::NAN, 800.0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "thermal_balance"
        ));
    }

    #[test]
    fn thermal_balance_fails_on_nan_loss() {
        let result = checker().check_thermal(&[1000.0], 200.0, f64::NAN);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "thermal_balance"
        ));
    }

    #[test]
    fn thermal_balance_fails_on_nan_alongside_finite_gains() {
        // A mixed slice: finite values plus one NaN — the guard catches the NaN.
        let result = checker().check_thermal(&[100.0, f64::NAN, 200.0], 300.0, 100.0);
        assert!(result.is_err());
    }

    #[test]
    fn moisture_balance_fails_on_nan_physical() {
        let result = checker().check_moisture(f64::NAN, 0.001, 0.001, 2.0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "moisture_balance"
        ));
    }

    #[test]
    fn moisture_balance_fails_on_nan_expected() {
        let result = checker().check_moisture(0.024, f64::NAN, 0.001, 2.0);
        assert!(result.is_err());
    }

    #[test]
    fn moisture_balance_fails_on_nan_actual() {
        let result = checker().check_moisture(0.024, 0.001, f64::NAN, 2.0);
        assert!(result.is_err());
    }

    #[test]
    fn moisture_balance_fails_on_nan_gross() {
        let result = checker().check_moisture(0.024, 0.001, 0.001, f64::NAN);
        assert!(result.is_err());
    }

    #[test]
    fn electrical_balance_fails_on_nan_grid() {
        let result = checker().check_electrical(f64::NAN, &[-3.0]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "electrical_balance"
        ));
    }

    #[test]
    fn electrical_balance_fails_on_nan_in_ports() {
        let result = checker().check_electrical(3.0, &[f64::NAN]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "electrical_balance"
        ));
    }

    #[test]
    fn electrical_balance_fails_on_nan_alongside_finite_ports() {
        let result = checker().check_electrical(3.0, &[-1.0, f64::NAN, -2.0]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::InvariantViolation { check_name, .. } if check_name == "electrical_balance"
        ));
    }

    #[test]
    fn electrical_balance_fails_on_nan_infinity_grid() {
        // ∞ is also non-finite and should be caught by the same guard.
        let result = checker().check_electrical(f64::INFINITY, &[-3.0]);
        assert!(result.is_err());
    }

    #[test]
    fn electrical_balance_fails_on_nan_infinity_port() {
        let result = checker().check_electrical(3.0, &[f64::NEG_INFINITY]);
        assert!(result.is_err());
    }

    #[test]
    fn existing_thermal_balance_tests_still_pass() {
        // Regression: previously passing tests are unaffected by the is_finite guard.
        let result = checker().check_thermal(&[1000.0], 200.0, 800.0);
        assert!(result.is_ok());
    }

    #[test]
    fn existing_moisture_balance_tests_still_pass() {
        let result = checker().check_moisture(0.024, 0.0016, 0.0016, 2.33);
        assert!(result.is_ok());
    }

    // ── check_nan_screen ─────────────────────────────────────────────────────

    /// Constructs a NaN-screen input with NaN in the position that would
    /// represent `q_sum` (total thermal gain) and asserts the NaN-detection
    /// path returns an error, not `Ok(())`.
    #[test]
    fn nan_screen_catches_nan_in_q_sum() {
        let result = checker().check_nan_screen(
            42,
            &[
                ("q_sum", None, f64::NAN),
                ("net_kw", None, 1.5),
                ("delta_m_water", None, 0.001),
            ],
        );
        assert!(
            result.is_err(),
            "NaN in q_sum must be caught by check_nan_screen"
        );
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::NanDetected { value_name, .. } if value_name == "q_sum"
        ));
    }

    /// All finite values pass the screen.
    #[test]
    fn nan_screen_passes_on_all_finite() {
        let result = checker().check_nan_screen(
            7,
            &[
                ("q_sum", None, 1000.0),
                ("net_kw", None, 1.5),
                ("delta_m_water", Some(ZoneId(1)), 0.001),
            ],
        );
        assert!(result.is_ok());
    }

    /// Infinity is non-finite and is caught by the same guard.
    #[test]
    fn nan_screen_catches_infinity() {
        let result = checker().check_nan_screen(1, &[("net_kw", None, f64::INFINITY)]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::NanDetected { value_name, .. } if value_name == "net_kw"
        ));
    }

    /// Verifies that zone_id context is carried through to the error.
    #[test]
    fn nan_screen_carries_zone_context() {
        let result = checker().check_nan_screen(99, &[("zone_temp_c", Some(ZoneId(3)), f64::NAN)]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::NanDetected {
                step_index: 99,
                zone_id: Some(ZoneId(3)),
                ..
            }
        ));
    }

    // ── check_hvac_power_non_negative ──────────────────────────────────────

    /// Negative hvac_heating_w returns NegativeDeliveredEnergy error.
    /// Heating convention: positive = heat into zone; negative is a sign error.
    #[test]
    fn hvac_power_negative_heating_fails() {
        let result = checker().check_hvac_power_non_negative(10, ZoneId(1), -500.0, 0.0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::NegativeDeliveredEnergy { field, value, step_index, .. }
                if field == "hvac_heating_w" && *value == -500.0 && *step_index == 10
        ));
    }

    /// Positive hvac_cooling_w returns NegativeDeliveredEnergy error.
    /// Cooling convention: negative = heat removed from zone; positive is a
    /// sign inversion (e.g. a cooling coil producing sensible gain instead of
    /// removal).
    #[test]
    fn hvac_power_positive_cooling_fails() {
        let result = checker().check_hvac_power_non_negative(5, ZoneId(2), 0.0, 300.0);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::NegativeDeliveredEnergy { field, value, step_index, .. }
                if field == "hvac_cooling_w" && *value == 300.0 && *step_index == 5
        ));
    }

    /// Valid signs pass the per-step check: heating ≥ 0, cooling ≤ 0.
    #[test]
    fn hvac_power_valid_signs_passes() {
        let result = checker().check_hvac_power_non_negative(0, ZoneId(1), 500.0, -200.0);
        assert!(result.is_ok());
    }

    /// Exactly zero passes (boundary case).
    #[test]
    fn hvac_power_zero_passes() {
        let result = checker().check_hvac_power_non_negative(0, ZoneId(1), 0.0, 0.0);
        assert!(result.is_ok());
    }

    // ── check_heating_accumulator ──────────────────────────────────────────

    /// Accumulated positive heating and negative cooling (correct signs) pass.
    #[test]
    fn heating_accumulator_correct_signs_passes() {
        let mut c = checker();
        for step in 0..10 {
            let result = c.check_heating_accumulator(step, ZoneId(1), 1000.0, -500.0, 0.25);
            assert!(result.is_ok(), "step {step} should pass");
        }
    }

    /// Negative heating step pushes cumulative heating below drift tolerance.
    #[test]
    fn heating_accumulator_negative_heating_fails() {
        let mut c = checker();
        c.check_heating_accumulator(0, ZoneId(1), 1000.0, 0.0, 1.0)
            .unwrap();
        // total_heating_wh = 1000 after step 0.
        // Step 1: hvac_heating_w = -2000 W × 1 h = -2000 Wh → total = -1000 Wh.
        // drift = f64::min(-1e-6, -1e-6*1) = -1e-6. -1000 < -1e-6 → must fail.
        let result = c.check_heating_accumulator(1, ZoneId(1), -2000.0, 0.0, 1.0);
        assert!(
            result.is_err(),
            "negative cumulative heating must fail accumulator check"
        );
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::NegativeDeliveredEnergy { field, .. }
                if field == "total_heating_wh"
        ));
    }

    /// Positive cooling step pushes cumulative cooling above drift tolerance
    /// (cooling convention: stores negative values; positive is a sign error).
    #[test]
    fn cooling_accumulator_positive_cooling_fails() {
        let mut c = checker();
        c.check_heating_accumulator(0, ZoneId(1), 0.0, -500.0, 1.0)
            .unwrap();
        // total_cooling_wh = -500 after step 0.
        // Step 1: cooling_w = +1000 W × 1 h = +1000 Wh → total = +500 Wh.
        // drift_pos = -drift_neg = 1e-6. +500 > +1e-6 → must fail.
        let result = c.check_heating_accumulator(1, ZoneId(1), 0.0, 1000.0, 1.0);
        assert!(
            result.is_err(),
            "positive cumulative cooling must fail accumulator check"
        );
        let err = result.unwrap_err();
        assert!(matches!(
            &err,
            HaresError::NegativeDeliveredEnergy { field, .. }
                if field == "total_cooling_wh"
        ));
    }

    /// Floating-point drift within tolerance does not trigger a violation.
    #[test]
    fn heating_accumulator_tolerates_floating_drift() {
        let mut c = checker();
        for step in 0..1000 {
            let result = c.check_heating_accumulator(step, ZoneId(1), 0.0, 0.0, 1.0);
            assert!(result.is_ok(), "step {step} with zero power should pass");
        }
    }

    /// Drift tolerance scales with step_index.
    #[test]
    fn heating_accumulator_drift_tolerance_scales_with_steps() {
        let mut c = checker();
        // At step 100k, drift tolerance ≈ -1e-6 * 100k = -0.1 Wh.
        // Tiny per-step drift of -1e-7 Wh accumulates to -0.01 Wh → within tolerance.
        let steps = 100_000u64;
        for step in 0..steps {
            c.check_heating_accumulator(step, ZoneId(1), -1e-7, 0.0, 1.0)
                .unwrap();
        }
        // Add a step that pushes cumulative past the tolerance.
        let result = c.check_heating_accumulator(steps, ZoneId(1), -1.0, 0.0, 1.0);
        assert!(
            result.is_err(),
            "cumulative exceeding drift tolerance must fail"
        );
    }
}
