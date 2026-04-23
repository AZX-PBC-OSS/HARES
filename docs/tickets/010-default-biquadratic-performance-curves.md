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
- Cutler, B., et al. (2013). *Improved Control Strategies for Residential
Heat Pump Systems*. NREL/TP-5500-57501. National Renewable Energy Laboratory.
- AHRI Standard 210/240-2023: H1/H2/H3 test conditions and performance rating. Table 9 specifies H1 condition as 8.3°C DB outdoor / 21.1°C DB indoor — rated capacity is defined at this point, so `cap_ratio ≈ 1.0` at H1 is the primary correctness check for any biquadratic capacity curve.
- OCHRE `HVAC.py:804-846`: `initialize_biquad_params` loads per-speed curves from CSV
- OCHRE `ochre/defaults/HVAC Heating/Biquadratic Heat Pump Heater.csv`: source curve data
- HARES `defaults/hvac_heating/ASHP Heater.csv`: ported curve data (same coefficients)

## Related Tickets

- [003-tighten-biquadratic-default-bounds.md](003-tighten-biquadratic-default-bounds.md) — see Dependencies above; must ship together
- [011-discrete-defrost-cycle.md](011-discrete-defrost-cycle.md) — defrost capacity/EIR model
- [012-heating-side-shr.md](012-heating-side-shr.md) — see Dependencies above; 012 verification requires this ticket
