# Ventilation bypass and defrost derating propagation to solver
**Review ID**: equip-loads-01
**Category**: equipment-loads
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/ventilation.rs` — ventilation equipment (HRV/ERV), effectiveness computation, trait method
- `crates/hares-envelope/src/thermal_solver/infiltration.rs` — solver-side infiltration/ventilation coupling, recovery efficiency consumption

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/HeatRecovery.cc` — EnergyPlus `HeatExchCond::FrostControl()` (lines 2811–3064), defrost fraction computation, per-timestep bypass+derating logic

## Findings

### Finding 1: Defrost model is a binary step, not temperature-dependent like EnergyPlus [Severity: medium]

**Description**: HARES uses a hard threshold (`defrost_temp_c`, default -5°C) with a constant derating multiplier (`defrost_effectiveness_fraction`, default 0.5). Below the threshold the rated effectiveness is multiplied by 0.5 regardless of how far below threshold the outdoor temperature is. EnergyPlus computes a continuous defrost fraction that depends on how far below the threshold temperature conditions fall.

**Code Location**:
- `crates/hares-equipment/src/ventilation.rs:262-268` (`compute_effective_sensible_effectiveness`)
- `crates/hares-equipment/src/ventilation.rs:279-284` (`compute_effective_latent_effectiveness`)

**Root Cause**: The HARES defrost model is deliberately simplified for residential energy audits. The EnergyPlus model has three distinct frost-control strategies (`ExhaustOnly`, `ExhaustAirRecirculation`, `MinimumExhaustTemperature`) each with its own defrost-fraction formula that varies continuously with the temperature deficit below threshold.

EnergyPlus `ExhaustOnly` / `ExhaustAirRecirculation` frost control (HeatRecovery.cc:2996,3034):
```
DFFraction = max(0.0, min(InitialDefrostTime + RateofDefrostTimeIncrease * (Threshold - SupInTemp), 1.0))
```

EnergyPlus `MinimumExhaustTemperature` (HeatRecovery.cc:2908):
```
DFFraction = max(0.0, min(1.0, (Threshold - SecOutTemp) / (SecInTemp - SecOutTemp)))
```

HARES (ventilation.rs:264-265):
```rust
if t_outdoor_c < self.defrost_temp_c {
    base * self.defrost_effectiveness_fraction  // always 0.5 × rated
}
```

**Impact**: For outdoor temperatures just at the defrost threshold (e.g., -6°C), HARES immediately drops effectiveness to 50% of rated, while E+ would apply a small defrost fraction (e.g., DFFraction ≈ 0.05 with typical parameters), producing nearly undiminished recovery. At very cold temperatures (e.g., -20°C), E+ may reach DFFraction = 1.0 (full-time defrost → zero recovery), while HARES still recovers at 50%. This causes:
- **Over-estimation of defrost derating** for temperatures just below threshold
- **Under-estimation of defrost derating** for extremely cold temperatures

### Finding 2: Effective effectiveness is not cleared when ventilation equipment enters OFF state [Severity: medium]

**Description**: When the ventilation equipment is OFF or in `GridEmergency` DR mode, `step()` returns early (ventilation.rs:376-401) without updating `effective_sensible_effectiveness` and `effective_latent_effectiveness`. The stale values from the most recent running timestep persist and are returned by `effective_ventilation_effectiveness()`. The dwelling orchestration in `mod.rs:2562-2567` reads and propagates these stale values to `ThermalSolverConfig.ventilation` before each thermal solver step.

**Code Location**:
- `crates/hares-equipment/src/ventilation.rs:375-401` — early return without clearing effective fields
- `crates/hares-core/src/dwelling/mod.rs:2562-2567` — unconditionally propagates whatever `effective_ventilation_effectiveness()` returns

**Root Cause**: The `step()` method's OFF-path only clears telemetry and electrical ports; it does not reset the cached effectiveness fields to zero. The trait method `effective_ventilation_effectiveness()` (line 525) unconditionally returns the stored tuple without checking the operating mode.

**Impact**: When the HRV/ERV is turned off, the thermal solver continues to apply the last known recovery efficiency (typically 0.70). This means the solver computes a reduced ventilation sensible load as if recovery were still active, under-estimating the actual load. Combined with the fact that `ventilation_flow_m3_s` in the solver config is also not dynamically updated for on/off state (it is set once at init from the rated flow), the solver may apply `rated_flow * (1 - stale_eff) * rho * cp * (T_out - T_zone)` for an equipment that is not running.

### Finding 3: Bypass during defrost not modelled — HARES separates bypass and defrost into disjoint temperature regimes [Severity: low]

**Description**: HARES treats bypass and defrost as mutually exclusive operating modes:
- **Bypass** (free cooling): active when outdoor temp ∈ [18, 24]°C, sets effectiveness to zero (ventilation.rs:259-261)
- **Defrost**: active when outdoor temp < -5°C, reduces effectiveness by a constant fraction (ventilation.rs:264-265)

Because these temperature ranges are disjoint, the two effects never interact. EnergyPlus, by contrast, uses supply air bypass *during* the defrost time period as the primary mechanism for some frost-control strategies (e.g., `ExhaustOnly` at HeatRecovery.cc:3043 sets `SupBypassMassFlow = SupInMassFlow * DFFraction`).

**Code Location**:
- `crates/hares-equipment/src/ventilation.rs:254-261` — bypass check returns 0.0 early, pre-empting defrost check
- `vendors/EnergyPlus/src/EnergyPlus/HeatRecovery.cc:3043` — E+ sets bypass mass flow proportional to defrost fraction

**Root Cause**: HARES' bypass is designed as a *comfort-based free-cooling* feature (not a frost-control mechanism). The comment at line 258 says "Bypass: when outdoor is within comfort range, bypass recovery entirely." This serves a different purpose from E+ frost-control bypass. The two models address different physical phenomena and are not directly comparable.

**Impact**: This is low severity because:
1. In practice, defrost temperatures (< -5°C) and comfort bypass temperatures (≥ 18°C) never overlap, so any interaction would require extremely unusual configuration
2. The constant-effectiveness-derating approach used by HARES produces the same steady-state T_supply as E+'s bypass-and-blend approach when both have the same defrost fraction (mathematically equivalent for `ExhaustOnly` frost control — see verification below)
3. The binary vs. continuous defrost fraction difference (Finding 1) is the dominant accuracy concern, not the bypass/defrost interaction itself

**Verification of equivalence** (for `ExhaustOnly` frost control, DFFraction = defrost_fraction):
- E+ blends: T_supply = T_out + (1-DF) × eff_rated × (T_in - T_out) [mixing core outlet with bypassed outdoor air]
- HARES: T_supply = T_out + (eff_rated × fraction) × (T_in - T_out) [derating effectiveness]
- These are identical when DFFraction = 1 - defrost_fraction (i.e., `fraction` = 1 - DFFraction)

### Finding 4: `save_state`/`load_state` does not persist effective effectiveness across checkpoint/restore [Severity: low]

**Description**: The `VentilationCheckpoint` struct (ventilation.rs:147-152) serialises `mode`, `dr_level`, `dr_duration_remaining_s`, and `schedule_source_state`, but does **not** include `effective_sensible_effectiveness` or `effective_latent_effectiveness`. After a checkpoint restore, the effective fields retain whatever values they held before `load_state()` was called (i.e., defaults from `new()` or rated values from `init_typed()`).

**Code Location**:
- `crates/hares-equipment/src/ventilation.rs:147-152` — `VentilationCheckpoint` struct definition
- `crates/hares-equipment/src/ventilation.rs:541-549` — `load_state()` does not restore effectiveness fields

**Root Cause**: The effectiveness fields are transient, per-timestep values derived from outdoor conditions. Including them in the checkpoint would require an outdoor temperature context to re-derive on restore, or storing the effective values directly. Neither approach is implemented.

**Impact**: Minimal in practice. After restore, `step()` is called before `effective_ventilation_effectiveness()` is read by the orchestration layer (the propagation in `mod.rs:2562` runs *after* equipment step in the dwelling loop). The first `step()` call recomputes and stores fresh values. This would only be a problem if `effective_ventilation_effectiveness()` were called between `load_state()` and the next `step()`, which does not occur in the current dwelling orchestration flow.

### Finding 5: Supply-air and solver formulation are mutually consistent [Severity: low — informational]

**Description**: The ventilation equipment computes supply air conditions directly from effectiveness (ventilation.rs:453-454):
```
T_supply = T_outdoor + eff_s × (T_indoor - T_outdoor)      [E+ equation]
```
The thermal solver treats recovery as a flow reduction (infiltration.rs:212):
```
sensible_flow = nat_flow + forced_flow × (1 - recovery_efficiency)
```
These formulations are mathematically equivalent for the sensible zone heat gain:
```
q = m_dot × cp × (T_supply - T_indoor)
  = m_dot × cp × [T_out + eff×(T_in - T_out) - T_in]
  = m_dot × cp × (1-eff) × (T_out - T_in)
  = ρ × forced_flow × (1-eff) × cp × (T_out - T_in)           [HARES form]
```
No sign or units mismatch is present.

**Code Location**:
- `crates/hares-equipment/src/ventilation.rs:453-454` — T_supply, w_supply computation
- `crates/hares-envelope/src/thermal_solver/infiltration.rs:212-213` — flow-reduction formulation

## Summary
- Total findings: 5
- Critical: 0
- High: 0
- Medium: 2 (Finding 1, Finding 2)
- Low: 3 (Finding 3, Finding 4, Finding 5)

## Recommendations

1. **Replace binary defrost threshold with temperature-dependent defrost fraction.** Adopt EnergyPlus' approach: `defrost_fraction = max(0, min(initial_defrost_time + rate_of_increase × (threshold - T_outdoor), 1))`. This would provide a continuous transition from no derating at the threshold to full defrost at extreme cold. The current config fields `defrost_temp_c` and `defrost_effectiveness_fraction` could be reinterpreted or supplemented with `initial_defrost_time` and `rate_of_defrost_time_increase` parameters matching the E+ input schema.

2. **Reset effective effectiveness to zero when equipment is OFF.** In `Ventilation::step()`, add `self.effective_sensible_effectiveness = 0.0; self.effective_latent_effectiveness = 0.0;` in the early-return OFF path (line 376). Alternatively, check `self.mode` in `effective_ventilation_effectiveness()` and return `(0.0, 0.0)` when mode is `Off` or DR level is `GridEmergency`.

3. **Consider persisting effective effectiveness through checkpoint.** If `load_state()` can be followed by a call to `effective_ventilation_effectiveness()` before the next `step()`, add effectiveness fields to `VentilationCheckpoint` and restore them. If the current orchestration flow guarantees `step()` runs first, document this invariant in a comment.

## References / Citations
- EnergyPlus Engineering Reference, "Heat Exchangers" chapter — per-timestep effectiveness application with frost-control bypass (HeatRecovery.cc:2811–3064)
- `HeatExchCond::FrostControl()`, lines 2811–3064 — three frost-control strategies, defrost fraction computation
- `CalcAirToAirGenericHeatExch()`, lines 2057–2417 — where `FrostControl()` is invoked during simulation
- ASHRAE HoF 2021 Ch.26 — air-to-air energy recovery equipment, frost-control approaches
- ARI Standard 1060 — rating standard for air-to-air heat exchangers (referenced by EnergyPlus)
