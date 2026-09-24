# Default parameter CSV loading: completeness, fallbacks, unit safety
**Review ID**: config-io-02
**Category**: config-io
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/defaults.rs` (1013 lines) — primary defaults loader
- `crates/hares-io/src/envelope_lut.rs` (447 lines) — envelope LUT CSV parser
- `defaults/zip_parameters.toml` — ZIP coefficients (279 lines)
- `defaults/hvac_cooling/*.csv` — 4 cooling biquadratic CSV files
- `defaults/hvac_heating/*.csv` — 3 heating biquadratic CSV files
- `defaults/HVAC Multispeed Parameters.csv` — multispeed lookup table
- `defaults/envelope/Envelope Materials.csv` — material thermal properties
- `defaults/envelope/Envelope Boundary Types.csv` — assembly R-values
- `defaults/envelope/Envelope Boundaries.csv` — zone label mappings
- `defaults/battery/`, `defaults/ev/`, `defaults/generator/`, `defaults/loads/`, `defaults/pv/`, `defaults/water_heating/` — equipment default CSVs

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/defaults/ZIP Parameters.csv` — OCHRE ZIP source format
- `vendors/OCHRE/ochre/defaults/HVAC Cooling/Biquadratic Air Conditioner.csv` — OCHRE biquadratic reference
- `vendors/OCHRE/ochre/defaults/HVAC Heating/`, `Battery/`, `Envelope/`, `EV/`, `Gas Generator/`, `Water Heating/` — OCHRE equipment defaults

## Findings

### Finding 1: [Severity: critical] CSV defaults silently bypassed — `load_toml_dir` only reads `.toml`, but subdirectories contain only `.csv`
**Description**: The `load_toml_dir()` function (`defaults.rs:345`) filters exclusively on the `toml` file extension:
```rust
if path.extension().is_some_and(|ext| ext == "toml") {
```
However, *every* equipment defaults subdirectory contains only **CSV** files, not TOML files:
- `defaults/battery/default_parameters.csv`, `degradation_curves.csv`
- `defaults/ev/` — 13 CSV files (EV Profiles, BEV/PHEV level CSVs, vehicle PDFs)
- `defaults/generator/default_parameters.csv`, `efficiency_curve.csv`, `efficiency_curve2.csv`
- `defaults/loads/cooking_range_events.csv`, `cooking_range_induction_events.csv`, `clothes_dryer_events.csv`
- `defaults/water_heating/default_paramters.csv`, `WH Medium UEF Schedule.csv`
- `defaults/pv/` — contains only `.gitkeep`

Because `load_toml_dir` returns `Ok(map)` with an empty HashMap when no `.toml` files match, these CSV files are **silently skipped** with no warning. The consequence is that calls to `equipment_defaults()` for Battery, EV, Generator, Loads, and WaterHeating categories will always return `None`, and those default parameters are completely unavailable at runtime.

By contrast, the OCHRE vendor reference loads the same data directly from CSV in Python, so the data format is substantively correct — it is the HARES loader that has the format mismatch.

**Code Location**: `crates/hares-io/src/defaults.rs:330-361` (`load_toml_dir`), line 345 (extension filter), lines 171-177 (invocations for battery/envelope/ev/generator/loads/pv/water_heating)

**Root Cause**: The defaults module was refactored to expect TOML config files (with per-entry key-value maps), but the actual data files were migrated from OCHRE's CSV format as-is without converting to TOML or adding a CSV loader path. The `load_hvac_curves_dir` function (`defaults.rs:363-398`) has both TOML and CSV branches, but `load_toml_dir` was never given a CSV branch.

**Impact**: Equipment default parameters for battery, EV, generator, loads, and water heating are not loaded. Simulations that depend on these defaults will either fail at runtime or run with zeroed/uninitialised parameters. The envelope directory works because it has a separate CSV parser (`EnvelopeLookup::load` in `envelope_lut.rs:108-126`) which directly loads the three envelope CSVs.

---

### Finding 2: [Severity: high] HVAC heating temperature bounds use Fahrenheit sentinel values but code documents Celsius
**Description**: The HVAC heating CSV files contain temperature bounds at ±100:
- `defaults/hvac_heating/ASHP Heater.csv:23-26`: `min_Twb,-100`, `max_Twb,100`, `min_Tdb,-100`, `max_Tdb,100`
- `defaults/hvac_heating/Heat Pump Heater.csv:23-26`: identical ±100 bounds

These values of -100 and +100 represent OCHRE's sentinel for "unbounded" temperature range in **Fahrenheit** (−73°C to +38°C). However, the TOML deserialization struct documents the bounds as Celsius:
- `RawHvacVariant` struct at `defaults.rs:414-417`: `/// [min, max] wet-bulb temperature bounds (deg C)` and `/// [min, max] dry-bulb temperature bounds (deg C)`

The cooling CSV files use physically correct Celsius values (e.g., `min_Twb,13.88`, `max_Twb,23.88` for Air Conditioner), which confirms the codebase uses Celsius. The fallback defaults in `load_hvac_csv_file` (`defaults.rs:532-534`) also use Celsius ranges (Twb `[-10.0, 50.0]`, Tdb `[-50.0, 60.0]`).

The -100°C Twb bound has no physical meaning as a design operating condition for any heat pump. It would allow the biquadratic curve to extrapolate far beyond its valid domain without clamping. If the curves are ever evaluated near these bounds, they will produce physically nonsensical results.

For comparison, the OCHRE source files (`vendors/OCHRE/ochre/defaults/HVAC Heating/Biquadratic ASHP Heater.csv`) have the same ±100 values, confirming this is an upstream data convention issue where OCHRE uses ±100°F as "effectively no clamp."

**Code Location**: `defaults/hvac_heating/ASHP Heater.csv:23-26`, `defaults/hvac_heating/Heat Pump Heater.csv:23-26`; fallback defaults at `crates/hares-io/src/defaults.rs:532-534`

**Root Cause**: OCHRE uses ±100°F as sentinel values for unbounded temperature operation and stores them as-is in the CSV. HARES imports these values without unit conversion or clamp-override detection.

**Impact**: Biquadratic performance curves for heating equipment can be evaluated at temperatures far outside their valid design range (−100°C to +100°C), producing physically implausible capacity and EIR predictions. The `warn_on_clamp: true` flag (`defaults.rs:443, 554`) would only trigger if the clamping logic is reached, and it would fire spuriously for completely reasonable operating points (−20°C) because the code thinks the valid range extends to −100°C.

---

### Finding 3: [Severity: high] Silent zero-substitution when CSV rows or values are missing or unparseable
**Description**: In `load_hvac_csv_file()` (`defaults.rs:493`), values that are missing or fail to parse silently become 0.0:
```rust
let values: Vec<f64> = (1..=n_variants)
    .map(|i| record.get(i).unwrap_or("0").parse::<f64>().unwrap_or(0.0))
    .collect();
```
Additionally, at lines 498-506, entirely missing row names silently produce vectors of zeros:
```rust
let get_row = |name: &str| -> Vec<f64> {
    data.get(name)
        .cloned()
        .unwrap_or_else(|| vec![0.0; n_variants])
};
let get_row_with_default = |name: &str, default: f64| -> Vec<f64> {
    data.get(name)
        .cloned()
        .unwrap_or_else(|| vec![default; n_variants])
};
```

This means a single-character typo in a row name (e.g., `a_eir_t` misspelled as `a_eirr_t`) silently produces all-zero coefficients for that parameter across all speed variants. No warning is emitted. A biquadratic curve with all-zero coefficients for EIR would nominally produce an EIR of 0.0 — meaning the equipment consumes zero electricity, a physically impossible result that silently corrupts simulation output.

The `load_hvac_multispeed_csv()` function has similar issues (`defaults.rs:602-603`): missing "HVAC Name" column silently defaults to empty string, and rows with empty names are skipped without warning (line 604).

For comparison with the envelope LUT, `load_materials` and `load_boundary_types` (`envelope_lut.rs:250-278`) use serde deserialization which would produce a clear error message with the failing row context if a required column is missing or has incompatible type — a much better pattern.

**Code Location**: `crates/hares-io/src/defaults.rs:492-496` (value fallback), `498-501` (row fallback), `583-585` (missing file fallback)

**Root Cause**: The code uses `unwrap_or`/`unwrap_or_else` at multiple levels of the CSV parsing pipeline, converting parse errors and missing data into silent defaults rather than propagating them as errors.

**Impact**: Corrupt or misformatted CSV data can silently produce physically impossible simulation results. This is particularly dangerous in energy modelling where a zero EIR coefficient is numerically valid but physically meaningless.

---

### Finding 4: [Severity: medium] Missing HVAC multispeed CSV file silently returns empty Vec with no warning
**Description**: At `defaults.rs:583-585`:
```rust
if !path.exists() {
    return Ok(Vec::new());
}
```
When `defaults/HVAC Multispeed Parameters.csv` is missing, the function returns an empty vector without any logging. This is inconsistent with the envelope LUT loader, which at `envelope_lut.rs:112-117` returns a `MissingFile` error, and the HVAC CSV parser path at `defaults.rs:386-392`, which logs a `tracing::warn!` on parse failure.

Additionally, the ZIP parameters file is treated as mandatory at `defaults.rs:152-154` (returns `MissingFile` error), effectively making `zip_parameters.toml` a hard requirement while `HVAC Multispeed Parameters.csv` is silently optional.

**Code Location**: `crates/hares-io/src/defaults.rs:583-585`

**Root Cause**: Inconsistent error handling strategy — some data sources are treated as mandatory, others as silently optional, with no documented rationale for which is which.

**Impact**: If the multispeed CSV is accidentally deleted or the filename changes, HVAC multispeed lookups will return `None` without any indication that data is missing. The simulation will proceed with equipment that cannot find match parameters, potentially defaulting to single-speed behavior without the user knowing why.

---

### Finding 5: [Severity: medium] `Envelope Boundaries.csv` exists but is never loaded by any code path
**Description**: The file `defaults/envelope/Envelope Boundaries.csv` (33 rows mapping boundary names to zone labels) exists in the repository but is never referenced by any Rust code. The `EnvelopeLookup::load()` function in `envelope_lut.rs:108-126` only loads `Envelope Boundary Types.csv` and `Envelope Materials.csv`.

All zone-to-boundary relationships are instead hardcoded in the `resolve_boundary_name()` function (`envelope_lut.rs:288-369`). This means the CSV file is dead data — it could contain errors or become stale without any test catching it.

The OCHRE vendor reference loads this file in Python (`ochre/defaults/Envelope/Envelope Boundaries.csv`), so the data was migrated but the loader was replaced with a hardcoded mapping.

**Code Location**: `crates/hares-io/src/envelope_lut.rs:108-126` (loads 2 of 3 CSVs), `defaults/envelope/Envelope Boundaries.csv` (unused file)

**Root Cause**: The boundary resolution was moved to a hardcoded Rust function for performance/reliability, but the source CSV was retained in the repository without an automated test asserting parity.

**Impact**: Low immediate impact, but risk of drift between the hardcoded mapping and the CSV over time. Also wastes ~33 lines of dead file.

---

### Finding 6: [Severity: medium] Error variant name `MalformedToml` used for CSV parse errors
**Description**: The `load_hvac_csv_file()` and `load_hvac_multispeed_csv()` functions at `defaults.rs:474-490` and `defaults.rs:594-596` map all CSV parse errors to the `DefaultsError::MalformedToml` variant:
```rust
Err(e) => Err(DefaultsError::MalformedToml {
    path: path.to_path_buf(),
    reason: e.to_string(),
})
```
and:
```rust
.map_err(|e| DefaultsError::MalformedToml {
    path: path.to_path_buf(),
    reason: e.to_string(),
})?;
```

This is misleading — the error message will say "malformed TOML in path/to/file.csv" when the actual problem is a CSV format error. A caller trying to diagnose whether a TOML syntax fix is needed would be misled.

**Code Location**: `crates/hares-io/src/defaults.rs:112-123` (error enum), `474-490` (CSV error mapping), `594-596` (multispeed CSV error mapping)

**Root Cause**: The error enum was designed for a TOML-only loading pipeline and was not updated when CSV support was added.

**Impact**: Confusing error messages for operators; harder to debug CSV formatting issues.

---

### Finding 7: [Severity: low] `Room AC.csv` and `MSHP Cooler.csv` lack temperature bound rows
**Description**: 
- `defaults/hvac_cooling/Room AC.csv` has no `min_Twb`, `max_Twb`, `min_Tdb`, or `max_Tdb` rows at all. All temperature bounds use the fallback values: Twb `[-10.0, 50.0]`, Tdb `[-50.0, 60.0]`.
- `defaults/hvac_cooling/MSHP Cooler.csv` similarly has no temperature bound rows, only `min_plf` and `max_plf`.

The fallback Tdb bounds of `[-50, 60]`°C are extremely wide. While physically possible for the dry-bulb range, relying on such wide bounds means the biquadratic curves may extrapolate into regions where the underlying fit data becomes unreliable.

The OCHRE reference biquadratic CSV files (`vendors/OCHRE/ochre/defaults/HVAC Cooling/Biquadratic Room AC.csv`) also lack temperature bounds — this is an upstream data gap.

**Code Location**: `defaults/hvac_cooling/Room AC.csv`, `defaults/hvac_cooling/MSHP Cooler.csv`, fallback at `crates/hares-io/src/defaults.rs:532-534`

**Root Cause**: OCHRE's Room AC and MSHP Cooler biquadratic CSVs were originally generated without explicit temperature bound rows.

**Impact**: These equipment types may produce unreliable performance predictions at edge-case outdoor temperatures. The `warn_on_clamp` flag would only fire if values actually exceed the bounds.

---

### Finding 8: [Severity: low] Filename typo: `default_paramters.csv` (missing 'e' in "parameters")
**Description**: The file `defaults/water_heating/default_paramters.csv` has a typo in its name ("paramters" instead of "parameters"). The OCHRE source file has the same typo (`vendors/OCHRE/ochre/defaults/Water Heating/default_paramters.csv`). Since `load_toml_dir` ignores CSV files anyway (Finding 1), this file is not currently loaded, but if a CSV loader is added, this typo could cause bugs in code that hardcodes the filename.

**Code Location**: `defaults/water_heating/default_paramters.csv` (filename)

**Root Cause**: Inherited typo from OCHRE upstream.

**Impact**: Minimal currently (file not loaded), but a trap for future CSV-based loaders.

---

### Finding 9: [Severity: low] ZIP parameters not validated for physical constraints
**Description**: The `ZipParameters` struct (`defaults.rs:30-46`) is deserialized from TOML without any validation that the coefficients satisfy the ZIP model constraint `zp + ip + pp ≈ 1.0` (and similarly for reactive). While it is acceptable for these to sum to values other than 1.0 in some models, several entries have extreme values:
- `refrigerator`: `zp = 5.03, ip = -8.48, pp = 4.45` — sum = 1.0 but individual magnitudes far exceed 1.0
- `ashp_heater`: `zq = 14.78, iq = -23.71, pq = 9.93` — large reactive components

The OCHRE source CSV (`vendors/OCHRE/ochre/defaults/ZIP Parameters.csv`, lines 22-23) has the same values for Refrigerator and cites published literature, confirming these are intentional but extreme. No validation or documentation explains why these coefficients are valid despite their large magnitudes.

**Code Location**: `crates/hares-io/src/defaults.rs:30-46` (struct definition), `defaults/zip_parameters.toml:155-162` (refrigerator values)

**Root Cause**: The ZIP model allows negative coefficients when the combination of ZIP terms yields the correct apparent power characteristic; large cancellation between terms is physically meaningful but unexpected at first glance.

**Impact**: Low — the values are correct per published literature. But the lack of validation means that data entry errors (e.g., a sign flip) would not be caught.

---

### Finding 10: [Severity: low] `Envelope Boundaries.csv` column not used; envelope material CSV column mismatch with deserialization struct
**Description**: 
The `MaterialRow` struct (`envelope_lut.rs:66-76`) declares 5 fields:
```rust
boundary_name, boundary_type, resistance, capacitance
```
The CSV `Envelope Materials.csv` has 14 columns but serde CSV gracefully ignores unmapped columns, so this is safe. However, the CSV includes raw material properties (`Thickness (m)`, `Conductivity (W/m-K)`, `Density (kg/m^3)`, `Specific Heat (kJ/kg-K)`) that could be useful for non-LUT simulation paths but are discarded during parsing. Additionally, the CSV has **two** specific heat columns with different units: `Specific Heat (kJ/kg-K)` and `Specific Heat (J/kg-K)`, which could cause confusion if someone later adds these fields to the struct.

**Code Location**: `crates/hares-io/src/envelope_lut.rs:66-76` (`MaterialRow` struct), `defaults/envelope/Envelope Materials.csv:1` (dual specific heat columns)

**Impact**: Low — current behavior is correct. Risk is future misuse of the dual-unit specific heat columns.

---

## Summary
- Total findings: 10
- Critical: 1 (CSV defaults silently skipped by TOML-only loader)
- High: 2 (temperature unit mismatch in heating bounds, silent zero substitution)
- Medium: 3 (missing multispeed CSV warning, dead Envelope Boundaries CSV, misleading error variant)
- Low: 4 (missing temp bounds in Room AC/MSHP CSVs, filename typo, unvalidated ZIP coefficients, dual-unit specific heat columns)

## Recommendations
1. **Immediate**: Add CSV loading support to `load_toml_dir()` (or create a parallel `load_csv_dir()`) for battery, EV, generator, loads, and water_heating equipment defaults. Each CSV follows the OCHRE `Description,Name,Value,Units` format and can be parsed into `EquipmentDefaults`. Until this is fixed, these equipment categories have no working defaults.
2. **Important**: Add a bounds-sanitisation step that detects ±100°F sentinel values in HVAC temperature bounds and either converts them to ±100°C (adjusting for unit) or replaces them with the fallback defaults and logs a warning.
3. **Important**: Replace silent zero fallbacks in `load_hvac_csv_file()` with `tracing::warn!` calls for each missing or unparseable value. Consider failing the load with an error if critical rows (e.g., `a_eir_t`, `a_cap_t`) are entirely absent.
4. Add a `tracing::warn!` when `load_hvac_multispeed_csv()` encounters a missing file or empty result.
5. Rename `DefaultsError::MalformedToml` to a more general variant (e.g., `ParseError`) or add a separate `MalformedCsv` variant.
6. Add an automated test that asserts every CSV file referenced by the defaults module actually exists and has the expected column headers before any simulation begins — a "defaults integrity check" that runs at startup.
7. Either remove the unused `Envelope Boundaries.csv` or add a test asserting its contents match the hardcoded `resolve_boundary_name()` mappings.
8. Fix the `default_paramters.csv` filename typo for future CSV-based loaders.

## References / Citations
- OCHRE defaults source: `vendors/OCHRE/ochre/defaults/ZIP Parameters.csv` — original ZIP CSV (migrated to TOML in HARES)
- OCHRE biquadratic source: `vendors/OCHRE/ochre/defaults/HVAC Cooling/Biquadratic Air Conditioner.csv` — column-identical with HARES copy
- OCHRE envelope source: `vendors/OCHRE/ochre/defaults/Envelope/Envelope Boundaries.csv` — loaded in OCHRE Python, replaced by hardcoded Rust mapping in HARES
- Hajagos & Danai (1998), Bokhari et al. (2014) — cited ZIP coefficient literature in OCHRE source
- ANSI/RESNET/ICC 301-2022 Addendum C — schedule fraction data source
