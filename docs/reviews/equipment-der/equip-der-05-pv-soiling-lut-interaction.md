# PV soiling with PVWatts vs SAM LUT paths consistency
**Review ID**: equip-der-05
**Category**: equipment-der
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/pv/mod.rs crates/hares-equipment/src/pv/soiling.rs crates/hares-equipment/src/pv/lut.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/PV.py

## Findings
### Finding 1: [Severity: medium]
**Description**: In the SAM LUT path, soiling (and shading) derating is applied to the LUT's AC output rather than at the irradiance/DC level. In the PVWatts path, soiling correctly reduces effective irradiance *before* cell temperature and DC power computation, flowing naturally through the inverter. In the LUT path, `step_one_array` at `mod.rs:227-231` multiplies the LUT's AC result by `soiling_ratio`:

```rust
let ac_power_kw = lut
    .interpolate(month, hour, ghi, dni, dhi, ambient_temp_c)
    .max(0.0)
    * soiling_ratio          // <-- applied to AC post-LUT
    * shading_factor;
```

The LUT already encapsulates the inverter model (SAM generates LUTs at the AC interface), so this effectively models soiling as a post-inverter loss.

**Code Location**: `crates/hares-equipment/src/pv/mod.rs:211-244`

**Root Cause**: SAM LUTs are indexed on weather-station irradiance (GHI/DNI/DHI), not plane-of-array irradiance, so soiling cannot be folded into the LUT input variables. The comment at `mod.rs:224-226` correctly identifies the constraint but applies soiling on the wrong side of the inverter boundary. For a **constant scalar inverter efficiency** the final AC output is mathematically equivalent (`(DC × eff) × soiling_ratio = (DC × soiling_ratio) × eff` due to commutativity). However, this breaks down if the LUT models part-load inverter efficiency (which SAM does). The DC telemetry back-calculation at line 232 (`dc_power_kw = ac_power_kw / self.inverter_efficiency`) also becomes physically inconsistent—the reported DC power mixes soiling into a value that doesn't represent true DC-side losses.

**Impact**: For constant-efficiency inverters the AC result is correct by coincidence of commutativity. For LUTs that embed part-load efficiency curves, the soiling derating bypasses the non-linear portion of the curve. The DC telemetry field is misleading whenever soiling is active.

### Finding 2: [Severity: medium]
**Description**: The soiling model is effectively dead code at runtime. `SoilingConfig` is never set to `Some(...)` during normal initialization. `init_typed()` at `mod.rs:417-418` explicitly sets both fields to `None`. The `PvConfig` typed config struct (`config.rs:13-41`) has no soiling parameter. The Python-to-Rust bridge (`pv_config_from_py` at `py_dwelling.rs:419-436`) ignores the `PyPv.soiling: Option<PyPvSoilingConfig>` field. There is no `set_soiling_config()` method on the `Equipment` trait.

**Code Location**:
- `crates/hares-equipment/src/pv/config.rs:13-41` — `PvConfig` lacks a soiling field
- `crates/hares-equipment/src/pv/mod.rs:417-418` — `init_typed()` nulls soiling config/state
- `crates/hares-python/src/py_dwelling.rs:419-436` — `pv_config_from_py` ignores `.soiling`

**Root Cause**: The soiling module was implemented and tested in isolation but never wired through the config pipeline. The only activation path is through `load_state()` (`mod.rs:616-617`), which restores a pre-existing checkpoint, but there's no way to create the initial checkpoint with soiling enabled.

**Impact**: Users cannot activate the Kimber soiling model despite the implementation being present and tested. The `PyPvSoilingConfig` Python class exists and is documented, but users who configure it will observe no soiling effect.

### Finding 3: [Severity: low]
**Description**: The Kimber soiling model uses linear accumulation with a hard `max_soiling` cap rather than an exponential asymptote. Kimber et al. (2006) describe a steady-state equilibrium where dust deposition competes with wind resuspension, producing an exponential approach to maximum soiling. HARES at `soiling.rs:152-154` accumulates linearly and clamps:

```rust
self.soiling_loss += config.soiling_loss_rate_per_s * dt_s;
self.soiling_loss = self.soiling_loss.min(config.max_soiling);
```

**Code Location**: `crates/hares-equipment/src/pv/soiling.rs:148-154`

**Root Cause**: The linear model with cap is a reasonable first-order approximation that avoids per-site dust resuspension rate calibration. The `max_soiling` default of 0.30 prevents unbounded divergence in extended dry periods.

**Impact**: For dry periods shorter than `max_soiling / rate` (≈200 days at the default 0.0015/day suburban rate), the linear and exponential models diverge by <5 percentage points. For longer dry periods (rare in most climates), the exponential would approach the cap smoothly while the linear model hits it abruptly.

### Finding 4: [Severity: low]
**Description**: Extended periods of persistent rain just below the cleaning threshold produce perpetual soiling accumulation. The Kimber model checks `accumulated_rain >= cleaning_threshold_m` at `soiling.rs:140`. If a climate experiences daily drizzle of e.g. 5 mm (below the 6 mm default threshold), the rolling window never crosses the threshold, `seconds_since_last_clean` advances past the grace period, and soiling accumulates normally—despite continuous moisture that should suppress dust. The model has no concept of partial cleaning from sub-threshold rain events.

**Code Location**: `crates/hares-equipment/src/pv/soiling.rs:138-155`

**Root Cause**: The binary threshold test is faithful to the Kimber 2006 algorithm, which was designed for arid/semi-arid climates with distinct rain events separated by long dry spells. The model has no mechanism for progressive cleaning.

**Impact**: In persistently humid or drizzly climates, the Kimber model may over-predict soiling. The `max_soiling` cap limits the worst-case error to 30% loss. Climate-specific tuning of `rain_accum_period_s` and `cleaning_threshold_m` can mitigate this.

### Finding 5: No vendor comparison available [Informational]
**Description**: OCHRE's `PV.py` does not implement a soiling accumulation model. It delegates entirely to PySAM's `Pvwattsv8` module (`PV.py:52-71`), which applies soiling as a fixed system loss percentage within SAM's internal pipeline. There is no Kimber rain-reset model, no rolling rainfall buffer, and no dynamic soiling state in the OCHRE reference implementation.

**Code Location**: `vendors/OCHRE/ochre/Equipment/PV.py:1-259` (entire file)

**Impact**: HARES's dynamic soiling model is a novel capability beyond the vendor reference. There is no reference behavior to validate the accumulation logic against.

## Summary
- Total findings: 5
- Critical: 0
- High: 0
- Medium: 2 (Finding 1: LUT-path AC-side soiling application; Finding 2: Soiling model dead code)
- Low: 2 (Finding 3: Linear vs. exponential accumulation; Finding 4: Sub-threshold rain behavior)
- Informational: 1 (Finding 5: OCHRE has no comparable soiling model)

## Recommendations
1. **Wire soiling config through the initialization pipeline.** Add a `soiling` field to `PvConfig`, pass it through `init_typed()`, construct `SoilingState` at init time. This is the blocker preventing any user from using the soiling model (Finding 2).

2. **Move LUT-path soiling derating to DC side, or compute DC power directly.** Extract the LUT's DC power by dividing the raw LUT output by the SAM-embedded inverter efficiency before applying soiling, then re-convert through HARES' inverter model. If the LUT format doesn't expose DC, at minimum add a comment documenting the commutativity assumption and its limits (Finding 1). Alternatively, apply soiling as an irradiance reduction to a subsidiary POA irradiance not used for LUT indexing—the current code already computes `irradiance_w_m2` at line 206-208 for cell temperature telemetry but doesn't use it in the LUT DC computation.

3. **Consider exponential accumulation** (`soiling(t) = max_soiling * (1 - exp(-rate/max_soiling * t))`) for physical fidelity in multi-year dry spells, or document that the linear model's error grows beyond ~5% after ~100 dry days (Finding 3).

4. **Consider sub-threshold rain attenuation**—e.g., scale the soiling accumulation rate by `(1 - accumulated_rain / cleaning_threshold)` when accumulated_rain is >0 but below the threshold—to avoid unrealistic soiling in drizzly climates (Finding 4).

## References / Citations
- Kimber, A., Mitchell, L., Nogradi, S., Wenger, H. (2006). "The Effect of Soiling on Large Grid-Connected Photovoltaic Systems in California and the Southwest Region of the United States." *IEEE 4th World Conference on Photovoltaic Energy Conversion*. DOI: 10.1109/WCPEC.2006.279690 — cited in `soiling.rs:7-11`
- PVWatts v8 Technical Reference, NREL/TP-7A40-80694 — cited in `mod.rs:56`
- SAM PV model documentation: https://pvpmc.sandia.gov/modeling-guide/2-dc-module-iv/
