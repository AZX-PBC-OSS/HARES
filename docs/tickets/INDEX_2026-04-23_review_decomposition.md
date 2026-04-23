# Review Decomposition Index — 2026-04-23

This index records the decomposition of five review reports into actionable
fix tickets. Source reviews:

- `docs/findings/reviews/01_weather_timestep.md`
- `docs/findings/reviews/02_rc_envelope_solver.md`
- `docs/findings/reviews/03_envelope_radiation_ports.md`
- `docs/findings/reviews/04_hvac_wh_hpxml.md`
- `docs/findings/reviews/05_dwelling_actors_ports.md`

Three blocker fixes were already applied in-tree before this decomposition
(WR-01 ZOH defaults, WR-02 resampled-weather test, F1 envelope `.max(2)`
node override) and are not ticketed here.

## New tickets — by severity

### HIGH (P1)

- **089** `radiation-frac-starmesh-rederivation` — re-derive `radiation_frac` voltage divider under StarMesh Y-Δ topology after S1/S5 changes; fixes ~27% spatial-distribution bias of solar+radiant gains
- **090** `rederive-per-surface-ua-from-first-principles` — drop hardcoded OCHRE-derived per-surface UA constants and parity tolerance widening; rederive expected UA from ASHRAE/E+ first principles, gate on BESTEST 140 reference bands
- **091** `port-radiant-inputs-all-zones` — `apply_port_radiant_inputs` filters to indoor zone only; make it iterate all zones consistent with the sensible path
- **092** `zone-sensible-breakdown-debug-includes-radiant` — `zone_sensible_breakdown_debug` omits radiant port inputs, understating zone-air contribution by ~21%
- **093** `seer-silent-zero-fallback-loud-error` — `apply_default_hvac_speed_fallback` uses `unwrap_or(0.0)` for SEER then infers single-speed from 0.0; fail loudly or use EER fallback
- **094** `bestest-tests-still-ignored` — all 5 BESTEST tests remain `#[ignore]` despite S4/S5 fixes; BESTEST is the primary acceptance gate
- **095** `backup-switchover-temp-er-lockout-only` — `BackupHeatingSwitchoverTemperature` incorrectly mapped to both HP and ER lockout; HPXML 4.x §8.4 specifies ER lockout only
- **096** `actor-trait-telemetry-method` — `Actor` trait has no `telemetry()` method, violating `feedback_actor_telemetry`
- **097** `tmy3-midpoint-offset-regression-test` — pin `tmy3.meta.midpoint_offset_secs == 1800` against future regression
- **098** `triangular-resample-docstring-or-mean-preserving` — `Triangular` doc claims hourly mean preservation but actual mean differs; correct doc or change filter
- **100** `toml-resample-method-silent-discard` — TOML config silently discards unknown resample method names while Python kwargs path raises; unify on loud error

### MEDIUM (P2)

- **099** `resstock-csv-midpoint-offset` — ResStock CSV has `midpoint_offset_secs = 0` for end-of-interval timestamps; analogous to fixed TMY3 B4 (should be 1800)
- **101** `interior-film-coefficients-recompute-per-step` — interior film coefficients still frozen at init time despite S1 partial fix; recompute per timestep
- **102** `thermal-solver-init-indoor-zone-loud-error` — silent flat-profile fallback when `indoor_zone_id` absent; error loudly
- **103** `zone-capacitance-air-density-loud-error` — silent sea-level air density when `site_pressure_pa <= 0`; error loudly
- **104** `r-film-interior-constant-mismatch` — `R_FILM_INTERIOR_M2_K_W = 0.12` is ISO 6946 combined value but production uses convection-only; rename or remove
- **106** `nfrc-fallback-condition-h-out-positive` — NFRC fallback condition `> 1.0` should be `> 0.0`; values in (0, 1] silently fall back
- **107** `port-radiant-input-index-bounds-check` — no bounds check on `input_index` before array access in radiant distribution
- **108** `port-radiant-w-python-and-diag-exposure` — `port_radiant_w` missing from Python `post_solvers` dict and `EnvelopeDiag`
- **111** `resistance-efficiency-silent-default-loud-error` — `resistance_efficiency_from_params` silently returns 1.0 when efficiency absent
- **112** `boiler-flow-rate-return-temp-silent-defaults` — boiler `flow_rate_kg_s` and `return_temp_c` silent defaults; either error or cite primary source
- **113** `reconcile-setpoint-pair-loud-error` — `reconcile_setpoint_pair` silently mutates HPXML setpoints when gap < 2°C; return error or expose machine-readable signal
- **114** `scheduled-load-sensible-fraction-loud-error` — `sensible_gain_fraction` falls back to 0.5 with `tracing::warn!`; return `Err`

### LOW (P3)

- **105** `default-hp-lockout-temp-citation` — `DEFAULT_HP_LOCKOUT_TEMP_C = -17.78°C` lacks primary-source citation
- **109** `occupant-count-zero-silent-when-schedule-domain-absent` — silent 0.0 occupants when schedule domain missing
- **110** `window-u-factor-shgc-silent-defaults` — `unwrap_or(5.0)` and `unwrap_or(0.4)` silently inject single-pane aluminum window performance
- **115** `h-rad-linearisation-consolidate` — `h_rad = 4εσT³` linearised in three locations; consolidate into shared `hares-physics` helper
- **116** `inner-node-id-defensive-assert` — fragile `inner_node` `NodeId` formula needs defensive `assert_ne!` guard
- **117** `window-input-index-comment-correction` — outdated comment claims "Windows (input_index=None)" but production sets `Some(zone_air_idx)`
- **118** `foundation-zone-name-default-warn` — silent map of absent foundation zone name to `"attic_vented"`
- **119** `biquadratic-curve-count-mismatch-warn` — silent fill with unity polynomials when cap/EIR curve counts mismatch
- **120** `compressor-type-wildcard-warn-fail-closed` — `_ => "single_speed"` wildcard silently maps unknown HPXML CompressorType values
- **121** `combi-boiler-indirect-tank-support` — combi boiler + indirect tank not implemented; resolver emits parse error
- **122** `water-density-temperature-correction` — `WATER_DENSITY_KG_PER_M3 = 1000.0` overestimates by ~1.2% at typical 50°C tank temp
- **123** `remove-false-ignore-comments-from-tests` — three tests carry false `#[ignore]` comments but actually run

### NIT (P4)

- **124** `isa-pressure-exponent-precision` — `ISA_PRESSURE_EXPONENT = 5.2559` constant vs inline `5.25588` in resstock_csv; unify and improve precision
- **125** `berdahl-martin-coefficient-citation` — coefficients 0.758/0.521/0.625 cited to Martin & Berdahl (1984) but are actually Li et al. (2017) recalibrated values
- **126** `ochre-compat-doc-sky-temp-divergence-note` — `ochre_compat()` doc should note sky-temperature recomputation diverges from OCHRE
- **127** `port-sensible-w-rename-port-convective-w` — naming asymmetry footgun; rename `port_sensible_w` → `port_convective_w`
- **128** `interior-lwr-method-explicit-starmesh-callsites` — use `InteriorLwrMethod::StarMesh` explicitly rather than `::default()`
- **129** `solve-ideal-capacity-failure-warn-not-debug` — promote ideal-capacity convergence-failure log from `debug!` to `warn!`
- **130** `h-out-nfrc-constant-export` — `H_OUT_NFRC` constant private; export to prevent future duplication
- **131** `cargo-fmt-boundary-diagnostics-test-literals` — 15+ `boundary_diagnostics: Vec::new()` test literals have wrong indentation; run `cargo fmt`

## Already-fixed findings — NOT TICKETED

| Finding | Status | Location of fix |
|---|---|---|
| Review 01 WR-01 (`ochre_compat()` ZOH defaults) | FIXED | `crates/hares-io/src/weather.rs:324-338` |
| Review 01 WR-02 (`resampled_weather_produces_smooth_environment` test) | FIXED | `crates/hares-core/tests/weather_integration.rs` |
| Review 02 F1 + F2 (envelope `.max(2)` node override) | FIXED | `crates/hares-envelope/src/boundary_rc.rs:1077-1088` and tests at lines 1561, 1660, 1745 |

## Dedup table — review findings already covered by existing tickets

| Review finding | Existing ticket | Notes |
|---|---|---|
| Review 01 WR-16 (EPW `liquid_precip` silent zero) | **031** `epw-liquid-precip-silent-zero` | Same defect, already documented |
| Review 02 F2 (`.max(2)`) | (BLOCKER fix, already done) | See "Already-fixed" table |
| Review 03 F9 (`Vec::with_capacity` in `apply_port_radiant_inputs`) | **040** `radiant-gain-weights-hot-alloc` | Same hot-loop allocation issue |
| Review 04 F3 (ER backup uses `plr`) | **015** `er-on-off-modeling` | Same root cause |
| Review 04 F4 (`ideal_hvac` no biquadratic) | **002** `ideal-hvac-biquadratic-fallback` | Same defect |
| Review 04 F10 (heating_config setpoint duplicated) | **009** `consolidate-config-helpers` | Subsumed by config-helper consolidation |

## Numbering note

Tickets 056-069 were left empty in the prior numbering scheme. New tickets
were assigned 089-131 sequentially without reusing 056-069, so future
contributors can find new tickets by looking at the highest-numbered files.
