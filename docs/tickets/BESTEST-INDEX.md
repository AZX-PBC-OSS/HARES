# BESTEST Fix Tickets

Standardize layer ordering to outside-in and fix BESTEST test failures.

## Dependency Graph

```
BESTEST-001 (implement: layer ordering)
  ├── BESTEST-002 (review)
  └── BESTEST-003 (fix: IdealHVAC cooling, conditional)
        └── BESTEST-004 (review, conditional)
```

## Tickets

| ID | Title | Kind | Depends On | Status |
|----|-------|------|------------|--------|
| [BESTEST-001](BESTEST-001.md) | Standardize layer ordering to exterior-to-interior | implement | — | todo |
| [BESTEST-002](BESTEST-002.md) | Review layer ordering convention change | review | 001 | todo |
| [BESTEST-003](BESTEST-003.md) | Fix IdealHVAC cooling capacity for BESTEST 600 | fix | 001 | todo (conditional) |
| [BESTEST-004](BESTEST-004.md) | Review IdealHVAC cooling capacity fix | review | 003 | todo (conditional) |

## Parallelism

- BESTEST-001 must complete first (all other tickets depend on it)
- BESTEST-002 and BESTEST-003 can run in parallel after BESTEST-001
- BESTEST-003/004 are conditional — only needed if BESTEST 600 cooling still fails after the layer fix

## Notes

- Steps 1-4 in the plan are atomic: `boundary_rc.rs` + `conversions.rs` + `synthetic.rs` + TOML fixtures must change together. Splitting them would break all tests between steps.
- Film coefficients and window LWR injection were investigated and ruled out as root causes.
