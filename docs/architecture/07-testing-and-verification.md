# Testing & Verification

## Validation Strategy

**Primary validation**: ochre_next produces the same output as OCHRE on the same inputs.
Not bit-identical — but within tight tolerance bands that confirm the physics is
correctly reimplemented.

**Secondary validation**: BESTEST / ASHRAE Standard 140 cases establish correctness
independently of OCHRE. A core BESTEST subset (600FF, 900FF, 640) is a firm Phase 2
deliverable; full ASHRAE 140 compliance follows in Phase 4.

## Phase 1 — OCHRE Output Parity (CI Gate)

Same HPXML + same schedule + same weather + same parameters → compare ochre_next output
against OCHRE output at 1-minute resolution.

### Test Corpus

10 reference buildings spanning:
- **Climate**: CZ 2A (hot-humid), 4A (mixed), 5A (cold), 6B (cold-dry)
- **HVAC**: Gas furnace+AC, ASHP, mini-split, electric resistance
- **Water heater**: Electric resistance, HPWH, gas tank
- **DERs**: PV, battery, EV, combinations
- **Duration**: 30-day summer + 30-day winter per building (development gate);
  full-year parity for at least 3 buildings spanning major equipment families (release gate)

### Tolerance Bands

| Metric | Tolerance | Rationale |
|--------|-----------|-----------|
| Zone temperature (conditioned) | ±0.1°C MAE | Same RC solver, same inputs |
| Zone temperature (unconditioned) | ±0.5°C MAE | Infiltration model sensitivity |
| Annual HVAC energy | ±1.0% relative | Same curves, same algorithms |
| Annual water heater energy | ±0.5% relative | Same tank model |
| Annual total site energy | ±1.0% relative | |
| Peak HVAC power | ±2.0% relative | Curve evaluation rounding |
| Battery SOC trajectory | ±1% MAE absolute | Same efficiency model |
| Equipment mode cycle count | ±5% relative | Deadband timing sensitivity |

These are tight bands — we're testing that the Rust implementation matches the Python
implementation, not comparing different models.

### Property Parity Test

Parse 10 ResStock HPXML bundles through both OCHRE and ochre_next. Assert that the
equipment sets, zone configurations, and key parameters match. This catches HPXML
parser divergence early.

## Phase 2 — BESTEST / ASHRAE Standard 140

ASHRAE Standard 140-2023 cases validate envelope and HVAC physics against the
reference program envelope (EnergyPlus, DOE-2, TRNSYS). This is the industry
standard for simulation engine credentialing and a firm Phase 2 deliverable.

Key cases:
| Case | Tests |
|------|-------|
| 600FF | Free-float lightweight envelope |
| 610 | South shading |
| 620 | East/west windows |
| 900FF | Heavyweight free-float (thermal mass) |
| 640 | Setback thermostat |
| CE100–CE200 | DX cooling mechanical equipment |
| §5.4 heating | Heat pump heating performance |

BESTEST buildings are not representable in HPXML. Use synthetic building configs
(TOML input format).

## Numerical Invariants (Runtime Assertions)

### Per-Timestep Energy Balance (Thermal)

```
|Σ(Q_gain) - ΔE_storage - Q_loss_envelope| < max(1.0 W, 1e-6 · |Σ Q_gain|)
```

### Per-Timestep Electrical Balance

All ports use `+consume / -generate` sign convention:
```
|P_grid + Σ P_equipment_ports| < 0.001 kW
```

### Per-Timestep Moisture Balance

```
|Δm_water - Σ(Q_latent_i · dt / h_fg)| < 1e-6 kg
```

Where `Δm_water = ΔW_zone · ρ_air · V_zone` (kg), `Q_latent_i` is latent heat gain
(W), and `h_fg ≈ 2.45e6 J/kg` (latent heat of vaporization at ~20°C). The division
by `h_fg` converts energy to equivalent moisture mass.

### SOC Bounds

```rust
debug_assert!(soc >= 0.0 && soc <= 1.0);
// Release: clamp + warn if accumulated error > 0.001
```

### Temperature Sanity

| Zone | Lower | Upper | Action |
|------|-------|-------|--------|
| Conditioned indoor | -30°C | 60°C | Warn + clamp |
| Attic | -40°C | 80°C | Warn + clamp |
| Water tank node | 0°C | 100°C | Error |

## Deterministic Replay

Built-in equipment produces identical results for the same inputs regardless of thread
count in fleet mode. Achieved by:

- Each dwelling is independent — no cross-dwelling state
- Deterministic RNG via hierarchical ChaCha8 seeding
- No floating-point non-determinism from parallel accumulation (each dwelling accumulates
  independently)

```rust
/// Derive per-dwelling RNG from master seed + building ID
fn derive_dwelling_rng(master_seed: u64, bldg_id: i64) -> ChaCha8Rng {
    let mut seed = [0u8; 32];
    seed[0..8].copy_from_slice(&master_seed.to_le_bytes());
    seed[8..16].copy_from_slice(&bldg_id.to_le_bytes());
    ChaCha8Rng::from_seed(seed)
}
```

Adding/removing a building from the fleet does not affect other buildings' RNG streams.

**Python equipment**: Determinism depends on the Python code. Built-in Rust equipment
is deterministic; Python adapters are best-effort.

## Test Matrix

| Layer | Test Type | What It Validates |
|-------|-----------|-------------------|
| **Physics kernel** | Unit test | Pure functions: biquadratic, psychrometrics, RC step |
| **Equipment** | Fixture test | Single equipment, fixed inputs → expected outputs |
| **Envelope** | Golden case | Known RC network → analytical solution |
| **Port accumulation** | Integration test | Multiple equipment → correct gain summation |
| **Multi-instance** | Integration test | 2 batteries + 2 PV → correct independent behavior |
| **Control signal** | Dispatch test | ControlSignal → correct equipment response |
| **Full simulation** | Regression test | Complete dwelling → OCHRE reference ± tolerance |
| **HPXML parser** | Parity test | Same HPXML → same equipment set as OCHRE |
| **Fleet** | Scale test | N dwellings without OOM, correct weighted aggregation |
| **RL Gym** | Determinism test | Same seed → identical trajectory |
| **BESTEST** | Standard 140 | Results within reference program envelope |

## Physics Decisions Log

Significant physics modeling choices are documented in `PHYSICS_DECISIONS.md`:

1. What the engine does and why (citing standards, papers, EnergyPlus precedent)
2. Where it differs from OCHRE and the rationale
3. Expected direction of change in outputs

This log is maintained alongside the codebase and updated whenever a physics modeling
decision is made that affects output behavior. It serves as the authoritative record
for why the engine produces the results it does.
