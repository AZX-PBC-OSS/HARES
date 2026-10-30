//! The IEEE 1547-2018 DER response layer: trip tables, the frequency-droop
//! (frequency-Watt) function, and the return-to-service delay.
//!
//! The layer is an explicit configuration surface: a DER (PV, battery
//! inverter, EV export) without a `der_1547` settings block behaves as the
//! pre-1547 model did (only the de-energized bus stops a grid-following
//! inverter) and warns once on the first grid excursion it declines to
//! govern. A DER with a block responds to the bus voltage and the grid
//! frequency per the standard's default shall-trip and enter-service
//! settings for its assigned abnormal-performance category, with every
//! parameter operator-settable within the surface.
//!
//! Discrete-time semantics: the standard's clearing times are continuous.
//! Each trip point is an on-delay timer (EPRI OpenDER `ConditionalDelay`,
//! Eq. 3.12.4-2..4): while the point's condition holds, the timer
//! integrates the timestep and the point trips when the accumulated time
//! reaches the clearing time. The clearing time is a maximum, so a step
//! whose accumulated time reaches it is a tripped step: with the common
//! 900 s timestep every trip-band excursion is a same-step trip, and at
//! fine timesteps the ride-through window is honored step by step. The
//! accumulation is per disturbance: a step the point's condition does not
//! hold resets its timer, and the trip spends it. The
//! return-to-service delay integrates the same way off the bus voltage and
//! frequency inside the enter-service windows.
//!
//! Each point's timer is its own: a step in two bands accumulates both, and
//! only the point that reaches its window trips the inverter, which spends
//! every timer. When several points reach their windows on the same step,
//! the trip reason is the first in the tables' fixed order (the deeper
//! point before the shallower within a class, the voltage points before the
//! frequency points), and the reported ride-through window is that point's
//! own: the standard sets each point against its own disturbance and does
//! not rank them.

use serde::{Deserialize, Serialize};

use hares_physics::constants::{
    IEEE1547_CAT1_UV1_TRIP_T_S, IEEE1547_CAT2_UV1_TRIP_T_S, IEEE1547_CAT3_OV1_TRIP_T_S,
    IEEE1547_CAT3_UV1_TRIP_T_S, IEEE1547_CAT3_UV1_TRIP_V_PU, IEEE1547_CAT3_UV2_TRIP_T_S,
    IEEE1547_CAT3_UV2_TRIP_V_PU, IEEE1547_CAT12_OV1_TRIP_T_S, IEEE1547_CAT12_UV1_TRIP_V_PU,
    IEEE1547_CAT12_UV2_TRIP_T_S, IEEE1547_CAT12_UV2_TRIP_V_PU, IEEE1547_ES_DELAY_S,
    IEEE1547_ES_F_HIGH_HZ, IEEE1547_ES_F_LOW_HZ, IEEE1547_ES_V_HIGH_PU, IEEE1547_ES_V_LOW_PU,
    IEEE1547_FW_DEADBAND_OF_HZ, IEEE1547_FW_DEADBAND_UF_HZ, IEEE1547_FW_SLOPE_OF,
    IEEE1547_FW_SLOPE_UF, IEEE1547_NOMINAL_FREQ_HZ, IEEE1547_OF1_TRIP_F_HZ, IEEE1547_OF1_TRIP_T_S,
    IEEE1547_OF2_TRIP_F_HZ, IEEE1547_OF2_TRIP_T_S, IEEE1547_OV1_TRIP_V_PU, IEEE1547_OV2_TRIP_T_S,
    IEEE1547_OV2_TRIP_V_PU, IEEE1547_UF1_TRIP_F_HZ, IEEE1547_UF1_TRIP_T_S, IEEE1547_UF2_TRIP_F_HZ,
    IEEE1547_UF2_TRIP_T_S,
};

use crate::HaresError;

/// Trip-condition codes published in `der1547_trip_reason` telemetry.
pub mod trip_reason {
    pub const NONE: u8 = 0;
    pub const UV2: u8 = 1;
    pub const UV1: u8 = 2;
    pub const OV1: u8 = 3;
    pub const OV2: u8 = 4;
    pub const UF2: u8 = 5;
    pub const UF1: u8 = 6;
    pub const OF1: u8 = 7;
    pub const OF2: u8 = 8;

    /// The label each telemetry code resolves to, in code order
    /// (index = the numeric code).
    pub const LABELS: [&str; 9] = [
        "none", "uv2", "uv1", "ov1", "ov2", "uf2", "uf1", "of1", "of2",
    ];
}

/// The standard's abnormal-performance category a DER's default settings
/// come from. The voltage shall-trip defaults differ by category; the
/// frequency trip defaults, the frequency-droop parameters and the
/// enter-service criteria are identical across categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Der1547Category {
    #[serde(rename = "I")]
    CatI,
    #[serde(rename = "II")]
    CatII,
    #[serde(rename = "III")]
    CatIII,
}

/// One shall-trip point: the condition's threshold and the maximum
/// clearing time the standard grants (the ride-through window the default
/// setting gives the disturbance).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TripPoint {
    pub threshold: f64,
    pub clearing_s: f64,
}

/// The resolved IEEE 1547 response tables: the category's defaults with
/// any operator overrides applied. Built by
/// [`Der1547Settings::resolve_tables`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Der1547Tables {
    /// Under-voltage point 1: trip when V < `uv1_v` sustained `uv1_t` s
    /// (the sustained-sag ride-through window).
    pub uv1: TripPoint,
    /// Under-voltage point 2: trip when V < `uv2_v` sustained `uv2_t` s
    /// (the deep-sag window).
    pub uv2: TripPoint,
    /// Over-voltage point 1: trip when V > `ov1_v` sustained `ov1_t` s.
    pub ov1: TripPoint,
    /// Over-voltage point 2: trip when V > `ov2_v` sustained `ov2_t` s.
    pub ov2: TripPoint,
    /// Under-frequency point 1: trip when f < `uf1_hz` sustained `uf1_t` s.
    pub uf1: TripPoint,
    /// Under-frequency point 2: trip when f < `uf2_hz` sustained `uf2_t` s.
    pub uf2: TripPoint,
    /// Over-frequency point 1: trip when f > `of1_hz` sustained `of1_t` s.
    pub of1: TripPoint,
    /// Over-frequency point 2: trip when f > `of2_hz` sustained `of2_t` s.
    pub of2: TripPoint,
    /// Frequency-droop deadband, over-frequency side, in Hz: no power
    /// adjustment while f <= nominal + `fw_db_of`.
    pub fw_db_of: f64,
    /// Frequency-droop deadband, under-frequency side, in Hz.
    pub fw_db_uf: f64,
    /// Frequency-droop slope, over-frequency side [pu power per pu
    /// frequency beyond the deadband edge].
    pub fw_k_of: f64,
    /// Frequency-droop slope, under-frequency side.
    pub fw_k_uf: f64,
    /// Enter-service voltage window, in per unit: a tripped inverter accumulates
    /// return time only while `es_v_low <= V <= es_v_high`.
    pub es_v_low: f64,
    pub es_v_high: f64,
    /// Enter-service frequency window, in Hz.
    pub es_f_low: f64,
    pub es_f_high: f64,
    /// Enter-service minimum delay, in seconds: the inverter re-energizes on the
    /// first step whose accumulated in-window time reaches it.
    pub es_delay_s: f64,
}

/// The operator-settable IEEE 1547 surface for one DER. Every field names
/// the standard's parameter it overrides; a field left `None` takes the
/// assigned category's default. Values are validated at resolution
/// ([`Der1547Settings::resolve_tables`]): an out-of-range or non-finite
/// override is a configuration error naming the parameter, never a silent
/// clamp.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Der1547Overrides {
    pub uv1_v: Option<f64>,
    pub uv1_s: Option<f64>,
    pub uv2_v: Option<f64>,
    pub uv2_s: Option<f64>,
    pub ov1_v: Option<f64>,
    pub ov1_s: Option<f64>,
    pub ov2_v: Option<f64>,
    pub ov2_s: Option<f64>,
    pub uf1_hz: Option<f64>,
    pub uf1_s: Option<f64>,
    pub uf2_hz: Option<f64>,
    pub uf2_s: Option<f64>,
    pub of1_hz: Option<f64>,
    pub of1_s: Option<f64>,
    pub of2_hz: Option<f64>,
    pub of2_s: Option<f64>,
    pub fw_db_of_hz: Option<f64>,
    pub fw_db_uf_hz: Option<f64>,
    pub fw_k_of: Option<f64>,
    pub fw_k_uf: Option<f64>,
    pub es_v_low_pu: Option<f64>,
    pub es_v_high_pu: Option<f64>,
    pub es_f_low_hz: Option<f64>,
    pub es_f_high_hz: Option<f64>,
    pub es_delay_s: Option<f64>,
}

/// A DER's IEEE 1547 configuration: the assigned abnormal-performance
/// category plus optional parameter overrides. Stored on the equipment
/// config (`der_1547`); `None` means the layer is not configured and the
/// DER behaves as the pre-1547 model did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Der1547Settings {
    pub category: Der1547Category,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<Der1547Overrides>,
}

impl Der1547Settings {
    /// The tables this settings block resolves to: the category's default
    /// shall-trip, frequency-droop and enter-service settings with the
    /// operator's overrides applied on top.
    ///
    /// # Errors
    ///
    /// `HaresError::Equipment` naming the parameter when an override is
    /// non-finite or outside the value's physical range (a voltage
    /// threshold must be positive; a clearing time or delay must be
    /// non-negative; a droop slope must be positive; the enter-service
    /// window's low bound must not exceed its high bound).
    pub fn resolve_tables(&self, owner: &str) -> crate::Result<Der1547Tables> {
        let o = self.overrides.as_ref();
        let pick = |override_value: Option<f64>, default: f64| override_value.unwrap_or(default);

        let (uv1_v_d, uv1_t_d, uv2_v_d, uv2_t_d, ov1_t_d) = match self.category {
            Der1547Category::CatI => (
                IEEE1547_CAT12_UV1_TRIP_V_PU,
                IEEE1547_CAT1_UV1_TRIP_T_S,
                IEEE1547_CAT12_UV2_TRIP_V_PU,
                IEEE1547_CAT12_UV2_TRIP_T_S,
                IEEE1547_CAT12_OV1_TRIP_T_S,
            ),
            Der1547Category::CatII => (
                IEEE1547_CAT12_UV1_TRIP_V_PU,
                IEEE1547_CAT2_UV1_TRIP_T_S,
                IEEE1547_CAT12_UV2_TRIP_V_PU,
                IEEE1547_CAT12_UV2_TRIP_T_S,
                IEEE1547_CAT12_OV1_TRIP_T_S,
            ),
            Der1547Category::CatIII => (
                IEEE1547_CAT3_UV1_TRIP_V_PU,
                IEEE1547_CAT3_UV1_TRIP_T_S,
                IEEE1547_CAT3_UV2_TRIP_V_PU,
                IEEE1547_CAT3_UV2_TRIP_T_S,
                IEEE1547_CAT3_OV1_TRIP_T_S,
            ),
        };

        let tables = Der1547Tables {
            uv1: TripPoint {
                threshold: pick(o.and_then(|x| x.uv1_v), uv1_v_d),
                clearing_s: pick(o.and_then(|x| x.uv1_s), uv1_t_d),
            },
            uv2: TripPoint {
                threshold: pick(o.and_then(|x| x.uv2_v), uv2_v_d),
                clearing_s: pick(o.and_then(|x| x.uv2_s), uv2_t_d),
            },
            ov1: TripPoint {
                threshold: pick(o.and_then(|x| x.ov1_v), IEEE1547_OV1_TRIP_V_PU),
                clearing_s: pick(o.and_then(|x| x.ov1_s), ov1_t_d),
            },
            ov2: TripPoint {
                threshold: pick(o.and_then(|x| x.ov2_v), IEEE1547_OV2_TRIP_V_PU),
                clearing_s: pick(o.and_then(|x| x.ov2_s), IEEE1547_OV2_TRIP_T_S),
            },
            uf1: TripPoint {
                threshold: pick(o.and_then(|x| x.uf1_hz), IEEE1547_UF1_TRIP_F_HZ),
                clearing_s: pick(o.and_then(|x| x.uf1_s), IEEE1547_UF1_TRIP_T_S),
            },
            uf2: TripPoint {
                threshold: pick(o.and_then(|x| x.uf2_hz), IEEE1547_UF2_TRIP_F_HZ),
                clearing_s: pick(o.and_then(|x| x.uf2_s), IEEE1547_UF2_TRIP_T_S),
            },
            of1: TripPoint {
                threshold: pick(o.and_then(|x| x.of1_hz), IEEE1547_OF1_TRIP_F_HZ),
                clearing_s: pick(o.and_then(|x| x.of1_s), IEEE1547_OF1_TRIP_T_S),
            },
            of2: TripPoint {
                threshold: pick(o.and_then(|x| x.of2_hz), IEEE1547_OF2_TRIP_F_HZ),
                clearing_s: pick(o.and_then(|x| x.of2_s), IEEE1547_OF2_TRIP_T_S),
            },
            fw_db_of: pick(o.and_then(|x| x.fw_db_of_hz), IEEE1547_FW_DEADBAND_OF_HZ),
            fw_db_uf: pick(o.and_then(|x| x.fw_db_uf_hz), IEEE1547_FW_DEADBAND_UF_HZ),
            fw_k_of: pick(o.and_then(|x| x.fw_k_of), IEEE1547_FW_SLOPE_OF),
            fw_k_uf: pick(o.and_then(|x| x.fw_k_uf), IEEE1547_FW_SLOPE_UF),
            es_v_low: pick(o.and_then(|x| x.es_v_low_pu), IEEE1547_ES_V_LOW_PU),
            es_v_high: pick(o.and_then(|x| x.es_v_high_pu), IEEE1547_ES_V_HIGH_PU),
            es_f_low: pick(o.and_then(|x| x.es_f_low_hz), IEEE1547_ES_F_LOW_HZ),
            es_f_high: pick(o.and_then(|x| x.es_f_high_hz), IEEE1547_ES_F_HIGH_HZ),
            es_delay_s: pick(o.and_then(|x| x.es_delay_s), IEEE1547_ES_DELAY_S),
        };
        tables.validate(owner)?;
        Ok(tables)
    }
}

impl Der1547Tables {
    fn validate(&self, owner: &str) -> crate::Result<()> {
        let points: [(&str, f64, bool); 25] = [
            ("uv1_v", self.uv1.threshold, self.uv1.threshold > 0.0),
            ("uv1_s", self.uv1.clearing_s, self.uv1.clearing_s >= 0.0),
            ("uv2_v", self.uv2.threshold, self.uv2.threshold > 0.0),
            ("uv2_s", self.uv2.clearing_s, self.uv2.clearing_s >= 0.0),
            ("ov1_v", self.ov1.threshold, self.ov1.threshold > 0.0),
            ("ov1_s", self.ov1.clearing_s, self.ov1.clearing_s >= 0.0),
            ("ov2_v", self.ov2.threshold, self.ov2.threshold > 0.0),
            ("ov2_s", self.ov2.clearing_s, self.ov2.clearing_s >= 0.0),
            ("uf1_hz", self.uf1.threshold, self.uf1.threshold > 0.0),
            ("uf1_s", self.uf1.clearing_s, self.uf1.clearing_s >= 0.0),
            ("uf2_hz", self.uf2.threshold, self.uf2.threshold > 0.0),
            ("uf2_s", self.uf2.clearing_s, self.uf2.clearing_s >= 0.0),
            ("of1_hz", self.of1.threshold, self.of1.threshold > 0.0),
            ("of1_s", self.of1.clearing_s, self.of1.clearing_s >= 0.0),
            ("of2_hz", self.of2.threshold, self.of2.threshold > 0.0),
            ("of2_s", self.of2.clearing_s, self.of2.clearing_s >= 0.0),
            ("fw_db_of_hz", self.fw_db_of, self.fw_db_of >= 0.0),
            ("fw_db_uf_hz", self.fw_db_uf, self.fw_db_uf >= 0.0),
            ("fw_k_of", self.fw_k_of, self.fw_k_of > 0.0),
            ("fw_k_uf", self.fw_k_uf, self.fw_k_uf > 0.0),
            ("es_v_low_pu", self.es_v_low, self.es_v_low > 0.0),
            ("es_v_high_pu", self.es_v_high, self.es_v_high > 0.0),
            ("es_f_low_hz", self.es_f_low, self.es_f_low > 0.0),
            ("es_f_high_hz", self.es_f_high, self.es_f_high > 0.0),
            ("es_delay_s", self.es_delay_s, self.es_delay_s >= 0.0),
        ];
        for (name, value, ok) in points {
            if !(value.is_finite() && ok) {
                return Err(HaresError::Equipment(format!(
                    "{owner}: der_1547 override {name} = {value} is not a valid value"
                )));
            }
        }
        if self.es_v_low > self.es_v_high {
            return Err(HaresError::Equipment(format!(
                "{owner}: der_1547 override es_v_low_pu ({}) must not exceed \
                 es_v_high_pu ({})",
                self.es_v_low, self.es_v_high
            )));
        }
        if self.es_f_low > self.es_f_high {
            return Err(HaresError::Equipment(format!(
                "{owner}: der_1547 override es_f_low_hz ({}) must not exceed \
                 es_f_high_hz ({})",
                self.es_f_low, self.es_f_high
            )));
        }
        Ok(())
    }
}

/// The persisted per-DER 1547 run state (checkpoint payload).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Der1547State {
    /// Accumulated in-band seconds per trip point, capped at the point's
    /// clearing time.
    pub uv1_s: f64,
    pub uv2_s: f64,
    pub ov1_s: f64,
    pub ov2_s: f64,
    pub uf1_s: f64,
    pub uf2_s: f64,
    pub of1_s: f64,
    pub of2_s: f64,
    /// The trip latch: a tripped inverter produces nothing until the
    /// enter-service delay completes inside the enter-service windows.
    pub tripped: bool,
    pub trip_reason: u8,
    /// Accumulated in-window seconds toward the return to service.
    pub es_s: f64,
    /// The frequency-droop pre-disturbance output (pu of the active-power
    /// rating), latched on the step the deadband is crossed and held while
    /// the excursion holds.
    pub fw_p_pre_pu: Option<f64>,
    /// The most recent step's frequency-droop adjustment, in per unit of the rating.
    pub last_fw_adj_pu: f64,
}

impl Der1547State {
    /// The largest accumulated in-band time across the eight trip points,
    /// in seconds: how far into the closest clearing-time window the
    /// disturbance is. The state-based read for the steps the layer does
    /// not step (an idle export leg); a stepped step publishes its
    /// outcome, whose trip carries the firing point's own window.
    pub fn max_accumulator_s(&self) -> f64 {
        self.uv1_s
            .max(self.uv2_s)
            .max(self.ov1_s)
            .max(self.ov2_s)
            .max(self.uf1_s)
            .max(self.uf2_s)
            .max(self.of1_s)
            .max(self.of2_s)
    }
}

/// One step's IEEE 1547 outcome for one DER inverter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Der1547Outcome {
    /// True when the inverter is tripped for this step: cease to energize.
    pub tripped: bool,
    pub trip_reason: u8,
    /// The active-power reference, in W in injection space (positive =
    /// export): the injection the droop allows when online, 0 when
    /// tripped. Bitwise the model's own output when the droop's reference
    /// coincides with it in per unit, so a caller applies the reference on
    /// an exact comparison without disturbing the undrooped path.
    pub p_ref_w: f64,
    /// The frequency-droop adjustment applied this step [pu of rating]
    /// (reference minus pre-disturbance output; 0.0 when inactive).
    pub fw_adj_pu: f64,
    /// The accumulated time of the point the step's excursion is timed
    /// against, in seconds: on a trip, the firing point's own window (its
    /// clearing time, the maximum the standard grants) even when other
    /// bands ran on the step; while the inverter rides, the largest
    /// accumulated time across the eight points; zero while the latch
    /// holds.
    pub ride_through_s: f64,
    /// The enter-service accumulation, in seconds while tripped.
    pub enter_service_s: f64,
}

/// The per-DER IEEE 1547 controller: the resolved tables plus the run
/// state. Constructed at equipment init from the config's `der_1547`
/// block; its state rides the equipment's checkpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct Der1547Controller {
    tables: Der1547Tables,
    p_max_w: f64,
    state: Der1547State,
}

impl Der1547Controller {
    /// Builds a controller from a settings block and the inverter's
    /// active-power rating, in W at unity power factor (the droop's per-unit
    /// base). The rating must be positive.
    ///
    /// # Errors
    ///
    /// `HaresError::Equipment` when the tables fail validation or the
    /// rating is not a positive finite value.
    pub fn new(settings: &Der1547Settings, p_max_w: f64, owner: &str) -> crate::Result<Self> {
        if !p_max_w.is_finite() || p_max_w <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "{owner}: der_1547 requires a positive active-power rating, \
                 got {p_max_w} W"
            )));
        }
        Ok(Self {
            tables: settings.resolve_tables(owner)?,
            p_max_w,
            state: Der1547State::default(),
        })
    }

    /// The resolved tables (defaults with the operator's overrides).
    pub fn tables(&self) -> &Der1547Tables {
        &self.tables
    }

    pub fn state(&self) -> &Der1547State {
        &self.state
    }

    /// Restores checkpointed state.
    pub fn restore_state(&mut self, state: Der1547State) {
        self.state = state;
    }

    /// Records a step in which the DER function did not conduct (the
    /// caller's inverter was off): the frequency-droop pre-disturbance
    /// latch clears, so an excursion that ended while the inverter was
    /// idle cannot anchor the next one. The trip latch and the
    /// enter-service accumulation are the export leg's own state and are
    /// untouched.
    pub fn note_inactive_step(&mut self) {
        self.state.fw_p_pre_pu = None;
        self.state.last_fw_adj_pu = 0.0;
    }

    /// True when the bus voltage or the grid frequency sits in a band the
    /// trip tables govern under any category's defaults: the widest
    /// watch band across the categories (under Cat III's UV1 0.88 pu and
    /// over the shared OV1 1.10 pu; below UF1's 58.5 Hz or above OF1's
    /// 61.2 Hz). The unconfigured layer's excursion detector uses it.
    pub fn excursion_present(v_pu: f64, f_hz: f64) -> bool {
        v_pu < IEEE1547_CAT3_UV1_TRIP_V_PU
            || v_pu > IEEE1547_OV1_TRIP_V_PU
            || f_hz < IEEE1547_UF1_TRIP_F_HZ
            || f_hz > IEEE1547_OF1_TRIP_F_HZ
    }

    /// Steps the controller. `v_pu` is the voltage the inverter's
    /// terminals see (the bus voltage); `f_hz` the grid frequency;
    /// `utility_up` false on a utility-outage step (the outage model's
    /// contract governs instead: the trip timers and the frequency-droop
    /// latch reset, a latched trip waits on the enter-service windows,
    /// which an islanded bus can satisfy). `p_normal_w` is the model's own
    /// output this step in injection space (positive = export, negative =
    /// import for a bidirectional inverter); `p_avail_w` the maximum
    /// injection the DER can deliver this step; `p_floor_w` the minimum
    /// injection the droop may drive toward (0 for a generation-only
    /// inverter, the negative charge limit for a battery). The droop's
    /// reference composes as the floor-bounded ask capped at the available
    /// injection, in both ramp directions: the up-ramp rises from the
    /// anchor but never past the availability, and the down-ramp floors at
    /// the injection floor and then caps at the availability, so a
    /// capability that tightened inside a held excursion governs the
    /// reference the same way it governs the model's own output. The cap
    /// composes last, so an availability below the floor (a contradictory
    /// pair: the DER cannot reach the floor's injection) leaves the
    /// reference at the availability rather than at an unreachable floor.
    ///
    /// A non-finite bus voltage or frequency trips the layer for the step
    /// (fail-safe, matching `GridState::bus_energized`'s NaN robustness: a
    /// poisoned grid state de-energizes the inverter rather than un-gating
    /// it). A non-positive or non-finite timestep is an error.
    ///
    /// # Errors
    ///
    /// `HaresError::Equipment` when `dt_s` is not a positive finite
    /// number of seconds.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        v_pu: f64,
        f_hz: f64,
        dt_s: f64,
        utility_up: bool,
        p_normal_w: f64,
        p_avail_w: f64,
        p_floor_w: f64,
    ) -> crate::Result<Der1547Outcome> {
        if !dt_s.is_finite() || dt_s <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "der_1547: the timestep must be a positive number of \
                 seconds, got {dt_s}"
            )));
        }
        if !v_pu.is_finite() || !f_hz.is_finite() {
            return Ok(Der1547Outcome {
                tripped: true,
                trip_reason: trip_reason::NONE,
                p_ref_w: 0.0,
                fw_adj_pu: 0.0,
                ride_through_s: 0.0,
                enter_service_s: self.state.es_s,
            });
        }
        let p_max = self.p_max_w;
        let p_normal_pu = p_normal_w / p_max;
        let p_avail_pu = p_avail_w / p_max;
        let p_floor_pu = p_floor_w / p_max;

        if self.state.tripped {
            // Return to service: accumulate inside the enter-service
            // windows, reset outside them, re-energize on the first
            // in-window step whose accumulated time reaches the delay.
            // The delay is a minimum, so the windows gate it even at
            // zero: a zero-length wait still needs an in-window step. The
            // re-energized step produces the model's own output from that
            // step on.
            let in_window = v_pu >= self.tables.es_v_low
                && v_pu <= self.tables.es_v_high
                && f_hz >= self.tables.es_f_low
                && f_hz <= self.tables.es_f_high;
            if in_window {
                self.state.es_s = (self.tables.es_delay_s).min(self.state.es_s + dt_s);
            } else {
                self.state.es_s = 0.0;
            }
            let reconnected = in_window && self.state.es_s >= self.tables.es_delay_s;
            if reconnected {
                self.state.tripped = false;
                self.state.trip_reason = trip_reason::NONE;
                self.state.es_s = 0.0;
                self.state.fw_p_pre_pu = None;
                self.state.last_fw_adj_pu = 0.0;
                Ok(Der1547Outcome {
                    tripped: false,
                    trip_reason: trip_reason::NONE,
                    p_ref_w: p_normal_w,
                    fw_adj_pu: 0.0,
                    ride_through_s: 0.0,
                    enter_service_s: 0.0,
                })
            } else {
                Ok(Der1547Outcome {
                    tripped: true,
                    trip_reason: self.state.trip_reason,
                    p_ref_w: 0.0,
                    fw_adj_pu: 0.0,
                    ride_through_s: 0.0,
                    enter_service_s: self.state.es_s,
                })
            }
        } else if !utility_up {
            // Utility outage with the inverter still online: the outage
            // model's contract governs (the dead-bus gate zeroes a
            // grid-following inverter; an islanded bus is the island
            // source's physics). The trip timers and the droop latch reset;
            // the caller's outage gate handles the power.
            self.state.uv1_s = 0.0;
            self.state.uv2_s = 0.0;
            self.state.ov1_s = 0.0;
            self.state.ov2_s = 0.0;
            self.state.uf1_s = 0.0;
            self.state.uf2_s = 0.0;
            self.state.of1_s = 0.0;
            self.state.of2_s = 0.0;
            self.state.fw_p_pre_pu = None;
            self.state.last_fw_adj_pu = 0.0;
            Ok(Der1547Outcome {
                tripped: false,
                trip_reason: trip_reason::NONE,
                p_ref_w: p_normal_w,
                fw_adj_pu: 0.0,
                ride_through_s: 0.0,
                enter_service_s: 0.0,
            })
        } else {
            // Grid-connected online: integrate every trip point's on-delay.
            // The points are visited deepest-first; the first firing point
            // in that order names the reason. A voltage point precedes a
            // frequency point when a simultaneous excursion completes both
            // bands on one step: every point integrates independently, and
            // the reason is the first fired in this fixed order rather than
            // a resolution between classes (the standard defines each point
            // against its own disturbance; it does not rank them).
            let dt = dt_s;
            let mut max_acc = 0.0_f64;
            // The reason and the window its own timer reached. Both ride
            // the trip: another holding band's accumulator is not evidence
            // about the point that fired.
            let mut fired: Option<(u8, f64)> = None;
            macro_rules! accumulate {
                ($field:ident, $cond:expr, $point:expr, $reason:expr) => {{
                    let point: &TripPoint = &$point;
                    let holds = $cond;
                    if holds {
                        self.state.$field = point.clearing_s.min(self.state.$field + dt);
                    } else {
                        self.state.$field = 0.0;
                    }
                    max_acc = max_acc.max(self.state.$field);
                    // A point fires only on a step its condition holds:
                    // the accumulator of a band the bus is not in is zero,
                    // and a zero clearing time must not turn that zero
                    // into a trip.
                    if holds && self.state.$field >= point.clearing_s && fired.is_none() {
                        fired = Some(($reason, self.state.$field));
                    }
                }};
            }
            accumulate!(
                uv2_s,
                v_pu < self.tables.uv2.threshold,
                self.tables.uv2,
                trip_reason::UV2
            );
            accumulate!(
                uv1_s,
                v_pu < self.tables.uv1.threshold,
                self.tables.uv1,
                trip_reason::UV1
            );
            accumulate!(
                ov2_s,
                v_pu > self.tables.ov2.threshold,
                self.tables.ov2,
                trip_reason::OV2
            );
            accumulate!(
                ov1_s,
                v_pu > self.tables.ov1.threshold,
                self.tables.ov1,
                trip_reason::OV1
            );
            accumulate!(
                uf2_s,
                f_hz < self.tables.uf2.threshold,
                self.tables.uf2,
                trip_reason::UF2
            );
            accumulate!(
                uf1_s,
                f_hz < self.tables.uf1.threshold,
                self.tables.uf1,
                trip_reason::UF1
            );
            accumulate!(
                of2_s,
                f_hz > self.tables.of2.threshold,
                self.tables.of2,
                trip_reason::OF2
            );
            accumulate!(
                of1_s,
                f_hz > self.tables.of1.threshold,
                self.tables.of1,
                trip_reason::OF1
            );

            if let Some((reason, window_reached_s)) = fired {
                self.state.tripped = true;
                self.state.trip_reason = reason;
                self.state.es_s = 0.0;
                self.state.uv1_s = 0.0;
                self.state.uv2_s = 0.0;
                self.state.ov1_s = 0.0;
                self.state.ov2_s = 0.0;
                self.state.uf1_s = 0.0;
                self.state.uf2_s = 0.0;
                self.state.of1_s = 0.0;
                self.state.of2_s = 0.0;
                self.state.fw_p_pre_pu = None;
                self.state.last_fw_adj_pu = 0.0;
                Ok(Der1547Outcome {
                    tripped: true,
                    trip_reason: reason,
                    p_ref_w: 0.0,
                    fw_adj_pu: 0.0,
                    ride_through_s: window_reached_s,
                    enter_service_s: 0.0,
                })
            } else {
                // Frequency-droop (frequency-Watt): no adjustment inside
                // the deadband; beyond it a linear ramp from the
                // pre-disturbance output at 1/(60·k) pu per Hz (EPRI
                // OpenDER Eq. 3.7.1-13/14). Over-frequency ramps down
                // toward the injection floor; under-frequency ramps up
                // toward the available injection.
                let over = f_hz > IEEE1547_NOMINAL_FREQ_HZ + self.tables.fw_db_of;
                let under = f_hz < IEEE1547_NOMINAL_FREQ_HZ - self.tables.fw_db_uf;
                let (p_ref_pu, fw_adj_pu) = if !over && !under {
                    self.state.fw_p_pre_pu = None;
                    self.state.last_fw_adj_pu = 0.0;
                    (p_normal_pu, 0.0)
                } else {
                    let p_pre = match self.state.fw_p_pre_pu {
                        Some(p) => p,
                        None => {
                            // Engage step: latch the pre-disturbance
                            // output (Eq. 3.7.1-12), which is the model's
                            // undrooped output this step: the operating
                            // point the disturbance found. It is held for
                            // the excursion, so the ramp does not ratchet
                            // down with its own curtailment.
                            self.state.fw_p_pre_pu = Some(p_normal_pu);
                            p_normal_pu
                        }
                    };
                    let p_ref = if under {
                        let delta = (IEEE1547_NOMINAL_FREQ_HZ - self.tables.fw_db_uf - f_hz)
                            / (IEEE1547_NOMINAL_FREQ_HZ * self.tables.fw_k_uf);
                        (p_pre + delta).min(p_avail_pu)
                    } else {
                        let delta = (f_hz - IEEE1547_NOMINAL_FREQ_HZ - self.tables.fw_db_of)
                            / (IEEE1547_NOMINAL_FREQ_HZ * self.tables.fw_k_of);
                        // The down-ramp's reference stays inside the
                        // deliverable window: the floor below, the
                        // available injection above. The anchor is the
                        // crossing step's operating point; a capability
                        // that tightened since the crossing (a falling
                        // irradiance, a deepening demand-response shed)
                        // governs the reference the same way it governs
                        // the model's own output.
                        ((p_pre - delta).max(p_floor_pu)).min(p_avail_pu)
                    };
                    (p_ref, p_ref - p_pre)
                };
                self.state.last_fw_adj_pu = fw_adj_pu;
                // When the reference coincides with the model's own output
                // in per unit (the droop is inactive, or its cap binds at
                // the model's output), the outcome carries the model's
                // value bitwise rather than its pu round-trip: the callers
                // apply the reference on an exact comparison, so the
                // nominal-frequency path stays bit-identical.
                let p_ref_w = if p_ref_pu == p_normal_pu {
                    p_normal_w
                } else {
                    p_ref_pu * p_max
                };
                Ok(Der1547Outcome {
                    tripped: false,
                    trip_reason: trip_reason::NONE,
                    p_ref_w,
                    fw_adj_pu,
                    ride_through_s: max_acc,
                    enter_service_s: 0.0,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "TestDER";

    fn cat2() -> Der1547Settings {
        Der1547Settings {
            category: Der1547Category::CatII,
            overrides: None,
        }
    }

    fn controller(settings: &Der1547Settings, p_max_w: f64) -> Der1547Controller {
        Der1547Controller::new(settings, p_max_w, OWNER).expect("valid controller")
    }

    /// One grid-connected step at the given voltage and frequency for a
    /// DER whose model output and availability are both `p_w`.
    fn one_step(
        ctrl: &mut Der1547Controller,
        v_pu: f64,
        f_hz: f64,
        dt_s: f64,
        p_w: f64,
    ) -> Der1547Outcome {
        ctrl.step(v_pu, f_hz, dt_s, true, p_w, p_w, 0.0)
            .expect("valid step")
    }

    // -- The table pins: the Category II defaults (IEEE 1547-2018 voltage
    //    shall-trip Tables 11-13 via NERC Table 1.2 / MISO Table 3; the
    //    frequency trip, droop and enter-service defaults via MISO Tables
    //    6, 8 and 9). --

    #[test]
    fn category_ii_defaults_match_the_standard() {
        let t = *controller(&cat2(), 5000.0).tables();
        // Voltage shall-trip defaults, Category II.
        assert_eq!(t.uv1.threshold, 0.70);
        assert_eq!(t.uv1.clearing_s, 10.0);
        assert_eq!(t.uv2.threshold, 0.45);
        assert_eq!(t.uv2.clearing_s, 0.16);
        assert_eq!(t.ov1.threshold, 1.10);
        assert_eq!(t.ov1.clearing_s, 2.0);
        assert_eq!(t.ov2.threshold, 1.20);
        assert_eq!(t.ov2.clearing_s, 0.16);
        // Frequency shall-trip defaults (identical across categories).
        assert_eq!(t.uf1.threshold, 58.5);
        assert_eq!(t.uf1.clearing_s, 300.0);
        assert_eq!(t.uf2.threshold, 56.5);
        assert_eq!(t.uf2.clearing_s, 0.16);
        assert_eq!(t.of1.threshold, 61.2);
        assert_eq!(t.of1.clearing_s, 300.0);
        assert_eq!(t.of2.threshold, 62.0);
        assert_eq!(t.of2.clearing_s, 0.16);
        // Frequency-droop defaults.
        assert_eq!(t.fw_db_of, 0.036);
        assert_eq!(t.fw_db_uf, 0.036);
        assert_eq!(t.fw_k_of, 0.05);
        assert_eq!(t.fw_k_uf, 0.05);
        // Enter-service defaults.
        assert_eq!(t.es_v_low, 0.917);
        assert_eq!(t.es_v_high, 1.05);
        assert_eq!(t.es_f_low, 59.5);
        assert_eq!(t.es_f_high, 60.1);
        assert_eq!(t.es_delay_s, 300.0);
    }

    #[test]
    fn category_i_and_iii_defaults_match_the_standard() {
        let cat1 = Der1547Settings {
            category: Der1547Category::CatI,
            overrides: None,
        };
        let t1 = *controller(&cat1, 5000.0).tables();
        assert_eq!(t1.uv1.threshold, 0.70);
        assert_eq!(t1.uv1.clearing_s, 2.0);
        assert_eq!(t1.uv2.threshold, 0.45);
        assert_eq!(t1.uv2.clearing_s, 0.16);
        assert_eq!(t1.ov1.clearing_s, 2.0);

        let cat3 = Der1547Settings {
            category: Der1547Category::CatIII,
            overrides: None,
        };
        let t3 = *controller(&cat3, 5000.0).tables();
        assert_eq!(t3.uv1.threshold, 0.88);
        assert_eq!(t3.uv1.clearing_s, 21.0);
        assert_eq!(t3.uv2.threshold, 0.50);
        assert_eq!(t3.uv2.clearing_s, 2.0);
        assert_eq!(t3.ov1.clearing_s, 13.0);
        // Frequency, droop and enter-service are category-invariant.
        assert_eq!(t3.uf1.threshold, 58.5);
        assert_eq!(t3.of2.threshold, 62.0);
        assert_eq!(t3.es_delay_s, 300.0);
    }

    #[test]
    fn overrides_apply_and_are_validated() {
        let settings = Der1547Settings {
            category: Der1547Category::CatII,
            overrides: Some(Der1547Overrides {
                uv1_s: Some(21.0),
                es_delay_s: Some(60.0),
                ..Default::default()
            }),
        };
        let t = *controller(&settings, 5000.0).tables();
        assert_eq!(t.uv1.clearing_s, 21.0);
        assert_eq!(t.uv1.threshold, 0.70, "an unset override takes the default");
        assert_eq!(t.es_delay_s, 60.0);

        for bad in [
            Der1547Overrides {
                uv1_v: Some(-1.0),
                ..Default::default()
            },
            Der1547Overrides {
                uv1_s: Some(f64::NAN),
                ..Default::default()
            },
            Der1547Overrides {
                fw_k_of: Some(0.0),
                ..Default::default()
            },
            Der1547Overrides {
                es_delay_s: Some(-5.0),
                ..Default::default()
            },
            Der1547Overrides {
                es_v_low_pu: Some(1.10),
                es_v_high_pu: Some(1.05),
                ..Default::default()
            },
        ] {
            let settings = Der1547Settings {
                category: Der1547Category::CatII,
                overrides: Some(bad),
            };
            let err = Der1547Controller::new(&settings, 5000.0, OWNER)
                .expect_err("an invalid override is a configuration error");
            assert!(err.to_string().contains("der_1547"), "{err}");
        }

        let err = Der1547Controller::new(&cat2(), -1.0, OWNER)
            .expect_err("a nonpositive rating is a configuration error");
        assert!(err.to_string().contains("active-power rating"), "{err}");
    }

    /// The timestep's contract: the layer integrates a positive finite
    /// number of seconds. A zero or negative step would freeze or rewind
    /// every timer, and a non-finite one would leave the accumulators
    /// meaningless, so both are refused with an error naming the
    /// timestep.
    #[test]
    fn a_non_positive_or_non_finite_timestep_is_refused() {
        let mut ctrl = controller(&cat2(), 5000.0);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let err = ctrl
                .step(0.6, 60.0, bad, true, 2700.0, 2700.0, 0.0)
                .expect_err("a non-positive or non-finite timestep is refused");
            assert!(err.to_string().contains("timestep"), "{err}");
        }
    }

    // -- The trip and ride-through witnesses. --

    /// The brownout round's F8 defect witness: a sustained 0.6 pu sag sits
    /// in UV1's band (0.45 <= V < 0.70); the Category II default clearing
    /// time is 10 s, so a timestep that reaches it is a tripped step. The
    /// pre-1547 model left the inverter at full output through the sag.
    #[test]
    fn sustained_06pu_sag_trips_a_category_ii_inverter() {
        let mut ctrl = controller(&cat2(), 5000.0);
        let out = one_step(&mut ctrl, 0.6, 60.0, 900.0, 2700.0);
        assert!(out.tripped, "the 0.6 pu sag must trip the inverter");
        assert_eq!(out.trip_reason, trip_reason::UV1);
        assert_eq!(out.p_ref_w, 0.0);
        assert_eq!(
            out.ride_through_s, 10.0,
            "the accumulator caps at the window"
        );
    }

    /// The ride-through window honored: at a timestep finer than the
    /// clearing time the inverter keeps producing until the accumulated
    /// in-band time reaches the window, and the window's boundary is the
    /// trip.
    #[test]
    fn ride_through_window_holds_until_the_boundary_step() {
        let mut ctrl = controller(&cat2(), 5000.0);
        // 5 s steps: two steps accumulate 10 s; the second is the trip.
        let ride = one_step(&mut ctrl, 0.6, 60.0, 5.0, 2700.0);
        assert!(!ride.tripped, "5 s inside a 10 s window rides through");
        assert_eq!(ride.p_ref_w, 2700.0);
        assert_eq!(ride.ride_through_s, 5.0);
        let trip = one_step(&mut ctrl, 0.6, 60.0, 5.0, 2700.0);
        assert!(
            trip.tripped,
            "the accumulated 10 s reaches the window: trip"
        );
        assert_eq!(trip.trip_reason, trip_reason::UV1);

        // The voltage boundary: exactly 0.70 pu is not inside UV1's band
        // (the condition is strict), so no trip however long it holds.
        let mut ctrl = controller(&cat2(), 5000.0);
        let at_boundary = one_step(&mut ctrl, 0.70, 60.0, 900.0, 2700.0);
        assert!(!at_boundary.tripped, "0.70 pu is not below UV1's 0.70");
        let below = one_step(&mut ctrl, 0.6999, 60.0, 900.0, 2700.0);
        assert!(below.tripped, "0.6999 pu is below UV1's 0.70");

        // The deep-sag window: 0.4 pu trips on UV2's 0.16 s, not UV1's.
        let mut ctrl = controller(&cat2(), 5000.0);
        let deep = one_step(&mut ctrl, 0.4, 60.0, 5.0, 2700.0);
        assert!(deep.tripped, "0.4 pu is below UV2's 0.45");
        assert_eq!(deep.trip_reason, trip_reason::UV2);
    }

    #[test]
    fn overvoltage_and_frequency_points_trip_per_their_windows() {
        // 1.25 pu is beyond OV2's 1.20: 0.16 s clears on any timestep.
        let mut ctrl = controller(&cat2(), 5000.0);
        let out = one_step(&mut ctrl, 1.25, 60.0, 900.0, 0.0);
        assert!(out.tripped);
        assert_eq!(out.trip_reason, trip_reason::OV2);

        // 1.15 pu is inside OV1's band (1.10 < V <= 1.20): a 900 s step
        // clears OV1's 2.0 s window.
        let mut ctrl = controller(&cat2(), 5000.0);
        let out = one_step(&mut ctrl, 1.15, 60.0, 900.0, 0.0);
        assert!(out.tripped);
        assert_eq!(out.trip_reason, trip_reason::OV1);

        // 62.5 Hz is beyond OF2's 62.0; 58.0 Hz sits in UF1's band
        // (56.5 <= f < 58.5); 56.0 Hz is beyond UF2's 56.5.
        let mut ctrl = controller(&cat2(), 5000.0);
        let out = one_step(&mut ctrl, 1.0, 62.5, 900.0, 2700.0);
        assert!(out.tripped);
        assert_eq!(out.trip_reason, trip_reason::OF2);

        let mut ctrl = controller(&cat2(), 5000.0);
        let out = one_step(&mut ctrl, 1.0, 58.0, 900.0, 2700.0);
        assert!(out.tripped, "900 s clears UF1's 300 s window");
        assert_eq!(out.trip_reason, trip_reason::UF1);

        let mut ctrl = controller(&cat2(), 5000.0);
        let out = one_step(&mut ctrl, 1.0, 56.0, 900.0, 2700.0);
        assert!(out.tripped);
        assert_eq!(out.trip_reason, trip_reason::UF2);

        // 61.5 Hz is inside OF1's band: a fine timestep rides; the window
        // completes on the step that reaches 300 s (the fifth 60 s step).
        let mut ctrl = controller(&cat2(), 5000.0);
        let ride = one_step(&mut ctrl, 1.0, 61.5, 60.0, 2700.0);
        assert!(!ride.tripped);
        for step in 2..=4 {
            let out = one_step(&mut ctrl, 1.0, 61.5, 60.0, 2700.0);
            assert!(!out.tripped, "step {step}: 60 x {step} s is below 300 s");
        }
        let out = one_step(&mut ctrl, 1.0, 61.5, 60.0, 2700.0);
        assert!(out.tripped, "300 s accumulated at the fifth 60 s step");
        assert_eq!(out.trip_reason, trip_reason::OF1);
    }

    /// A zero clearing time is "trip on the step the condition holds". The
    /// accumulator of a point whose condition is false is zero, and a
    /// zero-length window is not itself a trip: a point must never fire on
    /// a condition it does not see, whatever its clearing time (the
    /// operator's surface admits 0.0 as a clearing time).
    #[test]
    fn zero_clearing_time_trips_only_while_its_condition_holds() {
        let settings = Der1547Settings {
            category: Der1547Category::CatII,
            overrides: Some(Der1547Overrides {
                uv1_s: Some(0.0),
                ..Default::default()
            }),
        };
        let mut ctrl = controller(&settings, 5000.0);
        let nominal = one_step(&mut ctrl, 1.0, 60.0, 900.0, 4500.0);
        assert!(
            !nominal.tripped,
            "1.0 pu is inside no under-voltage band: the zero window never starts"
        );
        assert_eq!(nominal.p_ref_w, 4500.0, "the inverter rides at full output");

        let sag = one_step(&mut ctrl, 0.6, 60.0, 900.0, 4500.0);
        assert!(
            sag.tripped,
            "0.6 pu starts the zero-length window: the same step trips"
        );
        assert_eq!(sag.trip_reason, trip_reason::UV1);
    }

    /// The enter-service window's bounds are inside the window (IEEE
    /// 1547-2018 clauses 4.10.2/4.10.3 defaults: V in [0.917, 1.05] pu, f
    /// in [59.5, 60.1] Hz): a restored bus at a bound accumulates the
    /// delay; a step just outside it resets the accumulation.
    #[test]
    fn enter_service_windows_include_their_bounds() {
        for (v, f, inside) in [
            (1.05, 60.1, true),
            (0.917, 59.5, true),
            (1.0001, 60.1, true),
            (1.0501, 60.0, false),
            (0.9169, 60.0, false),
            (1.0, 60.1001, false),
            (1.0, 59.4999, false),
        ] {
            let mut ctrl = controller(&cat2(), 5000.0);
            let trip = one_step(&mut ctrl, 0.6, 60.0, 60.0, 2700.0);
            assert!(trip.tripped, "the 60 s sag trips UV1");
            let out = one_step(&mut ctrl, v, f, 60.0, 2700.0);
            assert!(out.tripped, "({v} pu, {f} Hz): the latch still holds");
            let expected = if inside { 60.0 } else { 0.0 };
            assert_eq!(
                out.enter_service_s, expected,
                "({v} pu, {f} Hz): the accumulation must be {expected} s"
            );
        }
    }

    /// A disturbance that ends inside the ride-through window resets the
    /// accumulator: the window is per-disturbance, not cumulative.
    #[test]
    fn recovery_resets_the_ride_through_accumulator() {
        let mut ctrl = controller(&cat2(), 5000.0);
        let _ = one_step(&mut ctrl, 0.6, 60.0, 5.0, 2700.0);
        let normal = one_step(&mut ctrl, 1.0, 60.0, 5.0, 2700.0);
        assert!(!normal.tripped);
        assert_eq!(normal.ride_through_s, 0.0, "the accumulator reset");
        let ride = one_step(&mut ctrl, 0.6, 60.0, 5.0, 2700.0);
        assert!(!ride.tripped, "a fresh disturbance starts a fresh window");
    }

    /// The eight accumulators are independent per-point timers: one
    /// point's excursion never gates another's timing. A step that leaves
    /// one band while another holds resets the first point's accumulator
    /// alone, and a fresh excursion in the first band starts a fresh
    /// window while the second point's keeps running.
    #[test]
    fn each_points_accumulator_runs_independently_of_the_others() {
        let mut ctrl = controller(&cat2(), 5000.0);
        // Both bands hold at once: UV1 (0.45 <= 0.6 < 0.70) and OF1
        // (61.5 > 61.2) accumulate together.
        let both = one_step(&mut ctrl, 0.6, 61.5, 5.0, 2700.0);
        assert!(!both.tripped, "5 s is inside both windows");
        assert_eq!(ctrl.state().uv1_s, 5.0);
        assert_eq!(ctrl.state().of1_s, 5.0);

        // UV1's band clears while OF1's holds: UV1's window resets
        // alone, OF1's accumulated time is untouched.
        let of_only = one_step(&mut ctrl, 1.0, 61.5, 5.0, 2700.0);
        assert!(!of_only.tripped);
        assert_eq!(
            ctrl.state().uv1_s,
            0.0,
            "UV1's recovery resets UV1's timer alone"
        );
        assert_eq!(
            ctrl.state().of1_s,
            10.0,
            "OF1's window does not borrow UV1's recovery"
        );

        // UV1's band returns while OF1's clears: UV1 starts a fresh
        // window, OF1's resets alone.
        let uv_only = one_step(&mut ctrl, 0.6, 60.0, 5.0, 2700.0);
        assert!(!uv_only.tripped);
        assert_eq!(ctrl.state().uv1_s, 5.0, "a fresh UV1 disturbance");
        assert_eq!(
            ctrl.state().of1_s,
            0.0,
            "OF1's recovery resets OF1's timer alone"
        );

        // Neither band holds: both timers are zero.
        let neither = one_step(&mut ctrl, 1.0, 60.0, 5.0, 2700.0);
        assert!(!neither.tripped);
        assert_eq!(ctrl.state().max_accumulator_s(), 0.0);
    }

    /// The trip's published window is the firing point's own. A 0.4 pu sag
    /// sits below both UV2's 0.45 pu and UV1's 0.70 pu, so both on-delay
    /// timers integrate; the deep point fires first (its 0.16 s window)
    /// and the ride-through channel reports that window, not UV1's 10 s
    /// timer that ran on the same excursion. The clearing time is each
    /// point's own maximum; a trip is not evidence another band's window
    /// elapsed.
    #[test]
    fn a_trip_publishes_the_firing_points_own_window() {
        let mut ctrl = controller(&cat2(), 5000.0);
        let deep = one_step(&mut ctrl, 0.4, 60.0, 900.0, 2700.0);
        assert!(deep.tripped);
        assert_eq!(
            deep.trip_reason,
            trip_reason::UV2,
            "the deep point is the governing one"
        );
        assert_eq!(
            deep.ride_through_s, 0.16,
            "UV2's window is the one the firing step reached"
        );
        assert_eq!(
            ctrl.state().max_accumulator_s(),
            0.0,
            "the trip spends every point's timer"
        );
    }

    /// A simultaneous voltage and frequency excursion runs both trip paths
    /// at once. Every point integrates independently, so a step that
    /// completes both windows trips on the step: the reason is the firing
    /// point in the tables' fixed order (the voltage points are visited
    /// before the frequency points, and the deeper point before the
    /// shallower within a class), and the ride-through channel reports
    /// that point's window rather than the longer frequency window the
    /// same excursion also completed. The trip spends all of them.
    #[test]
    fn a_simultaneous_voltage_and_frequency_excursion_names_the_voltage_point() {
        // 0.6 pu completes UV1's 10 s window; 61.5 Hz completes OF1's
        // 300 s window on the same 300 s step.
        let mut ctrl = controller(&cat2(), 5000.0);
        let out = one_step(&mut ctrl, 0.6, 61.5, 300.0, 2700.0);
        assert!(out.tripped, "a step that completes either window trips");
        assert_eq!(
            out.trip_reason,
            trip_reason::UV1,
            "the voltage point is visited first: a step that completes both \
             windows names it"
        );
        assert_eq!(
            out.ride_through_s, 10.0,
            "the published window is UV1's 10 s, not OF1's 300 s"
        );
        assert_eq!(
            ctrl.state().max_accumulator_s(),
            0.0,
            "the trip spends both excursions' timers"
        );

        // The deeper point wins within a class: 1.25 pu completes OV2's
        // 0.16 s window before OV1's 2.0 s one.
        let mut ctrl = controller(&cat2(), 5000.0);
        let out = one_step(&mut ctrl, 1.25, 60.0, 900.0, 0.0);
        assert_eq!(out.trip_reason, trip_reason::OV2);
        assert_eq!(out.ride_through_s, 0.16);
    }

    // -- The frequency-droop shape. --

    /// The droop's shape beyond the deadband: linear from the
    /// pre-disturbance output at 1/(60·k) pu per Hz. At 60.536 Hz
    /// (0.5 Hz beyond the 60.036 edge) the reference is p_pre − 1/6 pu.
    #[test]
    fn over_frequency_droop_ramps_down_linearly_from_the_deadband_edge() {
        let mut ctrl = controller(&cat2(), 5000.0);
        // Baseline output 0.9 pu (4500 W of 5000 W).
        let _ = one_step(&mut ctrl, 1.0, 60.0, 900.0, 4500.0);
        let out = one_step(&mut ctrl, 1.0, 60.536, 900.0, 4500.0);
        assert!(!out.tripped);
        // 0.5 Hz / (60 * 0.05) = 1/6 pu down from the 0.9 pu latch.
        let expected_w = (0.9 - 1.0 / 6.0) * 5000.0;
        assert!(
            (out.p_ref_w - expected_w).abs() < 1e-9,
            "droop reference {} must be {expected_w}",
            out.p_ref_w
        );
        assert!((out.fw_adj_pu - (-1.0 / 6.0)).abs() < 1e-12);

        // Exactly at the deadband edge: no adjustment.
        let mut ctrl = controller(&cat2(), 5000.0);
        let _ = one_step(&mut ctrl, 1.0, 60.0, 900.0, 4500.0);
        let out = one_step(&mut ctrl, 1.0, 60.0 + 0.036, 900.0, 4500.0);
        assert_eq!(out.p_ref_w, 4500.0);
        assert_eq!(out.fw_adj_pu, 0.0);
    }

    /// The under-frequency leg ramps up toward the available injection,
    /// capped at it: a PV running at its available output cannot respond
    /// upward; a curtailed one restores output.
    #[test]
    fn under_frequency_droop_ramps_up_capped_at_availability() {
        // At available output: the cap binds, no headroom.
        let mut ctrl = controller(&cat2(), 5000.0);
        let _ = one_step(&mut ctrl, 1.0, 60.0, 900.0, 4500.0);
        let out = ctrl
            .step(1.0, 59.5, 900.0, true, 4500.0, 4500.0, 0.0)
            .expect("valid step");
        assert!(!out.tripped);
        assert_eq!(out.p_ref_w, 4500.0, "the availability cap binds");
        assert_eq!(out.fw_adj_pu, 0.0);

        // Curtailed to 0.5 pu with 0.9 pu available: 59.5 Hz is 0.464 Hz
        // below the 59.964 edge; the ramp is 0.464/3 = 0.154667 pu up
        // from the 0.5 pu latch, under the 0.9 pu cap.
        let mut ctrl = controller(&cat2(), 5000.0);
        let _ = ctrl
            .step(1.0, 60.0, 900.0, true, 2500.0, 4500.0, 0.0)
            .expect("valid step");
        let out = ctrl
            .step(1.0, 59.5, 900.0, true, 2500.0, 4500.0, 0.0)
            .expect("valid step");
        let expected_pu = 0.5 + (59.964 - 59.5) / 3.0;
        assert!((out.p_ref_w - expected_pu * 5000.0).abs() < 1e-9);
        assert!((out.p_ref_w - 2500.0).abs() > 1.0, "the ramp moved output");
    }

    /// The down-ramp's mirror of the up-ramp's availability cap: the
    /// reference never leaves the deliverable window (the floor below,
    /// the available injection above). The anchor is the crossing step's
    /// operating point; a capability that tightened since the crossing (a
    /// falling irradiance, a deepening demand-response shed) must govern
    /// the reference the same way it governs the model's own output.
    #[test]
    fn over_frequency_reference_is_capped_at_the_available_injection() {
        let mut ctrl = controller(&cat2(), 5000.0);
        // The crossing step: the excursion begins at a 0.9 pu operating
        // point (4500 W of 5000 W), so the anchor latches at 0.9 pu and
        // 60.536 Hz's ask is 0.9 - 0.5/3 = 0.7333 pu.
        let _ = one_step(&mut ctrl, 1.0, 60.0, 900.0, 4500.0);
        let _ = one_step(&mut ctrl, 1.0, 60.536, 900.0, 4500.0);
        // The capability tightened to 0.2 pu (1000 W) inside the held
        // excursion: the stale anchor's ask (3666.67 W) exceeds the
        // available injection and the cap binds at it.
        let out = ctrl
            .step(1.0, 60.536, 900.0, true, 1000.0, 1000.0, 0.0)
            .expect("valid step");
        assert!(!out.tripped);
        assert_eq!(
            out.p_ref_w, 1000.0,
            "the down-ramp's reference is capped at the available injection"
        );
        assert!((out.fw_adj_pu - (0.2 - 0.9)).abs() < 1e-12);
    }

    /// The deliverable window's two ends compose in a fixed order: the
    /// floor-bounded ask, capped at the available injection last. When a
    /// caller's availability sits below its floor the pair is
    /// contradictory in injection space (the DER cannot reach the floor's
    /// injection), and the physical cap governs: the reference is the
    /// availability, never the floor's unreachable ask. Both ramp
    /// directions compose the same way.
    #[test]
    fn the_availability_caps_the_reference_below_the_floor() {
        // Over-frequency: a 0.9 pu output asks 0.9 - 0.5/3 = 0.7333 pu;
        // the floor is 0.2 pu (1000 W) and the availability 0.1 pu
        // (500 W), below it.
        let mut ctrl = controller(&cat2(), 5000.0);
        let _ = one_step(&mut ctrl, 1.0, 60.0, 900.0, 4500.0);
        let down = ctrl
            .step(1.0, 60.536, 900.0, true, 4500.0, 500.0, 1000.0)
            .expect("valid step");
        assert_eq!(
            down.p_ref_w, 500.0,
            "the availability composes last and caps the floor-bounded ask"
        );

        // Under-frequency: the up-ramp's ask (0.9 + 0.154667 pu) caps at
        // the same availability.
        let mut ctrl = controller(&cat2(), 5000.0);
        let _ = one_step(&mut ctrl, 1.0, 60.0, 900.0, 4500.0);
        let up = ctrl
            .step(1.0, 59.5, 900.0, true, 4500.0, 500.0, 1000.0)
            .expect("valid step");
        assert_eq!(up.p_ref_w, 500.0);
    }

    /// The pre-disturbance anchor is the model's undrooped output at the
    /// step the deadband is crossed. A controller whose very first step is
    /// the excursion has no earlier step to read, and that step's own
    /// operating point is the pre-disturbance output: an absent history is
    /// not a zero anchor (a zero anchor would curtail the DER to the
    /// injection floor for the whole excursion).
    #[test]
    fn droop_anchors_on_the_step_the_deadband_is_crossed() {
        let mut ctrl = controller(&cat2(), 5000.0);
        let out = one_step(&mut ctrl, 1.0, 60.536, 900.0, 4500.0);
        assert!(!out.tripped);
        // 0.5 Hz beyond the 60.036 edge at 1/3 pu per Hz = 1/6 pu down
        // from the 0.9 pu the model delivers this step.
        let expected_w = (0.9 - 1.0 / 6.0) * 5000.0;
        assert!(
            (out.p_ref_w - expected_w).abs() < 1e-9,
            "the ramp starts from the 0.9 pu operating point: expected \
             {expected_w} W, got {} W",
            out.p_ref_w
        );
        assert!((out.fw_adj_pu + 1.0 / 6.0).abs() < 1e-12);
    }

    /// The droop's shape over the governed frequency range: constant at
    /// the pre-disturbance output inside the deadband, linearly decreasing
    /// beyond each edge at 1/(60 x 0.05) pu per Hz, capped at the
    /// available injection on the under-frequency side, and monotone
    /// non-increasing in frequency throughout (an over-frequency rise
    /// would inject harder the worse the over-frequency gets).
    #[test]
    fn droop_shape_is_monotone_over_the_full_frequency_range() {
        // 59.0 Hz to 61.0 Hz sits inside every default trip band's
        // ride-through side (UF1's 58.5 and OF1's 61.2 never see it), so
        // the sweep measures the droop shape alone.
        let mut ctrl = controller(&cat2(), 5000.0);
        let mut prev = f64::INFINITY;
        for i in 0..=2000 {
            let f = 59.0 + 0.001 * i as f64;
            let out = ctrl
                .step(1.0, f, 900.0, true, 4500.0, 5000.0, 0.0)
                .expect("valid step");
            assert!(!out.tripped, "the sweep stays out of every trip band");
            // The tolerance is far below one sweep step's 1.67 W, so it
            // admits the pu-ratio rounding only, not a rise.
            assert!(
                out.p_ref_w <= prev + 1e-9,
                "the reference rose with frequency at {f} Hz: {prev} -> {}",
                out.p_ref_w
            );
            if (59.964..=60.036).contains(&f) {
                assert!(
                    (out.p_ref_w - 4500.0).abs() < 1e-9,
                    "inside the deadband the reference is the 4500 W anchor, \
                     got {} W at {f} Hz",
                    out.p_ref_w
                );
            }
            if i == 0 {
                assert!(
                    (out.p_ref_w - 5000.0).abs() < 1e-9,
                    "the under-frequency ramp caps at the available 5000 W, \
                     got {} W",
                    out.p_ref_w
                );
            }
            prev = out.p_ref_w;
        }
        let expected_w = (0.9 - (61.0 - 60.036) / 3.0) * 5000.0;
        assert!(
            (prev - expected_w).abs() < 1e-6,
            "at 61.0 Hz the ramp is 0.9 - 0.32133 pu = {expected_w} W, \
             got {prev} W"
        );
    }

    /// The droop's pre-disturbance latch holds while the excursion holds
    /// and re-latches from the output on recovery: two identical
    /// excursions ramp from the same base, not a ratcheted one.
    #[test]
    fn droop_latch_holds_through_the_excursion_and_resets_after() {
        let mut ctrl = controller(&cat2(), 5000.0);
        let _ = one_step(&mut ctrl, 1.0, 60.0, 900.0, 4500.0);
        let first = one_step(&mut ctrl, 1.0, 60.536, 900.0, 4500.0);
        let second = one_step(&mut ctrl, 1.0, 60.536, 900.0, 4500.0);
        assert_eq!(first.p_ref_w, second.p_ref_w, "the latch holds");

        // Back inside the deadband: the output returns to normal and the
        // next excursion re-latches from it.
        let recovered = one_step(&mut ctrl, 1.0, 60.0, 900.0, 4500.0);
        assert_eq!(recovered.p_ref_w, 4500.0);
        assert_eq!(recovered.fw_adj_pu, 0.0);
        let again = one_step(&mut ctrl, 1.0, 60.536, 900.0, 4500.0);
        assert_eq!(again.p_ref_w, first.p_ref_w);
    }

    /// The over-frequency ramp's floor: a bidirectional inverter can be
    /// driven into import up to the floor the caller passes (the battery's
    /// charge limit); a generation-only inverter's floor is zero.
    #[test]
    fn over_frequency_floor_stops_the_ramp() {
        // Generation-only: the ramp cannot go below zero. A 63.5 Hz step
        // is 3.464 Hz beyond the deadband edge, 1.155 pu of ramp, beyond
        // the 0.9 pu the inverter delivers: the reference floors at zero
        // injection.
        let mut ctrl = controller(&cat2(), 5000.0);
        let out = ctrl
            .step(1.0, 63.5, 900.0, true, 4500.0, 4500.0, 0.0)
            .expect("valid step");
        assert_eq!(out.p_ref_w, 0.0, "the ramp floors at zero injection");

        // The default trip tables bound a full-output inverter's reach:
        // at OF1's 61.2 Hz edge the ramp is (61.2 - 60.036)/3 = 0.388 pu,
        // so a 1.0 pu output's reference cannot reach the floor before a
        // trip fires. The floor is not reserved for the override surface
        // though: a partly loaded inverter's ramp can reach it inside a
        // trip band (pinned below). With the frequency trip points
        // widened, a 64 Hz excursion ramps a discharging inverter down
        // through zero into import, and the floor (the -2000 W charge
        // limit) stops the ramp at -0.4 pu of the 5000 W rating.
        let settings = Der1547Settings {
            category: crate::der1547::Der1547Category::CatII,
            overrides: Some(crate::der1547::Der1547Overrides {
                of1_hz: Some(61.2),
                of1_s: Some(10_000.0),
                of2_hz: Some(70.0),
                of2_s: Some(0.16),
                ..Default::default()
            }),
        };
        let mut ctrl = controller(&settings, 5000.0);
        let _ = ctrl
            .step(1.0, 60.0, 900.0, true, 4500.0, 4500.0, -2000.0)
            .expect("valid step");
        let out = ctrl
            .step(1.0, 64.0, 900.0, true, 4500.0, 4500.0, -2000.0)
            .expect("valid step");
        assert!((out.p_ref_w + 2000.0).abs() < 1e-9);
        assert!(!out.tripped, "no trip point fires inside the widened band");
    }

    /// The droop's floor may bind inside a trip band while the ride-through
    /// timer runs: a partly loaded inverter's 0.388 pu reach at OF1's edge
    /// is more than its output, so the frequency-Watt function curtails it
    /// to zero injection and it still rides the window (the trip, not the
    /// floor, is the disconnection).
    #[test]
    fn the_droop_floor_binds_inside_a_trip_band_for_a_partly_loaded_inverter() {
        let mut ctrl = controller(&cat2(), 5000.0);
        let mut out = ctrl
            .step(1.0, 61.5, 60.0, true, 1500.0, 1500.0, 0.0)
            .expect("valid step");
        assert!(!out.tripped, "60 s is inside OF1's 300 s window");
        assert_eq!(
            out.p_ref_w, 0.0,
            "0.3 pu of output less the 0.488 pu ramp floors at zero injection"
        );
        assert!((out.fw_adj_pu + 0.3).abs() < 1e-12);
        assert_eq!(out.ride_through_s, 60.0);

        for _ in 0..4 {
            out = ctrl
                .step(1.0, 61.5, 60.0, true, 1500.0, 1500.0, 0.0)
                .expect("valid step");
        }
        assert!(out.tripped, "the 300 s window completes on the fifth step");
        assert_eq!(out.trip_reason, trip_reason::OF1);
        assert_eq!(
            out.ride_through_s, 300.0,
            "the firing step reports the window it reached"
        );
        // The trip and the droop engage on the same step: the trip wins
        // and the step takes no frequency-Watt adjustment. The published
        // adjustment is the step's outcome, not a value the spent anchor
        // would compute.
        assert_eq!(
            out.fw_adj_pu, 0.0,
            "a firing step takes no droop adjustment"
        );
        assert_eq!(ctrl.state().fw_p_pre_pu, None, "the trip clears the anchor");
    }

    // -- Return to service (the reconnection delay). --

    /// The reconnection delay's pin: after a trip, the inverter stays
    /// offline until the bus has held the enter-service windows for the
    /// delay (300 s default). At 60 s steps the fifth restored step
    /// completes the 300 s.
    #[test]
    fn reconnection_waits_out_the_enter_service_delay() {
        let mut ctrl = controller(&cat2(), 5000.0);
        let trip = one_step(&mut ctrl, 0.6, 60.0, 60.0, 2700.0);
        assert!(trip.tripped);

        let mut out = trip;
        for step in 1..=5 {
            out = one_step(&mut ctrl, 1.0, 60.0, 60.0, 2700.0);
            if step < 5 {
                assert!(out.tripped, "step {step}: the delay has not completed");
                assert_eq!(out.p_ref_w, 0.0);
                assert!((out.enter_service_s - 60.0 * step as f64).abs() < 1e-9);
            }
        }
        assert!(!out.tripped, "300 s accumulated: reconnected");
        assert_eq!(out.p_ref_w, 2700.0, "the reconnected step produces");
    }

    /// The delay is a minimum, not the window's substitute: an operator
    /// who sets it to zero (the surface admits 0.0) does not waive the
    /// enter-service windows. A zero-length wait still needs an in-window
    /// step, exactly as a zero clearing time still needs the trip point's
    /// condition: a controller that reconnected on an out-of-window step
    /// would energize the inverter's full output into the disturbance the
    /// windows exist to keep it out of.
    #[test]
    fn a_zero_enter_service_delay_still_honours_the_windows() {
        let settings = Der1547Settings {
            category: Der1547Category::CatII,
            overrides: Some(Der1547Overrides {
                es_delay_s: Some(0.0),
                ..Default::default()
            }),
        };
        let mut ctrl = controller(&settings, 5000.0);
        let trip = one_step(&mut ctrl, 0.4, 60.0, 900.0, 2700.0);
        assert!(
            trip.tripped,
            "0.4 pu starts UV2's window: the same step trips"
        );

        // 0.3 pu is outside the enter-service windows (below 0.917 pu)
        // and still a fault: the latch holds whatever the delay.
        let out = one_step(&mut ctrl, 0.3, 60.0, 900.0, 2700.0);
        assert!(
            out.tripped,
            "a zero delay does not waive the enter-service windows"
        );
        assert_eq!(out.p_ref_w, 0.0, "the latched inverter produces nothing");
        assert_eq!(
            out.enter_service_s, 0.0,
            "an out-of-window step accrues nothing"
        );

        // The first in-window step completes the zero-length delay.
        let out = one_step(&mut ctrl, 1.0, 60.0, 900.0, 2700.0);
        assert!(
            !out.tripped,
            "inside the windows a zero delay completes on the first step"
        );
        assert_eq!(out.p_ref_w, 2700.0);
    }

    /// The enter-service windows gate the delay: a restored bus outside
    /// them (0.8 pu, still inside the ride-through band) does not count
    /// toward reconnection.
    #[test]
    fn reconnection_counts_only_inside_the_enter_service_windows() {
        let mut ctrl = controller(&cat2(), 5000.0);
        let trip = one_step(&mut ctrl, 0.6, 60.0, 60.0, 2700.0);
        assert!(trip.tripped);

        // 0.8 pu is above UV1's 0.70 (no fresh trip) but below the
        // enter-service window's 0.917: the latch holds and the delay
        // resets.
        let out = one_step(&mut ctrl, 0.8, 60.0, 900.0, 2700.0);
        assert!(out.tripped, "the latch holds on an out-of-window bus");
        assert_eq!(out.p_ref_w, 0.0, "the latch holds: no output");
        assert_eq!(out.enter_service_s, 0.0, "out of window: reset");

        // Restoration inside the windows: the delay runs from zero.
        let out = one_step(&mut ctrl, 1.0, 60.0, 900.0, 2700.0);
        assert!(!out.tripped);
        assert_eq!(out.p_ref_w, 2700.0, "900 s clears the 300 s delay at once");
    }

    /// A chattering grid restarts the delay: the enter-service
    /// accumulation is in-window time, and an out-of-window step (a sag
    /// again before the delay completed) spends it. The restored stretch
    /// must then hold the whole delay from zero.
    #[test]
    fn a_sag_inside_the_delay_restarts_the_enter_service_accumulation() {
        let mut ctrl = controller(&cat2(), 5000.0);
        let trip = one_step(&mut ctrl, 0.6, 60.0, 60.0, 2700.0);
        assert!(trip.tripped);

        // Two restored steps accumulate 120 s, then the grid sags again.
        for step in 1..=2 {
            let out = one_step(&mut ctrl, 1.0, 60.0, 60.0, 2700.0);
            assert!(out.tripped, "step {step}: the delay has not completed");
            assert_eq!(out.enter_service_s, 60.0 * step as f64);
        }
        let sag_again = one_step(&mut ctrl, 0.6, 60.0, 60.0, 2700.0);
        assert!(sag_again.tripped, "the latch holds through the second sag");
        assert_eq!(
            sag_again.enter_service_s, 0.0,
            "the out-of-window step spends the accumulation: the delay \
             restarts, it does not resume"
        );

        // The restored stretch must hold the whole delay from zero: four
        // 60 s steps ride offline, the fifth reconnects.
        let mut out = sag_again;
        for step in 1..=5 {
            out = one_step(&mut ctrl, 1.0, 60.0, 60.0, 2700.0);
            assert_eq!(out.tripped, step < 5, "step {step}");
        }
        assert_eq!(out.p_ref_w, 2700.0, "the reconnected step produces");
    }

    /// The reconnection re-acquires the droop anchor: the trip cleared
    /// the pre-disturbance latch, so the first excursion after the return
    /// to service ramps from the output the reconnected inverter
    /// delivers, not from the spent trip's anchor (a stale anchor would
    /// ramp from a higher output than the pre-disturbance one: the wrong
    /// direction for an over-frequency droop). The reconnected step
    /// itself produces the model's own output.
    #[test]
    fn reconnection_reacquires_the_droop_anchor() {
        let mut ctrl = controller(&cat2(), 5000.0);
        // 0.9 pu at nominal, then the sag trips (60 s clears UV1's 10 s
        // window).
        let _ = one_step(&mut ctrl, 1.0, 60.0, 60.0, 4500.0);
        let trip = one_step(&mut ctrl, 0.6, 60.0, 60.0, 4500.0);
        assert!(trip.tripped);

        // Five restored 60 s steps complete the 300 s delay; the model's
        // output is 2500 W by the reconnection.
        let mut out = trip;
        for step in 1..=5 {
            out = one_step(&mut ctrl, 1.0, 60.0, 60.0, 2500.0);
            if step < 5 {
                assert!(out.tripped, "step {step}: the delay is still running");
            }
        }
        assert!(!out.tripped, "the fifth restored step reconnects");
        assert_eq!(
            out.p_ref_w, 2500.0,
            "the reconnected step produces the model's own output"
        );

        // The first post-reconnection excursion ramps from the
        // reconnected 0.5 pu operating point: 0.5 Hz beyond the 60.036 Hz
        // edge at 1/3 pu per Hz is 1/6 pu down. A stale pre-trip anchor
        // (0.9 pu) would put the reference at 3666.67 W, above the
        // pre-disturbance output.
        let excursion = one_step(&mut ctrl, 1.0, 60.536, 60.0, 2500.0);
        let expected_w = (0.5 - 1.0 / 6.0) * 5000.0;
        assert!(
            (excursion.p_ref_w - expected_w).abs() < 1e-9,
            "the ramp anchors on the reconnected operating point: expected \
             {expected_w} W, got {} W",
            excursion.p_ref_w
        );
    }

    /// A utility outage resets the trip timers and the droop latch but
    /// keeps a latched trip; the islanded bus's nominal voltage counts
    /// toward the enter-service delay.
    #[test]
    fn outage_resets_timers_and_keeps_the_trip_latch() {
        let mut ctrl = controller(&cat2(), 5000.0);
        let _ = one_step(&mut ctrl, 0.6, 60.0, 5.0, 2700.0); // 5 s into UV1

        // The outage step: timers reset, the inverter is not tripped by
        // the layer (the caller's dead-bus gate handles the power).
        let out = ctrl
            .step(0.0, 60.0, 900.0, false, 2700.0, 2700.0, 0.0)
            .expect("valid step");
        assert!(!out.tripped);
        assert_eq!(out.p_ref_w, 2700.0, "the layer defers to the outage gate");
        assert_eq!(out.ride_through_s, 0.0, "the timers reset");

        // A fresh disturbance after the outage starts a fresh window.
        let ride = one_step(&mut ctrl, 0.6, 60.0, 5.0, 2700.0);
        assert!(!ride.tripped);

        // A latched trip survives the outage step (the caller's dead-bus
        // gate holds the power at zero either way), and the islanded bus
        // (1.0 pu, in the enter-service windows) accumulates the delay.
        let mut ctrl = controller(&cat2(), 5000.0);
        let _ = one_step(&mut ctrl, 0.4, 60.0, 900.0, 2700.0); // UV2 trips
        let out = ctrl
            .step(0.0, 60.0, 900.0, false, 2700.0, 2700.0, 0.0)
            .expect("valid step");
        assert!(out.tripped, "the latch holds through the outage step");
        assert_eq!(out.p_ref_w, 0.0);
        let islanded = ctrl
            .step(1.0, 60.0, 900.0, false, 2700.0, 2700.0, 0.0)
            .expect("valid step");
        assert!(!islanded.tripped, "900 s inside the windows reconnects");
        assert_eq!(islanded.p_ref_w, 2700.0, "the reconnected step produces");
    }

    /// The trip latch carries no running ride-through window: the firing
    /// step reports the window it reached (the clearing time is a
    /// maximum), and the latched inverter's per-disturbance timers are
    /// spent. The ride-through measurement is per excursion
    /// (`der1547_ride_through_s` "resets when the condition clears or the
    /// point trips"), so a mid-trip checkpoint carries no stale window
    /// either.
    #[test]
    fn the_trip_latch_carries_no_running_ride_through_window() {
        let mut ctrl = controller(&cat2(), 5000.0);
        let ride = one_step(&mut ctrl, 0.6, 60.0, 5.0, 2700.0);
        assert_eq!(ride.ride_through_s, 5.0);
        let trip = one_step(&mut ctrl, 0.6, 60.0, 5.0, 2700.0);
        assert!(trip.tripped);
        assert_eq!(
            trip.ride_through_s, 10.0,
            "the firing step reports the window it reached"
        );
        assert_eq!(
            ctrl.state().max_accumulator_s(),
            0.0,
            "the latched inverter's ride-through timers are spent"
        );
        assert_eq!(ctrl.state().uv1_s, 0.0);

        // The next step (still tripped) keeps the latch and reports no
        // window; the accumulation toward return-to-service is its own.
        let latched = one_step(&mut ctrl, 1.0, 60.0, 60.0, 2700.0);
        assert!(latched.tripped);
        assert_eq!(latched.ride_through_s, 0.0);
        assert_eq!(latched.enter_service_s, 60.0);
    }

    /// The fail-safe: a non-finite bus voltage or frequency de-energizes
    /// the inverter for the step without latching it. A poisoned grid
    /// state is not evidence about any table's condition, so the step
    /// takes no trip reason, spends no ride-through window, and accrues
    /// no enter-service time: the state carries unchanged and the next
    /// finite step resumes exactly where it stood.
    #[test]
    fn a_non_finite_bus_reading_de_energizes_the_step_without_latching() {
        // A latched controller with a part-spent enter-service wait: the
        // poisoned step de-energizes, names no table, and carries the
        // wait (60 s) unadvanced and unreset.
        let mut ctrl = controller(&cat2(), 5000.0);
        let _ = one_step(&mut ctrl, 0.6, 60.0, 60.0, 2700.0);
        let wait = one_step(&mut ctrl, 1.0, 60.0, 60.0, 2700.0);
        assert_eq!(wait.enter_service_s, 60.0);
        let poisoned = ctrl
            .step(1.0, f64::NAN, 60.0, true, 2700.0, 2700.0, 0.0)
            .expect("the fail-safe is not an error");
        assert!(poisoned.tripped, "the poisoned step is de-energized");
        assert_eq!(
            poisoned.trip_reason,
            trip_reason::NONE,
            "no table fired: the fail-safe names none"
        );
        assert_eq!(poisoned.p_ref_w, 0.0);
        assert_eq!(poisoned.ride_through_s, 0.0);
        assert_eq!(
            poisoned.enter_service_s, 60.0,
            "the wait neither advances (120) nor resets (0): it carries"
        );
        assert!(ctrl.state().tripped, "the fail-safe does not clear a latch");
        assert_eq!(
            ctrl.state().trip_reason,
            trip_reason::UV1,
            "the latch keeps its own reason; the step's outcome does not"
        );

        // A poisoned voltage on an untripped controller: the same
        // per-step de-energization, and a part-spent ride-through window
        // carried (not advanced into the trip, not spent by it).
        let mut ctrl = controller(&cat2(), 5000.0);
        let _ = one_step(&mut ctrl, 0.6, 60.0, 5.0, 2700.0);
        let poisoned = ctrl
            .step(f64::NAN, 60.0, 60.0, true, 2700.0, 2700.0, 0.0)
            .expect("the fail-safe is not an error");
        assert!(poisoned.tripped);
        assert_eq!(poisoned.trip_reason, trip_reason::NONE);
        assert!(!ctrl.state().tripped, "no latch from a poisoned step");
        assert_eq!(
            ctrl.state().uv1_s,
            5.0,
            "the window carries: not advanced (10), not spent (0)"
        );
        let resume = one_step(&mut ctrl, 0.6, 60.0, 5.0, 2700.0);
        assert!(
            resume.tripped,
            "the next in-band step completes the carried window"
        );
        assert_eq!(resume.ride_through_s, 10.0);
    }

    // -- The state's checkpoint round trip. --

    #[test]
    fn state_round_trips_through_serde() {
        let mut ctrl = controller(&cat2(), 5000.0);
        let _ = one_step(&mut ctrl, 0.6, 60.0, 5.0, 2700.0);
        let _ = one_step(&mut ctrl, 1.0, 60.536, 900.0, 2700.0);

        let bytes = postcard::to_allocvec(&ctrl.state).expect("serializable state");
        let restored: Der1547State = postcard::from_bytes(&bytes).expect("decodable state");
        assert_eq!(restored, ctrl.state);

        let mut other = controller(&cat2(), 5000.0);
        other.restore_state(restored);
        // The restored controller reproduces the same step.
        let a = one_step(&mut ctrl, 1.0, 60.536, 900.0, 2700.0);
        let b = one_step(&mut other, 1.0, 60.536, 900.0, 2700.0);
        assert_eq!(a, b);
    }

    // -- The unconfigured layer's excursion detector. --

    #[test]
    fn excursion_detector_matches_the_widest_watch_band() {
        // Inside every category's continuous band: no excursion.
        assert!(!Der1547Controller::excursion_present(1.0, 60.0));
        assert!(!Der1547Controller::excursion_present(0.95, 60.0));
        assert!(!Der1547Controller::excursion_present(1.05, 59.0));

        // Below Cat III's UV1 (0.88): the widest undervoltage watch.
        assert!(Der1547Controller::excursion_present(0.87, 60.0));
        assert!(Der1547Controller::excursion_present(0.5, 60.0));
        // Above OV1 (1.10).
        assert!(Der1547Controller::excursion_present(1.11, 60.0));
        // Frequency: below UF1's 58.5, above OF1's 61.2.
        assert!(Der1547Controller::excursion_present(1.0, 58.4));
        assert!(Der1547Controller::excursion_present(1.0, 61.3));
        // 58.5 Hz exactly is not below 58.5; 61.2 exactly is not above it.
        assert!(!Der1547Controller::excursion_present(1.0, 58.5));
        assert!(!Der1547Controller::excursion_present(1.0, 61.2));
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = cat2();
        let json = serde_json::to_string(&settings).expect("serializable");
        assert_eq!(json, r#"{"category":"II"}"#);
        let decoded: Der1547Settings = serde_json::from_str(&json).expect("decodable");
        assert_eq!(decoded, settings);

        let with_overrides = Der1547Settings {
            category: Der1547Category::CatIII,
            overrides: Some(Der1547Overrides {
                uv1_s: Some(50.0),
                ..Default::default()
            }),
        };
        let json = serde_json::to_string(&with_overrides).expect("serializable");
        let decoded: Der1547Settings = serde_json::from_str(&json).expect("decodable");
        assert_eq!(decoded, with_overrides);

        // Unknown keys are configuration errors.
        let err = serde_json::from_str::<Der1547Settings>(r#"{"category":"II","uv1_s":5.0}"#)
            .expect_err("a top-level unknown key is an error");
        assert!(err.to_string().contains("unknown"), "{err}");
        serde_json::from_str::<Der1547Settings>(r#"{"category":"IV"}"#)
            .expect_err("an unknown category is an error");
    }
}
