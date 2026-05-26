# Battery pack voltage and topology derivation from cell parameters
**Review ID**: dercat-02
**Category**: der-catalog
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/battery/config.rs` — Typed `BatteryConfig` struct (lines 1–255)
- `crates/hares-equipment/src/battery/mod.rs` — Topology derivation in `init_typed` (lines 672–741), electrical model in `compute_electrical` (lines 466–535), defaults (lines 103–151)
- `crates/hares-equipment/src/battery/ocv.rs` — Per-cell OCV curves (lines 1–384)
- `crates/hares-equipment/src/battery/catalog.rs` — Catalog product specs and test assertions (lines 121–710)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/Battery.py` — Cell topology derivation and voltage model (lines 91–102, 289–301)
- `vendors/OCHRE/ochre/defaults/Battery/default_parameters.csv` — Cell parameter defaults
- `vendors/EnergyPlus/src/EnergyPlus/ElectricPowerServiceManager.cc` — KiBaM and Li-Ion storage models with integer cell count constraints (lines 3270–3344, 4106–4111)
- `vendors/EnergyPlus/third_party/ssc/shared/lib_battery_voltage.cpp` — Tremblay voltage model pack-level derivation (lines 71–75, 324–368)
- `vendors/EnergyPlus/third_party/ssc/ssc/cmod_battwatts.cpp` — Auto-sizing with `std::ceil()` and topology recomputation (lines 74–170)

## Findings

### Finding 1: Explicit topology silently overridden by cell-parameter derivation without warning or consistency check
**Severity**: high
**Description**: When a user provides *both* explicit `n_series`/`n_parallel` values *and* `ah_cell`/`v_cell` cell parameters, the explicit topology is set at lines 688–689, then unconditionally overwritten at lines 692–701 by the cell-parameter derivation path. No validation or merge logic exists to detect the conflict, compare the two sets of values, or emit a warning. The user's explicit topology is silently discarded in favor of a coarse `round()`-based derivation from a hardcoded 350 V target.
**Code Location**: `crates/hares-equipment/src/battery/mod.rs:688-703`
```rust
self.n_series = c.n_series.unwrap_or(DEFAULT_N_SERIES);          // line 688 — explicit value set here
self.n_parallel = c.n_parallel.unwrap_or(DEFAULT_N_PARALLEL);     // line 689
if let (Some(ah_cell), Some(v_cell)) = (c.ah_cell, c.v_cell) {    // line 690
    if ah_cell > 0.0 && v_cell > 0.0 {
        let target_pack_v = c.pack_voltage_v.unwrap_or(350.0);
        self.n_series = (target_pack_v / v_cell).round() as u32;   // line 693 — overwrites line 688
        ...
        self.n_parallel = (pack_ah / ah_cell).round() as u32;      // line 699 — overwrites line 689
        ...
    }
}
```
**Root Cause**: The derivation path at line 690 does not check whether `c.n_series` or `c.n_parallel` was already explicitly provided. It unconditionally recomputes both values from `ah_cell`/`v_cell` regardless of existing user intent.
**Impact**: A user who carefully specifies `n_series=14, n_parallel=3, ah_cell=70, v_cell=3.6` for a small 48 V pack will get `n_series=97, n_parallel=1` instead (derived from the 350 V target). The pack resistance shifts from `0.005 * 14 / 3 ≈ 0.023 Ω` to `0.005 * 97 / 1 = 0.485 Ω` — a 20× difference that completely invalidates the electrical model for the intended use case. EnergyPlus avoids this by using user-specified topology directly (`ElectricPowerServiceManager.cc:3270–3272`) with no automatic derivation step.

### Finding 2: Pack voltage shift from integer rounding is not validated against the target voltage
**Severity**: medium
**Description**: `n_series` is computed as `round(target_pack_v / v_cell)` at line 693, producing a cell count that is always an integer (good). However, the resulting actual pack voltage `pack_v = n_series * v_cell` (line 697) can differ from `target_pack_v` by up to `v_cell / 2` volts. This shifted voltage is then used to compute `pack_ah` and subsequently `n_parallel` (lines 698–699). The derived `pack_v` is never compared against `target_pack_v` or `pack_voltage_v` for tolerance, and no validation ensures the rounding-induced shift is acceptable.
**Code Location**: `crates/hares-equipment/src/battery/mod.rs:692-697`
```rust
let target_pack_v = c.pack_voltage_v.unwrap_or(350.0);
self.n_series = (target_pack_v / v_cell).round() as u32;
if self.n_series == 0 { self.n_series = 1; }
let pack_v = self.n_series as f64 * v_cell;
```
**Root Cause**: IEEE 754 `round()` ties-to-even rounding on a floating-point division produces integer cell counts, but the resulting pack voltage deviates from the target. No post-derivation tolerance check exists.
**Impact**: For common cell voltages:
- LFP 3.2 V cell, 350.0 V target: `n_series = round(109.375) = 109`, `pack_v = 348.8 V` (1.2 V shift, 0.34% error)
- LFP 3.2 V cell, 50.4 V target: `n_series = round(15.75) = 16`, `pack_v = 51.2 V` (0.8 V shift, 1.6% error)
- NMC 3.6 V cell, 350.0 V target: `n_series = round(97.22) = 97`, `pack_v = 349.2 V` (0.8 V shift, 0.23% error)

These shifts are small but systematic — the SOC-OCV curve interpolated at a given SOC maps to a per-cell voltage that is multiplied by the *actual* series count (line 480), so the voltage domain is self-consistent. However, if any downstream component expects the *nominal* pack voltage (e.g., for sizing calculations), the discrepancy could cause errors. Both EnergyPlus and OCHRE avoid this: EnergyPlus uses explicit integer `seriesNum_` from user input, and OCHRE keeps `n_parallel` as a float (`Battery.py:94: "not necessarily an integer"`) so there is no rounding-induced voltage shift.

### Finding 3: Derived `n_parallel` may round to zero or produce topology inconsistent with declared capacity
**Severity**: high
**Description**: `n_parallel = round(pack_ah / ah_cell)` at line 699 can round to zero when `pack_ah < 0.5 * ah_cell` (i.e., when a single cell's Ah capacity exceeds half the total pack Ah requirement). The zero-guard at line 700 clamps to 1, but this produces a topology whose physical capacity (`n_parallel * ah_cell * n_series * v_cell / 1000`) can be wildly different from the declared `capacity_kwh`. The simulation then uses `capacity_kwh` for energy accounting (line 681) but the topology for voltage and resistance — an inconsistency that makes the electrical model physically invalid.
**Code Location**: `crates/hares-equipment/src/battery/mod.rs:697-701`
```rust
let pack_ah = self.capacity_kwh * 1000.0 / pack_v;
self.n_parallel = (pack_ah / ah_cell).round() as u32;
if self.n_parallel == 0 {
    self.n_parallel = 1;   // <— clamped, but topology now grossly inconsistent
}
```
**Root Cause**: The derivation does not validate that the resulting topology's implied capacity matches `capacity_kwh`. After deriving `n_series` and `n_parallel`, the code never computes `n_parallel * ah_cell * n_series * v_cell / 1000` to check for consistency. The problem arises when `ah_cell` is large relative to the pack design (e.g., a 100 Ah prismatic cell for a 5 kWh pack).
**Impact**: With `ah_cell=100, v_cell=3.6, capacity_kwh=13.5, target=350V`:
- `n_series = 97`, `pack_v = 349.2 V`, `pack_ah = 38.66 Ah`
- `n_parallel = round(0.387) = 0` → clamped to 1
- Physical capacity of derived topology = `1 * 100 * 97 * 3.6 / 1000 = 34.92 kWh` vs declared `13.5 kWh`
- Pack resistance `R = cell_R * 97 / 1 = 0.485 Ω` instead of the correct `0.005 * 97 / 0.39 ≈ 1.24 Ω` — a 2.6× underestimate

EnergyPlus's `cmod_battwatts.cpp` (line 133) handles this correctly by using `ceil()` and recalculating the actual kWh back from the integer topology. OCHRE avoids this entirely by using floating-point `n_parallel` so there is no rounding-induced capacity shift.

### Finding 4: No cabinet-level physical limit check on total cell count
**Severity**: low
**Description**: The derivation produces `n_series` and `n_parallel` without any upper bound on `n_series * n_parallel` (total cell count). Residential battery cabinets have physical volume constraints — a single Powerwall 3 contains ~872 cells (109S8P). Unbounded cell counts from pathological inputs (e.g., extremely low `ah_cell` with high `capacity_kwh`) can produce thousands of cells, driving resistance unrealistically low and thermal mass unrealistically high.
**Code Location**: `crates/hares-equipment/src/battery/mod.rs:692-701` — no upper bound enforcement.
**Root Cause**: The derivation is designed for well-formed inputs but does not guard against physically impossible configurations.
**Impact**: Marginal in practice (realistic cell parameters produce plausible topologies), but a validation gate would prevent silently wrong results from bad input data. EnergyPlus validates topology inputs with explicit fatal errors for zero/negative values (`ElectricPowerServiceManager.cc:3293–3319`), and the auto-sizing in `cmod_battwatts.cpp` bounds the computed topology by the `ceil()` constraint.

### Finding 5: `pack_voltage_v` config field is inaccessible after init and not validated for positivity
**Severity**: low
**Description**: The `pack_voltage_v` field on `BatteryConfig` (config.rs:32) is consumed only in `init_typed` at line 692 as the target for topology derivation. It is not stored on the `Battery` struct and cannot be read back to verify what target was used. When neither `ah_cell`/`v_cell` are provided (the catalog entry path: catalog.rs:138–143), `pack_voltage_v` is silently ignored — no warning is emitted. Additionally, `validate()` in config.rs never checks that `pack_voltage_v.map(|v| v > 0.0)`, so a negative or zero target voltage passes validation silently.
**Code Location**:
- Field: `crates/hares-equipment/src/battery/config.rs:32`
- Consumption: `crates/hares-equipment/src/battery/mod.rs:692`
- Missing validation: `crates/hares-equipment/src/battery/config.rs:81-147`
**Impact**: Users cannot verify post-init what target voltage was used for derivation. A negative `pack_voltage_v` (e.g., `-350.0`) would produce a negative `n_series` after `round()` and `as u32`, wrapping to a large positive value — a silently corrupt topology. The catalog path (`ah_cell=None, v_cell=None, pack_voltage_v=None`) correctly avoids this, but the raw config path is vulnerable.

### Finding 6: Electrical model correctly uses topology-adjusted voltage — SOC-OCV accuracy is preserved
**Severity**: low (positive finding)
**Description**: The `compute_electrical` function at line 480 computes `pack_ocv = cell_ocv * self.n_series as f64`, using the actual topology-derived series count rather than any nominal target voltage. This means that once `n_series` is set (however derived), the SOC-OCV curve lookup is internally consistent: the per-cell OCV from `OcvTable::voltage_at_soc(soc)` (ocv.rs) is multiplied by the actual integer series count. Any rounding error from topology derivation is absorbed into the `n_series` value and the electrical model remains self-consistent thereafter.
**Code Location**: `crates/hares-equipment/src/battery/mod.rs:479-482`
```rust
let cell_ocv = self.ocv_table.voltage_at_soc(self.soc);
let pack_ocv = cell_ocv * self.n_series as f64;
let pack_resistance =
    self.cell_resistance_ohm * self.n_series as f64 / self.n_parallel as f64;
```
**Impact**: This correctly mirrors both vendor implementations — OCHRE uses `voc * self.n_series` at `Battery.py:289`, and EnergyPlus/SSC uses `num_cells_series * cell_voltage` at `lib_battery_voltage.cpp:71`. The HARES implementation correctly follows the same pattern.

### Finding 7: `validate()` has no cross-field consistency checks for topology fields
**Severity**: medium
**Description**: The `validate()` method in config.rs (lines 81–147) validates each field independently (non-zero, non-negative, finite, range checks) but performs zero cross-field validations. Missing checks include:
- `pack_voltage_v` vs. `n_series * v_cell`: when all three are provided, no check that they are mutually consistent
- `capacity_kwh` vs. topology: no check that `n_parallel * ah_cell * n_series * v_cell / 1000` is within tolerance of `capacity_kwh`
- `n_series` count vs. `v_cell` defaults: with explicit `n_series` but no `v_cell`, the actual pack voltage range is unvalidated against any expected range
**Code Location**: `crates/hares-equipment/src/battery/config.rs:81-147`
**Root Cause**: `validate()` is designed as a simple field-level sanity checker, not a comprehensive consistency validator. The topology derivation in `init_typed` at mod.rs:690–703 performs the computation but does not feed results back to the config for validation.
**Impact**: Configurations that pass `validate()` can produce silently wrong behavior at init time when topology derivation produces unexpected results (see Findings 1 and 3). EnergyPlus avoids this by performing cross-field validation at input parsing time (`ElectricPowerServiceManager.cc:3304–3319` checks voltage hierarchy; lines 3326–3339 check capacity hierarchy).

## Summary
- **Total findings**: 7
- **High**: 2 (Findings 1, 3)
- **Medium**: 3 (Findings 2, 4, 7)
- **Low**: 2 (Findings 5, 6)

## Recommendations
1. **Add conflict detection for co-provided topology fields** (Finding 1). When both explicit `n_series`/`n_parallel` AND `ah_cell`/`v_cell` are set, emit a warning and prefer the explicit topology. Alternatively, validate that both paths produce the same results within tolerance.
2. **Validate topology-derived capacity consistency** (Finding 3). After deriving `n_series` and `n_parallel` at init, compute `implied_capacity = n_parallel * ah_cell * n_series * v_cell / 1000` and compare against `capacity_kwh`. Warn or error if the discrepancy exceeds a threshold (e.g., 5%).
3. **Replace zero-clamp on `n_parallel` with an error** (Finding 3). If `pack_ah / ah_cell < 0.5`, the cell parameters are incompatible with the pack capacity — emit a clear error rather than silently clamping to 1.
4. **Validate `pack_voltage_v > 0` in `validate()`** (Finding 5).
5. **Add post-derivation tolerance check on `pack_v` vs `target_pack_v`** (Finding 2). If the shift exceeds `v_cell` (more than one cell's worth of voltage), emit a warning.
6. **Add an upper bound on total cell count** (Finding 4). A reasonable limit for residential cabinets is ~2,000 cells (e.g., a stacked multi-unit system).
7. **Add cross-field consistency checks to `validate()`** (Finding 7). At minimum, when `n_series`, `v_cell`, and `pack_voltage_v` are all set, confirm `|n_series * v_cell - pack_voltage_v| ≤ v_cell`.

## References / Citations
- OCHRE `Battery.py:91-102`: Floating-point cell counts with explicit non-integer comment; no rounding applied.
- OCHRE `Battery.py:289`: `voc = float(self.voc_curve(self.soc)) * self.n_series` — pack-level OCV from per-cell curve × series count.
- EnergyPlus `ElectricPowerServiceManager.cc:3270-3272`: `parallelNum_` and `seriesNum_` read from user input, stored as `int`; `numBattery_ = parallelNum_ * seriesNum_`.
- EnergyPlus `ElectricPowerServiceManager.cc:4201-4204`: Pack voltage from SSC: `batteryVoltage_ = ssc_battery_->V()` where SSC computes `num_cells_series * cell_voltage` (`lib_battery_voltage.cpp:71`).
- EnergyPlus SSC `cmod_battwatts.cpp:133-170`: Auto-sizing uses `std::ceil()` for both series and parallel, then recalculates `batt_kwh` from the integer topology to ensure consistency.
- HARES OCV tables (`ocv.rs:37-45`): 51-point per-cell OCV curves from 2.5 V to 4.2 V (NMC), multiplied by `n_series` in `compute_electrical` at `mod.rs:480`.
