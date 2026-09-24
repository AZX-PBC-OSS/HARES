# Occupant actor: behavior model, stochastic schedules, gain computation
**Review ID**: agents-02
**Category**: agents
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/actors/occupant.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/schedule.py`
- `vendors/OCHRE/ochre/Models/Envelope.py`

## Findings

### Finding 1: Occupant actor is a behavioral controller, not a stochastic occupancy generator [Severity: medium]
**Description**: The review criteria ask about stochastic occupancy models, RNG consistency, and gain computation within the Occupant actor. However, `crates/hares-core/src/actors/occupant.rs` implements a **reactive behavioral controller** — not a stochastic schedule generator or internal gain computer. It has no RNG usage, no gain computation, and no stochastic model. This is an architectural design decision where stochastic schedule generation, internal gain computation, and equipment control are separated into distinct components.

**Code Location**: `crates/hares-core/src/actors/occupant.rs` (entire file, 865 lines)

**Root Cause**: The HARES codebase decomposes the "occupant" concern differently than traditional building energy simulation frameworks:
- **Stochastic schedule generation**: `ScheduleSource` in `crates/hares-types/src/schedule.rs` (lines 337-344: `Stochastic` variant with `DistributionKind`) generates stochastic occupancy profiles from distributions at schedule time.
- **Internal gain computation**: `apply_occupancy_gains()` in `crates/hares-core/src/dwelling/mod.rs` (lines 2070-2130) computes sensible convective, sensible radiative, and latent gains per occupant.
- **Occupant behavioral control**: `Occupant` actor in `actors/occupant.rs` dispatches equipment control signals (ModeOverride, PowerSetpoint, LoadFraction) based on presence transitions.

The Occupant actor has **no `rand` import, no RNG field, no distribution sampling**, and **no gain computation**. It consumes a pre-computed, deterministic `Vec<Presence>` schedule provided externally via `with_presence_schedule()` (line 169).

**Impact**: The Occupant actor does not satisfy the review criteria for stochastic occupancy generation, but this is by design — not a bug. The review criteria presume a monolithic occupant model; HARES decomposes the concern. The stochastic capability exists at the schedule layer; internal gains are computed in `dwelling/mod.rs`. However, if a reviewer expects the Occupant actor to serve as the single entry point for occupant-related functionality, this decomposition could cause confusion.

### Finding 2: No presence schedule construction from schedule inputs in production code [Severity: high]
**Description**: The Occupant actor accepts a presence schedule via `with_presence_schedule()`, but the production factory in `actor_registry.rs:118-127` does **not** call it. The Occupant is constructed with only the default `vec![Presence::Home]` and optional lighting/appliance/EV/plug-load targets. There is no integration path to derive a `Vec<Presence>` from the building's occupancy schedule column or from workday/weekend parameters defined in the schedule inputs.

**Code Location**: `crates/hares-core/src/actor_registry.rs:121-127`
```rust
let mut occupant = Occupant::new(&config.name);
if let Some(target) = config.get_str("lighting_target") {
    occupant = occupant
        .with_lighting(target, EquipmentBehavior::default().off_when_away());
}
Ok(Box::new(occupant))
```

**Root Cause**: The actor registry's Occupant factory only handles lighting target configuration. It never reads the occupancy schedule column, never computes presence transitions from the occupancy time series, and never calls `with_presence_schedule()`. As a result, every Occupant actor in production is perpetually in the `Home` state, making its behavioral control logic (away/returning transitions) unreachable in practice.

**Impact**: The Occupant actor's behavioral control logic — turning equipment off when away, on when returning home, EV plug/unplug on transitions — is **dead code in production**. The only way to exercise this logic is through unit tests that manually construct a presence schedule. This means:
1. Lighting and appliance schedules are **not correlated with occupancy status** at runtime.
2. The actor cannot replicate workday/weekend departure/arrival patterns.
3. EV plug-in behavior never triggers transition-based connect/disconnect signals.

### Finding 3: Occupancy gains are computed independently of the Occupant actor [Severity: medium]
**Description**: The internal gain computation in `apply_occupancy_gains()` (`dwelling/mod.rs:2070-2130`) reads the occupancy count directly from the schedule payload (a single scalar per timestep) and applies per-person gain constants. This computation is completely independent of the Occupant actor — the actor has no involvement in gain deposition. The separation is architecturally valid but means the actor provides **no guarantee of correlation** between its equipment control signals and the internal gain state.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2085-2130`

**Root Cause**: The two systems (gain computation in the dwelling loop, equipment control in the actor) are decoupled. The Occupant actor's `decide()` method receives only an `EnvironmentState` parameter and has no access to the gain ports. This is consistent with the actor design philosophy (actors push control signals, never mutate environment state), but it prevents the actor from coordinating equipment behavior with internal load magnitudes.

**Impact**: Equipment that the Occupant turns on/off will have its own internal gains computed independently through the equipment's port contributions. The occupancy metabolic gains are computed separately through the schedule. There is no risk of double-counting, but no mechanism ensures consistency either — e.g., if the actor's presence schedule disagrees with the occupancy schedule column used for gains, equipment state and heat gains can become decoupled.

### Finding 4: Zero and negative occupancy correctly produce zero gains [Severity: none / verification]
**Description**: The `apply_occupancy_gains()` function correctly guards against zero and negative occupancy values with an explicit early return: `if n_occupants <= 0.0 { return; }` (dwelling/mod.rs line 2108). This was previously a bug (it used `.unwrap_or(0.0)` which silently produced zero gains when the schedule payload was absent), and has been fixed with `expect()` and construction-time validation. A dedicated regression test (`crates/hares-core/tests/occupant_count_silent_zero.rs`) verifies both the unoccupied-dwelling path and the validation path.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2086-2110`, `crates/hares-core/tests/occupant_count_silent_zero.rs`

**Root Cause**: Previously fixed bug; current code is correct.

**Impact**: No impact. Zero-occupancy timesteps produce exactly zero gains for all gain types. Negative occupancy values (which should never occur with valid schedule data) are also correctly guarded.

### Finding 5: Gain fractions correctly separate sensible convective, sensible radiative, and latent [Severity: none / verification]
**Description**: The occupancy gain computation correctly tracks three separate components:
- Sensible convective: `n_occupants × 66.0 × 0.70 = 46.2 W/person`
- Sensible radiative: `n_occupants × 66.0 × 0.30 = 19.8 W/person`
- Latent: `n_occupants × 51.2 W/person`

Total agrees with the OCHRE reference: 400 BTU/h = 117.2 W per person, split 0.563 sensible × 0.437 latent. HARES additionally splits sensible gain into convective (70%) and radiative (30%) per ASHRAE HoF 2021 Ch.18 Table 1, which OCHRE does not do (OCHRE treats all sensible gain as convective). The constants are documented in `crates/hares-physics/src/constants.rs:172-198` with full derivations.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2112-2128`, `crates/hares-physics/src/constants.rs:172-198`

**Root Cause**: N/A — implementation is correct.

**Impact**: No impact. Gains are correctly computed and delivered to the thermal solver through `PortContribution::Thermal` with distinct sensible, radiant, and latent components.

### Finding 6: No workday/weekend pattern differentiation in the Occupant actor [Severity: medium]
**Description**: The Occupant actor's `Presence` enum (`Home`, `Away`, `Sleeping`) and the flat `Vec<Presence>` schedule have **no concept of day type** (weekday vs weekend). The OCHRE vendor reference explicitly differentiates `weekday_fractions` and `weekend_fractions` in `create_simple_schedule()` (`vendors/OCHRE/ochre/utils/schedule.py:282-304`) and indexes schedules by `df.index.weekday < 5` (`schedule.py:440`). The Occupant actor cannot distinguish between weekdays and weekends, which means departure/arrival times and occupancy patterns cannot vary by day type.

**Code Location**: `crates/hares-core/src/actors/occupant.rs:38-43` (Presence enum), `crates/hares-core/src/actors/occupant.rs:128` (presence_schedule field)

**Root Cause**: The `Vec<Presence>` is a flat time-indexed array with no day-type metadata. If the schedule infrastructure generates a workday/weekend-aware occupancy profile, it would need to flatten it into this linear array, losing the day-type distinction. The actor itself has no weekday/weekend parameter support.

**Impact**: Realistic occupancy behavior with different weekday/weekend schedules cannot be modeled through the Occupant actor. However, as noted in Finding 2, the actor is not integrated with schedule generation in production anyway, so this limitation is currently theoretical.

### Finding 7: RNG isolation is architecturally sound but Occupant actor never uses it [Severity: low]
**Description**: Codebase-wide RNG infrastructure exists and is correct:
- `derive_dwelling_rng()` (`crates/hares-core/src/rng.rs:12-17`) produces a deterministic `ChaCha8Rng` from `master_seed` + `building_id`, guaranteeing building-level isolation.
- `derive_rng_seed()` in event_load.rs (lines 1421-1444) derives per-equipment seeds from the dwelling seed + equipment name hash.
- `StochasticState` in schedule.rs (lines 268-295) manages per-schedule RNG state with replay support for checkpoint/restore.

However, the Occupant actor has **no RNG dependency** — it never imports `rand`, never seeds an RNG, and makes no stochastic decisions. The RNG architecture is sound for the components that use it (EventBasedLoad, WetAppliance, EvDriver, DR compliance), but the review criteria's concern about dwelling-RNG consistency is not applicable to this actor.

**Code Location**: `crates/hares-core/src/rng.rs:1-17`, `crates/hares-core/src/actors/occupant.rs` (no RNG usage)

**Root Cause**: The Occupant actor is deterministic by design — it reacts to a pre-computed presence schedule and dispatches signals accordingly. There are no stochastic decisions to make within the actor.

**Impact**: Low. RNG reproducibility is preserved for all stochastic components. The Occupant actor's determinism is a feature, not a bug, provided the presence schedule is seeded correctly at the schedule-generation level.

## Summary
- **Total findings**: 7
- **Critical**: 0
- **High**: 1 (Finding 2: no presence schedule integration in production)
- **Medium**: 3 (Findings 1, 3, 6: architectural decomposition, decoupled gains, no workday/weekend)
- **Low**: 1 (Finding 7: no RNG used)
- **Verification / no severity**: 2 (Findings 4, 5: zero-occupancy and gain fractions verified correct)

## Recommendations

1. **Wire the occupancy schedule column into the Occupant actor** (Finding 2): The `actor_registry.rs` factory should read the dwelling's occupancy schedule column and convert it into a `Vec<Presence>` (e.g., thresholding: occupancy > 0 → `Home`, occupancy == 0 → `Away`). Without this, the actor's behavioral control is dead code.

2. **Add day-type awareness to the `Presence` schedule generation** (Finding 6): If stochastic occupancy schedule generation is implemented at the schedule layer, the resulting presence schedule should incorporate weekday/weekend parameters (departure/arrival times, base occupancy levels) matching the OCHRE pattern.

3. **Document the architectural decomposition** (Finding 1): Add a module-level comment or architecture document explaining that the Occupant actor handles reactive equipment control, while stochastic generation lives in `ScheduleSource` and gains are computed by the dwelling loop. This prevents future reviewers from making the same mismatch assumption.

4. **Ensure consistency between actor presence and gain occupancy count** (Finding 3): If the actor is integrated with the occupancy schedule (Recommendation 1), both systems will derive from the same source, providing consistency. If they remain separate, add an integration test that verifies the actor's presence state aligns with the gain occupancy count.

## References / Citations

- OCHRE `schedule.py:282-304` — `create_simple_schedule()`: weekday/weekend fraction differentiation
- OCHRE `schedule.py:402-449` — `import_occupancy_schedule()`: occupancy column normalization and scaling by number of occupants
- OCHRE `Envelope.py:902-908` — `occupancy_sensible_gain` / `occupancy_latent_gain` derivation: 400 BTU/h × 0.563 sensible, × 0.437 latent
- HARES `dwelling/mod.rs:2070-2130` — `apply_occupancy_gains()`: gain computation and deposition into thermal ports
- HARES `physics/constants.rs:172-198` — Occupant gain constants with ASHRAE HoF 2021 derivation
- HARES `actor_registry.rs:118-127` — Production Occupant factory: missing presence schedule construction
- HARES `tests/occupant_count_silent_zero.rs` — Regression test verifying zero-occupancy behavior
- HARES `rng.rs:12-17` — `derive_dwelling_rng()`: deterministic per-building RNG isolation
