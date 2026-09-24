# EV charging strategy priority and interaction
**Review ID**: equip-der-03
**Category**: equipment-der
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/ev/mod.rs` (lines 1–1076)
- `crates/hares-equipment/src/ev/charging_curve.rs` (lines 1–336)
- `crates/hares-equipment/src/ev/config.rs` (lines 1–334)
- `crates/hares-core/src/actors/ev_driver/mod.rs` (lines 1–200)
- `crates/hares-core/src/actors/ev_driver/composer.rs` (lines 1–390)
- `crates/hares-core/src/actors/ev_driver/departure.rs` (lines 1–436)
- `crates/hares-core/src/actors/ev_driver/time_window.rs` (lines 1–156)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/EV.py` (lines 1–371)

## Constraint Priority Architecture

The HARES EV charging priority spans two layers: a **scheduler layer** (`ev_driver` actor with `ChargingComposer`) and an **equipment layer** (`Ev::compute_charging_power_kw`). The scheduler evaluates `ChargingPreference` instances per timestep and emits `ControlSignal` variants; the equipment applies hard BMS limits. The priority chain is:

### Scheduler Layer (`composer.rs:40-78`)
1. **Constraint checks** (first `Override` wins, short-circuits)
2. **Score collection** from all non-overriding preferences
3. **Vote resolution** (highest-scored target_soc, most conservative power_kw, max min_soc, earliest departure)
4. **Signal emission** (departure_hour → `EvSetReadyBy`; power_kw → `PowerSetpoint`; target_soc → `SOCTarget`)

### Equipment Layer (`mod.rs:378-455`)
1. **V2G/V2L discharge**: negative `power_setpoint_kw` → immediate return (lines 385–394)
2. **SOC limit**: `soc >= soc_limit` → return 0 (lines 396–413)
3. **PowerSetpoint**: skips `bms_ready_by_power` (line 440)
4. **Ready‑By BMS**: `bms_ready_by_power` calculates charge/delay decision (lines 440–442)
5. **Taper limit**: `(soc_limit - soc) * capacity / dt / efficiency` (lines 444–446)
6. **PowerLimit**: final cap (lines 449–453)

---

## Findings

### Finding 1: CC_CV_MARGIN applied as flat derating — not time‑dependent power profile — and double‑counts with charging‑curve LUT
**Severity**: high

**Description**: The constant `CC_CV_MARGIN = 0.85` at `mod.rs:57` is used in `bms_ready_by_power` at line 473 as a flat multiplier on effective charging power:

```rust
let eff_power = derated_rated * self.charging_efficiency * CC_CV_MARGIN;
let hours_needed = soc_deficit * self.battery_capacity_kwh / eff_power;
```

Real CC‑CV charging delivers *constant* power through the constant‑current (CC) region up to approximately 85% SOC, then *tapers* power as current is reduced in the constant‑voltage (CV) region. The flat 0.85 multiplier:

1. **Derates the entire charge curve** — it reduces effective power by 15% even from 20%→85% SOC, where real power is constant and at maximum.
2. **Double‑counts the CV taper** when a charging‑curve LUT is loaded. The `ChargingCurveLut` / PyBaMM LUT (applied at `mod.rs:420–431`) is a 4‑D interpolator over `[SOC, temperature, C‑rate, SOH]` that already captures the electrochemical power roll‑off at high SOC. When present, the LUT reduces `derated_rated` for high SOC, then `CC_CV_MARGIN` reduces it further — applying the same physical effect twice.
3. **Can underestimate charge time at extreme SOCs** when no LUT is present (see example below).

**Code Location**:
- `CC_CV_MARGIN` constant: `mod.rs:57`
- Application in charge‑time estimate: `mod.rs:473`
- LUT application (precedes the margin): `mod.rs:420–431`

**Impact**: When a LUT is present, `hours_needed` is overestimated, causing Ready‑By charging to start earlier than physically necessary (wastes TOU‑optimal scheduling). When no LUT is present and SOC starts above ~85%, the flat derating may *underestimate* charge time (the CV region taper is far more severe than 15% near the top of charge), risking a missed departure SOC.

**Example** (no LUT, 60 kWh battery, 7.2 kW, 0.9 eff, SOC 0.88→0.90):
- Real: entirely in CV region, average power ≈ 3.5 kW → charge time ≈ 0.34 h
- With CC_CV_MARGIN: effective power = 7.2 × 0.9 × 0.85 = 5.51 kW → charge time ≈ 0.22 h
- **Under‑estimate of 35%** — the model thinks it needs less time than it actually does.

**Remediation**: Model the CC‑CV transition as a piecewise function: constant power up to a chemistry‑specific transition SOC (e.g., 0.85 for NMC), then a power‑fraction curve (from LUT or empirical formula) above that threshold. If the LUT already provides this curve, remove the redundant `CC_CV_MARGIN` multiplier entirely when a LUT is present.

---

### Finding 2: PowerSetpoint silently bypasses Ready‑By deadline logic at equipment layer
**Severity**: high

**Description**: In `compute_charging_power_kw` (line 440):

```rust
if self.ready_by_hour.is_some() && self.power_setpoint_kw.is_none() {
    requested = self.bms_ready_by_power(now, derated_rated, soc_limit);
}
```

The BMS‑level Ready‑By deadline enforcement (`bms_ready_by_power`) is **only invoked when no PowerSetpoint is active**. If an external controller sends a `PowerSetpoint` signal (for TOU avoidance, price‑responsive dispatch, or any other reason), the equipment’s own deadline urgency logic is completely bypassed. The external controller bears **sole responsibility** for meeting the Ready‑By departure SOC — the EV equipment will not override the controller’s power setpoint even if a missed deadline is imminent.

This is in contrast to `PowerLimit`, which is applied as a final cap **after** Ready‑By logic has determined the requested power (lines 449–453). An external PowerLimit cannot prevent the equipment from complying with the Ready‑By deadline, but an external PowerSetpoint can.

**Code Location**: `mod.rs:385–394` (V2G/V2L gate), `mod.rs:434–438` (setpoint override), `mod.rs:440–442` (Ready‑By bypass condition)

**Comparison with OCHRE**: OCHRE’s `ElectricVehicle.calculate_power_and_heat` (`EV.py:290–313`) uses a simple `min(max(p_setpoint, 0), soc_max_power)` with no deadline concept — the two models are not directly comparable on this point. OCHRE has no equivalent deadline override mechanism.

**Impact**: An external controller unaware of this bypass could inadvertently cause missed departure SOCs. The behaviour is by design (external setpoint = external responsibility), but it is undocumented and non‑obvious from the API.

---

### Finding 3: DepartureDeadline Override’s `power_kw` silently dropped — scheduler urgency intent not propagated to equipment
**Severity**: high

**Description**: When `DepartureDeadline.constraint()` fires an `Override` due to urgency (buffer window or safety margin), it constructs a vote that includes `power_kw: Some(ctx.max_charge_kw)` to force max‑rate charging (`departure.rs:84–92`, `departure.rs:97–104`). However, in `ChargingComposer::emit_vote` (`composer.rs:150–193`), the presence of `departure_hour` in the vote takes precedence:

```rust
// Line 152: departure_hour checked first → emits EvSetReadyBy, returns
if let (Some(departure), Some(target)) = (vote.departure_hour, vote.target_soc) {
    out.push(DispatchRequest { signal: ControlSignal::EvSetReadyBy { ... }, ... });
    return;  // <-- power_kw is discarded here
}
```

The `power_kw` field is **silently discarded**. The equipment receives `EvSetReadyBy` but no `PowerSetpoint`, so the equipment’s own `bms_ready_by_power` independently decides whether to charge at full rate. The scheduler’s urgency margin (1.2× safety factor at `departure.rs:96`) and the equipment’s urgency calculation (CC_CV_MARGIN = 0.85 at `mod.rs:473`) are **different formulas** applied to **different power values** (`ctx.max_charge_kw` vs. `derated_rated`), creating a decoupled two‑stage decision with no guarantee of consistency.

**Code Location**: Override construction: `departure.rs:84–92` (buffer), `departure.rs:97–104` (safety margin); Silent drop: `composer.rs:152–161`

**Impact**: The scheduler “thinks” it’s forcing max‑rate charging via Override, but the equipment independently applies its own (different) urgency formula. In edge cases where only one of the two systems considers the deadline urgent, the charging behaviour may not match the scheduler’s intent.

---

### Finding 4: TOU‑avoidance `TimeWindowPref` prevents Ready‑By override for `Nightly` strategy
**Severity**: medium

**Description**: The `Nightly` charging strategy builds a preference stack of `[TimeWindowPref, SocTarget]` (`mod.rs:86–99`). `TimeWindowPref.constraint()` returns `Constraint::Override` with an idle vote when outside the off‑peak window (`time_window.rs:36`), which causes the `ChargingComposer` to emit nothing — **no signal reaches the equipment**. The `DepartureDeadline` preference is absent from the `Nightly` stack, so there is **no path for Ready‑By urgency to override the time‑window gate**.

For the `TouAware` strategy, by contrast, the stack is `[PriceOptimizer, DepartureDeadline, SocTarget]` — `DepartureDeadline` does check its constraint after `PriceOptimizer` and can override TOU‑based pricing decisions as the deadline approaches.

**Code Location**:
- `Nightly` preference build: `mod.rs:86–99`
- `TimeWindowPref` constraint: `time_window.rs:33–39`
- `TouAware` preference build (with `DepartureDeadline`): `mod.rs:132–153`

**Note**: An external controller CAN still send `EvSetReadyBy` directly to a `Nightly` EV, and the equipment’s `bms_ready_by_power` WILL respect it (including during TOU window blocks). The gap is only in the built‑in `ev_driver` actor.

**Impact**: `Nightly`‑configured EVs will never automatically override their off‑peak window to meet a departure deadline. Users relying on the built‑in actor must use `TouAware` for deadline‑aware scheduling.

---

### Finding 5: V2L/V2G discharge reuses charging efficiency — no separate inverter discharge efficiency
**Severity**: medium

**Description**: Both `compute_v2l_discharge` (`mod.rs:498–508`) and `compute_v2g_discharge` (`mod.rs:510–520`) compute the return value as negative AC power at the grid interface. In `apply_soc_and_thermal` (line 640), the DC energy consumed from the battery is calculated as:

```rust
let dc_discharge_kwh = (ac_discharge_kw / self.charging_efficiency.max(0.01)) * dt_hours;
```

This divides by `self.charging_efficiency` (an AC→DC onboard charger metric), assuming the inverter’s **DC→AC** efficiency equals its **AC→DC** efficiency. Bidirectional (V2G‑capable) inverters typically have different efficiency curves for rectification and inversion, and separate ratings for each direction.

**Code Location**: `mod.rs:640` (DC discharge energy), `mod.rs:498–508` (V2L), `mod.rs:510–520` (V2G)

**Comparison with OCHRE**: OCHRE has no V2L or V2G support — only unidirectional charging.

**Impact**: SOC depletion during discharge may be slightly over‑ or under‑estimated depending on actual inverter losses. For simulation purposes this is a modest accuracy concern, but integration with hardware‑in‑the‑loop or digital twin use cases would benefit from a separate `discharge_efficiency` parameter.

---

### Finding 6: V2L/V2G discharge does not consider Ready‑By deadline requirements
**Severity**: medium

**Description**: The V2L and V2G discharge paths gate discharge solely on the SOC reserve floor (`v2l_soc_reserve` / `v2g_soc_reserve`) at lines 499 and 511. There is **no check** against `ready_by_soc` or any forward‑looking deadline constraint. A V2H‑configured EV could discharge down to its reserve (default 20%) even if a Ready‑By deadline at 07:00 requires 90% SOC and there is insufficient time to recharge afterward.

The V2G `V2GExport` preference (`strategy.rs`) has its own `min_soc` field, but this operates at the scheduler layer and is independent of the equipment‑level `ready_by_soc`.

**Code Location**: `mod.rs:498–508` (V2L discharge floor), `mod.rs:510–520` (V2G discharge floor)

**Impact**: Discharge during evening hours could compromise next‑morning departure readiness. A safety interlock that raises the effective discharge floor to `max(reserve, ready_by_soc)` when a deadline is pending would prevent this.

---

### Finding 7: OCHRE has no equivalent multi‑strategy priority model
**Severity**: low

**Description**: The OCHRE `ElectricVehicle` model (`EV.py:290–313`) handles constraints with a single priority rule: `ac_power = min(max(p_setpoint, 0), soc_max_power)`. It has:
- No Ready‑By / departure deadline concept
- No TOU window or price‑based avoidance
- No V2L / V2G discharge paths
- No temperature derating or charging curve LUT
- No degradation tracking

The HARES model adds significant capability beyond the OCHRE reference, and the constraint‑interaction findings above are unique to the HARES implementation. OCHRE cannot serve as a reference for correctness of these interactions.

---

## Summary
- **Total findings**: 7
- **Critical**: 0
- **High**: 3 (Findings 1, 2, 3)
- **Medium**: 3 (Findings 4, 5, 6)
- **Low**: 1 (Finding 7)

## Recommendations

1. **Replace flat CC_CV_MARGIN with SOC‑dependent taper model** (`mod.rs:473`). When a charging‑curve LUT is present, remove the 0.85 multiplier entirely — the LUT already captures the power roll‑off. When no LUT is present, apply the margin only above a chemistry‑specific transition SOC (e.g., 0.85 for NMC), scaling the derating linearly from 1.0 at the transition SOC to a minimum at 1.0 SOC.

2. **Revisit PowerSetpoint / Ready‑By interaction** (`mod.rs:440`). Either: (a) document that an external PowerSetpoint fully bypasses deadline enforcement and the controller bears sole responsibility; or (b) allow `bms_ready_by_power` to raise the power setpoint above the external setpoint when a deadline is imminent (treating the external setpoint as a soft floor, not a hard ceiling, during urgent deadlines).

3. **Fix the DepartureDeadline Override power propagation** (`composer.rs:150–193`). When a vote has both `departure_hour` and `power_kw`, emit both `EvSetReadyBy` and a separate `PowerSetpoint` dispatch, or introduce a combined signal variant that sets the deadline and specifies a charging rate.

4. **Add `discharge_efficiency` parameter** (`mod.rs`, `config.rs`). Introduce a separate bidirectional inverter efficiency for the DC→AC path, defaulting to the same value as `charging_efficiency` for backward compatibility.

5. **Couple V2L/V2G discharge floor to Ready‑By deadline** (`mod.rs:498, 511`). When `ready_by_hour` is active, raise the effective discharge reserve to `max(soc_reserve, ready_by_soc)` so discharge cannot compromise departure readiness.

6. **Add `DepartureDeadline` to `Nightly` strategy** or document the limitation. The `Nightly` strategy is strictly window‑gated and cannot automatically meet deadlines; users needing both must use `TouAware`.

## References / Citations
- OCHRE EV model: `vendors/OCHRE/ochre/Equipment/EV.py:290–313` — `calculate_power_and_heat` priority logic
- OCHRE control: `vendors/OCHRE/ochre/Equipment/EV.py:236–272` — external control signal handling
- HARES CC‑CV margin: `crates/hares-equipment/src/ev/mod.rs:57, 473`
- HARES BMS ready‑by: `crates/hares-equipment/src/ev/mod.rs:457–496`
- HARES charging power computation: `crates/hares-equipment/src/ev/mod.rs:378–455`
- HARES composer priority: `crates/hares-core/src/actors/ev_driver/composer.rs:40–78`
- HARES departure deadline: `crates/hares-core/src/actors/ev_driver/departure.rs:68–109`
- HARES time window constraint: `crates/hares-core/src/actors/ev_driver/time_window.rs:32–39`
- HARES charging curve LUT: `crates/hares-equipment/src/ev/charging_curve.rs:8–12`
- HARES V2L/V2G discharge: `crates/hares-equipment/src/ev/mod.rs:498–520`
- HARES SOC application for discharge: `crates/hares-equipment/src/ev/mod.rs:636–641`
