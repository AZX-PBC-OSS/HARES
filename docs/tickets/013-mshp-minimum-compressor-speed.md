# MSHP Minimum Compressor Speed Not Configurable

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/hvac/heat_pump, hares-equipment/hvac

## Problem

Real mini-split heat pumps have a minimum compressor speed (typically 25–40% of rated capacity). HARES hardcodes MSHP speed stages at 25%/50%/75%/100% with no way to configure the minimum. This matters because:

1. **Overheating at low load** — when the zone needs only 15% of rated capacity, a 25% minimum stage will overshoot, causing thermostat cycling (on/off) instead of continuous low-speed operation. Real MSHPs with 30% minimum would have the same issue, but the behavior should be configurable.

2. **COP at minimum speed differs from rated** — at minimum compressor speed, COP is typically higher than at rated speed (lower pressure ratio). HARES uses the same EIR for all 4 stages (`heater.rs:524`), which is incorrect for units where minimum-speed COP is significantly different.

3. **Different MSHP manufacturers have different minimums** — some inverter-driven units operate down to 20% while others bottom out at 40%. A fixed 25% assumption is not generalizable.

## Current Behavior

1. **MSHP speed generation** in `heater.rs:517-525`:
   ```rust
   if cfg.is_mini_split || matches!(self.variant, HeaterVariant::Minisplit) {
       self.hvac.speed_control_mode = SpeedControlMode::VariableSpeedIdeal;
       if self.hvac.heating_capacities_w.len() == 1 {
           let base_cap = self.hvac.heating_capacities_w[0];
           let base_eir = self.hvac.eir_by_stage[0];
           self.hvac.heating_capacities_w =
               vec![base_cap * 0.25, base_cap * 0.5, base_cap * 0.75, base_cap];
           self.hvac.eir_by_stage = vec![base_eir; 4];
       }
   ```
   The 25% minimum is hardcoded in `base_cap * 0.25`. The 4-stage spacing (25/50/75/100) is also hardcoded.

2. **MSHP cooling path** — the MSHP cooler does NOT generate speed stages itself; it delegates to the inner `AirConditioner` via `typed_hp_to_central_ac_config` in `cooler.rs:104-172`, which then calls `AirConditioner.init`. The cooling-side speed generation happens in `AirConditioner.init` with the same hardcoded fractions.

3. **No config field** — `HeatPumpHeaterConfig` (`heat_pump_config.rs:20-139`) has no `min_compressor_fraction` field. The only speed-related field is `number_of_speeds`, which is forced to 4 for mini-splits.

4. **Same EIR for all stages** — `eir_by_stage = vec![base_eir; 4]` assumes constant EIR across all speeds. Real MSHPs have higher COP (lower EIR) at part-load speeds.

## Required Behavior

1. **Configurable minimum compressor fraction** — a `min_compressor_fraction: f64` field (default: 0.25) in `HeatPumpHeaterConfig` and `HeatPumpCoolerConfig`.

2. **Speed stages derived from min and max** — given `min_fraction` and rated capacity, generate N evenly-spaced stages:
   ```
   stage[i] = rated * (min_fraction + (1.0 - min_fraction) * i / (n_stages - 1))
   ```
   For `min_fraction = 0.25, n_stages = 4`: produces [0.25, 0.50, 0.75, 1.00] (same as current).
   For `min_fraction = 0.30, n_stages = 4`: produces [0.30, 0.533, 0.767, 1.00].

3. **Per-stage EIR option** — when `stage_heating_eirs` is provided, use it; otherwise, derive part-load EIR from a simple model:
   ```
   eir[i] = rated_eir * (1.0 - eir_part_load_benefit * (1.0 - fraction[i]))
   ```
   where `eir_part_load_benefit` defaults to 0.0 (current behavior: constant EIR) and can be set to ~0.1-0.2 for units with documented part-load COP improvement.

## Approach

1. **Add `min_compressor_fraction` field** to `HeatPumpHeaterConfig` (`heat_pump_config.rs`):
   ```rust
   #[serde(default = "default_min_compressor_fraction")]
   pub min_compressor_fraction: f64,
   ```
   with:
   ```rust
   fn default_min_compressor_fraction() -> f64 { 0.25 }
   ```

2. **Add `eir_part_load_benefit` field** (optional, default 0.0):
   ```rust
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub eir_part_load_benefit: Option<f64>,
   ```

3. **Modify speed generation** in `heater.rs:517-525` to use `min_compressor_fraction`:
   ```rust
   let min_frac = cfg.min_compressor_fraction.clamp(0.1, 0.5);
   let n = 4;  // MSHP always 4 stages
   let stages: Vec<f64> = (0..n)
       .map(|i| base_cap * (min_frac + (1.0 - min_frac) * i as f64 / (n - 1) as f64))
       .collect();
   self.hvac.heating_capacities_w = stages;
   ```

4. **Derive per-stage EIR** if `eir_part_load_benefit` is provided:
   ```rust
   let benefit = cfg.eir_part_load_benefit.unwrap_or(0.0);
   self.hvac.eir_by_stage = (0..n)
       .map(|i| {
           let frac = min_frac + (1.0 - min_frac) * i as f64 / (n - 1) as f64;
           base_eir * (1.0 - benefit * (1.0 - frac))
       })
       .collect();
   ```

5. **Apply same changes to cooling side** — add the fields to `HeatPumpCoolerConfig` and update the cooler's speed generation path (which delegates to `AirConditioner`).

6. **Validate** in `HeatPumpHeaterConfig::validate()`: `min_compressor_fraction` must be in `[0.1, 0.5]`. For `eir_part_load_benefit`: no hard ceiling can be cited from a primary source; validate that the value is in `[0.0, 1.0]` and emit a warning when it exceeds 0.5, since measured COP improvement at minimum stage versus rated is typically 30–50% for inverter compressors (NREL/PNNL inverter heat pump field studies, e.g., Cutler et al., NREL/TP-5500-52791, and Daikin/Mitsubishi product engineering data). Do not silently clamp; error loudly outside the validated range.

7. **Cross-ticket interaction with 015 (ER on/off)**: when zone load falls below the minimum-stage HP capacity, the HP cycles on/off. Binary ER staging from ticket 015 may fire during HP cycling and cause zone temperature overshoot. An integration test must run both changes together under a low-load scenario. This test is owned by ticket 015 (see "Verification" in that ticket).

8. **Update existing tests** — the MSHP speed stage tests in `cooler.rs:599-675` assume 4 stages at 0.25/0.5/0.75/1.0. Update to use explicit config or verify the new default produces equivalent results.

## HPXML Wiring

`min_compressor_fraction`: HPXML `MinimumCapacity` / `HeatingCapacity`.
Currently `MinimumCapacity` is not read by `resolve_hvac.rs`. Add extraction:
`min_compressor_fraction = MinimumCapacity / HeatingCapacity` if provided,
otherwise default 0.25.

`eir_part_load_benefit`: No HPXML source. Comes from equipment defaults
or manual config. Default 0.0 (no EIR benefit at part load).

## Definition of Done

- [ ] `min_compressor_fraction` field in `HeatPumpHeaterConfig` and `HeatPumpCoolerConfig`
- [ ] Default value 0.25 preserves existing behavior
- [ ] Speed stages derived from `min_compressor_fraction` instead of hardcoded 0.25
- [ ] Per-stage EIR derived from `eir_part_load_benefit` when provided
- [ ] Validation: `min_compressor_fraction` in [0.1, 0.5] errors loudly; `eir_part_load_benefit` in [0.0, 1.0] errors loudly; warning emitted when `eir_part_load_benefit > 0.5`
- [ ] Existing MSHP tests pass with default `min_compressor_fraction = 0.25`
- [ ] New test: `min_compressor_fraction = 0.30` produces stages [0.30, 0.533, 0.767, 1.00]
- [ ] Cooling-side MSHP also uses configurable minimum
- [ ] `tracing::debug!` when generating speed stages with configurable minimum: `"MSHP generating {n} stages from min_fraction={min:.2} to rated"`
- [ ] Telemetry key `MIN_COMPRESSOR_FRACTION` added

## Verification

1. **Regression test**: MSHP with default `min_compressor_fraction = 0.25` produces identical behavior to current code.
2. **Unit test**: MSHP with `min_compressor_fraction = 0.30`; verify first stage capacity = `0.30 * rated`.
3. **Unit test**: MSHP with `eir_part_load_benefit = 0.15`; verify first-stage EIR < rated EIR.
4. **Unit test**: Validate rejects `min_compressor_fraction < 0.1` and `> 0.5`.
5. **Integration test**: Simulate a low-load condition (10% of rated) with `min_compressor_fraction = 0.25`. The HP should cycle on/off (load < minimum). With `min_compressor_fraction = 0.10`, the HP should run continuously at stage 1.

## References

- AHRI Standard 1230: variable-rate heat pump testing and rating
- Mitsubishi Electric MSHP specifications: minimum capacity typically 30-40% of rated
- Daikin inverter specifications: minimum compressor frequency ~20-25 Hz vs rated 60-80 Hz
- EnergyPlus I/O Reference, `Coil:Heating:DX:MultiSpeed` fields: `Minimum Flow Fraction` and `Gross Rated Heating Capacity`
- OCHRE `HVAC.py:774-797`: multispeed parameters loaded from CSV, not hardcoded

## Related Tickets

- [014-defrost-typed-config.md](014-defrost-typed-config.md) — config struct improvements
- [010-default-biquadratic-performance-curves.md](010-default-biquadratic-performance-curves.md) — curves interact with speed stages
