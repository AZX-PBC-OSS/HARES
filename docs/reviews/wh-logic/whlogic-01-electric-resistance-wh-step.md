# Electric resistance WH step(): heating element, thermostat, energy accounting
**Review ID**: whlogic-01
**Category**: wh-logic
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/water_heater/resistance.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/WaterHeater.py vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc

## Findings

### Finding 1: Thermostat deadband model is asymmetric, not the symmetric SP±DB/2 model [Severity: low]

**Description**: The hysteresis function `hysteresis_call()` (`mod.rs:183-195`) uses OCHRE's asymmetric deadband: element turns ON at `T < setpoint - deadband` and OFF at `T >= setpoint`. The default deadband is 5.556°C (10°F, `resistance.rs:42`). The review instructions describe a symmetric model where the element turns ON at `T < setpoint - deadband/2` and OFF at `T > setpoint + deadband/2`. The asymmetric model produces a wider effective temperature swing: the tank must cool the full deadband below setpoint before reheating begins, whereas a symmetric model would begin reheating after cooling only half the deadband. The current default 10°F deadband provides adequate short-cycling protection. The model is feature-for-feature consistent with OCHRE's `run_thermostat_control()` (`WaterHeater.py:221-224`), which also uses `t_lower < self.setpoint_temp - self.deadband_temp` as the ON condition and `t_lower > self.setpoint_temp` as the OFF condition.

**Code Location**: `crates/hares-equipment/src/water_heater/mod.rs:183-195`; `resistance.rs:42`
**Root Cause**: Design choice tracking OCHRE, not a bug.
**Impact**: None — this is a deliberate modeling choice. Users who expect symmetric deadband behavior should halve the `deadband_c` input or configure a custom deadband.

---

### Finding 2: Combined element power in Simultaneous mode exceeds standard residential circuit rating [Severity: medium]

**Description**: In `ElementPriorityMode::Simultaneous` (`resistance.rs:414-417`), both elements operate independently based on their thermostat calls. The default element power is 4,500 W each (`resistance.rs:43`). When both fire simultaneously, the total draw is 9,000 W, which at 240 V draws 37.5 A — exceeding the 30 A rating of a typical residential water-heater branch circuit. In reality, simultaneous-operation water heaters use one of three mechanisms to avoid this: (a) interlocked wiring so only one element is ever energized, (b) lower-rated elements (typically 3,000–3,500 W each), or (c) a dedicated dual-circuit installation. The code does not enforce any total-power ceiling. EnergyPlus addresses this by supporting independent capacities (`MaxCapacity` and `MaxCapacity2` at `WaterThermalTanks.cc:8061-8190`) and only allows simultaneous operation when explicitly configured. OCHRE's `ElectricResistanceWaterHeater.run_thermostat_control()` (`WaterHeater.py:405-421`) uses the same MasterSlave-first logic as HARES, but in ideal-capacity mode, duty cycles are allocated with upper-element priority and the lower-element duty is clamped by the remaining fraction (`d_lower = min(max(h_lower/self.capacity_rated, 0), 1 - d_upper)`).

The hardware risk is not a simulation concern, but model users could inadvertently configure Simultaneous mode with 4.5 kW elements and get unrealistically high power draws.

**Code Location**: `resistance.rs:413-418` (control logic); `resistance.rs:43` (default element power)
**Root Cause**: No gating check on combined power in Simultaneous mode. The `element_priority_mode` config field defaults to `"MasterSlave"` (`resistance.rs:316-318`), which is the safe default for residential circuits, but there is no validation that a user-configured Simultaneous mode with standard-power elements produces a plausible total load.
**Impact**: 9 kW draw on a 30 A / 240 V circuit would trip a breaker in reality; this is a modeling fidelity concern, not a code crash or panic. Mitigated by the MasterSlave default.

---

### Finding 3: Single setpoint for both upper and lower elements — no independent upper/lower thermostat offset [Severity: low]

**Description**: The thermostat logic uses a single `setpoint_c` and `deadband_c` for both elements (`resistance.rs:206-225`). EnergyPlus supports independent setpoints for each element (`SetPointTemp` and `SetPointTemp2` at `WaterThermalTanks.cc:8019-8020`) with independent deadbands (`DeadBandDeltaTemp` and `DeadBandDeltaTemp2`). In physical WHs, the upper thermostat is typically set 5–10°F (2.8–5.6°C) above the lower thermostat to ensure upper-element priority during recovery from a deep draw. In the HARES model, priority is achieved exclusively through the MasterSlave interlock (`resistance.rs:410-413`): when the upper element is firing, the lower is unconditionally disabled. This works correctly for the modeled priority, but it means the model cannot represent the "staggered thermostat" behavior where both thermostats call simultaneously and the interlock resolves the conflict. OCHRE also uses a single setpoint for its resistance WH thermostat (`WaterHeater.py:221`), so HARES is consistent with the primary vendor reference.

**Code Location**: `resistance.rs:206-225`; contrast with EnergyPlus `WaterThermalTanks.cc:8019-8020`
**Root Cause**: Simplified single-setpoint design inherited from OCHRE. Not a defect, but a modeling restriction.
**Impact**: The model cannot represent configurations where the upper thermostat is intentionally set above the lower thermostat. The MasterSlave interlock compensates functionally, so the impact on simulation results is minimal for non-simultaneous WHs.

---

### Finding 4: Per-node UA distributed proportionally to node volume, not node surface area [Severity: low]

**Description**: The jacket loss UA is apportioned to individual nodes proportionally to node volume (`tank.rs:159-162`): `ua_per_node[i] = config.ua_w_per_k * node_volumes_m3[i] / total_volume_m3`. For a cylindrical tank with uniform node heights (always the case: `tank.rs:151` gives `node_height_m = config.height_m / n_nodes`), every node has identical lateral surface area, so UA should be distributed equally or proportionally to lateral surface area, not volume. When all nodes have equal volume (the default for n_nodes ≥ 3), volume-proportional and equal-proportional give identical results. For non-uniform node volumes (e.g., the 2-node tank's 1/3–2/3 split at `tank.rs:133-136`), the lower (2/3-volume) node receives twice the UA of the upper node, but since both nodes have equal height and therefore equal lateral cylinder area, this over-allocates jacket loss to the lower node.

In EnergyPlus, per-node ambient losses use per-node UA values derived from tank geometry and insulation properties (distributed based on node surface area exposed to ambient, not volume). OCHRE's multi-node tank model similarly distributes UA by node surface area.

**Code Location**: `tank.rs:158-162`
**Root Cause**: UA allocation formula uses volume fraction instead of surface-area (height) fraction. For uniform-volume nodes the two are equivalent; for the 2-node case, the split is slightly off.
**Impact**: For the default 6-node uniform tank, zero impact. For the 2-node case, the bottom node's standby loss is overstated by a factor of 2× relative to the top node's (2/3 volume vs. 1/2 surface area for equal-height nodes). This is a minor deviation affecting a non-default configuration.

---

### Finding 5: Step sequence — element heat injected AFTER standby losses but outlet temperature snapshotted BEFORE both [No finding]

**Description**: Verified that the energy accounting pipeline is correct. In `step()` and `step_tempered()` (`tank.rs:301-330, 341-421`), the tank physics are applied in the correct sequence: (1) `apply_conduction_and_standby` (conduction + jacket loss), (2) snapshot pre-injection temperatures for energy-out accounting, (3) `apply_heat_injections` (element heat to target nodes), (4) `apply_draw` (outlet flow + displacement), (5) `mix_inversions` (buoyancy). The outlet temperature is snapshotted from the **pre-heating** top-node value (`tank.rs:357`), matching OCHRE `Water.py:284`. Electric power is reported through `PortContribution::Electrical` (`resistance.rs:534-538`) and jacket loss through `PortContribution::Thermal` (`resistance.rs:551-562`). The total electric energy = element power × timestep × duty fraction, which is correctly accounted in both the electrical balance and the tank thermal balance (via `heat_injections`).

**Code Location**: `tank.rs:301-330` (untempered step), `tank.rs:341-421` (tempered step), `resistance.rs:427-612` (WH step)
**Root Cause**: N/A
**Impact**: No issue found. Energy accounting is consistent with both OCHRE and EnergyPlus.

---

## Summary
- Total findings: 5
- Critical / High: 0
- Medium: 1 (Finding 2 — Simultaneous mode circuit exceedance)
- Low: 4 (Findings 1, 3, 4, 5 is informational/no-finding)

## Recommendations

1. **Simultaneous-mode power guard (Finding 2)**: Add a configurable per-equipment maximum total power (`max_combined_power_w`) and clamp combined draw in Simultaneous mode. Alternatively, emit a startup warning when `ElementPriorityMode::Simultaneous` is configured with two 4,500 W elements and no power limit set. Defaulting to MasterSlave (which already happens) is the right safety behavior.

2. **Clarify deadband semantics in documentation (Finding 1)**: Add a doc comment in `resistance.rs` and/or the `hysteresis_call()` function noting that the deadband is the full temperature below setpoint (not ±DB/2), matching OCHRE. Users migrating from symmetric-deadband models should adjust their `deadband_c` input accordingly.

3. **Optional dual-setpoint support (Finding 3)**: Consider adding optional `lower_setpoint_c` and `lower_deadband_c` fields to `ElectricResistanceWaterHeaterConfig`. If absent, the current single-setpoint behavior (matching OCHRE) is fine. EnergyPlus dual-setpoint support could be added as a non-breaking config extension.

4. **UA distribution by surface area (Finding 4)**: Change `tank.rs:159-162` to distribute `ua_w_per_k` proportionally to the lateral surface-area fraction (which is `1/n_nodes` for uniform-height nodes) rather than volume fraction. For the 2-node special case, use `0.5` for each node regardless of the 1/3–2/3 volume split.

## References / Citations

- **OCHRE WaterHeater.py**: Thermostat control at lines 221-224 and 405-422; ideal capacity at lines 376-403; tempered draw at lines 305-363.
- **EnergyPlus WaterThermalTanks.cc**: MasterSlave interlock at lines 8167-8184; dual-setpoint/deadband at lines 8019-8020; sub-timestepping and dT_max logic at lines 8194-8234.
- **HARES resistance.rs**: Element priority at lines 407-418; ideal capacity mode at lines 441-483; energy accounting through electrical port (line 534) and thermal port (line 551).
- **HARES tank.rs**: Per-node UA allocation at lines 158-162; conduction and standby loss at lines 550-587; inversion mixing (buoyancy) at lines 464-526.
