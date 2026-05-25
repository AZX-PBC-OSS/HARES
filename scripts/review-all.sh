#!/usr/bin/env bash
# -*- mode: shell-script; -*-
#
# HARES Systematic Code Review Dispatcher
# ========================================
# Idempotent — re-running skips reviews whose output .md already exists.
# Each review is a single narrow concern with deterministic output path.
#
# Usage:
#   ./scripts/review-all.sh              # run all pending reviews
#   ./scripts/review-all.sh --dry-run    # list what would run
#   ./scripts/review-all.sh hpxml-01     # run a single review by ID
#   ./scripts/review-all.sh --category hpxml  # run all in one category
#
# Output:
#   docs/reviews/{category}/{id}-{slug}.md
#
# Invoke opencode with:
#   opencode run --prompt-file <tmpfile>
# or similar.  Adjust OPENCODE_CMD below for your environment.
# ---------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REVIEWS_DIR="$REPO_ROOT/docs/reviews"

# ---------- opencode invocation ----------
# Change to match your opencode CLI.
# Options:
#   "prompt"       -> echo "$prompt" | opencode
#   "prompt-file"  -> write prompt to tempfile, opencode reads it
OPENCODE_MODE="${OPENCODE_MODE:-prompt}"
OPENCODE_BIN="${OPENCODE_BIN:-opencode}"
# Extra flags, e.g. "--model deepseek-v4-pro" or "--verbose"
OPENCODE_FLAGS="${OPENCODE_FLAGS:-}"

run_opencode() {
    local prompt="$1"
    local output_path="$2"

    mkdir -p "$(dirname "$output_path")"

    echo "---"
    echo "Starting review → $output_path"
    echo "---"

    case "$OPENCODE_MODE" in
        prompt-file)
            local tmpfile
            tmpfile="$(mktemp)"
            printf '%s' "$prompt" > "$tmpfile"
            trap 'rm -f "$tmpfile"' RETURN
            # shellcheck disable=SC2086
            $OPENCODE_BIN run $OPENCODE_FLAGS --prompt-file "$tmpfile"
            ;;
        *)
            # shellcheck disable=SC2086
            printf '%s' "$prompt" | $OPENCODE_BIN $OPENCODE_FLAGS
            ;;
    esac
    echo "--- Done: $output_path ---"
}

# ---------- helpers ----------
review_id()          { echo "$1" | cut -d'|' -f1; }
review_category()    { echo "$1" | cut -d'|' -f2; }
review_slug()        { echo "$1" | cut -d'|' -f3; }
review_title()       { echo "$1" | cut -d'|' -f4; }
review_vendor_refs() { echo "$1" | cut -d'|' -f5; }
review_files()       { echo "$1" | cut -d'|' -f6; }
review_prompt()      { echo "$1" | cut -d'|' -f7-; }

output_path_for() {
    local cat; cat=$(review_category "$1")
    local id;  id=$(review_id "$1")
    local slug; slug=$(review_slug "$1")
    echo "$REVIEWS_DIR/$cat/$id-$slug.md"
}

# ---------- review manifest ----------
# Format:  ID | CATEGORY | SLUG | TITLE | VENDOR_REF_FILES | HARES_FILES | PROMPT
# Fields separated by | (pipe).  Prompt is the rest of the line.
declare -a REVIEWS=()

# ═══════════════════════════════════════════════════════════════
#  HPXML INPUT PARSING   (hpxml-01 … hpxml-12)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("hpxml-01|hpxml|duct-leakage-cfm25-silent-skip|HPXML duct leakage CFM25 units silently skipped|vendors/OCHRE/ochre/utils/hpxml.py crates/hares-io/src/hpxml/building.rs|docs/hpxml/hpxml-elements.md|crates/hares-io/src/hpxml/building.rs crates/hares-io/src/hpxml/equipment.rs|When HPXML <DuctLeakageMeasurements/Units> is CFM25 (volumetric flow, not fraction), the parser logs tracing::warn and skips the value, producing zero duct leakage for DSE. OCHRE handles this by using fan flow to convert CFM25 to a fraction. Review the code at building.rs:1809-1812. Check: does HARES have fan flow available at parse time? Should this be a hard error instead of silent skip? Compare against OCHRE handle at hpxml.py DSE computation. Write findings to the output file.")

REVIEWS+=("hpxml-02|hpxml|wh-jacket-ua-reduction-mismatch|Water heater jacket R-value UA reduction timing mismatch vs OCHRE|vendors/OCHRE/ochre/utils/hpxml.py#L1132-L1138 crates/hares-equipment/src/water_heater/wh_config.rs|docs/equipment/water-heater.md|crates/hares-io/src/hpxml/resolve_water_heater.rs crates/hares-io/src/hpxml/water_heater_ua.rs|OCHRE reduces tank UA by jacket insulation at parse time (hpxml.py:1132-1138). HARES parses jacket_r_value_m2_k_w and passes it through to the equipment config, but the stored ua_w_per_k is un-reduced. Verify that the water heater equipment model correctly applies the jacket UA reduction at init time. If it does NOT, this is a silent energy balance error. Check resolve_water_heater.rs jacket handling vs wh_config.rs and the tank.rs init path. Write findings to the output file.")

REVIEWS+=("hpxml-03|hpxml|ceiling-height-fallback|Ceiling height defaults to 2.5m with only a warning|vendors/OCHRE/ochre/utils/hpxml.py|docs/hpxml/hpxml-elements.md|crates/hares-io/src/hpxml/building.rs|When conditioned volume or floor area is absent, the parser silently defaults ceiling height to 2.5 m (building.rs:768-774) with only tracing::warn. This propagates into zone volumes, attic geometry, and infiltration calculations. Review: should this be a hard error requiring explicit input? What does OCHRE do for missing ceiling height? Check downstream propagation: does attic volume calculation depend on this fallback? Write findings to the output file.")

REVIEWS+=("hpxml-04|hpxml|foundation-wall-area-zero|Foundation wall area defaults to 0.0 for missing Area element|vendors/OCHRE/ochre/utils/hpxml.py|docs/hpxml/hpxml-elements.md|crates/hares-io/src/hpxml/building.rs|Unlike walls/roofs/floors which hard-error on missing area, FoundationWall and Slab areas silently default to 0.0 (building.rs:1281). This means a malformed foundation wall produces zero heat loss with no diagnostic. Review: is this intentional (some HPXML files omit foundation wall area, derived from height×perimeter)? Compare OCHRE behavior. Should missing area produce a warning or derived value? Write findings to the output file.")

REVIEWS+=("hpxml-05|hpxml|refrigerator-location-string-match|Refrigerator gain fraction zeroing uses fragile string matching|vendors/OCHRE/ochre/utils/hpxml.py|docs/hpxml/hpxml-enumerations.md|crates/hares-io/src/hpxml/resolve_loads.rs|is_conditioned_location() in resolve_loads.rs matches only 'conditioned space', 'living space', or 'indoor'. A refrigerator in 'basement - conditioned' (valid HPXML zone) is treated as non-conditioned with zero gains. Review against valid HPXML zone labels; check what OCHRE does for this edge case. Should matching use zone type enum instead of string comparison? Write findings to the output file.")

REVIEWS+=("hpxml-06|hpxml|infiltration-unit-conversion-exponent|Infiltration ACHnatural/CFMnatural conversion uses hardcoded exponent|vendors/OCHRE/ochre/utils/hpxml.py crates/hares-physics/src/infiltration.rs|docs/hpxml/hpxml-units.md|crates/hares-io/src/hpxml/building.rs|ach_nat_to_ach50() uses NATURAL_TO_50PA_EXPONENT from hares_physics::infiltration. The same exponent applies to both ACH and CFM rates (building.rs:2170, 2200). Review whether using the identical power-law exponent for CFM (which depends on house volume) is correct, or whether CFM should be normalized by volume first. Check ASHRAE 119 reference. Write findings to the output file.")

REVIEWS+=("hpxml-07|hpxml|adjacent-zone-boundaries-filtered|Adjacent (adiabatic) zone boundaries silently filtered out|vendors/OCHRE/ochre/utils/hpxml.py vendors/OCHRE/ochre/Models/Envelope.py|docs/hpxml/hpxml-elements.md|crates/hares-io/src/hpxml/building.rs|ZoneType::Adjacent maps to HPXML 'other' zone names (multifamily adjacent units). All adjacent zones are filtered out at building.rs:758-760 before zones_vec is built, meaning heat transfer to/from adjacent dwelling units is silently ignored. Review: is this acceptable in all simulation modes? Should there be a configurable adiabatic boundary model? What does OCHRE do with adjacent zones? Write findings to the output file.")

REVIEWS+=("hpxml-08|hpxml|wh-uef-ef-regression-constants|Water heater UEF→EF regression constants are hardcoded OCHRE-specific values|vendors/OCHRE/ochre/utils/hpxml.py vendors/OCHRE/ochre/Equipment/WaterHeater.py|docs/equipment/water-heater.md|crates/hares-io/src/hpxml/water_heater_ua.rs crates/hares-io/src/hpxml/resolve_water_heater.rs|Constants GAS_UEF_TO_EF_SLOPE (0.9066), GAS_UEF_TO_EF_INTERCEPT (0.0711), electric (2.4029/1.2844), and HPWH UEF→COP coefficient (1.174536058) come from ResStock / Maguire & Roberts 2020. The low-power HPWH path (UEF==4.9) is OCHRE calibration. Review: should these be configurable from defaults CSV rather than compiled in? Are the regression constants valid for the latest HPXML 4.x UEF values? Check original source citations. Write findings to the output file.")

REVIEWS+=("hpxml-09|hpxml|hvac-efficiency-normalization|HVAC efficiency metric normalization uses uniform conversion factors|vendors/OCHRE/ochre/utils/hpxml.py vendors/OCHRE/ochre/Equipment/HVAC.py|docs/hpxml/hpxml-elements.md|crates/hares-io/src/hpxml/resolve_hvac.rs|insert_annual_efficiency() and normalize_efficiency_units() convert SEER2→SEER, HSPF2→HSPF, EER2→EER using hardcoded factors (1/0.95, 1/0.85 or 1/0.90, 1/0.96). These are applied uniformly regardless of equipment type, size bin, split vs packaged, or installation type. The CEC conversion table shows EER = EER2 × factor varying from 1.038 to 1.043 by equipment type. Review whether using constant conversion factors creates systematic efficiency bias. Check AHRI 210/240-2023 conversion rules. Write findings to the output file.")

REVIEWS+=("hpxml-10|hpxml|hpxml-validation-coverage-gaps|HPXML validation test coverage gaps: 3.x fixtures, multi-heating, pool/spa|vendors/OCHRE/test/|docs/hpxml/hpxml-elements.md tests/fixtures/hpxml/|crates/hares-io/src/hpxml/validation.rs crates/hares-io/tests/*.rs|No test uses HPXML 3.x fixtures (only 4.0). No test for multiple heating systems or multiple water heaters. No test for HPWH low-power (120V) path. No test for generator with missing ElectricalPowerOutput. No test for pool/spa equipment via MiscLoads. Review: which of these gaps represent real risk? Audit test fixture inventory in tests/fixtures/hpxml/ against OCHRE test fixtures. Write findings to the output file.")

REVIEWS+=("hpxml-11|hpxml|hpxml-xml-helpers|HPXML XML helper utilities: correctness and edge cases|vendors/OCHRE/ochre/utils/base.py vendors/OCHRE/ochre/utils/hpxml.py|docs/hpxml/hpxml-elements.md docs/hpxml/hpxml-data-types.md|crates/hares-io/src/hpxml/xml_helpers.rs crates/hares-io/src/hpxml/mod.rs|Review the XML helper functions: normalize_name(), parse_value_with_units(), temperature/Fahrenheit conversion, area/volume/length unit parsing. Check: does normalize_name handle all HPXML element naming variants? Are unit conversion constants mathematically exact? Check ASHRAE 152 duct leakage value parsing. Verify that quick_xml error messages are propagated with context. Write findings to the output file.")

REVIEWS+=("hpxml-12|hpxml|hpxml-building-geometry-garage-attic|HPXML garage and attic geometry computation correctness|vendors/OCHRE/ochre/utils/hpxml.py#L560-L650|docs/hpxml/hpxml-elements.md|crates/hares-io/src/hpxml/building.rs|Review compute_garage_geometry() and compute_attic_volume() for physical correctness. Check: garage protrusion area computation, attic compound volume (prism + pyramid), gable wall count vs garage presence, roof pitch to tilt conversion (atan(pitch/12)). Compare against OCHRE's geometry computations and known FIXMEs about Attic↔Garage wall deletion. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  WEATHER DATA PIPELINE   (weather-01 … weather-08)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("weather-01|weather|resstock-ghi-no-upper-bound|ResStock CSV GHI has no upper validation bound unlike other formats|vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc|docs/weather-pipeline.md|crates/hares-io/src/resstock_csv.rs crates/hares-io/src/epw.rs crates/hares-io/src/tmy3.rs crates/hares-io/src/psm3.rs|TMY3, EPW, and PSM3 all validate GHI ≤ 1500 W/m². ResStock CSV validates GHI only ≥ 0 with no upper bound. A corrupted value of 10000 W/m² would pass validation but produce physically impossible solar output. Review: should the same 1500 W/m² ceiling be added? What is the expected max GHI for ResStock data sources (AMY 2018, etc.)? Write findings to the output file.")

REVIEWS+=("weather-02|weather|brunt-idso-sky-emissivity-dead-code|Brunt and Idso sky emissivity models implemented but unwired|vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc|docs/weather-pipeline.md|crates/hares-io/src/epw.rs|brunt_sky_emissivity() and idso_sky_emissivity() in epw.rs are fully implemented and tested but #[allow(dead_code)] and not selectable. EnergyPlus exposes these as user-selectable SkyTempModel options. Review: is model selection planned? If not, should dead code be removed or documented as intentionally excluded? If planned, what needs to be added (config enum, weather init, cascade wiring)? Write findings to the output file.")

REVIEWS+=("weather-03|weather|ground-temp-reference-depth|Ground temperature reference depth mismatch vs OCHRE (0.5m vs 2m)|vendors/OCHRE/ochre/utils/schedule.py vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc|docs/weather-pipeline.md|crates/hares-physics/src/ground.rs crates/hares-io/src/epw.rs|HARES uses DOE2_GROUND_REFERENCE_DEPTH_M = 0.5 (shallow foundation), while OCHRE uses ~2m reference depth. This produces significantly less amplitude damping in HARES (gm ≈ 0.97 vs ~0.80). The EPW GROUND TEMPERATURES header specifies 0.5m as reference for GroundTemperatures:Surface. Review: is 0.5m correct for building foundation heat transfer? Should this be a configurable parameter? Check EnergyPlus ground temperature model reference depths. Write findings to the output file.")

REVIEWS+=("weather-04|weather|epw-midpoint-offset|EPW hour-ending midpoint offset convention verification|vendors/OCHRE/ochre/utils/schedule.py vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc|docs/weather-pipeline.md|crates/hares-io/src/epw.rs crates/hares-io/src/weather.rs|EPW uses midpoint_offset_secs = 1800 (30 min). TMY3 and ResStock CSV also use 1800s. PSM3 uses 0 (hour-beginning). OCHRE applies offset = timedelta(minutes=30). Review: is the 30-min offset correct for all EPW variants? Does the offset interact correctly with DST handling and leap-year indexing? Check EnergyPlus WeatherManager conventions. Write findings to the output file.")

REVIEWS+=("weather-05|weather|triangular-resampling-footgun|Triangular solar resampling energy non-conservation — feature or footgun?|vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc|docs/weather-pipeline.md|crates/hares-io/src/weather.rs|ResampleMethod::Triangular docstring warns of 12.5% energy shortfall at sunrise and boundary leakage into nighttime. It is only accessible via ResampleOverrides, never used as default. EnergyPlus also uses triangular interpolation with the same non-conservation issue. Review: are there legitimate use cases for triangular resampling? Should this be removed (footgun) or kept with stronger warnings? Check EnergyPlus solar interpolation discussion in WeatherManager.cc:3076-3116. Write findings to the output file.")

REVIEWS+=("weather-06|weather|resstock-constant-pressure|ResStock CSV uses constant ISA atmospheric pressure for all timesteps|vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc|docs/weather-pipeline.md|crates/hares-io/src/resstock_csv.rs|ResStock CSV has no per-row pressure data; uses a single ISA-derived pressure for all 8760 timesteps. Real atmospheric pressure varies ±2-3 kPa diurnally and ±5 kPa with weather systems. Review: what is the quantitative impact on infiltration, HVAC fan curves, and psychrometric calculations? Is a simple sinusoidal diurnal variation model justified? Check what OCHRE and EnergyPlus do for pressure when absent from weather data. Write findings to the output file.")

REVIEWS+=("weather-07|weather|missing-ddy-stat-format|Missing .ddy and .stat weather file format support|vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc vendors/EnergyPlus/weather/|docs/weather-pipeline.md|crates/hares-io/src/epw.rs crates/hares-io/src/weather.rs crates/hares-core/src/dwelling/autosize.rs|EnergyPlus uses .stat files for sizing and .ddy for design day definitions. HARES parses DesignConditions from EPW header line 2 for heating/cooling design temperatures but does not parse standalone DDY/STAT files. Review: does ASHRAE 152 sizing compliance require dedicated design-day weather input? What does OCHRE use for design-day sizing? Check autosize.rs fallback path when EPW header is unavailable. Write findings to the output file.")

REVIEWS+=("weather-08|weather|weather-test-coverage-gaps|Weather test coverage gaps: synthetic weather, edge-case formats|vendors/OCHRE/ochre/utils/schedule.py|docs/weather-pipeline.md tests/fixtures/weather/|crates/hares-io/src/epw.rs crates/hares-io/src/tmy3.rs crates/hares-io/src/psm3.rs crates/hares-io/src/resstock_csv.rs|Review weather test coverage completeness. Check: leap-year EPW (8784 rows), PSM3 with 5-min resolution, weather files with missing columns, extreme temperature values, DST transitions in weather data, and multi-year weather support. Audit all test fixtures in tests/fixtures/weather/ and parity fixtures. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  THERMAL ENVELOPE SOLVER   (envelope-01 … envelope-12)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("envelope-01|envelope|radiation-frac-starmesh|radiation_frac voltage-divider formula wrong in StarMesh topology|vendors/OCHRE/ochre/Models/Envelope.py vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc|docs/thermal-envelope-solver.md docs/findings/|crates/hares-envelope/src/thermal_solver/solar.rs crates/hares-envelope/src/rc_network.rs crates/hares-envelope/src/state_space.rs|The formula r_film_conv/(r_film_conv + r_inner_half) was derived for ScriptF series-resistor topology. In StarMesh mode where surface_node is Y-Δ eliminated, it overestimates radiation_frac by ~27% (0.90 vs ~0.71 correct), biasing solar/radiant gains toward thermal mass rather than zone air. Review: what is the correct radiation_frac formula for StarMesh topology? Is this the root cause of BESTEST 900FF discrepancies? Check EnergyPlus Option 2 (StarMesh) radiation split documentation. Write findings to the output file.")

REVIEWS+=("envelope-02|envelope|frozen-film-coefficients|Interior film coefficients frozen at init — should be per-timestep|vendors/OCHRE/ochre/Models/Envelope.py vendors/EnergyPlus/src/EnergyPlus/ConvectionCoefficients.cc|docs/findings/ docs/thermal-envelope-solver.md|crates/hares-physics/src/film_coefficients.rs crates/hares-envelope/src/rc_network.rs crates/hares-envelope/src/thermal_solver/stepping.rs|TARP natural convection is evaluated once at initialization with 0.1°C ΔT floor. The resulting R_film is frozen for all timesteps. At min-temp conditions (ΔT≈2°C), actual TARP gives R≈0.147 vs frozen R≈0.122 — a 17% coupling overestimate. The diagnostic path in stepping.rs DOES recompute TARP per timestep, proving the machinery exists. Review: wire per-timestep TARP evaluation into the production path, or document the frozen-film limitation with error bounds. Check EnergyPlus per-timestep TARP evaluation. Write findings to the output file.")

REVIEWS+=("envelope-03|envelope|energy-balance-observability|Full-system energy balance residual not in public telemetry|vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc|docs/invariants-and-observability.md docs/findings/|crates/hares-envelope/src/thermal_solver/stepping.rs crates/hares-core/src/invariants.rs|Per-zone check uses zone air capacitance only (C_zone×ΔT/dt vs q_port). Full-system check (Σ C_i×ΔT_i/dt over all nodes) is debug-level only. No energy_balance_residual_w in public telemetry. Review: export the full-system residual as a telemetry column for observability. Document the expected kW-scale residual from wall mass redistribution. Write findings to the output file.")

REVIEWS+=("envelope-04|envelope|interior-lwr-diagnostic-zero|Interior LWR diagnostic reports zero by construction|vendors/OCHRE/ochre/Models/Envelope.py|docs/findings/ docs/thermal-envelope-solver.md|crates/hares-envelope/src/longwave_radiation.rs crates/hares-envelope/src/thermal_solver/longwave.rs|interior_lwr_w accumulates Σq_i which is always zero (enclosure energy conservation). The diagnostic lwr_by_zone_buf uses Σ|q_i|/2 which IS meaningful, but the top-level component_gains.interior_lwr_w still reports zero-sum. The |q_i|/2 value is buried in interior_lwr_by_zone. Review: expose the correct per-zone LWR magnitude diagnostic. Write findings to the output file.")

REVIEWS+=("envelope-05|envelope|port-radiant-multizone-asymmetry|Port radiant distribution has multi-zone asymmetry|vendors/OCHRE/ochre/Models/Envelope.py|docs/findings/ docs/thermal-envelope-solver.md|crates/hares-envelope/src/thermal_solver/ports.rs crates/hares-core/src/environment.rs|apply_port_radiant_inputs uses TMULT area×emissivity weighting via interior_lwr_zones surfaces, but zone-scoping logic prefers indoor_zone only in some paths. Equipment emitting radiant gains to non-indoor zones (attic, garage) may have radiant components silently dropped. Review: trace all equipment radiant port → zone routing paths. Check if any equipment (generator, water heater, ducts) can emit radiant gains to non-conditioned zones. Write findings to the output file.")

REVIEWS+=("envelope-06|envelope|gershgorin-vs-full-eigenvalue|Gershgorin stability check may produce false positives|vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc|docs/thermal-envelope-solver.md docs/findings/|crates/hares-envelope/src/state_space.rs|from_continuous() uses Gershgorin circle bounds (O(n²)) to avoid nalgebra complex_eigenvalues which can stall on large matrices. The bound is conservative — systems flagged unstable may actually be stable. Full eigenvalue check verify_stability() is available but not called in production. Review: add an optional (feature-gated?) full eigenvalue stability check for debugging. Document the conservatism of Gershgorin bounds. Write findings to the output file.")

REVIEWS+=("envelope-07|envelope|window-exterior-lwr-h-out-guard|Window exterior LWR h_out guard threshold verification|vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc|docs/findings/ docs/thermal-envelope-solver.md|crates/hares-envelope/src/longwave_radiation.rs|Window exterior LWR uses info.h_out_w_m2_k > 0.0 as guard (corrected from prior > 1.0). A computed 0.0 falls back to ASHRAE 34 W/(m²·K). Review: verify the guard comment and warning message reference the NFRC/ASHRAE conventional value consistently. Check that 0.0 threshold is correct for all window film coefficient models (DOE-2, TARP, etc.). Write findings to the output file.")

REVIEWS+=("envelope-08|envelope|infiltration-coupling-lu-allocation|Semi-implicit infiltration coupling LU is the only per-timestep allocation in hot loop|vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc|docs/thermal-envelope-solver.md docs/profile-debug-hangs-and-slowness.md|crates/hares-envelope/src/state_space.rs crates/hares-envelope/src/thermal_solver/infiltration.rs|build_coupled_lu() calls std::mem::take(m_scratch).lu() once per step when infiltration is active. The LU factorization allocates internally (pivot + factor matrices). This is the only per-timestep allocation in the hot loop. Review: is this unavoidable given the per-step coupling design? Could a preallocated workspace be reused? Document the allocation and its performance impact. Write findings to the output file.")

REVIEWS+=("envelope-09|envelope|starmesh-cascading-elimination|StarMesh floating-node cascading elimination edge case|vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc|docs/thermal-envelope-solver.md docs/findings/|crates/hares-envelope/src/rc_network.rs crates/hares-envelope/src/boundary_rc.rs|In StarMesh mode, both the radiation star_node AND per-boundary surface_nodes are floating. reduce_floating_nodes() must eliminate them in correct order (inner first) via iterative loop. Test zone_with_star_and_window_cascading_elimination verifies this works, but no debug_assert checks that a window surface_node with only one neighbor after star elimination doesn't produce a degenerate graph. Review: add a debug_assert for this edge case. Write findings to the output file.")

REVIEWS+=("envelope-10|envelope|steady-state-init-zone-pinning|Steady-state initialization zone pinning creates fragile invariant|vendors/OCHRE/ochre/Models/Envelope.py|docs/thermal-envelope-solver.md docs/findings/|crates/hares-envelope/src/thermal_solver/initialization.rs|initialize_steady_state() pins only conditioned zones; unconditioned zones (attic, garage, foundation) are left free to reach conduction equilibrium. This is physically correct but creates a fragile invariant: if a non-conditioned zone's initial env.zones temperature differs significantly from its steady-state value, the first few timesteps have large transients. Review: add a consistency check or assert. Document the warmup period requirement. Write findings to the output file.")

REVIEWS+=("envelope-11|envelope|precomputed-rc-node-id-ordering|Precomputed RC path node ID allocation ordering is fragile|vendors/OCHRE/ochre/Models/Envelope.py|docs/thermal-envelope-solver.md|crates/hares-envelope/src/boundary_rc.rs|build_precomputed_boundary() allocates capacitor nodes sequentially then surface_node last. n_cap_nodes is computed as graph.next_layer_id - nodes_before minus 1 when surface_node is present. This subtraction is fragile — if future code allocates nodes between capacitor allocation and surface_node allocation, the count breaks. Review: add a debug_assert verifying graph.capacitances.contains_key(&inner_node) to catch regressions. Write findings to the output file.")

REVIEWS+=("envelope-12|envelope|bestest-tolerance-widening|BESTEST tolerance widening masks known physics defects|vendors/OCHRE/ochre/Models/Envelope.py|docs/bestest-900ff-root-cause-analysis.md docs/findings/ tests/bestest/reference_bands.rs|crates/hares-envelope/src/thermal_solver/stepping.rs tests/bestest/|ZONE_TEMP_CONDITIONED_C_MAE_MAX widened from 0.1°C to 0.6°C and PEAK_HVAC_POWER_REL_PCT_MAX to 80%, explicitly citing known defects (step-0 back-solve). Per project policy, defects must be remediated not tolerated. Review: track which tolerance widenings correspond to which open defects. Create a mapping of tolerance → defect ticket so tolerances can be tightened when defects are fixed. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  HVAC EQUIPMENT   (equip-hvac-01 … equip-hvac-10)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("equip-hvac-01|equipment-hvac|furnace-fuel-thermal-balance|Furnace fuel/thermal balance correctness with duct DSE|vendors/OCHRE/ochre/Equipment/HVAC.py|docs/equipment/hvac.md docs/equipment/control-system.md|crates/hares-equipment/src/hvac/furnace.rs crates/hares-equipment/src/hvac/duct_distribution.rs crates/hares-equipment/src/hvac/hvac_core.rs|Verify that fuel consumption is independent of duct DSE (gas burned = capacity/eff, duct loss absorbed by duct zone) and that fan waste heat correctly offsets/reinforces thermal delivery per OCHRE HVAC.py:543 convention. Check: does fan power appear in the correct thermal zone? Does fuel consumption reflect gross (pre-DSE) capacity? Write findings to the output file.")

REVIEWS+=("equip-hvac-02|equipment-hvac|boiler-hydronic-loop-integration|Boiler hydronic loop integration and EIR polynomial paths|vendors/OCHRE/ochre/Equipment/HVAC.py|docs/equipment/hvac.md|crates/hares-equipment/src/hvac/boiler.rs crates/hares-equipment/src/hvac/hvac_core.rs|Audit condensing/non-condensing EIR polynomial paths. Condensing uses zone air temperature for t_in (OCHRE HVAC.py:651). Non-condensing uses outlet water temp with 10-coeff polynomial. Check jacket loss energy conservation at partial loads. Verify flow_rate_kg_s default (0.5 kg/s) and return_temp_c default (40.0°C) are physically reasonable. Write findings to the output file.")

REVIEWS+=("equip-hvac-03|equipment-hvac|heat-pump-defrost-cycle-tracker|Heat pump discrete defrost cycle tracker edge cases|vendors/OCHRE/ochre/Equipment/HVAC.py vendors/EnergyPlus/src/EnergyPlus/Coils/|docs/equipment/hvac.md|crates/hares-equipment/src/hvac/heat_pump/defrost.rs crates/hares-equipment/src/hvac/heat_pump/heater.rs crates/hares-equipment/src/hvac/heat_pump/cooler.rs|Validate the DefrostCycleTracker FSM's inter-defrost interval formula (interval = cycle_duration / time_fraction). Edge case: very low humidity drives time_fraction → 0, interval → ∞. Verify OnDemand and Timed modes produce consistent average defrost penalties when tracked discretely vs continuous averaging. Check activation threshold (OAT ≤ 4.4445°C). Write findings to the output file.")

REVIEWS+=("equip-hvac-04|equipment-hvac|ac-shr-latent-degradation|Air conditioner SHR and latent degradation model parameters|vendors/EnergyPlus/src/EnergyPlus/Coils/ vendors/OCHRE/ochre/Equipment/HVAC.py|docs/equipment/hvac.md|crates/hares-equipment/src/hvac/coil_physics.rs crates/hares-equipment/src/hvac/latent_degradation.rs crates/hares-equipment/src/hvac/air_conditioner.rs|Review Henderson-Rengarajan model parameter selection: twet_rated_s=1500, gamma_rated=1.5, max_cycling_rate=3.0, latent_time_constant_s=45. Check the To fixed-point solver convergence for realistic zone conditions. Verify interaction with per-stage SHR arrays. Compare defaults against EnergyPlus Coil:Cooling:DX defaults. Write findings to the output file.")

REVIEWS+=("equip-hvac-05|equipment-hvac|baseboard-model-completeness|Electric baseboard model: bypass verification|vendors/OCHRE/ochre/Equipment/HVAC.py|docs/equipment/hvac.md|crates/hares-equipment/src/hvac/baseboard.rs crates/hares-equipment/src/hvac/hvac_core.rs|Electric baseboard forces DSE=1.0 and has no duct zone but shares HvacEquipment core. Verify that speed control, staging, PLF degradation, and startup ramp are all correctly bypassed. Check: no HAS_SETPOINT in core capabilities — is this correct? Does baseboard thermal output go to the correct zone? Write findings to the output file.")

REVIEWS+=("equip-hvac-06|equipment-hvac|thermostat-fsm-edge-cases|Thermostat FSM edge cases: deadband, cycle time, cold-start|vendors/OCHRE/ochre/Equipment/HVAC.py|docs/equipment/hvac.md docs/equipment/control-system.md|crates/hares-equipment/src/hvac/thermostat.rs|Verify asymmetric deadband (deadband_offset) behavior for extreme values (0.0 vs 1.0). Check interaction with min_cycle_time_s that auto-clamps to timestep. Verify cold-start mode initialization avoids deadband lock. Test: what happens when setpoints change during a cycle? What happens with external setpoint override? Write findings to the output file.")

REVIEWS+=("equip-hvac-07|equipment-hvac|duct-heat-fraction-basement-routing|Duct heat fraction correctness with basement routing dedup|vendors/OCHRE/ochre/Equipment/HVAC.py|docs/equipment/hvac.md|crates/hares-equipment/src/hvac/duct_distribution.rs crates/hares-equipment/src/hvac/hvac_core.rs|Audit zone_heat_fractions computation when basement_zone == duct_zone (deduplication merges them, but the merged entry gets tagged DuctLoss even though part is basement heat routing). Verify total wattage is physically correct regardless of category tagging. Check that basement_heat_frac is 0 for cooling. Write findings to the output file.")

REVIEWS+=("equip-hvac-08|equipment-hvac|multispeed-curve-interleaving|Multi-speed biquadratic curve interleaving correctness|vendors/OCHRE/ochre/Equipment/HVAC.py|docs/equipment/hvac.md|crates/hares-equipment/src/hvac/hvac_core.rs crates/hares-physics/src/biquadratic.rs|Capacity and EIR curves are interleaved per speed stage: [cap_0, eir_0, cap_1, eir_1, ...]. Validate index calculation for fetch: even indices = capacity, odd = EIR. Check flow-fraction correction application. Verify PLF division of EIR curves is correct at part load. Check default curve substitution logic in default_curves.rs. Write findings to the output file.")

REVIEWS+=("equip-hvac-09|equipment-hvac|ideal-hvac-back-solve|Ideal HVAC unlimited-capacity back-solve correctness|vendors/OCHRE/ochre/Equipment/HVAC.py|docs/equipment/hvac.md|crates/hares-equipment/src/hvac/ideal_hvac.rs crates/hares-envelope/src/thermal_solver/stepping.rs|Ideal HVAC provides unlimited thermal capacity. The solver back-calculates required capacity from zone setpoint. Review: is the back-solve correct for multi-zone? What happens when both heating and cooling are disabled? Check interaction with infiltration coupling. Write findings to the output file.")

REVIEWS+=("equip-hvac-10|equipment-hvac|dehumidifier-implementation|Dehumidifier model completeness audit|vendors/OCHRE/ochre/Equipment/|docs/equipment/hvac.md|crates/hares-equipment/src/hvac/dehumidifier.rs crates/hares-equipment/src/hvac/dehumidifier_defaults.rs|The dehumidifier is registered as a canonical equipment type. Audit: does it have a complete step() implementation? How does it interact with the humidity solver? Check: what happens when dehumidistat setpoint is absent? Is the latent removal physically correct (moisture mass flow in kg/s)? Write findings to the output file.")

REVIEWS+=("equip-hvac-11|equipment-hvac|speed-control-modes|Multi-speed and variable-speed control mode completeness and correctness|vendors/OCHRE/ochre/Equipment/HVAC.py|docs/equipment/hvac.md docs/equipment/control-system.md|crates/hares-equipment/src/hvac/speed_control.rs crates/hares-equipment/src/hvac/staging.rs|Six speed control modes (SingleSpeed, TwoSpeedSetpoint, TwoSpeedTime, TwoSpeedAlternating, MultiSpeedInterpolated, VariableSpeedIdeal). Review: are all six fully implemented? Verify speed selection transitions are physically plausible (no instantaneous jumps). Check PLF/PLR calculation for partial-load cycling at lowest speed. Verify disabled-speed-stage external control interaction. Write findings to the output file.")

REVIEWS+=("equip-hvac-12|equipment-hvac|hvac-equivalent-battery|HVAC as equivalent battery model for demand response|vendors/EnergyPlus/src/EnergyPlus/HVACManager.cc|docs/equipment/hvac.md docs/equipment/control-system.md|crates/hares-equipment/src/hvac/equivalent_battery.rs|The equivalent battery model represents HVAC thermal storage as electrical storage for DR dispatch. Review: is the thermal capacitance ↔ electrical energy mapping physically correct? Are the charge/discharge power limits consistent with equipment capacity? Check deadband and temperature constraints. Write findings to the output file.")

REVIEWS+=("equip-hvac-13|equipment-hvac|room-ac-vs-central-ac|Room AC vs Central AC model differences and defaults|vendors/OCHRE/ochre/Equipment/HVAC.py|docs/equipment/hvac.md|crates/hares-equipment/src/hvac/air_conditioner.rs crates/hares-equipment/src/hvac/core_config.rs|Room AC has no duct zone, uses different CFM/ton (320 vs 400), and has different compressor type and speed count defaults. Review: are all room-AC-specific paths correct? Check: DSE forced to 1.0, no duct zone, window-mount heat loss, thermostat location implications. Write findings to the output file.")

REVIEWS+=("equip-hvac-14|equipment-hvac|startup-ramp-winkler|Startup capacity degradation ramp (Winkler 2011) correctness|vendors/OCHRE/ochre/Equipment/HVAC.py|docs/equipment/hvac.md|crates/hares-equipment/src/hvac/hvac_core.rs crates/hares-equipment/src/hvac/staging.rs|Startup ramp: mult = clamp(0,1, -1.025*exp(-3.79936*t/t_full) + 1.025) where t_full = 20*Cd + 0.4. Review: are the Winkler 2011 coefficients correctly ported? Does the ramp reset on every off-cycle? Is it correctly bypassed for variable-speed (Cd=0) and ideal HVAC? Check interaction with min_cycle_time_s. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  WATER HEATERS   (equip-wh-01 … equip-wh-05)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("equip-wh-01|equipment-wh|tank-inversion-correction|Water heater tank PAV inversion correction effectiveness|vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc vendors/OCHRE/ochre/Models/Water.py|docs/equipment/water-heater.md|crates/hares-equipment/src/water_heater/tank.rs|The PAV (piecewise average volumes) algorithm corrects temperature inversions energy-conservatively. Verify behavior at boundaries (top/bottom nodes) and with the positive displacement draw model where mains water injection can create inversions at the bottom. Compare against EnergyPlus WaterHeater:Stratified inversion correction (Engineering Reference §14.8). Write findings to the output file.")

REVIEWS+=("equip-wh-02|equipment-wh|draw-tempering-model|Water heater draw tempering mixing valve model|vendors/OCHRE/ochre/Equipment/WaterHeater.py|docs/equipment/water-heater.md|crates/hares-equipment/src/water_heater/tank.rs|The mixing valve logic combines tank outlet with mains water to reach fixture temperature. Audit the unmet_load calculation when outlet temp is below fixture target but above mains temp. Verify that outlet_temp_c is correctly snapshotted pre-step to prevent same-step heat inflation. Check tempering valve mass balance. Write findings to the output file.")

REVIEWS+=("equip-wh-03|equipment-wh|hpwh-compressor-curves|HPWH compressor biquadratic curve parameter audit|vendors/OCHRE/ochre/Equipment/WaterHeater.py|docs/equipment/water-heater.md|crates/hares-equipment/src/water_heater/heat_pump_wh.rs crates/hares-equipment/src/water_heater/hpwh_compressor.rs|Review HPWH compressor COP and capacity biquadratic curves. Check: are the independent variables correct (air dry-bulb, water temperature)? Are the default curve coefficients physically sourced? Verify the low-power HPWH path (UEF==4.9) is detected and handled. Write findings to the output file.")

REVIEWS+=("equip-wh-04|equipment-wh|tankless-model-correctness|Tankless water heater model physics audit|vendors/OCHRE/ochre/Equipment/WaterHeater.py|docs/equipment/water-heater.md|crates/hares-equipment/src/water_heater/tankless.rs|Tankless WH has no storage tank — verify the step() logic correctly handles instantaneous heating, flow-rate-dependent efficiency, and minimum flow rate activation. Check: is the thermal mass of the heat exchanger modeled? Does the model handle simultaneous draws? Write findings to the output file.")

REVIEWS+=("equip-wh-05|equipment-wh|wh-skin-loss-end-caps|Water heater tank end-cap skin loss correction|vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc|docs/equipment/water-heater.md|crates/hares-equipment/src/water_heater/tank.rs|Top/bottom nodes get additional ua_w_per_k * 0.1 (per EnergyPlus §14.8). Review: is this correction applied correctly for all node counts (1-12)? Is the factor 0.1 appropriate or should it scale with tank geometry? Check EnergyPlus WaterThermalTanks.cc end-cap loss computation. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  DER: PV, BATTERY, EV, GENERATOR   (equip-der-01 … equip-der-08)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("equip-der-01|equipment-der|battery-degradation-daily-boundary|Battery degradation daily update at midnight boundary|vendors/OCHRE/ochre/Equipment/Battery.py|docs/equipment/battery.md|crates/hares-equipment/src/battery/degradation.rs crates/hares-equipment/src/battery/mod.rs|Smith 2017 model updates at midnight. Verify partial cycles straddling midnight are correctly split between days. Check RainflowCounter::reset_daily() preserves the reversal buffer as documented (residential cycles can straddle midnight). Write findings to the output file.")

REVIEWS+=("equip-der-02|equipment-der|battery-ocv-chemistry-injection|Battery OCV chemistry selection and custom table injection|vendors/OCHRE/ochre/Equipment/Battery.py|docs/equipment/battery.md|crates/hares-equipment/src/battery/ocv.rs crates/hares-equipment/src/battery/mod.rs|Validate that custom OCV table injection via set_ocv_table() correctly propagates to terminal voltage calculation AND the degradation model's Tafel correction at the graphite anode. Check 4 default chemistries (NMC, NMC811, LFP, NCA, LTO). Write findings to the output file.")

REVIEWS+=("equip-der-03|equipment-der|ev-charging-strategy-interaction|EV charging strategy priority and interaction|vendors/OCHRE/ochre/Equipment/EV.py|docs/equipment/ev.md|crates/hares-equipment/src/ev/mod.rs crates/hares-equipment/src/ev/charging_curve.rs|Verify priority logic when multiple constraints are active: Immediate Hold, TOU avoidance, Ready-By mandate, and PowerLimit. Check that cc_cv_margin (0.85) correctly accounts for CV-region taper when computing required power for Ready-By charging. Review V2L/V2G path completeness. Write findings to the output file.")

REVIEWS+=("equip-der-04|equipment-der|ev-degradation-sharing-with-battery|EV degradation model sharing Battery degradation pipeline|vendors/OCHRE/ochre/Equipment/EV.py|docs/equipment/ev.md|crates/hares-equipment/src/ev/mod.rs crates/hares-equipment/src/battery/degradation.rs|Both EV and Battery use the same DegradationState and RainflowCounter. Verify the degradation pipeline (push SOC → extract cycles → daily update) works correctly for EV's discharge-then-charge cycles vs Battery's more frequent micro-cycling. Check that EV's larger depth-of-discharge cycles produce consistent degradation predictions. Write findings to the output file.")

REVIEWS+=("equip-der-05|equipment-der|pv-soiling-lut-interaction|PV soiling model interaction with PVWatts vs SAM LUT paths|vendors/OCHRE/ochre/Equipment/PV.py|docs/equipment/pv.md|crates/hares-equipment/src/pv/mod.rs crates/hares-equipment/src/pv/soiling.rs crates/hares-equipment/src/pv/lut.rs|PV model has two DC power paths (PVWatts and 6-D SAM LUT). Soiling model applies as pre-LUT derating in PVWatts path but is described as post-LUT derating in the LUT path. Verify soiling_ratio correctly feeds into both paths consistently. Check Kimber soiling rain accumulation edge cases. Write findings to the output file.")

REVIEWS+=("equip-der-06|equipment-der|pv-inverter-priority-reactive-power|PV inverter priority mode reactive power limits and PF enforcement|vendors/OCHRE/ochre/Equipment/PV.py|docs/equipment/pv.md|crates/hares-equipment/src/pv/mod.rs crates/hares-equipment/src/pv/array_config.rs|Audit inverter priority modes (Watt/Var/Cpf) and min-PF enforcement (default 0.8). Verify that when PowerSetpoint provides both P and Q, the reactive setpoint interacts correctly with the priority mode and doesn't overshoot apparent power rating. Check inverter capacity clipping interaction. Write findings to the output file.")

REVIEWS+=("equip-der-07|equipment-der|generator-chp-thermal-routing|Generator CHP thermal routing energy balance|vendors/OCHRE/ochre/Equipment/Generator.py|docs/equipment/generator.md|crates/hares-equipment/src/generator.rs|Generator has CHP thermal ports (fluid loop and flue loss to zone). Audit energy balance: P_fuel = P_electric + Q_thermal + Q_flue. Verify CHP fluid supply/return temperatures are physically reasonable. Compare against OCHRE's stubbed CHP implementation and EnergyPlus Generator:FuelCell. Write findings to the output file.")

REVIEWS+=("equip-der-08|equipment-der|ev-driver-archetype-behavior|EV driver archetype behavior and SOC estimation divergence|vendors/OCHRE/ochre/Equipment/EV.py|docs/equipment/ev.md|crates/hares-core/src/actors/ev_driver/|crates/hares-core/src/actors/ev_driver/ crates/hares-equipment/src/ev/mod.rs|EvDriverActor maintains estimated_soc that intentionally diverges from equipment SOC (actor doesn't observe CC-CV taper, thermal derating, BMS termination). Review: what is the max divergence across multi-day simulations? Can range anxiety fire late (or not at all) when actual SOC < estimated? Check all 7 driver archetype log-normal distributions. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  SCHEDULED/EVENT LOADS, VENTILATION   (equip-loads-01 … equip-loads-05)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("equip-loads-01|equipment-loads|ventilation-bypass-defrost-derating|Ventilation bypass and defrost derating propagation to solver|vendors/EnergyPlus/src/EnergyPlus/HeatRecovery.cc|docs/equipment/loads.md|crates/hares-equipment/src/ventilation.rs crates/hares-envelope/src/thermal_solver/infiltration.rs|effective_ventilation_effectiveness() returns (sensible, latent) accounting for bypass and defrost. Verify these values are propagated to ThermalSolverConfig.ventilation before each thermal solver step. Check supply air conditions. Compare against EnergyPlus HeatExchanger:AirToAir:SensibleAndLatent. Write findings to the output file.")

REVIEWS+=("equip-loads-02|equipment-loads|scheduled-load-zip-zone-routing|Scheduled load ZIP model and auto-zone routing by name convention|vendors/OCHRE/ochre/Equipment/ScheduledLoad.py vendors/OCHRE/ochre/Equipment/EventBasedLoad.py|docs/equipment/loads.md|crates/hares-equipment/src/scheduled_load.rs crates/hares-equipment/src/event_load.rs|ZIP model (z*v² + i*v + p_coeff) has reactive coefficients defaulting to zero. Verify garage/basement/exterior zone auto-routing by equipment name convention is robust. Check thermal gain fractions (sensible/radiant/latent) are correctly split when zone is None (outdoor loss). Audit all 14+ scheduled load types. Write findings to the output file.")

REVIEWS+=("equip-loads-03|equipment-loads|event-load-wet-appliance|Event-based load wet appliance model completeness|vendors/OCHRE/ochre/Equipment/EventBasedLoad.py vendors/OCHRE/ochre/Equipment/WetAppliance.py|docs/equipment/loads.md|crates/hares-equipment/src/event_load.rs|Clothes washer, dishwasher, clothes dryer, and cooking range use the event-based load model. Review: is the multi-phase flow rate model from OCHRE WetAppliance correctly implemented? Check: dryer CEF vs EF conversion, vented/unvented gas dryer latent gains, dishwasher water consumption impact on water heater. Write findings to the output file.")

REVIEWS+=("equip-loads-04|equipment-loads|lighting-gain-fraction-defaults|Lighting thermal gain fraction defaults and location routing|vendors/OCHRE/ochre/utils/hpxml.py vendors/OCHRE/ochre/Equipment/ScheduledLoad.py|docs/equipment/loads.md docs/hpxml/hpxml-enumerations.md|crates/hares-io/src/hpxml/resolve_loads.rs crates/hares-equipment/src/scheduled_load.rs|Lighting gain fractions (sensible/radiant/latent) vary by lighting type (LED, CFL, incandescent). Verify defaults are correct for each type. Check: does garage lighting route gains to garage zone? Does exterior lighting route gains outdoors (loss)? Compare against OCHRE lighting gain handling. Write findings to the output file.")

REVIEWS+=("equip-loads-05|equipment-loads|appliance-defaults-citation|Appliance default energy values: source citation and coverage|vendors/OCHRE/ochre/utils/hpxml.py|docs/equipment/loads.md docs/hpxml/hpxml-elements.md|crates/hares-io/src/hpxml/resolve_loads.rs|Appliance defaults (RatedAnnualkWh formulas for refrigerator, freezer, dishwasher, cooking range, etc.) are hardcoded. Review: what is the source for each default? Are the bedroom-count-adjustment formulas from the correct ANSI/RESNET 301 version? Check if any defaults are stale vs HPXML 4.2 conventions. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  CORE SIMULATION ENGINE   (core-01 … core-12)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("core-01|core|thermal-invariant-unwired|Thermal energy balance invariant defined but not wired into Dwelling|vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc|docs/invariants-and-observability.md|crates/hares-core/src/invariants.rs crates/hares-core/src/dwelling/mod.rs|InvariantChecker::check_thermal() is fully defined with tolerance logic (absolute 1.0 W, relative 1e-6 × gross flux) but NOT called from Dwelling::check_invariants(). The comment at dwelling/mod.rs:3135-3142 documents this as deferred. Review: wire it in behind the check_invariants feature gate. What API does ThermalSolver need to expose for per-node capacitance tracking? Write findings to the output file.")

REVIEWS+=("core-02|core|moisture-balance-invariant|Moisture balance invariant: semi-implicit coupling correctness|vendors/EnergyPlus/src/EnergyPlus/HumidityManager.cc|docs/invariants-and-observability.md|crates/hares-core/src/invariants.rs crates/hares-core/src/dwelling/mod.rs crates/hares-envelope/src/humidity_solver.rs|check_invariants() moisture mass conservation back-calculates from thermal domain quint-coded payloads. Review: verify the 5-number per-zone payload format [zone_id, q_latent_w, m_dot_inf_kg_s, w_outdoor, energy_balance_residual_w] is correctly aligned between thermal solver, humidity solver, and invariant checker. Check that clamping of humidity ratios doesn't invalidate the balance. Write findings to the output file.")

REVIEWS+=("core-03|core|solver-feedback-loop-ordering|Solver feedback loop run_timestep() phase sequencing audit|vendors/OCHRE/ochre/Dwelling.py vendors/EnergyPlus/src/EnergyPlus/HVACManager.cc|docs/architecture.md docs/equipment/control-system.md|crates/hares-core/src/engine.rs crates/hares-core/src/dwelling/mod.rs|run_timestep() has a precise 10-phase ordering. update_control() is called twice per timestep (pre-solver and post-dispatch). IdealCapacity dispatch from solver feedback must flow through actors before second update_control() and before equipment step. Audit this dual-dispatch, dual-update-control pattern. Verify no phase ordering violation possible. Write findings to the output file.")

REVIEWS+=("core-04|core|control-dispatch-cross-pass-ledger|Control dispatch cross-pass priority ledger semantics|vendors/OCHRE/ochre/Dwelling.py|docs/architecture.md docs/equipment/control-system.md|crates/hares-control/src/dispatch.rs crates/hares-core/src/dwelling/mod.rs|ControlDispatcher maintains a seen_targets ledger surviving across multiple dispatch passes within a timestep. The ledger ensures lower-priority signals in later passes can't overwrite higher-priority signals from earlier passes. Review: Step 1a signals are not rejected (ledger starts empty) while identical Step 2 signals would be. Is this asymmetry correct? Verify the relative ordering of Step 1a queue entries (they drain in tier order within pass). Write findings to the output file.")

REVIEWS+=("core-05|core|equipment-execution-stage-ordering|Equipment execution ordering via stage ranks — same-stage implicit dependencies|vendors/OCHRE/ochre/Dwelling.py|docs/architecture.md|crates/hares-core/src/dwelling/mod.rs crates/hares-types/src/domain_solver.rs|Equipment executes in pre-computed order based on ExecutionStage rank (Independent=0, Electrical=1, Thermal=2, EnvelopeResolution=3). Within a stage, order is unspecified. Are there any equipment pairs in the same stage that silently depend on each other's outputs (e.g., Battery and PV both at Electrical)? Review the accumulator visibility window for same-stage equipment. Write findings to the output file.")

REVIEWS+=("core-06|core|dst-schedule-indexing|DST-aware schedule indexing correctness at transitions|vendors/OCHRE/ochre/utils/schedule.py|docs/schedule-sources.md|crates/hares-core/src/environment.rs crates/hares-core/src/clock.rs|compute_schedule_idx() has two paths: DST-enabled (chrono_tz civil time) and non-DST (modular offset). Review: do both paths produce identical results for UTC-only schedules? Does the DST path produce off-by-one-hour errors during spring-forward/fall-back transitions? Verify weather indexing is explicitly NOT dst-affected. Check feature-gate compile correctness. Write findings to the output file.")

REVIEWS+=("core-07|core|checkpoint-round-trip-completeness|Checkpoint round-trip: missing state fields|vendors/EnergyPlus/src/EnergyPlus/api/state.cc|docs/architecture.md|crates/hares-core/src/checkpoint.rs crates/hares-core/src/dwelling/mod.rs|DwellingCheckpoint saves 10 state fields but is missing: PortSlots (port accumulator — restarting mid-simulation has zeroed accumulators), zone temperatures in latest_env.zones, humidity solver internal state beyond humidity ratios, custom domain solver state, billing accumulator state. Review each missing field for correctness impact on restart. Write findings to the output file.")

REVIEWS+=("core-08|core|scheduler-empty-placeholder|scheduler.rs is an empty placeholder module|vendors/EnergyPlus/src/EnergyPlus/SimulationManager.cc|docs/architecture.md|crates/hares-core/src/scheduler.rs crates/hares-core/src/lib.rs|scheduler.rs contains only the doc comment '//! Event scheduler for time-triggered actions.' with zero implementation. The module is declared pub mod scheduler; in lib.rs. Review: what should this module contain? BMS Scheduled mode and future time-triggered events need this subsystem. Write findings to the output file.")

REVIEWS+=("core-09|core|synthetic-weather-bestest|Synthetic weather for BESTEST: ground/sky physics verification|vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc|docs/validation.md docs/bestest-900ff-root-cause-analysis.md|crates/hares-core/src/dwelling/synthetic.rs crates/hares-physics/src/solar.rs|build_synthetic_weather() was recently updated (B3 fix series) for physically consistent sky temperature but has: 8760 constant arrays, solar hardcoded to 0.0, ground_temp_c using zero-amplitude Kusuda-Achenbach. Review: is solar=0 correct for BESTEST winter cases but wrong for summer? What happens when simulation duration exceeds 8760 rows? Write findings to the output file.")

REVIEWS+=("core-10|core|occupancy-gains-zone-routing|Occupancy gains: all routed to indoor_zone only — multi-zone limitation|vendors/OCHRE/ochre/Models/Envelope.py|docs/architecture.md|crates/hares-core/src/environment.rs crates/hares-core/src/dwelling/mod.rs|apply_occupancy_gains() routes ALL occupant heat gains (sensible convective, radiative, latent) exclusively to indoor_zone_id. There is only one occupancy schedule for the whole building. Review: is this an intentional simplification? Should multi-zone occupancy routing be documented as a known limitation? Compare against OCHRE behavior. Write findings to the output file.")

REVIEWS+=("core-11|core|autosizing-cold-path-fallbacks|Autosizing cold path: design temperature fallbacks and oversizing factors|vendors/OCHRE/ochre/Dwelling.py vendors/EnergyPlus/src/EnergyPlus/Autosizing/|docs/architecture.md|crates/hares-core/src/dwelling/autosize.rs|autosize_equipment_capacities() mutates specs before equipment creation. Design temps from EPW Extremes header → ASHRAE 152 station lookup → hardcoded defaults (-10°C/35°C). Review: are oversizing factors (1.4x heating, 1.15x cooling per ACCA Manual S-2017) correctly applied? Are fallback defaults appropriate? Do they produce positive, physically plausible capacity values? Write findings to the output file.")

REVIEWS+=("core-12|core|bms-actor-modes|BMS actor mode completeness and transitions|vendors/OCHRE/ochre/Dwelling.py|docs/architecture.md docs/equipment/control-system.md|crates/hares-core/src/actors/bms.rs|BMS actor supports multiple modes (Scheduled, SelfConsumption, PeakShaving, etc.). Review: are all advertised BMS modes implemented? Are mode transitions smooth (no step-change artifacts)? Check interaction with BmsMode config validation. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  ACTORS / AGENTS   (agents-01 … agents-07)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("agents-01|agents|actor-trait-registry-dispatch|Actor trait definition, registry, and dispatch ordering correctness|vendors/OCHRE/ochre/Dwelling.py|docs/architecture.md docs/equipment/control-system.md|crates/hares-core/src/actor.rs crates/hares-core/src/actor_registry.rs crates/hares-core/src/engine.rs|Review the Actor trait: update() signature, access to dwelling state, ordering guarantees. Check actor_registry.rs: are actors registered in the correct execution order? Does the registry enforce that solver feedback actor runs before thermostat before BMS? Verify no actor can observe stale state from another actor in the same timestep. Write findings to the output file.")

REVIEWS+=("agents-02|agents|occupant-actor|Occupant actor: behavior model, stochastic schedules, and gain computation|vendors/OCHRE/ochre/utils/schedule.py vendors/OCHRE/ochre/Models/Envelope.py|docs/architecture.md|crates/hares-core/src/actors/occupant.rs|Occupant actor generates stochastic occupancy, lighting, and appliance schedules. Review: is the stochastic model correct? Does the random number generator use the dwelling RNG consistently? Are per-occupant sensible/latent/fractional gains correctly computed? Check that occupancy count 0 does not produce negative gains. Verify occupancy schedule respects workday/weekend patterns. Write findings to the output file.")

REVIEWS+=("agents-03|agents|ideal-thermostat-actor|Ideal thermostat actor: setpoint determination and interaction with equipment FSM|vendors/OCHRE/ochre/Equipment/HVAC.py|docs/architecture.md docs/equipment/control-system.md|crates/hares-core/src/actors/ideal_thermostat.rs crates/hares-equipment/src/hvac/thermostat.rs|The ideal thermostat actor determines heating/cooling setpoints from schedules and external signals, then feeds them to equipment thermostats. Review: does the actor correctly handle setpoint priority (override > schedule > static)? What happens when multiple setpoint sources conflict? Does the deadband transition cleanly between heat and cool? Check no_spacing_heating/cooling sentinel values. Write findings to the output file.")

REVIEWS+=("agents-04|agents|solver-feedback-actor|Solver feedback actor: ideal capacity dispatch bridge between solvers and equipment|vendors/OCHRE/ochre/Dwelling.py|docs/architecture.md docs/equipment/control-system.md|crates/hares-core/src/actors/solver_feedback.rs crates/hares-envelope/src/thermal_solver/stepping.rs|The solver feedback actor is the critical bridge: it collects ideal target setpoints from thermal equipment, runs the thermal solver's solve_ideal_capacity_for_target(), and dispatches IdealCapacity signals back. Review: is the collect-and-solve ordering correct? Does the actor correctly handle multiple zones with different setpoints? What happens when no equipment provides an ideal target? Verify the IdealCapacity → equipment dispatch path does not loop. Write findings to the output file.")

REVIEWS+=("agents-05|agents|dr-compliance-actor|Demand response compliance actor: DR event enforcement and constraints|vendors/OCHRE/ochre/Dwelling.py|docs/architecture.md docs/equipment/control-system.md|crates/hares-core/src/actors/dr_compliance.rs|Review the DR compliance actor: how does it enforce power limits? Does it correctly dispatch PowerLimit or LoadFraction signals? What happens when multiple DR events overlap? How does it interact with BMS mode transitions? Check that DR compliance does not violate equipment operational constraints. Write findings to the output file.")

REVIEWS+=("agents-06|agents|ev-driver-composer|EV driver charging strategy composer: policy composition logic|vendors/OCHRE/ochre/Equipment/EV.py|docs/equipment/ev.md|crates/hares-core/src/actors/ev_driver/composer.rs crates/hares-core/src/actors/ev_driver/*.rs|The EV driver composer assembles charging policy from multiple sub-strategies (departure, efficiency, preference, price, SOC gate, SOC target, solar, time window, V2G, V2H). Review: how are conflicting sub-strategies resolved? Is the priority ordering correct? Does the composer correctly handle the case where all sub-strategies return None? Write findings to the output file.")

REVIEWS+=("agents-07|agents|ev-driver-sub-strategies|EV driver sub-strategies: individual policy correctness|vendors/OCHRE/ochre/Equipment/EV.py|docs/equipment/ev.md|crates/hares-core/src/actors/ev_driver/departure.rs crates/hares-core/src/actors/ev_driver/soc_target.rs crates/hares-core/src/actors/ev_driver/time_window.rs crates/hares-core/src/actors/ev_driver/price.rs crates/hares-core/src/actors/ev_driver/solar.rs crates/hares-core/src/actors/ev_driver/soc_gate.rs crates/hares-core/src/actors/ev_driver/efficiency.rs crates/hares-core/src/actors/ev_driver/preference.rs|Review each EV driver sub-strategy for correctness. Departure: log-normal distribution parameters. SOC target: ready-by computation. Time window: wrap-around handling. Solar: forecast vs actual. Price: TOU period detection. SOC gate: hysteresis. Efficiency: driving efficiency model. Preference: user preference weighting. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  CORE ENGINE - ADDITIONAL   (core-13 … core-18)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("core-13|core|engine-main-loop-flow|Engine main simulation loop: complete phase sequencing audit with diagram|vendors/OCHRE/ochre/Simulator.py vendors/OCHRE/ochre/Dwelling.py vendors/EnergyPlus/src/EnergyPlus/SimulationManager.cc|docs/architecture.md|crates/hares-core/src/engine.rs crates/hares-core/src/dwelling/mod.rs|Trace the full engine::run() loop from initialization through shutdown. Map all phases: begin_step, pre-solver dispatch, equipment update_control (first), solver feedback collection and solve, actor dispatch, post-actor dispatch, equipment update_control (second), equipment step (thermal then non-thermal), domain solver resolution, invariant check, telemetry, end_step. Verify no phase can be skipped or reordered without breaking correctness. Write findings to the output file.")

REVIEWS+=("core-14|core|observer-diagnostics-pipeline|Observer, observer capture, and diagnostics pipeline audit|vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc|docs/invariants-and-observability.md docs/architecture.md|crates/hares-core/src/observer.rs crates/hares-core/src/observer_capture.rs crates/hares-core/src/diagnostics.rs|Observer pattern: what state is captured per timestep? ObserverCapture: how is captured data stored and retrieved? Diagnostics: what diagnostic checks run and when? Review: is the observer capture complete (all telemetry keys have values)? Does the diagnostics pipeline detect NaN/infinity/negative-energy conditions? Compare against EnergyPlus OutputProcessor conventions. Write findings to the output file.")

REVIEWS+=("core-15|core|rng-state-management|RNG state management: determinism, seeding, checkpointing|vendors/OCHRE/ochre/|docs/architecture.md|crates/hares-core/src/rng.rs crates/hares-core/src/checkpoint.rs|The dwelling RNG is used for stochastic schedules, occupant behavior, and event-based loads. Review: is the RNG seeded deterministically from config? Does checkpoint restore RNG state to the exact same position? Are multiple parallel dwellings in a fleet guaranteed to have independent RNG streams? Check for accidental shared RNG state. Write findings to the output file.")

REVIEWS+=("core-16|core|dwelling-assembly|Dwelling assembly: from_preparsed() ordering and error propagation|vendors/OCHRE/ochre/Dwelling.py|docs/architecture.md|crates/hares-core/src/dwelling/mod.rs crates/hares-core/src/dwelling/solver_builder.rs crates/hares-core/src/dwelling/conversions.rs|Dwelling::from_preparsed() orchestrates: solver construction, equipment creation, actor registration, autosizing. Review: is the construction ordering correct (solvers before equipment, equipment before actors)? Are errors propagated correctly (no swallowed errors)? Check conversions.rs: are all data conversions correct and lossless? Verify that the solver_builder correctly wires the RC model from parsed building data. Write findings to the output file.")

REVIEWS+=("core-17|core|synthetic-dwelling-generation|Synthetic dwelling generation: building, equipment, weather synthesis|vendors/OCHRE/ochre/|docs/validation.md|crates/hares-core/src/dwelling/synthetic.rs|Synthetic dwelling generation creates a complete dwelling from scratch for testing. Review: are all required building parameters synthesized? Are equipment specs physically plausible? Does the synthetic RC model produce a stable state-space system? Verify the synthetic weather function produces self-consistent atmospheric data. Write findings to the output file.")

REVIEWS+=("core-18|core|environment-state-management|Environment state management: weather polling, zone state update, schedule evaluation|vendors/OCHRE/ochre/Dwelling.py vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc|docs/architecture.md|crates/hares-core/src/environment.rs crates/hares-core/src/clock.rs|EnvironmentManager orchestrates weather polling, zone temperature/rh update from solver, schedule index computation. Review: is weather polling correct for sub-hourly timesteps? Are zone states correctly synchronized after thermal solver completion? Does the clock correctly handle simulation start/end boundaries? Check that schedule values are evaluated with correct time offset. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  CONFIG / IO INFRASTRUCTURE   (config-io-01 … config-io-06)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("config-io-01|config-io|config-parsing|Configuration parsing: TOML deserialization, validation, error messages|vendors/OCHRE/ochre/|docs/architecture.md docs/development.md|crates/hares-io/src/config.rs|HARES uses TOML for configuration (dwelling, fleet, solver options). Review: are all config fields documented? Does serde deny_unknown_fields prevent silent mistyping? Are validation error messages user-actionable (not just 'invalid value')? Check that environment-specific config (debug/release, testing) is correctly layered. Write findings to the output file.")

REVIEWS+=("config-io-02|config-io|defaults-csv-loading|Default parameter CSV loading: completeness, fallbacks, unit safety|vendors/OCHRE/ochre/|docs/architecture.md defaults/|crates/hares-io/src/defaults.rs|Default parameters for envelope, equipment, and schedules are loaded from CSV files in defaults/. Review: are all required CSV files present? Are column names stable (no silent breakage on rename)? Are default values physically plausible? What happens when a CSV file is missing or malformed? Check that temperature defaults are in Celsius not Fahrenheit. Write findings to the output file.")

REVIEWS+=("config-io-03|config-io|equipment-registry|Equipment type registry: canonical name resolution, config dispatch, error messages|vendors/OCHRE/ochre/Equipment/__init__.py|docs/architecture.md docs/equipment/|crates/hares-equipment/src/registry.rs crates/hares-equipment/src/config.rs|The EquipmentRegistry maps canonical type names (from HPXML resolver) to equipment constructors. Review: is every canonical name covered by a constructor? Are error messages clear when an unrecognized type is requested? Check the TypedConfig vs RawConfig dispatch: does the 'magic string' prohibition (Makefile check-no-magic-config) actually prevent all raw config access in built-in equipment? Write findings to the output file.")

REVIEWS+=("config-io-04|config-io|config-round-trip|Configuration serialization round-trip correctness|vendors/OCHRE/|docs/architecture.md|crates/hares-io/src/config.rs crates/hares-io/tests/config_round_trip.rs|Config objects must round-trip through TOML without data loss. Review: do all config structs implement Serialize + Deserialize? Are enum variants stable (no breaking changes on rename)? Are default values preserved through round-trip? Check test coverage for config_round_trip.rs completeness. Write findings to the output file.")

REVIEWS+=("config-io-05|config-io|resstock-dataset|ResStock building dataset integration: sampling, fixture loading, AMY pairing|vendors/OCHRE/|docs/architecture.md|crates/hares-io/src/resstock.rs crates/hares-io/src/resstock_csv.rs python/ochre_next/data/resstock.py|ResStock integration supports sampling buildings from the national dataset and pairing with AMY weather. Review: is the sampling logic correct (weighted by housing stock)? Are building-weather pairings valid (same climate zone)? Does the OEDI S3 download pipeline handle missing/corrupted files? Check test fixture coverage in tests/fixtures/resstock/. Write findings to the output file.")

REVIEWS+=("config-io-06|config-io|draw-profile-pv-sizing|Electrical draw profile construction and PV sizing from HPXML|vendors/OCHRE/ochre/Equipment/PV.py|docs/equipment/pv.md docs/equipment/loads.md|crates/hares-io/src/draw_profile.rs crates/hares-io/src/pv_sizing.rs|draw_profile.rs constructs annual electrical load profiles from HPXML appliance data. pv_sizing.rs computes PV array sizing. Review: is the draw profile construction correct for all 14+ load types? Are coincidence factors applied? Does PV sizing account for inverter clipping, system losses, and array configuration? Check cross-reference against OCHRE PV sizing. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  OUTPUT PIPELINE   (output-01 … output-03)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("output-01|output|arrow-parquet-writer|Arrow/Parquet output writer: schema stability, column types, file rotation|vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc|docs/architecture.md docs/invariants-and-observability.md|crates/hares-io/src/output/writer.rs crates/hares-io/src/output/columns.rs|The Arrow/Parquet writer produces columnar output for analysis. Review: is the schema stable across runs (same columns, same types)? Are numeric types appropriate (f64 for continuous, i32 for discrete)? Does file rotation work correctly for long simulations? Check that timestamps are stored with correct timezone. Verify Arrow RecordBatch construction does not silently drop columns. Write findings to the output file.")

REVIEWS+=("output-02|output|output-column-definitions|Output column definitions: completeness, naming conventions, unit labeling|vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc|docs/invariants-and-observability.md|crates/hares-io/src/output/columns.rs crates/hares-types/src/telemetry.rs crates/hares-types/src/telemetry_keys.rs|Review all output column definitions. Are all telemetry keys that equipment can emit represented as output columns? Do column names include unit suffixes consistently? Are there columns defined but never populated? Are there telemetry keys with no corresponding output column? Check that column ordering is deterministic. Write findings to the output file.")

REVIEWS+=("output-03|output|output-metrics-computation|Output metrics computation: aggregation, resampling, correctness|vendors/OCHRE/ochre/Analysis.py|docs/architecture.md|crates/hares-io/src/output/metrics.rs|Output metrics compute summary statistics from raw telemetry. Review: are aggregation functions correct (sum for energy, mean for temperature, max for demand)? Is sub-hourly to hourly resampling correct? Are annual totals correct for leap years? Check that metrics computation does not silently drop NaN or use stale accumulator state. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  SOLVER GAPS   (solver-01 … solver-03)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("solver-01|solver|electrical-solver-standalone|Electrical domain solver: ZIP model, voltage dependency, complex power|vendors/EnergyPlus/src/EnergyPlus/ElectricPowerServiceManager.cc|docs/architecture.md|crates/hares-envelope/src/electrical_solver.rs|The electrical solver handles voltage-dependent loads via ZIP model and computes net electrical balance. Review: is the ZIP model (z*v² + i*v + p_coeff) correctly implemented? Are complex power calculations correct (P, Q, S, PF)? Does the solver aggregate generation and load correctly with sign conventions? Check voltage range [0.9, 1.1] pu. Write findings to the output file.")

REVIEWS+=("solver-02|solver|fluid-solver-standalone|Fluid domain solver: loop resolution, temperature propagation, mass conservation|vendors/EnergyPlus/src/EnergyPlus/Plant/|docs/architecture.md|crates/hares-envelope/src/fluid_solver.rs crates/hares-types/src/fluid.rs|The fluid solver resolves hydronic loops (hot water, DHW). Review: are fluid loops resolved correctly (mass balance, temperature propagation)? Does the solver handle multiple equipment on the same loop? Are supply/return temperatures consistent? Check that no fluid loop produces unphysical temperatures. Write findings to the output file.")

REVIEWS+=("solver-03|solver|envelope-lut-precomputed|Envelope LUT precomputed RC path: correctness and performance|vendors/OCHRE/ochre/utils/envelope.py|docs/thermal-envelope-solver.md|crates/hares-io/src/envelope_lut.rs crates/hares-envelope/src/boundary_rc.rs|The envelope LUT (lookup table) provides a fast precomputed RC network path. Review: are LUT-generated RC networks equivalent to the full computation path? Are the LUT parameter ranges adequate for all building types? Is the interpolation correct for off-grid parameter values? Check that the LUT path does not bypass necessary validation steps. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  ENVELOPE: MATERIALS & RC DISCRETIZATION   (rc-mat-01 … rc-mat-03)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("rc-mat-01|envelope-rc-mat|material-layer-to-rc-nodes|HPXML construction material layers → RC network node mapping correctness|vendors/OCHRE/ochre/utils/envelope.py vendors/OCHRE/ochre/Models/RCModel.py vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc|docs/thermal-envelope-solver.md|crates/hares-io/src/hpxml/building.rs crates/hares-envelope/src/boundary_rc.rs crates/hares-envelope/src/rc_network.rs|Trace how HPXML material layers (Thickness, Conductivity, Density, SpecificHeat) map to RC network capacitances and resistances. Review: is the diurnal penetration depth criterion (Λ = sqrt(α * 86400 / (4π))) correctly applied per ISO 13786:2007? Are insulation layers correctly modeled as single-node? Is the material property conversion from IP to SI correct (conductivity, density, specific heat)? Check that window layers follow EnergyPlus 5-step calculation. Write findings to the output file.")

REVIEWS+=("rc-mat-02|envelope-rc-mat|rc-network-discretization|RC network discretization: node count, time constants, stability|vendors/OCHRE/ochre/Models/RCModel.py vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc|docs/thermal-envelope-solver.md docs/findings/|crates/hares-envelope/src/rc_network.rs crates/hares-envelope/src/state_space.rs|RC network construction from material layers produces a thermal state-space model. Review: is split_layer_count() correct for all material types (dense, insulation, air gaps)? Are time constants physically plausible (no sub-second nodes)? Is the system observable (all thermal mass affects zone air)? Check same-zone boundary inner-half-layer handling. Write findings to the output file.")

REVIEWS+=("rc-mat-03|envelope-rc-mat|window-material-model|Window material model: IAM curves, SHGC decomposition, U-factor three-resistance split|vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc vendors/EnergyPlus/src/EnergyPlus/WindowModel.cc vendors/OCHRE/ochre/utils/envelope.py|docs/thermal-envelope-solver.md docs/findings/|crates/hares-physics/src/solar.rs crates/hares-envelope/src/boundary_rc.rs|Window modeling: 6 IAM polynomial curves, SHGC decomposed into transmitted and absorbed-inward fractions per EnergyPlus Window Calculation Module Steps 1-5. Review: are the IAM curves correct for each glazing type? Is the three-resistance decomposition (r_glass, r_film_interior, r_film_exterior) physically correct? Does the Walton effective-temperature correction for exterior LWR correctly adjust the heat flux? Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  OVERALL ARCHITECTURE ASSESSMENT   (arch-01 … arch-04)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("arch-01|architecture|overall-architecture|Overall architecture assessment: crate dependency graph, data flow, coupling|vendors/OCHRE/ochre/ vendors/EnergyPlus/src/EnergyPlus/|docs/architecture.md|crates/*/src/lib.rs crates/*/Cargo.toml|Assess the overall HARES architecture. Review: is the crate dependency graph clean (no circular dependencies)? Are data flows well-defined (types → physics → envelope → equipment → io → core → fleet → python)? Is coupling appropriate (does hares-types truly have no internal dependencies)? Are there any 'god objects' or excessively large modules? Compare against OCHRE's Simulator/Dwelling/Equipment hierarchy and EnergyPlus's EnergyPlusData monolith. Write findings to the output file.")

REVIEWS+=("arch-02|architecture|core-state-structs|Core state types: EnvironmentState, WeatherState, ZoneState completeness and correctness|vendors/OCHRE/ochre/Models/Envelope.py vendors/EnergyPlus/src/EnergyPlus/Data/|docs/architecture.md|crates/hares-types/src/environment.rs crates/hares-core/src/environment.rs|EnvironmentState holds current weather, zone temperatures, humidity ratios, and schedule indices. WeatherState holds per-timestep weather derivatives. ZoneState holds per-zone thermal conditions. Review: are all required state fields present? Are units consistent? Can any field become stale (not updated for multiple timesteps)? Is the state sufficient for checkpoint/restore? Check that weather state is correctly refreshed at timestep boundaries. Write findings to the output file.")

REVIEWS+=("arch-03|architecture|control-capability-matching|Control capability matching: equipment declares capabilities, dispatcher matches signals|vendors/OCHRE/ochre/Dwelling.py|docs/architecture.md docs/equipment/control-system.md|crates/hares-control/src/capabilities.rs crates/hares-control/src/dispatch.rs crates/hares-control/src/signal.rs|Equipment declares capabilities (CoreCapabilities and ControlCapabilities bitflags). The dispatcher matches incoming signals to equipment that can accept them. Review: is the capability matching correct for every signal type? Can a signal be silently dropped because no equipment declares the capability? Are the bitflag definitions complete (no missing capabilities)? Check that HAS_SETPOINT, HAS_POWER_LIMIT, etc. are correctly declared by each equipment type. Write findings to the output file.")

REVIEWS+=("arch-04|architecture|port-slot-accumulation|Port slot accumulation: declaration vs runtime accumulation, zone indexing, category tagging|vendors/OCHRE/ochre/Dwelling.py|docs/architecture.md|crates/hares-types/src/ports.rs crates/hares-core/src/dwelling/mod.rs|PortSlots are declared at init time and accumulated at runtime. Review: does the declarative port wiring correctly prevent accumulation to undeclared slots? Are ThermalCategory tags (HvacHeating, HvacCooling, DuctLoss, JacketLoss, InternalGain) correctly set by each equipment? Does the accumulator correctly sum contributions from multiple equipment to the same zone? Check that zone_id indexing is consistent between ports and environment zones. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  PYTHON COMPANION CODE   (py-companion-01 … py-companion-04)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("py-companion-01|py-companion|python-adapters|Python adapter models: PyBaMM battery, SAM battery, SAM PV correctness|vendors/OCHRE/ochre/Equipment/Battery.py vendors/OCHRE/ochre/Equipment/PV.py|docs/python.md|python/ochre_next/adapters/pybamm_battery.py python/ochre_next/adapters/sam_battery.py python/ochre_next/adapters/sam_pv.py|Python adapters provide alternative high-fidelity models for battery and PV. Review: are the PyBaMM and SAM models correctly integrated? Are parameter conversions correct (Python ↔ Rust)? Do the adapters respect the Equipment trait contract? Check that adapter-generated LUT tables are correctly formatted and injected. Write findings to the output file.")

REVIEWS+=("py-companion-02|py-companion|helics-federate|HELICS federate: dwelling and fleet federates, time synchronization, signal mapping|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md|python/ochre_next/helics/dwelling.py python/ochre_next/helics/fleet.py python/ochre_next/helics/broker.py python/ochre_next/helics/runner.py python/ochre_next/helics/_types.py|HELICS co-simulation integration: dwelling federate step logic, fleet federate aggregation, broker management, time synchronization. Review: is time synchronization correct (no skipped timesteps)? Are HELICS signal types correctly mapped to HARES control signals? Does the federate handle HELICS errors gracefully? Check that the broker lifecycle (start → run → shutdown) is correct. Write findings to the output file.")

REVIEWS+=("py-companion-03|py-companion|resstock-data-pipeline|ResStock data pipeline: OEDI S3 download, building sampling, weather pairing|vendors/OCHRE/|docs/weather-pipeline.md|python/ochre_next/data/resstock.py python/ochre_next/data/weather.py|ResStock data module downloads building and weather data from OEDI S3. Review: is the data download robust (retries, checksums)? Are building-weather pairings correct (matching climate zones)? Does the sampling respect housing stock weights? Check that downloaded data is cached correctly. Write findings to the output file.")

REVIEWS+=("py-companion-04|py-companion|rl-gym-and-vec|RL Gymnasium environment: obs/action spaces, reward, vec env correctness|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md|crates/hares-python/src/py_gym.rs python/ochre_next/rl/gym_env.py python/ochre_next/rl/vec_env.py|Review the Gymnasium RL environment in detail. Check: observation space definition (all telemetry keys? correct bounds?), action space (correct dimension and range per equipment type?), reward function (correct sign and scaling?), done/truncation conditions. For vec_env.py: does SubprocVecEnv correctly handle parallel dwelling instances? Are observations correctly stacked? Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  TEST ORACLES & INTEGRATION   (test-01 … test-04)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("test-01|tests|test-oracle-infrastructure|Test oracle infrastructure: BESTEST, conditioned/freefloat/envelope oracles, parity corpus|vendors/OCHRE/test/ vendors/EnergyPlus/testfiles/|docs/validation.md|crates/hares-core/tests/ tests/bestest/ tests/parity/|Review the test oracle infrastructure. Are all oracle types (BESTEST, conditioned, freefloat, envelope, structural envelope) correctly validated? Is the parity corpus (10 climate zones) representative? Are tolerance bands physically justified? Check that oracle generation scripts (Python tests) produce reproducible reference data. Write findings to the output file.")

REVIEWS+=("test-02|tests|integration-test-coverage|Integration test coverage: full simulation scenarios, ResStock smoke, multi-instance, determinism|vendors/OCHRE/test/test_dwelling/|docs/validation.md docs/development.md|tests/regression/ tests/resstock_smoke.rs crates/hares-core/tests/integration.rs crates/hares-core/tests/|Review integration test coverage. Do integration tests cover: full HPXML→output pipeline, multi-day simulation, weather seasonal variation, equipment interaction, fleet multi-dwelling, checkpoint restart, determinism? Check that all equipment types appear in at least one integration test. Write findings to the output file.")

REVIEWS+=("test-03|tests|parity-test-tolerance|Parity test tolerance definitions: are they physically justified or hiding defects?|vendors/OCHRE/ vendors/EnergyPlus/|docs/validation.md tests/parity/tolerance.rs|Parity tests compare HARES output against OCHRE reference. Review the tolerance definitions in tolerance.rs: are the numeric values physically justified? Are any tolerances wide enough to mask real discrepancies? Is there a process for tightening tolerances as defects are fixed? Check that parity tests do not use different weather/schedule inputs than the reference. Write findings to the output file.")

REVIEWS+=("test-04|tests|core-output-contract|CoreOutput contract validation: invariants, bitflag checks, test coverage|vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc|docs/invariants-and-observability.md|crates/hares-types/tests/core_output_invariants.rs crates/hares-types/src/telemetry.rs|CoreOutput validation uses bitflag arithmetic to check that required fields are populated. Review: is the contract validation complete (all required fields checked)? Are the bitflag checks correct (no off-by-one or missing flags)? Does the validation catch realistic data corruption? Check test coverage for core_output_invariants.rs. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  PYTHON BINDINGS - DEDICATED   (py-binding-01 … py-binding-08)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("py-binding-01|py-binding|py-config-binding|Python config binding: completeness, type conversion safety, error handling|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md|crates/hares-python/src/py_config.rs crates/hares-python/src/conversions.rs|py_config.rs exposes Rust config types to Python. Review: are all config fields accessible from Python? Are type conversions safe (no silent truncation of f64→i32)? Are Python exceptions raised for invalid config values? Check that TOML config round-trips correctly through Python. Write findings to the output file.")

REVIEWS+=("py-binding-02|py-binding|py-dwelling-binding|Python dwelling binding: lifecycle, step execution, state access|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md|crates/hares-python/src/py_dwelling.rs crates/hares-python/src/py_enums.rs|py_dwelling.rs exposes Dwelling::run(), step(), and state access to Python. Review: is the dwelling lifecycle correct (new → run → step → shutdown)? Are Python exceptions raised for simulation errors? Is PyO3 GIL handling correct for long-running simulations? Check that py_enums.rs covers all Rust enums needed by Python. Write findings to the output file.")

REVIEWS+=("py-binding-03|py-binding|py-equipment-binding|Python equipment binding: descriptor, mutation, LUT injection|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md|crates/hares-python/src/py_equipment.rs crates/hares-python/src/py_lut_injection.rs|py_equipment.rs exposes equipment descriptors and mutation to Python for custom equipment. Review: can Python code create and register custom equipment? Is LUT injection robust (correct dimensions, value ranges)? Are equipment mutations applied before or after init? Write findings to the output file.")

REVIEWS+=("py-binding-04|py-binding|py-control-binding|Python control binding: signal construction, dispatch, compat|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md docs/equipment/control-system.md|crates/hares-python/src/py_control.rs|py_control.rs exposes control signal construction and dispatch to Python. Review: can Python code construct all control signal types? Is the dispatch API consistent with Rust-side ControlDispatcher? Are priority tiers correctly mapped? Check OCHRE compat signal routing from Python. Write findings to the output file.")

REVIEWS+=("py-binding-05|py-binding|py-fleet-binding|Python fleet binding: parallel execution, progress, aggregation|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md|crates/hares-python/src/py_fleet.rs crates/hares-fleet/src/fleet.rs|py_fleet.rs exposes parallel fleet simulation to Python. Review: is rayon parallel execution safe with Python GIL? Are progress callbacks correctly threaded? Is fleet aggregation result correctly returned to Python as Arrow/Polars? Check that fleet shutdown cleans up all dwelling resources. Write findings to the output file.")

REVIEWS+=("py-binding-06|py-binding|py-tariff-binding|Python tariff binding: URDB parsing, evaluation, billing|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md|crates/hares-python/src/py_tariff.rs crates/hares-tariff/src/|py_tariff.rs exposes tariff parsing and evaluation to Python. Review: can Python code load URDB JSON tariffs? Is the billing calculation correct? Are demand charges correctly computed? Check that tariff evaluation is idempotent across calls. Write findings to the output file.")

REVIEWS+=("py-binding-07|py-binding|py-weather-binding|Python weather binding: EPW/PSM3/TMY3 parsing from Python|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md docs/weather-pipeline.md|crates/hares-python/src/py_weather.rs|py_weather.rs exposes weather parsing to Python. Review: can Python code load EPW, PSM3, TMY3 files? Is weather resampling accessible? Are location metadata correctly exposed? Check that weather data is correctly passed to dwelling config. Write findings to the output file.")

REVIEWS+=("py-binding-08|py-binding|py-actor-binding|Python actor binding: custom actor implementation from Python|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md|crates/hares-python/src/py_actor.rs crates/hares-core/src/actor.rs|py_actor.rs allows Python code to implement the Actor trait for custom control logic. Review: is the Python→Rust callback safe? Does the actor have access to all necessary dwelling state? Are Python actor execution times accounted for in the timestep budget? Check that Python actor errors propagate correctly. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  EQUIPMENT DATA / UTILITIES   (equip-util-01 … equip-util-03)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("equip-util-01|equipment-util|nd-interpolation|N-dimensional interpolation correctness and boundary handling|vendors/OCHRE/ochre/Equipment/ vendors/EnergyPlus/src/EnergyPlus/|docs/equipment/|crates/hares-equipment/src/ndinterp.rs|N-dimensional interpolation is used for SAM LUT PV and PyBaMM EV charging curves. Review: is multilinear interpolation correct for 4-D and 6-D grids? Are extrapolation bounds handled correctly (clamp vs NaN)? Is the grid search efficient? Check that degenerate grids (single point per dimension) produce correct results. Write findings to the output file.")

REVIEWS+=("equip-util-02|equipment-util|schedule-helpers|Schedule evaluation helpers: duty cycle, stochastic, interpolation|vendors/OCHRE/ochre/utils/schedule.py|docs/schedule-sources.md|crates/hares-equipment/src/schedule_helpers.rs crates/hares-io/src/schedule.rs|Schedule helpers bridge between HPXML schedule metadata and runtime schedule evaluation. Review: are duty cycle fractions correctly converted to on/off patterns? Is stochastic schedule generation consistent with expected energy? Are interpolation methods correct for sub-hourly timesteps? Write findings to the output file.")

REVIEWS+=("equip-util-03|equipment-util|equipment-macros|Equipment macros: code generation correctness and error messages|vendors/OCHRE/|docs/architecture.md|crates/hares-equipment/src/macros.rs|Internal macros in hares-equipment generate boilerplate for equipment structs. Review: do macros produce correct trait implementations? Are macro error messages helpful when misused? Is generated code free of silent default-forgetting? Check that all equipment types using macros have consistent behavior. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  TYPES, PHYSICS, CONTROL, TARIFF   (types-physics-01 … types-physics-12)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("types-physics-01|types-physics|latent-heat-reference-state|Latent heat reference-state inconsistency (2450 vs 2501 kJ/kg)|vendors/OCHRE/ochre/utils/psychrolib_jit.py|docs/python.md|crates/hares-physics/src/psychrometrics.rs crates/hares-physics/src/constants.rs crates/hares-envelope/src/humidity_solver.rs|Two latent heat constants: LATENT_HEAT_VAPORISATION_0C (2501 kJ/kg for enthalpy) and LATENT_HEAT_VAPORISATION_J_KG (2450 kJ/kg at ~20°C for moisture balance). The constants.rs docblock says 2450 must NOT be used for moisture mass balance, yet it's used in latent-conversion code paths. Audit every h_fg usage against the humidity solver convention. Write findings to the output file.")

REVIEWS+=("types-physics-02|types-physics|perez-epsilon-formula|Perez epsilon formula: code duplication and verification|vendors/EnergyPlus/src/EnergyPlus/SolarShading.cc|docs/weather-pipeline.md docs/findings/|crates/hares-physics/src/solar.rs|epsilon computation in perez_sky_diffuse() and perez_tilted_irradiance() is duplicated. Verify both are algebraically equivalent to the canonical Perez 1990 Eq. 1. Check against pvlib-python implementation which uses slightly different denominator convention for standalone perez_sky_diffuse. Write findings to the output file.")

REVIEWS+=("types-physics-03|types-physics|window-u-factor-decomposition|Window U-factor decomposition: three-resistance model divergence from OCHRE|vendors/OCHRE/ochre/utils/envelope.py vendors/EnergyPlus/src/EnergyPlus/WindowManager.cc|docs/findings/|crates/hares-physics/src/solar.rs crates/hares-envelope/src/boundary_rc.rs|HARES separates window into r_glass, r_film_interior, r_film_exterior vs OCHRE absorbing Ro,w into r_window. Docblock claims HARES diverges 'for correctness.' Review: is the three-resistance decomposition physically correct? Does the exterior film use standard winter correlation regardless of actual wind speed? Map the 3 resistances to the single res_material_m2_k_w parameter. Write findings to the output file.")

REVIEWS+=("types-physics-04|types-physics|duct-leakage-infiltration-duplication|Duct-leakage infiltration formula duplicates ASHRAE 152 logic across crates|vendors/EnergyPlus/src/EnergyPlus/HVACManager.cc|docs/equipment/hvac.md|crates/hares-physics/src/infiltration.rs crates/hares-physics/src/ashrae152.rs|duct_leakage_infiltration_m3_s() in infiltration.rs and calculate_dse() in ashrae152.rs independently implement ASHRAE 152 §9.3 superposition (1.5/0.67 factors with infil_fan_off). The scaling factor in infiltration.rs adjusts the caller's base rate rather than replacing it entirely, creating potential inconsistency. Review: refactor to a single implementation. Write findings to the output file.")

REVIEWS+=("types-physics-05|types-physics|uom-typed-wrapper-coverage|Incomplete uom typed wrapper coverage across physics modules|vendors/EnergyPlus/src/EnergyPlus/|docs/rust-primer.md|crates/hares-physics/src/units.rs crates/hares-physics/src/lib.rs|Only air_properties, infiltration, and psychrometrics have typed uom wrappers. ground.rs, constants.rs, water_mains.rs, film_coefficients.rs have no uom integration. Wrappers that exist return raw f64 for compound types. Review: what is the policy for uom adoption? Should the crate converge on typed wrappers or raw f64 with naming convention? Write findings to the output file.")

REVIEWS+=("types-physics-06|types-physics|ochre-compat-mapping-coarse|OCHRE compatibility mapping: ambiguous string-based routing|vendors/OCHRE/ochre/Dwelling.py|docs/architecture.md|crates/hares-control/src/compat.rs crates/hares-control/src/signal.rs|OCHRE compat maps only 7 key types. KEY_SETPOINT_TEMPERATURE_C routes to heating OR cooling based on name containing 'cool' (string heuristic). KEY_SELF_CONSUMPTION_MODE always sets solar_only_charging: false. Review: is the string heuristic robust? Should there be a way to route to both heating and cooling simultaneously? List all unsupported OCHRE control keys. Write findings to the output file.")

REVIEWS+=("types-physics-07|types-physics|dispatch-target-conflict-detection|DispatchTarget conflict detection: by-name vs by-end-use ambiguity|vendors/OCHRE/ochre/Dwelling.py|docs/architecture.md docs/equipment/control-system.md|crates/hares-control/src/dispatch.rs crates/hares-core/src/dwelling/mod.rs|conflicts_with() never considers a signal routed ByName and ByEndUse as conflicting, even though they may target the same physical equipment. The actual delivery mechanism is in hares-core. Review: can a ByEndUse signal silently overwrite a ByName signal for the same equipment? Write findings to the output file.")

REVIEWS+=("types-physics-08|types-physics|demand-averaging-window-collapse|Tariff demand averaging window silently collapses when window < interval|vendors/EnergyPlus/src/EnergyPlus/EconomicTariff.cc|docs/architecture.md|crates/hares-tariff/src/evaluator.rs|When demand window (minutes) < simulation interval, it silently collapses to 1-sample instantaneous peak. The return type gives no indication this happened. Review: should this emit a warning? Should the demand peak be marked as 'instantaneous' rather than 'rolling average'? Write findings to the output file.")

REVIEWS+=("types-physics-09|types-physics|urdb-summer-month-heuristic|URDB summer-month heuristic: fragile for southern hemisphere and tropics|vendors/EnergyPlus/src/EnergyPlus/EconomicTariff.cc|docs/architecture.md|crates/hares-tariff/src/urdb.rs|detect_summer_months() compares each month to December; if >6 months differ, it inverts (southern hemisphere). Flat-rate tariffs with zero seasonal variation produce None. Tariffs where 7 months differ from December for non-seasonal reasons trigger inversion. No latitude input to anchor hemisphere decision. Review: add latitude parameter or explicit season configuration. Write findings to the output file.")

REVIEWS+=("types-physics-10|types-physics|humidity-port-latent-consistency|No cross-validation between humidity port contributions and latent thermal contributions|vendors/EnergyPlus/src/EnergyPlus/HumidityManager.cc|docs/architecture.md|crates/hares-types/src/ports.rs crates/hares-core/src/dwelling/mod.rs|PortContribution::Humidity (moisture mass flow) and PortContribution::Thermal with latent_gain_w can be emitted simultaneously by equipment but are treated as independent channels. If one uses h_fg=2501 and another uses different latent heat, moisture balances diverge. Review: add a debug_assert or consistency check. Write findings to the output file.")

REVIEWS+=("types-physics-11|types-physics|biquadratic-default-bounds-too-wide|Biquadratic curve default bounds too wide — documented bug|vendors/EnergyPlus/src/EnergyPlus/CurveManager.cc|docs/equipment/hvac.md|crates/hares-physics/src/biquadratic.rs crates/hares-equipment/src/hvac/hvac_core.rs|Tests explicitly document DEFAULT_BIQUADRATIC_X2_BOUNDS of (-100,100)°C as a bug (test: default_x2_lower_bound_clamps_at_neg50_not_neg100). The bounds allow extrapolation far outside AHRI rating range. Review: fix the default bounds to physically meaningful limits. Check EnergyPlus Curve:Biquadratic bounds conventions. Write findings to the output file.")

REVIEWS+=("types-physics-12|types-physics|missing-mechanical-ventilation-model|No HRV/ERV/balanced mechanical ventilation physics model|vendors/EnergyPlus/src/EnergyPlus/HeatRecovery.cc|docs/architecture.md|crates/hares-physics/src/lib.rs crates/hares-types/src/telemetry_keys.rs|telemetry_keys.rs defines SENSIBLE_RECOVERY_W and LATENT_RECOVERY_W keys, but no physics to populate them exists in hares-physics. The ventilation equipment model exists in hares-equipment but relies on physics not in this crate. Review: where should balanced ventilation physics live? What ASHRAE 152.2 equations are missing? Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  FLEET & PYTHON BINDINGS   (fleet-python-01 … fleet-python-08)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("fleet-python-01|fleet-python|aggregation-empty-batch-silent|Fleet aggregation: silent empty-batch on column mismatch and bucket intersection drop|vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc|docs/architecture.md|crates/hares-fleet/src/aggregation.rs|build_aggregate_batch() silently returns empty RecordBatch when column sets differ between dwellings. Bucket-intersection logic silently drops timesteps where any dwelling is missing a bucket. Review: should these be warnings? What downstream consumers expect non-empty output? Add diagnostics for data loss. Write findings to the output file.")

REVIEWS+=("fleet-python-02|fleet-python|aggregation-column-suffix-matching|Fleet aggregation: column classification by suffix string matching only|vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc|docs/architecture.md|crates/hares-fleet/src/aggregation.rs|ColumnAggregation::for_column() uses ends_with('(kWh)'), ends_with('(kW)'), ends_with('(C)'), ends_with('°C'). Any deviation (whitespace, formatting) silently falls to Mean instead of Sum. Review: is suffix matching robust enough? Should column metadata (from telemetry_keys) drive aggregation instead of string parsing? Write findings to the output file.")

REVIEWS+=("fleet-python-03|fleet-python|aggregation-energy-column-weighting|Fleet aggregation: energy column weighting with non-unit sample weights|vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc|docs/architecture.md|crates/hares-fleet/src/aggregation.rs|FleetAggregation::for_column() maps energy columns to WeightedSum — correct only if sample weights are all 1.0. ColumnAggregation classifies (kWh) as Sum but FleetAggregation classifies everything non-temperature as WeightedSum. Review: is this a double-weighting bug when dwellings have unequal sample weights? Write findings to the output file.")

REVIEWS+=("fleet-python-04|fleet-python|send-dwelling-ptr-safety|Python binding: unsafe SendDwellingPtr safety audit|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md|crates/hares-python/src/py_gym.rs crates/hares-python/src/conversions.rs|SendDwellingPtr wraps *const PyDwelling with unsafe impl Send+Sync. GIL release pattern means PyRef borrows must stay alive. No static/dynamic check ensures Py<PyDwelling> handles outlive the parallel section. Action mapping is not yet implemented (returns PyNotImplementedError). Review: is the unsafe Send+Sync actually sound? What prevents use-after-free when dwelling handles drop early? Write findings to the output file.")

REVIEWS+=("fleet-python-05|fleet-python|python-dead-code-cleanup|Python binding: dead code with #[allow(dead_code)]|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md|crates/hares-python/src/conversions.rs|obs_to_numpy() and record_batch_arc_to_polars_df() are #[allow(dead_code)], never called. obs_to_numpy is exactly what batch_step_py would use when action mapping is implemented. Review: remove, or wire into the API surface, or document as forward-declaration. Write findings to the output file.")

REVIEWS+=("fleet-python-06|fleet-python|helics-integration-test-gaps|HELICS co-simulation integration test coverage gaps|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md|crates/hares-python/src/|python/ochre_next/helics/ tests/python/test_helics_*.py|Review HELICS integration test coverage. Check: federate setup/teardown, time synchronization, signal subscription, error handling on federate failure, multi-year co-simulation. Compare against EnergyPlus BCVTB co-simulation patterns. Write findings to the output file.")

REVIEWS+=("fleet-python-07|fleet-python|rl-gym-env-completeness|RL Gymnasium environment completeness and observation space correctness|vendors/EnergyPlus/src/EnergyPlus/api/|docs/python.md|crates/hares-python/src/py_gym.rs python/ochre_next/rl/gym_env.rs python/ochre_next/rl/vec_env.rs|RL environment: review observation space definition, action space definition, reward function computation, and termination/truncation conditions. Check: does the vectorized env (vec_env.rs) correctly batch observations? Are reset() and step() semantics Gymnasium-compliant? Write findings to the output file.")

REVIEWS+=("fleet-python-08|fleet-python|fleet-progress-empty-stub|Fleet progress.rs is an empty stub|vendors/EnergyPlus/src/EnergyPlus/|docs/architecture.md|crates/hares-fleet/src/progress.rs crates/hares-fleet/src/fleet.rs|progress.rs contains only a doc comment with zero types or functions. Progress reporting is implemented inline in fleet.rs via ProgressCallback type alias. Review: either populate progress.rs with the progress types or remove the module declaration. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  INFRASTRUCTURE: TESTS, CI, DOCS, BENCHMARKS   (infra-01 … infra-08)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("infra-01|infrastructure|dead-module-aggregation-check|Dead module reference aggregation_check.rs causes compile failure|vendors/EnergyPlus/|docs/development.md|tests/regression/mod.rs|tests/regression/mod.rs line 16 declares mod aggregation_check; and line 77 calls aggregation_check::run_aggregation_check(), but no aggregation_check.rs file exists. Causes compilation failure for cargo test --test regression. Review: remove the dead reference or implement the missing module. Write findings to the output file.")

REVIEWS+=("infra-02|infrastructure|bestest-should-panic-xfail|BESTEST #[should_panic] xfail pattern has soundness hazard|vendors/EnergyPlus/testfiles/|docs/validation.md docs/bestest-900ff-root-cause-analysis.md tests/bestest/mod.rs|Case 900FF uses #[should_panic(expected = 'metric=min_zone_temp_c')]. If the test panics for a different reason, Rust test harness still reports PASS. All other BESTEST tests are #[ignore]. Review: replace should_panic with explicit test that checks failure condition and reports as known issue. Write findings to the output file.")

REVIEWS+=("infra-03|infrastructure|empty-fixture-directories|Empty golden/ and schedules/ test fixture directories|vendors/OCHRE/test/ vendors/EnergyPlus/testfiles/|docs/development.md|tests/fixtures/golden/ tests/fixtures/schedules/|golden/.gitkeep and schedules/.gitkeep are empty. golden/ suggests golden-file tests (none exist). schedules/ suggests schedule fixture data (none exist). Review: should these be populated or removed? If populated, what format and content is needed? Write findings to the output file.")

REVIEWS+=("infra-04|infrastructure|ci-no-test-execution|CI runs only fmt + clippy — no test execution, Python tests, or benchmarks|vendors/OCHRE/.github/ vendors/EnergyPlus/.github/|docs/development.md|.github/workflows/ci.yml|ci.yml runs cargo fmt --check and cargo clippy only. No cargo test, no cargo nextest run, no Python tests, no benchmark regression detection. Python tests depend on fixture files and Rust extension builds that CI never exercises. Review: propose a CI matrix covering tests, Python tests, and benchmark regression. Write findings to the output file.")

REVIEWS+=("infra-05|infrastructure|benchmark-coverage-gaps|Benchmark coverage gaps: no aggregation, co-simulation, or RL episode benchmarks|vendors/EnergyPlus/performance_tests/|docs/development.md docs/profile-debug-hangs-and-slowness.md|benches/fleet.rs benches/rl_step.rs benches/single_building.rs|Missing: aggregation benchmark (RecordBatch manipulation + resampling + weighted combining), HELICS co-simulation benchmark, full RL episode benchmark (reset + N steps). All benchmarks use synthetic data, not real ResStock buildings. Review: identify which missing benchmarks are highest priority for production performance prediction. Write findings to the output file.")

REVIEWS+=("infra-06|infrastructure|docs-handoff-items-remaining|HANDOFF items remaining as pre-release technical debt|vendors/OCHRE/ vendors/EnergyPlus/|docs/HANDOFF.md docs/development.md|docs/HANDOFF.md|HANDOFF.md lists 7 items remaining at commit ba133ca: BESTEST 900FF root cause, 5 physics constants matching ASHRAE/EnergyPlus not OCHRE, Python test suite not run at handoff. Review: check current status of each item. Which have been resolved? Which remain? Create a tracking table linking HANDOFF items to review areas in this script. Write findings to the output file.")

REVIEWS+=("infra-07|infrastructure|docs-citation-verification|Documentation citation verification: physics references, model sources, validation data|vendors/EnergyPlus/doc/engineering-reference/ vendors/OCHRE/docs/|docs/ docs/findings/|docs/|Review all docs/*.md and docs/findings/*.md for: correct citation of ASHRAE HOF, EnergyPlus Engineering Reference, OCHRE publications, and peer-reviewed sources. Check that model equation references match the implemented code. Flag any undocumented physics assumptions. Write findings to the output file.")

REVIEWS+=("infra-08|infrastructure|error-handling-review|Error handling audit: panics, unwraps, expects across the codebase|vendors/EnergyPlus/src/EnergyPlus/|docs/rust-primer.md|All crates|Review all .unwrap(), .expect(), unreachable!(), and panic!() calls across the full workspace. Categorize by severity: (1) should be proper error types, (2) guarded by preconditions, (3) acceptable. Check that no panic path can be triggered by malformed HPXML or EPW input. Write findings to the output file.")

# ═══════════════════════════════════════════════════════════════
#  CROSS-CUTTING WIRING & ATTRIBUTE CONSISTENCY   (wiring-01 … wiring-08)
# ═══════════════════════════════════════════════════════════════
REVIEWS+=("wiring-01|wiring|port-naming-consistency|Port name and declaration consistency across equipment, solver, and actors|vendors/OCHRE/ochre/Equipment/ vendors/EnergyPlus/src/EnergyPlus/|docs/architecture.md|crates/hares-types/src/ports.rs crates/hares-equipment/src/ports.rs crates/hares-envelope/src/thermal_solver/ports.rs|Audit PortDeclaration strings used by equipment against the port names consumed by solvers and actors. Check for: orphaned ports (declared but never accumulated), missing ports (accumulated but never declared), inconsistent naming conventions between crates. Trace every port name string. Write findings to the output file.")

REVIEWS+=("wiring-02|wiring|thermal-zone-routing|Thermal contribution zone routing: verify every equipment thermal output reaches the correct zone|vendors/OCHRE/ochre/Equipment/ vendors/EnergyPlus/src/EnergyPlus/|docs/architecture.md docs/thermal-envelope-solver.md|crates/hares-types/src/ports.rs crates/hares-core/src/dwelling/mod.rs crates/hares-envelope/src/thermal_solver/ports.rs|For every equipment type, trace the thermal output path: PortContribution::Thermal { zone, sensible_gain_w, radiant_gain_w, latent_gain_w, category } → zone-specific accumulator → solver input. Verify: no equipment writes to None zone (outdoor loss) incorrectly, no equipment writes to wrong zone, duct-loss zone routing is correct, water heater jacket loss goes to correct zone. Write findings to the output file.")

REVIEWS+=("wiring-03|wiring|electrical-power-accumulation|Electrical power accumulation: generation vs load sign conventions|vendors/OCHRE/ochre/Dwelling.py vendors/EnergyPlus/src/EnergyPlus/ElectricPowerServiceManager.cc|docs/architecture.md|crates/hares-types/src/ports.rs crates/hares-core/src/dwelling/mod.rs|Verify electrical sign conventions are consistent: PV generation = negative (or positive?), battery discharge = negative (or positive?), loads = positive. Check net power calculation in Dwelling. Check: do reactive power contributions aggregate correctly? Does the ZIP model produce correct complex power? Write findings to the output file.")

REVIEWS+=("wiring-04|wiring|humidity-solver-coupling|Humidity solver coupling: thermal→humidity latent payload alignment|vendors/EnergyPlus/src/EnergyPlus/HumidityManager.cc|docs/architecture.md docs/thermal-envelope-solver.md|crates/hares-envelope/src/humidity_solver.rs crates/hares-envelope/src/thermal_solver/stepping.rs crates/hares-core/src/dwelling/mod.rs|Thermal solver produces a per-zone payload with latent load, infiltration mass flow, and outdoor humidity ratio. The humidity solver consumes this payload for moisture balance. Verify: payload format [zone_id, q_latent_w, m_dot_inf_kg_s, w_outdoor, energy_balance_residual_w] is read in correct order. Check that humidity clamping (0 to saturation) doesn't create mass imbalance. Write findings to the output file.")

REVIEWS+=("wiring-05|wiring|equipment-telemetry-correctness|Equipment telemetry: verify all reported values are physically correct|vendors/OCHRE/ochre/Dwelling.py vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc|docs/invariants-and-observability.md|crates/hares-types/src/telemetry.rs crates/hares-types/src/telemetry_keys.rs crates/hares-core/src/telemetry.rs|Audit telemetry columns: are all values in correct units? Are reported power/energy values consistent with port accumulations? Check: COP reported for heat pumps, EER for AC, degradation metrics for battery, SOC for EV. Verify no NaN or infinity can be emitted. Write findings to the output file.")

REVIEWS+=("wiring-06|wiring|schedule-resolution-pipeline|Schedule resolution pipeline: HPXML → schedule source → runtime evaluation|vendors/OCHRE/ochre/utils/schedule.py|docs/schedule-sources.md|crates/hares-io/src/schedule.rs crates/hares-io/src/schedule_resolve.rs crates/hares-core/src/environment.rs|Trace the full schedule pipeline: HPXML schedule extension params → ScheduleSource → schedule index computation → runtime value lookup. Check: are duty cycle fractions correctly stored and applied? Are month/weekday/weekend multipliers correctly combined? Does stochastic/noisy schedule generation preserve expected energy? Write findings to the output file.")

REVIEWS+=("wiring-07|wiring|fluid-loop-port-wiring|Fluid loop port wiring: water heater ↔ hydronic heating loop connectivity|vendors/OCHRE/ochre/Equipment/WaterHeater.py vendors/EnergyPlus/src/EnergyPlus/Plant/|docs/architecture.md docs/equipment/water-heater.md|crates/hares-types/src/fluid.rs crates/hares-envelope/src/fluid_solver.rs crates/hares-equipment/src/water_heater/mod.rs|PortContribution::Fluid { loop_id, flow_rate_kg_s, supply_temp_c, return_temp_c, fluid_type } — verify that fluid ports are correctly wired for: boiler→hydronic distribution, water heater→hot water draws, HPWH compressor loop. Check: are fluid loops correctly identified? Does the fluid solver receive all contributions for a given loop_id? Write findings to the output file.")

REVIEWS+=("wiring-08|wiring|unit-consistency-cross-crate|Unit consistency across crate boundaries: W, kW, °C, kg/s, m³, Pa, kPa|vendors/OCHRE/ochre/utils/units.py vendors/EnergyPlus/src/EnergyPlus/Data/|docs/rust-primer.md|All crates|Audit unit conventions across crate boundaries. Check: does any function receive W but interpret as kW? Are temperature units always °C (never K or °F)? Are pressure units kPa or Pa consistently? Are mass flow rates kg/s consistently? Are there any silent unit conversions at crate boundaries? Use variable naming conventions as signal (suffixes _w, _kw, _c, _k, _kg_s, _kpa, _pa). Write findings to the output file.")

# ==================================================================
#  MAIN
# ==================================================================

main() {
    local mode="${1:-all}"
    local filter="${2:-}"

    if [ "$mode" = "--dry-run" ]; then
        echo "=== DRY RUN: Would execute the following reviews ==="
        for entry in "${REVIEWS[@]}"; do
            local id; id=$(review_id "$entry")
            local cat; cat=$(review_category "$entry")
            local title; title=$(review_title "$entry")
            local out; out=$(output_path_for "$entry")
            if [ -f "$out" ]; then
                echo "  SKIP ($out exists)  $id | $cat | $title"
            else
                echo "  RUN                 $id | $cat | $title  →  $out"
            fi
        done
        echo "=== End dry run ==="
        return 0
    fi

    # --category <name> or single review ID
    local filter_cat=""
    local filter_id=""
    if [ "$mode" = "--category" ]; then
        filter_cat="$filter"
    elif [ "$mode" != "all" ]; then
        filter_id="$mode"
    fi

    local count_skipped=0
    local count_ran=0
    local count_errors=0

    for entry in "${REVIEWS[@]}"; do
        local id; id=$(review_id "$entry")
        local cat; cat=$(review_category "$entry")

        # Apply filters
        if [ -n "$filter_cat" ] && [ "$cat" != "$filter_cat" ]; then
            continue
        fi
        if [ -n "$filter_id" ] && [ "$id" != "$filter_id" ]; then
            continue
        fi

        local out; out=$(output_path_for "$entry")
        if [ -f "$out" ]; then
            echo "SKIP $id — $out already exists"
            ((count_skipped++)) || true
            continue
        fi

        local prompt; prompt=$(review_prompt "$entry")
        local title; title=$(review_title "$entry")
        local vendor; vendor=$(review_vendor_refs "$entry")
        local files; files=$(review_files "$entry")
        local slug; slug=$(review_slug "$entry")

        # Build full prompt with output path directive
        local full_prompt
        full_prompt=$(cat <<PROMPT
You are performing a focused, single-concern code review of the HARES residential energy simulation codebase.

## Review: $title
## Review ID: $id
## Category: $cat

## HARES Source Files to Review
$files

## Vendor Reference Files (compare/contrast)
$vendor

## Review Instructions
$prompt

## Output Requirements
Write your findings to the file:
  $out

The output MUST be a markdown file with this structure:

```markdown
# $title
**Review ID**: $id
**Category**: $cat
**Date**: $(date +%Y-%m-%d)

## Files Reviewed
$files

## Vendor/Reference Files Consulted
$vendor

## Findings
### Finding 1: [Severity: critical|high|medium|low]
**Description**: ...
**Code Location**: ...
**Root Cause**: ...
**Impact**: ...

### Finding 2: [Severity: ...]
...

## Summary
- Total findings: N
- Critical: N
- High: N
- Medium: N
- Low: N

## Recommendations
1. ...

## References / Citations
- ...
```

Be thorough but focused. Only write findings that are relevant to this specific review area. Cite specific line numbers. Compare against vendor reference implementations where applicable.
PROMPT
)

        echo ""
        echo "============================================================"
        echo "REVIEW $id: $title"
        echo "Output: $out"
        echo "============================================================"

        if run_opencode "$full_prompt" "$out"; then
            ((count_ran++)) || true
            echo "PASS $id"
        else
            ((count_errors++)) || true
            echo "FAIL $id (exit code $?)"
        fi
    done

    echo ""
    echo "============================================================"
    echo "SUMMARY: $count_ran run, $count_skipped skipped, $count_errors errors"
    echo "============================================================"
}

main "$@"
