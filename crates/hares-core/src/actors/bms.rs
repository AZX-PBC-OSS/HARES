//! Battery Management System actor -- dispatches charge/discharge control
//! signals based on the configured `BmsMode`, PV production, grid prices,
//! and battery SOC.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use chrono::{Datelike, Timelike};
use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_types::{
    BmsAction, BmsMode, BmsScheduleWindow, ControlSignal, EnvironmentState, EquipmentId,
    GridExportRule, HaresError, StormWatchTrigger, Telemetry,
};

use crate::Actor;

pub struct BatteryManagementActor {
    name: String,
    dispatch_target: DispatchTarget,
    equipment_id: Option<EquipmentId>,
    bms_mode: BmsMode,
    grid_export_rule: GridExportRule,
    charge_price_threshold: f64,
    discharge_price_threshold: f64,
    daily_avg_price: f64,
    current_day_ordinal0: u32,
    max_charge_kw: f64,
    max_discharge_kw: f64,
    price_schedule: Option<Arc<[f64]>>,
    steps_per_day: usize,
    last_action: String,
    storm_watch_active: bool,
    dr_active: bool,
    telemetry: Telemetry,
}

impl BatteryManagementActor {
    pub fn new(
        battery_name: &str,
        bms_mode: BmsMode,
        grid_export_rule: GridExportRule,
        max_charge_kw: f64,
        max_discharge_kw: f64,
        price_schedule: Option<Arc<[f64]>>,
        steps_per_day: usize,
    ) -> Self {
        Self::with_name(
            &format!("BatteryManagementActor:{battery_name}"),
            battery_name,
            bms_mode,
            grid_export_rule,
            max_charge_kw,
            max_discharge_kw,
            price_schedule,
            steps_per_day,
        )
    }

    #[allow(clippy::too_many_arguments)] // Why: all fields are distinct BMS configuration parameters; introducing a builder adds complexity for no structural benefit
    pub fn with_name(
        name: &str,
        battery_name: &str,
        bms_mode: BmsMode,
        grid_export_rule: GridExportRule,
        max_charge_kw: f64,
        max_discharge_kw: f64,
        price_schedule: Option<Arc<[f64]>>,
        steps_per_day: usize,
    ) -> Self {
        let mut telemetry = Telemetry::with_capacity(7);
        telemetry.insert("bms_action", -1.0);
        telemetry.insert("soc", f64::NAN);
        telemetry.insert("pv_kw", 0.0);
        telemetry.insert("load_kw", 0.0);
        telemetry.insert("bms_pv_stale_kw", 0.0);
        telemetry.insert("bms_pv_actual_kw", 0.0);
        telemetry.insert("bms_toggled", 0.0);
        Self {
            name: name.to_string(),
            dispatch_target: DispatchTarget::ByName(Arc::from(battery_name)),
            equipment_id: None,
            bms_mode,
            grid_export_rule,
            charge_price_threshold: 0.0,
            discharge_price_threshold: f64::INFINITY,
            daily_avg_price: 0.0,
            current_day_ordinal0: u32::MAX,
            max_charge_kw,
            max_discharge_kw,
            price_schedule,
            steps_per_day,
            last_action: String::new(),
            storm_watch_active: false,
            dr_active: false,
            telemetry,
        }
    }

    pub fn last_action(&self) -> &str {
        &self.last_action
    }

    pub fn bms_mode(&self) -> &BmsMode {
        &self.bms_mode
    }

    pub fn resolve_equipment_id(&mut self, equipment_id_by_name: &HashMap<String, EquipmentId>) {
        let battery_name = match &self.dispatch_target {
            DispatchTarget::ByName(n) => n.as_ref(),
            DispatchTarget::ByEndUse(_) => return,
        };
        self.equipment_id = match equipment_id_by_name.get(battery_name) {
            Some(id) => Some(*id),
            None => {
                tracing::warn!(
                    equipment = %battery_name,
                    actor = "BatteryManagementActor",
                    "Equipment name not found in registry — actor will operate without SOC feedback"
                );
                None
            }
        };
    }

    fn read_soc(&self, env: &EnvironmentState) -> Option<f64> {
        let id = self.equipment_id?;
        env.equipment_core
            .get(&id)
            .and_then(|co| co.state.soc)
            .map(|s| s.get())
    }

    fn emit(&self, signal: ControlSignal, out: &mut Vec<DispatchRequest>) {
        // All BMS signals use `Schedule` tier: the BMS executes a pre-defined
        // battery management schedule (self-consumption, TOU optimisation,
        // backup reserve). It is not a user override or grid DR actor.
        // This overrides the central `From<&ControlSignal> for PriorityTier`
        // mapping where `PowerLimit` would map to `Grid` — the BMS's use of
        // `PowerLimit` is a charge-rate cap within a scheduled mode, not a
        // grid-imposed power constraint.
        out.push(DispatchRequest {
            target: self.dispatch_target.clone(),
            signal,
            priority: PriorityTier::Schedule,
        });
    }

    /// Clamp discharge power (negative `active_power_kw`) based on grid export rule.
    ///
    /// `pv_kw` must be the actual current-step PV generation so that the clamp is
    /// correct for both the primary evaluate-mode path and the re-evaluation path.
    ///
    /// - `Unrestricted`: no clamping.
    /// - `Disabled`: clamp discharge so net export is zero (battery only offsets home load).
    /// - `SolarOnly`: clamp discharge so net export does not exceed PV generation.
    fn clamp_discharge_for_export(
        &self,
        discharge_kw: f64,
        pv_kw: f64,
        env: &EnvironmentState,
    ) -> f64 {
        // discharge_kw is the raw magnitude (positive value) of desired discharge.
        match self.grid_export_rule {
            GridExportRule::Unrestricted => discharge_kw,
            GridExportRule::Disabled => {
                // Battery can only offset home load, never export to grid.
                discharge_kw.min(env.electrical.base_load_kw.max(0.0))
            }
            GridExportRule::SolarOnly => {
                // Battery may discharge up to load + PV, so net grid export
                // (discharge - load) is at most PV generation.
                let max_allowed = env.electrical.base_load_kw.max(0.0) + pv_kw;
                discharge_kw.min(max_allowed)
            }
        }
    }

    fn evaluate_mode(
        &mut self,
        mode: &BmsMode,
        env: &EnvironmentState,
        out: &mut Vec<DispatchRequest>,
    ) {
        match mode {
            BmsMode::Manual => {
                self.last_action = "idle".into();
            }

            BmsMode::SelfConsumption {
                min_soc,
                max_soc,
                solar_only_charging,
                surplus_deadband_kw,
            } => {
                let Some(soc) = self.read_soc(env) else {
                    self.last_action = "idle:no_soc".into();
                    return;
                };
                let pv = env.electrical.actual_pv_kw_or_fallback();
                let load = env.electrical.base_load_kw;
                let surplus = pv - load;
                let deadband = *surplus_deadband_kw;

                if *solar_only_charging && env.electrical.actual_pv_kw <= 0.0 {
                    self.emit(ControlSignal::GridConnect { connected: false }, out);
                    self.last_action = "grid_disconnect:solar_only".into();
                    return;
                }

                let was_charging = self.last_action.contains("self_consumption:charge")
                    && !self.last_action.contains(":discharge");
                let was_discharging = self.last_action.contains("self_consumption:discharge");

                let should_charge = if was_charging {
                    surplus >= -deadband && soc < *max_soc
                } else {
                    surplus > deadband && soc < *max_soc
                };
                let should_discharge = if was_discharging {
                    surplus <= deadband && soc > *min_soc
                } else {
                    surplus < -deadband && soc > *min_soc
                };

                if should_charge {
                    let force_solar_only =
                        matches!(self.grid_export_rule, GridExportRule::Disabled);
                    self.emit(
                        ControlSignal::SelfConsumption {
                            enabled: true,
                            solar_only_charging: *solar_only_charging || force_solar_only,
                        },
                        out,
                    );
                    self.last_action = "self_consumption:charge".into();
                } else if should_discharge {
                    match self.grid_export_rule {
                        GridExportRule::Unrestricted => {
                            self.emit(
                                ControlSignal::SelfConsumption {
                                    enabled: true,
                                    solar_only_charging: false,
                                },
                                out,
                            );
                        }
                        GridExportRule::Disabled | GridExportRule::SolarOnly => {
                            let raw_discharge = (-surplus).min(self.max_discharge_kw);
                            let clamped = self.clamp_discharge_for_export(
                                raw_discharge,
                                env.electrical.actual_pv_kw_or_fallback(),
                                env,
                            );
                            self.emit(
                                ControlSignal::PowerSetpoint {
                                    active_power_kw: -clamped,
                                    reactive_power_kvar: None,
                                    min_soc: None,
                                    max_soc: None,
                                },
                                out,
                            );
                        }
                    }
                    self.last_action = "self_consumption:discharge".into();
                } else {
                    self.last_action = "idle:self_consumption".into();
                }
            }

            BmsMode::TimeOfUseOptimization {
                reserve_soc,
                charge_threshold_percentile,
                discharge_threshold_percentile,
                solar_only_charging,
                price_deadband,
            } => {
                let day_ordinal0 = env.current_time.ordinal0();
                if day_ordinal0 != self.current_day_ordinal0 {
                    self.recompute_tou_thresholds(
                        env,
                        *charge_threshold_percentile,
                        *discharge_threshold_percentile,
                    );
                }

                let Some(soc) = self.read_soc(env) else {
                    self.last_action = "idle:no_soc".into();
                    return;
                };
                let price = env.price_signal.electricity_price.unwrap_or(0.0);
                let deadband = *price_deadband;

                let was_charging = self.last_action.contains("tou:charge")
                    && !self.last_action.contains(":discharge");
                let was_discharging = self.last_action.contains("tou:discharge");

                let should_charge = if was_charging {
                    price <= self.charge_price_threshold + deadband && soc < (1.0 - reserve_soc)
                } else {
                    price <= self.charge_price_threshold && soc < (1.0 - reserve_soc)
                };
                let should_discharge = if was_discharging {
                    price >= self.discharge_price_threshold - deadband && soc > *reserve_soc
                } else {
                    price >= self.discharge_price_threshold && soc > *reserve_soc
                };

                if should_charge {
                    if *solar_only_charging && env.electrical.actual_pv_kw <= 0.0 {
                        self.emit(ControlSignal::GridConnect { connected: false }, out);
                        self.last_action = "grid_disconnect:tou_solar_only".into();
                        return;
                    }
                    self.emit(
                        ControlSignal::PowerSetpoint {
                            active_power_kw: self.max_charge_kw,
                            reactive_power_kvar: None,
                            min_soc: None,
                            max_soc: None,
                        },
                        out,
                    );
                    self.last_action = "tou:charge".into();
                } else if should_discharge {
                    let clamped = self.clamp_discharge_for_export(
                        self.max_discharge_kw,
                        env.electrical.actual_pv_kw_or_fallback(),
                        env,
                    );
                    self.emit(
                        ControlSignal::PowerSetpoint {
                            active_power_kw: -clamped,
                            reactive_power_kvar: None,
                            min_soc: None,
                            max_soc: None,
                        },
                        out,
                    );
                    self.last_action = "tou:discharge".into();
                } else {
                    self.last_action = "idle:tou".into();
                }
            }

            BmsMode::BackupReserve {
                target_soc,
                charge_from_grid,
                charge_rate_fraction,
                soc_deadband,
            } => {
                let Some(soc) = self.read_soc(env) else {
                    self.last_action = "idle:no_soc".into();
                    return;
                };
                let deadband = *soc_deadband;
                let was_charging = self.last_action.contains("backup:charge");

                let need_charge = if was_charging {
                    soc < *target_soc + deadband
                } else {
                    soc < *target_soc - deadband
                };

                if need_charge {
                    if !charge_from_grid && env.electrical.actual_pv_kw <= 0.0 {
                        self.emit(ControlSignal::GridConnect { connected: false }, out);
                        self.last_action = "grid_disconnect:backup_no_pv".into();
                        return;
                    }
                    self.emit(
                        ControlSignal::SOCTarget {
                            target_soc: *target_soc,
                            min_soc: None,
                            max_soc: Some(*target_soc),
                        },
                        out,
                    );
                    if *charge_rate_fraction < 1.0 {
                        self.emit(
                            ControlSignal::PowerLimit {
                                max_power_kw: charge_rate_fraction * self.max_charge_kw,
                                ramp_rate_kw_per_s: None,
                            },
                            out,
                        );
                    }
                    self.last_action = "backup:charge".into();
                } else {
                    self.last_action = "idle:backup_at_target".into();
                }
            }

            BmsMode::DemandResponse {
                base_mode,
                dr_discharge_rate,
                min_soc_during_dr,
                dr_deactivation_multiplier,
            } => {
                let price = env.price_signal.electricity_price.unwrap_or(0.0);
                let dr_active = self.is_dr_active(env, price, *dr_deactivation_multiplier);

                if dr_active {
                    self.dr_active = true;
                    let Some(soc) = self.read_soc(env) else {
                        self.last_action = "idle:no_soc".into();
                        return;
                    };
                    if soc > *min_soc_during_dr {
                        let raw = dr_discharge_rate * self.max_discharge_kw;
                        let clamped = self.clamp_discharge_for_export(
                            raw,
                            env.electrical.actual_pv_kw_or_fallback(),
                            env,
                        );
                        self.emit(
                            ControlSignal::PowerSetpoint {
                                active_power_kw: -clamped,
                                reactive_power_kvar: None,
                                min_soc: None,
                                max_soc: None,
                            },
                            out,
                        );
                        self.last_action = "dr:discharge".into();
                    } else {
                        self.last_action = "dr:soc_too_low".into();
                    }
                } else {
                    self.dr_active = false;
                    self.evaluate_mode(base_mode, env, out);
                }
            }

            BmsMode::Scheduled { windows } => {
                let weekday = env.current_time.weekday();
                let minute_of_day =
                    (env.current_time.hour() * 60 + env.current_time.minute()) as u16;

                if let Some(window) = find_matching_window(windows, weekday, minute_of_day) {
                    match &window.action {
                        BmsAction::Charge { rate_fraction } => {
                            self.emit(
                                ControlSignal::PowerSetpoint {
                                    active_power_kw: rate_fraction * self.max_charge_kw,
                                    reactive_power_kvar: None,
                                    min_soc: None,
                                    max_soc: None,
                                },
                                out,
                            );
                            self.last_action = "scheduled:charge".into();
                        }
                        BmsAction::Discharge { rate_fraction } => {
                            let raw = rate_fraction * self.max_discharge_kw;
                            let clamped = self.clamp_discharge_for_export(
                                raw,
                                env.electrical.actual_pv_kw_or_fallback(),
                                env,
                            );
                            self.emit(
                                ControlSignal::PowerSetpoint {
                                    active_power_kw: -clamped,
                                    reactive_power_kvar: None,
                                    min_soc: None,
                                    max_soc: None,
                                },
                                out,
                            );
                            self.last_action = "scheduled:discharge".into();
                        }
                        BmsAction::Idle => {
                            self.last_action = "scheduled:idle".into();
                        }
                        BmsAction::Hold { target_soc } => {
                            self.emit(
                                ControlSignal::SOCTarget {
                                    target_soc: *target_soc,
                                    min_soc: None,
                                    max_soc: None,
                                },
                                out,
                            );
                            self.last_action = "scheduled:hold".into();
                        }
                    }
                } else {
                    self.last_action = "idle:no_window".into();
                }
            }

            BmsMode::StormWatch {
                target_soc,
                trigger,
                base_mode,
            } => {
                let active = match trigger {
                    StormWatchTrigger::ManualEnable => true,
                    StormWatchTrigger::WeatherSignal {
                        wind_speed_threshold_m_s,
                        wind_speed_deactivation_threshold_m_s,
                    } => {
                        let deactivation = if *wind_speed_deactivation_threshold_m_s > 0.0 {
                            *wind_speed_deactivation_threshold_m_s
                        } else {
                            wind_speed_threshold_m_s * 0.9
                        };
                        if self.storm_watch_active {
                            env.weather.wind_speed_m_s >= deactivation
                        } else {
                            env.weather.wind_speed_m_s > *wind_speed_threshold_m_s
                        }
                    }
                };

                if active {
                    self.storm_watch_active = true;
                    self.emit(
                        ControlSignal::SOCTarget {
                            target_soc: *target_soc,
                            min_soc: None,
                            max_soc: None,
                        },
                        out,
                    );
                    self.last_action = "storm_watch:active".into();
                } else {
                    self.storm_watch_active = false;
                    self.evaluate_mode(base_mode, env, out);
                }
            }
        }
    }

    fn ensure_daily_prices(&mut self, env: &EnvironmentState) {
        let day_ordinal0 = env.current_time.ordinal0();
        if day_ordinal0 == self.current_day_ordinal0 {
            return;
        }

        let Some(prices) = &self.price_schedule else {
            return;
        };

        let day_of_year = day_ordinal0 as usize;
        let start = day_of_year * self.steps_per_day;
        let end = (start + self.steps_per_day).min(prices.len());

        if start >= prices.len() || start >= end {
            return;
        }

        let today_prices = &prices[start..end];
        let sum: f64 = today_prices.iter().sum();
        self.daily_avg_price = sum / today_prices.len() as f64;
        self.current_day_ordinal0 = day_ordinal0;
    }

    fn recompute_tou_thresholds(
        &mut self,
        env: &EnvironmentState,
        charge_percentile: f64,
        discharge_percentile: f64,
    ) {
        self.ensure_daily_prices(env);

        let Some(prices) = &self.price_schedule else {
            return;
        };

        let day_of_year = env.current_time.ordinal0() as usize;
        let start = day_of_year * self.steps_per_day;
        let end = (start + self.steps_per_day).min(prices.len());

        if start >= prices.len() || start >= end {
            return;
        }

        let today_prices = &prices[start..end];
        self.charge_price_threshold = compute_percentile(today_prices, charge_percentile);
        self.discharge_price_threshold = compute_percentile(today_prices, discharge_percentile);
    }

    fn is_dr_active(
        &mut self,
        env: &EnvironmentState,
        current_price: f64,
        dr_deactivation_multiplier: f64,
    ) -> bool {
        self.ensure_daily_prices(env);

        if self.price_schedule.is_none() {
            return false;
        }

        if self.daily_avg_price <= 0.0 {
            return false;
        }

        if self.dr_active {
            let deactivation = if dr_deactivation_multiplier > 0.0 {
                dr_deactivation_multiplier
            } else {
                2.0
            };
            current_price > deactivation * self.daily_avg_price
        } else {
            current_price > 2.0 * self.daily_avg_price
        }
    }

    #[allow(clippy::too_many_arguments)]
    // Why: all arguments are distinct SelfConsumption configuration parameters
    // and per-step context; bundling them into a struct would add indirection
    // for no structural benefit.
    /// Re-evaluate SelfConsumption decision with actual (not prior-step) PV.
    fn reevaluate_self_consumption(
        &mut self,
        pv_kw: f64,
        env: &EnvironmentState,
        min_soc: f64,
        max_soc: f64,
        solar_only_charging: bool,
        surplus_deadband_kw: f64,
        out: &mut Vec<DispatchRequest>,
    ) {
        let Some(soc) = self.read_soc(env) else {
            return;
        };
        let load = env.electrical.base_load_kw;
        let surplus = pv_kw - load;
        let deadband = surplus_deadband_kw;

        // If solar-only and actual PV is zero, disconnect from grid.
        // This handles the case where stale PV was positive (grid connected)
        // but actual PV is now zero — the battery should stop importing.
        let force_solar_only = matches!(self.grid_export_rule, GridExportRule::Disabled);
        let effective_solar_only = solar_only_charging || force_solar_only;

        if effective_solar_only && pv_kw <= 0.0 {
            // No real PV: disconnect from grid regardless of last_action.
            self.emit(ControlSignal::GridConnect { connected: false }, out);
            self.last_action = "grid_disconnect:solar_only".into();
            return;
        }

        // Re-connect grid if PV is back and we were previously forced off.
        if effective_solar_only && pv_kw > 0.0 && self.last_action.contains("grid_disconnect") {
            self.emit(ControlSignal::GridConnect { connected: true }, out);
            self.last_action = "self_consumption:grid_reconnected".into();
            return;
        }

        let was_charging = self.last_action.contains("self_consumption:charge")
            && !self.last_action.contains(":discharge");
        let was_discharging = self.last_action.contains("self_consumption:discharge");

        let should_charge = if was_charging {
            surplus >= -deadband && soc < max_soc
        } else {
            surplus > deadband && soc < max_soc
        };
        let should_discharge = if was_discharging {
            surplus <= deadband && soc > min_soc
        } else {
            surplus < -deadband && soc > min_soc
        };

        if should_charge {
            if !self.is_last_action_charging() {
                self.emit(
                    ControlSignal::SelfConsumption {
                        enabled: true,
                        solar_only_charging: effective_solar_only,
                    },
                    out,
                );
            }
            self.last_action = "self_consumption:charge".into();
        } else if should_discharge {
            if !self.is_last_action_discharging() {
                match self.grid_export_rule {
                    GridExportRule::Unrestricted => {
                        self.emit(
                            ControlSignal::SelfConsumption {
                                enabled: true,
                                solar_only_charging: false,
                            },
                            out,
                        );
                    }
                    GridExportRule::Disabled | GridExportRule::SolarOnly => {
                        let raw_discharge = (-surplus).min(self.max_discharge_kw);
                        let clamped = self.clamp_discharge_for_export(raw_discharge, pv_kw, env);
                        self.emit(
                            ControlSignal::PowerSetpoint {
                                active_power_kw: -clamped,
                                reactive_power_kvar: None,
                                min_soc: None,
                                max_soc: None,
                            },
                            out,
                        );
                    }
                }
            }
            self.last_action = "self_consumption:discharge".into();
        } else if self.is_last_action_charging() || self.is_last_action_discharging() {
            self.emit(
                ControlSignal::SelfConsumption {
                    enabled: false,
                    solar_only_charging: false,
                },
                out,
            );
            self.last_action = "idle:self_consumption".into();
        }
    }

    /// Re-evaluate BackupReserve (no grid charging) with actual PV.
    fn reevaluate_backup_no_grid(
        &mut self,
        pv_kw: f64,
        _env: &EnvironmentState,
        out: &mut Vec<DispatchRequest>,
    ) {
        // Stale PV was zero → grid was disconnected. Actual PV is positive
        // → reconnect so the battery can charge from solar.
        if pv_kw > 0.0 && self.last_action.contains("grid_disconnect") {
            self.emit(ControlSignal::GridConnect { connected: true }, out);
            self.last_action = "backup:grid_reconnected".into();
        }
        // Stale PV was positive (grid connected) but actual PV is zero
        // and charge_from_grid is false → disconnect.
        if pv_kw <= 0.0 && !self.last_action.contains("grid_disconnect") {
            self.emit(ControlSignal::GridConnect { connected: false }, out);
            self.last_action = "grid_disconnect:backup_no_pv".into();
        }
    }

    /// Re-evaluate TimeOfUseOptimization (solar-only) with actual PV.
    fn reevaluate_tou_solar_only(
        &mut self,
        pv_kw: f64,
        _env: &EnvironmentState,
        out: &mut Vec<DispatchRequest>,
    ) {
        if pv_kw > 0.0 && self.last_action.contains("grid_disconnect") {
            self.emit(ControlSignal::GridConnect { connected: true }, out);
            self.last_action = "tou:grid_reconnected".into();
        }
        if pv_kw <= 0.0 && !self.last_action.contains("grid_disconnect") {
            self.emit(ControlSignal::GridConnect { connected: false }, out);
            self.last_action = "grid_disconnect:tou_solar_only".into();
        }
    }

    /// Re-evaluate the given mode (potentially nested) using actual-step PV data.
    ///
    /// Dispatches to the correct leaf re-evaluator (`reevaluate_self_consumption`,
    /// `reevaluate_backup_no_grid`, `reevaluate_tou_solar_only`) for PV-dependent
    /// leaf modes. For composite modes (`DemandResponse`, `StormWatch`), checks
    /// whether the outer mode is active and, if inactive, recurses into the base
    /// mode so that SelfConsumption (or equivalent) decisions use actual PV.
    ///
    /// Non-PV-dependent modes are a no-op.
    fn adjust_for_pv_in_mode(
        &mut self,
        pv_kw: f64,
        env: &EnvironmentState,
        mode: &BmsMode,
        out: &mut Vec<DispatchRequest>,
    ) {
        match mode {
            BmsMode::SelfConsumption {
                min_soc,
                max_soc,
                solar_only_charging,
                surplus_deadband_kw,
            } => {
                self.reevaluate_self_consumption(
                    pv_kw,
                    env,
                    *min_soc,
                    *max_soc,
                    *solar_only_charging,
                    *surplus_deadband_kw,
                    out,
                );
            }
            BmsMode::BackupReserve {
                charge_from_grid: false,
                ..
            } => {
                self.reevaluate_backup_no_grid(pv_kw, env, out);
            }
            BmsMode::TimeOfUseOptimization {
                solar_only_charging: true,
                ..
            } => {
                self.reevaluate_tou_solar_only(pv_kw, env, out);
            }
            BmsMode::DemandResponse {
                base_mode,
                dr_deactivation_multiplier,
                ..
            } if Self::recurse_pv_dependent(base_mode) => {
                let price = env.price_signal.electricity_price.unwrap_or(0.0);
                let dr_active = self.is_dr_active(env, price, *dr_deactivation_multiplier);
                if !dr_active {
                    self.adjust_for_pv_in_mode(pv_kw, env, base_mode, out);
                }
            }
            BmsMode::StormWatch {
                trigger, base_mode, ..
            } if Self::recurse_pv_dependent(base_mode) => {
                let sw_active = match trigger {
                    StormWatchTrigger::ManualEnable => true,
                    StormWatchTrigger::WeatherSignal {
                        wind_speed_threshold_m_s,
                        wind_speed_deactivation_threshold_m_s,
                    } => {
                        let deactivation = if *wind_speed_deactivation_threshold_m_s > 0.0 {
                            *wind_speed_deactivation_threshold_m_s
                        } else {
                            wind_speed_threshold_m_s * 0.9
                        };
                        if self.storm_watch_active {
                            env.weather.wind_speed_m_s >= deactivation
                        } else {
                            env.weather.wind_speed_m_s > *wind_speed_threshold_m_s
                        }
                    }
                };
                if !sw_active {
                    self.adjust_for_pv_in_mode(pv_kw, env, base_mode, out);
                }
            }
            _ => { /* PV-independent mode: no re-evaluation needed */ }
        }
    }

    /// Returns true if the BMS mode (potentially nested) depends on real-time PV.
    fn recurse_pv_dependent(mode: &BmsMode) -> bool {
        match mode {
            BmsMode::SelfConsumption { .. } => true,
            BmsMode::BackupReserve {
                charge_from_grid: false,
                ..
            }
            | BmsMode::TimeOfUseOptimization {
                solar_only_charging: true,
                ..
            } => true,
            BmsMode::DemandResponse { base_mode, .. } => Self::recurse_pv_dependent(base_mode),
            BmsMode::StormWatch { base_mode, .. } => Self::recurse_pv_dependent(base_mode),
            _ => false,
        }
    }

    /// Returns true if `last_action` indicates the BMS decided to charge.
    /// Uses colon-prefixed matching to distinguish `:charge` from `:discharge`.
    fn is_last_action_charging(&self) -> bool {
        self.last_action.contains(":charge")
    }

    /// Returns true if `last_action` indicates the BMS decided to discharge.
    fn is_last_action_discharging(&self) -> bool {
        self.last_action.contains(":discharge")
    }
}

/// Serializable snapshot of BatteryManagementActor mutable runtime state for checkpointing.
#[derive(Serialize, Deserialize)]
struct BmsSnapshot {
    current_day_ordinal0: u32,
    daily_avg_price: f64,
    charge_price_threshold: f64,
    discharge_price_threshold: f64,
    last_action: String,
    storm_watch_active: bool,
    dr_active: bool,
}

impl Actor for BatteryManagementActor {
    fn name(&self) -> &str {
        &self.name
    }

    fn telemetry(&self) -> Option<&Telemetry> {
        Some(&self.telemetry)
    }

    fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        let action_before = self.last_action.clone();
        let mode = std::mem::take(&mut self.bms_mode);
        self.evaluate_mode(&mode, env, out);
        self.bms_mode = mode;

        // Populate telemetry: bms_action uses a numeric code from last_action.
        self.telemetry
            .set("bms_action", bms_action_code(&self.last_action));
        self.telemetry
            .set("soc", self.read_soc(env).unwrap_or(f64::NAN));
        let pv_stale = env.electrical.actual_pv_kw_or_fallback();
        self.telemetry.set("pv_kw", pv_stale);
        self.telemetry.set("bms_pv_stale_kw", pv_stale);
        self.telemetry.set("load_kw", env.electrical.base_load_kw);
        let toggled = self.last_action != action_before;
        self.telemetry
            .set("bms_toggled", if toggled { 1.0 } else { 0.0 });
    }

    fn adjust_for_pv(
        &mut self,
        pv_kw: f64,
        env: &EnvironmentState,
        out: &mut Vec<DispatchRequest>,
    ) {
        self.telemetry.set("bms_pv_actual_kw", pv_kw);

        // Re-evaluate PV-dependent modes with actual-step PV data.
        // Non-PV-dependent and active composite modes are no-ops.
        let mode = std::mem::take(&mut self.bms_mode);
        self.adjust_for_pv_in_mode(pv_kw, env, &mode, out);
        self.bms_mode = mode;
    }

    fn save_state(&self) -> Result<Vec<u8>, HaresError> {
        let snap = BmsSnapshot {
            current_day_ordinal0: self.current_day_ordinal0,
            daily_avg_price: self.daily_avg_price,
            charge_price_threshold: self.charge_price_threshold,
            discharge_price_threshold: self.discharge_price_threshold,
            last_action: self.last_action.clone(),
            storm_watch_active: self.storm_watch_active,
            dr_active: self.dr_active,
        };
        postcard::to_allocvec(&snap)
            .map_err(|e| HaresError::Io(format!("BatteryManagementActor save_state: {e}")))
    }

    fn load_state(&mut self, data: &[u8]) -> Result<(), HaresError> {
        if data.is_empty() {
            return Ok(());
        }
        let snap: BmsSnapshot = postcard::from_bytes(data)
            .map_err(|e| HaresError::Io(format!("BatteryManagementActor load_state: {e}")))?;
        self.current_day_ordinal0 = snap.current_day_ordinal0;
        self.daily_avg_price = snap.daily_avg_price;
        self.charge_price_threshold = snap.charge_price_threshold;
        self.discharge_price_threshold = snap.discharge_price_threshold;
        self.last_action = snap.last_action;
        self.storm_watch_active = snap.storm_watch_active;
        self.dr_active = snap.dr_active;
        Ok(())
    }
}

fn find_matching_window(
    windows: &[BmsScheduleWindow],
    weekday: chrono::Weekday,
    minute_of_day: u16,
) -> Option<&BmsScheduleWindow> {
    windows
        .iter()
        .find(|w| w.time_window.contains(weekday, minute_of_day))
}

fn compute_percentile(prices: &[f64], percentile: f64) -> f64 {
    if prices.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f64> = prices.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((percentile * (sorted.len() - 1) as f64).round() as usize).min(sorted.len() - 1);
    sorted[idx]
}

/// Maps `last_action` string to a numeric code for telemetry.
/// 0=idle, 1=charge, 2=discharge, 3=grid_disconnect, 4=reconnect, 5=storm_watch, -1=unknown.
fn bms_action_code(action: &str) -> f64 {
    if action.contains("discharge") {
        2.0
    } else if action.contains("charge") {
        1.0
    } else if action.contains("grid_disconnect") {
        3.0
    } else if action.contains("reconnect") {
        4.0
    } else if action.contains("storm_watch") {
        5.0
    } else if action.contains("idle") {
        0.0
    } else {
        -1.0
    }
}

#[cfg(test)]
mod tests {
    use hares_types::{
        BmsScheduleWindow, BmsTimeWindow, CoreOutput, CoreState, DayFilter, ElectricalSummary,
        EquipmentId, PriceSignal, Soc, WeatherState,
    };

    use super::*;
    use crate::actor::testing::TestEnvBuilder;

    fn set_soc(
        actor: &mut BatteryManagementActor,
        env: &mut EnvironmentState,
        battery_name: &str,
        soc: f64,
    ) {
        let id = EquipmentId(1);
        let mut id_by_name = std::collections::HashMap::new();
        id_by_name.insert(battery_name.to_string(), id);
        actor.resolve_equipment_id(&id_by_name);
        env.equipment_core.insert(
            id,
            CoreOutput {
                state: CoreState {
                    soc: Some(Soc::try_from(soc).expect("valid test soc")),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
    }

    #[test]
    fn bms_manual_no_dispatch() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::Manual,
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let env = TestEnvBuilder::new().build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn bms_missing_equipment_core_entry_is_graceful() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut id_by_name = std::collections::HashMap::new();
        id_by_name.insert("bat1".to_string(), EquipmentId(1));
        actor.resolve_equipment_id(&id_by_name);

        let env = TestEnvBuilder::new().build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert!(
            out.is_empty(),
            "missing equipment_core entry must not panic or emit invalid control"
        );
        assert_eq!(actor.last_action(), "idle:no_soc");
    }

    #[test]
    fn bms_self_consumption_pv_surplus_charges() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 5.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::SelfConsumption { enabled: true, .. }
        ));
        assert_eq!(out[0].priority, PriorityTier::Schedule);
    }

    #[test]
    fn bms_self_consumption_deficit_discharges() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 1.0,
                base_load_kw: 4.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::SelfConsumption {
                enabled,
                solar_only_charging,
            } => {
                assert!(*enabled);
                assert!(!solar_only_charging);
            }
            other => panic!("expected SelfConsumption, got {other:?}"),
        }
    }

    #[test]
    fn bms_self_consumption_solar_only_blocks_grid() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: true,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 0.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::GridConnect { connected: false }
        ));
    }

    #[test]
    fn bms_tou_low_price_charges() {
        let prices: Vec<f64> = (0..24).map(|h| if h < 8 { 0.05 } else { 0.30 }).collect();
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.2,
                charge_threshold_percentile: 0.25,
                discharge_threshold_percentile: 0.75,
                solar_only_charging: false,
                price_deadband: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            Some(prices.into()),
            24,
        );

        let mut env = TestEnvBuilder::new()
            .hour(3)
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.05),
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!(*active_power_kw > 0.0);
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn bms_tou_high_price_discharges() {
        let prices: Vec<f64> = (0..24).map(|h| if h < 8 { 0.05 } else { 0.30 }).collect();
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.2,
                charge_threshold_percentile: 0.25,
                discharge_threshold_percentile: 0.75,
                solar_only_charging: false,
                price_deadband: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            Some(prices.into()),
            24,
        );

        let mut env = TestEnvBuilder::new()
            .hour(14)
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.30),
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.8);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!(*active_power_kw < 0.0);
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn bms_tou_respects_reserve_soc() {
        let prices: Vec<f64> = (0..24).map(|h| if h < 8 { 0.05 } else { 0.30 }).collect();
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.2,
                charge_threshold_percentile: 0.25,
                discharge_threshold_percentile: 0.75,
                solar_only_charging: false,
                price_deadband: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            Some(prices.into()),
            24,
        );

        let mut env = TestEnvBuilder::new()
            .hour(14)
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.30),
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.15);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn bms_backup_reserve_charges_to_target() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::BackupReserve {
                target_soc: 0.8,
                charge_from_grid: true,
                charge_rate_fraction: 0.5,
                soc_deadband: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );

        let mut env = TestEnvBuilder::new().build();
        set_soc(&mut actor, &mut env, "bat1", 0.3);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 2);
        match &out[0].signal {
            ControlSignal::SOCTarget {
                target_soc,
                max_soc,
                ..
            } => {
                assert!((target_soc - 0.8).abs() < 1e-9);
                assert_eq!(*max_soc, Some(0.8));
            }
            other => panic!("expected SOCTarget, got {other:?}"),
        }
        match &out[1].signal {
            ControlSignal::PowerLimit {
                max_power_kw,
                ramp_rate_kw_per_s,
            } => {
                assert!((*max_power_kw - 2.5).abs() < 1e-9);
                assert_eq!(*ramp_rate_kw_per_s, None);
            }
            other => panic!("expected PowerLimit, got {other:?}"),
        }
    }

    #[test]
    fn bms_backup_reserve_idle_above_target() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::BackupReserve {
                target_soc: 0.8,
                charge_from_grid: true,
                charge_rate_fraction: 0.5,
                soc_deadband: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );

        let mut env = TestEnvBuilder::new().build();
        set_soc(&mut actor, &mut env, "bat1", 0.9);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn bms_demand_response_active() {
        let prices: Vec<f64> = vec![0.10; 24];
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::DemandResponse {
                base_mode: Box::new(BmsMode::Manual),
                dr_discharge_rate: 0.8,
                min_soc_during_dr: 0.1,
                dr_deactivation_multiplier: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            Some(prices.into()),
            24,
        );

        let mut env = TestEnvBuilder::new()
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.50),
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.6);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!((*active_power_kw - (-0.8 * 5.0)).abs() < 1e-9);
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn bms_demand_response_delegates_to_base() {
        let prices: Vec<f64> = vec![0.10; 24];
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::DemandResponse {
                base_mode: Box::new(BmsMode::SelfConsumption {
                    min_soc: 0.1,
                    max_soc: 0.95,
                    solar_only_charging: false,
                    surplus_deadband_kw: 0.0,
                }),
                dr_discharge_rate: 0.8,
                min_soc_during_dr: 0.1,
                dr_deactivation_multiplier: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            Some(prices.into()),
            24,
        );

        let mut env = TestEnvBuilder::new()
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.10),
                ..Default::default()
            })
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 5.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::SelfConsumption { enabled: true, .. }
        ));
    }

    #[test]
    fn bms_scheduled_charge_window() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::Scheduled {
                windows: vec![BmsScheduleWindow {
                    time_window: BmsTimeWindow {
                        day: DayFilter::Any,
                        start_minute: 0,
                        end_minute: 1440,
                    },
                    action: BmsAction::Charge { rate_fraction: 0.5 },
                }],
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );

        let env = TestEnvBuilder::new().hour(6).build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!((*active_power_kw - 2.5).abs() < 1e-9);
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn bms_scheduled_no_matching_window() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::Scheduled {
                windows: vec![BmsScheduleWindow {
                    time_window: BmsTimeWindow {
                        day: DayFilter::Weekends,
                        start_minute: 0,
                        end_minute: 360,
                    },
                    action: BmsAction::Charge { rate_fraction: 1.0 },
                }],
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );

        // 2026-01-05 is a Monday (weekday), so weekend window won't match
        let env = TestEnvBuilder::new().date(2026, 1, 5).hour(3).build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn bms_storm_watch_active_full_charge() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::StormWatch {
                target_soc: 1.0,
                trigger: StormWatchTrigger::ManualEnable,
                base_mode: Box::new(BmsMode::Manual),
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );

        let env = TestEnvBuilder::new().build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::SOCTarget { target_soc, .. } => {
                assert!((*target_soc - 1.0).abs() < 1e-9);
            }
            other => panic!("expected SOCTarget, got {other:?}"),
        }
    }

    #[test]
    fn bms_storm_watch_weather_signal_high_wind_activates() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::StormWatch {
                target_soc: 1.0,
                trigger: StormWatchTrigger::WeatherSignal {
                    wind_speed_threshold_m_s: 25.0,
                    wind_speed_deactivation_threshold_m_s: 0.0,
                },
                base_mode: Box::new(BmsMode::Manual),
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );

        let env = TestEnvBuilder::new()
            .with_weather(WeatherState {
                wind_speed_m_s: 30.0,
                ..Default::default()
            })
            .build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].signal, ControlSignal::SOCTarget { .. }));
    }

    #[test]
    fn bms_storm_watch_inactive_delegates() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::StormWatch {
                target_soc: 1.0,
                trigger: StormWatchTrigger::WeatherSignal {
                    wind_speed_threshold_m_s: 25.0,
                    wind_speed_deactivation_threshold_m_s: 0.0,
                },
                base_mode: Box::new(BmsMode::SelfConsumption {
                    min_soc: 0.1,
                    max_soc: 0.95,
                    solar_only_charging: false,
                    surplus_deadband_kw: 0.0,
                }),
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );

        let mut env = TestEnvBuilder::new()
            .with_weather(WeatherState {
                wind_speed_m_s: 5.0,
                ..Default::default()
            })
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 5.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::SelfConsumption { enabled: true, .. }
        ));
    }

    #[test]
    fn bms_tou_threshold_recomputed_only_on_day_boundary() {
        let prices: Vec<f64> = (0..48).map(|h| if h < 8 { 0.05 } else { 0.30 }).collect();
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.2,
                charge_threshold_percentile: 0.25,
                discharge_threshold_percentile: 0.75,
                solar_only_charging: false,
                price_deadband: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            Some(prices.into()),
            24,
        );

        let mut env = TestEnvBuilder::new()
            .hour(3)
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.05),
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        let charge_threshold_after_first = actor.charge_price_threshold;
        let discharge_threshold_after_first = actor.discharge_price_threshold;

        // Second call same day: thresholds unchanged
        out.clear();
        actor.decide(&env, &mut out);

        assert!((actor.charge_price_threshold - charge_threshold_after_first).abs() < 1e-15,);
        assert!((actor.discharge_price_threshold - discharge_threshold_after_first).abs() < 1e-15,);
    }

    #[test]
    fn bms_tou_soc_at_reserve_does_not_discharge() {
        let prices: Vec<f64> = (0..24).map(|h| if h < 8 { 0.05 } else { 0.30 }).collect();
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.2,
                charge_threshold_percentile: 0.25,
                discharge_threshold_percentile: 0.75,
                solar_only_charging: false,
                price_deadband: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            Some(prices.into()),
            24,
        );

        let mut env = TestEnvBuilder::new()
            .hour(14)
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.30),
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.2);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn bms_self_consumption_soc_at_min_does_not_discharge() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 1.0,
                base_load_kw: 4.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.1);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn bms_backup_reserve_soc_at_target_idles() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::BackupReserve {
                target_soc: 0.8,
                charge_from_grid: true,
                charge_rate_fraction: 1.0,
                soc_deadband: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );

        let mut env = TestEnvBuilder::new().build();
        set_soc(&mut actor, &mut env, "bat1", 0.8);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn bms_no_soc_telemetry_emits_nothing() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 5.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert!(out.is_empty());
        assert_eq!(actor.last_action(), "idle:no_soc");
    }

    #[test]
    fn bms_tou_solar_only_blocks_grid_charging() {
        let prices: Vec<f64> = (0..24).map(|h| if h < 8 { 0.05 } else { 0.30 }).collect();
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.2,
                charge_threshold_percentile: 0.25,
                discharge_threshold_percentile: 0.75,
                solar_only_charging: true,
                price_deadband: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            Some(prices.into()),
            24,
        );

        let mut env = TestEnvBuilder::new()
            .hour(3)
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.05),
                ..Default::default()
            })
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 0.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::GridConnect { connected: false }
        ));
    }

    #[test]
    fn compute_percentile_empty_returns_zero() {
        assert_eq!(compute_percentile(&[], 0.5), 0.0);
    }

    #[test]
    fn bms_last_action_tracks_decisions() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::Manual,
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let env = TestEnvBuilder::new().build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(actor.last_action(), "idle");
    }

    #[test]
    fn bms_disabled_export_clamps_discharge_to_home_load() {
        // Battery wants to discharge 5 kW, but home load is only 2 kW.
        // With Disabled export rule, discharge should be clamped to 2 kW.
        let prices = vec![0.10_f64; 24];
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.1,
                charge_threshold_percentile: 0.3,
                discharge_threshold_percentile: 0.7,
                solar_only_charging: false,
                price_deadband: 0.0,
            },
            GridExportRule::Disabled,
            5.0,
            5.0,
            Some(Arc::from(prices.as_slice())),
            24,
        );

        let mut env = TestEnvBuilder::new()
            .hour(14)
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.30),
                ..Default::default()
            })
            .with_electrical(ElectricalSummary {
                base_load_kw: 2.0,
                pv_generation_kw: 0.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.8);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                // Clamped to home load: -2.0 kW (not -5.0)
                assert!(
                    (*active_power_kw + 2.0).abs() < 1e-10,
                    "discharge should be clamped to home load (2 kW), got {}",
                    active_power_kw
                );
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn bms_disabled_export_allows_full_discharge_when_load_exceeds() {
        // Home load 8 kW > max discharge 5 kW → no clamping needed.
        let prices = vec![0.10_f64; 24];
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.1,
                charge_threshold_percentile: 0.3,
                discharge_threshold_percentile: 0.7,
                solar_only_charging: false,
                price_deadband: 0.0,
            },
            GridExportRule::Disabled,
            5.0,
            5.0,
            Some(Arc::from(prices.as_slice())),
            24,
        );

        let mut env = TestEnvBuilder::new()
            .hour(14)
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.30),
                ..Default::default()
            })
            .with_electrical(ElectricalSummary {
                base_load_kw: 8.0,
                pv_generation_kw: 0.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.8);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                // Full discharge allowed: load exceeds battery capacity.
                assert!(
                    (*active_power_kw + 5.0).abs() < 1e-10,
                    "discharge should be full 5 kW when load exceeds, got {}",
                    active_power_kw
                );
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn bms_unrestricted_export_no_clamping() {
        // Unrestricted: full 5 kW discharge regardless of home load.
        let prices = vec![0.10_f64; 24];
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.1,
                charge_threshold_percentile: 0.3,
                discharge_threshold_percentile: 0.7,
                solar_only_charging: false,
                price_deadband: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            Some(Arc::from(prices.as_slice())),
            24,
        );

        let mut env = TestEnvBuilder::new()
            .hour(14)
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.30),
                ..Default::default()
            })
            .with_electrical(ElectricalSummary {
                base_load_kw: 1.0,
                pv_generation_kw: 0.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.8);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!(
                    (*active_power_kw + 5.0).abs() < 1e-10,
                    "unrestricted discharge should be full 5 kW, got {}",
                    active_power_kw
                );
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn bms_solar_only_export_allows_load_plus_pv() {
        // SolarOnly: discharge clamped to home load + PV generation.
        // Home load 2 kW, PV 1 kW → max discharge 3 kW.
        let prices = vec![0.10_f64; 24];
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.1,
                charge_threshold_percentile: 0.3,
                discharge_threshold_percentile: 0.7,
                solar_only_charging: false,
                price_deadband: 0.0,
            },
            GridExportRule::SolarOnly,
            5.0,
            5.0,
            Some(Arc::from(prices.as_slice())),
            24,
        );

        let mut env = TestEnvBuilder::new()
            .hour(14)
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.30),
                ..Default::default()
            })
            .with_electrical(ElectricalSummary {
                base_load_kw: 2.0,
                pv_generation_kw: 1.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.8);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!(
                    (*active_power_kw + 3.0).abs() < 1e-10,
                    "SolarOnly discharge should be clamped to load+PV (3 kW), got {}",
                    active_power_kw
                );
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn bms_self_consumption_disabled_export_clamps_discharge() {
        // SelfConsumption + Disabled: deficit is 3 kW (load 5, PV 2).
        // Raw discharge = min(deficit=3, max_discharge=5) = 3 kW.
        // Export clamp = min(3, load=5) = 3 kW. Deficit is the binding constraint.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.9,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Disabled,
            5.0,
            5.0,
            None,
            24,
        );

        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 2.0,
                base_load_kw: 5.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                // Deficit is 3 kW. With Disabled export, clamped to min(3, 5) = 3.
                assert!(
                    (*active_power_kw + 3.0).abs() < 1e-10,
                    "self_consumption+disabled discharge should be 3 kW (deficit), got {}",
                    active_power_kw
                );
            }
            other => panic!("expected PowerSetpoint for Disabled export, got {other:?}"),
        }
    }

    #[test]
    fn bms_self_consumption_disabled_export_clamps_to_load_when_no_pv() {
        // SelfConsumption + Disabled: PV=0, load=2, max_discharge=5.
        // Deficit = 2 kW. Clamped to min(2, 2) = 2. (load only, no export)
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.9,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Disabled,
            5.0,
            5.0,
            None,
            24,
        );

        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 0.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!(
                    (*active_power_kw + 2.0).abs() < 1e-10,
                    "discharge should match load (2 kW), got {}",
                    active_power_kw
                );
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn bms_self_consumption_unrestricted_uses_self_consumption_signal() {
        // SelfConsumption + Unrestricted: should emit SelfConsumption signal (not PowerSetpoint)
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.9,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );

        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 0.0,
                base_load_kw: 3.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert!(
            matches!(out[0].signal, ControlSignal::SelfConsumption { .. }),
            "unrestricted self-consumption should emit SelfConsumption signal, got {:?}",
            out[0].signal
        );
    }

    #[test]
    fn bms_self_consumption_solar_only_clamps_to_load_plus_pv() {
        // SelfConsumption + SolarOnly: load=4, PV=1, max_discharge=5.
        // Deficit = 3 kW. SolarOnly max = load + PV = 5.
        // raw_discharge = min(3, 5) = 3. clamped = min(3, 5) = 3.
        // Here the deficit is the binding constraint, not the export rule.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.9,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::SolarOnly,
            5.0,
            5.0,
            None,
            24,
        );

        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 1.0,
                base_load_kw: 4.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                assert!(
                    (*active_power_kw + 3.0).abs() < 1e-10,
                    "SolarOnly self-consumption discharge should be 3 kW (deficit), got {}",
                    active_power_kw
                );
            }
            other => panic!("expected PowerSetpoint for SolarOnly, got {other:?}"),
        }
    }

    #[test]
    fn bms_self_consumption_disabled_export_clamp_is_binding() {
        // Test where the export clamp is the binding constraint, not the deficit.
        // SelfConsumption + Disabled: load=1, PV=0, max_discharge=5.
        // Deficit = 1 kW. raw_discharge = min(1, 5) = 1.
        // Export clamp (Disabled) = min(1, load=1) = 1. Both align.
        // Now set max_discharge=0.5 so hardware cap binds:
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.9,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Disabled,
            5.0,
            0.5, // max_discharge_kw is small
            None,
            24,
        );

        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 0.0,
                base_load_kw: 3.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.8);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                // deficit=3, but max_discharge=0.5 → hardware cap binds.
                assert!(
                    (*active_power_kw + 0.5).abs() < 1e-10,
                    "hardware cap (0.5 kW) should be binding, got {}",
                    active_power_kw
                );
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    #[test]
    fn bms_self_consumption_disabled_forces_solar_only_charging() {
        // SelfConsumption + Disabled + surplus: battery should charge from PV only,
        // not grid, even when solar_only_charging is false in the BmsMode config.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.9,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0, // user says allow grid charging
            },
            GridExportRule::Disabled, // but export rule says no grid interaction
            5.0,
            5.0,
            None,
            24,
        );

        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 4.0,
                base_load_kw: 2.0, // surplus = 2 kW
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.3);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::SelfConsumption {
                solar_only_charging,
                ..
            } => {
                assert!(
                    *solar_only_charging,
                    "GridExportRule::Disabled should force solar_only_charging=true"
                );
            }
            other => panic!("expected SelfConsumption signal, got {other:?}"),
        }
    }

    #[test]
    fn bms_self_consumption_unrestricted_respects_user_solar_only_false() {
        // Unrestricted: user's solar_only_charging=false should be preserved.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.9,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );

        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 4.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.3);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::SelfConsumption {
                solar_only_charging,
                ..
            } => {
                assert!(
                    !solar_only_charging,
                    "Unrestricted should preserve user's solar_only_charging=false"
                );
            }
            other => panic!("expected SelfConsumption signal, got {other:?}"),
        }
    }

    #[test]
    fn bms_storm_watch_custom_threshold_triggers_and_does_not_trigger() {
        let make_actor = || {
            BatteryManagementActor::new(
                "bat1",
                BmsMode::StormWatch {
                    target_soc: 1.0,
                    trigger: StormWatchTrigger::WeatherSignal {
                        wind_speed_threshold_m_s: 20.0,
                        wind_speed_deactivation_threshold_m_s: 0.0,
                    },
                    base_mode: Box::new(BmsMode::Manual),
                },
                GridExportRule::Unrestricted,
                5.0,
                5.0,
                None,
                24,
            )
        };

        // 22 m/s exceeds 20 m/s threshold → storm watch activates
        let mut actor_above = make_actor();
        let env_above = TestEnvBuilder::new()
            .with_weather(WeatherState {
                wind_speed_m_s: 22.0,
                ..Default::default()
            })
            .build();
        let mut out = Vec::new();
        actor_above.decide(&env_above, &mut out);
        assert_eq!(
            out.len(),
            1,
            "wind 22 m/s should trigger at threshold 20 m/s"
        );
        assert!(matches!(out[0].signal, ControlSignal::SOCTarget { .. }));

        // 18 m/s below 20 m/s threshold → delegates to base (Manual = no output)
        let mut actor_below = make_actor();
        let env_below = TestEnvBuilder::new()
            .with_weather(WeatherState {
                wind_speed_m_s: 18.0,
                ..Default::default()
            })
            .build();
        let mut out = Vec::new();
        actor_below.decide(&env_below, &mut out);
        assert!(
            out.is_empty(),
            "wind 18 m/s should NOT trigger at threshold 20 m/s, got {} signals",
            out.len()
        );
    }

    #[test]
    fn resolve_equipment_id_missing_name_sets_equipment_id_to_none() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::Manual,
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let id_by_name: HashMap<String, EquipmentId> = HashMap::new();
        actor.resolve_equipment_id(&id_by_name);
        assert!(
            actor.equipment_id.is_none(),
            "equipment_id must be None when target name is not in the registry"
        );
    }

    #[test]
    fn resolve_equipment_id_found_name_sets_equipment_id() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::Manual,
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut id_by_name = HashMap::new();
        id_by_name.insert("bat1".to_string(), EquipmentId(42));
        actor.resolve_equipment_id(&id_by_name);
        assert_eq!(
            actor.equipment_id,
            Some(EquipmentId(42)),
            "equipment_id must match the registry entry"
        );
    }

    // ── actual-vs-forecast PV regression tests ──

    #[test]
    fn bms_self_consumption_uses_actual_pv_not_forecast() {
        // pv_generation_kw=10.0 (forecast), actual_pv_kw=3.0 (observed).
        // Load=5.0 → deficit = 5.0 - 3.0 = 2.0 → should discharge.
        // With forecast alone (10.0) there'd be surplus=5.0 → would charge.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 10.0,
                actual_pv_kw: 3.0,
                base_load_kw: 5.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.7);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::SelfConsumption {
                enabled,
                solar_only_charging,
            } => {
                assert!(*enabled);
                assert!(!solar_only_charging);
            }
            other => panic!("expected SelfConsumption, got {other:?}"),
        }
        assert_eq!(actor.last_action(), "self_consumption:discharge");
    }

    #[test]
    fn bms_self_consumption_solar_only_disconnects_when_actual_pv_is_zero() {
        // pv_generation_kw=5.0 (forecast), actual_pv_kw=0.0 (actual).
        // solar_only_charging=true → should grid-disconnect because actual PV is zero.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: true,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 5.0,
                actual_pv_kw: 0.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::GridConnect { connected: false }
        ));
        assert_eq!(actor.last_action(), "grid_disconnect:solar_only");
    }

    #[test]
    fn bms_solar_only_export_uses_actual_pv() {
        // pv_generation_kw=10.0 (forecast), actual_pv_kw=2.0 (observed).
        // Load=4.0 → deficit = 4.0 - 2.0 = 2.0 → raw discharge = 2.0.
        // SolarOnly clamp: max = load + actual_pv = 4.0 + 2.0 = 6.0.
        // 2.0 < 6.0 → discharge passes clamp. With forecast PV alone
        // (10.0), there'd be surplus = 6.0 → would charge instead.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::SolarOnly,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 10.0,
                actual_pv_kw: 2.0,
                base_load_kw: 4.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.7);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        assert_eq!(out.len(), 1);
        match &out[0].signal {
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                // deficit=2.0 → raw discharge = 2.0, SolarOnly clamp load+actual=4+2=6.0 → 2.0
                assert!((*active_power_kw + 2.0).abs() < 1e-9);
            }
            other => panic!("expected PowerSetpoint, got {other:?}"),
        }
    }

    // ── adjust_for_pv re-evaluation tests ──

    #[test]
    fn adjust_for_pv_stale_surplus_actual_deficit_switches_to_discharge() {
        // Stale PV=4 kW, load=2 kW → surplus=+2 → decide() charges.
        // Actual PV=1 kW, load=2 kW → deficit=-1 → adjust_for_pv should discharge.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 4.0,
                actual_pv_kw: 4.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        // decide() with stale PV → charge (surplus=2)
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::SelfConsumption { enabled: true, .. }
        ));
        assert_eq!(actor.last_action(), "self_consumption:charge");

        // adjust_for_pv with actual PV=1 → deficit=-1 → discharge
        out.clear();
        actor.adjust_for_pv(1.0, &env, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::SelfConsumption { enabled: true, .. }
        ));
        assert_eq!(actor.last_action(), "self_consumption:discharge");
    }

    #[test]
    fn adjust_for_pv_stale_deficit_actual_surplus_switches_to_charge() {
        // Stale PV=1 kW, load=4 kW → deficit=-3 → decide() discharges.
        // Actual PV=6 kW, load=4 kW → surplus=+2 → adjust_for_pv should charge.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 1.0,
                actual_pv_kw: 1.0,
                base_load_kw: 4.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        // decide() with stale PV → discharge (deficit=-3)
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(actor.last_action(), "self_consumption:discharge");

        // adjust_for_pv with actual PV=6 → surplus=+2 → charge
        out.clear();
        actor.adjust_for_pv(6.0, &env, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::SelfConsumption { enabled: true, .. }
        ));
        assert_eq!(actor.last_action(), "self_consumption:charge");
    }

    #[test]
    fn adjust_for_pv_surplus_unchanged_no_re_emit() {
        // Stale PV=4 kW, load=2 kW → surplus=+2 → decide() charges.
        // Actual PV=4.1 kW, load=2 kW → surplus still positive → no re-emit needed.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 4.0,
                actual_pv_kw: 4.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(actor.last_action(), "self_consumption:charge");

        out.clear();
        actor.adjust_for_pv(4.1, &env, &mut out);
        assert!(
            out.is_empty(),
            "re-evaluation should not re-emit when surplus direction is unchanged"
        );
    }

    #[test]
    fn adjust_for_pv_zero_pv_high_load_idles_when_already_acting() {
        // Stale PV=4 kW, load=2 kW → decide() charges.
        // Actual PV=2 kW, load=2 kW → surplus=0, was_charging, deadband=0.0.
        // With hysteresis: surplus >= -deadband → stays charging (no idle).
        // To trigger idle, use surplus that crosses below the exit threshold.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 4.0,
                actual_pv_kw: 4.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(actor.last_action(), "self_consumption:charge");

        // Actual PV=2.0 kW, load=2 kW → surplus = 0.
        // With deadband=0.0 and was_charging: surplus >= 0.0 stays charging.
        out.clear();
        actor.adjust_for_pv(2.0, &env, &mut out);
        assert_eq!(
            actor.last_action(),
            "self_consumption:charge",
            "surplus=0 with deadband=0.0 stays charging (hysteresis)"
        );

        // Actual PV=1.0 kW, load=2 kW → surplus = -1.0.
        // Surplus < -deadband (0.0) → should switch to discharge.
        out.clear();
        actor.adjust_for_pv(1.0, &env, &mut out);
        assert!(!out.is_empty(), "should emit when switching to discharge");
        assert_eq!(actor.last_action(), "self_consumption:discharge");
    }

    #[test]
    fn adjust_for_pv_full_rated_pv_no_re_eval_for_charging() {
        // Stale PV=0, load=5 → deficit=-5 → decide() may idle or discharge.
        // Actual PV=10 kW (full-rated), load=5 → surplus=+5 → should charge.
        // Batter max_charge_kw=5, so PV=10 exceeds it but that's fine.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 0.0,
                actual_pv_kw: 0.0,
                base_load_kw: 5.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.3);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(actor.last_action(), "self_consumption:discharge");

        // adjust_for_pv with full-rated PV → surplus=+5 → charge
        out.clear();
        actor.adjust_for_pv(10.0, &env, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::SelfConsumption { enabled: true, .. }
        ));
        assert_eq!(actor.last_action(), "self_consumption:charge");
    }

    #[test]
    fn adjust_for_pv_solar_only_actual_pv_zero_disconnects() {
        // Stale PV=3 kW (solar_only), actual PV=0 → should disconnect grid.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: true,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 3.0,
                actual_pv_kw: 3.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(actor.last_action(), "self_consumption:charge");

        // Actual PV=0, solar_only → disconnect
        out.clear();
        actor.adjust_for_pv(0.0, &env, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::GridConnect { connected: false }
        ));
        assert_eq!(actor.last_action(), "grid_disconnect:solar_only");
    }

    #[test]
    fn adjust_for_pv_solar_only_actual_pv_restored_reconnects() {
        // Stale PV=0, solar_only → decide() disconnected grid.
        // Actual PV=5 → should reconnect grid.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: true,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 0.0,
                actual_pv_kw: 0.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(actor.last_action(), "grid_disconnect:solar_only");

        // Actual PV=5 → reconnect
        out.clear();
        actor.adjust_for_pv(5.0, &env, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::GridConnect { connected: true }
        ));
    }

    #[test]
    fn adjust_for_pv_manual_mode_no_op() {
        // Manual mode never emits in decide or adjust_for_pv.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::Manual,
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let env = TestEnvBuilder::new().build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert!(out.is_empty());

        out.clear();
        actor.adjust_for_pv(10.0, &env, &mut out);
        assert!(out.is_empty(), "Manual mode should not react to PV");
    }

    #[test]
    fn adjust_for_pv_telemetry_populated() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 4.0,
                actual_pv_kw: 4.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        let stale = actor
            .telemetry()
            .unwrap()
            .get("bms_pv_stale_kw")
            .unwrap_or(-1.0);
        assert!(
            (stale - 4.0).abs() < 1e-9,
            "bms_pv_stale_kw should be {stale}"
        );

        out.clear();
        actor.adjust_for_pv(7.0, &env, &mut out);
        let actual = actor
            .telemetry()
            .unwrap()
            .get("bms_pv_actual_kw")
            .unwrap_or(-1.0);
        assert!(
            (actual - 7.0).abs() < 1e-9,
            "bms_pv_actual_kw should be 7.0, got {actual}"
        );
    }

    #[test]
    fn adjust_for_pv_stale_zero_actual_surplus_starts_charging() {
        // Stale PV=0 (night), load=2 → decide() may idle or discharge.
        // Actual PV=5 → adjust_for_pv should initiate charge.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 0.0,
                actual_pv_kw: 0.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        // With PV=0 and load=2, deficit → discharge
        assert_eq!(actor.last_action(), "self_consumption:discharge");

        // Actual PV=5, load=2 → surplus=+3 → charge
        out.clear();
        actor.adjust_for_pv(5.0, &env, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::SelfConsumption { enabled: true, .. }
        ));
        assert_eq!(actor.last_action(), "self_consumption:charge");
    }

    #[test]
    fn adjust_for_pv_dr_inactive_re_evaluates_base_with_actual_pv() {
        // DemandResponse with a PV-dependent base (SelfConsumption).
        // When DR is inactive, the base mode should be re-evaluated with
        // actual-step PV — not re-read from the stale env.
        let prices: Vec<f64> = vec![0.10; 24];
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::DemandResponse {
                base_mode: Box::new(BmsMode::SelfConsumption {
                    min_soc: 0.1,
                    max_soc: 0.95,
                    solar_only_charging: false,
                    surplus_deadband_kw: 0.0,
                }),
                dr_discharge_rate: 0.8,
                min_soc_during_dr: 0.1,
                dr_deactivation_multiplier: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            Some(prices.into()),
            24,
        );

        // Stale PV=4, load=2 → surplus=2 → decide() charges via base SelfConsumption.
        let mut env = TestEnvBuilder::new()
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.10),
                ..Default::default()
            })
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 4.0,
                actual_pv_kw: 4.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::SelfConsumption { enabled: true, .. }
        ));
        assert_eq!(actor.last_action(), "self_consumption:charge");

        // Actual PV=0 → deficit=-2 → should switch to discharge.
        out.clear();
        actor.adjust_for_pv(0.0, &env, &mut out);
        assert_eq!(out.len(), 1);
        assert!(
            matches!(
                out[0].signal,
                ControlSignal::SelfConsumption { enabled: true, .. }
            ),
            "expected SelfConsumption discharge signal, got {:?}",
            out[0].signal
        );
        assert_eq!(actor.last_action(), "self_consumption:discharge");
    }

    #[test]
    fn adjust_for_pv_storm_watch_inactive_re_evaluates_base_with_actual_pv() {
        // StormWatch with a PV-dependent base (SelfConsumption).
        // When StormWatch is inactive (wind below threshold), the base mode
        // should be re-evaluated with actual-step PV.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::StormWatch {
                target_soc: 1.0,
                trigger: StormWatchTrigger::WeatherSignal {
                    wind_speed_threshold_m_s: 25.0,
                    wind_speed_deactivation_threshold_m_s: 0.0,
                },
                base_mode: Box::new(BmsMode::SelfConsumption {
                    min_soc: 0.1,
                    max_soc: 0.95,
                    solar_only_charging: false,
                    surplus_deadband_kw: 0.0,
                }),
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );

        // Wind below threshold → storm watch inactive → delegates to SelfConsumption.
        // Stale PV=4, load=2 → surplus=2 → decide() charges.
        let mut env = TestEnvBuilder::new()
            .with_weather(WeatherState {
                wind_speed_m_s: 5.0,
                ..Default::default()
            })
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 4.0,
                actual_pv_kw: 4.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0].signal,
            ControlSignal::SelfConsumption { enabled: true, .. }
        ));
        assert_eq!(actor.last_action(), "self_consumption:charge");

        // Actual PV=0 → deficit=-2 → should switch to discharge.
        out.clear();
        actor.adjust_for_pv(0.0, &env, &mut out);
        assert_eq!(out.len(), 1);
        assert!(
            matches!(
                out[0].signal,
                ControlSignal::SelfConsumption { enabled: true, .. }
            ),
            "expected SelfConsumption discharge signal, got {:?}",
            out[0].signal
        );
        assert_eq!(actor.last_action(), "self_consumption:discharge");
    }

    #[test]
    fn adjust_for_pv_respects_surplus_deadband_does_not_toggle_within_deadband() {
        // Regression: the re-evaluate path (`adjust_for_pv` → `reevaluate_self_consumption`)
        // must apply the same deadband hysteresis as the main `evaluate_mode` path.
        // Without this fix, actual PV that oscillates within the deadband zone causes
        // charge/discharge toggling on every step.
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.5,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        // decide() with surplus=0.0 → idle (0.0 not > 0.5 deadband)
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 2.0,
                actual_pv_kw: 2.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(actor.last_action(), "idle:self_consumption");

        // adjust_for_pv: surplus=+0.4 (within deadband) → must stay idle
        // (old code: surplus > 0.0 would have triggered charge — the bypass bug)
        out.clear();
        actor.adjust_for_pv(2.4, &env, &mut out);
        assert_eq!(
            actor.last_action(),
            "idle:self_consumption",
            "surplus +0.4 within deadband 0.5 must stay idle"
        );

        // adjust_for_pv: surplus=-0.4 (within deadband) → must stay idle
        out.clear();
        actor.adjust_for_pv(1.6, &env, &mut out);
        assert_eq!(
            actor.last_action(),
            "idle:self_consumption",
            "surplus -0.4 within deadband 0.5 must stay idle"
        );

        // adjust_for_pv: surplus=+1.0 (above deadband) → should charge
        out.clear();
        actor.adjust_for_pv(3.0, &env, &mut out);
        assert_eq!(
            actor.last_action(),
            "self_consumption:charge",
            "surplus +1.0 above deadband 0.5 should charge"
        );

        // adjust_for_pv: surplus drops to +0.2 (within hysteresis band but was_charging)
        // → should stay charging (surplus >= -deadband)
        out.clear();
        actor.adjust_for_pv(2.2, &env, &mut out);
        assert_eq!(
            actor.last_action(),
            "self_consumption:charge",
            "was_charging should stay charging (surplus +0.2 >= -deadband -0.5)"
        );

        // adjust_for_pv: surplus=-0.6 (below -deadband) → switch to discharge
        out.clear();
        actor.adjust_for_pv(1.4, &env, &mut out);
        assert_eq!(
            actor.last_action(),
            "self_consumption:discharge",
            "surplus -0.6 below -deadband -0.5 should discharge"
        );
    }

    #[test]
    fn bms_action_code_correct_for_all_reachable_actions() {
        // Code 0: idle
        assert_eq!(bms_action_code("idle"), 0.0);
        assert_eq!(bms_action_code("idle:no_soc"), 0.0);
        assert_eq!(bms_action_code("idle:self_consumption"), 0.0);
        assert_eq!(bms_action_code("idle:backup_at_target"), 0.0);
        assert_eq!(bms_action_code("idle:tou"), 0.0);
        assert_eq!(bms_action_code("idle:no_window"), 0.0);
        assert_eq!(bms_action_code("scheduled:idle"), 0.0);

        // Code 1: charge
        assert_eq!(bms_action_code("self_consumption:charge"), 1.0);
        assert_eq!(bms_action_code("tou:charge"), 1.0);
        assert_eq!(bms_action_code("scheduled:charge"), 1.0);
        assert_eq!(bms_action_code("backup:charge"), 1.0);

        // Code 2: discharge (must return 2, not 1 — substring bug verification)
        assert_eq!(bms_action_code("self_consumption:discharge"), 2.0);
        assert_eq!(bms_action_code("tou:discharge"), 2.0);
        assert_eq!(bms_action_code("scheduled:discharge"), 2.0);
        assert_eq!(bms_action_code("dr:discharge"), 2.0);

        // Code 3: grid_disconnect
        assert_eq!(bms_action_code("grid_disconnect:solar_only"), 3.0);
        assert_eq!(bms_action_code("grid_disconnect:tou_solar_only"), 3.0);
        assert_eq!(bms_action_code("grid_disconnect:backup_no_pv"), 3.0);

        // Code 4: reconnect
        assert_eq!(bms_action_code("self_consumption:grid_reconnected"), 4.0);
        assert_eq!(bms_action_code("backup:grid_reconnected"), 4.0);
        assert_eq!(bms_action_code("tou:grid_reconnected"), 4.0);

        // Code 5: storm_watch
        assert_eq!(bms_action_code("storm_watch:active"), 5.0);

        // Code -1: unknown
        assert_eq!(bms_action_code("dr:soc_too_low"), -1.0);
        assert_eq!(bms_action_code("scheduled:hold"), -1.0);
    }

    #[test]
    fn save_state_load_state_round_trip_preserves_to_thresholds() {
        let prices: Vec<f64> = (0..24).map(|h| if h < 8 { 0.05 } else { 0.30 }).collect();
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.2,
                charge_threshold_percentile: 0.25,
                discharge_threshold_percentile: 0.75,
                solar_only_charging: false,
                price_deadband: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            Some(prices.into()),
            24,
        );
        // Force state to a known non-default value
        actor.daily_avg_price = 0.15;
        actor.charge_price_threshold = 0.05;
        actor.discharge_price_threshold = 0.25;
        actor.current_day_ordinal0 = 42;
        actor.last_action = "idle:no_soc".to_string();

        let blob = actor.save_state().expect("save_state should succeed");
        assert!(
            !blob.is_empty(),
            "stateful actor must produce non-empty blob"
        );

        let mut restored = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.2,
                charge_threshold_percentile: 0.25,
                discharge_threshold_percentile: 0.75,
                solar_only_charging: false,
                price_deadband: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        restored
            .load_state(&blob)
            .expect("load_state should succeed");

        assert_eq!(restored.current_day_ordinal0, 42);
        assert!((restored.daily_avg_price - 0.15).abs() < 1e-12);
        assert!((restored.charge_price_threshold - 0.05).abs() < 1e-12);
        assert!((restored.discharge_price_threshold - 0.25).abs() < 1e-12);
        assert_eq!(restored.last_action, "idle:no_soc");
    }

    // ── hysteresis regression tests ──

    #[test]
    fn self_consumption_hysteresis_stays_charging_within_deadband() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.5,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 5.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        // Step 1: surplus = +3.0 > +0.5 deadband → enter charge
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(actor.last_action(), "self_consumption:charge");

        // Step 2: surplus drops to +0.2, still within deadband (≥ -0.5)
        // Should stay charging because we were charging.
        let mut env2 = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 3.0,
                base_load_kw: 2.8,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env2, "bat1", 0.5);
        out.clear();
        actor.decide(&env2, &mut out);
        assert_eq!(
            actor.last_action(),
            "self_consumption:charge",
            "should stay charging within deadband"
        );

        // Step 3: surplus drops below -deadband → should exit charge
        let mut env3 = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 1.0,
                base_load_kw: 3.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env3, "bat1", 0.5);
        out.clear();
        actor.decide(&env3, &mut out);
        assert_eq!(
            actor.last_action(),
            "self_consumption:discharge",
            "should switch to discharge when surplus < -deadband"
        );
    }

    #[test]
    fn tou_hysteresis_does_not_toggle_on_price_noise() {
        let prices: Vec<f64> = vec![
            0.10, 0.30, 0.10, 0.30, 0.10, 0.30, 0.10, 0.30, 0.10, 0.30, 0.10, 0.30, 0.10, 0.30,
            0.10, 0.30, 0.10, 0.30, 0.10, 0.30, 0.10, 0.30, 0.10, 0.30,
        ];
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::TimeOfUseOptimization {
                reserve_soc: 0.2,
                charge_threshold_percentile: 0.25,
                discharge_threshold_percentile: 0.75,
                solar_only_charging: false,
                price_deadband: 0.05,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            Some(prices.into()),
            24,
        );
        let mut env = TestEnvBuilder::new()
            .hour(14)
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.30),
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(actor.last_action(), "tou:discharge");

        // Price fluctuates to 0.28 (still >= 0.25 - 0.05 = 0.20)
        // Should stay discharging.
        let mut env2 = TestEnvBuilder::new()
            .hour(14)
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.28),
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env2, "bat1", 0.5);
        out.clear();
        actor.decide(&env2, &mut out);
        assert_eq!(
            actor.last_action(),
            "tou:discharge",
            "should stay discharging within price deadband"
        );
    }

    #[test]
    fn storm_watch_schmitt_trigger_activates_and_deactivates_at_separate_thresholds() {
        let make_actor = || {
            BatteryManagementActor::new(
                "bat1",
                BmsMode::StormWatch {
                    target_soc: 1.0,
                    trigger: StormWatchTrigger::WeatherSignal {
                        wind_speed_threshold_m_s: 20.0,
                        wind_speed_deactivation_threshold_m_s: 15.0,
                    },
                    base_mode: Box::new(BmsMode::Manual),
                },
                GridExportRule::Unrestricted,
                5.0,
                5.0,
                None,
                24,
            )
        };

        // Wind at 22 m/s: above activation threshold → active
        let mut actor = make_actor();
        let env1 = TestEnvBuilder::new()
            .with_weather(WeatherState {
                wind_speed_m_s: 22.0,
                ..Default::default()
            })
            .build();
        let mut out = Vec::new();
        actor.decide(&env1, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].signal, ControlSignal::SOCTarget { .. }));

        // Wind drops to 17 m/s: below activation (20) but above deactivation (15)
        // Should stay active (Schmitt trigger hysteresis).
        let env2 = TestEnvBuilder::new()
            .with_weather(WeatherState {
                wind_speed_m_s: 17.0,
                ..Default::default()
            })
            .build();
        out.clear();
        actor.decide(&env2, &mut out);
        assert_eq!(
            out.len(),
            1,
            "should stay active above deactivation threshold"
        );

        // Wind drops to 12 m/s: below deactivation threshold → deactivate
        let env3 = TestEnvBuilder::new()
            .with_weather(WeatherState {
                wind_speed_m_s: 12.0,
                ..Default::default()
            })
            .build();
        out.clear();
        actor.decide(&env3, &mut out);
        assert!(
            out.is_empty(),
            "should deactivate below deactivation threshold"
        );
    }

    #[test]
    fn backup_reserve_hysteresis_starts_below_target_minus_deadband_stops_above_target_plus_deadband()
     {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::BackupReserve {
                target_soc: 0.8,
                charge_from_grid: true,
                charge_rate_fraction: 1.0,
                soc_deadband: 0.05,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );

        // SOC at 0.73 (0.8 - 0.05 = 0.75): below target - deadband → charge
        let mut env = TestEnvBuilder::new().build();
        set_soc(&mut actor, &mut env, "bat1", 0.73);
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        assert_eq!(actor.last_action(), "backup:charge");

        // SOC rises to 0.78 (still below target + deadband = 0.85) → keep charging
        let mut env2 = TestEnvBuilder::new().build();
        set_soc(&mut actor, &mut env2, "bat1", 0.78);
        out.clear();
        actor.decide(&env2, &mut out);
        assert_eq!(
            actor.last_action(),
            "backup:charge",
            "should stay charging while soc is between target-deadband and target+deadband"
        );

        // SOC rises to 0.86 (> target + deadband = 0.85) → stop charging
        let mut env3 = TestEnvBuilder::new().build();
        set_soc(&mut actor, &mut env3, "bat1", 0.86);
        out.clear();
        actor.decide(&env3, &mut out);
        assert_eq!(actor.last_action(), "idle:backup_at_target");
    }

    #[test]
    fn bms_toggled_telemetry_set_on_action_change() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::SelfConsumption {
                min_soc: 0.1,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        let mut env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 5.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        set_soc(&mut actor, &mut env, "bat1", 0.5);

        // First step: action changes from "" to "self_consumption:charge" → toggled
        let mut out = Vec::new();
        actor.decide(&env, &mut out);
        let toggled_after_first = actor
            .telemetry()
            .unwrap()
            .get("bms_toggled")
            .unwrap_or(-1.0);
        assert!(
            (toggled_after_first - 1.0).abs() < 1e-9,
            "first step should be toggled"
        );

        // Second step: same surplus → same action → not toggled
        out.clear();
        actor.decide(&env, &mut out);
        let toggled_after_second = actor
            .telemetry()
            .unwrap()
            .get("bms_toggled")
            .unwrap_or(-1.0);
        assert!(
            (toggled_after_second - 0.0).abs() < 1e-9,
            "same action should not be toggled"
        );
    }

    #[test]
    fn bms_save_load_preserves_storm_watch_and_dr_state() {
        let mut actor = BatteryManagementActor::new(
            "bat1",
            BmsMode::StormWatch {
                target_soc: 1.0,
                trigger: StormWatchTrigger::WeatherSignal {
                    wind_speed_threshold_m_s: 20.0,
                    wind_speed_deactivation_threshold_m_s: 0.0,
                },
                base_mode: Box::new(BmsMode::Manual),
            },
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        actor.storm_watch_active = true;
        actor.dr_active = true;

        let blob = actor.save_state().expect("save_state should succeed");
        let mut restored = BatteryManagementActor::new(
            "bat1",
            BmsMode::Manual,
            GridExportRule::Unrestricted,
            5.0,
            5.0,
            None,
            24,
        );
        restored
            .load_state(&blob)
            .expect("load_state should succeed");
        assert!(restored.storm_watch_active);
        assert!(restored.dr_active);
    }
}
