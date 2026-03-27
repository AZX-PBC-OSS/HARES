# RV-Series: Code Review Findings

22 tickets (from 48 review findings) across 4 execution waves. Source: `docs/review-findings.md`.

---

## Recent Progress

**Session 2026-03-27 — review-findings fix session (24 findings addressed).**

Tickets partially advanced:

| Ticket | Done | Pending |
|--------|------|---------|
| RV-001a | `finished` flag, `advance()` sets flag, `step()` returns `None` when finished | Accessor index clamping; `is_finished()` method |
| RV-001b | Per-rate ratchet via `effective_peak_for_period()`; `prior_period_peaks` history | Global `ratchet_config` / `find_map` cleanup for coincident demand |
| RV-001c | `finalize()` method created; double-call guard via `reset()` | `sim_end` parameter; partial-period proration of fixed charges |
| RV-005 | T2-6 thermal_update clone eliminated; T2-8 timestamp_buf; telemetry `clone_from`; zone column HashMaps | EnvironmentManager `update()` in-place refactor; solar/schedule/zone Vec prealloc; InvariantChecker field |
| RV-007 | T2-7 `lwr_by_zone_buf` swap (zero-copy); T2-9 partial `infiltration_by_zone_buf` prealloc + swap | T2-3 zone_temps prealloc in `format_domain_update`; T2-9 full consolidation of 8 infiltration lookups |
| RV-004 | Confirmed closed — DutyCycle already implemented | — |
| RV-008 | Confirmed subsumed by RV-005; T2-6 and T2-8 complete; T2-5 documented as structurally unavoidable | — |

---

---

## Wave 1 — Correctness Bugs (fix first, all independent)

| ID | Title | Kind | Crate(s) | Depends On |
|----|-------|------|----------|------------|
| [RV-001a](RV-001.md) | TariffEvaluator: bounds-check accessors (OOB panics) | fix | hares-tariff | **Done** |
| [RV-001b](RV-001b.md) | TariffEvaluator: fix ratchet drop for non-first demand rates | fix | hares-tariff | **Done** |
| [RV-001c](RV-001c.md) | TariffEvaluator: fix partial-period overbilling in finalize() | fix | hares-tariff | **Done** |
| [RV-001d](RV-001d.md) | TariffEvaluator: configurable demand window duration | fix | hares-tariff | **Done** |
| [RV-002](RV-002.md) | batch_step RL actions: raise NotImplementedError | fix | hares-python | **Done** |
| [RV-003](RV-003.md) | GridExportRule: wire into BMS evaluate_mode | fix | hares-core | **Done** |
| ~~[RV-004](RV-004.md)~~ | ~~DutyCycle control signal~~ (already implemented) | — | — | Closed |

## Wave 2 — Hot-Loop Performance (all independent)

| ID | Title | Kind | Crate(s) | Depends On |
|----|-------|------|----------|------------|
| [RV-005](RV-005.md) | Dwelling hot-path allocation elimination (env + step loop) | refactor | hares-core | — |
| [RV-006](RV-006.md) | StratifiedTank: preallocate scratch buffers (~3.15M allocs/year) | refactor | hares-equipment | **Done** (pre-existing) |
| [RV-007](RV-007.md) | ThermalSolver: prealloc, dedup infiltration lookups, borrow lwr_buf | refactor | hares-envelope | **Done** (all items implemented) |
| [RV-007b](RV-007b.md) | ThermalSolver: precompute GlazingCurve (verify immutability first) | refactor | hares-envelope | **Done** (pre-existing) |
| ~~[RV-008](RV-008.md)~~ | ~~Dwelling step loop alloc cleanup~~ | — | — | Subsumed by RV-005 |

## Wave 3 — Missing Tests + Invariant Wiring

| ID | Title | Kind | Crate(s) | Depends On |
|----|-------|------|----------|------------|
| [RV-009](RV-009.md) | Cooling coil psychrometrics tests (calculate_shr, AO, BPF) | test | hares-equipment | **Done** |
| [RV-010](RV-010.md) | Battery degradation tests (RainflowCounter, DegradationState) | test | hares-equipment | **Done** |
| [RV-011](RV-011.md) | HVAC staging + duct DSE tests (392 lines, zero tests) | test | hares-equipment | **Done** |
| [RV-012](RV-012.md) | Infiltration methods + multi-zone humidity coupling tests | test | hares-envelope | **Done** |
| [RV-013](RV-013.md) | EV driver full departure/drive/arrival/plug-in cycle test | test | hares-equipment | — |
| [RV-014](RV-014.md) | Wire tank temp, SOC, thermal balance, moisture balance invariants | implement | hares-core, hares-envelope | HARES-071, RV-005 |

## Wave 4 — Parity, Test Quality, Code Quality (all independent)

| ID | Title | Kind | Crate(s) | Depends On |
|----|-------|------|----------|------------|
| [RV-015](RV-015.md) | RoomAC equipment type (~15% of ResStock) | implement | hares-equipment | — |
| [RV-016](RV-016.md) | Parity gaps design (islanded, generic, EVI-Pro) | implement | (design doc) | — |
| [RV-017](RV-017.md) | Test collision fixes (temp paths); remaining items tracked | fix | multiple | **Partial** |
| [RV-018](RV-018.md) | Smoke test numeric assertions | test | tests | — |
| [RV-019](RV-019.md) | Sentinels, panics, clippy, zero-div guard | fix | hares-core, hares-equipment | **Done** |
| [RV-020](RV-020.md) | Config-driven summer months, duck typing, thresholds | fix | hares-tariff, hares-python, hares-core | **Done** (pre-existing) |

---

## Structural Changes from Reviews

**K2.5 review:**
1. **RV-001 split into RV-001a/b/c/d** — 4 atomic tickets instead of 1 compound ticket
2. **RV-008 subsumed by RV-005** — avoid concurrent `dwelling/mod.rs` modifications
3. **RV-007 T2-10 split into RV-007b** — GlazingCurve precomputation requires immutability verification
4. **RV-002 explicit decision** — raises `NotImplementedError` (not conditional if/else)
5. **RV-014 contract specified** — `ThermalSolver::balance_inputs()` interface documented

**GLM-5 review:**
6. **RV-004 closed** — DutyCycle already implemented in `control_signal.rs:65-73` and 6 equipment files
7. **RV-001a design clarified** — return last valid value (clamped index), not `Option`, to avoid breaking callers
8. **RV-003 formula fixed** — `LimitedExport` uses `min(dispatch, load + max_kw)` not subtraction
9. **RV-014 depends on RV-005** — explicit ordering for `dwelling/mod.rs` file conflict
10. **RV-019 save_postcard decision** — add `try_save_postcard` alongside existing, don't change 20+ call sites
11. **RV-019 InvariantChecker dedup** — handled by RV-005, removed from RV-019
12. **RV-001c finalize idempotency test added** — prevent double-billing on repeated `finalize()` calls

## Dependency Graph

```
All tickets independent EXCEPT:
  RV-014 ──depends on──> HARES-071
  RV-014 ──depends on──> RV-005  (dwelling/mod.rs file conflict)

File conflict zones (sequenced, not parallel):
  dwelling/mod.rs:  RV-005 must complete before RV-014
  evaluator.rs:     RV-001a/b/c/d are independent (different code sections)
```

## Finding → Ticket Cross-Reference

| Finding | Ticket | | Finding | Ticket |
|---------|--------|-|---------|--------|
| T1-1 | RV-002 | | T3-5 | RV-012 |
| T1-2 | RV-001a | | T3-6 | RV-012 |
| T1-3 | RV-003 | | T3-7 | RV-013 |
| T1-4 | RV-004 | | T4-1 | RV-014 |
| T1-5 | RV-001b | | T4-2 | RV-014 |
| T1-6 | RV-001c | | T4-3 | RV-014 |
| T2-1 | RV-005 | | T4-4 | RV-014 |
| T2-2 | RV-005 | | T5-1 | RV-015 |
| T2-3 | RV-007 | | T5-2 | RV-016 |
| T2-4 | RV-006 | | T5-3 | RV-016 |
| T2-5 | RV-005 | | T5-4 | RV-012 |
| T2-6 | RV-005 | | T5-5 | RV-016 |
| T2-7 | RV-007 | | T6-1 | RV-017 |
| T2-8 | RV-005 | | T6-2 | RV-018 |
| T2-9 | RV-007 | | T6-3 | RV-017 |
| T2-10 | RV-007b | | T6-4 | RV-017 |
| T3-1 | RV-009 | | T6-5 | RV-017 |
| T3-2 | RV-010 | | T6-6 | RV-017 |
| T3-3 | RV-011 | | T7-1 | RV-019 |
| T3-4 | RV-011 | | T7-2 | RV-020 |
| | | | T7-3 | RV-020 |
| | | | T7-4 | RV-019 |
| | | | T7-5 | RV-001d |
| | | | T7-6 | RV-019 |
| | | | T7-7 | RV-019 |
| | | | T7-8 | RV-020 |
| | | | T7-9 | RV-019 |
