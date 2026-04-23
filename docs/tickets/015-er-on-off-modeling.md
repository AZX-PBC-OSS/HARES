# Electric Resistance Heat Not Modeled as On/Off

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/hvac/heat_pump, hares-equipment/hvac

## Problem

Electric resistance (ER) backup heat capacity is modulated by PLR (part-load ratio), but real ER heaters are inherently binary on/off devices — they are resistive elements that are either energized or not. EnergyPlus models `Coil:Heating:Electric` as strictly binary. Modulating ER by PLR produces:

1. **Unrealistic partial-load ER power** — at PLR=0.5, the code computes `er_capacity_w = backup_capacity_w * 0.5`, which implies a 4 kW element drawing 2 kW. This is physically impossible for a resistive element without a multi-stage design.

2. **Incorrect peak demand modeling** — for grid simulation, the peak ER draw matters. The modulated model underestimates peak draw when PLR < 1.0, because it spreads the power across timesteps rather than cycling full-on/full-off.

3. **EnergyPlus mismatch** — E+ `Coil:Heating:Electric` has no PLR modulation; it is either on at rated capacity or off. The only way to get partial ER output in E+ is through time-averaging (cycling), which is what the thermostat/PLF system should handle.

## Current Behavior

1. **ER capacity modulated by PLR** — `heater.rs:1034-1042`:
   ```rust
   let er_capacity_w = if er_on && self.use_ideal {
       (self.ideal_capacity_w - hp_capacity_w)
           .max(0.0)
           .min(self.backup_capacity_w)
   } else if er_on {
       self.backup_capacity_w * plr  // <-- PLR modulation
   } else {
       0.0
   };
   ```
   The `self.backup_capacity_w * plr` expression produces fractional ER output proportional to PLR.

2. **ER power proportional to capacity** — `heater.rs:1043`:
   ```rust
   let er_power_w = er_capacity_w * self.backup_eir;
   ```
   Since `backup_eir = 1.0` for electric resistance, `er_power_w = er_capacity_w`, which means at PLR=0.5, the ER draws half its rated power.

3. **Ideal-capacity mode already handles this better** — the `use_ideal` branch (lines 1034-1037) caps ER at `backup_capacity_w` and only uses the residual of `ideal_capacity_w - hp_capacity_w`. This is closer to correct but still allows fractional ER below rated capacity.

4. **Load fraction scaling explicitly excludes ER** — `heater.rs:1078-1095`:
   ```rust
   // ER is on/off -- not modulatable -- so only compressor and fan are scaled.
   let effective_load = ...;
   if effective_load < 1.0 {
       thermal_output_w = hp_thermal * effective_load + er_thermal;
       // backup_er_kw, fuel_w, and step_er_capacity_w are not scaled (ER is not modulatable)
   }
   ```
   The comment says "ER is not modulatable" but the `backup_capacity_w * plr` at line 1039 contradicts this.

5. **Test locks in the wrong behavior** — `heater.rs:1993-2035`: `er_backup_capacity_modulated_by_plr` test explicitly verifies that ER draws below full-strip+fan at PLR=0.5. This test must be updated as part of this ticket to assert binary behavior instead.

## Physics

EnergyPlus `Coil:Heating:Electric` is strictly on/off at the coil level; partial-load behavior comes from time-averaged PLR cycling at the thermostat, not coil modulation. Source: EnergyPlus Engineering Reference, "Electric Heating Coil" section.

## Required Behavior

1. **ER must be binary on/off when `er_stages = 1`** — when the ER element is on, it draws full rated power (`backup_capacity_w * backup_eir`). When off, it draws zero.

2. **Multi-stage ER is a valid configuration** — some systems have 2 or 3 ER stages (e.g., 5 kW + 5 kW for a 10 kW backup). This should be modeled as discrete steps, not continuous modulation. `er_stages` must be validated: `1 <= er_stages <= 4`. Values outside this range must produce a loud error (no silent clamp). Real residential systems have at most 3–4 backup ER stages.

3. **PLF/PLR cycling applies to the HP compressor, not to ER** — the thermostat/on-off cycling that produces PLR < 1.0 should affect the compressor, while the ER is controlled by its own thermostat threshold (the `er_setpoint_offset_c` / `er_thermostat_call` logic already exists in `heater.rs:1257-1263`).

4. **In ideal-capacity mode**, ER should fill the residual up to the nearest stage capacity, not the exact residual.

### Formula

```rust
// When er_stages = 1 (binary):
if er_on {
    er_capacity_w = self.backup_capacity_w;  // Full rated, not modulated
} else {
    er_capacity_w = 0.0;
}

// When er_stages > 1 (multi-stage):
let stage_capacity_w = self.backup_capacity_w / er_stages as f64;
if er_on {
    // In ideal mode: find smallest n such that n * stage_capacity >= residual
    let residual = (ideal_capacity_w - hp_capacity_w).max(0.0);
    let n_stages_on = (residual / stage_capacity_w).ceil().min(er_stages as f64) as u32;
    er_capacity_w = n_stages_on as f64 * stage_capacity_w;
} else {
    er_capacity_w = 0.0;
}
```

## Approach

1. **Add `er_stages: u8` field** to `HeatPumpHeaterConfig` (`heat_pump_config.rs`):
   ```rust
   #[serde(default = "default_one")]
   pub er_stages: u8,
   ```
   Default: 1 (binary on/off). Validate that `1 <= er_stages <= 4`; error loudly outside that range.

2. **Add `er_stages` and `er_stage_capacity_w`** to `HeatPumpHeaterCore`:
   ```rust
   er_stages: u8,               // from config
   er_stage_capacity_w: f64,    // backup_capacity_w / er_stages
   ```

3. **Modify `compute_step`** (`heater.rs:1034-1042`) to use discrete ER stages:
   - Remove `self.backup_capacity_w * plr` modulation
   - For non-ideal mode: when `er_on`, `er_capacity_w = self.backup_capacity_w` (full rated for `er_stages = 1`)
   - For ideal mode: compute residual and round up to nearest stage

**Non-ideal multi-stage ER**: For simplicity, all stages activate together
(treat multi-stage ER as a single binary unit with total capacity =
`er_stages * stage_capacity_w`). Per-stage activation would require a
separate thermostat or time delay, which is beyond the scope of this ticket.
If per-stage control is needed in the future, add a separate ticket.

**Ideal-mode overshoot**: The formula `n_stages_on = ceil(residual / stage_capacity_w)` deliberately rounds up, potentially over-delivering. In non-ideal mode this produces overshoot that the thermostat cycles down on the next step (acceptable). In ideal mode the back-calc absorbs it. No cap is needed beyond the `er_stages` upper bound of 4.

4. **Update the load-fraction scaling** (`heater.rs:1078-1095`) — the comment "ER is not modulatable" is now accurate; ER power is already at full rated when on.

5. **Update the `er_backup_capacity_modulated_by_plr` test** (`heater.rs:1993-2035`) — this test currently verifies PLR modulation. Change it to verify binary behavior: ER at full rated when on, zero when off.

6. **Add test for multi-stage ER** — `er_stages = 2`, verify that in ideal mode the ER activates the minimum number of stages needed to cover the residual.

7. **Handle the cycling physics** — in non-ideal mode with `er_stages = 1`, the ER thermostat already handles on/off control via `er_thermostat_call` (`heater.rs:1257-1263`). The ER will be fully on when the thermostat calls for it and fully off otherwise. The time-averaged output comes from the cycling, not from PLR modulation.

8. **Cross-ticket interaction with 013 (MSHP min-speed cycling)**: binary full-rated ER firing during HP short-cycling at minimum speed can amplify zone temperature overshoot. This ticket owns the integration test: run a low-load scenario with both the 013 `min_compressor_fraction` change and the 015 binary ER change active simultaneously. Verify that zone temperature overshoot remains within the thermostat hysteresis band after convergence.

## HPXML Wiring

`er_stages`: No standard HPXML source. HPXML `BackupHeatingCapacity`
provides total capacity but not stage count. Default to 1 (single-stage
binary on/off) matching residential practice. A future extension field
`extension/ERStages` could override this.

## Definition of Done

- [ ] `er_stages` field in `HeatPumpHeaterConfig` (default 1)
- [ ] Validation: `1 <= er_stages <= 4`; error loudly (no silent clamp) outside that range
- [ ] ER capacity is full rated when `er_on` and `er_stages = 1` (no PLR modulation)
- [ ] ER capacity is zero when `er_on = false`
- [ ] Multi-stage ER: ideal mode rounds residual up to nearest stage
- [ ] Non-ideal mode: ER is binary on/off regardless of `er_stages`
- [ ] `er_backup_capacity_modulated_by_plr` test updated to verify binary behavior
- [ ] New test: `er_stages = 2` in ideal mode activates correct number of stages
- [ ] New integration test: low-load scenario with 013 + 015 changes active; zone temperature overshoot within hysteresis band
- [ ] Peak ER draw equals `backup_capacity_w` when ER is on (not reduced by PLR)
- [ ] Telemetry `BACKUP_ER_KW` reports full rated power when ER is on
- [ ] Telemetry key `er_stages_on` added

## Verification

1. **Unit test**: `er_stages = 1`, ER on → `er_capacity_w == backup_capacity_w` (not `backup_capacity_w * plr`).
2. **Unit test**: `er_stages = 1`, ER off → `er_capacity_w == 0.0`.
3. **Unit test**: `er_stages = 2`, ideal mode, residual = 3 kW, stage_capacity = 5 kW → `er_capacity_w = 5 kW` (1 stage, not 3 kW).
4. **Unit test**: `er_stages = 2`, ideal mode, residual = 7 kW, stage_capacity = 5 kW → `er_capacity_w = 10 kW` (2 stages).
5. **Peak demand test**: Run simulation with cycling thermostat; peak ER draw must equal `backup_capacity_w`, never a fraction.
6. **Energy balance test**: Time-averaged ER output over many cycles must approximately equal the modulated `backup_capacity_w * average_plr` (same average, different instantaneous profile).

## References

- EnergyPlus I/O Reference, `Coil:Heating:Electric` — single-stage resistive
heating element with fixed capacity. ASHRAE Handbook — Fundamentals Ch.33:
residential electric strip heat is typically a single element or 2-stage
sequenced by an outdoor thermostat.
- AHRI Standard 210/240 — ER backup is rated at full capacity, not modulated
- HARES `heater.rs:1034-1042`: current ER capacity computation
- HARES `heater.rs:1078-1095`: load-fraction scaling with "ER is not modulatable" comment

## Related Tickets

- [014-defrost-typed-config.md](014-defrost-typed-config.md) — config struct improvements
- [011-discrete-defrost-cycle.md](011-discrete-defrost-cycle.md) — defrost ER behavior for resistive strategy
