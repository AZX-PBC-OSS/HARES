# ENV Ticket Index — Envelope Construction OCHRE Parity

Source: [Envelope Rewrite Plan](../../.claude/plans/mutable-plotting-sonnet.md)

Cross-validated by Kimi K2.5 and GLM-5 reviews (2026-03-23).

**Problem**: HARES cools 1.8x faster than OCHRE for the BEopt example building. Effective UA ~1000+ W/K vs OCHRE's 568 W/K. Root causes: slab→outdoor (not ground), window 5x R-value error, wrong LUT boundary names, missing foundation/slab insulation parsing.

**Relationship to FX tickets**: FX-034 (ZoneType::Ground) overlaps with ENV-002. ENV-002 takes the simpler approach (map "ground" → Foundation) vs FX-034's new enum variant. Coordinate accordingly.

## Sequential Execution Order

| Ticket | Title | Depends On | Files |
|--------|-------|------------|-------|
| [ENV-001](ENV-001.md) | EnvelopeDiagnostics capture struct + diagnostic test | — | `boundary_rc.rs`, `structural_envelope_oracle.rs` |
| [ENV-002](ENV-002.md) | Fix slab ground connection | ENV-001 | `building.rs`, `structural_envelope_oracle.rs` |
| [ENV-003](ENV-003.md) | Window U-factor EnergyPlus decomposition | ENV-001 | `solar.rs`, `conversions.rs` |
| [ENV-004](ENV-004.md) | Raised floor LUT boundary name | ENV-001 | `envelope_lut.rs` |
| [ENV-005](ENV-005.md) | Foundation wall insulation parsing + area scaling | ENV-001 | `building.rs` |
| [ENV-006](ENV-006.md) | Slab insulation detail parsing | ENV-002 | `building.rs` |
| [ENV-007](ENV-007.md) | Restrict insulation_details to foundation/slab only | ENV-005, ENV-006 | `building.rs` |
| [ENV-008](ENV-008.md) | OCHRE oracle extraction script | ENV-001 | `extract_ochre_rc.py` |
| [ENV-009](ENV-009.md) | Asserting UA parity test | ENV-002–ENV-007, ENV-008 | `structural_envelope_oracle.rs` |
| [ENV-010](ENV-010.md) | Wall area reduction by window/door subtraction | ENV-001 | `building.rs` |

## Dependency Graph

```
ENV-001 (diagnostics)
  ├── ENV-002 (slab ground)
  │     └── ENV-006 (slab insulation)
  │           └── ENV-007 (restrict insulation_details) ─┐
  ├── ENV-003 (window U-factor)                          │
  ├── ENV-004 (raised floor)                             │
  ├── ENV-005 (foundation wall)                          │
  │     └── ENV-007 ─────────────────────────────────────┤
  ├── ENV-008 (oracle script)                            │
  └── ENV-010 (wall area reduction)                      │
                                                          ▼
                                                ENV-009 (asserting UA test)
```

## Maximum Parallelism

**After ENV-001**: ENV-002, ENV-003, ENV-004, ENV-005, ENV-008, ENV-010 can all run in parallel.

**After ENV-002 + ENV-005**: ENV-006, ENV-007 can run.

**After all**: ENV-009 validates everything.

## File Overlap Coordination

- `building.rs`: ENV-002, ENV-005, ENV-006, ENV-007, ENV-010 all touch this file. **Must be sequenced**: ENV-002 → ENV-005 → ENV-006 → ENV-007. ENV-010 can run in parallel with ENV-002.
- `structural_envelope_oracle.rs`: ENV-001, ENV-002, ENV-009. Sequenced by dependency.
- All other files have no overlap.
