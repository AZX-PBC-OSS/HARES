# BESTEST tolerance widening masks underlying model defects
**Review ID**: envelope-12
**Category**: envelope
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-envelope/src/thermal_solver/mod.rs` (lines 1583–1834)
- `crates/hares-envelope/src/rc_network.rs` (lines 863–907)
- `crates/hares-envelope/src/boundary_rc.rs` (full file, 1118+ lines)
- `crates/hares-envelope/tests/` (all 11 test files)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py` (1465 lines) — OCHRE's full multi-layer RC envelope, linearized interior LWR, boundary RC construction
- `vendors/OCHRE/ochre/Models/RCModel.py` (star-mesh transform implementation)
- `vendors/EnergyPlus/src/EnergyPlus/` — CTF-based conduction solver (`HeatBalance*.cc`, `Construction.cc`); no BESTEST-specific case routing found

## Findings

### Finding 1: [Severity: high]
**Description**: BESTEST Case 600 validation tests (`thermal_solver/mod.rs:1609–1670`) construct a simplified 2R1C model (zone air + outdoor + ground nodes) rather than exercising the production-grade multi-layer StarMesh construction path through `assemble_building_rc`. The tests bypass the entire `boundary_rc.rs` assembly pipeline — including material-layer discretization (`split_layer_count` at `boundary_rc.rs:81–96`), interior LWR StarMesh conductances (`boundary_rc.rs:922–1029`), film resistance decomposition (`boundary_rc.rs:755–862`), and floating-node Y-Δ elimination (`rc_network.rs:220–276`) — and instead directly assemble a 1-state RC network via `RCNetwork::from_elements`.

**Code Location**: `thermal_solver/mod.rs:1609–1670` (`bestest_case_600_model`), with the comment at lines 1589–1592 explicitly acknowledging this simplification.

**Root Cause**: The HARES test suite lacks any BESTEST-parameterized integration test that exercises the full `assemble_building_rc` → `ThermalSolver` production pipeline. The existing BESTEST tests validate only the ZOH state-space solver's analytical consistency (time constants, energy balance) against a trivial 2R1C topology. This arrangement ensures the solver core is correct but provides zero regression coverage for defects in multi-layer discretization, StarMesh elimination, film decomposition, framing-factor correction, or per-depth ground node wiring — all of which are exercised exclusively in the production path.

**Impact**: Solver defects that emerge only in multi-layer constructions (e.g., incorrect inner/outer half-resistance assignment, incorrect split-layer count for thin/dense layers, StarMesh edge omission for fallback-R boundaries, per-depth ground wire cross-talk) are completely invisible to the BESTEST validation suite. These are precisely the defects that compound with HPXML complexity, where buildings have diverse constructions (mixed framing factors, varied material layers, multiple foundation depths). A defect that would cause 10% error in BESTEST Case 600 is structurally undetectable because the simplified 2R1C model does not even have the code paths that carry the defect.

**Comparison with Vendors**:
- **EnergyPlus**: Uses Conduction Transfer Functions (CTF) derived from full multi-layer constructions for ALL surfaces, including BESTEST validation runs. The CTF coefficients are computed once from material properties and then applied at each timestep; there is no separate "simplified validation model" path.
- **OCHRE**: Uses its full multi-layer RC boundary model (with precomputed RC values from its material database) for all envelope simulations. The `linearize_int_radiation` mode (star-mesh) is applied to the full RC network, not a reduced 2R1C proxy. OCHRE's `test_update_model` in `test_models/test_envelope.py:172` exercises the full production `Envelope.__init__` path.

### Finding 2: [Severity: high]
**Description**: The comment at `rc_network.rs:901–905` invokes BESTEST tolerance (~10% on annual loads) to justify a known 10–12% systematic error in the h_rad linearization: *"For extreme cases... the error reaches 10-12%, which is still within BESTEST tolerance (~10% on annual loads) but will be noticeable in detailed comfort calcs."* However, this 10–12% error is only the linearization component of a single sub-model (interior LWR). When layered on top of:
1. Multi-layer discretization error from split-layer-count approximations (`boundary_rc.rs:81–96`)
2. Combined film + R_inner_half decomposition in StarMesh mode (wrong inner/outer node coupling)
3. Framing-factor correction (`BoundaryInput::framing_factor` at `boundary_rc.rs:163–170`)
4. Foundation-depth ground node wiring
...the total compound error in the production path can significantly exceed the BESTEST 10% envelope.

**Code Location**: `rc_network.rs:901–905` (linearization justification comment)

**Root Cause**: The tolerance justification treats each error source independently and applies the ~10% BESTEST tolerance as a per-component guardrail. However, BESTEST 140-2017 §5.2 specifies 10% tolerance on the *aggregate* annual heating/cooling load, not per-component model error. Using the tolerance to dismiss a 10–12% component error leaves zero headroom for additional error sources (discretization, assembly, wiring, gridding) to remain within the aggregate bound.

**Impact**: Systematic errors from different model layers can add constructively (all biasing in the same direction for heating-dominated climates) or destructively (masking each other in cooling-dominated validation). Without a full-pipeline BESTEST integration test, there is no measured evidence that the aggregate error is within 10%. The current approach substitutes analytical error budgeting for empirical measurement.

### Finding 3: [Severity: medium]
**Description**: The Case 900FF root-cause analysis in `tests/bestest_900ff_root_cause.rs` identifies two confirmed root causes (warm initial conditions: −4.53 °C; 100% convective gains vs. 30% radiant: −0.295 °C) but was performed using a manually constructed 2R1C model rather than the full `assemble_building_rc` production pipeline. The investigation ruled out RC discretization as a cause (+0.029 °C delta, wrong direction) but only tested the discretization path between 2→4 concrete nodes within the same simplified test harness. No test verifies that the production-grade multi-layer assembly (with StarMesh, film decomposition, framing correction) produces the same thermal response as the simplified model used for root-cause attribution.

**Code Location**: `tests/bestest_900ff_root_cause.rs:206–303`

**Root Cause**: The root-cause investigation was performed in the simplified test harness (`assemble_building_rc` at line 185), not inside the full `ThermalSolver` integration path. While this test uses `InteriorLwrMethod::StarMesh` and exercises the boundary RC builder, it only validates *structural* properties (node count, capacitance ratio) — not *behavioral* properties (simulated zone temperature after 30 days against EnergyPlus reference).

**Impact**: Medium. The root-cause conclusions (initial conditions dominate) are directionally correct based on the physics (τ_concrete ≈ 3 days), but a remaining 2.50 °C outlier after accounting for all known causes leaves unexplained error that could originate in the production path components not exercised by the simplified model.

### Finding 4: [Severity: medium]
**Description**: The BESTEST Case 900 heavyweight test (`thermal_solver/mod.rs:3113–3198`) reuses the simplified `bestest_case_600_model()` 2R1C topology, only increasing the zone capacitance by 10× (line 3120). A real Case 900 building has heavy *multi-layer* walls (concrete + insulation + siding), not just a single heavier air node. The structural difference between 2R1C (single time constant, no distributed thermal mass) and a multi-layer wall model (multiple time constants from layer discretization) means the simplified test validates the solver's handling of capacitance scaling but not its handling of distributed thermal mass with different time constants per layer.

**Code Location**: `thermal_solver/mod.rs:3117–3120`

**Root Cause**: The Case 900 test was designed to verify that higher capacitance → longer time constant → slower thermal response (a physically necessary correctness check), which it does. But it was not designed to validate the production path's handling of multi-layer heavy constructions. The gap is in test *scope*, not implementation correctness.

**Impact**: Medium. The test correctly validates the analytical property (τ ∝ C) but provides no validation that the production path correctly discretizes the concrete layer, assigns correct inner/outer half-resistances, or wires the StarMesh radiation topology for heavy walls.

### Finding 5: [Severity: low]
**Description**: No integration test exists that exercises the full production path `assemble_building_rc(…, InteriorLwrMethod::StarMesh)` → `ThermalSolver::step_zone_temperatures` with BESTEST-defined geometry, materials, and boundary conditions. The existing integration tests (`synthetic_box.rs`, `thermal_pathway_physics.rs`, `solver_energy_conservation.rs`) use custom parameterizations, not the BESTEST specification. This means there is no automated regression guard that would detect a wiring change that alters BESTEST results.

**Code Location**: `crates/hares-envelope/tests/` — absence of a BESTEST integration test

**Root Cause**: The test architecture separates concern into "solver core correctness" (synthetic_box, unit tests) and "physics correctness" (pathway tests), but never brings them together with the BESTEST specification as the integration oracle. An integration test that exercises the full production pipeline with BESTEST geometries (Case 600, Case 610, Case 900) and validates against the ASHRAE 140-2017 temperature/load band would fill this gap.

**Impact**: Low for current correctness (the separate test layers provide good coverage), but high for future regression risk. A refactor to B-matrix wiring, ground-node assignment, or film resistance computation could silently break BESTEST compliance because no CI test measures it.

## Summary
- **Total findings**: 5
- **Critical**: 0
- **High**: 2
- **Medium**: 2
- **Low**: 1

## Recommendations

1. **Add a full-pipeline BESTEST Case 600 integration test** that exercises `assemble_building_rc` with multi-layer wall/roof/floor constructions matching the BESTEST Case 600 specification (vinyl siding → fiberglass batt → gypsum board, etc.) and validates the 24h free-float temperature response against the ASHRAE 140-2017 band. This is a one-time test fixture that pays ongoing dividends as a regression guard. Even if the full multi-layer model exceeds the BESTEST tolerance (due to the compound error sources described in Finding 2), *measuring the gap* provides actionable data about which error components dominate.

2. **Replace the tolerance-justification comment at `rc_network.rs:901–905`** with a measured error budget table that attributes the total BESTEST error gap across components (linearization, discretization, wiring), rather than invoking the ~10% aggregate tolerance as a per-component guardrail.

3. **Add a Case 900 full-pipeline test** that uses the heavy construction material layers from the 900FF root-cause investigation (concrete + insulation + wood siding, `bestest_900ff_root_cause.rs:148–173`) within the production `ThermalSolver`, running a 30-day January simulation and comparing against the EnergyPlus reference min/max temperature band. This closes the gap between the current structural validation (node count, capacitance ratio) and behavioral validation (simulated temperature trajectory).

4. **Document the gap between simplified 2R1C validation and production-grade multi-layer StarMesh** in the BESTEST test module doc comment at `thermal_solver/mod.rs:1583–1604`, noting that the simplified tests validate solver-core correctness (time constants, energy balance) but do not represent production behavior with HPXML-derived constructions.

## References / Citations
- ANSI/ASHRAE Standard 140-2017, Section 5.2.1 (Case 600): Low Mass Building specification
- ANSI/ASHRAE Standard 140-2017, Section 5.2.4 (Case 900): High Mass Building specification
- EnergyPlus Engineering Reference, "Conduction Transfer Functions" — multi-layer CTF derivation from material properties
- OCHRE `Envelope.py:1048–1061` — interior LWR linearization and star-mesh transform for full multi-layer RC networks
- HARES `boundary_rc.rs:57–96` — diurnal penetration depth `split_layer_count` for multi-layer discretization
- HARES `boundary_rc.rs:922–1029` — StarMesh interior LWR conductance wiring
- HARES `rc_network.rs:220–276` — `reduce_floating_nodes` star-mesh Y-Δ elimination
- HARES `thermal_solver/mod.rs:1609–1670` — `bestest_case_600_model` simplified 2R1C construction
- HARES `thermal_solver/mod.rs:3113–3198` — `bestest_case_900_heavyweight_longer_time_constant` simplified 2R1C test
- Incropera & DeWitt, *Fundamentals of Heat and Mass Transfer* §5.8 — diurnal penetration depth for semi-infinite solid
- ISO 13786:2007 §6.2 — dynamic thermal characteristics, diffusion-length criterion for periodic heat flow
