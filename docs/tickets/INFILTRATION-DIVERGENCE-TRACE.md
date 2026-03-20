# Complete Infiltration Numerical Trace: HARES vs OCHRE

**Date:** 2026-03-20
**Purpose:** Debug 72x infiltration discrepancy (HARES -847 W vs OCHRE -12 W)

## Input Parameters (Corrected BEopt)

| Parameter | Value | Unit |
|-----------|-------|------|
| ACH50 | 5.0 | ACH |
| Volume | 272.0 | m³ |
| Floor Area | 111.5 | m² |
| Infiltration Height | 8.0 ft → 2.4384 | m |
| Floors Above Grade | 1 | - |
| Has Flue/Chimney | No | - |
| Shielding | Normal (0.5) | - |
| Terrain | Suburban | - |
| Indoor Temp | 21.0 | °C |
| Outdoor Temp | 12.0 | °C |
| ΔT | 9.0 | K |
| Wind Speed | 3.0 | m/s |

---

## TASK 1: OCHRE Computation (envelope.py:488-633)

OCHRE uses an **IP-unit-based path** with SLA/ELA intermediate calculations:

### Step 1: Terrain Correction (f_t)
```
f_t = (δ_met / h_met)^α_met × (H / δ_site)^α_site
    = (270 / 10)^0.14 × (2.4384 / 370)^0.22
    = 1.586320 × 0.331252
    = 0.525472
```

### Step 2: Shelter Factor
```
inf_sft = f_t × (shelter_raw × (1 - y_i) + s_wflue × 1.5 × y_i)
        = 0.525472 × 0.5 × (1 - 0)
        = 0.262736
```

### Step 3-4: SLA & ELA (IP Path)
```
Q_50 = 5.0 × 272 / 3600 = 0.377778 m³/s

living_sla = (ach × 0.2835 × 4^n_i × V_ft³) / (floor_area_in² × dp^n_i × 60)
           = (5.0 × 0.2835 × 2.462289 × 9605.6) / (172826.8 × 12.715414 × 60)
           = 0.03661476 ft²

ELA = SLA × floor_area_ft² = 0.03661476 × 1200.2 = 43.94452 ft²
```

### Step 4: Flow Coefficient C (via ASHRAE Orifice Equation)

**CRITICAL DIVERGENCE POINT:** OCHRE uses a complex orifice formula:
```
C_i (CFM) = ELA × √(2/ρ) × (ΔP_ref)^(0.5-n_i) × conv_factor
          = 43.94452 × 5.113100 × 1.859439 × 776.25
          = 324,319.18 CFM/(inH2O^0.65)

C_i (SI) = 324,319.18 / 2118.88 / 249.089^0.65
         = 153.06160688 m³/s/Pa^0.65
```

**This is 5,159× LARGER than HARES's C value.**

### Step 5-6: Leakage Distribution & Stack Factor
```
r_i = 0.50,  x_i = 0.0,  y_i = 0.0
m_o = 0,     m_i = 0
f_s = (0.803030) × (0.5)^1.65 = 0.255878
```

### Step 7: Stack Coefficient Cs
```
Cs (IP) = f_s × (ρ × g × H_ft / T_R)^n_i
        = 0.255878 × 0.036908^0.65
        = 0.029968 inH2O^0.65/°R^0.65

inf_Cs (SI) = 0.029968 × (1/249.089)^0.65
            = 0.00082991 Pa^0.65/K^0.65
```

### Step 8-9: Wind Factor & Coefficient
```
J_i = (x_i + r_i + 2×y_i) / 2 = 0.25
f_w = 0.224438

inf_Cw (SI) = 0.224438 × (0.0765/2)^0.65 × (1/(249.089×0.44704))^0.65
            = 0.00125733 Pa^0.65/(m/s)^1.3
```

### Step 10: OCHRE Final Coefficients
```
c_s = C × Cs = 153.06160688 × 0.00082991 = 0.12702745 m³/s/K^0.65
c_w = C × Cw = 153.06160688 × 0.00125733 = 0.19244938 m³/s/(m/s)^1.3
shelter = 0.262736
```

### Runtime: OCHRE Flow at ΔT=9K, v=3 m/s
```
Q_stack = 0.12702745 × 9^0.65 = 0.52985278 m³/s
Q_wind  = 0.19244938 × (0.262736×3)^1.3 = 0.14123739 m³/s
Q_total = √(0.52985² + 0.14123²) = 0.54835388 m³/s

Q_sensible = 1.2041 × 1005 × 0.54835388 × 9 = 5,972.2 W ✓
```

---

## TASK 2: HARES Computation (infiltration.rs:389-487)

HARES uses a **pure SI approach** with direct flow coefficient:

### Step 1: Flow Coefficient C (SI, Direct)
```
C = Q_50 / 50^n_i
  = (5.0 × 272 / 3600) / 50^0.65
  = 0.377778 / 12.715414
  = 0.02971022 m³/s/Pa^0.65
```

**This is 5,159× SMALLER than OCHRE's C.**

### Step 2-3: Leakage Distribution & Stack Factor (identical)
```
r_i = 0.50,  x_i = 0.0
f_s = 0.255878
```

### Step 4: Stack Coefficient Cs (Pure SI)
```
Cs = f_s × (ρ × g × H / T_in)^n_i
   = 0.255878 × (1.2041 × 9.80665 × 2.4384 / 296.15)^0.65
   = 0.255878 × 0.097225^0.65
   = 0.05624542 Pa^0.65/K^0.65
```

**This is 68× LARGER than OCHRE's Cs.**

### Step 5-6: Wind Factor & Coefficient
```
f_w = 0.224438  (identical to OCHRE)
Cw = 0.224438 × (1.2041/2)^0.65
   = 0.16138256 Pa^0.65/(m/s)^1.3
```

**This is 128× LARGER than OCHRE's Cw.**

### Step 7: Shelter Coefficient (identical)
```
f_t = 0.525472
shelter = 0.262736
```

### Step 8: HARES Final Coefficients
```
c_s = 0.02971022 × 0.05624542 = 0.00167106 m³/s/K^0.65
c_w = 0.02971022 × 0.16138256 = 0.00479471 m³/s/(m/s)^1.3
shelter = 0.262736
```

### Runtime: HARES Flow at ΔT=9K, v=3 m/s
```
Q_stack = 0.00167106 × 9^0.65 = 0.00697029 m³/s
Q_wind  = 0.00479471 × (0.262736×3)^1.3 = 0.00351881 m³/s
Q_total = √(0.00697² + 0.00351²) = 0.00780813 m³/s

Q_sensible = 1.2041 × 1005 × 0.00780813 × 9 = 85.0 W ✗
```

---

## TASK 3: Divergence Analysis

### Coefficient Comparison

| Parameter | OCHRE | HARES | Ratio (HARES/OCHRE) |
|-----------|-------|-------|---------------------|
| **C** | 153.062 | 0.02971 | **0.000194x** (5159x smaller) |
| **Cs** | 0.00083 | 0.05625 | **67.9x larger** |
| **Cw** | 0.00126 | 0.16138 | **128x larger** |
| **c_s = C×Cs** | 0.127 | 0.00167 | **76x smaller** |
| **c_w = C×Cw** | 0.192 | 0.00479 | **40x smaller** |
| **shelter** | 0.262736 | 0.262736 | **1.0x (identical)** |
| **Q_total (m³/s)** | 0.548 | 0.00781 | **70x smaller** |

### Root Cause: Two Fundamentally Different C Definitions

#### OCHRE's Path: IP-Unit ELA Formula
```
1. Converts Q_50 to SLA (ft²) using ASHRAE pressure-exponent formula
2. Computes ELA = SLA × floor_area_ft²
3. Uses ASHRAE orifice equation with √(2/ρ) term
4. Converts result through IP→SI unit chain with (Pa/inH2O)^n_i factors
Result: C = 153.062 m³/s/Pa^0.65
```

#### HARES's Path: Direct SI Formula
```
1. Computes Q_50 directly in SI: Q_50 = ACH50 × V / 3600 [m³/s]
2. Divides by 50^n_i to get C
Result: C = 0.02971 m³/s/Pa^0.65
```

### Compensation Mechanism

OCHRE's massive C is **compensated** by tiny Cs/Cw coefficients:
- Large C × Small (Cs, Cw) = **Moderate c_s, c_w**

HARES's reasonable C is **combined** with reasonable Cs/Cw:
- Small C × Large (Cs, Cw) = **Tiny c_s, c_w**

Both should be equivalent per Walker-Wilson, but the formulas diverge at the **flow coefficient C definition**.

---

## TASK 4: Ventilation Lumping Analysis

From `crates/hares-envelope/src/thermal_solver.rs:968-988`:

```rust
let total_nat_flow = q_inf_m3_s + q_nat_m3_s;
let sensible_flow = if balanced {
    total_nat_flow + forced * (1 - recovery_efficiency)
} else {
    sqrt(total_nat_flow² + forced²)
};
let q_sensible = m_dot * c_p * (t_out - t_zone);
```

**Result:** Infiltration and forced ventilation are **ADDITIVELY combined** (balanced case).

If the test case has:
- Forced ventilation: **0.268 m³/s** (ASHRAE 62.2 ≈ 15 CFM/100 ft² ≈ 0.27 m³/s for 111.5 m²)
- Recovery efficiency: **70%** (typical HRV)
- Then: `sensible_flow = 0.00781 + 0.268 × (1 - 0.70) = 0.00781 + 0.0804 = 0.088 m³/s`
- Leading to: `Q_sensible ≈ 960 W` (still way too small vs OCHRE's -12 W isolated infiltration)

The measured **-847 W** includes both infiltration **and** ventilation, but even combined they're **70x too small**.

---

## Summary of Findings

| Finding | Details |
|---------|---------|
| **HARES infiltration** | 85.0 W (0.00781 m³/s) ← **70x too small** |
| **OCHRE infiltration** | 5,972 W (0.548 m³/s) ← Physically reasonable |
| **Root cause** | HARES uses C = Q_50/50^n_i directly; OCHRE uses ELA-based orifice formula |
| **C divergence** | OCHRE's 153.062 vs HARES's 0.02971 (5159x) |
| **Cs divergence** | OCHRE 0.00083 vs HARES 0.05625 (68x) |
| **Cw divergence** | OCHRE 0.00126 vs HARES 0.16138 (128x) |
| **Final c_s product** | OCHRE 0.127 vs HARES 0.00167 (76x) |
| **Runtime flow** | OCHRE 0.548 m³/s vs HARES 0.00781 m³/s (70x) |

---

## Critical Finding: Flow Coefficient Definition Mismatch

**The HARES formula `C = Q_50 / 50^n_i` is NOT the same as OCHRE's ELA-based approach.**

- OCHRE derives C from the ASHRAE orifice equation applied to ELA
- HARES derives C directly from the pressure-exponent definition Q ∝ ΔP^n_i
- These define C with different **physical dimensions and meaning**

### Hypothesis

HARES's implementation may be using the wrong definition of the flow coefficient. The Walker-Wilson (1998) paper likely uses the **ELA-based** C, not the simple Q/ΔP^n relationship.

### Evidence

If HARES is correct, then infiltration would be **trivial** (~85 W) compared to solar gains and other loads. This is physically implausible for a house with **ACH50 = 5.0** (moderately leaky).

OCHRE's **5,972 W** is consistent with:
- Typical US homes: 0.3–0.8 m³/s natural infiltration (winter conditions)
- This translates to 1.4–2.0 ACH at building baseline

---

## Recommended Next Steps

1. **Audit Walker-Wilson (1998) directly** for the exact definition of flow coefficient C
2. **Compare both C calculations** against Table 5 in ASHRAE HOF 2017 Ch. 16 (if available in HARES)
3. **Check if HARES is missing the ELA step** — the SLA/ELA calculation may be the "calibration" that converts raw Q_50 into proper infiltration parameters
4. **Verify OCHRE's SLA/ELA formula** against published ASHRAE 136 or ResStock sources
5. **Consider unit conversion errors** — ensure all n_i exponents are applied correctly in both paths

---

## Files Involved

- **OCHRE:** `/home/rich/src/HARES/vendors/OCHRE/ochre/utils/envelope.py:488-633`
- **HARES:** `/home/rich/src/HARES/crates/hares-physics/src/infiltration.rs:389-487`
- **HARES Thermal Solver:** `/home/rich/src/HARES/crates/hares-envelope/src/thermal_solver.rs:889-1005`
- **HARES Dwelling Config:** `/home/rich/src/HARES/crates/hares-core/src/dwelling.rs:1543-1559`

