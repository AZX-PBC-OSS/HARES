//! Speed staging, part-load, and capacity interpolation for HVAC equipment.
//!
//! Extracted from `hvac_core.rs` to isolate speed selection algorithms from
//! thermostat control and duct distribution. No zone or duct knowledge here.

use hares_physics::biquadratic::quadratic;

use super::hvac_core::HvacEquipment;
use super::speed_control::{SpeedControlMode, SpeedSelection};
use super::thermostat::ThermostatMode;

/// Default low-speed capacity fraction for two-speed equipment.
/// Matches OCHRE/AHRI lookup table: 0.72 for 2-speed AC and ASHP coolers.
pub(super) const DEFAULT_LOW_SPEED_CAPACITY_FRACTION: f64 = 0.72;

/// Default part-load factor degradation coefficient (Cd).
/// AHRI Standard 210/240-2023, S6.6.3 default when no test data available.
pub(super) const DEFAULT_PLF_DEGRADATION_COEFF: f64 = 0.25;

impl HvacEquipment {
    /// Number of discrete speed stages. For single-speed equipment this is 1.
    pub fn n_speed_stages(&self) -> usize {
        match self.speed_control_mode {
            SpeedControlMode::SingleSpeed => 1,
            SpeedControlMode::TwoSpeedSetpoint
            | SpeedControlMode::TwoSpeedTime
            | SpeedControlMode::TwoSpeedAlternating => 2,
            SpeedControlMode::MultiSpeedInterpolated => {
                let caps = self
                    .heating_capacities_w
                    .len()
                    .max(self.cooling_capacities_w.len());
                caps.max(1)
            }
            SpeedControlMode::VariableSpeedIdeal => 1,
        }
    }

    /// Dynamically disable or re-enable individual speed stages at runtime.
    ///
    /// If all speeds are disabled, `max_enabled_speed` falls back to the last stage.
    pub fn set_disabled_speeds(&mut self, disabled: &[bool]) {
        let n = self.n_speed_stages();
        self.disabled_speeds.resize(n, false);
        for (i, slot) in self.disabled_speeds.iter_mut().enumerate() {
            *slot = disabled.get(i).copied().unwrap_or(false);
        }
        self.max_enabled_speed = self
            .disabled_speeds
            .iter()
            .enumerate()
            .rev()
            .find(|&(_, d)| !d)
            .map(|(i, _)| i)
            .unwrap_or(n.saturating_sub(1));
    }

    /// Record the current zone temperature for the next step's `TwoSpeedTime` comparison.
    /// Pass `None` when the unit turns off to ensure the next on-cycle starts at low speed.
    pub fn update_prev_zone_temp(&mut self, zone_temp_c: Option<f64>) {
        self.prev_zone_temp_c = zone_temp_c;
    }

    /// Advance the speed-stage timer by `dt_s` seconds.
    pub fn advance_speed_timer(&mut self, dt_s: f64) {
        self.time_at_current_speed_s += dt_s;
    }

    pub fn select_speed(&mut self, load_fraction: f64) -> SpeedSelection {
        self.select_speed_with_zone_temp(load_fraction, None, false)
    }

    /// Speed selection that accepts the current zone temperature and heating
    /// direction for `TwoSpeedTime` mode.
    pub fn select_speed_with_zone_temp(
        &mut self,
        load_fraction: f64,
        zone_temp_c: Option<f64>,
        is_heating: bool,
    ) -> SpeedSelection {
        let load_fraction = load_fraction.clamp(0.0, 1.0);
        let selection = match self.speed_control_mode {
            SpeedControlMode::SingleSpeed => SpeedSelection {
                speed_index: 0,
                part_load_ratio: load_fraction,
                speed_frac: load_fraction,
            },
            SpeedControlMode::TwoSpeedSetpoint => self.select_two_speed_setpoint(load_fraction),
            SpeedControlMode::TwoSpeedTime => {
                self.select_two_speed_time(load_fraction, zone_temp_c, is_heating)
            }
            SpeedControlMode::TwoSpeedAlternating => {
                let desired_index = self.apply_disabled_speeds_two_speed(1);
                if desired_index != self.last_speed_index {
                    self.time_at_current_speed_s = 0.0;
                }
                let low_cap = self.low_speed_capacity_fraction.clamp(0.01, 0.999);
                let high_cap = 1.0; // high speed = full capacity fraction
                if desired_index == 1 {
                    SpeedSelection {
                        speed_index: 1,
                        part_load_ratio: (load_fraction / high_cap).clamp(0.0, 1.0),
                        speed_frac: 1.0,
                    }
                } else {
                    SpeedSelection {
                        speed_index: 0,
                        part_load_ratio: (load_fraction / low_cap).clamp(0.0, 1.0),
                        speed_frac: low_cap,
                    }
                }
            }
            SpeedControlMode::MultiSpeedInterpolated => self.select_multi_speed(load_fraction),
            SpeedControlMode::VariableSpeedIdeal => self.select_multi_speed(load_fraction),
        };
        self.last_speed_index = selection.speed_index;
        self.last_speed_frac = selection.speed_frac;
        selection
    }

    fn select_two_speed_setpoint(&mut self, load_fraction: f64) -> SpeedSelection {
        let low_cap = self.low_speed_capacity_fraction.clamp(0.01, 0.999);
        let desired_index = if load_fraction > low_cap { 1 } else { 0 };
        let desired_index = self.apply_disabled_speeds_two_speed(desired_index);
        let locked = self.time_at_current_speed_s < self.min_time_per_speed_s
            && desired_index != self.last_speed_index;
        let speed_index = if locked {
            self.last_speed_index
        } else {
            if desired_index != self.last_speed_index {
                self.time_at_current_speed_s = 0.0;
            }
            desired_index
        };
        if speed_index == 1 {
            SpeedSelection {
                speed_index: 1,
                part_load_ratio: load_fraction,
                speed_frac: 1.0,
            }
        } else {
            SpeedSelection {
                speed_index: 0,
                part_load_ratio: (load_fraction / low_cap).clamp(0.0, 1.0),
                speed_frac: low_cap,
            }
        }
    }

    fn select_two_speed_time(
        &mut self,
        load_fraction: f64,
        zone_temp_c: Option<f64>,
        is_heating: bool,
    ) -> SpeedSelection {
        let (desired_index, fresh_cycle) = if let (Some(current), Some(prev)) =
            (zone_temp_c, self.prev_zone_temp_c)
        {
            let moving_wrong_way = if is_heating {
                current < prev
            } else {
                current > prev
            };
            let idx =
                if moving_wrong_way && self.time_at_current_speed_s >= self.min_time_per_speed_s {
                    1
                } else {
                    self.last_speed_index
                };
            (idx, false)
        } else {
            (0, true)
        };
        let desired_index = self.apply_disabled_speeds_two_speed(desired_index);
        let locked = !fresh_cycle
            && self.time_at_current_speed_s < self.min_time_per_speed_s
            && desired_index != self.last_speed_index;
        let speed_index = if locked {
            self.last_speed_index
        } else {
            if desired_index != self.last_speed_index {
                self.time_at_current_speed_s = 0.0;
            }
            desired_index
        };
        let low_cap = self.low_speed_capacity_fraction.clamp(0.01, 0.999);
        if speed_index == 1 {
            SpeedSelection {
                speed_index: 1,
                part_load_ratio: load_fraction,
                speed_frac: 1.0,
            }
        } else {
            SpeedSelection {
                speed_index: 0,
                part_load_ratio: (load_fraction / low_cap).clamp(0.0, 1.0),
                speed_frac: low_cap,
            }
        }
    }

    fn select_multi_speed(&self, load_fraction: f64) -> SpeedSelection {
        let caps = match self.mode {
            ThermostatMode::Heating if !self.heating_capacities_w.is_empty() => {
                &self.heating_capacities_w
            }
            ThermostatMode::Cooling if !self.cooling_capacities_w.is_empty() => {
                &self.cooling_capacities_w
            }
            _ if !self.heating_capacities_w.is_empty() => &self.heating_capacities_w,
            _ => &self.cooling_capacities_w,
        };
        let cap_fracs = Self::capacity_fractions_for(caps);
        if cap_fracs.is_empty() || load_fraction <= 0.0 {
            return SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: 0.0,
            };
        }
        if load_fraction <= cap_fracs[0] {
            return SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: load_fraction / cap_fracs[0],
            };
        }
        // SAFETY: cap_fracs is non-empty (early return above checks is_empty).
        if load_fraction >= *cap_fracs.last().expect("cap_fracs non-empty") {
            return SpeedSelection {
                speed_index: cap_fracs.len() - 1,
                speed_frac: 0.0,
                part_load_ratio: 1.0,
            };
        }
        let hi = cap_fracs.partition_point(|&f| f < load_fraction);
        let lo = hi - 1;
        let span = cap_fracs[hi] - cap_fracs[lo];
        let frac = if span > f64::EPSILON {
            (load_fraction - cap_fracs[lo]) / span
        } else {
            0.0
        };
        SpeedSelection {
            speed_index: lo,
            speed_frac: frac,
            part_load_ratio: 1.0,
        }
    }

    fn apply_disabled_speeds_two_speed(&self, desired_index: usize) -> usize {
        if self.disabled_speeds.is_empty() {
            return desired_index;
        }
        if self
            .disabled_speeds
            .get(desired_index)
            .copied()
            .unwrap_or(false)
        {
            self.max_enabled_speed
        } else {
            desired_index
        }
    }

    /// Compute part-load factor at the current speed stage.
    pub fn part_load_factor(&mut self, plr: f64) -> f64 {
        self.part_load_factor_for_stage(plr, self.last_speed_index)
    }

    /// Compute PLF for an explicit speed stage index.
    pub fn part_load_factor_for_stage(&mut self, plr: f64, stage_index: usize) -> f64 {
        if matches!(
            self.speed_control_mode,
            SpeedControlMode::VariableSpeedIdeal
        ) {
            self.plf_state = 1.0;
            return 1.0;
        }
        let plr = plr.clamp(0.0, 1.0);

        let plf_raw = if let Some(ref curves) = self.eir_plr_coefficients {
            let coeffs = if let Some(&c) = curves.get(stage_index) {
                c
            } else {
                tracing::debug!(
                    stage_index,
                    n_curves = curves.len(),
                    "PLF stage index out of range, using last curve"
                );
                curves.last().copied().unwrap_or([1.0, 0.0, 0.0])
            };
            quadratic(&coeffs, plr)
        } else {
            let cd = self.plf_cooling_degradation_coeff.clamp(0.0, 1.0);
            1.0 - cd * (1.0 - plr)
        };

        if plf_raw < self.plf_min {
            tracing::warn!(
                plf_raw,
                plr,
                stage_index,
                plf_min = self.plf_min,
                "PLF curve returned value < plf_min; check eir_plr or cooling_cd. Clamping to max(plf_min, PLR)."
            );
        }
        let plf = plf_raw.clamp(self.plf_min.max(plr), 1.0);
        self.plf_state = plf;
        plf
    }

    /// Apply the Winkler (2011) exponential startup capacity ramp.
    pub fn apply_startup_capacity_degradation(
        &mut self,
        steady_capacity_w: f64,
        dt_min: f64,
    ) -> f64 {
        let on_now = self.duty_cycle > 0.0;
        let mult = self.startup.capacity_multiplier(on_now, dt_min);
        steady_capacity_w * mult
    }

    pub fn rated_capacity_w(&self, mode: ThermostatMode) -> f64 {
        match mode {
            ThermostatMode::Heating => self
                .heating_capacities_w
                .first()
                .copied()
                .unwrap_or_default(),
            ThermostatMode::Cooling => self
                .cooling_capacities_w
                .first()
                .copied()
                .unwrap_or_default(),
            ThermostatMode::Deadband => 0.0,
        }
    }

    pub fn capacity_at_stage(capacities: &[f64], stage_index: usize) -> f64 {
        if capacities.is_empty() {
            return 0.0;
        }
        capacities[stage_index.min(capacities.len() - 1)]
    }

    pub fn eir_at_stage(&self, stage_index: usize) -> f64 {
        if self.eir_by_stage.is_empty() {
            return 1.0;
        }
        self.eir_by_stage[stage_index.min(self.eir_by_stage.len() - 1)]
    }

    /// Normalized capacity fractions `cap[i] / cap[last]` for the populated capacities array.
    pub fn capacity_fractions(&self) -> Vec<f64> {
        let caps = match self.mode {
            ThermostatMode::Heating if !self.heating_capacities_w.is_empty() => {
                &self.heating_capacities_w
            }
            ThermostatMode::Cooling if !self.cooling_capacities_w.is_empty() => {
                &self.cooling_capacities_w
            }
            _ if self.heating_capacities_w.len() >= self.cooling_capacities_w.len() => {
                &self.heating_capacities_w
            }
            _ => &self.cooling_capacities_w,
        };
        Self::capacity_fractions_for(caps)
    }

    fn capacity_fractions_for(caps: &[f64]) -> Vec<f64> {
        let max_cap = caps.last().copied().unwrap_or(0.0);
        if max_cap <= 0.0 {
            return vec![];
        }
        caps.iter().map(|&c| c / max_cap).collect()
    }

    /// Interpolate capacity between two bracket stages using `speed_frac`.
    pub fn interpolated_capacity(
        &self,
        capacities: &[f64],
        speed_index: usize,
        speed_frac: f64,
    ) -> f64 {
        let cap_lo = Self::capacity_at_stage(capacities, speed_index);
        if speed_frac > 0.0 {
            let cap_hi = Self::capacity_at_stage(capacities, speed_index + 1);
            cap_lo * (1.0 - speed_frac) + cap_hi * speed_frac
        } else {
            cap_lo
        }
    }

    /// Interpolate EIR between two bracket stages using `speed_frac`.
    pub fn interpolated_eir(&self, speed_index: usize, speed_frac: f64) -> f64 {
        let eir_lo = self.eir_at_stage(speed_index);
        if speed_frac > 0.0 {
            let eir_hi = self.eir_at_stage(speed_index + 1);
            eir_lo * (1.0 - speed_frac) + eir_hi * speed_frac
        } else {
            eir_lo
        }
    }

    pub fn airflow_m3_s_for_capacity_w(&self, capacity_w: f64) -> f64 {
        capacity_w.max(0.0) * self.airflow_m3_s_per_w
    }

    pub fn fan_power_w(&self, airflow_m3_s: f64) -> f64 {
        airflow_m3_s.max(0.0) * self.fan_power_w_per_m3_s
    }

    pub fn sensible_latent_from_shr(&self, total_cooling_w: f64) -> (f64, f64) {
        let shr = self.shr.clamp(0.0, 1.0);
        let sensible = total_cooling_w * shr;
        let latent = total_cooling_w - sensible;
        (sensible, latent)
    }
}

#[cfg(test)]
mod tests {
    use hares_types::ZoneId;

    use super::super::hvac_core::{HvacEquipment, HvacEquipmentType};
    use super::super::speed_control::SpeedControlMode;
    use super::super::thermostat::ThermostatMode;

    fn make_single_speed() -> HvacEquipment {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::SingleSpeed;
        hvac.heating_capacities_w = vec![10_000.0];
        hvac
    }

    fn make_multi_speed_4() -> HvacEquipment {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::MultiSpeedInterpolated;
        hvac.cooling_capacities_w = vec![2_500.0, 5_000.0, 7_500.0, 10_000.0];
        hvac
    }

    /// AHRI 210/240 S6.6.3: PLF = 1 - Cd*(1-PLR), default Cd = 0.25.
    #[test]
    fn plf_ahri_210_240_default_cd() {
        let mut hvac = make_single_speed();
        assert!((hvac.plf_cooling_degradation_coeff - 0.25).abs() < 1e-12);

        let cases: &[(f64, f64)] = &[(1.00, 1.000), (0.75, 0.9375), (0.50, 0.875)];
        for &(plr, expected_plf) in cases {
            let plf = hvac.part_load_factor_for_stage(plr, 0);
            assert!(
                (plf - expected_plf).abs() < 0.001,
                "PLR={plr}: expected PLF={expected_plf}, got {plf}"
            );
        }
    }

    /// PLR = 0.0: raw PLF = 1 - 0.25*1 = 0.75.
    /// Clamp is max(0.7, PLR) = max(0.7, 0.0) = 0.7.
    /// Since 0.75 >= 0.7, PLF = 0.75.
    #[test]
    fn plf_zero_load_returns_floor() {
        let mut hvac = make_single_speed();
        let plf = hvac.part_load_factor_for_stage(0.0, 0);
        assert!(
            (plf - 0.75).abs() < 0.001,
            "PLR=0: expected PLF=0.75, got {plf}"
        );
    }

    /// High Cd produces raw PLF below default plf_min (0.7), clamped to max(0.7, PLR).
    /// Cd=0.5, PLR=0.3 → raw PLF = 1 - 0.5*(1-0.3) = 0.65 → clamped to max(0.7, 0.3) = 0.7.
    #[test]
    fn plf_floor_clamp_with_high_cd() {
        let mut hvac = make_single_speed();
        hvac.plf_cooling_degradation_coeff = 0.5;
        let plf = hvac.part_load_factor_for_stage(0.3, 0);
        assert!(
            (plf - 0.7).abs() < 1e-9,
            "PLR=0.3, Cd=0.5: raw PLF=0.65 must clamp to 0.7, got {plf}"
        );
    }

    /// MSHP CSV-derived plf_min=0.2195: the PLF floor should use that value, not 0.7.
    /// Cd=0.5, PLR=0.3 → raw PLF = 1 - 0.5*(1-0.3) = 0.65 → clamped to max(0.2195, 0.3) = 0.3.
    #[test]
    fn plf_floor_uses_custom_plf_min() {
        let mut hvac = make_single_speed();
        hvac.plf_cooling_degradation_coeff = 0.5;
        hvac.plf_min = 0.2195;
        let plf = hvac.part_load_factor_for_stage(0.3, 0);
        // raw=0.65, floor=max(0.2195, 0.3)=0.3, so PLF=0.65 (above floor)
        assert!(
            (plf - 0.65).abs() < 1e-9,
            "PLR=0.3, Cd=0.5, plf_min=0.2195: raw PLF=0.65 >= floor 0.3, got {plf}"
        );

        // PLR=0.1 → raw PLF = 1 - 0.5*(1-0.1) = 0.55 → floor=max(0.2195, 0.1)=0.2195
        // 0.55 >= 0.2195, so PLF=0.55
        let plf2 = hvac.part_load_factor_for_stage(0.1, 0);
        assert!(
            (plf2 - 0.55).abs() < 1e-9,
            "PLR=0.1, Cd=0.5, plf_min=0.2195: raw PLF=0.55 >= floor 0.2195, got {plf2}"
        );

        // Extreme: Cd=0.95, PLR=0.1 → raw PLF = 1 - 0.95*0.9 = 0.145 < plf_min=0.2195
        // floor=max(0.2195, 0.1)=0.2195, clamp to 0.2195
        hvac.plf_cooling_degradation_coeff = 0.95;
        let plf3 = hvac.part_load_factor_for_stage(0.1, 0);
        assert!(
            (plf3 - 0.2195).abs() < 1e-9,
            "PLR=0.1, Cd=0.95, plf_min=0.2195: raw PLF=0.145 clamped to 0.2195, got {plf3}"
        );
    }

    /// SingleSpeed: any load fraction yields speed_index=0, PLR=load_fraction.
    #[test]
    fn single_speed_always_stage_zero() {
        let mut hvac = make_single_speed();
        for load in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let sel = hvac.select_speed(load);
            assert_eq!(sel.speed_index, 0, "load={load}: speed_index must be 0");
            assert!(
                (sel.part_load_ratio - load).abs() < 1e-12,
                "load={load}: PLR must equal load_fraction, got {}",
                sel.part_load_ratio
            );
        }
    }

    /// 4-speed [0.25, 0.50, 0.75, 1.0] with load_fraction = 0.625.
    /// Brackets between stage 1 (0.50) and stage 2 (0.75).
    /// speed_index = 1, speed_frac = (0.625-0.50)/(0.75-0.50) = 0.50.
    #[test]
    fn multi_speed_interpolation_between_stages() {
        let mut hvac = make_multi_speed_4();
        let sel = hvac.select_speed(0.625);
        assert_eq!(sel.speed_index, 1, "should bracket at stage 1");
        assert!(
            (sel.speed_frac - 0.5).abs() < 1e-9,
            "expected speed_frac=0.5, got {}",
            sel.speed_frac
        );
        assert!(
            (sel.part_load_ratio - 1.0).abs() < 1e-9,
            "inter-speed PLR must be 1.0, got {}",
            sel.part_load_ratio
        );
    }

    /// interpolated_capacity with capacities=[5000, 10000], speed_index=0, speed_frac=0.5.
    /// Expected: 5000*(1-0.5) + 10000*0.5 = 7500.
    #[test]
    fn interpolated_capacity_linear() {
        let hvac = make_single_speed();
        let capacities = [5_000.0, 10_000.0];
        let result = hvac.interpolated_capacity(&capacities, 0, 0.5);
        assert!(
            (result - 7_500.0).abs() < 1e-9,
            "expected 7500 W, got {result}"
        );
    }

    /// interpolated_eir with eir_by_stage=[0.3, 0.4], speed_index=0, speed_frac=0.5.
    /// Expected: 0.3*(1-0.5) + 0.4*0.5 = 0.35.
    #[test]
    fn interpolated_eir_between_stages() {
        let mut hvac = make_single_speed();
        // COP ~3.3 at low speed, ~2.5 at high speed (realistic AC EIR values)
        hvac.eir_by_stage = vec![0.30, 0.40];
        let result = hvac.interpolated_eir(0, 0.5);
        assert!(
            (result - 0.35).abs() < 1e-9,
            "expected EIR=0.35, got {result}"
        );
    }

    /// interpolated_eir with speed_frac=0.0 returns stage 0 EIR unchanged.
    #[test]
    fn interpolated_eir_at_stage_boundary() {
        let mut hvac = make_single_speed();
        hvac.eir_by_stage = vec![0.30, 0.40];
        let result = hvac.interpolated_eir(0, 0.0);
        assert!(
            (result - 0.30).abs() < 1e-9,
            "expected EIR=0.30 at stage 0, got {result}"
        );
    }

    /// capacity_fractions for a 2-speed heating config [5000, 10000 W].
    /// Expected fractions: [0.5, 1.0] — normalized to the highest stage.
    #[test]
    fn capacity_fractions_two_speed() {
        let mut hvac = make_single_speed();
        hvac.heating_capacities_w = vec![5_000.0, 10_000.0];
        hvac.mode = ThermostatMode::Heating;
        let fracs = hvac.capacity_fractions();
        assert_eq!(fracs.len(), 2, "two-speed must yield two fractions");
        assert!(
            (fracs[0] - 0.5).abs() < 1e-9,
            "low-speed fraction: expected 0.5, got {}",
            fracs[0]
        );
        assert!(
            (fracs[1] - 1.0).abs() < 1e-9,
            "high-speed fraction: expected 1.0, got {}",
            fracs[1]
        );
        for (i, &f) in fracs.iter().enumerate() {
            assert!(
                (0.0..=1.0).contains(&f),
                "fraction[{i}]={f} must be in [0, 1]"
            );
        }
    }

    #[test]
    fn capacity_fractions_follow_active_mode() {
        let mut hvac = make_single_speed();
        hvac.heating_capacities_w = vec![4_000.0, 8_000.0];
        hvac.cooling_capacities_w = vec![2_000.0, 4_000.0, 6_000.0, 12_000.0];

        hvac.mode = ThermostatMode::Heating;
        let heating_fracs = hvac.capacity_fractions();
        assert_eq!(heating_fracs, vec![0.5, 1.0]);

        hvac.mode = ThermostatMode::Cooling;
        let cooling_fracs = hvac.capacity_fractions();
        assert_eq!(cooling_fracs, vec![1.0 / 6.0, 1.0 / 3.0, 0.5, 1.0]);
    }

    #[test]
    fn two_speed_alternating_normalizes_low_stage_plr_when_high_disabled() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::TwoSpeedAlternating;
        hvac.low_speed_capacity_fraction = 0.5;
        hvac.set_disabled_speeds(&[false, true]);

        let sel = hvac.select_speed(0.3);
        assert_eq!(sel.speed_index, 0);
        assert!((sel.part_load_ratio - 0.6).abs() < 1e-9);
        assert!((sel.speed_frac - 0.5).abs() < 1e-9);
    }

    fn make_two_speed_setpoint() -> HvacEquipment {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::TwoSpeedSetpoint;
        hvac.cooling_capacities_w = vec![5_000.0, 10_000.0];
        hvac.low_speed_capacity_fraction = 0.5;
        // Set min_time_per_speed_s = 0 so tests are not time-locked.
        hvac.min_time_per_speed_s = 0.0;
        hvac.time_at_current_speed_s = 0.0;
        hvac
    }

    fn make_two_speed_time() -> HvacEquipment {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::TwoSpeedTime;
        hvac.cooling_capacities_w = vec![5_000.0, 10_000.0];
        hvac.low_speed_capacity_fraction = 0.5;
        hvac.min_time_per_speed_s = 300.0;
        hvac.time_at_current_speed_s = 0.0;
        hvac
    }

    /// TwoSpeedSetpoint: load ≤ low_speed_capacity_fraction → speed 0.
    /// load=0.3, low_cap=0.5 → desired=0.  PLR = 0.3/0.5 = 0.6, speed_frac = 0.5.
    #[test]
    fn select_speed_two_speed_setpoint_low_load() {
        let mut hvac = make_two_speed_setpoint();
        let sel = hvac.select_speed(0.3);
        assert_eq!(sel.speed_index, 0, "low load must select speed 0");
        assert!(
            (sel.part_load_ratio - 0.6).abs() < 1e-9,
            "PLR must be load/low_cap = 0.3/0.5 = 0.6, got {}",
            sel.part_load_ratio
        );
        assert!(
            (sel.speed_frac - 0.5).abs() < 1e-9,
            "speed_frac must equal low_speed_capacity_fraction=0.5, got {}",
            sel.speed_frac
        );
    }

    /// TwoSpeedSetpoint: load > low_speed_capacity_fraction → speed 1.
    /// load=0.8, low_cap=0.5 → desired=1.  PLR = 0.8, speed_frac = 1.0.
    #[test]
    fn select_speed_two_speed_setpoint_high_load() {
        let mut hvac = make_two_speed_setpoint();
        let sel = hvac.select_speed(0.8);
        assert_eq!(sel.speed_index, 1, "high load must select speed 1");
        assert!(
            (sel.part_load_ratio - 0.8).abs() < 1e-9,
            "PLR must equal load_fraction=0.8, got {}",
            sel.part_load_ratio
        );
        assert!(
            (sel.speed_frac - 1.0).abs() < 1e-9,
            "speed_frac must be 1.0 at high speed, got {}",
            sel.speed_frac
        );
    }

    /// TwoSpeedTime: fresh cycle (no prev_zone_temp) starts at speed 0.
    /// After min_time_per_speed_s has elapsed and zone temp is moving wrong way
    /// (cooling: zone temp rising), speed escalates to 1.
    /// At speed 1 with load_fraction=0.8: PLR=0.8, speed_frac=1.0.
    #[test]
    fn select_speed_two_speed_time_direction_change() {
        let mut hvac = make_two_speed_time();
        // Fresh start: no previous temperature, must start at speed 0.
        let sel0 = hvac.select_speed_with_zone_temp(0.8, Some(25.0), false);
        assert_eq!(sel0.speed_index, 0, "fresh cycle must start at speed 0");

        // Advance timer past the minimum guard and record current zone temp.
        hvac.advance_speed_timer(300.0);
        hvac.update_prev_zone_temp(Some(25.0));

        // Next step: zone temp rose to 26.0°C during cooling — moving wrong way.
        let sel1 = hvac.select_speed_with_zone_temp(0.8, Some(26.0), false);
        assert_eq!(
            sel1.speed_index, 1,
            "rising zone temp during cooling after min_time must escalate to speed 1"
        );
        assert!(
            (sel1.part_load_ratio - 0.8).abs() < 1e-9,
            "speed 1 PLR must equal load_fraction=0.8, got {}",
            sel1.part_load_ratio
        );
        assert!(
            (sel1.speed_frac - 1.0).abs() < 1e-9,
            "speed 1 speed_frac must be 1.0, got {}",
            sel1.speed_frac
        );
    }

    /// TwoSpeedTime: speed change is blocked when time_at_current_speed_s < min_time_per_speed_s.
    /// Even with temperature moving in the wrong direction, the speed must not change.
    #[test]
    fn select_speed_two_speed_time_min_guard() {
        let mut hvac = make_two_speed_time();
        // Establish: running at speed 0, timer has not yet expired.
        hvac.last_speed_index = 0;
        hvac.time_at_current_speed_s = 100.0; // less than 300 s minimum
        hvac.prev_zone_temp_c = Some(25.0);

        // Zone temp is rising during cooling — would normally trigger escalation,
        // but the min-time guard must block it.
        let sel = hvac.select_speed_with_zone_temp(0.8, Some(26.0), false);
        assert_eq!(
            sel.speed_index, 0,
            "speed must not change before min_time_per_speed_s expires: got {}",
            sel.speed_index
        );
    }

    /// apply_startup_capacity_degradation: cold start (duty_cycle > 0, timer=0) must
    /// return capacity below steady-state.  Winkler (2011) c_d=0.25, dt=1 min → t_full=5.4 min,
    /// first-step mult < 1.0.
    #[test]
    fn startup_capacity_degradation_cold_start() {
        let mut hvac = make_single_speed();
        hvac.duty_cycle = 1.0; // unit is on
        hvac.startup.c_d = 0.25;
        hvac.startup.time_since_start_min = 0.0;

        let steady_w = 10_000.0;
        let actual_w = hvac.apply_startup_capacity_degradation(steady_w, 1.0);

        assert!(
            actual_w < steady_w,
            "cold-start capacity must be below steady-state {steady_w} W, got {actual_w} W"
        );
        assert!(
            actual_w > 0.0,
            "startup capacity must be positive, got {actual_w} W"
        );
        // Winkler formula at t=0.5 min (mid-step), t_full=5.4 min:
        // mult = -1.025 * exp(-3.79936 * 0.5 / 5.4) + 1.025
        let t_full = 20.0 * 0.25_f64 + 0.4;
        let expected_mult =
            (-1.025_f64 * (-3.799_36_f64 * 0.5 / t_full).exp() + 1.025).clamp(0.0, 1.0);
        assert!(
            (actual_w - steady_w * expected_mult).abs() < 1.0,
            "cold-start capacity: expected {:.1} W, got {actual_w:.1} W",
            steady_w * expected_mult
        );
    }

    /// apply_startup_capacity_degradation: when c_d = 0.0 (variable-speed / no ramp),
    /// the multiplier is always 1.0 and capacity equals steady-state on the first step.
    #[test]
    fn startup_capacity_degradation_c_d_zero_no_ramp() {
        let mut hvac = make_single_speed();
        hvac.duty_cycle = 1.0;
        hvac.startup.c_d = 0.0;
        hvac.startup.time_since_start_min = 0.0;

        let steady_w = 10_000.0;
        let actual_w = hvac.apply_startup_capacity_degradation(steady_w, 1.0);

        assert!(
            (actual_w - steady_w).abs() < 1e-9,
            "c_d=0 must yield full capacity immediately: expected {steady_w} W, got {actual_w} W"
        );
    }

    /// apply_startup_capacity_degradation warm-restart scenario:
    /// with c_d=0.25, once time_since_start_min >= t_full the multiplier is 1.0.
    /// After an off cycle, the first on-step must start below 1.0 again.
    #[test]
    fn startup_capacity_degradation_warm_restart_real() {
        let steady_w = 10_000.0;
        let c_d = 0.25_f64;
        let t_full = 20.0 * c_d + 0.4; // 5.4 min

        // Run enough on-steps to pass t_full.
        let mut hvac = make_single_speed();
        hvac.duty_cycle = 1.0;
        hvac.startup.c_d = c_d;
        hvac.startup.time_since_start_min = 0.0;

        // Advance beyond t_full with 1-min steps.
        let mut mult_at_full = 0.0_f64;
        for _ in 0..=((t_full as usize) + 1) {
            let w = hvac.apply_startup_capacity_degradation(steady_w, 1.0);
            mult_at_full = w / steady_w;
        }
        assert!(
            (mult_at_full - 1.0).abs() < 1e-9,
            "past t_full the multiplier must be 1.0, got {mult_at_full}"
        );

        // Off cycle resets the timer.
        hvac.duty_cycle = 0.0;
        let _ = hvac.apply_startup_capacity_degradation(steady_w, 1.0);

        // First on-step after off must ramp again (mult < 1.0).
        hvac.duty_cycle = 1.0;
        let w_restart = hvac.apply_startup_capacity_degradation(steady_w, 1.0);
        assert!(
            w_restart < steady_w,
            "first on-step after off cycle must be below steady-state: got {w_restart} W"
        );
    }
}
