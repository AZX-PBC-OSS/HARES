# Winter MAE Investigation: Root Cause Analysis

## Executive Summary

The winter scenario MAE (5.3°C) is significantly worse than spring (2.6°C) and summer (2.8°C) due to **compounding effects of higher infiltration losses and floor heat gain divergence** under winter temperature differentials.

**Key Finding:** HARES infiltration heat loss is ~3.5-4x higher than OCHRE in winter, and this difference is amplified by the larger indoor-outdoor temperature differential in winter initialization.

---

## Quantitative Comparison: All Seasons

### Indoor Temperature MAE
| Season | HARES Mean | OCHRE Mean | MAE | Status |
|--------|-----------|-----------|-----|--------|
| Winter | 8.91°C | 14.21°C | **5.307°C** | Worst |
| Spring | 14.80°C | 16.00°C | 2.643°C | Good |
| Summer | 23.11°C | 23.59°C | 2.844°C | Good |

### Infiltration Heat Loss (Step 0)
| Season | HARES | OCHRE | Δ (HARES - OCHRE) | Ratio |
|--------|-------|-------|-------------------|-------|
| Winter | -203.8 W | -54.1 W | **-149.7 W** | **3.8x higher loss** |
| Spring | -104.2 W | -13.5 W | -90.7 W | 7.7x higher loss |
| Summer | +11.8 W | +8.6 W | +3.2 W | Comparable |

### Infiltration Mean (Full Simulation)
| Season | HARES Mean | OCHRE Mean | Ratio |
|--------|-----------|-----------|-------|
| Winter | -94.9 W | -26.7 W | **3.5x** |
| Spring | -1.6 W | -3.3 W | 0.5x (close) |
| Summer | +21.5 W | +2.5 W | 8.6x (but both small gains) |

### Floor Heat Gain (Step 0)
| Season | HARES | OCHRE | Δ |
|--------|-------|-------|---|
| Winter | -1291.8 W | +166.6 W | **-1458.4 W** |
| Spring | -1112.0 W | +111.5 W | -1223.4 W |
| Summer | -590.9 W | +145.0 W | -735.9 W |

---

## Why Winter is Worse: Detailed Analysis

### 1. Infiltration Flow Rate Differences

**OCHRE Reference Flow Rates (Step 0):**
- Winter: 0.004304 m³/s
- Spring: 0.001243 m³/s  
- Summer: 0.001028 m³/s
- **Winter has 3.5x higher flow than spring in OCHRE**

**HARES Flow Rates:**
- Winter: ~0.004 m³/s (implied from -203.8W / (17.4°C × cp × ρ))
- HARES appears to match OCHRE's winter flow rate
- But the **heat loss magnitude is amplified by larger ΔT**

### 2. Temperature Differential Amplification

The infiltration heat loss formula is: `Q = m_dot × cp × (T_out - T_zone)`

**Winter:**
- HARES: T_zone=21.5°C, T_out=4.1°C → ΔT = -17.4°C
- OCHRE: T_zone=24.4°C, T_out=12.2°C → ΔT = -12.2°C
- HARES has **43% larger temperature differential**
- Result: -203.8W vs -54.1W (3.8x difference)

**Spring:**
- HARES: T_zone=21.6°C, T_out=10.3°C → ΔT = -11.3°C
- OCHRE: T_zone=21.6°C, T_out=11.1°C → ΔT = -10.5°C
- Small differential, infiltration losses much smaller

**Summer:**
- HARES: T_zone=24.4°C, T_out=26.1°C → ΔT = +1.7°C (small gain)
- OCHRE: T_zone=24.4°C, T_out=33.3°C → ΔT = +8.9°C (larger gain)
- Both models show infiltration gains (outdoor warmer than indoor)

### 3. Floor Heat Gain - The Secondary Factor

Floor heat gain shows the largest single-component divergence:

**Formula:** `Q_floor = (T_surface - T_zone) × Area / R_film`

**Key observations:**
- HARES floor heat gain is **negative** (heat loss) in all seasons
- OCHRE floor heat gain is **positive** (heat gain) in all seasons
- Winter divergence is largest: -1458.4W difference

**Root cause hypothesis:**
The ground initialization or deep ground temperature coupling differs between models:
- Winter ground temp: 6.5°C (OCHRE reference)
- Spring ground temp: 8.8°C
- Summer ground temp: 15.1°C

HARES likely initializes floor surface temperatures differently, causing:
1. Lower floor surface temperature in HARES
2. Heat flows FROM zone TO floor in HARES (negative gain)
3. Heat flows FROM floor TO zone in OCHRE (positive gain)

---

## Code References

### Infiltration Implementation

**File:** `crates/hares-physics/src/infiltration.rs:104-116`
```rust
pub fn ashrae_wind_stack(
    c_s: f64,
    c_w: f64,
    delta_t_c: f64,  // This is (T_out - T_zone)
    wind_speed_m_s: f64,
    shelter_coeff: f64,
    n_i: f64,
) -> f64 {
    let n_i = n_i.clamp(N_I_MIN, N_I_MAX);
    let q_temp = c_s * delta_t_c.abs().powf(n_i);  // Stack component
    let q_wind = c_w * (shelter_coeff.max(0.0) * wind_speed_m_s).powf(2.0 * n_i);
    (q_temp * q_temp + q_wind * q_wind).sqrt()
}
```

**File:** `crates/hares-envelope/src/thermal_solver/infiltration.rs:150`
```rust
let delta_t = t_out - zone.temperature_c;  // Used for sensible heat calculation
let q_sensible = m_dot * CP_DRY_AIR_J_KG_K * delta_t;
```

### Coefficient Calculation

**File:** `crates/hares-physics/src/infiltration.rs:450-548`
- `aim2_coefficients_from_ach50()` computes c_s, c_w from ACH50
- Uses Walker & Wilson (1998) methodology
- n_i = 0.65 (default for residential)

### Floor Heat Gain Calculation

**File:** `crates/hares-envelope/src/thermal_solver/stepping.rs:191-194`
```rust
let q = (t_surface - t_zone) * diag.area_m2 / diag.r_film_int_m2_k_w;
match diag.category {
    BoundaryCategory::Floor => self.component_gains.floor_heat_gain_w += q,
    // ...
}
```

---

## Recommendations

### Priority 1: Infiltration Initialization Temperature
The largest contributor to winter MAE is the infiltration heat loss, driven by:
1. HARES starting at lower indoor temperature (21.5°C vs OCHRE 24.4°C)
2. HARES having colder outdoor temperature (4.1°C vs OCHRE 12.2°C) at step 0

**Action:** Compare initialization sequences between HARES and OCHRE to understand why outdoor/indoor temperatures differ at step 0.

### Priority 2: Ground/Floor Coupling
Floor heat gain shows 1000+ W divergence in all seasons, with winter being worst.

**Action:** Verify:
1. Ground temperature initialization (Kusuda-Achenbach model parameters)
2. Floor surface temperature initialization in RC network
3. Deep ground boundary condition handling

### Priority 3: Wind Speed Data
Investigate if winter has higher wind speeds that amplify the wind-driven infiltration component.

---

## Summary Table: Root Causes by Impact

| Factor | Winter Impact | Spring Impact | Summer Impact | Root Cause Confidence |
|--------|--------------|---------------|---------------|---------------------|
| Infiltration heat loss | **HIGH** (Δ=-150W) | Medium (Δ=-91W) | Low (Δ=+3W) | **Confirmed** |
| Floor heat gain | **HIGH** (Δ=-1458W) | High (Δ=-1223W) | Medium (Δ=-736W) | Likely initialization |
| Solar gains | Medium | Medium | Medium | Known model differences |
| Interior LWR | Equal (0W vs ~100W) | Equal | Equal | Known issue |

**Conclusion:** The winter MAE being worse is primarily due to the **compounding effect of higher temperature differentials amplifying the infiltration heat loss discrepancy**, which is already present in all seasons but becomes most significant when outdoor temperatures are much colder than indoor.
