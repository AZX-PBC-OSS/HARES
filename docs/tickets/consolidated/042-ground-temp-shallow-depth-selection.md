# Ground Temperature Uses Shallowest EPW Depth; Should Use Foundation Depth

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io, hares-envelope

## Problem

When an EPW file contains multiple ground-temperature depth entries, `parse_ground_temperatures` in `crates/hares-io/src/epw.rs` selects the entry with the **smallest depth** (`best_depth < best_depth` selection loop at line 354). EPW files from EnergyPlus typically include ground temperatures at 0.5 m, 2.0 m, and 4.0 m depths. The 0.5 m depth has the largest seasonal amplitude — it closely tracks the outdoor air temperature sinusoid with minimal damping. For a residential slab-on-grade or basement foundation, the relevant depth for the thermal boundary condition is closer to 0.5–1.0 m for the slab surface but 1.5–3.0 m for foundation walls and the undisturbed soil boundary.

Selecting the shallowest depth (0.5 m) as the ground boundary temperature for all ground-coupled boundaries — slab, basement wall, and crawlspace — over-estimates the seasonal swing of the ground temperature. In winter, 0.5 m ground temperature is substantially colder than 2.0 m; in summer, substantially warmer. This produces a systematic over-prediction of slab heat loss in winter and heat gain in summer relative to the physically correct deeper boundary temperature.

The DOE-2 fallback (`doe2_ground_temp_monthly`) uses a fixed depth factor of `DOE2_GROUND_DEPTH_FACTOR = 10.0` m (line 371 in `epw.rs`), which is far too deep and essentially eliminates all seasonal variation. The actual floor/basement boundary in residential construction sits at 0.3–1.5 m below grade.

## Evidence

`crates/hares-io/src/epw.rs:354` — shallowest depth selected:
```rust
if valid && depth_m < best_depth {
    best_depth = depth_m;
    best_monthly = Some(monthly);
}
```

`crates/hares-io/src/epw.rs:371` — DOE-2 fallback uses 10 m depth factor:
```rust
const DOE2_GROUND_DEPTH_FACTOR: f64 = 10.0;
```

The Kusuda-Achenbach model in `crates/hares-physics/src/ground.rs` already supports depth-parameterized temperature at any depth, but is not used in the EPW-path ground temperature selection.

## Required Fix

1. Replace the shallowest-depth selection heuristic with a configurable target depth. The default should be 0.5 m for slab surface temperature (appropriate for slab-on-grade) and 2.0 m for foundation wall midpoint temperature (appropriate for basements). The solver builder at `hares-core/src/dwelling/solver_builder.rs` should specify the relevant depth when constructing the thermal solver config.

2. For the DOE-2 fallback, replace the `DOE2_GROUND_DEPTH_FACTOR = 10.0` constant with a depth parameter (default 0.5 m for slab surface). At 10 m depth the seasonal amplitude is attenuated by a factor of `exp(-10 * sqrt(π / (α * τ)))` ≈ `exp(-10 * 0.4)` ≈ 0.018 — effectively zero. At 0.5 m it is `exp(-0.5 * 0.4)` ≈ 0.82, which correctly captures seasonal variation.

3. Expose the ground boundary depth as a per-boundary configuration field in the HPXML parser and `ThermalSolverConfig`, allowing slab vs. basement vs. crawlspace to each specify their relevant depth.

## OCHRE Cross-Check

OCHRE `Envelope.py` uses the EPW 0.5 m depth by default but allows configuration. The current HARES behavior (selecting shallowest regardless of foundation type) is consistent with OCHRE's default but known to introduce error for deep foundations. EnergyPlus uses the Kusuda-Achenbach model with building-specific soil properties and depth.

## Annual kWh Impact

Medium. For slab-on-grade buildings, the 0.5 m selection is approximately correct. For buildings with basements or deep crawlspaces, the error in annual ground heat loss can reach 5–15% of the total ground coupling load. In climate zones 4–7 (cold climates), ground coupling is a significant fraction of the heating load.

## References

- EnergyPlus Engineering Reference §3.17 "Ground Heat Transfer Calculations" — Kusuda-Achenbach model with site-specific depth
- Kusuda, T. and Achenbach, P.R. (1965) ASHRAE Transactions 71(1):61-74 — original derivation
- ASHRAE Handbook of Fundamentals 2021 Ch. 18.31 "Below-Grade Heat Transfer" — recommended depths for different foundation types
- `crates/hares-physics/src/ground.rs:63-80` — Kusuda-Achenbach implementation already present
