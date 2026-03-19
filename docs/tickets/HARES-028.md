---
id: HARES-028
title: "hares-equipment — Battery"
kind: implement
depends_on: [HARES-018]
files_to_touch:
  - crates/hares-equipment/src/battery.rs
references:
  - docs/architecture/02-equipment-and-ports.md
  - docs/architecture/03-control-interfaces.md
  - docs/architecture/05-external-tools.md
verification:
  - cargo check -p hares-equipment
  - cargo test -p hares-equipment
  - cargo clippy -p hares-equipment -- -D warnings
---

## Background/Context
Battery storage is central to grid-interactive simulation. The OCHRE battery model has a hardcoded 0 W standby power, which this implementation fixes. Multi-instance support is required for homes with multiple battery systems.

## Work to Do
- [x] Implement `Battery` struct implementing `Equipment` with SOC tracking clamped to `[0.0, 1.0]`
- [x] Load OCV curve, internal resistance, and self-discharge rate from SAM-generated TOML per the adapter chain defined in `docs/architecture/05-external-tools.md`; the TOML path is provided via `EquipmentConfig`. Built-in Li-NMC defaults matching OCHRE are used when no TOML is present.
- [x] Implement OCV (open circuit voltage) curve: table-based interpolation indexed by SOC
- [x] Implement internal resistance model: derive pack voltage and resistance from cell specs (`n_series`, `n_parallel`); compute ohmic losses from current
- [x] Derive charge/discharge efficiency from ohmic losses rather than a fixed efficiency parameter
- [x] Implement self-discharge: configurable percentage per day, applied each timestep
- [x] Implement configurable standby power (OCHRE fix: was hardcoded 0 W); write to Electrical port when idle
- [x] Implement self-consumption controller: reads accumulated Stage 1 electrical power from `PortSlots.electrical` (not `EnvironmentState`) to determine current-step net load after PV and scheduled loads have run; charge from excess PV, discharge to offset load; respect SOC bounds. Battery runs in `ExecutionStage::Electrical` (Stage 2) after Stage 1 accumulation.
  - Port sign convention: `active_power_kw` is positive for consumption, negative for generation. PV generation therefore appears as a negative value in the accumulated `PortSlots.electrical`. A negative `active_power_kw` means net generation exceeds load (surplus to charge from); a positive value means net load (deficit to discharge into).
  - Reading `PortSlots` in `step()` is a sanctioned pattern — the `step()` signature provides `&mut PortSlots`. The staged execution model (Independent → Electrical → Thermal) guarantees Stage 1 equipment (PV, scheduled loads) has already written its contributions before Stage 2 equipment (Battery) executes, so Battery reads only completed Stage 1 contributions.
- [x] Implement `apply_control` handling for `ControlSignal::SelfConsumption { enabled, solar_only_charging }`: when `solar_only_charging` is true, the controller charges only when net Stage 1 generation is positive (PV surplus) and never from grid import
- [x] Implement rainflow cycle counting for degradation tracking; update daily
- [x] Persist rainflow accumulator in `save_state` / `load_state` so degradation history survives RL episode checkpointing
- [ ] Implement OCHRE's full 3-state degradation model: Q1 (calendar aging, Arrhenius), Q2 (cycle aging, rainflow), Q3 (SEI lithium plating, Tafel kinetics). Six constants: `b1_arr`, `b2_arr`, `b3_arr`, `b1_tfl`, `b3_tfl`, plus cycle depth from rainflow. Reference OCHRE `Battery.py:365-441` and IEEE 7963578. **Stubbed in v1**: `capacity_fade_pct` returns 0.0; `DegradationState::update_daily` is a no-op with TODO markers.
- [x] `save_state` serializes only the three degradation scalars `(q1, q2, q3)` plus a compact rainflow half-cycle buffer — NOT the raw SOC timeseries. Clear raw data after each daily degradation update to bound serialized state size.
- [x] Implement optional thermal model: compute heat dissipated from ohmic losses; write to Thermal port when zone is configured
- [x] Implement lumped cell thermal model: track cell temperature with configurable thermal mass (`cell_thermal_mass_j_per_k`) and heat loss coefficient (`cell_ua_w_per_k`); cell temp evolves based on ambient temperature, ohmic heating, and heater power
- [x] Implement low-temperature cell heater: configurable `heater_power_w` and `heater_threshold_c`; heater activates when cell temp is below threshold and charging is requested (e.g. winter sunny day with PV surplus). Modeled after Franklin WH aPower 2 (~500 W heater at 0 C). Heater draws from electrical port and contributes heat to the cell thermal model and zone thermal port.
- [x] Implement `heater_on_discharge` config flag (default false): when true, heater also activates when discharge is requested but blocked/derated by cold temps (Tesla Powerwall 3 Heat Mode style). Default false = Franklin-style charge-only heater.
- [x] Implement temperature-dependent discharge derating: linear derating from full power at `full_power_temp_c` (default 10 C, per Tesla/Franklin specs) to zero at `min_discharge_temp_c` (default -20 C)
- [x] Implement charge temperature lockout: charging blocked below `min_charge_temp_c` (default 0 C, Li-ion lithium plating safety limit)
- [x] Ensure each `Battery` instance owns fully independent state (SOC, OCV table, cycle count)
- [x] Declare `control_capabilities`: `POWER_SETPOINT | SOC_TARGET | GRID_CONNECT | SELF_CONSUMPTION`
- [x] Declare `telemetry_fields`: `soc`, `active_power_kw`, `ohmic_loss_w`, `standby_power_w`, `cell_temp_c`, `heater_power_w`, `discharge_derate`, `cycle_count`, `capacity_fade_pct`
- [x] Implement `save_state() -> Vec<u8>`: serialize SOC, cycle count, rainflow accumulator, cell temperature, heater state, and control state
- [x] Implement `load_state(&mut self, state: &[u8]) -> Result<()>`
- [x] Assign `ExecutionStage::Electrical` (Stage 2) in `EquipmentDescriptor`
- [x] Register in `EquipmentRegistry`

## Config Keys

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `equipment_id` | u32 | 0 | Unique equipment instance ID |
| `zone_id` | u16 | None | Zone for thermal port (ohmic + heater heat) |
| `capacity_kwh` | f64 | 13.5 | Usable energy capacity |
| `max_charge_kw` | f64 | 5.0 | Maximum charge power |
| `max_discharge_kw` | f64 | 5.0 | Maximum discharge power |
| `n_series` | u32 | 96 | Cells in series (sets pack voltage) |
| `n_parallel` | u32 | 1 | Cells in parallel (sets pack resistance) |
| `cell_resistance_ohm` | f64 | 0.005 | Per-cell internal resistance |
| `self_discharge_pct_per_day` | f64 | 0.02 | Self-discharge rate |
| `standby_power_w` | f64 | 5.0 | Parasitic standby draw (OCHRE fix) |
| `initial_soc` | f64 | 0.5 | Initial state of charge |
| `min_soc` | f64 | 0.0 | Minimum SOC bound |
| `max_soc` | f64 | 1.0 | Maximum SOC bound |
| `heater_power_w` | f64 | 0.0 | Cell heater power (0 = no heater); ~500 W for Franklin aPower 2, ~100 W for Tesla Powerwall 3 |
| `heater_threshold_c` | f64 | 0.0 | Heater activation temperature; Tesla targets 0 C min, Franklin activates ~5-10 C |
| `heater_on_discharge` | bool | false | Also activate heater when discharge is requested but blocked by cold (Tesla-style always-on); default false = Franklin-style charge-only |
| `min_discharge_temp_c` | f64 | -20.0 | Discharge fully blocked below this; matches Powerwall/aPower 2/Enphase specs |
| `full_power_temp_c` | f64 | 10.0 | Full power above this; Tesla and Franklin both require ~10 C for full charge power |
| `min_charge_temp_c` | f64 | 0.0 | Charging blocked below this; Li-ion lithium plating safety limit |
| `cell_thermal_mass_j_per_k` | f64 | 25000 | Lumped pack thermal mass (~25 kg * 1000 J/kg/K) |
| `cell_ua_w_per_k` | f64 | 5.0 | Pack-to-ambient heat loss coefficient |

## Files to Touch
- `crates/hares-equipment/src/battery.rs`: new file — `Battery` struct and full `Equipment` implementation

## Measures of Success
- [x] Charge/discharge SOC trajectory over known power profile matches analytical integral
- [x] Round-trip efficiency (charge then discharge same energy) is less than 1.0 due to ohmic losses
- [x] Self-consumption mode charges when Stage 1 accumulated `active_power_kw` is negative (net generation surplus from PV) and discharges when it is positive (net load exceeds PV)
- [x] `SelfConsumption { solar_only_charging: true }` does not charge from grid import
- [x] Standby power is non-zero and written to Electrical port when battery is neither charging nor discharging
- [x] SOC is clamped at 0.0 and 1.0 and charging/discharging stops at bounds
- [x] Two `Battery` instances with different initial SOCs evolve independently
- [x] `save_state` / `load_state` round-trip preserves SOC, cycle count, and rainflow accumulator
- [x] Cell heater activates when cold and charging is requested; does not activate when warm
- [x] `heater_on_discharge=true` activates heater when discharge is requested at cold temps; default false does not
- [x] Discharge power derates linearly between `full_power_temp_c` and `min_discharge_temp_c`
- [x] Charging is blocked below `min_charge_temp_c`
- [x] Cell temperature evolves toward ambient when idle

## Verification
- [x] `cargo check -p hares-equipment` passes
- [x] `cargo test -p hares-equipment` passes (26 battery tests)
- [x] `cargo clippy -p hares-equipment -- -D warnings` passes
