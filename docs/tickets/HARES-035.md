---
id: HARES-035
title: "hares-io — HPXML 4.0 Parser (Building and Envelope)"
kind: implement
depends_on: [HARES-002, HARES-001, HARES-009]
files_to_touch:
  - crates/hares-io/src/hpxml/mod.rs
  - crates/hares-io/src/hpxml/building.rs
  - crates/hares-io/src/hpxml/validation.rs
  - crates/hares-io/src/lib.rs
references:
  - docs/architecture/06-input-output.md
  - vendors/OCHRE/ochre/utils/hpxml.py
verification:
  - cargo check -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io -- -D warnings
---

## Background/Context
HPXML 4.0 is the industry-standard XML format for describing residential building characteristics. OCHRE's `hpxml.py` (1807 lines) loads zones, boundaries, and geometry from HPXML and derives RC thermal network parameters for the envelope. HARES replicates this pipeline in Rust using `quick-xml` for streaming parse performance. Validation is co-located so that input errors are caught at load time with precise, actionable messages before any simulation begins.

## Work to Do
- [ ] Implement `hpxml/building.rs`: HPXML structural extraction
  - [ ] Parse `Building/Site`: `Elevation`, `SiteType` (rural/suburban/urban), `ShieldingOfHome`
  - [ ] Extract zones: conditioned, attic, garage, foundation, outdoor — capture `ZoneType`, `FloorArea`, attached wall list
  - [ ] Extract boundaries: walls, roofs, windows, doors, foundation walls, slabs — capture area (m²), orientation (azimuth), R-value layers, assembly R-value
  - [ ] Extract window properties: area, orientation, U-factor, SHGC, frame type
  - [ ] Extract raw material layer data per boundary: `thickness_m`, `conductivity_w_m_k`, `density_kg_m3`, `specific_heat_j_kg_k`, `area_m2` — store in `MaterialLayer` structs on `Boundary`. Do NOT compute RC parameters here; RC computation belongs in `hares-envelope`, which consumes the parsed `Building` struct.
  - [ ] Unit conversion safety: HPXML uses mixed imperial/SI units. All parsed values MUST be converted to SI before storage: conductivity to W/m·K, U-factor from BTU/hr·ft²·°F to W/m²·K (multiply by 5.678), R-value from hr·ft²·°F/BTU to m²·K/W (multiply by 0.1761), area from ft² to m². Do NOT store imperial values.
  - [ ] Extract `DuctSystem` elements: leakage fraction, insulation R-value, location (inside/outside conditioned space) — store in a `DuctSystem` struct on the appropriate HVAC zone. Required by HVAC models in `hares-equipment`.
  - [ ] Represent results in a `Building` struct with typed sub-structs for `Site`, `Zone`, `Boundary`, `Window`, `DuctSystem`, `MaterialLayer`
- [ ] Implement `hpxml/validation.rs`: XSD schema validation and range checks
  - [ ] XSD schema validation: validate the parsed XML against the HPXML 4.0 XSD before any domain range checks; structurally invalid HPXML (wrong element names, missing required children, wrong types) must return `ValidationError` immediately
  - [ ] Floor area: 20–1000 m² (error outside range)
  - [ ] Infiltration ACH50: 0.5–30 (warn if outside range, matching architecture policy — not an error)
  - [ ] HVAC capacity: 1–200 kBtu/h (error outside range)
  - [ ] SEER2: 10–40 (error outside range)
  - [ ] HSPF2: 6–15 (error outside range)
  - [ ] WH setpoint: 40–70 °C (warn if < 49 °C per Legionella risk, matching architecture policy; error only if outside 40–70 range)
  - [ ] Battery round-trip efficiency: 0.70–0.99 (error outside range)
  - [ ] PV tilt: 0–90° (error outside range); warn if > 60°
  - [ ] Window-to-wall ratio: 0.02–0.40 (error outside range); warn if > 0.30
  - [ ] Return `ValidationReport { errors: Vec<ValidationError>, warnings: Vec<ValidationWarning> }`
- [ ] Implement `hpxml/mod.rs`: entry point `parse_hpxml(path: &Path) -> Result<Building>` that runs XSD validation, then structural parse, then domain range checks, failing on any error
- [ ] Re-export `Building`, `parse_hpxml`, `ValidationReport` from `lib.rs`

## Files to Touch
- `crates/hares-io/src/hpxml/mod.rs`: new file — `parse_hpxml` entry point, module declarations
- `crates/hares-io/src/hpxml/building.rs`: new file — `Building` struct and all envelope extraction logic (raw material layers, no RC computation)
- `crates/hares-io/src/hpxml/validation.rs`: new file — XSD validation, range checks, `ValidationReport`
- `crates/hares-io/src/lib.rs`: re-export new public types

## Measures of Success
- [ ] Parsing a ResStock HPXML fixture produces the correct zone list (type and floor area)
- [ ] `Boundary` structs carry raw `MaterialLayer` data (thickness, conductivity, density, specific_heat, area); no RC values are computed in this crate
- [ ] A `DuctSystem` element in HPXML produces a populated `DuctSystem` struct with leakage, insulation R-value, and location
- [ ] An HPXML file that violates the HPXML 4.0 XSD structure (e.g. a required element missing) returns a `ValidationError` before domain range checks run
- [ ] An HPXML with floor area of 5 m² returns a `ValidationError`; 1200 m² also returns an error
- [ ] ACH50 of 35 produces a warning, not an error
- [ ] WH setpoint of 45 °C returns a Legionella warning but not an error
- [ ] Window-to-wall ratio of 0.35 returns a warning; 0.45 returns an error
- [ ] Fiberglass batt at R-19 parses to ~0.040 W/m·K conductivity in SI

## Verification
- [ ] `cargo check -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io -- -D warnings` passes
