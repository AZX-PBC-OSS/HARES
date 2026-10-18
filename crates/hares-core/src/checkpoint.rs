//! Simulation checkpoint schema and file I/O.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::checksum;
use hares_envelope::ThermalSnapshot;
use hares_types::{ElectricalSummary, HaresError, ZoneId};
use serde::{Deserialize, Serialize};

/// Bump whenever checkpoint schema or state encoding changes. The test
/// `checkpoint_shape_is_pinned_to_its_version` pins a fingerprint of the
/// type-derived serde schema to this version, so a schema change fails it
/// until the version is bumped and the fingerprint re-pinned.
///
/// v7: `actor_states` entries became [`ActorStateCheckpoint`] records
/// carrying a per-actor schema version, and `EvDriverSnapshot` gained the
/// `drive_cancelled` counter in the same change — checkpoints written by
/// v6 builds are rejected by the version gate in [`DwellingCheckpoint::load`].
///
/// v8: `equipment_states` entries became [`EquipmentStateCheckpoint`]
/// records carrying the equipment's name and id, and restore validates
/// both against the live equipment — checkpoints written by v7 builds
/// (positionally-indexed opaque blobs) are rejected by the version gate.
///
/// v10 (the fold of the two v9-claiming branches): event-load equipment
/// state records its random stream (key and stream nonce) instead of the
/// key alone, and the thermal solver's state became one
/// [`ThermalSnapshot`] record, which carries the last step's coupling
/// terms the ideal-capacity solve reads, the ventilation recovery
/// effectiveness the next step reads, and the ideal-capacity failure
/// counts and last-good capacities its degraded fallback reads; the
/// snapshot carries its own `schema_version`
/// ([`hares_envelope::THERMAL_SNAPSHOT_SCHEMA_VERSION`]), validated
/// independently of this constant.
///
/// v11: `next_ev_driver_stream` carries the built-in EV driver stream
/// cursor; checkpoints written by v10 builds are rejected by the version
/// gate.
///
/// v12: `tariff_state` carries the tariff evaluator's full mutable state
/// (the tariff, its horizon, the price-index position and the open billing
/// period with its accruals and demand-window history) as a versioned
/// JSON payload, for runs with a tariff attached, so a resumed dwelling
/// prices and bills the post-resume steps exactly as the continuous run;
/// a mid-run attached or replaced tariff bills from the period containing
/// the attach or switch step. Checkpoints written by v11 builds are
/// rejected by the version gate. The payload carries its own schema
/// version ([`hares_tariff::TARIFF_SNAPSHOT_SCHEMA_VERSION`]), validated
/// independently of this constant.
///
/// v14: the thermal snapshot carries the per-zone non-HVAC share of the
/// zone sensible input column the next ideal-capacity solve estimates from
/// (the `ThermalSnapshot`'s `non_hvac_zone_input_w`; the snapshot's own
/// schema version is unchanged at 1 because v1 was never released in a
/// build without the field).
/// Checkpoints written by v13 builds are rejected by the version gate.
///
/// v15: the checkpoint carries everything else the run needs to continue
/// seamlessly — the closed billing periods' summaries, the run metrics'
/// accumulator (present exactly when the run requested metrics), every
/// `Memory` report's row stream with its counters, the run health totals
/// and the warning record (both bounded buffers and their dropped
/// counts) — so a run checkpointed mid-year and resumed finishes bitwise
/// to the continuous run in every output surface. Checkpoints written by
/// v14 builds are rejected by the version gate.
///
/// v16: each report's row-stream section carries the chunk size the
/// writing run's recorder flushed at, and a restoring recorder built at
/// another chunk size is a typed refusal (the same rows would rebuild
/// into batches of different shapes, so the resumed frames would not be
/// the writing run's bitwise). Checkpoints written by v15 builds are
/// rejected by the version gate.
///
/// v17: the metrics section carries the time resolution it was written
/// at, and a restoring run at another is a typed refusal (the input plan
/// is resolution-free, and two distinct resolutions can size the rolling
/// peak's ring identically). Checkpoints written by v16 builds are
/// rejected by the version gate.
///
/// v19: the run health totals carry the cross-tier deadband event count
/// (the runtime collision the init's static deadband check cannot see),
/// the count's equipment-side counter in `EquipmentHealthCounts` beside
/// it. Checkpoints written by v18 builds are rejected by the version gate.
pub const CHECKPOINT_VERSION: u32 = 19;

/// One equipment's checkpointed state, identity-keyed.
///
/// `name` and `equipment_id` are recorded from the equipment's descriptor
/// at save time and both are validated on restore: state is matched to
/// equipment by name, and the id must agree. A spec reorder between save
/// and restore (or any other identity drift) is then a typed error naming
/// both sides — never state silently loading into the wrong equipment,
/// which is what the positional `zip` this record replaced could do (and
/// did, invisibly, for any reordered equipment set).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EquipmentStateCheckpoint {
    /// Equipment instance name, matched against `descriptor().name` on
    /// restore.
    pub name: String,
    /// Equipment id at save time, matched against `descriptor().id` on
    /// restore. Ids are deterministic from spec order, so an id mismatch
    /// is direct evidence the equipment set or its order changed between
    /// save and restore.
    pub equipment_id: u32,
    /// Consecutive failed steps of this equipment at save time.
    pub consecutive_step_failures: u32,
    /// Opaque state blob from `Equipment::save_state()`.
    pub blob: Vec<u8>,
}

/// One actor's checkpointed decision-state.
///
/// `schema_version` is recorded from
/// [`Actor::checkpoint_version`](crate::Actor::checkpoint_version) at save
/// time and validated against the live actor's version on restore, so an
/// actor whose snapshot schema changed is rejected at the checkpoint
/// boundary with a version-mismatch error naming the actor — the actor-side
/// counterpart of the equipment's per-equipment `checkpoint_version()` /
/// `load_versioned` gate (`hares-equipment/src/serial.rs`). Without it, an
/// actor snapshot could gain a field with no gate of its own: the only
/// protection was this file's `format_version`, so an actor-only schema
/// change surfaced as a raw postcard decode failure deep inside the
/// actor's `load_state` instead of as a version mismatch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActorStateCheckpoint {
    /// Actor name, matched against `Actor::name()` on restore.
    pub name: String,
    /// Schema version of `blob`, from `Actor::checkpoint_version()` at save
    /// time. Must equal the live actor's version on restore.
    pub schema_version: u32,
    /// Opaque actor state blob from `Actor::save_state()`. Actors with no
    /// mutable state contribute an empty blob.
    pub blob: Vec<u8>,
}

/// Serializable snapshot of all state required to resume a dwelling run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DwellingCheckpoint {
    pub format_version: u32,
    pub bldg_id: i64,
    pub timestep_index: u64,
    /// Per-equipment state, identity-keyed. Each entry carries the
    /// equipment's name and id alongside its opaque state blob; restore
    /// matches by name and validates the id before handing the blob to
    /// `Equipment::load_state` (see [`EquipmentStateCheckpoint`]).
    pub equipment_states: Vec<EquipmentStateCheckpoint>,
    pub rng_state: [u8; 32],
    /// The thermal solver's complete mutable state, as
    /// [`ThermalSolver::snapshot_state`](hares_envelope::ThermalSolver::snapshot_state)
    /// captures it.
    pub thermal: ThermalSnapshot,
    /// Per-zone humidity ratios. Each entry is `(ZoneId, humidity_ratio)`.
    pub humidity_states: Vec<(ZoneId, f64)>,
    pub fluid_states: Vec<f64>,
    pub rng_stream: u64,
    pub rng_word_pos: u128,
    /// Actor decision-state, one per registered actor. Each entry carries
    /// the actor's name, its snapshot schema version, and its opaque state
    /// blob; the version is validated on restore before the blob is handed
    /// to the actor (see [`ActorStateCheckpoint`]).
    pub actor_states: Vec<ActorStateCheckpoint>,
    /// Prior-step electrical summary (grid, PV, base load, battery, EV)
    /// so the first post-restore step sees the same env.electrical that
    /// the original continuous run would have at the same step index.
    pub prior_electrical_summary: ElectricalSummary,
    /// Offset of the next unused built-in EV driver RNG stream. A dwelling
    /// rebuilt to resume replays only the surviving roster, not its history,
    /// so without this a driver built after the resume could take the
    /// stream of a restored driver.
    pub next_ev_driver_stream: u64,
    /// The tariff evaluator's complete mutable state, when a tariff is
    /// attached: the tariff, the horizon, the price-index position, the
    /// open billing period and its accruals and demand-window history, as a
    /// versioned JSON payload ([`hares_tariff::TariffSnapshot`]). `None`
    /// when the run has no tariff, and a dwelling restored from such a
    /// checkpoint has none either. The payload carries its own schema
    /// version ([`hares_tariff::TARIFF_SNAPSHOT_SCHEMA_VERSION`]) validated
    /// on restore, independently of this constant (the [`ThermalSnapshot`]
    /// pattern).
    pub tariff_state: Option<Vec<u8>>,
    /// Every closed billing period's summary, in close order: a restored
    /// run's billing summaries continue the written run's. A section
    /// present without a tariff section is a typed restore error (the
    /// summaries' bounds resolve in the tariff's zone).
    pub billing_summaries: Vec<hares_tariff::BillingPeriodSummarySnapshot>,
    /// The run metrics' accumulator, exact at the checkpoint step, present
    /// exactly when the run requested metrics (`SimulationConfig::metrics`):
    /// a section without the request, or the request without a section, is
    /// a typed restore error naming the section.
    pub metrics: Option<hares_io::MetricsSnapshot>,
    /// One section per configured report, in `reports` order: the report's
    /// name and its recorder's row stream. A `Memory` report's rows restore
    /// so the resumed run's frame continues the written run's; rows the
    /// restoring dwelling recorded past the checkpoint step are discarded.
    pub reports: Vec<ReportSection>,
    /// The run's health totals: the counters continue across the restore.
    pub health: crate::health::RunHealth,
    /// The warning record: both bounded buffers and their dropped counts,
    /// so the restored run's `RunResult.warnings` and its caller's next
    /// drain continue the written run's.
    pub warnings: crate::dwelling::WarningLogSnapshot,
}

/// One report's checkpoint section: its name and its recorder's row
/// stream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReportSection {
    /// The report's name, matched against the restoring dwelling's
    /// configured reports.
    pub name: String,
    /// The recorder's row stream.
    pub rows: hares_io::ReportRowsSnapshot,
}

impl DwellingCheckpoint {
    /// Persist checkpoint atomically with SHA-256 integrity checksum.
    pub fn save(&self, path: &Path) -> Result<(), HaresError> {
        let result = (|| -> Result<(), HaresError> {
            let json_bytes = serde_json::to_vec(self)
                .map_err(|err| HaresError::Io(format!("checkpoint serialize failed: {err}")))?;

            let tmp_bytes = checksum::write_with_sha256(&json_bytes);
            let tmp_path = temp_checkpoint_path(path);
            fs::write(&tmp_path, &tmp_bytes)
                .map_err(|err| HaresError::Io(format!("checkpoint temp write failed: {err}")))?;
            fs::rename(&tmp_path, path)
                .map_err(|err| HaresError::Io(format!("checkpoint atomic rename failed: {err}")))?;
            Ok(())
        })();

        #[cfg(feature = "observe")]
        {
            match &result {
                Ok(()) => tracing::info!(
                    checkpoint_path = %path.display(),
                    "checkpoint saved successfully",
                ),
                Err(e) => tracing::error!(
                    checkpoint_path = %path.display(),
                    error = %e,
                    "checkpoint save failed",
                ),
            }
        }

        result
    }

    /// Deserialize a checkpoint from JSON bytes, gating on
    /// `format_version` before the schema-dependent parse.
    ///
    /// The version probe runs first because a checkpoint written by an
    /// older build may use a schema this build cannot parse (e.g. pre-v7
    /// tuple-shaped `actor_states`), and the actionable error for such a
    /// file is the version mismatch naming both versions — not a parse
    /// failure naming a field the reader cannot act on. Every
    /// deserialisation path (file load, the Python binding's byte-level
    /// save/restore) goes through here so the gate ordering has one home.
    pub fn deserialize_gated(json_bytes: &[u8]) -> Result<Self, HaresError> {
        #[derive(Deserialize)]
        struct FormatProbe {
            format_version: u32,
        }
        let probe: FormatProbe = serde_json::from_slice(json_bytes)
            .map_err(|err| HaresError::Io(format!("checkpoint parse failed: {err}")))?;
        if probe.format_version != CHECKPOINT_VERSION {
            return Err(HaresError::Io(format!(
                "checkpoint version mismatch: file={}, expected={}",
                probe.format_version, CHECKPOINT_VERSION
            )));
        }
        serde_json::from_slice(json_bytes)
            .map_err(|err| HaresError::Io(format!("checkpoint parse failed: {err}")))
    }

    /// Load checkpoint from disk, verifying SHA-256 integrity before
    /// deserialisation.
    pub fn load(path: &Path) -> Result<Self, HaresError> {
        let file_bytes = fs::read(path)
            .map_err(|err| HaresError::Io(format!("checkpoint read failed: {err}")))?;

        let json_bytes = checksum::verify_sha256(&file_bytes).map_err(|msg| {
            #[cfg(feature = "observe")]
            tracing::info!(
                checkpoint_path = %path.display(),
                sha256_pass = false,
                "checkpoint integrity check FAILED: {msg}",
            );
            HaresError::Io(format!("checkpoint file '{}' {msg}", path.display()))
        })?;

        #[cfg(feature = "observe")]
        tracing::info!(
            checkpoint_path = %path.display(),
            sha256_pass = true,
            "checkpoint integrity check passed",
        );

        Self::deserialize_gated(json_bytes)
    }
}

fn temp_checkpoint_path(path: &Path) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tid = std::thread::current().id();
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dwelling.chkpt");
    let tmp_name = format!("{file_name}.{nonce}.{tid:?}.tmp");
    path.with_file_name(tmp_name)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        ActorStateCheckpoint, CHECKPOINT_VERSION, DwellingCheckpoint, EquipmentStateCheckpoint,
        ReportSection,
    };
    use hares_envelope::{THERMAL_SNAPSHOT_SCHEMA_VERSION, ThermalSnapshot};
    use hares_types::{ElectricalSummary, ZoneId};

    fn empty_thermal() -> ThermalSnapshot {
        ThermalSnapshot {
            schema_version: THERMAL_SNAPSHOT_SCHEMA_VERSION,
            x: vec![],
            last_u: vec![],
            lwr_t_prev_c: vec![],
            interior_surface_temps: vec![],
            interior_surface_prev_temps: vec![],
            last_coupling: vec![],
            sensible_recovery_efficiency: 0.0,
            latent_recovery_efficiency: 0.0,
            ideal_capacity_failure_counts: vec![],
            last_good_capacity_w: vec![],
            non_hvac_zone_input_w: vec![],
        }
    }

    fn checkpoint_path() -> (tempfile::TempDir, std::path::PathBuf) {
        crate::temp_file::temp_file("checkpoint.json")
    }

    /// A checkpoint with every field holding a distinct, non-default value.
    fn populated_checkpoint() -> DwellingCheckpoint {
        DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: 7,
            timestep_index: 12,
            equipment_states: vec![EquipmentStateCheckpoint {
                name: "EV".into(),
                equipment_id: 3,
                consecutive_step_failures: 1,
                blob: vec![1, 2, 3],
            }],
            rng_state: [42; 32],
            thermal: ThermalSnapshot {
                schema_version: THERMAL_SNAPSHOT_SCHEMA_VERSION,
                x: vec![1.0, 2.0],
                last_u: vec![0.1],
                lwr_t_prev_c: vec![15.0, 18.0],
                interior_surface_temps: vec![vec![20.0, 21.0], vec![22.0]],
                interior_surface_prev_temps: vec![vec![19.5, 20.5], vec![21.5]],
                last_coupling: vec![(0, 0.25, 3.5), (1, 0.0, -1.25)],
                sensible_recovery_efficiency: 0.34,
                latent_recovery_efficiency: 0.12,
                ideal_capacity_failure_counts: vec![(ZoneId(1), 2)],
                last_good_capacity_w: vec![(ZoneId(1), 5537.98)],
                non_hvac_zone_input_w: vec![(ZoneId(1), 412.5)],
            },
            humidity_states: vec![(ZoneId(1), 0.005)],
            fluid_states: vec![1.0, 2.0, 3.0],
            rng_stream: 3,
            rng_word_pos: 8,
            actor_states: vec![ActorStateCheckpoint {
                name: "test_actor".into(),
                schema_version: 1,
                blob: vec![1, 2, 3],
            }],
            prior_electrical_summary: ElectricalSummary::default(),
            next_ev_driver_stream: 0,
            tariff_state: None,
            billing_summaries: vec![hares_tariff::BillingPeriodSummarySnapshot {
                zone: "America/Denver".into(),
                period_start: hares_tariff::LocalInstant {
                    wall_clock: "2023-01-25T00:00:00".into(),
                    offset_seconds: -25200,
                },
                period_end: hares_tariff::LocalInstant {
                    wall_clock: "2023-02-01T00:00:00".into(),
                    offset_seconds: -25200,
                },
                energy_charge_usd: 12.345_678_9,
                demand_charge_usd: 3.5,
                fixed_charge_usd: 10.0,
                export_credit_usd: 1.25,
                net_bill_usd: 24.595_678_9,
                peak_demand_kw: 4.75,
                total_import_kwh: 103.25,
                total_export_kwh: 21.5,
            }],
            metrics: Some(hares_io::MetricsSnapshot {
                time_res_secs: 900,
                plan: hares_io::MetricsPlanSnapshot {
                    total_electric_power_slot: 0,
                    total_gas_power_slot: Some(1),
                    grid_power_slot: 0,
                    pv_power_slot: None,
                    end_use_columns: vec![("HVAC".to_string(), 2)],
                    setpoints: None,
                    hvac_capacity_pairs: vec![(3, 4)],
                    window_solar_w_slot: None,
                    infiltration_w_slot: None,
                    interior_lwr_w_slot: None,
                    internal_gains_w_slot: None,
                    opaque_conduction_w_slot: None,
                    window_conduction_w_slot: None,
                    internal_mass_w_slot: None,
                    forced_ventilation_w_slot: None,
                    natural_ventilation_w_slot: None,
                    duct_loss_w_slot: None,
                    hvac_heating_delivered_w_slot: None,
                    hvac_cooling_delivered_w_slot: None,
                    hvac_heating_kw_slot: Some(2),
                    hvac_cooling_kw_slot: None,
                    battery_kw_slot: None,
                },
                accumulators: hares_io::MetricsAccumulatorsSnapshot {
                    total_electric_energy_kwh: 14.829_037_328_432_975,
                    total_gas_energy_therms: 3.25,
                    total_consumption_kwh: 15.1,
                    total_pv_generation_kwh_abs: 2.5,
                    peak_import_kw: 4.25,
                    peak_export_kw: 1.75,
                    energy_by_end_use: BTreeMap::from([("HVAC".to_string(), 9.875)]),
                    peak_by_end_use: BTreeMap::from([
                        ("HVAC".to_string(), Some(4.25)),
                        ("Idle".to_string(), None),
                    ]),
                    comfort_step_count: 10.0,
                    unmet_step_count: 2.0,
                    rolling_peak: hares_io::MetricsRollingPeakSnapshot {
                        buffer: vec![1.0, 2.0, 3.0, 4.0],
                        head: 2,
                        count: 4,
                        peak_15min_kw: 3.5,
                        peak_30min_kw: 3.75,
                        peak_60min_kw: 4.0,
                    },
                    envelope_window_solar_wh: 1.5,
                    envelope_opaque_conduction_wh: 2.5,
                    envelope_window_conduction_wh: 3.5,
                    envelope_internal_mass_wh: 4.5,
                    envelope_interior_lwr_wh: 5.5,
                    envelope_infiltration_wh: 6.5,
                    envelope_ventilation_wh: 7.5,
                    envelope_hvac_heating_wh: 8.5,
                    envelope_hvac_cooling_wh: 9.5,
                    envelope_internal_gains_wh: 10.5,
                    envelope_duct_loss_wh: 11.5,
                    hvac_heating_electric_wh: 12.5,
                    hvac_cooling_electric_wh: 13.5,
                    battery_energy_in_kwh: 14.5,
                    battery_energy_out_kwh: 15.5,
                    total_row_count: 96,
                    setpoint_missing_row_count: 3,
                    nan_step_count: 1,
                },
            }),
            reports: vec![ReportSection {
                name: "frame".into(),
                rows: hares_io::ReportRowsSnapshot {
                    timestamps: vec![1_000, 2_000, 3_000, 4_000],
                    values: vec![1.5, -0.25, 2.5, -0.5, 3.5, 0.75, 4.5, -1.0],
                    valid: vec![true, true, true, true, true, false, true, false],
                    buffered_rows: 1,
                    batches_built: 3,
                    chunk_size: 4,
                },
            }],
            health: crate::health::RunHealth {
                port_rollbacks: 1,
                rejected_control_signals: 2,
                clamped_actions: 3,
                curve_index_clamps: 4,
                fluid_payload_absences: 5,
                evicted_actors: 6,
                cross_tier_deadband_events: 7,
                warmup: crate::health::WarmupOutcome::Disabled,
            },
            warnings: crate::dwelling::WarningLogSnapshot {
                run_record: vec![hares_types::Warning::new("test", "recorded warning")],
                run_record_dropped: 7,
                pending: vec![hares_types::Warning::new("test", "pending warning")],
                pending_dropped: 8,
            },
        }
    }

    /// The serde schema of `T` and every type it nests, derived from the
    /// types (not from a sample value), as canonical JSON: each container's
    /// field names in order, every primitive's exact type (`F64`, `U64`,
    /// `U128`, ...), tuple arity and element order, enum variant names and
    /// payloads, and `Option` inner types.
    fn type_schema<T: serde::Deserialize<'static>>() -> String {
        type_schema_with(|tracer| tracer.trace_simple_type::<T>().map(|(format, _)| format))
    }

    /// [`type_schema`] for a type that nests enums: `trace` traces each
    /// nested enum before the root so every variant is recorded.
    fn type_schema_with(
        trace: impl FnOnce(
            &mut serde_reflection::Tracer,
        ) -> serde_reflection::Result<serde_reflection::Format>,
    ) -> String {
        let mut tracer = serde_reflection::Tracer::new(serde_reflection::TracerConfig::default());
        let root = trace(&mut tracer).expect("trace the type");
        let registry = tracer.registry().expect("every traced type is complete");
        serde_json::to_string(&(root, registry)).expect("serialize the schema")
    }

    fn sha256_hex(text: &str) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// The checkpoint schema `PINNED_VERSION` reads, as the SHA-256 of
    /// [`type_schema_with`] over `DwellingCheckpoint`, its nested enums
    /// traced before the root so every variant is recorded.
    const PINNED_VERSION: u32 = 19;
    const PINNED_SCHEMA_SHA256: &str =
        "aff064cfac915a29ddef48afedca0e77fc5060932c2ee60f8d4cfb9b44d23ce0";

    /// The checkpoint's serde schema, including the embedded
    /// `ThermalSnapshot` and every type it nests, is the one pinned for the
    /// current `CHECKPOINT_VERSION`. Any field added, removed, renamed,
    /// reordered or retyped changes the schema, and the version gate is only
    /// sound if that change comes with a version bump: bump
    /// `CHECKPOINT_VERSION` and re-pin both constants together.
    #[test]
    fn checkpoint_shape_is_pinned_to_its_version() {
        let schema = type_schema_with(|tracer| {
            tracer
                .trace_simple_type::<crate::health::WarmupOutcome>()
                .map(|_| ())?;
            tracer
                .trace_simple_type::<hares_io::MetricsDeadbandSnapshot>()
                .map(|_| ())?;
            tracer
                .trace_simple_type::<DwellingCheckpoint>()
                .map(|(format, _)| format)
        });
        let fingerprint = sha256_hex(&schema);
        assert_eq!(
            fingerprint, PINNED_SCHEMA_SHA256,
            "the checkpoint schema changed: bump CHECKPOINT_VERSION and re-pin \
             PINNED_VERSION and PINNED_SCHEMA_SHA256 together. Actual fingerprint: \
             {fingerprint}. Schema:\n{schema}"
        );
        assert_eq!(
            CHECKPOINT_VERSION, PINNED_VERSION,
            "CHECKPOINT_VERSION changed: re-pin PINNED_SCHEMA_SHA256 for the new version"
        );
    }

    /// Each kind of change the tripwire must catch, as the field type before
    /// and after the change: the schemas differ for every pair, and a
    /// field's schema is part of its container's, so the pinned fingerprint
    /// fails on each.
    #[test]
    fn type_schema_distinguishes_every_kind_of_retype() {
        mod before {
            #[derive(serde::Deserialize)]
            pub enum Mode {
                Heat,
                Cool,
            }
        }
        mod after {
            #[derive(serde::Deserialize)]
            pub enum Mode {
                Heat,
                Off,
            }
        }

        for (change, before, after) in [
            ("f64 to u64", type_schema::<f64>(), type_schema::<u64>()),
            ("u32 to i64", type_schema::<u32>(), type_schema::<i64>()),
            (
                "tuple arity in a Vec element",
                type_schema::<Vec<(usize, f64, f64)>>(),
                type_schema::<Vec<(usize, f64)>>(),
            ),
            (
                "tuple element order in a Vec element",
                type_schema::<Vec<(usize, f64, f64)>>(),
                type_schema::<Vec<(f64, usize, f64)>>(),
            ),
            (
                "unit enum variant rename",
                type_schema::<before::Mode>(),
                type_schema::<after::Mode>(),
            ),
            (
                "Option inner type",
                type_schema::<Option<f64>>(),
                type_schema::<Option<u64>>(),
            ),
        ] {
            assert_ne!(before, after, "the schema must change on a {change}");
        }
    }

    #[test]
    fn save_then_load_round_trip() {
        let cp = populated_checkpoint();

        let (_dir, path) = checkpoint_path();
        cp.save(&path).unwrap();
        let loaded = DwellingCheckpoint::load(&path).unwrap();
        assert_eq!(loaded, cp);
        // The identity fields must survive the round-trip — they are what
        // restore validates against, so a serialization that dropped them
        // would silently regress restore to positional matching.
        assert_eq!(loaded.equipment_states[0].name, "EV");
        assert_eq!(loaded.equipment_states[0].equipment_id, 3);
        assert_eq!(loaded.equipment_states[0].blob, vec![1, 2, 3]);
    }

    #[test]
    fn multi_zone_humidity_round_trip() {
        let cp = DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: 42,
            timestep_index: 100,
            equipment_states: vec![],
            rng_state: [0; 32],
            thermal: empty_thermal(),
            humidity_states: vec![(ZoneId(1), 0.008), (ZoneId(2), 0.012), (ZoneId(3), 0.006)],
            fluid_states: vec![],
            rng_stream: 0,
            rng_word_pos: 0,
            actor_states: vec![],
            prior_electrical_summary: ElectricalSummary::default(),
            next_ev_driver_stream: 0,
            tariff_state: None,
            billing_summaries: Vec::new(),
            metrics: None,
            reports: Vec::new(),
            health: crate::health::RunHealth::default(),
            warnings: crate::dwelling::WarningLogSnapshot::default(),
        };

        let (_dir, path) = checkpoint_path();
        cp.save(&path).unwrap();
        let loaded = DwellingCheckpoint::load(&path).unwrap();
        assert_eq!(loaded.humidity_states.len(), 3);
        assert_eq!(loaded.humidity_states[0], (ZoneId(1), 0.008));
        assert_eq!(loaded.humidity_states[1], (ZoneId(2), 0.012));
        assert_eq!(loaded.humidity_states[2], (ZoneId(3), 0.006));
    }

    #[test]
    fn version_mismatch_rejected() {
        let cp = DwellingCheckpoint {
            format_version: 1, // intentionally wrong — older than CHECKPOINT_VERSION
            bldg_id: 1,
            timestep_index: 0,
            equipment_states: vec![],
            rng_state: [0; 32],
            thermal: empty_thermal(),
            humidity_states: vec![(ZoneId(1), 0.005)],
            fluid_states: vec![],
            rng_stream: 0,
            rng_word_pos: 0,
            actor_states: vec![],
            prior_electrical_summary: ElectricalSummary::default(),
            next_ev_driver_stream: 0,
            tariff_state: None,
            billing_summaries: Vec::new(),
            metrics: None,
            reports: Vec::new(),
            health: crate::health::RunHealth::default(),
            warnings: crate::dwelling::WarningLogSnapshot::default(),
        };

        let (_dir, path) = checkpoint_path();
        cp.save(&path).unwrap();

        let err = DwellingCheckpoint::load(&path).unwrap_err();
        assert!(
            err.to_string().contains("version mismatch"),
            "expected version mismatch error, got: {err}"
        );
    }

    /// A checkpoint written by the build one version behind is rejected by
    /// the version gate in `deserialize_gated`, naming both versions: the
    /// previous schema's sections cannot parse into this build's.
    #[test]
    fn previous_version_checkpoint_is_rejected() {
        let mut body: serde_json::Value =
            serde_json::to_value(populated_checkpoint()).expect("serialize the checkpoint");
        body["format_version"] = serde_json::json!(CHECKPOINT_VERSION - 1);
        let json_bytes = serde_json::to_vec(&body).expect("serialize the v-1 blob");

        let err = DwellingCheckpoint::deserialize_gated(&json_bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("checkpoint version mismatch")
                && msg.contains(&format!("file={}", CHECKPOINT_VERSION - 1))
                && msg.contains(&format!("expected={CHECKPOINT_VERSION}")),
            "a one-version-behind checkpoint must be rejected by the gate with both \
             versions named; got: {msg}"
        );
    }

    #[test]
    fn old_version_checkpoint_rejected_as_version_mismatch_not_parse_error() {
        // A checkpoint written by a pre-v7 build carries tuple-shaped
        // `actor_states` this build's schema cannot parse. The format-version
        // gate must reject such a file with a version mismatch naming both
        // versions — the actionable error — before any schema-dependent
        // deserialisation, not as a parse failure naming a field the reader
        // cannot act on.
        let body = serde_json::json!({
            "format_version": 6,
            "bldg_id": 1,
            "timestep_index": 0,
            "equipment_states": [],
            "rng_state": vec![0u8; 32],
            "envelope_state": [],
            "humidity_states": [[1, 0.005]],
            "fluid_states": [],
            "rng_stream": 0,
            "rng_word_pos": 0,
            "thermal_last_u": [],
            "lwr_t_prev_c": [],
            "interior_surface_temps": [],
            "interior_surface_prev_temps": [],
            // Pre-v7 shape: (name, blob) tuples, no schema_version.
            "actor_states": [["OldActor", [1, 2, 3]]],
            "prior_electrical_summary": {
                "pv_generation_kw": 0.0,
                "base_load_kw": 0.0,
                "net_grid_kw": 0.0,
                "battery_power_kw": 0.0,
                "ev_power_kw": 0.0,
                "actual_pv_kw": 0.0,
            },
        });
        let json_bytes = serde_json::to_vec(&body).expect("serialize test fixture");
        let file_bytes = crate::checksum::write_with_sha256(&json_bytes);

        let (_dir, path) = checkpoint_path();
        std::fs::write(&path, &file_bytes).expect("write test fixture");

        let err = DwellingCheckpoint::load(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("checkpoint version mismatch")
                && msg.contains("file=6")
                && msg.contains(&format!("expected={CHECKPOINT_VERSION}")),
            "a pre-v7 checkpoint must be rejected by the version gate with both versions named; got: {msg}"
        );
        assert!(
            !msg.contains("parse failed"),
            "the rejection must come from the version gate, not schema parsing; got: {msg}"
        );
    }

    /// A checkpoint from the schema immediately before
    /// `consecutive_step_failures` joined `EquipmentStateCheckpoint`
    /// (format_version 12, equipment states without the field) must be
    /// rejected by the version gate with both versions named. The body is
    /// otherwise exactly this build's schema, so a full parse that ran
    /// before the gate would report a missing field instead of the
    /// actionable version mismatch.
    #[test]
    fn v12_checkpoint_without_step_failures_rejected_by_the_version_gate() {
        let mut body: serde_json::Value =
            serde_json::to_value(populated_checkpoint()).expect("serialize the checkpoint");
        body["format_version"] = serde_json::json!(12);
        let mut removed = 0;
        for state in body["equipment_states"]
            .as_array_mut()
            .expect("equipment_states is an array")
            .iter_mut()
        {
            if state
                .as_object_mut()
                .expect("each equipment state is an object")
                .remove("consecutive_step_failures")
                .is_some()
            {
                removed += 1;
            }
        }
        assert!(
            removed > 0,
            "the fixture must carry the field for its removal to be the schema change under test"
        );
        let json_bytes = serde_json::to_vec(&body).expect("serialize the v12 blob");
        let file_bytes = crate::checksum::write_with_sha256(&json_bytes);

        let (_dir, path) = checkpoint_path();
        std::fs::write(&path, &file_bytes).expect("write the v12 blob");

        let err = DwellingCheckpoint::load(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("checkpoint version mismatch")
                && msg.contains("file=12")
                && msg.contains(&format!("expected={CHECKPOINT_VERSION}")),
            "a v12 checkpoint must be rejected by the version gate with both versions named; got: {msg}"
        );
        assert!(
            !msg.contains("parse failed"),
            "the rejection must come from the version gate, not schema parsing; got: {msg}"
        );
    }

    #[test]
    fn deserialize_gated_rejects_malformed_bytes_loudly() {
        // The Python byte-level restore paths call `deserialize_gated`
        // directly on raw caller-supplied bytes, with no SHA-256 pre-check.
        // Bytes that are not JSON at all must produce a typed
        // `checkpoint parse failed` error from the format probe — not a
        // panic and not a silently substituted default.
        let err = DwellingCheckpoint::deserialize_gated(b"this is not json").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("checkpoint parse failed"),
            "malformed bytes must fail loudly at the probe; got: {msg}"
        );

        // Valid JSON carrying the current format_version but a structurally
        // invalid body must likewise fail at the full parse — not be
        // misreported as a version mismatch (the versions agree).
        let body = serde_json::json!({
            "format_version": CHECKPOINT_VERSION,
            "bldg_id": "not-a-number",
        });
        let err = DwellingCheckpoint::deserialize_gated(
            &serde_json::to_vec(&body).expect("serialize test fixture"),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("checkpoint parse failed"),
            "a matching-version but invalid body must fail at the full parse; got: {msg}"
        );
        assert!(
            !msg.contains("version mismatch"),
            "the versions agree, so the failure must not be reported as a mismatch; got: {msg}"
        );
    }

    #[test]
    fn sha256_corruption_rejected() {
        let cp = DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: 1,
            timestep_index: 0,
            equipment_states: vec![],
            rng_state: [0; 32],
            thermal: empty_thermal(),
            humidity_states: vec![(ZoneId(1), 0.005)],
            fluid_states: vec![],
            rng_stream: 0,
            rng_word_pos: 0,
            actor_states: vec![],
            prior_electrical_summary: ElectricalSummary::default(),
            next_ev_driver_stream: 0,
            tariff_state: None,
            billing_summaries: Vec::new(),
            metrics: None,
            reports: Vec::new(),
            health: crate::health::RunHealth::default(),
            warnings: crate::dwelling::WarningLogSnapshot::default(),
        };

        let (_dir, path) = checkpoint_path();
        cp.save(&path).unwrap();

        // Corrupt one byte in the JSON body (after the `sha256:` prefix line)
        let mut file_bytes = std::fs::read(&path).unwrap();
        let nl_pos = file_bytes.iter().position(|&b| b == b'\n').unwrap();
        // Flip a bit in the JSON payload
        if nl_pos + 10 < file_bytes.len() {
            file_bytes[nl_pos + 5] ^= 0x01;
        }
        std::fs::write(&path, &file_bytes).unwrap();

        let err = DwellingCheckpoint::load(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("SHA-256") || msg.contains("checksum"),
            "expected SHA-256 checksum error, got: {msg}"
        );
    }

    #[test]
    fn prior_electrical_summary_round_trip() {
        let summary = ElectricalSummary {
            pv_generation_kw: 5.0,
            base_load_kw: 2.5,
            net_grid_kw: 1.0,
            battery_power_kw: 1.5,
            ev_power_kw: 3.0,
            actual_pv_kw: 4.5,
        };
        let cp = DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: 7,
            timestep_index: 12,
            equipment_states: vec![],
            rng_state: [0; 32],
            thermal: empty_thermal(),
            humidity_states: vec![(ZoneId(1), 0.005)],
            fluid_states: vec![],
            rng_stream: 0,
            rng_word_pos: 0,
            actor_states: vec![],
            prior_electrical_summary: summary.clone(),
            next_ev_driver_stream: 0,
            tariff_state: None,
            billing_summaries: Vec::new(),
            metrics: None,
            reports: Vec::new(),
            health: crate::health::RunHealth::default(),
            warnings: crate::dwelling::WarningLogSnapshot::default(),
        };

        let (_dir, path) = checkpoint_path();
        cp.save(&path).unwrap();
        let loaded = DwellingCheckpoint::load(&path).unwrap();
        assert_eq!(loaded.prior_electrical_summary, summary);
    }

    /// A checkpoint file returns every float bit for bit: 14.829037328432975
    /// is a value serde_json parses one unit in the last place off unless
    /// its exact round-trip parsing is enabled for every crate.
    #[test]
    fn checkpoint_file_round_trips_floats_exactly() {
        let state = 14.829_037_328_432_975_f64;
        let mut cp = populated_checkpoint();
        cp.thermal.x = vec![state];

        let (_dir, path) = checkpoint_path();
        cp.save(&path).unwrap();
        let loaded = DwellingCheckpoint::load(&path).unwrap();
        assert_eq!(loaded.thermal.x[0].to_bits(), state.to_bits());
    }

    /// A checkpoint written, read and written again is byte-identical:
    /// every section's floats read back with the bits they were written
    /// with (float_roundtrip parsing), so a resumed run holds the written
    /// run's state exactly.
    #[test]
    fn restore_round_trips_every_float_bitwise() {
        let cp = populated_checkpoint();

        let (_dir, path) = checkpoint_path();
        cp.save(&path).expect("save the checkpoint");
        let first = std::fs::read(&path).expect("read the checkpoint file");
        let loaded = DwellingCheckpoint::load(&path).expect("load the checkpoint");
        loaded.save(&path).expect("save the loaded checkpoint");
        let second = std::fs::read(&path).expect("read the rewritten file");
        assert_eq!(
            first, second,
            "the checkpoint file is byte-identical through a read and a rewrite"
        );
        assert_eq!(loaded, cp);
    }

    /// A checkpoint file's tariff state returns every float bit for bit:
    /// the blob re-encodes to the same bytes, so a resumed evaluator bills
    /// with exactly the accruals the interrupted run held.
    #[test]
    fn tariff_state_round_trips_through_the_file_bitwise() {
        use chrono::TimeZone;
        let start = chrono_tz::America::New_York
            .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
            .earliest()
            .expect("start");
        let mut tariff = hares_tariff::ElectricTariff::default();
        tariff.fixed_charges.monthly_usd = 10.0;
        let mut evaluator = hares_tariff::TariffEvaluator::new(
            tariff,
            start,
            start + chrono::Duration::hours(48),
            3600,
        )
        .expect("evaluator");
        for i in 0..30u32 {
            let step_end = start + chrono::Duration::hours(i as i64 + 1);
            evaluator.step(2.5, 0.0, 3600.0, step_end);
        }
        let blob = evaluator
            .snapshot_state()
            .to_blob()
            .expect("encode the snapshot");

        let cp = DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: 1,
            timestep_index: 30,
            equipment_states: vec![],
            rng_state: [0; 32],
            thermal: empty_thermal(),
            humidity_states: vec![],
            fluid_states: vec![],
            rng_stream: 0,
            rng_word_pos: 0,
            actor_states: vec![],
            prior_electrical_summary: ElectricalSummary::default(),
            next_ev_driver_stream: 0,
            tariff_state: Some(blob),
            billing_summaries: Vec::new(),
            metrics: None,
            reports: Vec::new(),
            health: crate::health::RunHealth::default(),
            warnings: crate::dwelling::WarningLogSnapshot::default(),
        };

        let (_dir, path) = checkpoint_path();
        cp.save(&path).expect("save the checkpoint");
        let loaded = DwellingCheckpoint::load(&path).expect("load the checkpoint");
        let saved_blob = cp.tariff_state.as_ref().expect("saved blob");
        let loaded_blob = loaded.tariff_state.as_ref().expect("loaded blob");
        assert_eq!(saved_blob, loaded_blob, "the tariff blob is byte-identical");
        let snapshot =
            hares_tariff::TariffSnapshot::from_blob(loaded_blob).expect("decode the loaded blob");
        assert_eq!(
            hares_tariff::TariffSnapshot::from_blob(saved_blob).expect("decode the saved blob"),
            snapshot
        );
        assert_eq!(snapshot.billing.steps_in_period, 30);
    }

    #[test]
    fn save_returns_err_on_non_writable_path() {
        let cp = DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: 1,
            timestep_index: 0,
            equipment_states: vec![],
            rng_state: [0; 32],
            thermal: empty_thermal(),
            humidity_states: vec![(ZoneId(1), 0.005)],
            fluid_states: vec![],
            rng_stream: 0,
            rng_word_pos: 0,
            actor_states: vec![],
            prior_electrical_summary: ElectricalSummary::default(),
            next_ev_driver_stream: 0,
            tariff_state: None,
            billing_summaries: Vec::new(),
            metrics: None,
            reports: Vec::new(),
            health: crate::health::RunHealth::default(),
            warnings: crate::dwelling::WarningLogSnapshot::default(),
        };

        let path = std::path::PathBuf::from("/nonexistent-dir-xyz/cp.json");
        let result = cp.save(&path);
        assert!(
            result.is_err(),
            "save() should return Err for a non-writable path, but returned Ok"
        );
    }
}
