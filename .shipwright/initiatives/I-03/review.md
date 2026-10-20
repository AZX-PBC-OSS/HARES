---
review_outcome: clean_with_nits_applied
---

# Review: I-03 — HPXML-declared EV ignores its charging strategy

The diagnosis and the fix are correct. I traced the root cause to the change,
ran the reproduction tests and the full verification gates myself, and found
exactly one defect — a one-line gap in a strict parser this same change
introduced, which I fixed directly and pinned with a regression test. The one
larger concern I initially raised did not survive scrutiny as a blocking
finding: it names no live broken caller and is the intended direction of the
fix, so it is recorded below as an observation rather than a finding.

## The fix, verified from root cause to change

- **Root cause (read at HEAD, pre-fix):** `Ev::actor_seed()`
  (`crates/hares-equipment/src/ev/mod.rs`) short-circuited
  `ChargingStrategy::Immediate` to `None` —
  `if matches!(self.charging_strategy, ChargingStrategy::Immediate { .. }) { return None; }` —
  withholding the only `EvDriverActor`, so a strategy-less/`Immediate` EV never
  left home, never discharged, and held 0.0 kW forever. Confirmed by reading
  the committed version (`git show HEAD:crates/hares-equipment/src/ev/mod.rs`).
- **The change:** the working tree's `actor_seed` now returns a seed for every
  strategy (`ev/mod.rs:1430`). That seed is consumed by
  `build_actors_from_seeds` (`hares-core/src/dwelling/mod.rs:6783`), attached by
  `auto_register_actors` (`:2983`), and re-run for late-added EVs by python
  `add_ev` (`hares-python/src/py_dwelling.rs:1084`) and by `add_ev_with_driver`'s
  canonical `EvDriver:<label>` dedup name (`py_dwelling.rs:1173`).
- **Reproduction tests now pass.** The three tests `reproduction.md` recorded as
  failing pre-fix — `actor_seed_default_strategy_returns_ev_seed`,
  `actor_seed_immediate_strategy_returns_ev_seed`,
  `actor_seed_returns_ev_seed_for_every_charging_strategy` — are now green
  (6 `actor_seed_*` tests hit, 6 passed). The end-to-end reported surface is
  pinned green too: `test_hpxml_declared_ev_default_strategy_charges` and the
  add_ev-with/without-tariff tests are inside the 21 passing python tests below.

Honesty note on "before/after": I did not re-run the pre-fix red state for the
`actor_seed` fix, because the working tree holds the fix and I will not revert
it; the pre-fix defect is established by reading the committed code plus
`reproduction.md`'s recorded failures. For the LUT fix I performed, I observed
both states directly (below).

## Fix applied (trivial, one line)

`harvest_kv_f64` (`crates/hares-equipment/src/pv/lut.rs:143`) — the strict LUT
metadata parser this change itself introduced — validated parseability only.
`f64::from_str` accepts `"nan"` and `"inf"` as valid parses, so a hand-edited
LUT carrying those for `harvest_lut_latitude_deg`, `harvest_lut_longitude_deg`,
`harvest_lut_sam_inv_eff`, or `harvest_lut_sam_losses` still sailed through and
silently poisoned PV output (e.g. `sam_inv_eff = NaN` multiplies into AC power
at `pv/mod.rs:160`), the exact class the strict parse was added to close.

I proved it first with a failing test, then applied the fix: reject non-finite
values after parsing.

- Before fix: `cargo test -p hares-equipment --lib pv::lut::tests::harvest_kv_rejects_non_finite_values -- --exact` → **FAILED** (`harvest_kv_f64 accepted non-finite 'nan' … as NaN`).
- After fix: `cargo test -p hares-equipment --lib pv::lut::tests::harvest_kv` → **2 passed** (the new `harvest_kv_rejects_non_finite_values` plus the existing `harvest_kv_rejects_non_numeric_values`).

**Class measured, instance fixed as a class.** I searched the change for every
other newly-added numeric string parse:
`git diff HEAD -- crates/ | grep -E 'parse::<f64>|\.parse\(\)'` returns exactly
one added line — the `harvest_kv_f64` body. Its sibling `harvest_kv_u8` cannot
produce a non-finite value (`u8`). The other new parsers run through
`serde_json::from_str` + `strategy.validate()`/`policy.validate()`, and
`validate_fraction` (`hares-types/src/equipment.rs:1018`) checks `is_finite`.
So the non-finite gap was a single instance within this change, and it is now
closed, pinned by a test.

## Observations

- **`add_ev` now always attaches a default driver; the "coexists" claim is not
  tested past the driver's active window.** `py_dwelling.rs:1084` makes every
  `add_ev` provision a default `EvDriverActor` (departs at `Constant(480.0)`
  minutes, 30 mi/day). That is the correct direction — it is the same bug the
  ticket is about — but it removes the previously driver-less `add_ev`, and the
  Implementation Notes' "Manual ControlSignal control coexists (40/40)" evidence
  is `TestEvControlSignals`, whose scenarios run ≤10 simulated minutes
  (`duration_s=600, time_res_s=60`), so the driver's 08:00 departure never
  fires. The claim therefore establishes coexistence only while the driver is
  dormant. No live caller currently exercises manual control of an `add_ev` EV
  past 08:00, so this is not a proven regression; it is a coverage gap worth a
  follow-up test (manual `ev_drive`/`ev_plug_in` on an `add_ev` EV over a
  >24 h window, asserting the two controllers do not double-deplete SOC) or an
  explicit opt-out flag if driver-less EVs remain a supported use case. Noted,
  not re-opened.
- **Stale Known Limitation.** `initiative.md` "### Known Limitations" #1 says
  `Ev::new()` "still silently falls back to `Immediate`". The code moved past
  that in the class-hardening round: `defer_strict` → `init_error`
  (`ev/mod.rs:275-282,292`) surfaces the error at `init()` (`:1254-1260`). The
  note should be corrected so the next reader doesn't hunt a dead path.
- **`charging_priority` has two spellings for two surfaces.** Raw path
  (`parse_charging_priority`, case/whitespace-insensitive) vs typed path
  (serde, exact-case enum). No live drift (all typed sites set `None`), but the
  "one source of truth for both surfaces" note overstates for this one field.
- **Pre-existing, out of scope:** `git diff HEAD` shows the change added no
  other `parse::<f64>`/`.parse()` sites, but the tree still contains the broad
  pre-existing `f64::from_str(...).ok()`-style surface in `hares-io`
  (`resolve_loads.rs`, `resolve_water_heater.rs`, `xml_helpers.rs`,
  `defaults.rs`, `resstock_csv.rs`, `tmy3.rs`) that would also accept a literal
  `"nan"` from a source file. That is HPXML/input parsing, predates this change,
  and is outside the config-override boundary this initiative owns. Flagged for
  awareness, not fixed here.

## Acknowledged limitations (conceded)

- **HPXML v3 `Systems/Vehicles/Vehicle` ignored** — a real pre-existing parser
  gap, correctly scoped out; both remedies change HPXML acceptance behaviour the
  operator owns.
- **Battery `actor_seed` returns `None` for `BmsMode::Manual`** — deliberate and
  correct (a manually driven battery needs no use-simulation).
- **`ScheduleSourceConfig` cannot carry `deny_unknown_fields`** — real
  serde_derive constraint; documented at the type rather than suppressed.

## Reflection

- **Goal.** The ticket asked (a) should the override path reject rather than
  accept-and-ignore, and (b) should a strategy-less EV really never charge.
  Both are answered and wired: (b) via `actor_seed` provisioning every strategy
  plus `add_ev` re-registration; (a) via the strict `init`/`init_typed` parses
  that now fail loudly. Acceptance surfaces pass (21 python tests incl. the
  HPXML-declared default-strategy case).
- **Wiring.** Every declared surface has a named supplier and reader:
  `apply_signal` (supplied by the equipment impls; read by
  `apply_control_unchecked`/`apply_control`, `lib.rs:184,224`);
  `dispatch_target_name` (`EvDriverActor`/`BatteryManagementActor`,
  `ev_driver/mod.rs:796`, `bms.rs:1020`; read by `evict_actors_targeting`/
  `clear_equipment`, `dwelling/mod.rs:2914,3224`); `Ev.init_error` (written by
  `defer_strict`, read by `Ev::init`); `add_actor`→`Result` (all call sites
  propagate or unwrap, no discarded `Result`).
- **Downgrades/reclassifications.** (1) The `add_ev` competing-driver concern:
  downgraded Warning→Observation because no live caller reaches the conflict and
  the direction is the intended fix; the residual is a test gap, named above.
  (2) `clear_equipment` retaining `dispatch_target_name()==None` actors: dropped
  because `BatteryManagementActor` only ever constructs `DispatchTarget::ByName`
  (`bms.rs:98`), so the `ByEndUse` arm has no live constructor.
  (3) `harvest_kv_f64` non-finite gap: raised as a finding, then fixed by me as
  a one-line class-closing change and demoted to "fix applied."
- **Strongest case against passing clean.** "A one-line hole in a parser you
  only found by writing a failing test means the suite still had a blind spot."
  Answer: the hole is now fixed *and* the test that would have caught it is
  permanently in the suite; the gate that mattered is the class search, which
  showed this was the only instance in the change.

## Verified

- `git show HEAD:crates/hares-equipment/src/ev/mod.rs` → `actor_seed` returned
  `None` for `Immediate` (the pre-fix bug).
- `cargo test -p hares-equipment --lib actor_seed_` → 6 passed (includes the
  three reproduction tests that were red pre-fix).
- `cargo test -p hares-equipment --lib pv::lut::tests::harvest_kv` → 2 passed
  (the new regression test green after the fix; red before, observed).
- `cargo fmt --check --all` → exit 0.
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0.
- `cargo nextest run --release --no-fail-fast --profile ci -p hares-types -p hares-equipment` → 2862 passed, 0 skipped.
- `cargo nextest run --release -F check_invariants --no-fail-fast --profile ci -p hares-core` → 1078 passed, 21 skipped.
- `.venv/bin/python -m pytest tests/python/test_ev_tariff_integration.py -q` → 21 passed.
- `git diff HEAD -- crates/ | grep -E 'parse::<f64>|\.parse\(\)'` → exactly one
  added line (the `harvest_kv_f64` body), grounding the class-closed claim.
