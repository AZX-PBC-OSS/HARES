# PvConfig physical plausibility constraints: capacity, tilt, azimuth, inverter
**Review ID**: dercat-07
**Category**: der-catalog
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/pv/config.rs` (100 lines + tests)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/PV.py` (259 lines)
- `vendors/EnergyPlus/src/EnergyPlus/PVWatts.cc` (468 lines)
- `vendors/EnergyPlus/src/EnergyPlus/Photovoltaics.cc` (line 365 surface-tilt check)
- `vendors/EnergyPlus/src/EnergyPlus/Photovoltaics.hh`

## Conventions Verified
The HARES codebase consistently uses a **north-referenced clockwise** azimuth convention:
- `crates/hares-types/src/environment.rs:148-151` — `solar_azimuth_deg`: "0=N, 90=E, 180=S, 270=W"
- `crates/hares-physics/src/solar.rs:135-145` — `solar_position()` internally shifts from south- to north-referenced via `+ PI`
- `crates/hares-physics/src/pv_sizing.rs:23` — "0 = north, 180 = south"
- All incidence-angle and shading calculations use the same convention without conversion.
- OCHRE differs: `PV.py:24` uses "0=south, west-of-south=positive" and converts for SAM via `(azimuth + 180) % 360` at line 58.
- EnergyPlus PVWatts passes azimuth directly to SAM, which uses south=180° convention.

The `PvConfig` azimuth convention matches the internal solar position convention. No convention mismatch.

---

## Findings

### Finding 1: [Severity: high] Tilt validation range `[0, 180]` is too permissive — should be `[0, 90]`
**Description**: The `validate()` method allows tilt values up to 180°, which would represent panels facing the ground — physically impossible for solar collection. Tilts from 91° to 180° would silently produce near-zero or nonsensical generation profiles in year-long simulations.
**Code Location**: `crates/hares-equipment/src/pv/config.rs:58-63`
```rust
if let Some(tilt) = self.tilt_deg {
    if !tilt.is_finite() || !(0.0..=180.0).contains(&tilt) {
        return Err(HaresError::Equipment(
            "PV tilt_deg must be finite and within [0, 180]".to_string(),
        ));
    }
}
```
**Root Cause**: The `[0, 180]` range appears to have been adopted from general surface-orientation conventions (where 0° = horizontal ceiling, 180° = floor) rather than PV-specific constraints. A PV array at tilt > 90° receives no direct beam irradiance because its surface normal points below the horizon.
**Vendor Reference**:
- EnergyPlus `PVWatts.cc:118-119` explicitly validates `tilt < 0 || tilt > 90`, rejecting anything outside `[0, 90]`.
- EnergyPlus `Photovoltaics.cc:365` uses surface `Tilt < -95 || > 95` only for the *building surface* itself (which can be a wall at 90° or a slight overhang at 95°), but the PVWatts module — the modern PV model — enforces `[0, 90]` for PV arrays.
- OCHRE derives tilt from roof envelope boundaries, which are inherently in `[0, 90]`.
**Impact**: A user entering `tilt_deg = 120.0` (a plausible copy-paste error) would pass validation but produce near-zero generation year-round with no error or warning, wasting simulation compute on an impossible configuration.

**Recommendation**: Change the range to `(0.0..=90.0)` and update the error message. The existing test at line 163-166 (`cfg.tilt_deg = Some(181.0)`) should also gain a companion test for `Some(91.0)` to verify the 90° upper bound.

---

### Finding 2: [Severity: medium] Missing DC-to-AC ratio validation between `capacity_kw` and `inverter_capacity_kw`
**Description**: No cross-field check ensures the DC-to-AC ratio (inverter loading ratio) falls within a physically reasonable range. The `inverter_capacity_kw` is validated independently for `> 0`, but not compared against `capacity_kw`. A user could configure a 10 kW array with a 0.5 kW inverter (ratio = 20.0, absurdly high clipping) or a 1 kW array with a 100 kW inverter (ratio = 0.01, absurdly oversized), both of which would silently produce distorted output.
**Code Location**: `crates/hares-equipment/src/pv/config.rs:91-96` (inverter_capacity_kw standalone check) — no comparison with `capacity_kw`.
**Root Cause**: When `inverter_capacity_kw` is `None`, the code at `mod.rs:270-273` treats the inverter as having unlimited capacity (no clipping). When specified, any value > 0 passes validation.
**Vendor Reference**:
- EnergyPlus `PVWatts.cc:88` defaults `DCtoACRatio_` to `1.1`, with SAM internally enforcing reasonableness.
- OCHRE `PV.py:122` defaults `inverter_capacity = capacity` (ratio = 1.0).
- Industry standard DC-to-AC ratios for residential systems: 1.0–1.5 (NREL, SAM documentation). A ratio of 0 would imply infinite inverter capacity.
**Impact**: A severely wrong DC-to-AC ratio produces silently wrong generation profiles — either excessive clipping (ratio >> 1.5) or unrealistically high AC output (ratio << 1.0). Both corrupt year-long simulation results.

**Recommendation**: When both `capacity_kw` and `inverter_capacity_kw` are set (i.e., `inverter_capacity_kw` is `Some`), compute the ratio `capacity_kw / inverter_capacity_kw` and reject or warn if outside `[0.8, 2.0]` (a generous but still protective range). Example:
```rust
if let Some(inv_cap) = self.inverter_capacity_kw {
    let dc_ac_ratio = self.capacity_kw / inv_cap;
    if dc_ac_ratio < 0.8 || dc_ac_ratio > 2.0 {
        return Err(HaresError::Equipment(format!(
            "PV DC-to-AC ratio {dc_ac_ratio:.2} is outside reasonable range [0.8, 2.0]"
        )));
    }
}
```

---

### Finding 3: [Severity: low] `noct_c` field lacks range validation in `validate()`
**Description**: The `noct_c` (Nominal Operating Cell Temperature) field has no validation in the `validate()` method. Only a finiteness check exists downstream in `mod.rs:346`. A nonsense value like `-100.0` or `1000.0` would pass config validation and produce wildly incorrect cell-temperature calculations.
**Code Location**: `crates/hares-equipment/src/pv/config.rs:27` (field definition) — no corresponding check in `validate()` (lines 51-99).
**Root Cause**: The `noct_c` field was added as `Option<f64>` but validation was never wired into the `validate()` method. Downstream code (`mod.rs:345`) applies a default of 47°C and checks finiteness only.
**Impact**: The cell-temperature model (`cell_temperature_noct_wind`) uses `noct_c` linearly — an extreme value produces proportionally extreme temperature estimates, derailing generation estimates.

**Recommendation**: Add validation in `validate()` for `noct_c` within a reasonable range, e.g., `[20.0, 60.0]` °C. Typical residential module NOCT values are 40–47°C (cf. SAM default = 47°C at `mod.rs:32`).

---

### Finding 4: [Severity: low] `surface_resolution_deg` lacks validation in `validate()`
**Description**: Similar to `noct_c`, `surface_resolution_deg` has no validation in `validate()`. It is checked downstream in `mod.rs:372` for `> 0` and finiteness, but config-only validation should catch it earlier.
**Code Location**: `crates/hares-equipment/src/pv/config.rs:38` (field definition) — no corresponding check in `validate()`.
**Impact**: The downstream check at `surface_id_for_orientation()` (`array_config.rs:83-86`) will catch invalid values, but the error surfaces only when the PV equipment is initialized, not at config validation time.

**Recommendation**: Add `surface_resolution_deg` validation to `validate()` for consistency: finite and > 0. This makes validation-fail-fast behavior uniform across all fields.

---

## Summary
- **Total findings**: 4
- **Critical**: 0
- **High**: 1 (tilt range)
- **Medium**: 1 (DC-to-AC ratio)
- **Low**: 2 (noct_c, surface_resolution_deg)

## Recommendations
1. Fix tilt validation range from `[0, 180]` to `[0, 90]` and update error message + tests (Finding 1 — high).
2. Add a cross-field DC-to-AC ratio plausibility check in `validate()` when both `capacity_kw` and `inverter_capacity_kw` are set (Finding 2 — medium).
3. Add `noct_c` range validation to `validate()` (Finding 3 — low).
4. Add `surface_resolution_deg` validation to `validate()` (Finding 4 — low).

## References / Citations
- EnergyPlus PVWatts tilt validation: `vendors/EnergyPlus/src/EnergyPlus/PVWatts.cc:117-126`
- EnergyPlus surface tilt bounds: `vendors/EnergyPlus/src/EnergyPlus/Photovoltaics.cc:365`
- OCHRE SAM integration, azimuth convention: `vendors/OCHRE/ochre/Equipment/PV.py:22-58`
- HARES solar position azimuth convention: `crates/hares-physics/src/solar.rs:135-145`
- HARES PvConfig validation: `crates/hares-equipment/src/pv/config.rs:49-99`
- HARES inverter-limiting logic: `crates/hares-equipment/src/pv/mod.rs:269-273`
- HARES `surface_id_for_orientation` validation: `crates/hares-equipment/src/pv/array_config.rs:82-86`
