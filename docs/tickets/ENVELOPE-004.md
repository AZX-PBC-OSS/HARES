---
id: ENVELOPE-004
title: Extend beopt_ua_parity with per-boundary RC comparison
kind: implement
depends_on: [ENVELOPE-001]
files_to_touch:
  - tests/structural_envelope_oracle.rs
references:
  - tests/fixtures/parity/ochre_rc_reference.json
  - tests/fixtures/parity/extract_ochre_rc.py
  - vendors/OCHRE/ochre/utils/envelope.py
verification:
  - cargo test --test structural_envelope_oracle -- beopt_ua_parity --nocapture
---

## Background/Context

`tests/structural_envelope_oracle.rs` already has a `beopt_ua_parity` test that
compares per-boundary UA values against hardcoded OCHRE constants at 1% tolerance.
This ticket extends that test with additional per-boundary columns (film R,
capacitance, node count) loaded from `ochre_rc_reference.json`, and adds aggregated
window comparison to diagnose the window UA gap.

## Work to Do

- [x] Load `ochre_rc_reference.json` instead of hardcoded UA constants
- [x] Per-boundary comparison table: UA, R_film_int, R_film_ext, capacitance, n_nodes
- [x] Aggregate HARES per-orientation boundaries to match OCHRE consolidated boundaries
- [x] Aggregate unmatched window boundaries and compare to OCHRE "Window" entry
- [x] Maintain existing 1% UA tolerance for opaque boundaries
- [x] Log window UFactor unit conversion discrepancy

## Findings (2026-03-25)

### All opaque boundaries: PERFECT match

| Parameter | Max deviation |
|-----------|--------------|
| UA (W/K) | +/-0.3% |
| R_film_int (m2.K/W) | exact |
| R_film_ext (m2.K/W) | exact |
| Capacitance (kJ/K) | exact |
| Area (m2) | exact |

### Window UFactor unit conversion bug in OCHRE

OCHRE reads HPXML `<UFactor>0.37</UFactor>` (BTU/hr.ft2.F per HPXML spec) and uses
it as SI W/(m2.K) without conversion:

| | HARES | OCHRE |
|-|-------|-------|
| Window U-factor | 2.101 W/(m2.K) | 0.370 W/(m2.K) |
| Window UA total | 32.79 W/K | 5.77 W/K |
| Window R | 0.476 m2.K/W | 2.703 m2.K/W |

- HARES correctly converts: 0.37 * 5.678 = 2.101 W/(m2.K)
- OCHRE uses raw imperial value as SI: 1/0.37 = 2.703 m2.K/W
- Delta UA = 27 W/K. At 20C winter delta = **~540W excess conduction**
- Measured HVAC delta: 2699W - 2160W = 539W — **exact match**

### Node count differences (cosmetic)

HARES keeps per-orientation walls separate; OCHRE consolidates. Ext Wall: 16 vs 4
(4 orientations x 4 nodes). Does not affect UA, which aggregates correctly.

### Total UA

HARES 585.76 vs OCHRE 558.74 (+4.8%). The 27 W/K gap is entirely from windows.

## Verification

- [x] `cargo test --test structural_envelope_oracle -- beopt_ua_parity --nocapture` passes
- [x] Comparison table logged with all columns
- [x] Window aggregation and UFactor analysis logged
