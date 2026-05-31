//! Per-timestep diagnostic output for debugging thermal runaway and solver issues.
//!
//! Enable by setting `output_verbosity >= 4` in SimulationConfig.
//! Writes a CSV with one row per timestep containing zone temps, solver inputs/outputs,
//! equipment operating points, and port accumulations.

use std::io::Write;

use hares_types::{EnvironmentState, PortSlots, ZoneId};

use hares_physics::units::power_w_to_kw;

/// Collects diagnostic data for a single timestep.
#[derive(Debug, Default)]
pub struct StepDiagnostics {
    pub step: u64,
    pub timestamp_s: f64,
    pub outdoor_temp_c: f64,
    pub zone_temps_c: Vec<(ZoneId, f64)>,
    /// Total thermal port sensible gain per zone [W] (HVAC + appliances combined).
    pub thermal_gains_w: Vec<(ZoneId, f64)>,
    pub thermal_latent_w: Vec<(ZoneId, f64)>,
    pub electrical_net_kw: f64,
    pub equipment: Vec<EquipmentDiag>,
    /// Per-equipment sensible gain contributions to each zone.
    pub equipment_sensible_w: Vec<(String, ZoneId, f64)>,
    /// Envelope component gains from the thermal solver.
    pub envelope: Option<EnvelopeDiag>,
}

#[derive(Debug, Default, Clone)]
pub struct EnvelopeDiag {
    pub window_solar_w: f64,
    pub opaque_solar_lwr_w: f64,
    /// Total interior LWR exchange activity [W] (Σ|q_i|/2).
    pub interior_lwr_w: f64,
    /// Per-zone infiltration+ventilation sensible [W].
    pub infiltration_by_zone: Vec<(ZoneId, f64)>,
    /// Non-HVAC internal gains [W].
    pub internal_gain_w: f64,
    /// Total port convective [W] (HVAC + appliances).
    pub port_convective_w: f64,
    /// Total port radiant [W] (HVAC + appliances distributed to surfaces).
    ///
    /// ASHRAE HoF Ch. 18: internal gains have separate convective and radiant
    /// components; reporting both enables MRT diagnosis and BESTEST comparisons.
    /// EnergyPlus exposes `OtherEquipment Radiant Heating Rate [W]` as a
    /// separate output variable (I/O Ref v8.4, Internal Gains group).
    pub port_radiant_w: f64,
}

#[derive(Debug)]
pub struct EquipmentDiag {
    pub name: String,
    /// Zone mapping is populated at dwelling init via `write_equipment_init`,
    /// not per-step in `capture()`. Per-step zone routing is verified through
    /// the per-zone thermal gain columns in the diagnostic CSV.
    pub zone_id: Option<ZoneId>,
    pub mode: f64,
    pub electric_kw: f64,
    pub sensible_gain_w: f64,
}

/// Writes diagnostic CSV header.
pub fn write_header(w: &mut impl Write, n_zones: usize) {
    let mut cols = vec![
        "step".to_string(),
        "timestamp_s".to_string(),
        "outdoor_temp_c".to_string(),
    ];
    for i in 0..n_zones {
        cols.push(format!("zone{}_temp_c", i + 1));
        cols.push(format!("zone{}_thermal_gain_w", i + 1));
        cols.push(format!("zone{}_latent_gain_w", i + 1));
    }
    cols.push("electrical_net_kw".to_string());
    cols.push("port_radiant_w".to_string());
    cols.push("port_convective_w".to_string());
    cols.push("window_solar_w".to_string());
    cols.push("opaque_solar_lwr_w".to_string());
    cols.push("interior_lwr_w".to_string());
    cols.push("internal_gain_w".to_string());
    let _ = writeln!(w, "{}", cols.join(","));
}

/// Writes one row of diagnostic data.
pub fn write_row(w: &mut impl Write, d: &StepDiagnostics, n_zones: usize) {
    let mut vals: Vec<String> = vec![
        d.step.to_string(),
        format!("{:.1}", d.timestamp_s),
        format!("{:.2}", d.outdoor_temp_c),
    ];
    for i in 0..n_zones {
        let zone_id = ZoneId((i + 1) as u16);
        let temp = d
            .zone_temps_c
            .iter()
            .find(|(z, _)| *z == zone_id)
            .map(|(_, t)| *t)
            .unwrap_or(f64::NAN);
        let gain = d
            .thermal_gains_w
            .iter()
            .find(|(z, _)| *z == zone_id)
            .map(|(_, g)| *g)
            .unwrap_or(0.0);
        let latent = d
            .thermal_latent_w
            .iter()
            .find(|(z, _)| *z == zone_id)
            .map(|(_, l)| *l)
            .unwrap_or(0.0);
        vals.push(format!("{:.4}", temp));
        vals.push(format!("{:.1}", gain));
        vals.push(format!("{:.1}", latent));
    }
    vals.push(format!("{:.4}", d.electrical_net_kw));
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| format!("{:.1}", e.port_radiant_w))
            .unwrap_or_default(),
    );
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| format!("{:.1}", e.port_convective_w))
            .unwrap_or_default(),
    );
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| format!("{:.1}", e.window_solar_w))
            .unwrap_or_default(),
    );
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| format!("{:.1}", e.opaque_solar_lwr_w))
            .unwrap_or_default(),
    );
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| format!("{:.1}", e.interior_lwr_w))
            .unwrap_or_default(),
    );
    vals.push(
        d.envelope
            .as_ref()
            .map(|e| format!("{:.1}", e.internal_gain_w))
            .unwrap_or_default(),
    );
    let _ = writeln!(w, "{}", vals.join(","));
}

/// Write equipment zone-id mapping at init for diagnostic traceability.
///
/// Emitted when `output_verbosity >= 4` to surface the zone routing
/// resolved from HPXML for each HVAC equipment. The mapping is written
/// as a comment block after the CSV header so downstream tools can
/// verify per-zone thermal attribution without inspecting config internals.
pub fn write_equipment_init(w: &mut impl Write, equipment: &[(String, u16)]) {
    for (name, zone_id) in equipment {
        let _ = writeln!(w, "# eq zone_id: {name} -> ZoneId({zone_id})");
    }
}

/// Capture diagnostics from the current environment and port state.
pub fn capture(
    step: u64,
    env: &EnvironmentState,
    ports: &PortSlots,
    envelope: Option<EnvelopeDiag>,
) -> StepDiagnostics {
    let timestamp_s = env.current_time.timestamp() as f64;
    let zone_temps_c: Vec<(ZoneId, f64)> =
        env.zones.iter().map(|z| (z.id, z.temperature_c)).collect();
    let thermal_gains_w: Vec<(ZoneId, f64)> = ports
        .thermal
        .iter()
        .map(|t| (t.zone, t.sensible_gain_w))
        .collect();
    let thermal_latent_w: Vec<(ZoneId, f64)> = ports
        .thermal
        .iter()
        .map(|t| (t.zone, t.latent_gain_w))
        .collect();
    let electrical_net_kw = power_w_to_kw(ports.electrical.net_active_w());

    StepDiagnostics {
        step,
        timestamp_s,
        outdoor_temp_c: env.weather.outdoor_temp_c,
        zone_temps_c,
        thermal_gains_w,
        thermal_latent_w,
        electrical_net_kw,
        equipment: Vec::new(),
        equipment_sensible_w: Vec::new(),
        envelope,
    }
}
