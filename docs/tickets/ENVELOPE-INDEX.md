# ENVELOPE Ticket Series — RC Network Diagnostics & Conduction Parity

## Root Cause Identified (2026-03-25)

The 25% HVAC heating over-prediction vs OCHRE is **fully explained** by an OCHRE
window UFactor unit conversion bug:

- HPXML `<UFactor>0.37</UFactor>` is in BTU/(hr*ft2*F) per spec
- OCHRE reads 0.37 and uses it as SI W/(m2*K) — no conversion
- Correct SI: 0.37 * 5.678 = **2.1 W/(m2*K)** (HARES is correct)
- OCHRE treats windows as 5.7x more insulating than they are
- Delta UA = 27 W/K * ~20C winter delta = **~540W** excess conduction in HARES
- Measured HVAC delta: 2699W - 2160W = **539W** — exact match

**All other RC parameters match OCHRE exactly**: UA (+/-0.3%), film R (exact),
capacitance (exact), area (exact). HARES envelope physics is correct.

## Progress Summary (2026-03-25)

After OCHRE window U-factor fix + HARES window interior LWR:
- Winter ideal: +3.2% (82W) — down from +25%
- Spring ideal: +15.4% (221W) — needs LWR split fix
- Dynamic winter: -3.1% — slight overcorrection
- Summer cooling: -55% — LWR split is the dominant remaining issue

## Remaining Work

- **ENVELOPE-008** (HIGH): Split LWR injection between RC node and zone air. Root
  cause of wall/roof/mass component discrepancies. OCHRE injects `radiation_frac` to
  node, `(1-radiation_frac)` to zone air. HARES puts 100% to node.
- ENVELOPE-002: 15% winter beam solar excess (22W, minor)
- ENVELOPE-003: Missing occupancy internal gains (~8W, minor)
- ENVELOPE-007: Tighten tolerances after fixes

## Critical Path

```
Phase 1 — DONE:
  ENVELOPE-001 (expose EnvelopeDiagnostics) — DONE
  ENVELOPE-004 (RC parity test) — DONE (OCHRE window bug found)
  ENVELOPE-005 (wire diagnostics for all boundary types) — DONE (window LWR added)

Phase 2 — next:
  ENVELOPE-008 (split LWR injection node/zone-air) — HIGH PRIORITY
  ENVELOPE-002 (solar gain investigation) — data collected, 15% beam delta remains
  ENVELOPE-003 (occupancy gains) — minor

Phase 3 — terminal:
  ENVELOPE-007 (tighten tolerances)
```

## Tickets

| ID | Title | Kind | Depends | Status |
|----|-------|------|---------|--------|
| ENVELOPE-001 | Store and expose EnvelopeDiagnostics from Dwelling | implement | — | done |
| ENVELOPE-002 | Investigate and reconcile solar gain vs OCHRE (beam + diffuse) | investigate | — | data collected |
| ENVELOPE-003 | Fallback occupancy internal gains | implement | — | open |
| ENVELOPE-004 | Extend beopt_ua_parity with per-boundary RC comparison | implement | 001 | done |
| ENVELOPE-005 | Wire boundary diagnostics for all boundary types | fix | 001 | done |
| ENVELOPE-006 | Per-node RC network dump for deep debugging | implement | 001 | deferred |
| ENVELOPE-007 | Tighten conditioned oracle tolerances | implement | 002, 003, 008 | deferred |
| ENVELOPE-008 | Split interior LWR injection between RC node and zone air | fix | 005 | open |
