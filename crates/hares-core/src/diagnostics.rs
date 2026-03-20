//! Per-timestep diagnostic output for debugging thermal runaway and solver issues.
//!
//! Enable by setting `output_verbosity >= 4` in SimulationConfig.
//! Writes a CSV with one row per timestep containing zone temps, solver inputs/outputs,
//! equipment operating points, and port accumulations.

use std::io::Write;

use hares_types::{EnvironmentState, PortSlots, ZoneId};

/// Collects diagnostic data for a single timestep.
#[derive(Debug, Default)]
pub struct StepDiagnostics {
    pub step: u64,
    pub timestamp_s: f64,
    pub outdoor_temp_c: f64,
    pub zone_temps_c: Vec<(ZoneId, f64)>,
    pub thermal_gains_w: Vec<(ZoneId, f64)>,
    pub thermal_latent_w: Vec<(ZoneId, f64)>,
    pub electrical_net_kw: f64,
    pub equipment: Vec<EquipmentDiag>,
}

#[derive(Debug)]
pub struct EquipmentDiag {
    pub name: String,
    pub mode: f64,
    pub electric_kw: f64,
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
    let _ = writeln!(w, "{}", vals.join(","));
}

/// Capture diagnostics from the current environment and port state.
pub fn capture(
    step: u64,
    env: &EnvironmentState,
    ports: &PortSlots,
) -> StepDiagnostics {
    let timestamp_s = env.current_time.timestamp() as f64;
    let zone_temps_c: Vec<(ZoneId, f64)> = env.zones.iter().map(|z| (z.id, z.temperature_c)).collect();
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
    let electrical_net_kw = ports.electrical.net_active_kw();

    StepDiagnostics {
        step,
        timestamp_s,
        outdoor_temp_c: env.weather.outdoor_temp_c,
        zone_temps_c,
        thermal_gains_w,
        thermal_latent_w,
        electrical_net_kw,
        equipment: Vec::new(),
    }
}
