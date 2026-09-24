# Default Biquadratic Performance Curves Are Identity

**Severity**: Critical
**Priority**: P0
**Status**: Open
**Areas**: hares-equipment/hvac/heat_pump, hares-equipment/hvac, hares-physics/biquadratic

## Dependencies

- **Must ship before or with ticket 003** (tighten biquadratic default bounds). Rationale: identity coefficients evaluated within tighter bounds would silently clamp every curve evaluation to the rated-condition point, producing `cap_ratio = 1.0` at all temperatures and making the bound tightening appear correct while leaving the physics broken. The two tickets must reach production together.
- **Ticket 012 verification depends on this ticket**. The heating-side SHR and defrost latent tests in ticket 012 require realistic `cap_ratio` values to trigger defrost at physically correct outdoor temperatures. Verification runs of ticket 012 with identity curves in place are not representative and must not be treated as passing.

## Problem

Biquadratic curves default to identity coefficients `[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]`, meaning COP and capacity are constant regardless of outdoor temperature. This produces physically impossible results:

- **COP = 1/EIR at ALL temperatures** — no degradation at low OAT for heating. Real heat pumps lose 30–50% capacity from 47°F (8.3°C) to 17°F (-8.3°C).
- **No capacity loss at low OAT** — the biquadratic formula evaluates to 1.0 for every `(T_indoor, T_outdoor)` pair, so `cap_ratio = 1.0` always.
- **Expected to produce significant annual energy differences in cold climates** — based on the magnitude of the capacity correction at typical operating conditions — e.g., a typical ASHP capacity correction factor at -8.3°C (17°F) is 0.60-0.70 vs identity curve's 1.0.

The identity default was a safe placeholder during early development, but it is now the single largest source of physics error in the HP heating model.

## Current Behavior

1. **Default coefficients** are defined in `hvac_core.rs:23`:
   ```rust
   pub(super) const DEFAULT_BIQUADRATIC_COEFFS: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
   ```

2. **Construction** initializes with identity: `hvac_core.rs:294`:
   ```rust
   biquadratic_coeffs: vec![DEFAULT_BIQUADRATIC_COEFFS],
   ```

3. **Curve evaluation** in `hvac_core.rs:811-829` — `evaluate_biquadratic` falls back to the last entry (or `DEFAULT_BIQUADRATIC_COEFFS` if empty), which is identity:
   ```rust
   let coeffs = self.biquadratic_coeffs.get(curve_index).copied()
       .or_else(|| self.biquadratic_coeffs.last().copied())
       .unwrap_or(DEFAULT_BIQUADRATIC_COEFFS);
   ```

4. **Capacity and EIR curves in `heater.rs:866-883` and `917-934`** — these evaluate `cap_ratio` and `eir_ratio_base` via `evaluate_biquadratic_with_flow`. With identity coefficients, both always return 1.0.

5. **Test code explicitly uses identity curves** — `heater.rs:1700-1704`:
   ```rust
   fn add_identity_biquadratic_curves(cfg: &mut EquipmentConfig) {
       cfg.test_extras_mut().insert(
           "biquadratic_coeffs".to_string(),
           "[[1,0,0,0,0,0],[1,0,0,0,0,0]]".into(),
       );
   }
   ```

6. **OCHRE biquadratic CSVs exist** and have been ported to HARES defaults, but they are only loaded when the config resolver explicitly provides curve data. The equipment init path does NOT auto-load them.

7. **HARES defaults directory** contains the CSVs but they are not wired into the equipment init:
   - `/home/rich/src/HARES/defaults/hvac_heating/ASHP Heater.csv` (has single/double/variable speed curves)
   - `/home/rich/src/HARES/defaults/hvac_heating/MSHP Heater.csv` (has single/variable speed curves)
   - `/home/rich/src/HARES/defaults/hvac_heating/Heat Pump Heater.csv`

## Required Behavior

The biquadratic formula:

```
f(x1, x2) = c0 + c1*x1 + c2*x1² + c3*x2 + c4*x2² + c5*x1*x2
```

where:
- **Heating**: `x1 = T_indoor_DB [°C]`, `x2 = T_outdoor_DB [°C]`
- **Cooling**: `x1 = T_indoor_WB [°C]`, `x2 = T_outdoor_DB [°C]`

MUST produce physically correct temperature-dependent capacity and EIR ratios. At minimum:

1. **Heating capacity at 8.3°C DB outdoor / 21.1°C DB indoor (AHRI H1 condition per AHRI 210/240-2023 Table 9) must equal rated capacity**: `cap_ratio ≈ 1.0`. This is the correct correctness check — the rated capacity is defined at H1 conditions, so any real curve set must produce `cap_ratio ≈ 1.0` there.
2. **Heating capacity at -8.3°C (17°F) OAT must be ~0.50–0.70 of rated**: this is the H2/H3 degradation per AHRI 210/240
3. **Heating COP at -8.3°C must be reduced**: EIR curve must increase at low OAT (worse efficiency)
4. **Cooling capacity must degrade at extreme outdoor temperatures** (>35°C): `cap_ratio < 1.0`

### Default curve coefficients by equipment type

**ASHP Heater (single-speed heating)** from OCHRE/HARES `ASHP Heater.csv` column `Single_1`:
- Capacity: `[0.878143655, -0.002914855, -0.00003337, 0.022386661, 0.000163944, -0.00002187]`
- EIR: `[0.716518071, 0.010275901, 0.000460734, -0.006480365, 0.000456354, -0.00069764]`

**MSHP Heater (variable-speed heating)** from OCHRE/HARES `MSHP Heater.csv` column `Variable_1`:
- Capacity: `[1.002928121, -0.010386676, 0.0, 0.025961538, 0.0, 0.0]`
- EIR: `[0.966475473, 0.00591495, 0.000191202, -0.012965668, 0.00004225, -0.000524003]`

These are already in `/home/rich/src/HARES/defaults/hvac_heating/` — they just need to be wired in.

**CSV bounds verification**: `defaults/hvac_heating/ASHP Heater.csv` rows `min_Twb`, `max_Twb`, `min_Tdb`, `max_Tdb` are all ±100°C for every speed column (confirmed by inspection). These wide bounds will be tightened to physically appropriate ranges by ticket 003; until then, the curves are evaluated without clamping.

## Approach

1. **Add equipment-type-aware default curves** to `HvacEquipmentType` or a new helper. When `biquadratic_coeffs` is still `[1,0,0,0,0,0]` after init, replace with the appropriate default from the OCHRE-derived CSV data, keyed by `HvacEquipmentType` and speed count.

2. **Create a `DefaultBiquadraticCurves` module** (or extend the existing CSV loader) that maps `HvacEquipmentType × number_of_speeds → (cap_coeffs, eir_coeffs)`. Embed the single-speed ASHP/MSHP defaults as constants to avoid runtime file I/O; multi-speed defaults can be loaded from the CSV files in `/home/rich/src/HARES/defaults/hvac_heating/`.

3. **Modify `HvacEquipment::init()`** (`hvac_core.rs:328`) to apply default curves after loading `biquadratic_coeffs` from config. If the loaded coefficients equal `DEFAULT_BIQUADRATIC_COEFFS` and no explicit `capacity_biquadratic_coeffs` / `eir_biquadratic_coeffs` were provided, substitute the equipment-type defaults.

4. **Update test helper** in `heater.rs:1700-1704` — `add_identity_biquadratic_curves` should be replaced by a helper that loads the default curves, or tests should explicitly verify that defaults produce non-identity results.

5. **Add telemetry field** `biquadratic_curve_source` ("user" | "default" | "identity") so downstream consumers can audit which curve set is active.

6. **Update `biquadratic_x1_bounds` / `biquadratic_x2_bounds` defaults** from ±100°C to narrower, physically appropriate ranges per equipment type (ticket 003 already covers this partially).

## HPXML Wiring

When HPXML provides biquadratic curves (via `Curve:Biquadratic` objects or
the defaults CSV loaded by `apply_multispeed_parameters`), those take
precedence over the identity defaults. The default substitution only occurs
when no curves are provided via HPXML or config. The `resolve_hvac.rs`
function `apply_multispeed_parameters` already loads curves from defaults CSV
for multi-speed equipment. Ticket 010 ensures single-speed equipment also
gets non-identity defaults when no curves are provided.

## Definition of Done

- [ ] ASHP heater with no user-supplied curves produces `cap_ratio < 0.8` at OAT = -8.3°C (17°F)
- [ ] MSHP heater with no user-supplied curves produces `cap_ratio < 0.8` at OAT = -8.3°C
- [ ] Heating EIR ratio increases (worse efficiency) at low OAT with default curves
- [ ] Existing tests that explicitly pass identity curves still pass
- [ ] New tests verify default curve values match OCHRE CSV data for single-speed ASHP
- [ ] Annual heating energy in a cold-climate simulation differs by >15% from identity-curve baseline
- [ ] Telemetry reports curve source (default vs user-supplied)
- [ ] `tracing::info!` when default curves are substituted for identity curves: `"substituting default biquadratic curves for {equipment_type:?}; original was identity"`
- [ ] Telemetry key `BIQUADRATIC_CURVE_SOURCE` added (values: "user", "default", "identity")
- [ ] Telemetry key `CAP_RATIO` added at current conditions for verifying curve produces non-trivial values
- [ ] Auto-loading path errors loudly (panics or returns `Err`) if no curve is available for a known equipment type; no silent substitution of identity coefficients as a runtime fallback

## Verification

1. **Unit test**: Construct `ASHPHeater` with no biquadratic config; evaluate `hvac.evaluate_biquadratic(0, 20.0, -8.3)` and assert `cap_ratio < 0.8`.
2. **Unit test**: Same for MSHP Heater.
3. **Unit test**: Verify default EIR curve produces `eir_ratio > 1.0` at OAT = -8.3°C.
4. **Regression test**: Run the existing heater test suite; all pass.
5. **Integration test**: Run a 1-year simulation in Climate Zone 5/6; compare total heating energy against identity-curve baseline. Expected: significant energy difference confirming the fix is meaningful (capacity correction at -8.3°C should be 0.60-0.70 with default curves vs 1.0 with identity).
6. **Spot-check**: At AHRI H1 conditions (OAT=8.3°C DB, indoor=21.1°C DB — AHRI 210/240-2023 Table 9), default curves must produce `cap_ratio ≈ 1.0` (within 5%).

## References

- EnergyPlus Engineering Reference, *DX Heating Coil* subsection for
`Coil:Heating:DX:SingleSpeed`, equations for temperature correction using
`Curve:Biquadratic` objects. See also I/O Reference for
`Coil:Heating:DX:SingleSpeed` fields
`Heating_Capacity_Function_of_Temperature_Curve_Name` and
`Heating_EIR_Function_of_Temperature_Curve_Name`. Default curves are in
EnergyPlus `DataSets/DXCoils/` directory.
- ~~Cutler, B., et al. (2013). *Improved Control Strategies for Residential Heat Pump Systems*. NREL/TP-5500-57501.~~ **Corrected:** Cutler, D., Winkler, J., Kruis, N., Christensen, C., Brandemuehl, M. (2013). *Improved Modeling of Residential Air Conditioners and Heat Pumps for Energy Calculations*. NREL/TP-5500-56354. National Renewable Energy Laboratory.
- AHRI Standard 210/240-2023: H1/H2/H3 test conditions and performance rating. Table 9 specifies H1 condition as 8.3°C DB outdoor / 21.1°C DB indoor — rated capacity is defined at this point, so `cap_ratio ≈ 1.0` at H1 is the primary correctness check for any biquadratic capacity curve.
- OCHRE `HVAC.py:804-846`: `initialize_biquad_params` loads per-speed curves from CSV
- OCHRE `ochre/defaults/HVAC Heating/Biquadratic Heat Pump Heater.csv`: source curve data
- HARES `defaults/hvac_heating/ASHP Heater.csv`: ported curve data (same coefficients)

## Related Tickets

- [003-tighten-biquadratic-default-bounds.md](003-tighten-biquadratic-default-bounds.md) — see Dependencies above; must ship together
- [011-discrete-defrost-cycle.md](011-discrete-defrost-cycle.md) — defrost capacity/EIR model
- [012-heating-side-shr.md](012-heating-side-shr.md) — see Dependencies above; 012 verification requires this ticket

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] **`hvac_core.rs:23`** — `DEFAULT_BIQUADRATIC_COEFFS: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]` — confirmed, identity coefficients exactly as described.
- [x] **`hvac_core.rs:294`** — `biquadratic_coeffs: vec![DEFAULT_BIQUADRATIC_COEFFS]` — confirmed in `HvacEquipment::new()`.
- [x] **`hvac_core.rs:811-829`** — `evaluate_biquadratic` falls back through `.get()` → `.last()` → `DEFAULT_BIQUADRATIC_COEFFS` — confirmed, logic matches ticket description exactly.
- [x] **`heater.rs:866-883` and `917-934`** — `evaluate_biquadratic_with_flow` called for `cap_ratio` (curve index `speed*2`) and `eir_ratio_base` (curve index `speed*2+1`) — confirmed at those line ranges.
- [x] **`heater.rs:1700-1704`** — `add_identity_biquadratic_curves` defined and called from `heater_config_with` at line 1716 — confirmed, all tests use identity curves.
- [x] **OCHRE cross-check** — `vendors/OCHRE/ochre/Equipment/HVAC.py:804-846` `initialize_biquad_params` loads per-speed CSV curves and raises `OCHREException` if params are not found (no silent fallback to identity). HARES diverges: it silently uses `DEFAULT_BIQUADRATIC_COEFFS` when no curves are configured.
- [x] **HARES defaults CSVs** — `/Users/rich/source/HARES/defaults/hvac_heating/ASHP Heater.csv`, `MSHP Heater.csv`, `Heat Pump Heater.csv` all exist with correct coefficients. They are NOT auto-loaded during equipment init.

### Coefficient Verification (hand-computed)

ASHP Single_1 `cap_t` from CSV: `[0.878143655, -0.002914855, -0.00003337, 0.022386661, 0.000163944, -0.00002187]`

| Condition | x1 (indoor DB) | x2 (outdoor DB) | cap_ratio |
|-----------|---------------|-----------------|-----------|
| AHRI H1   | 21.1°C        | 8.3°C           | **0.9951** (≈ 1.0 ✓) |
| AHRI H3   | 21.1°C        | −8.3°C          | **0.6311** (< 0.8 ✓) |
| Identity  | any           | any             | **1.0000** (BUG) |

ASHP EIR at H1: 0.9939; at H3: 1.3459 (H3 > H1 → worse efficiency at low OAT ✓)

MSHP Variable_1 `cap_t` from CSV: `[1.002928121, -0.010386676, 0.0, 0.025961538, 0.0, 0.0]`

| Condition | cap_ratio |
|-----------|-----------|
| AHRI H1   | **0.9993** (≈ 1.0 ✓) |
| AHRI H3   | **0.5683** (< 0.8 ✓) |

The OCHRE-sourced coefficients in HARES defaults are **identical** to those in `vendors/OCHRE/ochre/defaults/HVAC Heating/Biquadratic ASHP Heater.csv` (cross-verified by byte-for-byte comparison).

### Web-Verified Citations

**Citation 1: EnergyPlus `Coil:Heating:DX:SingleSpeed` biquadratic curve**

- **Ticket claims**: EnergyPlus uses biquadratic curves for heating capacity and EIR as functions of temperature for `Coil:Heating:DX:SingleSpeed`.
- **Source found**: DesignBuilder EnergyPlus help (HeatingCoilDX.htm); building-simulation-data.com IDD explorer for `COIL:HEATING:DX:SINGLESPEED`; BigLadder EnergyPlus docs.
- **Quoted passage** (from DesignBuilder `HeatingCoilDX.htm`): *"The curve 'parameterises the variation of the total heating capacity as a function of both the indoor and outdoor air dry-bulb temperature' for bi-quadratic curves. The curve is 'normalised to have the value of 1.0 at the rating point,' which corresponds to: outdoor air dry-bulb of 8.33°C, outdoor wet-bulb of 6.11°C, coil entering air dry-bulb of 21.11°C."*
- **Quoted passage** (from building-simulation-data.com IDD): The biquadratic equation is `a + b*iat + c*iat**2 + d*oat + e*oat**2 + f*iat*oat` where `iat = indoor air dry-bulb (°C)` and `oat = outdoor air dry-bulb (°C)`.
- **Verdict**: **Confirmed**. The ticket's formula description (`c0 + c1*x1 + c2*x1² + c3*x2 + c4*x2² + c5*x1*x2` with x1=indoor DB, x2=outdoor DB for heating) matches EnergyPlus exactly. The normalization point (8.33°C outdoor / 21.11°C indoor) matches the ticket's H1 claim.

**Citation 2: AHRI Standard 210/240-2023, Table 9, H1 condition**

- **Ticket claims**: AHRI 210/240-2023 Table 9 specifies H1 as 8.3°C DB outdoor / 21.1°C DB indoor; H3 at −8.3°C outdoor. Rated capacity defined at H1.
- **Source found**: Multiple AHRI 210/240 standard documents retrieved (2003, 2017 versions; 2023 version returned 403 Forbidden). U.S. DOE rulemaking document (EERE-2022-BT-TP-0028-0017) returned 403 Forbidden. AHRI Search Standards page confirmed H1/H2/H3 test structure.
- **Quoted passage** (from web search aggregate): *"Three tests must be conducted: the high temperature (H1) test, the frost accumulation (H2) test, and the low temperature (H3) test. The H1 test specifies air entering the outdoor unit at 47.0°F (8.33°C) dry-bulb and air entering the indoor unit at 70.0°F (21.1°C) dry-bulb."* (ANSI/AHRI 210/240-2008 consistent with all versions checked).
- **Quoted passage** (from DesignBuilder EnergyPlus, confirming normalization): *"normalised to have the value of 1.0 at the rating point … outdoor air dry-bulb of 8.33°C … coil entering air dry-bulb of 21.11°C"* — confirming this is the H1 point.
- **Verdict**: **Confirmed**. H1 at 8.3°C outdoor / 21.1°C indoor is consistent with all AHRI 210/240 versions reviewed. Table 9 is cited as the table number in the 2023 edition, which cannot be verified (document access blocked); the conditions themselves are confirmed correct across versions. The specific claim that H2 is "frost accumulation" and H3 is "low temperature (17°F / −8.3°C)" is consistent with industry documentation.
- **Minor note**: The ticket's citation says "Table 9" which is the 2023-edition table number. The 2017 edition uses different table numbering. The table number itself is plausible but could not be independently verified against the paywalled 2023 PDF. The temperature values are correct.

**Citation 3: NREL/TP-5500-57501 (Cutler et al. 2013)** — **FIXED** by ticket T-0309; References section entry strikethrough-corrected.

- **Ticket claims**: Cutler, B., et al. (2013). *Improved Control Strategies for Residential Heat Pump Systems*. NREL/TP-5500-57501.
- **Source found**: OSTI.GOV biblio/1219902; docs.nrel.gov/docs/fy13osti/56354.pdf
- **Quoted passage** (from OSTI): The 2013 NREL paper by Cutler et al. has report number **NREL/TP-5500-56354**, not 57501. Title is *"Improved Modeling of Residential Air Conditioners and Heat Pumps for Energy Calculations."* Authors are Cutler, D.; Winkler, J.; Kruis, N.; Christensen, C.; Brandemuehl, M.
- **Verdict**: **Incorrect on multiple points** (now fixed — T-0309). (a) The report number is NREL/TP-5500-**56354**, not 57501. (b) The title is "Improved *Modeling*…" not "Improved *Control Strategies*…". (c) The first author is "Cutler, D." not "Cutler, B." The ticket's citation had three independent errors. No NREL document with number TP-5500-57501 was found in any search. The citation has been corrected to: *Cutler, D., et al. (2013). Improved Modeling of Residential Air Conditioners and Heat Pumps for Energy Calculations. NREL/TP-5500-56354.* The References section now has the strikethrough-corrected entry.

**Citation 4: OCHRE `HVAC.py:804-846` `initialize_biquad_params`**

- **Ticket claims**: `vendors/OCHRE/ochre/Equipment/HVAC.py:804-846` loads per-speed curves from CSV.
- **Source found**: Direct file read at `/Users/rich/source/HARES/vendors/OCHRE/ochre/Equipment/HVAC.py`.
- **Quoted passage**: Lines 804–846 are confirmed to contain `def initialize_biquad_params(self, **kwargs)`. The function loads from `f"Biquadratic {self.name}.csv"` and raises `OCHREException` if no params found: `if not biquad_params: raise OCHREException(...)`.
- **Verdict**: **Confirmed** (line numbers accurate, logic matches description).

**Citation 5: OCHRE `ochre/defaults/HVAC Heating/Biquadratic Heat Pump Heater.csv`**

- **Ticket claims**: OCHRE CSV is the source for curve data.
- **Source found**: Direct file read. The relevant OCHRE CSV is actually at `vendors/OCHRE/ochre/defaults/HVAC Heating/Biquadratic ASHP Heater.csv` (not the generic `Biquadratic Heat Pump Heater.csv`). Both exist.
- **Verdict**: **Confirmed** (minor filename discrepancy — ticket names the generic file; the ASHP-specific file is the more direct source for the coefficients cited in the ticket's Required Behavior section).

### Legitimacy

- **Verdict**: **Legitimate**

**Rationale**: All core claims in the ticket are confirmed by direct code inspection and web-verified standards. (1) `DEFAULT_BIQUADRATIC_COEFFS = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]` is demonstrably identity — confirmed at `hvac_core.rs:23`. (2) Equipment initialization unconditionally uses identity regardless of type — confirmed at `hvac_core.rs:294`. (3) The identity coefficients produce `cap_ratio = 1.0` at all temperatures — confirmed analytically. (4) OCHRE-derived default coefficients for ASHP Single_1 and MSHP Variable_1 exist in HARES's own `defaults/` directory but are not auto-loaded — confirmed by Glob. (5) With the OCHRE coefficients, cap_ratio at AHRI H3 is 0.631 (ASHP) and 0.568 (MSHP), both satisfying the `< 0.8` criterion. (6) EnergyPlus independently confirms the biquadratic formula structure and H1 normalization point. (7) Four new failing regression tests were written and confirmed to fail with the present identity defaults, proving the bug is live.

The one previously inaccurate reference (NREL/TP-5500-57501) has been corrected above (ticket T-0309). See References section for the strikethrough-and-correct entry.

### Proposed Fix Summary

In `HvacEquipment::new()` (`hvac_core.rs:294`), instead of unconditionally assigning `vec![DEFAULT_BIQUADRATIC_COEFFS]`, branch on `equipment_type` to load the OCHRE-derived defaults:

- `AshpHeatPumpOnly` / `AshpHeatPumpAux` → ASHP Single_1 cap and EIR coefficients (from `defaults/hvac_heating/ASHP Heater.csv`)
- `MiniSplitHeat` → MSHP Variable_1 cap and EIR coefficients (from `defaults/hvac_heating/MSHP Heater.csv`)
- All others → continue using `DEFAULT_BIQUADRATIC_COEFFS` (gas furnace, baseboard, etc. do not use biquadratic curves in the heating path)

The coefficients should be embedded as `const` arrays in a new `default_curves.rs` helper, not loaded at runtime from CSV, to avoid I/O in the constructor. The `add_identity_biquadratic_curves` test helper in `heater.rs` should be retained (tests that explicitly want identity behavior should continue to override). The config-loading path that reads `biquadratic_coeffs` from `test_extras` or user config should take precedence over the new defaults (i.e., defaults are applied in `new()`, overridden by `init()`).

### Test Written

- **File**: `crates/hares-equipment/src/hvac/hvac_core.rs` (inside `#[cfg(test)]` module, lines ~3107–3215)
- **Tests added** (4):
  1. `ticket_010_ashp_default_cap_curve_unity_at_ahri_h1` — asserts `AshpHeatPumpOnly` default cap curve produces `cap_ratio ≈ 1.0 ± 5%` at AHRI H1 (8.3°C OAT / 21.1°C indoor). **Currently FAILS** (identity coefficients ≠ non-identity).
  2. `ticket_010_ashp_default_cap_curve_below_08_at_ahri_h3` — asserts cap_ratio < 0.8 at −8.3°C OAT. **Currently FAILS** (identity returns 1.0).
  3. `ticket_010_ashp_default_eir_curve_increases_at_low_oat` — asserts EIR at H3 > EIR at H1 and EIR at H3 > 1.0. **Currently FAILS**.
  4. `ticket_010_mshp_default_cap_curve_below_08_at_ahri_h3` — asserts `MiniSplitHeat` cap_ratio < 0.8 at −8.3°C OAT. **Currently FAILS**.

Confirmed: `cargo test -p hares-equipment --lib -- ticket_010` → 0 passed, 4 failed, 102 other hvac_core tests unaffected.
