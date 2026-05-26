# PV inverter priority mode reactive power limits and PF enforcement
**Review ID**: equip-der-06
**Category**: equipment-der
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/pv/mod.rs`
- `crates/hares-equipment/src/pv/array_config.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/PV.py`

## Findings

### Finding 1: [Severity: medium]
**Description**: Explicit zero-reactive-power command (`ReactiveSetpoint { kvar: 0.0 }`) is indistinguishable from "no Q setpoint," causing the static power factor computation to override the explicit zero command.

**Code Location**: `crates/hares-equipment/src/pv/mod.rs:528`

```rust
let mut reactive_power_kvar = self.q_setpoint_kvar;
if reactive_power_kvar == 0.0 && self.power_factor < 1.0 {
    reactive_power_kvar = total_ac_power_kw * (self.power_factor.acos().tan());
}
```

The guard `reactive_power_kvar == 0.0` cannot distinguish between:
1. No explicit Q setpoint has been applied (initial/default state, `q_setpoint_kvar = 0.0`), and
2. An explicit `ReactiveSetpoint { kvar: 0.0 }` control signal was applied.

**Root Cause**: Both `PowerFactorSetpoint` (mod.rs:683) and `ReactiveSetpoint` (mod.rs:673) write to the same `q_setpoint_kvar` field. `PowerFactorSetpoint` sets `q_setpoint_kvar = 0.0` to clear a prior explicit Q value, but `ReactiveSetpoint { kvar: 0.0 }` produces the same stored value. The step logic in `self.step()` (line 528) interprets any zero as "fall back to PF."

**Impact**: If a controller issues `ReactiveSetpoint { kvar: 0.0 }` to command zero reactive power on a PV system configured with `power_factor < 1.0`, the step method overrides Q with `total_ac_power_kw * tan(acos(pf))` instead of zero. The inverter might export reactive power contrary to the explicit control command.

**Vendor comparison**: OCHRE handles this via an `if-elif` chain in `update_external_control` (PV.py:191–196): `"Q Setpoint"` sets Q directly, `elif "Power Factor"` computes Q from PF. The two are mutually exclusive within the same control signal dictionary. HARES' approach of storing both independently and resolving in `step()` introduces this ambiguity.

---

### Finding 2: [Severity: medium]
**Description**: Minimum power factor enforcement is applied as a post-processing clip, not integrated into the inverter constraint resolution. While this produces correct steady-state boundary values, it can introduce non-physical discontinuities in time-series simulation where `P` varies near the threshold where the PF limit becomes binding.

**Code Location**: `crates/hares-equipment/src/pv/mod.rs:269–320` (`apply_inverter_limits`), specifically lines 285–287 (Watt-priority PF enforcement) and lines 291–302 (Var-priority PF enforcement).

Watt-priority (lines 281–288):
```rust
InverterPriority::Watt => {
    let p_out = p_kw.min(inv_cap);
    let q_max = (inv_cap * inv_cap - p_out * p_out).max(0.0).sqrt();
    let mut q_out = q_kvar.clamp(-q_max, q_max);          // Step A: kVA clamp
    if let Some(min_pf) = self.inverter_min_pf {
        q_out = enforce_min_pf(p_out, q_out, min_pf);     // Step B: PF clamp (post-processing)
    }
    (p_out, q_out)
}
```

The PF constraint is enforced in a second pass (Step B) after the kVA headroom clamp (Step A). If `P` crosses a threshold where `|Q| ≤ P·tan(acos(min_pf))` becomes the binding constraint instead of the kVA headroom, the two-step enforcement produces the same final Q, but the sequential nature means intermediate values (used by any future extension or observer) may temporarily violate the PF constraint before step B corrects it.

**Root Cause**: The priority-mode dispatch executes kVA and PF constraints as sequential clamps rather than solving them as simultaneous constraints. The approach matches OCHRE's `min(abs(q), max_q_capacity, max_q_pf)` pattern (PV.py:227), which also applies PF as a clip. Both implementations inherit the same limitation.

**Impact**: In steady-state operation, the final P and Q correctly satisfy all constraints. However, test frameworks and external controllers that observe intermediate `q_out` between calls could see values that temporarily exceed the PF limit. The behavior is identical to OCHRE and consistent with IEEE 1547-2018 static inverter models, which typically use clipping rather than optimal dispatch.

---

### Finding 3: [Severity: low]
**Description**: In the `step()` method, when reactive power is derived from the static power factor (no explicit Q setpoint), Q is computed using the pre-inverter-clip AC power, which may exceed the inverter AC rating. The `apply_inverter_limits` function later corrects this, but computing Q from the inflated P value is logically suboptimal and produces spurious intermediate reactive power values.

**Code Location**: `crates/hares-equipment/src/pv/mod.rs:527–534`

```rust
let mut reactive_power_kvar = self.q_setpoint_kvar;
if reactive_power_kvar == 0.0 && self.power_factor < 1.0 {
    reactive_power_kvar = total_ac_power_kw * (self.power_factor.acos().tan());
    //                   ^^^^^^^^^^^^^^^^^ pre-clip P may already exceed inverter_capacity_kw
}

let (final_p_kw, final_q_kvar) =
    self.apply_inverter_limits(total_ac_power_kw, reactive_power_kvar);
```

**Root Cause**: The DC-to-AC ratio clipping (`p_kw.min(inv_cap)`) is performed inside `apply_inverter_limits`, but `total_ac_power_kw` at line 529 has already passed through only curtailment and power-limit caps—not inverter capacity clipping. When `total_ac_power_kw > inv_cap`, the computed Q is inflated (`Q_inflated = P_inflated * tan(acos(pf))`), and `apply_inverter_limits` must later clamp it down.

**Impact**: The final P and Q are correct because `apply_inverter_limits` enforces all constraints. However, the inflated intermediate Q value adds unnecessary magnitude to the constraint resolution and makes the code harder to reason about. In Var-priority mode, the `max_q_pf = tan(acos(pf)) * p_kw` computation at line 301 uses this same pre-clip P, but mathematical analysis confirms that the more restrictive absolute Q ceiling (`sin(acos(pf)) * inv_cap` at line 296) is always binding whenever P would later be reduced, so no incorrect final values result.

**Vendor comparison**: OCHRE avoids this issue by having SAM apply the DC-to-AC ratio internally during simulation (PV.py:60: `system_model.value("dc_ac_ratio", capacity / inv_capacity)`). The AC power output from SAM therefore already respects the inverter capacity, and Q is computed from a correctly clipped P. HARES' separation of physics model from inverter model requires more care in ordering.

---

### Finding 4: [Severity: low]
**Description**: In Var-priority mode, the final Q sign is determined from the stored `self.q_setpoint_kvar` field rather than from the `q_kvar` function parameter passed to `apply_inverter_limits`.

**Code Location**: `crates/hares-equipment/src/pv/mod.rs:306–309`

```rust
let q_out = if self.q_setpoint_kvar >= 0.0 {
    q_abs
} else {
    -q_abs
};
```

The parameter `q_kvar` is used for magnitude computation (via `q_abs`), but its sign is discarded in favor of `self.q_setpoint_kvar`. The sign of `q_kvar` is only indirectly relevant because the caller (`step()`) always derives it from `self.q_setpoint_kvar` or from the static power factor (which always produces non-negative Q for P ≥ 0).

**Root Cause**: The function uses a stored field for sign determination instead of the parameter that carries the original sign. This creates a hidden coupling between `apply_inverter_limits` and the stored state.

**Impact**: Currently harmless because the only caller (`step()`) propagates the same `self.q_setpoint_kvar` value. Future refactoring or additional callers of `apply_inverter_limits` could pass a `q_kvar` with a different sign, causing the sign to be silently overridden by the stored `q_setpoint_kvar`.

**Vendor comparison**: OCHRE's Var-priority code uses the same pattern: `q = q if self.q_set_point >= 0 else -q` (PV.py:239). HARES matches OCHRE exactly in this regard.

---

### Finding 5: [Severity: low]
**Description**: The `enforce_min_pf` function returns Q = 0 when P = 0, regardless of the Q setpoint. While physically correct (power factor is undefined at zero real power), this truncates any Q request at zero production, which may be counterintuitive for controllers that expect the inverter to supply reactive power when real power is negligible.

**Code Location**: `crates/hares-equipment/src/pv/mod.rs:323–331`

```rust
fn enforce_min_pf(p: f64, q: f64, min_pf: f64) -> f64 {
    if p == 0.0 {
        return 0.0;                     // Q forced to zero when no real power
    }
    ...
}
```

**Root Cause**: The mathematical relationship `|Q| ≤ P·tan(acos(min_pf))` degenerates at P = 0, producing an unbounded Q range. The code conservatively returns Q = 0.

**Impact**: In edge cases (e.g., nighttime inverter operation), if a controller requests reactive power support from a PV inverter that has zero real power, the PF enforcement nullifies the Q setpoint. This is consistent with IEEE 1547-2018 reactive power priority modes but may diverge from controller expectations. The Var-priority path without `min_pf` (line 304: `q_abs = q_abs.min(inv_cap)`) does NOT have this limitation and allows full kVA for reactive power at zero P.

**Vendor comparison**: OCHRE exhibits the same behavior: `max_q_pf = self.inverter_min_pf_factor * -p` equals zero when `p = 0`, forcing Q to zero through the `min()` operation (PV.py:226–227).

---

## Summary
- **Total findings**: 5
- **Critical / High / Medium / Low**: 0 / 0 / 2 / 3

| # | Severity | Summary |
|---|----------|---------|
| 1 | Medium | Explicit `ReactiveSetpoint { kvar: 0.0 }` overridden by static power factor |
| 2 | Medium | PF enforcement is a post-processing clip; can produce non-physical discontinuities (matches OCHRE) |
| 3 | Low | PF-derived Q computed from pre-inverter-clip P (inefficient, not incorrect) |
| 4 | Low | Var-priority Q sign from stored state rather than function parameter |
| 5 | Low | `enforce_min_pf` returns Q=0 at P=0 (matches OCHRE; may surprise controllers) |

## Recommendations

1. **Distinguish explicit zero Q from "no Q setpoint"** (Finding 1): Add a boolean flag (e.g., `q_setpoint_active: bool`) that is set to `true` by `ReactiveSetpoint` and cleared by `PowerFactorSetpoint`. In `step()`, check the flag before falling back to PF-based Q. Alternatively, reorder the logic so `PowerFactorSetpoint` writes to `power_factor` without clearing `q_setpoint_kvar`, and use a separate mechanism to indicate which source is active.

2. **Consider simultaneous constraint enforcement** (Finding 2): The sequential kVA-then-PF clip is mathematically equivalent to OCHRE's `min()` approach for the current constraint set, but if additional inverter constraints are added (e.g., volt-watt curves), refactoring to a unified constraint-satisfaction step would improve clarity and reduce the risk of order-dependent bugs.

3. **Compute Q from post-clip P** (Finding 3): Apply `total_ac_power_kw = total_ac_power_kw.min(inv_cap.unwrap_or(f64::MAX))` before computing PF-derived reactive power at line 529. This ensures Q is based on the actual deliverable P and eliminates the need for downstream correction of an inflated Q value.

4. **Root comment cleanup** (mod.rs:292–300): The Var-priority `max_q_cap` computation includes tentative derivation comments (`// simplifies to: ... no`) that should be replaced with the final confirmed formula and a brief explanation of the OCHRE equivalence for future maintainers.

## References / Citations
- OCHRE PV.py `calculate_power_and_heat()`: `vendors/OCHRE/ochre/Equipment/PV.py:214–252`
- OCHRE PV.py `__init__` inverter configuration: `vendors/OCHRE/ochre/Equipment/PV.py:122–129`
- HARES `apply_inverter_limits()`: `crates/hares-equipment/src/pv/mod.rs:269–320`
- HARES `step()` inverter integration: `crates/hares-equipment/src/pv/mod.rs:526–535`
- HARES `apply_control_unchecked()` signal handlers: `crates/hares-equipment/src/pv/mod.rs:623–694`
- IEEE 1547-2018, Clause 5.4: Reactive power capability and voltage/power control functions
