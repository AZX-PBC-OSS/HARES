# HPXML Parsing Audit - Document Index

This index helps navigate the audit findings for HARES HPXML parsing.

## Quick Links

### Main Findings
**File:** `HPXML_AUDIT_REPORT.md`
- Executive summary
- 8 detailed findings with OCHRE vs HARES comparisons
- Impact descriptions and concrete examples
- Summary table of all issues
- Recommendations and verification steps

### Detailed Code References
**File:** `HPXML_AUDIT_DETAILED_REFERENCES.md`
- Exact line numbers and code snippets for each finding
- Side-by-side OCHRE vs HARES implementation
- Key constants and helper function references
- Water heater category mappings
- Conversion factor comparisons

### Remediation Checklist
**File:** `HPXML_AUDIT_CHECKLIST.md`
- Task checklist for fixing each issue
- Acceptance criteria for each fix
- Testing strategy and test cases
- Phased remediation priority (P1-P4)
- Sign-off template

---

## Issue Summary

| # | Title | Severity | Status | File |
|---|-------|----------|--------|------|
| 1 | HVAC Number of Speeds Not Determined | CRITICAL | TBD | `equipment.rs:166-271` |
| 2 | Startup Capacity Degradation Not Calculated | HIGH | TBD | `equipment.rs:166-271` |
| 3 | Water Heater UA Calculation Incomplete | HIGH | TBD | `water_heater_ua.rs`, `equipment.rs:336-452` |
| 4 | Heat Pump Backup Heating Incomplete | MEDIUM | TBD | `equipment.rs:187-224` |
| 5 | Auxiliary Power Calculation Mismatch | MEDIUM | TBD | `equipment.rs:119-124, 153-158` |
| 6 | Duct Parameters Incomplete | MEDIUM | TBD | `building.rs:113-118` |
| 7 | Water Heater Location Zone Mapping Missing | MEDIUM | TBD | `equipment.rs:446-449` |
| 8 | Efficiency Unit Normalization Mismatch | LOW-MEDIUM | TBD | `equipment.rs:1189-1241` |
| 9 | Efficiency Unit Clarification (verification) | LOW | TBD | Multiple |
| 10 | HVAC Setpoint Scheduling (verification) | LOW | TBD | `equipment.rs:1415-1468`, `building.rs:139-143` |

---

## Key Files Referenced

### HARES Source
- `crates/hares-io/src/hpxml/equipment.rs` - Main equipment resolution logic
- `crates/hares-io/src/hpxml/building.rs` - Zone and boundary parsing
- `crates/hares-io/src/hpxml/water_heater_ua.rs` - WH physics calculations
- `crates/hares-io/src/defaults.rs` - ZIP and HVAC curve loading

### OCHRE Reference
- `vendors/OCHRE/ochre/utils/hpxml.py` - HPXML parsing (lines 65-1161)
- `vendors/OCHRE/ochre/utils/equipment.py` - Equipment configuration (lines 88-158)
- `vendors/OCHRE/ochre/utils/envelope.py` - Envelope utilities

---

## Audit Methodology

This audit compared HARES Rust implementation against OCHRE Python reference implementation by:

1. **Direct code comparison** of HPXML field extraction
2. **Parameter tracing** from HPXML → equipment config
3. **Derived parameter identification** (values calculated by OCHRE, missing in HARES)
4. **Physics correctness check** (DOE procedures, engineering formulas)
5. **Concrete impact analysis** (how equipment behaves differently)

---

## Reading Order

### For Developers Fixing Issues
1. Start with `HPXML_AUDIT_CHECKLIST.md` (what to fix)
2. Read specific finding in `HPXML_AUDIT_REPORT.md` (why it matters)
3. Check `HPXML_AUDIT_DETAILED_REFERENCES.md` for code lines (implementation pattern)

### For Reviewers
1. Read `HPXML_AUDIT_REPORT.md` (executive summary + detailed findings)
2. Spot-check code references in `HPXML_AUDIT_DETAILED_REFERENCES.md`
3. Verify fixes against `HPXML_AUDIT_CHECKLIST.md` acceptance criteria

### For Testing/Validation
1. Use BESTEST cases listed in checklist
2. Run OCHRE vs HARES on same HPXML
3. Compare JSON equipment configs
4. Run simulation parity tests

---

## Finding Categories

### Equipment Behavior (Will cause wrong simulation results)
- Finding #1: Number of Speeds (affects all HVAC)
- Finding #2: Startup C_D (affects AC/HP startup)
- Finding #3: WH UA (affects WH standby losses)
- Finding #5: Auxiliary Power (affects fan/pump power)

### Equipment Control (Will prevent proper operation)
- Finding #4: Backup Heating (HP won't switch to backup)
- Finding #7: WH Zone (heat gains go to wrong zone)

### Optimization/Defaults (May use suboptimal values)
- Finding #6: Duct Parameters (DSE can't be calculated)
- Finding #8: Efficiency Units (possible model interpretation errors)

### Verification Needed
- Finding #9: Efficiency unit handling
- Finding #10: HVAC setpoint profiles

---

## Contact & Questions

Audit completed: March 2026
Reference implementation: OCHRE (vendors/OCHRE/)
Audit scope: HPXML field extraction → equipment configuration loading

For questions or clarifications:
- Check the specific finding in `HPXML_AUDIT_REPORT.md`
- Review code references in `HPXML_AUDIT_DETAILED_REFERENCES.md`
- Consult OCHRE source code (provided in vendors/ directory)

