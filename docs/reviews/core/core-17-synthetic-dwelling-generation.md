# Synthetic dwelling generation statistical distributions and correlations
**Review ID**: core-17
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/dwelling/synthetic.rs crates/hares-io/src/resstock.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/utils/hpxml.py

## Findings
### Finding 1: [Severity: high]
**Description**: No statistical distributions exist for synthetic building characteristics. The synthetic dwelling path (`build_synthetic_building`) constructs buildings entirely from explicit values in a manually-authored TOML file. House size (`floor_area_m2`), zone volume, wall area, insulation R-value, HVAC capacity, window count and properties, and every other building characteristic is a scalar hardcoded in the TOML — not drawn from a vintage-parameterized or location-parameterized distribution. There is no mechanism to specify a statistical distribution (e.g., "lognormal with μ = 5.0, σ = 0.4 for floor area in climate zone 5A, 1980s vintage").

**Code Location**: `synthetic.rs:327-793` (`build_synthetic_building` — entire function reads scalar fields from `SyntheticTomlConfig`); `synthetic.rs:58-66` (`SyntheticGeometryConfig` — `floor_area_m2` is a bare `f64` with no distribution wrapper); `synthetic.rs:17-47` (`SyntheticTomlConfig` — all fields are deterministic scalars or vectors, not distribution descriptors)

**Root Cause**: The synthetic builder was designed for BESTEST-style benchmark cases — a handful of explicitly specified reference buildings with known, exact geometry. It was never extended with a statistical generation layer for stock analysis. The config schema has no fields for distribution parameters (e.g., `floor_area_m2 ~ LogNormal(mean, stdev)` or `floor_area_m2_by_vintage = { "pre-1980": LogNormal(...), "1980-2000": LogNormal(...) }`).

**Impact**: Users wanting to generate a representative building stock must either (a) manually write one TOML file per building (impractical beyond ~10 buildings), or (b) use ResStock pre-generated HPXML buildings. The synthetic path cannot serve as a lightweight alternative to ResStock for rapid stock generation. It also cannot be used for sensitivity analysis where building characteristics should be systematically varied.

### Finding 2: [Severity: high]
**Description**: No correlation enforcement between building characteristics. Every characteristic in the synthetic TOML is independently configured with zero cross-validation. Larger houses do NOT automatically get more bedrooms; the synthetic config has no `bedrooms` field at all. Newer houses do NOT automatically get better insulation; there is no vintage parameter to condition insulation on. Climate zones do NOT determine foundation types or glazing ratios; the synthetic config has no climate zone concept and no foundation type parameter. A user can freely specify `floor_area_m2 = 400`, `wall_r_value_m2_k_w = 0.5` (essentially uninsulated), and single-pane windows (`u_factor_w_m2_k = 5.7`) — producing a 400 m² mansion with R-0.5 walls and single-pane glazing that would be physically absurd and economically impossible.

**Code Location**: `synthetic.rs:58-71` (`SyntheticGeometryConfig` and `SyntheticMaterialsConfig` — independent config structs with no inter-field validation); `synthetic.rs:232-241` (`SyntheticWindowConfig` — U-factor and SHGC independently specified with no WWR or climate suitability checks); `synthetic.rs:327-793` (no correlation logic in `build_synthetic_building` — fields are read and wired independently)

**Root Cause**: The synthetic path has no concept of "characteristic dependence." NREL ResStock uses conditional probability tables (CPTs) that encode dependencies like `P(Insulation | Vintage, Location)` and `P(FoundationType | Location, Vintage)`. HARES has no equivalent mechanism. The `Building` struct does carry fields like `foundation_name` and `floors_above_grade` but these are always `None` for synthetic buildings (lines 786-787), meaning no synthetic dwelling has a foundation type or floor count.

**Impact**: Unrealistic dwellings can be silently constructed. A synthetically-generated stock (if one could be built from multiple TOMLs) would contain physically implausible combinations that bias aggregate results — e.g., overestimating heating loads by pairing large houses with poor insulation, or underestimating cooling loads by omitting windows on south-facing walls. Critically: the synthetic path offers NO guard against independent draws producing contradictory characteristics.

### Finding 3: [Severity: medium]
**Description**: No dwelling count specification for synthetic stock generation. The synthetic path generates exactly one dwelling per TOML file via `Dweling::from_toml_config()`. There is no "generate N dwellings" parameter — no mechanism to say "produce 100 agents from this statistical profile." Multiple synthetic dwellings require manually listing multiple TOML configs and constructing a `Fleet::from_buildings()` with one entry per TOML. The fleet builder at `fleet.rs:104-116` accepts a `Vec<DwellingConfig>` but has no synthesis or templating capability to expand a config template into N variants.

**Code Location**: `synthetic.rs:327` (builds one `Building` per call); `mod.rs:842-894` (`from_toml_config` — single-dwelling constructor, no loop or count parameter); `fleet.rs:104-116` (`Fleet::from_buildings` — accepts explicit config list, no template expansion)

**Root Cause**: The synthetic path was conceived as a single-dwelling path for detailed analysis (BESTEST benchmarks). The stock-analysis path exclusively routes through ResStock pre-generated buildings (`Fleet::from_resstock` at `fleet.rs:118-161`). There is no bridge between the two: no synthetic-stock generator that takes a statistical profile and produces N dwellings.

**Impact**: Stock-level analysis with synthetic dwellings is impossible without external scripting to generate hundreds of TOML files. Running sensitivity analysis over, say, a parameter grid of floor_area × insulation_level × window_count requires an O(n³) explosion of hand-written configs. This forces users into the ResStock path (with its heavy data dependencies — HPXML zips, weather CSVs, metadata parquets) for any multi-dwelling analysis.

### Finding 4: [Severity: medium]
**Description**: `master_seed` does not control dwelling generation — only simulation randomness. The seed is deserialized into `SyntheticOutputConfig.master_seed` (line 162) and flows to `SimulationConfig.master_seed` (line 870 of mod.rs), where it seeds the per-dwelling `ChaCha8Rng` via `derive_dwelling_rng` (rng.rs:12-17). This RNG is used exclusively for stochastic schedule event resampling, EV driver behavior, and DR compliance — NOT for building geometry. Building geometry from `build_synthetic_building` is fully deterministic from TOML values. The promise of "identical input seeds produce identical dwelling sets" is vacuously true because the seed does not participate in dwelling generation at all.

**Code Location**: `synthetic.rs:162` (`master_seed` field in `SyntheticOutputConfig`); `mod.rs:870` (seed copied to `SimulationConfig`); `mod.rs:931` (seed used for RNG, not geometry); `rng.rs:12-17` (RNG derivation — `(master_seed, bldg_id)` → `ChaCha8Rng`)

**Root Cause**: The seed is in the output config section alongside `output_chunk_size` and `output_format`, but its consumer is the simulation engine's environment initialization. It was never wired into a synthetic dwelling generator because no such generator exists.

**Impact**: Two TOML configs with identical geometry but different `master_seed` values will produce identical building geometry (as expected) but different schedule/EV/DR behavior. This is correct behavior — building geometry should be deterministic — but the configuration schema is misleading. A user who expects `master_seed` to control which buildings are generated from a statistical distribution will find no such behavior. If a synthetic stock generator were added, `master_seed` would need to be wired into its RNG to ensure reproducible dwelling sets.

### Finding 5: [Severity: medium]
**Description**: No CPT or conditional distribution machinery exists anywhere in the codebase. The review instructions ask whether the synthetic generator "uses ResStock-style sampling from conditional probability tables (CPTs) or simpler independent distributions." The answer is neither — the synthetic path uses no distributions at all. The ResStock path (`crates/hares-io/src/resstock.rs`) parses pre-generated building metadata from NREL's CPT-derived sampling but does not implement CPTs itself. The Python layer (`python/ochre_next/data/resstock.py:482`) uses `polars.DataFrame.sample()` with `shuffle=True` — uniform random sampling — not weighted CPT sampling. Sample weights are read from the parquet but used only for downstream aggregation attribution, not for weighted building selection.

**Code Location**: `resstock.py:473-489` (uniform sampling with `df.sample(shuffle=True)`, weights collected but not used for selection); `crates/hares-io/src/resstock.rs:1-662` (metadata parsing — reads building IDs and `in.*` characteristics, no CPT logic); `fleet.rs:118-161` (`Fleet::from_resstock` — processes ALL buildings in metadata, no sampling or weighting at the fleet level)

**Root Cause**: NREL's ResStock sampling toolchain performs CPT-based weighted sampling during HPXML generation (using Python `buildstockbatch`). The output is a set of weighted HPXML files. HARES consumes these pre-generated HPXML files as-is — it does not replicate the CPT sampling logic. The Python fleet fetch layer adds a convenience `n_buildings` uniform subsample on top of the already-weighted ResStock output, further degrading representativeness (also noted in finding 1 of config-io-05).

**Impact**: For ResStock-based fleets: a uniform subsample of N buildings from the metadata table is NOT representative of the housing stock unless N approaches the full dataset size. The fleet's sample weights remain accurate for what IS sampled, but the sample itself is biased. For synthetic fleets: no distribution-based generation exists, so this concern is moot but represents a feature gap relative to NREL ResStock's statistical methodology.

### Finding 6: [Severity: low]
**Description**: OCHRE validates cross-parameter consistency; HARES synthetic path does not. The OCHRE vendor reference (`vendors/OCHRE/ochre/utils/hpxml.py`) demonstrates substantial cross-parameter validation during HPXML parsing: ceiling height is verified against conditioned volume/floor area (`hpxml.py:258-260` — `assert abs(... - ceiling_height) < 0.1`), number of conditioned floors determines foundation zone handling (`hpxml.py:268-270`), garage geometry is validated with wall height consistency checks (`hpxml.py:442-477`), and foundation type is inferred from envelope structure (`hpxml.py:271-290`). By contrast, HARES's synthetic builder performs none of these checks. Ceiling height is `None` for all synthetic buildings (`synthetic.rs:783`), no floor count is set (`synthetic.rs:785`), and there is no foundation zone concept in the synthetic config.

**Code Location**: `synthetic.rs:783-787` (all structural fields set to `None`); `hpxml.py:258-260` (ceiling height assertion); `hpxml.py:268-290` (foundation type derivation)

**Root Cause**: The synthetic TOML schema was designed for minimal thermal modeling (floor area + zone volume + wall R-value + HVAC) rather than comprehensive building representation. Fields like `floors_above_grade`, `foundation_name`, `ceiling_height_m` exist in the `Building` struct but are never populated by the synthetic builder. The builder was intentionally simple for BESTEST where these details are irrelevant.

**Impact**: Synthetic dwellings are structurally underspecified compared to HPXML-derived buildings. Any feature that depends on floor count, foundation type, or ceiling height (e.g., multi-zone modeling, foundation heat transfer, stack-effect infiltration) will silently produce incorrect or default-valued results for synthetic buildings. This limits the synthetic path to single-zone, slab-on-grade approximations regardless of what the user intends.

### Finding 7: [Severity: low]
**Description**: No vintage concept exists in the synthetic configuration. Neither `SyntheticTomlConfig` nor its sub-configs contain any field for building age, construction era, or vintage. The word "vintage" does not appear in `synthetic.rs`. This means insulation levels, HVAC equipment types, air leakage rates, and window properties cannot be conditioned on construction era — a fundamental parameter in ResStock's CPT sampling that strongly correlates with building performance.

**Code Location**: `synthetic.rs:14-47` (`SyntheticTomlConfig` and all nested config structs — no vintage or year_built field)

**Root Cause**: BESTEST benchmarks use reference buildings with known material properties, making vintage irrelevant for that use case. The synthetic config was never generalized beyond BESTEST-style specification.

**Impact**: Even if a synthetic stock generator were added, it could not produce vintage-dependent characteristics without schema extensions. This is a structural gap in the config schema, not just a missing code path.

## Summary
- Total findings: 7
- Critical: 0
- High: 2 (no statistical distributions for synthetic generation; no correlation enforcement between characteristics)
- Medium: 3 (no dwelling count specification for stock generation; master_seed does not control building generation; no CPT/distribution machinery)
- Low: 2 (no cross-parameter validation vs OCHRE; no vintage parameter in config schema)

## Recommendations
1. **Add a statistical generation layer on top of the synthetic TOML schema.** Define distribution descriptors (e.g., `floor_area_m2_d = { dist = "LogNormal", mu = 5.2, sigma = 0.3 }`) or vintage/location lookup tables that parameterize each field. Wire `master_seed` into this generator to ensure deterministic, reproducible dwelling sets. This would enable rapid stock generation without ResStock's heavy data dependencies.

2. **Implement cross-characteristic correlation rules.** After drawing primary characteristics (floor area, vintage, location), derive secondary characteristics from regressions or CPTs: bedrooms from floor area, insulation from vintage × location, foundation type from location, glazing ratio from orientation × vintage. Validate combinations for physical plausibility (e.g., reject WWR > 0.40, reject R-2 walls in zone 7).

3. **Add a dwelling count parameter to synthetic fleet construction.** A function like `generate_synthetic_stock(profile, n, master_seed)` that draws N dwellings from the statistical profile, with weights proportional to population distributions if desired. Provide both weighted (stock analysis) and unweighted (sensitivity grids) modes.

4. **Populate structural Building fields from synthetic geometry.** Compute `ceiling_height_m` from `zone_volume_m3 / floor_area_m2` (the data is already available at `synthetic.rs:334-336`). Allow optional `foundation_name` and `floors_above_grade` in the TOML schema. Wire these through to the `Building` struct so synthetic dwellings are as complete as HPXML-derived ones.

5. **Align ResStock fleet sampling with stock weights.** Modify `python/ochre_next/data/resstock.py:482` to pass the `weights` argument to `polars.DataFrame.sample()` using the discovered weight column. This partially mitigates the lack of CPT-based generation in the Python fetch layer.

## References / Citations
- NREL ResStock Methodology: Wilson, E. et al. (2021). "End-Use Load Profiles for the U.S. Building Stock." NREL/TP-5500-79367. https://www.nrel.gov/docs/fy21osti/79367.pdf — describes CPT-based characteristic sampling
- NREL ResStock CPT implementation: https://github.com/NREL/resstock/blob/develop/resources/data/options_lookup.tsv — conditional probability tables mapping location, vintage, and housing type to building characteristics
- Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Trans. 71(1):61-74 — ground temperature model referenced in synthetic weather
- Berdahl, P. and Martin, M. (1984), Solar Energy 33(3/4):321-336 — sky emissivity model used in synthetic weather
- ANSI/RESNET/ICC 301-2019 — standard residential energy rating methodology with bedroom-based hot water draw profiles
