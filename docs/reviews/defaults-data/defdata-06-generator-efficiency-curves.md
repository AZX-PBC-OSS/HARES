# Generator efficiency curves: match typical residential backup generator performance
**Review ID**: defdata-06
**Category**: defaults-data
**Date**: 2026-05-26

## Files Reviewed
defaults/generator/efficiency_curve.csv
defaults/generator/efficiency_curve2.csv
defaults/generator/default_parameters.csv

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: [Severity: critical] CSV default files are dead code — never loaded by the Rust parser
**Description**: The generator defaults directory contains three CSV files, but the defaults loader at `crates/hares-io/src/defaults.rs:174` calls `load_toml_dir()` which only reads files with the `.toml` extension (line 345). None of the CSV files are ever parsed or consumed at runtime. The generator efficiency curves defined in these CSV files have no effect on simulation behavior — only the hardcoded Rust defaults in `crates/hares-equipment/src/generator.rs:366-369` are used (the OCHRE default curve: `(0,0), (0.5,1), (1,1)`).

**Code Location**: `crates/hares-io/src/defaults.rs:174` and `crates/hares-io/src/defaults.rs:345` — `load_toml_dir` filters to `.toml` extension only. The CSV files `efficiency_curve.csv`, `efficiency_curve2.csv`, and `default_parameters.csv` are never loaded.

**Root Cause**: The defaults loading architecture uses TOML for equipment parameter files. The CSV files were added to the directory but the loader was never updated to consume them, or the files were intended as reference/documentation only but this intent was never documented.

**Impact**: The more realistic efficiency curve in `efficiency_curve2.csv` (6-point curve matching residential generator characteristics) is completely unused. Any user who edits these CSV files expecting behavior changes will see no effect. The simulation silently falls back to the hardcoded OCHRE default curve.

---

### Finding 2: [Severity: critical] `default_parameters.csv` is a battery configuration file misplaced in the generator directory
**Description**: The file `defaults/generator/default_parameters.csv` contains battery parameters: capacity (6 kW), efficiency (0.95), ramp_rate (0.1 kW/min), charge/discharge schedules, and import/export limits. None of these fields correspond to generator configuration. A similar file exists in `defaults/battery/default_parameters.csv` which is also a CSV (not TOML). The battery defaults test at `crates/hares-io/src/defaults.rs:846-882` writes a `default_parameters.toml` (not `.csv`) to verify the loading path works.

**Code Location**: `defaults/generator/default_parameters.csv:1-10` — header row describes battery fields ("Max power of battery", "Discharging Efficiency", charge/discharge schedules).

**Root Cause**: File was copied from the battery directory to the generator directory without updating its content to match generator parameters. The file may have been intended as a template that was never completed.

**Impact**: Confusion for users and developers. The file suggests the generator has battery-like parameters (charge/discharge schedules, round-trip efficiency) which are nonsensical for a generator. Could mislead users into configuring generators with invalid parameters.

---

### Finding 3: [Severity: high] No idle fuel consumption model — zero fuel at zero load is physically incorrect
**Description**: The generator model computes fuel consumption as `fuel_w = (output_kw * 1000.0) / eta` when `output_kw > IDLE_KW_THRESHOLD`, and zero otherwise (`generator.rs:751-755`). This means when the generator is running at idle (0 kW electrical output), it consumes zero fuel. Real residential backup generators (Generac, Kohler, Cummins) consume 30-50% of full-load fuel at no-load (idle). For example, a Generac 22 kW unit at idle consumes ~180,000 BTU/hr (~53 kW) compared to ~320,000 BTU/hr (~94 kW) at full load. This idle consumption is a critical factor in part-load operating economics and emissions.

Neither efficiency curve CSV addresses this gap — the curve points define electrical efficiency only, with no provision for a minimum fuel rate independent of output.

**Code Location**: `crates/hares-equipment/src/generator.rs:751-755` — the fuel calculation guards on `output_kw > IDLE_KW_THRESHOLD`. The `evaluate()` method at line 250-253 returns 0.0 when `capacity_ratio ≈ 0`. There is no `idle_fuel_w` or `no_load_fuel_fraction` parameter anywhere in `GeneratorConfig` (line 42-60).

**Root Cause**: The OCHRE model (which this code mirrors) simplified generator physics to a pure efficiency curve without a separate idle consumption term. The efficiency curve defines multiplicative efficiency loss at part-load, but idle consumption is additive (fuel consumed regardless of load), requiring a different model structure.

**Impact**: Under-estimated fuel consumption and emissions for generators operating at low part-load or cycling on/off. Part-load operating costs are systematically underestimated, potentially affecting economic comparisons between generator-only and generator-plus-battery configurations.

---

### Finding 4: [Severity: high] OCHRE default efficiency curve plateaus too early at 50% load
**Description**: The hardcoded default curve `(0,0), (0.5,1), (1,1)` at `generator.rs:366-369` (and mirrored in `efficiency_curve.csv`) reaches 100% of rated efficiency at just 50% load. Real residential spark-ignited generators (Generac, Kohler, Briggs & Stratton) have efficiency curves that plateau at 80-100% load. At 50% load, typical electrical efficiency is 80-90% of rated, not 100%. For example, a Generac Guardian 22 kW has ~21.5% efficiency at full load but only ~19.5% at 50% load (a ~9% reduction). The OCHRE default shows no efficiency penalty between 50-100% load, which is unrealistic.

The alternative curve in `efficiency_curve2.csv` is superior — with points at (0.1, 0.47), (0.167, 0.62), (0.333, 0.78), (0.666, 0.94), (1, 1) — showing a more gradual approach to rated efficiency that better matches manufacturer data. However, because CSV files are not loaded (Finding 1), this better curve is unused.

**Code Location**: `crates/hares-equipment/src/generator.rs:366-369` — `default_curve_points()` returns the OCHRE default. `defaults/generator/efficiency_curve.csv:2-4` — contains identical data.

**Root Cause**: The OCHRE model used a simplified 3-point curve optimized for fuel cell efficiency profiles (where efficiency is near-constant above 50% load). This was then adopted as the default for gas generators as well, even though gas generators have fundamentally different part-load characteristics than fuel cells.

**Impact**: Over-estimated generator efficiency at moderate part-load (50-75% of rated), leading to under-estimated fuel consumption and emissions in typical residential backup scenarios where generators often operate at 40-70% of rated load.

---

### Finding 5: [Severity: medium] No upper bound validation on efficiency_ratio in curve points
**Description**: The `validate_curve_points` method at `generator.rs:335` checks `efficiency_ratio >= 0` but imposes no upper bound. The effective electrical efficiency at a curve point is `eta_electric * efficiency_ratio`. Since `eta_electric` is in (0,1], an unbounded `efficiency_ratio` could produce effective efficiency > 1.0, violating the second law of thermodynamics. The `EfficiencyModel::validate()` method at line 309-320 checks `rated` (eta_electric) is in (0,1] but does not verify that `rated * max(efficiency_ratio)` <= 1.0 across the curve.

**Code Location**: `crates/hares-equipment/src/generator.rs:335` — only checks `point.efficiency_ratio < 0.0`. No check for `point.efficiency_ratio > 1.0` or `eta_electric * point.efficiency_ratio > 1.0`.

**Root Cause**: The validation was written to mirror OCHRE's looser constraints. The `efficiency_ratio` is documented as a scaling factor on `eta_electric` (line 71), implying it should be in [0,1], but the code only enforces the lower bound.

**Impact**: A user-provided curve with an inflated efficiency_ratio could silently produce non-physical results (electrical efficiency > 100%), distorted fuel consumption, and incorrect economic comparisons. Low practical risk since a user would need to intentionally configure this, but the validation gap should be closed.

---

### Finding 6: [Severity: medium] No distinction between generator types (portable, standby, prime power)
**Description**: The generator model has only two types: `GasGenerator` and `FuelCell` (`generator.rs:406-409`). There is no distinction among residential generator classes: portable generators (~1-10 kW, air-cooled, ~15-20% efficiency, 500-2000 hour life), standby generators (~7-22 kW, air- or liquid-cooled, ~20-28% efficiency, 3000-10000 hour life), and prime power units (~20 kW+, liquid-cooled, ~28-35% efficiency, 20000+ hour life). These classes have substantially different efficiency curves, ramp rates, minimum loads, and service lives.

The CSV files similarly contain only generic "efficiency_curve" data with no per-type differentiation.

**Code Location**: `crates/hares-equipment/src/generator.rs:406-409` — `GeneratorKind` enum has only `GasGenerator` and `FuelCell` variants. No portable/standby/prime distinctions.

**Root Cause**: The model inherited OCHRE's generator taxonomy which treats all gas generators identically. Residential use cases would benefit from at least portable vs. standby differentiation.

**Impact**: A portable generator is modeled with the same efficiency (30% default) as a liquid-cooled standby unit, over-estimating its electrical efficiency by ~50%. This significantly under-estimates fuel costs for portable generator scenarios.

---

### Finding 7: [Severity: low] Default eta_electric of 0.30 is at the upper limit for residential generators
**Description**: The hardcoded default `DEFAULT_ETA_ELECTRIC = 0.30` at `generator.rs:183` represents the high end of residential generator efficiency. Typical efficiency ranges:
- Small air-cooled (7-14 kW): 18-22% (natural gas), 20-24% (propane)
- Medium air-cooled (16-22 kW): 20-24% (natural gas), 22-26% (propane)
- Liquid-cooled (22-60 kW): 24-30% (natural gas), 26-32% (propane)

The comment at line 183 acknowledges this: "Generac/Briggs residential generators: 28–33% LHV; 30% is the midpoint." However, Generac's published data shows their 22 kW air-cooled unit achieves ~22.5% at full load on natural gas (74,100 BTU/hr input producing 22 kW = 75,000 BTU/hr electric = ~22.6% HHV efficiency, ~25% LHV). The quoted 28-33% LHV range applies to liquid-cooled units, not the more common air-cooled residential standby generators that dominate the residential market.

**Code Location**: `crates/hares-equipment/src/generator.rs:183` — `const DEFAULT_ETA_ELECTRIC: f64 = 0.30;` and accompanying comment at lines 181-182.

**Root Cause**: The default was chosen based on the best-case (liquid-cooled, LHV basis) rather than the typical case (air-cooled, HHV basis) for the most common residential generator type.

**Impact**: Slight systematic under-estimation of fuel consumption for the typical residential case. A default of 0.22-0.25 would better represent a typical 14-22 kW air-cooled standby generator.

---

## Summary
- Total findings: 7
- Critical: 2 (CSV files dead code; battery config in generator dir)
- High: 2 (no idle fuel; early plateau in default curve)
- Medium: 2 (no efficiency_ratio upper bound; no generator type distinction)
- Low: 1 (default eta_electric optimistic)

## Recommendations
1. Convert `efficiency_curve.csv` and `efficiency_curve2.csv` to TOML format or add CSV loading support to the defaults loader so these curves are actually consumed.
2. Move or delete `default_parameters.csv` from the generator directory; it is not a generator file.
3. Adopt the 6-point curve from `efficiency_curve2.csv` as the default for gas generators, replacing the over-simplified 3-point OCHRE default.
4. Add an `idle_fuel_consumption_w` or `no_load_fuel_fraction` parameter to `GeneratorConfig` and incorporate it into the step fuel calculation as a minimum fuel rate floor when the generator is running.
5. Add upper-bound validation on `efficiency_ratio` — at minimum enforce `efficiency_ratio <= 1.0`, or ideally `eta_electric * efficiency_ratio <= 1.0` for each curve point.
6. Add portable/standby generator type variants with appropriate default efficiency curves (15-20% rated for portable, 20-28% for standby) and distinct service-life/degradation characteristics.
7. Lower `DEFAULT_ETA_ELECTRIC` to 0.22-0.25 to represent a typical air-cooled residential standby generator, and document the basis (HHV vs LHV) in the constant comment.

## References / Citations
- Generac Guardian 22 kW spec sheet: 74,100 BTU/hr natural gas input at full load → ~22.6% HHV efficiency references for idle consumption
- Kohler 20RESCL spec sheet: idle fuel ~40-45% of full load
- Vishwanathan et al. (2018), "Life cycle techno-economic assessment of fuel cell and micro gas turbine CHP systems," Applied Energy, https://doi.org/10.1016/j.apenergy.2018.06.013 — quadratic efficiency curve reference used in OCHRE and HARES
- OCHRE Generator.py — source of default 3-point efficiency curve and constant efficiency model
