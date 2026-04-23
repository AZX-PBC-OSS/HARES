# Semi-Implicit Infiltration Latent Coupling

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope

## Problem

The infiltration sensible load uses semi-implicit coupling (implicit diagonal, explicit forcing) in the thermal solver. The infiltration latent load is fully explicit — computed from current-timestep humidity ratios with no implicit treatment. This is a **consistency gap**: the two paths apply different numerical methods to the same physical phenomenon (air exchange across the envelope), and there is no physical justification for the asymmetry.

As the stability analysis below shows, the system is already numerically stable under all realistic operating conditions with the current 15× moisture buffering multiplier. This ticket is not a stability fix. It is a **consistency improvement** and a **future-proofing measure**: if the buffering multiplier is ever reduced (e.g., for sensitivity analysis or calibration), the latent path degrades before the sensible path does. Making both paths semi-implicit removes that asymmetric dependence and aligns the implementation with its own stated design intent. The semi-implicit form is also the standard discretization cited in EnergyPlus Engineering Reference §13.3 "Zone Mass Balance" for coupled zone air mass flow.

## Current Behavior

### Sensible infiltration — semi-implicit (stable)

In `infiltration.rs:25-32`, the `InfiltrationCoupling` struct carries the semi-implicit decomposition:

```rust
/// The sensible infiltration load `q = h_inf * (T_out - T_zone)` is split:
///   - Implicit part: `-h_inf * T_zone` added to the A-matrix diagonal
///   - Explicit part: `h_inf * T_out` added as forcing
```

The thermal solver applies this in `stepping.rs` (the semi-implicit coupling code): the infiltration conductance `h_inf_w_k` is added to the implicit diagonal, while the outdoor temperature `t_forcing_c` provides the explicit forcing. This makes the sensible infiltration unconditionally stable regardless of ACH or timestep, as documented in the `InfiltrationCoupling` struct doc comment (citing EnergyPlus Engineering Reference §13.3).

### Latent infiltration — fully explicit (conditionally stable)

In `infiltration.rs:215`:

```rust
let q_latent = m_dot_lat * H_FG_J_PER_KG * (w_out - zone.humidity_ratio);
```

This computes the latent gain from the **current** zone humidity ratio (`zone.humidity_ratio`), making it fully explicit. The result is accumulated into `latent_out` (line 244) and passed to the humidity solver.

In the humidity solver (`humidity_solver.rs:116-122`), the latent gain is summed from both port contributions and the thermal domain payload, then converted to a humidity ratio increment:

```rust
let latent_gain_w = ports.thermal.iter()...sum() + self.latent_buf.get(&zone_id)...;
```

And at line 141-148:

```rust
let d_w = humidity_ratio_increment(
    latent_gain_w, dt_s, self.config.h_fg_j_kg,
    rho_air, volume_m3, self.config.moisture_buffering_multiplier,
);
let w_new = (w_old + d_w).clamp(0.0, w_sat);
```

The formula (line 164-175):

```
d_w = (latent_gain_w * dt_s) / (h_fg * rho * V * moisture_buffering)
```

is fully explicit: `latent_gain_w` depends on `w_old`, and `w_new = w_old + d_w`.

### Stability analysis

For the explicit humidity solver with infiltration, the update is:

```
w_{n+1} = w_n + (m_dot * h_fg * (w_out - w_n) + Q_other) * dt / (h_fg * rho * V * M)
         = w_n * (1 - m_dot * dt / (rho * V * M)) + (m_dot * w_out * dt / (rho * V * M)) + Q_other * dt / (h_fg * rho * V * M)
```

where M = moisture_buffering_multiplier (15.0). The amplification factor for the homogeneous part is:

```
A = 1 - m_dot * dt / (rho * V * M)
```

For stability, we need `|A| ≤ 1`, i.e., `m_dot * dt ≤ 2 * rho * V * M`.

With typical values: rho = 1.2 kg/m³, V = 200 m³, M = 15, ACH = 1.0, dt = 300 s:
- m_dot = 1.2 * (1.0 * 200 / 3600) = 0.0667 kg/s
- m_dot * dt = 0.0667 * 300 = 20.0
- 2 * rho * V * M = 2 * 1.2 * 200 * 15 = 7200

So `A = 1 - 20/7200 = 0.997` — stable. But without the 15× multiplier (M = 1):
- `A = 1 - 20/480 = 0.958` — still stable, but approaching marginal at higher ACH or coarser dt.

At ACH = 2, dt = 600 s (10 min), M = 1:
- m_dot * dt = 2 * 0.0667 * 600 = 80
- 2 * rho * V * 1 = 480
- `A = 1 - 80/480 = 0.833` — still stable but with significant damping.

At ACH = 3, dt = 600 s, M = 1:
- m_dot * dt = 120
- `A = 1 - 120/480 = 0.75` — significant damping but stable.

The 15× multiplier keeps the system well within the stable regime. However, the coupling is inconsistent with the sensible path, and if the multiplier were ever reduced (e.g., for sensitivity analysis), the latent path would be the first to show instability. Making both paths semi-implicit ensures consistency and removes the dependency on the buffering multiplier for stability.

## Required Behavior

Apply the same semi-implicit treatment to infiltration latent as is used for sensible. The mathematical form:

```
W_{n+1} = W_n + dt / C_eff * [Q_latent_other + m_dot_inf * h_fg * (W_out - W_{n+1})]
```

where `C_eff = h_fg * rho * V * M` is the effective moisture capacitance and `W_{n+1}` appears on both sides. Solving algebraically:

```
W_{n+1} * (1 + m_dot_inf * dt / C_eff) = W_n + dt / C_eff * [Q_latent_other + m_dot_inf * h_fg * W_out]
```

```
W_{n+1} = [W_n + dt / C_eff * (Q_latent_other + m_dot_inf * h_fg * W_out)] / (1 + m_dot_inf * dt / C_eff)
```

The implicit part (`m_dot_inf * h_fg * W_{n+1}`) moves to the denominator, providing unconditional stability regardless of ACH, timestep, or moisture buffering multiplier.

In practice, this means the humidity solver needs to know the infiltration mass flow rate and outdoor humidity ratio separately (not just the aggregated `latent_gain_w`), so it can apply the semi-implicit coupling.

## Approach

### Step 1: Extend `InfiltrationCoupling` with moisture coupling fields

`InfiltrationCoupling` in `infiltration.rs:25-52` already carries `q_latent_w` (with `#[allow(dead_code)]` at lines 33-34). Rather than introducing a parallel struct, extend the existing struct with the one additional field needed for semi-implicit treatment:

```rust
/// Outdoor humidity ratio driving the moisture forcing [kg/kg].
pub w_outdoor: f64,
```

`m_dot_lat` is re-derivable as `rho * combined_flow_m3_s` (both fields already in the struct) and must NOT be stored as a separate field to avoid duplication. The semi-implicit formula can compute `m_dot_lat = rho * coupling.combined_flow_m3_s` inline.

`apply_infiltration_and_ventilation()` already computes `m_dot_lat` at line 212 (`let m_dot_lat = rho * latent_flow_m3_s`); wiring `w_outdoor` into the struct requires passing the outdoor humidity ratio from the environment state, which is already available at the call site.

### Step 2: Pass moisture coupling to humidity solver

Currently the humidity solver receives infiltration latent as watts via `latent_by_zone` (in the thermal domain's custom_payload). Add a new mechanism:

Option A: Store `InfiltrationMoistureCoupling` data in the `DomainUpdate` custom_payload alongside the existing `(zone_id, q_latent_w)` pairs. Extend the payload format to include `(zone_id, q_latent_w, m_dot_inf, w_outdoor)`.

Option B (simpler): Store the moisture coupling in a shared field accessible to both the thermal and humidity solvers. Since they run sequentially (thermal first, then humidity), the thermal solver can deposit the coupling data in a field that the humidity solver reads.

Prefer Option A for now — extend the custom_payload format from 2 floats per zone to 4 floats per zone:
```
[zone_id, q_latent_w, m_dot_inf_kg_s, w_outdoor_kg_kg]
```

### Step 3: Implement semi-implicit humidity update

In `humidity_solver.rs`, modify the update loop (currently lines 141-151):

```rust
// BEFORE:
let d_w = humidity_ratio_increment(latent_gain_w, dt_s, h_fg, rho, volume, M);
let w_new = (w_old + d_w).clamp(0.0, w_sat);

// AFTER:
let C_eff = h_fg * rho * volume * M;
let alpha = m_dot_inf * dt_s / C_eff;  // infiltration coupling coefficient

// Split latent into infiltration (semi-implicit) and other (explicit)
let q_other = latent_gain_w - m_dot_inf * h_fg * (w_outdoor - w_old);
let numerator = w_old + dt_s / C_eff * (q_other + m_dot_inf * h_fg * w_outdoor);
let denominator = 1.0 + alpha;
let w_new = (numerator / denominator).clamp(0.0, w_sat);
```

### Step 4: Treat missing coupling data as a hard error

`w_outdoor` is always available from the environment state at the call site in `apply_infiltration_and_ventilation()`. There is no valid runtime state in which it would be absent. If coupling data is missing when the humidity solver attempts to apply the semi-implicit formula, that is a bug — panic with a clear message. No fallback path to the explicit implementation exists; the semi-implicit path is the only path.

### Step 5: Verify stability improvement

Run a test case with high infiltration (2 ACH), coarse timestep (600 s), and `moisture_buffering_multiplier = 1.0` (no buffering). Verify that:
- The semi-implicit path produces a stable, non-oscillatory convergence to outdoor humidity
- The explicit path (current behavior, for comparison) shows oscillations or significant damping at these conditions

## Definition of Done

- [ ] `InfiltrationCoupling` struct extended with `w_outdoor: f64` field in `infiltration.rs`
- [ ] `m_dot_lat` is NOT added as a struct field; derived inline as `rho * coupling.combined_flow_m3_s`
- [ ] `apply_infiltration_and_ventilation()` populates `w_outdoor` from the environment state
- [ ] Thermal solver's custom_payload format extended to carry `w_outdoor` alongside existing fields
- [ ] Humidity solver reads `w_outdoor` from the extended payload and derives `m_dot_lat` locally
- [ ] Semi-implicit update formula implemented in humidity solver
- [ ] Missing coupling data at the humidity solver panics with a descriptive message; the explicit path is removed entirely
- [ ] `tracing::debug!` logging semi-implicit coefficient `alpha = m_dot_inf * dt_s / C_eff` per zone per step
- [ ] Telemetry key `HUMIDITY_SEMI_IMPLICIT_ALPHA` added per zone
- [ ] Existing tests pass (no regression)
- [ ] New test: high ACH + coarse dt + M=1 produces stable humidity convergence
- [ ] New test: semi-implicit and explicit paths agree to within 1% at fine dt (1 min)

## Verification

1. **Stability test**: Configure humidity solver with ACH=2, dt=600s, M=1.0. Outdoor humidity = 0.012 kg/kg, indoor initial = 0.005 kg/kg. Run 100 steps. Verify no oscillations (w alternately above/below w_outdoor). Verify convergence to within 1% of w_outdoor.

2. **Accuracy test**: Same scenario at dt=60s. Compare semi-implicit result with explicit result (the current behavior). They should agree to within 0.1% at fine timesteps (both methods converge to the same steady state).

3. **Existing regression**: The existing `infiltration_humidity_ratio_matches_first_principles` test at `humidity_solver.rs:501-528` must still pass. This test verifies that the latent heat cancels correctly in the humidity ratio formula. With semi-implicit coupling, the cancellation still holds at the steady-state limit.

4. **Moisture mass balance**: After the semi-implicit update, verify that the total moisture mass change equals the sum of all moisture source/sink terms over the timestep. This is the fundamental conservation check.

## References

- EnergyPlus Engineering Reference §13.3 "Zone Mass Balance": standard semi-implicit discretization `C dw/dt = m_dot*(w_out - w) + Q_other` for zone air moisture balance, where infiltration mass flow enters the implicit term. The sensible heat balance in §13.1 uses the same pattern — implicit diagonal entry for the infiltration conductance, explicit forcing from outdoor temperature.
- ASHRAE Handbook of Fundamentals 2021 Ch.24, zone air moisture balance: `dm_w/dt = Σṁ_w,i - ṁ_w,out`
- `hares-envelope/src/thermal_solver/infiltration.rs:25-52`: `InfiltrationCoupling` struct, including `q_latent_w` field (lines 33-34, currently `#[allow(dead_code)]`)
- `hares-envelope/src/humidity_solver.rs:164-175`: Current explicit humidity ratio increment formula
- `hares-envelope/src/humidity_solver.rs:34`: 15× moisture buffering multiplier

## Related Tickets

- 001-unify-hfg-add-humidity-port.md (h_fg consistency is prerequisite for correct semi-implicit latent coupling)
