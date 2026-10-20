---
coverage_outcome: adequate
new_test_paths:
  - tests/python/test_equipment_zip_voltage_response.py
  - crates/hares-core/src/dwelling/premise_zip.rs
  - crates/hares-equipment/src/scheduled_load.rs
  - crates/hares-io/src/schedule.rs
  - crates/hares-types/src/schedule.rs
  - crates/hares-equipment/src/schedule_helpers.rs
  - crates/hares-equipment/src/event_load.rs
  - crates/hares-io/src/defaults.rs
  - crates/hares-equipment/src/config.rs
  - crates/hares-types/src/zip.rs
failing_tests: []
---

# I-01 coverage triage (eighth pass)

## What I reviewed (content, not titles)

- `coverage-note.md` — new "v0-validation round": the fix phase extended
  `validate_zip_sums` with the non-positive-`v0` rejection (the red
  team's noticed-but-unpinned division hazard, which my pass-5 triage
  verified closed) plus a new magnitude-bounds guard; the coverage step
  added the missing non-positive-`v0` arm.
- The guard in source (`crates/hares-types/src/zip.rs:140`,
  `validate_plausible_magnitudes`): bounds every coefficient and `v0`,
  with a why-comment that carries a genuine insight — a row like
  `zp = 1.79e308, ip = -1.79e308, pp = 1.0` cancels to exactly 1.0 at
  nominal voltage, passing sum checks and any single-voltage probe, yet
  produces `inf − inf = NaN` at 1.05 pu. Bounds make the overflow class
  unrepresentable rather than sampling it — the constitution's preferred
  fix shape. Wired into the init funnel at `config.rs:538-540`.
- Tests verified to exist: `magnitude_bounds_reject_overflow_class_rows`
  and `magnitude_bounds_accept_every_literature_row` (`zip.rs:765,791` —
  the accept arm iterates every literature row, so the bound can't drift
  below legitimate data) and `validate_zip_sums_rejects_non_positive_v0`
  (`config.rs:978`).
- Re-ran every claimed command on this checkout; all figures match:
  `hares-equipment` 2184, `hares-types` 394, `hares-core` 904,
  `hares-io` 918 — all 0 failed; Python ZIP suite 10 passed.

## Assessment

This round is the tail of the red-team Finding 1 work and is handled
correctly at both ends: the fix extended validation at the shared funnel
(init-time `validate_zip_sums`, so every ZIP-bearing equipment gets it),
and the coverage step added only the arm the fix phase had not covered
(zero/negative `v0` — finite, so only the positivity check guards it),
rather than duplicating the overflow arms. The accept-every-literature-row
test is the right guard against the bound itself becoming a false
rejection.

Every finding routed across the eight passes is resolved with the pinned
or recommended semantic and verified green on this checkout. The
validation ladder for the ZIP channel is now: TOML boundary rejection →
init-time finiteness + sum + positivity + magnitude funnel →
premise-aggregate finiteness backstop — each layer tested.

## Remaining gaps — no action (standing dispositions, re-verified)

- **DER `real_power_zip_applies=False` / `equipment_zip() → None` at
  Python level** — covered at Rust level; fixture-limited;
  disproportionate to close.
- **`resample` remainder-drop** — pre-existing, untouched by this diff,
  named owner (parity ticket); recoverable, not silent.
- **`ScheduleSource::mean()` equal-weight averaging (~1%)** — documented,
  bounded approximation feeding an aggregate that is itself an estimate.

## Verdict

**adequate.** The v0/magnitude round closes the last noticed item on the
ZIP-validation channel with a class-eliminating guard and correctly
scoped tests; all claimed commands reproduce; no finding lacks a
disposition. No follow-up action items.
