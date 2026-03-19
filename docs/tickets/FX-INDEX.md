# FX Ticket Index — Comprehensive Audit Fixes

Generated 2026-03-19 from 8-agent parallel audit of HARES vs OCHRE.
Reviewed by Codex gpt-5.3-codex 2026-03-19; corrections applied to FX-002, FX-023.

**Execution risk note (Codex):** FX-005, FX-006, FX-007, FX-008, FX-029 all touch HPXML parsing in `building.rs`. High merge-conflict risk if landed independently. Recommend: coordinate with shared test fixtures and a single integration gate test, or batch into one HPXML-parity epic. The `ZoneType::Ground` fix (FX-034) should land before or alongside these to avoid incorrect ground-zone semantics.

## Phase 1: Critical Correctness Blockers

| Ticket | Title | Depends On | Parallel? |
|--------|-------|------------|-----------|
| FX-001 | Wire MetricsCalculator into simulation engine | — | Yes |
| FX-003 | Fix HSPF/EER/SEER to EIR unit conversion | — | Yes |
| FX-005 | Parse ConditionedBuildingVolume from HPXML | — | Yes |
| FX-006 | Parse SolarAbsorptance and Emittance from HPXML | — | Yes |
| FX-007 | Parse Window InteriorShading from HPXML | — | Yes |
| FX-008 | Parse FrameFloor HPXML elements | — | Yes |
| FX-010 | Add surface absorptivity to opaque solar gain | FX-006 | After FX-006 |
| FX-009 | Implement true exterior surface temperature for LWR | FX-006, FX-010 | After FX-010 |
| FX-011 | Add absorbed glass heat for windows | FX-007 | After FX-007 |
| FX-014 | Fix Python initialization_duration silently dropped | — | Yes |
| FX-015 | Fix batch_step_py GIL re-acquisition | — | Yes |

## Phase 2: High-Impact Fixes

| Ticket | Title | Depends On | Parallel? |
|--------|-------|------------|-----------|
| FX-002 | Fix fleet weighted mean aggregation | FX-001 | After FX-001 |
| FX-004 | Fix HVAC airflow defaults per equipment type | — | Yes |
| FX-012 | Implement true interior surface temps for LWR | — | Yes |
| FX-013 | Fix steady-state initialization | — | Yes |
| FX-016 | Implement conditioned_space_fraction for HVAC | — | Yes |
| FX-017 | Fix MSHP 4-speed selection | — | Yes |
| FX-018 | Fix gas WH safety cutout node | — | Yes |
| FX-019 | Fix WH element heat ordering | — | Yes |
| FX-020 | Fix WH inversion mixing algorithm | — | Yes |
| FX-021 | Fix HPWH thermostat control | — | Yes |
| FX-022 | Fix multi-zone humidity checkpoint | — | Yes |
| FX-023 | Fix ventilation/infiltration interaction | — | Yes |
| FX-024 | Wire wet appliance hot-water demand | — | Yes |
| FX-025 | Fix EventBasedLoad fuel type handling | — | Yes |
| FX-026 | Fix Python OCHRE compat cooling setpoint | — | Yes |
| FX-027 | Expand Python telemetry DataFrame | FX-001 | After FX-001 |
| FX-028 | Relax HPXML schema version requirement | — | Yes |
| FX-029 | Parse missing Building fields | FX-005 | After FX-005 |
| FX-030 | Upgrade solar position to NREL SPA | — | Yes |
| FX-031 | Fix control dispatch ordering | — | Yes |

## Phase 3: Medium Severity Batches

| Ticket | Title | Depends On | Parallel? |
|--------|-------|------------|-----------|
| FX-032 | Medium HVAC fixes batch | FX-003, FX-004 | After Phase 1 |
| FX-033 | Medium water heater fixes batch | — | Yes |
| FX-034 | Medium physics and IO fixes batch | — | Yes |
| FX-035 | Medium Python/RL fixes batch | — | Yes |
| FX-036 | Medium control and fleet fixes batch | FX-001 | After FX-001 |
| FX-037 | Add EndUse::Appliances variant | — | Yes |

## Dependency Graph (critical path)

```
FX-006 → FX-010 → FX-009
FX-007 → FX-011
FX-005 → FX-029
FX-001 → FX-002
FX-001 → FX-027
FX-001 → FX-036
FX-003 → FX-032
FX-004 → FX-032
```

## Maximum Parallelism

**Phase 1 parallel set** (no dependencies, can all start simultaneously):
FX-001, FX-003, FX-005, FX-006, FX-007, FX-008, FX-014, FX-015

**Phase 2 parallel set** (after their Phase 1 deps):
FX-004, FX-012, FX-013, FX-016–FX-026, FX-028, FX-030, FX-031

**Phase 3 parallel set** (after their deps):
FX-032–FX-037

## Design Principle

Where HARES physics is better than OCHRE, we **keep HARES and document the divergence**:
- Below-freezing wet-bulb coefficient (2.006 vs OCHRE's buggy 0.24)
- Per-curve diffuse IAM (vs OCHRE's flat 0.854 fudge factor)
- Henderson-Rengarajan latent degradation model (not in OCHRE)
- HPWH COP scaling from rated UEF (OCHRE hardcodes)
- OCV-based battery with rainflow counting (>> OCHRE's constant-efficiency)
- Tankless WH parasitic power modeling (OCHRE doesn't model this)
- ASHRAE HOF 2021 wet-bulb coefficient (2.381 vs OCHRE's 2017-era 2.326)
