use hares_types::{DRLevel, EvConnectionState, OperatingMode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct EvCheckpoint {
    pub(super) soc: f64,
    pub(super) connection_state: EvConnectionState,
    pub(super) away_charger_power_w: f64,
    pub(super) active_power_w: f64,
    pub(super) power_limit_w: Option<f64>,
    pub(super) power_setpoint_w: Option<f64>,
    pub(super) power_setpoint_min_soc: Option<f64>,
    pub(super) power_setpoint_max_soc: Option<f64>,
    pub(super) dr_level: DRLevel,
    pub(super) dr_duration_remaining_s: Option<f64>,
    pub(super) soc_target: Option<f64>,
    pub(super) soc_target_min: Option<f64>,
    pub(super) soc_target_max: Option<f64>,
    pub(super) battery_temp_c: f64,
    pub(super) ready_soc: f64,
    pub(super) ready_by_hour: Option<f64>,
    pub(super) ready_by_soc: Option<f64>,
    pub(super) v2l_enabled: bool,
    pub(super) v2l_soc_reserve: f64,
    pub(super) v2l_max_discharge_w: f64,
    pub(super) v2g_enabled: bool,
    pub(super) v2g_soc_reserve: f64,
    pub(super) v2g_max_discharge_w: f64,
    pub(super) degradation: crate::battery::degradation::DegradationState,
    pub(super) rainflow: crate::battery::degradation::RainflowCounter,
    pub(super) last_daily_update_day: i32,
    /// Reactive-power override [VAR]. `None` = no override (baseline
    /// power-factor path); `Some(0.0)` is a commanded zero.
    pub(super) q_setpoint_var: Option<f64>,
    pub(super) power_factor: f64,
    /// Cumulative drive energy [J] dispatched but not deliverable by the
    /// pack; see `Ev::drive_shortfall_j`.
    pub(super) drive_shortfall_j: f64,
    /// Drive energy [J] dispatched since the last step, awaiting its
    /// thermal application (I²R at the equivalent discharge current) — see
    /// `Ev::pending_drive_j`.
    pub(super) pending_drive_j: f64,
    /// Reactive power emitted on the last step [VAR] - same signed value
    /// on port, CoreOutput, and telemetry, keyed on the inverter leg (the
    /// charge/export conversion only; the DC-fed pack heater produces no
    /// vars). Checkpointed verbatim so the restore publishes the
    /// step-published flow: the inverter-leg basis is not derivable at
    /// restore — the heater draw that splits the port total into leg +
    /// heater AC-equivalent is transient step state, not checkpointed.
    pub(super) reactive_power_var: f64,
    /// Actual post-cap heater draw on the last step [W] — pack-side DC,
    /// funding the port total's heater AC-equivalent share. Checkpointed
    /// verbatim for the same reason the reactive flow is: its basis (the
    /// charge-leg/heater split of the port total) is transient step
    /// state, not derivable at restore, and zeroing it pairs the
    /// `Heating` mode — which `classify_mode` only produces with a nonzero
    /// draw — with a zero heater column, a state no live step publishes.
    /// `heater_active` is derived from it (draw > 0), not checkpointed.
    pub(super) heater_draw_w: f64,
    /// V2L dispatch state on the last step: true when a discharge dispatch
    /// was latched (negative setpoint with V2L/V2G enabled) — the step keys
    /// it on dispatch **presence**, not the computed export, so it stays
    /// true at the reserve floor and under export DR while the export
    /// itself computes to exactly zero. Checkpointed verbatim because no
    /// checkpointed basis reproduces it: the mode is `Off` in those corners
    /// (zero pack net rate), so deriving it from `last_mode` diverges from
    /// the saved state exactly where `island_source_available` reads it.
    pub(super) v2l_active: bool,
    /// V2L export magnitude on the last step [kW] — zero exactly when the
    /// dispatch computed no export (floor-held or DR-zeroed). Checkpointed
    /// verbatim for the same reason as `v2l_active`: the export is the port
    /// total's discharge sign case, but the sign alone cannot distinguish
    /// a dispatched zero export from an idle connection, so the zero is
    /// publishable information only the step's dispatch state carries.
    pub(super) v2l_power_w: f64,
    /// Actual charge intake of the away session on the last step [kW] —
    /// the intake after taper, never the rating, never overloaded with
    /// heater draw; the charge-side counterpart of `v2l_power_w` and
    /// the documented way away charging is observed
    /// (`AWAY_CHARGE_POWER_KW`). Checkpointed verbatim because its basis
    /// (the away charge leg) is not derivable at restore: the away state
    /// publishes zero port power, so no checkpointed column reconstructs
    /// the intake, and leaving it zero misreports an active away session
    /// until the next step.
    pub(super) away_charge_actual_w: f64,
    /// Operating mode computed on the last step; `update_control` reports
    /// it between steps (mode is a step outcome, keyed on the pack-side
    /// net charge rate and the export leg — see `classify_mode`).
    pub(super) last_mode: OperatingMode,
    /// The IEEE 1547 layer's run state (the per-point trip timers, the
    /// trip latch, the enter-service accumulation, the frequency-droop
    /// anchor) and the unconfigured layer's excursion count and warn
    /// latch. The state is the layer's own across steps: no step's inputs
    /// re-derive a part-spent window, a latched reason or a held anchor,
    /// so it rides the blob verbatim.
    pub(super) der1547_state: crate::der1547::Der1547State,
    pub(super) der1547_unconfigured_excursions: u32,
    pub(super) der1547_unconfigured_warned: bool,
    pub(super) der1547_unconfigured_in_excursion: bool,
}
