# Engine runtime and memory

Implements `plan.md` §2.9 and the inputs half of §2.7. Every entry here is
bitwise frame-equal before and after on M-2's golden fixtures, asserts
allocation and factorization counts as tests, and records performance
shares as measurements in `02-measurement.md`. RT-3 and SL-7 edit
`hares-envelope`'s thermal solver, which `dl-i08-process.scope` puts in
scope.

## RT-1: per-step equipment and domain snapshots without reallocation or search

**Kind:** initiative work (`context.md` item 1; no per-step allocation,
search or sort in the step path: `dl-i08-criteria`,
`dl-i08-architecture.engine_performance`). Fixes V27.

**Problem** (lines at `7b7a6a7d` unless another commit is named): the
end-of-step snapshot rebuilds a `HashSet` of every
equipment id (`crates/hares-core/src/dwelling/mod.rs:6919-6929`) only to
`retain` `equipment_core` against it (`:6930-6932`), and copies each
equipment's telemetry with `existing.clone_from(telemetry)` (`:6961-6966`),
under a comment claiming the copy reuses the map's slots (`:6961-6963`).
`Telemetry` is `#[derive(Clone)]` over the map and the non-finite latch
`03-output-reporting.md` OR-5 added and the unknown-key latch OR-11
added, `Telemetry(pub HashMap<String, f64>, Option<(String, f64)>,
#[serde(skip)] Option<String>)` (`crates/hares-types/src/telemetry.rs:26-30`
at `7b7a6a7d`; the dwelling enforces the unknown-key latch at
`crates/hares-core/src/dwelling/mod.rs:6479`); a derived
`Clone` does not override `clone_from`, so it runs
`*self = source.clone()`: a new table and every `String` key reallocated,
every step, for every equipment. The restore
path `snapshot_equipment_state` (`mod.rs:5246-5285`, run after
`load_checkpoint`) does the same at `:5273-5278`.

The domain updates have the same defect and a second one. `DomainUpdate` is
`#[derive(Clone)]` (`crates/hares-types/src/domain_solver.rs:14-19`), so
`EnvironmentState::upsert_domain_ref`
(`crates/hares-types/src/environment.rs:513-523`), whose doc claims it
reuses the slot's allocations, reallocates both vectors.
`EnvironmentManager::update_in_place` drains every domain entry each step
(`crates/hares-core/src/environment.rs:678`) and pushes the schedule and
mains entries back (`:701, 706`), so the thermal, humidity, electrical,
fluid and custom upserts (`mod.rs:6569, 6679-6682, 6697`) push a fresh clone
every step, and the custom update buffers grow on the step path
(`mod.rs:6685-6686`). Every write and every read is a linear search of
`EnvironmentState.custom_domains: Vec<DomainUpdate>` by `DomainId`: the two
upserts (`environment.rs:498-523`) and the readers
`crates/hares-envelope/src/humidity_solver.rs:151`,
`crates/hares-equipment/src/hvac/boiler.rs:1009-1014`,
`crates/hares-types/src/schedule.rs:631` and `mod.rs:4556, 5313, 7784,
7808`. The built-in domains are fixed (`THERMAL`, `ELECTRICAL`, `HUMIDITY`,
`FLUID`, `domain_solver.rs:9-12`; `SCHEDULE_DOMAIN_ID`, `schedule.rs:159`;
`MAINS_WATER_DOMAIN_ID`, `crates/hares-core/src/environment.rs:34`). No
production code installs a custom solver: `Dwelling.custom_domain_solvers`
is private, starts empty at construction (`mod.rs:2883`), and
only two unit tests push to it directly
(`custom_solvers_captured_in_observer_snapshot`, `:15816`, push at
`:15903`; `builtin_and_custom_solver_captures_coexist`, `:15929`), each
also pushing the matching update buffer by hand. The `DomainSolver` trait
(`crates/hares-types/src/domain_solver.rs:40`) is public, so the seam
exists without a way to use it.

`Telemetry::set` requires every key to be registered at init
(`telemetry.rs:62` at `afa9a071`), including the per-stage air-conditioner keys
registered under `observe` (`crates/hares-equipment/src/hvac/air_conditioner.rs:883-905`),
so steady-state key sets are fixed.

**Change:**
1. A hand-written `Clone for Telemetry` over all three fields: `clone`
   clones the map and both latches; `clone_from` overwrites values in place
   when both maps have the same length and every source key is present in
   `self`, and otherwise falls back to `self.0.clone_from(&source.0)`; in
   both cases it then copies the latches with `self.1.clone_from(&source.1)`
   and `self.2.clone_from(&source.2)`, which allocate nothing when both are
   `None` (the steady state), so a latched source always yields a latched
   copy and the dwelling's non-finite and unknown-key checks see it. A
   field-count assertion in the test module (destructuring
   `let Telemetry(_, _, _) = ..`) fails to compile when a field is added,
   so the hand-written impl cannot silently miss one. No API change;
   values are copied as `f64`, so the result is bitwise identical.
2. `Dwelling` keeps an `active_equipment_ids: HashSet<EquipmentId>` field,
   computed by the roster plan and set by its install
   (`03-output-reporting.md` OR-19, row 21c), which run on every equipment
   and actor change. The per-step `retain` reads it.
3. Domain updates live in fixed slots (`hares-types`):

   ```rust
   pub struct DomainSlots {                   // replaces EnvironmentState::custom_domains
       pub thermal: DomainSlot,
       pub humidity: DomainSlot,
       pub electrical: DomainSlot,
       pub fluid: DomainSlot,
       pub schedule: DomainSlot,
       pub mains_water: DomainSlot,
       custom: Vec<DomainSlot>,               // one per installed custom solver, in installation order
   }
   pub struct DomainSlot { update: DomainUpdate, present: bool }
   impl DomainSlot {
       pub fn get(&self) -> Option<&DomainUpdate>;      // None when not written this step
       pub fn set_from(&mut self, source: &DomainUpdate);
       pub fn payload_mut(&mut self) -> &mut Vec<f64>;  // schedule and mains: written in place
   }
   impl DomainSlots {
       pub fn install_custom(&mut self, id: DomainId) -> Result<CustomDomainSlot, HaresError>;
       pub fn custom(&self, slot: CustomDomainSlot) -> Option<&DomainUpdate>;
   }
   ```

   `EnvironmentState.domains: DomainSlots` replaces `custom_domains`.
   `present` keeps what every reader sees during a step: `update_in_place`
   clears it for the solver domains where it drains today, and writes the
   schedule and mains payloads into their slots' retained vectors (the swap
   buffers go); each solver's upsert becomes `set_from`, which copies through
   a hand-written `DomainUpdate::clone_from` that reuses both vectors. A
   reader of a built-in domain reads its field
   (`env.domains.thermal.get()`), with no search. Custom solvers are
   installed through one method,
   `Dwelling::install_domain_solver(&mut self, solver: Box<dyn DomainSolver>)
   -> Result<CustomDomainSlot, HaresError>`, which calls
   `DomainSlots::install_custom` with the solver's `DomainId`, allocates its
   update buffer and appends the solver, so the slot is resolved once, at
   installation. Installing a built-in `DomainId` or one already installed,
   or installing after the dwelling's first step, is an error naming the id
   (the step path's slot set is fixed once stepping starts). The private
   `custom_domain_solvers` and `custom_update_bufs` fields are written only
   by this method. `upsert_domain`,
   `upsert_domain_ref` and `custom_domains` are deleted. `DomainSlots`
   derives `Default`, `Clone`, `PartialEq`, `Serialize` and `Deserialize`,
   so the struct literals of `EnvironmentState` (174 `custom_domains` sites
   across `crates`, `tests` and `benches` at `ebdb1a17`) write
   `domains: DomainSlots::default()`, and tests that pushed an entry write
   its slot.
4. The per-equipment snapshot body shared by `step` and
   `snapshot_equipment_state` becomes one function; the false comments are
   replaced by ones that state the copy is in place (SL-1).

Telemetry storage stays a `HashMap`, so the row plan's producer slots
(`03-output-reporting.md` OR-2) are unaffected and are not re-resolved.

**Acceptance test:** each allocation test installs
`hares_types::alloc_count::CountingAllocator` (`07-profiler.md` PR-3) as
its binary's `#[global_allocator]` and asserts
`thread_allocations()` deltas of `Some(0)`.
- `telemetry_clone_from_same_keys_allocates_nothing` (`crates/hares-types/tests/`,
  its own binary): `clone_from` between two maps with the same keys and no
  latch performs zero allocations; with different key sets the result
  equals the source; a source with a latched non-finite write yields a copy
  whose `non_finite_latch()` equals the source's, and a source with a
  latched unknown key yields a copy whose `unknown_key_latch()` equals the
  source's, each on both the in-place and the fallback path; a clean source
  clears both latches of a latched destination.
- `domain_update_clone_from_reuses_allocations`: same harness, zero
  allocations when capacities suffice.
- `dwelling_snapshot_allocation_free_after_first_step`: a `crates/hares-core`
  unit test, run by the block's `[test-observe-profiling]` line; the crate's
  test binary installs the allocator under `#[cfg(test)]`. On
  `tests/fixtures/hpxml/ochre_samples/base.xml` (central air conditioner with
  its per-stage keys, storage water heater), the test steps the dwelling
  normally and reads, after each step, a `#[cfg(test)]` per-site
  allocation accumulator on the dwelling: the step path itself takes a
  `thread_allocations()` reading before and after the extracted snapshot
  function and each domain-slot write (`update_in_place`'s slot writes and
  every `set_from`), adds each difference to that site's accumulator, and
  compiles none of this outside the crate's own test build. Every site's
  accumulator grows by zero on every step after the first. Allocations
  elsewhere in the step (the recorder, until `03-output-reporting.md` OR-2
  and OR-3) are outside the brackets.
- `custom_domain_slot_resolved_at_installation`: a stub custom solver
  installed with `install_domain_solver` is readable through the returned
  `CustomDomainSlot` after a step; installing a second solver with the same
  `DomainId`, one with `THERMAL`, or any solver after the first step is an
  error naming the id. `custom_solvers_captured_in_observer_snapshot` and
  `builtin_and_custom_solver_captures_coexist` install their stub solvers
  through `install_domain_solver` in place of pushing to the private
  fields.
- `git grep -nE 'custom_domains|upsert_domain' -- crates tests benches`
  prints nothing.
- M-2's golden fixtures compare bitwise equal.

**Verification:** `cargo nextest run -p hares-types`; `cargo nextest run
--workspace`; `cargo nextest run -p hares-core -F observe,profiling
dwelling::`; `frame_golden compare-all`.

**Commit:** `perf: copy equipment and domain snapshots in place instead of reallocating them every step`

**Dependencies:** M-1, M-2, PR-3, OR-19 (`03-output-reporting.md`, the
roster plan that computes `active_equipment_ids`).

**Measurement:** share of `simulate()` spent in the snapshot and in the
domain writes and reads, before and after.

## RT-2: exogenous forcing computed once per weather and time grid

**Kind:** initiative work (`context.md` items 2 and 4).

**Problem:** `update_in_place` (`environment.rs:510-755`) recomputes, every
step and for every dwelling, quantities that depend only on the weather and
the time grid: the humidity ratio, wet bulb and enthalpy (`:534-537`), the
civil day of year (`:543-554`), the solar position (`:556-561`) and the mains
temperature (`:581-585`). They depend on:
- the resampled weather rows, i.e. the weather content, the resampling
  methods and `time_res` (`environment.rs:195-203`);
- the start, through `weather_start_offset` (`:240`), which reads the
  start's local wall clock (`compute_annual_offset`, `:796-808`: its
  `ordinal0()`, `hour()` and `minute()` in the start's own offset), and the
  local day of year (`now.ordinal()`, `:554`), while the solar position
  converts to UTC (`crates/hares-physics/src/solar.rs:97`): both the local
  wall clock and the instant matter, so the UTC offset does;
- the horizon, which fixes the step range. Warm-up replays the first day:
  each warm-up day resets `clock.current_step` to 0 and runs
  `steps_per_day` steps (`run_warmup_converged`, `mod.rs:5396-5398`), so it
  reads rows `0..steps_per_day` and adds no row past
  them. The horizon is `max(duration steps, steps_per_day)` when warm-up is
  on and the duration steps otherwise; adding the warm-up days to the
  duration would size up to `max_days` (`09-numeric-fixes.md` B-2) unused
  days of rows and give runs that differ only in warm-up different keys;
- the resolved latitude, longitude and elevation, which
  `SimulationConfig.site_location` can override (`blueprint.rs:74-86`), and
  the mains inputs derived from the weather (`environment.rs:242-243`);
- the civil timezone under `dst` (`environment.rs:568-574`).
Per-surface Perez irradiance and angle of incidence (`environment.rs:599-637`) also depend
on each surface's tilt, azimuth and omnidirectional flag, and PV roof
coverage is applied after them (`:640-649`).

**Change:**
- A `ForcingTable` holds, per step index, the surface-independent values
  above (the values `update_in_place` computes before its Perez loop,
  including the solar zenith and the `observe` day-of-year diagnostics). It is
  built eagerly in one pass, with the same functions and inputs the step
  uses, and is immutable afterwards.
- `ForcingKey` and `ForcingTable` are `plan.md` §2.7's (`hares-io`). The
  key holds plain values, never a `DateTime<FixedOffset>` (whose `Eq` and
  `Hash` compare the UTC instant alone): `time_res` in seconds, the naive
  local start, the resolved site UTC offset in seconds, the horizon in
  steps (the rule above), the resolved latitude, longitude and elevation
  (as bits), and the
  civil timezone. Two blueprints over one `ParsedWeather` with the same
  start instant but different offsets therefore get different tables. The
  resample policy is not in the key: after `12-inputs.md` IN-6 the engine
  has one method per weather field. `09-numeric-fixes.md` B-1a and B-1c add
  `FirstHour` and `WeatherYear` to the key in their own commits, since each
  changes which weather rows a step reads. The weather content (including
  its record labelling) is not in the key because the tables are stored on
  the `ParsedWeather` they came from (RT-4), in a map from key to
  `Arc<ForcingTable>` behind a mutex that is locked once per blueprint
  construction and never on the step path. A blueprint holds its
  `Arc<ForcingTable>` and hands it to every dwelling it builds.
- `update_in_place` reads the table row for the step, then computes the
  per-surface irradiance from it exactly as today.
- The surface-independent table itself is kept or dropped by the same
  measurement rule. The entry records the share of `simulate()` spent
  computing the surface-independent values on `consumer_shape_900s` and on
  M-3 item 5's stepped-episode loop at the parent, and the wall time of
  that loop at the parent and with the table. The table is kept only when
  the loop's wall time
  with the table is lower than without it by more than M-3's 5% noise band
  (a single run computes each row once either way, so the saving is the
  repeated builds'); otherwise it is dropped, `update_in_place` stays as
  it is, and the shares and wall times are recorded as the measurement for
  which it is dropped (`dl-i08-architecture.engine_performance`), with the
  per-surface table below dropped with it.
- Per-surface reuse is decided by the profile. Once the surface-independent
  table is in place, the entry records the share of `simulate()` spent in
  the per-surface Perez and incidence loop on `consumer_shape_900s` and on
  `02-measurement.md` M-3 item 5's stepped-episode loop (one blueprint built
  and run repeatedly, the shape of RL resets, repeated evaluations and fleet
  homes built from one blueprint). If either share is at or above 5% of
  `simulate()` (M-3's noise band, below which a removal cannot be measured),
  this commit adds a per-blueprint per-surface table: built on the
  blueprint's first build from its surfaces and its `ForcingTable`, held as
  an `Arc` on the blueprint and shared by every dwelling it builds (about 40
  bytes per surface and step, about 21 MB for a year at 900 s), with the
  test `per_surface_table_shared_across_builds_of_one_blueprint`. If both
  shares are below it, no per-surface value is cached and both shares are
  recorded as the measurement for which it is dropped
  (`dl-i08-architecture.engine_performance`). Geometry is never part of a
  shared key: blueprints with different geometry never share per-surface
  values.
- A dwelling with `solar_override` set (`:594`) uses the table for the
  surface-independent values; the override already replaces the per-surface
  values.

**Acceptance test:**
- `forcing_table_run_equals_uncached_run`: M-2's fixtures (including
  `full_year_900s_dst`, `leap_year_february_900s` and `weather_wrap_900s`)
  and a two-day warm-up variant of the cz4a inputs built in the test, run
  with the table and with the per-step computation, give bitwise-equal
  frames (`-F dst` for the civil timezone fixture).
- `forcing_key_differs_by_start_date`: two configurations that differ only in
  start date produce different keys and different tables.
- `forcing_key_differs_by_utc_offset_at_equal_instant`: two blueprints over
  one `ParsedWeather` of `tests/fixtures/parity/cz4a_ashp_hpwh/weather.epw`
  with `cz4a_ashp_hpwh/building.xml`, one with `site_location` at offset
  -07:00 and `start_time = 2023-01-01T00:00:00-07:00`, the other at -05:00
  and `start_time = 2023-01-01T02:00:00-05:00` (the same instant), 2 d at
  900 s, get different keys and different tables, and each frame equals its
  uncached run bitwise. A second pair isolates the offset: the same naive
  local start `2023-01-01T00:00:00` with `site_location` offsets -07:00
  and -05:00 (two instants two hours apart); their keys differ only in the
  offset field, and their tables differ in the solar position rows.
- `blueprints_from_different_hpxml_share_forcing`: RT-4's
  `blueprints_from_different_hpxml_share_weather` pair (`base.xml` and
  `base-pv.xml` over one `ParsedWeather`) hold the same `ForcingTable`
  (`Arc::ptr_eq`).
- `azimuth_change_shares_the_table`: two HPXMLs that differ only in a wall's
  `<Azimuth>`, built over one `ParsedWeather` (RT-4), share one table (same
  `Arc`) and produce different frames, each bitwise equal to its uncached
  run.
- If the per-surface table is added:
  `per_surface_table_shared_across_builds_of_one_blueprint` (two dwellings
  from one blueprint hold the same per-surface `Arc`, and their frames equal
  the uncached run bitwise).

**Verification:** `cargo nextest run -p hares-core environment::`; `cargo
nextest run -p hares-core -F dst environment::`; `frame_golden compare-all`.

**Commit:** `perf: compute weather-derived forcing once per weather and time grid`

**Dependencies:** RT-4 (`ParsedWeather`, blueprints from parsed handles),
M-2, M-3; `12-inputs.md` IN-6 (one method per field, so no policy in the
key).

**Measurement:** `update_in_place` share of `simulate()` before and after;
the surface-independent shares and the stepped-episode wall times above
and the keep or drop decision they produced; the table's size in bytes at
900 s and at 60 s for one year; the two per-surface shares above and the
decision they produced. If the table is dropped, the row commits no engine
change: the numbers go into `02-measurement.md`'s log and the row's state
is recorded as measured and dropped.

**State:** committed `3fa77b58` (2026-10-12, branch `row/r62-rt2`, based
on `9cc6d578`), rebased onto RT-4's fold (`row/r61-rt4`: `2434deb8` +
`c7098a38`) as `470b0473` + `902d8d60`, and the review fix `81a8969d`
below on top.
The table is kept: the stepped-episode loop's wall, the rule's arbiter,
read 5.8% and 7.8% lower with it in two paired sets (the exact
accumulator under it: the loop's 9600 per-step computations, about 9 ms
of the loop's work, removed; the full-year forcing 2.9% to 0.3% of
`simulate()`); the numbers, the noise verdicts and the fleet step-loop
figure at `02-measurement.md`'s log entry, raw record
`.shipwright/runs/r62-rt2-measurement.md`.

The row first ran ahead of its dependency (the entry's dependencies name
RT-4 first, which had not landed): its share mechanism was a
process-global map from a (content digest, grid key) pair to
`Arc<ForcingTable>`, the interim the paragraph below recorded. RT-4
landed the parsed-weather handles and the row folded onto them, and the
independent review found the interim map an unbounded cache the ledger
disclosed nowhere: N distinct weather contents leave N tables resident
forever (the reviewer's probe, 300 distinct contents, reaches 300
resident tables and grows without bound), and a changed-on-disk file's
key stops hitting but its stale table still stays resident while any
construction over the old content's digest remains a live entry.

**The review fix: the map moved onto the handles, whole.** The map from
grid key to `Arc<ForcingTable>` is now a field of the parsed-weather
handle (`ParsedWeather` in `hares-io/src/inputs.rs`, beside its resample
cache), resolved by `ParsedWeather::forcing_table` with the same
lock-once-per-construction pattern. The handle is the weather content's
identity, so the key's content-digest half is deleted and the key is the
grid key alone ([`ForcingKey`]; `ForcingCacheKey` is deleted): a table
belongs to the handle it was built from, every dwelling built from one
handle shares its table (`Arc`), a dropped handle drops its map and
every table in it, and a re-parse of a changed file is a new handle with
its own table, so no stale table survives its content. The resident
tables are bounded by the handles' lifetimes. The step path is
unchanged: the manager holds the `Arc` and reads it lock-free.

**The leak-death proof:** the counter `FORCING_TABLES_LIVE` (in
`forcing.rs`: one increment per table built, one decrement when a
table's last owner drops it) turns the reviewer's probe into the test
`dropped_dwellings_drop_their_forcing_tables`
(`crates/hares-core/tests/forcing_table.rs`): 40 distinct weather
contents, each a dwelling constructed from its own file and dropped
(the reviewer ran 300; the growth is linear, the bound the same). RED
at the pre-fix tip, the process-global map in place: 40 tables stayed
resident after every dwelling was dropped (the count read 40 and never
fell). GREEN on the fix: the count sits at exactly one table per live
handle while the dwellings are held and returns to the baseline (zero
resident) when they drop, with one computation per distinct content
(the duplicate-computation probe still reads exactly one build per
content). A changed-on-disk file is pinned by the same mechanics: a new
parse is a new handle, its tables its own; the sharing tests build from
one handle and hold the same table (`Arc::ptr_eq`), the key tests
resolve two keys on one handle and never one table.

**The fold's found defect, fixed here:** the rebase's auto-merge left
the table build reading the shared series' raw header while the step
path reads the manager's resolved metadata copy (the blueprint's
resolution of the HPXML site, the weather header and the config's
override). The consumer-shape differential caught it (its ResStock CSV
weather parses with a placeholder header the site resolution replaces:
the two paths' solar positions diverged and the bitwise frames
differed); `build_forcing_table` now takes the resolved metadata and
the differentials are bitwise green again.

The share mechanism, landed: `ForcingKey` and `ForcingTable` are
`hares-io`'s (`forcing.rs`), the key exactly the entry's plain values
(the resolution in seconds, the naive local start, the resolved site
UTC offset in seconds, the horizon in steps, the resolved latitude,
longitude and elevation as bit patterns, the civil timezone's IANA
name). The horizon rule is the entry's: `max(duration steps, steps per
day)` with warm-up on, the duration's steps without, sized once and
never read past (a step past the horizon, like a stepping clock whose
time base is not the construction's, reads the per-step computation,
whose semantics are the parent's own; two existing tests that step such
clocks caught the first cut indexing blindly and the check is the fix).

The thread safety: `ForcingTable` is plain data (64-byte `Copy` rows),
shared as an `Arc`, read-only during stepping; each handle's map mutex
is touched only where a manager is constructed. The fleet's parallel
steps (rayon, one dwelling per thread) read the shared table through
their own `Arc`s with no lock on the step path, which is the entry's
`Send + Sync` requirement verbatim.

RED first (the row's own): the duplicate-computation proof is the
`FORCING_COMPUTATIONS` counter (kept, documented, one increment per
computation): at the parent, three dwellings built over one weather
file and stepped ten steps each moved it by 33 (one per step, plus each
construction's initial state); with the table, by exactly 1. The
acceptance tests failed to compile at the parent (no `ForcingTable`
surface) and the differential failed behaviourally once the switch
existed; all eight tests in `crates/hares-core/tests/forcing_table.rs`
are green on the commit, bitwise frames against the per-step computation
selected by the kept `FORCE_PER_STEP_FORCING` switch (the per-step path
is the parent's own block, so one binary runs both sides of the
comparison and the pin cannot rot).

The per-surface branch: its condition fires (the measured shares, 7.3%
to 7.5% of `simulate()` on the full year and 5.7% to 5.8% of the
stepped-episode loop, at or above the 5% band; a temporary accumulator,
removed) and its mechanism landed with RT-4's blueprint that builds
many dwellings: a per-blueprint per-surface table shared by every
dwelling the blueprint builds is the row's follow-up (the shares stand
in the measurement log as the measurement that fired it).

Gates (at the review fix, canonical env `uv sync --locked --all-extras
--all-groups` + `uv run maturin develop`): fmt clean; clippy
--workspace --all-targets --all-features -D warnings clean; nextest
--workspace --all-features -j 4 8012 passed, 3 skipped (the ignored
kind-pin generator and the two hand-run measurement tests); `-F dst
environment::` 62 passed; pytest -m '' 1724 passed, 6 xfailed, 0
failed, 0 errors; pyright 0; the goldens bitwise (compare-all 8,
`full_year_900s_dst` 1, compare-all under `observe` 8).

## RT-3: thermal solver steps without heap allocation

**Kind:** initiative work (`context.md` items 4 and 5).

**Problem** (lines at `afa9a071`): the thermal solver allocates on every
step, in its step and in the ideal-capacity solve the dwelling calls
between its two phases. The envelope's own counting-allocator test allows
11 allocations per step
(`crates/hares-envelope/tests/radiant_gain_weights_zero_alloc.rs:241-282`)
and never calls the ideal-capacity solve. The allocations, after SL-7
removes the coupled LU path:
- `prepare_inputs_inner` clones `exterior_surface_temps` to restore it
  afterwards (`thermal_solver/stepping.rs:672-674`).
- `prepare_inputs_inner` drops the latent map it gets back from
  `build_input_vector` (`stepping.rs:673`, `_latent`). `build_input_vector`
  had taken it out of `self.latent_buf` (`thermal_solver/mod.rs:1271`), so
  `integrate_inner` receives an empty map and allocates a new table every
  step; only `format_domain_update` puts it back (`mod.rs:1512`).
- `self.last_u.clone_from(&u)` (`stepping.rs:691, 1208`): nalgebra 0.35.0
  (`Cargo.lock` at `21d90588`) `Matrix` is `#[derive(Clone, Copy)]`
  (`src/base/matrix.rs:163`), so `clone_from` is the derived one and
  allocates.
- `self.model.output(&self.x, &u)` (`stepping.rs:744`) builds
  `&self.c * x + &self.d * u` (`state_space.rs:471-473`): two vectors, one
  per product; the owned-plus-owned sum reuses the left operand's buffer.
- `solve_ideal_capacity_for_target` (`stepping.rs:366`), which
  `SolverFeedbackActor` calls every step for every conditioned zone
  (`crates/hares-core/src/actors/solver_feedback.rs:61`), reaches one of
  two solves after SL-7, both allocating on every call.
  `solve_for_scalar_input_identity_coupled` (`state_space.rs:780-853`,
  couplings active) builds `&self.n_mat * x + &self.b_eff * u -
  self.b_eff.column(i) * u_i` (one vector per product and one for the
  scaled column), `vec![0.0f64; n]` for `d_agg` and
  `self.b_eff.column(i).into_owned()` for the gain.
  `solve_for_scalar_input` (`state_space.rs:855-900`, behind
  `solve_for_output_input`, no couplings) builds the same three
  temporaries and the gain copy. The `c_row * v` and `d_row * u` products
  of both have a statically sized 1×1 result and do not reach the heap;
  the test below confirms it. The doc comment of
  `solve_ideal_capacity_for_target` (`stepping.rs:350-364`) says the solve
  is "Zero allocation", which is false.

The test's own note also names a `DVector::zeros(0)` in the `u_buf` swap,
which does not allocate (a zero-length vector), and `build_coupled_lu`,
which SL-7 deletes.

Separately, `refresh_solar_slot_map` (`mod.rs:1549`) clears and
refills a `HashMap` from the surface ids twice per step (`mod.rs:585` and
`:1206`, once from each phase), although the surface order is fixed once
the environment is built. The coupled LU factorization and `m_scratch` are dead code
removed by SL-7.

**Change:**
- A persistent scratch `Vec<f64>` for the exterior surface temperatures,
  saved and restored with `copy_from_slice`.
- `prepare_inputs_inner` returns the latent map to `self.latent_buf`.
- `last_u` is sized at construction and updated with `copy_from`.
- An `output_into(x, u, y, du_scratch)` that computes `C·x` into `y`, `D·u`
  into a persistent scratch and adds them elementwise, the same operations in
  the same order as today, so the result is bitwise equal.
- The solver keeps the surface-id sequence it last mapped; the slot map is
  rebuilt only when the incoming sequence differs (compared id by id, no
  hashing).
- The two scalar solves take a `&mut SolveScratch` the `ThermalSolver`
  owns, sized to the state dimension at construction (`SolveScratch` is a
  new `hares-envelope` type: `n_x`, `b_u`, `rhs`, `gain` and `d_agg`; SL-7
  deletes the unused `SolverScratch`). Each computes `N·x` into `n_x` and
  `B_eff·u` into `b_u` with `mul_to` (the call the `*` operator makes),
  adds them elementwise into `rhs`, then subtracts `b_col[i] * u_i` element
  by element: the same operations in the same order as today, so the
  result is bitwise equal. `d_agg` is zeroed and accumulated in place. The
  identity-coupled solve copies the `b_eff` column into `gain` with
  `copy_from` and divides it where it divided the owned copy; the uncoupled
  solve, whose gain after SL-7 is the column itself, reads the column view
  directly. The autosizing and design-day callers
  (`stepping.rs:1445`) and the state-space unit tests pass a local scratch.
  The doc comment of `solve_ideal_capacity_for_target` then states what
  the zero-allocation test proves.
- These are the step path's allocation sites. If the zero-allocation test
  reports another, the entry stops, the site is added to this list with its
  location, and it is removed the same way (a buffer sized at construction)
  before the entry commits.

These are unconditional: the no-allocation rule for the hot loop is binding.
The profile share is recorded, not used as a gate.

The zero-allocation test also carries the factorization count that
`plan.md` §2.9 asks for: every factorization in `hares-envelope`
(`state_space.rs:202, 299, 311, 405, 1141, 1162, 1203, 1320`;
`thermal_solver/initialization.rs:255, 265`; after SL-7, `:707` is gone)
factors a dynamically sized nalgebra matrix and allocates its result, so a
step with zero allocations performs zero factorizations.

The stability-check comment in `StateSpaceModel`'s constructor
(`state_space.rs:266-275` at `7b7a6a7d`) still justifies the singular-A_c
exemption by a "CN implicit path" that SL-7 removed, and carries an em
dash. This row rewrites it to state the reason the exemption holds for the
explicit discrete step that remains (found by the SL-7 audit,
`verification/audit-SL-7-7b7a6a7d.md`).

The identity-coupled solve divides by `1 + d_i` per coupled state
(`state_space.rs:487-491` at `7b7a6a7d`) with no guard (red team,
`verification/redteam-OR-11-SL-7.md`, SL-7 minor). The divisor depends
only on the model, so this row checks it once at construction: every
`1 + d_i` finite and positive, else a typed `HaresError` naming the state;
nothing is added to the step.

**Acceptance test:**
- `coupled_solve_divisor_checked_at_construction`: a model whose coupling
  makes one `1 + d_i` zero, and one making it negative, each fail
  construction with the state named; every BESTEST and golden model
  constructs.
- `thermal_solver_step_allocation_free_after_first_step` replaces the
  11-per-step bound in `radiant_gain_weights_zero_alloc.rs`: over 100 steps
  after the first, each step running the dwelling's sequence
  (`prepare_inputs`, then `solve_ideal_capacity_for_target` for every zone
  with a sensible input, then `integrate`), the bracket around the whole
  sequence reads zero allocations, and therefore zero factorizations. It
  runs twice, once with couplings active (the identity-coupled solve) and
  once with none (the uncoupled solve), and also checks 100 calls of
  `ThermalSolver::resolve`. The file's header and assertion text are
  rewritten to state what the test proves, without the "ticket 040"
  references (`10-slop-cleanup.md` SL-2).
- `solar_slot_map_rebuilt_only_on_surface_change`: the map is rebuilt once
  for a fixed surface list over 100 steps, and again when the list changes.
- M-2's golden fixtures compare bitwise equal.

**Verification:** `cargo nextest run -p hares-envelope`; `frame_golden
compare-all`.

**Commit:** `perf: step the thermal solver without heap allocation`

**Dependencies:** M-1, M-2, SL-7 (`10-slop-cleanup.md`), PR-3 (satisfied
at `26626710`: `radiant_gain_weights_zero_alloc.rs:33-48` installs
`hares_types::alloc_count::CountingAllocator`, and the tests read
`thread_allocations()`).

**Measurement:** `prepare_inputs_inner` and `solve_ideal_capacity_for_target`
shares of `simulate()` before and after, per fixture. This entry removes
the ideal-capacity solve's allocations, not its compute (`context.md` item
5 measured it at 6.8% of `simulate()`). If, after this entry, the
ideal-capacity phase is at or above 5% of `simulate()` on
`consumer_shape_900s` (M-3's noise band), the row stops and a new entry
that removes the repeated resolve is added to this file and scheduled
directly after it; below 5%, the measurement is recorded as the reason no
further change is made (`dl-i08-architecture.engine_performance`).

**Stop-and-add record (2026-10-02, at `7b7a6a7d`).** The zero-allocation
test first reported 6 allocations per step from sites the list above did
not name, all in `build_input_vector` (`thermal_solver/mod.rs:1390-1391`,
`:1449-1453`, `:1464-1473`), all removed the same way (buffers
round-tripped at construction): `ext_surface_diag_buf.clone()` and the
`int_surface_diag_buf.clone()` and `window_solar_diag_buf.clone()` into
the `#[cfg(any(debug_assertions, feature = "observe_detailed"))]` fields
of `EnvelopeComponentGains` (now swapped through the struct replacement),
and the same freshly-constructed-struct dance handing
`infiltration_by_zone_buf` and `lwr_by_zone_buf` a capacity-0 `Vec` so
their `reserve` re-allocated every call (now the populated vec moves
into the new struct and the previous struct's vec returns to the
buffer slot with its capacity).

**Stop-rule record (2026-10-02).** After this entry the ideal-capacity
phase is 6% of `step_total` on `consumer_shape_900s` (at or above the 5%
band), so the rule fires: a new entry removing the repeated resolve is
scheduled directly after this row (row 21a in `00-schedule.md`).

## RT-3a: one shared-term resolve per step for the ideal-capacity solves

**Kind:** initiative work (the stop-rule entry RT-3 schedules; added
2026-10-02).

**Problem** (lines at the RT-3 head): `collect_and_solve`
(`crates/hares-core/src/actors/solver_feedback.rs:52-66`) calls
`solve_ideal_capacity_for_target` once per equipment with an ideal
target, and several equipment share a zone or share the step's solver
state: every call recomputes the terms that depend only on the step's
`x` and `u` (`N·x` into `n_x`, `B_eff·u` into `b_u`, and the elementwise
sum into `rhs`) and only the target-dependent tail differs. With
R conditioned zones and E equipment holding targets, the shared terms
are recomputed E times per step where once would do; the profiled
`ideal_capacity` share sits at 6% of `step_total` on
`consumer_shape_900s` (the stop-rule trigger).

**Change:** the shared-term prefix of the two scalar solves is computed
once per step and reused. `ThermalSolver` keeps the step's shared
scratch state (`n_x`, `b_u`, `rhs` filled) marked with the
`CoupledState` variant and the input hash-free identity it was built
from (the solver's own `last_u`/`x` sequence numbers, no hashing): the
first `solve_ideal_capacity_for_target` call of a step fills the
shared prefix; later calls in the same step with unchanged `x`/`u`
skip the recomputation and run only the target-dependent tail (the
coupling subtraction, `d_agg`, the gain division and the 1x1 products)
into a fresh per-call scratch region. Any change to `x` or `u` between
calls (the dwelling steps the solver between scheduler passes) invalidates
the prefix. The per-call operation order on the reused terms is
unchanged (the prefix is the same numbers the repeated calls computed);
the scalar solves' bitwise contract is unchanged. `collect_and_solve`
is the only hot caller; the autosizing and design-day callers pass
local scratches and keep their own sequencing.

**Acceptance test:** the zero-allocation test of RT-3 still reads zero
(bracket wraps prepare → per-zone solves → integrate); a new test
`shared_terms_computed_once_per_step` counts the prefix computations
(the scratch fill is instrumented with the profiling counter or a
test-visible counter) across a step with three equipment in one zone:
exactly one prefix computation, three target tails, and the returned
capacities equal the per-call computation's to bitwise equality (a
same-run A/B: the repeated-computation path and the shared path must
return identical f64s for every zone).

**Verification:** `cargo nextest run -p hares-envelope`;
`frame_golden compare-all` bitwise; the profiled `ideal_capacity`
share after, recorded in `02-measurement.md`.

**Commit:** `perf: compute the ideal-capacity solve's shared terms once per step`

**Dependencies:** RT-3, PR-3.

## RT-4: parsed inputs shared immutably, supplied as paths, content or parsed handles

**Kind:** initiative work (`context.md` items 3 and 6;
`dl-i08-architecture.inputs`).

**Problem:** parsing runs once per blueprint: `from_config` parses and calls
`from_parts` once (`crates/hares-core/src/dwelling/blueprint.rs:75-181`), and
`build()` never calls it again. What prevents reuse is that one blueprint
builds exactly one dwelling:
- `build(self)` consumes the blueprint (`blueprint.rs:236`);
- `build_from_blueprint` moves the rng out (`mod.rs:2174`), moves weather and
  schedule into `EnvironmentManager::new_with_resample`, which resamples them
  per build (`environment.rs:195-203`), and takes the PV panel map out of the
  defaults (`defaults.take_pv_panel_map()`, `mod.rs:2875`);
- `from_parts` writes the resolved site into `weather.meta` and
  `building.site` (`blueprint.rs:74-86`), so neither can be shared;
- the mutators (`remove_equipment`, `remove_equipment_by_end_use`,
  `add_equipment_spec`, `blueprint.rs:185-232`) take `&mut self`;
- inputs are paths only (`DwellingConfig`, `mod.rs:211-228`).

So every evaluation parses everything again: `Dwelling::from_config`
duplicates the parse (`mod.rs:1930-1954`, a copy of `blueprint.rs:75-181`),
and Python's `reset_with_seed` re-parses
(`crates/hares-python/src/py_dwelling.rs:1577-1591`, which
calls `Dwelling::from_config`).
In the calibration fit path the HPXML differs per candidate: of the
consumer's 11 free factors, 8 edit HPXML text (routes `hpxml` and
`hpxml_list`), 2 edit `envelope/Envelope Materials.csv` in a copied defaults
tree (`opaque_r_scale`, `thermal_mass_scale`) and 1 is an engine override
(`wh_ua_scale`) (`HARES_CALIBRATION_SYSTEM/calibration/registry.py`, `FREE`).
Only weather, schedule and all but one defaults file are shareable across
candidates, and each candidate is its own blueprint: sharing across
candidates needs a blueprint built from inputs parsed once elsewhere, which
no API offers.

**Change:**
- The types of `plan.md` §2.7 "Inputs": `InputSource`, `WeatherInput`,
  `DefaultsSource` and the parsed-input handles `ParsedHpxml`,
  `ParsedWeather`, `ParsedSchedule`, `ParsedDefaults`, each immutable and
  shared as `Arc`. Each handle is built from an `InputSource`
  (`ParsedWeather` from a `WeatherInput`, whose `format` is required for
  `Text` and `Bytes` and absent for a `Path`, which keeps today's
  detection). Defaults are a directory plus per-file replacements
  (`DefaultsSource.files`): `ParsedDefaults::with_file(relative_path,
  InputSource)` re-parses that one file and shares every other table; an
  unknown relative path is an error naming the known files. The default
  schedule profiles of `Default Schedule Parameters.csv`, which
  `11-other-defects.md` OD-17 made a checked `Result`, are one of those
  tables: the schedule generator and the schedule resolver read them from
  the `ParsedDefaults` handle instead of re-reading the file per build
  (`crates/hares-io/src/hpxml_schedule.rs:43`,
  `crates/hares-io/src/schedule_resolve.rs:475` at `21d90588`). Each handle
  exposes `record()`, which `12-inputs.md` IN-1 fills.
- `DwellingConfig` holds `hpxml: InputSource`, `schedule:
  Option<InputSource>` (`11-other-defects.md` OD-6 made it optional),
  `weather: WeatherInput` and `defaults: DefaultsSource` in place of the
  four path fields, plus `settings: DwellingSettings`.
- A blueprint can be built from handles parsed once and shared across
  blueprints: `ParsedInputs` (the four handles), `DwellingSettings`
  (`DwellingConfig` less its four input sources) and
  `DwellingBlueprintBuilder::new(inputs, settings)`, which parses nothing,
  exactly as `plan.md` §2.7 defines them.
  `DwellingBlueprintBuilder::from_config` is `ParsedInputs::parse` followed
  by `new`, and `DwellingBlueprint::from_config` is that followed by
  `freeze()`, so there is one construction path. `DefaultsSource.base` is `Option<PathBuf>` here, keeping today's
  optional `defaults_path`, until `09-numeric-fixes.md` B-10 makes it
  required. A calibration batch parses its weather, schedule and defaults
  once and builds each candidate's blueprint from its own `ParsedHpxml`
  plus those shared handles (`08-consumer.md` CN-10).
- Python: the handle classes of `plan.md` §2.7 (`hares.ParsedHpxml`,
  `hares.ParsedWeather`, `hares.ParsedSchedule` with `from_path(path)` and
  `from_text(text)`, `ParsedWeather` also taking `format=`, and
  `hares.ParsedDefaults(base, files=None)`), frozen pyclasses over the
  `Arc`. `Dwelling.from_hpxml` and `DwellingBlueprint.from_hpxml` accept a
  handle anywhere they accept a path or content, and share it; construction
  from handles releases the GIL like construction from paths
  (`05-python-surface.md` PY-3).
- `ParsedWeather::resampled(time_res) -> Arc<WeatherTimeSeries>`
  caches each resampled series on the handle (same map pattern as RT-2's
  tables), so blueprints at the same resolution share one copy.
- `DwellingBlueprint` holds the handles, the resolved `SiteLocation`, and the
  resampled weather and schedule. `from_parts` no longer writes
  `weather.meta` or `building.site`; every reader of the resolved latitude,
  longitude, elevation and UTC offset reads `site_location` (census:
  `grep -rnE 'meta\.(latitude|longitude|elevation_m|timezone_offset_h)|site\.(latitude_deg|longitude_deg|elevation_m|utc_offset_h)' crates/hares-core crates/hares-io`).
- `build(&self)` and `build_with_seed(&self, master_seed)`: the first clones
  the blueprint's rng, the second derives it from the seed; equipment specs
  are cloned; the PV panel map is read by reference; the environment holds
  `Arc`s to the resampled series.
- The mutators move to `DwellingBlueprintBuilder`, as `plan.md` §2.7
  lists them (`equipment_names`, `add_equipment_spec`, `remove_equipment`,
  `remove_equipment_by_end_use`, today's `blueprint.rs:185-232`), with
  `freeze(self) -> Arc<DwellingBlueprint>`; a
  `DwellingBlueprint` is immutable and shared as `Arc<DwellingBlueprint>`.
  Today's public blueprint fields (`config`, `schedule`, `defaults_path`,
  `equipment_specs`, `blueprint.rs:19-28`) become private, so the one
  test that edits them directly,
  `crates/hares-core/tests/equipment_id_regressions.rs:1037`
  (`blueprint_b.equipment_specs.rotate_left(1)`), is rewritten as
  `remove_equipment` of the first spec on a builder followed by
  `add_equipment_spec` of that spec, then `freeze()`. Python's blueprint
  wrapper (`py_blueprint.rs:45-107`) wraps the builder and freezes it on
  `build()`.
- `Dwelling::from_config` becomes `DwellingBlueprint::from_config(..)?.build()`,
  removing the duplicate parse path.
- The fleet's site-offset pre-pass (`04-fleet.md` FL-4: `read_site`,
  `read_weather_meta`, `site_offset`) takes the config's `InputSource` and
  `WeatherInput`, reading a `Path` as today and `Text` or `Bytes` from
  memory; its two FL-4 tests run unchanged.
- `PyDwelling` keeps the `Arc<DwellingBlueprint>` it was built from, and
  `reset_with_seed` calls `build_with_seed` on it.
- HPXML is still parsed once per candidate in the fit path; sharing it needs
  an edit language, which the decision defers. The shared handles are that
  seam.

**Acceptance test:**
- `blueprint_build_is_repeatable`: two dwellings built from one
  `Arc<DwellingBlueprint>` produce bitwise-equal `RunResult`s.
- `builds_share_parsed_inputs`: both dwellings hold the same `Arc`s for
  weather, schedule, defaults and HPXML (`Arc::ptr_eq`), so no input was
  parsed twice.
- `blueprints_from_different_hpxml_share_weather`: blueprints from
  `tests/fixtures/hpxml/ochre_samples/base.xml` and `base-pv.xml`, built
  with `DwellingBlueprintBuilder::new` over one `Arc<ParsedWeather>` of
  `data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw` and one
  `ParsedSchedule` of `data/examples/BEopt_example_schedule.csv`, hold the
  same weather `Arc` and the same resampled series `Arc`, and each frame
  equals, bitwise, the frame of a blueprint that parsed its own inputs.
- `content_source_run_equals_path_source_run`: HPXML, weather and schedule
  given as `InputSource::Text`, and defaults given as the repository directory with
  `envelope/Envelope Materials.csv` replaced by edited content, produce a
  frame bitwise equal to a run from paths with that file edited in a copied
  directory.
- `test_parsed_weather_shared_across_dwellings` (Python): the weather file
  copied to a temporary path is parsed with `ParsedWeather.from_path`, the
  copy is deleted, and two dwellings (`base.xml` and `base-pv.xml`) built
  from the handle run and equal, column by column, the same dwellings built
  from the repository path, so construction from a handle reads no file.
- `reset_with_seed_reuses_blueprint` (`hares-python` unit test): the
  blueprint `Arc` is the same before and after `reset_with_seed`, and the
  frame equals a fresh build with that seed.
- `unknown_defaults_file_is_an_error`.

**Verification:** `cargo nextest run -p hares-core blueprint::`; `cargo
nextest run -p hares-io`; `cargo nextest run -p hares-python`;
`uv run pytest tests/python`; `frame_golden compare-all`.

**Commit:** `perf: share parsed inputs immutably and build many dwellings from one blueprint`

**Dependencies:** M-1, M-2, OD-6 and OD-17 (`11-other-defects.md`),
`03-output-reporting.md` OR-6 (the tests compare `RunResult`s).

**Measurement:** time and bytes for the first build and for each further
build from one blueprint; the per-candidate cost in the fit path (HPXML parse
plus build from shared handles) against a full parse of every input,
both measured in the same run on `consumer_shape_900s`'s inputs; and M-3
item 5's three memory-growth readings (repeated construct-and-simulate,
long horizon, stepped episodes) repeated after this entry and recorded next
to M-3's baseline, with the Python surface of this commit (`duration` and
`reports=[]` in place of M-3's `duration_s` and `write_output=False`, as
`03-output-reporting.md` OR-6 states for its own repeat).

**State:** committed `2434deb8` (branch `row/r61-rt4`, cut from `9cc6d578`,
one code commit; 2026-10-12). The entry landed as designed, with the
account here.

The types (a new `crates/hares-io/src/inputs.rs`): `InputSource`
(`Path(PathBuf)`, `Text(Arc<str>)`, `Bytes(Arc<[u8]>)`, with `read()`,
`read_text()`, `path()` and a `display()` for error messages),
`WeatherInput` (`Path`, or `Text`/`Bytes` with the `format` required),
`DefaultsSource { base: Option<PathBuf>, files: BTreeMap<String,
InputSource> }`, and the parsed handles `ParsedHpxml` (over
`Arc<Building>`; `from_building` for the synthetic parts),
`ParsedWeather` (over `Arc<WeatherTimeSeries>` at the file's own step,
the `resampled(step_secs)` map cache RT-2's tables follow, `from_series`
for the synthetic parts), `ParsedSchedule` (over
`Arc<ScheduleTimeSeries>`, `parse(source, weather_meta)` for the CSV and
`generate(start, duration, interval, profiles)` for the generated one,
`record()` `None` when generated) and `ParsedDefaults` (the store plus
the profiles, `with_file(relative_path, source)` re-parsing that one
group over the store's `Arc`-shared tables). Every handle exposes
`record()`, the source it was parsed from: the identity the shared-parse
cache keys on, and the seed the digest record's later row fills.
`ParsedInputs` holds the four handles plus the effective IECC climate
zone (the HPXML's declared zone, else the weather station's, the
OS-HPXML default) resolved once at parse with its warning.
`ParsedInputs::parse` is the source path, `ParsedInputs::from_handles`
the assembly the Python constructors and the cache share.

`DwellingSettings` (a new `hares-core` type: the sim config, the
overrides, the building id, the warm-up duration and the patches) is
`DwellingConfig` less its four input sources, and `DwellingConfig` holds
`hpxml: InputSource`, `schedule: Option<InputSource>`,
`weather: WeatherInput`, `defaults: DefaultsSource`, `settings:
DwellingSettings`. `DwellingBlueprintBuilder::new(inputs, settings)`
parses nothing: it resolves the site (`resolve_site_location`), the
effective zone and the equipment once, and carries the mutators
(`equipment_names`, `remove_equipment` returning the removed spec,
`remove_equipment_by_end_use`, `add_equipment_spec`); `freeze()` returns
the `Arc<DwellingBlueprint>`; `from_config` is `ParsedInputs::parse`
followed by `new`, and `DwellingBlueprint::from_config` that followed by
`freeze()`, one construction path. The builder's `build(self)` is the
mutator tests' sugar for `freeze()` then `build()`. The blueprint holds
the handles, the resolved `SiteLocation`, the resampled weather and
schedule `Arc`s, the weather averages over the file's own step, the
equipment, the warnings and the rng; `build(&self)` clones the rng and
the equipment specs and shares the rest, `build_with_seed(&self, seed)`
derives the rng from the seed and carries the seed in the settings it
builds with. `Dwelling::from_config` is
`DwellingBlueprint::from_config(..)?.build()` (the duplicate parse path
in the old `from_config` is gone), and the `Dwelling` keeps the
`Arc<DwellingBlueprint>` it was built from, readable by
`Dwelling::blueprint()`.

The shared parse never mutates: `from_parts`' writes are gone. The
resolved site rides the blueprint and every reader reads it: the duct
DSE takes the resolved site (or the building's own, the direct resolvers'
form; the strict no-silent-default error naming `Site/Latitude` kept),
the solver build takes the resolved elevation (the zone capacitances'
ISA pressure), the autosize and PV readers read the blueprint's
`SiteLocation`, and the environment patches its own metadata copy (the
shared series are never touched) with the resolved values before the
offset warning and the hemisphere rule read it. The climate zone is no
longer written into the building either: the parse resolves the zone
(`climate_zone_resolution` replacing the mutating form, the same message)
and `resolve_equipment` takes it plus the resolved site as parameters.
The equipment resolve's water heater hierarchy reads the passed zone;
the fleet's zone validation is unchanged.

The environment holds `Arc<WeatherTimeSeries>` (the shared resampled
series; the manager never mutates it) and an owned schedule (the
assembly's unavailable-period zeroing and the schedule injection write
the build's own copy, cloned from the blueprint's shared series: the
same single copy cost the old move from the blueprint paid). The old
constructors keep their signatures and resample per call; the blueprint
build goes through `new_shared` with the pre-resampled `Arc` and the
resolved site. The weather resample happens once per unique weather file
and resolution, on the handle's cache; the schedule resample once per
blueprint at freeze.

The defaults store's sixteen tables are `Arc`-shared, so
`ParsedDefaults::with_file` clones the store (every table shared) and
re-parses only the replaced file's group; the loaders read through one
swap helper (`read_defaults_file` / `csv_reader`, the replacement's
content at the display path, the base's bytes otherwise) and
`DefaultsStore::load_with_overrides(base, files)` validates every
replacement against the tree's own file list (the unknown relative path
error naming the known files, the new `DefaultsError::UnknownDefaultsFile`).
The default schedule profiles are the schedule side's table:
`ParsedDefaults` parses them (the content twin
`parse_default_profiles_str` for a replacement) and the schedule
injection takes `Option<&DefaultProfiles>` from the handle instead of a
defaults directory it re-read per build. The generated schedule's order
is the config path's: the profiles load, the generation fails on the
profile it is missing, then the store; with a schedule source the store
loads first. The PV panel map is read by reference (`pv_panel_map()`
returning the shared `Arc`; the taking accessor is gone) and the
dwelling's own field is the shared `Arc`.

The fleet's many-home construction (the streaming `simulate` and
`SteppableFleet::from_configs`) builds every home through
`hares_io::ParsedInputsCache`: the weather and the defaults are cached
for the construction (keyed by the source identity: a path, or a
content source's allocation), the HPXML and the schedule documents for
one chunk (`evict_documents()` after each chunk's run, the streaming
memory bound kept), the schedule generated per home when a home carries
no schedule source (not cached, today's behavior). The construction's
one build is `fleet::build_dwelling`: the cache's inputs, the builder,
the frozen blueprint's build. The selection's catalogue probe builds
through the same cache, so a `Named` or `Pattern` selection's probe and
its home's run share one parse per unique file. The alignment pre-pass
is the entry's own form unchanged for paths (`read_site`,
`read_weather_meta`, `site_offset`) and reads content sources from
memory (`read_site_input`, `read_weather_meta_input`), its two FL-4
tests running unchanged.

Python (`crates/hares-python/src/py_inputs.rs`): the frozen pyclasses
`ParsedHpxml`, `ParsedWeather`, `ParsedSchedule` (each with
`from_path(path)` and `from_text(text)`; `ParsedWeather.from_text`
taking the required `format` string: `epw`, `psm3`, `res_stock_csv` or
`tmy3`) and `ParsedDefaults(base, files=None)` (the per-file
replacements a relative-path to content dict, `with_file` the one-file
replacement), each parse running under the released GIL.
`Dwelling.from_hpxml`, `DwellingBlueprint.from_hpxml` and the
`DwellingConfig` constructor accept a handle anywhere they accept a
path (`str | handle` unions, the stub and the changes section updated)
and share it; the construction's parse-and-assemble runs in the
detached section (the arguments' Send forms resolved under the GIL
first), so construction from handles releases the GIL like construction
from paths. `PyDwelling` keeps the `Arc<DwellingBlueprint>` it was
built from and `reset_with_seed` calls `build_with_seed` on it (no
re-parse; the GIL released); `Dwelling.blueprint()` exposes the frozen
blueprint (`DwellingBlueprintRef`, its `parsed_inputs()` the handles).
The blueprint wrapper wraps the builder and freezes it on `build()`.

RED first: the duplicate-parse proof's counter tests failed at the
parent behaviourally (three homes sharing one weather file read it
three times: the parse counters' delta 3, the assertion expecting 1),
and the entry's other acceptance tests failed as compile errors on the
missing API (`blueprint_sharing.rs`), the Python test on the import
(`ParsedWeather` not exported). All green at the commit:
`blueprint_build_is_repeatable` (two dwellings from one
`Arc<DwellingBlueprint>`, the frames bitwise equal),
`builds_share_parsed_inputs` (`Arc::ptr_eq` over the four handles),
`blueprints_from_different_hpxml_share_weather` (one weather `Arc`, one
resampled-series `Arc`, each run bitwise the own-inputs parse's),
`content_source_run_equals_path_source_run` (HPXML, weather and
schedule as text, the defaults tree with the envelope materials
replaced by content, the run bitwise the copied-tree run's),
`test_parsed_weather_shared_across_dwellings` (the weather file deleted
after the handle's parse; both dwellings run and equal column by column
the path-form dwellings), `reset_with_seed_reuses_blueprint` (the
blueprint `Arc` the same across the reset, the reset run's frame
bitwise a fresh build with that seed) and
`unknown_defaults_file_is_an_error`. The full block's gates green:
fmt clean; clippy `--workspace --all-targets --all-features -D warnings`
clean; nextest `--workspace --all-features -j 4` 8003/8003 (3 skipped:
the ignored kind-pin generator and the two hand-run measurement tests);
`pytest -m ''` 1724 passed, 6 xfailed, 0 failed, 0 errors; pyright 0;
the goldens bitwise (compare-all 8 identical, `full_year_900s_dst`
identical, the observe set's 8 identical). The measurement record is in
the log: the 1000-home construction 37.20-38.59 s before, 31.07-32.14 s
after; the parse counts 1 per unique input against the parent's one per
home; the fit path's per-candidate cost 3.20 ms from shared handles
against 11.01 ms with the full parse; M-3 item 5's three readings flat.
One test deleted with its premise: the basement-lighting copy-conflict
probe corrupted the blueprint's public schedule field, and the field is
private now (a parsed schedule is consistent by construction; the
copy-conflict branch the probe exercised is unreachable from parsed
inputs and is unpinned: no test covers it).

Independent review round at tip `c7098a38`, five findings fixed in the
same session (Rust tip `862660b3`, the Python surface `ca267df1`):

1. MAJOR, the fit-path measurement record: the re-measurement with the
   committed harness in the canonical release build reproduces the
   recorded numbers (3.58 ms per candidate from shared handles against
   11.89 ms with the full parse, 3.3x; five runs within 5% of their
   median, load average 8.25 to 8.44; the log's record carries the
   command and the load). The review round's own 153.7-209.3 ms with a
   1.2-1.3x ratio was the debug profile's run of the same test, not the
   release build the record is taken in.
2. MAJOR, the per-file replacement did not share: `with_file` re-read
   and re-parsed the entire tree from disk (a fresh
   `load_with_overrides`) where the docs claimed every untouched table
   was shared. Fixed: the handle's store tables are `Arc`s;
   `with_file` clones the store (every table shared), re-parses only
   the replaced file's group over the shared rest
   (`DefaultsStore::with_replaced_group`, the same `apply_override`
   the fresh parse applies for one file), shares the profiles unless
   the replacement names the profiles file, and bumps the defaults
   parse counter once per group re-parse. Pinned by
   `with_file_shares_the_untouched_tables`: the counter's delta 1,
   twelve untouched tables' `Arc::ptr_eq` with the source's, the
   profiles' too, the replaced row present and the source's absent.
   RED on the old code: the profiles re-parsed (counter delta 1) and
   every `ptr_eq` failed.
3. MINOR, the cache key was an allocation address:
   `SourceIdentity::Content(usize)` keyed the public parse cache on
   the content's allocation, so a dropped source and a different
   source at a reused address shared one entry and the second
   construction silently read the first source's parse. Fixed: content
   sources key by their content's 64-bit digest, computed when the
   identity is constructed; the sha256 form belongs to the input-digest
   row's machinery, not yet landed, so the cheap in-process hash is the
   key.
   Pinned by `a_dropped_source_and_a_reused_address_are_distinct_cache_keys`
   (RED on the old code: the allocator reused the address, both keys
   read `Content(137342585146448)`), by
   `equal_content_sources_share_one_cache_key` (equal contents share
   one parse, counter delta 1; RED on the old code: one parse per
   allocation), and by
   `different_content_sources_parse_once_each_through_one_cache`.
4. MINOR, the deleted probe's record overclaimed: the record said the
   injection's failure paths stay pinned; the copy-conflict branch is
   unreachable from parsed inputs and unpinned. The record says that.
5. MINOR, the Python schedule handles dropped the weather meta:
   `ParsedSchedule.from_path`/`from_text` parsed with
   `weather_meta=None`, so a schedule CSV whose timestamps carry no
   UTC offset failed at the handle where the config path (which passes
   the weather file's meta) succeeded. Fixed: both take
   `weather_meta=`, the parsed weather whose meta the offset-less
   timestamps resolve against, documented in the stub and docs. Pinned
   by `test_schedule_handle_takes_the_weather_meta_fallback`: the
   offset-less CSV fails without the meta naming the fallback, parses
   with it, and the handle-built run is bitwise the config path's run.

Gates at the fix tip: fmt clean; clippy `--workspace --all-targets
--all-features -D warnings` clean; nextest `--workspace
--all-features -j 4` 8007/8007 (3 skipped: the ignored kind-pin
generator and the two hand-run measurement tests); `pytest -m ''` 1725
passed, 6 xfailed, 0 failed, 0 errors; pyright 0; the goldens bitwise
(compare-all 8 identical, `full_year_900s_dst` identical, the observe
set's 8 identical).

## RT-5: per-run health state, thread safety and reproducibility

**Kind:** initiative work (`context.md` item 6;
`dl-i08-architecture.health`, `reproducibility`). Fixes V25, and V30 for
single dwellings (`04-fleet.md` FL-1 fixes it for fleets).

**Problem:** process-global statics make health counts and warn-once
logging depend on whatever else runs in the process: in `hares-fleet`'s
rayon pool today, and in the consumer once it runs threads (CN-10). The
`Send` side is already enforced for the default build:
`PyDwelling { dwelling: Mutex<Dwelling> }` is a `#[pyclass]`
(`py_dwelling.rs:616-617` at `afa9a071`) and `SteppableFleet` steps its
`Vec<Dwelling>` with `par_iter_mut` (`fleet.rs:415`, `:742`), both
requiring `Dwelling: Send`. (`Fleet::simulate`'s `par_iter`, `fleet.rs:346`,
builds and drops each dwelling inside its closure and needs no `Send`
dwelling.) Nothing asserts it under
every feature set, and nothing asserts `DwellingBlueprint: Send + Sync`,
which sharing one blueprint across threads (RT-4) needs. `Dwelling: Sync`
is not needed: each thread owns its dwelling. The `observe` `Cell`s in
`clock.rs:50-56` are `Send`.

The single-dwelling determinism check (`tests/regression/determinism.rs:17-80`)
sets `RAYON_NUM_THREADS` around a single-dwelling `engine.run`, which uses
no pool, and compares sorted metric totals with an epsilon.

**Evidence: the census** (at `afa9a071`). Command:
`grep -rnE '^\s*(pub(\([a-z]+\))?\s+)?static\s|thread_local!|OnceLock|LazyLock|\bOnce::new' crates/*/src --include=*.rs`,
excluding `&'static str` matches. The two rows OR-5 already deleted
(`dc033cee`) no longer print and are kept below for the check this entry
makes.

| Static | Location | Class | Disposition |
|---|---|---|---|
| `CATALOG` | `hares-equipment/src/battery/catalog.rs:263` | immutable data | unchanged |
| `CATALOG`, `ARCHETYPE_CATALOG` | `hares-equipment/src/ev/catalog.rs:210,655` | immutable data | unchanged |
| `NAMES` | `hares-types/src/equipment.rs:1614` | immutable data | unchanged |
| `CLIMATE_DATA` | `hares-physics/src/ashrae152.rs:265` | immutable, parsed once from an embedded CSV (`:267`) | unchanged |
| `FINDER` | `hares-io/src/site_location.rs:51` | immutable, embedded timezone data | unchanged |
| `CHECK_CATALOG_THERMAL`, `CHECK_PSYCHROMETER_INVARIANT` | `battery/catalog.rs:187`, `hares-physics/src/psychrometrics.rs:46` | one-time check of constants | unchanged |
| `COUNTER`, `EMPTY_TELEMETRY` | `hares-fleet/src/fleet.rs:1060`, `hares-equipment/src/pv/mod.rs:1804`, `hares-core/src/dwelling/mod.rs:9445` | test modules only | unchanged |
| `INSTALLED`, `THREAD_ALLOCATIONS` (`thread_local!`) | `hares-types/src/alloc_count.rs:28,35` | the counting allocator's process-wide "installed" flag and per-thread allocation counter (`07-profiler.md` PR-3), read only through `thread_allocations()` | unchanged; never feed `RunHealth` |
| `HOOK_INSTALLED`, `HOOK_STATE`, `PANIC_COUNT`, `PANIC_WITH_LOCATION_COUNT`, `DOUBLE_PANIC_PREVENTED_COUNT`, `PANIC_INFO` (`thread_local!`) | `hares-types/src/panic_hook.rs:38-42,67,71,75,101,128` | process diagnostics: the panic hook is process-global by construction | stay process-wide; never feed `RunHealth`; a panic in a run already surfaces as that run's error (`fleet.rs:348-393`) |
| `REJECTED_SIGNAL_COUNT`, `WARNED_REJECTED_SIGNAL` | `hares-equipment/src/lib.rs:35,38` | health event (rejected signal), `observe` only | already deleted by `03-output-reporting.md` OR-5 (counted into `RunHealth.rejected_control_signals`, each rejection a `Warning`, `plan.md` §2.3); this entry checks none remains |
| `ACTIONS_CLIPPED`, `ACTIONS_CLIPPED_WARNED` | `hares-python/src/py_gym.rs:208,212` | health event (clamp), `observe` only; still live at `21d90588` (`:277-284` increment them process-wide), and nothing calls `Dwelling::record_clamped_actions`, so `RunHealth.clamped_actions` is always 0 | deleted by `11-other-defects.md` OD-18 (every clipped value counted per dwelling into `RunHealth.clamped_actions`, in every build); this entry checks none remains |
| `NON_FINITE_REJECTIONS` | `hares-types/src/telemetry.rs:20` | NaN, which the health decision says fails the run | already deleted by OR-5 (the `Telemetry` latch of `plan.md` §2.3) |
| `RANGE_REJECTION_COUNT` | `hares-types/src/equipment.rs:1236` | counts rejections that already fail the run (`validate_core_contract` `Err` propagates, `mod.rs:6270-6273`) | deleted |
| `MODE_FLOW_INCONSISTENCY_COUNT` | `hares-types/src/mode_flow_guard.rs:16` | counts rejections that already fail the run (`equipment.rs:1605`) | deleted |
| `RANGE_WARNING_COUNT`, `RANGE_EVENTS` | `hares-types/src/equipment.rs:1239,1313` | plausibility warnings, `observe` only; one ring shared by every dwelling in the process | deleted; `validate_core_contract` pushes each warning into a `&mut Vec<Warning>` the caller supplies (`03-output-reporting.md` OR-11), and the dwelling records it in `RunResult.warnings` in every build |
| `GERSHGORIN_FALSE_POSITIVE_COUNT` | `hares-envelope/src/state_space.rs:181` | construction diagnostic, `observe_detailed` only | deleted; the model records the event on itself and the test reads that |
| `COUNTERS`, `DUTY_CYCLE_COMPONENT_EVENTS` | `hares-python/src/py_actor.rs:539-541` | written, never read; the `Vec` grows for the life of the process under `observe` | deleted with `record_dispatch` and `record_duty_cycle_component` |
| `WARNED_ACTOR_TELEMETRY_DOT`, `WARNED_ACTOR_TELEMETRY_BARE` | `hares-core/src/telemetry.rs:61,64` | warn-once | per-run flags on the object that owns the run's observation field list (the gym environment, `py_gym.rs:39`) |
| `ISLAND_UNSERVED_WARNED` | `hares-core/src/dwelling/mod.rs:145` | warn-once | per-dwelling flag; the first occurrence in a run goes through `03-output-reporting.md` OR-11's `WarningLog::push_warning` |
| `NO_AMPACITY_WARNED` | `hares-physics/src/pv_sizing.rs:232` | warn-once during construction | deleted; `size_pv_system` returns the missing-ampacity condition, and the blueprint's resolver warning collector (`03-output-reporting.md` OR-11) records a `Warning` for that dwelling |

No process-global counter feeds `RunHealth` (`03-output-reporting.md` OR-5).

One iteration over a hashed collection reaches model construction:
`solver_builder.rs` groups interior surfaces in a `HashMap<ZoneId, _>`
(`surfaces_by_zone`) and iterates it to build `interior_solar_zones` and
`interior_lwr_zones` (`crates/hares-core/src/dwelling/solver_builder.rs:1317,
1345`). Each zone's entry configures only that zone's own
surfaces and inputs, so the order does not reach the numbers; the
spawned-process test below, where every process draws its own hash seed,
is the test that would show otherwise.

**Change:**
1. Apply every disposition above that OR-5 has not already applied.
2. Compile-time assertions in `hares-core`: `Dwelling: Send`,
   `DwellingBlueprint: Send + Sync`, and `Send + Sync` for the four parsed
   input handles and, when RT-2 kept it, `ForcingTable`, checked in every
   feature set of the verification block.
3. `tests/regression/determinism.rs` is rewritten: each building runs twice
   concurrently on two threads from one blueprint and once alone, and the
   three frames and metrics are compared with `to_bits()`.
4. The consumer's switch from processes to threads is `08-consumer.md` CN-10.

Effect on fleets: plausibility warnings reach `RunResult.warnings` in every
build from this commit, and `04-fleet.md` FL-2 makes a home with any warning
`Degraded`, outside the aggregate. The fleet goldens must compare bitwise;
if a golden home now carries a warning, the row stops and the warning's
cause is fixed in the row (a wrong bound or a real model defect) or, if it
moves numbers, it becomes its own numeric entry under the schedule's
contingency rule.

**Acceptance test:**
- `dwelling_and_blueprint_thread_bounds` (compile-time) under every feature
  set.
- `concurrent_dwellings_match_sequential`: four dwellings from one shared
  blueprint of `tests/fixtures/hpxml/ochre_samples/base.xml`
  (`data/examples/BEopt_example_schedule.csv`,
  `data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw`, 24 h at 900 s), on
  four threads, give `RunResult`s bitwise equal to the same four run one at
  a time.
- `concurrent_runs_report_independent_counts`: two concurrent runs of the
  same inputs, one with a test actor that sends an out-of-bounds control
  signal every step, report `rejected_control_signals` of exactly the number
  sent and 0.
- `warn_once_is_per_run`: in `crates/hares-core/tests/grid_outage_islanding.rs`,
  two dwellings built one after the other with that file's
  `build_dwelling_with_base_load` and
  `add_battery_with_discharge_limit(&mut dwelling, 0.6, 0.5)` (a 1.5 kW base
  load against a 0.5 kW discharge limit), each stepped once, then islanded
  with `set_grid_voltage(0.0)` and stepped three more times, each carry
  exactly one island-unserved `Warning` in their `RunResult.warnings`.
- `whole_frame_identical_across_spawned_processes`: the test binary spawns
  itself 20 times on `tests/fixtures/hpxml/ochre_samples/base-foundation-basement-garage.xml`
  (below-grade) and `base-hvac-multiple.xml`, 48 h at 900 s; every child's
  frame hash is equal.
- The rewritten `regression_full_suite` determinism section passes.
- The fleet goldens compare bitwise (`frame_golden compare-all`).

**Verification:** `cargo nextest run --workspace` under every feature set of
the verification block; `cargo nextest run -p hares-core --test
regression`; `frame_golden compare-all`.

**Commit:** `fix: keep health counts and warn-once state per run and prove dwellings thread-safe`

**Dependencies:** RT-1, RT-4; `03-output-reporting.md` OR-5 (`RunHealth`),
OR-6 (`RunResult.warnings`); `04-fleet.md` FL-2; `11-other-defects.md`
OD-18 (the gym clipping statics).

**Measurement:** with the bench `benches/single_building.rs`, new group
`concurrent_dwellings` (one shared blueprint of `consumer_shape_900s`'s
inputs, 7 d, `BenchmarkId` per thread count 1, 8 and 26): dwellings per
second at each thread count; and peak RSS (`VmHWM`) for 26 concurrent
dwellings sharing one set of weather, schedule and defaults against 26
`frame_golden run` processes (`context.md`: about 4.7 GB for 26
processes). HPXML stays per candidate (RT-4), so the thread figure is
recorded as measured, not against the `context.md` estimate of about 1 GB
for fully shared inputs. Cross-platform agreement is `02-measurement.md`
M-4a and M-4b.

**State:** committed `9212f342` (2026-10-12, branch `row/r70-rt5`,
based on `e3bc4870`, one code commit, tree clean after). The row landed
as the user's work order distilled it: the health counts and the
warn-once state per run, the dwellings proven thread-safe, RED first on
the leak. The Measurement section's bench group (`concurrent_dwellings`
in `benches/single_building.rs`) did not run: the work order scoped the
row to the state model and the proofs, and the thread/dwellings-per-
second figure waits for the consumer rows that will size the pools
(CN-5, CN-10).

The per-run state model. A run is one dwelling's life: the dwelling is
built from the blueprint, stepped to its finish, and a reset
(`reset_with_seed`) rebuilds the dwelling from the kept blueprint, so
every per-run field on `Dwelling` is fresh for the next run by
construction. What changed is that the warn-once and health state that
lived in process statics moved onto that per-run surface:

- `ISLAND_UNSERVED_WARNED` (`dwelling/mod.rs`): the process-global
  `AtomicBool` is gone; the dwelling carries `island_unserved_warned`,
  and the first unserved islanded step in a run both logs at warn level
  and records a `Warning` (source `island`, the unserved kW in the
  message) through `WarningLog::push_from`; later occurrences log at
  debug level. Before the fix the warning was a log throttle only: it
  never reached `RunResult.warnings` at all, and a second run in the
  same process inherited the consumed gate.
- `WARNED_ACTOR_TELEMETRY_DOT` / `WARNED_ACTOR_TELEMETRY_BARE`
  (`telemetry.rs`): both statics are gone; the dispatch takes the run's
  own `ObservationWarnOnce` gates (two flags) as a `&mut` parameter. The
  gates live on the dwelling (the entry's "object that owns the run":
  the dwelling owns the run in Rust; the gym environment owns the field
  list in Python, and it threads the call through the dwelling's guard),
  read through the new `Dwelling::observation_vec(&mut self, fields)`,
  so a run's first dot-notation and bare-name fallback each warn once
  and a reset starts fresh. The dispatch method renamed
  (`to_observation_vec` to `observation_vec`, the gates parameter
  explicit); the one-argument form survives as the tests' trait.
- `RANGE_REJECTION_COUNT`, `RANGE_WARNING_COUNT`, `RANGE_EVENTS` and
  the `RangeEventRing`, `RangeViolationEvent`, `RangeViolationSeverity`,
  the four `range_*_counter`/`range_violation_events`/`clear_range_…`
  getters (`hares-types/src/equipment.rs`): all deleted.
  `validate_core_contract(desc, co, warnings)` now pushes every
  plausibility-bound crossing (cop past 20, setpoint outside
  [-50, 80] °C, thermal output outside ±100 kW) into the `&mut Vec<
  Warning>` the caller supplies, in every build, and the dwelling's
  `validate_equipment_contract(idx)` drains the reused
  `warning_scratch` into the run's warning log, step-stamped. The
  rejection sites keep their typed errors, counters gone.
- The cop == 0 warning died whole: the state is the documented-valid
  off-or-transition case (zero useful output, zero work input), and it
  reports `Some(0.0)` from every idling step, so recording it would put
  a warning in every idling home and, through FL-2, out of every fleet
  aggregate. This is the entry's contingency's wrong bound, fixed in the
  row before the goldens could catch it.
- `MODE_FLOW_INCONSISTENCY_COUNT` (`mode_flow_guard.rs`) and its getter:
  deleted; the rejections already fail the run.
- `GERSHGORIN_FALSE_POSITIVE_COUNT` (`state_space.rs`) and its getter
  and re-export: deleted; the model records the event on itself
  (`StateSpaceModel::gershgorin_false_positive()`), set when
  `from_continuous`'s eigenvalue fallback accepts what the Gershgorin
  bound flagged, and the acceptance test reads the model's own record.
- `COUNTERS`, `DUTY_CYCLE_COMPONENT_EVENTS` and the whole
  `dispatch_observer` module (`py_actor.rs`), written and never read:
  deleted with the two record calls.
- `NO_AMPACITY_WARNED` (`pv_sizing.rs`): deleted;
  `size_pv_system` returns the missing-ampacity condition
  (`PvSizingResult::main_panel_ampacity_missing`), and the
  dwelling-facing sizing path (`PyDwelling::estimate_pv_capacity`)
  records a `Warning` (source `pv_sizing`) on the dwelling's own log
  when the NEC backfeed check was skipped. The entry's "blueprint's
  resolver warning collector" does not exist for PV sizing (the sizing
  is caller-invoked, not part of blueprint resolution), so the record
  lands on the dwelling at the one dwelling-facing call.

`RunHealth` itself needed no change: the counters were already fields on
the dwelling, per run by construction. What the row adds is the proof
(`concurrent_runs_report_independent_counts`, below) that they do not
cross runs, and the leak proof for the warn-once state that did live in
statics.

The thread-safety surface. Compile-time: `dwelling_and_blueprint_thread_
bounds` (new `crates/hares-core/tests/thread_bounds.rs`) asserts
`Dwelling: Send`, `DwellingBlueprint: Send + Sync`, `Send + Sync` for
the four parsed input handles and `ForcingTable`, with const functions
so the bounds hold in every build. Run under the default, `dst`,
`observe` and `observe_detailed` feature sets and inside the
`--all-features` workspace pass. Behavioral: `concurrent_dwellings_
match_sequential` (new `crates/hares-core/tests/concurrency.rs`) builds
four dwellings from one shared blueprint of `base.xml`
(`BEopt_example_schedule.csv`, `USA_CO_Denver.Intl.AP.725650_TMY3.epw`,
24 h at 900 s), runs the first four one at a time, then steps four on
four threads (one dwelling per thread, the blueprint `Arc` the only
shared object), and compares every pair bitwise: frames per column
(f64 by `to_bits()`, utf8 by bytes), and metrics, health and warnings by
their postcard bytes (`RunMetrics` gained the `serde::Serialize` derive
for this; postcard writes every f64 as its IEEE-754 bytes, BTreeMaps
inside, so the encoding is deterministic). No shared-mutable-state
defect surfaced: the concurrent runs match the serial runs on the first
pass. `concurrent_runs_report_independent_counts` runs the same inputs
twice concurrently, one run carrying a test actor that sends an
out-of-bounds control signal (a sub-band thermostat deadband, rejected
at dispatch) every step: the actor run reports
`rejected_control_signals` of exactly the number it sent, the concurrent
plain run reports 0. `whole_frame_identical_across_spawned_processes`
(the same file) spawns the test binary twenty times (ten per fixture:
`base-foundation-basement-garage.xml`, `base-hvac-multiple.xml`, 48 h at
900 s) through an ignored child test, and every child's frame digest
(every buffer's raw bytes and validity masks through an unkeyed
`DefaultHasher`) is equal, so the solver_builder's hashed `HashMap`
iteration reaches no number. `tests/regression/determinism.rs` is
rewritten as the entry specifies: each building (the two BEopt and the
ResStock vendor buildings) runs once alone and twice concurrently on two
threads from one blueprint, the file report stripped (one shared
destination path would race the concurrent runs), and the three frames
and metrics compare bitwise; the `RAYON_NUM_THREADS` sweep and the
epsilon-compare are gone.

RED first: `warn_once_is_per_run` (in
`crates/hares-core/tests/grid_outage_islanding.rs`, the entry's shape)
failed at the parent with `left: 0, right: 1` (the first run's record
carried no island-unserved warning at all, the warn-once state a process
static gating a log line only), and passed after the fix with each of
the two runs' records carrying exactly one. The telemetry gates' unit
test and the range-warning record test were witnessed red by compile
errors at the missing API before their implementations.

Gates at `9212f342` (canonical env: `uv sync --locked --all-extras
--all-groups`, `uv run maturin develop`): fmt clean; clippy `--workspace
--all-targets --all-features -D warnings` clean; nextest `--workspace
--all-features -j 4` 8102/8102 (5 skipped: the ignored kind-pin
generator, the two hand-run measurement tests, the spawned-process
child, the delta fixture); nextest `-p hares-core --test regression`
20/20; `pytest -m ''` 1744 passed, 6 xfailed, 0 failed, 0 errors;
pyright 0; the goldens bitwise (compare-all 8 identical and
`full_year_900s_dst` identical, and the dst build's compare-all
identical: the nine manifests the base carries). The submodule note: the
worktree needed `git submodule update --init` for the vendor fixtures
the census-adjacent tests read; no tracked path moved.

## RT-6: resolved-parameter read-back

**Kind:** initiative work (`dl-i08-architecture.inputs`, `new_capability`).

**Problem:** a caller cannot see the parameters the physics actually uses
after defaults, autosizing and overrides are applied; the calibration
consumer reconstructs them from HPXML text (`08-consumer.md` CN-3).

**Change:** `ResolvedModel` exactly as `plan.md` §2.7 defines it (types,
fields and the required-parameter table live there only), less its
`clock` field, which `12-inputs.md` IN-2 adds in the next commit; computed at
assembly, after defaults, autosizing and overrides and before warm-up, and
returned by `Dwelling::resolved_model() -> &ResolvedModel`. Sources, all in
SI:
- each equipment's `resolved_parameters()` (the new `Equipment` trait
  method, no default implementation, returning `hares-types`'
  `ResolvedParameter`, `crates/hares-types/src/resolved.rs`), from its typed config after
  autosizing (`hares-equipment/src/hvac/heating_config.rs`,
  `cooling_config.rs`, `heat_pump_config.rs`, `water_heater/wh_config.rs`
  `ua_w_per_k`, and the PV, battery, EV and load configs); a config field
  still stored in kW, kWh or kVA at this row is converted to SI with `uom`
  when reported, and `09-numeric-fixes.md` B-14's equipment config commit,
  which moves those fields to SI, deletes the conversions. The read-back
  names are the resolved model's (`plan.md` §2.7) and map to these config
  fields: PV `nameplate_w` from `PvConfig.capacity_kw` (from B-14,
  `capacity_w`); battery `energy_capacity_j` from
  `BatteryConfig.capacity_kwh` (`capacity_j`) and `power_capacity_w` from
  `BatteryConfig.max_charge_kw` (`max_charge_w`); EV `battery_capacity_j`
  from `EvConfig.capacity_kwh` (`capacity_j`) and `max_charging_power_w`
  from `EvConfig.max_charging_power_kw` (`max_charging_power_w`);
- per HVAC equipment: the active heating and cooling setpoint sources,
  `ResolvedEquipment.heating_setpoint` and `cooling_setpoint`, copied from
  the `EquipmentSpec` fields `heating_setpoint_source` and
  `cooling_setpoint_source` that `12-inputs.md` IN-5 adds and fills by its
  rule. `SetpointSource` is a `hares-io` type
  (`crates/hares-io/src/setpoint_source.rs`, created by IN-5, `plan.md`
  §2.7); a generated schedule's constant column is
  `SetpointSource::GeneratedConstant` until `11-other-defects.md` OD-9
  deletes it;
- infiltration ACH50 and effective leakage area (`hares-io/src/hpxml/building.rs:240,252`);
- per window: U-factor and SHGC (`thermal_solver/config.rs:277-281`);
- per boundary: effective R and C from the assembled RC network, and each
  layer's R and C, as `ResolvedBoundary.layers`.
Python: `Dwelling.resolved_model()` returns the same values as a dict of
SI floats and strings.

**Acceptance test:**
- `resolved_model_available_before_simulating`: the value after build equals
  the value after a full run of `base.xml`.
- `resolved_model_reflects_hpxml_efficiency_edit`: changing `base.xml`'s
  furnace `AnnualHeatingEfficiency` from its value to 0.80 changes only that
  furnace's AFUE in the read-back, to 0.80.
- `resolved_model_reports_autosized_capacity`: a fixture without
  `HeatingCapacity` reports the autosized capacity the equipment uses.
- `every_equipment_class_reports_its_required_parameters`: for every class
  in `EquipmentRegistry::known_names()`, the built equipment's
  `resolved_parameters()` includes every name the §2.7 table requires for
  its end use.
- `resolved_layers_sum_to_the_boundary_resistance`: for every non-slab
  opaque boundary of `base.xml`, the layer resistances plus
  `films_m2k_w` equal `r_effective_m2k_w` to 1e-12 relative.

**Verification:** `cargo nextest run -p hares-core`; `uv run pytest
tests/python`.

**Commit:** `feat: read back the resolved parameters the physics uses`

**Dependencies:** RT-4; `12-inputs.md` IN-5 (the setpoint-source rule it
records).

**State:** committed `cb4c09d2` (2026-10-12, branch `row/r65-rt6`, based
on `228eecbe`). The read-back landed as the entry defines it.

The types: `hares-types`' `ResolvedParameter`
(`crates/hares-types/src/resolved.rs`: the name, the SI value, the unit)
and `hares-core`'s `ResolvedModel` (`crates/hares-core/src/resolved.rs`:
`ResolvedEquipment`, `ResolvedBoundary`, `ResolvedLayer`,
`ResolvedWindow`, `ResolvedInfiltration`, no `clock` — the next commit
adds it). The `Unit` enum gained the Joule, CubicMeter,
CubicMeterPerSecond and WattPerKelvin spellings the SI read-back spells.

The surface: `Dwelling::resolved_model() -> &ResolvedModel`, computed in
the build after defaults, autosizing and overrides and before warm-up,
never rebuilt afterwards; Python's `Dwelling.resolved_model()` returns
the same values as a dict of SI floats and strings (the setpoint sources
as their provenance labels), the stub documented.

The parameters, per equipment through the new `Equipment`
trait method `resolved_parameters` (no default implementation; the
delegation macro forwards for the wrapped units; the test-only stubs
report none): the HVAC classes read the staging config's first-stage
capacity and EIR (the coolers the rated SHR the latent degradation
reads; the heat-pump heater its backup capacity and EIR); the fuel
classes the AFUE; the resistance classes the efficiency — the reciprocal
of the EIR the physics stores — with the EIR reported beside it; the
element- and burner-heated tanks the capacity, setpoint, volume and UA
(the HPWH the rated COP the curve anchors to, now stored at init); the
indirect tank its tank parameters and the heat-exchanger UA (it has no
element: the boiler loop heats it); PV the nameplate (kW→W); the battery
the energy capacity (kWh→J), the charge×discharge product as the
round-trip efficiency and the max charge power (kW→W); the EV the rated
pack (kWh→J) and the clamped charge power (kW→W); the scheduled loads
the usage multiplier they applied, kept on the load now because the
applied scaling no longer shows it. The kW→J/W conversions run through
uom. The setpoint sources are copied from the spec fields the
setpoint-source resolution stamps, matched by instance name.

The envelope half is computed where the RC network assembles (the solver
bundle carries it in every build profile), from the same boundary inputs
and RC diagnostics the network was built from: per boundary the
diagnostic's total resistance (films included) and capacitance, and the
layers as built — the LUT layers, the material layers' framing-corrected
resistances, or the fallback's one lumped layer; none for slabs (the
F-factor method); per window the config's resolved effective SHGC and
U-factor in the sorted surface order; the infiltration from the parsed
building's ACH50, ELA (cm²→m²) and constant ACH.

Found on base.xml's same-zone surfaces: the auto-generated interior
partition wall and the furniture mass face no outside, and their RC
construction keeps the interior half of the stack as a dead-end fin.
Three conventions for the same-zone resistance are in play, and the
records state them truthfully: the read-back's effective R is the
network's actual discrete chain (the kept layers' series minus the
half-resistor the network drops, the precomputed path popping the
innermost kept layer's inner half and the material path skipping the
cut layer's outer half, plus the one interior film); an outside-facing
boundary's effective R is the full chain (the layers' series plus both
films, the diagnostic's total and the network's chain agreeing); and
the RC diagnostic's same-zone total is a different record with a
different convention (the full stack halved plus the one film, the
per-component heat-gain tracking's UA), which neither the kept layers'
series nor the network's chain equals in general. The physics is
unaffected (that number feeds only the per-component heat-gain
tracking); the read-back reports the network's chain.

RED first: the acceptance file failed to compile at the base with only
the tests applied (no `ResolvedModel`, no `ResolvedParameter`, no
`resolved_model()`, no `resolved_parameters()`), the every-class test on
the missing trait method, the Python test on AttributeError; 15
test-only `Equipment` stubs and 21 in-crate ones gained the method.
GREEN: `resolved_model_available_before_simulating`;
`resolved_model_reflects_hpxml_efficiency_edit` (only the furnace's AFUE
moves, to 0.80, every boundary, window and infiltration value equal);
`resolved_model_reports_autosized_capacity`;
`every_equipment_class_reports_its_required_parameters` (the table keyed
per class; the indirect tank's element-less water heating and the
scheduled-EV class's schedule physics documented in the test);
`resolved_layers_sum_to_the_boundary_resistance` (every non-slab opaque
boundary, 1e-12 relative); and the input-vs-default pin: base.xml's
declared AFUE and capacity read back at the input's values (the capacity
in SI watts, the Btu/h input converted) and its setpoints as the HPXML
source with the 24 hourly values, while the minimal fixture's furnace
(no HVACControl, no setpoint column) reads back the manual-thermostat
default profile.

Gates (canonical env `uv sync --locked --all-extras --all-groups` +
`uv run maturin develop`), on the staged change and again on the commit:
fmt clean; clippy `--workspace --all-targets --all-features --
-D warnings` clean; nextest `--workspace --all-features -j 4` 8041
passed, 3 skipped (the ignored kind-pin generator and the two hand-run
measurement tests); pytest `-m ''` 1730 passed, 6 xfailed, 0 failed,
0 errors; pyright 0; the goldens bitwise (compare-all 8,
`full_year_900s_dst` 1).

Independent review round at tip `801633e5`, five findings fixed on the
branch. (1) MAJOR, the same-zone fin disclosure's core claim was false,
proven by probe: the read-back's `r_effective` reported the kept
layers' full series plus the interior film (a third convention) while
the network's discrete chain drops one half-resistor (the precomputed
path pops the innermost kept layer's inner half; the material path
skips the cut layer's outer half), up to +67% overstatement (base.xml's
furniture mass: the read-back 1.645 against the network's 0.985). The
fix computes the read-back's `r_effective` for a same-zone finned
boundary from the assembled legs' path rules (the precomputed path
minus half the last kept layer's R, the material path minus half the
first kept layer's R), `ResolvedBoundary` carries `same_zone` so a
record's convention is readable, and the module's comment, the
acceptance test's doc and the paragraph above state all three
conventions truthfully: the outside-facing boundaries' full chain, the
same-zone fins' actual chain, the diagnostic's full-stack-halved total
named as a different record. The pins updated to the actual values:
`same_zone_fin_r_pins_the_network_chain` holds the furniture mass to
0.9850975292587776 and the partition wall to 0.5916125292587776 (the
network's chain sums), and the sum tests (the Rust acceptance file and
the Python surface's) apply the fin rule per boundary. RED: the
reworded sum tests and the pins fail at the old read-back (the
furniture 1.645, the wall 0.631); GREEN at the fix. (2) the step-use
pin: `step_telemetry_uses_the_read_back_furnace_values` runs the parity
furnace fixture and holds the step telemetry's `fuel_input_w` equal to
the read-back's rated capacity over AFUE, bit for bit, at the first
full-fire step (pinned at step 4, the duty exactly 1.0);
`network_conductance_matches_the_reported_r` holds the diagnostic's
per-boundary UA equal to the area over the read-back's effective R, bit
for bit, for one envelope case. (3) the every-class test's `_ => &[]`
fallthrough silently exempted unlisted classes: the table now carries a
row for every registered class (the event loads' power and duration,
the wet appliances' unit count, the generators' rated power, the
dehumidifier's removal, energy factor and load fraction, the
ventilation units' two fans and flow rate, the protocol bridge an
explicit empty row) and the test fails a class the table omits. (4) the
clippy gate string above gains the `--` separator before `-D warnings`.
(5) the read-back dict's flaky inequality observation:
`test_resolved_model_reads_identical_twice` builds the Python dict
twice from one dwelling and holds the reads identical, so a recurrence
fails in CI.

## RT-7: the per-step RL observation is built without allocation

**Kind:** initiative work (`dl-i08-architecture.rl_observation_allocation`:
per-step RL observation allocation is part of the runtime work).

**State:** committed `37237b24` (2026-10-12, branch `row/r71-rt7-rt8`,
based on `e3bc4870`): `ObservationPlan` (`crates/hares-core/src/observation.rs`,
new) resolves every field of the environment's field list once, at plan
build, to a typed source — the outdoor scalars, the total and reactive
powers, the zone brackets (the zone's id and its position in the
sorted-zone-id order both frozen), the equipment brackets, the flat
aliases (resolved to the equipment index at build), the time features
and the actor channels (dot and bare, the bare search's sorted-name
order frozen; the actors' published channels are their sealed
declarations, so plan time sees the key set any step would) — failing
at the build on an unknown field and naming it, the resolution order
and error messages the string walk's, so a field the plan rejects the
old walk rejects too. `Dwelling::observe_into(plan, out)` and the
telemetry executor behind `to_observation_vec` (now a thin wrapper over
the same plan) fill a caller-owned slice with no allocation, the values
and NaN handling `to_observation_vec` always reported (the
uninitialized row all NaN; the dwelling-direct read keys the zone
fields by id, the snapshot read by position). RED first: the new
hares-core tests compile-error at the parent on the missing surface,
and the allocation proof's before side measured — the read allocated 42
per call. GREEN: `observe_into_allocates_nothing_after_the_first_step`
(the counting allocator, 96 reads on base.xml of every field kind,
delta 0) and `observation_plan_matches_to_observation_vec` (the
dwelling-direct executor against the snapshot read on bits, NaN before
the first step and after one); every existing telemetry observation
test passes through the wrapper unchanged. Python: `ObservationPlan`,
`Dwelling.observation_plan(fields)`, `Dwelling.observe_into(plan, out)`
(any writable C-contiguous float64 buffer view); the gym environments
build one plan per dwelling at reset (lazily before one, so a step
before a reset builds it too) and `batch_step` now takes the plans and
the vector environment's own `(n_envs, n_fields)` buffer, filled in
place through its buffer protocol — no per-row lists, no Python object
per field; the row dicts no longer carry `obs`. The per-step `info`
keys are `&'static str` and the dict is built only for the keys set in
that step. A step whose observation read fails ends that environment
with its error (`terminated=True`, `info["error_msg"]`, its row NaN,
the flag kept): no substitute vector; the
`dwelling.observation().unwrap_or_default()` fallback deleted with the
method. `VecDwellingGymEnv(..., copy=True)` (the default) returns a
fresh copy of the buffer per step; `copy=False` returns the buffer
itself, documented as overwritten by the next step or reset. The
`_verify_observation_equivalence` check compared plan output with plan
output and is deleted with its flag and its three pinning tests; the
pure-Python path reads through the same plans, so the two paths agree
by construction (`rust_and_python_paths_give_identical_rewards` still
passes). New Python pins: `unknown_observation_field_fails_at_reset`
(both environments, naming the field),
`batch_step_returns_a_fresh_observation_by_default` (different objects,
the first unchanged, each bitwise the pure-Python path's),
`batch_step_copy_false_returns_the_shared_buffer` (`is` the buffer).
The hares-python unit test `observation_failure_terminates_the_environment`
faults the batch's one observation wrapper (`#[cfg(test)]` switch
naming the environment index) for environment 0 of two base.xml
dwellings: terminated, the error naming the environment, the row NaN,
environment 1 steps normally. Numbers in `02-measurement.md`: the
read's 42 allocations per environment per call at the base against 0;
the whole call's calling-thread count n=1 78 to 34, n=8 120 to 121
(the reads ride the workers); the batch loop's per-step wall (8 envs,
1 000 steps of 60 s) 0.942 ms at the base against 0.898 to 0.934 ms
copy=True and 0.881 ms copy=False — inside the host's noise, recorded
as no measurable change, the verdict the removed read. Gates: fmt
clean, clippy `--workspace --all-targets --all-features -D warnings`
clean, nextest `--workspace --all-features -j 4` 8102/8102 (5 skipped:
the ignored kind-pin generator and the hand-run measurement tests),
`pytest -m ''` 1745 passed + 6 xfailed, 0 failed, 0 errors, pyright 0,
the nine goldens bitwise (compare-all 8 identical, `full_year_900s_dst`
identical).

**Problem:** every RL step rebuilds the observation from strings. For each
environment and step:
- `Dwelling::telemetry()` builds a new `DwellingTelemetry`: it collects and
  sorts the zone ids, allocates a `String` per zone name (`format!` for
  every zone after the first), and allocates the temperature, residual and
  per-equipment vectors;
- `Telemetry::to_observation_vec` rebuilds two `HashMap<String, usize>`
  name indexes (`index_map`, which also lowercases every name), parses
  every requested field name (`bracket_name`) and allocates the output
  `Vec<f64>`;
- `observation_for_fields` allocates a `Vec<&str>` of the field names; the
  batch path builds a `HashMap<String, f64>` `info` with string keys and a
  `PyDict` per environment;
- `VecDwellingGymEnv.step` converts the returned lists into new numpy
  arrays per call.
The field list is fixed for the life of the environment, so all of the
name resolution is repeatable work. When the field observation fails at a
step, the batch path substitutes `dwelling.observation().unwrap_or_default()`,
a different vector (or an empty one), and reports only an
`observation_error` flag (`py_gym.rs:486-491`).

**Evidence:** `crates/hares-core/src/dwelling/mod.rs:4525-4551`
(`telemetry()`); `crates/hares-core/src/telemetry.rs:92-160`
(`to_observation_vec`), `:250-258` (`index_map`);
`crates/hares-python/src/py_gym.rs:34-41` (`observation_for_fields`),
`:470-513` (per-step `info`, observation and reward), `:486-491` (the
substituted observation), `:530-550` (per-row `PyDict`);
`python/ochre_next/rl/vec_env.py:226-240` (numpy conversion per step),
`:187` and `:243` (the Python observation path). After
`05-python-surface.md` PY-7 the Python files are
`python/hares/rl/vec_env.py` and `python/hares/rl/gym_env.py`.

**Change:**
- `ObservationPlan` (`hares-core`, `crates/hares-core/src/observation.rs`,
  new): built once from the environment's field list against a built
  dwelling, resolving every field to a typed source (outdoor temperature,
  zone index, equipment index, time feature) and failing at construction
  on an unknown field, naming it. `Dwelling::observe_into(&self, plan:
  &ObservationPlan, out: &mut [f64]) -> Result<()>` writes the observation
  into a caller-owned slice with no allocation; values and NaN handling are
  those of `to_observation_vec` today, which becomes a thin wrapper over
  the same plan for its remaining callers.
- The gym environment and `batch_step` build the plan at `reset()`.
  `batch_step` writes each environment's observation into one
  `(n_envs, n_fields)` `float64` buffer owned by the vector environment
  (`python/hares/rl/vec_env.py`), filled in place through its buffer
  protocol with no per-row lists and no Python object per field. The
  per-step `info` keeps its keys but they are `&'static str`, and the dict
  is built only for the keys set in that step.
- `VecDwellingGymEnv.step` returns a copy of that buffer by default: one
  contiguous `n_envs × n_fields` allocation per step, so an observation a
  caller keeps across steps is never overwritten. `VecDwellingGymEnv(...,
  copy=False)` returns the buffer itself, documented as overwritten by the
  next `step()` or `reset()`, for callers that consume each observation
  before stepping again (Gymnasium's `SyncVectorEnv(copy=...)` has the same
  contract).
- A step whose observation fails ends that environment with its error:
  `terminated=True`, `info["error_msg"]` holding the error, and its row of
  the buffer set to NaN. No substitute vector is produced; the
  `dwelling.observation().unwrap_or_default()` fallback is deleted.
- The pure-Python path uses the same plan through
  `Dwelling.observe_into(plan, out_row)`, so the two paths read the same
  values by construction; the `_verify_observation_equivalence` check
  compares plan output with plan output and is deleted with its flag.

**Acceptance test:**
- `observe_into_allocates_nothing_after_the_first_step` (`crates/hares-core`,
  `hares_types::alloc_count::CountingAllocator` installed): 96 steps of
  `observe_into` on `base.xml` with every field kind perform zero
  allocations.
- `observation_plan_matches_to_observation_vec`: for every field kind, the
  plan's values equal today's `to_observation_vec` on bits, including NaN
  before initialisation.
- `unknown_observation_field_fails_at_reset` (Python), naming the field.
- `batch_step_returns_a_fresh_observation_by_default` (Python, two `base.xml`
  environments): the arrays returned by two consecutive steps are different
  objects, the first is unchanged by the second step, and each equals the
  pure-Python path's observation on bits.
- `batch_step_copy_false_returns_the_shared_buffer` (Python): with
  `copy=False`, two consecutive steps return the same object (`is`).
- `observation_failure_terminates_the_environment` (`hares-python` unit
  test, calling `batch_step` directly on two `base.xml` dwellings): the
  batch path reads each environment's observation through one wrapper
  function in `py_gym.rs` around `Dwelling::observe_into`, and that wrapper
  carries a `#[cfg(test)]` fault switch naming the environment index to
  fail. `cfg(test)` is set only for the crate under test, so the switch
  lives in `hares-python`, not in `hares-core`'s `observe_into`. With the
  switch set for environment 0, that environment ends with
  `terminated=True`, an `error_msg` and a NaN row, while environment 1
  steps normally.
- `rust_and_python_paths_give_identical_rewards`
  (`05-python-surface.md` PY-5) still passes.

**Verification:** `cargo nextest run -p hares-core observation`;
`cargo nextest run -p hares-python`; `[pytest]`.

**Commit:** `perf: build RL observations into reused buffers from a frozen observation plan`

**Dependencies:** `05-python-surface.md` PY-5 (one reward contract),
PY-7 (the `python/hares/rl/` paths), PY-9; `07-profiler.md` PR-3 (the
allocator); RT-1.

**Measurement:** allocations per `batch_step` call and per-step wall time
for 8 environments over 1 000 steps, before and after, with `copy=True`
and `copy=False`, recorded in `02-measurement.md` (M-3 item 5's
stepped-episode loop is the harness).

## RT-8: RL reset restores the warmed-up state and reseeds without rebuilding

**Kind:** initiative work (requirement RL-3: reset restores a captured
initialised state and re-seeds without rebuilding;
`dl-i08-architecture.results`, `engine_performance`).

**State:** committed `fe79a83d` (2026-10-12, branch `row/r71-rt7-rt8`,
based on RT-7's `37237b24`): `Dwelling::reseed(master_seed)` re-derives
every random stream the dwelling holds — the dwelling's own stream 0,
every event-load equipment's (`Equipment::uses_rng_stream`) and every
built-in EV driver actor's — with the stream indices construction uses,
each at its start (word position zero). The components keep their own
nonces and take the dwelling's new generator key through `reseed(seed:
[u8; 32])` on the `Equipment` and `Actor` traits, whose defaults refuse
with a typed error the dwelling wraps naming the component, so a seeded
component that does not implement it fails instead of keeping its old
stream. An event load's start draws start over (its generator rebuilt
at word 0, the draw count zeroed; it draws nothing ahead of its stream
at construction, so there is nothing to redraw — stated at the impl);
an EV driver replaces its generator, keeps the nonce construction (or
the take-over) assigned. The dwelling records the seeded actors when a
roster change installs them (the auto-registered actors reporting a
stream through the new `rng_stream_state` read-back; a user actor's own
config-seeded generator is never the dwelling's to reseed), and
`Dwelling::seeded_streams()` reads every seeded component's (key,
nonce, word position) back, the dwelling's own stream first. Python:
`Dwelling.reseed(seed)`; the gym environments' reset is
`load_state(snapshot)` then `reseed(seed)` and no longer calls
`reset_with_seed`, which stays the explicit full rebuild; `docs/python.md`'s
RL section states the contract (the warmed-up state is the one the
environment's construction built; `reset(seed=...)` fixes the random
streams of the episode that follows; no re-construction, no second
warm-up). RED first: the tests are written against the missing surface
(the reseed methods compile-error at the parent, the Python
`Dwelling.reseed` an AttributeError) and the build counter's before
side is the reset path itself. GREEN:
`reseed_resets_every_stream` (hares-core, the cz5a EV fixture: a
dwelling stepped 96 times and reseeded with 7 holds, for every seeded
component, the (seed, stream, word position) the seed-7 fresh build
holds at construction — the comparison build warm-up-off so no stream
has advanced past its start — and a post-reseed step advances the
dwelling's stream, so the read-back is the components' own state, not
bookkeeping); `gym_reset_restores_snapshot_and_reseeds` (Python, the
cz5a fixture, warm-up on, 900 s, 1 d, an ASHP action: two `reset(seed=7)`
calls each followed by the same 96 actions give bitwise-equal
observations and rewards, and the restored first observation is the
same under seed 8);
`gym_reset_builds_nothing_and_runs_no_warmup` (hares-python unit test:
the reset sequence leaves the `#[cfg(test)]` build counter on
`PyDwelling` unchanged, incremented at `reset_with_seed`'s one
`build_with_seed` call (the counter lives in `hares-python`, which
`cfg(test)` does not set for `hares-core`); the blueprint `Arc` is the
same object before and after (`Arc::ptr_eq`); `health().warmup` equals
the snapshot's; `reset_with_seed(7)` increments the counter by one, so
the counter is shown to count). Numbers in `02-measurement.md`: the
reset's median wall against the construction's (20 each,
consumer_shape_900s's inputs at 1 d): the base's rebuild path 5.620 ms
against 39.6 ms (14.4%), the row's reseed path 0.047 ms against 39.0 ms
(0.1%), about 120x lower; requirement RL-3's well-under-10% holds with
orders of magnitude to spare. Gates: fmt clean, clippy
`--workspace --all-targets --all-features -D warnings` clean, nextest
`--workspace --all-features -j 4` 8102/8102 (5 skipped),
`pytest -m ''` 1745 passed + 6 xfailed, 0 failed, 0 errors, pyright 0,
the nine goldens bitwise (compare-all 8 identical,
`full_year_900s_dst` identical).

**Problem:** `DwellingGymEnv.reset` loads the post-warm-up snapshot and
then calls `reset_with_seed` (`python/ochre_next/rl/gym_env.py:546-548`;
the vector environment does the same, `vec_env.py:180-182`).
`reset_with_seed` rebuilds the dwelling from its config and runs warm-up
again (`crates/hares-python/src/py_dwelling.rs:1570-1584` at `21d90588`), so the loaded
state is discarded. After RT-4 the rebuild no longer re-parses, but
`build()` still warms up, so every episode repeats warm-up.

**Evidence:** the dwelling's random streams are derived from
`derive_dwelling_rng(master_seed, bldg_id)` (`crates/hares-core/src/rng.rs:31`)
with fixed stream indices: the dwelling's own stream 0, EV driver streams
from 1 and event-load streams from 1000 (`rng.rs:16-23`); event-load
equipment receives its derived seed at construction (`mod.rs:2562-2587`),
and actors built from equipment seeds draw from the dwelling's stream
(`mod.rs:8146-8147`).

**Change:**
- `Dwelling::reseed(&mut self, master_seed: u64) -> Result<()>` re-derives
  every random stream the dwelling holds from `master_seed`, with the stream
  indices construction uses, each at its start. The dwelling records at
  construction which equipment and actors received a derived seed and with
  which index; each of them is reseeded through a new method
  `reseed(&mut self, seed: [u8; 32]) -> Result<()>` on the `Equipment` and
  `Actor` traits, whose default returns `HaresError::InvariantViolation`
  naming the component, so a seeded component that does not implement it
  fails instead of keeping its old stream. A component that draws state for
  the run ahead from its stream at construction (for example a pre-drawn
  event schedule) redraws it in `reseed`.
- Python: `Dwelling.reseed(seed)`. The gym and vector environments reset by
  `load_state(snapshot)` and then `reseed(seed)`, and no longer call
  `reset_with_seed`, which stays the explicit full rebuild.
- `docs/python.md`'s RL section states the reset contract: the warmed-up
  state is the one built with the environment's construction seed, and
  `reset(seed=...)` fixes the random streams of the episode that follows.

**Acceptance test** (inputs `tests/fixtures/parity/cz5a_ev_charging/`
`building.xml`, `schedule.csv` and `weather.epw`, 900 s, 1 d, warm-up on):
- `reseed_resets_every_stream` (`crates/hares-core`): a dwelling stepped
  96 times and then reseeded with 7 holds, for every seeded component, the
  stream (seed, stream index, word position) a fresh build with seed 7 holds
  at construction.
- `gym_reset_restores_snapshot_and_reseeds` (Python): two `reset(seed=7)`
  calls, each followed by the same 96 actions, give bitwise-equal
  observations and rewards, and `reset(seed=8)` returns the same first
  observation as `reset(seed=7)` (the restored state).
- `gym_reset_builds_nothing_and_runs_no_warmup` (`hares-python` unit test):
  on a `PyDwelling` of these inputs, the reset sequence the environments
  use (`load_state(snapshot)`, then `reseed(7)`) leaves unchanged a
  `#[cfg(test)]` build counter on `PyDwelling`, incremented at its one call
  of `DwellingBlueprint::build_with_seed` (in `reset_with_seed`; the
  counter is in `hares-python` because `cfg(test)` is not set for
  `hares-core` when `hares-python`'s tests compile it); the dwelling's
  blueprint `Arc` is the same object before and after (`Arc::ptr_eq`); and
  `health().warmup` equals the snapshot's. Calling `reset_with_seed(7)`
  instead increments the counter by one, so the counter is shown to count.

**Verification:** `cargo nextest run -p hares-core -p hares-python`;
`uv run pytest tests/python -k gym`.

**Commit:** `perf: reset RL environments from the warmed-up state and reseed them without rebuilding`

**Dependencies:** RT-4 (`PyDwelling` keeps its blueprint), RT-7 (the same
files); `03-output-reporting.md` OR-10 (a checkpoint resumes the whole run);
`05-python-surface.md` PY-7.

**Measurement:** median reset time against median construction time (20
each, `consumer_shape_900s`'s inputs at 1 d), recorded in
`02-measurement.md`; requirement RL-3 asks for well under 10%.

## RT-9: the thermal solver's restore covers every piece of state a step reads, and its geometry cannot change after construction

**Kind:** found defect (red team of RT-3 and RT-3a,
`verification/redteam-RT-3-RT-3a-69950a98.md`, findings 1 to 3; all
three predate those commits).

**Problem** (at `69950a98`, `crates/hares-envelope/src/thermal_solver/`):
- `ThermalSnapshot` (`mod.rs:401-407`) carries `x`, `last_u`, the LWR and
  interior-surface histories, but not `last_coupling` or
  `last_coupled_state` (`mod.rs:128-129`), which
  `solve_ideal_capacity_for_target` reads (`stepping.rs:394-438`).
  `restore_state` (`mod.rs:1235-1350`) therefore leaves the pre-restore
  coupling terms in place. Its callers are the dwelling's checkpoint
  restores (`crates/hares-core/src/dwelling/mod.rs:5060`, `:5185`), so
  the first ideal-capacity solve of a resumed run can apply couplings
  from a different state than the one restored.
- `config_mut()` (`mod.rs:438-440`) hands out the whole
  `ThermalSolverConfig`, including the surface and zone lists RT-3's
  scratch buffers are sized from at construction. Its one production use
  mutates only `ventilation` (`dwelling/mod.rs:6536`).
- The zero-allocation test (`crates/hares-envelope/tests/radiant_gain_weights_zero_alloc.rs:238-243`)
  calls `solve_ideal_capacity_for_target` once per zone per step, so the
  several-solves-per-zone case RT-3a optimises is outside its bracket.

**Change:**
1. A test first: `resumed_run_equals_continuous_run`, a coupled
   multi-zone dwelling run for N steps continuously and, separately, run
   to step k, checkpointed, restored into a fresh dwelling and run to N;
   every output column bitwise equal. It fails at `69950a98` if the
   stale coupling reaches a result; if it passes, the step order is shown
   (file:line) to recompute the couplings before any solve reads them,
   and the snapshot still gains them so restore is complete by
   construction rather than by call order.
2. `ThermalSnapshot` gains `last_coupling` and `last_coupled_state`;
   `restore_state` validates and restores them; the dwelling checkpoint
   carries them (`CHECKPOINT_VERSION` bumped, the version mismatch error
   unchanged).
3. `config_mut()` is replaced by `ventilation_mut(&mut self) ->
   &mut MechanicalVentilationParams` (the one mutable part; the type is
   `MechanicalVentilationParams`, there is no `VentilationConfig`); every other config
   field is read-only after construction.
4. The zero-allocation test calls the ideal-capacity solve three times
   per conditioned zone per step, with and without couplings.

**Acceptance test:** `resumed_run_equals_continuous_run`;
`restore_state_restores_coupling_state` (a snapshot taken under one
coupling, restored onto a solver holding another, then one
`solve_ideal_capacity_for_target` equals the solve on the original
solver bitwise); the extended zero-allocation test.

**Verification:** the full block; `[golden]` bitwise unchanged (no golden
resumes from a checkpoint); the physics suites.

**Commit:** `fix: restore the thermal solver's coupling state with the rest of its state and keep its geometry fixed after construction`

**Dependencies:** RT-3, RT-3a. Runs on branch `i08-fixes` (a defect in
committed code).
