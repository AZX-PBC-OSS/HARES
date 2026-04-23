# ResStock CSV Parser: Constant Pressure From Elevation, No Diurnal Variation

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-io/resstock_csv
**Weather-cluster note**: This ticket belongs adjacent to the 024–034 weather pipeline cluster. Do not move — renumber when the cluster is sequenced.

## Problem

`parse_resstock_csv` in `hares-io/src/resstock_csv.rs:115` computes `let pressure_kpa = isa_pressure_kpa(elevation_m)` once and reuses it for every row. The ResStock 8-column format has no pressure field; the ISA estimate is the best available approximation. This is a known format limitation, not a parsing bug. However, a second defect in the same file is a fixable bug: timestep inference hardcodes an 8 760-hour year.

Downstream effects of constant pressure:
1. `humidity_ratio_from_tdp(dew_point_c, pressure_pa)` receives a constant pressure at every step. At sea level a 10 hPa swing causes ~0.5 % error in humidity ratio; at Denver (1 609 m), 1–2 %.
2. Psychrometric properties (wet-bulb, enthalpy) also depend on pressure; constant pressure produces constant specific volume. Acceptable for ResStock's comparative-benchmark role but outside ASHRAE HoF 2021 Ch. 1 §1.2 requirements for full psychrometric accuracy.

### Leap-year timestep inference bug

`resstock_csv.rs:254–263`:
```rust
let total_seconds_in_year = 8760 * 3600;
if total_seconds_in_year % n == 0 {
    (total_seconds_in_year / n) as u32
} else {
    3600
}
```

For a leap-year file with 8 784 hourly rows, `8760 * 3600 % 8784 != 0`, so the modulo check fails and the step defaults to 3 600 s. For hourly data this is accidentally correct, but for sub-hourly leap-year files it will produce a wrong step. The is-leap-year detection at lines 266–276 correctly uses the first data row's timestamp, but its result is not fed back into the step inference.

## Current Behavior

`hares-io/src/resstock_csv.rs:115`: single constant pressure for all rows (format limitation — no fix available; document only).
`hares-io/src/resstock_csv.rs:254–263`: step inference hardcodes `8760 * 3600` regardless of leap year (fixable bug).

## Required Behavior

1. Move is-leap-year detection (currently lines 266–276) to before the `source_step_secs` block. Compute `total_seconds_in_year = (if is_leap_year { 8_784_u64 } else { 8_760_u64 }) * 3_600` and use it for the modulo check. This is the only code change required.

2. Add a module-level doc comment at the top of `resstock_csv.rs` stating: "ResStock CSV pressure is a constant ISA estimate derived from site elevation. The 8-column format contains no measured pressure data; per-row pressure variation is not achievable. See ISO 2533:1975 §5 for the ISA model." Do not add a silent fallback, override mechanism, or any warning per-row — the constant is the correct best estimate for this format.

Per project policy `feedback_no_silent_defaults.md`: the constant-pressure limitation is documented, not silently hidden. Per `feedback_no_backward_compat.md`: no shim or override hook for callers who want different pressure.

## Approach

1. Extract or reorder the is-leap-year detection block so it runs before `source_step_secs` assignment.
2. Replace `let total_seconds_in_year = 8760 * 3600` with the leap-year-aware expression.
3. Add the module-level doc comment.

## Definition of Done

- [ ] Timestep inference uses leap-year-aware year length (`8_784` or `8_760` based on first-row timestamp)
- [ ] Module doc comment documents constant-pressure limitation with ISO 2533:1975 citation
- [ ] `cargo test -p hares-io resstock` passes
- [ ] Test: 8 784-row ResStock CSV (leap year) correctly infers 3 600 s timestep

## Verification

```bash
cargo test -p hares-io resstock
```

## References

- ResStock weather format: https://github.com/NREL/resstock — 8-column simplified CSV; no pressure field
- ASHRAE Handbook of Fundamentals 2021 Ch. 1 §1.2 "Psychrometrics" — pressure dependence of humidity ratio
- ISO 2533:1975 Standard Atmosphere §5 — basis for `isa_pressure_kpa` approximation
