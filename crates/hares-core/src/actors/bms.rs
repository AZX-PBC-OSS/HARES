//! Battery Management System actor -- dispatches charge/discharge control
//! signals based on the configured `BmsMode`, PV production, grid prices,
//! and battery SOC.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Datelike, Timelike};
use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_types::{
    BmsAction, BmsMode, BmsScheduleWindow, ControlSignal, EnvironmentState, EquipmentId,
    GridExportRule, StormWatchTrigger, Telemetry,
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

    #[allow(clippy::too_many_arguments)]
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
        let mut telemetry = Telemetry::with_capacity(4);
        telemetry.insert("bms_action", -1.0);
        telemetry.insert("soc", f64::NAN);
        telemetry.insert("pv_kw", 0.0);
        telemetry.insert("load_kw", 0.0);
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
        self.equipment_id = equipment_id_by_name.get(battery_name).copied();
    }

    fn read_soc(&self, env: &EnvironmentState) -> Option<f64> {
        let id = self.equipment_id?;
        env.equipment_core
            .get(&id)
            .and_then(|co| co.state.soc)
            .map(|s| s.get())
    }

    fn emit(&self, signal: ControlSignal, out: &mut Vec<DispatchRequest>) {
        out.push(DispatchRequest {
            target: self.dispatch_target.clone(),
            signal,
            priority: PriorityTier::Schedule,
        });
    }

    /// Clamp discharge power (negative `active_power_kw`) based on grid export rule.
    ///
    /// - `Unrestricted`: no clamping.
    /// - `Disabled`: clamp discharge so net export is zero (battery only offsets home load).
    /// - `SolarOnly`: clamp discharge so net export does not exceed PV generation.
    fn clamp_discharge_for_export(&self, discharge_kw: f64, env: &EnvironmentState) -> f64 {
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
                let max_allowed =
                    env.electrical.base_load_kw.max(0.0) + env.electrical.pv_generation_kw.max(0.0);
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
            } => {
                let Some(soc) = self.read_soc(env) else {
                    self.last_action = "idle:no_soc".into();
                    return;
                };
                let pv = env.electrical.pv_generation_kw;
                let load = env.electrical.base_load_kw;
                let surplus = pv - load;

                if *solar_only_charging && pv <= 0.0 {
                    self.emit(ControlSignal::GridConnect { connected: false }, out);
                    self.last_action = "grid_disconnect:solar_only".into();
                    return;
                }

                if surplus > 0.0 && soc < *max_soc {
                    // When GridExportRule::Disabled, force solar-only charging
                    // to prevent grid-to-battery import that would increase
                    // net grid consumption.
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
                } else if surplus < 0.0 && soc > *min_soc {
                    // The battery equipment's SelfConsumption handler already caps
                    // discharge to net_load_kw, so Unrestricted is inherently safe.
                    // For Disabled/SolarOnly we emit an explicit clamped PowerSetpoint
                    // as defense-in-depth (the actor is the authoritative export policy
                    // layer, not the equipment).
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
                            let clamped = self.clamp_discharge_for_export(raw_discharge, env);
                            self.emit(
                                ControlSignal::PowerSetpoint {
                                    active_power_kw: -clamped,
                                    reactive_power_kvar: None,
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

                if price <= self.charge_price_threshold && soc < (1.0 - reserve_soc) {
                    if *solar_only_charging && env.electrical.pv_generation_kw <= 0.0 {
                        self.emit(ControlSignal::GridConnect { connected: false }, out);
                        self.last_action = "grid_disconnect:tou_solar_only".into();
                        return;
                    }
                    self.emit(
                        ControlSignal::PowerSetpoint {
                            active_power_kw: self.max_charge_kw,
                            reactive_power_kvar: None,
                        },
                        out,
                    );
                    self.last_action = "tou:charge".into();
                } else if price >= self.discharge_price_threshold && soc > *reserve_soc {
                    let clamped = self.clamp_discharge_for_export(self.max_discharge_kw, env);
                    self.emit(
                        ControlSignal::PowerSetpoint {
                            active_power_kw: -clamped,
                            reactive_power_kvar: None,
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
            } => {
                let Some(soc) = self.read_soc(env) else {
                    self.last_action = "idle:no_soc".into();
                    return;
                };
                if soc < *target_soc {
                    if !charge_from_grid && env.electrical.pv_generation_kw <= 0.0 {
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
                    self.last_action = "backup:charging".into();
                } else {
                    self.last_action = "idle:backup_at_target".into();
                }
            }

            BmsMode::DemandResponse {
                base_mode,
                dr_discharge_rate,
                min_soc_during_dr,
            } => {
                let price = env.price_signal.electricity_price.unwrap_or(0.0);
                let dr_active = self.is_dr_active(env, price);

                if dr_active {
                    let Some(soc) = self.read_soc(env) else {
                        self.last_action = "idle:no_soc".into();
                        return;
                    };
                    if soc > *min_soc_during_dr {
                        let raw = dr_discharge_rate * self.max_discharge_kw;
                        let clamped = self.clamp_discharge_for_export(raw, env);
                        self.emit(
                            ControlSignal::PowerSetpoint {
                                active_power_kw: -clamped,
                                reactive_power_kvar: None,
                            },
                            out,
                        );
                        self.last_action = "dr:discharging".into();
                    } else {
                        self.last_action = "dr:soc_too_low".into();
                    }
                } else {
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
                                },
                                out,
                            );
                            self.last_action = "scheduled:charge".into();
                        }
                        BmsAction::Discharge { rate_fraction } => {
                            let raw = rate_fraction * self.max_discharge_kw;
                            let clamped = self.clamp_discharge_for_export(raw, env);
                            self.emit(
                                ControlSignal::PowerSetpoint {
                                    active_power_kw: -clamped,
                                    reactive_power_kvar: None,
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
                    } => env.weather.wind_speed_m_s > *wind_speed_threshold_m_s,
                };

                if active {
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

    fn is_dr_active(&mut self, env: &EnvironmentState, current_price: f64) -> bool {
        self.ensure_daily_prices(env);

        if self.price_schedule.is_none() {
            return false;
        }

        if self.daily_avg_price <= 0.0 {
            return false;
        }

        current_price > 2.0 * self.daily_avg_price
    }
}

impl Actor for BatteryManagementActor {
    fn name(&self) -> &str {
        &self.name
    }

    fn telemetry(&self) -> Option<&Telemetry> {
        Some(&self.telemetry)
    }

    fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        let mode = std::mem::take(&mut self.bms_mode);
        self.evaluate_mode(&mode, env, out);
        self.bms_mode = mode;

        // Populate telemetry: bms_action uses a numeric code from last_action.
        self.telemetry
            .set("bms_action", bms_action_code(&self.last_action));
        self.telemetry
            .set("soc", self.read_soc(env).unwrap_or(f64::NAN));
        self.telemetry.set("pv_kw", env.electrical.pv_generation_kw);
        self.telemetry.set("load_kw", env.electrical.base_load_kw);
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
/// 0=idle, 1=charge, 2=discharge, 3=grid_disconnect, -1=unknown.
fn bms_action_code(action: &str) -> f64 {
    if action.contains("charge") {
        1.0
    } else if action.contains("discharge") {
        2.0
    } else if action.contains("grid_disconnect") {
        3.0
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
                }),
                dr_discharge_rate: 0.8,
                min_soc_during_dr: 0.1,
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
                },
                base_mode: Box::new(BmsMode::SelfConsumption {
                    min_soc: 0.1,
                    max_soc: 0.95,
                    solar_only_charging: false,
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
                solar_only_charging: false, // user says allow grid charging
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
}
