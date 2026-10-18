---
diagnosis_outcome: root_cause_found
complexity: moderate
failing_tests:
  - command: cargo test -p hares-core --test i06_ev_nightly_diag
    path: crates/hares-core/tests/i06_ev_nightly_diag.rs
    name: i06_equipment_id_probe
  - command: cargo test -p hares-core --test i06_ev_nightly_diag
    path: crates/hares-core/tests/i06_ev_nightly_diag.rs
    name: i06_ev_nightly_45day_diag
passing_commands:
  - command: cargo nextest run --workspace
  - command: cargo clippy --workspace -- -D warnings
  - command: cargo fmt --check --all
---

# RCA: EV under Nightly strategy stops charging after the first week (gh #26/#27)

## Symptom (restated)

On a 45-day run, an EV with a `Nightly{22:00–06:00, target 0.90}` strategy charges for
the first few nights, then never again: the reported SOC drains to near zero, daily
trips start being cancelled for range (`drive_cancelled` climbs to ~31), and the
range-anxiety backstop appears never to fire despite the "empty" pack.

## Root cause (confirmed by execution, not inference)

**No code path in a real dwelling build assigns unique `EquipmentId`s, so every
equipment instance is `EquipmentId(0)`, and every `EquipmentId`-keyed map collapses
to a single entry. The EV driver actor's observation channel reads the wrong
equipment's core output — the bug is in observation, not in charging.**

Direct evidence (probe test `i06_equipment_id_probe`, which builds the ticket's
exact dwelling — bldg0000007 + injected 75 kWh/11.5 kW EV — through the public
`Dwelling::from_config`):

```
12 equipment, 1 distinct EquipmentIds        ← all EquipmentId(0)
latest_env.equipment_core keys: [EquipmentId(0)]   ← one entry for the whole dwelling
```

Mechanics of the collapse:

- Each equipment constructor defaults its descriptor id to 0 when the config lacks
  `equipment_id`: `ev/mod.rs:207` (`config.get_f64(KEY_EQUIPMENT_ID).unwrap_or_default()`),
  same pattern in `scheduled_load.rs:178`, `event_load.rs:314,926`, `battery/mod.rs:429`,
  `hvac/helpers.rs:121`, `ventilation.rs`, `pv/mod.rs`, `protocol_bridge/mod.rs`.
- Nothing populates that key: `hares-io` sets `equipment_id: None` in every typed spec,
  `equipment_config_from_spec` (`dwelling/conversions.rs:665`) never adds it, and the
  registry (`hares-equipment/src/registry.rs`) passes the config through untouched.
- `equipment_id_by_name` (`dwelling/mod.rs:2252`) therefore maps *every* name to
  `EquipmentId(0)`, and the end-of-step snapshot (`dwelling/mod.rs:5856–5867`) inserts
  all 12 core outputs into `equipment_core` under one key — last equipment in the
  vector wins. In this fixture that is **MELs** (a ~90 W scheduled plug load).

## Causal chain to the ticket symptom

Reproduced with `i06_ev_nightly_45day_diag` (ticket setup verbatim: fixture +
injected EV, Nightly override, 45 days @ 900 s, seed 0). Step-level telemetry from
the run (`EV SOC (-)` is the equipment's own core-output SOC; `actor:…:soc` is the
driver channel):

1. **The equipment is healthy.** Whenever the driver's dispatch carries
   `SOCTarget(0.9)`, the EV charges to 0.9000 overnight (nights 1–6 and night 8:
   `0.7705 → 0.9000` at 22:00–22:45). The #25 hold/target contract works.
2. **The driver's observation is cross-wired to MELs.** `EvDriverActor::actual_soc`
   reads `equipment_core[0].state.soc` → MELs has no SOC → `None`. The day-start and
   arrival `reconcile_soc` calls therefore no-op, and `estimated_soc` (start 1.0)
   only ever decreases, ~0.131/day of driving. The `soc` telemetry channel silently
   falls back to this estimate (`mod.rs` `populate_telemetry`) — that channel is the
   ticket's "SOC 1.00 → 0.072" and "pack ends empty". The physical pack was at 0.90
   almost every morning of my run. `observed_charge_kw` reads MELs' flow instead —
   the phantom ~0.091 kW "charging" in `actor:…:charge_kw` matches the
   `MELs Electric Power (kW)` column step-for-step (0.091/0.091, 0.126/0.126).
3. **Spurious trip cancellations.** `observed_pack_kwh` falls back to
   `perceived × capacity`. Once the estimate decays to 0.084 (6.3 kWh < the
   9.8 kWh trip), every trip cancels (`drive_cancelled` 0→32 by day 45; ticket saw
   31). The vehicle stays home; `phase` never leaves `HomePluggedIn`.
4. **The backstop fires — and is moot.** Cancellation consumes `todays_event`
   (`= None`), which removes the urgency gate in `maybe_push_range_anxiety_override`,
   so from each 08:00 cancellation the override dispatches `SOCTarget(anxiety_band
   ≈ 0.22)` **every step, pre-empting the composer's in-window `SOCTarget(0.9)`**
   all night. The physical pack (0.90) already exceeds 0.22, so the equipment
   correctly charges nothing. "Needed_charge_hours ≈ 2 h" and
   `soc_gate_charging_allowed = 1` are computed from the phantom perceived SOC —
   both are telemetry faces of the same misbinding.
5. Stable pathological fixed point: pack full, driver convinced it's empty, mobility
   cancelled daily, zero EV load for 38 days. Days 6–8 trace the transition exactly
   (anxiety first active day 6; non-driving day 7 fully suppressed; driving day 8's
   urgency gate stands the override down at 22:00 so one last real charge lands;
   daily cancellations from day 9 lock it in).

## Strongest alternative explanation, and why it is ruled out

The strongest competing explanation is that the root cause is behavioral, in the
driver: *the range-anxiety override pre-empts the Nightly composer inside the
window (dispatching `SOCTarget(band ≈ 0.22)` instead of `SOCTarget(0.9)`), and
that pre-emption — not an observation failure — is what stops charging.* The
override code path is real and does fire every step from day 9 on; a reviewer
reading only the actor code could reasonably land there.

It is ruled out as the primary cause by two observations from the run:

1. **The override is only ever reached because observation failed first.**
   `below_anxiety_band` compares `perceived_soc()` (the un-reconciled estimate)
   against the band. With working observation, the day-start reconciliation
   (`maybe_roll_daily_event` → `reconcile_soc`, `ev_driver/mod.rs:593`) would
   reset the estimate to ~0.9 every morning and post-drive evenings would read
   ~0.77 — always far above the ~0.22 band — so the override would never fire and
   the composer would govern every night. The causal order in the trace is
   unambiguous: nights 1–6 charge correctly while the decaying estimate is still
   above the band; the override begins pre-empting only on day 7, after the
   estimate has decayed below it. The override is a downstream amplifier of the
   observation bug, not an independent cause.
2. **The override target is not what stops the energy.** When the override
   dispatches `SOCTarget(0.22)` against a physical pack at 0.90, the equipment
   correctly computes `soc >= soc_limit` → 0 kW (`ev/mod.rs:654`). The pack
   is full; there is nothing to charge. What is broken is that the *driver
   believes otherwise* and cancels real mobility on that belief — the
   cancellation warnings quote the phantom directly:
   `available_pack_kwh=6.2924` (= 0.0839 perceived × 75 kWh) against
   `requested_drive_kwh=10.11`, while the physical pack held ~67.5 kWh.

Also considered and ruled out:

- **"It is only a telemetry/reporting artifact."** Ruled out: the misbinding
  changes simulated behavior, not just reporting — 32 trips cancelled and zero EV
  charging load for 38 days versus the ~9.8 kWh/night the strategy should draw.
  The output *energy profile* is wrong, not merely the SOC column.
- **"The #25 hold/dispatch contract regressed"** (equipment latching a zero-power
  hold, or the window constraint misbehaving): ruled out by direct observation —
  the equipment charges to 0.9000 on every night a `SOCTarget(0.9)` arrives
  (nights 1–6 and night 8), and `apply_signal`'s SOCTarget arm clears latched
  holds (`ev/mod.rs:1754–1760`).
- **"The equipment loses the charged energy"** (SOC integration bug): ruled out —
  the equipment's own `EV SOC (-)` core-output column rises by exactly the
  charged energy each night (0.7705 → 0.9000 on night 8).
- **"Signals are dropped by the control pipeline"**: ruled out — the same
  dispatch path demonstrably reaches the equipment whenever the composer's target
  is dispatched (nights 1–6, 8).

## Why the safety nets didn't catch it

- `resolve_equipment_id` only warns when the *name* is absent — the name resolves
  (to the shared id 0), so the designed "operate without SOC feedback" warning never
  fires.
- The debug-only ">15% SOC divergence" warning in `ev_driver/mod.rs` `decide()` only
  runs when `actual_soc` is `Some` — blind to exactly this failure mode.
- The debug invariant "`equipment_core` missing entry" (`dwelling/mod.rs:4737`) is
  vacuous when all ids are equal: one entry satisfies every lookup.
- Unit tests hand-build `equipment_core` with unique ids
  (`equipment_ordering_tests.rs:269` assigns them explicitly); EV driver tests run
  120 one-minute steps — far too short for the belief drift to matter; the ResStock
  smoke fixtures ship no EV. The ticket called out the horizon gap explicitly.

## Class audit (same bug shape elsewhere — constitution: fix the class, not the instance)

Every `EquipmentId`-keyed structure in the dwelling is collapsed or vacuous today,
and every SOC-feedback actor is misbound, not just the EV driver:

- `BatteryManagementActor::read_soc` (`actors/bms.rs:149`) — identical
  `equipment_core[id].state.soc` read; a battery home's BMS reads the same
  last-equipment-wins slot.
- `prev_equipment_modes: HashMap<EquipmentId, _>` (`dwelling/mod.rs:1545`, rebuilt
  per step at 6014) — collapses to one entry; `equipment_mode_changed`
  (interest filtering, `dwelling/mod.rs:4602`) compares each actor's target against
  the *wrong* equipment's previous mode (spurious/missed actor wakeups).
- Checkpoint restore retain (`dwelling/mod.rs:7514`) and the active-id retain at
  5844 — no-ops under a single shared id.
- The duplicate check at assembly validates name uniqueness only; id collisions are
  never validated.
- Which equipment wins the single `equipment_core` slot depends on vector order
  (HPXML spec order) — a silent behavioral dependency on input ordering.

## Scope boundary of the diagnosis

Not the cause, verified and ruled out:

- Equipment-side hold latching / `SOCTarget` clearing (the #25 fix) — works; the EV
  charges whenever `SOCTarget(0.9)` actually arrives (nights 1–6, 8).
- `TimeWindowPref` window logic — sessions start exactly 22:00 while the composer is
  allowed to govern.
- The range-anxiety override "never firing" — it fires constantly from day 9; its
  minimal band target is simply moot against a full pack.
- Energy vanishing inside the equipment — `apply_soc_and_thermal` accounting is
  correct; equipment SOC rises by exactly the charged amount.

Secondary (contributing, not the root cause): the drive-cancel path clearing
`todays_event` removes the urgency gate, letting the band-target override pre-empt
the strategy's 0.9 target in-window. Correct behavior for a genuinely empty pack;
toxic only because the pack reading is phantom. The plan-fix step should decide
whether the override should also yield in-window when the strategy target exceeds
the band — flagged, not required for this fix.

## Fix direction (for plan-fix; not implemented here)

Assign unique `EquipmentId`s in the dwelling assembly path (or re-key
`equipment_core` by name — design choice for plan-fix), plus a debug-build
uniqueness invariant so a collision fails loudly. One assignment point fixes every
consumer above at once. Must be accompanied by dwelling-level (not unit-level)
regression coverage, since unit tests inject ids manually and never exercise the
real assembly.

## Reproduction

Command (this is also the `failing_tests` entry in the frontmatter):

```
cargo test -p hares-core --test i06_ev_nightly_diag
```

Verbatim result on the pre-fix tree (2 tests, both fail):

```
thread 'i06_equipment_id_probe' panicked:
  every equipment must have a distinct EquipmentId: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
  (probe printed: "12 equipment, 1 distinct EquipmentIds")
thread 'i06_ev_nightly_45day_diag' panicked:
  Nightly strategy must charge on most of 45 days, got 7 charge-days
  (and drive_cancelled = 32 by day 45)
```

The run's own warnings confirm the phantom pack arithmetic on every cancelled
day: `EV trip cancelled … requested_drive_kwh=10.11 available_pack_kwh=6.2924`
(actor `EvDriver:EV`) — 6.2924 = 0.0839 × 75 kWh is the un-reconciled driver
belief; the physical pack held ~67.5 kWh at 0.90 SOC the same morning.

## Scope note (what this phase changed)

No production code was modified (`git status --porcelain` shows only the new
test file). `crates/hares-core/tests/i06_ev_nightly_diag.rs` is a diagnostic
harness written to run the ticket scenario and capture the telemetry quoted
above; its two assertions encode the post-fix contract so the same command
serves the reproduce step's must_fail gate and the fix step's must_pass gate.
The reproduce step should formalize/rename it per the test-naming rules
(behavior names, not ticket ids) and decide what survives as the permanent
regression test. Nothing in the file masks the bug: both tests fail today.

- `i06_equipment_id_probe` — asserts distinct ids across the built dwelling.
- `i06_ev_nightly_45day_diag` — 45-day run of the ticket scenario; prints
  per-day and step-level EV telemetry, then asserts ≥30 charge-days and zero
  cancelled trips.

`cargo clippy -p hares-core --tests` and `cargo fmt --check -p hares-core` are
clean with this file included.

## Resolution (fix phase)

The root cause is confirmed fixed by execution, not inference: both
reproduction gates are green — `equipment_core_has_one_entry_per_equipment`
(12 equipment → 12 distinct core-output entries) and
`nightly_strategy_keeps_ev_charged_over_long_horizon` (45-day ticket scenario:
≥30 charge-days, zero cancelled trips) — on the combined tree with the
secondary override fix.

What landed, per the fix plan plus its corrections:

- **WI-1..3 (identity)**: unique ids injected at assembly through the config
  channel constructors already read; tri-state id reader consolidated in
  `hares-equipment/src/config.rs` with the six one-sided readers aligned onto
  it and the dehumidifier init site made preserve-when-absent;
  `equipment_id` reserved against all three override sources; required
  guard-checked `Equipment::set_equipment_id` with macro forwarding;
  entrance validation (id 0 and duplicates rejected) plus auto-assignment
  with a never-reused counter and a landed-write postcondition. Zero
  consumer-side edits — `equipment_core`, `equipment_id_by_name`,
  `prev_equipment_modes`, the EV driver and BMS observation channels are
  correct by construction, pinned by dwelling-level regression tests
  (including the BMS, the RCA's second misbound actor, observed through real
  assembly reading its own battery's SOC).
- **Correction**: the plan's "both entrances" claim missed
  `Dwelling::replace_equipment` (Python-reachable). The invariant is
  enforced at all three entrances; see `decision-log.md`.
- **WI-4 (override precedence)**: landed as planned — the override yields
  in-window to a strategy target at or above the anxiety band, path
  exclusivity preserved, stand-down and raise sides both pinned.
- **WI-5, superseded by operator instruction (no deferrals)**: the two
  pre-existing defects the plan would have tracked are fixed in-tree —
  checkpoint restore is identity-keyed (format v8; reorder between save and
  restore is a typed error, where the positional `zip` silently misloaded
  state), and port slots are rebuilt for post-construction equipment
  (monotonic pre-step rebuild; post-step adds with unsatisfiable or
  unknown-zone declarations are rejected loudly). The same-class audit also
  found and fixed the positional output column maps (mid-run removal
  silently misattributed output values); every fix is pinned by a test
  proven to fail with the fix neutered.

The RCA's secondary observation (the override pre-empting the composer
in-window) was confirmed downstream of the observation bug, exactly as
ruled out in the "strongest alternative explanation" section: with unique
ids the belief reconciles daily and the override never fires in the ticket
scenario; the precedence fix (WI-4) closes the residual case where a
strategy target below the band would still have been pre-empted
pointlessly.

Artifacts updated by this phase: `initiative.md` (Implementation Notes and
Known Limitations), `fix-plan.md` (correction section), `decision-log.md`.
