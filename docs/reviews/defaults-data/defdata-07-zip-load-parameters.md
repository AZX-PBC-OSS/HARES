# ZIP load parameters: Z, I, P coefficients and reactive power defaults
**Review ID**: defdata-07
**Category**: defaults-data
**Date**: 2026-05-26

## Files Reviewed
defaults/zip_parameters.toml

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/defaults/ZIP Parameters.csv
vendors/OCHRE/ochre/Equipment/Equipment.py (lines 65-77, 200-218)
vendors/OCHRE/test/test_equipment/test_equipment.py (lines 160-209)

## Findings

### Finding 1: [Severity: high] Scheduled load reactive power formula misinterprets `pf` as raw scalar instead of power factor
**Description**: The scheduled load ZIP implementation at `crates/hares-equipment/src/scheduled_load.rs:505` computes reactive power as:
```rust
let reactive_kvar = real_kw * self.zip.pf * reactive_base;
```
This treats the `pf` parameter as a raw scalar multiplier. However, `pf` is named "power factor" and represents cos(φ), so the correct reactive-to-real-power ratio is `tan(acos(pf))`, not `pf` itself. The water heater implementation at `crates/hares-equipment/src/water_heater/mod.rs:320-321` correctly converts pf:
```rust
let tan_phi = self.pf.clamp(-1.0, 1.0).acos().tan();
let reactive_kvar = actual_w / 1_000.0 * tan_phi * reactive_base;
```
At nominal voltage (reactive_base = 1.0), the impact varies by equipment type:

| Equipment | pf | Scheduled (raw pf) | Correct tan(acos(pf)) | Error |
|-----------|-----|-----|------|-------|
| Clothes dryer | 0.99 | Q = 0.99P | Q = 0.14P | 7x overestimate |
| Dishwasher | 0.99 | Q = 0.99P | Q = 0.14P | 7x overestimate |
| Electric baseboard | 1.00 | Q = 1.00P | Q = 0.00P | Infinite (should be 0) |
| Lighting | 1.00 | Q = 1.00P | Q = 0.00P | Infinite (should be 0) |
| Pool pump | 0.84 | Q = 0.84P | Q = 0.65P | 29% overestimate |
| Ventilation fan | 0.87 | Q = 0.87P | Q = 0.57P | 53% overestimate |
| Clothes washer | 0.65 | Q = 0.65P | Q = 1.17P | 44% underestimate |

All 30 equipment types are affected — only the water heater code path uses the correct formula. The electrical solver (`crates/hares-envelope/src/electrical_solver.rs:123`) passes reactive power through unchanged, so the incorrect values propagate directly to net reactive power calculations.

**Code Location**: `crates/hares-equipment/src/scheduled_load.rs:505`

**Root Cause**: The `ZipCoefficients` struct documentation at `scheduled_load.rs:75` explicitly states `reactive_power = real_power * pf * (Zq*v² + Iq*v + Pq)`, treating `pf` as a raw multiplier. This diverges from:
- The OCHRE reference (`Equipment.py:74`) which constructs `pf_mult = np.tan(np.arccos(kwargs["pf"]))` — converting pf to tan(φ) at init time.
- The HARES water heater model which correctly converts pf at application time.
- Standard power engineering convention where "power factor" means cos(φ).

**Impact**: Reactive power is significantly overestimated for all near-unity-power-factor scheduled loads (resistive heating elements, lighting, dryer, dishwasher, range) and incorrectly non-zero for purely resistive loads. This distorts total feeder reactive power, apparent power, and the voltage/reactive-power feedback loop. For equipment with pf < 0.85, the error is smaller but still wrong.

---

### Finding 2: [Severity: high] Real/reactive ZIP coefficient assignment differs from OCHRE reference; undocumented divergence
**Description**: HARES water heater (`water_heater/mod.rs:315-317`) maps real-power coefficients (zp, ip, pp) to real-power scaling and reactive-power coefficients (zq, iq, pq) to reactive-power scaling — an intuitive, physically natural assignment. OCHRE (`Equipment.py:217-218`) does the opposite:
```python
self.reactive_kvar = self.electric_kw * pf_mult * zip_p.dot(v_quadratic)  # zip_p = [Zp,Ip,Pp]
self.electric_kw    = self.electric_kw        * zip_q.dot(v_quadratic)  # zip_q = [Zq,Iq,Pq]
```
OCHRE scales real power by the Zq/Iq/Pq (reactive-labelled) coefficients and reactive power by the Zp/Ip/Pp (real-labelled) coefficients. At nominal voltage (v = 1.0 pu), both approaches give identical results since both coefficient triples sum to 1.0. At non-nominal voltages, results diverge significantly:

| Equipment | Voltage | HARES real mult. | OCHRE real mult. | Difference |
|-----------|---------|-------------------|-------------------|------------|
| ashp_heater | 0.95 pu | 0.979 | 0.744 | −24% |
| air_conditioner | 0.95 pu | 0.979 | 0.834 | −15% |
| refrigerator | 0.95 pu | 0.934 | 0.731 | −22% |

The HARES scheduled load at `scheduled_load.rs:500-505` uses the same assignment as the water heater (real→real, reactive→reactive), so the divergence from OCHRE is consistent within HARES.

**Code Location**: `crates/hares-equipment/src/water_heater/mod.rs:315-317` vs `vendors/OCHRE/ochre/Equipment/Equipment.py:217-218`

**Root Cause**: The coefficient assignment in the CSV and TOML uses a naming convention where `_p`-suffixed coefficients (Zp, Ip, Pp) are for real power and `_q`-suffixed coefficients (Zq, Iq, Pq) are for reactive power. HARES follows this naming convention literally. OCHRE's code swaps them — it may be a bug in OCHRE or a deliberate (but undocumented) convention. Without the original cited papers (Hajagos & Danai 1998, Bokhari et al. 2014), it is unclear which assignment is physically correct.

**Impact**: Medium — at nominal voltage the results are identical. At practical voltage deviations (0.95–1.05 pu), the difference is 15–24% for motor-type loads. This should be documented as a known divergence from the reference implementation, and the original papers should be consulted to determine the correct coefficient assignment.

---

### Finding 3: [Severity: low] All Z+I+P coefficient triples sum to exactly 1.0; reactive triples also sum to 1.0
**Description**: Verified that for all 30 equipment types in `defaults/zip_parameters.toml`:
- Real power: `zp + ip + pp = 1.0` for every load type (within 1e-12 tolerance).
- Reactive power: `zq + iq + pq = 1.0` for every load type (within 1e-12 tolerance).

No violations were found. This was previously noted in config-io-02 Finding 9, which identified the large-magnitude cancellation in refrigerator (zp=5.03, ip=−8.48, pp=4.45) and similar entries.

**Code Location**: `defaults/zip_parameters.toml` (all 30 TOML sections)

**Root Cause**: The coefficients are correctly sourced from the OCHRE defaults CSV, which in turn cites published literature (Hajagos & Danai 1998, Bokhari et al. 2014, Lu et al. 2008, Arif et al. 2013). The extreme values (e.g., refrigerator reactive iq=−28.62) are physically meaningful because the large negative current-fraction term offsets the large impedance and power terms at typical operating voltages.

**Impact**: None — coefficients are correct as supplied. The extreme-magnitude entries are a documentation concern (previously raised in config-io-02 Finding 9) but not a data error.

---

### Finding 4: [Severity: medium] Compressor/motor load power factors are at upper bound of typical ranges
**Description**: Several compressor and motor load types have power factors that are high compared to traditional equipment:

| Equipment | pf | Typical range | Status |
|-----------|-----|---------------|--------|
| Air conditioner | 0.96 | 0.60–0.85 | High |
| HPWH | 0.97 | 0.60–0.85 | High |
| Dehumidifier | 0.96 | 0.60–0.85 | High |
| Pool/Spa/Well pump | 0.84 | 0.60–0.85 | Upper bound |
| Ventilation/Ceiling fan | 0.87 | 0.60–0.85 | Slightly high |
| Refrigerator/Freezer | 0.80 | 0.60–0.80 | At bound |
| Clothes washer | 0.65 | 0.60–0.80 | Realistic |

The Bokhari et al. 2014 paper experimentally measured these values from modern equipment. Modern inverter-driven compressors with active power factor correction (PFC) can achieve pf > 0.95, so values at the upper end are plausible for recently manufactured units. However, these values may not be representative of the existing residential building stock, where older equipment with lower power factors predominates.

**Code Location**: `defaults/zip_parameters.toml:56-63` (air_conditioner pf=0.96), `defaults/zip_parameters.toml:119-127` (hpwh pf=0.97)

**Root Cause**: Values are sourced directly from published measurements of modern equipment. The OCHRE CSV does not distinguish between new and aged equipment power factors.

**Impact**: Low — the values are defensible per published literature. Using them may underestimate reactive power for simulations representing older building stock. A sensitivity analysis or configurable pf override per dwelling would allow users to model a range of equipment vintages.

---

### Finding 5: [Severity: low] Dishwasher real/reactive ZIP parameters are asymmetric
**Description**: The dishwasher entry (`defaults/zip_parameters.toml:146-153`) models real power as pure constant impedance (zp=1.0, ip=0.0, pp=0.0) but reactive power as pure constant power (zq=0.0, iq=0.0, pq=1.0). For a device containing both a resistive heating element and a pump motor, it is physically unusual for the real and reactive voltage dependencies to follow diametrically opposite ZIP models.

The asymmetry originates from the source CSV (`vendors/OCHRE/ochre/defaults/ZIP Parameters.csv:21`):
```
Dishwasher,TRUE,1,0,0,0,0,1,0.99
```
The Lu et al. 2008 reference from which these values were taken should be consulted to verify this asymmetry is intentional. The heating element (resistive, constant-impedance) dominating real power while the motor (constant-power) dominates reactive power is physically plausible, but the total assignment to one extreme or the other is unusual.

**Code Location**: `defaults/zip_parameters.toml:146-153`

**Root Cause**: Values copied directly from the OCHRE CSV source, which in turn cites Lu et al. 2008.

**Impact**: Low — the real-power modeling is reasonable (resistive heat dominates dishwasher energy), and at nominal voltage the reactive power is correct regardless of ZIP split (since sums are 1.0). The difference only matters at non-nominal voltage, where the reactive power would remain constant rather than varying with V² as a motor-dominated reactive load would.

---

### Finding 6: [Severity: low] No frequency sensitivity parameters in ZIP model
**Description**: The ZIP load model in `defaults/zip_parameters.toml` contains only voltage-dependent terms (Zp, Ip, Pp for real; Zq, Iq, Pq for reactive; pf). There are no frequency sensitivity parameters (typically denoted `Kpf` or `Kqf`, representing df/dP slope). Grid frequency is tracked in the `GridState` struct (`crates/hares-types/src/environment.rs:305-308`) but is never read by any ZIP model or equipment implementation — it is initialized to a hardcoded `60.0` Hz and never used.

This is consistent with the OCHRE reference implementation (`Equipment.py:200-218`) which also lacks frequency dependence. The standard polynomial (ZIP) load model commonly includes optional frequency terms:
```
P(V,f) = P0 * (Z*(V/V0)² + I*(V/V0) + P) * (1 + Kpf*(f - f0))
```

**Code Location**: `defaults/zip_parameters.toml` (no frequency fields), `crates/hares-io/src/defaults.rs:31-46` (ZipParameters struct), `crates/hares-types/src/environment.rs:305-308` (unused frequency_hz field)

**Root Cause**: Design limitation inherited from OCHRE. The reference model does not implement frequency-dependent load behavior.

**Impact**: Low for most applications — voltage variation is the dominant driver of residential load variation in distribution system studies. Frequency dependence becomes important in islanded microgrid and transient stability studies, which are outside HARES's current scope.

---

### Finding 7: [Severity: low] TOML file is valid and parseable; deserialization has no load-time validation
**Description**: The file `defaults/zip_parameters.toml` parses successfully with both Python `tomllib` (3.11+) and the Rust `toml` crate. The `ZipParameters` struct (`crates/hares-io/src/defaults.rs:31-46`) has the correct field names and types for deserialization.

However, the struct has no serde validation attributes — there is no check at TOML-deserialization time that `zp + ip + pp ≈ 1.0` or `zq + iq + pq ≈ 1.0`. Validation occurs downstream:
- Water heater: `validate_zip_terms()` at `water_heater/mod.rs:335` uses tolerance 0.01.
- Scheduled load: `parse_zip_coefficients()` at `scheduled_load.rs:891-896` uses tolerance 1e-9.

If a TOML entry has a data-entry error causing non-unity sums, the error is only detected at equipment initialization time rather than at defaults-load time.

**Code Location**: `crates/hares-io/src/defaults.rs:31-46` (struct, no validation), `defaults/zip_parameters.toml:313-327` (loading)

**Root Cause**: The `ZipParameters` struct directly deserializes without post-load validation. Only equipment-level constructors enforce the sum constraints.

**Impact**: Low — current data is correct (all sums are exactly 1.0). A data-entry error in the TOML would be caught when equipment is instantiated, not when defaults are loaded, which is slightly less convenient for debugging.

---

## Summary
- Total findings: 7
- Critical: 0
- High: 2
- Medium: 2
- Low: 3

## Recommendations
1. **Fix scheduled load reactive power formula** (Finding 1): Change `scheduled_load.rs:505` from `real_kw * self.zip.pf * reactive_base` to `real_kw * self.zip.pf.clamp(-1.0, 1.0).acos().tan() * reactive_base` to match the water heater implementation and OCHRE reference. This is the highest-priority fix — it causes incorrect reactive power for every scheduled-load equipment type.

2. **Document HARES/OCHRE coefficient assignment divergence** (Finding 2): Add a comment in `water_heater/mod.rs:315-317` and `scheduled_load.rs:500-505` noting that HARES intentionally maps real coefficients to real power (unlike OCHRE which swaps them), and that this produces different results at non-nominal voltage. Ideally, consult the original Hajagos & Danai 1998 and Bokhari et al. 2014 papers to confirm which assignment is physically correct.

3. **Add pf validation at defaults-load time** (Finding 7): Add a `#[serde(deserialize_with)]` or a post-load validation step in `load_zip_parameters()` (`defaults.rs:313-327`) that checks `zp + ip + pp` and `zq + iq + pq` sums within a reasonable tolerance (e.g., 1e-6) and emits a clear diagnostic on failure.

4. **Document compressor pf values as reflecting modern equipment** (Finding 4): Add a comment in the TOML file header or in the `ZipParameters` struct documentation noting that the power factor values are from published measurements of modern (post-2014) equipment and may underestimate reactive power for simulations of older building stock.

5. **Consider adding frequency sensitivity as a future enhancement** (Finding 6): For islanded microgrid or transient stability studies, extend the ZIP model schema to include optional `kpf` and `kqf` frequency-sensitivity fields.

## References / Citations
- Hajagos, L.M. and Danai, B. (1998). "Laboratory measurements and models of modern loads and their effect on voltage stability studies." IEEE Trans. Power Systems, 13(2), pp. 584–592.
- Bokhari, A. et al. (2014). "Experimental Determination of the ZIP Coefficients for Modern Residential, Commercial, and Industrial Loads." IEEE Trans. Power Delivery, 29(3), pp. 1372–1381.
- Lu, N. et al. (2008). "Load component database of household appliances and small office equipment." IEEE PES General Meeting, pp. 1–5.
- Arif, A. et al. (2013). "Load Modeling: A Review." IEEE Trans. Smart Grid.
- IEEE Task Force on Load Representation (1993). IEEE Trans. Power Systems, 8(2):472–482.
- OCHRE reference: `vendors/OCHRE/ochre/Equipment/Equipment.py:65-77, 200-218`
- OCHRE defaults: `vendors/OCHRE/ochre/defaults/ZIP Parameters.csv`
- Prior review: `docs/reviews/config-io/config-io-02-defaults-csv-loading.md` Finding 9 (large ZIP coefficient magnitudes)
