//! Zero-cost step observer for deep simulation debugging.
//!
//! All types and the buffer are gated by `#[cfg(feature = "observe")]` at the
//! module level -- when the feature is off, this module does not exist and the
//! compiler eliminates every observation site in `run_timestep`.

use std::collections::VecDeque;

use chrono::{DateTime, FixedOffset};
use hares_control::{DispatchTarget, PriorityTier};
use hares_envelope::EnvelopeComponentGains;
use hares_types::{
    ControlSignal, DomainUpdate, EndUse, FluidType, FuelType, LoopId, PortDeclaration, Telemetry,
    ZoneId,
};

/// Complete snapshot of a single simulation timestep, populated incrementally
/// at phase boundaries within `Dwelling::run_timestep`.
#[derive(Debug, Clone)]
pub struct StepSnapshot {
    pub step_index: u64,
    pub timestamp: DateTime<FixedOffset>,
    pub phases: PhaseSnapshots,
    /// Number of actors skipped this step due to interest filtering.
    pub actor_skips: usize,
    /// Number of actors whose `decide()` was called this step.
    pub actor_calls: usize,
}

/// Incrementally populated captures for each phase of `run_timestep`.
#[derive(Debug, Clone, Default)]
pub struct PhaseSnapshots {
    pub post_environment: Option<EnvironmentCapture>,
    pub post_dispatch: Option<DispatchCapture>,
    pub post_nonthermal_equipment: Option<EquipmentPhaseCapture>,
    pub post_thermal_equipment: Option<EquipmentPhaseCapture>,
    pub post_solvers: Option<SolverCapture>,
    pub post_zone_update: Option<ZoneUpdateCapture>,
}

/// Weather and zone state after environment update.
#[derive(Debug, Clone)]
pub struct EnvironmentCapture {
    pub outdoor_temp_c: f64,
    pub ghi_w_m2: f64,
    pub dni_w_m2: f64,
    pub dhi_w_m2: f64,
    pub solar_altitude_deg: f64,
    pub solar_azimuth_deg: f64,
    pub wind_speed_m_s: f64,
    pub mains_temp_c: f64,
    pub ground_temp_c: f64,
    pub sky_temp_c: f64,
    pub zone_temps_c: Vec<(ZoneId, f64)>,
    pub zone_humidity_ratios: Vec<(ZoneId, f64)>,
    pub solar_irradiance: Vec<hares_types::SurfaceIrradiance>,
    /// Whether the EPW HOLIDAYS/DAYLIGHT SAVINGS header permits leap year
    /// observation. Downstream tooling can use this to flag mismatches between
    /// file content and explicit header declarations.
    pub wf_allows_leap_years: bool,
}

/// Equipment telemetry + accumulated port state after an equipment phase.
#[derive(Debug, Clone)]
pub struct EquipmentPhaseCapture {
    pub equipment: Vec<EquipmentObservation>,
    pub ports: PortsCapture,
}

/// Per-equipment observation: identity, telemetry, port declarations, and contributions.
#[derive(Debug, Clone)]
pub struct EquipmentObservation {
    pub name: String,
    pub equipment_type: String,
    pub end_use: EndUse,
    pub telemetry: Telemetry,
    /// The port declarations this equipment registered at init.
    pub port_declarations: Vec<PortDeclaration>,
    /// What this equipment contributed to the shared port accumulators during its `step()`.
    pub contribution: EquipmentContribution,
    /// Snapshot of the aggregate port accumulators visible to this equipment before its `step()`.
    pub pre_step_ports: PortsCapture,
    /// Whether zone_id was explicitly set in config (true) or fell back to ZoneId(1) (false).
    pub zone_id_explicit: bool,
}

/// Per-equipment port contribution: the delta this equipment added to `PortSlots` during one step.
#[derive(Debug, Clone)]
pub struct EquipmentContribution {
    pub thermal: Vec<(ZoneId, f64, f64)>,
    pub electrical_load_kw: f64,
    pub electrical_gen_kw: f64,
    pub electrical_reactive_kvar: f64,
    pub fuel_consumption_w: Vec<(FuelType, f64)>,
    pub fluid: Vec<FluidContributionCapture>,
}

/// Per-equipment fluid contribution for a single loop, back-calculated from accumulator diffs.
#[derive(Debug, Clone)]
pub struct FluidContributionCapture {
    pub loop_id: LoopId,
    pub fluid_type: FluidType,
    pub delta_flow_kg_s: f64,
    pub supply_temp_c: f64,
    pub return_temp_c: f64,
}

/// Snapshot of all port accumulators at a phase boundary.
#[derive(Debug, Clone)]
pub struct PortsCapture {
    pub thermal: Vec<(ZoneId, f64, f64)>,
    pub electrical_load_kw: f64,
    pub electrical_gen_kw: f64,
    pub electrical_reactive_kvar: f64,
    pub fuel_consumption_w: Vec<(FuelType, f64)>,
    pub fluid: Vec<FluidPortCapture>,
}

/// Snapshot of a single fluid port accumulator.
#[derive(Debug, Clone)]
pub struct FluidPortCapture {
    pub loop_id: LoopId,
    pub fluid_type: FluidType,
    pub total_flow_kg_s: f64,
    pub mean_supply_temp_c: f64,
    pub mean_return_temp_c: f64,
    /// Sum of declared thermal_power_w values from all contributors to this loop.
    pub total_thermal_power_w: f64,
}

/// Domain solver outputs + envelope component gains.
#[derive(Debug, Clone)]
pub struct SolverCapture {
    pub thermal_update: DomainUpdate,
    pub humidity_update: DomainUpdate,
    pub electrical_update: DomainUpdate,
    pub fluid_update: DomainUpdate,
    pub envelope_gains: EnvelopeComponentGains,
    /// ZIP load scale applied by the electrical solver: `Z·V² + I·V + P`.
    pub zip_load_scale: f64,
    /// Raw `load_power_w` from port accumulator (before ZIP adjustment).
    pub port_load_raw_kw: f64,
    /// `load_power_w * zip_load_scale` (ZIP-adjusted port load).
    pub port_load_adjusted_kw: f64,
    /// Corrected electrical balance residual after applying ZIP scaling
    /// to both solver and port sides: `|net_active_kw() + port_net|`.
    pub residual_kw: f64,
}

/// Final zone state after thermal + humidity updates are applied.
#[derive(Debug, Clone)]
pub struct ZoneUpdateCapture {
    pub zone_temps_c: Vec<(ZoneId, f64)>,
    pub zone_humidity_ratios: Vec<(ZoneId, f64)>,
}

/// Capture of all dispatched control signals and their resolution.
#[derive(Debug, Clone)]
pub struct DispatchCapture {
    /// All signals dispatched this timestep, in tier order (low → high).
    pub signals: Vec<DispatchedSignal>,
    /// Same-tier conflicts detected this timestep: two or more dispatch
    /// requests at the same priority tier targeting the same equipment.
    /// Captured before draining so that losing signals are recorded even
    /// though only the last-queued signal in each conflict pair is applied.
    pub same_tier_conflicts: Vec<SameTierConflict>,
}

/// One dispatched control signal with its resolution outcome.
#[derive(Debug, Clone)]
pub struct DispatchedSignal {
    pub target: DispatchTarget,
    pub signal: ControlSignal,
    pub priority: PriorityTier,
    /// Whether this signal overwrote an earlier lower-priority signal to the same target.
    pub overwrote_earlier: bool,
    /// Whether the target equipment was found.
    pub delivered: bool,
}

/// A same-tier conflict: two dispatch requests at the same priority tier
/// target the same equipment, making the outcome dependent on FIFO order
/// (last-write-wins).
#[derive(Debug, Clone)]
pub struct SameTierConflict {
    pub tier: PriorityTier,
    pub target: DispatchTarget,
    /// All signals in the conflict, in FIFO order. The last signal is the
    /// winning signal (applied last via overwrite-safe `apply_control`).
    pub signals: Vec<ControlSignal>,
}

/// Ring buffer of step snapshots with configurable capacity.
#[derive(Debug, Clone)]
pub struct ObserverBuffer {
    snapshots: VecDeque<StepSnapshot>,
    capacity: usize,
}

impl ObserverBuffer {
    /// Creates a new buffer that retains up to `capacity` snapshots.
    ///
    /// # Panics
    /// Panics if `capacity` is zero.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "ObserverBuffer capacity must be > 0");
        Self {
            snapshots: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Pushes a snapshot, evicting the oldest if at capacity.
    pub fn push(&mut self, snapshot: StepSnapshot) {
        if self.snapshots.len() == self.capacity {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back(snapshot);
    }

    /// Drains all snapshots out of the buffer.
    pub fn drain(&mut self) -> Vec<StepSnapshot> {
        self.snapshots.drain(..).collect()
    }

    /// Returns a reference to the most recent snapshot, if any.
    #[must_use]
    pub fn last(&self) -> Option<&StepSnapshot> {
        self.snapshots.back()
    }

    /// Returns a slice-like view of all buffered snapshots.
    #[must_use]
    pub fn snapshots(&self) -> &VecDeque<StepSnapshot> {
        &self.snapshots
    }

    /// Number of snapshots currently buffered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    /// Whether the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }
}
