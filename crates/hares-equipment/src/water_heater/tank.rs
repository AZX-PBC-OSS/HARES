//! Stratified tank thermal model shared by storage water heaters.

use std::{f64::consts::PI, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{Result, load_postcard, save_postcard};
use hares_types::HaresError;

use super::WATER_DENSITY_KG_PER_M3;

const MIN_NODES: usize = 1;
const MAX_NODES: usize = 12;
const WATER_SPECIFIC_HEAT_J_PER_KG_K: f64 = 4183.0;

/// Configuration for a stratified tank.
#[derive(Debug, Clone)]
pub struct StratifiedTankConfig {
    pub n_nodes: usize,
    pub height_m: f64,
    pub diameter_m: f64,
    pub ua_w_per_k: f64,
    pub conductivity_w_m_k: f64,
    pub initial_temp_c: f64,
    pub element_nodes: [Option<usize>; 2],
    pub node_volumes_m3: Option<Vec<f64>>,
    /// Additional UA (W/K) added to the top and bottom boundary nodes to account
    /// for flat end-cap heat losses. When `None`, defaults to `ua_w_per_k * 0.1`
    /// (approximately 10% of total UA for typical cylindrical aspect ratios).
    ///
    /// OCHRE Water.py:250-258: top and bottom nodes get additional parallel UA
    /// for flat end caps. Reference: EnergyPlus Engineering Reference, §14.8.
    pub ua_end_cap_w_per_k: Option<f64>,
}

/// Draw-step summary values used by water heater implementations and tests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawResult {
    /// Instantaneous outlet temperature at the beginning of the draw.
    pub outlet_temp_c: f64,
    /// Thermal energy removed from the tank by the draw [J].
    pub energy_out_j: f64,
    /// Thermal energy added to the tank by incoming mains water [J].
    pub energy_in_j: f64,
    /// Unmet load power [W]: heat that could not be delivered because outlet
    /// temperature was below the fixture setpoint. Zero when outlet >= setpoint.
    ///
    /// OCHRE Water.py:363: `h_unmet_load = max(draw_tempered / 60 * water_c *
    /// (tempered_draw_temp - outlet_temp), 0)`.
    pub unmet_load_w: f64,
}

/// Parameters that describe tempered (mixed) water draw behaviour.
///
/// OCHRE Water.py:305-325 — the mixing valve blends hot tank water with cold
/// mains water to reach a target fixture temperature.  For "hot" draws
/// (e.g. dishwasher) the target is `hot_draw_temp_c`; for fixture draws
/// (sink/shower/bath) the target is `tempered_draw_temp_c`.
#[derive(Debug, Clone, Copy)]
pub struct TemperedDrawConfig {
    /// Target delivery temperature for fixture draws [°C].
    /// Default: 40.6°C (≈ 105°F).
    pub tempered_draw_temp_c: f64,
    /// Target delivery temperature for "hot" draws (e.g. dishwasher) [°C].
    /// Default: 51.7°C (≈ 125°F).
    pub hot_draw_temp_c: f64,
    /// Tank setpoint [°C]; used to decide whether TMV logic applies at all.
    pub setpoint_temp_c: f64,
}

#[derive(Debug, Clone)]
pub struct StratifiedTank {
    height_m: f64,
    diameter_m: f64,
    ua_w_per_k: f64,
    conductivity_w_m_k: f64,
    node_volumes_m3: Vec<f64>,
    node_temps_c: Vec<f64>,
    node_edges_m3: Vec<f64>,
    total_volume_m3: f64,
    cross_section_area_m2: f64,
    node_height_m: f64,
    element_nodes: [Option<usize>; 2],
    /// Per-node UA (W/K). Boundary nodes include end-cap contribution.
    ua_per_node: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StratifiedTankState {
    node_temps_c: Vec<f64>,
}

impl StratifiedTank {
    pub fn new(config: StratifiedTankConfig) -> Result<Self> {
        validate_n_nodes(config.n_nodes)?;
        validate_positive("height_m", config.height_m)?;
        validate_positive("diameter_m", config.diameter_m)?;
        validate_nonnegative("ua_w_per_k", config.ua_w_per_k)?;
        validate_nonnegative("conductivity_w_m_k", config.conductivity_w_m_k)?;
        validate_finite("initial_temp_c", config.initial_temp_c)?;
        validate_element_nodes(config.n_nodes, config.element_nodes)?;

        let node_volumes_m3 = match config.node_volumes_m3 {
            Some(volumes) => {
                if volumes.len() != config.n_nodes {
                    return Err(HaresError::Equipment(format!(
                        "node_volumes_m3 length {} does not match n_nodes {}",
                        volumes.len(),
                        config.n_nodes
                    )));
                }
                for (idx, &volume_m3) in volumes.iter().enumerate() {
                    validate_positive(&format!("node_volumes_m3[{idx}]"), volume_m3)?;
                }
                volumes
            }
            None => {
                let total_volume_m3 = cylinder_volume(config.height_m, config.diameter_m);
                if config.n_nodes == 2 {
                    // OCHRE Water.py:475: TwoNodeWaterModel uses water_vol_fractions=[1/3, 2/3]
                    // giving the top (hot) node one-third and bottom (cold) node two-thirds.
                    vec![total_volume_m3 / 3.0, 2.0 * total_volume_m3 / 3.0]
                } else {
                    let node_volume_m3 = total_volume_m3 / config.n_nodes as f64;
                    vec![node_volume_m3; config.n_nodes]
                }
            }
        };

        let total_volume_m3 = node_volumes_m3.iter().sum::<f64>();
        validate_positive("total_volume_m3", total_volume_m3)?;

        let node_edges_m3 = build_node_edges(&node_volumes_m3);
        let cross_section_area_m2 = cylinder_cross_section_area(config.diameter_m);
        validate_positive("cross_section_area_m2", cross_section_area_m2)?;

        let node_height_m = config.height_m / config.n_nodes as f64;
        validate_positive("node_height_m", node_height_m)?;

        // Build per-node UA values from the uniform volume-fraction allocation,
        // then add end-cap UA to the top (index 0) and bottom (last index) nodes.
        // OCHRE Water.py:250-258: top/bottom nodes get additional parallel UA for
        // flat end caps. Reference: EnergyPlus Engineering Reference, §14.8.
        let ua_end = config.ua_end_cap_w_per_k.unwrap_or(config.ua_w_per_k * 0.1);
        let mut ua_per_node: Vec<f64> = node_volumes_m3
            .iter()
            .map(|&v| config.ua_w_per_k * v / total_volume_m3)
            .collect();
        ua_per_node[0] += ua_end;
        let last = config.n_nodes - 1;
        if last != 0 {
            ua_per_node[last] += ua_end;
        }

        Ok(Self {
            height_m: config.height_m,
            diameter_m: config.diameter_m,
            ua_w_per_k: config.ua_w_per_k,
            conductivity_w_m_k: config.conductivity_w_m_k,
            node_temps_c: vec![config.initial_temp_c; config.n_nodes],
            node_volumes_m3,
            node_edges_m3,
            total_volume_m3,
            cross_section_area_m2,
            node_height_m,
            element_nodes: config.element_nodes,
            ua_per_node,
        })
    }

    pub fn n_nodes(&self) -> usize {
        self.node_temps_c.len()
    }

    pub fn height_m(&self) -> f64 {
        self.height_m
    }

    pub fn diameter_m(&self) -> f64 {
        self.diameter_m
    }

    pub fn ua_w_per_k(&self) -> f64 {
        self.ua_w_per_k
    }

    pub fn element_nodes(&self) -> [Option<usize>; 2] {
        self.element_nodes
    }

    pub fn ua_per_node(&self) -> &[f64] {
        &self.ua_per_node
    }

    pub fn node_temps(&self) -> &[f64] {
        &self.node_temps_c
    }

    #[cfg(test)]
    pub fn node_temps_mut(&mut self) -> &mut [f64] {
        &mut self.node_temps_c
    }

    pub fn node_volumes_m3(&self) -> &[f64] {
        &self.node_volumes_m3
    }

    pub fn total_volume_m3(&self) -> f64 {
        self.total_volume_m3
    }

    /// Advance the tank by one time step with a raw (untempered) draw volume.
    ///
    /// `heat_injections` is a slice of `(node_index, power_w)` pairs representing
    /// element or condenser heat to inject during this step. Heat injection and
    /// draw are integrated in the same Euler step (matching OCHRE's single-step
    /// ODE integration). Outlet temperature is snapshotted from the **pre-heating**
    /// top-node value (OCHRE Water.py:284).
    ///
    /// Use [`step_tempered`] when the draw comes from a mixing-valve schedule
    /// that specifies a fixture delivery temperature.
    pub fn step(
        &mut self,
        ambient_temp_c: f64,
        draw_volume_m3: f64,
        mains_temp_c: f64,
        heat_injections: &[(usize, f64)],
        dt: Duration,
    ) -> Result<DrawResult> {
        validate_finite("ambient_temp_c", ambient_temp_c)?;
        validate_finite("mains_temp_c", mains_temp_c)?;
        validate_nonnegative("draw_volume_m3", draw_volume_m3)?;
        if draw_volume_m3 > self.total_volume_m3 {
            return Err(HaresError::Equipment(format!(
                "draw_volume_m3 {draw_volume_m3} exceeds tank volume {}",
                self.total_volume_m3
            )));
        }

        // OCHRE Water.py:284 — snapshot outlet from pre-step top-node temperature.
        let pre_step_outlet_temp_c = self.node_temps_c[0];

        self.apply_conduction_and_standby(ambient_temp_c, dt)?;
        // Snapshot post-conduction/pre-injection temps for energy accounting.
        // energy_out_j must reflect the water actually in the tank before element
        // heat is added, not the heated water.
        let pre_injection_temps = self.node_temps_c.clone();
        self.apply_heat_injections(heat_injections, dt)?;
        let mut draw = self.apply_draw(draw_volume_m3, mains_temp_c, &pre_injection_temps)?;
        draw.outlet_temp_c = pre_step_outlet_temp_c;
        self.mix_inversions();
        draw.unmet_load_w = 0.0;
        Ok(draw)
    }

    /// Advance the tank by one time step using a TMV-style tempered draw.
    ///
    /// `tempered_volume_m3_s` is the requested delivery flow rate [m³/s] at
    /// `tmv.tempered_draw_temp_c`.  The method computes the actual hot-water
    /// withdrawal from the tank after mixing-valve adjustment, and also reports
    /// the unmet-load power when the tank cannot meet the fixture temperature.
    ///
    /// OCHRE Water.py:305-363 — "tempered draw" logic.
    #[allow(clippy::too_many_arguments)]
    pub fn step_tempered(
        &mut self,
        ambient_temp_c: f64,
        tempered_flow_m3_s: f64,
        hot_flow_m3_s: f64,
        mains_temp_c: f64,
        heat_injections: &[(usize, f64)],
        tmv: TemperedDrawConfig,
        dt: Duration,
    ) -> Result<DrawResult> {
        validate_finite("ambient_temp_c", ambient_temp_c)?;
        validate_finite("mains_temp_c", mains_temp_c)?;
        validate_nonnegative("tempered_flow_m3_s", tempered_flow_m3_s)?;
        validate_nonnegative("hot_flow_m3_s", hot_flow_m3_s)?;

        // OCHRE Water.py:284 — snapshot outlet from pre-step top-node temperature.
        let outlet_est_c = self.node_temps_c[0];

        // --- TMV mixing-valve calculation (OCHRE Water.py:305-325) ---
        // For hot draws (e.g. dishwasher): reduce draw if outlet > hot_draw_temp_c.
        let hot_draw_volume_m3 = if tmv.setpoint_temp_c > tmv.hot_draw_temp_c {
            // Setpoint exceeds delivery target — tank water needs blending.
            if outlet_est_c <= tmv.hot_draw_temp_c {
                hot_flow_m3_s * dt.as_secs_f64()
            } else {
                let vol_ratio =
                    (tmv.hot_draw_temp_c - mains_temp_c) / (outlet_est_c - mains_temp_c).max(1e-9);
                hot_flow_m3_s * dt.as_secs_f64() * vol_ratio.clamp(0.0, 1.0)
            }
        } else {
            hot_flow_m3_s * dt.as_secs_f64()
        };

        // For fixture draws: reduce draw if outlet > tempered_draw_temp_c.
        let tempered_draw_volume_m3 = if tempered_flow_m3_s > 0.0 {
            if outlet_est_c <= tmv.tempered_draw_temp_c {
                tempered_flow_m3_s * dt.as_secs_f64()
            } else {
                let vol_ratio = (tmv.tempered_draw_temp_c - mains_temp_c)
                    / (outlet_est_c - mains_temp_c).max(1e-9);
                tempered_flow_m3_s * dt.as_secs_f64() * vol_ratio.clamp(0.0, 1.0)
            }
        } else {
            0.0
        };

        let total_draw_m3 = hot_draw_volume_m3 + tempered_draw_volume_m3;
        let clamped_draw = total_draw_m3.min(self.total_volume_m3);

        self.apply_conduction_and_standby(ambient_temp_c, dt)?;
        let pre_injection_temps = self.node_temps_c.clone();
        self.apply_heat_injections(heat_injections, dt)?;
        let mut draw = self.apply_draw(clamped_draw, mains_temp_c, &pre_injection_temps)?;
        // Override outlet with pre-heating snapshot.
        draw.outlet_temp_c = outlet_est_c;
        self.mix_inversions();

        // Unmet load: watts of heat the fixture didn't receive because outlet_temp < setpoint.
        // OCHRE Water.py:363: h_unmet_load = max(draw_tempered/60 * water_c * (t_fix - t_out), 0)
        // Here tempered_flow_m3_s is already in m³/s, so kg/s = flow_m3_s * density.
        const WATER_C_J_PER_KG_K: f64 = WATER_SPECIFIC_HEAT_J_PER_KG_K;
        let unmet_load_w = if tempered_flow_m3_s > 0.0 {
            let deficit = (tmv.tempered_draw_temp_c - draw.outlet_temp_c).max(0.0);
            tempered_flow_m3_s * WATER_DENSITY_KG_PER_M3 * WATER_C_J_PER_KG_K * deficit
        } else {
            0.0
        };
        draw.unmet_load_w = unmet_load_w;
        Ok(draw)
    }

    /// Apply a batch of heat injections to the tank.
    ///
    /// Each entry is `(node_index, power_w)`. Multiple injections to the same
    /// node are additive. Called by `step()` / `step_tempered()` so that element
    /// heating and draw occur within the same Euler step.
    fn apply_heat_injections(&mut self, injections: &[(usize, f64)], dt: Duration) -> Result<()> {
        let seconds = dt.as_secs_f64();
        if seconds == 0.0 {
            return Ok(());
        }
        for &(node, power_w) in injections {
            if power_w == 0.0 {
                continue;
            }
            validate_finite("heat_injection_power_w", power_w)?;
            let volume_m3 = self.node_volume(node)?;
            let delta_t_c = power_w * seconds
                / (WATER_DENSITY_KG_PER_M3 * volume_m3 * WATER_SPECIFIC_HEAT_J_PER_KG_K);
            self.node_temps_c[node] += delta_t_c;
        }
        Ok(())
    }

    /// Directly heat a single node. Intended for test setup only.
    ///
    /// Production code should pass heat injections through `step()` or
    /// `step_tempered()` so that heating and draw are integrated in the
    /// same Euler step with correct outlet temperature and energy accounting.
    pub fn heat_node(&mut self, node: usize, power_w: f64, dt: Duration) -> Result<()> {
        validate_finite("power_w", power_w)?;
        let seconds = dt.as_secs_f64();
        validate_nonnegative("dt_seconds", seconds)?;
        let volume_m3 = self.node_volume(node)?;
        let delta_t_c = power_w * seconds
            / (WATER_DENSITY_KG_PER_M3 * volume_m3 * WATER_SPECIFIC_HEAT_J_PER_KG_K);
        self.node_temps_c[node] += delta_t_c;
        Ok(())
    }

    pub fn mix_inversions(&mut self) -> usize {
        let mut layer_temps = Vec::<f64>::with_capacity(self.n_nodes());
        let mut layer_volumes = Vec::<f64>::with_capacity(self.n_nodes());
        let mut layer_counts = Vec::<usize>::with_capacity(self.n_nodes());
        let mut total_merges = 0usize;

        for (&temp_c, &volume_m3) in self.node_temps_c.iter().zip(self.node_volumes_m3.iter()) {
            layer_temps.push(temp_c);
            layer_volumes.push(volume_m3);
            layer_counts.push(1);

            while layer_temps.len() >= 2 {
                let lower = layer_temps.len() - 1;
                let upper = lower - 1;
                if layer_temps[upper] >= layer_temps[lower] {
                    break;
                }

                let merged_volume = layer_volumes[upper] + layer_volumes[lower];
                let merged_temp = (layer_temps[upper] * layer_volumes[upper]
                    + layer_temps[lower] * layer_volumes[lower])
                    / merged_volume;

                layer_temps[upper] = merged_temp;
                layer_volumes[upper] = merged_volume;
                layer_counts[upper] += layer_counts[lower];

                layer_temps.pop();
                layer_volumes.pop();
                layer_counts.pop();
                total_merges += 1;
            }
        }

        let mut output_idx = 0usize;
        for (layer_idx, &layer_temp_c) in layer_temps.iter().enumerate() {
            let count = layer_counts[layer_idx];
            for _ in 0..count {
                self.node_temps_c[output_idx] = layer_temp_c;
                output_idx += 1;
            }
        }

        // Verify the profile is monotone non-increasing (top ≥ bottom).
        #[cfg(debug_assertions)]
        {
            let n = self.node_temps_c.len();
            for i in 0..n.saturating_sub(1) {
                debug_assert!(
                    self.node_temps_c[i] >= self.node_temps_c[i + 1] - 1e-9,
                    "inversion mixing failed: node {} ({}) < node {} ({})",
                    i,
                    self.node_temps_c[i],
                    i + 1,
                    self.node_temps_c[i + 1]
                );
            }
        }

        total_merges
    }

    pub fn save_state(&self) -> Vec<u8> {
        save_postcard(&StratifiedTankState {
            node_temps_c: self.node_temps_c.clone(),
        })
    }

    pub fn load_state(&mut self, state: &[u8]) -> Result<()> {
        let decoded: StratifiedTankState = load_postcard(state)?;
        if decoded.node_temps_c.len() != self.n_nodes() {
            return Err(HaresError::Equipment(format!(
                "state node count {} does not match tank node count {}",
                decoded.node_temps_c.len(),
                self.n_nodes()
            )));
        }
        for (idx, &temp_c) in decoded.node_temps_c.iter().enumerate() {
            validate_finite(&format!("node_temps_c[{idx}]"), temp_c)?;
        }
        self.node_temps_c = decoded.node_temps_c;
        Ok(())
    }

    fn apply_conduction_and_standby(&mut self, ambient_temp_c: f64, dt: Duration) -> Result<()> {
        let seconds = dt.as_secs_f64();
        validate_nonnegative("dt_seconds", seconds)?;
        if seconds == 0.0 {
            return Ok(());
        }

        let old_temps_c = self.node_temps_c.clone();
        let mut delta_energy_j = vec![0.0_f64; self.n_nodes()];

        let conduction_w_per_k =
            self.conductivity_w_m_k * self.cross_section_area_m2 / self.node_height_m;
        for idx in 0..(self.n_nodes() - 1) {
            let heat_flow_w = conduction_w_per_k * (old_temps_c[idx + 1] - old_temps_c[idx]);
            let transfer_j = heat_flow_w * seconds;
            delta_energy_j[idx] += transfer_j;
            delta_energy_j[idx + 1] -= transfer_j;
        }

        for idx in 0..self.n_nodes() {
            let loss_w = self.ua_per_node[idx] * (old_temps_c[idx] - ambient_temp_c);
            delta_energy_j[idx] -= loss_w * seconds;
        }

        for (idx, node_temp_c) in self.node_temps_c.iter_mut().enumerate() {
            let thermal_mass_j_per_k = WATER_DENSITY_KG_PER_M3
                * self.node_volumes_m3[idx]
                * WATER_SPECIFIC_HEAT_J_PER_KG_K;
            *node_temp_c += delta_energy_j[idx] / thermal_mass_j_per_k;
        }

        Ok(())
    }

    /// Apply a draw to the tank, displacing water downward with mains water entering
    /// from the bottom. `energy_temps_c` provides the temperature profile used for
    /// computing `energy_out_j` — typically the pre-injection snapshot so that
    /// element heat does not inflate the reported energy removed by the draw.
    fn apply_draw(
        &mut self,
        draw_volume_m3: f64,
        mains_temp_c: f64,
        energy_temps_c: &[f64],
    ) -> Result<DrawResult> {
        let outlet_temp_c = self.node_temps_c[0];
        if draw_volume_m3 == 0.0 {
            return Ok(DrawResult {
                outlet_temp_c,
                energy_out_j: 0.0,
                energy_in_j: 0.0,
                unmet_load_w: 0.0,
            });
        }

        let old_temps_c = self.node_temps_c.clone();
        let top_segment_edges_m3 = [0.0, draw_volume_m3];
        // Use pre-injection temperatures for energy accounting so element heat
        // does not inflate the reported energy removed by the draw.
        let energy_out_j = WATER_DENSITY_KG_PER_M3
            * WATER_SPECIFIC_HEAT_J_PER_KG_K
            * segment_average_temp(
                &self.node_edges_m3,
                energy_temps_c,
                top_segment_edges_m3[0],
                top_segment_edges_m3[1],
            )
            * draw_volume_m3;
        let energy_in_j = WATER_DENSITY_KG_PER_M3
            * WATER_SPECIFIC_HEAT_J_PER_KG_K
            * mains_temp_c
            * draw_volume_m3;

        let mains_start_m3 = self.total_volume_m3 - draw_volume_m3;
        let mut new_temps_c = vec![0.0_f64; self.n_nodes()];
        for (idx, new_temp_c) in new_temps_c.iter_mut().enumerate().take(self.n_nodes()) {
            let target_start = self.node_edges_m3[idx];
            let target_end = self.node_edges_m3[idx + 1];
            let mut energy_volume_temp = 0.0_f64;

            for (src_idx, src_temp_c) in old_temps_c.iter().enumerate().take(self.n_nodes()) {
                let shifted_start = self.node_edges_m3[src_idx] - draw_volume_m3;
                let shifted_end = self.node_edges_m3[src_idx + 1] - draw_volume_m3;
                let overlap = overlap_length(target_start, target_end, shifted_start, shifted_end);
                if overlap > 0.0 {
                    energy_volume_temp += overlap * src_temp_c;
                }
            }

            let mains_overlap = overlap_length(
                target_start,
                target_end,
                mains_start_m3,
                self.total_volume_m3,
            );
            if mains_overlap > 0.0 {
                energy_volume_temp += mains_overlap * mains_temp_c;
            }

            let node_volume = self.node_volumes_m3[idx];
            *new_temp_c = energy_volume_temp / node_volume;
        }

        self.node_temps_c = new_temps_c;
        Ok(DrawResult {
            outlet_temp_c,
            energy_out_j,
            energy_in_j,
            unmet_load_w: 0.0,
        })
    }

    fn node_volume(&self, node: usize) -> Result<f64> {
        self.node_volumes_m3.get(node).copied().ok_or_else(|| {
            HaresError::Equipment(format!(
                "node index {node} out of bounds for {} nodes",
                self.n_nodes()
            ))
        })
    }
}

fn validate_n_nodes(n_nodes: usize) -> Result<()> {
    if !(MIN_NODES..=MAX_NODES).contains(&n_nodes) {
        return Err(HaresError::Equipment(format!(
            "n_nodes must be in [{MIN_NODES}, {MAX_NODES}], got {n_nodes}"
        )));
    }
    Ok(())
}

fn validate_element_nodes(n_nodes: usize, element_nodes: [Option<usize>; 2]) -> Result<()> {
    for (idx, maybe_node) in element_nodes.iter().enumerate() {
        if let Some(node) = maybe_node
            && *node >= n_nodes
        {
            return Err(HaresError::Equipment(format!(
                "element_nodes[{idx}]={node} out of bounds for {n_nodes} nodes"
            )));
        }
    }
    Ok(())
}

fn validate_positive(name: &str, value: f64) -> Result<()> {
    if !value.is_finite() || value <= 0.0 {
        return Err(HaresError::Equipment(format!(
            "{name} must be finite and > 0, got {value}"
        )));
    }
    Ok(())
}

fn validate_nonnegative(name: &str, value: f64) -> Result<()> {
    if !value.is_finite() || value < 0.0 {
        return Err(HaresError::Equipment(format!(
            "{name} must be finite and >= 0, got {value}"
        )));
    }
    Ok(())
}

fn validate_finite(name: &str, value: f64) -> Result<()> {
    if !value.is_finite() {
        return Err(HaresError::Equipment(format!("{name} must be finite")));
    }
    Ok(())
}

fn cylinder_cross_section_area(diameter_m: f64) -> f64 {
    PI * (diameter_m * 0.5).powi(2)
}

fn cylinder_volume(height_m: f64, diameter_m: f64) -> f64 {
    cylinder_cross_section_area(diameter_m) * height_m
}

fn build_node_edges(node_volumes_m3: &[f64]) -> Vec<f64> {
    let mut edges = Vec::with_capacity(node_volumes_m3.len() + 1);
    edges.push(0.0);
    let mut running = 0.0;
    for &volume in node_volumes_m3 {
        running += volume;
        edges.push(running);
    }
    edges
}

fn overlap_length(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

fn segment_average_temp(edges_m3: &[f64], temps_c: &[f64], start_m3: f64, end_m3: f64) -> f64 {
    let volume = end_m3 - start_m3;
    let mut energy = 0.0_f64;
    for idx in 0..temps_c.len() {
        let overlap = overlap_length(start_m3, end_m3, edges_m3[idx], edges_m3[idx + 1]);
        if overlap > 0.0 {
            energy += overlap * temps_c[idx];
        }
    }
    energy / volume
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        StratifiedTank, StratifiedTankConfig, WATER_DENSITY_KG_PER_M3,
        WATER_SPECIFIC_HEAT_J_PER_KG_K,
    };

    const EPSILON: f64 = 1.0e-9;

    fn test_tank(n_nodes: usize, initial_temp_c: f64) -> StratifiedTank {
        StratifiedTank::new(StratifiedTankConfig {
            n_nodes,
            height_m: 1.2,
            diameter_m: 0.5,
            ua_w_per_k: 0.0,
            conductivity_w_m_k: 0.0,
            initial_temp_c,
            element_nodes: [Some(0), Some(n_nodes - 1)],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })
        .expect("tank construction")
    }

    fn test_tank_with_ua(n_nodes: usize, ua_w_per_k: f64) -> StratifiedTank {
        StratifiedTank::new(StratifiedTankConfig {
            n_nodes,
            height_m: 1.2,
            diameter_m: 0.5,
            ua_w_per_k,
            conductivity_w_m_k: 0.0,
            initial_temp_c: 50.0,
            element_nodes: [Some(0), Some(n_nodes - 1)],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })
        .expect("tank construction")
    }

    fn total_energy_j(tank: &StratifiedTank) -> f64 {
        tank.node_temps()
            .iter()
            .zip(tank.node_volumes_m3().iter())
            .map(|(temp_c, volume_m3)| {
                WATER_DENSITY_KG_PER_M3 * WATER_SPECIFIC_HEAT_J_PER_KG_K * volume_m3 * temp_c
            })
            .sum()
    }

    #[test]
    fn single_node_heating_matches_analytic_delta_t() {
        let mut tank = test_tank(1, 50.0);
        let power_w = 4_500.0;
        let dt = Duration::from_secs(60);
        let mcp =
            WATER_DENSITY_KG_PER_M3 * tank.node_volumes_m3()[0] * WATER_SPECIFIC_HEAT_J_PER_KG_K;
        let expected_delta_t = power_w * dt.as_secs_f64() / mcp;

        tank.heat_node(0, power_w, dt).expect("heat top node");

        assert!((tank.node_temps()[0] - (50.0 + expected_delta_t)).abs() < 1.0e-12);
    }

    #[test]
    fn draw_event_cools_top_and_pushes_bottom_toward_mains() {
        let mut tank = test_tank(6, 60.0);
        {
            let temps = &mut tank.node_temps_c;
            temps.copy_from_slice(&[70.0, 68.0, 66.0, 64.0, 62.0, 60.0]);
        }

        let draw_volume = tank.node_volumes_m3()[0] * 1.5;
        let draw = tank
            .step(20.0, draw_volume, 10.0, &[], Duration::from_secs(60))
            .expect("draw step");

        assert!((draw.outlet_temp_c - 70.0).abs() < EPSILON);
        assert!(tank.node_temps()[0] < 70.0);
        assert!(tank.node_temps()[5] < 20.0);
    }

    #[test]
    fn standby_decay_uses_ua_over_mcp_dynamics() {
        let mut tank = StratifiedTank::new(StratifiedTankConfig {
            n_nodes: 1,
            height_m: 1.2,
            diameter_m: 0.5,
            ua_w_per_k: 8.0,
            conductivity_w_m_k: 0.0,
            initial_temp_c: 60.0,
            element_nodes: [Some(0), None],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })
        .expect("tank");

        let dt = Duration::from_secs(30);
        let ambient = 20.0;
        let mcp =
            WATER_DENSITY_KG_PER_M3 * tank.node_volumes_m3()[0] * WATER_SPECIFIC_HEAT_J_PER_KG_K;
        // Use the actual per-node UA (includes end-cap contribution) for the expected value.
        let effective_ua = tank.ua_per_node[0];
        let expected = 60.0 - (effective_ua * (60.0 - ambient) * dt.as_secs_f64()) / mcp;

        tank.step(ambient, 0.0, 12.0, &[], dt).expect("standby step");
        assert!((tank.node_temps()[0] - expected).abs() < 1.0e-12);
    }

    #[test]
    fn outlet_temp_matches_pre_draw_top_and_bottom_reaches_mains_with_large_draw() {
        let mut tank = test_tank(6, 55.0);
        {
            let temps = &mut tank.node_temps_c;
            temps.copy_from_slice(&[65.0, 63.0, 61.0, 59.0, 57.0, 55.0]);
        }

        let first = tank
            .step(
                20.0,
                tank.node_volumes_m3()[0] * 0.25,
                12.0,
                &[],
                Duration::from_secs(60),
            )
            .expect("first draw");
        assert!((first.outlet_temp_c - 65.0).abs() < EPSILON);

        let second = tank
            .step(
                20.0,
                tank.total_volume_m3() * 0.95,
                12.0,
                &[],
                Duration::from_secs(60),
            )
            .expect("second draw");
        assert!(second.outlet_temp_c.is_finite());
        assert!((tank.node_temps()[5] - 12.0).abs() < 1.0e-9);
    }

    #[test]
    fn draw_energy_balance_is_conserved_to_within_one_joule() {
        let mut tank = test_tank(6, 50.0);
        {
            let temps = &mut tank.node_temps_c;
            temps.copy_from_slice(&[62.0, 60.0, 58.0, 56.0, 54.0, 52.0]);
        }

        let before = total_energy_j(&tank);
        let draw = tank
            .step(
                20.0,
                tank.total_volume_m3() * 0.37,
                11.0,
                &[],
                Duration::from_secs(60),
            )
            .expect("draw step");
        let after = total_energy_j(&tank);

        let expected_after = before - draw.energy_out_j + draw.energy_in_j;
        assert!((after - expected_after).abs() <= 1.0);
    }

    #[test]
    fn dual_element_nodes_heat_correctly() {
        let mut tank = test_tank(6, 45.0);
        let dt = Duration::from_secs(120);
        let power_w = 3_000.0;
        let node_mass_cp =
            WATER_DENSITY_KG_PER_M3 * tank.node_volumes_m3()[0] * WATER_SPECIFIC_HEAT_J_PER_KG_K;
        let expected_delta = power_w * dt.as_secs_f64() / node_mass_cp;

        tank.heat_node(0, power_w, dt).expect("heat top");
        tank.heat_node(5, power_w, dt).expect("heat bottom");

        assert!((tank.node_temps()[0] - (45.0 + expected_delta)).abs() < 1.0e-12);
        assert!((tank.node_temps()[5] - (45.0 + expected_delta)).abs() < 1.0e-12);
        assert_eq!(tank.element_nodes(), [Some(0), Some(5)]);
    }

    #[test]
    fn save_load_round_trip_restores_temperatures() {
        let mut tank = test_tank(4, 50.0);
        {
            let temps = &mut tank.node_temps_c;
            temps.copy_from_slice(&[58.0, 54.0, 50.0, 46.0]);
        }
        let state = tank.save_state();

        tank.heat_node(0, 2_000.0, Duration::from_secs(60))
            .expect("mutate");
        tank.load_state(&state).expect("restore");
        assert_eq!(tank.node_temps(), &[58.0, 54.0, 50.0, 46.0]);
    }

    #[test]
    fn inversion_mixing_converges_within_n_passes_and_stabilizes_profile() {
        let mut tank = test_tank(12, 20.0);
        for (idx, temp) in tank.node_temps_c.iter_mut().enumerate() {
            *temp = 20.0 + idx as f64;
        }

        let swaps = tank.mix_inversions();
        assert!(swaps > 0);
        for pair in tank.node_temps().windows(2) {
            assert!(pair[0] >= pair[1]);
        }
    }

    #[test]
    fn inversion_mixing_conserves_energy_to_within_one_joule() {
        let mut tank = test_tank(10, 20.0);
        for (idx, temp) in tank.node_temps_c.iter_mut().enumerate() {
            *temp = 38.0 + idx as f64 * 1.25;
        }

        let before = total_energy_j(&tank);
        let merges = tank.mix_inversions();
        let after = total_energy_j(&tank);

        assert!(merges > 0);
        assert!((after - before).abs() <= 1.0);
    }

    #[test]
    fn uniform_profile_mixing_has_zero_swaps() {
        let mut tank = test_tank(8, 49.0);
        let swaps = tank.mix_inversions();
        assert_eq!(swaps, 0);
    }

    #[test]
    fn construction_validates_node_count_and_element_bounds() {
        let err = StratifiedTank::new(StratifiedTankConfig {
            n_nodes: 13,
            height_m: 1.2,
            diameter_m: 0.5,
            ua_w_per_k: 5.0,
            conductivity_w_m_k: 0.6,
            initial_temp_c: 50.0,
            element_nodes: [Some(0), None],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })
        .expect_err("invalid node count should fail");
        assert!(err.to_string().contains("n_nodes"));

        let err = StratifiedTank::new(StratifiedTankConfig {
            n_nodes: 6,
            height_m: 1.2,
            diameter_m: 0.5,
            ua_w_per_k: 5.0,
            conductivity_w_m_k: 0.6,
            initial_temp_c: 50.0,
            element_nodes: [Some(0), Some(6)],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })
        .expect_err("invalid element node should fail");
        assert!(err.to_string().contains("element_nodes"));
    }

    /// End-cap UA: boundary nodes (first and last) must have higher UA than interior nodes.
    #[test]
    fn end_cap_ua_boundary_nodes_higher_than_interior() {
        let tank = test_tank_with_ua(6, 4.0);
        let interior_ua = tank.ua_per_node[1];
        assert!(
            tank.ua_per_node[0] > interior_ua,
            "top node UA ({}) must exceed interior node UA ({})",
            tank.ua_per_node[0],
            interior_ua
        );
        assert!(
            tank.ua_per_node[5] > interior_ua,
            "bottom node UA ({}) must exceed interior node UA ({})",
            tank.ua_per_node[5],
            interior_ua
        );
        // Interior nodes (1..=4) should all be equal.
        for idx in 1..5 {
            assert!(
                (tank.ua_per_node[idx] - interior_ua).abs() < 1e-12,
                "interior node {idx} UA should equal base UA"
            );
        }
    }

    /// Single-node tank: end-cap is only added once (same index for top and bottom).
    #[test]
    fn end_cap_ua_single_node_added_once() {
        let tank = test_tank_with_ua(1, 2.0);
        // Single node UA = base_portion + 1x end_cap (top == bottom, only one addition).
        let expected = 2.0 + 2.0 * 0.1; // ua_w_per_k + ua_end_cap_w_per_k (applied once via "if last != 0" guard)
        assert!(
            (tank.ua_per_node[0] - expected).abs() < 1e-12,
            "single-node UA should be {} (base + 1×end_cap), got {}",
            expected,
            tank.ua_per_node[0]
        );
    }

    /// For n_nodes=2 without explicit volumes, top node gets 1/3 and bottom 2/3 of total.
    #[test]
    fn two_node_volumes_follow_ochre_one_third_two_thirds_split() {
        let tank = test_tank(2, 50.0);
        let total = tank.total_volume_m3();
        let expected_top = total / 3.0;
        let expected_bot = 2.0 * total / 3.0;
        assert!(
            (tank.node_volumes_m3()[0] - expected_top).abs() < 1e-12,
            "top node volume should be total/3 = {expected_top}, got {}",
            tank.node_volumes_m3()[0]
        );
        assert!(
            (tank.node_volumes_m3()[1] - expected_bot).abs() < 1e-12,
            "bottom node volume should be 2*total/3 = {expected_bot}, got {}",
            tank.node_volumes_m3()[1]
        );
    }

    fn ochre_draw_general_reference(
        states_c: &[f64],
        vol_fractions: &[f64],
        draw_fraction: f64,
        mains_temp_c: f64,
    ) -> (f64, Vec<f64>) {
        let n = states_c.len();
        let mut vols_pre = vec![0.0_f64; n + 1];
        let mut vols_post = vec![0.0_f64; n + 1];
        let mut temps = vec![0.0_f64; n + 1];

        let mut running = 0.0_f64;
        for (i, vf) in vol_fractions.iter().enumerate() {
            running += *vf;
            vols_pre[i] = running;
            temps[i] = states_c[i];
        }
        vols_pre[n] = running + draw_fraction;
        vols_post[0] = draw_fraction;
        temps[n] = mains_temp_c;

        running = draw_fraction;
        for (i, vf) in vol_fractions.iter().enumerate() {
            running += *vf;
            vols_post[i + 1] = running;
        }

        let mut outlet_temp = 0.0_f64;
        let mut prev = 0.0_f64;
        for (i, vol_pre) in vols_pre.iter().enumerate() {
            let clipped = (*vol_pre).min(draw_fraction);
            let vol_del = clipped - prev;
            outlet_temp += temps[i] * vol_del;
            prev = clipped;
        }
        outlet_temp /= draw_fraction;

        let mut t_end = vec![0.0_f64; n];
        for i in 0..n {
            let mut weighted_temp = 0.0_f64;
            let mut prev_v = vols_post[i];
            for j in 0..=n {
                let clipped = vols_pre[j].max(vols_post[i]).min(vols_post[i + 1]);
                let vol_del = clipped - prev_v;
                weighted_temp += temps[j] * vol_del;
                prev_v = clipped;
            }
            t_end[i] = weighted_temp / vol_fractions[i];
        }

        (outlet_temp, t_end)
    }

    #[test]
    fn draw_step_matches_ochre_general_algorithm_for_equal_node_volumes() {
        let mut tank = test_tank(6, 50.0);
        {
            let temps = &mut tank.node_temps_c;
            temps.copy_from_slice(&[62.0, 59.0, 56.0, 53.0, 50.0, 47.0]);
        }

        let draw_volume = tank.total_volume_m3() * 0.12;
        let draw_fraction = draw_volume / tank.total_volume_m3();
        let old_temps = tank.node_temps().to_vec();
        let vol_fraction = 1.0 / tank.n_nodes() as f64;
        let vol_fractions = vec![vol_fraction; tank.n_nodes()];
        let (expected_outlet_temp, expected_temps) =
            ochre_draw_general_reference(&old_temps, &vol_fractions, draw_fraction, 12.0);

        let draw = tank
            .step(20.0, draw_volume, 12.0, &[], Duration::from_secs(60))
            .expect("draw step");

        assert!((draw.outlet_temp_c - expected_outlet_temp).abs() < 1.0e-12);
        for (actual, expected) in tank.node_temps().iter().zip(expected_temps.iter()) {
            assert!((*actual - *expected).abs() < 1.0e-12);
        }
    }

    // --- TMV (tempered draw) tests ---

    /// When outlet_temp == tempered_draw_temp, no mixing reduction: raw draw volume is used.
    /// No unmet load either.
    #[test]
    fn tmv_no_reduction_when_outlet_at_fixture_temp() {
        use super::TemperedDrawConfig;
        let mut tank = test_tank(6, 40.6); // initial temp exactly at fixture setpoint

        let tmv = TemperedDrawConfig {
            tempered_draw_temp_c: 40.6,
            hot_draw_temp_c: 51.7,
            setpoint_temp_c: 51.7,
        };
        let flow_m3_s = 1e-4; // 0.1 L/s
        let dt = Duration::from_secs(60);
        let draw = tank
            .step_tempered(20.0, flow_m3_s, 0.0, 15.0, &[], tmv, dt)
            .expect("step_tempered");

        assert_eq!(draw.unmet_load_w, 0.0);
    }

    /// When outlet_temp < tempered_draw_temp, unmet load must be positive and proportional to deficit.
    #[test]
    fn tmv_unmet_load_when_outlet_below_fixture_temp() {
        use super::TemperedDrawConfig;
        let mut tank = test_tank(1, 30.0); // cold tank

        let tmv = TemperedDrawConfig {
            tempered_draw_temp_c: 40.6,
            hot_draw_temp_c: 51.7,
            setpoint_temp_c: 51.7,
        };
        let flow_m3_s = 1e-4;
        let mains_temp_c = 15.0;
        let dt = Duration::from_secs(60);
        let draw = tank
            .step_tempered(20.0, flow_m3_s, 0.0, mains_temp_c, &[], tmv, dt)
            .expect("step_tempered");

        // outlet_temp < fixture setpoint → unmet load > 0
        assert!(
            draw.unmet_load_w > 0.0,
            "expected positive unmet load, got {}",
            draw.unmet_load_w
        );
        // Rough sanity: unmet load ≈ flow_kg_s * Cp * deficit
        let deficit = (tmv.tempered_draw_temp_c - draw.outlet_temp_c).max(0.0);
        let expected =
            flow_m3_s * WATER_DENSITY_KG_PER_M3 * WATER_SPECIFIC_HEAT_J_PER_KG_K * deficit;
        assert!(
            (draw.unmet_load_w - expected).abs() < 1.0,
            "unmet_load_w {:.2} should be close to {expected:.2}",
            draw.unmet_load_w
        );
    }

    /// When outlet_temp > tempered_draw_temp, TMV blends in cold water: actual hot draw < requested.
    /// The draw volume applied to the tank must be less than the requested tempered volume.
    #[test]
    fn tmv_reduces_draw_when_outlet_above_fixture_temp() {
        use super::TemperedDrawConfig;
        let mut tank = test_tank(1, 65.0); // very hot tank

        let mains_temp_c = 15.0;
        let fixture_temp_c = 40.6;
        let flow_m3_s = 1e-4;
        let dt = Duration::from_secs(60);
        let tmv = TemperedDrawConfig {
            tempered_draw_temp_c: fixture_temp_c,
            hot_draw_temp_c: 51.7,
            setpoint_temp_c: 51.7,
        };

        let draw = tank
            .step_tempered(20.0, flow_m3_s, 0.0, mains_temp_c, &[], tmv, dt)
            .expect("step_tempered");

        // energy removed should be less than if raw draw (65°C) was used
        let raw_draw_energy = WATER_DENSITY_KG_PER_M3
            * WATER_SPECIFIC_HEAT_J_PER_KG_K
            * (flow_m3_s * dt.as_secs_f64())
            * 65.0;
        assert!(
            draw.energy_out_j < raw_draw_energy,
            "energy_out {:.1} should be less than raw draw energy {raw_draw_energy:.1}",
            draw.energy_out_j
        );
        assert_eq!(
            draw.unmet_load_w, 0.0,
            "no unmet load when outlet > fixture temp"
        );
    }

    /// `DrawResult` from `step()` has zero `unmet_load_w` (backward compat).
    #[test]
    fn raw_step_has_zero_unmet_load() {
        let mut tank = test_tank(1, 55.0);
        let draw = tank
            .step(
                20.0,
                tank.total_volume_m3() * 0.1,
                15.0,
                &[],
                Duration::from_secs(60),
            )
            .expect("step");
        assert_eq!(draw.unmet_load_w, 0.0);
    }

    /// With nonzero element heat and nonzero draw in the same step, the outlet
    /// temperature must equal the pre-step top-node value (not inflated by
    /// current-step element heat). This validates the FX-019 fix: element heat
    /// does not leak into outlet temperature.
    #[test]
    fn outlet_temp_not_inflated_by_same_step_element_heat() {
        let mut tank = test_tank(6, 50.0);
        let pre_step_top = tank.node_temps()[0];

        let draw_volume = tank.node_volumes_m3()[0] * 0.5;
        let element_power_w = 10_000.0; // large power to make the effect obvious
        let draw = tank
            .step(
                20.0,
                draw_volume,
                15.0,
                &[(0, element_power_w)],
                Duration::from_secs(60),
            )
            .expect("step with heat + draw");

        // Outlet must equal the pre-step top-node temperature, not the
        // heated value. The old code (heat_node before step) would have
        // returned a higher outlet temperature.
        assert!(
            (draw.outlet_temp_c - pre_step_top).abs() < 1e-12,
            "outlet_temp ({:.4}) must equal pre-step top-node ({pre_step_top:.4}), \
             not be inflated by element heat",
            draw.outlet_temp_c
        );
    }

    /// Deterministic fixture test for PAV inversion mixing on a complex
    /// multi-inversion profile. Verifies exact output temperatures (not just
    /// monotonicity/energy conservation, which are tested elsewhere).
    ///
    /// Input profile (top→bottom): [20, 60, 10, 50, 30, 40]
    /// Equal-volume nodes. PAV merges:
    ///   nodes 0,1 → 40.0  (20+60)/2
    ///   nodes 2,3 → 30.0  (10+50)/2
    ///   nodes 4,5 → 35.0  (30+40)/2, then merges with nodes 2,3 → 32.5
    /// Expected output: [40.0, 40.0, 32.5, 32.5, 32.5, 32.5]
    #[test]
    fn pav_deterministic_multi_inversion_fixture() {
        let mut tank = test_tank(6, 0.0);
        let input = [20.0, 60.0, 10.0, 50.0, 30.0, 40.0];
        for (node, &temp) in input.iter().enumerate() {
            tank.node_temps_mut()[node] = temp;
        }

        let merges = tank.mix_inversions();
        assert!(merges > 0, "multi-inversion profile must require merges");

        let expected = [40.0, 40.0, 32.5, 32.5, 32.5, 32.5];
        for (i, (&got, &exp)) in tank.node_temps().iter().zip(expected.iter()).enumerate() {
            assert!(
                (got - exp).abs() < 1e-9,
                "node {i}: expected {exp}, got {got}"
            );
        }

        // Energy conservation: sum of temps must be preserved (equal volumes).
        let input_sum: f64 = input.iter().sum();
        let output_sum: f64 = tank.node_temps().iter().sum();
        assert!(
            (output_sum - input_sum).abs() < 1e-9,
            "energy not conserved: input sum {input_sum}, output sum {output_sum}"
        );
    }
}
