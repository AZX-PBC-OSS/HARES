# HPXML validation test coverage gaps
**Review ID**: hpxml-10
**Category**: hpxml
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/hpxml/validation.rs` — HPXML schema and domain validation (710 lines)
- `crates/hares-io/tests/hpxml_parsing_tests.rs` — Integration tests for parsing/validation/resolution (1315 lines)
- `crates/hares-io/tests/hpxml_parity.rs` — OCHRE parity tests for typed config extraction (1404 lines)
- `crates/hares-io/tests/hpxml_fixture_inventory.rs` — Fixture inventory and well-formedness checks (91 lines)
- `crates/hares-io/tests/hpxml_defaults_regressions.rs` — Silent default substitution regressions (312 lines)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/test/OS-HPXML Sample Files/` — 425 OCHRE HPXML v4.0 fixtures
- `vendors/OCHRE/test/test_equipment/test_hvac.py` — OCHRE HVAC test harness (18 KB)
- `vendors/OCHRE/test/test_equipment/test_scheduled.py` — OCHRE scheduled load tests (5.9 KB)
- `vendors/OCHRE/test/test_equipment/test_waterheater.py` — OCHRE water heater tests (21 KB)
- `vendors/OCHRE/test/test_dwelling/test_dwelling.py` — OCHRE dwelling integration tests (7.3 KB)

## Findings

### Finding 1: No test coverage for multiple heating systems
**Severity**: critical

**Description**: HARES has zero tests for a building with multiple heating systems. OCHRE's `base-hvac-multiple.xml` (934 lines) models 7 `<HeatingSystem>` entries (electric resistance, gas furnace, electric boiler, gas boiler, electric furnace, fuel oil furnace, propane furnace), 3 `<CoolingSystem>` entries (central AC, room AC, PTAC), and 3 `<HeatPump>` entries (air-to-air, ground-to-air, mini-split) with `<PrimaryHeatingSystem>`/`<PrimaryCoolingSystem>` attributes designating one system as primary.

**Code Location**:
- HARES curated fixture set: `crates/hares-io/tests/hpxml_fixture_inventory.rs:13-24` — lists only 10 fixtures, none for multiple HVAC
- HARES fixture inventory test asserts coverage of PV, battery, EV, washer, dryer, dishwasher, and lighting (`hpxml_fixture_inventory.rs:60-91`), but never validates multiple heating/cooling systems
- HARES equipment resolution (`crates/hares-io/src/hpxml/equipment.rs`) has no test path for iterating multiple `HeatingSystem` or `CoolingSystem` entries
- OCHRE reference fixture: `vendors/OCHRE/test/OS-HPXML Sample Files/base-hvac-multiple.xml:324-546`

**Root Cause**: The HARES curated fixture set mirrors a subset of OCHRE's HVAC fixtures focused on single-system configurations (furnace, AC, heat pump, mini-split). The `base-hvac-multiple.xml` fixture was never imported or tested.

**Impact**: Without multi-HVAC fixture testing, HARES cannot validate that `<PrimaryHeatingSystem>`/`<PrimaryCoolingSystem>` attributes are respected, that non-primary systems are handled correctly, or that capacity aggregation across multiple systems works.

**Contrast with OCHRE**: OCHRE's `test_hvac.py` operates on arbitrary system counts through its document-level iteration. OCHRE does not have a dedicated multi-HVAC unit test per se, but its architecture handles it through aggregation in `hpxml.py`. The `base-hvac-multiple.xml` fixture is listed in OCHRE's test matrix.

---

### Finding 2: No test coverage for multiple water heaters
**Severity**: critical

**Description**: HARES has zero tests for a building with multiple water heating systems. OCHRE's `base-dhw-multiple.xml` (564 lines) models 6 `<WaterHeatingSystem>` entries (electric resistance, heat pump, gas storage, gas tankless, electric tankless, indirect). The OCHRE shared-laundry-room fixture (`base-bldgtype-multifamily-shared-laundry-room-multiple-water-heaters.xml`) adds a second multi-WH scenario for multifamily shared systems.

**Code Location**:
- The only "multiple water heater" reference in HARES Rust code is a code comment unrelated to parsing: `crates/hares-equipment/src/water_heater/mod.rs:13`
- HARES fixture inventory (`hpxml_fixture_inventory.rs`) has no coverage assertion for water heater counts
- OCHRE reference fixture: `vendors/OCHRE/test/OS-HPXML Sample Files/base-dhw-multiple.xml:353-412`
- OCHRE shared fixture: `vendors/OCHRE/test/OS-HPXML Sample Files/base-bldgtype-multifamily-shared-laundry-room-multiple-water-heaters.xml`

**Root Cause**: HARES's curated fixture set was drawn from OCHRE's `base.xml` variations but the `base-dhw-multiple.xml` fixture was omitted.

**Impact**: Without multi-WH testing, HARES cannot validate that multiple water heaters are parsed, that their capacities/setpoints are independently resolved, or that aggregate DHW energy is correct. This is a significant gap for large homes and multifamily buildings where multiple WH systems are common.

---

### Finding 3: No integration test with HPXML 3.x fixture
**Severity**: high

**Description**: HARES's `validate_hpxml_schema()` at `validation.rs:74-122` explicitly supports HPXML 3.x (with a warning that "3.x may have untested element paths; 4.x is recommended"). Unit tests at `validation.rs:618-632` verify schema version acceptance/rejection at the string level. However, no full end-to-end integration test parses a real HPXML 3.x fixture through the complete parse_building → validate → resolve pipeline.

**Code Location**:
- 3.x version gate: `crates/hares-io/src/hpxml/validation.rs:103-109`
- Schema version unit tests only: `crates/hares-io/src/hpxml/validation.rs:618-632`
- All 38+ fixture files use `schemaVersion="4.0"` — zero 3.x fixtures in `tests/fixtures/`
- All inline XML in `hpxml_parsing_tests.rs` and `hpxml_parity.rs` uses `schemaVersion="4.0"`

**Root Cause**: HARES's validation module claims 3.x support but the test infrastructure only exercises 4.x paths. OCHRE also only ships 4.0 fixtures (425 files, all v4.0), so the OCHRE-derived curated fixture set naturally carries the v4.0 requirement forward.

**Impact**: Without 3.x integration tests, HARES cannot verify that deprecated 3.x element paths (e.g., `<EnergyFactor>` vs `<UniformEnergyFactor>`, `<WindowArea>` vs per-window `<Area>`, wall `<ConstructionType>` enum changes) produce correct warnings or fallbacks. The warning "3.x may have untested element paths" correctly describes the risk but does not mitigate it.

---

### Finding 4: No HPXML `<Pools>/<Pool>` parsing code or test coverage
**Severity**: high

**Description**: HARES has zero HPXML-side parsing for pool and spa loads. The HPXML schema defines `<Pools>/<Pool>` as a dedicated element under `<BuildingDetails>` for modeling pool pumps, pool heaters, and spa heaters. OCHRE has 5 fixtures with `<Pools>` elements. HARES only supports pool/spa loads through schedule CSV column mappings (`pool_pump`, `pool_heater`, `permanent_spa_pump`, `permanent_spa_heater` at `schedule_resolve.rs:127-144`) and a `default_gain_fractions` entry that assigns zero zone gain (`resolve_loads.rs:792-794`). The `<Pools>` HPXML element is never parsed.

**Code Location**:
- Schedule-side references (not HPXML parsing): `crates/hares-io/src/schedule_resolve.rs:127-144`
- Gain fractions only: `crates/hares-io/src/hpxml/resolve_loads.rs:792-794`
- Unit test for gain fractions (not parsing): `crates/hares-io/src/hpxml/resolve_loads.rs:1027-1036`
- No `<Pools>` element parsing anywhere in `crates/` (confirmed via grep)
- OCHRE pool fixtures (5 files):
  - `vendors/OCHRE/test/OS-HPXML Sample Files/base-misc-loads-large-uncommon.xml`
  - `vendors/OCHRE/test/OS-HPXML Sample Files/base-misc-loads-large-uncommon2.xml`
  - `vendors/OCHRE/test/OS-HPXML Sample Files/base-residents-1-misc-loads-large-uncommon.xml`
  - `vendors/OCHRE/test/OS-HPXML Sample Files/base-residents-1-misc-loads-large-uncommon2.xml`
  - `vendors/OCHRE/test/OS-HPXML Sample Files/base-misc-usage-multiplier.xml`

**Root Cause**: HARES's load resolution (`resolve_loads.rs`) focuses on `<MiscLoads>/<PlugLoad>` and `<MiscLoads>/<FuelLoad>`. Pool equipment is treated as external scheduled load only, without HPXML-side binding. The `<Pools>` HPXML path was never implemented.

**Impact**: Users who model pool/spa equipment via the HPXML `<Pools>` element (the standard path) cannot get those loads into HARES. Only schedule CSV-side pool/spa columns work. This is a parsing gap, not a test gap — the feature doesn't exist at all.

---

### Finding 5: No HPXML 4.0 BuildingSync integration or fixtures
**Severity**: high

**Description**: The HPXML 4.0 specification defines integration pathways with BuildingSync (an XML schema for commercial/residential building energy audit data, maintained by the U.S. DOE). BuildingSync allows audit data (e.g., from RESNET, BPI, or home energy scores) to feed into HPXML-based simulation tools. HARES contains zero references to BuildingSync anywhere in the codebase (confirmed via full-repository grep). No fixtures exercise audit-to-simulation data flows.

**Code Location**:
- Zero matches for "BuildingSync" in any `.rs` or `.xml` file in the repository
- The only match is in `scripts/reviews/hpxml.json:1` (this review's configuration file)
- HARES validation only checks `xmlns` contains `hpxmlonline.com` (`validation.rs:127-147`), never checks for BuildingSync namespaces or cross-walks
- OCHRE also has no BuildingSync fixtures — this is a gap shared with the reference implementation

**Root Cause**: BuildingSync integration is not a priority for the current HARES development phase, which focuses on HPXML-as-primary-input. However, HPXML 4.0 explicitly supports BuildingSync data import for audit-based workflows, and lack of any test scaffolding means the feature cannot be added later without foundational work.

**Impact**: HARES cannot accept BuildingSync audit data as an alternative input format or validate BuildingSync-to-HPXML cross-walks. This limits HARES's utility in home energy scoring, retrofit analysis, and utility program contexts where audit data is the primary input.

---

### Finding 6: No multifamily building type fixtures
**Severity**: high

**Description**: OCHRE ships 33 multifamily-specific HPXML fixtures covering attached dwellings, shared HVAC systems (boilers, chillers, cooling towers, water-loop heat pumps, ground-loop GSHPs), shared water heaters, shared PV, shared generators, shared mechanical ventilation, and compartmentalization testing. HARES has zero multifamily fixtures in its curated set or parity fixtures. The curated fixture inventory asserts only single-family detached coverage.

**Code Location**:
- HARES curated fixture set: `hpxml_fixture_inventory.rs:13-24` — all 10 fixtures model single-family detached homes
- OCHRE multifamily fixtures: 33 files under `vendors/OCHRE/test/OS-HPXML Sample Files/base-bldgtype-multifamily-*.xml`
- OCHRE shared-system fixtures include boiler/chiller/fan-coil, boiler/water-loop HP, ground-loop GSHP, shared laundry room with multiple WHs, shared PV, and shared mechanical ventilation

**Root Cause**: The HARES curated fixture set was drawn from OCHRE's single-family `base-*.xml` variations. Multifamily fixtures were excluded, likely because HARES's building model (`building.rs`) currently targets single-family geometry (e.g., single `BuildingID`, single-dwelling zone adjacency logic at `building.rs:2362`).

**Impact**: Multifamily is a major housing type. Without multifamily fixtures, HARES cannot validate shared system modeling, compartmentalization, multi-zone adjacency logic in the enclosure parser, or the interaction between multiple dwelling units in one HPXML document.

---

### Finding 7: Missing fixture classes for key equipment types
**Severity**: medium

**Description**: Several equipment types tested by OCHRE have zero HARES fixture coverage:

| Equipment Class | OCHRE Fixtures | HARES Fixtures | Notes |
|----------------|----------------|----------------|-------|
| Dual-fuel heat pump | 10 fixtures | 0 | Lockout temp tests exist in `hpxml_parity.rs:733-976` but no full dual-fuel fixture |
| Ground-source HP | 10 fixtures | 0 | GSHP has unique ground-loop modeling; no code path tested |
| Solar DHW | 6 fixtures | 0 | Solar thermal DHW (direct/indirect, flat-plate/evacuated-tube/ICS) completely untested |
| Evaporative cooler | 5 fixtures | 0 | No evap cooler parsing or resolution tested |
| Multiple buildings | 1 fixture | 0 | `base-multiple-buildings.xml` (60 KB, 1592 lines) — largest OCHRE fixture |
| Desuperheater | 7 fixtures | 0 | GSHP/HPWH desuperheater configurations untested |

**Code Location**:
- Dual-fuel lockout temp tests (no fixture): `crates/hares-io/tests/hpxml_parity.rs:733-976`
- Ground-source/GSHP: No references in HARES test files
- Solar DHW: No references in HARES test files or `resolve_water_heater.rs`
- Evaporative cooler: No references in HARES test files
- Multiple buildings: No `<Building>/<BuildingID>` multi-building parsing test
- Desuperheater: Untested; production code handles only combi-tankless rejection (`resolve_water_heater.rs:539-541`)

**Root Cause**: The curated fixture set focused on the most common single-family configuration (gas furnace + central AC + electric WH) with variations for PV, battery, enclosure, and appliances. The 10 fixtures represent approximately 2.4% of OCHRE's 425-fixture set.

**Impact**: Each missing equipment class represents an unvalidated code path. If any of these equipment types are parsed without tests, silent errors in capacity conversion, setpoint resolution, or fuel type assignment will go undetected.

---

### Finding 8: HPWH low-power path IS tested (confirming coverage)
**Severity**: low (informational)

**Description**: Contrary to the stated review concern, the HPWH low-power path HAS test coverage. Two tests validate it:
- `hpxml_parity.rs:1126-1173`: `low_power_hpwh_sets_ochre_hp_only_mode_and_defaults()` — validates UEF=4.9 triggers `hp_only_mode=true`, `COP=4.2`, `setpoint=60°C`, `tempering=51.67°C`
- `heat_pump_wh.rs:1438-1484`: `low_power_hpwh_thermal_capacity_is_1499_4()` — validates thermal capacity = 1499.4 W

The production code path is at `resolve_water_heater.rs:198-243` where `(uef - 4.9).abs() < 1e-9` gates the low-power branch.

**Impact**: This is not a gap. The low-power HPWH path is well-tested.

---

### Finding 9: `<MiscLoads>/<FuelLoad>` parsing has no dedicated test
**Severity**: medium

**Description**: The production code at `resolve_loads.rs:595` iterates `<FuelLoad>` children under `<MiscLoads>` and maps them to equipment names (grill, fireplace, lighting). EV plug loads are tested via `<PlugLoad>` at `hpxml_parsing_tests.rs:286-363`. The `<FuelLoad>` path has zero dedicated tests.

**Code Location**:
- FuelLoad parser (untested): `crates/hares-io/src/hpxml/resolve_loads.rs:595`
- PlugLoad EV tests: `crates/hares-io/tests/hpxml_parsing_tests.rs:286-363`
- OCHRE has FuelLoad coverage in `base-misc-loads-large-uncommon.xml` and variants

**Impact**: As with other parsing gaps, FuelLoad parsing may have bugs that go undetected until exercised by real data.

---

## Summary

- **Total findings**: 9 (1 informational, 8 actionable)
- **Critical**: 2 (multiple heating systems, multiple water heaters)
- **High**: 4 (HPXML 3.x integration, pool/spa parsing, BuildingSync integration, multifamily buildings)
- **Medium**: 2 (missing equipment fixture classes, FuelLoad parsing)
- **Low**: 1 (HPWH low-power path is actually covered — confirming existing coverage)

### Highest-priority missing fixture classes

Based on impact, frequency of real-world occurrence, and comparison with OCHRE's fixture priorities:

1. **Multiple HVAC systems** (`base-hvac-multiple.xml`) — many real homes have separate heating and cooling systems or supplementary systems
2. **Multiple water heaters** (`base-dhw-multiple.xml`) — large homes and multifamily common
3. **Multifamily building types** (any of the 33 OCHRE fixtures) — 30+% of U.S. housing stock
4. **Pool/Spa HPXML parsing** (`<Pools>` element) — present in 5 OCHRE fixtures, completely unimplemented in HARES
5. **Solar DHW** (`base-dhw-solar-*.xml`) — common in new construction and retrofits
6. **Dual-fuel heat pump** (`base-hvac-dual-fuel-*.xml`) — lockout temp logic exists but needs fixture validation
7. **Ground-source heat pump** (`base-hvac-ground-to-air-heat-pump*.xml`) — growing market share, distinct modeling
8. **HPXML 3.x integration fixture** — needed for production deployments encountering legacy data

## Recommendations

1. Import `base-hvac-multiple.xml` and `base-dhw-multiple.xml` from OCHRE into the HARES curated fixture set. Add inventory assertions and typed config resolution tests that validate equipment count, primary system designation, and independent capacity/setpoint resolution.

2. Add at least one multifamily fixture (e.g., `base-bldgtype-multifamily.xml` or `base-bldgtype-multifamily-shared-boiler-only-baseboard.xml`) to validate shared-system parsing, multi-zone adjacency, and compartmentalization.

3. Implement `<Pools>/<Pool>` HPXML parsing in `resolve_loads.rs` (or a new `resolve_pool.rs`) and add a fixture test using OCHRE's `base-misc-usage-multiplier.xml`.

4. Add a single HPXML 3.x end-to-end integration test fixture (e.g., using v3.4 namespace `http://hpxmlonline.com/2014/10`) that validates the 3.x warning path through the full parse → validate → resolve pipeline.

5. Add fixtures for solar DHW, dual-fuel HP, GSHP, and evaporative cooler, prioritizing by the presence of corresponding production code paths.

6. Add a test for `<MiscLoads>/<FuelLoad>` parsing covering gas grill, gas fireplace, and gas lighting load types.

7. Create a BuildingSync-to-HPXML integration test fixture or at minimum validate that BuildingSync namespaces in HPXML 4.0 documents are recognized and handled gracefully (not rejected as invalid `xmlns`).

## References / Citations

- HPXML 4.0 schema specification: `http://hpxmlonline.com/2023/09`
- HPXML 3.x namespace: `http://hpxmlonline.com/2014/10`
- HPXML BuildingSync integration: HPXML 4.0 §2.4 (BuildingSync audit data import)
- HPXML §6.5 window U-Factor/SHGC requirements: cited at `validation.rs:314-315`
- OCHRE HPXML parsing: `vendors/OCHRE/ochre/hpxml.py` (Python module)
- OCHRE test infrastructure: `vendors/OCHRE/test/test_dwelling/test_dwelling.py`
- HARES HPXML parsing entry point: `crates/hares-io/src/hpxml/mod.rs:86`
- HARES equipment resolution orchestration: `crates/hares-io/src/hpxml/equipment.rs`
