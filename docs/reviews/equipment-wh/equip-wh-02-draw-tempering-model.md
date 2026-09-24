# Water heater draw tempering mixing valve model
**Review ID**: equip-wh-02
**Category**: equipment-wh
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/water_heater/tank.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/WaterHeater.py
vendors/OCHRE/ochre/Models/Water.py

## Findings
### Finding 1: [Severity: critical]
**Description**: Unmet load (`unmet_load_w`) is computed from post-heating outlet temperature instead of the pre-heating snapshot, causing same-step element heat injection to inflate the apparent outlet temperature and under-report unmet load.

**Code Location**: `tank.rs:410-418` — `unmet_load_w` uses `draw.outlet_temp_c`, which is computed inside `apply_draw()` at `tank.rs:606-607` from `self.node_temps_c` after `apply_conduction_and_standby()` (line 390) and `apply_heat_injections()` (line 393) have already modified node temperatures.

**Root Cause**: In `step_tempered()`, the TMV volume-ratio calculation correctly snapshots the pre-everything top-node temperature at line 357:
```rust
let outlet_est_c = self.node_temps_c[0];
```
But `apply_draw()` at line 606-607 recomputes `outlet_temp_c` from the *current* (post-injection) node temperatures:
```rust
let outlet_temp_c = segment_average_temp(&self.node_edges_m3, &self.node_temps_c, 0.0, draw_volume_m3);
```
This diverges from the `energy_out_j` accounting inside `apply_draw()` at line 624, which correctly uses the pre-injection snapshot:
```rust
let t = self.scratch_pre_injection_temps[idx];
```
The `unmet_load_w` formula at line 410-414 then consumes this post-injection outlet temperature, meaning any element heat added to the drawn segment during the same Euler step reduces the reported deficit:
```rust
let unmet_load_w = if tempered_flow_m3_s > 0.0 {
    let deficit = (tmv.tempered_draw_temp_c - draw.outlet_temp_c).max(0.0);
    tempered_flow_m3_s * water_density_kg_m3(draw.outlet_temp_c)
        * CP_LIQUID_WATER_J_KG_K * deficit
} else { 0.0 };
```

**Impact**: In OCHRE (`Water.py:284,347`), `update_water_draw()` computes the outlet temperature from pre-injection states because water draws are processed *before* heat injections in the same timestep. HARES applies heat injections *before* the draw outlet is sampled, so a 4500W element firing into the top node during a concurrent draw could mask 5-15°C of temperature deficit, under-reporting unmet load by hundreds to thousands of watts. The test `outlet_temp_reflects_post_injection_segment_average` at line 1314 explicitly validates this post-injection behavior for `step()`, confirming it is structural rather than accidental.

**Reproduction** (pseudocode):
- Tank top node = 30°C, fixture setpoint = 40.6°C, element fires 4500W into node 0
- Pre-heating outlet = 30°C, unmet load should be ~4.4 kW
- Post-heating outlet ≈ 35°C (after 60s), reported unmet load ≈ 2.3 kW
- Difference: ~2 kW under-reporting

### Finding 2: [Severity: high]
**Description**: Hot draw TMV gating condition differs from OCHRE, causing HARES to skip tempering for hot draws (clothes washer, dishwasher) in standard configurations and when the tank is temporarily above setpoint.

**Code Location**: `tank.rs:361` — the hot draw TMV guard:
```rust
let hot_draw_volume_m3 = if tmv.setpoint_temp_c > tmv.hot_draw_temp_c {
```
**Compare OCHRE**: `Water.py:305`:
```python
if self.tempered_draw_temp < self.setpoint_temp:
```

**Root Cause**: HARES keys the hot-draw TMV decision on whether the tank *setpoint* exceeds the *hot draw temperature* (`setpoint > hot_draw`). OCHRE keys it on whether the setpoint exceeds the *tempered draw temperature* (`setpoint > tempered_draw`).

**Impact**: In standard US configuration (setpoint=51.7°C, hot_draw=51.7°C, tempered=40.6°C):
- HARES evaluates `51.7 > 51.7` → `false` → **TMV never applied to hot draws**, regardless of actual tank temperature
- OCHRE evaluates `40.6 < 51.7` → `true` → TMV reduces hot-draw volume when outlet exceeds 51.7°C

When the tank is overheated (e.g., solar preheat at 70°C, or element over-firing), HARES will draw the full hot-water volume from the tank even though the outlet temperature far exceeds what the appliance needs, wasting stored thermal energy. OCHRE would instead temper the hot draw by blending with cold mains, reducing tank hot-water depletion.

In practice, when the tank is exactly at setpoint (51.7°C), both approaches produce identical results (no volume reduction). The divergence occurs only when outlet > hot_draw_temp, which can happen with variable heat sources (solar thermal, heat pump, off-peak storage).

### Finding 3: [Severity: medium]
**Description**: `step_tempered()` uses two different outlet temperature values — `outlet_est_c` (snapshot at line 357) and `draw.outlet_temp_c` (from `apply_draw` at line 394) — without clearly documenting which is authoritative for each consumer. The TMV calculation correctly uses the snapshot, but the draw outlet and unmet load use the post-injection value.

**Code Location**: `tank.rs:357` vs `tank.rs:411`

**Root Cause**: The `DrawResult.outlet_temp_c` field is populated by `apply_draw()` at line 667 from the post-injection segment average. The `step_tempered()` caller never overwrites it with the pre-step snapshot (`outlet_est_c`). Any downstream consumer of `DrawResult.outlet_temp_c` (e.g., the water heater controller that uses this to evaluate outlet quality) would see the post-heating value, not the true instantaneous outlet at draw onset.

**Impact**: The `outlet_temp_c` field has a doc-comment at line 41 claiming it is the "Instantaneous outlet temperature at the beginning of the draw," but this is factually incorrect when heat injections are present in the same step. The test at line 1331-1335 confirms post-injection behavior:
```rust
assert!((draw.outlet_temp_c - expected_top).abs() < 0.01,
    "outlet ({:.4}) must reflect post-injection top-node temp ({expected_top:.4})",
```
Either the doc comment or the implementation is wrong. If the post-injection value is the intended semantics (as OCHRE's `_water_draw_general` does update `outlet_temp` from the draw computation on line 347), then the doc comment and the "pre-heating" claim on line 298 should be corrected.

### Finding 4: [Severity: low]
**Description**: The TMV volume-ratio denominator guard at lines 368 and 380 uses `.max(1e-9)` to prevent division by zero when the outlet approaches the mains temperature:
```rust
let vol_ratio = (tmv.tempered_draw_temp_c - mains_temp_c)
    / (outlet_est_c - mains_temp_c).max(1e-9);
```
The preceding `if outlet_est_c <= tmv.tempered_draw_temp_c` branch already handles the case where the outlet is at or below the fixture target (including the `outlet ≈ mains` case), so the denominator should never be zero in the `else` branch under normal conditions. However, the guard is harmless and semantically correct.

In an edge case where `outlet_est_c` is very close to (but not exactly equal to) `mains_temp_c` while also exceeding `tmv.tempered_draw_temp_c` — a physically impossible scenario requiring `mains_temp_c > tmv.tempered_draw_temp_c` — the `.max(1e-9)` prevents an infinite `vol_ratio` which the `.clamp(0.0, 1.0)` would otherwise clamp. The guard adds robustness at no runtime cost.

**Code Location**: `tank.rs:368,380`

**Impact**: No current impact. This is a defensive coding note only.

## Summary
- Total findings: 4
- Critical: 1 (unmet load uses post-heating outlet temp)
- High: 1 (hot draw TMV gating differs from OCHRE)
- Medium: 1 (dual-outlet-temp ambiguity / misleading doc)
- Low: 1 (denominator guard is harmless but noteworthy)

## Recommendations
1. **Fix the unmet load calculation** (Finding 1): Compute `unmet_load_w` from the pre-step outlet snapshot (`outlet_est_c` at line 357) rather than from `draw.outlet_temp_c`. This aligns with OCHRE's approach where water draw is evaluated against pre-injection tank states. The formula maps `outlet_est_c` against `tmv.tempered_draw_temp_c` the same way OCHRE does at `Water.py:363`.
2. **Correct the hot draw TMV gate** (Finding 2): Change the condition at line 361 from `tmv.setpoint_temp_c > tmv.hot_draw_temp_c` to `tmv.setpoint_temp_c > tmv.tempered_draw_temp_c` to match OCHRE's logic, or alternatively use `outlet_est_c > tmv.hot_draw_temp_c` to unconditionally enable TMV for hot draws when the tank is hotter than the hot delivery target.
3. **Clarify or fix `DrawResult.outlet_temp_c` semantics** (Finding 3): Either (a) store the pre-step snapshot in `outlet_temp_c` and let callers that need the segment-average temperature compute it separately, or (b) update the doc comment at line 41 and the "pre-heating" claim at line 298 to accurately describe the post-injection behavior. If changing behavior, add a test with nonzero heat injection + nonzero draw that verifies `outlet_temp_c` reflects the pre-heating value.

## References / Citations
- OCHRE `Water.py:280-365`: `update_water_draw()` — water draws computed from pre-injection states; TMV volume ratio at lines 305-325; unmet load at line 363.
- OCHRE `Water.py:284`: `self.outlet_temp = self.states[self.t_1_idx]` — outlet snapshot at start of water draw, before any this-step heat injection.
- OCHRE `Water.py:305`: `if self.tempered_draw_temp < self.setpoint_temp:` — hot-draw TMV gate keys off tempered draw temperature, not hot draw temperature.
- HARES `tank.rs:357`: `let outlet_est_c = self.node_temps_c[0];` — correct pre-step snapshot for TMV volume ratio.
- HARES `tank.rs:390-394`: Sequence is `apply_conduction_and_standby → snapshot pre_injection → apply_heat_injections → apply_draw`, meaning `apply_draw` sees post-injection temps.
- HARES `tank.rs:606-607`: `segment_average_temp` computes `outlet_temp_c` from current `node_temps_c` (post-conduction, post-injection).
- HARES `tank.rs:624`: `let t = self.scratch_pre_injection_temps[idx]` — `energy_out_j` correctly uses pre-injection temperatures, creating an inconsistency with `outlet_temp_c`.
- HARES `tank.rs:1314-1344`: test `outlet_temp_reflects_post_injection_segment_average` explicitly validates that element heat injected in the same step DOES appear in `outlet_temp_c`.
