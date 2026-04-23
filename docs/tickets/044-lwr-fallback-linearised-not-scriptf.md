# Interior LWR Fallback Path Uses Linearised h_r Approximation Without Warning

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope

## Problem

When `zone_cfg.scriptf` is `None` in `apply_interior_longwave_inputs`, the code silently engages `interior_longwave_linearised_w_into` — an h_r ≈ 4εσT³ linearization. The fallback is not a valid production path: per project policy `feedback_no_silent_defaults.md`, a silent fallback that degrades physics fidelity must error loudly. Any zone with `interior_lwr_zones` configured must have ScriptF view factors precomputed; if they are absent at simulation start, that is a configuration error, not a runtime mode to silently tolerate.

The EnergyPlus Engineering Reference §"Interior Long-Wave Radiation Exchange" specifies the full radiosity method with view factors (ScriptF / Hottel script-F formulation) as the required approach. The linearized h_r path is a deliberate downgrade. Siegel and Howell "Thermal Radiation Heat Transfer" 4th ed. Ch. 4 quantifies the linearization error: at ΔT = 20 K and T_mean = 290 K, error ≈ (ΔT/2T_mean)² ≈ 0.5 %; at ΔT = 40 K the error reaches 2 %, which is unacceptable for ASHRAE-grade zone LWR exchange.

### Secondary defect: redundant surface-list rebuild inside convergence loop

`hares-envelope/src/thermal_solver/longwave.rs:292–303` rebuilds `lwr_surfaces_buf` on every iteration of the convergence loop (`n_iter = floor(dt/300)+3`, minimum 3 iterations). The surface geometry does not change between iterations; the rebuild is a pure waste of n_iter − 1 allocations and copies per timestep.

## Current Behavior

`hares-envelope/src/thermal_solver/longwave.rs:289–304`:

```rust
if let Some(ref scriptf) = zone_cfg.scriptf {
    scriptf.net_flux_w_into(buf, &mut self.lwr_net_flux_buf);
} else {
    self.lwr_surfaces_buf.clear();
    self.lwr_surfaces_buf
        .extend(zone_cfg.surfaces.iter().map(|s| InteriorSurface { ... }));
    interior_longwave_linearised_w_into(
        &self.lwr_surfaces_buf, buf, t_zone_c, &mut self.lwr_net_flux_buf,
    );
};
```

No error or `tracing::warn!` emitted when `scriptf` is `None`. The same fallback pattern repeats at lines 323–338 (the post-loop net-flux call). The surface-list build is inside the `for _ in 0..n_iter` loop.

## Required Behavior

1. **Error on `scriptf == None` at solver construction**: In the solver builder (`hares-core/src/dwelling/solver_builder.rs`), assert `zone_cfg.scriptf.is_some()` for each zone that has `interior_lwr_zones` configured. Return a configuration error if ScriptF factors are absent — do not silently fall back. Per `feedback_no_silent_defaults.md`, the linearized path must not engage without a loud diagnostic. Per `feedback_no_backward_compat.md`, no shim or fallback mode is acceptable.

2. **One-time runtime warning if fallback is reached**: If the linearized path is reached at runtime despite the construction check (e.g., via `from_discrete` path), emit a one-time `tracing::warn!` per zone per simulation (guard with an `AtomicBool` or `HashSet<ZoneId>`), including the zone ID and a statement that LWR is at reduced physics fidelity. Do not emit per-timestep.

3. **Move surface-list build outside convergence loop**: Call `lwr_surfaces_buf.clear(); extend(...)` once before the `for _ in 0..n_iter` loop at `longwave.rs:286`, not inside it. This eliminates (n_iter − 1) redundant reconstructions per timestep — a minor but gratuitous allocation inside a hot loop (project policy: `feedback_hot_loop_minimal.md`).

Reference: EnergyPlus Engineering Reference §"Interior Long-Wave Radiation Exchange" — ScriptF radiosity method; ASHRAE HoF 2021 Ch. 18 §18.35 "Mean Radiant Temperature".

## Approach

1. In `solver_builder.rs`, after zone configuration is assembled, iterate over zones with `interior_lwr_zones` and return a `HaresError::Configuration` if any have `scriptf.is_none()`.
2. Add a `lwr_scriptf_warned: HashSet<ZoneId>` to `ThermalSolver` (or an `AtomicBool` per zone, depending on `Send` requirements). Check and set it in the `else` branch.
3. In `apply_interior_longwave_inputs`, move `lwr_surfaces_buf.clear(); extend(...)` to before the `for _ in 0..n_iter` loop. Apply the same fix to the post-loop call at lines 323–338 if it has a separate rebuild.
4. Add a test that the solver builder returns `Err` when a zone has `interior_lwr_zones` but `scriptf` is `None`.

## Definition of Done

- [ ] Solver builder returns `HaresError::Configuration` when `interior_lwr_zones` is configured for a zone without ScriptF factors
- [ ] If the linearized fallback is reached at runtime, a one-time `tracing::warn!` fires per zone with zone ID and fidelity statement
- [ ] `lwr_surfaces_buf.clear(); extend(...)` called once before the convergence loop, not inside it
- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo test -p hares-core` passes
- [ ] Test: solver builder errors when ScriptF absent for LWR zone

## Verification

```bash
cargo test -p hares-envelope
cargo test -p hares-core
```

## References

- EnergyPlus Engineering Reference §"Interior Long-Wave Radiation Exchange" — ScriptF (Hottel script-F) radiosity method with exact T⁴ formulation
- Siegel, R. and Howell, J.R. "Thermal Radiation Heat Transfer" 4th ed. Ch. 4 — linearization error for h_r ≈ 4εσT³: grows as (ΔT/2T_mean)²; at ΔT = 20 K, T_mean = 290 K, error ≈ 0.5 %
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.35 "Mean Radiant Temperature"
- Project policy `feedback_no_silent_defaults.md` — never silently substitute fallback values
- Project policy `feedback_hot_loop_minimal.md` — no per-step allocs or redundant work in convergence loops
