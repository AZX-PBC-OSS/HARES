# Verification L5-c

Scope: `ledger/04-fleet.md`, `ledger/06-runtime-memory.md` (RT-3 and RT-3a
first), `ledger/10-slop-cleanup.md` entries not committed (SL-1 through
SL-6, SL-8 through SL-11; SL-7 is already committed and independently
audited elsewhere), `ledger/11-other-defects.md` (all entries, OD-22 to
OD-27 read in full), and `plan.md` §2.4, §2.9, §2.10 as they bear on the
above. Checked against HEAD `7b7a6a7d` in the clean worktree
`.workstrees/audit-7b7a6a7d`, with historical citations (lines "at
<commit>") checked against that commit's tree with `git show`. RT-3's
in-progress uncommitted edits were read from the main tree
(`git -C /home/rich/src/HARES diff`) and from running
`cargo test -p hares-envelope` there.

Counts: 2 findings total. 0 blocker. 1 major. 1 minor.

## Findings

### Major: RT-3's stop-and-add record cites the wrong line numbers for its three fixed allocation sites

**Location:** `ledger/06-runtime-memory.md`, RT-3, "Stop-and-add record
(2026-10-02, at `7b7a6a7d`)".

**Evidence:** the record says the three extra allocation sites
`build_input_vector` found are "all in `build_input_vector`
(`thermal_solver/mod.rs:1408-1426`, `:1494-1497`)". At `7b7a6a7d`,
`thermal_solver/mod.rs:1408-1426` is the `driving_ground_temp_c` Kusuda
diagnostic block (unrelated to allocation), and `:1494-1497` is the
`latent_pairs_buf`/`custom_payload_buf` reuse block (also unrelated). The
three sites the record actually describes:
`int_surface_diag: self.int_surface_diag_buf.clone()`,
`infiltration_by_zone: Vec::new()` and the `infiltration_by_zone_buf` /
`lwr_by_zone_buf` capacity-0 `reserve` dance, sit at lines 1390, 1450 and
1470-1473 respectively (confirmed with `grep -n` against the committed
HEAD). The citation is off by 15-80 lines and does not point at the code
it describes.

**Why it matters:** this is a newly added record (same day as the audit),
and a reviewer or future reader following the citation lands on unrelated
code. The description of the defect and its fix is otherwise accurate; I
verified in the uncommitted working-tree diff that `int_surface_diag`,
`infiltration_by_zone` and `interior_lwr_by_zone` are now round-tripped
through the struct replacement (`std::mem::take` / move) instead of
`.clone()` and a capacity-0 `Vec::new()` + `reserve`, matching the
record's "Change" description, and the allocation test
(`thermal_solver_step_allocation_free_after_first_step`) passes with
`cargo test -p hares-envelope`.

**Correction:** fix the two line citations to `1388-1391` (or wherever
`int_surface_diag_buf.clone()`/`infiltration_by_zone: Vec::new()` sit in
the pre-fix tree) and `1463-1473` (the swap-and-reserve block), re-checked
against the commit the record is filed under.

### Minor: RT-3's stability-check comment rewrite and the zero-alloc test's header/assertion rewrite are not yet done in the in-progress work

**Location:** `ledger/06-runtime-memory.md`, RT-3 "Change" (the
`StateSpaceModel` constructor comment) and "Acceptance test" (the
`radiant_gain_weights_zero_alloc.rs` header/assertion rewrite).

**Evidence:** at `7b7a6a7d`, `crates/hares-envelope/src/state_space.rs`
still carries the comment "ZOH discretization produces pole at |λ|=1, but
CN implicit path remains well-behaved" with an em dash a few lines below
it (lines ~266-275); the uncommitted diff (`git -C /home/rich/src/HARES
diff -- crates/hares-envelope/src/state_space.rs`) does not touch this
comment at all. Likewise `radiant_gain_weights_zero_alloc.rs` still opens
with "Regression test for ticket 040" and its assertion message still
reads "(ticket 040)" and "Pre-fix count was 1300 (13/step)", text the
ledger requires the row to replace, and which `10-slop-cleanup.md` SL-2
separately censuses as a planning reference that must go.

**Why it matters:** these are explicit, named deliverables of RT-3's
Change and Acceptance-test sections, not optional cleanup; as currently
staged the row is incomplete against its own text. This is not a defect
in the ledger's writing, only a gap in the in-progress implementation
(RT-3 is correctly marked "in progress" in `00-schedule.md`), so I rate it
minor and flag it as outstanding work rather than a wrong entry.

**Correction:** none needed to the ledger text; the implementer should
finish the comment rewrite and the test file's header/assertion rewrite
before committing RT-3, per the entry's own Change and Acceptance-test
text.

## Verified correct (no finding)

- `00-schedule.md`: row order, state column and the contingency rules are
  internally consistent with the entries they reference; RT-3 (row 21) and
  RT-3a (row 21a, added 2026-10-02) are correctly placed as "in progress"
  / not yet run, consistent with the stop-rule record in
  `06-runtime-memory.md`.
- `04-fleet.md`: every citation in the "Facts" header I spot-checked
  against `ebdb1a17` matches exactly: `fleet.rs:313-324`
  (`Fleet::simulate`'s local-vs-global rayon pool), `fleet.rs:33-40`
  (`SimStatus`), `aggregation.rs:43-50`/`:63-72` (the time-stage unit
  table and its `Mean` fallback). FL-1 through FL-5's problem statements
  are consistent with the facts header and with each other (e.g. FL-5's
  unit census lines up with FL-1's "any other unit" list).
- `11-other-defects.md` OD-22 through OD-27 (all read in full, all
  citations checked against the committed tree): every file:line and
  symbol exists and matches the described behavior.
  - OD-22: `dwelling/mod.rs:2554-2575` at `afa9a071` matches exactly
    (`is_critical` list, "init failed, skipping" message).
  - OD-23: `defaults.rs:500`/`:764`/`:2103-2104` at `7b7a6a7d` match; the
    dead CSV (`defaults/battery/default_parameters.csv`) exists on disk
    and is read by nothing.
  - OD-24: `defaults.rs:1653-1781` (`load_vehicle_mapping_csv`) matches
    the described silent-skip behavior for a missing file, an open error,
    and malformed/out-of-range rows.
  - OD-25: `schedule_resolve.rs:1019-1032` matches the three
    `unwrap_or(0.0)` fallbacks and the "THIS MAY BE INCORRECT" warning
    text verbatim.
  - OD-26: `py_dwelling.rs:1322` (`fuel_economy =
    spec.capacity_kwh / spec.range_miles`) and the literal `0.0` for
    `away_charge_power_kw` against `actor_registry.rs:425`'s `6.6` default
    both confirmed; the duplicated `spec.max_l2_power_kw.min(1.8)` L1 cap
    exists verbatim in both `py_dwelling.rs` and `py_equipment.rs`.
  - OD-27: `py_dwelling.rs:add_actor_by_name` has no
    `#[pyo3(signature = ...)]` attribute while the generated stub
    declares `params: dict[str, Any] | None = ...`, confirmed.
- `10-slop-cleanup.md` SL-1: `mod.rs:6431-6433` at `ebdb1a17` and
  `telemetry.rs`'s `#[derive(Clone, ...)]` over a `HashMap`-backed struct,
  and `environment.rs:510-512`'s `upsert_domain_ref` "reusing the existing
  slot's allocations" doc over a `.clone()`/`.push(update.clone())` body,
  all confirmed false-comment claims as described.
- `plan.md` §2.4 (Fleet) and §2.9 (Engine performance): consistent with
  `04-fleet.md` and `06-runtime-memory.md`'s RT entries; §2.9's claim that
  "the coupled factorization is ... removed as dead code (SL-7)" matches
  SL-7's committed state. §3's thermal-solver scope list explicitly
  authorizes RT-3 and RT-3a to edit `hares-envelope`.
- The WSL2 `perf` unavailability and the profiling-feature fallback the
  task description asked me to check against RT-3/RT-3a's measurement
  method is independently and correctly documented in
  `02-measurement.md` (lines ~652-653, ~772, ~891-896: "`perf` is
  unavailable on this host (WSL2 exposes no PMU)" / "perf not found for
  kernel 6.18.33.2-microsoft"), consistent with RT-3's and RT-3a's
  Measurement sections, which use the profiled `step_total`/
  `ideal_capacity` phase shares rather than `perf`.
- RT-3's core allocation-site citations (`stepping.rs:672-674, 691/1208,
  744`, `state_space.rs:780-853, 855-900`, all "at `afa9a071`") all match
  exactly at that commit. The current HEAD code (`7b7a6a7d`, after SL-7
  removed the coupled-LU/identity-solve path) correctly no longer calls
  `m_lu.solve()` in `solve_for_scalar_input`, which resolves what would
  otherwise be a gap in RT-3's Change section (it describes a pure
  mul_to/elementwise fix with no `solve_mut`, which is only correct
  because SL-7 already eliminated the LU solve from that path).
- RT-3's uncommitted implementation in the main tree
  (`crates/hares-envelope/src/state_space.rs`,
  `thermal_solver/mod.rs`, `thermal_solver/stepping.rs`) matches the
  ledger's Change section: `SolveScratch` type with `n_x`, `b_u`, `rhs`,
  `gain`, `d_agg`; `output_into` using two `mul_to` calls and an
  elementwise add; the persistent `ext_temps_save_buf`; the surface-id
  sequence comparison in `refresh_solar_slot_map` (rebuild only on
  change, no hashing). `cargo test -p hares-envelope` passes in full,
  including `thermal_solver_step_allocation_free_after_first_step` and
  `solar_slot_map_rebuilt_only_on_surface_change`.
- No em dashes, LLM tics, agent/session references or archaeology found
  in the ledger prose of the sections read (04-fleet.md, 06-runtime-
  memory.md, 11-other-defects.md OD-22 to OD-27, relevant 10-slop-
  cleanup.md entries, plan.md §2.4/§2.9/§2.10).

## Verification gaps (not run)

- I did not execute `cargo nextest run -p hares-fleet`, `frame_golden
  compare-all`, or the full workspace test suite; I relied on the
  committed facts/evidence citations and a direct `cargo test
  -p hares-envelope` run for RT-3's in-progress code only, since that is
  the entry actively being implemented. FL-1 through FL-5, RT-1, RT-2,
  RT-4 through RT-8, and the OD/SL entries other than those code-checked
  above are not yet implemented (no commits), so their acceptance tests
  cannot be run; I verified their Problem citations against code instead.
- I did not independently re-derive the numeric deltas RT-3/RT-3a's
  Measurement sections report (6% of `step_total` for `ideal_capacity`);
  I confirmed only that the measurement method they use (profiled phase
  shares, not `perf`) is consistent with what this host can actually run,
  per `02-measurement.md`'s own recorded findings.
- Given the scope size (11-other-defects.md is 1829 lines; plan.md 2274
  lines), I read 11-other-defects.md OD-1 through OD-8 in less depth than
  OD-22 to OD-27 (the newest entries) and did not re-verify every citation
  in OD-9 through OD-21 line-by-line; the ones I spot-checked (OD-2, OD-3,
  OD-6, OD-8, OD-20, OD-21) were internally consistent with their own
  evidence sections and with 00-schedule.md's placement of their commits.
